use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;
// 🚨 BESM-6: Integração com Indoor Utility Provider (CAESB Infrastructure)
use crate::providers::{Feature, GeometryType, SemanticGroup};

const V_SCALE: f64 = 1.15;

pub fn generate_man_made(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    // 🚨 BESM-6: Controle Absoluto de Submundo e Infraestrutura
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            // Se for cano de esgoto, duto ou subterrâneo mapeado da CAESB, NÃO pule. Deixe o motor gerar.
            let is_underground_infra = element.tags().get("man_made").map(|s: &String| s.as_str())
                == Some("pipeline")
                || element.tags().get("diameter").is_some();
            if !is_underground_infra {
                return;
            }
        }
    }

    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 && element.tags().get("diameter").is_none() {
            return;
        }
    }

    if let Some(man_made_type) = element.tags().get("man_made") {
        match man_made_type.as_str() {
            "pier" => generate_pier(editor, element, args),
            "antenna" | "mast" => generate_antenna(editor, element, args),
            "tower" => generate_tower(editor, element, args),
            "chimney" => generate_chimney(editor, element, args),
            "water_well" => generate_water_well(editor, element, args),
            "water_tower" => generate_water_tower(editor, element, args),
            "water_tank" | "storage_tank" | "reservoir_covered" | "silo" => {
                generate_tank(editor, element, args, flood_fill_cache)
            }
            "street_cabinet" => generate_street_cabinet(editor, element, args),
            "utility_pole" => generate_utility_pole(editor, element, args),
            "wastewater_plant" | "works" => {
                generate_industrial_works(editor, element, args, flood_fill_cache)
            }
            _ => {} // Fallback silencioso
        }
    }
}

/// Geração de Pier e Passarelas do Lago Paranoá
fn generate_pier(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let ProcessedElement::Way(way) = element {
        let nodes = &way.nodes;
        if nodes.len() < 2 {
            return;
        }

        let pier_width = element
            .tags()
            .get("width")
            .and_then(|w: &String| w.parse::<i32>().ok())
            .unwrap_or(5);

        let pier_height = 2; // Acima da água
        let support_spacing = 5;

        for i in 0..nodes.len() - 1 {
            let start_node = &nodes[i];
            let end_node = &nodes[i + 1];

            let line_points =
                bresenham_line(start_node.x, 0, start_node.z, end_node.x, 0, end_node.z);

            for (index, (center_x, _y, center_z)) in line_points.iter().enumerate() {
                let half_width = pier_width / 2;

                let ground_y = if args.terrain {
                    editor.get_ground_level(*center_x, *center_z)
                } else {
                    0
                };
                let absolute_pier_y = ground_y + pier_height;

                for x in (center_x - half_width)..=(center_x + half_width) {
                    for z in (center_z - half_width)..=(center_z + half_width) {
                        editor.set_block_absolute(OAK_SLAB, x, absolute_pier_y, z, None, None);
                    }
                }

                // Pilares fincados até o chão
                if index % support_spacing == 0 {
                    let support_positions = [
                        (center_x - half_width, center_z),
                        (center_x + half_width, center_z),
                    ];

                    for (pillar_x, pillar_z) in support_positions {
                        let bottom_y = if args.terrain {
                            editor.get_ground_level(pillar_x, *pillar_z)
                        } else {
                            0
                        };
                        // Garante que o pilar atravesse a água
                        let start_pillar = bottom_y.min(absolute_pier_y - 3);
                        for y in start_pillar..absolute_pier_y {
                            editor.set_block_absolute(OAK_LOG, pillar_x, y, *pillar_z, None, None);
                        }
                    }
                }
            }
        }
    }
}

/// Antenas de celular e rádio (Torres treliçadas metálicas)
fn generate_antenna(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        let height = match element.tags().get("height") {
            Some(h) => (h.parse::<f64>().unwrap_or(30.0) * V_SCALE) as i32,
            None => 40,
        }
            .min(80);

        editor.set_block_absolute(IRON_BLOCK, x, ground_y + 1, z, None, None);
        for y in 2..height {
            editor.set_block_absolute(IRON_BARS, x, ground_y + y, z, None, None);
        }

        for y in (10..height).step_by(10) {
            editor.set_block_absolute(IRON_BLOCK, x, ground_y + y, z, Some(&[IRON_BARS]), None);
            let support_positions = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            for (dx, dz) in support_positions {
                editor.set_block_absolute(IRON_BLOCK, x + dx, ground_y + y, z + dz, None, None);
            }
        }

        // Casa de máquinas da base
        editor.fill_blocks(
            LIGHT_GRAY_CONCRETE,
            x - 2,
            ground_y + 1,
            z - 2,
            x + 2,
            ground_y + 3,
            z + 2,
            Some(&[LIGHT_GRAY_CONCRETE]),
            None,
        );
    }
}

