//! Processing of historic elements.
//!
//! This module handles historic OSM elements including:
//! - `historic=memorial` - Memorials, monuments, and commemorative structures

use crate::args::Args;
use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Generate historic structures from node elements
pub fn generate_historic(editor: &mut WorldEditor, node: &ProcessedNode) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = node.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = node.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(historic_type) = node.tags.get("historic") {
        match historic_type.as_str() {
            "memorial" => generate_memorial(editor, node),
            "monument" => generate_monument(editor, node),
            "wayside_cross" => generate_wayside_cross(editor, node),
            _ => {}
        }
    }
}

/// Generate a memorial structure (Ground Aware)
fn generate_memorial(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;
    let base_y = editor.get_ground_level(x, z);

    let mut rng = element_rng(node.id);

    let memorial_type = node
        .tags
        .get("memorial")
        .map(|s: &String| s.as_str())
        .unwrap_or("yes");

    match memorial_type {
        "plaque" => {
            // Placa em pedestal de mármore (Brasília style)
            editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + 1, z, None, None);
            editor.set_block_absolute(POLISHED_ANDESITE, x, base_y + 2, z, None, None);
        }
        "statue" | "sculpture" | "bust" => {
            // TWEAK: Esculturas Monumentais (Ex: Os Candangos / A Justiça)
            // Pedestal Monumental 3x3 para suportar a escala 1.33H
            for dx in -1i32..=1i32 {
                for dz in -1i32..=1i32 {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x + dx, base_y + 1, z + dz, None, None);
                }
            }
            editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + 2, z, None, None);

            // Material: Bronze ou Metal Nobre (Rigor Brasília)
            let statue_block = if rng.random_bool(0.6) {
                POLISHED_BASALT // Bronze Escuro / Pátina
            } else {
                POLISHED_ANDESITE // Aço Escovado
            };

            // Corpo da escultura (modernista) com escala 1.15V
            for y_off in 3i32..=6i32 {
                let wy = base_y + y_off;
                editor.set_block_absolute(statue_block, x, wy, z, None, None);
                if y_off == 5 {
                    // Projeção lateral (Abstracionismo de Niemeyer)
                    editor.set_block_absolute(ANDESITE_WALL, x + 1, wy, z, None, None);
                    editor.set_block_absolute(ANDESITE_WALL, x - 1, wy, z, None, None);
                }
            }
        }
        "stone" | "stolperstein" => {
            let stone_block = if memorial_type == "stolperstein" { GOLD_BLOCK } else { POLISHED_ANDESITE };
            editor.set_block_absolute(stone_block, x, base_y, z, None, None);
        }
        "cross" | "war_memorial" => {
            generate_monumental_cross(editor, x, z, base_y, 9);
        }
        "obelisk" => {
            // TWEAK: Mastros da Bandeira e Obeliscos do DF
            let is_flagpole = node.tags.contains_key("flag:type") ||
                node.tags.get("name").map(|s: &String| s.contains("Bandeira")).unwrap_or(false);

            let parsed_height = node
                .tags
                .get("height")
                .and_then(|h: &String| h.parse::<f64>().ok())
                .map(|h| (h * 1.15) as i32);

            let obelisk_height = parsed_height.unwrap_or_else(|| rng.random_range(20..35));

            // Base estável 5x5 (Rigor Urbanístico)
            for dx in -2i32..=2i32 {
                for dz in -2i32..=2i32 {
                    editor.set_block_absolute(POLISHED_ANDESITE, x + dx, base_y + 1, z + dz, None, None);
                }
            }

            if is_flagpole {
                // Mastro Monumental (Treliça metálica da Praça dos Três Poderes)
                for y_off in 2..=obelisk_height {
                    let wy = base_y + y_off;
                    editor.set_block_absolute(IRON_BLOCK, x, wy, z, None, None);
                    if y_off % 2 == 0 {
                        editor.set_block_absolute(IRON_BARS, x + 1, wy, z, None, None);
                        editor.set_block_absolute(IRON_BARS, x - 1, wy, z, None, None);
                        editor.set_block_absolute(IRON_BARS, x, wy, z + 1, None, None);
                        editor.set_block_absolute(IRON_BARS, x, wy, z - 1, None, None);
                    }
                }
            } else {
                // Obelisco Modernista de Quartzo
                for y_off in 2..=obelisk_height {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + y_off, z, None, None);
                }
                editor.set_block_absolute(QUARTZ_SLAB_TOP, x, base_y + obelisk_height + 1, z, None, None);
            }
        }
        "stele" => {
            editor.set_block_absolute(POLISHED_ANDESITE, x, base_y + 1, z, None, None);
            for y_off in 2i32..=5i32 {
                editor.set_block_absolute(ANDESITE_WALL, x, base_y + y_off, z, None, None);
            }
            editor.set_block_absolute(SMOOTH_STONE_SLAB, x, base_y + 6, z, None, None);
        }
        _ => {
            editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + 1, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + 2, z, None, None);
            editor.set_block_absolute(POLISHED_ANDESITE, x, base_y + 3, z, None, None);
        }
    }
}

