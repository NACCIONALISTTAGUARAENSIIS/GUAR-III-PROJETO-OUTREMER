use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use geo::Geometry;
use geozero::wkb::GpkgWkb;
use geozero::ToGeo;
use proj::Proj;
use rusqlite::{types::ValueRef, Connection};

/// Provedor de Banco de Dados GeoPackage (OGC Padr�o Moderno).
/// Varre todas as tabelas espaciais do DB SQLite, decodifica a geometria bin�ria WKB propriet�ria,
/// reprojeta de qualquer EPSG dinamico para WGS84 e injeta as Features Voxelizadas no motor.
pub struct GpkgProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    pub semantic_override: Option<SemanticGroup>,
}

impl GpkgProvider {
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

    /// O "Rosetta Stone": Converte atributos de banco de dados do GDF para o dialeto OSM
    fn translate_attributes(attributes: &HashMap<String, String>) -> HashMap<String, String> {
        let mut tags = HashMap::with_capacity(attributes.len() + 1);
        tags.insert("source".to_string(), "GDF_GeoPackage".to_string());

        for (key, val_str) in attributes {
            if val_str.is_empty() {
                continue;
            }

            let col = key.to_uppercase();

            // Mapeamento Heur�stico (Baseado no padr�o SEDUH/SITURB do DF / IBGE)
            match col.as_str() {
                "PAVIMENTOS" | "GABARITO" | "N_PAV" | "LEVELS" => {
                    tags.insert("building:levels".to_string(), val_str.clone());
                    if !tags.contains_key("building") {
                        tags.insert("building".to_string(), "yes".to_string());
                    }
                }
                "ALTURA" | "COTA_TOPO" | "HEIGHT" => {
                    tags.insert("height".to_string(), val_str.clone());
                }
                "USO" | "USO_SOLO" | "DESTINACAO" | "LANDUSE" | "TIPO" => {
                    let uso = val_str.to_lowercase();
                    let mapped_uso = if uso.contains("comercial") || uso.contains("commercial") {
                        "commercial"
                    } else if uso.contains("residencial") || uso.contains("residential") {
                        "residential"
                    } else if uso.contains("institucional")
                        || uso.contains("equipamento")
                        || uso.contains("civic")
                    {
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
                    tags.insert("highway".to_string(), "residential".to_string());
                    // Fallback
                }
                "NATURAL" | "VEGETACAO" | "ARVORE" | "BIOMA" => {
                    tags.insert("natural".to_string(), val_str.to_lowercase());
                }
                _ => {
                    // Mant�m atributos crus para expans�o ou telemetria
                    tags.insert(format!("gdf:{}", col.to_lowercase()), val_str.clone());
                }
            }
        }

        if !tags.contains_key("building")
            && !tags.contains_key("highway")
            && !tags.contains_key("natural")
        {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
    }
}

impl DataProvider for GpkgProvider {
    fn priority(&self) -> u8 {
        self.priority
    }
    fn name(&self) -> &str {
        "GDF GeoPackage (SQLite Spatial DB)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] ??? Abrindo conex�o SQLite/GeoPackage em: {}",
            self.file_path.display()
        );

        let conn = Connection::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir GeoPackage SQLite: {}", e))?;

        // 1. Interrogar o �ndice mestre para encontrar todas as tabelas e colunas com Geometria
        let mut stmt_geom_cols = conn
            .prepare("SELECT table_name, column_name, srs_id FROM gpkg_geometry_columns")
            .map_err(|e| format!("Falha ao ler gpkg_geometry_columns: {}", e))?;

        let tables_iter = stmt_geom_cols
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            })
            .map_err(|e| format!("Erro de query SQLite: {}", e))?;

        // Mestre de Transforma��o para a Malha Voxel Minecraft
        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        // Sistema de cache para proje��es EPSG din�micas
        let mut proj_cache: HashMap<u32, Proj> = HashMap::new();

        let mut features = Vec::new();
        let mut next_id = 5_000_000_000; // Offset dedicado (Evita colis�o com Shapefile e OSM)

