//! Java Edition Anvil format world saving.
//!
//! This module handles saving worlds in the Java Edition Anvil (.mca) format.

use super::common::{Chunk, ChunkToModify, Section};
use super::WorldEditor;
use crate::block_definitions::GRASS_BLOCK;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::Value;
use fnv::FnvHashMap;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::OnceLock;

/// Índice da seção mais baixa do mundo (Y=-64 do `common::MIN_Y`, >> 4) — o
/// `yPos` exigido na raiz de todo chunk pelo formato Java atual (1.18+).
const MIN_SECTION_Y: i32 = -4;

/// Cached base chunk sections (grass at Y=-62)
/// Computed once on first use and reused for all empty chunks
static BASE_CHUNK_SECTIONS: OnceLock<Vec<Section>> = OnceLock::new();

/// Get or create the cached base chunk sections
fn get_base_chunk_sections() -> &'static [Section] {
    BASE_CHUNK_SECTIONS.get_or_init(|| {
        let mut chunk = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, -62, z, GRASS_BLOCK);
            }
        }
        chunk.sections().collect()
    })
}

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

impl<'a> WorldEditor<'a> {
    /// Creates a region file for the given region coordinates.
    pub(super) fn create_region(&self, region_x: i32, region_z: i32) -> Region<File> {
        let region_dir = self.world_dir.join("region");
        let out_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));

        // Ensure region directory exists before creating region files
        std::fs::create_dir_all(&region_dir).expect("Failed to create region directory");

        const REGION_TEMPLATE: &[u8] = include_bytes!("../../assets/minecraft/region.template");

        let mut region_file: File = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&out_path)
            .expect("Failed to open region file");

        region_file
            .write_all(REGION_TEMPLATE)
            .expect("Could not write region template");

        Region::from_stream(region_file).expect("Failed to load region")
    }

    /// Helper function to create a base chunk with grass blocks at Y -62
    /// Uses cached sections for efficiency - only serialization happens per chunk
    pub(super) fn create_base_chunk(abs_chunk_x: i32, abs_chunk_z: i32) -> (Vec<u8>, bool) {
        // Use cached sections (computed once on first call)
        let sections = get_base_chunk_sections();

        // Prepare chunk data with cloned sections
        let chunk_data = Chunk {
            sections: sections.to_vec(),
            x_pos: abs_chunk_x,
            z_pos: abs_chunk_z,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };

        // Monta a raiz do chunk (formato Java atual)
        let level_data = build_chunk_root(&chunk_data);

        // Serializa o chunk
        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &level_data).unwrap();

        (ser_buffer, true)
    }

    // ========================================================================
    // ?? BESM-6: SAVE EPISODICO E METADADOS FINAIS
    // ========================================================================

    /// Salva a regi�o ativa no disco e n�o faz mais nada.
    /// Invocado pelo roteador Scanline.
    pub(super) fn save_java_region(&mut self, rx: i32, rz: i32) {
        // Se a regi�o n�o existir na mem�ria (totalmente vazia), apenas cria blocos base
        let region_exists = self.world.regions.contains_key(&(rx, rz));

        let mut ser_buffer = Vec::with_capacity(8192);
        let mut region = self.create_region(rx, rz);

        if region_exists {
            let region_to_modify = self.world.regions.get(&(rx, rz)).unwrap();

            // First pass: write all chunks that have content
            for (&(chunk_x, chunk_z), chunk_to_modify) in &region_to_modify.chunks {
                if !chunk_to_modify.sections.is_empty() || !chunk_to_modify.other.is_empty() {
                    let chunk = Chunk {
                        sections: chunk_to_modify.sections().collect(),
                        x_pos: chunk_x + (rx * 32),
                        z_pos: chunk_z + (rz * 32),
                        is_light_on: 0,
                        other: chunk_to_modify.other.clone(),
                    };

                    let level_data = build_chunk_root(&chunk);
                    ser_buffer.clear();
                    fastnbt::to_writer(&mut ser_buffer, &level_data).unwrap();
                    region
                        .write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)
                        .unwrap();
                }
            }
        }

        // Second pass: ensure all chunks exist (fill with base layer if not)
        for chunk_x in 0..32 {
            for chunk_z in 0..32 {
                let abs_chunk_x = chunk_x + (rx * 32);
                let abs_chunk_z = chunk_z + (rz * 32);

                let chunk_exists = if region_exists {
                    self.world
                        .regions
                        .get(&(rx, rz))
                        .unwrap()
                        .chunks
                        .contains_key(&(chunk_x, chunk_z))
                } else {
                    false
                };

                if !chunk_exists {
                    let (base_buffer, _) = Self::create_base_chunk(abs_chunk_x, abs_chunk_z);
                    region
                        .write_chunk(chunk_x as usize, chunk_z as usize, &base_buffer)
                        .unwrap();
                }
            }
        }
    }

    /// Relê uma região já gravada (`r.<rx>.<rz>.mca`) para dentro do Core Cache,
    /// bloco a bloco (paleta + índices empacotados do formato Java atual),
    /// preservando propriedades de blockstate e os demais campos do chunk
    /// (`block_entities`, `entities`...). Usado pela segunda passada do Halo
    /// (`WorldEditor::flush_pending_halo`): operações que vazaram para uma
    /// região já selada são aplicadas sobre o conteúdo REAL dela e a região é
    /// regravada — nenhum bloco se perde na borda de região.
    ///
    /// Chunks que contêm só a camada-base (grama em Y=-62 escrita por
    /// `create_base_chunk`) são recriados iguais no `save` seguinte; chunks
    /// ausentes/ilegíveis são pulados (e recriados como base).
    pub(super) fn load_java_region_from_disk(
        &mut self,
        region_x: i32,
        region_z: i32,
    ) -> Result<usize, String> {
        let region_path = self
            .world_dir
            .join("region")
            .join(format!("r.{}.{}.mca", region_x, region_z));
        let file = File::open(&region_path)
            .map_err(|e| format!("não foi possível reabrir {}: {e}", region_path.display()))?;
        let mut region = Region::from_stream(file)
            .map_err(|e| format!("região ilegível {}: {e}", region_path.display()))?;

        const ROOT_KEYS: &[&str] = &[
            "DataVersion",
            "xPos",
            "zPos",
            "yPos",
            "Status",
            "isLightOn",
            "sections",
        ];

        let mut chunks_loaded = 0usize;
        for chunk_x in 0..32usize {
            for chunk_z in 0..32usize {
                let Ok(Some(raw)) = region.read_chunk(chunk_x, chunk_z) else {
                    continue;
                };
                if raw.is_empty() {
                    continue;
                }
                let Ok(Value::Compound(root)) = fastnbt::from_bytes::<Value>(&raw) else {
                    continue;
                };

                let region_to_modify = self.world.get_or_create_region(region_x, region_z);
                let chunk = region_to_modify.get_or_create_chunk(chunk_x as i32, chunk_z as i32);

                for (key, value) in &root {
                    if !ROOT_KEYS.contains(&key.as_str()) {
                        chunk.other.insert(key.clone(), value.clone());
                    }
                }

                let Some(Value::List(sections)) = root.get("sections") else {
                    chunks_loaded += 1;
                    continue;
                };
                for section in sections {
                    let Value::Compound(section) = section else {
                        continue;
                    };
                    let section_y = match section.get("Y") {
                        Some(Value::Byte(y)) => *y,
                        Some(Value::Int(y)) => *y as i8,
                        _ => continue,
                    };
                    let Some(Value::Compound(block_states)) = section.get("block_states") else {
                        continue;
                    };
                    let Some(Value::List(palette)) = block_states.get("palette") else {
                        continue;
                    };
                    let palette: Vec<(Option<crate::block_definitions::Block>, Option<Value>)> =
                        palette
                            .iter()
                            .map(|item| {
                                let Value::Compound(item) = item else {
                                    return (None, None);
                                };
                                let block = match item.get("Name") {
                                    Some(Value::String(name)) => {
                                        crate::block_definitions::Block::from_name(name)
                                    }
                                    _ => None,
                                };
                                let props = item.get("Properties").cloned();
                                (block, props)
                            })
                            .collect();
                    if palette.is_empty() {
                        continue;
                    }

                    let indices: Vec<usize> = match block_states.get("data") {
                        Some(Value::LongArray(data)) if palette.len() > 1 => {
                            let mut bits = 4usize;
                            while (1usize << bits) < palette.len() {
                                bits += 1;
                            }
                            let per_long = 64 / bits;
                            let mask = (1u64 << bits) - 1;
                            let mut out = Vec::with_capacity(4096);
                            'outer: for long in data.iter() {
                                let long = *long as u64;
                                for i in 0..per_long {
                                    out.push(((long >> (i * bits)) & mask) as usize);
                                    if out.len() == 4096 {
                                        break 'outer;
                                    }
                                }
                            }
                            out.resize(4096, 0);
                            out
                        }
                        _ => vec![0usize; 4096],
                    };

                    let target = chunk.sections.entry(section_y).or_default();
                    for (index, &pal_idx) in indices.iter().enumerate() {
                        let Some((Some(block), props)) = palette.get(pal_idx) else {
                            continue;
                        };
                        if *block == crate::block_definitions::AIR {
                            continue;
                        }
                        target.storage.set(index, *block);
                        if let Some(props) = props {
                            target.properties.insert(index, props.clone());
                        }
                    }
                }
                chunks_loaded += 1;
            }
        }
        Ok(chunks_loaded)
    }

    /// Executado apenas no FIM da esteira de produ��o para gravar as coordenadas e finalizar.
    pub(super) fn save_java(&mut self) {
        println!("{} Saving world metadata...", "[7/7]".bold());
        emit_gui_progress_update(95.0, "Saving world metadata...");

        if let Err(e) = self.save_metadata() {
            eprintln!("Failed to save world metadata: {}", e);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, "Failed to save world metadata.");
        }

        emit_gui_progress_update(100.0, "Java World generation complete.");
    }
}

