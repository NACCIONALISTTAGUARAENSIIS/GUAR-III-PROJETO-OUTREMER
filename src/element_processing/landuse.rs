use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::prelude::IndexedRandom;
use crate::providers::vegetation_provider::{
    BIOME_NONE, BIOME_MATA_GALERIA, BIOME_CERRADAO, BIOME_CERRADO_SS,
    BIOME_CAMPO_SUJO, BIOME_VEREDA, BIOME_CAMPO_RUPESTRE, MASK_APP_SICAR
};
use rand::Rng;

pub fn generate_landuse(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element
        .tags
        .get("landuse")
        .unwrap_or_else(|| element.tags.get("leisure").unwrap_or(&binding));

    // Use deterministic RNG seeded by element ID for consistent results across region boundaries
    let mut rng = element_rng(element.id);

    // Block type base setup
    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" | "orchard" | "forest" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "cemetery" => PODZOL, // Restaurado o PODZOL base do cemitério
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB, // Restaurado o slab para ilhas de tráfego
        "residential" => {
            // Restaurada a diferenciação rural/urbano do original
            let residential_tag = element.tags.get("residential").unwrap_or(&binding);
            if residential_tag == "rural" {
                GRASS_BLOCK
            } else {
                GRASS_BLOCK // Base para lote de RP (Quintal), será randomizado depois
            }
        }
        "commercial" => POLISHED_ANDESITE,
        "education" | "religious" => SMOOTH_QUARTZ,
        "industrial" => GRAY_CONCRETE,
        "military" => GRAY_CONCRETE,
        "railway" => GRAVEL,
        "park" | "common" => GRASS_BLOCK, // For Portuguese Pavement logic
        "landfill" => {
            let manmade_tag = element.tags.get("man_made").unwrap_or(&binding);
            if manmade_tag == "spoil_heap" || manmade_tag == "heap" {
                GRAVEL
            } else {
                COARSE_DIRT
            }
        }
        "quarry" => STONE, // Restaurado o quarry
        _ => GRASS_BLOCK,
    };

    // Get the area of the landuse element using cache
    let floor_area: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    let trees_ok_to_generate: Vec<TreeType> = {
        let mut trees: Vec<TreeType> = vec![];
        if let Some(leaf_type) = element.tags.get("leaf_type") {
            match leaf_type.as_str() {
                "broadleaved" => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::Acacia);
                }
                "needleleaved" => trees.push(TreeType::Spruce),
                _ => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Spruce);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::Acacia);
                }
            }
        } else {
            // Dominância do DF
            trees.push(TreeType::Oak);
            trees.push(TreeType::Acacia);
        }
        trees
    };

    for (x, z) in floor_area {
        // Apply per-block randomness for certain landuse types
        let mut actual_block = block_type;

        if landuse_tag == "residential" && block_type != GRASS_BLOCK {
            // Se for urbano, gera um misto de calçamento de lote e terra
            let random_value = rng.random_range(0..100);
            actual_block = if random_value < 60 {
                GRASS_BLOCK // Gramado do quintal
            } else if random_value < 90 {
                DIRT_PATH // Caminhos de terra batida
            } else {
                COARSE_DIRT // Terra dura exposta (Latossolo)
            };
        } else if landuse_tag == "commercial" {
            // Calçamento Comercial (Sólido e transitável)
            let random_value = rng.random_range(0..100);
            actual_block = if random_value < 70 {
                POLISHED_ANDESITE
            } else {
                STONE_BRICKS
            };
        } else if landuse_tag == "industrial" {
            // Chão de Fábrica sujo / Pátio de manobras
            let random_value = rng.random_range(0..100);
            actual_block = if random_value < 60 {
                GRAY_CONCRETE
            } else if random_value < 90 {
                GRAVEL // Brita industrial
            } else {
                COBBLESTONE // Pedregulho sujo
            };
        } else if landuse_tag == "education" || landuse_tag == "religious" {
            // Identidade Institucional Brutalista Monumental
            let random_value = rng.random_range(0..100);
            actual_block = if random_value < 45 {
                SMOOTH_QUARTZ
            } else if random_value < 70 {
                LIGHT_GRAY_CONCRETE
            } else {
                WHITE_CONCRETE
            };
        } else if landuse_tag == "park" || landuse_tag == "common" {
            // Parque e Lazer (Portuguese Pavement - Calçadão ajustado para 20% para não poluir)
            if rng.random_range(0..100) < 20 {
                actual_block = if (x + z) % 2 == 0 {
                    BLACK_CONCRETE
                } else {
                    WHITE_CONCRETE
                };
            }
        } else if ["forest", "grass", "meadow", "greenfield"].contains(&landuse_tag.as_str()) {
            // Injeção de base de Cerrado Seco (Serapilheira) direto no solo
            if rng.random_range(0..100) < 15 {
                actual_block = PODZOL;
            }
        }

        // Lógica Restaurada para placement base de Ilhas de Trânsito, Obras e Ferrovias
        if landuse_tag == "traffic_island" {
            editor.set_block(actual_block, x, 1, z, None, None);
        } else if landuse_tag == "construction" || landuse_tag == "railway" {
            editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));
        } else {
            editor.set_block(actual_block, x, 0, z, None, None);
        }

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "cemetery" => {
                // Lógica de Tumbas do Arnis Original Restaurada e Otimizada
                if (x % 3 == 0) && (z % 3 == 0) {
                    let random_choice: i32 = rng.random_range(0..100);
                    if random_choice < 15 {
                        if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                            if rng.random_bool(0.5) {
                                editor.set_block(COBBLESTONE, x - 1, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x - 1, 2, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x + 1, 1, z, None, None);
                            } else {
                                editor.set_block(COBBLESTONE, x, 1, z - 1, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 2, z - 1, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z + 1, None, None);
                            }
                        }
                    } else if random_choice < 30 {
                        if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                            editor.set_block(RED_FLOWER, x, 1, z, None, None);
                        }
                    } else if random_choice < 33 {
                        Tree::create(editor, (x, 1, z), Some(building_footprints));
                    } else if random_choice < 35 {
                        editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                    } else if random_choice < 37 {
                        editor.set_block(FERN, x, 1, z, None, None);
                    } else if random_choice < 41 {
                        editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                        editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                    }
                }
            }
            "forest" => {
                // Ecossistema de Cerrado (Cerradão)
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK, PODZOL])) {
                    let random_choice: i32 = rng.random_range(0..100);
                    if random_choice < 6 {
                        let tree_type = *trees_ok_to_generate.choose(&mut rng).unwrap_or(&TreeType::Acacia);
                        Tree::create_of_type(editor, (x, 1, z), tree_type, Some(building_footprints));
                    } else if random_choice < 14 { // 8% chance de galhos (reduzido para não travar RP)
                        editor.set_block(DEAD_BUSH, x, 1, z, None, None);
                    } else if random_choice < 70 {
                        editor.set_block(GRASS, x, 1, z, None, None);
                    }
                    if rng.random_range(0..100) < 15 {
                        editor.set_block(MOSS_CARPET, x, 1, z, Some(&[AIR]), None);
                    }
                }
            }
            "grass" | "greenfield" => {
                // Ecossistema de Cerrado (Campo Sujo - RP transitável)
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK, PODZOL])) {
                    let random_choice: i32 = rng.random_range(0..100);
                    if random_choice < 5 {
                        editor.set_block(DEAD_BUSH, x, 1, z, None, None);
                    } else if random_choice < 70 {
                        editor.set_block(GRASS, x, 1, z, None, None);
                    }
                    if rng.random_range(0..100) < 15 {
                        editor.set_block(MOSS_CARPET, x, 1, z, Some(&[AIR]), None);
                    }
                }
            }
            "meadow" => {
                // Meadow: Variante campestre com flores (restaurada do original)
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK, PODZOL])) {
                    let random_choice: i32 = rng.random_range(0..1001);
                    if random_choice < 5 {
                        Tree::create(editor, (x, 1, z), Some(building_footprints));
                    } else if random_choice < 6 {
                        editor.set_block(RED_FLOWER, x, 1, z, None, None);
                    } else if random_choice < 9 {
                        editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                    } else if random_choice < 40 {
                        editor.set_block(FERN, x, 1, z, None, None);
                    } else if random_choice < 65 {
                        editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                        editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                    } else if random_choice < 825 {
                        editor.set_block(GRASS, x, 1, z, None, None);
                    }
                }
            }
            "orchard" => {
                // Lógica original de pomares restaurada
                if x % 18 == 0 && z % 10 == 0 {
                    Tree::create(editor, (x, 1, z), Some(building_footprints));
                } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK, PODZOL])) {
                    match rng.random_range(0..100) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                        1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                        3..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "farmland" => {
                // Restaurada a lógica completa de Fazendas (Água e Plantações)
                if !editor.check_for_block(x, 0, z, Some(&[WATER])) {
                    if x % 9 == 0 && z % 9 == 0 {
                        editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
                    } else if rng.random_range(0..76) == 0 {
                        let special_choice: i32 = rng.random_range(1..=10);
                        if special_choice <= 4 {
                            editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                        } else {
                            editor.set_block(OAK_LEAVES, x, 1, z, None, Some(&[SPONGE]));
                        }
                    } else {
                        if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                            let crop_choice = [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                            editor.set_block(crop_choice, x, 1, z, None, None);
                        }
                    }
                }
            }
            "construction" => {
                // Restaurada a gigante lógica de Obras do original (Guindastes, areia, blocos)
                let random_choice: i32 = rng.random_range(0..1501);
                if random_choice < 15 {
                    editor.set_block(SCAFFOLDING, x, 1, z, None, None);
                    if random_choice < 2 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                    } else if random_choice < 4 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 5, z, None, None);
                        editor.set_block(SCAFFOLDING, x - 1, 1, z, None, None);
                        editor.set_block(SCAFFOLDING, x + 1, 1, z - 1, None, None);
                    }
                } else if random_choice < 55 {
                    let construction_items: [Block; 13] = [
                        OAK_LOG, COBBLESTONE, GRAVEL, GLOWSTONE, STONE, COBBLESTONE_WALL,
                        BLACK_CONCRETE, SAND, OAK_PLANKS, DIRT, BRICK, CRAFTING_TABLE, FURNACE,
                    ];
                    editor.set_block(
                        construction_items[rng.random_range(0..construction_items.len())],
                        x, 1, z, None, None,
                    );
                } else if random_choice < 65 {
                    if random_choice < 60 {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x + 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x - 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z - 1, None, None);
                    }
                } else if random_choice < 100 {
                    editor.set_block(GRAVEL, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 115 {
                    editor.set_block(SAND, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 125 {
                    editor.set_block(DIORITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 145 {
                    editor.set_block(BRICK, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 155 {
                    editor.set_block(GRANITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 180 {
                    editor.set_block(ANDESITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 565 {
                    editor.set_block(COBBLESTONE, x, 0, z, None, Some(&[SPONGE]));
                }
            }
            "quarry" => {
                // Lógica de Pedreira/Mineração restaurada
                editor.set_block(STONE, x, -1, z, Some(&[STONE]), None);
                editor.set_block(STONE, x, -2, z, Some(&[STONE]), None);
                if let Some(resource) = element.tags.get("resource") {
                    let ore_block = match resource.as_str() {
                        "iron_ore" => IRON_ORE,
                        "coal" => COAL_ORE,
                        "copper" => COPPER_ORE,
                        "gold" => GOLD_ORE,
                        "clay" | "kaolinite" => CLAY,
                        _ => STONE,
                    };
                    let random_choice: i32 = rng.random_range(0..100 + editor.get_absolute_y(x, 0, z));
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, 0, z, Some(&[STONE]), None);
                    }
                }
            }
            _ => {}
        }
    }

    // Generate a stone brick wall fence around cemeteries (Restaurado)
    if landuse_tag == "cemetery" {
        generate_cemetery_fence(editor, element);
    }
}

/// Draws a stone-brick wall fence (with slab cap) along the outline of a cemetery way.
fn generate_cemetery_fence(editor: &mut WorldEditor, element: &ProcessedWay) {
    for i in 1..element.nodes.len() {
        let prev = &element.nodes[i - 1];
        let cur = &element.nodes[i];

        let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);
        for (bx, _, bz) in points {
            editor.set_block(STONE_BRICK_WALL, bx, 1, bz, None, None);
            editor.set_block(STONE_BRICK_SLAB, bx, 2, bz, None, None);
        }
    }
}

pub fn generate_landuse_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if rel.tags.contains_key("landuse") || rel.tags.contains_key("leisure") {
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_landuse(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            }
        }
    }
}

pub fn generate_place(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let binding = String::new();
    let place_tag = element.tags.get("place").unwrap_or(&binding);
    let block_type = match place_tag.as_str() {
        "square" => STONE_BRICKS,
        "neighbourhood" | "city_block" | "quarter" | "suburb" => POLISHED_ANDESITE, // Concreto urbano de Brasília
        _ => return,
    };
    let floor_area: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
    for (x, z) in floor_area {
        editor.set_block(block_type, x, 0, z, None, None);
    }
}