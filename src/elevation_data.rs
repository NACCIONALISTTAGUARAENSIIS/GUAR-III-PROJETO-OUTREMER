//! Elevação real do terreno (SRTM via AWS Terrarium, ou LiDAR local).
//!
//! Produz uma grade densa de cotas em blocos, com **exatamente** as dimensões
//! do `XZBBox` do mundo (1 célula = 1 coluna do Minecraft), consumida por
//! `ground::Ground`.
//!
//! # O que mudou (qualidade da geração)
//!
//! 1. **Dimensão exata.** A grade era dimensionada por `geo_distance × scale_h`,
//!    uma fórmula de distância diferente da usada para construir o `XZBBox`
//!    (ENU/elipsoide ancorado no Marco Zero). As duas divergiam por alguns
//!    blocos, deixando uma faixa sem dado (portanto plana) numa borda do
//!    mundo. Agora a grade nasce com `width/height` do próprio `XZBBox`.
//!
//! 2. **Mesma projeção que o resto do motor.** Cada célula é convertida para
//!    lat/lon pela inversa exata do `CoordTransformer` (a mesma transformação
//!    que posiciona ruas e prédios), e cada ponto LiDAR é projetado com a
//!    transformação direta — em vez de mapeamentos lineares em graus que só
//!    coincidiam aproximadamente com o ENU.
//!
//! 3. **Amostragem bilinear dos tiles.** Antes, cada pixel Terrarium (~4,8 m
//!    no zoom 15) era "jogado" na célula mais próxima; com células de 0,75 m,
//!    mais de 95% da grade ficava vazia (NaN) e era preenchida por dilatação
//!    iterativa 3×3 — degraus visíveis, depois disfarçados por um blur muito
//!    forte (σ crescia com o tamanho do mapa: ~22 blocos num mapa de 2 km).
//!    Agora cada célula interpola bilinearmente os 4 pixels vizinhos (também
//!    através de bordas de tile), e o blur residual é pequeno e constante.
//!
//! 4. **Sem cortar picos reais.** O filtro de outliers descartava tudo abaixo
//!    do percentil 1 e acima do percentil 99 — num bbox com um único morro,
//!    o topo dele (1% da área) era achatado. Agora só valores fisicamente
//!    impossíveis (`< -500 m` ou `> 9000 m`, como os voids do SRTM) e picos
//!    isolados de 1 célula (spikes) são removidos.
//!
//! 5. **Teto coerente com o escritor.** O antigo `MAX_Y = 4064` ("datapack")
//!    não existe no escritor de chunks, que trunca em 319; a compressão
//!    vertical usa agora `ground::TERRAIN_MAX_Y`.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::ground::TERRAIN_MAX_Y;
#[cfg(feature = "gui")]
use crate::progress::emit_gui_progress_update;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use image::Rgb;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const AWS_TERRARIUM_URL: &str =
    "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
const TERRARIUM_OFFSET: f64 = 32768.0;
const TILE_SIZE: u32 = 256;
const MIN_ZOOM: u8 = 10;
const MAX_ZOOM: u8 = 15;
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
const TILE_CACHE_MAX_AGE_DAYS: u64 = 7;

/// Suavização residual (σ em blocos). SRTM já chega bilinear e vem de dado de
/// 30 m: um σ pequeno só remove o serrilhado da quantização. LiDAR preserva
/// quebras secas (muros de arrimo, meios-fios) com σ ainda menor.
const BLUR_SIGMA_SRTM: f64 = 6.0;
const BLUR_SIGMA_LIDAR: f64 = 1.5;

/// Faixa fisicamente plausível para cotas em metros (voids do SRTM viram
/// valores como -32768 depois do decode Terrarium).
const MIN_PLAUSIBLE_M: f32 = -500.0;
const MAX_PLAUSIBLE_M: f32 = 9000.0;
/// Um pico isolado de 1 célula que destoa dos vizinhos por mais que isto é
/// ruído (ponto LiDAR mal classificado, pixel corrompido), não relevo.
const SPIKE_THRESHOLD_M: f32 = 25.0;

