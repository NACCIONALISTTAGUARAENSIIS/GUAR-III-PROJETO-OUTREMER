use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::OsmData;
use crate::progress::{emit_gui_error, emit_gui_progress_update, is_running_with_gui};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use colored::Colorize;
use rand::prelude::IndexedRandom;
use reqwest::blocking::Client;
use reqwest::blocking::ClientBuilder;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Write};
use std::process::Command;
use std::time::Duration;

/// Function to download data using reqwest
fn download_with_reqwest(url: &str, query: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client: Client = ClientBuilder::new()
        .timeout(Duration::from_secs(360))
        .build()?;

    let response: Result<reqwest::blocking::Response, reqwest::Error> =
        client.get(url).query(&[("data", query)]).send();

    match response {
        Ok(resp) => {
            emit_gui_progress_update(3.0, "Downloading data...");
            if resp.status().is_success() {
                let text = resp.text()?;
                if text.is_empty() {
                    return Err("Error! Received invalid from server".into());
                }
                Ok(text)
            } else {
                Err(format!("Error! Received response code: {}", resp.status()).into())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                let msg = "Request timed out. Try selecting a smaller area.";
                eprintln!("{}", format!("Error! {msg}").red().bold());
                Err(msg.into())
            } else if e.is_connect() {
                let msg = "No internet connection.";
                eprintln!("{}", format!("Error! {msg}").red().bold());
                Err(msg.into())
            } else {
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Error,
                    &format!("Request error in download_with_reqwest: {e}"),
                );
                eprintln!("{}", format!("Error! {e:.52}").red().bold());
                Err(format!("{e:.52}").into())
            }
        }
    }
}

