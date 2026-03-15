use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Type alias for highway connectivity map
pub type HighwayConnectivityMap = HashMap<(i32, i32), Vec<i32>>;

/// Minimum terrain dip (in blocks) below max endpoint elevation to classify a bridge as valley-spanning
const VALLEY_BRIDGE_THRESHOLD: i32 = 5;

// --- OTIMIZAÇÃO: ARRAYS GLOBAIS DE BLOCOS PROTEGIDOS (Impede recriação no Heap em O(n^2)) ---
const PROTECTED_BLOCKS: &[Block] = &[
    WHITE_TERRACOTTA,
    LIGHT_GRAY_TERRACOTTA,
    YELLOW_TERRACOTTA,
    BRICK,
    MUD_BRICKS,
    SMOOTH_QUARTZ,
    WATER,
    GLASS,
    GLASS_PANE,
    OAK_DOOR,
    IRON_DOOR,
    STONE_BRICKS,
    ANDESITE_WALL,
    COBBLESTONE_WALL,
    BRICK_WALL,
    STONE_BRICK_WALL,
    OAK_PLANKS,
    WHITE_CONCRETE,
    POLISHED_BASALT,
    ACACIA_LOG,
    OAK_LOG,
    LEAVES,
    OAK_LEAVES,
    JUNGLE_LEAVES,
    IRON_BLOCK,
    IRON_BARS,
    STONE_STAIRS,
    OAK_STAIRS,
];

const SAFE_FOR_SIDEWALK: &[Block] = &[
    GRASS_BLOCK,
    DIRT,
    COARSE_DIRT,
    PODZOL,
    AIR,
    GRAY_CONCRETE,
    BLACK_CONCRETE,
    GRAY_TERRACOTTA,
    TALL_GRASS,
    FERN,
];

// --- SISTEMA DE TIPOLOGIA VIÁRIA DO DF ---
#[derive(PartialEq)]
enum DFRoadType {
    Eixao,                // DF-002 (Pistas separadas por canteiro)
    W3,                   // Comercial (Estacionamentos espinha de peixe)
    L2L4,                 // Residenciais Largas
    Monumental,           // Eixo Monumental (S1/N1)
    ExpressaDF,           // EPIA / EPTG / Estrutural
    Arterial,             // Primary genérica
    Coletora,             // Secondary genérica
    Tesourinha,           // Retornos e motorway_links
    Rotatoria,            // Balões
    ViaSuperquadra,       // Vias locais do Plano Piloto (MUITO verde ao redor)
    ViaComercialSatelite, // TWEAK RP: Avenidas largas de Taguatinga/Ceilândia (Hélio Prates, Comercial)
    ViaGuara,             // Vias de Cidades-Satélites (Casas coladas na rua)
    ViaLocal,             // Fallback residencial
    Generic(String),      // Fallback OSM
}

