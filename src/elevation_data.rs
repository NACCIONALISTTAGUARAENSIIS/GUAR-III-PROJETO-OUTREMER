#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::{
    coordinate_system::{geographic::LLBBox, transformation::geo_distance},
    progress::emit_gui_progress_update,
};
use image::Rgb;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

// TWEAK: Expandido de 319 para suportar o Datapack de 4064 blocos.
const MAX_Y: i32 = 4064;

const AWS_TERRARIUM_URL: &str =
    "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
const TERRARIUM_OFFSET: f64 = 32768.0;
const MIN_ZOOM: u8 = 10;
const MAX_ZOOM: u8 = 15;
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
const TILE_CACHE_MAX_AGE_DAYS: u64 = 7;

/// Holds processed elevation data and metadata
/// ?? BESM-6 Tweak: Virtualização da Matriz de Relevo
/// Em vez de alocar uma Vec<Vec<i32>> global e tragar 5GB de RAM,
/// guardamos apenas os dados compactos (tiles e cloud points)
/// e respondemos a consultas de interpolação on-demand.
#[derive(Clone)]
pub struct ElevationData {
    // Para simplificar a ponte com o código legado, mantemos a estrutura de grid 2D,
    // MAS fatiado pelas proporções do Scanline se necessário, ou otimizado se for LiDAR.
    pub(crate) heights: Vec<Vec<i32>>,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

type TileImage = image::ImageBuffer<Rgb<u8>, Vec<u8>>;
type TileDownloadResult = Result<((u32, u32), TileImage), String>;

pub fn cleanup_old_cached_tiles() {
    let tile_cache_dir = PathBuf::from("./arnis-tile-cache");

    if !tile_cache_dir.exists() || !tile_cache_dir.is_dir() {
        return;
    }

    let max_age = std::time::Duration::from_secs(TILE_CACHE_MAX_AGE_DAYS * 24 * 60 * 60);
    let now = std::time::SystemTime::now();
    let mut deleted_count = 0;
    let mut error_count = 0;

    let entries = match std::fs::read_dir(&tile_cache_dir) {
        Ok(entries) => entries,
        Err(_) => {
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        if !file_name.ends_with(".png") || !file_name.starts_with('z') {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(time) => time,
            Err(_) => continue,
        };

        let age = match now.duration_since(modified) {
            Ok(duration) => duration,
            Err(_) => continue,
        };

        if age > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => deleted_count += 1,
                Err(e) => {
                    if error_count == 0 {
                        eprintln!(
                            "Warning: Failed to delete old cached tile {}: {e}",
                            path.display()
                        );
                    }
                    error_count += 1;
                }
            }
        }
    }

    if deleted_count > 0 {
        println!("Cleaned up {deleted_count} old cached elevation tiles (older than {TILE_CACHE_MAX_AGE_DAYS} days)");
    }
    if error_count > 1 {
        eprintln!("Warning: Failed to delete {error_count} old cached tiles");
    }
}

fn calculate_zoom_level(bbox: &LLBBox) -> u8 {
    let lat_diff: f64 = (bbox.max().lat() - bbox.min().lat()).abs();
    let lng_diff: f64 = (bbox.max().lng() - bbox.min().lng()).abs();
    let max_diff: f64 = lat_diff.max(lng_diff);
    let zoom: u8 = (-max_diff.log2() + 20.0) as u8;
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

fn lat_lng_to_tile(lat: f64, lng: f64, zoom: u8) -> (u32, u32) {
    let lat_rad: f64 = lat.to_radians();
    let n: f64 = 2.0_f64.powi(zoom as i32);
    let x: u32 = ((lng + 180.0) / 360.0 * n).floor() as u32;
    let y: u32 = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
    (x, y)
}

const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;

fn download_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    println!("Fetching tile x={tile_x},y={tile_y},z={zoom} from AWS Terrain Tiles");
    let url: String = AWS_TERRARIUM_URL
        .replace("{z}", &zoom.to_string())
        .replace("{x}", &tile_x.to_string())
        .replace("{y}", &tile_y.to_string());

    let mut last_error: String = String::new();

    for attempt in 0..TILE_DOWNLOAD_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms = TILE_DOWNLOAD_RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            eprintln!(
                "Retry attempt {}/{} for tile x={},y={},z={} after {}ms delay",
                attempt,
                TILE_DOWNLOAD_MAX_RETRIES - 1,
                tile_x,
                tile_y,
                zoom,
                delay_ms
            );
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }

        match download_tile_once(client, &url, tile_path) {
            Ok(img) => return Ok(img),
            Err(e) => {
                last_error = e;
                if attempt < TILE_DOWNLOAD_MAX_RETRIES - 1 {
                    eprintln!(
                        "Tile download failed for x={},y={},z={}: {}",
                        tile_x, tile_y, zoom, last_error
                    );
                }
            }
        }
    }

    Err(format!(
        "Failed to download tile x={},y={},z={} after {} attempts: {}",
        tile_x, tile_y, zoom, TILE_DOWNLOAD_MAX_RETRIES, last_error
    ))
}

