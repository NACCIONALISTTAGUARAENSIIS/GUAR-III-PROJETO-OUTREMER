use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

// Processa elementos marcados como ponte (Ex: Viadutos da EPIA, Ponte JK, Pontes do Lago)
pub fn generate_bridges(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(_bridge_type) = element.tags.get("bridge") {
        // =================================================================
        // 🚨 INTERCEPTADOR DOCUMENTAL DO IMPÉRIO ROGACIONISTA 🚨
        // =================================================================
        let base_ground_y = if !element.nodes.is_empty() {
            editor.get_ground_level(element.nodes[0].x, element.nodes[0].z)
        } else {
            0
        };

        // 🚨 BESM-6: Corrigido caminho do sub-módulo (Erro E0433)
        if crate::element_processing::landmarks::generate_unique_landmark(
            editor,
            element,
            base_ground_y,
        ) {
            return; // Se for um monumento único, o landmarks.rs assume o controle total.
        }
        // =================================================================

        // RIGOR DE ESCALA: 10 blocos de altura total (11.5 metros na vertical 1.15V)
        let bridge_height = 10;

        // Largura baseada em faixas (Rigor 1.33H)
        let lanes: f64 = element
            .tags
            .get("lanes")
            .and_then(|s: &String| s.parse::<f64>().ok())
            .unwrap_or(2.0);

        // Cada faixa real (3.6m) vira ~2.7 blocos na escala 1.33.
        // 5.5 é o raio para 2 faixas + acostamento (sua medida original aprovada).
        let road_radius = (lanes * 1.35 + 2.8).clamp(4.0, 12.0);
        let structure_radius = road_radius + 2.0;

        // Ponto de cota máxima para manter o tabuleiro nivelado (Evita "montanha-russa")
        let bridge_deck_ground_y = if element.nodes.len() >= 2 {
            let start_node = &element.nodes[0];
            let end_node = &element.nodes[element.nodes.len() - 1];
            let start_y = editor.get_ground_level(start_node.x, start_node.z);
            let end_y = editor.get_ground_level(end_node.x, end_node.z);
            start_y.max(end_y)
        } else {
            return;
        };

        let total_length: f64 = element
            .nodes
            .windows(2)
            .map(|pair| {
                let dx = (pair[1].x - pair[0].x) as f64;
                let dz = (pair[1].z - pair[0].z) as f64;
                (dx * dx + dz * dz).sqrt()
            })
            .sum();

        if total_length == 0.0 {
            return;
        }

        let mut accumulated_length: f64 = 0.0;

        for i in 1..element.nodes.len() {
            let prev = &element.nodes[i - 1];
            let cur = &element.nodes[i];
            let segment_dx = (cur.x - prev.x) as f64;
            let segment_dz = (cur.z - prev.z) as f64;
            let segment_length = (segment_dx * segment_dx + segment_dz * segment_dz).sqrt();

            // Cálculo Vetorial da Normal para largura constante independente do ângulo
            let (dir_x, dir_z, norm_x, norm_z) = if segment_length > 0.0 {
                (
                    segment_dx / segment_length,
                    segment_dz / segment_length,
                    -segment_dz / segment_length,
                    segment_dx / segment_length,
                )
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            // Rampa suavizada (20% do comprimento) - Mínimo de 40 blocos para permitir veículos de RP
            let ramp_length = (total_length * 0.20).clamp(40.0, 150.0) as usize;

            for (idx, (x, _, z)) in points.iter().enumerate() {
                let segment_progress = if points.len() > 1 {
                    idx as f64 / (points.len() - 1) as f64
                } else {
                    0.0
                };
                let point_distance = accumulated_length + segment_progress * segment_length;
                let overall_progress = (point_distance / total_length).clamp(0.0, 1.0);
                let total_len_usize = total_length as usize;
                let overall_idx = (overall_progress * total_len_usize as f64) as usize;

                // Cálculo da inclinação da rampa
                let ramp_offset = if overall_idx < ramp_length {
                    (overall_idx as f64 * bridge_height as f64 / ramp_length as f64) as i32
                } else if overall_idx >= total_len_usize.saturating_sub(ramp_length) {
                    let dist_from_end = total_len_usize - overall_idx;
                    (dist_from_end as f64 * bridge_height as f64 / ramp_length as f64) as i32
                } else {
                    bridge_height
                };

                let bridge_y = bridge_deck_ground_y + ramp_offset;

                // VARREDURA DE PINCEL VETORIAL (Área de influência da ponte)
                let brush_limit = (structure_radius + 2.0) as i32;

                for dx in -brush_limit..=brush_limit {
                    for dz in -brush_limit..=brush_limit {
                        let trans_dist = (dx as f64 * norm_x + dz as f64 * norm_z).abs();
                        let long_dist = (dx as f64 * dir_x + dz as f64 * dir_z).abs();

                        if long_dist <= 0.8 {
                            // Traço fino para evitar sobreposição
                            let bx = *x + dx;
                            let bz = *z + dz;
                            let ground_y = editor.get_ground_level(bx, bz);

                            // Limpa o ar sob a ponte para evitar árvores ou highways clipando
                            for ay in (ground_y + 1)..bridge_y {
                                editor.set_block_absolute(AIR, bx, ay, bz, None, None);
                            }

                            // 1. PISTA (Asfalto e Base de Concreto)
                            if trans_dist <= road_radius {
                                editor.set_block_absolute(
                                    GRAY_CONCRETE,
                                    bx,
                                    bridge_y,
                                    bz,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    POLISHED_ANDESITE,
                                    bx,
                                    bridge_y - 1,
                                    bz,
                                    None,
                                    None,
                                );

                                // ATERRO ORGÂNICO (Abutment)
                                if ramp_offset < bridge_height {
                                    // O aterro desce conforme se afasta do centro (trans_dist)
                                    let slope_drop =
                                        (trans_dist - (road_radius - 2.0)).max(0.0) as i32;
                                    let fill_top_y = (bridge_y - 1 - slope_drop).max(ground_y);

                                    for py in ground_y..=fill_top_y {
                                        let block = if py == fill_top_y {
                                            GRASS_BLOCK
                                        } else {
                                            COARSE_DIRT
                                        };
                                        editor.set_block_absolute(block, bx, py, bz, None, None);
                                    }
                                }
                            }
                            // 2. ESTRUTURA LATERAL E GUARDA-CORPO
                            else if trans_dist <= structure_radius {
                                editor.set_block_absolute(
                                    POLISHED_ANDESITE,
                                    bx,
                                    bridge_y,
                                    bz,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    STONE_BRICK_WALL,
                                    bx,
                                    bridge_y + 1,
                                    bz,
                                    None,
                                    None,
                                );

                                if ramp_offset < bridge_height {
                                    let slope_drop =
                                        (trans_dist - (road_radius - 2.0)).max(0.0) as i32;
                                    let fill_top_y = (bridge_y - 1 - slope_drop).max(ground_y);
                                    for py in ground_y..=fill_top_y {
                                        editor.set_block_absolute(
                                            GRASS_BLOCK,
                                            bx,
                                            py,
                                            bz,
                                            None,
                                            None,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // 3. PILARES EM LÂMINA ADAPTATIVOS (Somente no vão livre)
                if overall_idx % 32 == 0 && ramp_offset >= 7 {
                    let mut min_ground = bridge_y;
                    // Amostragem transversal para pilar não flutuar
                    for w in -5i32..=5i32 {
                        let gy = editor.get_ground_level(
                            *x + (w as f64 * norm_x) as i32,
                            *z + (w as f64 * norm_z) as i32,
                        );
                        if gy < min_ground {
                            min_ground = gy;
                        }
                    }

                    // Pilares mais grossos para vãos muito altos
                    let p_thick = if (bridge_y - min_ground) > 15 { 2 } else { 1 };
                    let blade_width = (road_radius * 0.8) as i32;

                    for py in min_ground..=(bridge_y - 2) {
                        for w in -blade_width..=blade_width {
                            for t in -p_thick..=p_thick {
                                let px = (w as f64 * norm_x + t as f64 * dir_x).round() as i32;
                                let pz = (w as f64 * norm_z + t as f64 * dir_z).round() as i32;
                                editor.set_block_absolute(
                                    POLISHED_ANDESITE,
                                    *x + px,
                                    py,
                                    *z + pz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
            }
            accumulated_length += segment_length;
        }
    }
}
