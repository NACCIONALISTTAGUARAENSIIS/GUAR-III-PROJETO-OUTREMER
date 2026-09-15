//! Processing of power infrastructure elements (BESM-6 Government Tier — CEB/Distrito Federal).
//!
//! This module handles power-related OSM elements including:
//! - `power=tower` - Large electricity pylons (lattice, com braços cruzados e isoladores)
//! - `power=pole` - Smaller wooden/concrete poles (com variação orgânica: espia/estai,
//!   transformador de poste e braço de iluminação pública, todos probabilísticos)
//! - `power=line` / `power=minor_line` - Power lines connecting towers/poles (catenária real)
//! - `power=substation` - Pátio de subestação (cerca, brita, transformadores, gantry de entrada)
//! - `power=plant` - Área de usina (perímetro cercado + geradores/paineis conforme `plant:source`)
//! - `power=transformer` - Transformador isolado (poste ou abrigado no chão)
//! - `power=switch` / `power=switchgear` / `power=compensator` / `power=converter` - Gabinete
//!   de manobra/controle
//! - `power=generator` - Gerador (solar/eólico/genérico, conforme `generator:source`)
//!
//! 🚨 BESM-6: Toda peça nova aqui usa `deterministic_rng` (não `rand::thread_rng`) para que a
//! MESMA torre/poste/subestação sempre renderize igual entre execuções (determinismo exigido
//! pelo resto do motor), mas com variação real ENTRE elementos distintos — nada aqui deveria
//! sair "clonado" (mesmo bloco, mesma proporção) do vizinho, na mesma lógica de desgaste
//! orgânico já usada em `man_made.rs::generate_underground_pipeline` e `colors.rs`.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::collections::HashMap;

// ============================================================================
// 🚨 DISPATCH: ELEMENTOS `power=*` (WAYS/RELATIONS COMO ProcessedElement)
// ============================================================================

/// Generate power infrastructure from way elements (power lines, torres, subestações, usinas)
pub fn generate_power(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
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
                    generate_power_line(editor, way, args);
                }
            }
            "tower" => generate_power_tower(editor, element, args),
            "pole" => generate_power_pole(editor, element, args),
            "substation" => generate_substation(editor, element, args),
            "plant" => generate_power_plant(editor, element, args),
            "transformer" => {
                if let Some(first_node) = element.nodes().next() {
                    generate_transformer_unit(
                        editor,
                        first_node.x,
                        first_node.z,
                        element.id(),
                        element.tags(),
                        args,
                        true,
                    );
                }
            }
            "switch" | "switchgear" | "compensator" | "converter" => {
                if let Some(first_node) = element.nodes().next() {
                    generate_switchgear_cabinet(
                        editor,
                        first_node.x,
                        first_node.z,
                        element.id(),
                        args,
                    );
                }
            }
            "generator" => {
                if let Some(first_node) = element.nodes().next() {
                    generate_generator(
                        editor,
                        first_node.x,
                        first_node.z,
                        element.id(),
                        element.tags(),
                        args,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Generate power infrastructure from node elements
pub fn generate_power_nodes(editor: &mut WorldEditor, node: &ProcessedNode, args: &Args) {
    // 🚨 BESM-6: Corrigido E0599 (Uso do campo .tags na struct ProcessedNode)
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
            "tower" => generate_power_tower_from_node(editor, node, args),
            "pole" => generate_power_pole_from_node(editor, node, args),
            "transformer" => {
                generate_transformer_unit(editor, node.x, node.z, node.id, &node.tags, args, true)
            }
            "switch" | "switchgear" | "compensator" | "converter" => {
                generate_switchgear_cabinet(editor, node.x, node.z, node.id, args)
            }
            "generator" => generate_generator(editor, node.x, node.z, node.id, &node.tags, args),
            _ => {}
        }
    }
}

// ============================================================================
// TORRES DE TRANSMISSÃO (LATTICE)
// ============================================================================

/// Generate a high-voltage transmission tower (pylon) from a ProcessedElement
fn generate_power_tower(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };
    // Rigor 1.15 Vertical: Torres de 25m -> 29 blocos
    let height = element
        .tags()
        .get("height")
        .and_then(|h: &String| h.parse::<f64>().ok())
        .map(|h| (h * 1.15).round() as i32)
        .unwrap_or(29)
        .clamp(17, 46);
    generate_power_tower_impl(
        editor,
        first_node.x,
        first_node.z,
        height,
        element.id(),
        args,
    );
}

