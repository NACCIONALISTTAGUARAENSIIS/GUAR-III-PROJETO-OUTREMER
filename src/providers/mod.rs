pub mod citygml_provider;
pub mod csv_provider;
pub mod dem_provider;
pub mod dsm_provider;
pub mod gdf_provider;
pub mod geojson_provider;
pub mod gpkg_provider;
pub mod ifc_provider;
pub mod indoor_utility_provider; // 🚨 Tornado público para que outros módulos possam usá-lo
pub mod kml_provider;
pub mod lidar_provider;
pub mod mesh_provider;
pub mod mvt_provider;
pub mod osm_provider;
pub mod pbf_provider;
pub mod postgis_provider;
pub mod raster_provider;
pub mod tiles3d_provider;
pub mod vegetation_provider;
pub mod wfs_provider;

use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::LLBBox;

use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// ADAPTER LAYER: Contrato Universal de Provedores de Dados (Tier Governamental)
// ============================================================================

/// Grupos Semânticos evitam falsos positivos na resolução de colisões.
/// Uma via (Highway) pode cruzar um rio (Waterway), mas dois provedores
/// diferentes não devem gerar o mesmo Building no mesmo lugar.
///
/// Nem toda variante tem um provedor que a produza hoje (ex.: `Military` —
/// nenhum provider atual classifica `military=*` do OSM para este grupo);
/// mantidas como categorização reservada para providers futuros/tags ainda
/// não mapeadas, não como sobra morta de um provider removido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum SemanticGroup {
    Building,
    BuildingPart,
    Military,
    Sanitation,
    Telecom,
    PublicTransport,
    VegetationManaged,
    Subsurface,
    SupportStructure,
    Highway,
    Waterway,
    Bathymetry,
    Geology,
    Lithology,
    Railway,
    Underground,
    Historic,
    Archaeological,
    Advertising,
    Emergency,
    Maritime,
    Indoor,
    Power,
    Leisure,
    Landuse,
    Utility,
    Aeroway,
    Amenity,
    Barrier,
    PublicSafety,
    Healthcare,
    Education,
    StreetFurniture,
    AviationObstacle,
    Agricultural,
    Industrial,
    Religious,
    Logistics,
    SensorNode,
    Monument,
    TerrainDetail,
    Bridge,
    Natural,
    Flora,
    Water,
    Sewage,
    Forest,
    Riparian,
    Boundary,
    ConservationArea,

    Infrastructure, // Postes, semáforos, pontos de ônibus
    Terrain,        // Curvas de nível, pontos LiDAR
    Other,
}

/// Tipos de geometria padronizados suportados pelo motor
#[derive(Debug, Clone)]
pub enum GeometryType {
    Point(XZPoint),
    LineString(Vec<XZPoint>),
    Polygon(Vec<XZPoint>),
    MultiPolygon {
        outer: Vec<Vec<XZPoint>>,
        inner: Vec<Vec<XZPoint>>,
    },
}

impl GeometryType {
    /// Calcula a Axis-Aligned Bounding Box (AABB) da geometria.
    /// Retorna (min_x, max_x, min_z, max_z)
    pub fn calculate_aabb(&self) -> (i32, i32, i32, i32) {
        match self {
            GeometryType::Point(p) => (p.x, p.x, p.z, p.z),
            GeometryType::LineString(pts) | GeometryType::Polygon(pts) => {
                let mut min_x = i32::MAX;
                let mut max_x = i32::MIN;
                let mut min_z = i32::MAX;
                let mut max_z = i32::MIN;
                for p in pts {
                    if p.x < min_x {
                        min_x = p.x;
                    }
                    if p.x > max_x {
                        max_x = p.x;
                    }
                    if p.z < min_z {
                        min_z = p.z;
                    }
                    if p.z > max_z {
                        max_z = p.z;
                    }
                }
                (min_x, max_x, min_z, max_z)
            }
            GeometryType::MultiPolygon { outer, .. } => {
                let mut min_x = i32::MAX;
                let mut max_x = i32::MIN;
                let mut min_z = i32::MAX;
                let mut max_z = i32::MIN;
                for ring in outer {
                    for p in ring {
                        if p.x < min_x {
                            min_x = p.x;
                        }
                        if p.x > max_x {
                            max_x = p.x;
                        }
                        if p.z < min_z {
                            min_z = p.z;
                        }
                        if p.z > max_z {
                            max_z = p.z;
                        }
                    }
                }
                (min_x, max_x, min_z, max_z)
            }
        }
    }
}