        for table_res in tables_iter {
            if let Ok((table_name, geom_column, srs_id)) = table_res {
                println!(
                    "[INFO] Inspecionando tabela espacial: {} (EPSG:{})",
                    table_name, srs_id
                );

                // Prepara a proje��o geogr�fica espec�fica desta tabela
                if !proj_cache.contains_key(&srs_id) && srs_id != 4326 {
                    let proj_str = format!("EPSG:{}", srs_id);
                    if let Ok(proj) = Proj::new_known_crs(&proj_str, "EPSG:4326", None) {
                        proj_cache.insert(srs_id, proj);
                    } else {
                        eprintln!(
                            "[AVISO] EPSG:{} n�o suportado. Pulando tabela {}.",
                            srs_id, table_name
                        );
                        continue;
                    }
                }

                // 2. Extrair dados da tabela
                let query = format!("SELECT * FROM \"{}\"", table_name);
                let mut stmt_data = match conn.prepare(&query) {
                    Ok(s) => s,
                    Err(_) => continue, // Tabela pode estar corrompida ou vazia, pula silenciosamente
                };

                let column_count = stmt_data.column_count();
                let column_names: Vec<String> = stmt_data
                    .column_names()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                let geom_col_index = column_names.iter().position(|r| r == &geom_column);

                if geom_col_index.is_none() {
                    continue;
                }
                let geom_idx = geom_col_index.unwrap();

                let mut rows = match stmt_data.query([]) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                while let Ok(Some(row)) = rows.next() {
                    // Tenta puxar o bin�rio (BLOB) da geometria
                    let geom_ref = match row.get_ref(geom_idx) {
                        Ok(ValueRef::Blob(b)) => b,
                        _ => continue,
                    };

                    // Decodifica a formata��o Bin�ria GPKG -> geo::Geometry
                    let geo_geom = match GpkgWkb(geom_ref.to_vec()).to_geo() {
                        Ok(g) => g,
                        Err(_) => continue,
                    };

                    // Extrai os outros atributos para o HashMap
                    let mut raw_attributes = HashMap::new();
                    for i in 0..column_count {
                        if i == geom_idx {
                            continue;
                        }
                        let col_name = &column_names[i];

                        let val_str = match row.get_ref(i) {
                            Ok(ValueRef::Integer(n)) => n.to_string(),
                            Ok(ValueRef::Real(f)) => f.to_string(),
                            Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
                            _ => continue,
                        };
                        raw_attributes.insert(col_name.clone(), val_str);
                    }

                    let tags = Self::translate_attributes(&raw_attributes);

                    let semantic_group = self.semantic_override.unwrap_or_else(|| {
                        if tags.contains_key("building") {
                            SemanticGroup::Building
                        } else if tags.contains_key("highway") {
                            SemanticGroup::Highway
                        } else if tags.contains_key("natural") {
                            SemanticGroup::Natural
                        } else {
                            SemanticGroup::Other
                        }
                    });

                    // 3. Helper de Proje��o Interna e Early-Z Culling
                    let proj_ref = proj_cache.get(&srs_id);
                    let mut is_completely_outside = true;

                    let mut process_coord = |x: f64, y: f64| -> Option<XZPoint> {
                        let (lon, lat) = if srs_id == 4326 {
                            (x, y) // J� � WGS84
                        } else {
                            proj_ref?.convert((x, y)).ok()?
                        };

                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            if bbox.contains(&llpoint) {
                                is_completely_outside = false;
                            }
                            Some(transformer.transform_point(llpoint))
                        } else {
                            None
                        }
                    };

                    // 4. Mapeamento de Geometrias Complexas
                    let engine_geometry = match geo_geom {
                        Geometry::Point(p) => {
                            if let Some(xz) = process_coord(p.x(), p.y()) {
                                GeometryType::Point(xz)
                            } else {
                                continue;
                            }
                        }
                        Geometry::LineString(ls) => {
                            let mut points = Vec::with_capacity(ls.0.len());
                            for p in ls.0 {
                                if let Some(xz) = process_coord(p.x, p.y) {
                                    points.push(xz);
                                }
                            }
                            if points.len() < 2 {
                                continue;
                            }
                            GeometryType::LineString(points)
                        }
                        Geometry::Polygon(poly) => {
                            let ext = poly.exterior();
                            let mut outer = Vec::with_capacity(ext.0.len());
                            for p in &ext.0 {
                                if let Some(xz) = process_coord(p.x, p.y) {
                                    outer.push(xz);
                                }
                            }

                            // Garante fechamento
                            if outer.len() > 2 && outer.first() != outer.last() {
                                let first = outer[0];
                                outer.push(first);
                            }

                            if outer.len() < 4 {
                                continue;
                            }
                            GeometryType::Polygon(outer)
                        }
                        Geometry::MultiPolygon(mp) => {
                            // Extrai o anel principal do primeiro pol�gono do conjunto para Voxel
                            if let Some(poly) = mp.0.first() {
                                let ext = poly.exterior();
                                let mut outer = Vec::with_capacity(ext.0.len());
                                for p in &ext.0 {
                                    if let Some(xz) = process_coord(p.x, p.y) {
                                        outer.push(xz);
                                    }
                                }

                                if outer.len() > 2 && outer.first() != outer.last() {
                                    let first = outer[0];
                                    outer.push(first);
                                }

                                if outer.len() < 4 {
                                    continue;
                                }
                                GeometryType::Polygon(outer)
                            } else {
                                continue;
                            }
                        }
                        _ => continue, // Multipoint, GeometryCollection, etc n�o s�o suportados.
                    };

                    // O Early-Z Culling impede que o IBGE carregue o Mato Grosso se a sua BBox for s� Bras�lia
                    if is_completely_outside {
                        continue;
                    }

                    let feature = Feature::new(
                        next_id,
                        semantic_group,
                        tags,
                        engine_geometry,
                        "GDF_GeoPackage".to_string(),
                        self.priority,
                    );

                    features.push(feature);
                    next_id += 1;
                }
            }
        }

        features.shrink_to_fit();
        println!(
            "[INFO] ? GeoPackage varrido com sucesso: {} blocos espaciais extra�dos.",
            features.len()
        );
        Ok(features)
    }
}
