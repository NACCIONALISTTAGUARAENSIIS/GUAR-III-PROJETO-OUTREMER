//! Processing of advertising elements (BESM-6 Government Tier).
//!
//! Este módulo processa elementos de mobiliário urbano e outdoors (MUB JCDecaux / DF).
//! Arquitetura Agnóstica: Aceita `Feature` de qualquer provedor (CSV, GeoJSON, PostGIS, OSM, 3D Tiles).
//!
//! Tipos cobertos e expandidos:
//! - `advertising=column` / `totem` - Totens cilíndricos e informativos de calçada
//! - `advertising=flag` - Mastros publicitários volumétricos de concessionárias
//! - `advertising=poster_box` - MUBs de vidro iluminados (NBT Entity Displays)
//! - `advertising=board` / `billboard` / `screen` - Outdoors massivos e painéis de LED rodoviários
//! - `advertising=wall_profile` - Painéis aderidos a fachadas

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::coord_rng;
use crate::ground::Ground;
use crate::providers::{Feature, GeometryType};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;
use std::f64::consts::PI;

// ============================================================================
// 🚨 MATEMÁTICA GEOMÉTRICA: Análise de Componentes Principais (PCA 2D)
// Extrai os autovetores (Eigenvectors) da nuvem de pontos para deduzir a
// orientação real e volumétrica do painel publicitário, ignorando anomalias.
// ============================================================================
fn extract_geometry_data(geom: &GeometryType) -> Option<(XZPoint, XZPoint)> {
    match geom {
        GeometryType::Point(p) => Some((*p, *p)),
        GeometryType::LineString(pts) | GeometryType::Polygon(pts) => {
            if pts.len() < 2 {
                return Some((pts[0], pts[0]));
            }

            // 1. Calcula o Centroide
            let mut sum_x = 0.0;
            let mut sum_z = 0.0;
            for p in pts {
                sum_x += p.x as f64;
                sum_z += p.z as f64;
            }
            let n = pts.len() as f64;
            let mean_x = sum_x / n;
            let mean_z = sum_z / n;

            // 2. Matriz de Covariância
            let mut cov_xx = 0.0;
            let mut cov_xz = 0.0;
            let mut cov_zz = 0.0;
            for p in pts {
                let dx = p.x as f64 - mean_x;
                let dz = p.z as f64 - mean_z;
                cov_xx += dx * dx;
                cov_xz += dx * dz;
                cov_zz += dz * dz;
            }
            cov_xx /= n;
            cov_xz /= n;
            cov_zz /= n;

            // 3. Autovalores e Autovetor Principal (PCA)
            let trace = cov_xx + cov_zz;
            let det = cov_xx * cov_zz - cov_xz * cov_xz;
            let lambda1 = (trace + ((trace * trace - 4.0 * det).abs()).sqrt()) / 2.0;

            let mut dir_x = cov_xz;
            let mut dir_z = lambda1 - cov_xx;

            // Fallback se for um quadrado perfeito
            if dir_x.abs() < 1e-6 && dir_z.abs() < 1e-6 {
                dir_x = 1.0;
                dir_z = 0.0;
            }

            let len = (dir_x * dir_x + dir_z * dir_z).sqrt();
            dir_x /= len;
            dir_z /= len;

            // 4. Projeta os pontos no eixo principal para achar as extremidades reais do Outdoor
            let mut min_proj = f64::MAX;
            let mut max_proj = f64::MIN;
            for p in pts {
                let proj = (p.x as f64 - mean_x) * dir_x + (p.z as f64 - mean_z) * dir_z;
                if proj < min_proj { min_proj = proj; }
                if proj > max_proj { max_proj = proj; }
            }

            let p1 = XZPoint::new(
                (mean_x + dir_x * min_proj).round() as i32,
                (mean_z + dir_z * min_proj).round() as i32,
            );
            let p2 = XZPoint::new(
                (mean_x + dir_x * max_proj).round() as i32,
                (mean_z + dir_z * max_proj).round() as i32,
            );

            Some((p1, p2))
        }
        _ => None,
    }
}

