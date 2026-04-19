//! KML Provider (Google Earth / IPHAN / ADASA Tier)
//!
//! Processa ficheiros Keyhole Markup Language (.kml) utilizando SAX Streaming O(1).
//! Essencial para extrair polígonos de tombamento histórico do IPHAN, malhas
//! de bacias hidrográficas da ADASA e vetores do Metrô-DF.
//! Aplica a Voxelização Local Determinística em tempo real durante a leitura do XML.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};

use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct KmlProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    pub semantic_override: Option<SemanticGroup>,
}

impl KmlProvider {
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        priority: u8,
        semantic_override: Option<SemanticGroup>,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
            semantic_override,
        }
    }

    /// O parser de coordenadas do KML. O formato é "lon,lat,alt lon,lat,alt ..."
    #[inline(always)]
    fn parse_kml_coordinates(
        text: &str,
        bbox: &LLBBox,
        transformer: &CoordTransformer,
        is_completely_outside: &mut bool,
    ) -> Vec<XZPoint> {
        let mut points = Vec::new();

        for chunk in text.split_whitespace() {
            let parts: Vec<&str> = chunk.split(',').collect();
            if parts.len() >= 2 {
                if let (Ok(lon), Ok(lat)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                    if let Ok(llpoint) = LLPoint::new(lat, lon) {
                        if bbox.contains(&llpoint) {
                            *is_completely_outside = false;
                        }
                        points.push(transformer.transform_point(llpoint));
                    }
                }
            }
        }
        points
    }
}