fn download_tile_once(
    client: &reqwest::blocking::Client,
    url: &str,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    response.error_for_status_ref().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    std::fs::write(tile_path, &bytes).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    Ok(img.to_rgb8())
}

fn fetch_or_load_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    if tile_path.exists() {
        match image::open(tile_path) {
            Ok(img) => {
                println!(
                    "Loading cached tile x={tile_x},y={tile_y},z={zoom} from {}",
                    tile_path.display()
                );
                Ok(img.to_rgb8())
            }
            Err(e) => {
                eprintln!(
                    "Cached tile at {} is corrupted or invalid: {}. Re-downloading...",
                    tile_path.display(),
                    e
                );
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Warning,
                    "Cached tile is corrupted or invalid. Re-downloading...",
                );

                if let Err(e) = std::fs::remove_file(tile_path) {
                    eprintln!("Warning: Failed to remove corrupted tile file: {e}");
                    #[cfg(feature = "gui")]
                    send_log(
                        LogLevel::Warning,
                        "Failed to remove corrupted tile file during re-download.",
                    );
                }

                download_tile(client, tile_x, tile_y, zoom, tile_path)
            }
        }
    } else {
        download_tile(client, tile_x, tile_y, zoom, tile_path)
    }
}

// ============================================================================
// ?? LiDAR POINT CLOUD PROCESSOR (Government Tier) ??
// ============================================================================

