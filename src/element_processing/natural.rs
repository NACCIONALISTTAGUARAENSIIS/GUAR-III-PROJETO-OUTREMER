use crate::args::Args;
use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::element_processing::tree::{Tree, TreeType}; // Atualizado para chamar o seu trees.rs governamental
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;

// 🚨 Importações Específicas do Cerrado (Corrigindo o Erro E0425)
use crate::element_processing::tree::{SHORT_GRASS, MOSS_CARPET, AZALEA_LEAVES, FLOWERING_AZALEA};

use rand::{prelude::IndexedRandom, Rng};
use std::collections::HashSet;
use std::sync::Arc;

// 🚨 BESM-6: Motor Biológico de Distribuição Espacial (Perlin Fake)
#[inline(always)]
fn organic_density_noise(x: i32, z: i32, scale: f64) -> f64 {
    let xf = x as f64 * scale;
    let zf = z as f64 * scale;
    ((xf.sin() * zf.cos()) + (xf * 0.5 + zf * 0.3).sin() * 0.5).abs() / 1.5
}

// 🚨 BESM-6: Topografia Biométrica (Gradiente 2D & Busca de Lençol Freático)
// O Cerrado possui declives e veios d'água que determinam a flora.
#[inline]
fn determine_cerrado_biome(x: i32, z: i32, ground_y: i32, editor: &WorldEditor) -> &'static str {
    // 1. Busca de água (Octogonal expandida para Veredas reais)
    // Usamos um raio maior para detectar corredores fluviais (Mata de Galeria)
    let water_radius = 8;
    let has_water =
        editor.check_for_block_absolute(x + water_radius, ground_y, z, Some(&[WATER]), None) ||
            editor.check_for_block_absolute(x - water_radius, ground_y, z, Some(&[WATER]), None) ||
            editor.check_for_block_absolute(x, ground_y, z + water_radius, Some(&[WATER]), None) ||
            editor.check_for_block_absolute(x, ground_y, z - water_radius, Some(&[WATER]), None) ||
            editor.check_for_block_absolute(x + 4, ground_y, z + 4, Some(&[WATER]), None) ||
            editor.check_for_block_absolute(x - 4, ground_y, z - 4, Some(&[WATER]), None);

    if has_water {
        return "mata_galeria";
    }

    // 2. Calcula Slope 2D (Gradiente Topográfico) para detectar Campos Rupestres
    // O código legado olhava apenas Norte-Sul. Agora olhamos os eixos cardeais.
    let y_north = editor.get_ground_level(x, z - 4);
    let y_south = editor.get_ground_level(x, z + 4);
    let y_east = editor.get_ground_level(x + 4, z);
    let y_west = editor.get_ground_level(x - 4, z);

    let slope_z = (y_north - y_south).abs();
    let slope_x = (y_east - y_west).abs();

    // Gradiente resultante
    let total_slope = ((slope_x * slope_x + slope_z * slope_z) as f64).sqrt();

    // Se o morro cai mais de 3.5 blocos em 8 metros (Alta inclinação)
    if total_slope > 3.5 {
        "campo_rupestre" // Morros pedregosos e secos
    } else {
        "cerrado_ss" // Cerrado Stricto Sensu (Típico e Plano)
    }
}