fn detect_df_road(way: &ProcessedWay, base_highway: &str) -> DFRoadType {
    let name = way
        .tags
        .get("name")
        .map(|s: &String| s.to_lowercase())
        .unwrap_or_default();
    let ref_tag = way
        .tags
        .get("ref")
        .map(|s: &String| s.to_uppercase())
        .unwrap_or_default();
    let junction = way
        .tags
        .get("junction")
        .map(|s: &String| s.as_str())
        .unwrap_or("");
    let suburb = way
        .tags
        .get("addr:suburb")
        .map(|s: &String| s.to_lowercase())
        .unwrap_or_default();
    let is_in = way
        .tags
        .get("is_in")
        .map(|s: &String| s.to_lowercase())
        .unwrap_or_default();
    let place = way
        .tags
        .get("place")
        .map(|s: &String| s.to_lowercase())
        .unwrap_or_default();

    if junction == "roundabout" {
        return DFRoadType::Rotatoria;
    }

    if base_highway.contains("link") {
        return DFRoadType::Tesourinha;
    }

    if ref_tag.contains("DF-002") || name.contains("eixo rodoviário") || name.contains("eixão") {
        return DFRoadType::Eixao;
    }

    if name.contains("eixo monumental")
        || name.contains("via s1")
        || name.contains("via n1")
        || name.contains("esplanada")
    {
        return DFRoadType::Monumental;
    }

    if name.contains("w3 sul") || name.contains("w3 norte") {
        return DFRoadType::W3;
    }

    if name.contains("l2") || name.contains("l4") {
        return DFRoadType::L2L4;
    }

    if ref_tag.contains("DF-003")
        || ref_tag.contains("DF-085")
        || ref_tag.contains("DF-095")
        || name.contains("epia")
        || name.contains("eptg")
        || name.contains("estrutural")
    {
        return DFRoadType::ExpressaDF;
    }

    // HEURÍSTICA ESTRUTURAL DE FALHA DO OSM
    if base_highway == "primary" || base_highway == "trunk" {
        return DFRoadType::Arterial;
    }
    if base_highway == "secondary" {
        return DFRoadType::Coletora;
    }

    // HEURÍSTICA DE MORFOLOGIA URBANA (Aprimorada com place=suburb)
    if base_highway == "residential"
        || base_highway == "living_street"
        || base_highway == "tertiary"
    {
        let in_plano_piloto = name.contains("sqs")
            || name.contains("sqn")
            || suburb.contains("asa sul")
            || suburb.contains("asa norte")
            || is_in.contains("plano piloto");
        let in_satelite = name.contains("guará")
            || suburb.contains("guara")
            || place.contains("guará")
            || suburb.contains("ceilândia")
            || suburb.contains("taguatinga")
            || is_in.contains("guará");

        if in_plano_piloto {
            return DFRoadType::ViaSuperquadra;
        } else if in_satelite {
            // Detecção da Nova Tipologia Comercial de Satélite
            if name.contains("comercial")
                || name.contains("hélio prates")
                || name.contains("central")
                || name.contains("samdu")
                || name.contains("sandu")
            {
                return DFRoadType::ViaComercialSatelite;
            }
            return DFRoadType::ViaGuara;
        }
        return DFRoadType::ViaLocal;
    }

    DFRoadType::Generic(base_highway.to_string())
}
// -----------------------------------------

pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
) {
    generate_highways_internal(
        editor,
        element,
        args,
        highway_connectivity,
        flood_fill_cache,
    );
}

pub fn build_highway_connectivity_map(elements: &[ProcessedElement]) -> HighwayConnectivityMap {
    let mut connectivity_map: HashMap<(i32, i32), Vec<i32>> = HashMap::new();

    for element in elements {
        if let ProcessedElement::Way(way) = element {
            if way.tags.contains_key("highway") {
                let layer_value = way
                    .tags
                    .get("layer")
                    .and_then(|layer: &String| layer.parse::<i32>().ok())
                    .unwrap_or(0);

                let layer_value = if layer_value < 0 { 0 } else { layer_value };

                if !way.nodes.is_empty() {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];

                    let start_coord = (start_node.x, start_node.z);
                    let end_coord = (end_node.x, end_node.z);

                    connectivity_map
                        .entry(start_coord)
                        .or_default()
                        .push(layer_value);
                    connectivity_map
                        .entry(end_coord)
                        .or_default()
                        .push(layer_value);
                }
            }
        }
    }

    connectivity_map
}

fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
    flood_fill_cache: &FloodFillCache,
) {
    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                let ground_y = editor.get_ground_level(x, z);

                editor.set_block_absolute(POLISHED_ANDESITE, x, ground_y + 1, z, None, None);
                for dy in 2i32..=7i32 {
                    editor.set_block_absolute(IRON_BARS, x, ground_y + dy, z, None, None);
                }
                editor.set_block_absolute(GLOWSTONE, x, ground_y + 8, z, None, None);
            }
        } else if highway_type == "crossing" {
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x: i32 = node.x;
                        let z: i32 = node.z;
                        let ground_y = editor.get_ground_level(x, z);

                        for dy in 1i32..=4i32 {
                            editor.set_block_absolute(
                                ANDESITE_WALL,
                                x,
                                ground_y + dy,
                                z,
                                None,
                                None,
                            );
                        }

                        editor.set_block_absolute(GREEN_WOOL, x, ground_y + 5, z, None, None);
                        editor.set_block_absolute(YELLOW_WOOL, x, ground_y + 6, z, None, None);
                        editor.set_block_absolute(RED_WOOL, x, ground_y + 7, z, None, None);
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                let ground_y = editor.get_ground_level(x, z);

                for dy in 1i32..=3i32 {
                    editor.set_block_absolute(IRON_BARS, x, ground_y + dy, z, None, None);
                    editor.set_block_absolute(IRON_BARS, x + 2, ground_y + dy, z, None, None);
                }
                for dx in 0i32..=2i32 {
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        x + dx,
                        ground_y + 4,
                        z,
                        None,
                        None,
                    );
                    if dx == 1 {
                        editor.set_block_absolute(
                            GRAY_STAINED_GLASS,
                            x + dx,
                            ground_y + 3,
                            z,
                            None,
                            None,
                        );
                    }
                }
            }
        } else if element
            .tags()
            .get("area")
            .is_some_and(|v: &String| v.as_str() == "yes")
        {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            let mut surface_block: Block = GRAY_CONCRETE;

            if let Some(surface) = element.tags().get("surface") {
                surface_block = match surface.as_str() {
                    "paving_stones" | "sett" => STONE_BRICKS,
                    "bricks" => BRICK,
                    "wood" => OAK_PLANKS,
                    "asphalt" => BLACK_CONCRETE,
                    "gravel" | "fine_gravel" => GRAVEL,
                    "grass" => GRASS_BLOCK,
                    "dirt" | "ground" | "earth" => DIRT,
                    "sand" => SAND,
                    "concrete" => LIGHT_GRAY_CONCRETE,
                    _ => GRAY_CONCRETE,
                };
            }

            let filled_area: Vec<(i32, i32)> =
                flood_fill_cache.get_or_compute(way, args.timeout.as_ref());

            for (x, z) in filled_area {
                let ground_y = editor.get_ground_level(x, z);
                editor.set_block_absolute(surface_block, x, ground_y, z, None, None);
            }
        } else {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            let df_road_type = detect_df_road(way, highway_type);

            let mut previous_node: Option<(i32, i32)> = None;
            let mut block_type = GRAY_CONCRETE;
            let mut block_range: i32 = 2;
            let mut grass_buffer: i32 = 0;
            let mut parking_lane: bool = false;
            let mut add_stripe = false;
            let mut add_outline = false;

            let mut physical_median_radius: i32 = 0;
            let mut is_detached_sidewalk = false;

            let scale_factor = args.scale;

            let is_indoor = element
                .tags()
                .get("indoor")
                .is_some_and(|v: &String| v.as_str() == "yes");
            let is_bridge = !is_indoor
                && element
                    .tags()
                    .get("bridge")
                    .is_some_and(|v: &String| v.as_str() != "no");

            let mut layer_value = element
                .tags()
                .get("layer")
                .and_then(|layer: &String| layer.parse::<i32>().ok())
                .unwrap_or(0);

            if layer_value < 0 || is_indoor {
                layer_value = 0;
            }

            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            // --- LARGURAS MONUMENTAIS E TIPOLOGIA DF (REFINADAS PARA 1.33H) ---
            match df_road_type {
                DFRoadType::Eixao => {
                    block_type = BLACK_CONCRETE;
                    block_range = 18; // TWEAK GPT: Eixão largo o suficiente para 3 pistas cada lado
                    add_stripe = true;
                    grass_buffer = 8;
                    physical_median_radius = 6; // Canteiro Largo do Eixão
                    is_detached_sidewalk = true;
                }
                DFRoadType::Monumental => {
                    block_type = BLACK_CONCRETE;
                    block_range = 26;
                    add_stripe = true;
                    grass_buffer = 12;
                    physical_median_radius = 10; // Gramadão Central da Esplanada
                    is_detached_sidewalk = true;
                }
                DFRoadType::ExpressaDF => {
                    block_type = BLACK_CONCRETE;
                    block_range = 12;
                    add_stripe = true;
                    grass_buffer = 5;
                    physical_median_radius = 1; // Barreira New Jersey (Mureta)
                }
                DFRoadType::L2L4 | DFRoadType::Arterial => {
                    block_type = BLACK_CONCRETE;
                    block_range = 9;
                    add_stripe = true;
                    grass_buffer = 5;
                    physical_median_radius = 1;
                    is_detached_sidewalk = true;
                }
                DFRoadType::W3 => {
                    block_type = POLISHED_BASALT; // Asfalto diferente para diferenciar W3
                    block_range = 8;
                    parking_lane = true;
                    add_stripe = true;
                    grass_buffer = 2;
                }
                DFRoadType::ViaComercialSatelite => {
                    block_type = GRAY_CONCRETE;
                    block_range = 7;
                    parking_lane = true;
                    add_stripe = true;
                    grass_buffer = 0;
                    is_detached_sidewalk = false;
                }
                DFRoadType::Coletora => {
                    block_type = GRAY_CONCRETE;
                    block_range = 6;
                    add_stripe = true;
                    grass_buffer = 2;
                }
                DFRoadType::Tesourinha => {
                    block_type = BLACK_CONCRETE;
                    block_range = 4;
                    add_stripe = false;
                    grass_buffer = 2;
                    add_outline = true;
                }
                DFRoadType::Rotatoria => {
                    block_type = BLACK_CONCRETE;
                    block_range = 6;
                    add_stripe = true;
                    grass_buffer = 2;
                    add_outline = true;
                }
                DFRoadType::ViaSuperquadra => {
                    block_type = GRAY_TERRACOTTA;
                    block_range = 4;
                    parking_lane = true;
                    grass_buffer = 5;
                    is_detached_sidewalk = true;
                }
                DFRoadType::ViaGuara | DFRoadType::ViaLocal => {
                    block_type = GRAY_CONCRETE;
                    block_range = 3;
                    parking_lane = true;
                    grass_buffer = 0;
                    is_detached_sidewalk = false;
                }
                DFRoadType::Generic(ref t) => match t.as_str() {
                    "footway" | "pedestrian" => {
                        block_type = POLISHED_ANDESITE;
                        block_range = 1;
                    }
                    "path" => {
                        block_type = COARSE_DIRT;
                        block_range = 1;
                    }
                    "track" => {
                        block_type = DIRT_PATH;
                        block_range = 1;
                    }
                    "escape" => {
                        block_type = SAND;
                        block_range = 1;
                    }
                    "steps" => {
                        block_type = STONE_STAIRS;
                        block_range = 1;
                    }
                    _ => {
                        block_type = GRAY_CONCRETE;
                        if let Some(lanes) = element.tags().get("lanes") {
                            if lanes == "2" {
                                block_range = 3;
                                add_stripe = true;
                                add_outline = true;
                            } else if lanes != "1" {
                                block_range = 4;
                                add_stripe = true;
                                add_outline = true;
                            }
                        }
                    }
                },
            }

            if scale_factor.unwrap_or(1.0) < 1.0 {
                block_range = ((block_range as f64) * scale_factor.unwrap_or(1.0)).floor() as i32;
            }

            const LAYER_HEIGHT_STEP: i32 = 5;
            let base_elevation = layer_value * LAYER_HEIGHT_STEP;

            let needs_start_slope =
                should_add_slope_at_node(&way.nodes[0], layer_value, highway_connectivity);
            let needs_end_slope = should_add_slope_at_node(
                &way.nodes[way.nodes.len() - 1],
                layer_value,
                highway_connectivity,
            );

            let total_way_length = calculate_way_length(way);

            let terrain_enabled = editor
                .get_ground()
                .map(|g| g.elevation_enabled)
                .unwrap_or(false);

            let (is_valley_bridge, bridge_deck_y) =
                if is_bridge && terrain_enabled && way.nodes.len() >= 2 && total_way_length >= 25 {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];
                    let start_y = editor.get_ground_level(start_node.x, start_node.z);
                    let end_y = editor.get_ground_level(end_node.x, end_node.z);
                    let max_endpoint_y = start_y.max(end_y);

                    let middle_nodes = &way.nodes[1..way.nodes.len().saturating_sub(1)];
                    let sampled_min = if middle_nodes.is_empty() {
                        start_y.min(end_y)
                    } else {
                        let sample_count = middle_nodes.len().min(3);
                        let step = if sample_count > 1 {
                            (middle_nodes.len() - 1) / (sample_count - 1)
                        } else {
                            1
                        };

                        middle_nodes
                            .iter()
                            .step_by(step.max(1))
                            .map(|node| editor.get_ground_level(node.x, node.z))
                            .min()
                            .unwrap_or(max_endpoint_y)
                    };

                    let min_terrain_y = sampled_min.min(start_y).min(end_y);
                    let is_valley = min_terrain_y < max_endpoint_y - VALLEY_BRIDGE_THRESHOLD;

                    if is_valley {
                        (true, max_endpoint_y)
                    } else {
                        (false, 0)
                    }
                } else {
                    (false, 0)
                };

            let is_short_isolated_elevated =
                needs_start_slope && needs_end_slope && layer_value > 0 && total_way_length <= 35;

            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_short_isolated_elevated {
                    (0, false, false)
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize;

            let mut distance_accumulator = 0;

            for node_idx in 0..way.nodes.len() {
                let node = &way.nodes[node_idx];

                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    let mut stripe_length: i32 = 0;

                    let dash_length: i32 = (3.0 * scale_factor.unwrap_or(1.0)).round() as i32;
                    let gap_length: i32 = (5.0 * scale_factor.unwrap_or(1.0)).round() as i32;

                    // OTIMIZAÇÃO: Cálculo de VETOR NORMAL extraído do loop de Bresenham
                    let dx_segment = (x2 - x1) as f64;
                    let dz_segment = (z2 - z1) as f64;
                    let len_segment = (dx_segment * dx_segment + dz_segment * dz_segment).sqrt();

                    let (norm_x, norm_z) = if len_segment > 0.0 {
                        (-dz_segment / len_segment, dx_segment / len_segment)
                    } else {
                        (1.0, 0.0)
                    };

                    for (_point_index, (bx, _, bz)) in bresenham_points.iter().enumerate() {
                        distance_accumulator += 1;

                        let (current_y, use_absolute_y) = if is_valley_bridge {
                            (bridge_deck_y, true)
                        } else {
                            let y = calculate_point_elevation(
                                distance_accumulator,
                                total_way_length,
                                effective_elevation,
                                effective_start_slope,
                                effective_end_slope,
                                slope_length,
                            );
                            (y, false)
                        };

                        if effective_elevation > 0 && !use_absolute_y {
                            let ground_y = editor.get_ground_level(*bx, *bz);
                            for fill_y in ground_y..current_y {
                                let fill_block = if fill_y % 2 == 0 { STONE } else { COARSE_DIRT };
                                editor.set_block_absolute(fill_block, *bx, fill_y, *bz, None, None);
                            }
                        }

                        let total_brush_range = block_range + grass_buffer;

                        // TWEAK ESTRUTURAL: Brush Circular para Rotatórias
                        if df_road_type == DFRoadType::Rotatoria {
                            for wx in -total_brush_range..=total_brush_range {
                                for wz in -total_brush_range..=total_brush_range {
                                    let dist_sq = wx * wx + wz * wz;

                                    if dist_sq <= total_brush_range * total_brush_range {
                                        let set_x = bx + wx;
                                        let set_z = bz + wz;
                                        let final_paint_y =
                                            if use_absolute_y || effective_elevation > 0 {
                                                current_y
                                            } else {
                                                editor.get_ground_level(set_x, set_z).max(current_y)
                                            };

                                        if dist_sq <= block_range * block_range {
                                            if !editor.check_for_block_absolute(
                                                set_x,
                                                final_paint_y,
                                                set_z,
                                                Some(PROTECTED_BLOCKS),
                                                None,
                                            ) {
                                                editor.set_block_absolute(
                                                    block_type,
                                                    set_x,
                                                    final_paint_y,
                                                    set_z,
                                                    None,
                                                    None,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            continue; // Pula o brush ortogonal
                        }

                        // Brush Chato (Ortogonal) para Vias Normais
                        for w in -total_brush_range..=total_brush_range {
                            let set_x = (*bx as f64 + w as f64 * norm_x).round() as i32;
                            let set_z = (*bz as f64 + w as f64 * norm_z).round() as i32;
                            let dist_from_center = w.abs();

                            // OTIMIZAÇÃO: Um único lookup por Célula Local
                            let local_ground = editor.get_ground_level(set_x, set_z);
                            let final_paint_y = if use_absolute_y || effective_elevation > 0 {
                                current_y
                            } else {
                                local_ground.max(current_y)
                            };

                            // ZONA 1: CANTEIRO FÍSICO CENTRAL (Impede asfalto de invadir)
                            if dist_from_center <= physical_median_radius
                                && !is_bridge
                                && effective_elevation == 0
                            {
                                let median_block = if df_road_type == DFRoadType::ExpressaDF {
                                    ANDESITE_WALL // Mureta New Jersey
                                } else {
                                    GRASS_BLOCK // Gramadão
                                };

                                if !editor.check_for_block_absolute(
                                    set_x,
                                    final_paint_y,
                                    set_z,
                                    Some(PROTECTED_BLOCKS),
                                    None,
                                ) {
                                    editor.set_block_absolute(
                                        median_block,
                                        set_x,
                                        final_paint_y,
                                        set_z,
                                        None,
                                        None,
                                    );
                                    if median_block == GRASS_BLOCK {
                                        editor.set_block_absolute(
                                            DIRT,
                                            set_x,
                                            final_paint_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                }
                                continue; // Pula pintura de asfalto aqui
                            }

                            // ZONA 2: ASFALTO E VAGAS (Ignorando o Canteiro Central)
                            if dist_from_center > physical_median_radius
                                && dist_from_center <= block_range
                            {
                                let mut final_block = block_type;

                                if parking_lane && dist_from_center >= block_range - 2 {
                                    let is_parking_line = distance_accumulator % 4 == 0;
                                    final_block = if is_parking_line {
                                        WHITE_CONCRETE
                                    } else {
                                        block_type
                                    };
                                }

                                if !editor.check_for_block_absolute(
                                    set_x,
                                    final_paint_y,
                                    set_z,
                                    Some(PROTECTED_BLOCKS),
                                    None,
                                ) {
                                    editor.set_block_absolute(
                                        final_block,
                                        set_x,
                                        final_paint_y,
                                        set_z,
                                        None,
                                        None,
                                    );
                                }

                                if (effective_elevation > 0 || use_absolute_y) && current_y > 0 {
                                    editor.set_block_absolute(
                                        POLISHED_ANDESITE,
                                        set_x,
                                        current_y - 1,
                                        set_z,
                                        None,
                                        None,
                                    );
                                    add_highway_support_pillar_absolute(
                                        editor,
                                        set_x,
                                        current_y,
                                        set_z,
                                        w,
                                        0,
                                        block_range,
                                    );
                                }
                            }
                            // ZONA 3: BORDAS E CALÇADAS
                            else if !is_bridge && effective_elevation == 0 {
                                let safe_to_build = editor.check_for_block_absolute(
                                    set_x,
                                    final_paint_y,
                                    set_z,
                                    Some(SAFE_FOR_SIDEWALK),
                                    None,
                                );

                                if safe_to_build
                                    && !editor.check_for_block_absolute(
                                        set_x,
                                        final_paint_y,
                                        set_z,
                                        Some(PROTECTED_BLOCKS),
                                        None,
                                    )
                                {
                                    editor.set_block_absolute(
                                        AIR,
                                        set_x,
                                        final_paint_y + 1,
                                        set_z,
                                        Some(&[TALL_GRASS, FERN]),
                                        None,
                                    );
                                    editor.set_block_absolute(
                                        AIR,
                                        set_x,
                                        final_paint_y + 2,
                                        set_z,
                                        Some(&[TALL_GRASS, FERN]),
                                        None,
                                    );

                                    if is_detached_sidewalk {
                                        if dist_from_center > block_range
                                            && dist_from_center < total_brush_range
                                        {
                                            editor.set_block_absolute(
                                                GRASS_BLOCK,
                                                set_x,
                                                final_paint_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                        if dist_from_center == total_brush_range {
                                            editor.set_block_absolute(
                                                POLISHED_ANDESITE,
                                                set_x,
                                                final_paint_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else {
                                        if dist_from_center == block_range + 1 {
                                            editor.set_block_absolute(
                                                SMOOTH_STONE_SLAB,
                                                set_x,
                                                final_paint_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        } else if dist_from_center == block_range + 2 {
                                            editor.set_block_absolute(
                                                POLISHED_ANDESITE,
                                                set_x,
                                                final_paint_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // --- PINTURA VIÁRIA: BORDAS E FAIXAS ---
                        if add_outline {
                            let outline_w = block_range;
                            let out_x1 = (*bx as f64 + outline_w as f64 * norm_x).round() as i32;
                            let out_z1 = (*bz as f64 + outline_w as f64 * norm_z).round() as i32;
                            let out_x2 = (*bx as f64 - outline_w as f64 * norm_x).round() as i32;
                            let out_z2 = (*bz as f64 - outline_w as f64 * norm_z).round() as i32;

                            let y1 = editor.get_ground_level(out_x1, out_z1);
                            let y2 = editor.get_ground_level(out_x2, out_z2);

                            if !editor.check_for_block_absolute(
                                out_x1,
                                y1,
                                out_z1,
                                Some(PROTECTED_BLOCKS),
                                None,
                            ) {
                                editor.set_block_absolute(
                                    LIGHT_GRAY_CONCRETE,
                                    out_x1,
                                    y1,
                                    out_z1,
                                    None,
                                    None,
                                );
                            }
                            if !editor.check_for_block_absolute(
                                out_x2,
                                y2,
                                out_z2,
                                Some(PROTECTED_BLOCKS),
                                None,
                            ) {
                                editor.set_block_absolute(
                                    LIGHT_GRAY_CONCRETE,
                                    out_x2,
                                    y2,
                                    out_z2,
                                    None,
                                    None,
                                );
                            }
                        }

                        if add_stripe {
                            stripe_length += 1;
                            if stripe_length <= dash_length {
                                // TWEAK DA DUPLICAÇÃO DE VIA
                                if physical_median_radius > 0 {
                                    // Se tem canteiro, a rua é duplicada. As faixas devem ir no meio de cada pista isolada
                                    let dist_faixa = physical_median_radius
                                        + ((block_range - physical_median_radius) / 2);

                                    let fx1 =
                                        (*bx as f64 + dist_faixa as f64 * norm_x).round() as i32;
                                    let fz1 =
                                        (*bz as f64 + dist_faixa as f64 * norm_z).round() as i32;
                                    let y_f1 = editor.get_ground_level(fx1, fz1);
                                    if !editor.check_for_block_absolute(
                                        fx1,
                                        y_f1,
                                        fz1,
                                        Some(PROTECTED_BLOCKS),
                                        None,
                                    ) {
                                        editor.set_block_absolute(
                                            WHITE_CONCRETE,
                                            fx1,
                                            y_f1,
                                            fz1,
                                            None,
                                            None,
                                        );
                                    }

                                    let fx2 =
                                        (*bx as f64 - dist_faixa as f64 * norm_x).round() as i32;
                                    let fz2 =
                                        (*bz as f64 - dist_faixa as f64 * norm_z).round() as i32;
                                    let y_f2 = editor.get_ground_level(fx2, fz2);
                                    if !editor.check_for_block_absolute(
                                        fx2,
                                        y_f2,
                                        fz2,
                                        Some(PROTECTED_BLOCKS),
                                        None,
                                    ) {
                                        editor.set_block_absolute(
                                            WHITE_CONCRETE,
                                            fx2,
                                            y_f2,
                                            fz2,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    // Via Simples (Faixa bem no centro)
                                    let center_x = (*bx as f64 + 0.0 * norm_x).round() as i32;
                                    let center_z = (*bz as f64 + 0.0 * norm_z).round() as i32;
                                    let y_center = editor.get_ground_level(center_x, center_z);

                                    if !editor.check_for_block_absolute(
                                        center_x,
                                        y_center,
                                        center_z,
                                        Some(PROTECTED_BLOCKS),
                                        None,
                                    ) {
                                        editor.set_block_absolute(
                                            WHITE_CONCRETE,
                                            center_x,
                                            y_center,
                                            center_z,
                                            None,
                                            None,
                                        );
                                    }
                                }
                            } else if stripe_length <= dash_length + gap_length {
                                // gap
                            } else {
                                stripe_length = 0;
                            }
                        }
                    }
                }
                previous_node = Some((node.x, node.z));
            }
        }
    }
}

fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
) -> bool {
    let node_coord = (node.x, node.z);

    if highway_connectivity.is_empty() {
        return current_layer != 0;
    }

    if let Some(connected_layers) = highway_connectivity.get(&node_coord) {
        let same_layer_count = connected_layers
            .iter()
            .filter(|&&layer| layer == current_layer)
            .count();

        if same_layer_count <= 1 {
            return current_layer != 0;
        }

        false
    } else {
        current_layer != 0
    }
}

fn calculate_way_length(way: &ProcessedWay) -> usize {
    let mut total_length: f64 = 0.0;
    let mut previous_node: Option<&crate::osm_parser::ProcessedNode> = None;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let dx = (node.x - prev.x) as f64;
            let dz = (node.z - prev.z) as f64;
            total_length += (dx * dx + dz * dz).sqrt();
        }
        previous_node = Some(node);
    }

    total_length.round() as usize
}

fn calculate_point_elevation(
    accumulated_distance: usize,
    total_way_length: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    if !needs_start_slope && !needs_end_slope {
        return base_elevation;
    }

    if total_way_length == 0 || slope_length == 0 {
        return base_elevation;
    }

    if needs_start_slope && accumulated_distance <= slope_length {
        let slope_progress = accumulated_distance as f32 / slope_length as f32;
        return (base_elevation as f32 * slope_progress) as i32;
    }

    if needs_end_slope && accumulated_distance >= (total_way_length.saturating_sub(slope_length)) {
        let distance_from_end = total_way_length - accumulated_distance;
        let slope_progress = distance_from_end as f32 / slope_length as f32;
        return (base_elevation as f32 * slope_progress) as i32;
    }

    base_elevation
}

fn add_highway_support_pillar_absolute(
    editor: &mut WorldEditor,
    x: i32,
    bridge_deck_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32,
) {
    if dx == 0 && dz == 0 && (x + z) % 18 == 0 {
        let ground_y = editor.get_ground_level(x, z);

        // TWEAK: Previne o pilar de nascer na água e afundar até o fundo do rio/lago
        if editor.check_for_block_absolute(x, ground_y, z, Some(&[WATER]), None) {
            return;
        }

        if bridge_deck_y > ground_y {
            for y in (ground_y + 1)..bridge_deck_y {
                editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
            }
        }
    }
}

pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        if let Some(prev_node) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                if !editor.check_for_block(bx, 0, bz, Some(&[BLACK_CONCRETE, WHITE_CONCRETE])) {
                    editor.set_block_absolute(
                        siding_block,
                        bx,
                        editor.get_ground_level(bx, bz) + 1,
                        bz,
                        None,
                        None,
                    );
                }
            }
        }

        previous_node = Some(current_node);
    }
}

pub fn generate_aeroway(editor: &mut WorldEditor, way: &ProcessedWay, args: &Args) {
    let mut previous_node: Option<(i32, i32)> = None;
    let surface_block = LIGHT_GRAY_CONCRETE;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let (x1, z1) = prev;
            let x2 = node.x;
            let z2 = node.z;
            let points = bresenham_line(x1, 0, z1, x2, 0, z2);
            let way_width: i32 = (20.0 * args.scale.unwrap_or(1.0)).ceil() as i32;

            for (x, _, z) in points {
                for dx in -way_width..=way_width {
                    for dz in -way_width..=way_width {
                        let set_x = x + dx;
                        let set_z = z + dz;
                        editor.set_block_absolute(
                            surface_block,
                            set_x,
                            editor.get_ground_level(set_x, set_z),
                            set_z,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        previous_node = Some((node.x, node.z));
    }
}
