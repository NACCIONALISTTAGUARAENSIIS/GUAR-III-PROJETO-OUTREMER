use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use osmpbf::{Element, ElementReader, RelMemberType};
use rustc_hash::FxHashMap; // BESM-6: Hash O(1) de extrema performance

/// Provedor PBF (Protocolbuffer Binary Format) Local de Alta Performance.
/// L� arquivos gigantescos do OSM (.osm.pbf) diretamente do SSD.
/// Implementa Streaming com Early-Z Culling BBox para não estourar a RAM.
pub struct PbfProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
}

impl PbfProvider {
    pub fn new(file_path: PathBuf, scale_h: f64, priority: u8) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
        }
    }

    /// Identifica se o elemento possui tags relevantes para a malha do Minecraft
    #[inline(always)]
    fn get_semantic_group(tags: &HashMap<String, String>) -> Option<SemanticGroup> {
        if tags.contains_key("building") || tags.contains_key("building:part") {
            return Some(SemanticGroup::Building);
        }
        if tags.contains_key("highway")
            || tags.contains_key("aeroway")
            || tags.contains_key("railway")
        {
            return Some(SemanticGroup::Highway);
        }
        if tags.contains_key("natural")
            || tags.contains_key("water")
            || tags.contains_key("waterway")
        {
            return Some(SemanticGroup::Terrain);
        }
        if tags.contains_key("landuse")
            || tags.contains_key("leisure")
            || tags.contains_key("amenity")
        {
            return Some(SemanticGroup::Landuse);
        }
        if tags.contains_key("power")
            || tags.contains_key("man_made")
            || tags.contains_key("barrier")
        {
            return Some(SemanticGroup::Infrastructure);
        }
        None
    }
}

impl DataProvider for PbfProvider {
    fn priority(&self) -> u8 {
        self.priority
    }
    fn name(&self) -> &str {
        "Local OSM PBF (Ultra-Fast Binary Stream)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] ? Iniciando motor de varredura PBF no SSD: {}",
            self.file_path.display()
        );

        let reader = ElementReader::from_path(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo PBF: {}", e))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        let mut features = Vec::new();
        let _next_id: u64 = 6_000_000_000; // Offset em u64 para evitar overflow

        // ?? BESM-6 Tweak: BBox Expandida (Buffer)
        // Expandimos a �rea de captura em ~2km (0.02 graus) para garantir que vias e
        // superquadras que come�am fora da BBox mas cruzam para dentro n�o sejam decepadas.
        let min_lat = bbox.min().lat() - 0.02;
        let max_lat = bbox.max().lat() + 0.02;
        let min_lng = bbox.min().lng() - 0.02;
        let max_lng = bbox.max().lng() + 0.02;

        // Mem�ria Cache ultrarr�pida exclusiva para elementos que passam no Culling
        let mut node_cache: FxHashMap<i64, XZPoint> = FxHashMap::default();
        let mut way_cache: FxHashMap<i64, Vec<XZPoint>> = FxHashMap::default();

        // Passagem �nica em Streaming PBF (O formato garante a ordem: Nodes -> Ways -> Relations)
        let _ = reader.for_each(|element| {
            match element {
                osmpbf::Element::DenseNode(_) => {}
                Element::Node(node) => {
                    let lat = node.lat();
                    let lon = node.lon();

                    // Culling Geogr�fico Cruel: S� guarda na RAM se estiver na regi�o do jogador
                    if lat >= min_lat && lat <= max_lat && lon >= min_lng && lon <= max_lng {
                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            let xz = transformer.transform_point(llpoint);
                            node_cache.insert(node.id(), xz);

                            let tags: HashMap<String, String> = node
                                .tags()
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                                .collect();

                            if let Some(semantic_group) = Self::get_semantic_group(&tags) {
                                features.push(Feature::new(
                                    node.id() as u64,
                                    semantic_group,
                                    tags,
                                    GeometryType::Point(xz),
                                    "OSM_PBF".to_string(),
                                    self.priority,
                                ));
                            }
                        }
                    }
                }
                Element::Way(way) => {
                    let mut coords = Vec::with_capacity(way.refs().count());
                    let mut is_completely_outside = true;

                    // Monta a geometria buscando os n�s no cache
                    for node_id in way.refs() {
                        if let Some(&xz) = node_cache.get(&node_id) {
                            coords.push(xz);
                            is_completely_outside = false; // Tem pelo menos um v�rtice no mapa
                        }
                    }

                    if coords.len() < 2 || is_completely_outside {
                        return; // Via fora do mapa ou incompleta
                    }

                    let tags: HashMap<String, String> = way
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    let is_closed = coords.first() == coords.last() && coords.len() >= 4;

                    // Armazena a geometria no cache de Vias pois uma Relation (Congresso Nacional) pode precisar dela
                    way_cache.insert(way.id(), coords.clone());

                    if let Some(semantic_group) = Self::get_semantic_group(&tags) {
                        let geometry = if is_closed
                            && (semantic_group == SemanticGroup::Building
                                || semantic_group == SemanticGroup::Landuse
                                || semantic_group == SemanticGroup::Terrain)
                        {
                            GeometryType::Polygon(coords)
                        } else {
                            GeometryType::LineString(coords)
                        };

                        features.push(Feature::new(
                            way.id() as u64,
                            semantic_group,
                            tags,
                            geometry,
                            "OSM_PBF".to_string(),
                            self.priority,
                        ));
                    }
                }
                Element::Relation(rel) => {
                    let tags: HashMap<String, String> = rel
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();

                    // Multipolygons s�o cr�ticos para constru��es massivas com p�tios internos
                    if tags.get("type").map(|s: &String| s.as_str()) == Some("multipolygon") {
                        if let Some(semantic_group) = Self::get_semantic_group(&tags) {
                            let mut outer_rings = Vec::new();
                            let mut inner_rings = Vec::new();

                            for member in rel.members() {
                                if member.member_type == RelMemberType::Way {
                                    if let Some(way_coords) = way_cache.get(&member.member_id) {
                                        if member.role().unwrap_or("") == "outer" {
                                            outer_rings.push(way_coords.clone());
                                        } else if member.role().unwrap_or("") == "inner" {
                                            inner_rings.push(way_coords.clone());
                                        }
                                    }
                                }
                            }

                            if !outer_rings.is_empty() {
                                features.push(Feature::new(
                                    rel.id() as u64,
                                    semantic_group,
                                    tags,
                                    GeometryType::MultiPolygon {
                                        outer: outer_rings,
                                        inner: inner_rings,
                                    },
                                    "OSM_PBF".to_string(),
                                    self.priority,
                                ));
                            }
                        }
                    }
                }
            }
        });

        features.shrink_to_fit();
        println!(
            "[INFO] ? Varredura PBF conclu�da em O(1): {} blocos extra�dos para o motor.",
            features.len()
        );
        Ok(features)
    }
}
