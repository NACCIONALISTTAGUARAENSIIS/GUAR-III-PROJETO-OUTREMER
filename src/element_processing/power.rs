//! Processing of power infrastructure elements.
//!
//! This module handles power-related OSM elements including:
//! - `power=tower` - Large electricity pylons
//! - `power=pole` - Smaller wooden/concrete poles
//! - `power=line` - Power lines connecting towers/poles

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;

/// Generate power infrastructure from way elements (power lines)
pub fn generate_power(editor: &mut WorldEditor, element: &ProcessedElement) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    // Skip underground power infrastructure
    if element
        .tags()
        .get("location")
        .map(|v| v == "underground" || v == "underwater")
        .unwrap_or(false)
    {
        return;
    }
    if element
        .tags()
        .get("tunnel")
        .map(|v| v == "yes")
        .unwrap_or(false)
    {
        return;
    }

    if let Some(power_type) = element.tags().get("power") {
        match power_type.as_str() {
            "line" | "minor_line" => {
                if let ProcessedElement::Way(way) = element {
                    generate_power_line(editor, way);
                }
            }
            "tower" => generate_power_tower(editor, element),
            "pole" => generate_power_pole(editor, element),
            _ => {}
        }
    }
}

/// Generate power infrastructure from node elements
pub fn generate_power_nodes(editor: &mut WorldEditor, node: &ProcessedNode) {
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

    // Skip underground power infrastructure
    if node
        .tags
        .get("location")
        .map(|v| v == "underground" || v == "underwater")
        .unwrap_or(false)
    {
        return;
    }
    if node.tags.get("tunnel").map(|v| v == "yes").unwrap_or(false) {
        return;
    }

    if let Some(power_type) = node.tags.get("power") {
        match power_type.as_str() {
            "tower" => generate_power_tower_from_node(editor, node),
            "pole" => generate_power_pole_from_node(editor, node),
            _ => {}
        }
    }
}

/// Generate a high-voltage transmission tower (pylon) from a ProcessedElement
fn generate_power_tower(editor: &mut WorldEditor, element: &ProcessedElement) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };
    // Rigor 1.15 Vertical: Torres de 25m -> 29 blocos
    let height = element
        .tags()
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .map(|h| (h as f32 * 1.15) as i32)
        .unwrap_or(29)
        .clamp(17, 46);
    generate_power_tower_impl(editor, first_node.x, first_node.z, height);
}

/// Generate a high-voltage transmission tower (pylon) from a ProcessedNode
fn generate_power_tower_from_node(editor: &mut WorldEditor, node: &ProcessedNode) {
    // Rigor 1.15 Vertical
    let height = node
        .tags
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .map(|h| (h as f32 * 1.15) as i32)
        .unwrap_or(29)
        .clamp(17, 46);
    generate_power_tower_impl(editor, node.x, node.z, height);
}

/// Generate a high-voltage transmission tower (pylon)
fn generate_power_tower_impl(editor: &mut WorldEditor, x: i32, z: i32, height: i32) {
    // Rigor 1.33 Horizontal: Base alargada para 11x11
    let base_width = 5;
    let top_width = 1;
    let arm_height = height - 5;
    let arm_length = 8;

    // Build the four corner legs with tapering
    for y in 1..=height {
        let progress = y as f32 / height as f32;
        let current_width = base_width - ((base_width - top_width) as f32 * progress) as i32;

        let corners = [
            (x - current_width, z - current_width),
            (x + current_width, z - current_width),
            (x - current_width, z + current_width),
            (x + current_width, z + current_width),
        ];

        for (cx, cz) in corners {
            editor.set_block(ANDESITE, cx, y, cz, None, None);
        }

        // Horizontal cross-bracing
        if y % 5 == 0 && y < height - 2 {
            for dx in -current_width..=current_width {
                editor.set_block(ANDESITE, x + dx, y, z - current_width, None, None);
                editor.set_block(ANDESITE, x + dx, y, z + current_width, None, None);
            }
            for dz in -current_width..=current_width {
                editor.set_block(ANDESITE, x - current_width, y, z + dz, None, None);
                editor.set_block(ANDESITE, x + current_width, y, z + dz, None, None);
            }
        }

        // Diagonal bracing internals (Visual Detail)
        if y % 5 >= 1 && y % 5 <= 4 && y > 1 && y < height - 2 {
            let prev_width = base_width
                - ((base_width - top_width) as f32 * ((y - 1) as f32 / height as f32)) as i32;

            if current_width != prev_width || y % 5 == 2 {
                editor.set_block(IRON_BARS, x, y, z, None, None);
            }
        }
    }

    // Cross-arms for power lines
    for arm_offset in [-arm_length, arm_length] {
        for dx in 0..=arm_length {
            let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
            editor.set_block(ANDESITE, arm_x, arm_height, z, None, None);
            editor.set_block(
                ANDESITE,
                x,
                arm_height,
                z + if arm_offset < 0 { -dx } else { dx },
                None,
                None,
            );
        }

        // Insulators (Isoladores)
        let end_x = if arm_offset < 0 { x - arm_length } else { x + arm_length };
        editor.set_block(END_ROD, end_x, arm_height - 1, z, None, None);
        editor.set_block(END_ROD, x, arm_height - 1, z + arm_offset, None, None);
    }

    // Lower arms for multi-circuit transmission lines
    let lower_arm_height = arm_height - 7;
    if lower_arm_height > 5 {
        let lower_arm_length = arm_length - 2;
        for arm_offset in [-lower_arm_length, lower_arm_length] {
            for dx in 0..=lower_arm_length {
                let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
                editor.set_block(ANDESITE, arm_x, lower_arm_height, z, None, None);
            }
            let end_x = if arm_offset < 0 { x - lower_arm_length } else { x + lower_arm_length };
            editor.set_block(END_ROD, end_x, lower_arm_height - 1, z, None, None);
        }
    }

    // Top finish and lightning protection
    editor.set_block(ANDESITE, x, height, z, None, None);
    editor.set_block(LIGHTNING_ROD, x, height + 1, z, None, None);

    // Brasília Concrete Foundation Pad (Sapata CEB)
    for dx in -4..=4 {
        for dz in -4..=4 {
            editor.set_block(POLISHED_ANDESITE, x + dx, 0, z + dz, None, None);
        }
    }
}

