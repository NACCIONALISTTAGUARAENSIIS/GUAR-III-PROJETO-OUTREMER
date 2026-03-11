use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(railway_type) = element.tags.get("railway") {
        if [
            "proposed",
            "abandoned",
            "construction",
            "razed",
            "turntable",
        ]
            .contains(&railway_type.as_str())
        {
            return;
        }

        // =================================================================
        // 🚨 INTERCEPTADOR DOCUMENTAL DO IMPÉRIO ROGACIONISTA 🚨
        // Se for uma Estação Monumental (Guará, Águas Claras), cancela a via genérica.
        // =================================================================
        let base_ground_y = editor.get_ground_level(element.nodes[0].x, element.nodes[0].z);
        if crate::landmarks::generate_unique_landmark(editor, element, base_ground_y) {
            return;
        }
        // =================================================================

        // --- DETECÇÃO DE OPERAÇÃO (METRÔ-DF VS CARGA VS PÁTIOS) ---
        let is_metro = element.tags.get("operator").map(|s| s.contains("Metrô") || s.contains("METRO")).unwrap_or(false)
            || element.tags.get("name").map(|s| s.contains("Metrô") || s.contains("Metro")).unwrap_or(false)
            || element.tags.get("usage").map(|s| s == "subway" || s == "urban").unwrap_or(false)
            || element.tags.get("service").map(|s| s == "metro").unwrap_or(false);

        // Identifica pátios de manobra (Yard) ou desvios (Siding/Spur) para gerar base compacta
        let is_yard = element.tags.get("service").map(|s| s == "yard" || s == "siding" || s == "spur").unwrap_or(false);

        // --- DETECÇÃO DE TÚNEIS E SUBWAY (RIGOR SUBTERRÂNEO) ---
        let layer_str = element.tags.get("layer").map(|s| s.as_str()).unwrap_or("0");
        let layer: i32 = layer_str.parse().unwrap_or(0);

        let is_tunnel = element.tags.get("tunnel").map(|s| s.as_str()) == Some("yes")
            || element.tags.get("subway").map(|s| s.as_str()) == Some("yes")
            || railway_type.as_str() == "subway"
            || layer < 0;

        // Offset de profundidade baseado na camada (layer -1 = 15 blocos abaixo da terra, alinhado com as estações)
        let depth_offset = if layer < 0 { layer * 15 } else if is_tunnel { -15 } else { 0 };

        // --- TWEAK DA DUPLA VIA (METRÔ-DF) ---
        let tracks_str = element.tags.get("tracks").map(|s| s.as_str()).unwrap_or(if is_metro && !is_yard { "2" } else { "1" });
        let is_double_track = tracks_str == "2";

        for i in 1..element.nodes.len() {
            let prev_node = element.nodes[i - 1].xz();
            let cur_node = element.nodes[i].xz();

            let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
            let smoothed_points = smooth_diagonal_rails(&points);

            // TWEAK DE ENGENHARIA 1: Topografia Suavizada (Lerp Longitudinal Ajustado)
            let base_start_y = editor.get_ground_level(prev_node.x, prev_node.z) as f64 + depth_offset as f64;
            let base_end_y = editor.get_ground_level(cur_node.x, cur_node.z) as f64 + depth_offset as f64;

            // O(1) Proteção: Divisão por zero caso o segmento seja apenas 1 ponto
            let total_points = (smoothed_points.len() as f64 - 1.0).max(1.0);

            for j in 0..smoothed_points.len() {
                let (bx, _, bz) = smoothed_points[j];

                // Interpolação matemática estrita da altura do trilho
                let progress = j as f64 / total_points;
                let track_y = (base_start_y + (base_end_y - base_start_y) * progress).round() as i32;

                let local_ground = editor.get_ground_level(bx, bz);

                // Túneis furam a terra (Y fixo na rota), Superfície acompanha o relevo (Y.max)
                let final_y = if is_tunnel || layer < 0 {
                    track_y
                } else {
                    track_y.max(local_ground)
                };

                // TWEAK DE ENGENHARIA 2: Vetor Normal de Manhattan
                let prev = if j > 0 { Some(smoothed_points[j - 1]) } else { None };
                let next = if j < smoothed_points.len() - 1 { Some(smoothed_points[j + 1]) } else { None };

                let (dx, dz) = match (prev, next) {
                    (Some((px, _, pz)), Some((nx, _, nz))) => (nx - px, nz - pz),
                    (None, Some((nx, _, nz))) => (nx - bx, nz - bz),
                    (Some((px, _, pz)), None) => (bx - px, bz - pz),
                    _ => (0, 1),
                };

                let norm_x: i32;
                let norm_z: i32;

                if dx.abs() > dz.abs() {
                    norm_x = 0; norm_z = 1;
                } else {
                    norm_x = 1; norm_z = 0;
                }

                let is_curve = prev.is_some() && next.is_some() && (dx.abs() > 0 && dz.abs() > 0);

                // --- NOVO CÁLCULO DE RAIO (Galeria Única Monumental para o Metrô-DF) ---
                let radius = if is_double_track { 7 } else if is_yard { 3 } else { 4 };
                let tunnel_radius = radius + 1; // Espessura da parede da galeria

                // --- INFRAESTRUTURA DE VIA (VOXEL BRUSH) ---

                for wx in -tunnel_radius..=tunnel_radius {
                    for wz in -tunnel_radius..=tunnel_radius {
                        let dist_sq = wx * wx + wz * wz;
                        let build_x = bx + wx;
                        let build_z = bz + wz;

                        if !is_tunnel && dist_sq <= radius * radius {
                            for fill_y in local_ground..final_y {
                                editor.set_block_absolute(DIRT, build_x, fill_y, build_z, None, None);
                            }
                        }

                        // --- ESCAVAÇÃO DA GALERIA ÚNICA (METRÔ-DF) ---
                        if is_tunnel && dist_sq <= tunnel_radius * tunnel_radius {
                            let is_wall = dist_sq >= radius * radius;

                            if is_wall {
                                // Escudo de Concreto Maciço
                                for ty in -1..=7 {
                                    editor.set_block_absolute(SMOOTH_STONE, build_x, final_y + ty, build_z, None, None);
                                }
                            } else {
                                // Lajes Inferior e Superior da Galeria Única
                                editor.set_block_absolute(SMOOTH_STONE, build_x, final_y - 1, build_z, None, None);
                                editor.set_block_absolute(SMOOTH_STONE, build_x, final_y + 8, build_z, None, None);

                                // O Volume Compartilhado (Cava o ar para as duas vias e passarela)
                                for ty in 0..=7 {
                                    editor.set_block_absolute(AIR, build_x, final_y + ty, build_z, None, None);
                                }
                            }
                        }

                        // --- CAMADA DE LASTRO E PASSARELAS ---
                        if dist_sq <= radius * radius {
                            let is_edge = dist_sq >= (radius - 1) * (radius - 1);
                            let banking_y = if is_curve && is_edge { final_y + 1 } else { final_y };

                            // Distância da normal (para criar o corredor central no meio da galeria)
                            let dist_from_center_normal = (wx * norm_x + wz * norm_z).abs();

                            if is_metro {
                                // RIGOR NATM/CONCRETO: Se for túnel, usa laje de concreto (Slab Track). Na superfície, brita.
                                let base_track_block = if is_tunnel { SMOOTH_STONE } else { GRAVEL };
                                editor.set_block_absolute(base_track_block, build_x, final_y, build_z, None, None);

                                // TWEAK DA GALERIA: Passarela Técnica Central (Entre as vias Ida/Volta)
                                if is_double_track && dist_from_center_normal <= 1 {
                                    editor.set_block_absolute(POLISHED_ANDESITE, build_x, final_y, build_z, None, None);
                                }

                                // Passarelas de fuga laterais
                                if is_edge && !is_yard {
                                    editor.set_block_absolute(POLISHED_ANDESITE, build_x, banking_y, build_z, None, None);
                                }
                            } else {
                                let is_stone = (build_x + build_z) % 2 == 0;
                                if is_stone && is_edge {
                                    editor.set_block_absolute(COBBLESTONE, build_x, banking_y, build_z, None, None);
                                } else {
                                    editor.set_block_absolute(GRAVEL, build_x, banking_y, build_z, None, None);
                                }
                            }
                        }
                    }
                }

                // --- POSICIONAMENTO DOS TRILHOS ---
                let rail_block = determine_rail_direction(
                    (bx, bz),
                    prev.map(|(x, _, z)| (x, z)),
                    next.map(|(x, _, z)| (x, z)),
                );

                if is_double_track {
                    // Deslocamento largo de 4 blocos mantido por rigor de Hitbox de Mods RP
                    let offset = 4;
                    let rail_1_x = bx + (offset * norm_x);
                    let rail_1_z = bz + (offset * norm_z);
                    let rail_2_x = bx - (offset * norm_x);
                    let rail_2_z = bz - (offset * norm_z);

                    editor.set_block_absolute(rail_block, rail_1_x, final_y + 1, rail_1_z, None, None);
                    editor.set_block_absolute(rail_block, rail_2_x, final_y + 1, rail_2_z, None, None);

                    if is_metro && !is_yard {
                        // Terceiro trilho para cada via
                        editor.set_block_absolute(SMOOTH_STONE_SLAB, rail_1_x + norm_x, final_y + 1, rail_1_z + norm_z, None, None);
                        editor.set_block_absolute(SMOOTH_STONE_SLAB, rail_2_x - norm_x, final_y + 1, rail_2_z - norm_z, None, None);
                    }
                } else {
                    editor.set_block_absolute(rail_block, bx, final_y + 1, bz, None, None);

                    if is_metro && !is_yard {
                        let third_rail_x = bx + (2 * norm_x);
                        let third_rail_z = bz + (2 * norm_z);
                        editor.set_block_absolute(SMOOTH_STONE_SLAB, third_rail_x, final_y + 1, third_rail_z, None, None);
                    }
                }

                // Dormentes
                if j % 3 == 0 {
                    let dormente_block = if is_metro { STONE_BRICKS } else { DARK_OAK_SLAB };
                    editor.set_block_absolute(dormente_block, bx, final_y, bz, None, None);
                }
            }
        }
    }
}

