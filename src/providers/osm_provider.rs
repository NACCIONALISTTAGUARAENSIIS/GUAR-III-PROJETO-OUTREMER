use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::{parse_osm_data, ProcessedElement};
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;

/// Provedor Nativo do OpenStreetMap (Baseado na Overpass API)
/// Responsável por converter os "ProcessedElements" legados para as novas "Features" Governamentais.
pub struct OSMProvider {
    pub scale_h: f64,
}

impl OSMProvider {
    pub fn new(scale_h: f64) -> Self {
        Self { scale_h }
    }

    /// Classifica a feature do OSM no Grupo Semântico correto
    /// O(1) Fast-fail checks order based on statistical probability of elements in urban areas.
    fn determine_semantic_group(tags: &HashMap<String, String>) -> SemanticGroup {
        if tags.contains_key("building")
            || tags.contains_key("building:part")
            || tags.contains_key("historic")
        {
            return SemanticGroup::Building;
        }
        if tags.contains_key("highway")
            || tags.contains_key("railway")
            || tags.contains_key("aeroway")
        {
            return SemanticGroup::Highway;
        }
        if tags.contains_key("waterway")
            || tags.contains_key("water")
            || tags
                .get("natural")
                .map_or(false, |v| v == "water" || v == "bay")
        {
            return SemanticGroup::Waterway;
        }
        if tags.contains_key("landuse")
            || tags.contains_key("leisure")
            || tags.contains_key("natural")
        {
            return SemanticGroup::Landuse;
        }
        if tags.contains_key("power")
            || tags.contains_key("amenity")
            || tags.contains_key("barrier")
        {
            return SemanticGroup::Infrastructure;
        }

        SemanticGroup::Other
    }

    /// Verifica se uma série de pontos forma um polígono (mesmo se o OSM não os conectou perfeitamente).
    /// Tolerância de fechamento para compensar a escala H_SCALE e erros de mapeadores.
    #[inline]
    fn is_nearly_closed(pts: &[XZPoint]) -> bool {
        if pts.len() < 3 {
            return false;
        }
        // Safety: pts.len() >= 3 ensures [0] and last() exist without panicking
        let first = &pts[0];
        let last = &pts[pts.len() - 1];

        let dx = (first.x - last.x).abs();
        let dz = (first.z - last.z).abs();

        // Se a distância entre o primeiro e o último ponto for menor que 2 blocos, assumimos polígono fechado.
        dx <= 2 && dz <= 2
    }
}

// Implementação do Contrato Universal (Trait)
impl DataProvider for OSMProvider {
    fn priority(&self) -> u8 {
        10
    }
    fn name(&self) -> &str {
        "OpenStreetMap (Overpass API)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        // 1. Usa o sistema legado do Arnis para baixar o JSON
        // 🚨 BESM-6 TWEAK: Chamada corrigida para o novo retrieve_data.rs
        let osm_json =
            crate::retrieve_data::fetch_data_from_overpass(*bbox, false, "requests", None)
                .map_err(|e| format!("Falha na Overpass API: {}", e))?;

        // 2. Usa o sistema legado para parsear em "ProcessedElements"
        let processed_elements = parse_osm_data(osm_json, *bbox, self.scale_h, false);

        // BESM-6 Tweak: Pré-alocação exata da memória.
        // Impede a re-alocação dinâmica do vetor no Heap que estrangula o processador.
        let mut features = Vec::with_capacity(processed_elements.0.len());

        // 3. A GRANDE TRADUÇÃO: Converte Elements legados para a nova Feature Tier-Gov
        for element in processed_elements.0 {
            // 🚨 BESM-6 Tweak: Adaptação para o ARC rigoroso do ProcessedElement
            let tags_owned = element.tags().clone();
            let id = element.id();

            let geometry = match element {
                ProcessedElement::Node(node) => GeometryType::Point(XZPoint::new(node.x, node.z)),
                ProcessedElement::Way(way) => {
                    // Proteção contra Ways degeneradas do OSM (Dados Corrompidos)
                    if way.nodes.is_empty() {
                        continue;
                    }

                    // Se a via só tem 1 ponto, ela é logicamente um Node. Fazemos o downgrade gracioso.
                    if way.nodes.len() == 1 {
                        let pt = XZPoint::new(way.nodes[0].x, way.nodes[0].z);
                        GeometryType::Point(pt)
                    } else {
                        // Pré-alocação com +1 de sobra caso precisemos fechar o anel
                        let mut pts: Vec<XZPoint> = Vec::with_capacity(way.nodes.len() + 1);
                        for n in &way.nodes {
                            pts.push(XZPoint::new(n.x, n.z));
                        }

                        // Tolerância geométrica e Fechamento Automático
                        if Self::is_nearly_closed(&pts) {
                            let first = pts[0];
                            if pts.last().unwrap() != &first {
                                pts.push(first);
                            }
                            GeometryType::Polygon(pts)
                        } else {
                            GeometryType::LineString(pts) // Rua/Rio/Caminho aberto
                        }
                    }
                }
                ProcessedElement::Relation(rel) => {
                    // Relations (MultiPolygons) do OSM são complexas.
                    // Nós extraímos os "Outer rings" (Bordas) e "Inner rings" (Buracos).

                    // Pré-alocação baseada na quantidade de membros da relação
                    let mut outer = Vec::with_capacity(rel.members.len());
                    let mut inner = Vec::new(); // Inners são mais raros, instanciamento leve

                    for member in &rel.members {
                        if member.way.nodes.is_empty() {
                            continue;
                        }

                        let mut pts: Vec<XZPoint> = Vec::with_capacity(member.way.nodes.len() + 1);
                        for n in &member.way.nodes {
                            pts.push(XZPoint::new(n.x, n.z));
                        }

                        // Auto-cicatrização geométrica de Relation Rings
                        if Self::is_nearly_closed(&pts) {
                            let first = pts[0];
                            if pts.last().unwrap() != &first {
                                pts.push(first);
                            }
                        }

                        if member.role == crate::osm_parser::ProcessedMemberRole::Outer {
                            outer.push(pts);
                        } else if member.role == crate::osm_parser::ProcessedMemberRole::Inner {
                            inner.push(pts);
                        }
                    }

                    // Se a Relation for inválida ou não tiver anéis exteriores estruturais, descarta.
                    if outer.is_empty() {
                        continue;
                    }

                    GeometryType::MultiPolygon { outer, inner }
                }
            };

            let semantic_group = Self::determine_semantic_group(&tags_owned);

            // A Prioridade do OSM é 10 (Baixa). Shapefiles locais terão prioridade 1 (Alta).
            // O Feature::new irá automaticamente calcular e fazer o cache da AABB (Axis-Aligned Bounding Box).
            let feature = Feature::new(
                id,
                semantic_group,
                tags_owned,
                geometry,
                "osm".to_string(),
                10,
            );

            features.push(feature);
        }

        // Limpa qualquer capacidade em excesso deixada no Heap (Gestão Militar de RAM)
        features.shrink_to_fit();

        Ok(features)
    }
}
