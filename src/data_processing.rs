use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, SMOOTH_STONE, STONE, POLISHED_ANDESITE, COARSE_DIRT, RED_TERRACOTTA, WATER, BRICK, COPPER_BLOCK, AIR, GRAVEL};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::{FloodFillCache, BuildingFootprintBitmap};
use crate::ground::Ground;
use crate::map_renderer;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedWay};
use crate::progress::{emit_gui_progress_update, emit_map_preview_ready, emit_open_mcworld_file};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::urban_ground;
use crate::world_editor::{WorldEditor, WorldFormat};
use crate::bresenham::bresenham_line;
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::io::{stdout, Write};

pub const MIN_Y: i32 = -64;

/// Generation options that can be passed separately from CLI Args
#[derive(Clone)]
pub struct GenerationOptions {
    pub path: PathBuf,
    pub format: WorldFormat,
    pub level_name: Option<String>,
    pub spawn_point: Option<(i32, i32)>,
}

// ============================================================================
// ?? INFRAESTRUTURA SUBTERRÂNEA (WFS) - GERAÇÃO ??
// ============================================================================

/// Injetor do Submundo (WFS CAESB / Neoenergia)
pub fn generate_underground_infrastructure(editor: &mut WorldEditor, element: &ProcessedWay, args: &Args) {
    let man_made = element.tags.get("man_made").map(|s| s.as_str());
    let power = element.tags.get("power").map(|s| s.as_str());
    
    if man_made != Some("pipeline") && power != Some("cable") && power != Some("line") {
        return;
    }

    let width_str = element.tags.get("width").map(|s| s.as_str()).unwrap_or("1");
    let radius = (width_str.parse::<i32>().unwrap_or(1) / 2).max(1);
    
    let layer_val = element.tags.get("layer").and_then(|s| s.parse::<i32>().ok()).unwrap_or(-1);
    let depth_offset = layer_val * 6;

    let substance = element.tags.get("substance").map(|s| s.as_str()).unwrap_or("");
    let is_sewage = substance == "sewage";
    let is_power = power == Some("cable") || power == Some("line");

    let (wall_block, fluid_block) = if is_sewage {
        (BRICK, Some(WATER))
    } else if is_power {
        (COPPER_BLOCK, None)
    } else {
        (SMOOTH_STONE, None)
    };

    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let current_node = (node.x, node.z);

        if let Some(prev) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev.0, 0, prev.1,
                current_node.0, 0, current_node.1
            );

            for (bx, _, bz) in bresenham_points {
                let local_ground = editor.get_ground_level(bx, bz);
                let pipe_center_y = local_ground + depth_offset;

                if pipe_center_y < MIN_Y + 5 { continue; }

                for wx in -radius..=radius {
                    for wy in -radius..=radius {
                        let dist_sq = wx * wx + wy * wy;
                        if dist_sq <= radius * radius {
                            let is_shell = dist_sq >= (radius - 1) * (radius - 1);
                            
                            let set_x = bx + wx;
                            let set_y = pipe_center_y + wy;
                            let set_z = bz;

                            if is_shell {
                                editor.set_block_absolute(wall_block, set_x, set_y, set_z, Some(&[DIRT, STONE, COARSE_DIRT, GRAVEL]), None);
                            } else {
                                let core_block = if fluid_block.is_some() && wy == -radius + 1 {
                                    fluid_block.unwrap()
                                } else {
                                    AIR
                                };
                                editor.set_block_absolute(core_block, set_x, set_y, set_z, None, None);
                            }
                        }
                    }
                }
            }
        }
        previous_node = Some(current_node);
    }
}

// ============================================================================
// ?? BESM-6 SCANLINE ENGINE (OUT-OF-CORE SPATIAL ROUTER) ??
// ============================================================================