fn smooth_diagonal_rails(points: &[(i32, i32, i32)]) -> Vec<(i32, i32, i32)> {
    let mut smoothed = Vec::new();

    for i in 0..points.len() {
        let current = points[i];
        smoothed.push(current);

        if i + 1 >= points.len() {
            continue;
        }

        let next = points[i + 1];
        let (x1, y1, z1) = current;
        let (x2, _, z2) = next;

        if (x2 - x1).abs() == 1 && (z2 - z1).abs() == 1 {
            let look_ahead = if i + 2 < points.len() {
                Some(points[i + 2])
            } else {
                None
            };

            let look_behind = if i > 0 { Some(points[i - 1]) } else { None };

            let intermediate = if let Some((prev_x, _, _prev_z)) = look_behind {
                if prev_x == x1 {
                    (x1, y1, z2)
                } else {
                    (x2, y1, z1)
                }
            } else if let Some((next_x, _, _next_z)) = look_ahead {
                if next_x == x2 {
                    (x2, y1, z1)
                } else {
                    (x1, y1, z2)
                }
            } else {
                (x2, y1, z1)
            };

            smoothed.push(intermediate);
        }
    }

    smoothed
}

fn determine_rail_direction(
    current: (i32, i32),
    prev: Option<(i32, i32)>,
    next: Option<(i32, i32)>,
) -> Block {
    let (x, z) = current;

    match (prev, next) {
        (Some((px, pz)), Some((nx, nz))) => {
            if px == nx {
                RAIL_NORTH_SOUTH
            } else if pz == nz {
                RAIL_EAST_WEST
            } else {
                let from_prev = (px - x, pz - z);
                let to_next = (nx - x, nz - z);

                match (from_prev, to_next) {
                    ((-1, 0), (0, -1)) | ((0, -1), (-1, 0)) => RAIL_NORTH_WEST,
                    ((1, 0), (0, -1)) | ((0, -1), (1, 0)) => RAIL_NORTH_EAST,
                    ((-1, 0), (0, 1)) | ((0, 1), (-1, 0)) => RAIL_SOUTH_WEST,
                    ((1, 0), (0, 1)) | ((0, 1), (1, 0)) => RAIL_SOUTH_EAST,
                    _ => {
                        if (px - x).abs() > (pz - z).abs() {
                            RAIL_EAST_WEST
                        } else {
                            RAIL_NORTH_SOUTH
                        }
                    }
                }
            }
        }
        (Some((px, pz)), None) | (None, Some((px, pz))) => {
            if px == x {
                RAIL_NORTH_SOUTH
            } else if pz == z {
                RAIL_EAST_WEST
            } else {
                RAIL_NORTH_SOUTH
            }
        }
        (None, None) => RAIL_NORTH_SOUTH,
    }
}

