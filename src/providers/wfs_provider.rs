//! Provedor de Serviços Web de Elementos (WFS) - O "Submundo" BESM-6
//!
//! Responsável por contactar servidores governamentais (CAESB, CEB, Novacap)
//! em tempo real para extrair infraestruturas subterrâneas e de serviços.
//! Aplica a Voxelização Local Determinística em profundidade (Z-Index invertido).

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::time::Duration;
use serde_json::Value;

pub struct WFSProvider {
    pub endpoint: String,
    pub scale_h: f64,
    pub priority: u8,
}

impl WFSProvider {
    pub fn new(endpoint: String, scale_h: f64, priority: u8) -> Self {
        Self {
            endpoint,
            scale_h,
            priority,
        }
    }

    /// O "Rosetta Stone" Subterrâneo: Traduz atributos WFS (GDF/CAESB/CEB) para tags do motor.
    fn translate_wfs_attributes(properties: &Value) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_WFS_Live".to_string());

        // Padrão WFS: Enterramos toda a infraestrutura por defeito, a menos que especificado o contrário
        tags.insert("layer".to_string(), "-1".to_string());
        tags.insert("location".to_string(), "underground".to_string());

        if let Some(obj) = properties.as_object() {
            for (key, value) in obj {
                let val_str = match value {
                    Value::String(s) => s.trim().to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };

                if val_str.is_empty() {
                    continue;
                }

                let col = key.to_uppercase();

                // 🚨 Mapeamento Heurístico BESM-6 (Redes de Saneamento e Energia)
                match col.as_str() {
                    "TIPO_REDE" | "SISTEMA" | "NETWORK" => {
                        let tipo = val_str.to_lowercase();
                        if tipo.contains("esgoto") || tipo.contains("sewage") {
                            tags.insert("man_made".to_string(), "pipeline".to_string());
                            tags.insert("substance".to_string(), "sewage".to_string());
                        } else if tipo.contains("pluvial") || tipo.contains("drain") {
                            tags.insert("man_made".to_string(), "pipeline".to_string());
                            tags.insert("substance".to_string(), "drain".to_string());
                        } else if tipo.contains("agua") || tipo.contains("água") || tipo.contains("water") {
                            tags.insert("man_made".to_string(), "pipeline".to_string());
                            tags.insert("substance".to_string(), "water".to_string());
                        } else if tipo.contains("energia") || tipo.contains("eletrica") || tipo.contains("power") {
                            tags.insert("power".to_string(), "cable".to_string());
                        }
                    }
                    "DIAMETRO" | "DIAMETER" | "DN" => {
                        tags.insert("diameter".to_string(), val_str.clone());
                    }
                    "PROFUNDIDADE" | "DEPTH" | "Z" => {
                        tags.insert("depth".to_string(), val_str.clone());
                        // Ajusta a camada com base na profundidade real
                        if let Ok(prof) = val_str.parse::<f64>() {
                            if prof > 3.0 {
                                tags.insert("layer".to_string(), "-2".to_string());
                            }
                        }
                    }
                    "MATERIAL" => {
                        let mat = val_str.to_lowercase();
                        if mat.contains("pvc") || mat.contains("plastico") {
                            tags.insert("material".to_string(), "plastic".to_string());
                        } else if mat.contains("concreto") || mat.contains("manilha") {
                            tags.insert("material".to_string(), "concrete".to_string());
                        } else if mat.contains("ferro") || mat.contains("aco") {
                            tags.insert("material".to_string(), "metal".to_string());
                        } else {
                            tags.insert("material".to_string(), val_str.clone());
                        }
                    }
                    "ESTADO" | "STATUS" | "SITUACAO" => {
                        let status = val_str.to_lowercase();
                        if status.contains("abandonado") || status.contains("inativo") {
                            tags.insert("abandoned".to_string(), "yes".to_string());
                        }
                    }
                    _ => {
                        // Preserva metadados crus para o sistema de telemetria
                        tags.insert(format!("wfs:{}", col.to_lowercase()), val_str);
                    }
                }
            }
        }

        // Fallback garantido: se for linha e não tiver classificação, é um cano genérico.
        if !tags.contains_key("man_made") && !tags.contains_key("power") {
            tags.insert("man_made".to_string(), "pipeline".to_string());
        }

        tags.shrink_to_fit();
        tags
    }

    /// Processa a coordenada extraída do WFS (assumindo WGS84 EPSG:4326 via GeoJSON output)
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

impl DataProvider for WFSProvider {
    fn priority(&self) -> u8 { self.priority }
    fn name(&self) -> &str {
        "Submundo WFS (Infraestrutura Subterrânea Live)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 🚇 A mergulhar no Submundo WFS. A construir a query espacial para o servidor...");

