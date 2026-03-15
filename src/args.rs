use crate::coordinate_system::geographic::LLBBox;
use clap::{ArgAction, Parser, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;
use std::process::Command;

/// Enum for explicit DEM source selection
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DemSource {
    AwsSrtm,
    Cop30,
    Topodata,
}

/// Enum for explicit Downloader selection
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum Downloader {
    Requests,
    Curl,
    Wget,
}

impl Downloader {
    pub fn as_str(&self) -> &'static str {
        match self {
            Downloader::Requests => "requests",
            Downloader::Curl => "curl",
            Downloader::Wget => "wget",
        }
    }
}

/// Enum for rendering priority layers
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum LayerPriority {
    Shp,
    Lidar,
    Wfs,
    Geojson,
    Osm,
}

/// Command-line arguments parser for the Bras�lia Digital Twin Engine
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// Bounding box of the area (min_lat,min_lng,max_lat,max_lng) (required)
    #[arg(long, allow_hyphen_values = true, value_parser = LLBBox::from_str)]
    pub bbox: LLBBox,

    /// JSON file containing OSM data (optional fallback)
    #[arg(long, group = "location")]
    pub file: Option<String>,

    /// JSON file to save OSM data to (optional)
    #[arg(long, group = "location")]
    pub save_json_file: Option<String>,

    /// Output directory for the generated world (required for Java, optional for Bedrock).
    #[arg(long = "output-dir", alias = "path")]
    pub path: Option<PathBuf>,

    /// Generate a Bedrock Edition world (.mcworld) instead of Java Edition
    #[arg(long)]
    pub bedrock: bool,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, value_enum, default_value_t = Downloader::Requests)]
    pub downloader: Downloader,

    /// Set number of CPU threads to use (0 = auto). Default: 0
    #[arg(long, default_value_t = 0)]
    pub threads: usize,

    /// Offline mode. Disables all HTTP requests (OSM/AWS). Forces engine to use local data.
    #[arg(long, default_value_t = false)]
    pub offline: bool,

    // ==========================================================
    // ESCALA H�BRIDA OFICIAL (G�meo Digital DF)
    // ==========================================================
    
    /// Horizontal scale (X, Z) to use, in blocks per meter. Default is 1.33 for proper street proportions.
    #[arg(long, default_value_t = 1.33)]
    pub scale_h: f64,

    /// Vertical scale (Y) to use, in blocks per meter. Default is 1.15 for proper ceiling heights.
    #[arg(long, default_value_t = 1.15)]
    pub scale_v: f64,

    /// TWEAK COMPATIBILIDADE: Mantido para retrocompatibilidade. Em nova arquitetura, use --scale-h.
    #[arg(long, hide = true)]
    pub scale: Option<f64>, 

    // ==========================================================
    // INTEGRA��O DE DADOS GOVERNAMENTAIS (GDF / CODEPLAN)
    // ==========================================================

    /// Path to local Shapefiles (.shp) from Geoportal DF (buildings, landuse, etc.)
    #[arg(long)]
    pub local_shp: Option<PathBuf>,

    /// Path to local GeoJSON file containing vector data
    #[arg(long)]
    pub local_geojson: Option<PathBuf>,

    /// Path to local LiDAR Point Clouds (.las or .laz) for hyper-accurate elevation
    #[arg(long)]
    pub local_lidar: Option<PathBuf>,

    /// API Endpoint for WFS services (e.g. CAESB or SISDIA)
    #[arg(long)]
    pub wfs_endpoint: Option<String>,

    /// EPSG code for local data reprojection (Government Tier). Defaults to 31983 (SIRGAS 2000 / UTM zone 23S for Bras�lia).
    #[arg(long, default_value_t = 31983)]
    pub epsg: u32,

    /// Source of Digital Elevation Model (DEM)
    #[arg(long, value_enum, default_value_t = DemSource::AwsSrtm)]
    pub dem: DemSource,

    /// Path to local DEM GeoTIFF files (Overrides API calls if provided)
    #[arg(long)]
    pub local_dem: Option<PathBuf>,

    /// Directory to cache downloaded DEM, OSM, and WFS data to prevent redundant API calls.
    #[arg(long, default_value = "./arnis_cache")]
    pub cache_dir: PathBuf,

    /// Maximum allowed bounding box area in square kilometers. Prevents accidental continental downloads.
    #[arg(long, default_value_t = 10000.0)]
    pub max_area_km2: f64,

    /// Safety limit: Maximum number of DEM tiles allowed to download. 
    #[arg(long, default_value_t = 200)]
    pub max_dem_tiles: usize,

    /// Safety limit: Maximum number of parsed OSM features before aborting.
    #[arg(long, default_value_t = 5_000_000)]
    pub max_osm_features: usize,

    /// Determines which layer overrides others when overlapping
    #[arg(long, value_enum, num_args = 1.., default_values_t = vec![LayerPriority::Shp, LayerPriority::Lidar, LayerPriority::Wfs, LayerPriority::Geojson, LayerPriority::Osm])]
    pub priority_layer: Vec<LayerPriority>,

    /// Enable underground WFS features (CAESB Sewage, Neoenergia conduits)
    #[arg(long, default_value_t = false)]
    pub enable_underground_wfs: bool,

    // ==========================================================
    // INTEGRA��O DE ALTA PERFORMANCE & G�MEO DIGITAL (BESM-6)
    // ==========================================================

    /// PostgreSQL/PostGIS connection URL for direct spatial queries (e.g., postgres://user:pass@localhost:5432/gis)
    #[arg(long)]
    pub postgis_url: Option<String>,

    /// Path to local GeoPackage (.gpkg) file, the modern OGC standard replacing shapefiles
    #[arg(long)]
    pub local_gpkg: Option<PathBuf>,

    /// Path to local OpenStreetMap PBF file (.osm.pbf) for ultra-fast local parsing without API limits
    #[arg(long)]
    pub local_pbf: Option<PathBuf>,

    /// Mapbox Vector Tiles (MVT) endpoint URL for streaming infrastructure data
    #[arg(long)]
    pub mvt_endpoint: Option<String>,

    /// Path to local CityGML file (.gml/.xml) for high-fidelity LOD1-LOD4 3D city models
    #[arg(long)]
    pub local_citygml: Option<PathBuf>,

    /// Path to local Photogrammetry Meshes (.obj, .gltf) directory for accurate monument representation
    #[arg(long)]
    pub local_mesh: Option<PathBuf>,

    // ==========================================================
    // CONFIGURA��ES BASE
    // ==========================================================

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62)]
    pub ground_level: i32,

    /// Enable terrain (optional)
    #[arg(long)]
    pub terrain: bool,

    /// Enable interior generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub interior: bool,

    /// Enable roof generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub roof: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false)]
    pub fillground: bool,

    /// Enable city ground generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub city_boundaries: bool,

    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Spawn point latitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lat: Option<f64>,

    /// Spawn point longitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lng: Option<f64>,
}