pub fn generate_natural(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    exclusion_mask: Option<&HashSet<(i32, i32)>>, // 🚨 BESM-6: O Veto Booleano (Inners)
) {
    if let Some(natural_type) = element.tags().get("natural") {
        if natural_type == "tree" {
            if let ProcessedElement::Node(node) = element {
                let x: i32 = node.x;
                let z: i32 = node.z;

                // Bloqueia a árvore se ela caiu exatamente dentro de um lago interno
                if let Some(mask) = exclusion_mask {
                    if mask.contains(&(x, z)) { return; }
                }

                let mut trees_ok_to_generate: Vec<TreeType> = vec![];
                if let Some(species) = element.tags().get("species") {
                    if species.contains("Ipê") || species.contains("Handroanthus") { trees_ok_to_generate.push(TreeType::IpeAmarelo); }
                    if species.contains("Copaíba") { trees_ok_to_generate.push(TreeType::Copaiba); }
                } else {
                    // Expurgagem de Espécies Exóticas, Força Ipê e Sucupira
                    trees_ok_to_generate.push(TreeType::Sucupira);
                    trees_ok_to_generate.push(TreeType::IpeAmarelo);
                }

                if trees_ok_to_generate.is_empty() {
                    trees_ok_to_generate.push(TreeType::Acacia);
                }

                let mut rng = element_rng(element.id());
                let tree_type = *trees_ok_to_generate.choose(&mut rng).unwrap_or(&TreeType::Acacia);

                // BESM-6 Tweak: GROUND AWARE (Árvores não voam nem ficam soterradas)
                let ground_y = editor.get_ground_level(x, z);

                // Protege estradas e águas de receberem árvores se o OSM errar 1 metro
                if !editor.check_for_block_absolute(x, ground_y, z, Some(&[BLACK_CONCRETE, POLISHED_BASALT, YELLOW_CONCRETE, RED_CONCRETE, WATER, POLISHED_ANDESITE]), None) {
                    Tree::create_of_type(editor, (x, ground_y + 1, z), tree_type, Some(building_footprints));
                }
            }
        } else {
            // BESM-6 Tweak: Correção na Inconsistência de Blocos orgânicos.
            let block_type: Block = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" | "heath" | "tree_row" => GRASS_BLOCK,
                "sand" | "dune" | "beach" | "shoal" => SAND,
                "water" | "reef" => WATER,
                "bare_rock" | "ridge" | "cliff" => STONE,
                "blockfield" | "mountain_range" | "saddle" => ANDESITE, // Rocha bruta
                "glacier" => PACKED_ICE,
                "mud" | "wetland" => MUD,
                "shrubbery" | "tundra" | "hill" => GRASS_BLOCK,
                _ => GRASS_BLOCK,
            };

            let ProcessedElement::Way(way) = element else { return; };

            let filled_area: Vec<(i32, i32)> = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
            if filled_area.is_empty() { return; }

            let trees_ok_to_generate: Vec<TreeType> = vec![TreeType::Sucupira, TreeType::Acacia];
            let mut rng = element_rng(way.id);

            // 🚨 BESM-6: Aplica Densidade de Vegetação baseada na Escala Híbrida (Fixado erro sintaxe)
            let scale_area_multiplier = args.scale_h * args.scale_h;
            let base_tree_chance = (6.0 / scale_area_multiplier).clamp(2.0, 10.0) as u32;

            // 🚨 PREENCHIMENTO DA ÁREA INTERNA (CORE E BORDAS via Scanline)
            for &(x, z) in &filled_area {
                // 🚨 O Veto Booleano (O Algo do Pintor Morreu Aqui)
                // Se a coordenada caiu num lago (Inner), o motor aborta instantaneamente.
                if let Some(mask) = exclusion_mask {
                    if mask.contains(&(x, z)) { continue; }
                }

                let ground_y = editor.get_ground_level(x, z);

                // Culling Híbrido: A natureza não sobrepõe a cidade. Se há concreto sob os pés, recua.
                if editor.check_for_block_absolute(x, ground_y, z, Some(&[BLACK_CONCRETE, POLISHED_BASALT, YELLOW_CONCRETE, RED_CONCRETE, WHITE_CONCRETE, POLISHED_ANDESITE]), None) {
                    continue;
                }

                let biome_class = determine_cerrado_biome(x, z, ground_y, editor);

                // Pedras e terras áridas nos morros (Campo Rupestre)
                let final_block = if biome_class == "campo_rupestre" && rng.random_range(0..100) < 40 {
                    if rng.random_bool(0.5) { COARSE_DIRT } else { GRAVEL }
                } else if block_type == ANDESITE && rng.random_range(0..100) < 30 {
                    if rng.random_bool(0.5) { STONE } else { GRAVEL }
                } else {
                    block_type
                };

                editor.set_block_absolute(final_block, x, ground_y, z, Some(&[GRASS_BLOCK, DIRT, PODZOL, COARSE_DIRT, STONE, GRAVEL]), None);

                // Pula decoração se for água pura ou gelo
                if block_type == WATER || block_type == PACKED_ICE {
                    continue;
                }

                // Noise Macro-Biológico para evitar florestas xadrez
                let bio_noise = organic_density_noise(x, z, 0.1);

                match natural_type.as_str() {
                    "grassland" | "heath" => {
                        if bio_noise > 0.7 {
                            editor.set_block_absolute(PODZOL, x, ground_y, z, Some(&[GRASS_BLOCK]), None);
                        }
                        if rng.random_range(0..100) < 40 {
                            editor.set_block_if_absent_absolute(SHORT_GRASS, x, ground_y + 1, z);
                        } else if rng.random_range(0..100) < 55 {
                            editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z); // Seca do Cerrado
                        }
                    }
                    "scrub" => {
                        if biome_class == "campo_rupestre" {
                            if rng.random_range(0..100) < 30 { editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z); }
                        } else {
                            if rng.random_range(0..100) < 8 { editor.set_block_absolute(COARSE_DIRT, x, ground_y, z, Some(&[GRASS_BLOCK]), None); }
                            else if rng.random_range(0..100) < 25 { editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z); }
                            else if rng.random_range(0..100) < 40 { editor.set_block_if_absent_absolute(ACACIA_LEAVES, x, ground_y + 1, z); }
                            else if rng.random_range(0..100) < 70 { editor.set_block_if_absent_absolute(SHORT_GRASS, x, ground_y + 1, z); }
                        }
                    }
                    "wood" | "tree_row" => {
                        // Matas de Galeria vs Cerrado Ralo
                        if biome_class == "mata_galeria" {
                            if bio_noise > 0.2 && rng.random_range(0..100) < (base_tree_chance * 3) {
                                let tree_type = if rng.random_bool(0.3) { TreeType::Buriti } else { TreeType::Copaiba };
                                Tree::create_of_type(editor, (x, ground_y + 1, z), tree_type, Some(building_footprints));
                            }
                        } else {
                            if bio_noise > 0.4 && rng.random_range(0..100) < (base_tree_chance * 2) {
                                let tree_type = *trees_ok_to_generate.choose(&mut rng).unwrap_or(&TreeType::Sucupira);
                                Tree::create_of_type(editor, (x, ground_y + 1, z), tree_type, Some(building_footprints));
                            } else if bio_noise < 0.2 {
                                editor.set_block_if_absent_absolute(TALL_GRASS, x, ground_y + 1, z);
                            }
                        }
                    }
                    "sand" | "shoal" => {
                        if rng.random_range(0..100) < 8 {
                            editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                        }
                    }
                    "wetland" => {
                        let base_block = if rng.random_bool(0.4) { MOSS_BLOCK } else { MUD };
                        editor.set_block_absolute(base_block, x, ground_y, z, Some(&[GRASS_BLOCK, MUD]), None);
                        if rng.random_bool(0.3) {
                            editor.set_block_absolute(WATER, x, ground_y, z, Some(&[MUD, MOSS_BLOCK]), None);
                        } else if rng.random_bool(0.6) {
                            editor.set_block_if_absent_absolute(SHORT_GRASS, x, ground_y + 1, z);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn generate_natural_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if rel.tags.contains_key("natural") {

        // 🚨 BESM-6: Geometria de Exclusão (Inner Polygons)
        // Extrai a matemática dos buracos (Lagos, clareiras, pedreiras) antes de pintar o mato.
        let mut exclusion_mask: HashSet<(i32, i32)> = HashSet::new();

        for member in &rel.members {
            if member.role == ProcessedMemberRole::Inner {
                let inner_filled = flood_fill_cache.get_or_compute(&member.way, args.timeout.as_ref());
                for coord in inner_filled {
                    exclusion_mask.insert(coord);
                }
            }
        }

        let exclusion_ref = if exclusion_mask.is_empty() { None } else { Some(&exclusion_mask) };

        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // 🚨 TWEAK: Usando Arc::new como requerido pela nossa nova assinatura do ProcessedElement
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_natural(
                    editor,
                    &ProcessedElement::Way(Arc::new(way_with_rel_tags)),
                    args,
                    flood_fill_cache,
                    building_footprints,
                    exclusion_ref, // Passa o Veto Booleano para o pintor do núcleo
                );
            }
        }
    }
}