/// Generate a high-voltage transmission tower (pylon) from a ProcessedNode
fn generate_power_tower_from_node(editor: &mut WorldEditor, node: &ProcessedNode, args: &Args) {
    // 🚨 BESM-6: Corrigido E0599 (Uso do campo .tags na struct ProcessedNode)
    let height = node
        .tags
        .get("height")
        .and_then(|h: &String| h.parse::<f64>().ok())
        .map(|h| (h * 1.15).round() as i32)
        .unwrap_or(29)
        .clamp(17, 46);
    generate_power_tower_impl(editor, node.x, node.z, height, node.id, args);
}

/// Generate a high-voltage transmission tower (pylon)
fn generate_power_tower_impl(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    height: i32,
    id: u64,
    args: &Args,
) {
    // 🚨 BESM-6: Aterrando a torre no terreno volumétrico em vez de deixá-la voar no Y=0
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };

    // Rigor 1.33 Horizontal: Base alargada para 11x11
    let base_width = 5;
    let top_width = 1;
    let arm_height = height - 5;
    let arm_length = 8;

    // Build the four corner legs with tapering
    for y in 1..=height {
        let absolute_y = ground_y + y;
        let progress = y as f32 / height as f32;
        let current_width = base_width - ((base_width - top_width) as f32 * progress) as i32;

        let corners = [
            (x - current_width, z - current_width),
            (x + current_width, z - current_width),
            (x - current_width, z + current_width),
            (x + current_width, z + current_width),
        ];

        for (cx, cz) in corners {
            // 🚨 RECONEXÃO ARTÍSTICA: Desgaste orgânico determinístico — cada torre
            // envelhece de um jeito ligeiramente diferente da vizinha (mesma lógica de
            // `man_made.rs::generate_underground_pipeline`, ~1 em cada 6 blocos vira a
            // variante "oxidada"), em vez de todas saírem clonadas.
            let mut weather_rng = coord_rng(cx, absolute_y, cz, id);
            let leg_block = if weather_rng.gen_bool(0.16) {
                STONE
            } else {
                ANDESITE
            };
            editor.set_block_absolute(leg_block, cx, absolute_y, cz, None, None);
        }

        // Horizontal cross-bracing
        if y % 5 == 0 && y < height - 2 {
            for dx in -current_width..=current_width {
                editor.set_block_absolute(
                    ANDESITE,
                    x + dx,
                    absolute_y,
                    z - current_width,
                    None,
                    None,
                );
                editor.set_block_absolute(
                    ANDESITE,
                    x + dx,
                    absolute_y,
                    z + current_width,
                    None,
                    None,
                );
            }
            for dz in -current_width..=current_width {
                editor.set_block_absolute(
                    ANDESITE,
                    x - current_width,
                    absolute_y,
                    z + dz,
                    None,
                    None,
                );
                editor.set_block_absolute(
                    ANDESITE,
                    x + current_width,
                    absolute_y,
                    z + dz,
                    None,
                    None,
                );
            }
        }

        // Diagonal bracing internals (Visual Detail)
        if y % 5 >= 1 && y % 5 <= 4 && y > 1 && y < height - 2 {
            let prev_width = base_width
                - ((base_width - top_width) as f32 * ((y - 1) as f32 / height as f32)) as i32;

            if current_width != prev_width || y % 5 == 2 {
                editor.set_block_absolute(IRON_BARS, x, absolute_y, z, None, None);
            }
        }
    }

    // Cross-arms for power lines
    let absolute_arm_y = ground_y + arm_height;
    for arm_offset in [-arm_length, arm_length] {
        for dx in 0..=arm_length {
            let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
            editor.set_block_absolute(ANDESITE, arm_x, absolute_arm_y, z, None, None);
            editor.set_block_absolute(
                ANDESITE,
                x,
                absolute_arm_y,
                z + if arm_offset < 0 { -dx } else { dx },
                None,
                None,
            );
        }

        // Insulators (Isoladores)
        let end_x = if arm_offset < 0 {
            x - arm_length
        } else {
            x + arm_length
        };
        editor.set_block_absolute(END_ROD, end_x, absolute_arm_y - 1, z, None, None);
        editor.set_block_absolute(END_ROD, x, absolute_arm_y - 1, z + arm_offset, None, None);
    }

    // Lower arms for multi-circuit transmission lines
    let lower_arm_height = arm_height - 7;
    if lower_arm_height > 5 {
        let absolute_lower_arm_y = ground_y + lower_arm_height;
        let lower_arm_length = arm_length - 2;
        for arm_offset in [-lower_arm_length, lower_arm_length] {
            for dx in 0..=lower_arm_length {
                let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
                editor.set_block_absolute(ANDESITE, arm_x, absolute_lower_arm_y, z, None, None);
            }
            let end_x = if arm_offset < 0 {
                x - lower_arm_length
            } else {
                x + lower_arm_length
            };
            editor.set_block_absolute(END_ROD, end_x, absolute_lower_arm_y - 1, z, None, None);
        }
    }

    // Top finish and lightning protection
    editor.set_block_absolute(ANDESITE, x, ground_y + height, z, None, None);
    editor.set_block_absolute(LIGHTNING_ROD, x, ground_y + height + 1, z, None, None);

    // Brasília Concrete Foundation Pad (Sapata CEB aterrada no relevo real)
    for dx in -4i32..=4i32 {
        for dz in -4i32..=4i32 {
            let local_ground = if args.terrain {
                editor.get_ground_level(x + dx, z + dz)
            } else {
                0
            };
            editor.set_block_absolute(POLISHED_ANDESITE, x + dx, local_ground, z + dz, None, None);
        }
    }

    // 🚨 Placa de risco (CEB): identidade real de operadora + aviso de segurança,
    // fixada na base de uma das pernas. Primeira utilização de `WorldEditor::set_sign`
    // no motor inteiro — a API já existia pronta e nunca tinha sido chamada.
    editor.set_sign(
        "PERIGO".to_string(),
        "ALTA TENSÃO".to_string(),
        "RISCO DE MORTE".to_string(),
        "CEB - Distrito Federal".to_string(),
        x + base_width,
        1,
        z,
        0,
    );
}

