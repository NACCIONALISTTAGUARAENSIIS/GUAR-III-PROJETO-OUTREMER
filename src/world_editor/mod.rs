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
/// Teto físico de Y que este escritor grava (formato Java atual). Toda cota
/// de terreno/superfície do motor é limitada por ele — ver `ground.rs`.
pub(crate) use common::MAX_Y as WORLD_MAX_Y;

/// Colunas de uma região Anvil (512 × 512) — tamanho do registro de
/// superfície de terreno intocada (`WorldEditor::terrain_surface_y`).
const TERRAIN_SURFACE_CELLS: usize = 512 * 512;

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
use std::collections::{hash_map::Entry, HashMap, HashSet};
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

/// Semântica de escrita de uma operação adiada no Halo — espelha exatamente as
/// regras que `set_block_absolute`/`set_block_if_absent_absolute`/
/// `fill_column_absolute` aplicam no Core, para que um bloco que "vazou" para
/// a região vizinha seja decidido pelas MESMAS regras quando aquela região
/// for a ativa (e o chão dela já existir).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HaloMode {
    /// `set_block_absolute(.., None, None)` e `set_block_if_absent_absolute`:
    /// só escreve se a posição estiver vazia (AIR/ausente).
    IfAbsent,
    /// `set_block_absolute(.., Some(whitelist), _)`: escreve se vazia OU se o
    /// bloco existente estiver na lista (índice em `halo_block_lists`).
    Whitelist(u16),
    /// `set_block_absolute(.., None, Some(blacklist))`: escreve se vazia OU se
    /// o bloco existente NÃO estiver na lista.
    Blacklist(u16),
    /// `fill_column_absolute(.., skip_existing = false)`: sobrescreve sempre.
    Force,
}

/// Uma escrita adiada, na ordem em que foi emitida.
struct HaloOp {
    x: i32,
    y: i32,
    z: i32,
    block: Block,
    properties: Option<Value>,
    mode: HaloMode,
    /// Precedência do elemento que emitiu a escrita (ver
    /// `WorldEditor::surface_writer_priority`), preservada até o replay.
    priority: u8,
}

/// Precedência do passe de chão: qualquer elemento vence sobre ele.
pub const TERRAIN_WRITE_PRIORITY: u8 = u8::MAX;
/// Maior precedência (inclusive) que ainda conta como ESTRUTURA mapeada
/// (`osm_parser::get_priority`: prédio, via, trilho, água, cerca, piso
/// esportivo, equipamento). Escritas até aqui atravessam vegetação existente.
pub const STRUCTURAL_WRITE_PRIORITY_MAX: u8 = 8;
/// Precedência de escritas que não vêm de um elemento OSM despachado
/// (floresta ambiente, features de provedor, HUD): abaixo de todo elemento
/// mapeado, acima do terreno.
pub const DEFAULT_WRITE_PRIORITY: u8 = 100;

/// Balde de uma região vizinha ainda não ativa.
#[derive(Default)]
struct HaloBucket {
    /// Log de operações, replayado em ordem por `load_halo_to_core`.
    ops: Vec<HaloOp>,
    /// Estado especulativo (última escrita por posição) para leituras
    /// (`check_for_block_absolute`/`block_at_absolute`) feitas ANTES da região
    /// virar ativa — ex.: um mesmo elemento conferindo o que ele próprio já
    /// pintou do outro lado da borda.
    shadow: HashMap<(i32, i32, i32), Block>,
}

