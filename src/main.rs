#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 🚨 BESM-6: Declaração de Módulos Base
mod args;
#[cfg(feature = "bedrock")]
mod bedrock_block_map;
mod block_definitions;
mod bresenham;
mod clipping;
mod colors;
mod coordinate_system;
mod data_processing;
mod deterministic_rng;
mod element_processing;
mod elevation_data;
mod floodfill;
mod floodfill_cache;
mod ground;
mod map_renderer;
mod master_control;
mod osm_parser;
#[cfg(feature = "gui")]
mod progress;
mod providers;
mod retrieve_data;
#[cfg(feature = "gui")]
mod telemetry;
#[cfg(test)]
mod test_utilities;
mod urban_ground;
mod version_check;
mod world_editor;
mod world_utils;

use args::Args;
use clap::Parser;
use colored::*;
// Só usados no dashboard TUI sem GUI (`#[cfg(not(feature = "gui"))]` abaixo) — sob
// `--all-features`/default (`gui` ligada) esse bloco não compila, e o import ficaria
// "não usado" sem o mesmo cfg aqui.
#[cfg(not(feature = "gui"))]
use coordinate_system::geographic::LLBBox;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
#[cfg(not(feature = "gui"))]
use std::sync::Arc;

#[cfg(feature = "gui")]
mod gui;