// ============================================================================
// POSTES DE DISTRIBUIÇÃO (COM VARIAÇÃO ORGÂNICA)
// ============================================================================

/// Generate a wooden/concrete power pole from a ProcessedElement
fn generate_power_pole(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };
    let height = element
        .tags()
        .get("height")
        .and_then(|h: &String| h.parse::<f64>().ok())
        .map(|h| (h * 1.15).round() as i32)
        .unwrap_or(12)
        .clamp(7, 18);
    let pole_material = element
        .tags()
        .get("material")
        .map(|m: &String| m.as_str())
        .unwrap_or("concrete");
    generate_power_pole_impl(
        editor,
        first_node.x,
        first_node.z,
        height,
        pole_material,
        element.id(),
        args,
    );
}

/// Generate a wooden/concrete power pole from a ProcessedNode
fn generate_power_pole_from_node(editor: &mut WorldEditor, node: &ProcessedNode, args: &Args) {
    // 🚨 BESM-6: Corrigido E0599
    let height = node
        .tags
        .get("height")
        .and_then(|h: &String| h.parse::<f64>().ok())
        .map(|h| (h * 1.15).round() as i32)
        .unwrap_or(12)
        .clamp(7, 18);
    let pole_material = node
        .tags
        .get("material")
        .map(|m: &String| m.as_str())
        .unwrap_or("concrete");
    generate_power_pole_impl(editor, node.x, node.z, height, pole_material, node.id, args);
}

