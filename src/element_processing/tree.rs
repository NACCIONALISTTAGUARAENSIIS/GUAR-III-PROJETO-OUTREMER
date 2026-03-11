#![allow(unused)]

use crate::block_definitions::*;
use crate::deterministic_rng::coord_rng;
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::world_editor::WorldEditor;
use noise::{NoiseFn, OpenSimplex};
use once_cell::sync::Lazy;
use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;
use std::collections::HashMap;
use std::f64::consts::PI;

type Coord = (i32, i32, i32);

// =====================================================
// CONSTANTES DE BLOCOS EXTRAS (Cerrado e Builder Specs)
// =====================================================
pub const AZALEA_LEAVES: Block = Block::new(225); 
pub const FLOWERING_AZALEA: Block = Block::new(189); 
pub const MOSS_CARPET: Block = Block::new(141); 
pub const SHORT_GRASS: Block = Block::new(29);
pub const POLISHED_BASALT: Block = Block::new(56); // Usado para casca podre/escura na base do tronco
pub const STRIPPED_DARK_OAK_LOG: Block = Block::new(20); // Variação de casca lisa
pub const BROWN_MUSHROOM_BLOCK: Block = Block::new(99);

pub const YELLOW_TERRACOTTA: Block = Block::new(159); // Ipê Amarelo
pub const PINK_TERRACOTTA: Block = Block::new(159);   // Ipê Roxo
pub const WHITE_TERRACOTTA: Block = Block::new(159);  // Ipê Branco
pub const ORANGE_TERRACOTTA: Block = Block::new(159); // Flamboyant
pub const YELLOW_CARPET: Block = Block::new(171); // Folhas caídas de Ipê Amarelo

// =====================================================
// ESCALAS E RUÍDO (CERRADO PROCEDURAL)
// =====================================================
const HORIZONTAL_SCALE: f64 = 1.0 / 33.0;
const VERTICAL_SCALE: f64 = 1.0 / 15.0;
const GOV_V_SCALE: f64 = 1.15; // Rigor Vertical Governamental
const GOV_H_SCALE: f64 = 1.33; // ?? BESM-6: Distorção Horizontal Anisotrópica

static NOISE_TERRAIN: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(4242));
static NOISE_DENSITY: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(8888));
static NOISE_SPECIES: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(1919));
static NOISE_JITTER: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(7777));
static NOISE_BARK: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(3131));

// =====================================================
// ?? BESM-6: VOXELIZAÇÃO LOCAL DETERMINÍSTICA
// Matrizes de copa calculadas em tempo de execução via equações 
// de elipsoide, respeitando matematicamente o GOV_H_SCALE e GOV_V_SCALE.
// =====================================================

struct CanopyPatterns {
    round_1: Vec<Coord>,
    round_2: Vec<Coord>,
    round_3: Vec<Coord>,
}

static CANOPY_CACHE: Lazy<CanopyPatterns> = Lazy::new(|| {
    let mut build_round = |r: f64| -> Vec<Coord> {
        let mut coords = Vec::new();
        // Os raios base originais expandidos pela matriz afim
        let rx = (r * GOV_H_SCALE).round() as i32;
        let rz = r.round() as i32; 

        for x in -rx..=rx {
            for z in -rz..=rz {
                // Equação da elipse: (x^2 / a^2) + (z^2 / b^2) <= 1
                let a = rx as f64 + 0.5; // +0.5 suaviza a quantização Voxel
                let b = rz as f64 + 0.5;
                if (x as f64 * x as f64) / (a * a) + (z as f64 * z as f64) / (b * b) <= 1.0 {
                    coords.push((x, 0, z));
                }
            }
        }
        coords
    };

    CanopyPatterns {
        round_1: build_round(2.0),
        round_2: build_round(3.0),
        round_3: build_round(4.0),
    }
});

// Volumes de preenchimento do caule central
const OAK_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 9, 0)), ((1, 3, 0), (1, 9, 0)),
    ((0, 3, -1), (0, 9, -1)), ((0, 3, 1), (0, 9, 1)),
    ((0, 9, 0), (0, 10, 0)),
];