/// (RegX, RegZ) -> operações adiadas
type HaloCache = HashMap<(i32, i32), HaloBucket>;

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
    halo_cache: HaloCache,
    /// Whitelists/blacklists internadas (poucas dezenas distintas em todo o
    /// motor) para que cada `HaloOp` guarde só um índice `u16`.
    halo_block_lists: Vec<Vec<Block>>,
    halo_block_list_index: HashMap<Vec<Block>, u16>,

    /// Cota Y da superfície de TERRENO ainda intocada em cada coluna da região
    /// ativa (`i32::MIN` = sem registro), indexada por `surface_idx`.
    ///
    /// 🚨 CORREÇÃO ESTRUTURAL (a causa de "as ruas não são geradas"): o
    /// Scanline preenche o chão da região ANTES de despachar os elementos, e a
    /// escrita padrão do motor (`set_block(..., None, None)`) é "só se vazio".
    /// No Arnis original o chão nasce DEPOIS dos elementos, então toda pintura
    /// de piso — asfalto, calçada, quadra, pátio, gramado de `landuse`, água —
    /// vencia o terreno; aqui ela batia no bloco do chão e era descartada em
    /// silêncio (19 pontos de escrita em 8 módulos). O registro abaixo devolve a
    /// semântica original: enquanto uma coluna ainda tem só o bloco de terreno
    /// do passe de chão, ela conta como VAZIA para qualquer escrita de
    /// elemento (if-absent, whitelist ou blacklist), e a primeira escrita a
    /// consome. Blocos postos por elementos continuam protegidos como antes.
    terrain_surface_y: Vec<i32>,
    /// Precedência (`osm_parser::get_priority`, menor = vence) do último
    /// elemento que escreveu na cota da superfície de cada coluna;
    /// `TERRAIN_WRITE_PRIORITY` enquanto só o passe de chão escreveu. Um
    /// elemento de precedência MAIOR (mais específico) pode repintar o piso
    /// de um de precedência menor — é o que faz o asfalto vencer o gramado de
    /// um `landuse` que chegou antes pelo Halo, qualquer que seja a ordem das
    /// regiões. Empate = o primeiro fica (semântica original do Arnis).
    surface_writer_priority: Vec<u8>,
    /// Precedência do elemento em despacho (definida por `data_processing`).
    current_write_priority: u8,
    /// Ligado durante `flush_pending_halo`: a região foi relida do disco, o
    /// registro acima não existe mais, e a superfície de terreno é reconhecida
    /// pela heurística (bloco de chão do passe de terreno na cota do `Ground`).
    replaying_sealed_region: bool,
    /// Ver `set_write_mask`.
    write_mask: Option<Arc<HashSet<(i32, i32)>>>,

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
            halo_block_lists: Vec::new(),
            halo_block_list_index: HashMap::new(),
            terrain_surface_y: vec![i32::MIN; TERRAIN_SURFACE_CELLS],
            surface_writer_priority: vec![TERRAIN_WRITE_PRIORITY; TERRAIN_SURFACE_CELLS],
            current_write_priority: DEFAULT_WRITE_PRIORITY,
            replaying_sealed_region: false,
            write_mask: None,
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
            halo_block_lists: Vec::new(),
            halo_block_list_index: HashMap::new(),
            terrain_surface_y: vec![i32::MIN; TERRAIN_SURFACE_CELLS],
            surface_writer_priority: vec![TERRAIN_WRITE_PRIORITY; TERRAIN_SURFACE_CELLS],
            current_write_priority: DEFAULT_WRITE_PRIORITY,
            replaying_sealed_region: false,
            write_mask: None,
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
        self.terrain_surface_y.fill(i32::MIN);
        self.surface_writer_priority.fill(TERRAIN_WRITE_PRIORITY);
    }

    /// Restringe as próximas escritas às colunas `(x, z)` do conjunto (`None`
    /// desliga). Existe para geradores que trabalham sobre a caixa envolvente
    /// de um polígono — o interior dos prédios (`buildings_interior.rs`: lajes,
    /// cômodos, corredores, mobília) varre `min..max` em X e Z. Num prédio
    /// inclinado a caixa é bem maior que o prédio, e as lajes de cada andar
    /// vazavam para fora da parede: no SIA, um anel de pisos soltos de ~8
    /// blocos em volta da torre, visível de cima como um retângulo
    /// quadriculado. Com a máscara = contorno real do prédio, a caixa continua
    /// servindo para o layout, mas nada é escrito fora dele.
    pub fn set_write_mask(&mut self, mask: Option<Arc<HashSet<(i32, i32)>>>) {
        self.write_mask = mask;
    }

    /// Coluna dentro do mundo e, se houver máscara ativa, dentro dela.
    #[inline(always)]
    fn accepts_column(&self, x: i32, z: i32) -> bool {
        self.xzbbox.contains(&XZPoint::new(x, z))
            && self
                .write_mask
                .as_ref()
                .is_none_or(|mask| mask.contains(&(x, z)))
    }

    /// Define a precedência das próximas escritas (o elemento em despacho).
    pub fn set_write_priority(&mut self, priority: u8) {
        self.current_write_priority = priority;
    }

    /// Volta à precedência padrão de escritas fora de elemento.
    pub fn reset_write_priority(&mut self) {
        self.current_write_priority = DEFAULT_WRITE_PRIORITY;
    }

    #[inline(always)]
    fn surface_idx(x: i32, z: i32) -> usize {
        (((x & 511) as usize) << 9) | ((z & 511) as usize)
    }

    /// Escreve o bloco de SUPERFÍCIE do passe de chão da região ativa e o marca
    /// como terreno intocado (ver `terrain_surface_y`). Só o passe de terreno
    /// de `data_processing.rs` chama isto; qualquer outra escrita usa
    /// `set_block*` e consome a marca.
    pub fn set_terrain_surface_absolute(&mut self, block: Block, x: i32, absolute_y: i32, z: i32) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }
        let rx = x >> 9;
        let rz = z >> 9;
        if rx != self.active_region_x || rz != self.active_region_z {
            self.push_halo_op(rx, rz, x, absolute_y, z, block, None, HaloMode::IfAbsent);
            return;
        }
        if self.world.get_block(x, absolute_y, z).is_none() {
            self.world.set_block(x, absolute_y, z, block);
            let idx = Self::surface_idx(x, z);
            self.terrain_surface_y[idx] = absolute_y;
            self.surface_writer_priority[idx] = TERRAIN_WRITE_PRIORITY;
        }
    }

    /// `true` se `(x, y, z)` é a cota de superfície da coluna e quem está lá
    /// (terreno intocado ou piso de um elemento menos prioritário) cede a uma
    /// escrita de precedência `writer` — para efeito dessa escrita, a posição
    /// conta como VAZIA.
    #[inline]
    fn surface_yields_to(
        &self,
        x: i32,
        absolute_y: i32,
        z: i32,
        existing: Block,
        writer: u8,
    ) -> bool {
        let idx = Self::surface_idx(x, z);
        if self.terrain_surface_y[idx] == absolute_y && writer < self.surface_writer_priority[idx] {
            return true;
        }
        // Região relida do disco (2ª passada do Halo): o registro se perdeu, mas
        // o bloco de chão do passe de terreno na cota exata do `Ground` só pode
        // ter vindo dele — elementos não põem GRASS/ANDESITO POLIDO exatamente
        // ali sem também terem consumido a coluna (calçadas e gramados de
        // elementos ficam a salvo porque o replay tardio só traz vazamentos
        // "para trás", que a âncora por canto mínimo já torna raros).
        self.replaying_sealed_region
            && (existing == GRASS_BLOCK || existing == POLISHED_ANDESITE)
            && self.get_ground_level(x, z) == absolute_y
    }

    /// Registra quem escreveu na cota da superfície de `(x, z)`.
    #[inline]
    fn record_surface_write(&mut self, x: i32, absolute_y: i32, z: i32, writer: u8) {
        let idx = Self::surface_idx(x, z);
        if self.terrain_surface_y[idx] == absolute_y {
            self.surface_writer_priority[idx] = writer;
        }
    }

    /// Bloco existente na região ativa do ponto de vista de uma escrita de
    /// precedência `writer`: superfície que cede é reportada como ausente.
    #[inline]
    fn existing_for_write(&self, x: i32, absolute_y: i32, z: i32, writer: u8) -> Option<Block> {
        let existing = self.world.get_block(x, absolute_y, z)?;
        if self.surface_yields_to(x, absolute_y, z, existing, writer) {
            return None;
        }
        // 🚨 Vegetação cede a estrutura. Árvores de `landuse`/`natural`/LiDAR
        // chegam com frequência ANTES do prédio ou da via (pelo Halo, ou porque
        // a nuvem LiDAR não distingue copa de telhado) e, como toda escrita é
        // "só se vazio", o prédio nascia com buracos e a rua com troncos. A
        // copa/tronco/capim existente conta como vazio para um escritor
        // estrutural; vegetação sobre vegetação segue "o primeiro fica".
        if writer <= STRUCTURAL_WRITE_PRIORITY_MAX && existing.is_vegetation() {
            return None;
        }
        Some(existing)
    }

    /// Remove a vegetação (copa, tronco, capim) da coluna acima de `from_y`
    /// até o primeiro bloco que não seja vegetação nem ar, no máximo
    /// `max_height` blocos — para o asfalto/calçada não ficarem debaixo de uma
    /// copa que chegou antes. Só na região ativa (na borda, o replay do Halo
    /// já aplica a regra de precedência acima).
    pub fn clear_vegetation_above(&mut self, x: i32, from_y: i32, z: i32, max_height: i32) {
        if (x >> 9) != self.active_region_x || (z >> 9) != self.active_region_z {
            return;
        }
        let mut gap = 0;
        for y in (from_y + 1)..=(from_y + max_height) {
            match self.world.get_block(x, y, z) {
                Some(b) if b.is_vegetation() => {
                    self.world.set_block(x, y, z, AIR);
                    gap = 0;
                }
                Some(_) => break,
                // copas têm vãos: tolera até 2 blocos de ar antes de desistir
                None => {
                    gap += 1;
                    if gap > 2 {
                        break;
                    }
                }
            }
        }
    }

    /// Idem, para o elemento em despacho.
    #[inline]
    fn existing_for_element_write(&self, x: i32, absolute_y: i32, z: i32) -> Option<Block> {
        self.existing_for_write(x, absolute_y, z, self.current_write_priority)
    }

    /// Idem, após a escrita do elemento em despacho.
    #[inline]
    fn consume_terrain_surface(&mut self, x: i32, absolute_y: i32, z: i32) {
        self.record_surface_write(x, absolute_y, z, self.current_write_priority);
    }

    /// Replaya, na região agora ativa, as escritas que elementos de regiões
    /// anteriores emitiram para cá — com a MESMA semântica (if-absent/
    /// whitelist/blacklist/force) que teriam tido se a região já fosse a ativa.
    /// Chamado DEPOIS do chão da região existir, para que as whitelists
    /// enxerguem o terreno real (exatamente como no desenho in-core).
    pub fn load_halo_to_core(&mut self) {
        let region_key = (self.active_region_x, self.active_region_z);

        if let Some(bucket) = self.halo_cache.remove(&region_key) {
            let count = bucket.ops.len();
            for op in bucket.ops {
                self.apply_halo_op(op);
            }
            if count > 0 {
                println!(
                    "[HALO] {} operações replayadas na região ({}, {})",
                    count, self.active_region_x, self.active_region_z
                );
            }
        }
    }

    fn apply_halo_op(&mut self, op: HaloOp) {
        let existing = self.existing_for_write(op.x, op.y, op.z, op.priority);
        let should_insert = match (op.mode, existing) {
            (HaloMode::Force, _) => true,
            (_, None) => true,
            (HaloMode::IfAbsent, Some(_)) => false,
            (HaloMode::Whitelist(i), Some(e)) => self.halo_block_lists[i as usize]
                .iter()
                .any(|b| b.id() == e.id()),
            (HaloMode::Blacklist(i), Some(e)) => !self.halo_block_lists[i as usize]
                .iter()
                .any(|b| b.id() == e.id()),
        };
        if should_insert {
            match op.properties {
                Some(props) => self.world.set_block_with_properties(
                    op.x,
                    op.y,
                    op.z,
                    BlockWithProperties::new(op.block, Some(props)),
                ),
                None => self.world.set_block(op.x, op.y, op.z, op.block),
            }
            self.record_surface_write(op.x, op.y, op.z, op.priority);
        }
    }

    fn intern_block_list(&mut self, list: &[Block]) -> u16 {
        if let Some(&idx) = self.halo_block_list_index.get(list) {
            return idx;
        }
        let idx = self.halo_block_lists.len() as u16;
        self.halo_block_lists.push(list.to_vec());
        self.halo_block_list_index.insert(list.to_vec(), idx);
        idx
    }

    /// Enfileira uma escrita para uma região ainda não ativa.
    #[allow(clippy::too_many_arguments)]
    fn push_halo_op(
        &mut self,
        rx: i32,
        rz: i32,
        x: i32,
        y: i32,
        z: i32,
        block: Block,
        properties: Option<Value>,
        mode: HaloMode,
    ) {
        let bucket = self.halo_cache.entry((rx, rz)).or_default();
        // Estado especulativo para leituras antecipadas: aplica a mesma regra
        // contra o que o próprio Halo já "escreveu" nesta posição.
        let speculative_insert = match (mode, bucket.shadow.get(&(x, y, z))) {
            (HaloMode::Force, _) | (_, None) => true,
            (HaloMode::IfAbsent, Some(_)) => false,
            (HaloMode::Whitelist(i), Some(e)) => self.halo_block_lists[i as usize]
                .iter()
                .any(|b| b.id() == e.id()),
            (HaloMode::Blacklist(i), Some(e)) => !self.halo_block_lists[i as usize]
                .iter()
                .any(|b| b.id() == e.id()),
        };
        if speculative_insert {
            bucket.shadow.insert((x, y, z), block);
        }
        bucket.ops.push(HaloOp {
            x,
            y,
            z,
            block,
            properties,
            mode,
            priority: self.current_write_priority,
        });
    }

    /// Segunda passada do Halo: aplica as operações que ficaram pendentes para
    /// regiões JÁ SELADAS em disco (vazamentos "para trás" que a âncora por
    /// canto mínimo não cobre — ex.: copas da floresta ambiente e troncos
    /// caídos gerados por chunk na borda oeste/norte de cada região), relendo
    /// cada região do disco, replayando com a semântica normal e regravando.
    /// Devolve (regiões regravadas, operações aplicadas). Só para Java Anvil —
    /// o Bedrock não tem região reabrível.
    pub fn flush_pending_halo(&mut self) -> Result<(usize, usize), String> {
        let mut keys: Vec<(i32, i32)> = self
            .halo_cache
            .iter()
            .filter(|(_, b)| !b.ops.is_empty())
            .map(|(k, _)| *k)
            .collect();
        if keys.is_empty() {
            return Ok((0, 0));
        }
        if self.format != WorldFormat::JavaAnvil {
            return Err(format!(
                "{} operações do Halo pendentes não podem ser aplicadas neste formato",
                self.pending_halo_ops()
            ));
        }
        keys.sort_unstable();
        let mut applied = 0usize;
        for (rx, rz) in &keys {
            let ops = self
                .halo_cache
                .get(&(*rx, *rz))
                .map(|b| b.ops.len())
                .unwrap_or(0);
            self.set_active_region(*rx, *rz);
            self.world = WorldToModify::default();
            self.load_java_region_from_disk(*rx, *rz)?;
            self.replaying_sealed_region = true;
            self.load_halo_to_core();
            self.replaying_sealed_region = false;
            self.flush_active_region();
            applied += ops;
        }
        Ok((keys.len(), applied))
    }

    /// Operações do Halo que NUNCA foram replayadas (regiões que a varredura
    /// não visitou depois de recebê-las). Com o roteamento por canto mínimo de
    /// `data_processing.rs` isto deve ser sempre zero; é conferido no fim da
    /// geração para que uma regressão apareça no log em vez de sumir em
    /// silêncio como blocos perdidos na borda de região.
    pub fn pending_halo_ops(&self) -> usize {
        self.halo_cache.values().map(|b| b.ops.len()).sum()
    }

    /// Retorna o tamanho atual do Halo Cache para estat�sticas do Terminal HUD
    /// (usado por `master_control.rs`, que só é alcançável no build sem `gui`).
    #[allow(dead_code)]
    pub fn get_halo_metrics(&self) -> (usize, usize) {
        let active_buckets = self.halo_cache.len();
        let total_blocks = self.pending_halo_ops();
        (active_buckets, total_blocks)
    }

    /// Comprime o Core Cache (WorldToModify) com Zlib, escreve o `.mca` no disco,
    /// e em seguida ANIQUILA a RAM do Core para garantir seguran�a de 24GB constante.
    pub fn flush_active_region(&mut self) {
        // Compacta as se��es para poupar banda de mem�ria antes da grava��o
        self.world.compact_sections();

        // ?? No Java Edition, uma regi�o equivale a um arquivo Anvil exato.
        match self.format {
            WorldFormat::JavaAnvil => {
                self.save_java_region(self.active_region_x, self.active_region_z)
            }
            WorldFormat::BedrockMcWorld => {
                // Para Bedrock, o fluxo out-of-core � mais complexo devido ao LevelDB.
                // Por ora, acumularemos as muta��es no driver apropriado que faremos no bedrock.rs
            }
        }

        // ?? EXPURGO ABSOLUTO O(1): Mata a RAM do Core
        self.world = WorldToModify::default();
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
        if !self.accepts_column(x, z) {
            return;
        }

        let rx = x >> 9; // Bitwise Shift instant�neo (512 = 2^9)
        let rz = z >> 9;

        // Se o bloco pertencer � regi�o ativamente processada, ele vai pro Core.
        if rx == self.active_region_x && rz == self.active_region_z {
            let should_insert =
                if let Some(existing_block) = self.existing_for_element_write(x, absolute_y, z) {
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
                self.consume_terrain_surface(x, absolute_y, z);
            }
        }
        // Se o bloco pertencer a uma regi�o vizinha (vazamento), ele vai pro Halo
        // Cache — com a semântica preservada, decidida no replay.
        else {
            let mode = self.halo_mode_for(override_whitelist, override_blacklist);
            self.push_halo_op(rx, rz, x, absolute_y, z, block, None, mode);
        }
    }

    fn halo_mode_for(
        &mut self,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) -> HaloMode {
        if let Some(whitelist) = override_whitelist {
            HaloMode::Whitelist(self.intern_block_list(whitelist))
        } else if let Some(blacklist) = override_blacklist {
            HaloMode::Blacklist(self.intern_block_list(blacklist))
        } else {
            HaloMode::IfAbsent
        }
    }

    /// Fast-path para blocos sem blacklist/whitelist
    #[inline]
    pub fn set_block_if_absent_absolute(&mut self, block: Block, x: i32, absolute_y: i32, z: i32) {
        if !self.accepts_column(x, z) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            if self.existing_for_element_write(x, absolute_y, z).is_none() {
                self.world.set_block(x, absolute_y, z, block);
                self.consume_terrain_surface(x, absolute_y, z);
            }
        } else {
            self.push_halo_op(rx, rz, x, absolute_y, z, block, None, HaloMode::IfAbsent);
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
        self.set_block_absolute(
            block,
            x,
            absolute_y,
            z,
            override_whitelist,
            override_blacklist,
        );
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
        if !self.accepts_column(x, z) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            let should_insert =
                if let Some(existing_block) = self.existing_for_element_write(x, absolute_y, z) {
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
                self.world
                    .set_block_with_properties(x, absolute_y, z, block_with_props);
                self.consume_terrain_surface(x, absolute_y, z);
            }
        } else {
            // Propriedades (orientação de escada, meia-laje em cima, etc.) são
            // preservadas — antes só o bloco base sobrevivia à borda de região.
            let mode = self.halo_mode_for(override_whitelist, override_blacklist);
            self.push_halo_op(
                rx,
                rz,
                x,
                absolute_y,
                z,
                block_with_props.block,
                block_with_props.properties,
                mode,
            );
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

        // Se a regi�o vazou, checa o estado especulativo do Halo
        if let Some(bucket) = self.halo_cache.get(&(rx, rz)) {
            if let Some(existing_block) = bucket.shadow.get(&(x, absolute_y, z)) {
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
            self.halo_cache
                .get(&(rx, rz))
                .is_some_and(|b| b.shadow.contains_key(&(x, absolute_y, z)))
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
        if !self.accepts_column(x, z) {
            return;
        }

        let rx = x >> 9;
        let rz = z >> 9;

        if rx == self.active_region_x && rz == self.active_region_z {
            self.world
                .fill_column(x, z, y_min, y_max, block, skip_existing);
        } else {
            // Emula o fill bloco a bloco no Halo, com a MESMA semântica do Core:
            // `skip_existing = false` sobrescreve (Force), não "só se vazio".
            let mode = if skip_existing {
                HaloMode::IfAbsent
            } else {
                HaloMode::Force
            };
            for y in y_min..=y_max {
                self.push_halo_op(rx, rz, x, y, z, block, None, mode);
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
                .unwrap_or("Pincelism World")
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
    pub fn set_sign(
        &mut self,
        line1: String,
        line2: String,
        line3: String,
        line4: String,
        x: i32,
        y: i32,
        z: i32,
        _rotation: i8,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        // Se o sinal n�o for da regi�o atual, n�s o evitamos no fluxo Scanline.
        if region_x != self.active_region_x || region_z != self.active_region_z {
            return;
        }

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
        block_entities.insert(
            "id".to_string(),
            Value::String("minecraft:sign".to_string()),
        );
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
            chunk.other.insert(
                "block_entities".to_string(),
                Value::List(vec![Value::Compound(block_entities)]),
            );
        }

        self.set_block(SIGN, x, y, z, None, None);
    }

    #[allow(dead_code)]
    pub fn add_entity(
        &mut self,
        id: &str,
        x: i32,
        y: i32,
        z: i32,
        extra_data: Option<HashMap<String, Value>>,
    ) {
        if !self.accepts_column(x, z) {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Limita a inser��o de entidades para a regi�o ativa do scanline.
        if region_x != self.active_region_x || region_z != self.active_region_z {
            return;
        }

        let absolute_y = self.get_absolute_y(x, y, z);
        let mut entity = HashMap::new();
        entity.insert("id".to_string(), Value::String(id.to_string()));
        entity.insert(
            "Pos".to_string(),
            Value::List(vec![
                Value::Double(x as f64 + 0.5),
                Value::Double(absolute_y as f64),
                Value::Double(z as f64 + 0.5),
            ]),
        );
        entity.insert(
            "Motion".to_string(),
            Value::List(vec![
                Value::Double(0.0),
                Value::Double(0.0),
                Value::Double(0.0),
            ]),
        );
        entity.insert(
            "Rotation".to_string(),
            Value::List(vec![Value::Float(0.0), Value::Float(0.0)]),
        );
        entity.insert("OnGround".to_string(), Value::Byte(1));
        entity.insert("FallDistance".to_string(), Value::Float(0.0));
        entity.insert("Fire".to_string(), Value::Short(-20));
        entity.insert("Air".to_string(), Value::Short(300));
        entity.insert("PortalCooldown".to_string(), Value::Int(0));
        entity.insert(
            "UUID".to_string(),
            Value::IntArray(build_deterministic_uuid(id, x, absolute_y, z)),
        );

        if let Some(extra) = extra_data {
            for (key, value) in extra {
                entity.insert(key, value);
            }
        }

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(entity));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(entity)]));
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_chest_with_items(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        items: Vec<HashMap<String, Value>>,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_chest_with_items_absolute(x, absolute_y, z, items);
    }

    #[allow(dead_code)]
    pub fn set_chest_with_items_absolute(
        &mut self,
        x: i32,
        absolute_y: i32,
        z: i32,
        items: Vec<HashMap<String, Value>>,
    ) {
        if !self.accepts_column(x, z) {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Evita chests vazando.
        if region_x != self.active_region_x || region_z != self.active_region_z {
            return;
        }

        let mut chest_data = HashMap::new();
        chest_data.insert(
            "id".to_string(),
            Value::String("minecraft:chest".to_string()),
        );
        chest_data.insert("x".to_string(), Value::Int(x));
        chest_data.insert("y".to_string(), Value::Int(absolute_y));
        chest_data.insert("z".to_string(), Value::Int(z));
        chest_data.insert(
            "Items".to_string(),
            Value::List(items.into_iter().map(Value::Compound).collect()),
        );
        chest_data.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(chest_data));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(chest_data)]));
            }
        }

        self.set_block_absolute(CHEST, x, absolute_y, z, None, None);
    }

    #[allow(dead_code)]
    pub fn set_block_entity_with_items(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        y: i32,
        z: i32,
        block_entity_id: &str,
        items: Vec<HashMap<String, Value>>,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_block_entity_with_items_absolute(
            block_with_props,
            x,
            absolute_y,
            z,
            block_entity_id,
            items,
        );
    }

    #[allow(dead_code)]
    pub fn set_block_entity_with_items_absolute(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        absolute_y: i32,
        z: i32,
        block_entity_id: &str,
        items: Vec<HashMap<String, Value>>,
    ) {
        if !self.accepts_column(x, z) {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        // Evita blocos entidade vazando pro Halo
        if region_x != self.active_region_x || region_z != self.active_region_z {
            return;
        }

        let mut block_entity = HashMap::new();
        block_entity.insert("id".to_string(), Value::String(block_entity_id.to_string()));
        block_entity.insert("x".to_string(), Value::Int(x));
        block_entity.insert("y".to_string(), Value::Int(absolute_y));
        block_entity.insert("z".to_string(), Value::Int(z));
        block_entity.insert(
            "Items".to_string(),
            Value::List(items.into_iter().map(Value::Compound).collect()),
        );
        block_entity.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(block_entity));
                }
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