// ============================================================================
// 🚨 SISTEMA GLOBAL DE VENTO (Perlin Vector Field)
// Oblitera a esquizofrenia visual: todas as bandeiras da cidade obedecem
// a uma frente de onda unificada e contínua baseada na coordenada macro.
// ============================================================================
#[inline(always)]
fn get_wind_vector(x: i32, z: i32) -> (i32, i32) {
    let scale = 0.005;
    let angle = ((x as f64 * scale).sin() + (z as f64 * scale).cos()) * PI;
    let dx = angle.cos().round() as i32;
    let dz = angle.sin().round() as i32;
    if dx == 0 && dz == 0 { (1, 0) } else { (dx, dz) }
}

/// 🚨 CULLING VOLUMÉTRICO O(1): Consulta Híbrida de Terreno e Asfalto
/// Aniquila o loop triplo assassino de CPU. Usa o mapa de Superfície (DSM).
#[inline(always)]
fn is_volume_obstructed(
    editor: &mut WorldEditor,
    ground: &Ground,
    x: i32,
    ground_y: i32,
    radius_x: i32,
    radius_z: i32,
) -> bool {
    for dx in -radius_x..=radius_x {
        for dz in -radius_z..=radius_z {
            let cx = x + dx;
            let cz = z + dz;

            // 1. Sondagem O(1) de Espaço Aéreo (Marquises, Telhados, Pontes)
            let surf_y = ground.surface_level(XZPoint::new(cx, cz));
            if surf_y > ground_y + 1 {
                return true;
            }

            // 2. Sondagem restrita à camada exata do chão (Asfalto, Ciclovias, Água)
            if editor.check_for_block_absolute(
                cx,
                ground_y,
                cz,
                Some(&[
                    WATER,
                    BLACK_CONCRETE, // Asfalto
                    GRAY_CONCRETE,
                    YELLOW_CONCRETE,
                    RED_CONCRETE, // Ciclovias GDF
                ]),
                None,
            ) {
                return true;
            }
        }
    }
    false
}

pub fn generate_advertising(
    editor: &mut WorldEditor,
    feature: &Feature,
    args: &Args,
    ground: &Ground,
) {
    if let Some(advertising_type) = feature.attributes.get("advertising") {
        if let Some(layer) = feature.attributes.get("layer") {
            if layer.parse::<i32>().unwrap_or(0) < 0 { return; }
        }
        if let Some(level) = feature.attributes.get("level") {
            if level.parse::<i32>().unwrap_or(0) < 0 { return; }
        }

        if let Some((p1, p2)) = extract_geometry_data(&feature.geometry) {
            let cx = (p1.x + p2.x) / 2;
            let cz = (p1.z + p2.z) / 2;
            let center_pt = XZPoint::new(cx, cz);

            let ground_y = ground.surface_level(center_pt);

            match advertising_type.as_str() {
                "column" | "totem" => generate_advertising_column(editor, feature, center_pt, args, ground_y, ground),
                "flag" => generate_advertising_flag(editor, feature, center_pt, args, ground_y, ground),
                "poster_box" => generate_poster_box(editor, feature, center_pt, p1, p2, ground_y, ground),
                "board" | "billboard" | "screen" | "wall_profile" => {
                    generate_billboard(editor, feature, p1, p2, args, ground_y, ground)
                }
                _ => {}
            }
        }
    }
}

/// Advertising Column & Totem (Mobiliário de Calçada / Informativos)
fn generate_advertising_column(
    editor: &mut WorldEditor,
    feature: &Feature,
    pt: XZPoint,
    args: &Args,
    ground_y: i32,
    ground: &Ground,
) {
    let x = pt.x;
    let z = pt.z;

    let height_real = feature
        .attributes
        .get("height")
        .and_then(|h| h.parse::<f64>().ok())
        .unwrap_or(2.8);

    let height_blocks = (height_real * args.scale_v).round().max(2.0) as i32;

    if is_volume_obstructed(editor, ground, x, ground_y, 0, 0) {
        return;
    }

    editor.set_block_absolute(POLISHED_ANDESITE, x, ground_y + 1, z, None, None);

    for dy in 2..height_blocks {
        editor.set_block_absolute(SEA_LANTERN, x, ground_y + dy, z, None, None);
    }

    editor.set_block_absolute(SMOOTH_STONE_SLAB, x, ground_y + height_blocks, z, None, None);
}