/// Torres Massivas (Torre de TV / Torres de Observação)
fn generate_tower(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        let tower_type = element
            .tags()
            .get("tower:type")
            .map(|s: &String| s.as_str())
            .unwrap_or("");

        let height = match element.tags().get("height") {
            Some(h) => (h.parse::<f64>().unwrap_or(50.0) * V_SCALE) as i32,
            None => {
                if tower_type == "observation" {
                    120
                } else {
                    60
                }
            }
        };

        if tower_type == "communication" || tower_type == "observation" {
            // 🚨 BESM-6: Geometria Treliçada Dinâmica (Conic/Eiffel Shape)
            let base_radius = (height / 8).clamp(3, 15);

            for y in 0..height {
                let current_y = ground_y + y;
                // Raio diminui conforme sobe
                let current_radius =
                    (base_radius as f64 * (1.0 - (y as f64 / height as f64))).max(1.0) as i32;

                for dx in -current_radius..=current_radius {
                    for dz in -current_radius..=current_radius {
                        let dist_sq = dx * dx + dz * dz;

                        // Parede externa do cone
                        if dist_sq <= current_radius * current_radius
                            && dist_sq >= (current_radius - 1) * (current_radius - 1)
                        {
                            if y % 5 == 0 {
                                editor.set_block_absolute(
                                    IRON_BLOCK,
                                    x + dx,
                                    current_y,
                                    z + dz,
                                    None,
                                    None,
                                ); // Anel de travamento
                            } else if (dx + dz + y) % 3 == 0 {
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x + dx,
                                    current_y,
                                    z + dz,
                                    None,
                                    None,
                                ); // Treliça
                            }
                        }
                    }
                }
            }

            // Se for torre de observação, cria um disco (mirante) a 70% da altura
            if tower_type == "observation" {
                let mirante_y = ground_y + (height as f64 * 0.7) as i32;
                let deck_radius = base_radius + 2;
                for dx in -deck_radius..=deck_radius {
                    for dz in -deck_radius..=deck_radius {
                        if dx * dx + dz * dz <= deck_radius * deck_radius {
                            editor.set_block_absolute(
                                SMOOTH_STONE,
                                x + dx,
                                mirante_y,
                                z + dz,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                SMOOTH_STONE,
                                x + dx,
                                mirante_y + 3,
                                z + dz,
                                None,
                                None,
                            ); // Teto
                            if dx * dx + dz * dz >= (deck_radius - 1) * (deck_radius - 1) {
                                editor.set_block_absolute(
                                    GLASS_PANE,
                                    x + dx,
                                    mirante_y + 1,
                                    z + dz,
                                    None,
                                    None,
                                ); // Vidro do Mirante
                                editor.set_block_absolute(
                                    GLASS_PANE,
                                    x + dx,
                                    mirante_y + 2,
                                    z + dz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
            }
        } else {
            // Torre Genérica (Pilar de Pedra)
            editor.fill_blocks(
                STONE_BRICKS,
                x - 1,
                ground_y,
                z - 1,
                x + 1,
                ground_y + height,
                z + 1,
                None,
                None,
            );
        }
    }
}

/// Reservatórios da CAESB e Tanques Industriais (SIA)
fn generate_tank(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let area = match element {
        ProcessedElement::Way(way) => flood_fill_cache.get_or_compute(way, args.timeout.as_ref()),
        ProcessedElement::Relation(rel) => {
            // Busca o membro outer para preencher
            if let Some(outer) = rel
                .members
                .iter()
                .find(|m| m.role == crate::osm_parser::ProcessedMemberRole::Outer)
            {
                flood_fill_cache.get_or_compute(&outer.way, args.timeout.as_ref())
            } else {
                Vec::new()
            }
        }
        _ => return,
    };

    if area.is_empty() {
        return;
    }

    let height = match element.tags().get("height") {
        Some(h) => (h.parse::<f64>().unwrap_or(12.0) * V_SCALE) as i32,
        None => 10,
    };

    let material = match element.tags().get("material").map(|s: &String| s.as_str()) {
        Some("metal" | "steel") => IRON_BLOCK,
        _ => WHITE_CONCRETE, // Padrão CAESB
    };

    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;

    for &(px, pz) in &area {
        if px < min_x {
            min_x = px;
        }
        if px > max_x {
            max_x = px;
        }
        if pz < min_z {
            min_z = pz;
        }
        if pz > max_z {
            max_z = pz;
        }
    }

    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;
    let base_y = if args.terrain {
        editor.get_ground_level(cx, cz)
    } else {
        0
    };

    // Constrói o cilindro/polígono sólido extrudando o footprint exato
    for &(px, pz) in &area {
        let is_edge = !area.contains(&(px + 1, pz))
            || !area.contains(&(px - 1, pz))
            || !area.contains(&(px, pz + 1))
            || !area.contains(&(px, pz - 1));

        for y in 0..height {
            let block = if is_edge { material } else { WATER }; // Tanques são cheios d'água por dentro
            editor.set_block_absolute(block, px, base_y + y, pz, None, None);
        }
        // Tampa do reservatório plana
        editor.set_block_absolute(material, px, base_y + height, pz, None, None);

        // Borda extra para telhado (Beiral)
        if is_edge {
            editor.set_block_absolute(STONE_BRICK_SLAB, px, base_y + height + 1, pz, None, None);
        }
    }
}

/// Infraestrutura Menor: Postes de Posteamento da CEB
fn generate_utility_pole(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        let height = (8.0 * V_SCALE).round() as i32; // ~9 blocos (Poste CEB padrão)
        let mat = match element.tags().get("material").map(|s: &String| s.as_str()) {
            Some("wood") => SPRUCE_LOG,
            Some("metal") => IRON_BLOCK,
            _ => POLISHED_ANDESITE, // Concreto armado padrão CEB
        };

        for y in 1..=height {
            editor.set_block_absolute(mat, x, ground_y + y, z, None, None);
        }

        // Braço do Poste (Luminária ou Fiação)
        editor.set_block_absolute(IRON_BARS, x + 1, ground_y + height - 1, z, None, None);
        editor.set_block_absolute(IRON_BARS, x - 1, ground_y + height - 1, z, None, None);
        editor.set_block_absolute(GLOWSTONE, x + 1, ground_y + height - 2, z, None, None);
    }
}

/// Infraestrutura Menor: Armários de Telecom / Semáforos Terrestres
fn generate_street_cabinet(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        editor.set_block_absolute(IRON_BLOCK, x, ground_y + 1, z, None, None);
        editor.set_block_absolute(SMOOTH_STONE_SLAB, x, ground_y + 2, z, None, None);
    }
}

/// Estações de Tratamento de Esgoto (CAESB) e Complexos Industriais
fn generate_industrial_works(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let area = match element {
        ProcessedElement::Way(way) => flood_fill_cache.get_or_compute(way, args.timeout.as_ref()),
        _ => return,
    };

    if area.is_empty() {
        return;
    }

    let cx = area[0].0;
    let cz = area[0].1;
    let base_y = if args.terrain {
        editor.get_ground_level(cx, cz)
    } else {
        0
    };

    // Planta de tratamento genérica (Piso de concreto e tanques abertos de água)
    for &(px, pz) in &area {
        let noise = ((px as f64 * 0.3).sin() * (pz as f64 * 0.3).cos()).abs();

        // Chão de concreto industrial
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, px, base_y, pz, None, None);

        // Tanques de aeração (buracos com água) e Pás Mecânicas (Estética Industrial CAESB)
        if noise > 0.8 {
            editor.set_block_absolute(WATER, px, base_y, pz, None, None);
            editor.set_block_absolute(WATER, px, base_y - 1, pz, None, None);
            editor.set_block_absolute(STONE_BRICKS, px + 1, base_y, pz, None, None); // Borda

            // Pás de aeração mecânica giratórias do tratamento de esgoto
            if (px + pz) % 7 == 0 {
                editor.set_block_absolute(IRON_BARS, px, base_y + 1, pz, None, None);
                editor.set_block_absolute(GRINDSTONE, px, base_y + 2, pz, None, None);
            }
        }
    }
}