/// 🚨 RECONEXÃO: classifica o valor bruto de `TIPO_VIA`/`CLASSE_VIA`/`HIERARQUIA`
/// (colunas comuns de shapefile/GeoJSON/GPKG/PostGIS do SEDUH/SITURB) num
/// `highway=*` real, em vez do fallback cego para `residential` que
/// `gdf_provider.rs`, `geojson_provider.rs`, `gpkg_provider.rs` e
/// `postgis_provider.rs` usavam todos independentemente. Sem isso, o Eixo
/// Rodoviário (Eixão), o Eixo Monumental, a EPTG e qualquer rodovia (BR-020,
/// DF-001...) vindos de dado governamental (em vez do OSM) eram achatados na
/// mesma classe de rua de bairro — `highways.rs` tem lógica real de
/// largura/pista por `highway=*` que nunca via essa distinção. Heurística por
/// palavra-chave (não há um dicionário de domínio oficial disponível aqui);
/// compartilhada entre os 4 provedores para não divergir.
pub fn classify_highway_from_tipo_via(raw: &str) -> &'static str {
    let classe = raw.to_lowercase();
    if classe.contains("eixo") || classe.contains("rodovia") {
        "trunk"
    } else if classe.contains("arterial") {
        "primary"
    } else if classe.contains("coletora") {
        "secondary"
    } else if classe.contains("ciclov") {
        "cycleway"
    } else if classe.contains("pedestr") || classe.contains("calçad") {
        "pedestrian"
    } else if classe.contains("vicinal") || classe.contains("rural") {
        "unclassified"
    } else {
        "residential"
    }
}

/// 🚨 RECONEXÃO: canal lateral para tags POR NÓ dentro de uma `Feature` (way/anel).
/// `GeometryType::LineString`/`Polygon`/`MultiPolygon` só guardam coordenadas
/// (`Vec<XZPoint>`) — não há onde armazenar a tag individual de um nó (`entrance=yes`,
/// `door=yes`, `highway=crossing`) dentro da geometria em si. Sem isso, TODO nó de
/// TODA via OSM perdia suas tags no round-trip `ProcessedElement` → `Feature` →
/// `into_processed_element` — mesmo vindo da Overpass API pelo caminho padrão, não só
/// de provedores alternativos. Na prática: `carve_and_place_door` (portas/entradas
/// mapeadas no OSM, ligado a `buildings.rs`) e a detecção de faixa de pedestres
/// (`highway=crossing`, em `highways.rs`) nunca disparavam para nenhum prédio ou rua
/// gerado a partir de dados OSM reais — universal, não um caso raro.
///
/// `OSMProvider::fetch_features` serializa (via `serde_json`) um
/// `Vec<Option<HashMap<String,String>>>` posicional (um item por ponto da geometria,
/// `None` se o nó não tinha tags) sob esta chave, dentro de `Feature::attributes`.
/// `Feature::into_processed_element` lê essa chave (se presente) para restaurar as
/// tags de cada nó reconstruído, e remove a chave antes de repassar as tags restantes
/// como as tags da própria via/relação — nunca vaza como uma tag "de verdade".
pub const NODE_TAGS_ATTR: &str = "__osm_node_tags__";

/// Formato serializado sob `NODE_TAGS_ATTR`: um item por ponto da geometria da
/// `Feature` (mesma ordem/índice), `None` para nós sem tags.
pub type NodeTagsSideChannel = Vec<Option<HashMap<String, String>>>;