/// Advertising Flag (Mastros de Concessionárias / SIA / EPIA)
fn generate_advertising_flag(
    editor: &mut WorldEditor,
    feature: &Feature,
    pt: XZPoint,
    args: &Args,
    ground_y: i32,
    ground: &Ground,
) {
    let x = pt.x;
    let z = pt.z;

    let height_real = feature
        .attributes
        .get("height")
        .and_then(|h| h.parse::<f64>().ok())
        .unwrap_or(12.0);

    let height_blocks = (height_real * args.scale_v).clamp(8.0, 30.0).round() as i32;

    if is_volume_obstructed(editor, ground, x, ground_y, 1, 1) {
        return;
    }

    let mut rng = coord_rng(x, ground_y, z, feature.id);
    let base_thickness = if height_blocks > 15 { 1 } else { 0 };

    for y in 1..=height_blocks {
        editor.set_block_absolute(IRON_BLOCK, x, ground_y + y, z, None, None);

        if y < height_blocks / 3 && base_thickness > 0 {
            editor.set_block_absolute(IRON_BARS, x + 1, ground_y + y, z, None, None);
            editor.set_block_absolute(IRON_BARS, x - 1, ground_y + y, z, None, None);
            editor.set_block_absolute(IRON_BARS, x, ground_y + y, z + 1, None, None);
            editor.set_block_absolute(IRON_BARS, x, ground_y + y, z - 1, None, None);
        }
    }

    let flag_colors = [RED_WOOL, YELLOW_WOOL, BLUE_WOOL, GREEN_WOOL, ORANGE_WOOL, WHITE_WOOL];
    let flag_block = flag_colors[rng.random_range(0..flag_colors.len())];

    // 🚨 BESM-6: Consulta à Corrente de Vento Global
    let (dir_x, dir_z) = get_wind_vector(x, z);

    let flag_length = (3.5 * args.scale_h).round() as i32;
    let flag_height = (3.0 * args.scale_v).round().max(2.0) as i32;

    for step in 1..=flag_length {
        let px = x + (dir_x * step);
        let pz = z + (dir_z * step);

        for fy in 0..flag_height {
            editor.set_block_absolute(flag_block, px, ground_y + height_blocks - fy, pz, None, None);

            // Engrossamento para visibilidade à distância
            if dir_x != 0 {
                editor.set_block_absolute(flag_block, px, ground_y + height_blocks - fy, pz + 1, None, None);
            } else {
                editor.set_block_absolute(flag_block, px + 1, ground_y + height_blocks - fy, pz, None, None);
            }
        }
    }

    editor.set_block_absolute(IRON_BLOCK, x, ground_y + height_blocks + 1, z, None, None);
}

/// 🚨 BESM-6 NBT DISPLAY ENTITIES: Poster Box (MUB JCDecaux de Calçada)
/// Fim da era das Trapdoors gordas. Renderização sub-métrica exata.
fn generate_poster_box(
    editor: &mut WorldEditor,
    feature: &Feature,
    pt: XZPoint,
    p1: XZPoint,
    p2: XZPoint,
    ground_y: i32,
    ground: &Ground,
) {
    let x = pt.x;
    let z = pt.z;

    if is_volume_obstructed(editor, ground, x, ground_y, 0, 0) {
        return;
    }

    let angle_deg = if p1.x != p2.x || p1.z != p2.z {
        ((p2.x - p1.x) as f64).atan2((p2.z - p1.z) as f64).to_degrees()
    } else {
        feature
            .attributes
            .get("direction")
            .or_else(|| feature.attributes.get("angle"))
            .and_then(|a| a.parse::<f64>().ok())
            .unwrap_or_else(|| {
                let mut rng = coord_rng(pt.x, ground_y, pt.z, feature.id);
                if rng.random_bool(0.5) { 0.0 } else { 90.0 }
            })
    };

    // Base MUB (Apenas 1 bloco de altura de haste de suporte)
    editor.set_block_absolute(POLISHED_ANDESITE, x, ground_y + 1, z, None, None);

    // 🚨 Injection do Block Display NBT (Espessura de 15cm)
    let mut nbt = HashMap::new();

    // State do bloco (Tela iluminada)
    let mut block_state = HashMap::new();
    block_state.insert("Name".to_string(), Value::String("minecraft:sea_lantern".to_string()));
    nbt.insert("block_state".to_string(), Value::Compound(block_state));

    // Matriz Afim de Transformação e Escala (Column-Major)
    // Scale X (Largura): 1.2, Scale Y (Altura): 2.0, Scale Z (Espessura Exata): 0.15
    let transform = vec![
        Value::Float(1.2), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0),
        Value::Float(0.0), Value::Float(2.0), Value::Float(0.0), Value::Float(0.0),
        Value::Float(0.0), Value::Float(0.0), Value::Float(0.15), Value::Float(0.0),
        Value::Float(0.0), Value::Float(0.0), Value::Float(0.0), Value::Float(1.0),
    ];
    nbt.insert("transformation".to_string(), Value::List(transform));

    // Rotação Exata do Vetor Eigen (Sem Snap na Grade de 90 graus)
    let rotation = vec![
        Value::Float(angle_deg as f32),
        Value::Float(0.0)
    ];
    nbt.insert("Rotation".to_string(), Value::List(rotation));

    // Pousa o display entity exatamente em cima da haste
    editor.add_entity("minecraft:block_display", x, ground_y + 2, z, Some(nbt));
}

