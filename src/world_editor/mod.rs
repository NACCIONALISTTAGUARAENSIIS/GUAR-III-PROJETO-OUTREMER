//! World editor module for generating Minecraft worlds.
//!
//! This module provides the `WorldEditor` struct which handles block placement
//! and world saving in both Java Edition (Anvil) and Bedrock Edition (.mcworld) formats.
//!
//! # Module Structure
//!
//! - `common` - Shared data structures for world modification
//! - `java` - Java Edition Anvil format saving
//! - `bedrock` - Bedrock Edition .mcworld format saving (behind `bedrock` feature)

mod common;
mod java;

#[cfg(feature = "bedrock")]
pub mod bedrock;

// Re-export common types used internally
pub(crate) use common::WorldToModify;

#[cfg(feature = "bedrock")]
pub(crate) use bedrock::{BedrockSaveError, BedrockWriter};

use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastnbt::{IntArray, Value};
use serde::Serialize;
use std::collections::{hash_map::Entry, HashMap};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

/// World format to generate
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum WorldFormat {
    /// Java Edition Anvil format (.mca region files)
    JavaAnvil,
    /// Bedrock Edition .mcworld format
    BedrockMcWorld,
}

/// Metadata saved with the world
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldMetadata {
    pub min_mc_x: i32,
    pub max_mc_x: i32,
    pub min_mc_z: i32,
    pub max_mc_z: i32,

    pub min_geo_lat: f64,
    pub max_geo_lat: f64,
    pub min_geo_lon: f64,
    pub max_geo_lon: f64,
}

/// The main world editor struct for placing blocks and saving worlds.
///
/// ?? BESM-6 OUT-OF-CORE ARCHITECTURE ??
/// The WorldEditor now acts as a spatial router. It maintains a "Core Cache" (the active region)
/// and a "Halo Cache" (orphaned blocks belonging to adjacent regions).
pub struct WorldEditor<'a> {
    world_dir: PathBuf,
    world: WorldToModify, // O CORE CACHE (Apenas a regi�o ativa reside aqui)
    
    // ?? O Roteador Espacial
    active_region_x: i32,
    active_region_z: i32,
    halo_cache: HashMap<(i32, i32), HashMap<(i32, i32, i32), Block>>, // (RegX, RegZ) -> (X, Y, Z) -> Block

    xzbbox: &'a XZBBox,
    llbbox: LLBBox,
    ground: Option<Arc<Ground>>,
    format: WorldFormat,
    
    #[cfg(feature = "bedrock")]
    bedrock_level_name: Option<String>,
    #[cfg(feature = "bedrock")]
    bedrock_spawn_point: Option<(i32, i32)>,
}