/// Extrai o Centroide Espacial Geométrico de um Elemento OSM.
/// Usado para indexar na R-Tree (Spatial Buckets).
fn get_element_centroid(element: &ProcessedElement) -> (i32, i32) {
    match element {
        ProcessedElement::Node(n) => (n.x, n.z),
        ProcessedElement::Way(w) => {
            if w.nodes.is_empty() { return (0, 0); }
            let sum_x: i64 = w.nodes.iter().map(|n| n.x as i64).sum();
            let sum_z: i64 = w.nodes.iter().map(|n| n.z as i64).sum();
            ((sum_x / w.nodes.len() as i64) as i32, (sum_z / w.nodes.len() as i64) as i32)
        },
        ProcessedElement::Relation(r) => {
            if let Some(m) = r.members.first() {
                if m.way.nodes.is_empty() { return (0, 0); }
                let sum_x: i64 = m.way.nodes.iter().map(|n| n.x as i64).sum();
                let sum_z: i64 = m.way.nodes.iter().map(|n| n.z as i64).sum();
                ((sum_x / m.way.nodes.len() as i64) as i32, (sum_z / m.way.nodes.len() as i64) as i32)
            } else {
                (0, 0)
            }
        }
    }
}

/// A Rota Individual de Geração (O "Bisturi" que o Orquestrador chama para cada forma)
#[allow(clippy::too_many_arguments)]
fn dispatch_element(
    element: ProcessedElement,
    editor: &mut WorldEditor,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    suppressed_building_outlines: &HashSet<u64>,
    xzbbox: &XZBBox,
) {
    match &element {
        ProcessedElement::Way(way) => {
            if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                if !suppressed_building_outlines.contains(&way.id) {
                    buildings::generate_buildings(editor, way, args, None, None, flood_fill_cache);
                }
            } else if way.tags.contains_key("highway") {
                highways::generate_highways(editor, &element, args, highway_connectivity, flood_fill_cache);
            } else if way.tags.contains_key("landuse") {
                landuse::generate_landuse(editor, way, args, flood_fill_cache, building_footprints);
            } else if way.tags.contains_key("natural") {
                natural::generate_natural(editor, &element, args, flood_fill_cache, building_footprints);
            } else if way.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, &element, args, flood_fill_cache);
            } else if way.tags.contains_key("leisure") {
                leisure::generate_leisure(editor, way, args, flood_fill_cache, building_footprints);
            } else if way.tags.contains_key("barrier") {
                barriers::generate_barriers(editor, &element);
            } else if let Some(val) = way.tags.get("waterway") {
                if val == "dock" {
                    water_areas::generate_water_area_from_way(editor, way, xzbbox);
                } else {
                    waterways::generate_waterways(editor, way);
                }
            } else if way.tags.contains_key("railway") {
                railways::generate_railways(editor, way);
            } else if way.tags.contains_key("roller_coaster") {
                railways::generate_roller_coaster(editor, way);
            } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway") {
                highways::generate_aeroway(editor, way, args);
            } else if way.tags.get("service") == Some(&"siding".to_string()) {
                highways::generate_siding(editor, way);
            } else if way.tags.get("tomb") == Some(&"pyramid".to_string()) {
                historic::generate_pyramid(editor, way, args, flood_fill_cache);
            } else if way.tags.contains_key("man_made") && way.tags.get("man_made") != Some(&"pipeline".to_string()) {
                man_made::generate_man_made(editor, &element, args, flood_fill_cache);
            } else if way.tags.contains_key("place") {
                landuse::generate_place(editor, way, args, flood_fill_cache);
            }
            
            // Infra Subterrânea WFS (Saneamento/Energia)
            if way.tags.contains_key("man_made") || way.tags.contains_key("power") {
                generate_underground_infrastructure(editor, way, args);
            }
        }
        ProcessedElement::Node(node) => {
            if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                doors::generate_doors(editor, node);
            } else if node.tags.contains_key("natural") && node.tags.get("natural") == Some(&"tree".to_string()) {
                natural::generate_natural(editor, &element, args, flood_fill_cache, building_footprints);
            } else if node.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, &element, args, flood_fill_cache);
            } else if node.tags.contains_key("barrier") {
                barriers::generate_barrier_nodes(editor, node);
            } else if node.tags.contains_key("highway") {
                highways::generate_highways(editor, &element, args, highway_connectivity, flood_fill_cache);
            } else if node.tags.contains_key("tourism") {
                tourisms::generate_tourisms(editor, node);
            } else if node.tags.contains_key("man_made") {
                man_made::generate_man_made_nodes(editor, node, args);
            } else if node.tags.contains_key("power") {
                power::generate_power_nodes(editor, node);
            } else if node.tags.contains_key("historic") {
                historic::generate_historic(editor, node);
            } else if node.tags.contains_key("emergency") {
                emergency::generate_emergency(editor, node);
            } else if node.tags.contains_key("advertising") {
                advertising::generate_advertising(editor, node);
            }
        }
        ProcessedElement::Relation(rel) => {
            let is_building_relation = rel.tags.contains_key("building")
                || rel.tags.contains_key("building:part")
                || rel.tags.get("type").map(|t| t.as_str()) == Some("building");
            if is_building_relation {
                buildings::generate_building_from_relation(editor, rel, args, flood_fill_cache, xzbbox);
            } else if rel.tags.contains_key("water") || rel.tags.get("natural").map(|val| val == "water" || val == "bay").unwrap_or(false) {
                water_areas::generate_water_areas_from_relation(editor, rel, xzbbox);
            } else if rel.tags.contains_key("natural") {
                natural::generate_natural_from_relation(editor, rel, args, flood_fill_cache, building_footprints);
            } else if rel.tags.contains_key("landuse") {
                landuse::generate_landuse_from_relation(editor, rel, args, flood_fill_cache, building_footprints);
            } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                leisure::generate_leisure_from_relation(editor, rel, args, flood_fill_cache, building_footprints);
            }
        }
    }
}