#[cfg(test)]
mod halo_tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    fn editor_for(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(-15.83, -47.98, -15.82, -47.97).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), xzbbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_flat(-62)));
        editor
    }

    fn block_at(editor: &WorldEditor, x: i32, y: i32, z: i32) -> Option<Block> {
        editor.world.get_block(x, y, z)
    }

    /// Região 0 ativa, escritas vazando para a região 1 (x >= 512), depois a
    /// região 1 vira ativa com um "chão" já posto — cada modo tem que decidir
    /// exatamente como decidiria in-core.
    #[test]
    fn halo_replay_honours_core_write_semantics() {
        let xzbbox = XZBBox::new(0, 1023, 0, 511);
        let mut editor = editor_for(&xzbbox);
        editor.set_active_region(0, 0);

        // 1. IfAbsent sobre chão existente: NÃO escreve.
        editor.set_block_absolute(OAK_PLANKS, 600, -62, 10, None, None);
        // 2. Whitelist que bate com o chão (GRASS_BLOCK): escreve.
        editor.set_block_absolute(BLACK_CONCRETE, 601, -62, 10, Some(&[GRASS_BLOCK]), None);
        // 3. Whitelist que NÃO bate (STONE): não escreve.
        editor.set_block_absolute(BLACK_CONCRETE, 602, -62, 10, Some(&[STONE]), None);
        // 4. Blacklist contendo o chão: não escreve.
        editor.set_block_absolute(BLACK_CONCRETE, 603, -62, 10, None, Some(&[GRASS_BLOCK]));
        // 5. Blacklist sem o chão: escreve.
        editor.set_block_absolute(BLACK_CONCRETE, 604, -62, 10, None, Some(&[STONE]));
        // 6. Force (fill_column sem skip): sobrescreve.
        editor.fill_column_absolute(STONE, 605, 10, -62, -62, false);
        // 7. Posição vazia: qualquer modo escreve.
        editor.set_block_absolute(OAK_PLANKS, 606, -60, 10, None, None);
        // 8. Propriedades sobrevivem à borda.
        let props = Value::Compound(HashMap::from([(
            "facing".to_string(),
            Value::String("north".to_string()),
        )]));
        editor.set_block_with_properties_absolute(
            BlockWithProperties::new(OAK_STAIRS, Some(props)),
            607,
            -60,
            10,
            None,
            None,
        );

        assert_eq!(editor.pending_halo_ops(), 8);
        assert!(block_at(&editor, 600, -62, 10).is_none());

        // Região 1 vira ativa: chão primeiro, halo depois (mesma ordem do Scanline).
        editor.flush_active_region_for_test();
        editor.set_active_region(1, 0);
        for x in 600..=607 {
            editor.set_block_if_absent_absolute(GRASS_BLOCK, x, -62, 10);
        }
        editor.load_halo_to_core();
        assert_eq!(editor.pending_halo_ops(), 0);

        assert_eq!(block_at(&editor, 600, -62, 10), Some(GRASS_BLOCK));
        assert_eq!(block_at(&editor, 601, -62, 10), Some(BLACK_CONCRETE));
        assert_eq!(block_at(&editor, 602, -62, 10), Some(GRASS_BLOCK));
        assert_eq!(block_at(&editor, 603, -62, 10), Some(GRASS_BLOCK));
        assert_eq!(block_at(&editor, 604, -62, 10), Some(BLACK_CONCRETE));
        assert_eq!(block_at(&editor, 605, -62, 10), Some(STONE));
        assert_eq!(block_at(&editor, 606, -60, 10), Some(OAK_PLANKS));
        assert_eq!(block_at(&editor, 607, -60, 10), Some(OAK_STAIRS));

        let (px, py, pz) = (607i32, -60i32, 10i32);
        let region = editor.world.get_region(1, 0).unwrap();
        let chunk = region.get_chunk((px >> 4) & 31, (pz >> 4) & 31).unwrap();
        let section = chunk.sections.get(&(py >> 4).try_into().unwrap()).unwrap();
        let idx = common::SectionToModify::index((px & 15) as u8, (py & 15) as u8, (pz & 15) as u8);
        assert!(
            section.properties.contains_key(&idx),
            "propriedades perdidas no halo"
        );
    }

    /// Leituras antecipadas veem o estado especulativo do halo, na ordem das
    /// escritas (o mesmo elemento conferindo o que já pintou do outro lado).
    #[test]
    fn halo_shadow_reflects_speculative_state_in_order() {
        let xzbbox = XZBBox::new(0, 1023, 0, 511);
        let mut editor = editor_for(&xzbbox);
        editor.set_active_region(0, 0);

        assert!(!editor.block_at_absolute(700, -62, 5));
        editor.set_block_absolute(GRASS_BLOCK, 700, -62, 5, None, None);
        assert!(editor.check_for_block_absolute(700, -62, 5, Some(&[GRASS_BLOCK]), None));
        // IfAbsent por cima: ignorado também no shadow.
        editor.set_block_absolute(STONE, 700, -62, 5, None, None);
        assert!(editor.check_for_block_absolute(700, -62, 5, Some(&[GRASS_BLOCK]), None));
        // Whitelist que bate: shadow atualizado.
        editor.set_block_absolute(STONE, 700, -62, 5, Some(&[GRASS_BLOCK]), None);
        assert!(editor.check_for_block_absolute(700, -62, 5, Some(&[STONE]), None));
    }

    /// O chão do passe de terreno conta como VAZIO para a primeira escrita de
    /// elemento (qualquer modo) — e só para ela; blocos de elementos seguem
    /// protegidos, e o mesmo vale para o que chega pelo Halo.
    #[test]
    fn untouched_terrain_surface_is_replaceable_exactly_once() {
        let xzbbox = XZBBox::new(0, 1023, 0, 511);
        let mut editor = editor_for(&xzbbox);
        editor.set_active_region(0, 0);
        for x in 10..=16 {
            editor.set_terrain_surface_absolute(POLISHED_ANDESITE, x, -62, 20);
        }
        // Lê como andesito (as whitelists dos módulos enxergam o terreno real)…
        assert!(editor.check_for_block_absolute(10, -62, 20, Some(&[POLISHED_ANDESITE]), None));
        // …mas a pintura de piso if-absent (asfalto) substitui.
        editor.set_block_absolute(BLACK_CONCRETE, 10, -62, 20, None, None);
        assert_eq!(block_at(&editor, 10, -62, 20), Some(BLACK_CONCRETE));
        // Uma segunda escrita if-absent já não vence: a coluna foi consumida.
        editor.set_block_absolute(SMOOTH_QUARTZ, 10, -62, 20, None, None);
        assert_eq!(block_at(&editor, 10, -62, 20), Some(BLACK_CONCRETE));
        // Whitelist que não inclui o terreno também vence sobre terreno intocado
        // (era o que acontecia no Arnis, onde o chão ainda não existia)…
        editor.set_block_absolute(RED_CONCRETE, 11, -62, 20, Some(&[GRASS_BLOCK]), None);
        assert_eq!(block_at(&editor, 11, -62, 20), Some(RED_CONCRETE));
        // …e a blacklist idem.
        editor.set_block_absolute(WATER, 12, -62, 20, None, Some(&[POLISHED_ANDESITE]));
        assert_eq!(block_at(&editor, 12, -62, 20), Some(WATER));
        // Fast-path if-absent também.
        editor.set_block_if_absent_absolute(SAND, 13, -62, 20);
        assert_eq!(block_at(&editor, 13, -62, 20), Some(SAND));
        // Fora da cota da superfície nada muda: um bloco de elemento acima segue protegido.
        editor.set_block_absolute(BRICK, 14, -61, 20, None, None);
        editor.set_block_absolute(STONE, 14, -61, 20, None, None);
        assert_eq!(block_at(&editor, 14, -61, 20), Some(BRICK));
        // Precedência de piso: um landuse (10) pinta grama; a via (2) que chega
        // depois repinta com asfalto; outro landuse (10) não desfaz; um prédio
        // (1) ainda vence a via.
        editor.set_write_priority(10);
        editor.set_block_absolute(GRASS_BLOCK, 15, -62, 20, None, None);
        assert_eq!(block_at(&editor, 15, -62, 20), Some(GRASS_BLOCK));
        editor.set_write_priority(2);
        editor.set_block_absolute(BLACK_CONCRETE, 15, -62, 20, None, None);
        assert_eq!(block_at(&editor, 15, -62, 20), Some(BLACK_CONCRETE));
        editor.set_write_priority(10);
        editor.set_block_absolute(GRASS_BLOCK, 15, -62, 20, None, None);
        assert_eq!(block_at(&editor, 15, -62, 20), Some(BLACK_CONCRETE));
        editor.set_write_priority(1);
        editor.set_block_absolute(SMOOTH_STONE, 15, -62, 20, None, None);
        assert_eq!(block_at(&editor, 15, -62, 20), Some(SMOOTH_STONE));
        editor.reset_write_priority();
        // Vegetação cede a estrutura: copa de uma árvore de landuse (10) onde
        // depois nasce a parede de um prédio (1); outro landuse (10) não passa;
        // e a via limpa a copa acima do asfalto.
        editor.set_write_priority(10);
        editor.set_block_absolute(OAK_LEAVES, 16, -58, 20, None, None);
        editor.set_block_absolute(OAK_LEAVES, 16, -57, 20, None, None);
        editor.set_block_absolute(OAK_LOG, 17, -61, 20, None, None);
        editor.set_block_absolute(OAK_LEAVES, 17, -60, 20, None, None);
        editor.set_block_absolute(OAK_LEAVES, 17, -58, 20, None, None); // vão de 1 em -59
        editor.set_write_priority(10);
        editor.set_block_absolute(BRICK, 16, -58, 20, None, None);
        assert_eq!(block_at(&editor, 16, -58, 20), Some(OAK_LEAVES));
        editor.set_write_priority(1);
        editor.set_block_absolute(BRICK, 16, -58, 20, None, None);
        assert_eq!(block_at(&editor, 16, -58, 20), Some(BRICK));
        editor.set_write_priority(2);
        editor.clear_vegetation_above(17, -62, 20, 14);
        assert!(block_at(&editor, 17, -61, 20).is_none());
        assert!(block_at(&editor, 17, -60, 20).is_none());
        assert!(block_at(&editor, 17, -58, 20).is_none());
        editor.reset_write_priority();

        // Pelo Halo: região 1 recebe uma escrita if-absent de asfalto antes de
        // existir; quando vira ativa, o chão de terreno nasce e o replay vence.
        editor.set_block_absolute(BLACK_CONCRETE, 700, -62, 20, None, None);
        // E o caso real da QE 17: um landuse (10) da região 0 vaza grama para a
        // região 1 pelo Halo (replayado ANTES dos elementos da região 1); a via
        // (2) despachada in-core na região 1 tem que vencer mesmo chegando depois.
        editor.set_write_priority(10);
        editor.set_block_absolute(GRASS_BLOCK, 701, -62, 20, None, None);
        editor.reset_write_priority();
        editor.flush_active_region_for_test();
        editor.set_active_region(1, 0);
        editor.set_terrain_surface_absolute(GRASS_BLOCK, 700, -62, 20);
        editor.set_terrain_surface_absolute(POLISHED_ANDESITE, 701, -62, 20);
        editor.load_halo_to_core();
        assert_eq!(block_at(&editor, 700, -62, 20), Some(BLACK_CONCRETE));
        assert_eq!(block_at(&editor, 701, -62, 20), Some(GRASS_BLOCK));
        editor.set_write_priority(2);
        editor.set_block_absolute(BLACK_CONCRETE, 701, -62, 20, None, None);
        assert_eq!(block_at(&editor, 701, -62, 20), Some(BLACK_CONCRETE));
        editor.reset_write_priority();
    }

    /// Vazamento "para trás" (para uma região já gravada em disco): a segunda
    /// passada relê a região, aplica e regrava — e o que já estava lá sobrevive.
    #[test]
    fn pending_halo_is_flushed_into_already_sealed_regions_on_disk() {
        use fastanvil::Chunk as _;
        let tmp = tempfile::tempdir().expect("tmp");
        let xzbbox = XZBBox::new(0, 1023, 0, 511);
        let llbbox = LLBBox::new(-15.83, -47.98, -15.82, -47.97).unwrap();
        let mut editor = WorldEditor::new(tmp.path().to_path_buf(), &xzbbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_flat(-62)));

        // Região 0 selada em disco com um bloco original (e uma escada com
        // propriedades, que precisa sobreviver à releitura).
        editor.set_active_region(0, 0);
        editor.set_block_absolute(BRICK, 100, -50, 100, None, None);
        let props = Value::Compound(HashMap::from([(
            "facing".to_string(),
            Value::String("east".to_string()),
        )]));
        editor.set_block_with_properties_absolute(
            BlockWithProperties::new(OAK_STAIRS, Some(props)),
            101,
            -50,
            100,
            None,
            None,
        );
        editor.flush_active_region();

        // Região 1 ativa: um elemento vaza para trás, para a região 0.
        editor.set_active_region(1, 0);
        editor.set_block_absolute(OAK_LEAVES, 510, -49, 100, None, None);
        editor.set_block_absolute(STONE, 100, -50, 100, None, None); // IfAbsent: não sobrescreve o tijolo
        editor.flush_active_region();
        assert_eq!(editor.pending_halo_ops(), 2);

        let (regions, applied) = editor.flush_pending_halo().expect("flush");
        assert_eq!((regions, applied), (1, 2));
        assert_eq!(editor.pending_halo_ops(), 0);

        let file = std::fs::File::open(tmp.path().join("region/r.0.0.mca")).unwrap();
        let mut region = fastanvil::Region::from_stream(file).unwrap();
        let block_name = |region: &mut fastanvil::Region<std::fs::File>, x: i32, y: i32, z: i32| {
            let raw = region
                .read_chunk((x >> 4) as usize, (z >> 4) as usize)
                .unwrap()
                .unwrap();
            let chunk: fastanvil::CurrentJavaChunk = fastnbt::from_bytes(&raw).unwrap();
            chunk
                .block((x & 15) as usize, y as isize, (z & 15) as usize)
                .map(|b| b.encoded_description().to_string())
        };
        assert_eq!(
            block_name(&mut region, 510, -49, 100).as_deref(),
            Some("minecraft:oak_leaves|persistent=true")
        );
        assert_eq!(
            block_name(&mut region, 100, -50, 100).as_deref(),
            Some("minecraft:bricks|")
        );
        assert_eq!(
            block_name(&mut region, 101, -50, 100).as_deref(),
            Some("minecraft:oak_stairs|facing=east")
        );
    }

    impl<'a> WorldEditor<'a> {
        /// Descarta o Core sem tocar no disco (equivalente de teste do
        /// `flush_active_region`, que gravaria um `.mca`).
        fn flush_active_region_for_test(&mut self) {
            self.world = WorldToModify::default();
        }
    }
}