/// Grade densa de cotas em blocos (row-major, `index = row * width + col`),
/// relativa ao canto mínimo do `XZBBox` (col 0 = `min_x`, row 0 = `min_z`).
#[derive(Clone)]
pub struct ElevationData {
    heights: Arc<Vec<i32>>,
    width: usize,
    height: usize,
}

impl ElevationData {
    /// Monta a partir de linhas (todas com o mesmo comprimento). Linhas mais
    /// curtas são completadas com o último valor; mais longas, truncadas.
    #[cfg(test)]
    pub fn from_rows(rows: Vec<Vec<i32>>) -> Self {
        let height = rows.len();
        let width = rows.first().map(|r| r.len()).unwrap_or(0);
        let mut heights = Vec::with_capacity(width * height);
        for row in rows {
            let last = row.last().copied().unwrap_or(0);
            let mut row = row;
            row.resize(width, last);
            heights.extend(row);
        }
        Self {
            heights: Arc::new(heights),
            width,
            height,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Cota na célula `(col, row)`, com clamp para a borda mais próxima.
    pub fn get_clamped(&self, col: usize, row: usize) -> i32 {
        if self.width == 0 || self.height == 0 {
            return 0;
        }
        let col = col.min(self.width - 1);
        let row = row.min(self.height - 1);
        self.heights[row * self.width + col]
    }

    /// Referência compartilhada (sem cópia) para a grade — `Ground` a usa
    /// diretamente quando as dimensões batem com o bbox.
    pub fn shared_heights(&self) -> Arc<Vec<i32>> {
        Arc::clone(&self.heights)
    }
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

/// Coordenadas GLOBAIS de pixel (Web Mercator) no zoom dado — fracionárias.
#[inline]
fn lat_lng_to_global_pixel(lat: f64, lng: f64, zoom: u8) -> (f64, f64) {
    let n: f64 = 2.0_f64.powi(zoom as i32) * TILE_SIZE as f64;
    let lat_rad: f64 = lat.to_radians();
    let px = (lng + 180.0) / 360.0 * n;
    let py = (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    (px, py)
}

fn lat_lng_to_tile(lat: f64, lng: f64, zoom: u8) -> (u32, u32) {
    let (px, py) = lat_lng_to_global_pixel(lat, lng, zoom);
    (
        (px / TILE_SIZE as f64).floor() as u32,
        (py / TILE_SIZE as f64).floor() as u32,
    )
}

const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;

fn download_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<TileImage, String> {
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
) -> Result<TileImage, String> {
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
) -> Result<TileImage, String> {
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
// MOSAICO TERRARIUM + AMOSTRAGEM BILINEAR
// ============================================================================

/// Mosaico de tiles Terrarium já baixados, endereçado por pixel global.
struct TerrariumMosaic {
    zoom: u8,
    tiles: FxHashMap<(u32, u32), TileImage>,
}

impl TerrariumMosaic {
    #[inline]
    fn decode(pixel: &Rgb<u8>) -> f64 {
        (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET
    }

    /// Cota (m) do pixel global inteiro `(gx, gy)`, se o tile estiver no mosaico.
    #[inline]
    fn pixel(&self, gx: i64, gy: i64) -> Option<f64> {
        if gx < 0 || gy < 0 {
            return None;
        }
        let tile = (
            (gx / TILE_SIZE as i64) as u32,
            (gy / TILE_SIZE as i64) as u32,
        );
        let img = self.tiles.get(&tile)?;
        let px = (gx % TILE_SIZE as i64) as u32;
        let py = (gy % TILE_SIZE as i64) as u32;
        Some(Self::decode(img.get_pixel(px, py)))
    }

    /// Interpolação bilinear entre os 4 pixels vizinhos ao ponto (lat, lng).
    /// Pixels ausentes (tile não baixado) são excluídos e os pesos restantes
    /// renormalizados; `None` só quando nenhum dos 4 existe.
    fn sample(&self, lat: f64, lng: f64) -> Option<f64> {
        let (px, py) = lat_lng_to_global_pixel(lat, lng, self.zoom);
        // Centro do pixel em +0.5
        let fx = px - 0.5;
        let fy = py - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (x0, y0) = (x0 as i64, y0 as i64);

        let taps = [
            (x0, y0, (1.0 - tx) * (1.0 - ty)),
            (x0 + 1, y0, tx * (1.0 - ty)),
            (x0, y0 + 1, (1.0 - tx) * ty),
            (x0 + 1, y0 + 1, tx * ty),
        ];

        let mut sum = 0.0;
        let mut weight = 0.0;
        for (gx, gy, w) in taps {
            if w <= 0.0 {
                continue;
            }
            if let Some(h) = self.pixel(gx, gy) {
                sum += h * w;
                weight += w;
            }
        }
        if weight > 0.0 {
            Some(sum / weight)
        } else {
            None
        }
    }
}

// ============================================================================
// 🚨 LiDAR POINT CLOUD PROCESSOR (Government Tier) 🚨
// ============================================================================

/// Processa um arquivo LiDAR .las/.laz local, extraindo cotas com precisão
/// sub-métrica. Cada ponto de solo é projetado com a MESMA transformação
/// (`CoordTransformer::transform_point`) usada para o resto do mundo, e cai
/// na coluna exata `(x - min_x, z - min_z)` da grade.
fn load_local_lidar(
    lidar_path: &Path,
    bbox: &LLBBox,
    transformer: &CoordTransformer,
    xzbbox: &XZBBox,
    grid: &mut [f32],
    grid_width: usize,
) -> Result<(), String> {
    use las::{Read, Reader};
    use proj::Proj;

    println!(
        "[INFO] 🛰️ Iniciando scanner LiDAR de alta densidade no arquivo: {}",
        lidar_path.display()
    );

    let mut reader =
        Reader::from_path(lidar_path).map_err(|e| format!("Falha ao ler arquivo LiDAR: {}", e))?;

    // LiDAR no GDF usa SIRGAS 2000 (EPSG:31983). O BBox usa WGS84 (EPSG:4326).
    let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
        .ok()
        .ok_or("Falha ao inicializar PROJ para conversão LiDAR SIRGAS 2000")?;

    let mut processed_pts = 0u64;
    let mut mapped_pts = 0u64;

    for point_result in reader.points() {
        let point = match point_result {
            Ok(p) => p,
            Err(_) => continue, // Fast-fail em pontos corrompidos
        };

        processed_pts += 1;

        // Apenas classes de solo (Class 2: Ground, Class 9: Water) para evitar
        // que o topo das árvores e dos prédios vire montanhas de terra.
        if u8::from(point.classification) != 2 && u8::from(point.classification) != 9 {
            continue;
        }

        let (lon, lat) = match proj.convert((point.x, point.y)) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Ok(llpoint) = LLPoint::new(lat, lon) else {
            continue;
        };
        if !bbox.contains(&llpoint) {
            continue;
        }

        let xz = transformer.transform_point(llpoint);
        if !xzbbox.contains(&xz) {
            continue;
        }
        let col = (xz.x - xzbbox.min_x()) as usize;
        let row = (xz.z - xzbbox.min_z()) as usize;
        let idx = row * grid_width + col;

        // Acumulação: o ponto de solo mais alto da célula (evita buracos de esgoto)
        let current = grid[idx];
        if current.is_nan() || (point.z as f32) > current {
            grid[idx] = point.z as f32;
        }
        mapped_pts += 1;
    }

    println!(
        "[INFO] 📍 LiDAR processado: {} milhões de pontos lidos, {} alocados no relevo de solo.",
        processed_pts / 1_000_000,
        mapped_pts
    );

    if mapped_pts == 0 {
        return Err("nenhum ponto de solo do LiDAR caiu dentro do bbox".to_string());
    }

    Ok(())
}

// ============================================================================
// MAIN ELEVATION PIPELINE
// ============================================================================

pub fn fetch_elevation_data(
    bbox: &LLBBox,
    xzbbox: &XZBBox,
    scale_v: f64,
    ground_level: i32,
    local_lidar: Option<&PathBuf>,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, 1.0)?;

    let grid_width = (xzbbox.max_x() as i64 - xzbbox.min_x() as i64 + 1).max(1) as usize;
    let grid_height = (xzbbox.max_z() as i64 - xzbbox.min_z() as i64 + 1).max(1) as usize;

    let mut raw: Vec<f32> = vec![f32::NAN; grid_width * grid_height];

    // 1. TENTATIVA DE LEITURA DO LiDAR (Prioridade Máxima Gov-Tier)
    let lidar_success = if let Some(lidar_path) = local_lidar {
        match load_local_lidar(lidar_path, bbox, &transformer, xzbbox, &mut raw, grid_width) {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "[ALERTA] Falha ao carregar LiDAR: {}. Caindo para SRTM...",
                    e
                );
                raw.fill(f32::NAN);
                false
            }
        }
    } else {
        false
    };

    // 2. FALLBACK PARA O SRTM (Se não houver LiDAR ou se ele falhar)
    if !lidar_success {
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

        let mut mosaic = TerrariumMosaic {
            zoom,
            tiles: FxHashMap::default(),
        };
        for result in downloaded_tiles {
            match result {
                Ok((key, img)) => {
                    mosaic.tiles.insert(key, img);
                }
                Err(e) => eprintln!("Warning: Failed to download tile: {e}"),
            }
        }

        if mosaic.tiles.is_empty() {
            return Err("nenhum tile de elevação pôde ser baixado".into());
        }

        println!(
            "Processing {} SRTM elevation tiles (bilinear, {}x{} cells)...",
            mosaic.tiles.len(),
            grid_width,
            grid_height
        );
        #[cfg(feature = "gui")]
        emit_gui_progress_update(15.0, "Processing elevation...");

        let min_x = xzbbox.min_x();
        let min_z = xzbbox.min_z();
        raw.par_chunks_mut(grid_width)
            .enumerate()
            .for_each(|(row, cells)| {
                let z = min_z + row as i32;
                for (col, cell) in cells.iter_mut().enumerate() {
                    let x = min_x + col as i32;
                    if let Ok(ll) = transformer.inverse_transform(XZPoint::new(x, z)) {
                        if let Some(h) = mosaic.sample(ll.lat(), ll.lng()) {
                            *cell = h as f32;
                        }
                    }
                }
            });
    }

    let sigma = if lidar_success {
        BLUR_SIGMA_LIDAR
    } else {
        BLUR_SIGMA_SRTM
    };

    let heights = finalize_grid(raw, grid_width, grid_height, sigma, scale_v, ground_level)?;

    Ok(ElevationData {
        heights: Arc::new(heights),
        width: grid_width,
        height: grid_height,
    })
}

/// Etapas puras (testáveis sem rede) depois da amostragem: saneamento,
/// preenchimento de buracos, suavização e quantização vertical em blocos.
fn finalize_grid(
    mut raw: Vec<f32>,
    width: usize,
    height: usize,
    sigma: f64,
    scale_v: f64,
    ground_level: i32,
) -> Result<Vec<i32>, String> {
    sanitize_implausible(&mut raw);
    remove_spikes(&mut raw, width, height);
    if !fill_nan_nearest(&mut raw, width, height) {
        return Err("grade de elevação sem nenhuma célula válida".to_string());
    }
    let blurred = gaussian_blur(&raw, width, height, sigma);
    drop(raw);
    Ok(quantize_to_blocks(&blurred, scale_v, ground_level))
}

fn sanitize_implausible(grid: &mut [f32]) {
    grid.par_iter_mut().for_each(|h| {
        if !h.is_finite() || *h < MIN_PLAUSIBLE_M || *h > MAX_PLAUSIBLE_M {
            *h = f32::NAN;
        }
    });
}

/// Remove picos isolados de 1 célula: valor que destoa da MEDIANA dos 8
/// vizinhos válidos por mais de `SPIKE_THRESHOLD_M`. Relevo real (mesmo um
/// muro de arrimo) tem continuidade lateral; um spike não.
fn remove_spikes(grid: &mut [f32], width: usize, height: usize) {
    if width < 3 || height < 3 {
        return;
    }
    let snapshot = grid.to_vec();
    grid.par_chunks_mut(width)
        .enumerate()
        .for_each(|(row, cells)| {
            if row == 0 || row == height - 1 {
                return;
            }
            for col in 1..width - 1 {
                let h = cells[col];
                if h.is_nan() {
                    continue;
                }
                let mut neighbours: Vec<f32> = Vec::with_capacity(8);
                for dr in [-1i64, 0, 1] {
                    for dc in [-1i64, 0, 1] {
                        if dr == 0 && dc == 0 {
                            continue;
                        }
                        let v = snapshot
                            [(row as i64 + dr) as usize * width + (col as i64 + dc) as usize];
                        if !v.is_nan() {
                            neighbours.push(v);
                        }
                    }
                }
                if neighbours.len() < 4 {
                    continue;
                }
                let mid = neighbours.len() / 2;
                let median = *neighbours
                    .select_nth_unstable_by(mid, |a, b| a.total_cmp(b))
                    .1;
                if (h - median).abs() > SPIKE_THRESHOLD_M {
                    cells[col] = f32::NAN;
                }
            }
        });
}

/// Preenche células NaN com o valor da célula válida mais próxima (BFS
/// multi-fonte, O(N)). Retorna `false` se não havia nenhuma célula válida.
fn fill_nan_nearest(grid: &mut [f32], width: usize, height: usize) -> bool {
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, h) in grid.iter().enumerate() {
        if !h.is_nan() {
            queue.push_back(i);
        }
    }
    if queue.is_empty() {
        return false;
    }
    if queue.len() == grid.len() {
        return true;
    }

    while let Some(i) = queue.pop_front() {
        let value = grid[i];
        let row = i / width;
        let col = i % width;
        let mut visit = |j: usize| {
            if grid[j].is_nan() {
                grid[j] = value;
                queue.push_back(j);
            }
        };
        if col > 0 {
            visit(i - 1);
        }
        if col + 1 < width {
            visit(i + 1);
        }
        if row > 0 {
            visit(i - width);
        }
        if row + 1 < height {
            visit(i + width);
        }
    }
    true
}

fn create_gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (sigma * 3.0).ceil().max(1.0) as usize;
    let size = radius * 2 + 1;
    let mut kernel: Vec<f64> = vec![0.0; size];
    for (i, value) in kernel.iter_mut().enumerate() {
        let x: f64 = i as f64 - radius as f64;
        *value = (-x * x / (2.0 * sigma * sigma)).exp();
    }
    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    kernel
}

/// Blur gaussiano separável com renormalização nas bordas (sem escurecer
/// as margens). Entrada sem NaN.
fn gaussian_blur(grid: &[f32], width: usize, height: usize, sigma: f64) -> Vec<f32> {
    if sigma <= 0.0 || width == 0 || height == 0 {
        return grid.to_vec();
    }
    let kernel = create_gaussian_kernel(sigma);
    let radius = kernel.len() / 2;

    // Passo horizontal
    let mut horizontal = vec![0.0f32; grid.len()];
    horizontal
        .par_chunks_mut(width)
        .zip(grid.par_chunks(width))
        .for_each(|(out_row, in_row)| {
            for (i, out) in out_row.iter_mut().enumerate() {
                let mut sum = 0.0;
                let mut weight = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx = i as i64 + j as i64 - radius as i64;
                    if idx >= 0 && (idx as usize) < width {
                        sum += in_row[idx as usize] as f64 * k;
                        weight += k;
                    }
                }
                *out = (sum / weight) as f32;
            }
        });

