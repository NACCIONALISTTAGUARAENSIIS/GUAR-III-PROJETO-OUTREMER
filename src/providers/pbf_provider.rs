use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use osmpbf::{Element, ElementReader, RelMemberType};
use rustc_hash::{FxHashMap, FxHashSet}; // BESM-6: Hash O(1) de extrema performance

/// Provedor PBF (Protocolbuffer Binary Format) Local de Alta Performance.
/// Lê arquivos gigantescos do OSM (.osm.pbf) diretamente do SSD.
/// 🚨 BESM-6: Arquitetura Multi-Pass para Preservação Topológica e DenseNode Unpacking.
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

    /// 🚨 EXTRAÇÃO SEMÂNTICA MULTI-TIER
    /// Corrige a miopia estrutural do antigo parser. Avalia todas as tags para inferir o grupo correto.
    #[inline(always)]
    fn get_semantic_group(tags: &HashMap<String, String>) -> SemanticGroup {
        if tags.contains_key("building") || tags.contains_key("building:part") {
            return SemanticGroup::Building;
        }
        if tags.contains_key("highway")
            || tags.contains_key("aeroway")
            || tags.contains_key("railway")
        {
            return SemanticGroup::Highway;
        }
        if tags.contains_key("natural")
            || tags.contains_key("water")
            || tags.contains_key("waterway")
        {
            return SemanticGroup::Terrain; // Matas, rios, lagos
        }
        if tags.contains_key("landuse")
            || tags.contains_key("leisure")
            || tags.contains_key("amenity")
        {
            return SemanticGroup::Landuse; // Parques, estacionamentos, superquadras
        }
        if tags.contains_key("power")
            || tags.contains_key("man_made")
            || tags.contains_key("barrier")
        {
            return SemanticGroup::Infrastructure; // Postes, muros, torres
        }

        SemanticGroup::Other
    }

    /// 🚨 HIGIENIZAÇÃO DE MEMÓRIA (O(1) RAM Control)
    /// Expurga metadados inúteis do OSM que incham a Heap em gigabytes.
    #[inline(always)]
    fn clean_tags<'a, I>(tag_iter: I) -> HashMap<String, String>
    where
        I: Iterator<Item = (&'a str, &'a str)>,
    {
        let mut clean = HashMap::new();
        for (k, v) in tag_iter {
            if k != "created_by" && k != "source" && k != "source:date" && k != "note" {
                clean.insert(k.to_string(), v.to_string());
            }
        }
        clean
    }
}

impl DataProvider for PbfProvider {
    fn priority(&self) -> u8 {
        self.priority
    }

    fn name(&self) -> &str {
        "Local OSM PBF (Multi-Pass Topological Stream)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] ⚙️ Iniciando motor de varredura PBF Topológica no SSD: {}",
            self.file_path.display()
        );

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        // BBox Expandida (Buffer Topológico Inicial)
        let min_lat = bbox.min().lat() - 0.02;
        let max_lat = bbox.max().lat() + 0.02;
        let min_lng = bbox.min().lng() - 0.02;
        let max_lng = bbox.max().lng() + 0.02;

        let mut required_nodes: FxHashSet<i64> = FxHashSet::default();
        let mut required_ways: FxHashSet<i64> = FxHashSet::default();