/// Function to download data using `curl`
fn download_with_curl(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("curl")
        .arg("-s") // Add silent mode to suppress output
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Curl command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Function to download data using `wget`
fn download_with_wget(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("wget")
        .arg("-qO-") // Use `-qO-` to output the result directly to stdout
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Wget command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

pub fn fetch_data_from_file(file: &str) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Loading data from file...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Loading data from file...");

    let file: File = File::open(file)?;
    let reader: BufReader<File> = BufReader::new(file);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    // 🚨 BESM-6: Utiliza a trait genérica de Deserialize do Serde para reconstruir a matriz.
    let data: OsmData = serde::Deserialize::deserialize(&mut deserializer)?;
    Ok(data)
}

/// 🚨 BESM-6 TWEAK: Subdivisão Tática de Consultas
/// Divide uma BBox massiva em pedaços menores para evitar que a API do Overpass estoure
/// a memória do servidor com "runtime error: out of memory".
fn split_bbox_if_needed(bbox: &LLBBox) -> Vec<LLBBox> {
    let max_span = 0.15; // Aproximadamente 16 km. Se passar disso, fatia o BBox.
    let lat_span = bbox.max().lat() - bbox.min().lat();
    let lon_span = bbox.max().lng() - bbox.min().lng();

    if lat_span <= max_span && lon_span <= max_span {
        return vec![*bbox];
    }

    let lat_splits = (lat_span / max_span).ceil() as u32;
    let lon_splits = (lon_span / max_span).ceil() as u32;

    let mut sub_boxes = Vec::new();
    let lat_step = lat_span / lat_splits as f64;
    let lon_step = lon_span / lon_splits as f64;

    for i in 0..lat_splits {
        for j in 0..lon_splits {
            let min_lat = bbox.min().lat() + (i as f64 * lat_step);
            let min_lon = bbox.min().lng() + (j as f64 * lon_step);
            let max_lat = if i == lat_splits - 1 {
                bbox.max().lat()
            } else {
                min_lat + lat_step
            };
            let max_lon = if j == lon_splits - 1 {
                bbox.max().lng()
            } else {
                min_lon + lon_step
            };

            if let Ok(b) = LLBBox::new(min_lat, min_lon, max_lat, max_lon) {
                sub_boxes.push(b);
            }
        }
    }
    sub_boxes
}

/// Gera a string de consulta Overpass para um BBox especifico.
fn build_overpass_query(bbox: &LLBBox) -> String {
    format!(
        r#"[out:json][timeout:360][bbox:{},{},{},{}];
    (
        nwr["building"];
        nwr["building:part"];
        nwr["highway"];
        nwr["landuse"];
        nwr["natural"];
        nwr["leisure"];
        nwr["water"];
        nwr["waterway"];
        nwr["amenity"];
        nwr["tourism"];
        nwr["bridge"];
        nwr["railway"];
        nwr["roller_coaster"];
        nwr["barrier"];
        nwr["entrance"];
        nwr["door"];
        nwr["power"];
        nwr["historic"];
        nwr["emergency"];
        nwr["advertising"];
        nwr["man_made"];
        nwr["aeroway"];
        way["place"];
        way;
    )->.relsinbbox;
    (
        way(r.relsinbbox);
    )->.waysinbbox;
    (
        node(w.waysinbbox);
        node(w.relsinbbox);
    )->.nodesinbbox;
    .relsinbbox out body;
    .waysinbbox out body;
    .nodesinbbox out skel qt;"#,
        bbox.min().lat(),
        bbox.min().lng(),
        bbox.max().lat(),
        bbox.max().lng(),
    )
}

/// Main function to fetch data
pub fn fetch_data_from_overpass(
    bbox: LLBBox,
    debug: bool,
    download_method: &str,
    save_file: Option<&str>,
) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Fetching data...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Fetching data...");

    // List of Overpass API servers
    let api_servers: Vec<&str> = vec![
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
    ];
    let fallback_api_servers: Vec<&str> =
        vec!["https://maps.mail.ru/osm/tools/overpass/api/interpreter"];
    let mut url: &&str = api_servers.choose(&mut rand::rng()).unwrap();

    // 🚨 BESM-6: Subdivisão do BBox
    let sub_boxes = split_bbox_if_needed(&bbox);
    let mut merged_data = OsmData::default(); // Instanciação correta via Trait
    let total_chunks = sub_boxes.len();

    if total_chunks > 1 {
        println!(
            "[INFO] 🧩 BBox massivo detectado. Dividindo consulta OSM em {} setores.",
            total_chunks
        );
    }

    for (i, sub_bbox) in sub_boxes.iter().enumerate() {
        let query = build_overpass_query(sub_bbox);

        // Fetch data from Overpass API
        let mut attempt = 0;
        let max_attempts = 1;
        let response: String = loop {
            if total_chunks > 1 {
                println!(
                    "Downloading sector {}/{} from {}...",
                    i + 1,
                    total_chunks,
                    url
                );
            } else {
                println!("Downloading from {url} with method {download_method}...");
            }

            let result = match download_method {
                "requests" => download_with_reqwest(url, &query),
                "curl" => download_with_curl(url, &query).map_err(|e| e.into()),
                "wget" => download_with_wget(url, &query).map_err(|e| e.into()),
                _ => download_with_reqwest(url, &query), // Default to requests
            };

            match result {
                Ok(response) => break response,
                Err(error) => {
                    if attempt >= max_attempts {
                        return Err(error);
                    }

                    println!("Request failed. Switching to fallback url...");
                    url = fallback_api_servers.choose(&mut rand::rng()).unwrap();
                    attempt += 1;
                }
            }
        };

        let mut deserializer =
            serde_json::Deserializer::from_reader(Cursor::new(response.as_bytes()));
        let chunk_data: OsmData = serde::Deserialize::deserialize(&mut deserializer)?;

        if chunk_data.is_empty() {
            if let Some(remark) = chunk_data.remark.as_deref() {
                if remark.contains("runtime error") && remark.contains("out of memory") {
                    eprintln!("{}", "Error! The query ran out of memory on the Overpass API server. Try using a smaller area.".red().bold());
                    emit_gui_error("Try using a smaller area.");
                } else {
                    eprintln!("{}", format!("Error! API returned: {remark}").red().bold());
                    emit_gui_error(&format!("API returned: {remark}"));
                }
            } else if total_chunks == 1 {
                eprintln!(
                    "{}",
                    "Error! API returned no data. Please try again!"
                        .red()
                        .bold()
                );
                emit_gui_error("API returned no data. Please try again!");
            }

            if !is_running_with_gui() && total_chunks == 1 {
                std::process::exit(1);
            } else if total_chunks == 1 {
                return Err("Data fetch failed".into());
            }
        }

        // 🚨 BESM-6: Funde as respostas mantendo a integridade dos IDs.
        merged_data.merge(chunk_data);

        // Anti-DDoS delay para respeitar o rate-limit do Overpass
        if total_chunks > 1 && i < total_chunks - 1 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }

    if let Some(save_file) = save_file {
        // Salva a estrutura OSM fundida
        let serialized = serde_json::to_string(&merged_data)?;
        let mut file: File = File::create(save_file)?;
        file.write_all(serialized.as_bytes())?;
        println!("API merged response saved to: {save_file}");
    }

    if debug {
        println!(
            "Additional debug information: {} nodes, {} ways, {} relations fetched.",
            merged_data
                .elements
                .iter()
                .filter(|e| e.type_str() == "node")
                .count(),
            merged_data
                .elements
                .iter()
                .filter(|e| e.type_str() == "way")
                .count(),
            merged_data
                .elements
                .iter()
                .filter(|e| e.type_str() == "relation")
                .count()
        );
    }

    emit_gui_progress_update(5.0, "");

    Ok(merged_data)
}

/// Fetches a short area name using Nominatim for the given lat/lon
pub fn fetch_area_name(lat: f64, lon: f64) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let client = Client::builder().timeout(Duration::from_secs(20)).build()?;

    let url = format!("https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat={lat}&lon={lon}&addressdetails=1");

    let resp = client.get(&url).header("User-Agent", "arnis-rust").send()?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let json: Value = resp.json()?;

    if let Some(address) = json.get("address") {
        let fields = ["city", "town", "village", "county", "borough", "suburb"];
        for field in fields.iter() {
            // 🚨 CORREÇÃO CRÍTICA: Sem tipagem forçada na closure.
            if let Some(name) = address.get(*field).and_then(|v| v.as_str()) {
                let mut name_str = name.to_string();

                // Remove "City of " prefix
                if name_str.to_lowercase().starts_with("city of ") {
                    name_str = name_str[name_str.find(" of ").unwrap() + 4..].to_string();
                }

                return Ok(Some(name_str));
            }
        }
    }

    Ok(None)
}
