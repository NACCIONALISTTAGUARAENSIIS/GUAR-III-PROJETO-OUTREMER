use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF Oficial
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde_json::Value;

/// Provedor de Dados Governamentais via GeoJSON.
/// Lê arquivos locais .geojson, aplica o Rosetta Stone para traduzir atributos do GDF para OSM,
/// e projeta as geometrias (WGS84) para a malha Voxel do Minecraft com escala controlada.
pub struct GeoJsonProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    pub semantic_override: Option<SemanticGroup>,
}

impl GeoJsonProvider {
    pub fn new(file_path: PathBuf, scale_h: f64, priority: u8, semantic_override: Option<SemanticGroup>) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
            semantic_override,
        }
    }

    /// O "Rosetta Stone": Converte atributos JSON do GDF/SITURB para o dialeto OSM
    fn translate_attributes(properties: &Value) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_GeoJSON".to_string());

        if let Some(obj) = properties.as_object() {
            for (key, value) in obj {
                let val_str = match value {
                    Value::String(s) => s.trim().to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue, // Ignora arrays ou objetos aninhados (não padrão para atributos simples)
                };

                if val_str.is_empty() {
                    continue;
                }

                let col = key.to_uppercase();

                // Mapeamento Heurístico (Baseado no padrão SEDUH/SITURB/CODEPLAN)
                match col.as_str() {
                    "PAVIMENTOS" | "GABARITO" | "N_PAV" | "LEVELS" => {
                        tags.insert("building:levels".to_string(), val_str);
                        if !tags.contains_key("building") {
                            tags.insert("building".to_string(), "yes".to_string());
                        }
                    }
                    "ALTURA" | "COTA_TOPO" | "HEIGHT" => {
                        tags.insert("height".to_string(), val_str);
                    }
                    "USO" | "USO_SOLO" | "DESTINACAO" | "LANDUSE" | "TIPO" => {
                        let uso = val_str.to_lowercase();
                        let mapped_uso = if uso.contains("comercial") || uso.contains("commercial") {
                            "commercial"
                        } else if uso.contains("residencial") || uso.contains("residential") {
                            "residential"
                        } else if uso.contains("institucional") || uso.contains("equipamento") || uso.contains("civic") {
                            "civic"
                        } else if uso.contains("industrial") {
                            "industrial"
                        } else {
                            "yes"
                        };
                        tags.insert("building".to_string(), mapped_uso.to_string());
                    }
                    "NOME" | "DESC" | "LOGRADOURO" | "NAME" => {
                        tags.insert("name".to_string(), val_str.clone());
                    }
                    "TIPO_VIA" | "CLASSE_VIA" | "HIGHWAY" => {
                        tags.insert("highway".to_string(), "residential".to_string()); // Fallback
                    }
                    "NATURAL" | "VEGETACAO" | "ARVORE" => {
                        tags.insert("natural".to_string(), val_str.to_lowercase());
                    }
                    _ => {
                        // Mantém atributos crus para expansão futura
                        tags.insert(format!("gdf:{}", col.to_lowercase()), val_str);
                    }
                }
            }
        }

        // Fallback: Se não identificou nada, assume prédio (útil para footprints brutos da Codeplan)
        if !tags.contains_key("building") && !tags.contains_key("highway") && !tags.contains_key("natural") {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
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

impl DataProvider for GeoJsonProvider {
    fn name(&self) -> &str {
        "GDF GeoJSON (Geoportal / OpenData)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] Carregando e parseando GeoJSON na RAM: {}", self.file_path.display());
        
        let json_data = fs::read_to_string(&self.file_path)
            .map_err(|e| format!("Falha ao ler o arquivo GeoJSON {}: {}", self.file_path.display(), e))?;

        let geojson: Value = serde_json::from_str(&json_data)
            .map_err(|e| format!("JSON malformado em {}: {}", self.file_path.display(), e))?;

        let feature_array = geojson.get("features")
            .and_then(|f| f.as_array())
            .ok_or("GeoJSON inválido: objeto principal não contém array 'features'")?;

        // Inicializa o Transformador de Projeção Mestre do Arnis (ECEF / ENU)
        // GeoJSON nativamente usa EPSG:4326, então não precisamos do proj-sys aqui, só alinhar para a BBox XZ.
        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        let mut features = Vec::with_capacity(feature_array.len());
        let mut next_id = 3_000_000_000; // Offset dedicado para GeoJSON para evitar colisão (Shapefile=1BI, WFS=2BI)

        for feat in feature_array {
            let properties = feat.get("properties").unwrap_or(&Value::Null);
            let geometry_json = feat.get("geometry");

            if geometry_json.is_none() || geometry_json.unwrap().is_null() {
                continue;
            }

            let geom_obj = geometry_json.unwrap();
            let geom_type = geom_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let coords = geom_obj.get("coordinates").unwrap_or(&Value::Null);

            let tags = Self::translate_attributes(properties);

            let semantic_group = self.semantic_override.unwrap_or_else(|| {
                if tags.contains_key("building") { SemanticGroup::Building }
                else if tags.contains_key("highway") { SemanticGroup::Highway }
                else if tags.contains_key("natural") { SemanticGroup::Natural }
                else { SemanticGroup::Other }
            });

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
                    // GeoJSON Polygon is an array of LinearRings. The first is the exterior ring.
                    if let Some(rings) = coords.as_array() {
                        if let Some(exterior_ring) = rings.first().and_then(|r| r.as_array()) {
                            let mut outer = Vec::with_capacity(exterior_ring.len());
                            for c in exterior_ring {
                                if let Some(pt) = Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside) {
                                    outer.push(pt);
                                }
                            }
                            
                            // Garante fechamento do polígono
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
                    // Pega o primeiro polígono, primeiro anel externo para não complexificar a engine voxel
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
                _ => continue, // FeatureCollection ou GeometryCollection aninhadas são ignoradas
            };

            // Early-Z Culling Geográfico: Não importa a feature na malha se ela estiver no Rio de Janeiro
            if is_completely_outside {
                continue;
            }

            let feature = Feature::new(
                next_id,
                semantic_group,
                tags,
                geometry,
                "GDF_GeoJSON".to_string(),
                self.priority,
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        println!("[INFO] ? GeoJSON processado com sucesso: {} geometrias extraídas.", features.len());
        Ok(features)
    }
}