    // Passo vertical (por linha de saída, lendo `radius` linhas acima/abaixo)
    let mut out = vec![0.0f32; grid.len()];
    out.par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            for (x, cell) in out_row.iter_mut().enumerate() {
                let mut sum = 0.0;
                let mut weight = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx = y as i64 + j as i64 - radius as i64;
                    if idx >= 0 && (idx as usize) < height {
                        sum += horizontal[idx as usize * width + x] as f64 * k;
                        weight += k;
                    }
                }
                *cell = (sum / weight) as f32;
            }
        });
    out
}

/// Converte metros em blocos: o ponto mais baixo do bbox vira `ground_level`,
/// o relevo é esticado por `scale_v` e, se não couber sob `TERRAIN_MAX_Y`,
/// comprimido proporcionalmente (com aviso).
fn quantize_to_blocks(grid: &[f32], scale_v: f64, ground_level: i32) -> Vec<i32> {
    let (min_height, max_height) = grid
        .par_iter()
        .fold(
            || (f32::MAX, f32::MIN),
            |(lo, hi), &h| (lo.min(h), hi.max(h)),
        )
        .reduce(
            || (f32::MAX, f32::MIN),
            |(lo1, hi1), (lo2, hi2)| (lo1.min(lo2), hi1.max(hi2)),
        );

    let height_range = (max_height - min_height).max(0.0) as f64;
    let ideal_scaled_range = height_range * scale_v;
    let available_y_range = (TERRAIN_MAX_Y - ground_level).max(0) as f64;

    let scaled_range = if ideal_scaled_range <= available_y_range {
        println!(
            "Realistic elevation: {:.1}m range stretched to {:.0} blocks (Scale {})",
            height_range, ideal_scaled_range, scale_v
        );
        ideal_scaled_range
    } else {
        eprintln!(
            "Elevation compressed due to world height limit: {:.1}m -> {:.0} blocks",
            height_range, available_y_range
        );
        available_y_range
    };

    grid.par_iter()
        .map(|&h| {
            let relative = if height_range > 0.0 {
                (h as f64 - min_height as f64) / height_range
            } else {
                0.0
            };
            ((ground_level as f64 + relative * scaled_range).round() as i32)
                .clamp(ground_level, TERRAIN_MAX_Y)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(h: f64) -> Rgb<u8> {
        let v = h + TERRARIUM_OFFSET;
        let r = (v / 256.0).floor();
        let g = (v - r * 256.0).floor();
        let b = ((v - r * 256.0 - g) * 256.0).round().min(255.0);
        Rgb([r as u8, g as u8, b as u8])
    }

    #[test]
    fn terrarium_decode_roundtrip() {
        for h in [-10.0, 0.0, 1050.25, 1172.5] {
            let decoded = TerrariumMosaic::decode(&encode(h));
            assert!((decoded - h).abs() < 0.01, "{h} -> {decoded}");
        }
    }

    /// Dois tiles lado a lado com um gradiente contínuo em X: a amostragem
    /// bilinear tem que ser contínua através da borda entre eles.
    #[test]
    fn bilinear_sampling_is_continuous_across_tile_borders() {
        let zoom = 15u8;
        let mut tiles = FxHashMap::default();
        for tx in [100u32, 101u32] {
            let mut img: TileImage = image::ImageBuffer::new(TILE_SIZE, TILE_SIZE);
            for py in 0..TILE_SIZE {
                for px in 0..TILE_SIZE {
                    let gx = tx * TILE_SIZE + px;
                    img.put_pixel(px, py, encode(gx as f64)); // altura = pixel global X
                }
            }
            tiles.insert((tx, 200u32), img);
        }
        let mosaic = TerrariumMosaic { zoom, tiles };

        // Pixel global (gx, gy) com gy no tile 200. Centro do pixel = gx + 0.5.
        let gy = 200.0 * TILE_SIZE as f64 + 10.5;
        let n = 2.0_f64.powi(zoom as i32) * TILE_SIZE as f64;
        let sample_at = |gx: f64| {
            let lng = gx / n * 360.0 - 180.0;
            let lat = ((std::f64::consts::PI * (1.0 - 2.0 * gy / n)).sinh())
                .atan()
                .to_degrees();
            mosaic.sample(lat, lng).expect("sample")
        };

        let border = 101.0 * TILE_SIZE as f64; // primeiro pixel do tile 101
        let a = sample_at(border - 0.5); // centro do último pixel do tile 100
        let b = sample_at(border); // exatamente entre os dois
        let c = sample_at(border + 0.5); // centro do primeiro pixel do tile 101
        assert!((a - (border - 1.0)).abs() < 0.05, "{a}");
        assert!((b - (border - 0.5)).abs() < 0.05, "{b}");
        assert!((c - border).abs() < 0.05, "{c}");
    }

    #[test]
    fn missing_tile_falls_back_to_available_taps() {
        let zoom = 15u8;
        let mut tiles = FxHashMap::default();
        let mut img: TileImage = image::ImageBuffer::new(TILE_SIZE, TILE_SIZE);
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                img.put_pixel(px, py, encode(500.0));
            }
        }
        tiles.insert((100u32, 200u32), img);
        let mosaic = TerrariumMosaic { zoom, tiles };
        // Último pixel do tile 100: o vizinho à direita está no tile 101 (ausente).
        let n = 2.0_f64.powi(zoom as i32) * TILE_SIZE as f64;
        let gx = 101.0 * TILE_SIZE as f64 - 0.2;
        let gy = 200.0 * TILE_SIZE as f64 + 10.5;
        let lng = gx / n * 360.0 - 180.0;
        let lat = ((std::f64::consts::PI * (1.0 - 2.0 * gy / n)).sinh())
            .atan()
            .to_degrees();
        assert!((mosaic.sample(lat, lng).unwrap() - 500.0).abs() < 0.05);
        // Longe de qualquer tile: nada.
        assert!(mosaic.sample(0.0, 0.0).is_none());
    }

    #[test]
    fn nearest_fill_uses_closest_valid_cell() {
        let mut grid = vec![
            1.0,
            f32::NAN,
            f32::NAN,
            9.0, //
            1.0,
            f32::NAN,
            f32::NAN,
            9.0, //
        ];
        assert!(fill_nan_nearest(&mut grid, 4, 2));
        assert_eq!(grid[1], 1.0);
        assert_eq!(grid[2], 9.0);
        assert!(grid.iter().all(|h| !h.is_nan()));
        let mut all_nan = vec![f32::NAN; 4];
        assert!(!fill_nan_nearest(&mut all_nan, 2, 2));
    }

    #[test]
    fn spike_filter_removes_isolated_peak_but_keeps_ridge() {
        let w = 5;
        let h = 5;
        let mut grid = vec![100.0f32; w * h];
        grid[2 * w + 2] = 400.0; // spike
        remove_spikes(&mut grid, w, h);
        assert!(grid[2 * w + 2].is_nan());

        // Uma crista real (linha inteira mais alta) sobrevive: a mediana dos
        // vizinhos de cada célula da crista inclui a própria crista.
        // (uma crista de 1 célula = 0,75 m de largura com 30 m de queda seca
        // não é relevo; uma com encostas é.)
        let mut ridge = vec![100.0f32; w * h];
        for col in 0..w {
            ridge[2 * w + col] = 160.0;
            ridge[w + col] = 145.0;
            ridge[3 * w + col] = 145.0;
        }
        remove_spikes(&mut ridge, w, h);
        assert!(ridge.iter().all(|v| !v.is_nan()));
    }

    #[test]
    fn implausible_values_become_nan() {
        let mut grid = vec![10.0, -32768.0, 12000.0, f32::INFINITY];
        sanitize_implausible(&mut grid);
        assert_eq!(grid[0], 10.0);
        assert!(grid[1..].iter().all(|v| v.is_nan()));
    }

    #[test]
    fn blur_preserves_constant_field_and_kernel_is_normalized() {
        let k = create_gaussian_kernel(2.0);
        assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        let grid = vec![42.0f32; 30 * 20];
        let out = gaussian_blur(&grid, 30, 20, 2.0);
        assert!(out.iter().all(|v| (v - 42.0).abs() < 1e-4));
    }

    #[test]
    fn quantization_anchors_minimum_at_ground_level_and_applies_scale_v() {
        let grid = vec![1000.0f32, 1010.0, 1020.0];
        let q = quantize_to_blocks(&grid, 1.15, -62);
        // 20 m × 1,15 = 23 blocos de relevo; o meio (11,5) arredonda para -51.
        assert_eq!(q, vec![-62, -51, -62 + 23]);
    }

    #[test]
    fn quantization_compresses_relief_that_would_exceed_the_world_ceiling() {
        let grid = vec![0.0f32, 2000.0];
        let q = quantize_to_blocks(&grid, 1.15, -62);
        assert_eq!(q[0], -62);
        assert_eq!(q[1], TERRAIN_MAX_Y);
    }

    #[test]
    fn elevation_data_clamps_out_of_range_indices() {
        let e = ElevationData::from_rows(vec![vec![1, 2], vec![3, 4]]);
        assert_eq!(e.get_clamped(0, 0), 1);
        assert_eq!(e.get_clamped(7, 0), 2);
        assert_eq!(e.get_clamped(0, 9), 3);
        assert_eq!(e.get_clamped(9, 9), 4);
    }
}