/// A "Feature" é a unidade universal de dados do motor.
/// Um provedor OSM, Shapefile ou GeoJSON irá cuspir Features.
#[derive(Debug, Clone)]
pub struct Feature {
    /// ID único gerado pelo provedor para evitar colisões
    pub id: u64,
    /// Categoria lógica para impedir sobreposição indevida
    pub semantic_group: SemanticGroup,
    /// Tags genéricas (Chave, Valor) - Pode ser OSM tags, ou atributos do Shapefile
    pub attributes: HashMap<String, String>,
    /// Geometria limpa e já projetada no sistema Minecraft (SIRGAS2000 ou UTM -> X,Z)
    pub geometry: GeometryType,
    /// Bounding Box em cache (min_x, max_x, min_z, max_z)
    pub aabb: (i32, i32, i32, i32),
    /// Origem do dado (ex: "osm", "gdf_shapefile", "caesb_wfs")
    pub source: String,
    /// Prioridade (menor número = maior prioridade no momento do merge). Ex: Shapefile(1) > OSM(10)
    pub priority: u8,
}

impl Feature {
    /// Construtor de Feature que já calcula e faz cache do AABB automaticamente
    pub fn new(
        id: u64,
        semantic_group: SemanticGroup,
        attributes: HashMap<String, String>,
        geometry: GeometryType,
        source: String,
        priority: u8,
    ) -> Self {
        let aabb = geometry.calculate_aabb();
        Self {
            id,
            semantic_group,
            attributes,
            geometry,
            aabb,
            source,
            priority,
        }
    }

    pub fn get_tag(&self, key: &str) -> Option<&String> {
        self.attributes.get(key)
    }

    /// Mutador genérico de atributos pós-construção; nenhum provider precisa
    /// dele hoje (todos montam `attributes` de uma vez no construtor), mas é
    /// API pública real para quem for adaptar/enriquecer uma `Feature` depois.
    #[allow(dead_code)]
    pub fn set_tag(&mut self, key: &str, value: &str) {
        self.attributes.insert(key.to_string(), value.to_string());
    }

    /// Verifica interseção básica de Bounding Box com outra Feature
    pub fn intersects_aabb(&self, other: &Feature) -> bool {
        let (x1_min, x1_max, z1_min, z1_max) = self.aabb;
        let (x2_min, x2_max, z2_min, z2_max) = other.aabb;

        !(x1_max < x2_min || x1_min > x2_max || z1_max < z2_min || z1_min > z2_max)
    }
}

/// O Trait (Interface) que todo provedor de dados deve implementar.
/// Requer Send + Sync para habilitar requisições assíncronas no futuro.
pub trait DataProvider: Send + Sync {
    fn name(&self) -> &str;
    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String>;
    // 🚨 ADICIONADO: Acesso genérico à prioridade para o Spatial Sweeper do Manager.
    // `resolve_collisions` hoje usa `Feature.priority` (copiado do provider na
    // construção) em vez de chamar isto de volta no trait object — mantido como
    // API de introspecção real (ex.: listar/logar a prioridade de cada provider
    // registrado) mesmo sem um chamador interno agora.
    #[allow(dead_code)]
    fn priority(&self) -> u8;
    // 🚨 BESM-6: Auditoria de fontes de dados — pedido explícito do usuário
    // pra saber não só QUAL provider gerou algo (ver `provenance.rs`), mas o
    // NOME EXATO do arquivo (ou endpoint, pra provedores de rede) usado.
    // Default vazio: providers que não sobrescrevem simplesmente não
    // reportam nada (nunca um erro de compilação por não implementar isto).
    // Providers baseados em arquivo local devolvem o caminho exato (o mesmo
    // que já guardam internamente pra abrir o arquivo); providers de rede
    // devolvem o endpoint configurado — ver o comentário em cada override
    // pra limitações reais (ex.: `OSMProvider` tenta vários servidores
    // Overpass em sequência; não rastreamos qual deles respondeu de fato).
    fn describe_sources(&self) -> Vec<String> {
        Vec::new()
    }
}

// ============================================================================
// O GERENCIADOR DE PROVEDORES (Provider Manager)
// ============================================================================

pub struct ProviderManager {
    providers: Vec<Box<dyn DataProvider>>,
}