/// Generate a concrete/metal power pole (CEB Standard)
///
/// 🚨 BESM-6 RECONEXÃO ARTÍSTICA: antes, todo poste da mesma altura/material saía
/// idêntico ao vizinho — nem uma escala de roleplay convincente, nem organicidade real.
/// Agora cada poste (semente = `id` do nó/way OSM) decide independentemente:
/// - ~30% de chance de ter um estai/espia de ancoragem (comum em postes de esquina/fim de linha);
/// - ~18% de chance de carregar um transformador de poste (só faz sentido em postes baixos,
///   de distribuição — altura <= 13, já que torres de transmissão não levam isso);
/// - ~22% de chance de também servir de poste de iluminação pública (braço + luminária),
///   sem duplicar o `highway=street_lamp` dedicado de `highways.rs` (esse é um elemento OSM
///   separado; aqui é só a combinação realista "poste de rede que também ilumina a rua").
fn generate_power_pole_impl(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    height: i32,
    pole_material: &str,
    id: u64,
    args: &Args,
) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };

    let pole_block = match pole_material {
        "concrete" => GRAY_CONCRETE,
        "steel" | "metal" => IRON_BLOCK,
        "wood" => OAK_LOG,
        _ => GRAY_CONCRETE,
    };

    for y in 1..=height {
        editor.set_block_absolute(pole_block, x, ground_y + y, z, None, None);
    }

    let arm_length = 2;
    for dx in -arm_length..=arm_length {
        editor.set_block_absolute(
            LIGHT_GRAY_CONCRETE,
            x + dx,
            ground_y + height,
            z,
            None,
            None,
        );
    }

    // Power line insulators on poles
    editor.set_block_absolute(
        END_ROD,
        x - arm_length,
        ground_y + height + 1,
        z,
        None,
        None,
    );
    editor.set_block_absolute(
        END_ROD,
        x + arm_length,
        ground_y + height + 1,
        z,
        None,
        None,
    );
    editor.set_block_absolute(END_ROD, x, ground_y + height + 1, z, None, None);

    let mut rng = element_rng(id);

    // Estai/espia de ancoragem: cabo diagonal até uma âncora no chão, típico de postes
    // de esquina ou de fim de linha (tensão assimétrica no topo).
    if rng.gen_bool(0.3) {
        let (gx, gz) = if rng.gen_bool(0.5) { (2, 0) } else { (0, 2) };
        let anchor_x = x + gx * 2;
        let anchor_z = z + gz * 2;
        let anchor_ground = if args.terrain {
            editor.get_ground_level(anchor_x, anchor_z)
        } else {
            0
        };
        editor.set_block_absolute(STONE, anchor_x, anchor_ground, anchor_z, None, None);
        let guy_points = bresenham_line(
            anchor_x,
            anchor_ground + 1,
            anchor_z,
            x,
            ground_y + height - 2,
            z,
        );
        for (gxp, gyp, gzp) in guy_points {
            editor.set_block_absolute(CHAIN, gxp, gyp, gzp, None, None);
        }
    }

    // Transformador de poste (só em postes baixos de distribuição — não em subtransmissão alta)
    if height <= 13 && rng.gen_bool(0.18) {
        let mount_y = ground_y + height - 3;
        generate_pole_mounted_transformer(editor, x + 1, mount_y, z, id);
    }

    // Braço de iluminação pública combinado (poste de rede + luminária de rua)
    if rng.gen_bool(0.22) {
        let arm_y = ground_y + height - 1;
        let (lx, lz) = if rng.gen_bool(0.5) { (1, 0) } else { (0, 1) };
        editor.set_block_absolute(IRON_BARS, x + lx, arm_y, z + lz, None, None);
        editor.set_block_absolute(GLOWSTONE, x + lx * 2, arm_y, z + lz * 2, None, None);
    }
}

/// Pequeno transformador de poste (drum lateral), montado direto no fuste
fn generate_pole_mounted_transformer(editor: &mut WorldEditor, x: i32, y: i32, z: i32, id: u64) {
    editor.set_block_absolute(IRON_BLOCK, x, y, z, None, None);
    editor.set_block_absolute(IRON_BLOCK, x, y + 1, z, None, None);
    let mut rng = coord_rng(x, y, z, id);
    let cap_block = if rng.gen_bool(0.5) {
        BARREL
    } else {
        IRON_BLOCK
    };
    editor.set_block_absolute(cap_block, x, y - 1, z, None, None);
    editor.set_block_absolute(END_ROD, x, y + 2, z, None, None);
}

// ============================================================================
// LINHAS (CATENÁRIA)
// ============================================================================