pub fn generate_roller_coaster(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(roller_coaster) = element.tags.get("roller_coaster") {
        if roller_coaster == "track" {
            if let Some(indoor) = element.tags.get("indoor") {
                if indoor == "yes" {
                    return;
                }
            }

            if let Some(layer) = element.tags.get("layer") {
                if let Ok(layer_value) = layer.parse::<i32>() {
                    if layer_value < 0 {
                        return;
                    }
                }
            }

            let elevation_height = 4;
            let pillar_interval = 6;

            for i in 1..element.nodes.len() {
                let prev_node = element.nodes[i - 1].xz();
                let cur_node = element.nodes[i].xz();

                let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
                let smoothed_points = smooth_diagonal_rails(&points);

                let start_y = editor.get_ground_level(prev_node.x, prev_node.z);

                for j in 0..smoothed_points.len() {
                    let (bx, _, bz) = smoothed_points[j];

                    let local_ground = editor.get_ground_level(bx, bz);
                    let final_y = start_y.max(local_ground) + elevation_height;

                    editor.set_block_absolute(IRON_BLOCK, bx, final_y, bz, None, None);

                    let prev = if j > 0 {
                        Some(smoothed_points[j - 1])
                    } else {
                        None
                    };
                    let next = if j < smoothed_points.len() - 1 {
                        Some(smoothed_points[j + 1])
                    } else {
                        None
                    };

                    let rail_block = determine_rail_direction(
                        (bx, bz),
                        prev.map(|(x, _, z)| (x, z)),
                        next.map(|(x, _, z)| (x, z)),
                    );

                    editor.set_block_absolute(rail_block, bx, final_y + 1, bz, None, None);

                    if j % pillar_interval == 0 {
                        for py in local_ground..final_y {
                            editor.set_block_absolute(IRON_BLOCK, bx, py, bz, None, None);
                        }
                    }
                }
            }
        }
    }
}