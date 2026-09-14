use crate::block_definitions::*;
use crate::coordinate_system::cartesian::XZPoint;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

// ============================================================================
// ?? BESM-6 TWEAKS: ENGENHARIA METROVIARIA E FERROVI�RIA PARAM�TRICA (NATM)
// ============================================================================

/// Calcula uma transi��o suave baseada numa aproxima��o de Curva Clotoide (Espiral de Euler).
/// Diferente de uma Spline comum que apenas arredonda cantos, a Clotoide garante
/// que a mudan�a de raio de curvatura seja linear, impedindo "quinas" f�sicas
/// que descarrilariam trens no mundo real (e que deixariam os trilhos em zigue-zague no Minecraft).
fn compute_clothoid_transition(nodes: &[XZPoint], segments_per_curve: usize) -> Vec<(i32, i32)> {
    if nodes.len() < 3 {
        // Se for s� uma reta ou um ponto, n�o h� transi��o a fazer.
        return nodes.iter().map(|n| (n.x, n.z)).collect();
    }

    let mut path = Vec::new();
    let n = nodes.len();

    // Adiciona o primeiro ponto estrito
    path.push((nodes[0].x, nodes[0].z));

    for i in 1..(n - 1) {
        let p0 = nodes[i - 1];
        let p1 = nodes[i];
        let p2 = nodes[i + 1];

        // Vetores de dire��o
        let v1_x = p1.x as f64 - p0.x as f64;
        let v1_z = p1.z as f64 - p0.z as f64;
        let len1 = (v1_x * v1_x + v1_z * v1_z).sqrt();

        let v2_x = p2.x as f64 - p1.x as f64;
        let v2_z = p2.z as f64 - p1.z as f64;
        let len2 = (v2_x * v2_x + v2_z * v2_z).sqrt();

        if len1 < 1.0 || len2 < 1.0 {
            path.push((p1.x, p1.z));
            continue;
        }

        // O raio da curva de transi��o � proporcional ao comprimento dos segmentos
        let transition_radius = (len1.min(len2) * 0.4).max(5.0).min(50.0); // Cap de raio realista

        // Pontos de controle da curva B�zier Racional (Emula��o da Clotoide)
        let t1_x = p1.x as f64 - (v1_x / len1) * transition_radius;
        let t1_z = p1.z as f64 - (v1_z / len1) * transition_radius;

        let t2_x = p1.x as f64 + (v2_x / len2) * transition_radius;
        let t2_z = p1.z as f64 + (v2_z / len2) * transition_radius;

        // Gera os segmentos da curva
        for step in 0..=segments_per_curve {
            let t = step as f64 / segments_per_curve as f64;
            let inv_t = 1.0 - t;

            // Curva Quadr�tica ancorada no v�rtice
            let x = inv_t * inv_t * t1_x + 2.0 * inv_t * t * (p1.x as f64) + t * t * t2_x;
            let z = inv_t * inv_t * t1_z + 2.0 * inv_t * t * (p1.z as f64) + t * t * t2_z;

            let px = x.round() as i32;
            let pz = z.round() as i32;

            if *path.last().unwrap() != (px, pz) {
                path.push((px, pz));
            }
        }
    }

    // Adiciona o �ltimo ponto estrito
    let last = nodes.last().unwrap();
    if *path.last().unwrap() != (last.x, last.z) {
        path.push((last.x, last.z));
    }

    path
}

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
        // ?? INTERCEPTADOR DOCUMENTAL DO IMP�RIO ROGACIONISTA ??
        // Se for uma Esta��o Monumental (Guar�, �guas Claras), cancela a via gen�rica.
        // =================================================================
        let base_ground_y = editor.get_ground_level(element.nodes[0].x, element.nodes[0].z);
        if crate::element_processing::landmarks::generate_unique_landmark(
            editor,
            element,
            base_ground_y,
        ) {
            return;
        }
        // =================================================================

        // --- DETEC��O DE OPERA��O (METR�-DF VS CARGA VS P�TIOS) ---
        let is_metro = element
            .tags
            .get("operator")
            .map(|s: &String| s.contains("Metr�") || s.contains("METRO"))
            .unwrap_or(false)
            || element
                .tags
                .get("name")
                .map(|s: &String| s.contains("Metr�") || s.contains("Metro"))
                .unwrap_or(false)
            || element
                .tags
                .get("usage")
                .map(|s| s == "subway" || s == "urban")
                .unwrap_or(false)
            || element
                .tags
                .get("service")
                .map(|s| s == "metro")
                .unwrap_or(false);

        let is_yard = element
            .tags
            .get("service")
            .map(|s| s == "yard" || s == "siding" || s == "spur")
            .unwrap_or(false);

        // --- DETEC��O DE T�NEIS E SUBWAY (RIGOR SUBTERR�NEO) ---
        let layer_str = element
            .tags
            .get("layer")
            .map(|s: &String| s.as_str())
            .unwrap_or("0");
        let layer: i32 = layer_str.parse().unwrap_or(0);

        let is_tunnel = element.tags.get("tunnel").map(|s: &String| s.as_str()) == Some("yes")
            || element.tags.get("subway").map(|s: &String| s.as_str()) == Some("yes")
            || railway_type.as_str() == "subway"
            || layer < 0;

        // Offset de profundidade baseado na camada (-1 = 15 blocos abaixo da terra. NATM Metro-DF Asa Sul)
        let depth_offset = if layer < 0 {
            layer * 15
        } else if is_tunnel {
            -15
        } else {
            0
        };

        let tracks_str = element
            .tags
            .get("tracks")
            .map(|s: &String| s.as_str())
            .unwrap_or(if is_metro && !is_yard { "2" } else { "1" });
        let is_double_track = tracks_str == "2" || tracks_str == "3"; // Trata >=2 como via dupla para o escopo do jogo

        // ?? BESM-6: Extrai pontos brutos e os suaviza via Curva Clotoide
        let raw_points: Vec<XZPoint> = element
            .nodes
            .iter()
            .map(|n| XZPoint::new(n.x, n.z))
            .collect();
        let smoothed_points = compute_clothoid_transition(&raw_points, 6);

        if smoothed_points.is_empty() {
            return;
        }

        let total_points = (smoothed_points.len() as f64 - 1.0).max(1.0);
        let start_node = smoothed_points.first().unwrap();
        let end_node = smoothed_points.last().unwrap();

        // Eleva��o ancorada no DEM Provider Global
        let base_start_y =
            editor.get_ground_level(start_node.0, start_node.1) as f64 + depth_offset as f64;
        let base_end_y =
            editor.get_ground_level(end_node.0, end_node.1) as f64 + depth_offset as f64;

        for j in 0..smoothed_points.len() {
            let (bx, bz) = smoothed_points[j];

            // Interpola��o topogr�fica linear rigorosa (O trilho n�o pode quicar igual terreno)
            let progress = j as f64 / total_points;
            let track_y = (base_start_y + (base_end_y - base_start_y) * progress).round() as i32;
            let local_ground = editor.get_ground_level(bx, bz);

            // T�neis furam a terra (Y fixo na rota calculada), Superf�cie acompanha o relevo se estiver acima
            let final_y = if is_tunnel || layer < 0 {
                track_y
            } else {
                track_y.max(local_ground)
            };

            // C�lculo do Vetor Normal (Perpendicular � via) para largura param�trica
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

            let (dx, dz) = match (prev, next) {
                (Some((px, pz)), Some((nx, nz))) => (nx - px, nz - pz),
                (None, Some((nx, nz))) => (nx - bx, nz - bz),
                (Some((px, pz)), None) => (bx - px, bz - pz),
                _ => (0, 1),
            };

            let norm_x: i32;
            let norm_z: i32;

            // Normal bruta (90 graus)
            if dx.abs() > dz.abs() {
                norm_x = 0;
                norm_z = 1;
            } else {
                norm_x = 1;
                norm_z = 0;
            }

            let is_curve = prev.is_some() && next.is_some() && (dx.abs() > 0 && dz.abs() > 0);

            // C�lculo do Perfil Transversal Param�trico
            // 1.6m de bitola real + gabarito de seguran�a do Metr�-DF
            let radius = if is_double_track {
                6
            } else if is_yard {
                3
            } else {
                4
            };
            let tunnel_radius = radius + 2; // O anel de concreto armado da galeria NATM

            // --- INFRAESTRUTURA DA VIA (Leito e Galeria) ---
            for wx in -tunnel_radius..=tunnel_radius {
                for wz in -tunnel_radius..=tunnel_radius {
                    let dist_sq = wx * wx + wz * wz;
                    let build_x = bx + wx;
                    let build_z = bz + wz;

                    // Aterro do terreno (Embankment) abaixo dos trilhos de superf�cie
                    if !is_tunnel && dist_sq <= radius * radius {
                        for fill_y in local_ground..final_y {
                            editor.set_block_absolute(DIRT, build_x, fill_y, build_z, None, None);
                        }
                    }

                    // --- ESCAVA��O DA GALERIA (T�NEL NATM - Padr�o Asa Sul Metr�-DF) ---
                    if is_tunnel && dist_sq <= tunnel_radius * tunnel_radius {
                        let is_wall = dist_sq >= (radius * radius);

                        if is_wall {
                            // Escudo do T�nel (Anel el�ptico de concreto)
                            for ty in -1i32..=7i32 {
                                editor.set_block_absolute(
                                    SMOOTH_STONE,
                                    build_x,
                                    final_y + ty,
                                    build_z,
                                    None,
                                    None,
                                );
                            }
                        } else {
                            // Laje do piso (Slab track base)
                            editor.set_block_absolute(
                                SMOOTH_STONE,
                                build_x,
                                final_y - 1,
                                build_z,
                                None,
                                None,
                            );
                            // Teto da galeria
                            editor.set_block_absolute(
                                SMOOTH_STONE,
                                build_x,
                                final_y + 8,
                                build_z,
                                None,
                                None,
                            );

                            // Oco do T�nel (Extirpa terra/pedra e coloca Ar)
                            for ty in 0i32..=7i32 {
                                editor.set_block_absolute(
                                    AIR,
                                    build_x,
                                    final_y + ty,
                                    build_z,
                                    None,
                                    None,
                                );
                            }
                        }
                    }

                    // --- LEITO DE VIA E PASSARELAS DE EMERG�NCIA ---
                    if dist_sq <= radius * radius {
                        let is_edge = dist_sq >= (radius - 1) * (radius - 1);
                        let banking_y = if is_curve && is_edge {
                            final_y + 1
                        } else {
                            final_y
                        };
                        let dist_from_center_normal = (wx * norm_x + wz * norm_z).abs();

                        if is_metro {
                            // Slab Track (Concreto liso) para Metr� em t�nel, Brita pesada na superf�cie
                            let base_track_block = if is_tunnel { SMOOTH_STONE } else { GRAVEL };
                            editor.set_block_absolute(
                                base_track_block,
                                build_x,
                                final_y,
                                build_z,
                                None,
                                None,
                            );

                            // Passarela T�cnica Central Iluminada do Metr�-DF
                            if is_double_track && dist_from_center_normal <= 1 {
                                editor.set_block_absolute(
                                    POLISHED_ANDESITE,
                                    build_x,
                                    final_y,
                                    build_z,
                                    None,
                                    None,
                                );
                                if j % 15 == 0 {
                                    // Ilumina��o central em t�neis
                                    editor.set_block_absolute(
                                        GLOWSTONE, build_x, final_y, build_z, None, None,
                                    );
                                }
                            }

                            // Passarelas de Fuga (Laterais elevadas)
                            if is_edge && !is_yard {
                                editor.set_block_absolute(
                                    POLISHED_ANDESITE,
                                    build_x,
                                    banking_y,
                                    build_z,
                                    None,
                                    None,
                                );
                            }
                        } else {
                            // Ferrovia Comum (RFFSA / FCA)
                            let is_stone = (build_x + build_z) % 2 == 0;
                            if is_stone && is_edge {
                                editor.set_block_absolute(
                                    COBBLESTONE,
                                    build_x,
                                    banking_y,
                                    build_z,
                                    None,
                                    None,
                                );
                            } else {
                                editor.set_block_absolute(
                                    GRAVEL, build_x, banking_y, build_z, None, None,
                                );
                            }
                        }
                    }
                }
            }

            // --- POSICIONAMENTO DIN�MICO DOS TRILHOS E DORMENTES ---
            let rail_block = determine_rail_direction(
                (bx, bz),
                prev.map(|(x, z)| (x, z)),
                next.map(|(x, z)| (x, z)),
            );

            if is_double_track {
                let offset = 3; // Dist�ncia exata do entre-eixo da via dupla
                let rail_1_x = bx + (offset * norm_x);
                let rail_1_z = bz + (offset * norm_z);
                let rail_2_x = bx - (offset * norm_x);
                let rail_2_z = bz - (offset * norm_z);

                editor.set_block_absolute(rail_block, rail_1_x, final_y + 1, rail_1_z, None, None);
                editor.set_block_absolute(rail_block, rail_2_x, final_y + 1, rail_2_z, None, None);

                if is_metro && !is_yard {
                    // Terceiro Trilho Energizado (Alojado no lado externo de cada via)
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        rail_1_x + norm_x,
                        final_y + 1,
                        rail_1_z + norm_z,
                        None,
                        None,
                    );
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        rail_2_x - norm_x,
                        final_y + 1,
                        rail_2_z - norm_z,
                        None,
                        None,
                    );
                }
            } else {
                editor.set_block_absolute(rail_block, bx, final_y + 1, bz, None, None);

                if is_metro && !is_yard {
                    let third_rail_x = bx + (2 * norm_x);
                    let third_rail_z = bz + (2 * norm_z);
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        third_rail_x,
                        final_y + 1,
                        third_rail_z,
                        None,
                        None,
                    );
                }
            }

            // Dormentes transversais de amarra��o (Frequ�ncia real)
            if j % 2 == 0 {
                let dormente_block = if is_metro {
                    STONE_BRICKS
                } else {
                    DARK_OAK_SLAB
                };
                editor.set_block_absolute(dormente_block, bx, final_y, bz, None, None);
            }
        }
    }
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
            if element.tags.get("indoor") == Some(&"yes".to_string()) {
                return;
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

            let raw_points: Vec<XZPoint> = element
                .nodes
                .iter()
                .map(|n| XZPoint::new(n.x, n.z))
                .collect();
            let smoothed_points = compute_clothoid_transition(&raw_points, 4);

            if smoothed_points.is_empty() {
                return;
            }

            let start_node = smoothed_points[0];
            let start_y = editor.get_ground_level(start_node.0, start_node.1);

            for j in 0..smoothed_points.len() {
                let (bx, bz) = smoothed_points[j];

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

                let rail_block = determine_rail_direction((bx, bz), prev, next);

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
