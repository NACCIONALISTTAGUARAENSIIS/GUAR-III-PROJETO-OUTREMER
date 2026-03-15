//! CAESB & CEB Indoor Utility Provider (BESM-6 Government Tier)
//!
//! Responsável pelo mapeamento de interiores, plantas baixas e saneamento.
//! Opera sob o rigor matemático extremo de 1.15:1 para a escala vertical (Z),
//! convertendo milimetragens de tubulações e cotas de subsolo em Voxels exatos.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde_json::Value;

pub struct IndoorUtilityProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
}

impl IndoorUtilityProvider {
    pub fn new(file_path: PathBuf, scale_h: f64, priority: u8) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
        }
    }

    /// 🚨 BESM-6 TWEAK: A Quantização Rigorosa 1.15:1
    /// Converte a altura real em metros diretamente para o grid Voxel do Minecraft
    #[inline(always)]
    fn quantize_height(raw_height_meters: f64) -> i32 {
        (raw_height_meters * 1.15).round() as i32
    }

    /// Processa uma única coordenada GeoJSON `[lon, lat]`
    #[inline(always)]
    fn parse_coord(coord: &Value, bbox: &LLBBox, transformer: &CoordTransformer, is_completely_outside: &mut bool) -> Option<XZPoint> {
        if let Some(arr) = coord.as_array() {
            if arr.len() >= 2 {
                let lon = arr[0].as_f64()?;
                let lat = arr[1].as_f64()?;

                if let Ok(llpoint) = LLPoint::new(lat, lon) {
                    if bbox.contains(&llpoint) {
                        *is_completely_outside = false;
                    }
                    return Some(transformer.transform_point(llpoint));
                }
            }
        }
        None
    }
}

impl DataProvider for IndoorUtilityProvider {
    fn name(&self) -> &str {
        "CAESB & CEB Indoor Topology (Rigor 1.15:1)"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 🏢 Iniciando escaneamento de Plantas Baixas e Saneamento Subterrâneo: {}", self.file_path.display());

        let json_data = fs::read_to_string(&self.file_path)
            .map_err(|e| format!("Falha ao ler o arquivo Indoor {}: {}", self.file_path.display(), e))?;

        let geojson: Value = serde_json::from_str(&json_data)
            .map_err(|e| format!("JSON malformado em {}: {}", self.file_path.display(), e))?;

        let feature_array = geojson.get("features")
            .and_then(|f| f.as_array())
            .ok_or("Formato inválido: o arquivo não contém um array 'features'")?;

        // Inicializa o Transformador de Projeção Mestre do Arnis (ECEF / ENU)
        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha no transformador ECEF: {}", e))?;

        let mut features = Vec::with_capacity(feature_array.len());
        // Offset isolado para não colidir com OSM, GDF GeoJSON, CityGML ou LiDAR
        let mut next_id = 9_000_000_000;

        for feat in feature_array {
            let properties = feat.get("properties").unwrap_or(&Value::Null);
            let geometry_json = feat.get("geometry");

            if geometry_json.is_none() || geometry_json.unwrap().is_null() {
                continue;
            }

            let geom_obj = geometry_json.unwrap();
            let geom_type = geom_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let coords = geom_obj.get("coordinates").unwrap_or(&Value::Null);

            let mut tags = HashMap::new();
            tags.insert("source".to_string(), "CAESB_Indoor_Topology".to_string());

            let mut semantic_group = SemanticGroup::Indoor;

            // Tradução Estrita de Atributos CAESB/CEB/Novacap
            if let Some(obj) = properties.as_object() {
                for (key, value) in obj {
                    let val_str = match value {
                        Value::String(s) => s.trim().to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => continue,
                    };

                    if val_str.is_empty() { continue; }
                    let col = key.to_lowercase();

                    match col.as_str() {
                        "level" | "pavimento" | "andar" => {
                            tags.insert("level".to_string(), val_str);
                        }
                        "height" | "altura" | "profundidade" | "z" => {
                            // Aplica a pré-quantização inteira 1.15:1 no exato momento da leitura
                            if let Ok(h_meters) = val_str.parse::<f64>() {
                                let h_voxels = Self::quantize_height(h_meters);
                                tags.insert("height".to_string(), h_voxels.to_string());
                            }
                        }
                        "indoor" | "room" | "sala" | "galeria" => {
                            tags.insert("indoor".to_string(), val_str);
                        }
                        "utility" | "rede" | "sistema" => {
                            tags.insert("utility".to_string(), val_str.clone());

                            // Mapeamento Dinâmico de Semântica do Submundo
                            let val_lower = val_str.to_lowercase();
                            if val_lower.contains("sewer") || val_lower.contains("esgoto") || val_lower.contains("water") || val_lower.contains("agua") {
                                semantic_group = SemanticGroup::Sanitation;
                            } else if val_lower.contains("power") || val_lower.contains("energia") || val_lower.contains("eletrico") {
                                semantic_group = SemanticGroup::Power;
                            } else if val_lower.contains("telecom") || val_lower.contains("fibra") {
                                semantic_group = SemanticGroup::Telecom;
                            } else {
                                semantic_group = SemanticGroup::Utility;
                            }
                        }
                        "diameter" | "diametro" | "bitola" => {
                            tags.insert("diameter".to_string(), val_str);
                        }
                        "material" | "composicao" => {
                            tags.insert("material".to_string(), val_str);
                        }
                        _ => {
                            // Preserva atributos crus de engenharias passadas
                            tags.insert(format!("indoor:{}", col), val_str);
                        }
                    }
                }
            }

            let mut is_completely_outside = true;

            let geometry = match geom_type {
                "Point" => {
                    if let Some(pt) = Self::parse_coord(coords, bbox, &transformer, &mut is_completely_outside) {
                        GeometryType::Point(pt)
                    } else { continue; }
                }
                "LineString" => {
                    // Essencial para o traçado de dutos e redes de esgoto
                    if let Some(arr) = coords.as_array() {
                        let mut line = Vec::with_capacity(arr.len());
                        for c in arr {
                            if let Some(pt) = Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside) {
                                line.push(pt);
                            }
                        }
                        if line.len() < 2 { continue; }
                        GeometryType::LineString(line)
                    } else { continue; }
                }
                "Polygon" => {
                    // Essencial para salas, estações de tratamento (ETE) e lajes
                    if let Some(rings) = coords.as_array() {
                        if let Some(exterior_ring) = rings.first().and_then(|r| r.as_array()) {
                            let mut outer = Vec::with_capacity(exterior_ring.len());
                            for c in exterior_ring {
                                if let Some(pt) = Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside) {
                                    outer.push(pt);
                                }
                            }
                            // Garante fechamento do polígono estrutural
                            if outer.len() > 2 && outer.first() != outer.last() {
                                let first = outer[0];
                                outer.push(first);
                            }
                            if outer.len() < 4 { continue; }
                            GeometryType::Polygon(outer)
                        } else { continue; }
                    } else { continue; }
                }
                _ => continue, // Complexidades não-euclidianas são ignoradas no submundo
            };

            // Culling Físico: Se a rede está fora do Distrito Federal carregado, ignora
            if is_completely_outside {
                continue;
            }

            let feature = Feature::new(
                next_id,
                semantic_group,
                tags,
                geometry,
                "CAESB_Indoor_Topology".to_string(),
                self.priority,
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        println!("[INFO] ✅ CAESB Indoor Topology: {} dutos, galerias e salas extraídas perfeitamente.", features.len());

        Ok(features)
    }
}