use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::LLBBox;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

// Necessário para ler Shapefiles e Tabelas DBF
use shapefile::{Reader, Shape};
// Necessário para reprojeção matemática SIRGAS 2000 -> WGS84
use proj::Proj;

/// Provedor de Dados Governamentais do Distrito Federal.
/// Lê Shapefiles locais, reprojeta de UTM 23S para WGS84, mapeia colunas DBF para tags OSM,
/// e injeta Features com prioridade maxima no motor.
pub struct GDFProvider {
    pub shp_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    pub semantic_override: Option<SemanticGroup>,
}

impl GDFProvider {
    pub fn new(
        shp_path: PathBuf,
        scale_h: f64,
        priority: u8,
        semantic_override: Option<SemanticGroup>,
    ) -> Self {
        Self {
            shp_path,
            scale_h,
            priority,
            semantic_override,
        }
    }

    /// O "Rosetta Stone": Converte atributos de banco de dados do GDF para o dialeto OSM
    /// que o `buildings.rs` e o `highways.rs` já sabem ler.
    fn translate_attributes(dbf_record: &shapefile::dbase::Record) -> HashMap<String, String> {
        let mut tags = HashMap::new();

        // Mantemos um marcador de origem
        tags.insert("source".to_string(), "GDF_Shapefile".to_string());

        // 🚨 CORREÇÃO CRÍTICA BESM-6: Iteração universal em Record DBF.
        // Ao transformar em iterador, desestruturamos a tupla nativa explícita (nome do campo, valor do campo).
        for (name, value) in dbf_record.clone().into_iter() {
            let val_str = match value {
                shapefile::dbase::FieldValue::Character(Some(s)) => s.trim().to_string(),
                shapefile::dbase::FieldValue::Numeric(Some(n)) => n.to_string(),
                shapefile::dbase::FieldValue::Float(Some(f)) => f.to_string(),
                shapefile::dbase::FieldValue::Integer(i) => i.to_string(),
                _ => continue,
            };

            if val_str.is_empty() {
                continue;
            }

            // 🚨 Tipagem resolvida e limpa: 'name' vira string e depois upper case.
            let col = name.to_string().to_uppercase();

            // Mapeamento Heurístico (Baseado no padrão SEDUH/SITURB do DF)
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
                "USO" | "USO_SOLO" | "DESTINACAO" | "TIPO" => {
                    let uso = val_str.to_lowercase();
                    let mapped_uso = if uso.contains("comercial") {
                        "commercial"
                    } else if uso.contains("residencial") {
                        "residential"
                    } else if uso.contains("institucional") || uso.contains("equipamento") {
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
                    // Hook automático para landmarks: se o nome do shapefile bater com
                    // o do OSM, ele será pescado pelo landmarks.rs
                }
                "TIPO_VIA" | "CLASSE_VIA" | "HIGHWAY" => {
                    tags.insert("highway".to_string(), "residential".to_string());
                    // Fallback
                }
                _ => {
                    // Mantém atributos crus para debug ou expansão futura
                    tags.insert(format!("gdf:{}", col.to_lowercase()), val_str);
                }
            }
        }

        // Se o shapefile não definiu nada estrutural, forçamos como building padrão
        // assumindo que a maioria dos SHPs locais que vamos injetar são footprints exatos da CODEPLAN.
        if !tags.contains_key("building") && !tags.contains_key("highway") {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
    }

    /// Helper Geométrico (Semelhante à lógica interna do Arnis) para projetar LL para XZ
    #[inline]
    fn project_to_minecraft_xz(lat: f64, lon: f64, bbox: &LLBBox, scale_h: f64) -> XZPoint {
        // Conversão Equirretangular baseada na origem da Bounding Box do jogador
        const EARTH_RADIUS_M: f64 = 6_371_000.0;

        let min_lat_rad = bbox.min().lat().to_radians();
        let lat_rad = lat.to_radians();
        let lon_rad = lon.to_radians();
        let min_lon_rad = bbox.min().lng().to_radians();

        let dx_meters = (lon_rad - min_lon_rad) * EARTH_RADIUS_M * min_lat_rad.cos();
        let dz_meters = (bbox.max().lat().to_radians() - lat_rad) * EARTH_RADIUS_M;

        let mx = (dx_meters * scale_h).round() as i32;
        let mz = (dz_meters * scale_h).round() as i32;

        XZPoint::new(mx, mz)
    }
}

impl DataProvider for GDFProvider {
    fn name(&self) -> &str {
        "GDF Shapefile (Geoportal SITURB)"
    }