impl Default for ProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register_provider(&mut self, provider: Box<dyn DataProvider>) {
        self.providers.push(provider);
    }

    pub fn fetch_all(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        let mut all_features = Vec::new();

        for provider in &self.providers {
            println!("[INFO] Motor iniciando provedor: {}", provider.name());
            for source in provider.describe_sources() {
                println!("       ↳ fonte: {}", source);
            }
            match provider.fetch_features(bbox) {
                Ok(mut features) => {
                    println!(
                        " -> {} features extraídas de {}.",
                        features.len(),
                        provider.name()
                    );
                    all_features.append(&mut features);
                }
                Err(e) => {
                    eprintln!(
                        "[AVISO] Timeout ou Falha Crítica no provedor {}: {}",
                        provider.name(),
                        e
                    );
                }
            }
        }

        println!(
            "[INFO] Merge Intelligence: Resolvendo colisões espaciais (Tier Governamental)..."
        );
        let merged_features = self.resolve_collisions(all_features);
        println!(
            "[INFO] Dados governamentais e públicos fundidos com sucesso. Total: {} features.",
            merged_features.len()
        );

        Ok(merged_features)
    }

    /// Deduplicação espacial ENTRE provedores (Spatial Sweeper O(N) por baldes).
    ///
    /// 🚨 CORREÇÃO DE QUALIDADE (defeito sistêmico): a versão anterior descartava
    /// QUALQUER feature cujo AABB tocasse o AABB de outra já aceita do mesmo
    /// `SemanticGroup` — sem olhar de onde cada uma veio. Com um único provedor
    /// (o caso padrão: só OSM), isso significava que de duas ruas que se cruzam
    /// só a primeira sobrevivia, que dois prédios vizinhos com AABBs encostados
    /// perdiam um deles, e que polígonos de uso do solo que se tocam eram
    /// dizimados. O objetivo documentado da função ("dois provedores diferentes
    /// não devem gerar o mesmo Building no mesmo lugar") nunca exigiu isso.
    ///
    /// Regra atual, restrita ao caso que a função existe para resolver:
    /// uma feature só é descartada se uma feature JÁ ACEITA
    ///   1. veio de OUTRA fonte (`source` diferente),
    ///   2. tem prioridade ESTRITAMENTE maior (número menor),
    ///   3. é do mesmo `SemanticGroup`, e
    ///   4. cobre pelo menos `MIN_COVERAGE_TO_SUPERSEDE` do AABB da candidata
    ///      (ou seja, é de fato "o mesmo objeto" e não um vizinho encostado).
    ///
    /// Duas features da mesma fonte nunca colidem entre si — o próprio provedor
    /// já é a autoridade sobre o que ele emite.
    fn resolve_collisions(&self, mut features: Vec<Feature>) -> Vec<Feature> {
        // Ordena garantindo que os dados de Shapefile do GDF (priority 1) sejam
        // processados primeiro. `sort_by_key` é estável: a ordem original dentro
        // de uma mesma prioridade (ordem de emissão do provedor) é preservada.
        features.sort_by_key(|f| f.priority);

        let mut accepted_features: Vec<Feature> = Vec::with_capacity(features.len());
        let mut superseded = 0usize;

        // Grid de indexação espacial (Baldes de 256x256 blocos)
        const GRID_SIZE: i32 = 256;
        let mut spatial_grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

        for new_feature in features {
            let dedup_eligible = new_feature.semantic_group != SemanticGroup::Terrain
                && new_feature.semantic_group != SemanticGroup::Infrastructure;

            let mut is_superseded = false;

            if dedup_eligible {
                let (min_x, max_x, min_z, max_z) = new_feature.aabb;
                let min_grid_x = min_x.div_euclid(GRID_SIZE);
                let max_grid_x = max_x.div_euclid(GRID_SIZE);
                let min_grid_z = min_z.div_euclid(GRID_SIZE);
                let max_grid_z = max_z.div_euclid(GRID_SIZE);

                'collision_check: for gx in min_grid_x..=max_grid_x {
                    for gz in min_grid_z..=max_grid_z {
                        if let Some(cell_indices) = spatial_grid.get(&(gx, gz)) {
                            for &idx in cell_indices {
                                if should_supersede(&accepted_features[idx], &new_feature) {
                                    is_superseded = true;
                                    break 'collision_check;
                                }
                            }
                        }
                    }
                }
            }

            if is_superseded {
                superseded += 1;
                continue;
            }

            let accepted_idx = accepted_features.len();
            if dedup_eligible {
                let (min_x, max_x, min_z, max_z) = new_feature.aabb;
                for gx in min_x.div_euclid(GRID_SIZE)..=max_x.div_euclid(GRID_SIZE) {
                    for gz in min_z.div_euclid(GRID_SIZE)..=max_z.div_euclid(GRID_SIZE) {
                        spatial_grid.entry((gx, gz)).or_default().push(accepted_idx);
                    }
                }
            }
            accepted_features.push(new_feature);
        }

        if superseded > 0 {
            println!(
                "[INFO] Merge Intelligence: {} feature(s) de menor prioridade substituídas por dado de provedor prioritário.",
                superseded
            );
        }

        accepted_features.shrink_to_fit();
        accepted_features
    }

    /// Lista, por provedor registrado, as fontes exatas (arquivos/endpoints)
    /// que ele usa — alimenta a auditoria de proveniência (`provenance.rs`).
    pub fn describe_registered_sources(&self) -> Vec<(String, Vec<String>)> {
        self.providers
            .iter()
            .map(|p| (p.name().to_string(), p.describe_sources()))
            .collect()
    }
}