/// Helper function to calculate Haversine distance in kilometers
fn haversine_distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371.0; // Earth radius in km
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();

    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

/// Helper function to retrieve strictly physical CPU cores to avoid logical hyper-threading IO starvation
fn get_physical_cores() -> usize {
    if cfg!(target_os = "linux") {
        if let Ok(output) = Command::new("lscpu").arg("-p=CORE").output() {
            let s = String::from_utf8_lossy(&output.stdout);
            let mut unique_cores = std::collections::HashSet::new();
            for line in s.lines() {
                if !line.starts_with('#') {
                    if let Ok(core_id) = line.trim().parse::<usize>() {
                        unique_cores.insert(core_id);
                    }
                }
            }
            if !unique_cores.is_empty() {
                return unique_cores.len();
            }
        }
    }
    
    // Fallback if lscpu fails or non-Linux system: halving logical cores is a standard assumption for SMT
    let logical_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (logical_cores / 2).max(1)
}

/// Validates CLI arguments after parsing.
pub fn validate_args(args: &mut Args) -> Result<(), String> {
    // ?? BESM-6: Retrocompatibilidade de Escala resolvida fisicamente
    if let Some(legacy_scale) = args.scale {
        args.scale_h = legacy_scale;
        args.scale_v = legacy_scale;
    }

    if args.bedrock {
        if let Some(ref path) = args.path {
            if !path.exists() {
                return Err(format!("Path does not exist: {}", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else {
        match &args.path {
            None => {
                return Err(
                    "The --output-dir argument is required for Java Edition. Provide the directory where the world should be created. Use --bedrock for Bedrock Edition output."
                        .to_string(),
                );
            }
            Some(ref path) => {
                if !path.exists() {
                    return Err(format!("Path does not exist: {}", path.display()));
                }
                if !path.is_dir() {
                    return Err(format!("Path is not a directory: {}", path.display()));
                }
            }
        }
    }

    // ?? BESM-6 Tweak: Validao Antidegenerao da BBox
    if args.bbox.max().lat() <= args.bbox.min().lat() || args.bbox.max().lng() <= args.bbox.min().lng() {
        return Err("Invalid bounding box: Max coordinates must be strictly greater than Min coordinates.".to_string());
    }

    // ?? BESM-6 Tweak: Geodesic Area Calculation (Haversine Absoluto)
    let min_lat = args.bbox.min().lat();
    let min_lng = args.bbox.min().lng();
    let max_lat = args.bbox.max().lat();
    let max_lng = args.bbox.max().lng();

    let width_km = haversine_distance_km(min_lat, min_lng, min_lat, max_lng);
    let height_km = haversine_distance_km(min_lat, min_lng, max_lat, min_lng);
    let area_km2 = width_km * height_km;

    if area_km2 > args.max_area_km2 {
        return Err(format!(
            "Bounding box area ({:.2} km�) exceeds the maximum allowed limit ({:.2} km�). Decrease the bbox or bypass with --max-area-km2.",
            area_km2, args.max_area_km2
        ));
    }

    // ?? BESM-6 Tweak: Prote��o Log�stica (Pre-flight IO Check).
    // Estimativa braba: ~100MB por km2 na escala 1.33. 
    // Se a estimativa ultrapassar os 180GB estipulados para a Oracle, trava.
    let estimated_weight_gb = (area_km2 * 100.0) / 1024.0;
    if estimated_weight_gb > 180.0 {
        return Err(format!(
            "Storage Limit Exceeded: The requested area ({:.2} km�) is estimated to consume {:.2} GB. The Oracle server is hard-capped at 180 GB.",
            area_km2, estimated_weight_gb
        ));
    }

    // Cache directory only created if online
    if !args.offline && !args.cache_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&args.cache_dir) {
            return Err(format!("Failed to create cache directory at {}: {}", args.cache_dir.display(), e));
        }
    }

    // ?? BESM-6 Tweak: Safe thread limit (N�cleos F�sicos Estritos)
    let max_safe_threads = get_physical_cores();
    
    // Se o usu�rio passou 0, atribu�mos o safe limit f�sico automaticamente.
    if args.threads == 0 {
        args.threads = max_safe_threads;
    } else if args.threads > max_safe_threads {
        return Err(format!(
            "Requested threads ({}) exceed the physical core limit for this machine ({}). Lower the thread count to prevent IO saturation and Zlib blocking.",
            args.threads, max_safe_threads
        ));
    }

    // Offline Mode Validations
    if args.offline {
        let has_local_vector_source = args.file.is_some() 
            || args.local_shp.is_some() 
            || args.local_gpkg.is_some() 
            || args.local_pbf.is_some() 
            || args.local_citygml.is_some()
            || args.postgis_url.is_some();

        if !has_local_vector_source {
            return Err("Offline mode requires a local vector source (--file, --local-shp, --local-gpkg, --local-pbf, --local-citygml, or --postgis-url). Cannot fetch from API.".to_string());
        }
        if args.terrain && args.local_lidar.is_none() && args.local_dem.is_none() {
            return Err("Offline terrain generation requires --local-lidar or --local-dem.".to_string());
        }
        if args.enable_underground_wfs || args.mvt_endpoint.is_some() {
            return Err("Cannot enable WFS or MVT streaming endpoints while running in --offline mode.".to_string());
        }
    }

    // Validate Shapefile integration path
    if let Some(ref shp_path) = args.local_shp {
        if !shp_path.exists() || !shp_path.is_dir() {
            return Err(format!("Shapefile directory does not exist or is not a directory: {}", shp_path.display()));
        }
    }

    // Validate GeoJSON integration path
    if let Some(ref geojson_path) = args.local_geojson {
        if !geojson_path.exists() || !geojson_path.is_file() {
            return Err(format!("GeoJSON file does not exist or is not a file: {}", geojson_path.display()));
        }
    }

    // Validate GeoPackage integration path
    if let Some(ref gpkg_path) = args.local_gpkg {
        if !gpkg_path.exists() || !gpkg_path.is_file() {
            return Err(format!("GeoPackage file does not exist or is not a file: {}", gpkg_path.display()));
        }
        let ext = gpkg_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.to_lowercase() != "gpkg" {
            return Err(format!("GeoPackage source must be a .gpkg file. Found: {}", gpkg_path.display()));
        }
    }

    // Validate PBF integration path
    if let Some(ref pbf_path) = args.local_pbf {
        if !pbf_path.exists() || !pbf_path.is_file() {
            return Err(format!("PBF file does not exist or is not a file: {}", pbf_path.display()));
        }
        let ext = pbf_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.to_lowercase() != "pbf" {
            return Err(format!("PBF source must be a .pbf file. Found: {}", pbf_path.display()));
        }
    }

    // Validate CityGML integration path
    if let Some(ref citygml_path) = args.local_citygml {
        if !citygml_path.exists() || !citygml_path.is_file() {
            return Err(format!("CityGML file does not exist or is not a file: {}", citygml_path.display()));
        }
        let ext = citygml_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.to_lowercase() != "gml" && ext.to_lowercase() != "xml" {
            return Err(format!("CityGML source must be a .gml or .xml file. Found: {}", citygml_path.display()));
        }
    }

    // Validate Photogrammetry Mesh integration path
    if let Some(ref mesh_path) = args.local_mesh {
        if !mesh_path.exists() {
            return Err(format!("Photogrammetry mesh directory or file does not exist: {}", mesh_path.display()));
        }
    }

    // Validate LiDAR integration path
    if let Some(ref lidar_path) = args.local_lidar {
        if !lidar_path.exists() || !lidar_path.is_file() {
            return Err(format!("LiDAR source file does not exist: {}", lidar_path.display()));
        }
        let ext = lidar_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.to_lowercase() != "las" && ext.to_lowercase() != "laz" {
            return Err(format!("LiDAR source must be a .las or .laz file. Found: {}", lidar_path.display()));
        }
    }

    // Validate Local DEM integration path
    if let Some(ref dem_path) = args.local_dem {
        if !dem_path.exists() || !dem_path.is_file() {
            return Err(format!("Local DEM file does not exist: {}", dem_path.display()));
        }
        let ext = dem_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.to_lowercase() != "tif" && ext.to_lowercase() != "tiff" {
            return Err(format!("Local DEM source must be a GeoTIFF (.tif/.tiff) file. Found: {}", dem_path.display()));
        }
    }

    // Validate WFS Endpoint dependency
    if args.enable_underground_wfs && args.wfs_endpoint.is_none() {
        return Err("You must provide a --wfs-endpoint when --enable-underground-wfs is true.".to_string());
    }

    // Validate spawn point: both or neither must be provided
    match (args.spawn_lat, args.spawn_lng) {
        (Some(_), None) | (None, Some(_)) => {
            return Err("Both --spawn-lat and --spawn-lng must be provided together.".to_string());
        }
        (Some(lat), Some(lng)) => {
            use crate::coordinate_system::geographic::LLPoint;
            let llpoint =
                LLPoint::new(lat, lng).map_err(|e| format!("Invalid spawn coordinates: {e}"))?;

            if !args.bbox.contains(&llpoint) {
                return Err(
                    "Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box."
                        .to_string(),
                );
            }
        }
        _ => {}
    }

    Ok(())
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flags() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--terrain",
            "--debug",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.debug);
        assert!(args.terrain);

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.debug);
        assert!(!args.terrain);
        assert!(!args.bedrock);
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.city_boundaries);
    }

    #[test]
    fn test_bool_flags_can_be_disabled() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior=false",
            "--roof=false",
            "--city-boundaries=false",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.interior);
        assert!(!args.roof);
        assert!(!args.city_boundaries);

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior",
            "--roof",
            "--city-boundaries",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.city_boundaries);
    }

    #[test]
    fn test_bedrock_flag() {
        let cmd = ["arnis", "--bedrock", "--bbox", "1,2,3,4"];
        let mut args = Args::parse_from(cmd.iter());
        assert!(args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&mut args).is_ok());
    }

    #[test]
    fn test_java_requires_path() {
        let cmd = ["arnis", "--bbox", "1,2,3,4"];
        let mut args = Args::parse_from(cmd.iter());
        assert!(!args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&mut args).is_err());
    }

    #[test]
    fn test_java_path_must_exist() {
        let cmd = [
            "arnis",
            "--output-dir",
            "/nonexistent/path",
            "--bbox",
            "1,2,3,4",
        ];
        let mut args = Args::parse_from(cmd.iter());
        let result = validate_args(&mut args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_bedrock_path_must_exist() {
        let cmd = [
            "arnis",
            "--bedrock",
            "--output-dir",
            "/nonexistent/path",
            "--bbox",
            "1,2,3,4",
        ];
        let mut args = Args::parse_from(cmd.iter());
        let result = validate_args(&mut args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_wfs_dependency() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--enable-underground-wfs"
        ];
        let mut args = Args::parse_from(cmd.iter());
        let result = validate_args(&mut args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must provide a --wfs-endpoint"));
    }

    #[test]
    fn test_offline_wfs_conflict() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--file",
            "dummy.json",
            "--offline",
            "--enable-underground-wfs",
            "--wfs-endpoint",
            "http://dummy"
        ];
        let mut args = Args::parse_from(cmd.iter());
        let result = validate_args(&mut args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot enable WFS or MVT"));
    }

    #[test]
    fn test_degenerate_bbox() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,1,1,1",
        ];
        let mut args = Args::parse_from(cmd.iter());
        let result = validate_args(&mut args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid bounding box"));
    }

    #[test]
    fn test_required_options() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let mut args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&mut args).is_ok());

        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let mut args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&mut args).is_ok());

        let cmd = ["arnis", "--output-dir", tmp_path, "--file", ""];
        assert!(Args::try_parse_from(cmd.iter()).is_err());
    }

    #[test]
    fn test_spawn_point_both_required() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
        ];
        let mut args = Args::parse_from(cmd.iter());
        assert!(validate_args(&mut args).is_err());

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lng",
            "3.0",
        ];
        let mut args = Args::parse_from(cmd.iter());
        assert!(validate_args(&mut args).is_err());

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
            "--spawn-lng",
            "3.0",
        ];
        let mut args = Args::parse_from(cmd.iter());
        assert!(validate_args(&mut args).is_ok());

        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "5.0",
            "--spawn-lng",
            "3.0",
        ];
        let mut args = Args::parse_from(cmd.iter());
        assert!(validate_args(&mut args).is_err());
    }
}