/// Chaminés Isoladas
fn generate_chimney(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        let height = match element.tags().get("height") {
            Some(h) => (h.parse::<f64>().unwrap_or(35.0) * V_SCALE) as i32,
            None => 40,
        };

        for y in 0..height {
            for dx in -2i32..=2i32 {
                for dz in -2i32..=2i32 {
                    if dx.abs() <= 1 && dz.abs() <= 1 {
                        continue;
                    }
                    editor.set_block_absolute(BRICK, x + dx, ground_y + y, z + dz, None, None);
                }
            }
        }
    }
}

/// Poços de Água Rurais
fn generate_water_well(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        for dx in -1i32..=1i32 {
            for dz in -1i32..=1i32 {
                if dx == 0 && dz == 0 {
                    editor.set_block_absolute(WATER, x, ground_y - 1, z, None, None);
                    editor.set_block_absolute(WATER, x, ground_y, z, None, None);
                } else {
                    editor.set_block_absolute(STONE_BRICKS, x + dx, ground_y, z + dz, None, None);
                    editor.set_block_absolute(
                        STONE_BRICKS,
                        x + dx,
                        ground_y + 1,
                        z + dz,
                        None,
                        None,
                    );
                }
            }
        }

        editor.fill_blocks(
            OAK_LOG,
            x - 2,
            ground_y + 1,
            z,
            x - 2,
            ground_y + 4,
            z,
            None,
            None,
        );
        editor.fill_blocks(
            OAK_LOG,
            x + 2,
            ground_y + 1,
            z,
            x + 2,
            ground_y + 4,
            z,
            None,
            None,
        );
        editor.set_block_absolute(OAK_SLAB, x - 1, ground_y + 5, z, None, None);
        editor.set_block_absolute(OAK_FENCE, x, ground_y + 4, z, None, None);
        editor.set_block_absolute(OAK_SLAB, x, ground_y + 5, z, None, None);
        editor.set_block_absolute(OAK_SLAB, x + 1, ground_y + 5, z, None, None);
        editor.set_block_absolute(IRON_BLOCK, x, ground_y + 3, z, None, None);
    }
}