impl<'a> WorldEditor<'a> {
    /// Creates a new WorldEditor with Java Anvil format (default).
    #[allow(dead_code)]
    pub fn new(world_dir: PathBuf, xzbbox: &'a XZBBox, llbbox: LLBBox) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            active_region_x: 0, // Ser� dinamicamente setado pelo loop principal
            active_region_z: 0,
            halo_cache: HashMap::new(),
            xzbbox,
            llbbox,
            ground: None,
            format: WorldFormat::JavaAnvil,
            #[cfg(feature = "bedrock")]
            bedrock_level_name: None,
            #[cfg(feature = "bedrock")]
            bedrock_spawn_point: None,
        }
    }

    /// Creates a new WorldEditor with a specific format and optional level name.
    #[allow(dead_code)]
    pub fn new_with_format_and_name(
        world_dir: PathBuf,
        xzbbox: &'a XZBBox,
        llbbox: LLBBox,
        format: WorldFormat,
        #[cfg_attr(not(feature = "bedrock"), allow(unused_variables))] bedrock_level_name: Option<
            String,
        >,
        #[cfg_attr(not(feature = "bedrock"), allow(unused_variables))] bedrock_spawn_point: Option<
            (i32, i32),
        >,
    ) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            active_region_x: 0,
            active_region_z: 0,
            halo_cache: HashMap::new(),
            xzbbox,
            llbbox,
            ground: None,
            format,
            #[cfg(feature = "bedrock")]
            bedrock_level_name,
            #[cfg(feature = "bedrock")]
            bedrock_spawn_point,
        }
    }

    // ========================================================================
    // ?? BESM-6 CONTROLE DO MOTOR DE VARREDURA (SCANLINE LIFECYCLE)
    // ========================================================================

    /// Move o foco do motor para uma nova Regi�o. 
    /// Isso � chamado pelo `data_processing.rs` antes de voxelizar as geometrias.
    pub fn set_active_region(&mut self, rx: i32, rz: i32) {
        self.active_region_x = rx;
        self.active_region_z = rz;
    }

    /// Injeta os blocos "�rf�os" que vazaram das regi�es vizinhas anteriores 
    /// para dentro do Core Cache atual, para que sejam selados no momento correto.
    pub fn load_halo_to_core(&mut self) {
        let region_key = (self.active_region_x, self.active_region_z);
        
        if let Some(blocks) = self.halo_cache.remove(&region_key) {
            let count = blocks.len();
            for ((x, y, z), block) in blocks {
                self.world.set_block(x, y, z, block);
            }
            if count > 0 {
                println!("[HALO] Despejados {} blocos vazados na regi�o ({}, {})", count, self.active_region_x, self.active_region_z);
            }
        }
    }

    /// Comprime o Core Cache (WorldToModify) com Zlib, escreve o `.mca` no disco,
    /// e em seguida ANIQUILA a RAM do Core para garantir seguran�a de 24GB constante.
    pub fn flush_active_region(&mut self) {
        // Compacta as se��es para poupar banda de mem�ria antes da grava��o
        self.world.compact_sections();
        
        // ?? No Java Edition, uma regi�o equivale a um arquivo Anvil exato.
        match self.format {
            WorldFormat::JavaAnvil => self.save_java_region(self.active_region_x, self.active_region_z),
            WorldFormat::BedrockMcWorld => {
                // Para Bedrock, o fluxo out-of-core � mais complexo devido ao LevelDB.
                // Por ora, acumularemos as muta��es no driver apropriado que faremos no bedrock.rs
            }
        }

        // ?? EXPURGO ABSOLUTO O(1): Mata a RAM do Core
        self.world = WorldToModify::default(); 
    }

    /// Retorna o tamanho atual do Halo Cache para estat�sticas do Terminal HUD
    pub fn get_halo_metrics(&self) -> (usize, usize) {
        let active_buckets = self.halo_cache.len();
        let total_blocks = self.halo_cache.values().map(|bucket| bucket.len()).sum();
        (active_buckets, total_blocks)
    }

    // ========================================================================

    pub fn set_ground(&mut self, ground: Arc<Ground>) {
        self.ground = Some(ground);
    }

    pub fn get_ground(&self) -> Option<&Ground> {
        self.ground.as_deref()
    }

    #[allow(dead_code)]
    pub fn format(&self) -> WorldFormat {
        self.format
    }

    #[inline(always)]
    pub fn get_absolute_y(&self, x: i32, y_offset: i32, z: i32) -> i32 {
        if let Some(ground) = &self.ground {
            ground.level(XZPoint::new(
                x - self.xzbbox.min_x(),
                z - self.xzbbox.min_z(),
            )) + y_offset
        } else {
            y_offset 
        }
    }

    #[inline(always)]
    pub fn get_ground_level(&self, x: i32, z: i32) -> i32 {
        if let Some(ground) = &self.ground {
            ground.level(XZPoint::new(
                x - self.xzbbox.min_x(),
                z - self.xzbbox.min_z(),
            ))
        } else {
            0 
        }
    }

    pub fn get_min_coords(&self) -> (i32, i32) {
        (self.xzbbox.min_x(), self.xzbbox.min_z())
    }

    pub fn get_max_coords(&self) -> (i32, i32) {
        (self.xzbbox.max_x(), self.xzbbox.max_z())
    }

    #[allow(unused)]
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> bool {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.block_at_absolute(x, absolute_y, z)
    }

    // ========================================================================
    // ?? BESM-6 ROUTING ENGINE (Roteador Espacial Voxel)
    // ========================================================================

    /// Sets a block of the specified type at the given coordinates with absolute Y value.
    /// Injeta a l�gica de Roteamento (Core vs Halo).
    #[inline]
    pub fn set_block_absolute(
        &mut self,
        block: Block,
        x: i32,
        absolute_y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let rx = x >> 9; // Bitwise Shift instant�neo (512 = 2^9)
        let rz = z >> 9;

        // Se o bloco pertencer � regi�o ativamente processada, ele vai pro Core.
        if rx == self.active_region_x && rz == self.active_region_z {
            let should_insert = if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
                if let Some(whitelist) = override_whitelist {
                    whitelist.iter().any(|b| b.id() == existing_block.id())
                } else if let Some(blacklist) = override_blacklist {
                    !blacklist.iter().any(|b| b.id() == existing_block.id())
                } else {
                    false
                }
            } else {
                true
            };

            if should_insert {
                self.world.set_block(x, absolute_y, z, block);
            }
        } 
        // Se o bloco pertencer a uma regi�o vizinha (vazamento), ele vai pro Halo Cache.
        else {
            // Nota: No Halo, ignoramos whitelists complexos por performance,
            // assumindo que a borda do pr�dio tem prioridade de escrita. O check definitivo
            // ocorre quando o Halo � despejado no Core.
            self.halo_cache
                .entry((rx, rz))
                .or_default()
                .insert((x, absolute_y, z), block);
        }
    }

    /// Fast-path para blocos sem blacklist/whitelist
    #[inline]
    pub fn set_block_if_absent_absolute(&mut self, block: Block, x: i32, absolute_y: i32, z: i32) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            self.world.set_block_if_absent(x, absolute_y, z, block);
        } else {
            // Se j� n�o existe no Halo, insere.
            let bucket = self.halo_cache.entry((rx, rz)).or_default();
            bucket.entry((x, absolute_y, z)).or_insert(block);
        }
    }

    #[inline]
    pub fn set_block(
        &mut self,
        block: Block,
        x: i32,
        y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_block_absolute(block, x, absolute_y, z, override_whitelist, override_blacklist);
    }

    #[inline]
    pub fn set_block_with_properties_absolute(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        absolute_y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            let should_insert = if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
                if let Some(whitelist) = override_whitelist {
                    whitelist.iter().any(|b| b.id() == existing_block.id())
                } else if let Some(blacklist) = override_blacklist {
                    !blacklist.iter().any(|b| b.id() == existing_block.id())
                } else {
                    false
                }
            } else {
                true
            };

            if should_insert {
                self.world.set_block_with_properties(x, absolute_y, z, block_with_props);
            }
        } else {
            // Blocos com propriedades vazando para o Halo (Armazenamos o bloco base por ora)
            // Futuro: Expans�o do Halo para suportar properties
            self.halo_cache
                .entry((rx, rz))
                .or_default()
                .insert((x, absolute_y, z), block_with_props.block);
        }
    }

    // ========================================================================
    // DEMAIS FUNÇÕES DO EDITOR (Mantidas, mas roteadas)
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn fill_blocks(
        &mut self,
        block: Block,
        x1: i32,
        y1: i32,
        z1: i32,
        x2: i32,
        y2: i32,
        z2: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for y_offset in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block(
                        block,
                        x,
                        y_offset,
                        z,
                        override_whitelist,
                        override_blacklist,
                    );
                }
            }
        }
    }

    #[inline]
    pub fn check_for_block(&self, x: i32, y: i32, z: i32, whitelist: Option<&[Block]>) -> bool {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.check_for_block_absolute(x, absolute_y, z, whitelist, None)
    }

    #[allow(unused)]
    pub fn check_for_block_absolute(
        &self,
        x: i32,
        absolute_y: i32,
        z: i32,
        whitelist: Option<&[Block]>,
        blacklist: Option<&[Block]>,
    ) -> bool {
        let rx = x >> 9;
        let rz = z >> 9;

        // Se est� na regi�o ativa, checa no Core
        if rx == self.active_region_x && rz == self.active_region_z {
            if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
                if let Some(whitelist) = whitelist {
                    return whitelist.iter().any(|b| b.id() == existing_block.id());
                }
                if let Some(blacklist) = blacklist {
                    return blacklist.iter().any(|b| b.id() == existing_block.id());
                }
                return whitelist.is_none() && blacklist.is_none();
            }
            return false;
        } 
        
        // Se a regi�o vazou, checa no Halo
        if let Some(bucket) = self.halo_cache.get(&(rx, rz)) {
            if let Some(existing_block) = bucket.get(&(x, absolute_y, z)) {
                if let Some(whitelist) = whitelist {
                    return whitelist.iter().any(|b| b.id() == existing_block.id());
                }
                if let Some(blacklist) = blacklist {
                    return blacklist.iter().any(|b| b.id() == existing_block.id());
                }
                return whitelist.is_none() && blacklist.is_none();
            }
        }

        false
    }

    #[allow(unused)]
    pub fn block_at_absolute(&self, x: i32, absolute_y: i32, z: i32) -> bool {
        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            self.world.get_block(x, absolute_y, z).is_some()
        } else {
            self.halo_cache.get(&(rx, rz)).map_or(false, |b| b.contains_key(&(x, absolute_y, z)))
        }
    }

    #[inline]
    pub fn fill_column_absolute(
        &mut self,
        block: Block,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
        skip_existing: bool,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            self.world.fill_column(x, z, y_min, y_max, block, skip_existing);
        } else {
            // Emula o fill block by block no Halo
            for y in y_min..=y_max {
                if skip_existing {
                    self.set_block_if_absent_absolute(block, x, y, z);
                } else {
                    self.set_block_absolute(block, x, y, z, None, None);
                }
            }
        }
    }

    // ========================================================================
    // SISTEMA LEGADO DE SAVE FINAL E METADADOS
    // ========================================================================

    pub fn save(&mut self) {
        println!(
            "{} Formatando Metadados de Encerramento: {}",
            "[INFO]".cyan().bold(),
            match self.format {
                WorldFormat::JavaAnvil => "Java Edition (Anvil)",
                WorldFormat::BedrockMcWorld => "Bedrock Edition (.mcworld)",
            }
        );

        // Compact sections before saving final bits
        self.world.compact_sections();

        match self.format {
            WorldFormat::JavaAnvil => self.save_java(), // Esta fun��o agora s� fechar� as �ltimas regi�es abertas, se houver
            WorldFormat::BedrockMcWorld => self.save_bedrock(),
        }
    }

    #[allow(unreachable_code)]
    fn save_bedrock(&mut self) {
        println!("{} Saving Bedrock world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving Bedrock world...");

        #[cfg(feature = "bedrock")]
        {
            if let Err(error) = self.save_bedrock_internal() {
                eprintln!("Failed to save Bedrock world: {error}");
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Error,
                    &format!("Failed to save Bedrock world: {error}"),
                );
            }
        }

        #[cfg(not(feature = "bedrock"))]
        {
            eprintln!(
                "Bedrock output requested but the 'bedrock' feature is not enabled at build time."
            );
            #[cfg(feature = "gui")]
            send_log(
                LogLevel::Error,
                "Bedrock output requested but the 'bedrock' feature is not enabled at build time.",
            );
        }
    }

    #[cfg(feature = "bedrock")]
    fn save_bedrock_internal(&mut self) -> Result<(), BedrockSaveError> {
        let level_name = self.bedrock_level_name.clone().unwrap_or_else(|| {
            self.world_dir
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Arnis World")
                .to_string()
        });

        BedrockWriter::new(
            self.world_dir.clone(),
            level_name,
            self.bedrock_spawn_point,
            self.ground.clone(),
        )
        .write_world(&self.world, self.xzbbox, &self.llbbox)
    }

    pub(crate) fn save_metadata(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let metadata_path = self.world_dir.join("metadata.json");

        let mut file = File::create(&metadata_path).map_err(|e| {
            format!(
                "Failed to create metadata file at {}: {}",
                metadata_path.display(),
                e
            )
        })?;

        let metadata = WorldMetadata {
            min_mc_x: self.xzbbox.min_x(),
            max_mc_x: self.xzbbox.max_x(),
            min_mc_z: self.xzbbox.min_z(),
            max_mc_z: self.xzbbox.max_z(),

            min_geo_lat: self.llbbox.min().lat(),
            max_geo_lat: self.llbbox.max().lat(),
            min_geo_lon: self.llbbox.min().lng(),
            max_geo_lon: self.llbbox.max().lng(),
        };

        let contents = serde_json::to_string(&metadata)
            .map_err(|e| format!("Failed to serialize metadata to JSON: {}", e))?;

        write!(&mut file, "{}", contents)
            .map_err(|e| format!("Failed to write metadata to file: {}", e))?;

        Ok(())
    }

    // L�gica Inalterada de Entidades e Chests (Apenas bypass para simplificar RAM)

    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn set_sign(&mut self, line1: String, line2: String, line3: String, line4: String, x: i32, y: i32, z: i32, _rotation: i8) {
        let absolute_y = self.get_absolute_y(x, y, z);
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        // Se o sinal n�o for da regi�o atual, n�s o evitamos no fluxo Scanline.
        if region_x != self.active_region_x || region_z != self.active_region_z { return; }

        let mut block_entities = HashMap::new();
        let messages = vec![
            Value::String(format!("\"{line1}\"")),
            Value::String(format!("\"{line2}\"")),
            Value::String(format!("\"{line3}\"")),
            Value::String(format!("\"{line4}\"")),
        ];
        let mut text_data = HashMap::new();
        text_data.insert("messages".to_string(), Value::List(messages));
        text_data.insert("color".to_string(), Value::String("black".to_string()));
        text_data.insert("has_glowing_text".to_string(), Value::Byte(0));

        block_entities.insert("front_text".to_string(), Value::Compound(text_data));
        block_entities.insert("id".to_string(), Value::String("minecraft:sign".to_string()));
        block_entities.insert("is_waxed".to_string(), Value::Byte(0));
        block_entities.insert("keepPacked".to_string(), Value::Byte(0));
        block_entities.insert("x".to_string(), Value::Int(x));
        block_entities.insert("y".to_string(), Value::Int(absolute_y));
        block_entities.insert("z".to_string(), Value::Int(z));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        if let Some(chunk_data) = chunk.other.get_mut("block_entities") {
            if let Value::List(entities) = chunk_data {
                entities.push(Value::Compound(block_entities));
            }
        } else {
            chunk.other.insert("block_entities".to_string(), Value::List(vec![Value::Compound(block_entities)]));
        }

        self.set_block(SIGN, x, y, z, None, None);
    }

    #[allow(dead_code)]
    pub fn add_entity(&mut self, id: &str, x: i32, y: i32, z: i32, extra_data: Option<HashMap<String, Value>>) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) { return; }
        
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Limita a inser��o de entidades para a regi�o ativa do scanline.
        if region_x != self.active_region_x || region_z != self.active_region_z { return; }

        let absolute_y = self.get_absolute_y(x, y, z);
        let mut entity = HashMap::new();
        entity.insert("id".to_string(), Value::String(id.to_string()));
        entity.insert("Pos".to_string(), Value::List(vec![
            Value::Double(x as f64 + 0.5), Value::Double(absolute_y as f64), Value::Double(z as f64 + 0.5),
        ]));
        entity.insert("Motion".to_string(), Value::List(vec![Value::Double(0.0), Value::Double(0.0), Value::Double(0.0)]));
        entity.insert("Rotation".to_string(), Value::List(vec![Value::Float(0.0), Value::Float(0.0)]));
        entity.insert("OnGround".to_string(), Value::Byte(1));
        entity.insert("FallDistance".to_string(), Value::Float(0.0));
        entity.insert("Fire".to_string(), Value::Short(-20));
        entity.insert("Air".to_string(), Value::Short(300));
        entity.insert("PortalCooldown".to_string(), Value::Int(0));
        entity.insert("UUID".to_string(), Value::IntArray(build_deterministic_uuid(id, x, absolute_y, z)));

        if let Some(extra) = extra_data {
            for (key, value) in extra { entity.insert(key, value); }
        }

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() { list.push(Value::Compound(entity)); }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(entity)]));
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_chest_with_items(&mut self, x: i32, y: i32, z: i32, items: Vec<HashMap<String, Value>>) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_chest_with_items_absolute(x, absolute_y, z, items);
    }

    #[allow(dead_code)]
    pub fn set_chest_with_items_absolute(&mut self, x: i32, absolute_y: i32, z: i32, items: Vec<HashMap<String, Value>>) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) { return; }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Evita chests vazando.
        if region_x != self.active_region_x || region_z != self.active_region_z { return; }

        let mut chest_data = HashMap::new();
        chest_data.insert("id".to_string(), Value::String("minecraft:chest".to_string()));
        chest_data.insert("x".to_string(), Value::Int(x));
        chest_data.insert("y".to_string(), Value::Int(absolute_y));
        chest_data.insert("z".to_string(), Value::Int(z));
        chest_data.insert("Items".to_string(), Value::List(items.into_iter().map(Value::Compound).collect()));
        chest_data.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() { list.push(Value::Compound(chest_data)); }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(chest_data)]));
            }
        }

        self.set_block_absolute(CHEST, x, absolute_y, z, None, None);
    }

    #[allow(dead_code)]
    pub fn set_block_entity_with_items(&mut self, block_with_props: BlockWithProperties, x: i32, y: i32, z: i32, block_entity_id: &str, items: Vec<HashMap<String, Value>>) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_block_entity_with_items_absolute(block_with_props, x, absolute_y, z, block_entity_id, items);
    }

    #[allow(dead_code)]
    pub fn set_block_entity_with_items_absolute(&mut self, block_with_props: BlockWithProperties, x: i32, absolute_y: i32, z: i32, block_entity_id: &str, items: Vec<HashMap<String, Value>>) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) { return; }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Evita blocos entidade vazando pro Halo
        if region_x != self.active_region_x || region_z != self.active_region_z { return; }

        let mut block_entity = HashMap::new();
        block_entity.insert("id".to_string(), Value::String(block_entity_id.to_string()));
        block_entity.insert("x".to_string(), Value::Int(x));
        block_entity.insert("y".to_string(), Value::Int(absolute_y));
        block_entity.insert("z".to_string(), Value::Int(z));
        block_entity.insert("Items".to_string(), Value::List(items.into_iter().map(Value::Compound).collect()));
        block_entity.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() { list.push(Value::Compound(block_entity)); }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(block_entity)]));
            }
        }

        self.set_block_with_properties_absolute(block_with_props, x, absolute_y, z, None, None);
    }
}

#[allow(dead_code)]
fn build_deterministic_uuid(id: &str, x: i32, y: i32, z: i32) -> IntArray {
    let mut hash: i64 = 17;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as i64);
    }

    let seed_a = hash ^ (x as i64).wrapping_shl(32) ^ (y as i64).wrapping_mul(17);
    let seed_b = hash.rotate_left(7) ^ (z as i64).wrapping_mul(31) ^ (x as i64).wrapping_mul(13);

    IntArray::new(vec![
        (seed_a >> 32) as i32,
        seed_a as i32,
        (seed_b >> 32) as i32,
        seed_b as i32,
    ])
}

#[allow(dead_code)]
fn single_item(id: &str, slot: i8, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Slot".to_string(), Value::Byte(slot));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}