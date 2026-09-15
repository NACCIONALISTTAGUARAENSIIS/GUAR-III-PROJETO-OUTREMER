use crate::args::Args;
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::{self, emit_gui_progress_update};
use crate::telemetry;
use crate::version_check;
use fastnbt::Value;
use flate2::read::GzDecoder;
use fs2::FileExt;
use log::LevelFilter;
use rfd::FileDialog;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{env, fs, io::Write};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};

/// Manages the session.lock file for a Minecraft world directory.
///
/// 🚨 BESM-6 RECONEXÃO: chamado por `main::run_generation_pipeline` (Java
/// Anvil apenas — Bedrock usa LevelDB, que tem seu próprio lock nativo) antes
/// de qualquer escrita, para impedir duas gerações concorrentes corromperem o
/// mesmo diretório de mundo. `pub(crate)` porque o ponto de chamada real vive
/// em `main.rs`, fora deste módulo.
pub(crate) struct SessionLock {
    file: fs::File,
    path: PathBuf,
}

#[allow(dead_code)]
impl SessionLock {
    /// Creates and locks a session.lock file in the specified world directory
    pub(crate) fn acquire(world_path: &Path) -> Result<Self, String> {
        let session_lock_path = world_path.join("session.lock");

        // Create or open the session.lock file
        let file = fs::File::create(&session_lock_path)
            .map_err(|e| format!("Failed to create session.lock file: {e}"))?;

        // Write the snowman character (U+2603) as specified by Minecraft format
        let snowman_bytes = "☃".as_bytes(); // This is UTF-8 encoded E2 98 83
        (&file)
            .write_all(snowman_bytes)
            .map_err(|e| format!("Failed to write to session.lock file: {e}"))?;

        // Acquire an exclusive lock on the file
        file.try_lock_exclusive()
            .map_err(|e| format!("Failed to acquire lock on session.lock file: {e}"))?;

        Ok(SessionLock {
            file,
            path: session_lock_path,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Release the lock and remove the session.lock file
        //
        // Chamada qualificada (`fs2::FileExt::unlock`, não `self.file.unlock()`): a
        // partir do Rust 1.89 `std::fs::File` ganhou um `unlock()` inerente próprio,
        // que teria prioridade de resolução sobre o trait do `fs2` em toolchains mais
        // novas — mas o MSRV declarado no Cargo.toml (1.75) é anterior a isso. Chamar
        // o trait explicitamente garante o mesmo método em qualquer toolchain
        // suportada, e resolve o lint `incompatible_msrv` do clippy.
        let _ = fs2::FileExt::unlock(&self.file);
        let _ = fs::remove_file(&self.path);
    }
}

pub fn run_gui() {
    // Configure thread pool with 90% CPU cap to keep system responsive
    crate::floodfill_cache::configure_rayon_thread_pool(0.9);

    // Clean up old cached elevation tiles on startup
    crate::elevation_data::cleanup_old_cached_tiles();

    // Launch the UI
    println!("Launching UI...");

    // Install panic hook for crash reporting
    telemetry::install_panic_hook();

    // Workaround WebKit2GTK issue with NVIDIA drivers and graphics issues
    // Source: https://github.com/tauri-apps/tauri/issues/10702
    #[cfg(target_os = "linux")]
    unsafe {
        // Disable problematic GPU features that cause map loading issues
        env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

        // Force software rendering for better compatibility
        env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        env::set_var("GALLIUM_DRIVER", "softpipe");

        // Note: Removed sandbox disabling for security reasons
        // Note: Removed Qt WebEngine flags as they don't apply to Tauri
    }

    tauri::Builder::default()
        .plugin(
            LogBuilder::default()
                .level(LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::LogDir {
                        file_name: Some("arnis".into()),
                    }),
                    Target::new(TargetKind::Stdout),
                ])
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            gui_create_world,
            gui_get_default_save_path,
            gui_set_save_path,
            gui_pick_save_directory,
            gui_start_generation,
            gui_get_version,
            gui_check_for_updates,
            gui_get_world_map_data,
            gui_show_in_folder
        ])
        .setup(|app| {
            let app_handle = app.handle();
            let main_window = tauri::Manager::get_webview_window(app_handle, "main")
                .expect("Failed to get main window");
            progress::set_main_window(main_window);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Error while starting the application UI (Tauri)");
}

/// Detects the default Minecraft Java Edition saves directory for the current OS.
/// Checks standard install paths including Flatpak on Linux.
/// Falls back to Desktop, then current directory.
fn detect_minecraft_saves_directory() -> PathBuf {
    // Try standard Minecraft saves directories per OS
    let mc_saves: Option<PathBuf> = if cfg!(target_os = "windows") {
        env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join(".minecraft").join("saves"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|home| {
            home.join("Library/Application Support/minecraft")
                .join("saves")
        })
    } else if cfg!(target_os = "linux") {
        dirs::home_dir().map(|home| {
            let flatpak_path = home.join(".var/app/com.mojang.Minecraft/.minecraft/saves");
            if flatpak_path.exists() {
                flatpak_path
            } else {
                home.join(".minecraft/saves")
            }
        })
    } else {
        None
    };

    if let Some(saves_dir) = mc_saves {
        if saves_dir.exists() {
            return saves_dir;
        }
    }

    // Fallback to Desktop
    if let Some(desktop) = dirs::desktop_dir() {
        if desktop.exists() {
            return desktop;
        }
    }

    // Last resort: current directory
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Returns the default save path (auto-detected on first run).
/// The frontend stores/retrieves this via localStorage and passes it here for validation.
#[tauri::command]
fn gui_get_default_save_path() -> String {
    detect_minecraft_saves_directory().display().to_string()
}

/// Validates and returns a user-provided save path.
/// Returns the path string if valid, or an error message.
#[tauri::command]
fn gui_set_save_path(path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("Path does not exist.".to_string());
    }
    if !p.is_dir() {
        return Err("Path is not a directory.".to_string());
    }
    Ok(path)
}

/// Opens a native folder-picker dialog and returns the chosen path.
#[tauri::command]
fn gui_pick_save_directory(start_path: String) -> Result<String, String> {
    let start = PathBuf::from(&start_path);
    let mut dialog = FileDialog::new();
    if start.is_dir() {
        dialog = dialog.set_directory(&start);
    }
    match dialog.pick_folder() {
        Some(folder) => Ok(folder.display().to_string()),
        None => Ok(start_path),
    }
}

/// Creates a new Java Edition world in the given base save directory.
/// Called when the user clicks "Create World".
#[tauri::command]
fn gui_create_world(save_path: String) -> Result<String, i32> {
    let trimmed = save_path.trim();
    if trimmed.is_empty() {
        return Err(3);
    }
    let base = PathBuf::from(trimmed);
    if !base.is_dir() {
        return Err(3); // Error code 3: Failed to create new world
    }
    create_new_world(&base).map_err(|_| 3)
}

fn create_new_world(base_path: &Path) -> Result<String, String> {
    crate::world_utils::create_new_world(base_path)
}

/// Sets the player spawn point in level.dat using Minecraft XZ coordinates.
/// The Y coordinate is set to a temporary value (150) and will be updated
/// after terrain generation by `update_player_spawn_y_after_generation`.
pub(crate) fn set_player_spawn_in_level_dat(
    world_path: &str,
    spawn_x: i32,
    spawn_z: i32,
) -> Result<(), String> {
    // Default y spawn position since terrain elevation cannot be determined yet
    let y = 150.0;

    // Read and update the level.dat file
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Set world spawn point
            data.insert("SpawnX".to_string(), Value::Int(spawn_x));
            data.insert("SpawnY".to_string(), Value::Int(y as i32));
            data.insert("SpawnZ".to_string(), Value::Int(spawn_z));

            // Update player position if Player compound exists
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    // Safely update position values with bounds checking
                    if pos.len() >= 3 {
                        if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                            *pos_x = spawn_x as f64;
                        }
                        if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                            *pos_y = y;
                        }
                        if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                            *pos_z = spawn_z as f64;
                        }
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