        // Constrói a query BBox no formato padrão OGC WFS (minX, minY, maxX, maxY)
        // Forçamos o formato GeoJSON para reaproveitar o nosso motor de parse robusto.
        let separator = if self.endpoint.contains('?') { "&" } else { "?" };
        let request_url = format!(
            "{}{}service=WFS&version=1.0.0&request=GetFeature&bbox={},{},{},{},EPSG:4326&outputFormat=application/json",
            self.endpoint,
            separator,
            bbox.min().lng(), bbox.min().lat(),
            bbox.max().lng(), bbox.max().lat()
        );

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|e| format!("Falha ao construir o cliente HTTP WFS: {}", e))?;

        let response = client.get(&request_url).send()
            .map_err(|e| format!("Timeout ou falha na ligação ao servidor WFS: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Servidor WFS rejeitou o pedido com o código: {}", response.status()));
        }

        let json_text = response.text()
            .map_err(|e| format!("Falha ao descodificar a resposta de texto WFS: {}", e))?;

        let geojson: Value = serde_json::from_str(&json_text)
            .map_err(|e| format!("Resposta WFS não é um GeoJSON válido: {}", e))?;

        let feature_array = geojson.get("features")
            .and_then(|f| f.as_array())
            .ok_or("WFS GeoJSON não contém a matriz 'features'. Pode estar vazio ou o formato não é suportado.")?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao instanciar o transformador de coordenadas WFS: {}", e))?;

        let mut features = Vec::with_capacity(feature_array.len());

        // 🚨 Offset Dedicado WFS: 5 mil milhões para isolamento estrito O(1)
        let mut next_id = 5_000_000_000;

        for feat in feature_array {
            let properties = feat.get("properties").unwrap_or(&Value::Null);
            let geometry_json = feat.get("geometry");

            if geometry_json.is_none() || geometry_json.unwrap().is_null() {
                continue;
            }

            let geom_obj = geometry_json.unwrap();
            let geom_type = geom_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let coords = geom_obj.get("coordinates").unwrap_or(&Value::Null);

            let tags = Self::translate_wfs_attributes(properties);

            // Resolução Semântica
            let semantic_group = if tags.contains_key("power") { SemanticGroup::Power }
            else if tags.contains_key("substance") && tags.get("substance").unwrap() == "sewage" { SemanticGroup::Sewage }
            else if tags.contains_key("substance") && tags.get("substance").unwrap() == "water" { SemanticGroup::Utility }
            else { SemanticGroup::Underground };

            let mut is_completely_outside = true;

            let geometry = match geom_type {
                "Point" => {
                    if let Some(pt) = Self::parse_coord(coords, bbox, &transformer, &mut is_completely_outside) {
                        GeometryType::Point(pt)
                    } else { continue; }
                }
                "LineString" => {
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
                    if let Some(rings) = coords.as_array() {
                        if let Some(exterior_ring) = rings.first().and_then(|r| r.as_array()) {
                            let mut outer = Vec::with_capacity(exterior_ring.len());
                            for c in exterior_ring {
                                if let Some(pt) = Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside) {
                                    outer.push(pt);
                                }
                            }

                            if outer.len() > 2 && outer.first() != outer.last() {
                                let first = outer[0];
                                outer.push(first);
                            }

                            if outer.len() < 4 { continue; }
                            GeometryType::Polygon(outer)
                        } else { continue; }
                    } else { continue; }
                }
                "MultiPolygon" => {
                    if let Some(multipoly) = coords.as_array() {
                        if let Some(first_poly) = multipoly.first().and_then(|p| p.as_array()) {
                            if let Some(exterior_ring) = first_poly.first().and_then(|r| r.as_array()) {
                                let mut outer = Vec::with_capacity(exterior_ring.len());
                                for c in exterior_ring {
                                    if let Some(pt) = Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside) {
                                        outer.push(pt);
                                    }
                                }

                                if outer.len() > 2 && outer.first() != outer.last() {
                                    let first = outer[0];
                                    outer.push(first);
                                }

                                if outer.len() < 4 { continue; }
                                GeometryType::Polygon(outer)
                            } else { continue; }
                        } else { continue; }
                    } else { continue; }
                }
                _ => continue,
            };

            if is_completely_outside {
                continue;
            }

            let feature = Feature::new(
                next_id,
                semantic_group,
                tags,
                geometry,
                "GDF_WFS_Live".to_string(),
                self.priority,
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        println!("[INFO] ✅ Varredura do Submundo concluída: {} redes de infraestrutura subterrânea extraídas via WFS.", features.len());
        Ok(features)
    }
}