#[cfg(test)]
mod write_mask_tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    /// Interior gerado sobre a caixa envolvente de um prédio: com a máscara do
    /// contorno real, nada é escrito fora dele — nem in-core nem no Halo.
    #[test]
    fn write_mask_confines_writes_to_the_footprint() {
        let xzbbox = XZBBox::new(0, 1023, 0, 511);
        let llbbox = LLBBox::new(-15.83, -47.98, -15.82, -47.97).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_flat(-62)));
        editor.set_active_region(0, 0);

        let footprint: HashSet<(i32, i32)> = [(10, 10), (11, 10)].into_iter().collect();
        editor.set_write_mask(Some(Arc::new(footprint)));
        for x in 9..=12 {
            editor.set_block_absolute(POLISHED_ANDESITE, x, -50, 10, None, None);
        }
        editor.set_block_absolute(POLISHED_ANDESITE, 600, -50, 10, None, None); // região vizinha
        editor.set_write_mask(None);
        editor.set_block_absolute(STONE, 12, -49, 10, None, None);

        assert_eq!(editor.world.get_block(9, -50, 10), None);
        assert_eq!(editor.world.get_block(10, -50, 10), Some(POLISHED_ANDESITE));
        assert_eq!(editor.world.get_block(11, -50, 10), Some(POLISHED_ANDESITE));
        assert_eq!(editor.world.get_block(12, -50, 10), None);
        assert!(
            editor.halo_cache.is_empty(),
            "máscara também vale para o Halo"
        );
        // Sem máscara, volta ao normal.
        assert_eq!(editor.world.get_block(12, -49, 10), Some(STONE));
    }
}