/// HUD do Motor de Varredura (A Colmeia)
fn render_hive_hud(
    current_rx: i32, current_rz: i32,
    min_rx: i32, max_rx: i32, min_rz: i32, max_rz: i32,
    editor: &WorldEditor,
    processed_count: usize,
    total_count: usize
) {
    print!("{}[2J", 27 as char); // Clear Screen
    print!("{}[H", 27 as char);  // Home Cursor
    
    println!("{}", "======================================================================".cyan().bold());
    println!("{} - {}", "[BESM-6]".yellow().bold(), "OUT-OF-CORE SCANLINE ENGINE (BRASÍLIA DF)".bright_white().bold());
    println!("{}", "======================================================================".cyan().bold());
    
    let (halo_buckets, halo_blocks) = editor.get_halo_metrics();
    let mem_mb = halo_blocks * 8 / 1024 / 1024; // Estimativa rústica
    
    println!("{} {} / {}", "Progress:".green().bold(), processed_count, total_count);
    println!("{} {} active buckets (Est. {} MB Ram cost)", "Halo Cache Load:".magenta().bold(), halo_buckets, mem_mb);
    println!("{} r.{}.{}.mca", "Sweeping Region:".yellow().bold(), current_rx, current_rz);
    println!();

    // Renderização da Matriz ASCII (Limita a janela visual a 20x20 para não estourar o terminal)
    let view_radius = 10;
    let view_min_x = (current_rx - view_radius).max(min_rx);
    let view_max_x = (current_rx + view_radius).min(max_rx);
    let view_min_z = (current_rz - view_radius).max(min_rz);
    let view_max_z = (current_rz + view_radius).min(max_rz);

    for z in view_min_z..=view_max_z {
        for x in view_min_x..=view_max_x {
            // Lógica do Scanline: Processa da esquerda pra direita, cima pra baixo.
            let is_current = x == current_rx && z == current_rz;
            let is_processed = z < current_rz || (z == current_rz && x < current_rx);
            
            if is_current {
                print!("{} ", "[>>]".yellow().bold());
            } else if is_processed {
                print!("{} ", "[¦¦]".green());
            } else {
                print!("{} ", "[  ]".bright_black());
            }
        }
        println!();
    }

    println!("\n{}: [¦¦] Selado | [>>] Scanline | [  ] Pendente", "LEGENDA".white().bold());
    println!("{}", "======================================================================".cyan().bold());
    let _ = stdout().flush();
}