/// Fração mínima do AABB da candidata que precisa estar coberta pelo AABB da
/// feature já aceita para que a candidata seja considerada "o mesmo objeto"
/// (e não um vizinho encostado) e descartada.
const MIN_COVERAGE_TO_SUPERSEDE: f64 = 0.5;

/// Fração da área do AABB `inner` coberta pela interseção com `outer`, em
/// blocos inclusivos (um ponto tem área 1, então também funciona para nós).
/// 0.0 quando não há interseção.
pub(crate) fn aabb_coverage_ratio(inner: (i32, i32, i32, i32), outer: (i32, i32, i32, i32)) -> f64 {
    let (a_min_x, a_max_x, a_min_z, a_max_z) = inner;
    let (b_min_x, b_max_x, b_min_z, b_max_z) = outer;

    let ix = (a_max_x.min(b_max_x) as i64 - a_min_x.max(b_min_x) as i64 + 1).max(0);
    let iz = (a_max_z.min(b_max_z) as i64 - a_min_z.max(b_min_z) as i64 + 1).max(0);
    if ix == 0 || iz == 0 {
        return 0.0;
    }
    let inner_area =
        ((a_max_x as i64 - a_min_x as i64 + 1) * (a_max_z as i64 - a_min_z as i64 + 1)).max(1);
    (ix * iz) as f64 / inner_area as f64
}

/// Ver `ProviderManager::resolve_collisions` para a regra completa.
fn should_supersede(accepted: &Feature, candidate: &Feature) -> bool {
    accepted.source != candidate.source
        && accepted.priority < candidate.priority
        && accepted.semantic_group == candidate.semantic_group
        && accepted.intersects_aabb(candidate)
        && aabb_coverage_ratio(candidate.aabb, accepted.aabb) >= MIN_COVERAGE_TO_SUPERSEDE
}

// ============================================================================
// PONTE BESM-6 (Tradução Reversa para Compatibilidade Legada)
// ============================================================================
use crate::osm_parser::{
    ProcessedElement, ProcessedMember, ProcessedMemberRole, ProcessedNode, ProcessedRelation,
    ProcessedWay,
};