#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    // Stub sem chamador no build sem GUI (nada em `!gui` invoca preview de mapa).
    #[allow(dead_code)]
    pub fn emit_map_preview_ready() {}
    pub fn emit_open_mcworld_file(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

pub fn run_generation_pipeline(
    args: Args,
    telemetry_tx: Option<mpsc::Sender<master_control::BesmSignal>>,
) {
    floodfill_cache::configure_rayon_thread_pool(0.9);
    elevation_data::cleanup_old_cached_tiles();

    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    let (generation_path, level_name) = if args.bedrock {
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) = world_utils::build_bedrock_output(&args.bbox, output_dir);
        (output_path, Some(lvl_name))
    } else {
        let base_dir = args
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("./world"));
        let world_path = match world_utils::create_new_world(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                let msg = format!("Error: {}", e);
                if let Some(ref tx) = telemetry_tx {
                    let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                }
                eprintln!("{}", msg.red().bold());
                return;
            }
        };

        let msg = format!("Created new world at: {}", world_path.display());
        if let Some(ref tx) = telemetry_tx {
            let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
        }
        println!("{}", msg.bright_white().bold());

        (world_path, None)
    };

    let mut provider_manager = providers::ProviderManager::new();

    // Prioridade 10 (Base)
    provider_manager.register_provider(Box::new(providers::osm_provider::OSMProvider::new(
        args.scale_h,
    )));

    if let Some(ref pbf_path) = args.local_pbf {
        provider_manager.register_provider(Box::new(providers::pbf_provider::PbfProvider::new(
            pbf_path.clone(),
            args.scale_h,
            10,
        )));
    }

    if args.enable_underground_wfs {
        if let Some(ref wfs_url) = args.wfs_endpoint {
            provider_manager.register_provider(Box::new(
                providers::wfs_provider::WFSProvider::new(wfs_url.clone(), args.scale_h, 2),
            ));
        }
    }

    if let Some(ref shp_path) = args.local_shp {
        provider_manager.register_provider(Box::new(providers::gdf_provider::GDFProvider::new(
            shp_path.clone(),
            args.scale_h,
            1,
            None,
        )));
    }
    if let Some(ref geojson_path) = args.local_geojson {
        provider_manager.register_provider(Box::new(
            providers::geojson_provider::GeoJsonProvider::new(
                geojson_path.clone(),
                args.scale_h,
                1,
                None,
            ),
        ));
    }
    if let Some(ref gpkg_path) = args.local_gpkg {
        provider_manager.register_provider(Box::new(providers::gpkg_provider::GpkgProvider::new(
            gpkg_path.clone(),
            args.scale_h,
            1,
            None,
        )));
    }
    if let Some(ref citygml_path) = args.local_citygml {
        provider_manager.register_provider(Box::new(
            providers::citygml_provider::CityGmlProvider::new(
                citygml_path.clone(),
                args.scale_h,
                1,
            ),
        ));
    }

    // 🚨 BESM-6: Registo do Provedor IFC BIM (LOD4/LOD5)
    if let Some(ref ifc_path) = args.local_ifc {
        // Assume Prioridade 1 (Governativa/Absoluta) e requer âncora geodésica.
        // Para testes, estamos hardcoding a Praça dos Três Poderes como âncora (Lat, Lon, Rotação).
        provider_manager.register_provider(Box::new(providers::ifc_provider::IfcProvider::new(
            ifc_path.clone(),
            args.scale_h,
            1.15, // scale_v rigorosa
            1,    // prioridade máxima
            -15.8000,
            -47.8600,
            0.0,
        )));
    }

    let mut optimized_features = match provider_manager.fetch_all(&args.bbox) {
        Ok(features) => features,
        Err(e) => {
            let msg = format!("Error Crítico no Provider Manager: {}", e);
            if let Some(ref tx) = telemetry_tx {
                let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
            }
            eprintln!("{}", msg.red().bold());
            return;
        }
    };

    optimized_features.sort_by_key(|f| f.priority);

    // 🚨 BESM-6: Dual Processing Pipeline - Separate Provider-Specific Features from OSM Data
    // Provider features (CAESB, CityGML, IFC) need direct processing to preserve metadata
    let mut provider_specific_features: Vec<providers::Feature> = Vec::new();
    let mut osm_convertible_features: Vec<providers::Feature> = Vec::new();

    for feature in optimized_features {
        // Features from government providers with specific semantic groups go direct
        //
        // 🚨 RECONEXÃO (Advertising): `SemanticGroup::Advertising` sempre caiu aqui como
        // `false` — a condição de `source` só reconhecia "CAESB"/"CityGML"/"IFC"/"Indoor",
        // e o `source` de um outdoor/totem do OSM é sempre "osm". Isso jogava toda
        // feature de propaganda para `osm_convertible_features`, que vira
        // `ProcessedElement` via `into_processed_element()` — mas `advertising::generate_advertising`
        // (element_processing/advertising.rs) foi escrito para consumir a `Feature`
        // original (geometria + atributos brutos, para o PCA de orientação do painel),
        // não o `ProcessedNode`/`ProcessedWay` traduzido. Diferente de Sanitation/Power/
        // Telecom/Indoor, Advertising é agnóstico de provedor por design (aceita OSM,
        // CSV, GeoJSON, PostGIS, 3D Tiles — ver o doc-comment do módulo), então aqui ele
        // não depende do `source` conter um nome de provedor governamental específico.
        let is_provider_specific = (matches!(
            feature.semantic_group,
            providers::SemanticGroup::Sanitation
                | providers::SemanticGroup::Sewage
                | providers::SemanticGroup::Utility
                | providers::SemanticGroup::Power
                | providers::SemanticGroup::Telecom
                | providers::SemanticGroup::Indoor
        ) && (feature.source.contains("CAESB")
            || feature.source.contains("CityGML")
            || feature.source.contains("IFC")
            || feature.source.contains("Indoor")))
            || feature.semantic_group == providers::SemanticGroup::Advertising;

        if is_provider_specific {
            provider_specific_features.push(feature);
        } else {
            osm_convertible_features.push(feature);
        }
    }

    println!(
        "[INFO] 🔀 Pipeline bifurcado: {} features governamentais diretas, {} features OSM convertidas.",
        provider_specific_features.len(),
        osm_convertible_features.len()
    );

    // Convert only non-provider-specific features to ProcessedElement (preserves existing flow)
    let parsed_elements: Vec<osm_parser::ProcessedElement> = osm_convertible_features
        .into_iter()
        .map(|feature| feature.into_processed_element())
        .collect();

    let xzbbox = coordinate_system::transformation::CoordTransformer::llbbox_to_xzbbox(
        &args.bbox,
        args.scale_h,
    )
    .unwrap()
    .1;

    // ================================================================
    // 🚨 RECONEXÃO ESTRUTURAL (BESM-6): Elevação real e Bioma real
    // ================================================================
    // Até aqui, `data_processing.rs` sempre construía `Ground::new_enabled`
    // com `bare_earth_cache`/`canopy_surface_cache`/`biome_cache` vazios —
    // o próprio código dizia "o orquestrador no futuro injetará os
    // rasterizadores (DEM/DSM/Vegetation) aqui". `get_ground_level` (usado
    // por árvores, estradas, pontes, prédios) e `get_biome` sempre caíam no
    // fallback plano/matemático, mesmo com `--terrain` ativo. As duas buscas
    // abaixo alimentam essas caches de verdade; ver `GenerationOptions` e o
    // Scanline em `data_processing.rs` para onde os dados são fatiados por
    // região.

    // Elevação real (SRTM via AWS Terrarium, ou LiDAR local se fornecido).
    // Busca UMA vez para o bbox inteiro (função já existente em
    // elevation_data.rs, só nunca chamada); desativada em --offline porque
    // a busca SRTM faz requisições HTTP.
    let elevation_data: Option<elevation_data::ElevationData> = if args.terrain && !args.offline {
        println!(
            "{} Fetching real elevation data (SRTM/LiDAR)...",
            "[3.5/7]".bold()
        );
        match elevation_data::fetch_elevation_data(
            &args.bbox,
            args.scale_h,
            args.scale_v,
            args.ground_level,
            args.local_lidar.as_ref(),
        ) {
            Ok(data) => Some(data),
            Err(e) => {
                let msg = format!(
                    "Falha ao buscar elevação real: {}. Terreno ficará plano nesta execução.",
                    e
                );
                if let Some(ref tx) = telemetry_tx {
                    let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                }
                eprintln!("{} {}", "Aviso:".yellow().bold(), msg);
                None
            }
        }
    } else {
        None
    };

    // Bioma real (MapBiomas + fitofisionomia IBGE + APP SICAR). Só produz dados
    // reais se os caminhos forem passados via CLI (--mapbiomas-tiff etc.);
    // sem eles, `fetch_quantized_biomes` retorna um mapa vazio de forma segura
    // e o motor mantém o fallback matemático de sempre em `natural.rs`.
    let biome_grid: Option<rustc_hash::FxHashMap<(i32, i32), u16>> = if args.terrain {
        let vegetation_provider = providers::vegetation_provider::VegetationProvider::new(
            args.mapbiomas_tiff.clone(),
            args.ibge_shapefile.clone(),
            args.sicar_shapefile.clone(),
            args.scale_h,
            args.mapbiomas_top_left_lat.unwrap_or(0.0),
            args.mapbiomas_top_left_lon.unwrap_or(0.0),
            args.mapbiomas_pixel_size_deg.unwrap_or(0.00027),
            args.mapbiomas_pixel_size_deg.unwrap_or(0.00027),
        );
        match vegetation_provider.fetch_quantized_biomes(&args.bbox) {
            Ok(grid) => Some(grid),
            Err(e) => {
                let msg = format!("Falha ao classificar biomas reais: {}.", e);
                if let Some(ref tx) = telemetry_tx {
                    let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                }
                eprintln!("{} {}", "Aviso:".yellow().bold(), msg);
                None
            }
        }
    } else {
        None
    };

    if args.debug {
        let mut buf = std::io::BufWriter::new(
            fs::File::create("parsed_osm_data.txt").expect("Failed to create output file"),
        );
        for element in &parsed_elements {
            writeln!(
                buf,
                "Element ID: {}, Type: {}, Tags: {:?}",
                element.id(),
                element.kind(),
                element.tags(), // Aqui é correto usar função em element, a feature foi resolvida no mod.rs
            )
            .expect("Failed to write to output file");
        }
        let msg = "Arquivo de depuração gerado: parsed_osm_data.txt.".to_string();
        if let Some(ref tx) = telemetry_tx {
            let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
        }
        println!("[INFO] {}", msg);
    }

    let spawn_point: Option<(i32, i32)> = match (args.spawn_lat, args.spawn_lng) {
        (Some(lat), Some(lng)) => {
            use coordinate_system::geographic::LLPoint;
            use coordinate_system::transformation::CoordTransformer;

            let llpoint = LLPoint::new(lat, lng).unwrap_or_else(|e| {
                eprintln!("{} Invalid spawn coordinates: {}", "Error:".red().bold(), e);
                std::process::exit(1);
            });

            let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(&args.bbox, args.scale_h)
                .unwrap_or_else(|e| {
                    eprintln!(
                        "{} Failed to convert spawn point: {}",
                        "Error:".red().bold(),
                        e
                    );
                    std::process::exit(1);
                });

            let xzpoint = transformer.transform_point(llpoint);
            Some((xzpoint.x, xzpoint.z))
        }
        _ => None,
    };

    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
        provider_features: provider_specific_features, // 🚨 BESM-6: Injeta features governamentais
        elevation_data: elevation_data.map(std::sync::Arc::new), // 🚨 Reconexão: elevação real
        biome_grid: biome_grid.map(std::sync::Arc::new), // 🚨 Reconexão: bioma real
        ambient_forest: args.terrain && !args.no_ambient_forest, // 🚨 Reconexão: floresta ambiente
        telemetry_tx,
    };

    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        args.bbox,
        &args,
        generation_options,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!(
                    "{} Bedrock world saved to: {}",
                    "Done!".green().bold(),
                    generation_path.display()
                );
            }

            if !args.bedrock {
                if let Some((spawn_x, spawn_z)) = spawn_point {
                    if let Err(e) =
                        world_utils::set_spawn_in_level_dat(&generation_path, spawn_x, spawn_z)
                    {
                        eprintln!(
                            "{} Failed to set spawn point in level.dat: {}",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
        }
    }
}

fn run_cli() {
    let version: &str = env!("CARGO_PKG_VERSION");
    let repository: &str = env!("CARGO_PKG_REPOSITORY");

    println!(
        r#"
         ███████  █_   █ ███████ ███████ █       ███ ███████ █_   _█
         █     █  █  █ █_ █ █       █       █        █  █       █ █_█ █
         █_____█  █  █   ██ █       ███████ █        █  ██████_ █     █
         █        █  █    █ █______ █______ █______  █  ______█ █     █

                               VERSION {}
                    {}
        "#,
        version,
        repository.bright_yellow().bold()
    );

    if let Err(e) = version_check::check_for_updates() {
        eprintln!(
            "{}: {}",
            "Error checking for version updates".red().bold(),
            e
        );
    }

    let mut args: Args = Args::parse();
    if let Err(e) = args::validate_args(&mut args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    if args.bedrock && !cfg!(feature = "bedrock") {
        eprintln!(
            "{}: The --bedrock flag requires the 'bedrock' feature.",
            "Error".red().bold()
        );
        std::process::exit(1);
    }

    run_generation_pipeline(args, None);
}

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    let args_count = std::env::args().len();

    #[cfg(feature = "gui")]
    {
        if args_count == 1 {
            crate::gui::run_gui();
            return;
        }
    }

    #[cfg(not(feature = "gui"))]
    {
        if args_count == 1 {
            // 🚨 RECONEXÃO: `MasterControl::new` passou a exigir `Arc<ProviderManager>`
            // e `Arc<Args>` (para que `dispatch_generation`/`generate_region_from_global`
            // tenham escala real e providers registrados), mas este chamador nunca foi
            // atualizado — só compila builds com a feature `gui` (default), então o
            // caminho `--no-default-features` (dashboard sem GUI) nunca compilou de
            // fato. Não há CLI args neste ponto (o dashboard é lançado sem nenhum
            // argumento), então montamos um `Args` mínimo com os mesmos defaults do
            // clap e um bbox provisório (Praça dos Três Poderes) só para satisfazer o
            // tipo — cada `MacroRegion` escolhido dentro do HUD define seu próprio
            // bbox real em `dispatch_generation`, o `bbox` daqui nunca é usado para
            // delimitar geração.
            let default_args = Args {
                bbox: LLBBox::new(-15.81, -47.87, -15.79, -47.85)
                    .expect("bbox provisório do dashboard é inválido"),
                file: None,
                save_json_file: None,
                path: None,
                bedrock: false,
                downloader: args::Downloader::Requests,
                threads: 0,
                offline: false,
                scale_h: 1.33,
                scale_v: 1.15,
                scale: None,
                local_shp: None,
                local_geojson: None,
                local_lidar: None,
                wfs_endpoint: None,
                epsg: 31983,
                dem: args::DemSource::AwsSrtm,
                local_dem: None,
                cache_dir: PathBuf::from("./arnis_cache"),
                max_area_km2: 10000.0,
                max_dem_tiles: 200,
                max_osm_features: 5_000_000,
                priority_layer: vec![
                    args::LayerPriority::Shp,
                    args::LayerPriority::Lidar,
                    args::LayerPriority::Wfs,
                    args::LayerPriority::Geojson,
                    args::LayerPriority::Osm,
                ],
                enable_underground_wfs: false,
                postgis_url: None,
                local_gpkg: None,
                local_pbf: None,
                mvt_endpoint: None,
                local_citygml: None,
                local_ifc: None,
                local_mesh: None,
                ibge_shapefile: None,
                sicar_shapefile: None,
                mapbiomas_tiff: None,
                mapbiomas_top_left_lat: None,
                mapbiomas_top_left_lon: None,
                mapbiomas_pixel_size_deg: None,
                no_ambient_forest: false,
                ground_level: -62,
                terrain: false,
                interior: true,
                roof: true,
                fillground: false,
                city_boundaries: true,
                debug: false,
                timeout: None,
                spawn_lat: None,
                spawn_lng: None,
            };

            let mut provider_manager = providers::ProviderManager::new();
            provider_manager.register_provider(Box::new(
                providers::osm_provider::OSMProvider::new(default_args.scale_h),
            ));

            let mut dashboard = master_control::MasterControl::new(
                Arc::new(provider_manager),
                Arc::new(default_args),
            );
            dashboard.run_interactive_shell();
            return;
        }
    }

    run_cli();
}