pub fn generate_world_with_options(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
    options: GenerationOptions,
) -> Result<PathBuf, String> {
    let output_path = options.path.clone();
    let world_format = options.format;

    let mut editor: WorldEditor = WorldEditor::new_with_format_and_name(
        options.path,
        &xzbbox,
        llbbox,
        options.format,
        options.level_name.clone(),
        options.spawn_point,
    );
    let ground = Arc::new(ground);
    editor.set_ground(Arc::clone(&ground));

    println!("{} Building Global Constraints...", "[4/7]".bold());
    
    // Preparação 2D Global (Usa pouquíssima RAM)
    let highway_connectivity = highways::build_highway_connectivity_map(&elements);
    let mut flood_fill_cache = FloodFillCache::new(); // Instância limpa e O(1)
    
    // O BuildingFootprintBitmap é seguro para RAM (1 bit por bloco)
    let building_footprints = flood_fill_cache.collect_building_footprints(&elements, &xzbbox);

    let building_centroids = if args.city_boundaries {
        flood_fill_cache.collect_building_centroids(&elements)
    } else {
        Vec::new()
    };

    let urban_lookup = if args.city_boundaries && !building_centroids.is_empty() {
        urban_ground::compute_urban_ground_lookup(building_centroids, &xzbbox)
    } else {
        urban_ground::UrbanGroundLookup::empty()
    };
    let has_urban_ground = !urban_lookup.is_empty();

    let suppressed_building_outlines: HashSet<u64> = {
        let mut outlines = HashSet::new();
        for element in &elements {
            if let ProcessedElement::Relation(rel) = element {
                let is_building_type = rel.tags.get("type").map(|t| t.as_str()) == Some("building");
                if is_building_type && rel.members.iter().any(|m| m.role == ProcessedMemberRole::Part) {
                    for member in &rel.members {
                        if member.role == ProcessedMemberRole::Outer {
                            outlines.insert(member.way.id);
                        }
                    }
                }
            }
        }
        outlines
    };

    // ?? BESM-6: Indexação Espacial (A R-Tree)
    println!("{} Spatially Indexing Vectors...", "[5/7]".bold());
    let mut spatial_index: HashMap<(i32, i32), Vec<ProcessedElement>> = HashMap::new();
    let total_elements = elements.len();

    for element in elements.into_iter() {
        let (cx, cz) = get_element_centroid(&element);
        let rx = cx >> 9;
        let rz = cz >> 9;
        spatial_index.entry((rx, rz)).or_default().push(element);
    }

    // Delimitação da Matriz Global
    let min_rx = xzbbox.min_x() >> 9;
    let max_rx = xzbbox.max_x() >> 9;
    let min_rz = xzbbox.min_z() >> 9;
    let max_rz = xzbbox.max_z() >> 9;
    
    let total_regions = ((max_rx - min_rx + 1) * (max_rz - min_rz + 1)) as usize;
    let mut processed_regions = 0;

    // ?? O MOTOR DE VARREDURA (SCANLINE) ??
    for rz in min_rz..=max_rz {
        for rx in min_rx..=max_rx {
            
            if args.debug {
                render_hive_hud(rx, rz, min_rx, max_rx, min_rz, max_rz, &editor, processed_regions, total_regions);
            } else {
                let p = (processed_regions as f64 / total_regions as f64) * 100.0;
                emit_gui_progress_update(p, &format!("Sweeping Region r.{}.{}", rx, rz));
            }

            editor.set_active_region(rx, rz);

            // 1. Gera a topografia Base APENAS para esta região 512x512
            let chunk_min_x = rx * 32;
            let chunk_max_x = (rx * 32) + 31;
            let chunk_min_z = rz * 32;
            let chunk_max_z = (rz * 32) + 31;

            let terrain_enabled = ground.elevation_enabled;

            for cx in chunk_min_x..=chunk_max_x {
                for cz in chunk_min_z..=chunk_max_z {
                    
                    let min_x = (cx << 4).max(xzbbox.min_x());
                    let max_x = ((cx << 4) + 15).min(xzbbox.max_x());
                    let min_z = (cz << 4).max(xzbbox.min_z());
                    let max_z = ((cz << 4) + 15).min(xzbbox.max_z());

                    for x in min_x..=max_x {
                        for z in min_z..=max_z {
                            let ground_y = if terrain_enabled {
                                editor.get_ground_level(x, z)
                            } else {
                                args.ground_level
                            };

                            let is_urban = has_urban_ground && urban_lookup.is_urban(x, z);

                            if !editor.check_for_block_absolute(x, ground_y, z, Some(&[STONE]), None) {
                                if is_urban {
                                    editor.set_block_if_absent_absolute(POLISHED_ANDESITE, x, ground_y, z);
                                } else {
                                    editor.set_block_if_absent_absolute(GRASS_BLOCK, x, ground_y, z);
                                }
                                editor.set_block_if_absent_absolute(COARSE_DIRT, x, ground_y - 1, z);
                                editor.set_block_if_absent_absolute(RED_TERRACOTTA, x, ground_y - 2, z);
                            }

                            if args.fillground {
                                editor.fill_column_absolute(
                                    STONE, x, z,
                                    MIN_Y + 1, ground_y - 3,
                                    true, 
                                );
                            }
                            editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));
                        }
                    }
                }
            }

            // 2. Injeta o Halo Cache (Construções vizinhas que vazaram pra cá)
            editor.load_halo_to_core();

            // 3. Processa os elementos cujo centroide está contido nesta região
            if let Some(region_elements) = spatial_index.remove(&(rx, rz)) {
                for element in region_elements {
                    dispatch_element(
                        element,
                        &mut editor,
                        args,
                        &highway_connectivity,
                        &flood_fill_cache,
                        &building_footprints,
                        &suppressed_building_outlines,
                        &xzbbox
                    );
                }
            }

            // 4. FLUSH DIRETO PARA O DISCO O(1) E ANIQUILA A RAM DO CORE
            editor.flush_active_region();
            
            // 5. Expurgo do FloodFillCache (Para limpar polígonos calculados)
            flood_fill_cache.clear_cache();

            processed_regions += 1;
        }
    }

    // Salva Metadados Finais
    editor.save();

    emit_gui_progress_update(99.0, "Finalizing world...");

    #[cfg(feature = "gui")]
    if world_format == WorldFormat::JavaAnvil {
        use crate::gui::update_player_spawn_y_after_generation;
        let bbox_string = format!(
            "{},{},{},{}",
            args.bbox.min().lat(),
            args.bbox.min().lng(),
            args.bbox.max().lat(),
            args.bbox.max().lng()
        );

        if let Some(ref world_path) = args.path {
            if let Err(e) = update_player_spawn_y_after_generation(
                world_path,
                bbox_string,
                args.scale_h, 
                ground.as_ref(),
            ) {
                let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
                eprintln!("Warning: {}", warning_msg);
                #[cfg(feature = "gui")]
                send_log(LogLevel::Warning, &warning_msg);
            }
        }
    }

    if world_format == WorldFormat::BedrockMcWorld {
        if let Some(path_str) = output_path.to_str() {
            emit_open_mcworld_file(path_str);
        }
    }

    Ok(output_path)
}