// Function to update player spawn Y coordinate based on terrain height after generation
// This updates the spawn Y coordinate to be at terrain height + 3 blocks
pub fn update_player_spawn_y_after_generation(
    world_path: &Path,
    bbox_text: String,
    scale: f64,
    ground: &Ground,
) -> Result<(), String> {
    use crate::coordinate_system::transformation::CoordTransformer;

    // Read the current level.dat file to get existing spawn coordinates
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Get existing spawn coordinates and calculate new Y based on terrain
    let (existing_spawn_x, existing_spawn_z) = if let Value::Compound(ref root) = nbt_data {
        if let Some(Value::Compound(ref data)) = root.get("Data") {
            let spawn_x = data.get("SpawnX").and_then(|v| {
                if let Value::Int(x) = v {
                    Some(*x)
                } else {
                    None
                }
            });
            let spawn_z = data.get("SpawnZ").and_then(|v| {
                if let Value::Int(z) = v {
                    Some(*z)
                } else {
                    None
                }
            });

            match (spawn_x, spawn_z) {
                (Some(x), Some(z)) => (x, z),
                _ => {
                    return Err("Spawn coordinates not found in level.dat".to_string());
                }
            }
        } else {
            return Err("Invalid level.dat structure: no Data compound".to_string());
        }
    } else {
        return Err("Invalid level.dat structure: root is not a compound".to_string());
    };

    // Calculate terrain-based Y coordinate
    let spawn_y = if ground.elevation_enabled {
        // Parse coordinates for terrain lookup
        let llbbox = LLBBox::from_str(&bbox_text)
            .map_err(|e| format!("Failed to parse bounding box for spawn point:\n{e}"))?;
        let (_, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&llbbox, scale)
            .map_err(|e| format!("Failed to build transformation:\n{e}"))?;

        // Calculate relative coordinates for ground system
        let relative_x = existing_spawn_x - xzbbox.min_x();
        let relative_z = existing_spawn_z - xzbbox.min_z();
        let terrain_point = XZPoint::new(relative_x, relative_z);

        ground.level(terrain_point) + 3 // Add 3 blocks above terrain for safety
    } else {
        -61 // Default Y if no terrain
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Only update the Y coordinate, keep existing X and Z
            data.insert("SpawnY".to_string(), Value::Int(spawn_y));

            // Update player position - only Y coordinate
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    // Safely update Y position with bounds checking
                    if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                        *pos_y = spawn_y as f64;
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

#[tauri::command]
fn gui_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn gui_check_for_updates() -> Result<bool, String> {
    match version_check::check_for_updates() {
        Ok(is_newer) => Ok(is_newer),
        Err(e) => Err(format!("Error checking for updates: {e}")),
    }
}

/// Returns the world map image data as base64 and geo bounds for overlay display.
/// Returns None if the map image or metadata doesn't exist.
#[tauri::command]
fn gui_get_world_map_data(world_path: String) -> Result<Option<WorldMapData>, String> {
    let world_dir = PathBuf::from(&world_path);
    let map_path = world_dir.join("arnis_world_map.png");
    let metadata_path = world_dir.join("metadata.json");

    // Check if both files exist
    if !map_path.exists() || !metadata_path.exists() {
        return Ok(None);
    }

    // Read and encode the map image as base64
    let image_data = fs::read(&map_path).map_err(|e| format!("Failed to read map image: {e}"))?;
    let base64_image =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

    // Read metadata
    let metadata_content =
        fs::read_to_string(&metadata_path).map_err(|e| format!("Failed to read metadata: {e}"))?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_content)
        .map_err(|e| format!("Failed to parse metadata: {e}"))?;

    // Extract geo bounds (metadata uses camelCase from serde)
    let min_lat = metadata["minGeoLat"]
        .as_f64()
        .ok_or("Missing minGeoLat in metadata")?;
    let max_lat = metadata["maxGeoLat"]
        .as_f64()
        .ok_or("Missing maxGeoLat in metadata")?;
    let min_lon = metadata["minGeoLon"]
        .as_f64()
        .ok_or("Missing minGeoLon in metadata")?;
    let max_lon = metadata["maxGeoLon"]
        .as_f64()
        .ok_or("Missing maxGeoLon in metadata")?;

    // Extract Minecraft coordinate bounds
    let min_mc_x = metadata["minMcX"].as_i64().unwrap_or(0) as i32;
    let max_mc_x = metadata["maxMcX"].as_i64().unwrap_or(0) as i32;
    let min_mc_z = metadata["minMcZ"].as_i64().unwrap_or(0) as i32;
    let max_mc_z = metadata["maxMcZ"].as_i64().unwrap_or(0) as i32;

    Ok(Some(WorldMapData {
        image_base64: format!("data:image/png;base64,{}", base64_image),
        min_lat,
        max_lat,
        min_lon,
        max_lon,
        min_mc_x,
        max_mc_x,
        min_mc_z,
        max_mc_z,
    }))
}

/// Data structure for world map overlay
#[derive(serde::Serialize)]
struct WorldMapData {
    image_base64: String,
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    // Minecraft coordinate bounds for coordinate copying
    min_mc_x: i32,
    max_mc_x: i32,
    min_mc_z: i32,
    max_mc_z: i32,
}

/// Opens the file with default application (Windows) or shows in file explorer (macOS/Linux)
#[tauri::command]
fn gui_show_in_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // On Windows, try to open with default application (Minecraft Bedrock)
        // If that fails, show in Explorer
        if std::process::Command::new("cmd")
            .args(["/C", "start", "", &path])
            .spawn()
            .is_err()
        {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
                .spawn()
                .map_err(|e| format!("Failed to open explorer: {}", e))?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, just reveal in Finder
        std::process::Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| format!("Failed to open Finder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, just show in file manager
        let path_parent = std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        // Try nautilus with select first, then fall back to xdg-open on parent
        if std::process::Command::new("nautilus")
            .args(["--select", &path])
            .spawn()
            .is_err()
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&path_parent)
                .spawn();
        }
    }

    Ok(())
}

