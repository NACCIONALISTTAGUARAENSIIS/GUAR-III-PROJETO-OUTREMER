use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF Oficial
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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

    /// O "Rosetta Stone": Converte atributos JSON do GDF/SITURB para o dialeto OSM
    fn translate_attributes(properties: &Value) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_GeoJSON".to_string());

        // 🚨 RECONEXÃO: sinal de curadoria `_gdf_layer` — não é uma coluna que o
        // ArcGIS do geoportal do GDF devolve nativamente, é anotada por quem baixa
        // o recorte (ver scripts/fetch_gdf_layers.py) para desambiguar de qual
        // camada real (Edificação, Calçadas, Estacionamentos, Jardins, Massa
        // Arbórea, Cercas e Muros, Eixo de Arruamento...) cada feição veio — algo
        // que o próprio ArcGIS não embute na resposta. Processado ANTES do loop
        // genérico de colunas para servir de valor-base, refinável pelos campos
        // reais de cada camada logo abaixo (ex.: CM_MAT refina barrier=fence→wall;
        // NRFAIXAS refina o highway= genérico). Camadas cuja mera presença no
        // dataset não garante a feição real (Calçadas: a área é mapeada mesmo onde
        // NÃO há calçada construída) não recebem valor-base aqui — ficam
        // inteiramente a cargo do campo `CALCADA` abaixo.
        if let Some(Value::String(layer)) = properties.get("_gdf_layer") {
            match layer.as_str() {
                "estacionamentos" => {
                    tags.insert("amenity".to_string(), "parking".to_string());
                }
                "jardins" => {
                    tags.insert("landuse".to_string(), "grass".to_string());
                }
                "massa_arborea" => {
                    tags.insert("natural".to_string(), "wood".to_string());
                }
                "cercas_muros" => {
                    tags.insert("barrier".to_string(), "fence".to_string());
                }
                "arruamento" => {
                    tags.insert("highway".to_string(), "unclassified".to_string());
                }
                _ => {}
            }
        }

        if let Some(obj) = properties.as_object() {
            for (key, value) in obj {
                if key == "_gdf_layer" {
                    continue;
                }
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
                    // 🚨 RECONEXÃO: `ED_NUM_PAV`/`ED_ALT_APROX`/`ED_NOME` são os nomes de
                    // coluna REAIS da camada "Edificação" do CADASTRO_TERRITORIAL do
                    // GDF (www.geoservicos.ide.df.gov.br) — verificado contra a
                    // resposta real do serviço para o Guará I (187 feições), não um
                    // palpite de nome de coluna. `PAVIMENTOS`/`GABARITO`/etc. eram
                    // nomes hipotéticos que NUNCA bateram contra o dado real do GDF.
                    "PAVIMENTOS" | "GABARITO" | "N_PAV" | "LEVELS" | "ED_NUM_PAV" => {
                        tags.insert("building:levels".to_string(), val_str);
                        if !tags.contains_key("building") {
                            tags.insert("building".to_string(), "yes".to_string());
                        }
                    }
                    "ALTURA" | "COTA_TOPO" | "HEIGHT" | "ED_ALT_APROX" => {
                        tags.insert("height".to_string(), val_str);
                    }
                    // `PN_USO` é a coluna real da camada "Lotes Registrados" — valores
                    // observados incluem abreviações como "Inst EP" (institucional,
                    // equipamento público), por isso o gatilho extra `starts_with("inst")`
                    // além do `contains("institucional")` já existente (que nunca bate
                    // contra a forma abreviada real).
                    "USO" | "USO_SOLO" | "DESTINACAO" | "LANDUSE" | "TIPO" | "PN_USO" => {
                        let uso = val_str.to_lowercase();
                        let mapped_uso = if uso.contains("comercial") || uso.contains("commercial")
                        {
                            "commercial"
                        } else if uso.contains("residencial") || uso.contains("residential") {
                            "residential"
                        } else if uso.contains("institucional")
                            || uso.contains("equipamento")
                            || uso.contains("civic")
                            || uso.starts_with("inst")
                        {
                            "civic"
                        } else if uso.contains("industrial") {
                            "industrial"
                        } else {
                            "yes"
                        };
                        tags.insert("building".to_string(), mapped_uso.to_string());
                    }
                    "NOME" | "DESC" | "LOGRADOURO" | "NAME" | "ED_NOME" => {
                        tags.insert("name".to_string(), val_str.clone());
                    }
                    // `CM_MAT` (camada real "Cercas e Muros", já resolvida de código
                    // numérico para texto na curadoria — ver `fetch_gdf_layers.py`):
                    // refina o `barrier=fence` de base (posto pelo `_gdf_layer` acima)
                    // para `wall` quando o material é rígido — alvenaria/concreto real
                    // é muro, não cerca.
                    "CM_MAT" => {
                        let mat = val_str.to_lowercase();
                        if mat.contains("alvenaria") || mat.contains("concreto") {
                            tags.insert("barrier".to_string(), "wall".to_string());
                        }
                    }
                    // `NRFAIXAS` (camada real "Eixo do Trecho de Arruamento"): número de
                    // faixas é um proxy mais confiável que o texto de `tipoarruamento`
                    // (que na prática descreve o TIPO DE TRECHO — "Entroncamento",
                    // "Logradouro", "Beco" — não uma hierarquia viária) para refinar o
                    // `highway=unclassified` de base.
                    "NRFAIXAS" => {
                        if let Ok(faixas) = val_str.parse::<i32>() {
                            let refined = if faixas >= 3 {
                                "secondary"
                            } else if faixas == 2 {
                                "tertiary"
                            } else if faixas == 1 {
                                "residential"
                            } else {
                                "unclassified"
                            };
                            tags.insert("highway".to_string(), refined.to_string());
                        }
                    }
                    // `CALCADA` (camada real "Passeio e ou Calçadas"): a camada mapeia a
                    // faixa ao longo da via mesmo onde NÃO há calçada construída — só
                    // "Sim" garante que a feição é uma calçada real; "Não" fica sem tag
                    // (área sem calçada, não deve virar `highway=footway` fantasma).
                    "CALCADA" => {
                        if val_str.eq_ignore_ascii_case("sim") {
                            tags.insert("highway".to_string(), "footway".to_string());
                            tags.insert("footway".to_string(), "sidewalk".to_string());
                        }
                    }
                    "TIPO_VIA" | "CLASSE_VIA" | "HIGHWAY" => {
                        // 🚨 RECONEXÃO: ver `providers::classify_highway_from_tipo_via`
                        // — antes, toda via virava `highway=residential` cego.
                        tags.insert(
                            "highway".to_string(),
                            crate::providers::classify_highway_from_tipo_via(&val_str).to_string(),
                        );
                        tags.insert("gdf:tipo_via".to_string(), val_str.clone());
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

        // Fallback: Se não identificou nada, assume prédio (útil para footprints brutos da Codeplan).
        // 🚨 RECONEXÃO: também respeita `landuse`/`amenity`/`barrier` — sem isso, uma
        // feição real de Estacionamento/Jardim/Cerca (identificada só pelo
        // `_gdf_layer` de base, sem nenhum campo próprio que batesse nos ramos
        // acima) tinha esse fallback cego sobrescrevendo o resultado correto,
        // virando "building=yes" por cima de um estacionamento ou jardim de verdade.
        if !tags.contains_key("building")
            && !tags.contains_key("highway")
            && !tags.contains_key("natural")
            && !tags.contains_key("landuse")
            && !tags.contains_key("amenity")
            && !tags.contains_key("barrier")
        {
            tags.insert("building".to_string(), "yes".to_string());
        }

        tags.shrink_to_fit();
        tags
    }

    /// Processa uma única coordenada GeoJSON `[lon, lat]`
    #[inline(always)]
    fn parse_coord(
        coord: &Value,
        bbox: &LLBBox,
        transformer: &CoordTransformer,
        is_completely_outside: &mut bool,
    ) -> Option<XZPoint> {
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
    fn priority(&self) -> u8 {
        self.priority
    }
    fn describe_sources(&self) -> Vec<String> {
        vec![self.file_path.display().to_string()]
    }
    fn name(&self) -> &str {
        "GDF GeoJSON (Geoportal / OpenData)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] 🌐 Carregando e parseando GeoJSON na RAM: {}",
            self.file_path.display()
        );

        let json_data = fs::read_to_string(&self.file_path).map_err(|e| {
            format!(
                "Falha ao ler o arquivo GeoJSON {}: {}",
                self.file_path.display(),
                e
            )
        })?;

        let geojson: Value = serde_json::from_str(&json_data)
            .map_err(|e| format!("JSON malformado em {}: {}", self.file_path.display(), e))?;

        let feature_array = geojson
            .get("features")
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

            // 🚨 CORREÇÃO CRÍTICA: Extração pura de valor JSON para str sem tipagem forçada em inferência de closure.
            let geom_type = geom_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

            let coords = geom_obj.get("coordinates").unwrap_or(&Value::Null);

            let tags = Self::translate_attributes(properties);

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

            let mut is_completely_outside = true;

            let geometry = match geom_type {
                "Point" => {
                    if let Some(pt) =
                        Self::parse_coord(coords, bbox, &transformer, &mut is_completely_outside)
                    {
                        GeometryType::Point(pt)
                    } else {
                        continue;
                    }
                }
                "LineString" => {
                    if let Some(arr) = coords.as_array() {
                        let mut line = Vec::with_capacity(arr.len());
                        for c in arr {
                            if let Some(pt) =
                                Self::parse_coord(c, bbox, &transformer, &mut is_completely_outside)
                            {
                                line.push(pt);
                            }
                        }
                        if line.len() < 2 {
                            continue;
                        }
                        GeometryType::LineString(line)
                    } else {
                        continue;
                    }
                }
                "Polygon" => {
                    // GeoJSON Polygon is an array of LinearRings. The first is the exterior ring.
                    if let Some(rings) = coords.as_array() {
                        if let Some(exterior_ring) = rings.first().and_then(|r| r.as_array()) {
                            let mut outer = Vec::with_capacity(exterior_ring.len());
                            for c in exterior_ring {
                                if let Some(pt) = Self::parse_coord(
                                    c,
                                    bbox,
                                    &transformer,
                                    &mut is_completely_outside,
                                ) {
                                    outer.push(pt);
                                }
                            }

                            // Garante fechamento do polígono
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
                    } else {
                        continue;
                    }
                }
                "MultiPolygon" => {
                    // Pega o primeiro polígono, primeiro anel externo para não complexificar a engine voxel
                    if let Some(multipoly) = coords.as_array() {
                        if let Some(first_poly) = multipoly.first().and_then(|p| p.as_array()) {
                            if let Some(exterior_ring) =
                                first_poly.first().and_then(|r| r.as_array())
                            {
                                let mut outer = Vec::with_capacity(exterior_ring.len());
                                for c in exterior_ring {
                                    if let Some(pt) = Self::parse_coord(
                                        c,
                                        bbox,
                                        &transformer,
                                        &mut is_completely_outside,
                                    ) {
                                        outer.push(pt);
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
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
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
        println!(
            "[INFO] ✅ GeoJSON processado com sucesso: {} geometrias extraídas.",
            features.len()
        );
        Ok(features)
    }
}