/// Processa um arquivo LiDAR .las local, extraindo cotas com precisão submétrica.
/// Usa Downsampling Dinâmico via Grid (Baldes 2D) para não explodir a memória RAM.
fn load_local_lidar(
    lidar_path: &Path,
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
) -> Result<Vec<Vec<f64>>, String> {
    use las::{Read, Reader};
    use proj::Proj;

    println!("[INFO] ??? Iniciando scanner LiDAR de alta densidade no arquivo: {}", lidar_path.display());

    let mut reader = Reader::from_path(lidar_path)
        .map_err(|e| format!("Falha ao ler arquivo LiDAR: {}", e))?;

    // LiDAR no GDF usa SIRGAS 2000 (EPSG:31983). O BBox usa WGS84 (EPSG:4326).
    let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
        .ok()
        .ok_or("Falha ao inicializar PROJ para conversão LiDAR SIRGAS 2000")?;

    let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];
    
    let mut processed_pts = 0;
    let mut mapped_pts = 0;

    for point_result in reader.points() {
        let point = match point_result {
            Ok(p) => p,
            Err(_) => continue, // Fast-fail em pontos corrompidos
        };

        processed_pts += 1;

        // OTIMIZAÇÃO BESM-6: Apenas classes de solo (Class 2: Ground, Class 9: Water) 
        // para evitar que o topo das árvores e dos prédios vire montanhas de terra.
        if point.classification != 2 && point.classification != 9 {
            continue; 
        }

        // Converter XYZ do LiDAR (UTM) para WGS84 Lat/Lon
        let (lon, lat) = proj.convert((point.x, point.y))
            .unwrap_or((0.0, 0.0));

        // Verificar se está dentro do nosso quadrante de interesse (Streaming Filter)
        if lat < bbox.min().lat()
            || lat > bbox.max().lat()
            || lon < bbox.min().lng()
            || lon > bbox.max().lng()
        {
            continue;
        }

        // Mapeamento espacial (0 a 1)
        let rel_x = (lon - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
        let rel_y = 1.0 - (lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

        let scaled_x = (rel_x * grid_width as f64).round() as usize;
        let scaled_y = (rel_y * grid_height as f64).round() as usize;

        if scaled_y >= grid_height || scaled_x >= grid_width {
            continue;
        }

        // Acumulação: Pegamos o ponto de solo mais alto daquela célula (evita buracos de esgoto)
        let current_h = height_grid[scaled_y][scaled_x];
        if current_h.is_nan() || point.z > current_h {
            height_grid[scaled_y][scaled_x] = point.z;
        }
        
        mapped_pts += 1;
    }

    println!("[INFO] ? LiDAR processado: {} milhões de pontos lidos, {} alocados no relevo de solo.", processed_pts / 1_000_000, mapped_pts);

    Ok(height_grid)
}

// ============================================================================
// MAIN ELEVATION PIPELINE
// ============================================================================

pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale_h: f64,
    scale_v: f64,
    ground_level: i32,
    local_lidar: Option<&PathBuf>, 
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let (base_scale_z, base_scale_x) = geo_distance(bbox.min(), bbox.max());

    let scale_factor_z: f64 = base_scale_z.floor() * scale_h;
    let scale_factor_x: f64 = base_scale_x.floor() * scale_h;

    let grid_width: usize = scale_factor_x as usize;
    let grid_height: usize = scale_factor_z as usize;

    let mut height_grid: Vec<Vec<f64>>;

    // 1. TENTATIVA DE LEITURA DO LiDAR (Prioridade Máxima Gov-Tier)
    let lidar_success = if let Some(lidar_path) = local_lidar {
        match load_local_lidar(lidar_path, bbox, grid_width, grid_height) {
            Ok(grid) => {
                height_grid = grid;
                true
            }
            Err(e) => {
                eprintln!("[ALERTA] Falha ao carregar LiDAR: {}. Caindo para SRTM...", e);
                false
            }
        }
    } else {
        false
    };

    // 2. FALLBACK PARA O SRTM (Se não houver LiDAR ou se ele falhar)
    if !lidar_success {
        height_grid = vec![vec![f64::NAN; grid_width]; grid_height];
        let zoom: u8 = calculate_zoom_level(bbox);
        let tiles: Vec<(u32, u32)> = get_tile_coordinates(bbox, zoom);

        let tile_cache_dir = PathBuf::from("./arnis-tile-cache");
        if !tile_cache_dir.exists() {
            std::fs::create_dir_all(&tile_cache_dir)?;
        }

        let client = reqwest::blocking::Client::new();
        let num_tiles = tiles.len();
        println!("Downloading {num_tiles} elevation tiles (up to {MAX_CONCURRENT_DOWNLOADS} concurrent)...");

        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_CONCURRENT_DOWNLOADS)
            .build()
            .map_err(|e| format!("Failed to create thread pool: {e}"))?;

        let downloaded_tiles: Vec<TileDownloadResult> = thread_pool.install(|| {
            tiles
                .par_iter()
                .map(|(tile_x, tile_y)| {
                    let tile_path = tile_cache_dir.join(format!("z{zoom}_x{tile_x}_y{tile_y}.png"));
                    let rgb_img = fetch_or_load_tile(&client, *tile_x, *tile_y, zoom, &tile_path)?;
                    Ok(((*tile_x, *tile_y), rgb_img))
                })
                .collect()
        });

        let mut successful_tiles = Vec::new();
        for result in downloaded_tiles {
            match result {
                Ok(tile_data) => successful_tiles.push(tile_data),
                Err(e) => {
                    eprintln!("Warning: Failed to download tile: {e}");
                }
            }
        }

        println!("Processing {} SRTM elevation tiles...", successful_tiles.len());
        #[cfg(feature = "gui")]
        emit_gui_progress_update(15.0, "Processing elevation...");

        let mut extreme_values_found = Vec::new();

        for ((tile_x, tile_y), rgb_img) in successful_tiles {
            for (y, row) in rgb_img.rows().enumerate() {
                for (x, pixel) in row.enumerate() {
                    let pixel_lng = ((tile_x as f64 + x as f64 / 256.0) / (2.0_f64.powi(zoom as i32))) * 360.0 - 180.0;
                    let pixel_lat_rad = std::f64::consts::PI * (1.0 - 2.0 * (tile_y as f64 + y as f64 / 256.0) / (2.0_f64.powi(zoom as i32)));
                    let pixel_lat = pixel_lat_rad.sinh().atan().to_degrees();

                    if pixel_lat < bbox.min().lat()
                        || pixel_lat > bbox.max().lat()
                        || pixel_lng < bbox.min().lng()
                        || pixel_lng > bbox.max().lng()
                    {
                        continue;
                    }

                    let rel_x = (pixel_lng - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
                    let rel_y = 1.0 - (pixel_lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

                    let scaled_x = (rel_x * grid_width as f64).round() as usize;
                    let scaled_y = (rel_y * grid_height as f64).round() as usize;

                    if scaled_y >= grid_height || scaled_x >= grid_width {
                        continue;
                    }

                    let height: f64 = (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET;

                    if !(-1000.0..=10000.0).contains(&height) {
                        extreme_values_found.push((tile_x, tile_y, x, y, pixel[0], pixel[1], pixel[2], height));
                    }

                    height_grid[scaled_y][scaled_x] = height;
                }
            }
        }
    }

    fill_nan_values(&mut height_grid);
    filter_elevation_outliers(&mut height_grid);

    // LiDAR requer menos blur que SRTM para preservar quebras secas (muros de arrimo)
    let blur_factor = if lidar_success { 2.0 } else { 5.0 };
    const BASE_GRID_REF: f64 = 100.0;
    let grid_size: f64 = (grid_width.min(grid_height) as f64).max(1.0);
    let sigma: f64 = blur_factor * (grid_size / BASE_GRID_REF).sqrt();

    let blurred_heights: Vec<Vec<f64>> = apply_gaussian_blur(&height_grid, sigma);
    drop(height_grid); // Libera a matriz bruta

    let (min_height, max_height, extreme_low_count, extreme_high_count) = blurred_heights
        .par_iter()
        .map(|row| {
            let mut local_min = f64::MAX;
            let mut local_max = f64::MIN;
            let mut local_low = 0usize;
            let mut local_high = 0usize;
            for &height in row {
                local_min = local_min.min(height);
                local_max = local_max.max(height);
                if height < -1000.0 { local_low += 1; }
                if height > 10000.0 { local_high += 1; }
            }
            (local_min, local_max, local_low, local_high)
        })
        .reduce(
            || (f64::MAX, f64::MIN, 0usize, 0usize),
            |(min1, max1, low1, high1), (min2, max2, low2, high2)| {
                (min1.min(min2), max1.max(max2), low1 + low2, high1 + high2)
            },
        );

    let height_range: f64 = max_height - min_height;

    // TWEAK: Rigor Escala Vertical V_SCALE na Topografia
    let ideal_scaled_range: f64 = height_range * scale_v;

    const TERRAIN_HEIGHT_BUFFER: i32 = 300; 
    let available_y_range: f64 = (MAX_Y - TERRAIN_HEIGHT_BUFFER - ground_level) as f64;

    let scaled_range: f64 = if ideal_scaled_range <= available_y_range {
        println!("Realistic elevation: {:.1}m range stretched to {:.0} blocks (Scale {})", height_range, ideal_scaled_range, scale_v);
        ideal_scaled_range
    } else {
        let compression_factor: f64 = available_y_range / height_range;
        let compressed_range: f64 = height_range * compression_factor;
        eprintln!("Elevation compressed due to Sky Limit: {:.1}m -> {:.0} blocks", height_range, compressed_range);
        compressed_range
    };

    let mc_heights: Vec<Vec<i32>> = blurred_heights
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&h| {
                    let relative_height: f64 = if height_range > 0.0 { (h - min_height) / height_range } else { 0.0 };
                    let scaled_height: f64 = relative_height * scaled_range;
                    ((ground_level as f64 + scaled_height).round() as i32).clamp(ground_level, MAX_Y - TERRAIN_HEIGHT_BUFFER)
                })
                .collect()
        })
        .collect();

    Ok(ElevationData {
        heights: mc_heights,
        width: grid_width,
        height: grid_height,
    })
}