// 🚨 BESM-6 Tweak: A ponte da GUI antiga para o Pipeline Governamental
#[tauri::command]
#[allow(clippy::too_many_arguments)]
#[allow(unused_variables)]
fn gui_start_generation(
    bbox_text: String,
    selected_world: String,
    world_scale: f64,
    ground_level: i32,
    terrain_enabled: bool,
    skip_osm_objects: bool,
    interior_enabled: bool,
    roof_enabled: bool,
    fillground_enabled: bool,
    city_boundaries_enabled: bool,
    is_new_world: bool,
    spawn_point: Option<(f64, f64)>,
    telemetry_consent: bool,
    world_format: String,
) -> Result<(), String> {
    use crate::args::{DemSource, Downloader, LayerPriority};
    use progress::emit_gui_error; // Imports necessários para os Enums

    // Store telemetry consent for crash reporting
    telemetry::set_telemetry_consent(telemetry_consent);
    telemetry::send_generation_click();

    let llbbox = match LLBBox::from_str(&bbox_text) {
        Ok(bbox) => bbox,
        Err(e) => {
            let error_msg = format!("Failed to parse bounding box: {e}");
            emit_gui_error(&error_msg);
            return Err(error_msg);
        }
    };

    let world_path = PathBuf::from(&selected_world);
    let is_bedrock = world_format == "bedrock";

    // Mapeamento direto de Argumentos para a Nova Estrutura de Comando BESM-6
    let args = Args {
        bbox: llbbox,
        file: None,
        save_json_file: None,
        path: Some(world_path),
        bedrock: is_bedrock,
        downloader: Downloader::Requests, // 🚨 Corrigido: Uso do Enum em vez de String
        threads: 0,                       // Auto
        offline: false,
        scale_h: world_scale,
        scale_v: 1.15, // Rigor Governamental
        scale: None,   // Legacy
        local_shp: None,
        local_geojson: None,
        local_lidar: None,
        wfs_endpoint: None,
        epsg: 31983, // Padrão Brasília
        dem: DemSource::AwsSrtm,
        local_dem: None,
        cache_dir: PathBuf::from("./arnis_cache"),
        max_area_km2: 10000.0,
        max_dem_tiles: 200,
        max_osm_features: 5000000,
        priority_layer: vec![
            LayerPriority::Shp,
            LayerPriority::Lidar,
            LayerPriority::Wfs,
            LayerPriority::Geojson,
            LayerPriority::Osm,
        ],
        enable_underground_wfs: false,
        postgis_url: None,
        postgis_table: None,
        postgis_geom_column: None,
        local_gpkg: None,
        local_pbf: None,
        mvt_endpoint: None,
        local_citygml: None,
        local_ifc: None,
        local_mesh: None,
        // 🚨 RECONEXÃO: A GUI ainda não expõe os campos de dados governamentais
        // adicionados depois (IFC BIM, IBGE/SICAR, MapBiomas, DSM, CAESB/CSV/KML,
        // 3D Tiles) — `Args` passou a exigi-los no initializer, mas nenhum control de
        // UI foi criado para eles ainda. `None`/`false` preserva o comportamento de
        // sempre (fallback matemático do motor, sem esses dados extras), até a GUI
        // ganhar os campos.
        local_caesb_geojson: None,
        local_csv: None,
        local_kml: None,
        tiles3d_endpoint: None,
        mapbiomas_tiff: None,
        ibge_shapefile: None,
        sicar_shapefile: None,
        mapbiomas_top_left_lat: None,
        mapbiomas_top_left_lon: None,
        mapbiomas_pixel_size_deg: None,
        local_dsm: None,
        dsm_top_left_lat: None,
        dsm_top_left_lon: None,
        dsm_pixel_size_deg: None,
        dem_top_left_lat: None,
        dem_top_left_lon: None,
        dem_pixel_size_deg: None,
        no_ambient_forest: false,
        ground_level,
        terrain: terrain_enabled,
        interior: interior_enabled,
        roof: roof_enabled,
        fillground: fillground_enabled,
        city_boundaries: city_boundaries_enabled,
        debug: false,
        timeout: Some(std::time::Duration::from_secs(60)),
        spawn_lat: spawn_point.map(|p| p.0),
        spawn_lng: spawn_point.map(|p| p.1),
    };

    // Despacha para a thread principal de forma desanexada
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tokio::task::spawn_blocking(move || {
            // Chamamos a função principal definida no main.rs/lib.rs
            crate::run_generation_pipeline(args, None);
        })
        .await
        {
            let error_msg = format!("Erro Crítico no despachante do Pincelism: {}", e);
            eprintln!("{}", error_msg);
            emit_gui_error(&error_msg);
        }

        emit_gui_progress_update(100.0, "Done! World generation completed.");
    });

    Ok(())
}
