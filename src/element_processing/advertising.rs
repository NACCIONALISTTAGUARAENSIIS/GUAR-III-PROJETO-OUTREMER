//! Processing of advertising elements.
//!
//! This module handles advertising-related OSM elements including:
//! - `advertising=column` - Cylindrical advertising columns (Litfaßsäule)
//! - `advertising=flag` - Advertising flags on poles
//! - `advertising=poster_box` - Illuminated poster display boxes

use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Generate advertising structures from node elements
pub fn generate_advertising(editor: &mut WorldEditor, node: &ProcessedNode) {
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

    if let Some(advertising_type) = node.tags.get("advertising") {
        match advertising_type.as_str() {
            "column" => generate_advertising_column(editor, node),
            "flag" => generate_advertising_flag(editor, node),
            "poster_box" => generate_poster_box(editor, node),
            _ => {}
        }
    }
}

/// Generate an advertising column (Litfaßsäule / Totem Urbano)
///
/// Creates a simple advertising column adapted for DF urban scale.
fn generate_advertising_column(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // DF Aesthetic: Illuminated center, adjusted for 1.15 vertical scale (~4.3m tall)
    editor.set_block(POLISHED_ANDESITE, x, 1, z, None, None);
    editor.set_block(SEA_LANTERN, x, 2, z, None, None);
    editor.set_block(SEA_LANTERN, x, 3, z, None, None);
    editor.set_block(SEA_LANTERN, x, 4, z, None, None); // Expanded screen height for RP visibility
    editor.set_block(POLISHED_ANDESITE, x, 5, z, None, None);

    // Smooth stone slab on top for a modern urban finish
    editor.set_block(SMOOTH_STONE_SLAB, x, 6, z, None, None);
}

/// Generate an advertising flag
///
/// Creates a flagpole with a banner/flag for advertising.
fn generate_advertising_flag(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Use deterministic RNG for flag color
    let mut rng = element_rng(node.id);

    // Get height from tags or default (Adjusted for massive DF auto-shop/expressway scales: up to ~25m)
    let height = node
        .tags
        .get("height")
        .and_then(|h: &String| h.parse::<i32>().ok())
        .unwrap_or(15) // Default subiu de 12 para 15
        .clamp(12, 28);

    // Flagpole
    for y in 1..=height {
        editor.set_block(IRON_BARS, x, y, z, None, None);
    }

    // Flag/banner at top (using colored wool)
    // Random bright advertising colors
    let flag_colors = [
        RED_WOOL,
        YELLOW_WOOL,
        BLUE_WOOL,
        GREEN_WOOL,
        ORANGE_WOOL,
        WHITE_WOOL,
    ];
    let flag_block = flag_colors[rng.random_range(0..flag_colors.len())];

    // Flag extends to one side (5 blocks fits 1.33 horizontal perfectly = ~3.7 meters wide)
    let flag_length = 5;
    for dx in 1..=flag_length {
        editor.set_block(flag_block, x + dx, height, z, None, None);
        editor.set_block(flag_block, x + dx, height - 1, z, None, None);
        editor.set_block(flag_block, x + dx, height - 2, z, None, None); // 3 blocks tall for realism
    }

    // Finial at top
    editor.set_block(IRON_BLOCK, x, height + 1, z, None, None);
}

/// Generate a poster box (city light / lollipop display / MUB)
///
/// Creates an illuminated poster display box on a pole.
fn generate_poster_box(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Y=1: Two andesite walls next to each other (Thick concrete/metallic base typical of DF MUBs)
    editor.set_block(ANDESITE_WALL, x, 1, z, None, None);
    editor.set_block(ANDESITE_WALL, x + 1, 1, z, None, None);

    // Y=2, Y=3, Y=4: Illuminated Ad Screens (Proporção monumental para o RP candango)
    editor.set_block(SEA_LANTERN, x, 2, z, None, None);
    editor.set_block(SEA_LANTERN, x + 1, 2, z, None, None);
    editor.set_block(SEA_LANTERN, x, 3, z, None, None);
    editor.set_block(SEA_LANTERN, x + 1, 3, z, None, None);
    editor.set_block(SEA_LANTERN, x, 4, z, None, None);
    editor.set_block(SEA_LANTERN, x + 1, 4, z, None, None);

    // Y=5: Two smooth stone slabs (Modern metallic/concrete roof)
    editor.set_block(SMOOTH_STONE_SLAB, x, 5, z, None, None);
    editor.set_block(SMOOTH_STONE_SLAB, x + 1, 5, z, None, None);
}