/// Torres de Água da CAESB Elevadas
fn generate_water_tower(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let ground_y = if args.terrain {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        let tower_height = (25.0 * V_SCALE) as i32;
        let tank_height = (8.0 * V_SCALE) as i32;

        let leg_positions = [(-3, -3), (3, -3), (-3, 3), (3, 3)];
        for (dx, dz) in leg_positions {
            for y in 0..tower_height {
                editor.set_block_absolute(
                    LIGHT_GRAY_CONCRETE,
                    x + dx,
                    ground_y + y,
                    z + dz,
                    None,
                    None,
                );
            }
        }

        for y in (7..tower_height).step_by(7) {
            for dx in -2i32..=2i32 {
                editor.set_block_absolute(STONE_BRICKS, x + dx, ground_y + y, z - 3, None, None);
                editor.set_block_absolute(STONE_BRICKS, x + dx, ground_y + y, z + 3, None, None);
            }
            for dz in -2i32..=2i32 {
                editor.set_block_absolute(STONE_BRICKS, x - 3, ground_y + y, z + dz, None, None);
                editor.set_block_absolute(STONE_BRICKS, x + 3, ground_y + y, z + dz, None, None);
            }
        }

        editor.fill_blocks(
            WHITE_CONCRETE,
            x - 4,
            ground_y + tower_height,
            z - 4,
            x + 4,
            ground_y + tower_height + tank_height,
            z + 4,
            None,
            None,
        );

        for y in 0..tower_height {
            editor.set_block_absolute(CYAN_TERRACOTTA, x, ground_y + y, z, None, None); // Encanamento mestre azul
            editor.set_block_absolute(CYAN_TERRACOTTA, x + 1, ground_y + y, z, None, None);
        }
    }
}

pub fn generate_man_made_nodes(editor: &mut WorldEditor, node: &ProcessedNode, args: &Args) {
    if let Some(man_made_type) = node.tags.get("man_made") {
        let element = ProcessedElement::Node(node.clone());

        match man_made_type.as_str() {
            "antenna" | "mast" => generate_antenna(editor, &element, args),
            "tower" => generate_tower(editor, &element, args),
            "chimney" => generate_chimney(editor, &element, args),
            "water_well" => generate_water_well(editor, &element, args),
            "water_tower" => generate_water_tower(editor, &element, args),
            "street_cabinet" => generate_street_cabinet(editor, &element, args),
            "utility_pole" => generate_utility_pole(editor, &element, args),
            _ => {}
        }
    }
}

