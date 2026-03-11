#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
// ?? INJEÇÃO DO NOVO MOTOR GOVERNAMENTAL E DASHBOARD
mod providers; 
mod master_control; 
#[cfg(feature = "gui")]
mod progress;
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
use std::path::PathBuf;
use std::sync::mpsc;
use std::{env, fs, io::Write};
use coordinate_system::geographic::LLBBox;

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn emit_map_preview_ready() {}
    pub fn emit_open_mcworld_file(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

/// Função principal que despacha a geração (agora aceita um canal de telemetria opcional)
pub fn run_generation_pipeline(mut args: Args, telemetry_tx: Option<mpsc::Sender<master_control::BesmSignal>>) {
    // Configure thread pool with 90% CPU cap to keep system responsive
    floodfill_cache::configure_rayon_thread_pool(0.9);

    // Clean up old cached elevation tiles on startup
    elevation_data::cleanup_old_cached_tiles();

    // Determine world format and output path
    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    // Build the generation output path and level name
    let (generation_path, level_name) = if args.bedrock {
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) = world_utils::build_bedrock_output(&args.bbox, output_dir);
        (output_path, Some(lvl_name))
    } else {
        let base_dir = args.path.clone().unwrap_or_else(|| PathBuf::from("./world"));
        let world_path = match world_utils::create_new_world(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                let msg = format!("Error: {}", e);
                if let Some(ref tx) = telemetry_tx { let _ = tx.send(master_control::BesmSignal::Log(msg.clone())); }
                eprintln!("{}", msg.red().bold());
                return;
            }
        };
        
        let msg = format!("Created new world at: {}", world_path.display().to_string());
        if let Some(ref tx) = telemetry_tx { let _ = tx.send(master_control::BesmSignal::Log(msg.clone())); }
        println!("{}", msg.bright_white().bold());
        
        (world_path, None)
    };

    // ==============================================================================
    // ?? GOLPE DE ESTADO (Pipeline BESM-6 Integralizado) ??
    // Hierarquia de Dados (0 = Máxima Prioridade, 10 = Mínima Prioridade)
    // ==============================================================================
    
    let mut provider_manager = providers::ProviderManager::new();

    // [PRIORIDADE 10] Base universal da Internet
    provider_manager.register_provider(Box::new(providers::osm_provider::OSMProvider::new(args.scale_h)));

    // [PRIORIDADE 10] Base universal Local (Ultra-rápido via SSD)
    if let Some(ref pbf_path) = args.local_pbf {
        provider_manager.register_provider(Box::new(providers::pbf_provider::PbfProvider::new(
            pbf_path.clone(), 
            args.scale_h, 
            10, 
        )));
    }

    // [PRIORIDADE 3] Matriz Orgânica da Natureza (1 pixel = 1 metro)
    if let Some(ref tiff_path) = args.local_geotiff {
        provider_manager.register_provider(Box::new(providers::raster_provider::RasterProvider::new(
            tiff_path.clone(), 
            args.scale_h, 
            3,
            args.geotiff_lat,
            args.geotiff_lon,
            args.geotiff_pixel_size
        )));
    }

    // [PRIORIDADE 2] Saneamento, Energia e Submundo (WFS Dinâmico)
    if args.enable_underground_wfs {
        if let Some(ref wfs_url) = args.wfs_endpoint {
            provider_manager.register_provider(Box::new(providers::wfs_provider::WFSProvider::new(
                wfs_url.clone(),
                args.scale_h,
                2, 
            )));
        }
    }

    // [PRIORIDADE 2] Mobiliário Urbano (Postes, Lixeiras e Árvores Exatas)
    if let Some(ref csv_path) = args.local_csv {
        provider_manager.register_provider(Box::new(providers::csv_provider::CsvProvider::new(
            csv_path.clone(), 
            args.scale_h, 
            2, 
            None
        )));
    }

    // [PRIORIDADE 1] Bases Vetoriais Oficiais do GDF / IBGE
    if let Some(ref shp_path) = args.local_shp {
        provider_manager.register_provider(Box::new(providers::gdf_provider::GDFProvider::new(
            shp_path.clone(), args.scale_h, 1, None
        )));
    }
    if let Some(ref geojson_path) = args.local_geojson {
        provider_manager.register_provider(Box::new(providers::geojson_provider::GeoJsonProvider::new(
            geojson_path.clone(), args.scale_h, 1, None
        )));
    }
    if let Some(ref gpkg_path) = args.local_gpkg {
        provider_manager.register_provider(Box::new(providers::gpkg_provider::GpkgProvider::new(
            gpkg_path.clone(), args.scale_h, 1, None
        )));
    }
    if let Some(ref citygml_path) = args.local_citygml {
        provider_manager.register_provider(Box::new(providers::citygml_provider::CityGmlProvider::new(
            citygml_path.clone(), args.scale_h, 1, args.stream_citygml
        )));
    }

    // [PRIORIDADE 0] A Supremacia Estrutural (Plantas Baixas CAESB)
    if let Some(ref utility_path) = args.local_utility {
        provider_manager.register_provider(Box::new(providers::indoor_utility_provider::IndoorUtilityProvider::new(
            utility_path.clone(), 
            args.scale_h, 
            1.15, // Escala Vertical Exata 
            0,
        )));
    }

    let mut optimized_features = match provider_manager.fetch_all(&args.bbox) {
        Ok(features) => features,
        Err(e) => {
            let msg = format!("Error Crítico no Provider Manager: {}", e);
            if let Some(ref tx) = telemetry_tx { let _ = tx.send(master_control::BesmSignal::Log(msg.clone())); }
            eprintln!("{}", msg.red().bold());
            return;
        }
    };

    // ?? BESM-6 TWEAK: O TRANCAMENTO DE PRIORIDADE
    optimized_features.sort_by_key(|f| f.priority);

    let parsed_elements: Vec<osm_parser::ProcessedElement> = optimized_features
        .into_iter()
        .map(|feature| feature.into_processed_element())
        .collect();

    // ?? BESM-6 Tweak: O Cálculo das Fronteiras Globais Absolutas
    let xzbbox = coordinate_system::transformation::CoordTransformer::llbbox_to_xzbbox(&args.bbox, args.scale_h)
        .unwrap()
        .1;

    let ground = ground::generate_ground_data(&args);

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
                element.tags(),
            )
            .expect("Failed to write to output file");
        }
        let msg = "Arquivo de depuração gerado: parsed_osm_data.txt. Verifique a retenção das tags governamentais.".to_string();
        if let Some(ref tx) = telemetry_tx { let _ = tx.send(master_control::BesmSignal::Log(msg.clone())); }
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
                    eprintln!("{} Failed to convert spawn point: {}", "Error:".red().bold(), e);
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
        telemetry_channel: telemetry_tx, // ?? Injeção do Canal para o Motor Out-of-Core
    };

    // Generate world (Ancorado no Scanline Engine do data_processing.rs)
    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        args.bbox,
        ground,
        &args,
        generation_options,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!("{} Bedrock world saved to: {}", "Done!".green().bold(), generation_path.display());
            }

            if !args.bedrock {
                if let Some((spawn_x, spawn_z)) = spawn_point {
                    if let Err(e) = world_utils::set_spawn_in_level_dat(&generation_path, spawn_x, spawn_z) {
                        eprintln!("{} Failed to set spawn point in level.dat: {}", "Warning:".yellow().bold(), e);
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
    
    // ?? BESM-6 TWEAK: A Nova Alma da Máquina
    println!(
        r#"
         ¦¯¯¯¯¯¦ ¯¦¯ ¦_   ¦ ¦¯¯¯¯¯¦ ¦¯¯¯¯¯¦ ¦       ¯¦¯ ¦¯¯¯¯¯¦ ¦_   _¦
         ¦     ¦  ¦  ¦ ¯_ ¦ ¦       ¦       ¦        ¦  ¦       ¦ ¯_¯ ¦
         ¦_____¦  ¦  ¦   ¦¦ ¦       ¦¯¯¯¯¯¦ ¦        ¦  ¯¯¯¯¯¯_ ¦     ¦
         ¦        ¦  ¦    ¦ ¦______ ¦______ ¦______  ¦  ______¦ ¦     ¦

                               VERSION {}
                    {}
        "#,
        version,
        repository.bright_yellow().bold()
    );

    if let Err(e) = version_check::check_for_updates() {
        eprintln!("{}: {}", "Error checking for version updates".red().bold(), e);
    }

    let args: Args = Args::parse();
    if let Err(e) = args::validate_args(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    if args.bedrock && !cfg!(feature = "bedrock") {
        eprintln!("{}: The --bedrock flag requires the 'bedrock' feature.", "Error".red().bold());
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

    // ?? BESM-6 TWEAK: Roteador de Modos de Execução
    let args_count = std::env::args().len();
    
    #[cfg(feature = "gui")]
    {
        if args_count == 1 {
            gui::run_gui();
            return; 
        }
    }

    #[cfg(not(feature = "gui"))]
    {
        // Se a GUI não está compilada e o usuário rodou liso, sobe o Master Control HUD
        if args_count == 1 {
            let mut dashboard = master_control::MasterControl::new();
            dashboard.run_interactive_shell();
            return;
        }
    }

    // Se o usuário passou argumentos (ex: pincelism --bbox ...), roda no modo Pipeline silencioso
    run_cli();
}