/// Generate power lines connecting towers/poles
fn generate_power_line(editor: &mut WorldEditor, way: &ProcessedWay, args: &Args) {
    if way.nodes.len() < 2 {
        return;
    }

    // 🚨 BESM-6: Corrigido E0599 (Uso do campo .tags na struct ProcessedWay)
    let base_height = way
        .tags
        .get("voltage")
        .and_then(|v: &String| v.parse::<i32>().ok())
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

            // 🚨 BESM-6: Aterrando os cabos da CEB acompanhando o relevo da pista
            let ground_y = if args.terrain {
                editor.get_ground_level(*lx, *lz)
            } else {
                0
            };
            let wire_y = (ground_y + base_height - sag).max(ground_y + 3);

            editor.set_block_absolute(chain_block, *lx, wire_y, *lz, None, None);

            // Double wiring for high-voltage circuits
            if base_height >= 24 {
                if dx.abs() >= dz.abs() {
                    editor.set_block_absolute(chain_block, *lx, wire_y, *lz + 1, None, None);
                    editor.set_block_absolute(chain_block, *lx, wire_y, *lz - 1, None, None);
                } else {
                    editor.set_block_absolute(chain_block, *lx + 1, wire_y, *lz, None, None);
                    editor.set_block_absolute(chain_block, *lx - 1, wire_y, *lz, None, None);
                }
            }
        }
    }
}

// ============================================================================
// 🚨 RECONEXÃO: SUBESTAÇÕES (`power=substation`)
// ============================================================================

/// Gera um pátio de subestação completo: brita de aterramento, cerca perimetral
/// (tela + postes de concreto), transformadores internos e gantry de entrada/saída
/// de linha nas extremidades do polígono.
///
/// Antes desta reconexão, `power=substation` caía no `_ => {}` silencioso — o pátio
/// inteiro (frequentemente um elemento urbano bem visível) simplesmente não existia
/// no mundo gerado.
fn generate_substation(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let ProcessedElement::Way(way) = element else {
        return;
    };
    if way.nodes.len() < 3 {
        return;
    }

    let ring: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let footprint = flood_fill_area(&ring, args.timeout.as_ref());
    if footprint.is_empty() {
        return;
    }

    let id = way.id;

    // 1. Pátio de brita (grounding yard) — com poças/manchas de umidade orgânicas
    for &(x, z) in &footprint {
        let ground_y = editor.get_ground_level(x, z);
        let mut yard_rng = coord_rng(x, ground_y, z, id);
        let yard_block = if yard_rng.gen_bool(0.08) {
            COARSE_DIRT
        } else {
            GRAVEL
        };
        editor.set_block_absolute(yard_block, x, ground_y, z, None, None);
    }

    // 2. Cerca perimetral
    generate_perimeter_fence(editor, &ring, id, args);

    // 3. Gantry de entrada/saída de linha nas duas extremidades do contorno
    generate_line_gantry(editor, ring[0].0, ring[0].1, id, args);
    let last = ring[ring.len() / 2];
    generate_line_gantry(editor, last.0, last.1, id.wrapping_add(1), args);

    // 4. Transformadores internos, distribuídos ao longo da varredura Scanline do
    // pátio (já visita o polígono em ordem de linhas, garantindo espalhamento real
    // sem precisar de geometria extra de grade).
    let bay_count = ((footprint.len() / 90) as i32).clamp(1, 5) as usize;
    for i in 0..bay_count {
        let frac = (i as f64 + 1.0) / (bay_count as f64 + 1.0);
        let idx = ((footprint.len() as f64 - 1.0) * frac).round() as usize;
        let (bx, bz) = footprint[idx.min(footprint.len() - 1)];
        generate_transformer_unit(
            editor,
            bx,
            bz,
            id.wrapping_add(10 + i as u64),
            &HashMap::new(),
            args,
            false,
        );
    }

    // 5. Casa de comando/controle (só em pátios grandes o bastante para justificar)
    if footprint.len() > 250 {
        let (cx, cz) = footprint[footprint.len() / 2];
        generate_control_kiosk(editor, cx, cz, id.wrapping_add(99), args);
    }
}

