use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::{parse_osm_data, ProcessedElement};
use crate::providers::{
    DataProvider, Feature, GeometryType, NodeTagsSideChannel, SemanticGroup, NODE_TAGS_ATTR,
};
use std::collections::HashMap;

/// Provedor Nativo do OpenStreetMap (Baseado na Overpass API)
/// Responsável por converter os "ProcessedElements" legados para as novas "Features" Governamentais.
pub struct OSMProvider {
    pub scale_h: f64,
    // 🚨 BESM-6 RECONEXÃO: antes `fetch_features` chamava a Overpass API
    // incondicionalmente, hardcodando `false`/"requests" — `--offline`,
    // `--file` (JSON pré-baixado) e `--downloader` eram lidos e validados em
    // `args.rs` mas nunca chegavam até aqui.
    pub local_file: Option<String>,
    pub offline: bool,
    pub downloader: String,
}

impl OSMProvider {
    pub fn new(
        scale_h: f64,
        local_file: Option<String>,
        offline: bool,
        downloader: String,
    ) -> Self {
        Self {
            scale_h,
            local_file,
            offline,
            downloader,
        }
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
        // Ferrovia tem grupo próprio (alinhado ao KML e ao classificador
        // canônico `providers::semantic_group_from_tags`) para o merge entre
        // providers reconhecer a mesma linha de metrô vinda de fontes distintas.
        if tags.contains_key("railway") {
            return SemanticGroup::Railway;
        }
        if tags.contains_key("highway") || tags.contains_key("aeroway") {
            return SemanticGroup::Highway;
        }
        if tags.contains_key("waterway")
            || tags.contains_key("water")
            || tags
                .get("natural")
                .is_some_and(|v| v == "water" || v == "bay")
        {
            return SemanticGroup::Waterway;
        }
        if tags.contains_key("landuse")
            || tags.contains_key("leisure")
            || tags.contains_key("natural")
        {
            return SemanticGroup::Landuse;
        }
        // 🚨 RECONEXÃO: `advertising=*` (totens, outdoors, painéis MUB) caía sempre no
        // fallback `Other` — o mesmo grupo semântico genérico usado por QUALQUER outra
        // tag não reconhecida. Isso fazia o Spatial Sweeper (`ProviderManager::resolve_collisions`
        // em providers/mod.rs) tratar um outdoor real e qualquer outro elemento "Other"
        // não relacionado como colisões do MESMO grupo sempre que os AABBs se tocassem,
        // descartando o outdoor silenciosamente antes mesmo dele chegar em `main.rs`.
        // `retrieve_data.rs` já baixa `nwr["advertising"]` da Overpass — o dado sempre
        // chegou aqui, só nunca foi classificado corretamente.
        if tags.contains_key("advertising") {
            return SemanticGroup::Advertising;
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
    fn describe_sources(&self) -> Vec<String> {
        match &self.local_file {
            Some(path) => vec![path.clone()],
            None if self.offline => vec!["(offline, sem fonte OSM)".to_string()],
            // Vários servidores Overpass são tentados em sequência
            // (`retrieve_data.rs`); qual respondeu não é rastreado aqui.
            None => vec![format!("Overpass API (downloader: {})", self.downloader)],
        }
    }
    fn name(&self) -> &str {
        "OpenStreetMap (Overpass API)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        // 1. Usa o sistema legado do Arnis para baixar o JSON
        // 🚨 BESM-6 RECONEXÃO: `--file` (JSON pré-baixado) tem prioridade sobre a
        // rede quando fornecido; sem ele, `--offline` falha rápido e claro em vez
        // de tentar a Overpass API silenciosamente (que só falharia depois, com
        // timeout, num ambiente sem rede).
        let osm_json = if let Some(ref file_path) = self.local_file {
            crate::retrieve_data::fetch_data_from_file(file_path)
                .map_err(|e| format!("Falha ao carregar OSM do arquivo local: {}", e))?
        } else if self.offline {
            return Err(
                "Modo --offline ativo, mas nenhum --file (JSON OSM local) foi fornecido."
                    .to_string(),
            );
        } else {
            crate::retrieve_data::fetch_data_from_overpass(*bbox, false, &self.downloader, None)
                .map_err(|e| format!("Falha na Overpass API: {}", e))?
        };

        // 2. Usa o sistema legado para parsear em "ProcessedElements"
        let processed_elements = parse_osm_data(osm_json, *bbox, self.scale_h, false);

        // BESM-6 Tweak: Pré-alocação exata da memória.
        // Impede a re-alocação dinâmica do vetor no Heap que estrangula o processador.
        let mut features = Vec::with_capacity(processed_elements.0.len());

        // 3. A GRANDE TRADUÇÃO: Converte Elements legados para a nova Feature Tier-Gov
        for element in processed_elements.0 {
            // 🚨 BESM-6 Tweak: Adaptação para o ARC rigoroso do ProcessedElement
            let mut tags_owned = element.tags().clone();
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
                        // Ver `NODE_TAGS_ATTR`: preserva as tags de CADA nó (entrance=*,
                        // door=*, highway=crossing) num canal lateral — sem isso, um nó
                        // com `entrance=yes` dentro de uma via de prédio perdia essa tag
                        // no round-trip `ProcessedElement` → `Feature`, e
                        // `carve_and_place_door`/a detecção de faixa de pedestres nunca
                        // disparava para NENHUM dado OSM real, nem pela Overpass API
                        // padrão. `None` para nós sem tags, pra manter o JSON pequeno.
                        let mut node_tags: NodeTagsSideChannel =
                            Vec::with_capacity(way.nodes.len());
                        let mut any_node_tags = false;
                        for n in &way.nodes {
                            pts.push(XZPoint::new(n.x, n.z));
                            if n.tags.is_empty() {
                                node_tags.push(None);
                            } else {
                                any_node_tags = true;
                                node_tags.push(Some(n.tags.clone()));
                            }
                        }

                        // Tolerância geométrica e Fechamento Automático
                        let closing = if Self::is_nearly_closed(&pts) {
                            let first = pts[0];
                            if pts.last().unwrap() != &first {
                                pts.push(first);
                                node_tags.push(node_tags[0].clone());
                                any_node_tags = any_node_tags || node_tags[0].is_some();
                            }
                            true
                        } else {
                            false
                        };

                        if any_node_tags {
                            if let Ok(json) = serde_json::to_string(&node_tags) {
                                tags_owned.insert(NODE_TAGS_ATTR.to_string(), json);
                            }
                        }

                        if closing {
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
