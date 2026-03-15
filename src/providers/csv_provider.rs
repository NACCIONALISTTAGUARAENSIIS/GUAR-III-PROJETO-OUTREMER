use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF Oficial
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs::File;

use csv::ReaderBuilder;

/// Provedor de Dados Tabulares Abertos (CSV).
/// Projetado para ler listagens governamentais do dados.df.gov.br (Postes, arvores da NOVACAP, Paragens de Autocarro).
/// Localiza dinamicamente as colunas de Latitude/Longitude e injeta pontos precisos na malha.
pub struct CsvProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    pub semantic_override: Option<SemanticGroup>,
}

impl CsvProvider {
    pub fn new(file_path: PathBuf, scale_h: f64, priority: u8, semantic_override: Option<SemanticGroup>) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
            semantic_override,
        }
    }

    /// Identifica dinamicamente os �ndices das colunas de latitude e longitude.
    fn detect_coordinate_columns(headers: &csv::StringRecord) -> Option<(usize, usize)> {
        let mut lat_idx = None;
        let mut lon_idx = None;

        for (i, header) in headers.iter().enumerate() {
            let clean_header = header.trim().to_lowercase();
            
            // Farejador heur�stico para Latitude
            if clean_header == "lat" || clean_header == "latitude" || clean_header == "y" || clean_header == "lat_y" {
                lat_idx = Some(i);
            }
            // Farejador heur�stico para Longitude
            if clean_header == "lon" || clean_header == "lng" || clean_header == "longitude" || clean_header == "x" || clean_header == "lon_x" {
                lon_idx = Some(i);
            }
        }

        if let (Some(lat), Some(lon)) = (lat_idx, lon_idx) {
            Some((lat, lon))
        } else {
            None
        }
    }

    /// O "Rosetta Stone": Converte as colunas do CSV para tags do OSM.
    fn translate_attributes(headers: &csv::StringRecord, record: &csv::StringRecord) -> HashMap<String, String> {
        let mut tags = HashMap::with_capacity(headers.len() + 1);
        tags.insert("source".to_string(), "GDF_OpenData_CSV".to_string());

        let mut is_tree = false;
        let mut is_pole = false;
        let mut is_bus_stop = false;

        for (i, value) in record.iter().enumerate() {
            let val_str = value.trim().to_string();
            if val_str.is_empty() { continue; }

            if let Some(header) = headers.get(i) {
                let col = header.trim().to_uppercase();
                let lower_val = val_str.to_lowercase();

                // Infer�ncia Sem�ntica do Distrito Federal (Mobili�rio Urbano)
                match col.as_str() {
                    "ESPECIE" | "NOME_CIENTIFICO" | "ARVORE" | "VEGETACAO" => {
                        is_tree = true;
                        tags.insert("species".to_string(), val_str.clone());
                    }
                    "TIPO" | "EQUIPAMENTO" | "CATEGORIA" => {
                        if lower_val.contains("poste") || lower_val.contains("iluminacao") {
                            is_pole = true;
                        } else if lower_val.contains("onibus") || lower_val.contains("parada") || lower_val.contains("abrigo") {
                            is_bus_stop = true;
                        } else if lower_val.contains("lixeira") || lower_val.contains("residuo") {
                            tags.insert("amenity".to_string(), "waste_basket".to_string());
                        }
                    }
                    "NOME" | "DESCRICAO" => {
                        tags.insert("name".to_string(), val_str.clone());
                    }
                    "ALTURA" => {
                        tags.insert("height".to_string(), val_str.clone());
                    }
                    _ => {
                        tags.insert(format!("csv:{}", col.to_lowercase()), val_str.clone());
                    }
                }
            }
        }

        // Aplica as tags estruturais baseadas na heur�stica
        if is_tree {
            tags.insert("natural".to_string(), "tree".to_string());
        } else if is_pole {
            tags.insert("highway".to_string(), "street_lamp".to_string());
        } else if is_bus_stop {
            tags.insert("highway".to_string(), "bus_stop".to_string());
            tags.insert("public_transport".to_string(), "platform".to_string());
        }

        tags.shrink_to_fit();
        tags
    }
}

impl DataProvider for CsvProvider {
    fn priority(&self) -> u8 { self.priority }
    fn name(&self) -> &str {
        "GDF Open Data (CSV Point Cloud)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] ?? A abrir planilha de dados abertos CSV: {}", self.file_path.display());

        let file = File::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir ficheiro CSV: {}", e))?;

        // Motor flex�vel: lida com separadores v�rgula ou ponto-e-v�rgula comuns no Brasil
        let mut rdr = ReaderBuilder::new()
            .flexible(true)
            .from_reader(file);

        let headers = rdr.headers()
            .map_err(|e| format!("Falha ao ler o cabe�alho do CSV: {}", e))?
            .clone();

        let (lat_idx, lon_idx) = match Self::detect_coordinate_columns(&headers) {
            Some(indices) => indices,
            None => return Err(format!("Colunas de Latitude e Longitude n�o encontradas no ficheiro: {}", self.file_path.display())),
        };

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        let mut features = Vec::new();
        let mut next_id = 8_000_000_000; // Offset dedicado para CSV OpenData

        for result in rdr.records() {
            let record = match result {
                Ok(rec) => rec,
                Err(_) => continue, // Ignora linhas corrompidas silenciosamente
            };

            // Extra��o segura das coordenadas
            let lat_str = record.get(lat_idx).unwrap_or("").replace(",", ".");
            let lon_str = record.get(lon_idx).unwrap_or("").replace(",", ".");

            let lat = match lat_str.parse::<f64>() {
                Ok(val) => val,
                Err(_) => continue,
            };

            let lon = match lon_str.parse::<f64>() {
                Ok(val) => val,
                Err(_) => continue,
            };

            // ?? BESM-6 Tweak: Early-Z Culling Geogr�fico Absoluto
            if let Ok(llpoint) = LLPoint::new(lat, lon) {
                if !bbox.contains(&llpoint) {
                    continue; // Ponto fora do mapa do jogador, � descartado instantaneamente
                }

                let tags = Self::translate_attributes(&headers, &record);

                let semantic_group = self.semantic_override.unwrap_or_else(|| {
                    if tags.contains_key("natural") { SemanticGroup::Natural }
                    else if tags.contains_key("highway") { SemanticGroup::Infrastructure }
                    else { SemanticGroup::Other }
                });

                let xz_point = transformer.transform_point(llpoint);

                let feature = Feature::new(
                    next_id,
                    semantic_group,
                    tags,
                    GeometryType::Point(xz_point),
                    "GDF_CSV".to_string(),
                    self.priority,
                );

                features.push(feature);
                next_id += 1;
            }
        }

        features.shrink_to_fit();
        println!("[INFO] ? Planilha CSV processada: {} pontos infraestruturais injetados.", features.len());
        Ok(features)
    }
}