/// Generate a wooden/concrete power pole from a ProcessedElement
fn generate_power_pole(editor: &mut WorldEditor, element: &ProcessedElement) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };
    let height = element
        .tags()
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .map(|h| (h as f32 * 1.15) as i32)
        .unwrap_or(12)
        .clamp(7, 18);
    let pole_material = element
        .tags()
        .get("material")
        .map(|m| m.as_str())
        .unwrap_or("concrete");
    generate_power_pole_impl(editor, first_node.x, first_node.z, height, pole_material);
}

/// Generate a wooden/concrete power pole from a ProcessedNode
fn generate_power_pole_from_node(editor: &mut WorldEditor, node: &ProcessedNode) {
    let height = node
        .tags()
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .map(|h| (h as f32 * 1.15) as i32)
        .unwrap_or(12)
        .clamp(7, 18);
    let pole_material = node
        .tags()
        .get("material")
        .map(|m| m.as_str())
        .unwrap_or("concrete");
    generate_power_pole_impl(editor, node.x, node.z, height, pole_material);
}

/// Generate a concrete/metal power pole (CEB Standard)
fn generate_power_pole_impl(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    height: i32,
    pole_material: &str,
) {
    let pole_block = match pole_material {
        "concrete" => GRAY_CONCRETE,
        "steel" | "metal" => IRON_BLOCK,
        "wood" => OAK_LOG,
        _ => GRAY_CONCRETE,
    };

    for y in 1..=height {
        editor.set_block(pole_block, x, y, z, None, None);
    }

    let arm_length = 2;
    for dx in -arm_length..=arm_length {
        editor.set_block(LIGHT_GRAY_CONCRETE, x + dx, height, z, None, None);
    }

    // Power line insulators on poles
    editor.set_block(END_ROD, x - arm_length, height + 1, z, None, None);
    editor.set_block(END_ROD, x + arm_length, height + 1, z, None, None);
    editor.set_block(END_ROD, x, height + 1, z, None, None);
}

/// Generate power lines connecting towers/poles
fn generate_power_line(editor: &mut WorldEditor, way: &ProcessedWay) {
    if way.nodes.len() < 2 {
        return;
    }

    let base_height = way
        .tags()
        .get("voltage")
        .and_then(|v| v.parse::<i32>().ok())
        .map(|voltage| {
            if voltage >= 220000 {
                29 // High voltage transmission
            } else if voltage >= 110000 {
                24
            } else if voltage >= 33000 {
                18
            } else {
                14 // Urban distribution
            }
        })
        .unwrap_or(18);

    for i in 1..way.nodes.len() {
        let start = &way.nodes[i - 1];
        let end = &way.nodes[i];

        let dx = (end.x - start.x) as f64;
        let dz = (end.z - start.z) as f64;
        let distance = (dx * dx + dz * dz).sqrt();
        let max_sag = (distance / 15.0).clamp(1.0, 6.0) as i32;

        let chain_block = if dx.abs() >= dz.abs() {
            CHAIN_X
        } else {
            CHAIN_Z
        };

        let line_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

        for (idx, (lx, _, lz)) in line_points.iter().enumerate() {
            let denom = (line_points.len().saturating_sub(1)).max(1) as f64;
            let t = idx as f64 / denom;
            let sag = (4.0 * max_sag as f64 * t * (1.0 - t)) as i32;
            let wire_y = (base_height - sag).max(3);

            editor.set_block(chain_block, *lx, wire_y, *lz, None, None);

            // Double wiring for high-voltage circuits
            if base_height >= 24 {
                if dx.abs() >= dz.abs() {
                    editor.set_block(chain_block, *lx, wire_y, *lz + 1, None, None);
                    editor.set_block(chain_block, *lx, wire_y, *lz - 1, None, None);
                } else {
                    editor.set_block(chain_block, *lx + 1, wire_y, *lz, None, None);
                    editor.set_block(chain_block, *lx - 1, wire_y, *lz, None, None);
                }
            }
        }
    }
}