#[derive(Clone)]
pub struct MapPreviewInfo {
    pub world_path: PathBuf,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
    pub world_area: i64,
}

impl MapPreviewInfo {
    pub fn new(world_path: PathBuf, xzbbox: &XZBBox) -> Self {
        let world_width = (xzbbox.max_x() - xzbbox.min_x()) as i64;
        let world_height = (xzbbox.max_z() - xzbbox.min_z()) as i64;
        Self {
            world_path,
            min_x: xzbbox.min_x(),
            max_x: xzbbox.max_x(),
            min_z: xzbbox.min_z(),
            max_z: xzbbox.max_z(),
            world_area: world_width * world_height,
        }
    }
}

pub const MAX_MAP_PREVIEW_AREA: i64 = 6400 * 6900;

pub fn start_map_preview_generation(info: MapPreviewInfo) {
    if info.world_area > MAX_MAP_PREVIEW_AREA {
        return;
    }

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            map_renderer::render_world_map(
                &info.world_path,
                info.min_x,
                info.max_x,
                info.min_z,
                info.max_z,
            )
        }));

        match result {
            Ok(Ok(_path)) => {
                emit_map_preview_ready();
            }
            Ok(Err(e)) => {
                eprintln!("Warning: Failed to generate map preview: {}", e);
            }
            Err(_) => {
                eprintln!("Warning: Map preview generation panicked unexpectedly");
            }
        }
    });
}