// ============================================================================
// 🚨 BESM-6: CAESB Infrastructure Generator (Indoor Utility Provider Integration)
// ============================================================================

/// Gera infraestrutura subterrânea a partir de Features do IndoorUtilityProvider.
/// Processa dutos, galerias, tubulações e redes de esgoto/água da CAESB.
pub fn generate_from_provider_feature(editor: &mut WorldEditor, feature: &Feature, args: &Args) {
    match feature.semantic_group {
        SemanticGroup::Sanitation | SemanticGroup::Utility | SemanticGroup::Sewage => {
            generate_underground_pipeline(editor, feature, args);
        }
        SemanticGroup::Power | SemanticGroup::Telecom => {
            generate_underground_cable(editor, feature, args);
        }
        SemanticGroup::Indoor => {
            generate_indoor_structure(editor, feature, args);
        }
        _ => {} // Outros grupos semânticos são ignorados neste módulo
    }
}

/// Gera tubulações subterrâneas (água, esgoto, drenagem) da CAESB.
fn generate_underground_pipeline(editor: &mut WorldEditor, feature: &Feature, args: &Args) {
    // Extrai diâmetro da tubulação (se disponível)
    let diameter = feature
        .get_tag("diameter")
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.5); // Padrão: 50cm

    // Converte diâmetro em metros para blocos (com escala 1.15 vertical)
    let pipe_radius = ((diameter * 1.15) / 2.0).ceil().max(1.0) as i32;

    // Determina profundidade (level negativo ou padrão -3 blocos)
    let depth_offset = feature
        .get_tag("level")
        .and_then(|l| l.parse::<i32>().ok())
        .map(|level| if level < 0 { level * 4 } else { -3 })
        .unwrap_or(-3);

    // Determina material baseado no tipo de utilidade
    let pipe_material = if let Some(utility) = feature.get_tag("utility") {
        let util_lower = utility.to_lowercase();
        if util_lower.contains("sewer") || util_lower.contains("esgoto") {
            STONE_BRICKS // Esgoto: tijolo de pedra
        } else if util_lower.contains("water") || util_lower.contains("agua") {
            CYAN_TERRACOTTA // Água: terracota ciano
        } else {
            POLISHED_ANDESITE // Genérico: andesito polido
        }
    } else {
        POLISHED_ANDESITE
    };

    match &feature.geometry {
        GeometryType::LineString(points) => {
            // Gera tubulação ao longo da linha
            for i in 0..points.len().saturating_sub(1) {
                let start = &points[i];
                let end = &points[i + 1];

                let line_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

                for (px, _, pz) in line_points {
                    let base_y = if args.terrain {
                        editor.get_ground_level(px, pz)
                    } else {
                        args.ground_level
                    };

                    let pipe_y = base_y + depth_offset;

                    // Gera seção circular da tubulação
                    for dy in -pipe_radius..=pipe_radius {
                        for dx in -pipe_radius..=pipe_radius {
                            if dx * dx + dy * dy <= pipe_radius * pipe_radius {
                                editor.set_block_absolute(
                                    pipe_material,
                                    px + dx,
                                    pipe_y + dy,
                                    pz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
            }
        }
        GeometryType::Point(pt) => {
            // Caixas de inspeção, válvulas, poços de visita
            let base_y = if args.terrain {
                editor.get_ground_level(pt.x, pt.z)
            } else {
                args.ground_level
            };

            let chamber_y = base_y + depth_offset;

            // Cria uma câmara cúbica 3x3x3
            for dx in -1i32..=1i32 {
                for dy in -1i32..=1i32 {
                    for dz in -1i32..=1i32 {
                        let is_wall = dx.abs() == 1 || dy.abs() == 1 || dz.abs() == 1;
                        let block = if is_wall {
                            pipe_material
                        } else {
                            AIR // Interior vazio
                        };
                        editor.set_block_absolute(
                            block,
                            pt.x + dx,
                            chamber_y + dy,
                            pt.z + dz,
                            None,
                            None,
                        );
                    }
                }
            }

            // Poço de acesso vertical até a superfície
            for y in chamber_y..base_y {
                editor.set_block_absolute(LADDER, pt.x, y, pt.z, None, None);
            }
            // Tampa de inspeção na superfície - 🚨 BESM-6: Utilizando IRON_TRAPDOOR
            editor.set_block_absolute(IRON_TRAPDOOR, pt.x, base_y, pt.z, None, None);
        }
        GeometryType::Polygon(points) => {
            // Estações de tratamento, reservatórios subterrâneos, galerias
            if points.len() < 4 {
                return;
            }

            let base_y = if args.terrain {
                let cx = points.iter().map(|p| p.x).sum::<i32>() / points.len() as i32;
                let cz = points.iter().map(|p| p.z).sum::<i32>() / points.len() as i32;
                editor.get_ground_level(cx, cz)
            } else {
                args.ground_level
            };

            let chamber_y = base_y + depth_offset;
            let chamber_height = 4; // Altura padrão de 4 blocos para galerias

            // Gera outline da galeria
            for i in 0..points.len().saturating_sub(1) {
                let start = &points[i];
                let end = &points[i + 1];

                let wall_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

                for (px, _, pz) in wall_points {
                    // Paredes laterais
                    for dy in 0..chamber_height {
                        editor.set_block_absolute(
                            pipe_material,
                            px,
                            chamber_y + dy,
                            pz,
                            None,
                            None,
                        );
                    }
                    // Piso
                    editor.set_block_absolute(pipe_material, px, chamber_y - 1, pz, None, None);
                    // Teto
                    editor.set_block_absolute(
                        pipe_material,
                        px,
                        chamber_y + chamber_height,
                        pz,
                        None,
                        None,
                    );
                }
            }
        }
        _ => {} // MultiPolygon não é esperado aqui
    }
}

/// Gera cabos subterrâneos (energia elétrica, fibra óptica).
fn generate_underground_cable(editor: &mut WorldEditor, feature: &Feature, args: &Args) {
    let depth_offset = -2; // Cabos são mais rasos que tubulações

    let cable_material = match feature.semantic_group {
        SemanticGroup::Power => ORANGE_TERRACOTTA, // Energia: laranja
        SemanticGroup::Telecom => BLUE_TERRACOTTA, // Telecom: azul
        _ => GRAY_TERRACOTTA,
    };

    if let GeometryType::LineString(points) = &feature.geometry {
        for i in 0..points.len().saturating_sub(1) {
            let start = &points[i];
            let end = &points[i + 1];

            let line_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

            for (px, _, pz) in line_points {
                let base_y = if args.terrain {
                    editor.get_ground_level(px, pz)
                } else {
                    args.ground_level
                };

                let cable_y = base_y + depth_offset;

                // Cabo fino (1 bloco)
                editor.set_block_absolute(cable_material, px, cable_y, pz, None, None);
            }
        }
    }
}

/// Gera estruturas internas (salas, corredores, níveis indoor).
fn generate_indoor_structure(editor: &mut WorldEditor, feature: &Feature, args: &Args) {
    // Indoor structures são polígonos representando plantas baixas
    if let GeometryType::Polygon(points) = &feature.geometry {
        if points.len() < 4 {
            return;
        }

        let level = feature
            .get_tag("level")
            .and_then(|l| l.parse::<i32>().ok())
            .unwrap_or(0);

        let room_height = feature
            .get_tag("height")
            .and_then(|h| h.parse::<i32>().ok())
            .unwrap_or(3); // Altura padrão de 3 blocos

        let base_y = if args.terrain {
            let cx = points.iter().map(|p| p.x).sum::<i32>() / points.len() as i32;
            let cz = points.iter().map(|p| p.z).sum::<i32>() / points.len() as i32;
            editor.get_ground_level(cx, cz)
        } else {
            args.ground_level
        };

        let floor_y = base_y + (level * 4); // 4 blocos por andar

        // Gera paredes da sala
        for i in 0..points.len().saturating_sub(1) {
            let start = &points[i];
            let end = &points[i + 1];

            let wall_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

            for (px, _, pz) in wall_points {
                // Piso
                editor.set_block_absolute(SMOOTH_STONE, px, floor_y, pz, None, None);

                // Paredes
                for dy in 1..room_height {
                    editor.set_block_absolute(WHITE_CONCRETE, px, floor_y + dy, pz, None, None);
                }

                // Teto
                editor.set_block_absolute(SMOOTH_STONE, px, floor_y + room_height, pz, None, None);
            }
        }
    }
}