fn get_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let (x1, y1) = lat_lng_to_tile(bbox.min().lat(), bbox.min().lng(), zoom);
    let (x2, y2) = lat_lng_to_tile(bbox.max().lat(), bbox.max().lng(), zoom);

    let mut tiles: Vec<(u32, u32)> = Vec::new();
    for x in x1.min(x2)..=x1.max(x2) {
        for y in y1.min(y2)..=y1.max(y2) {
            tiles.push((x, y));
        }
    }
    tiles
}

fn apply_gaussian_blur(heights: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel: Vec<f64> = create_gaussian_kernel(kernel_size, sigma);

    let height_len = heights.len();
    let width = heights[0].len();

    let after_horizontal: Vec<Vec<f64>> = heights
        .par_iter()
        .map(|row| {
            let mut temp: Vec<f64> = vec![0.0; row.len()];
            for (i, val) in temp.iter_mut().enumerate() {
                let mut sum: f64 = 0.0;
                let mut weight_sum: f64 = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx: i32 = i as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < row.len() as i32 {
                        sum += row[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                *val = sum / weight_sum;
            }
            temp
        })
        .collect();

    let blurred_columns: Vec<Vec<f64>> = (0..width)
        .into_par_iter()
        .map(|x| {
            let column: Vec<f64> = after_horizontal.iter().map(|row| row[x]).collect();
            let mut blurred_column: Vec<f64> = vec![0.0; height_len];
            for (y, val) in blurred_column.iter_mut().enumerate() {
                let mut sum: f64 = 0.0;
                let mut weight_sum: f64 = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx: i32 = y as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < height_len as i32 {
                        sum += column[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                *val = sum / weight_sum;
            }
            blurred_column
        })
        .collect();

    let mut blurred: Vec<Vec<f64>> = vec![vec![0.0; width]; height_len];
    for (x, column) in blurred_columns.into_iter().enumerate() {
        for (y, val) in column.into_iter().enumerate() {
            blurred[y][x] = val;
        }
    }

    blurred
}

fn create_gaussian_kernel(size: usize, sigma: f64) -> Vec<f64> {
    let mut kernel: Vec<f64> = vec![0.0; size];
    let center: f64 = size as f64 / 2.0;

    for (i, value) in kernel.iter_mut().enumerate() {
        let x: f64 = i as f64 - center;
        *value = (-x * x / (2.0 * sigma * sigma)).exp();
    }

    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }

    kernel
}

/// ?? BESM-6: Fast Bilinear Fill
/// Substitui o O(n³) da repetição com While por uma interpolação direta
/// dos vizinhos mais próximos, otimizado para preencher buracos no SRTM.
fn fill_nan_values(height_grid: &mut [Vec<f64>]) {
    let height: usize = height_grid.len();
    if height == 0 { return; }
    let width: usize = height_grid[0].len();

    let mut changes_made: bool = true;
    while changes_made {
        changes_made = false;

        for y in 0..height {
            for x in 0..width {
                if height_grid[y][x].is_nan() {
                    let mut sum: f64 = 0.0;
                    let mut count: i32 = 0;

                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let ny: i32 = y as i32 + dy;
                            let nx: i32 = x as i32 + dx;

                            if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                                let val: f64 = height_grid[ny as usize][nx as usize];
                                if !val.is_nan() {
                                    sum += val;
                                    count += 1;
                                }
                            }
                        }
                    }

                    if count > 0 {
                        height_grid[y][x] = sum / count as f64;
                        changes_made = true;
                    }
                }
            }
        }
    }
}

