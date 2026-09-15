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
mod world_viewer;

use args::Args;
use clap::Parser;
use colored::*;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;

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

    // 🚨 BESM-6 RECONEXÃO: Trava o diretório do mundo (Java Anvil apenas — Bedrock
    // usa LevelDB, que já tem seu próprio lock nativo) para impedir que duas
    // gerações concorrentes escrevam no mesmo lugar e corrompam as regiões .mca.
    // `_session_lock` fica vivo até o fim da função (RAII) e o Drop de
    // `SessionLock` libera/apaga o session.lock automaticamente ao sair.
    #[cfg(feature = "gui")]
    let _session_lock = if world_format == world_editor::WorldFormat::JavaAnvil {
        match gui::SessionLock::acquire(&generation_path) {
            Ok(lock) => Some(lock),
            Err(e) => {
                let msg = format!("Aviso: não foi possível travar o diretório do mundo: {e}");
                if let Some(ref tx) = telemetry_tx {
                    let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                }
                eprintln!("{}", msg.yellow().bold());
                None
            }
        }
    } else {
        None
    };

    let mut provider_manager = providers::ProviderManager::new();
    register_providers(&mut provider_manager, &args);

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
        // 🚨 RECONEXÃO: WFS ao vivo (CAESB/CEB/Novacap, `wfs_provider.rs`) classifica
        // corretamente suas Features como Sewage/Utility/Power (ver `wfs_provider.rs`
        // linha ~231), mas marca `source = "GDF_WFS_Live"` — uma string que não batia
        // com nenhum dos filtros abaixo. Resultado: todo dado WFS ao vivo (o único
        // caminho de água/esgoto/energia em TEMPO REAL da CAESB/CEB, diferente dos
        // GeoJSON locais estáticos do `IndoorUtilityProvider`) caía no pipeline
        // genérico OSM, nunca chegava em `man_made::generate_from_provider_feature` —
        // a engine especializada que desenha seções transversais ocas, poços de
        // visita com escada e `IRON_TRAPDOOR`, musgo e teias de aranha. Toda a
        // infraestrutura subterrânea real e ao vivo do DF era desenhada pelo caminho
        // genérico, perdendo esse detalhamento.
        let is_infrastructure_feature = matches!(
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
            || feature.source.contains("Indoor")
            || feature.source.contains("WFS"));

        // 🚨 BESM-6 RECONEXÃO: `TerrainDetail` (voxels de fotogrametria do
        // `MeshProvider`) entra aqui pela mesma razão que Sanitation/Power/etc:
        // preservar as tags `color`/`elevation`/`material` que só a Feature
        // original carrega. Sem isso, ela cairia em `osm_convertible_features`
        // → `into_processed_element()` → `ProcessedElement::Node` — e o branch
        // `Node` de `dispatch_element` (data_processing.rs) não reconhece
        // nenhuma dessas tags, descartando a Feature silenciosamente mesmo após
        // o voxel ser processado com sucesso pelo provedor.
        let is_photogrammetry_voxel = feature.semantic_group
            == providers::SemanticGroup::TerrainDetail
            && feature.source.contains("Photogrammetry_Mesh");

        // 🚨 RECONEXÃO (Advertising): `SemanticGroup::Advertising` sempre caiu no `else`
        // acima como `false` — a condição de `source` só reconhecia "CAESB"/"CityGML"/
        // "IFC"/"Indoor"/"WFS", e o `source` de um outdoor/totem do OSM é sempre "osm".
        // Isso jogava toda feature de propaganda para `osm_convertible_features`, que
        // vira `ProcessedElement` via `into_processed_element()` — mas
        // `advertising::generate_advertising` (element_processing/advertising.rs) foi
        // escrito para consumir a `Feature` original (geometria + atributos brutos,
        // para o PCA de orientação do painel), não o `ProcessedNode`/`ProcessedWay`
        // traduzido. Diferente de Sanitation/Power/Telecom/Indoor, Advertising é
        // agnóstico de provedor por design (aceita OSM, CSV, GeoJSON, PostGIS, 3D
        // Tiles — ver o doc-comment do módulo), então não depende do `source` conter
        // um nome de provedor governamental específico.
        let is_advertising_feature =
            feature.semantic_group == providers::SemanticGroup::Advertising;

        let is_provider_specific =
            is_infrastructure_feature || is_photogrammetry_voxel || is_advertising_feature;

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
    //
    // CORREÇÃO: `--local-lidar` não precisa de rede (só o fallback SRTM
    // precisa — ver `elevation_data::fetch_elevation_data`, que pula o
    // download de tiles inteiramente quando o LiDAR local é lido com
    // sucesso). Bloquear isso atrás de `!args.offline` descartava SEMPRE o
    // LiDAR local em modo offline, mesmo com o arquivo fornecido.
    let elevation_data: Option<elevation_data::ElevationData> =
        if args.terrain && (!args.offline || args.local_lidar.is_some()) {
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

    // 🚨 RECONEXÃO DSM: Superfície real (telhados/copas). Só produz dados reais
    // se --local-dsm (+ --dsm-top-left-lat/-lon/--dsm-pixel-size-deg) forem
    // passados via CLI; sem eles, `canopy_surface_cache` fica vazio e
    // `Ground::surface_level` degrada graciosamente para o chão nu (`level()`),
    // exatamente como antes desta reconexão.
    let surface_data: Option<rustc_hash::FxHashMap<(i32, i32), i32>> = if args.terrain {
        if let Some(ref dsm_path) = args.local_dsm {
            let dsm_provider = providers::dsm_provider::DsmProvider::new(
                dsm_path.clone(),
                args.scale_h,
                args.scale_v,
                args.ground_level,
                args.dsm_top_left_lat.unwrap_or(0.0),
                args.dsm_top_left_lon.unwrap_or(0.0),
                args.dsm_pixel_size_deg.unwrap_or(0.00027),
                args.dsm_pixel_size_deg.unwrap_or(0.00027),
                -9999.0,
            );
            match dsm_provider.fetch_quantized_surface(&args.bbox) {
                Ok(grid) => Some(grid),
                Err(e) => {
                    let msg = format!("Falha ao ler DSM real: {}.", e);
                    if let Some(ref tx) = telemetry_tx {
                        let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                    }
                    eprintln!("{} {}", "Aviso:".yellow().bold(), msg);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // 🚨 RECONEXÃO DEM: terreno nu de um GeoTIFF DEM local explícito, como
    // alternativa/complemento ao SRTM/LiDAR acima (útil offline, ou quando o
    // usuário tem um DEM oficial mais preciso que o SRTM público).
    let dem_override: Option<rustc_hash::FxHashMap<(i32, i32), i32>> = if args.terrain {
        if let Some(ref dem_path) = args.local_dem {
            let dem_provider = providers::dem_provider::DemProvider::new(
                dem_path.clone(),
                args.scale_h,
                args.scale_v,
                args.ground_level,
                args.dem_top_left_lat.unwrap_or(0.0),
                args.dem_top_left_lon.unwrap_or(0.0),
                args.dem_pixel_size_deg.unwrap_or(0.00027),
                args.dem_pixel_size_deg.unwrap_or(0.00027),
                -9999.0,
            );
            match dem_provider.fetch_quantized_elevation(&args.bbox) {
                Ok(grid) => Some(grid),
                Err(e) => {
                    let msg = format!("Falha ao ler DEM local: {}.", e);
                    if let Some(ref tx) = telemetry_tx {
                        let _ = tx.send(master_control::BesmSignal::Log(msg.clone()));
                    }
                    eprintln!("{} {}", "Aviso:".yellow().bold(), msg);
                    None
                }
            }
        } else {
            None
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
        // 🚨 BESM-6 RECONEXÃO: sem --spawn-lat/--spawn-lng explícitos, cai no
        // fallback (X=1,Z=1 relativo à bbox) em vez de deixar o spawn por conta
        // do próprio Minecraft (que pode cair fora da área gerada, no vazio).
        _ => Some(world_utils::calculate_default_spawn(&xzbbox)),
    };

    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
        provider_features: provider_specific_features, // 🚨 BESM-6: Injeta features governamentais
        elevation_data: elevation_data.map(std::sync::Arc::new), // 🚨 Reconexão: elevação real
        biome_grid: biome_grid.map(std::sync::Arc::new), // 🚨 Reconexão: bioma real
        surface_data: surface_data.map(std::sync::Arc::new), // 🚨 Reconexão: superfície real (DSM)
        dem_override: dem_override.map(std::sync::Arc::new), // 🚨 Reconexão: DEM local explícito
        ambient_forest: args.terrain && !args.no_ambient_forest, // 🚨 Reconexão: floresta ambiente
        telemetry_tx,
    };

    // Capturado antes do `generate_world_with_options` mover `xzbbox` — usado
    // pelo preview de mapa (`map_renderer::render_world_map` abaixo, só no build
    // com GUI, que é quem exibe a imagem).
    #[cfg(feature = "gui")]
    let (preview_min_x, preview_max_x) = (xzbbox.min_x(), xzbbox.max_x());
    #[cfg(feature = "gui")]
    let (preview_min_z, preview_max_z) = (xzbbox.min_z(), xzbbox.max_z());

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
                    // 🚨 BESM-6 RECONEXÃO: no build com GUI, usa a versão mais completa
                    // (também atualiza o NBT `Player/Pos`, não só `SpawnX/Y/Z`), que
                    // ficava definida mas nunca chamada; no build sem GUI (`gui.rs` nem
                    // compila), mantém o fallback que já funcionava.
                    #[cfg(feature = "gui")]
                    let spawn_result = gui::set_player_spawn_in_level_dat(
                        generation_path.to_str().unwrap_or_default(),
                        spawn_x,
                        spawn_z,
                    );
                    #[cfg(not(feature = "gui"))]
                    let spawn_result =
                        world_utils::set_spawn_in_level_dat(&generation_path, spawn_x, spawn_z);

                    if let Err(e) = spawn_result {
                        eprintln!(
                            "{} Failed to set spawn point in level.dat: {}",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }

                // 🚨 BESM-6 RECONEXÃO: enriquece o nome do mundo Java com a área
                // reverse-geocoded (o Bedrock já faz isso na criação, via
                // `get_area_name_for_bedrock`); pulado em --offline (rede) e quando o
                // usuário já deu um --path próprio explícito (nome fora do padrão
                // "Pincelism World N", `add_localized_world_name` já detecta e ignora).
                if !args.offline {
                    world_utils::add_localized_world_name(generation_path.clone(), &args.bbox);
                }

                // 🚨 BESM-6 RECONEXÃO: gera o PNG de preview top-down (só faz sentido
                // com a GUI, que é quem exibe a imagem) e avisa a janela Tauri que ele
                // está pronto — as duas peças existiam prontas e nunca eram chamadas.
                #[cfg(feature = "gui")]
                {
                    match map_renderer::render_world_map(
                        &generation_path,
                        preview_min_x,
                        preview_max_x,
                        preview_min_z,
                        preview_max_z,
                    ) {
                        Ok(_) => progress::emit_map_preview_ready(),
                        Err(e) => eprintln!(
                            "{} Failed to render map preview: {}",
                            "Warning:".yellow().bold(),
                            e
                        ),
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
        }
    }
}

/// Registra todos os provedores de dados (OSM + governamentais) num `ProviderManager`,
/// cada um condicionado à flag de CLI correspondente estar presente.
///
/// 🚨 BESM-6 RECONEXÃO: extraído de `run_generation_pipeline` para que o dashboard
/// `MasterControl` (modo Tile Streaming, ver `main()` abaixo) possa montar o MESMO
/// conjunto de provedores que o pipeline de um único lote — antes, o dashboard nunca
/// recebia um `ProviderManager` de verdade (a chamada `MasterControl::new()` nem
/// compilava: o construtor exige `Arc<ProviderManager>` + `Arc<Args>`).
fn register_providers(provider_manager: &mut providers::ProviderManager, args: &Args) {
    // Prioridade 10 (Base)
    provider_manager.register_provider(Box::new(providers::osm_provider::OSMProvider::new(
        args.scale_h,
        args.file.clone(),
        args.offline,
        args.downloader.as_str().to_string(),
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

    // 🚨 BESM-6 RECONEXÃO: IndoorUtilityProvider — o pipeline CAESB/CEB
    // (interiores, plantas baixas, saneamento) existia pronto desde outra rodada
    // de reconexão (ver `data_processing.rs`'s `is_caesb_infrastructure_feature`,
    // que já sabe rotear features de Sanitation/Utility/Sewage/Indoor/Power/
    // Telecom para o motor especializado), mas nada nunca instanciava o
    // provedor em si — o roteador estava pronto, a fonte nunca era ligada.
    if let Some(ref caesb_path) = args.local_caesb_geojson {
        provider_manager.register_provider(Box::new(
            providers::indoor_utility_provider::IndoorUtilityProvider::new(
                caesb_path.clone(),
                args.scale_h,
                1,
            ),
        ));
    }

    // 🚨 BESM-6 RECONEXÃO: CsvProvider — listagens tabulares do dados.df.gov.br
    // (postes, árvores da NOVACAP, paragens de ônibus). `semantic_override: None`
    // deixa o provedor inferir o grupo semântico a partir das próprias colunas/tags.
    if let Some(ref csv_path) = args.local_csv {
        provider_manager.register_provider(Box::new(providers::csv_provider::CsvProvider::new(
            csv_path.clone(),
            args.scale_h,
            5,
            None,
        )));
    }

    // 🚨 BESM-6 RECONEXÃO: KmlProvider — tombamento IPHAN, bacias ADASA, Metrô-DF.
    if let Some(ref kml_path) = args.local_kml {
        provider_manager.register_provider(Box::new(providers::kml_provider::KmlProvider::new(
            kml_path.clone(),
            args.scale_h,
            2,
            None,
        )));
    }

    // 🚨 BESM-6 RECONEXÃO: Tiles3DProvider — streaming de malhas 3D texturizadas
    // (OGC 3D Tiles/Cesium) com culling espacial HLOD.
    if let Some(ref tiles3d_url) = args.tiles3d_endpoint {
        provider_manager.register_provider(Box::new(
            providers::tiles3d_provider::Tiles3DProvider::new(
                tiles3d_url.clone(),
                args.scale_h,
                args.scale_v,
                2,
            ),
        ));
    }

    // 🚨 BESM-6 RECONEXÃO: LidarProvider — o mesmo arquivo .las/.laz de --local-lidar
    // já alimenta o heightmap (`elevation_data::fetch_elevation_data`, acima); aqui ele
    // TAMBÉM alimenta o pipeline de Features vetoriais (prédios/vegetação extraídos por
    // classificação de pontos), que antes nunca era chamado.
    if let Some(ref lidar_path) = args.local_lidar {
        provider_manager.register_provider(Box::new(
            providers::lidar_provider::LidarProvider::new(
                lidar_path.clone(),
                args.scale_h,
                args.scale_v,
                1,
                &format!("EPSG:{}", args.epsg),
            ),
        ));
    }

    // 🚨 BESM-6 RECONEXÃO: MeshProvider — malhas de fotogrametria (.obj/.gltf) de
    // monumentos/estátuas. `crs_source: None` e offset zero assumem que a malha já
    // veio pré-alinhada ao sistema local do motor (caso comum para scans de drone
    // recortados manualmente); um CRS/offset explícito pode ser adicionado via nova
    // flag de CLI caso surja a necessidade real de malhas em CRS bruto.
    if let Some(ref mesh_path) = args.local_mesh {
        provider_manager.register_provider(Box::new(providers::mesh_provider::MeshProvider::new(
            mesh_path.clone(),
            args.scale_h,
            args.scale_v,
            1,
            None,
            0.0,
            0.0,
            0.0,
        )));
    }

    // 🚨 BESM-6 RECONEXÃO: PostGisProvider — consulta espacial direta ao SISDIA/GDF
    // via PostgreSQL+PostGIS. `postgis_table`/`postgis_geom_column` são necessários
    // porque não existe um nome de tabela/coluna universal a assumir por padrão.
    if let (Some(ref pg_url), Some(ref pg_table)) = (&args.postgis_url, &args.postgis_table) {
        let geom_column = args.postgis_geom_column.as_deref().unwrap_or("geom");
        provider_manager.register_provider(Box::new(
            providers::postgis_provider::PostGisProvider::new(
                pg_url.clone(),
                pg_table,
                geom_column,
                args.scale_h,
                1,
            ),
        ));
    }

    // 🚨 BESM-6 RECONEXÃO: MvtProvider — streaming de Mapbox Vector Tiles. A
    // decodificação MVT em si ainda é um placeholder (ver o aviso que o próprio
    // provedor imprime em `fetch_features`); registrá-lo aqui já é o ponto de conexão
    // correto para quando a decodificação for implementada, e por ora é inofensivo
    // (retorna zero features com um aviso claro em vez de a flag ser silenciosamente
    // ignorada, que era o comportamento antes desta reconexão).
    if let Some(ref mvt_url) = args.mvt_endpoint {
        provider_manager.register_provider(Box::new(providers::mvt_provider::MvtProvider::new(
            mvt_url.clone(),
            None,
            14,
            args.scale_h,
            5,
            None,
        )));
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

    // 🚨 BESM-6: `--view-world` interceptado cru em `std::env::args()`, ANTES do
    // parsing normal do clap — ver o comentário de módulo em `world_viewer.rs`
    // para o porquê (esse modo não usa `--bbox`, obrigatório em `Args` em todo
    // outro modo; tornar `bbox` opcional só para isto tocaria dezenas de
    // chamadores em toda a base de código). Sobe um servidor HTTP local
    // mostrando o relevo 3D do mundo já gerado em `<PASTA>` e nunca retorna
    // (roda até `Ctrl+C`).
    let raw_args: Vec<String> = std::env::args().collect();
    if let Some(idx) = raw_args.iter().position(|a| a == "--view-world") {
        let Some(world_dir) = raw_args.get(idx + 1) else {
            eprintln!("Uso: pincelism --view-world <PASTA_DO_MUNDO> [--port <PORTA>]");
            std::process::exit(1);
        };
        let port = raw_args
            .iter()
            .position(|a| a == "--port")
            .and_then(|i| raw_args.get(i + 1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        world_viewer::serve(PathBuf::from(world_dir), port);
        return;
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
            // 🚨 BESM-6 RECONEXÃO: `MasterControl::new` exige `Arc<ProviderManager>` +
            // `Arc<Args>` — nada os construía aqui antes (chamada de 0 argumentos,
            // que nunca chegava a compilar neste branch específico sob
            // `--all-features`, já que `gui` é uma feature padrão). O dashboard
            // nunca lê `args.bbox` (cada tile calcula seu próprio bbox a partir do
            // preset de `MacroRegion` escolhido interativamente, ver
            // `master_control::dispatch_generation`), então usamos um placeholder
            // geograficamente válido só para satisfazer o parser do clap; todo o
            // resto usa os mesmos defaults do caminho de CLI (`run_cli`) — via
            // `Args::parse_from` + `register_providers`, em vez de listar cada campo
            // manualmente (frágil: quebra a cada novo campo adicionado a `Args`).
            let mut args = Args::parse_from(["pincelism", "--bbox", "-16.0,-48.0,-15.5,-47.5"]);
            if let Err(e) = args::validate_args(&mut args) {
                eprintln!("{} {}", "Aviso:".yellow().bold(), e);
            }

            let mut provider_manager = providers::ProviderManager::new();
            register_providers(&mut provider_manager, &args);

            let mut dashboard = master_control::MasterControl::new(
                std::sync::Arc::new(provider_manager),
                std::sync::Arc::new(args),
            );
            dashboard.run_interactive_shell();
            return;
        }
    }

    run_cli();
}