        // ====================================================================
        // 🚨 PASSO 1: DISCOVERY PASS (Descoberta Topológica)
        // Lemos as Vias (Ways) e Relações (Relations) primeiro.
        // O PBF armazena Nodes -> Ways -> Relations.
        // Como a biblioteca osmpbf não suporta leitura reversa limpa sem sobrecarga,
        // faremos dois leitores sequenciais rápidos.
        // ====================================================================
        println!("[INFO] 🔍 PBF Pass 1: Mapeando malha vetorial e âncoras distantes...");
        let reader_pass1 = ElementReader::from_path(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo PBF (Pass 1): {}", e))?;

        let _ = reader_pass1.for_each(|element| {
            match element {
                Element::Way(way) => {
                    // Preservamos os IDs dos nós APENAS se a via cruza nossa bounding box de forma grosseira.
                    // Para economizar ciclos no Pass 1, lemos todas as vias para garantir que não decepamos pontas.
                    // (Um algoritmo PBF otimizado usa o BBox da própria Way, mas na ausência dele, guardamos os nós).
                    for node_id in way.refs() {
                        required_nodes.insert(node_id);
                    }
                    required_ways.insert(way.id());
                }
                Element::Relation(rel) => {
                    if rel.tags().any(|(k, v)| k == "type" && v == "multipolygon") {
                        for member in rel.members() {
                            if member.member_type == RelMemberType::Way {
                                required_ways.insert(member.member_id);
                            }
                        }
                    }
                }
                _ => {} // Ignora Nodes no Pass 1
            }
        });

        // ====================================================================
        // 🚨 PASSO 2: EXTRAÇÃO CIENTÍFICA (Nodes + Geometria)
        // Agora que sabemos EXATAMENTE quais Nodes ancoram nossa cidade,
        // lemos o PBF novamente, extraindo os DenseNodes vitais e montando o quebra-cabeça.
        // ====================================================================
        println!("[INFO] 📥 PBF Pass 2: Descomprimindo DenseNodes e instanciando voxels...");
        let reader_pass2 = ElementReader::from_path(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo PBF (Pass 2): {}", e))?;

        let mut node_cache: FxHashMap<i64, XZPoint> = FxHashMap::default();
        let mut way_cache: FxHashMap<i64, Vec<XZPoint>> = FxHashMap::default();
        let mut features = Vec::new();

        let _ = reader_pass2.for_each(|element| {
            match element {
                // 🚨 A CORREÇÃO FATAL: Descompressão Ativa de DenseNodes
                osmpbf::Element::DenseNode(dense_node) => {
                    let lat = dense_node.lat();
                    let lon = dense_node.lon();
                    let id = dense_node.id();

                    // Se a coordenada está estritamente dentro da BBox Expandida
                    // OU se ela é uma âncora distante de uma via (vetor) que cruza a BBox.
                    let in_bbox = lat >= min_lat && lat <= max_lat && lon >= min_lng && lon <= max_lng;

                    if in_bbox || required_nodes.contains(&id) {
                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            let xz = transformer.transform_point(llpoint);
                            node_cache.insert(id, xz);

                            // Extração de Pontos (Semáforos, Árvores Isoladas, Lixeiras)
                            if in_bbox {
                                let tags = Self::clean_tags(dense_node.tags());
                                if !tags.is_empty() {
                                    let semantic_group = Self::get_semantic_group(&tags);
                                    if semantic_group != SemanticGroup::Other {
                                        features.push(Feature::new(
                                            id as u64,
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
                    }
                }
                // 🚨 Nós Tradicionais (Geralmente escassos em PBFs modernos, mas devem ser lidos)
                Element::Node(node) => {
                    let lat = node.lat();
                    let lon = node.lon();
                    let id = node.id();

                    let in_bbox = lat >= min_lat && lat <= max_lat && lon >= min_lng && lon <= max_lng;

                    if in_bbox || required_nodes.contains(&id) {
                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            let xz = transformer.transform_point(llpoint);
                            node_cache.insert(id, xz);

                            if in_bbox {
                                let tags = Self::clean_tags(node.tags());
                                if !tags.is_empty() {
                                    let semantic_group = Self::get_semantic_group(&tags);
                                    if semantic_group != SemanticGroup::Other {
                                        features.push(Feature::new(
                                            id as u64,
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
                    }
                }
                Element::Way(way) => {
                    if !required_ways.contains(&way.id()) { return; }

                    let mut coords = Vec::with_capacity(way.refs().count());
                    let mut is_completely_outside = true;

                    for node_id in way.refs() {
                        if let Some(&xz) = node_cache.get(&node_id) {
                            coords.push(xz);
                            // Verificamos a BBox nativa (X, Z Minecraft)
                            if xz.x >= 0 && xz.z >= 0 {
                                is_completely_outside = false;
                            }
                        }
                    }

                    if coords.len() < 2 || is_completely_outside {
                        return; // Via fora do mapa ou incompleta
                    }

                    let tags = Self::clean_tags(way.tags());
                    let is_closed = coords.first() == coords.last() && coords.len() >= 4;

                    // Armazena a geometria no cache de Vias para as Relations
                    way_cache.insert(way.id(), coords.clone());

                    if !tags.is_empty() {
                        let semantic_group = Self::get_semantic_group(&tags);
                        if semantic_group != SemanticGroup::Other {
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
                }
                Element::Relation(rel) => {
                    let tags = Self::clean_tags(rel.tags());

                    if tags.get("type").map(|s: &String| s.as_str()) == Some("multipolygon") {
                        let semantic_group = Self::get_semantic_group(&tags);
                        if semantic_group != SemanticGroup::Other {
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
            "[INFO] ✅ Varredura PBF Topológica concluída: {} blocos extraídos para o motor.",
            features.len()
        );
        Ok(features)
    }
}