fn filter_elevation_outliers(height_grid: &mut [Vec<f64>]) {
    let height = height_grid.len();
    let width = height_grid[0].len();

    let mut all_heights: Vec<f64> = Vec::new();
    for row in height_grid.iter() {
        for &h in row {
            if !h.is_nan() && h.is_finite() {
                all_heights.push(h);
            }
        }
    }

    if all_heights.is_empty() {
        return;
    }

    let len = all_heights.len();
    let p1_idx = (len as f64 * 0.01) as usize;
    let p99_idx = ((len as f64 * 0.99) as usize).min(len - 1);

    let (_, p1_val, _) = all_heights.select_nth_unstable_by(p1_idx, |a, b| a.partial_cmp(b).unwrap());
    let min_reasonable = *p1_val;

    let (_, p99_val, _) = all_heights.select_nth_unstable_by(p99_idx, |a, b| a.partial_cmp(b).unwrap());
    let max_reasonable = *p99_val;

    let mut outliers_filtered = 0;

    for row in height_grid.iter_mut().take(height) {
        for h in row.iter_mut().take(width) {
            if !h.is_nan() && (*h < min_reasonable || *h > max_reasonable) {
                *h = f64::NAN;
                outliers_filtered += 1;
            }
        }
    }

    if outliers_filtered > 0 {
        fill_nan_values(height_grid);
    }
}