/// Helper function to get entity coordinates
#[inline]
#[allow(dead_code)]
fn get_entity_coords(entity: &HashMap<String, Value>) -> Option<(i32, i32, i32)> {
    if let Some(Value::List(pos)) = entity.get("Pos") {
        if pos.len() == 3 {
            if let (Some(x), Some(y), Some(z)) = (
                value_to_i32(&pos[0]),
                value_to_i32(&pos[1]),
                value_to_i32(&pos[2]),
            ) {
                return Some((x, y, z));
            }
        }
    }

    let (Some(x), Some(y), Some(z)) = (
        entity.get("x").and_then(value_to_i32),
        entity.get("y").and_then(value_to_i32),
        entity.get("z").and_then(value_to_i32),
    ) else {
        return None;
    };

    Some((x, y, z))
}

/// Builds the root chunk NBT (Java Edition, post-1.18 "current" format).
///
/// 🚨 CORREÇÃO CRÍTICA: antes, esta função (então chamada
/// `create_level_wrapper`) envelopava todo o chunk num compound `"Level"` —
/// o formato PRÉ-1.18 do Minecraft. `assets/minecraft/level.dat` (o
/// template usado por este motor) declara `DataVersion 4189` (Minecraft
/// 1.21.4), mas todo chunk escrito usava a estrutura antiga: sem "Level" o
/// formato atual não tem, e sem `DataVersion`/`Status`/`yPos` soltos na
/// raiz do chunk — os 4 sempre obrigatórios desde 1.18. Confirmado contra
/// o próprio tipo `CurrentJavaChunk` da crate `fastanvil` (a mesma que
/// este projeto usa): ele exige esses campos na raiz, não dentro de um
/// wrapper "Level" (que a crate reserva para seu módulo `pre18`,
/// deserialização de mundos ANTIGOS). Sem esta correção, nenhum mundo
/// gerado por este motor carregava corretamente num cliente Minecraft
/// real 1.18+: o cliente lê um DataVersion moderno, espera a estrutura
/// moderna, não encontra `sections`/`Status` na raiz (estavam dentro de
/// "Level"), e trata o chunk como ausente ou corrompido — descartando ou
/// regenerando todo o conteúdo gerado. `block_entities` (já com o nome
/// correto do formato moderno, vindo de `chunk.other`) sofria o mesmo
/// aninhamento incorreto.
#[inline]
fn build_chunk_root(chunk: &Chunk) -> HashMap<String, Value> {
    let mut level_map = HashMap::from([
        ("DataVersion".to_string(), Value::Int(4189)),
        ("xPos".to_string(), Value::Int(chunk.x_pos)),
        ("zPos".to_string(), Value::Int(chunk.z_pos)),
        ("yPos".to_string(), Value::Int(MIN_SECTION_Y)),
        (
            "Status".to_string(),
            Value::String("minecraft:full".to_string()),
        ),
        (
            "isLightOn".to_string(),
            Value::Byte(i8::try_from(chunk.is_light_on).unwrap()),
        ),
        (
            "sections".to_string(),
            Value::List(
                chunk
                    .sections
                    .iter()
                    .map(|section| {
                        let mut block_states = HashMap::from([(
                            "palette".to_string(),
                            Value::List(
                                section
                                    .block_states
                                    .palette
                                    .iter()
                                    .map(|item| {
                                        let mut palette_item = HashMap::from([(
                                            "Name".to_string(),
                                            Value::String(item.name.clone()),
                                        )]);
                                        if let Some(props) = &item.properties {
                                            palette_item
                                                .insert("Properties".to_string(), props.clone());
                                        }
                                        Value::Compound(palette_item)
                                    })
                                    .collect(),
                            ),
                        )]);

                        if let Some(data) = &section.block_states.data {
                            if !data.is_empty() {
                                block_states
                                    .insert("data".to_string(), Value::LongArray(data.to_owned()));
                            }
                        }

                        Value::Compound(HashMap::from([
                            ("Y".to_string(), Value::Byte(section.y)),
                            ("block_states".to_string(), Value::Compound(block_states)),
                        ]))
                    })
                    .collect(),
            ),
        ),
    ]);

    for (key, value) in &chunk.other {
        level_map.insert(key.clone(), value.clone());
    }

    level_map
}

#[allow(dead_code)]
fn merge_compound_list(chunk: &mut Chunk, chunk_to_modify: &ChunkToModify, key: &str) {
    if let Some(existing_entities) = chunk.other.get_mut(key) {
        if let Some(new_entities) = chunk_to_modify.other.get(key) {
            if let (Value::List(existing), Value::List(new)) = (existing_entities, new_entities) {
                existing.retain(|e| {
                    if let Value::Compound(map) = e {
                        if let Some((x, y, z)) = get_entity_coords(map) {
                            return !new.iter().any(|new_e| {
                                if let Value::Compound(new_map) = new_e {
                                    get_entity_coords(new_map) == Some((x, y, z))
                                } else {
                                    false
                                }
                            });
                        }
                    }
                    true
                });
                existing.extend(new.clone());
            }
        }
    } else if let Some(new_entities) = chunk_to_modify.other.get(key) {
        chunk.other.insert(key.to_string(), new_entities.clone());
    }
}

#[allow(dead_code)]
fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(i32::from(*v)),
        Value::Short(v) => Some(i32::from(*v)),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        Value::Float(v) => Some(*v as i32),
        Value::Double(v) => Some(*v as i32),
        _ => None,
    }
}