const DARK_OAK_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 6, 0)), ((1, 3, 0), (1, 6, 0)),
    ((0, 3, -1), (0, 6, -1)), ((0, 3, 1), (0, 6, 1)),
    ((0, 6, 0), (0, 7, 0)),
];

const ACACIA_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 5, 0), (-1, 8, 0)), ((1, 5, 0), (1, 8, 0)),
    ((0, 5, -1), (0, 8, -1)), ((0, 5, 1), (0, 8, 1)),
    ((0, 8, 0), (0, 9, 0)),
];

// =====================================================
// LÓGICA DE ÁRVORE INDIVIDUAL E ORGÂNICA
// =====================================================

fn round(editor: &mut WorldEditor, material: Block, (x, y, z): Coord, block_pattern: &[Coord], blacklist: &[Block]) {
    for (i, j, k) in block_pattern {
        let px = x + i;
        let py = y + j;
        let pz = z + k;
        
        // ?? Soft Canopy Check: Apenas planta a folha se não invadir os prédios
        if !editor.check_for_block_absolute(px, py, pz, Some(blacklist), None) {
            editor.set_block_absolute(material, px, py, pz, None, None);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum TreeType {
    Oak,     
    DarkOak, 
    Acacia,  
    // Espécies catalogadas da NOVACAP / DF
    IpeAmarelo,
    IpeRoxo,
    IpeBranco,
    Buriti,   // Palmeira de Vereda
    Sucupira, // Tronco muito retorcido
    Copaiba,  // Copa grande e arredondada
}

pub struct Tree<'a> {
    log_block: Block,
    twig_block: Block, // Fences simulando galhos
    log_height: i32,
    leaves_block: Block,
    leaves_fill: &'a [(Coord, Coord)],
    round_ranges: [Vec<i32>; 3],
}

impl Tree<'_> {
    /// ??? BESM-6: Interceptor de Dados Reais do Provedor (NOVACAP CSV)
    pub fn create_from_provider(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        tags: &HashMap<String, String>,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        let species = tags.get("species").or_else(|| tags.get("csv:especie")).map(|s| s.to_lowercase()).unwrap_or_default();
        
        let tree_type = if species.contains("ipê amarelo") || species.contains("tabebuia alba") || species.contains("handroanthus albus") {
            TreeType::IpeAmarelo
        } else if species.contains("ipê roxo") || species.contains("ipê rosa") || species.contains("handroanthus impetiginosus") {
            TreeType::IpeRoxo
        } else if species.contains("ipê branco") || species.contains("tabebuia roseoalba") {
            TreeType::IpeBranco
        } else if species.contains("buriti") || species.contains("mauritia flexuosa") {
            TreeType::Buriti
        } else if species.contains("sucupira") || species.contains("pterodon") {
            TreeType::Sucupira
        } else if species.contains("copaíba") || species.contains("copaifera") {
            TreeType::Copaiba
        } else if species.contains("flamboyant") {
            TreeType::Acacia 
        } else {
            let mut rng = coord_rng(x, z, 0);
            match rng.random_range(1..=10) {
                1..=4 => TreeType::Oak,
                5..=7 => TreeType::DarkOak,
                8..=10 => TreeType::Acacia,
                _ => unreachable!(),
            }
        };

        let height_override = tags.get("height").and_then(|h| h.trim_end_matches('m').trim().parse::<f64>().ok());
        Self::create_of_type_with_height(editor, (x, y, z), tree_type, height_override, building_footprints);
    }

    pub fn create(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        let mut rng = coord_rng(x, z, 0);
        let tree_type = match rng.random_range(1..=10) {
            1..=4 => TreeType::Oak,
            5..=7 => TreeType::DarkOak,
            8..=10 => TreeType::Acacia,
            _ => unreachable!(),
        };
        Self::create_of_type_with_height(editor, (x, y, z), tree_type, None, building_footprints);
    }

    pub fn create_of_type(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        tree_type: TreeType,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        Self::create_of_type_with_height(editor, (x, y, z), tree_type, None, building_footprints);
    }

    pub fn create_of_type_with_height(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        tree_type: TreeType,
        height_override: Option<f64>,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        if let Some(footprints) = building_footprints {
            if footprints.contains(x, z) {
                return;
            }
        }

        let ground_y = if editor.get_ground().is_some() { editor.get_ground_level(x, z) } else { y };
        
        let mut blacklist: Vec<Block> = Vec::new();
        blacklist.extend(Self::get_building_wall_blocks());
        blacklist.extend(Self::get_building_floor_blocks());
        blacklist.extend(Self::get_structural_blocks());
        blacklist.extend(Self::get_functional_blocks());
        blacklist.push(WATER); 
        blacklist.push(BLACK_CONCRETE); 
        blacklist.push(YELLOW_CONCRETE);
        blacklist.push(WHITE_CONCRETE);
        blacklist.push(RED_CONCRETE);
        blacklist.push(POLISHED_BASALT); 

        // Se a semente principal bateu no prédio, cancela.
        if editor.check_for_block_absolute(x, ground_y, z, Some(&blacklist), None) {
            return;
        }

        let mut tree = Self::get_tree(tree_type);
        let mut rng = coord_rng(x, z, 0);

        if let Some(h) = height_override {
            tree.log_height = (h * GOV_V_SCALE).round() as i32;
        } else {
            tree.log_height = (tree.log_height as f64 * GOV_V_SCALE).round() as i32;
            tree.log_height += rng.random_range(-2..=2);
        }

        let is_old_tree = tree.log_height > 10;
        let is_twisted = tree_type == TreeType::Sucupira || tree_type == TreeType::Acacia;

        if tree_type == TreeType::Buriti {
            editor.fill_column_absolute(JUNGLE_LOG, x, z, ground_y, ground_y + tree.log_height, true);
            for dy in -2..=1 {
                for dx in -2..=2 {
                    for dz in -2..=2 {
                        if dx.abs() + dz.abs() <= 3 && dy != -2 {
                            if !editor.check_for_block_absolute(x + dx, ground_y + tree.log_height + dy, z + dz, Some(&blacklist), None) {
                                editor.set_block_absolute(JUNGLE_LEAVES, x + dx, ground_y + tree.log_height + dy, z + dz, None, None);
                            }
                        } else if dx.abs() + dz.abs() == 1 && dy == -2 {
                            if !editor.check_for_block_absolute(x + dx, ground_y + tree.log_height + dy, z + dz, Some(&blacklist), None) {
                                editor.set_block_absolute(JUNGLE_LEAVES, x + dx, ground_y + tree.log_height + dy, z + dz, None, None);
                            }
                        }
                    }
                }
            }
            return;
        }

        // ?? BESM-6 TWEAK: Center of Mass Shift (Simulação Rápida de Fototropismo O(1))
        // Verifica se há paredes nos eixos cardeais num raio de 4 blocos na altura H=2.
        // Se houver, a árvore tomba todo o seu esqueleto pro lado oposto, tirando
        // a copa da colisão estúpida.
        let mut drift_offset_x = 0;
        let mut drift_offset_z = 0;
        
        let check_h = ground_y + 2;
        if editor.check_for_block_absolute(x + 4, check_h, z, Some(&blacklist), None) { drift_offset_x = -1; }
        if editor.check_for_block_absolute(x - 4, check_h, z, Some(&blacklist), None) { drift_offset_x = 1; }
        if editor.check_for_block_absolute(x, check_h, z + 4, Some(&blacklist), None) { drift_offset_z = -1; }
        if editor.check_for_block_absolute(x, check_h, z - 4, Some(&blacklist), None) { drift_offset_z = 1; }

        let root_radius = if is_old_tree { 2 } else { 1 };
        
        for dy in -2..=1 {
            for dx in -root_radius..=root_radius {
                for dz in -root_radius..=root_radius {
                    if dx == 0 && dz == 0 { continue; } 

                    let dist = dx.abs() + dz.abs();
                    let radius_check = if dy < 0 { root_radius + 1 } else { root_radius };

                    if dist <= radius_check && rng.gen_bool(0.65) {
                        let rx = x + dx;
                        let rz = z + dz;
                        let ry = ground_y + dy;

                        let root_block = if dy >= 0 && rng.gen_bool(0.3) { POLISHED_BASALT } else { tree.log_block };

                        if dy >= 0 {
                            if !editor.check_for_block_absolute(rx, ry, rz, Some(&blacklist), None) {
                                editor.set_block_absolute(root_block, rx, ry, rz, None, None);
                            }
                        } else {
                            editor.set_block_absolute(root_block, rx, ry, rz, Some(&[DIRT, GRASS_BLOCK, PODZOL, COARSE_DIRT, STONE, GRAVEL]), None);
                        }
                    }
                }
            }
        }

        let mut current_x = x;
        let mut current_z = z;

        for ty in 0..tree.log_height {
            let wy = ground_y + ty;
            
            // Entorta levemente pela Sucupira/Acacia
            if is_twisted && ty > 2 && ty < tree.log_height - 2 {
                let drift_n = NOISE_JITTER.get([x as f64 * 0.1, wy as f64 * 0.3, z as f64 * 0.1]);
                if drift_n > 0.4 { current_x += 1; } 
                else if drift_n < -0.4 { current_x -= 1; }

                let drift_nz = NOISE_JITTER.get([z as f64 * 0.1, wy as f64 * 0.3, x as f64 * 0.1]);
                if drift_nz > 0.4 { current_z += 1; } 
                else if drift_nz < -0.4 { current_z -= 1; }
            }

            // Aplica o Puxão de Gravidade Arquitetônico (Tira do Prédio)
            if ty > 1 {
                current_x += drift_offset_x;
                current_z += drift_offset_z;
            }
            
            let bark_block = if is_old_tree && ty < 4 && rng.gen_bool(0.2) { STRIPPED_DARK_OAK_LOG } else { tree.log_block };
            editor.set_block_if_absent_absolute(bark_block, current_x, wy, current_z);
            
            if is_old_tree && ty < 3 {
                let support_b = if rng.gen_bool(0.3) { POLISHED_BASALT } else { tree.log_block };
                if rng.gen_bool(0.5) && !editor.check_for_block_absolute(current_x+1, wy, current_z, Some(&blacklist), None) { editor.set_block_if_absent_absolute(support_b, current_x+1, wy, current_z); }
                if rng.gen_bool(0.5) && !editor.check_for_block_absolute(current_x-1, wy, current_z, Some(&blacklist), None) { editor.set_block_if_absent_absolute(support_b, current_x-1, wy, current_z); }
                if rng.gen_bool(0.5) && !editor.check_for_block_absolute(current_x, wy, current_z+1, Some(&blacklist), None) { editor.set_block_if_absent_absolute(support_b, current_x, wy, current_z+1); }
                if rng.gen_bool(0.5) && !editor.check_for_block_absolute(current_x, wy, current_z-1, Some(&blacklist), None) { editor.set_block_if_absent_absolute(support_b, current_x, wy, current_z-1); }
            }
        }

        // --- ?? BESM-6: L-SYSTEM COM RAYCASTING LOCAL DE OCLUSÃO ---
        if tree.log_height > 6 {
            let branch_count = rng.random_range(2..=5);
            for _ in 0..branch_count {
                let branch_y = ground_y + rng.random_range((tree.log_height / 3)..tree.log_height - 1);
                let mut angle = rng.random_range(0..360) as f64 * PI / 180.0;
                let branch_len = rng.random_range(2..=5);
                
                let mut bx = current_x;
                let mut bz = current_z;
                let mut by = branch_y;
                let mut path_blocked = false;

                for step in 1..=branch_len {
                    // Cálculo do vetor (aplicando distorção local)
                    let next_bx = current_x + (angle.cos() * step as f64 * GOV_H_SCALE).round() as i32;
                    let next_bz = current_z + (angle.sin() * step as f64).round() as i32;
                    let next_by = branch_y + (step / 2); 
                    
                    // Raycast O(1): O galho vai bater na parede?
                    if editor.check_for_block_absolute(next_bx, next_by, next_bz, Some(&blacklist), None) {
                        // Poda orgânica: Obstáculo detectado. Aborta crescimento principal,
                        // inverte o ângulo em 180º para o lado escancarado.
                        path_blocked = true;
                        angle += PI; 
                        break; 
                    }

                    bx = next_bx;
                    bz = next_bz;
                    by = next_by;
                    
                    editor.set_block_absolute(tree.log_block, bx, by, bz, None, None);

                    // Bifurcação secundária 
                    if step >= branch_len - 1 || path_blocked {
                        let force_split = path_blocked;
                        if force_split || rng.gen_bool(0.5) {
                            let sub_angle = angle + (rng.random_range(30..60) as f64 * PI / 180.0) * if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
                            for sub_step in 1..=2 {
                                let sbx = bx + (sub_angle.cos() * sub_step as f64 * GOV_H_SCALE).round() as i32;
                                let sbz = bz + (sub_angle.sin() * sub_step as f64).round() as i32;
                                
                                if !editor.check_for_block_absolute(sbx, by + sub_step, sbz, Some(&blacklist), None) {
                                    editor.set_block_absolute(tree.twig_block, sbx, by + sub_step, sbz, None, None);
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        let is_ipe = tree_type == TreeType::IpeAmarelo || tree_type == TreeType::IpeRoxo || tree_type == TreeType::IpeBranco;

        // --- GERAÇÃO DA COPA (DESLOCADA DO PRÉDIO) ---
        for ((i1, j1, k1), (i2, j2, k2)) in tree.leaves_fill {
            let start_y = (*j1 as f64 * GOV_V_SCALE).round() as i32;
            let end_y = (*j2 as f64 * GOV_V_SCALE).round() as i32;
            
            for ly in start_y..=end_y {
                let leaf_b = if is_ipe && rng.gen_bool(0.85) { tree.leaves_block } else if is_ipe { AIR } else { tree.leaves_block };
                if leaf_b != AIR {
                    let dist_x = (*i1 as f64 * GOV_H_SCALE).round() as i32;
                    let px = current_x + dist_x;
                    let py = ground_y + ly;
                    let pz = current_z + k1;
                    
                    if !editor.check_for_block_absolute(px, py, pz, Some(&blacklist), None) {
                        editor.set_block_if_absent_absolute(leaf_b, px, py, pz);
                    }
                }
            }
        }

        let canopy = &*CANOPY_CACHE;
        let patterns: [&[Coord]; 3] = [&canopy.round_1, &canopy.round_2, &canopy.round_3];

        for (round_range, round_pattern) in tree.round_ranges.iter().zip(patterns) {
            for offset in round_range {
                let y_scaled = (*offset as f64 * GOV_V_SCALE).round() as i32;
                let leaf_b = if is_ipe && rng.gen_bool(0.85) { tree.leaves_block } else if is_ipe { AIR } else { tree.leaves_block };
                if leaf_b != AIR {
                    // Copa centralizada em current_x/current_z (que já entortaram pra longe do prédio)
                    round(editor, leaf_b, (current_x, ground_y + y_scaled, current_z), round_pattern, &blacklist);
                }
            }
        }

        // --- EFEITOS DE CHÃO (FOLHAS CAÍDAS DE IPÊ) ---
        if tree_type == TreeType::IpeAmarelo && rng.gen_bool(0.6) {
            for lx in -4..=4 { 
                for lz in -3..=3 {
                    if lx.abs() + lz.abs() <= 5 && rng.gen_bool(0.4) {
                        let fx = current_x + lx;
                        let fz = current_z + lz;
                        let fy = if editor.get_ground().is_some() { editor.get_ground_level(fx, fz) } else { ground_y };
                        
                        if editor.check_for_block_absolute(fx, fy, fz, Some(&[GRASS_BLOCK, DIRT, PODZOL]), None) &&
                           editor.check_for_block_absolute(fx, fy + 1, fz, Some(&[AIR]), None) {
                            editor.set_block_absolute(YELLOW_CARPET, fx, fy + 1, fz, None, None);
                        }
                    }
                }
            }
        }
    }

    fn get_tree(kind: TreeType) -> Self {
        match kind {
            TreeType::Oak => Self {
                log_block: OAK_LOG,
                twig_block: OAK_FENCE,
                log_height: 8,
                leaves_block: OAK_LEAVES,
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
            },
            TreeType::DarkOak => Self {
                log_block: DARK_OAK_LOG,
                twig_block: DARK_OAK_FENCE,
                log_height: 5,
                leaves_block: DARK_OAK_LEAVES,
                leaves_fill: &DARK_OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=6).rev().collect(),
                    (3..=5).rev().collect(),
                    (4..=5).rev().collect(),
                ],
            },
            TreeType::Acacia | TreeType::Sucupira => Self {
                log_block: ACACIA_LOG,
                twig_block: ACACIA_FENCE,
                log_height: 6,
                leaves_block: ACACIA_LEAVES,
                leaves_fill: &ACACIA_LEAVES_FILL,
                round_ranges: [
                    (5..=8).rev().collect(),
                    (5..=7).rev().collect(),
                    (6..=7).rev().collect(),
                ],
            },
            TreeType::Copaiba => Self {
                log_block: DARK_OAK_LOG,
                twig_block: DARK_OAK_FENCE,
                log_height: 10,
                leaves_block: OAK_LEAVES, 
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (4..=9).rev().collect(),
                    (5..=8).rev().collect(),
                    (6..=7).rev().collect(),
                ],
            },
            TreeType::IpeAmarelo => Self {
                log_block: SPRUCE_LOG,
                twig_block: SPRUCE_FENCE,
                log_height: 9,
                leaves_block: YELLOW_TERRACOTTA,
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
            },
            TreeType::IpeRoxo => Self {
                log_block: SPRUCE_LOG,
                twig_block: SPRUCE_FENCE,
                log_height: 9,
                leaves_block: PINK_TERRACOTTA,
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
            },
            TreeType::IpeBranco => Self {
                log_block: SPRUCE_LOG,
                twig_block: SPRUCE_FENCE,
                log_height: 8,
                leaves_block: WHITE_TERRACOTTA,
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
            },
            TreeType::Buriti => Self {
                log_block: JUNGLE_LOG,
                twig_block: JUNGLE_FENCE,
                log_height: 12,
                leaves_block: JUNGLE_LEAVES,
                leaves_fill: &[],
                round_ranges: [vec![], vec![], vec![]],
            },
        }
    }

    fn get_building_wall_blocks() -> Vec<Block> {
        vec![
            BLACKSTONE, BLACK_TERRACOTTA, BRICK, BROWN_CONCRETE, BROWN_TERRACOTTA,
            DEEPSLATE_BRICKS, END_STONE_BRICKS, GRAY_CONCRETE, GRAY_TERRACOTTA,
            LIGHT_BLUE_TERRACOTTA, LIGHT_GRAY_CONCRETE, MUD_BRICKS, NETHER_BRICK,
            NETHERITE_BLOCK, POLISHED_ANDESITE, POLISHED_BLACKSTONE,
            POLISHED_BLACKSTONE_BRICKS, POLISHED_DEEPSLATE, POLISHED_GRANITE,
            QUARTZ_BLOCK, QUARTZ_BRICKS, SANDSTONE, SMOOTH_SANDSTONE, SMOOTH_STONE,
            STONE_BRICKS, WHITE_CONCRETE, WHITE_TERRACOTTA, ORANGE_TERRACOTTA,
            BLUE_TERRACOTTA, YELLOW_TERRACOTTA,
            BLACK_CONCRETE, GRAY_CONCRETE, RED_CONCRETE, LIME_CONCRETE,
            CYAN_CONCRETE, LIGHT_BLUE_CONCRETE, BLUE_CONCRETE, PURPLE_CONCRETE,
            MAGENTA_CONCRETE, RED_TERRACOTTA,
        ]
    }

    fn get_building_floor_blocks() -> Vec<Block> {
        vec![GRAY_CONCRETE, LIGHT_GRAY_CONCRETE, WHITE_CONCRETE, SMOOTH_STONE, POLISHED_ANDESITE, STONE_BRICKS]
    }

    fn get_structural_blocks() -> Vec<Block> {
        vec![
            OAK_FENCE, COBBLESTONE_WALL, ANDESITE_WALL, STONE_BRICK_WALL, OAK_STAIRS,
            OAK_SLAB, STONE_BLOCK_SLAB, STONE_BRICK_SLAB, RAIL, RAIL_NORTH_SOUTH,
            RAIL_EAST_WEST, OAK_DOOR, OAK_TRAPDOOR, LADDER,
        ]
    }

    fn get_functional_blocks() -> Vec<Block> {
        vec![
            CHEST, CRAFTING_TABLE, FURNACE, ANVIL, BREWING_STAND, NOTE_BLOCK,
            BOOKSHELF, CAULDRON, SIGN, BEDROCK, IRON_BARS, IRON_BLOCK, SCAFFOLDING,
            GLASS, WHITE_STAINED_GLASS, GRAY_STAINED_GLASS, LIGHT_GRAY_STAINED_GLASS,
            BROWN_STAINED_GLASS, CYAN_STAINED_GLASS, BLUE_STAINED_GLASS,
            LIGHT_BLUE_STAINED_GLASS, TINTED_GLASS, WHITE_CARPET, RED_CARPET,
        ]
    }
}

// =====================================================
// GERAÇÃO PROCEDURAL DE FLORESTAS E TRONCOS CAÍDOS
// =====================================================

pub fn generate_chunk(
    chunk_x: i32,
    chunk_z: i32,
    building_footprints: Option<&BuildingFootprintBitmap>,
    editor: &mut WorldEditor,
) {
    let mut tree_positions: Vec<(i32, i32)> = Vec::new();
    let chunk_seed = (chunk_x as u64).wrapping_mul(2654435761) ^ (chunk_z as u64).wrapping_mul(2246822519);
    let mut chunk_rng = SmallRng::seed_from_u64(chunk_seed);

    // Tronco Caído Gigante (Dead Log do Cerrado)
    if chunk_rng.gen_bool(0.02) {
        generate_fallen_log(chunk_x, chunk_z, editor, &mut chunk_rng, building_footprints);
    }

    for lx in 0..16 {
        for lz in 0..16 {
            let wx = chunk_x * 16 + lx;
            let wz = chunk_z * 16 + lz;

            if let Some(footprints) = building_footprints {
                if footprints.contains(wx, wz) { continue; }
            }

            // Escala o Ruído de Perlin para a grade distorcida
            let sx = wx as f64 * HORIZONTAL_SCALE * (1.0 / GOV_H_SCALE);
            let sz = wz as f64 * HORIZONTAL_SCALE;

            let topo = NOISE_TERRAIN.get([sx * 0.8, sz * 0.8]);
            let moisture = ((-topo + 0.5).clamp(0.0, 1.0));
            
            let base_height = if editor.get_ground().is_some() {
                editor.get_ground_level(wx, wz)
            } else {
                (topo * 40.0 * VERTICAL_SCALE) as i32 + 70
            };

            let density = NOISE_DENSITY.get([sx * 0.6, sz * 0.6]);
            let spawn_threshold = 0.25 - (moisture * 0.4);

            if density < spawn_threshold {
                generate_undergrowth(wx, base_height, wz, moisture, editor);
                continue;
            }

            let seed = (wx as u64).wrapping_mul(2654435761) ^ (wz as u64).wrapping_mul(2246822519);
            let mut rng = SmallRng::seed_from_u64(seed);

            let jitter_x = (NOISE_JITTER.get([sx * 3.0, sz * 3.0]) * 1.5).round() as i32;
            let jitter_z = (NOISE_JITTER.get([sz * 3.0, sx * 3.0]) * 1.5).round() as i32;
            
            let safe_lx = (lx + jitter_x).clamp(0, 15);
            let safe_lz = (lz + jitter_z).clamp(0, 15);
            
            let tx = chunk_x * 16 + safe_lx;
            let tz = chunk_z * 16 + safe_lz;

            // Root Kiss Fix: Raio mínimo entre árvores orgânicas
            let mut too_close = false;
            for &(px, pz) in &tree_positions {
                let dx = tx - px;
                let dz = tz - pz;
                if (dx * dx) + (dz * dz) < 16 {
                    too_close = true;
                    break;
                }
            }
            if too_close { continue; }
            tree_positions.push((tx, tz));

            let ground_check = editor.check_for_block_absolute(tx, base_height, tz, Some(&[WATER, BLACK_CONCRETE, WHITE_CONCRETE, YELLOW_CONCRETE, RED_CONCRETE, POLISHED_BASALT]), None);
            if ground_check { continue; }

            let species_roll = NOISE_SPECIES.get([sx * 1.5, sz * 1.5]);
            let tree_type = if moisture > 0.8 && species_roll > 0.7 {
                TreeType::Buriti 
            } else if species_roll > 0.5 {
                TreeType::Copaiba 
            } else if species_roll < -0.5 {
                TreeType::Sucupira 
            } else {
                TreeType::Acacia
            };

            Tree::create_of_type_with_height(editor, (tx, base_height, tz), tree_type, None, building_footprints);
        }
    }
}

/// Gera um tronco tombado e podre (Fallen Log) no chão do bioma
fn generate_fallen_log(
    chunk_x: i32, 
    chunk_z: i32, 
    editor: &mut WorldEditor, 
    rng: &mut SmallRng,
    building_footprints: Option<&BuildingFootprintBitmap>
) {
    let tx = chunk_x * 16 + rng.random_range(4..12);
    let tz = chunk_z * 16 + rng.random_range(4..12);

    if let Some(footprints) = building_footprints {
        if footprints.contains(tx, tz) { return; }
    }

    let ground_y = if editor.get_ground().is_some() {
        editor.get_ground_level(tx, tz)
    } else {
        return;
    };

    if editor.check_for_block_absolute(tx, ground_y, tz, Some(&[WATER, BLACK_CONCRETE, WHITE_CONCRETE, YELLOW_CONCRETE, RED_CONCRETE, POLISHED_BASALT]), None) {
        return;
    }

    let len = rng.random_range(5..=12);
    let angle = rng.random_range(0..360) as f64 * PI / 180.0;
    
    let is_dark_oak = rng.gen_bool(0.5);
    let log_type = if is_dark_oak { DARK_OAK_LOG } else { OAK_LOG };

    for step in 0..len {
        let lx = tx + (angle.cos() * step as f64 * GOV_H_SCALE).round() as i32; // Distorcido horizontalmente
        let lz = tz + (angle.sin() * step as f64).round() as i32;
        let ly = if editor.get_ground().is_some() { editor.get_ground_level(lx, lz) + 1 } else { ground_y + 1 };

        if let Some(footprints) = building_footprints {
            if footprints.contains(lx, lz) { continue; }
        }

        // Casca podre na base / textura
        let final_block = if rng.gen_bool(0.2) { POLISHED_BASALT } else { log_type };
        editor.set_block_if_absent_absolute(final_block, lx, ly, lz);

        // Musgo ou cogumelos espalhados pelo tronco
        if rng.gen_bool(0.4) {
            editor.set_block_absolute(MOSS_CARPET, lx, ly + 1, lz, Some(&[AIR]), None);
        } else if rng.gen_bool(0.15) {
            editor.set_block_absolute(BROWN_MUSHROOM_BLOCK, lx, ly + 1, lz, Some(&[AIR]), None);
        }
    }
}

fn generate_undergrowth(x: i32, y: i32, z: i32, moisture: f64, editor: &mut WorldEditor) {
    let mut rng = SmallRng::seed_from_u64((x as u64).wrapping_add(z as u64));
    let wy = y + 1;

    if moisture < 0.4 {
        if rng.gen_bool(0.15) { editor.set_block_if_absent_absolute(SHORT_GRASS, x, wy, z); }
        if rng.gen_bool(0.1) { editor.set_block_if_absent_absolute(MOSS_CARPET, x, wy, z); }
    } else {
        if rng.gen_bool(0.12) { editor.set_block_if_absent_absolute(FLOWERING_AZALEA, x, wy, z); }
        // Pequenos arbustos e mato seco
        if moisture > 0.6 && rng.gen_bool(0.3) { 
            editor.set_block_if_absent_absolute(DEAD_BUSH, x, wy, z); 
        }
    }
}