/// Outdoors e Painéis Rodoviários (Voxelização Real via Bresenham)
fn generate_billboard(
    editor: &mut WorldEditor,
    feature: &Feature,
    p1: XZPoint,
    p2: XZPoint,
    args: &Args,
    ground_y: i32,
    ground: &Ground,
) {
    let mut final_p1 = p1;
    let mut final_p2 = p2;

    if p1.x == p2.x && p1.z == p2.z {
        let angle_deg = feature
            .attributes
            .get("direction")
            .or_else(|| feature.attributes.get("angle"))
            .and_then(|a| a.parse::<f64>().ok())
            .unwrap_or_else(|| {
                let mut rng = coord_rng(p1.x, ground_y, p1.z, feature.id);
                if rng.random_bool(0.5) { 0.0 } else { 90.0 }
            });

        let panel_width_radius = 4.5 * args.scale_h;
        let angle_rad = angle_deg.to_radians();

        let dx = (angle_rad.cos() * panel_width_radius).round() as i32;
        let dz = (angle_rad.sin() * panel_width_radius).round() as i32;

        final_p1 = XZPoint::new(p1.x - dx, p1.z - dz);
        final_p2 = XZPoint::new(p1.x + dx, p1.z + dz);
    }

    let cx = (final_p1.x + final_p2.x) / 2;
    let cz = (final_p1.z + final_p2.z) / 2;

    let base_height = (3.0 * args.scale_v).round() as i32;
    let panel_height = (3.0 * args.scale_v).round() as i32;

    if is_volume_obstructed(editor, ground, cx, ground_y, 1, 1) {
        return;
    }

    let dist_sq = (final_p2.x - final_p1.x).pow(2) + (final_p2.z - final_p1.z).pow(2);
    let mut pillars = vec![XZPoint::new(cx, cz)];
    if dist_sq > 100 {
        pillars.push(final_p1);
        pillars.push(final_p2);
    }

    for pilar in pillars {
        for dy in 1..=base_height {
            editor.set_block_absolute(IRON_BLOCK, pilar.x, ground_y + dy, pilar.z, None, None);
        }
    }

    let is_screen = feature.attributes.get("advertising").map_or(false, |v| v == "screen");
    let board_material = if is_screen {
        SEA_LANTERN
    } else {
        match feature.attributes.get("material").map(|s| s.as_str()) {
            Some("wood") => OAK_PLANKS,
            _ => LIGHT_GRAY_CONCRETE,
        }
    };

    let start_y = ground_y + base_height + 1;
    let end_y = start_y + panel_height;

    let bresenham_points = bresenham_line(final_p1.x, 0, final_p1.z, final_p2.x, 0, final_p2.z);

    // Calcula Normal
    let dx = (final_p2.x - final_p1.x) as f64;
    let dz = (final_p2.z - final_p1.z) as f64;
    let length = (dx * dx + dz * dz).sqrt();
    let nx = if length != 0.0 { (-dz / length).round() as i32 } else { 0 };
    let nz = if length != 0.0 { (dx / length).round() as i32 } else { 1 };

    for dy in start_y..=end_y {
        for (bx, _, bz) in &bresenham_points {
            editor.set_block_absolute(board_material, *bx, dy, *bz, None, None);
        }
    }

    if !is_screen {
        for (i, (bx, _, bz)) in bresenham_points.iter().enumerate() {
            if i % 3 == 0 {
                editor.set_block_absolute(SEA_LANTERN, bx + nx, start_y, bz + nz, None, None);
            }
        }
    }
}