/// Generate a large monument (Ex: Memorial JK / Panteão da Pátria)
fn generate_monument(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;
    let base_y = editor.get_ground_level(x, z);

    let parsed_height = node
        .tags
        .get("height")
        .and_then(|h: &String| h.parse::<f64>().ok())
        .map(|h| (h * 1.15) as i32);

    let height = parsed_height.unwrap_or(25).clamp(15, 100);

    // Large base platform 9x9 (Estilo Esplanada)
    for dx in -4i32..=4i32 {
        for dz in -4i32..=4i32 {
            editor.set_block_absolute(WHITE_CONCRETE, x + dx, base_y + 1, z + dz, None, None);
        }
    }

    // Main tower structure (2x2)
    for y_off in 2..height {
        let wy = base_y + y_off;
        editor.set_block_absolute(WHITE_CONCRETE, x, wy, z, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, x + 1, wy, z, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, x, wy, z + 1, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, x + 1, wy, z + 1, None, None);

        // Elementos curvos/abstratos (Niemeyer fake)
        if y_off > height - 8 {
            editor.set_block_absolute(SMOOTH_QUARTZ, x + 2, wy, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x - 1, wy, z + 1, None, None);
        }
    }
    editor.set_block_absolute(SMOOTH_QUARTZ, x, base_y + height, z, None, None);
    editor.set_block_absolute(SMOOTH_QUARTZ, x + 1, base_y + height, z + 1, None, None);
}

/// Generate a wayside cross (Ground Aware)
fn generate_wayside_cross(editor: &mut WorldEditor, node: &ProcessedNode) {
    let base_y = editor.get_ground_level(node.x, node.z);
    generate_monumental_cross(editor, node.x, node.z, base_y, 6);
}

/// Helper function to generate a cross structure
fn generate_monumental_cross(editor: &mut WorldEditor, x: i32, z: i32, base_y: i32, height: i32) {
    editor.set_block_absolute(POLISHED_ANDESITE, x, base_y + 1, z, None, None);
    for y_off in 2..=height {
        editor.set_block_absolute(WHITE_CONCRETE, x, base_y + y_off, z, None, None);
    }
    let arm_y = base_y + (height * 3 / 4).max(3);
    for dw in -2i32..=2i32 {
        if dw != 0 {
            editor.set_block_absolute(WHITE_CONCRETE, x + dw, arm_y, z, None, None);
        }
    }
}

/// Generates a solid modernist pyramid (Ex: Templo da Boa Vontade - LBV)
pub fn generate_pyramid(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    if element.nodes.len() < 3 {
        return;
    }

    let footprint: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
    if footprint.is_empty() {
        return;
    }

    // Ground Aware Base
    let base_y = footprint
        .iter()
        .map(|&(x, z)| editor.get_ground_level(x, z))
        .min()
        .unwrap_or(args.ground_level);

    let min_x = footprint.iter().map(|&(x, _)| x).min().unwrap();
    let max_x = footprint.iter().map(|&(x, _)| x).max().unwrap();
    let min_z = footprint.iter().map(|&(_, z)| z).min().unwrap();
    let max_z = footprint.iter().map(|&(_, z)| z).max().unwrap();

    let center_x = (min_x + max_x) as f64 / 2.0;
    let center_z = (min_z + max_z) as f64 / 2.0;

    let width = (max_x - min_x + 1) as f64;
    let length = (max_z - min_z + 1) as f64;
    let half_base = width.min(length) / 2.0;
    let pyramid_height = (half_base * 1.15) as i32;

    let mut last_placed_layer: Option<i32> = None;
    for layer in 0..pyramid_height {
        let radius = half_base * (1.0 - layer as f64 / pyramid_height as f64);
        if radius < 0.0 { break; }

        let y = base_y + 1 + layer;
        let mut placed = false;

        for &(x, z) in &footprint {
            let dx = (x as f64 - center_x).abs();
            let dz = (z as f64 - center_z).abs();

            if dx <= radius && dz <= radius {
                // Bordas de Quartzo Modernista
                let block = if (dx - radius).abs() < 1.0 || (dz - radius).abs() < 1.0 {
                    SMOOTH_QUARTZ
                } else {
                    WHITE_CONCRETE
                };

                editor.set_block_absolute(
                    block,
                    x, y, z,
                    Some(&[
                        GRASS_BLOCK, DIRT, STONE, SAND, GRAVEL,
                        COARSE_DIRT, PODZOL, DIRT_PATH, WHITE_CONCRETE, AIR
                    ]),
                    None,
                );
                placed = true;
            }
        }
        if placed { last_placed_layer = Some(y); } else { break; }
    }

    if let Some(top_y) = last_placed_layer {
        // O Cristal da LBV (Representado por Sea Lantern)
        editor.set_block_absolute(
            SEA_LANTERN,
            center_x.round() as i32,
            top_y + 1,
            center_z.round() as i32,
            None,
            None,
        );
    }
}