impl DataProvider for KmlProvider {
    fn name(&self) -> &str {
        "GDF KML/KMZ Provider (IPHAN / Metrô-DF / ADASA)"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 📍 Iniciando scanner SAX Streaming no KML: {}", self.file_path.display());

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        let mut reader = Reader::from_file(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo KML {}: {}", self.file_path.display(), e))?;

        reader.trim_text(true);

        let mut buf = Vec::new();
        let mut features = Vec::new();
        let mut next_id = 7_000_000_000; // Offset dedicado para KML

        // Máquina de Estados Lexical
        let mut in_placemark = false;
        let mut capture_target = String::new(); // "name", "description", "coordinates"
        let mut current_geom_type = String::new(); // "Point", "LineString", "Polygon"

        let mut current_tags: HashMap<String, String> = HashMap::new();
        let mut current_geometry: Option<GeometryType> = None;
        let mut is_completely_outside = true;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.as_ref()).to_lowercase();

                    if name_str == "placemark" {
                        in_placemark = true;
                        current_tags.clear();
                        current_tags.insert("source".to_string(), "GDF_KML".to_string());
                        current_geometry = None;
                        is_completely_outside = true;
                    } else if in_placemark {
                        match name_str.as_str() {
                            "name" => capture_target = "name".to_string(),
                            "description" => capture_target = "description".to_string(),
                            "point" => current_geom_type = "Point".to_string(),
                            "linestring" => current_geom_type = "LineString".to_string(),
                            "polygon" => current_geom_type = "Polygon".to_string(),
                            "coordinates" => capture_target = "coordinates".to_string(),
                            _ => {}
                        }
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_placemark && !capture_target.is_empty() {
                        let text = String::from_utf8_lossy(e.as_ref()).into_owned();

                        match capture_target.as_str() {
                            "name" => {
                                current_tags.insert("name".to_string(), text.clone());

                                // 🚨 Heurística Governamental (Rigor BESM-6)
                                let name_lower = text.to_lowercase();
                                if name_lower.contains("metrô") || name_lower.contains("metro") || name_lower.contains("trilho") {
                                    current_tags.insert("railway".to_string(), "subway".to_string());
                                } else if name_lower.contains("tombamento") || name_lower.contains("iphan") {
                                    current_tags.insert("historic".to_string(), "yes".to_string());
                                    current_tags.insert("boundary".to_string(), "protected_area".to_string());
                                } else if name_lower.contains("parque") || name_lower.contains("app") {
                                    current_tags.insert("leisure".to_string(), "nature_reserve".to_string());
                                }
                            }
                            "description" => {
                                current_tags.insert("description".to_string(), text);
                            }
                            "coordinates" => {
                                let pts = Self::parse_kml_coordinates(&text, bbox, &transformer, &mut is_completely_outside);

                                if !pts.is_empty() {
                                    match current_geom_type.as_str() {
                                        "Point" => {
                                            current_geometry = Some(GeometryType::Point(pts[0]));
                                        }
                                        "LineString" => {
                                            if pts.len() >= 2 {
                                                current_geometry = Some(GeometryType::LineString(pts));
                                            }
                                        }
                                        "Polygon" => {
                                            let mut ring = pts;
                                            if ring.len() >= 3 {
                                                // Garante fechamento
                                                if ring.first() != ring.last() {
                                                    let first = ring[0];
                                                    ring.push(first);
                                                }
                                                current_geometry = Some(GeometryType::Polygon(ring));
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Event::CData(e)) => {
                    if in_placemark && !capture_target.is_empty() {
                        let text = String::from_utf8_lossy(e.as_ref()).into_owned();

                        match capture_target.as_str() {
                            "name" => {
                                current_tags.insert("name".to_string(), text.clone());

                                // 🚨 Heurística Governamental (Rigor BESM-6)
                                let name_lower = text.to_lowercase();
                                if name_lower.contains("metrô") || name_lower.contains("metro") || name_lower.contains("trilho") {
                                    current_tags.insert("railway".to_string(), "subway".to_string());
                                } else if name_lower.contains("tombamento") || name_lower.contains("iphan") {
                                    current_tags.insert("historic".to_string(), "yes".to_string());
                                    current_tags.insert("boundary".to_string(), "protected_area".to_string());
                                } else if name_lower.contains("parque") || name_lower.contains("app") {
                                    current_tags.insert("leisure".to_string(), "nature_reserve".to_string());
                                }
                            }
                            "description" => {
                                current_tags.insert("description".to_string(), text);
                            }
                            "coordinates" => {
                                let pts = Self::parse_kml_coordinates(&text, bbox, &transformer, &mut is_completely_outside);

                                if !pts.is_empty() {
                                    match current_geom_type.as_str() {
                                        "Point" => {
                                            current_geometry = Some(GeometryType::Point(pts[0]));
                                        }
                                        "LineString" => {
                                            if pts.len() >= 2 {
                                                current_geometry = Some(GeometryType::LineString(pts));
                                            }
                                        }
                                        "Polygon" => {
                                            let mut ring = pts;
                                            if ring.len() >= 3 {
                                                // Garante fechamento
                                                if ring.first() != ring.last() {
                                                    let first = ring[0];
                                                    ring.push(first);
                                                }
                                                current_geometry = Some(GeometryType::Polygon(ring));
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.as_ref()).to_lowercase();

                    if name_str == "placemark" {
                        in_placemark = false;

                        if !is_completely_outside {
                            if let Some(geom) = current_geometry.take() {

                                // Resolve Grupo Semântico
                                let semantic_group = self.semantic_override.unwrap_or_else(|| {
                                    if current_tags.contains_key("railway") { SemanticGroup::Railway }
                                    else if current_tags.contains_key("historic") { SemanticGroup::Historic }
                                    else if current_tags.contains_key("leisure") { SemanticGroup::ConservationArea }
                                    else { SemanticGroup::Other }
                                });

                                let feature = Feature::new(
                                    next_id,
                                    semantic_group,
                                    current_tags.clone(),
                                    geom,
                                    "GDF_KML".to_string(),
                                    self.priority,
                                );

                                features.push(feature);
                                next_id += 1;
                            }
                        }
                    } else if name_str == capture_target {
                        capture_target.clear();
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    eprintln!("[ALERTA] Erro de parsing SAX no KML: {:?}", e);
                    break;
                }
                _ => {}
            }
            buf.clear();
        }

        features.shrink_to_fit();
        println!("[INFO] ✅ Parsing KML concluído: {} áreas governamentais protegidas mapeadas.", features.len());
        Ok(features)
    }
}