impl Feature {
    /// Tradução Reversa: Converte a Feature Otimizada do Motor de volta para o formato
    /// legado do Arnis. Isso impede que os módulos de geração originais quebrem.
    pub fn into_processed_element(self) -> ProcessedElement {
        let mut fake_node_id = self.id.wrapping_mul(1000);

        match self.geometry {
            GeometryType::Point(pt) => ProcessedElement::Node(ProcessedNode {
                id: self.id,
                x: pt.x,
                z: pt.z,
                tags: self.attributes,
            }),
            GeometryType::LineString(pts) | GeometryType::Polygon(pts) => {
                // Ver `NODE_TAGS_ATTR`: restaura as tags por nó (entrance/door/
                // highway=crossing) que `OSMProvider::fetch_features` preservou
                // no canal lateral — sem isso, todo nó reconstruído aqui nascia
                // sem tags, mesmo quando o nó original as tinha.
                let mut attributes = self.attributes;
                let node_tags = attributes
                    .remove(NODE_TAGS_ATTR)
                    .and_then(|json| serde_json::from_str::<NodeTagsSideChannel>(&json).ok());

                let nodes = pts
                    .into_iter()
                    .enumerate()
                    .map(|(i, pt)| {
                        fake_node_id = fake_node_id.wrapping_add(1);
                        let tags = node_tags
                            .as_ref()
                            .and_then(|v| v.get(i))
                            .and_then(|opt| opt.clone())
                            .unwrap_or_default();
                        ProcessedNode {
                            id: fake_node_id,
                            x: pt.x,
                            z: pt.z,
                            tags,
                        }
                    })
                    .collect();

                // 🚨 O REVESTIMENTO ARC É OBRIGATÓRIO AQUI!
                ProcessedElement::Way(Arc::new(ProcessedWay {
                    id: self.id,
                    nodes,
                    tags: attributes,
                }))
            }
            GeometryType::MultiPolygon { outer, inner } => {
                let mut members = Vec::new();

                for ring in outer {
                    let mut nodes = Vec::new();
                    for pt in ring {
                        fake_node_id = fake_node_id.wrapping_add(1);
                        nodes.push(ProcessedNode {
                            id: fake_node_id,
                            x: pt.x,
                            z: pt.z,
                            tags: HashMap::new(),
                        });
                    }
                    let way_id = fake_node_id.wrapping_add(100000);
                    members.push(ProcessedMember {
                        role: ProcessedMemberRole::Outer,
                        // 🚨 O REVESTIMENTO ARC NA RELATION (Apenas nas ways que compõem os membros)
                        way: Arc::new(ProcessedWay {
                            id: way_id,
                            nodes,
                            tags: HashMap::new(),
                        }),
                    });
                }

                for ring in inner {
                    let mut nodes = Vec::new();
                    for pt in ring {
                        fake_node_id = fake_node_id.wrapping_add(1);
                        nodes.push(ProcessedNode {
                            id: fake_node_id,
                            x: pt.x,
                            z: pt.z,
                            tags: HashMap::new(),
                        });
                    }
                    let way_id = fake_node_id.wrapping_add(100000);
                    members.push(ProcessedMember {
                        role: ProcessedMemberRole::Inner,
                        // 🚨 O REVESTIMENTO ARC NA RELATION
                        way: Arc::new(ProcessedWay {
                            id: way_id,
                            nodes,
                            tags: HashMap::new(),
                        }),
                    });
                }

                // 🚨 O REVESTIMENTO ARC É OBRIGATÓRIO AQUI TAMBÉM!
                ProcessedElement::Relation(Arc::new(ProcessedRelation {
                    id: self.id,
                    members,
                    tags: self.attributes,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn rect(
        id: u64,
        group: SemanticGroup,
        source: &str,
        priority: u8,
        x0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
    ) -> Feature {
        let ring = vec![
            XZPoint::new(x0, z0),
            XZPoint::new(x1, z0),
            XZPoint::new(x1, z1),
            XZPoint::new(x0, z1),
            XZPoint::new(x0, z0),
        ];
        Feature::new(
            id,
            group,
            HashMap::new(),
            GeometryType::Polygon(ring),
            source.to_string(),
            priority,
        )
    }

    fn line(id: u64, source: &str, priority: u8, pts: &[(i32, i32)]) -> Feature {
        let pts = pts.iter().map(|&(x, z)| XZPoint::new(x, z)).collect();
        Feature::new(
            id,
            SemanticGroup::Highway,
            HashMap::new(),
            GeometryType::LineString(pts),
            source.to_string(),
            priority,
        )
    }

    fn ids(features: &[Feature]) -> Vec<u64> {
        let mut v: Vec<u64> = features.iter().map(|f| f.id).collect();
        v.sort_unstable();
        v
    }

    /// Regressão do defeito sistêmico: duas ruas OSM que se cruzam têm AABBs
    /// que se intersectam — a segunda era descartada e a malha viária inteira
    /// virava um conjunto de vias isoladas.
    #[test]
    fn crossing_highways_from_same_provider_are_both_kept() {
        let manager = ProviderManager::new();
        let features = vec![
            line(1, "osm", 10, &[(0, 50), (100, 50)]),
            line(2, "osm", 10, &[(50, 0), (50, 100)]),
            line(3, "osm", 10, &[(0, 0), (100, 100)]),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1, 2, 3]);
    }

    #[test]
    fn adjacent_buildings_from_same_provider_are_both_kept() {
        let manager = ProviderManager::new();
        let features = vec![
            rect(1, SemanticGroup::Building, "osm", 10, 0, 0, 10, 10),
            rect(2, SemanticGroup::Building, "osm", 10, 10, 0, 20, 10),
            rect(3, SemanticGroup::Building, "osm", 10, 5, 5, 15, 15),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1, 2, 3]);
    }

    /// O caso que a função existe para resolver: o lote oficial do GDF
    /// (prioridade 1) substitui o mesmo prédio desenhado no OSM (prioridade 10).
    #[test]
    fn higher_priority_provider_supersedes_same_object_from_lower_priority() {
        let manager = ProviderManager::new();
        let features = vec![
            rect(10, SemanticGroup::Building, "osm", 10, 0, 0, 10, 10),
            rect(
                1,
                SemanticGroup::Building,
                "gdf_shapefile",
                1,
                -1,
                -1,
                11,
                11,
            ),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1]);
    }

    #[test]
    fn partial_overlap_below_threshold_keeps_both() {
        let manager = ProviderManager::new();
        let features = vec![
            rect(10, SemanticGroup::Building, "osm", 10, 0, 0, 10, 10),
            // Cobre só ~25% do AABB do prédio OSM: vizinho, não o mesmo objeto.
            rect(1, SemanticGroup::Building, "gdf_shapefile", 1, 5, 5, 20, 20),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1, 10]);
    }

    #[test]
    fn different_semantic_group_never_collides() {
        let manager = ProviderManager::new();
        let features = vec![
            rect(10, SemanticGroup::Building, "osm", 10, 0, 0, 10, 10),
            rect(
                1,
                SemanticGroup::Landuse,
                "gdf_shapefile",
                1,
                -50,
                -50,
                50,
                50,
            ),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1, 10]);
    }

    #[test]
    fn equal_priority_from_different_sources_keeps_both() {
        let manager = ProviderManager::new();
        let features = vec![
            rect(1, SemanticGroup::Building, "gdf_shapefile", 1, 0, 0, 10, 10),
            rect(2, SemanticGroup::Building, "gdf_geojson", 1, 0, 0, 10, 10),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1, 2]);
    }

    #[test]
    fn point_features_use_unit_area_for_coverage() {
        assert_eq!(aabb_coverage_ratio((5, 5, 5, 5), (0, 10, 0, 10)), 1.0);
        assert_eq!(aabb_coverage_ratio((5, 5, 5, 5), (6, 10, 6, 10)), 0.0);
        assert!((aabb_coverage_ratio((0, 9, 0, 9), (5, 20, 0, 9)) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn negative_coordinates_bucket_consistently() {
        // Guará/Taguatinga ficam a oeste do Marco Zero: X negativo é o caso normal.
        let manager = ProviderManager::new();
        let features = vec![
            rect(
                10,
                SemanticGroup::Building,
                "osm",
                10,
                -300,
                -300,
                -290,
                -290,
            ),
            rect(
                1,
                SemanticGroup::Building,
                "gdf_shapefile",
                1,
                -301,
                -301,
                -289,
                -289,
            ),
        ];
        let kept = manager.resolve_collisions(features);
        assert_eq!(ids(&kept), vec![1]);
    }
}
