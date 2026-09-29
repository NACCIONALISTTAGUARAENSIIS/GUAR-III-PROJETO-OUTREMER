use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
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

        let mut uso_raw: Option<String> = None;

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
                    // Decidido DEPOIS do loop (`uso_to_tag`): prédio só com
                    // evidência estrutural no registro; senão é lote (landuse).
                    uso_raw = Some(val_str);
                }
                "NOME" | "DESC" | "LOGRADOURO" | "NAME" => {
                    tags.insert("name".to_string(), val_str.clone());
                    // Hook automático para landmarks: se o nome do shapefile bater com
                    // o do OSM, ele será pescado pelo landmarks.rs
                }
                "TIPO_VIA" | "CLASSE_VIA" | "HIGHWAY" => {
                    // 🚨 RECONEXÃO: antes, QUALQUER via do SEDUH/SITURB virava
                    // `highway=residential`, sem exceção — ver
                    // `providers::classify_highway_from_tipo_via`. O valor bruto
                    // ainda vira `gdf:tipo_via` (não se perde).
                    tags.insert(
                        "highway".to_string(),
                        crate::providers::classify_highway_from_tipo_via(&val_str).to_string(),
                    );
                    tags.insert("gdf:tipo_via".to_string(), val_str.clone());
                }
                _ => {
                    // Mantém atributos crus para debug ou expansão futura
                    tags.insert(format!("gdf:{}", col.to_lowercase()), val_str);
                }
            }
        }

        if let Some(uso) = uso_raw {
            let has_structure = tags.contains_key("building:levels")
                || tags.contains_key("height")
                || tags.contains_key("building");
            let (key, value) = crate::providers::uso_to_tag(&uso, has_structure);
            if !tags.contains_key(key) {
                tags.insert(key.to_string(), value.to_string());
            }
        }

        // Se o shapefile não definiu nada estrutural, forçamos como building padrão
        // assumindo que a maioria dos SHPs locais que vamos injetar são footprints exatos da CODEPLAN.
        if !tags.contains_key("building")
            && !tags.contains_key("highway")
            && !tags.contains_key("landuse")
        {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
    }

    /// Projeta lat/lon para blocos com a MESMA transformação do resto do motor
    /// (`CoordTransformer`: ENU ancorado no Marco Zero de Brasília).
    ///
    /// 🚨 CONFLITO REAL ENTRE PROVIDERS: este provider tinha uma projeção
    /// própria, equirretangular e RELATIVA ao canto do bbox (0..largura), enquanto
    /// todos os outros providers e o próprio OSM usam o ENU absoluto (Guará em
    /// X≈−18.000). Toda feição do Shapefile caía milhares de blocos fora do
    /// mundo e era descartada — ou, num bbox que incluísse a origem, pintada no
    /// lugar errado.
    #[inline]
    fn project_to_minecraft_xz(
        transformer: &CoordTransformer,
        lat: f64,
        lon: f64,
    ) -> Option<XZPoint> {
        LLPoint::new(lat, lon)
            .ok()
            .map(|ll| transformer.transform_point(ll))
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
    fn describe_sources(&self) -> Vec<String> {
        vec![self.shp_path.display().to_string()]
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        let mut reader = Reader::from_path(&self.shp_path)
            .map_err(|e| format!("Falha ao ler Shapefile {}: {}", self.shp_path.display(), e))?;

        // Inicializa o pipeline de reprojeção
        // EPSG:31983 (SIRGAS 2000 / UTM zone 23S) -> EPSG:4326 (WGS84 Lat/Lon)
        let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
            .ok()
            .ok_or("Falha ao inicializar biblioteca PROJ para CRS 31983 -> 4326")?;

        let (transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador ENU: {}", e))?;

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
            let semantic_group = self
                .semantic_override
                .unwrap_or_else(|| crate::providers::semantic_group_from_tags(&tags));

            // Extração e Reprojeção da Geometria
            let geometry = match shape {
                Shape::Polygon(poly) => {
                    // Shapefile Polygons contêm anéis (Rings). Assumimos o primeiro anel
                    // como exterior por simplicidade (Otimização BESM-6: ignora buracos
                    // internos complexos de shapefiles residenciais).
                    let mut outer_ring = Vec::new();
                    if let Some(ring) = poly.rings().iter().next() {
                        let mut mc_points = Vec::with_capacity(ring.points().len());

                        for pt in ring.points() {
                            // Converte de UTM para Lat/Lon
                            let (lon, lat) = proj
                                .convert((pt.x, pt.y))
                                .map_err(|e| format!("Erro de reprojeção PROJ: {}", e))?;

                            // Projeta para blocos do Minecraft
                            if let Some(xz) = Self::project_to_minecraft_xz(&transformer, lat, lon)
                            {
                                mc_points.push(xz);
                            }
                        }

                        // Garante o fechamento topológico do anel
                        if mc_points.len() > 2 && mc_points.first() != mc_points.last() {
                            let first = mc_points[0];
                            mc_points.push(first);
                        }

                        outer_ring = mc_points;
                    }

                    if outer_ring.len() < 4 {
                        continue;
                    }
                    GeometryType::Polygon(outer_ring)
                }
                Shape::Polyline(pline) => {
                    // Pega só o primeiro segmento contínuo para evitar complexidade
                    let mut lines = Vec::new();
                    if let Some(part) = pline.parts().iter().next() {
                        for pt in part {
                            let (lon, lat) = proj.convert((pt.x, pt.y)).unwrap_or((0.0, 0.0));
                            if let Some(xz) = Self::project_to_minecraft_xz(&transformer, lat, lon)
                            {
                                lines.push(xz);
                            }
                        }
                    }
                    if lines.len() < 2 {
                        continue;
                    }
                    GeometryType::LineString(lines)
                }
                Shape::Point(pt) => {
                    let (lon, lat) = proj.convert((pt.x, pt.y)).unwrap_or((0.0, 0.0));
                    let Some(xz) = Self::project_to_minecraft_xz(&transformer, lat, lon) else {
                        continue;
                    };
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
            // Recorte pelo XZBBox REAL do mundo (mesma transformação), não por
            // uma origem (0,0) suposta.
            if max_x < xzbbox.min_x()
                || min_x > xzbbox.max_x()
                || max_z < xzbbox.min_z()
                || min_z > xzbbox.max_z()
            {
                continue;
            }

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        Ok(features)
    }
}
