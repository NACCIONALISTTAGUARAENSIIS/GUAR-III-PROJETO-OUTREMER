//! Provedor de Banco de Dados Espacial (PostgreSQL + PostGIS)
//!
//! Permite conexão direta ao SISDIA (Sistema de Informações Territoriais e Urbanas do DF)
//! ou qualquer banco de dados corporativo, executando queries espaciais (ST_Intersects)
//! no lado do servidor para trazer apenas o que importa para a Voxelização.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use postgres::Client;
use serde_json::Value;

pub struct PostGisProvider {
    pub connection_string: String,
    pub scale_h: f64,
    pub priority: u8,
    pub table_name: String,
    pub geom_column: String,
}

impl PostGisProvider {
    pub fn new(connection_string: String, table_name: &str, geom_column: &str, scale_h: f64, priority: u8) -> Self {
        Self {
            connection_string,
            scale_h,
            priority,
            table_name: table_name.to_string(),
            geom_column: geom_column.to_string(),
        }
    }

    /// Tradutor dinâmico de colunas do Banco de Dados para Tags do Motor
    fn translate_sql_attributes(row: &postgres::Row) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_PostGIS_Live".to_string());

        for column in row.columns() {
            let col_name = column.name();
            // Ignoramos a coluna de geometria pois ela é tratada separadamente
            if col_name == "geom_json" { continue; }

            // Tenta extrair como String de forma genérica
            let val_str: String = match column.type_().name() {
                "varchar" | "text" | "char" => {
                    let v: Option<&str> = row.get(col_name);
                    v.unwrap_or("").to_string()
                }
                "int4" | "int8" | "int2" => {
                    let v: Option<i32> = row.get(col_name);
                    v.map(|n| n.to_string()).unwrap_or_default()
                }
                "float4" | "float8" | "numeric" => {
                    let v: Option<f64> = row.get(col_name);
                    v.map(|n| n.to_string()).unwrap_or_default()
                }
                "bool" => {
                    let v: Option<bool> = row.get(col_name);
                    v.map(|b| b.to_string()).unwrap_or_default()
                }
                _ => continue,
            };

            if val_str.is_empty() { continue; }

            let upper_col = col_name.to_uppercase();

            // Mapeamento Heurístico SEDUH/CODEPLAN
            match upper_col.as_str() {
                "PAVIMENTOS" | "GABARITO" | "N_PAV" => {
                    tags.insert("building:levels".to_string(), val_str);
                    if !tags.contains_key("building") {
                        tags.insert("building".to_string(), "yes".to_string());
                    }
                }
                "ALTURA" | "HEIGHT" => {
                    tags.insert("height".to_string(), val_str);
                }
                "USO_SOLO" | "DESTINACAO" | "TIPO_LOTE" => {
                    let uso = val_str.to_lowercase();
                    let mapped_uso = if uso.contains("comercial") { "commercial" }
                    else if uso.contains("residencial") { "residential" }
                    else if uso.contains("institucional") { "civic" }
                    else if uso.contains("industrial") { "industrial" }
                    else { "yes" };
                    tags.insert("building".to_string(), mapped_uso.to_string());
                }
                "NOME" | "LOGRADOURO" => {
                    tags.insert("name".to_string(), val_str.clone());
                }
                "CLASSE_VIA" | "HIERARQUIA" => {
                    tags.insert("highway".to_string(), "residential".to_string());
                }
                _ => {
                    tags.insert(format!("db:{}", col_name.to_lowercase()), val_str);
                }
            }
        }

        // Fallback genérico
        if !tags.contains_key("building") && !tags.contains_key("highway") {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
    }

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

impl DataProvider for PostGisProvider {
    fn name(&self) -> &str {
        "PostGIS SISDIA DB Connection"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 🐘 Estabelecendo conexão direta com Banco de Dados Espacial PostGIS: Tabela '{}'", self.table_name);

        let mut client = Client::connect(&self.connection_string, postgres::NoTls)
            .map_err(|e| format!("Falha de conexão com o banco PostGIS: {}", e))?;

        // 🚨 BESM-6 Query: O PostGIS faz o trabalho pesado de Culling usando ST_Intersects,
        // e devolve a geometria nativamente transformada em GeoJSON via ST_AsGeoJSON
        // para facilitar o parse em Rust.
        let query = format!(
            "SELECT *, ST_AsGeoJSON(ST_Transform({}, 4326)) as geom_json
             FROM {}
             WHERE ST_Intersects(ST_Transform({}, 4326), ST_MakeEnvelope($1, $2, $3, $4, 4326))",
            self.geom_column, self.table_name, self.geom_column
        );

        let min_lon = bbox.min().lng();
        let min_lat = bbox.min().lat();
        let max_lon = bbox.max().lng();
        let max_lat = bbox.max().lat();

        let rows = client.query(&query, &[&min_lon, &min_lat, &max_lon, &max_lat])
            .map_err(|e| format!("Falha ao executar query espacial ST_Intersects: {}", e))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha no transformador de coordenadas: {}", e))?;

        let mut features = Vec::with_capacity(rows.len());
        let mut next_id = 10_000_000_000; // Offset para DB

        for row in rows {
            let geom_json_str: Option<&str> = row.get("geom_json");
            if geom_json_str.is_none() { continue; }

            let geom_obj: Value = serde_json::from_str(geom_json_str.unwrap()).unwrap_or(Value::Null);
            let geom_type = geom_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let coords = geom_obj.get("coordinates").unwrap_or(&Value::Null);

            let tags = Self::translate_sql_attributes(&row);

            let semantic_group = if tags.contains_key("building") { SemanticGroup::Building }
            else if tags.contains_key("highway") { SemanticGroup::Highway }
            else { SemanticGroup::Other };

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
                _ => continue,
            };

            if is_completely_outside { continue; }

            let feature = Feature::new(
                next_id,
                semantic_group,
                tags,
                geometry,
                "GDF_PostGIS_Live".to_string(),
                self.priority,
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        println!("[INFO] ✅ Query PostGIS concluída: {} registros processados.", features.len());
        Ok(features)
    }
}