/// Gera uma usina/central geradora (`power=plant`): perímetro cercado + geradores
/// distribuídos (painéis solares em grade, se `plant:source=solar`; unidades genéricas
/// caso contrário).
fn generate_power_plant(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let ProcessedElement::Way(way) = element else {
        return;
    };
    if way.nodes.len() < 3 {
        return;
    }

    let ring: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let footprint = flood_fill_area(&ring, args.timeout.as_ref());
    if footprint.is_empty() {
        return;
    }

    let id = way.id;
    generate_perimeter_fence(editor, &ring, id, args);

    let source = way
        .tags
        .get("plant:source")
        .map(|s| s.as_str())
        .unwrap_or("");

    if source == "solar" {
        // Grade de painéis solares cobrindo o pátio inteiro (usina fotovoltaica real).
        for (i, &(x, z)) in footprint.iter().enumerate() {
            let ground_y = editor.get_ground_level(x, z);
            if i % 3 == 0 && i % 7 != 0 {
                generate_solar_panel_module(editor, x, ground_y, z, id.wrapping_add(i as u64));
            } else {
                editor.set_block_absolute(GRAVEL, x, ground_y, z, None, None);
            }
        }
    } else {
        // Usina genérica (térmica/hidro/etc.): pátio de brita + algumas unidades
        // geradoras espalhadas, sem inventar detalhe demais sem mapeamento explícito.
        for &(x, z) in &footprint {
            let ground_y = editor.get_ground_level(x, z);
            editor.set_block_absolute(GRAVEL, x, ground_y, z, None, None);
        }
        let unit_count = ((footprint.len() / 150) as i32).clamp(1, 3) as usize;
        for i in 0..unit_count {
            let frac = (i as f64 + 1.0) / (unit_count as f64 + 1.0);
            let idx = ((footprint.len() as f64 - 1.0) * frac).round() as usize;
            let (bx, bz) = footprint[idx.min(footprint.len() - 1)];
            generate_generator(
                editor,
                bx,
                bz,
                id.wrapping_add(20 + i as u64),
                &way.tags,
                args,
            );
        }
    }
}

// ============================================================================
// TRANSFORMADORES, GABINETES DE MANOBRA E GERADORES (UNIDADES REUTILIZÁVEIS)
// ============================================================================

/// Unidade de transformador (drum + aletas de refrigeração + buchas), reutilizada
/// tanto para `power=transformer` isolado (poste/solo) quanto para os transformadores
/// internos de uma subestação. Quando `standalone` é verdadeiro (transformador de
/// solo urbano, fora de um pátio já cercado), ganha uma cerquinha própria de 1 bloco.
fn generate_transformer_unit(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    id: u64,
    tags: &HashMap<String, String>,
    args: &Args,
    standalone: bool,
) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };

    // Base de concreto
    for dx in -1..=1 {
        for dz in -1..=1 {
            editor.set_block_absolute(POLISHED_ANDESITE, x + dx, ground_y, z + dz, None, None);
        }
    }

    let mut rng = element_rng(id);
    let rated_high = tags
        .get("voltage:primary")
        .or_else(|| tags.get("voltage"))
        .and_then(|v| v.parse::<i32>().ok())
        .map(|v| v >= 33000)
        .unwrap_or(false);
    let body_height = if rated_high { 3 } else { 2 };

    // Corpo (drum) com aletas de refrigeração alternadas — desgaste orgânico por bloco
    for y in 1..=body_height {
        let absolute_y = ground_y + y;
        let mut weather_rng = coord_rng(x, absolute_y, z, id);
        let side_block = if weather_rng.gen_bool(0.2) {
            OXIDIZED_COPPER
        } else {
            COPPER_BLOCK
        };
        editor.set_block_absolute(BARREL, x, absolute_y, z, None, None);
        editor.set_block_absolute(side_block, x + 1, absolute_y, z, None, None);
        editor.set_block_absolute(side_block, x - 1, absolute_y, z, None, None);
        editor.set_block_absolute(side_block, x, absolute_y, z + 1, None, None);
        editor.set_block_absolute(side_block, x, absolute_y, z - 1, None, None);
    }

    // Buchas de alta tensão (bushings) no topo
    editor.set_block_absolute(END_ROD, x, ground_y + body_height + 1, z, None, None);
    if rated_high {
        editor.set_block_absolute(END_ROD, x + 1, ground_y + body_height + 1, z, None, None);
        editor.set_block_absolute(END_ROD, x - 1, ground_y + body_height + 1, z, None, None);
    }

    if standalone && rng.gen_bool(0.7) {
        // Cerquinha de proteção urbana (transformador de solo — comum em calçadas de Brasília)
        for dx in -2..=2 {
            for dz in [-2, 2] {
                editor.set_block_absolute(IRON_BARS, x + dx, ground_y + 1, z + dz, None, None);
            }
        }
        for dz in -2..=2 {
            for dx in [-2, 2] {
                editor.set_block_absolute(IRON_BARS, x + dx, ground_y + 1, z + dz, None, None);
            }
        }
        editor.set_sign(
            "PERIGO".to_string(),
            "CHOQUE ELÉTRICO".to_string(),
            "NÃO TOQUE".to_string(),
            "CEB".to_string(),
            x + 2,
            1,
            z,
            0,
        );
    }
}

