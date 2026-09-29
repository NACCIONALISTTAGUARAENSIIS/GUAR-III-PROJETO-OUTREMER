use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::retrieve_data;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use fastnbt::Value;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

/// Returns the Desktop directory for Bedrock .mcworld file output.
/// Falls back to home directory, then current directory.
pub fn get_bedrock_output_directory() -> PathBuf {
    dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Gets the area name for a given bounding box using the center point.
pub fn get_area_name_for_bedrock(bbox: &LLBBox) -> String {
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => "Unknown Location".to_string(),
    }
}

/// Adds a localized area name to the world name in level.dat.
///
/// 🚨 BESM-6 RECONEXÃO: espelha `get_area_name_for_bedrock` acima, mas para Java
/// Anvil. O Bedrock já embute o nome da área no nome da pasta na hora da criação
/// (`build_bedrock_output`); o Java não pode fazer isso na criação porque o nome
/// só é resolvido depois de checar colisão de contador ("Pincelism World N") em
/// `create_new_world` — então enriquecemos o `LevelName` já gravado, chamado por
/// `main::run_generation_pipeline` logo após a criação do mundo Java.
pub fn add_localized_world_name(world_path: PathBuf, bbox: &LLBBox) -> PathBuf {
    // Only proceed if the path exists
    if !world_path.exists() {
        return world_path;
    }

    // Check the level.dat file first to get the current name
    let level_path = world_path.join("level.dat");

    if !level_path.exists() {
        return world_path;
    }

    // Try to read the current world name from level.dat
    let Ok(level_data) = std::fs::read(&level_path) else {
        return world_path;
    };

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if decoder.read_to_end(&mut decompressed_data).is_err() {
        return world_path;
    }

    let Ok(Value::Compound(ref root)) = fastnbt::from_bytes::<Value>(&decompressed_data) else {
        return world_path;
    };

    let Some(Value::Compound(ref data)) = root.get("Data") else {
        return world_path;
    };

    let Some(Value::String(current_name)) = data.get("LevelName") else {
        return world_path;
    };

    // Only modify if it's a Pincelism world and doesn't already have an area name
    if !current_name.starts_with("Pincelism World ") || current_name.contains(": ") {
        return world_path;
    }

    // Calculate center coordinates of bbox
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    // Try to fetch the area name
    let area_name = match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => return world_path, // Keep original name if no area name found
    };

    // Create new name with localized area name, ensuring total length doesn't exceed 30 characters
    let base_name = current_name.clone();
    let max_area_name_len = 30 - base_name.len() - 2; // 2 chars for ": "

    let truncated_area_name =
        if area_name.chars().count() > max_area_name_len && max_area_name_len > 0 {
            // Truncate the area name to fit within the 30 character limit
            area_name
                .chars()
                .take(max_area_name_len)
                .collect::<String>()
        } else if max_area_name_len == 0 {
            // If base name is already too long, don't add area name
            return world_path;
        } else {
            area_name
        };

    let new_name = format!("{base_name}: {truncated_area_name}");

    // Update the level.dat file with the new name
    if let Ok(level_data) = std::fs::read(&level_path) {
        let mut decoder = GzDecoder::new(level_data.as_slice());
        let mut decompressed_data = Vec::new();
        if decoder.read_to_end(&mut decompressed_data).is_ok() {
            if let Ok(mut nbt_data) = fastnbt::from_bytes::<Value>(&decompressed_data) {
                // Update the level name in NBT data
                if let Value::Compound(ref mut root) = nbt_data {
                    if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
                        data.insert("LevelName".to_string(), Value::String(new_name));

                        // Save the updated NBT data
                        if let Ok(serialized_data) = fastnbt::to_bytes(&nbt_data) {
                            let mut encoder = flate2::write::GzEncoder::new(
                                Vec::new(),
                                flate2::Compression::default(),
                            );
                            if encoder.write_all(&serialized_data).is_ok() {
                                if let Ok(compressed_data) = encoder.finish() {
                                    if let Err(e) = std::fs::write(&level_path, compressed_data) {
                                        eprintln!("Failed to update level.dat with area name: {e}");
                                        #[cfg(feature = "gui")]
                                        send_log(
                                            LogLevel::Warning,
                                            "Failed to update level.dat with area name",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Return the original path since we didn't change the directory name
    world_path
}

/// Calculates the default spawn point at X=1, Z=1 relative to the world origin.
/// Used as a fallback when the user does not pass --spawn-lat/--spawn-lng, so every
/// generated world still gets a sane spawn instead of Minecraft's own arbitrary default.
pub fn calculate_default_spawn(xzbbox: &XZBBox) -> (i32, i32) {
    (xzbbox.min_x() + 1, xzbbox.min_z() + 1)
}

/// Sanitizes an area name for safe use in filesystem paths.
/// Replaces characters that are invalid on Windows/macOS/Linux, trims whitespace,
/// and limits length to prevent excessively long filenames.
pub fn sanitize_for_filename(name: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_control() || invalid_chars.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    sanitized = sanitized.trim().to_string();

    // Limit length to avoid excessively long filenames
    const MAX_LEN: usize = 64;
    if sanitized.len() > MAX_LEN {
        // Find a valid UTF-8 char boundary at or before MAX_LEN bytes
        let cutoff = sanitized
            .char_indices()
            .take_while(|(idx, _)| *idx < MAX_LEN)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        sanitized.truncate(cutoff);
        sanitized = sanitized.trim_end().to_string();
    }

    if sanitized.is_empty() {
        "Unknown Location".to_string()
    } else {
        sanitized
    }
}

/// Builds the Bedrock output path and level name for a given bounding box.
/// Combines area name lookup, sanitization, and path construction.
pub fn build_bedrock_output(bbox: &LLBBox, output_dir: PathBuf) -> (PathBuf, String) {
    let area_name = get_area_name_for_bedrock(bbox);
    let safe_name = sanitize_for_filename(&area_name);
    let filename = format!("Pincelism {safe_name}.mcworld");
    let lvl_name = format!("Pincelism World: {safe_name}");
    (output_dir.join(&filename), lvl_name)
}

/// Creates a new Java Edition world in the given base directory.
///
/// Generates a unique "Pincelism World N" name, creates the directory structure
/// (with a `region/` subdirectory), writes the region template, level.dat
/// (with updated name, timestamp, and spawn position), and icon.png.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world(base_path: &Path) -> Result<String, String> {
    // Generate a unique world name with proper counter
    // Check for both "Pincelism World X" and "Pincelism World X: Location" patterns
    let mut counter: i32 = 1;
    let unique_name: String = loop {
        let candidate_name: String = format!("Pincelism World {counter}");
        let candidate_path: PathBuf = base_path.join(&candidate_name);

        // Check for exact match (no location suffix)
        let exact_match_exists = candidate_path.exists();

        // Check for worlds with location suffix (Pincelism World X: Location)
        let location_pattern = format!("Pincelism World {counter}: ");
        let location_match_exists = fs::read_dir(base_path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .any(|name| name.starts_with(&location_pattern))
            })
            .unwrap_or(false);

        if !exact_match_exists && !location_match_exists {
            break candidate_name;
        }
        counter += 1;
    };

    let new_world_path: PathBuf = base_path.join(&unique_name);

    // Create the new world directory structure
    fs::create_dir_all(new_world_path.join("region"))
        .map_err(|e| format!("Failed to create world directory: {e}"))?;

    // Copy the region template file
    const REGION_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/region.template");
    let region_path = new_world_path.join("region").join("r.0.0.mca");
    fs::write(&region_path, REGION_TEMPLATE)
        .map_err(|e| format!("Failed to create region file: {e}"))?;

    // Add the level.dat file
    const LEVEL_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/level.dat");

    // Decompress the gzipped level.template
    let mut decoder = GzDecoder::new(LEVEL_TEMPLATE);
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.template: {e}"))?;

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat template: {e}"))?;

    // Modify the LevelName, LastPlayed and player position fields
    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Update LevelName
            data.insert("LevelName".to_string(), Value::String(unique_name.clone()));

            // Update LastPlayed to the current Unix time in milliseconds
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("Failed to get current time: {e}"))?;
            let current_time_millis = current_time.as_millis() as i64;
            data.insert("LastPlayed".to_string(), Value::Long(current_time_millis));

            // Update player position and rotation
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if pos.len() < 3 {
                        return Err(
                            "Invalid level.dat template: Player Pos list has fewer than 3 elements"
                                .to_string(),
                        );
                    }
                    if let Value::Double(ref mut x) = pos[0] {
                        *x = -5.0;
                    }
                    if let Value::Double(ref mut y) = pos[1] {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos[2] {
                        *z = -5.0;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if rot.is_empty() {
                        return Err(
                            "Invalid level.dat template: Player Rotation list is empty".to_string()
                        );
                    }
                    if let Value::Float(ref mut x) = rot[0] {
                        *x = -45.0;
                    }
                }
            }
        }
    }

    // Serialize the updated NBT data back to bytes
    let serialized_level_data: Vec<u8> = fastnbt::to_bytes(&level_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_level_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    // Write the level.dat file
    fs::write(new_world_path.join("level.dat"), compressed_level_data)
        .map_err(|e| format!("Failed to create level.dat file: {e}"))?;

    // Add the icon.png file
    const ICON_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/icon.png");
    fs::write(new_world_path.join("icon.png"), ICON_TEMPLATE)
        .map_err(|e| format!("Failed to create icon.png file: {e}"))?;

    Ok(new_world_path.display().to_string())
}

/// Sets the player spawn point in an existing Java Edition level.dat file.
///
/// Updates both the world spawn point (SpawnX/SpawnY/SpawnZ) and the player
/// position if a Player compound exists. `spawn_y` é a cota real do terreno
/// no ponto de spawn (+ folga), calculada pelo chamador a partir do `Ground`
/// definitivo do mundo — usado nos dois builds (com e sem `gui`).
pub fn set_spawn_in_level_dat(
    world_path: &Path,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    // Read and decompress
    let level_data = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut nbt_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat NBT data: {e}"))?;

    // Update spawn point
    let data = match nbt_data {
        Value::Compound(ref mut root) => match root.get_mut("Data") {
            Some(Value::Compound(ref mut data)) => data,
            _ => {
                return Err(
                    "Invalid level.dat structure: missing or non-compound \"Data\" section"
                        .to_string(),
                );
            }
        },
        _ => {
            return Err(
                "Invalid level.dat structure: root NBT value is not a compound".to_string(),
            );
        }
    };

    data.insert("SpawnX".to_string(), Value::Int(spawn_x));
    data.insert("SpawnY".to_string(), Value::Int(spawn_y));
    data.insert("SpawnZ".to_string(), Value::Int(spawn_z));

    // Update player position if Player compound exists
    if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
        if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
            if pos.len() >= 3 {
                if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                    *pos_x = spawn_x as f64;
                }
                if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                    *pos_y = spawn_y as f64;
                }
                if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                    *pos_z = spawn_z as f64;
                }
            }
        }
    }

    // Serialize, compress, and write back
    let serialized_data = fastnbt::to_bytes(&nbt_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    fs::write(&level_path, compressed_data)
        .map_err(|e| format!("Failed to write updated level.dat: {e}"))?;

    Ok(())
}