    // 🚨 BESM-6: Satisfazendo o contrato mestre do ProviderManager
    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        let mut reader = Reader::from_path(&self.shp_path)
            .map_err(|e| format!("Falha ao ler Shapefile {}: {}", self.shp_path.display(), e))?;

        // Inicializa o pipeline de reprojeção
        // EPSG:31983 (SIRGAS 2000 / UTM zone 23S) -> EPSG:4326 (WGS84 Lat/Lon)
        let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
            .ok()
            .ok_or("Falha ao inicializar biblioteca PROJ para CRS 31983 -> 4326")?;

        let mut features = Vec::new();
        let mut next_id = 1_000_000_000; // Offset alto para não colidir com IDs do OSM

        // Itera pelos registros (Geometria + Atributos do DBF) simultaneamente
        for result in reader.iter_shapes_and_records() {
            let (shape, record) = match result {
                Ok(data) => data,
                Err(_) => continue, // Ignora geometria corrompida silenciosamente (Fast-Fail)
            };

            let tags = Self::translate_attributes(&record);

            // Define o grupo semântico (usa override se a camada inteira for de um tipo específico, ex: "Árvores")
            let semantic_group = self.semantic_override.unwrap_or_else(|| {
                if tags.contains_key("building") {
                    SemanticGroup::Building
                } else if tags.contains_key("highway") {
                    SemanticGroup::Highway
                } else {
                    SemanticGroup::Other
                }
            });

            // Extração e Reprojeção da Geometria
            let geometry = match shape {
                Shape::Polygon(poly) => {
                    let mut outer_ring = Vec::new();

                    // Shapefile Polygons contêm anéis (Rings). O primeiro geralmente é o Outer.
                    for ring in poly.rings() {
                        let mut mc_points = Vec::with_capacity(ring.points().len());

                        for pt in ring.points() {
                            // Converte de UTM para Lat/Lon
                            let (lon, lat) = proj
                                .convert((pt.x, pt.y))
                                .map_err(|e| format!("Erro de reprojeção PROJ: {}", e))?;

                            // Projeta para blocos do Minecraft
                            let xz = Self::project_to_minecraft_xz(lat, lon, bbox, self.scale_h);
                            mc_points.push(xz);
                        }

                        // Garante o fechamento topológico do anel
                        if mc_points.len() > 2 && mc_points.first() != mc_points.last() {
                            let first = mc_points[0];
                            mc_points.push(first);
                        }

                        // Assumimos o primeiro anel como exterior por simplicidade.
                        // (Otimização BESM-6: Ignora buracos internos complexos de shapefiles residenciais)
                        outer_ring = mc_points;
                        break;
                    }

                    if outer_ring.len() < 4 {
                        continue;
                    }
                    GeometryType::Polygon(outer_ring)
                }
                Shape::Polyline(pline) => {
                    let mut lines = Vec::new();
                    for part in pline.parts() {
                        for pt in part {
                            let (lon, lat) = proj.convert((pt.x, pt.y)).unwrap_or((0.0, 0.0));
                            lines.push(Self::project_to_minecraft_xz(lat, lon, bbox, self.scale_h));
                        }
                        break; // Pega só o primeiro segmento contínuo para evitar complexidade
                    }
                    if lines.len() < 2 {
                        continue;
                    }
                    GeometryType::LineString(lines)
                }
                Shape::Point(pt) => {
                    let (lon, lat) = proj.convert((pt.x, pt.y)).unwrap_or((0.0, 0.0));
                    let xz = Self::project_to_minecraft_xz(lat, lon, bbox, self.scale_h);
                    GeometryType::Point(xz)
                }
                _ => continue, // MultiPatch e outros formatos não suportados descartados
            };

            // Criar Feature (Calcula AABB dinamicamente)
            let feature = Feature::new(
                next_id,
                semantic_group,
                tags,
                geometry,
                "GDF_Shapefile".to_string(),
                self.priority,
            );

            // Filtro Espacial (Early-Z Culling)
            // Se o AABB da feature inteira estiver fora da BBox do mapa do Minecraft, descartamos.
            let (min_x, max_x, min_z, max_z) = feature.aabb;
            let (mc_max_x, mc_max_z) = (
                Self::project_to_minecraft_xz(
                    bbox.max().lat(),
                    bbox.max().lng(),
                    bbox,
                    self.scale_h,
                )
                .x,
                Self::project_to_minecraft_xz(
                    bbox.min().lat(),
                    bbox.max().lng(),
                    bbox,
                    self.scale_h,
                )
                .z,
            );

            // Verifica se está dentro do quadrante gerado (assumindo origem 0,0)
            if max_x < 0 || min_x > mc_max_x || max_z < 0 || min_z > mc_max_z {
                continue;
            }

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        Ok(features)
    }
}