/// Gabinete de manobra/controle (`power=switch`, `switchgear`, `compensator`, `converter`)
fn generate_switchgear_cabinet(editor: &mut WorldEditor, x: i32, z: i32, id: u64, args: &Args) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };

    editor.set_block_absolute(POLISHED_ANDESITE, x, ground_y, z, None, None);

    let mut rng = element_rng(id);
    for y in 1..=2 {
        let absolute_y = ground_y + y;
        let mut weather_rng = coord_rng(x, absolute_y, z, id);
        let body_block = if weather_rng.gen_bool(0.15) {
            LIGHT_GRAY_CONCRETE
        } else {
            GRAY_CONCRETE
        };
        editor.set_block_absolute(body_block, x, absolute_y, z, None, None);
    }

    if rng.gen_bool(0.6) {
        editor.set_sign(
            "CEB".to_string(),
            "QUADRO DE".to_string(),
            "MANOBRA".to_string(),
            "ACESSO RESTRITO".to_string(),
            x,
            3,
            z,
            0,
        );
    }
}

/// Gerador isolado (`power=generator`), com estilo determinado por `generator:source`.
fn generate_generator(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    id: u64,
    tags: &HashMap<String, String>,
    args: &Args,
) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };

    match tags.get("generator:source").map(|s| s.as_str()) {
        Some("solar") | Some("photovoltaic") => {
            generate_solar_panel_module(editor, x, ground_y, z, id);
        }
        Some("wind") => generate_wind_turbine(editor, x, ground_y, z, id),
        _ => generate_generic_genset_shed(editor, x, ground_y, z, id),
    }
}

/// Módulo de painel solar (moldura + vidro escuro inclinado, orientado ao norte
/// verdadeiro por simplicidade — o DF fica no hemisfério sul, então o Norte capta mais sol).
fn generate_solar_panel_module(editor: &mut WorldEditor, x: i32, ground_y: i32, z: i32, id: u64) {
    editor.set_block_absolute(IRON_BLOCK, x, ground_y, z, None, None);
    editor.set_block_absolute(IRON_BLOCK, x, ground_y + 1, z, None, None);

    let mut rng = coord_rng(x, ground_y, z, id);
    let panel_block = if rng.gen_bool(0.5) {
        BLACK_STAINED_GLASS
    } else {
        TINTED_GLASS
    };
    for dx in -1..=1 {
        editor.set_block_absolute(panel_block, x + dx, ground_y + 2, z - 1, None, None);
        editor.set_block_absolute(panel_block, x + dx, ground_y + 3, z, None, None);
    }
}

/// Aerogerador simplificado: torre + três pás (portões de cerca como aproximação
/// visual leve, sem geometria rotativa real).
fn generate_wind_turbine(editor: &mut WorldEditor, x: i32, ground_y: i32, z: i32, id: u64) {
    let mut rng = element_rng(id);
    let height = 20 + rng.gen_range(-2..=2);

    for y in 1..=height {
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, ground_y + y, z, None, None);
    }
    editor.set_block_absolute(IRON_BLOCK, x, ground_y + height + 1, z, None, None);

    // Três pás em Y (120° aproximado via deslocamentos ortogonais/diagonais)
    let blade_len = 6;
    let hub_y = ground_y + height + 1;
    for i in 1..=blade_len {
        editor.set_block_absolute(WHITE_CONCRETE, x, hub_y + i, z, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, x + i, hub_y - (i / 2), z, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, x - i, hub_y - (i / 2), z, None, None);
    }
}

/// Abrigo genérico para gerador (diesel/térmico/hídrico não classificado) — galpão
/// pequeno com chaminé de exaustão.
fn generate_generic_genset_shed(editor: &mut WorldEditor, x: i32, ground_y: i32, z: i32, id: u64) {
    let mut rng = element_rng(id);
    for dx in -1..=1 {
        for dz in -1..=1 {
            editor.set_block_absolute(POLISHED_ANDESITE, x + dx, ground_y, z + dz, None, None);
        }
    }
    for y in 1..=3 {
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 && y < 3 {
                    continue; // interior oco
                }
                let block = if rng.gen_bool(0.12) {
                    LIGHT_GRAY_CONCRETE
                } else {
                    GRAY_CONCRETE
                };
                editor.set_block_absolute(block, x + dx, ground_y + y, z + dz, None, None);
            }
        }
    }
    // Chaminé de exaustão
    for y in 4..=6 {
        editor.set_block_absolute(IRON_BLOCK, x, ground_y + y, z, None, None);
    }
}

/// Pequeno quiosque de comando/controle dentro de uma subestação grande.
fn generate_control_kiosk(editor: &mut WorldEditor, x: i32, z: i32, id: u64, args: &Args) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };
    let mut rng = element_rng(id);
    for dx in -1..=1 {
        for dz in -1..=1 {
            editor.set_block_absolute(POLISHED_ANDESITE, x + dx, ground_y, z + dz, None, None);
            for y in 1..=3 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let block = if rng.gen_bool(0.1) {
                    LIGHT_GRAY_CONCRETE
                } else {
                    GRAY_CONCRETE
                };
                editor.set_block_absolute(block, x + dx, ground_y + y, z + dz, None, None);
            }
        }
    }
    editor.set_block_absolute(IRON_TRAPDOOR, x, ground_y + 1, z - 1, None, None);
}

// ============================================================================
// CERCA PERIMETRAL E GANTRY DE LINHA (COMPARTILHADOS)
// ============================================================================

/// Cerca de tela (chain-link) com postes de concreto a cada 4 blocos, seguindo o
/// contorno real do polígono OSM. Não reaproveita `barriers::generate_barriers`
/// (que funde leitura de tags OSM de `barrier=*` com a varredura da linha em um só
/// bloco monolítico, sem um helper de "linha + material" isolado) — o padrão aqui é
/// mais simples e propositalmente fixo (tela + poste), já que uma subestação real
/// nunca é um muro decorativo.
fn generate_perimeter_fence(editor: &mut WorldEditor, ring: &[(i32, i32)], id: u64, args: &Args) {
    if ring.len() < 2 {
        return;
    }

    let mut step: i32 = 0;
    for i in 0..ring.len() {
        let (sx, sz) = ring[i];
        let (ex, ez) = ring[(i + 1) % ring.len()];

        let points = bresenham_line(sx, 0, sz, ex, 0, ez);
        for (px, _, pz) in points {
            let ground_y = if args.terrain {
                editor.get_ground_level(px, pz)
            } else {
                0
            };

            let is_post = step % 4 == 0;
            let post_block = if is_post { ANDESITE_WALL } else { IRON_BARS };
            editor.set_block_absolute(post_block, px, ground_y + 1, pz, None, None);
            editor.set_block_absolute(IRON_BARS, px, ground_y + 2, pz, None, None);

            if is_post {
                let mut rng = coord_rng(px, ground_y, pz, id);
                if rng.gen_bool(0.15) {
                    editor.set_block_absolute(SEA_LANTERN, px, ground_y + 3, pz, None, None);
                }
            }

            step += 1;
        }
    }
}

/// Gantry (pórtico) de entrada/saída de linha: dois postes curtos com isoladores no
/// topo, marcando onde as linhas aéreas se conectam ao pátio.
fn generate_line_gantry(editor: &mut WorldEditor, x: i32, z: i32, id: u64, args: &Args) {
    let ground_y = if args.terrain {
        editor.get_ground_level(x, z)
    } else {
        0
    };
    let height = 6 + element_rng(id).gen_range(-1..=1);

    for offset in [-1, 1] {
        for y in 1..=height {
            editor.set_block_absolute(ANDESITE, x + offset, ground_y + y, z, None, None);
        }
        editor.set_block_absolute(END_ROD, x + offset, ground_y + height + 1, z, None, None);
    }
    for dx in -1..=1 {
        editor.set_block_absolute(ANDESITE, x + dx, ground_y + height, z, None, None);
    }
}
