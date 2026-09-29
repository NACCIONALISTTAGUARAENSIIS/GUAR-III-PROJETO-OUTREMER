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

use std::collections::{HashMap, HashSet};
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

/// Classificador semântico CANÔNICO a partir de tags no dialeto OSM.
///
/// Cada provider mantém o SEU tradutor de atributos (o dialeto do DBF do
/// SITURB, do GeoJSON curado, do GeoPackage, do PostGIS — são conhecimento
/// de domínio de cada fonte e ficam onde estão). O que precisa ser comum é a
/// resposta a "que TIPO de objeto essas tags descrevem": o
/// `ProviderManager::resolve_collisions` só deduplica entre providers quando o
/// grupo semântico coincide, então uma via férrea do KML e a mesma via no OSM
/// precisam cair no MESMO grupo para o dado prioritário substituir o outro.
/// Antes cada provider tinha sua própria tabela (o OSM punha `railway` em
/// `Highway`, o KML em `Railway`, o PBF punha `natural` em `Terrain` e o
/// GeoJSON em `Natural`) e o merge nunca os enxergava como o mesmo objeto.
pub fn semantic_group_from_tags(tags: &HashMap<String, String>) -> SemanticGroup {
    if tags.contains_key("building")
        || tags.contains_key("building:part")
        || tags.contains_key("historic")
    {
        return SemanticGroup::Building;
    }
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
            .is_some_and(|v| v == "water" || v == "bay" || v == "wetland")
    {
        return SemanticGroup::Waterway;
    }
    if tags.contains_key("natural") {
        return SemanticGroup::Natural;
    }
    if tags.contains_key("landuse") || tags.contains_key("leisure") {
        return SemanticGroup::Landuse;
    }
    if tags.contains_key("advertising") {
        return SemanticGroup::Advertising;
    }
    if tags.contains_key("power")
        || tags.contains_key("amenity")
        || tags.contains_key("barrier")
        || tags.contains_key("man_made")
    {
        return SemanticGroup::Infrastructure;
    }
    SemanticGroup::Other
}

/// Decide o que uma coluna de USO DO SOLO (`USO`, `USO_SOLO`, `DESTINACAO`,
/// `PN_USO`, `TIPO_LOTE`…) significa: um **prédio** só quando há evidência
/// estrutural no mesmo registro (pavimentos/altura) ou a camada é de
/// edificações; caso contrário é um **lote** — `landuse=*`.
///
/// 🚨 CONFLITO REAL ENTRE PROVIDERS: os quatro tradutores GDF (Shapefile,
/// GeoJSON, GeoPackage, PostGIS) mapeavam `USO=Residencial` direto para
/// `building=residential`. A camada "Lotes Registrados" do SITURB (a mais comum
/// de todas) virava um prédio por LOTE — quadras inteiras de caixas — e, com
/// prioridade 1 e grupo `Building`, esses lotes ainda substituíam no merge os
/// prédios reais do OSM que cobriam. Devolve `(chave, valor)`.
pub fn uso_to_tag(uso_raw: &str, has_structure: bool) -> (&'static str, &'static str) {
    let uso = uso_raw.to_lowercase();
    let is_building = has_structure || uso.contains("edific") || uso.contains("constru");
    let key = if is_building { "building" } else { "landuse" };
    let value = if uso.contains("comercial") || uso.contains("commercial") {
        if is_building {
            "commercial"
        } else {
            "retail"
        }
    } else if uso.contains("residencial") || uso.contains("residential") {
        "residential"
    } else if uso.contains("institucional")
        || uso.contains("equipamento")
        || uso.contains("civic")
        || uso.starts_with("inst")
    {
        if is_building {
            "civic"
        } else {
            "institutional"
        }
    } else if uso.contains("industrial") {
        "industrial"
    } else if is_building {
        "yes"
    } else {
        "residential"
    };
    (key, value)
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
        let mut replaced_by_structure = 0usize;

        // Grid de indexação espacial (Baldes de 256x256 blocos) — só áreas e
        // pontos. Linhas têm índice e regra próprios (`LineIndex`).
        const GRID_SIZE: i32 = 256;
        let mut spatial_grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        let mut lines = LineIndex::default();
        // Linhas aceitas que depois foram substituídas por uma via com estrutura
        // (ponte/túnel/`layer`) de menor prioridade — ver `LineIndex`.
        let mut removed: HashSet<usize> = HashSet::new();

        for new_feature in features {
            // `Infrastructure` (estacionamento, cerca/muro, poste, equipamento)
            // era isenta — a regra por AABB faria a caixa de um estacionamento
            // engolir o poste dentro dele. Com linhas à parte e a comparação
            // restrita ao MESMO tipo de objeto e de geometria
            // (`should_supersede`), ela deduplica como os outros grupos: no
            // Guará, 76 dos 105 estacionamentos do OSM repetiam um do GDF (62%
            // da área), e as duas grades de vagas, defasadas, viravam listras.
            let dedup_eligible = new_feature.semantic_group != SemanticGroup::Terrain;

            // LINHAS (vias, trilhos, cercas, cursos d'água): a regra por AABB não
            // serve. A caixa de uma avenida diagonal cobre um bairro inteiro —
            // medido no Guará, ela descartava 1.711 vias do OSM que NÃO eram a
            // mesma rua (779 de serviço, 546 calçadas, trechos da EPTG/EPIA) — e
            // uma rua longa do OSM quase nunca tem a caixa ≥50% dentro da de um
            // trecho curto do eixo do GDF, então 457 ruas saíam desenhadas duas
            // vezes. Linha é "a mesma" que outra quando CORRE junto dela.
            if dedup_eligible && line_points(&new_feature).is_some() {
                match lines.conflate(&new_feature, &accepted_features, &removed) {
                    LineMatch::Distinct => {}
                    LineMatch::Duplicate(targets) if has_structure(&new_feature) => {
                        // O eixo do GDF não sabe representar tabuleiro nem túnel:
                        // a via com estrutura fica e substitui os eixos que cobre.
                        for (idx, reverse_coverage) in targets {
                            if reverse_coverage >= LINE_MIN_COVERAGE
                                && !has_structure(&accepted_features[idx])
                            {
                                removed.insert(idx);
                                replaced_by_structure += 1;
                            }
                        }
                    }
                    LineMatch::Duplicate(targets) => {
                        for (idx, reverse_coverage) in targets {
                            // Só empresta semântica a quem ela cobre de fato: a rua
                            // transversal numa interseção não herda o nome.
                            if reverse_coverage >= LINE_ENRICH_COVERAGE {
                                inherit_line_semantics(&mut accepted_features[idx], &new_feature);
                            }
                        }
                        superseded += 1;
                        continue;
                    }
                }
                lines.insert(accepted_features.len(), &new_feature);
                accepted_features.push(new_feature);
                continue;
            }

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
                                    // 🚨 O vencedor fica com a GEOMETRIA (é para isso que
                                    // ele tem prioridade: contorno LiDAR/CityGML exato),
                                    // mas herda a SEMÂNTICA que não tem. Antes o prédio
                                    // do LiDAR (só `building=yes`) apagava nome,
                                    // pavimentos, amenity/shop do prédio OSM que cobria —
                                    // e a mata LiDAR apagava `leaf_type`/`name` do
                                    // `natural=wood` do OSM: a fonte que não sabe o que
                                    // vê "comia" a que sabe.
                                    inherit_missing_semantics(
                                        &mut accepted_features[idx],
                                        &new_feature,
                                    );
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
        if replaced_by_structure > 0 {
            println!(
                "[INFO] Merge Intelligence: {} eixo(s) substituídos por via com ponte/túnel de outro provedor.",
                replaced_by_structure
            );
        }

        let mut accepted_features: Vec<Feature> = accepted_features
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !removed.contains(i))
            .map(|(_, f)| f)
            .collect();
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
/// Chaves que descrevem a ORIGEM/medição de uma feature, não o objeto — nunca
/// são herdadas (a proveniência do vencedor tem que continuar verdadeira).
///
/// `NODE_TAGS_ATTR` também fica de fora: são tags POR NÓ, casadas pela posição
/// do nó na geometria (portas, travessias). Herdadas por um vencedor de outra
/// geometria, cairiam nos nós errados dele.
const NON_INHERITABLE_KEYS: &[&str] = &[
    "source",
    "density",
    "layer_name",
    "id",
    "osm_id",
    NODE_TAGS_ATTR,
];

/// Copia para `winner` toda chave semântica de `loser` que o vencedor não
/// tem. O vencedor mantém geometria, prioridade e tudo que já declarava; a
/// origem herdada fica registrada em `merged:source` para a auditoria.
fn inherit_missing_semantics(winner: &mut Feature, loser: &Feature) {
    let mut inherited = 0usize;
    for (k, v) in &loser.attributes {
        if NON_INHERITABLE_KEYS.contains(&k.as_str())
            || k.starts_with("merged:")
            || winner.attributes.contains_key(k)
        {
            continue;
        }
        winner.attributes.insert(k.clone(), v.clone());
        inherited += 1;
    }
    if inherited > 0 {
        winner
            .attributes
            .entry("merged:source".to_string())
            .or_insert_with(|| loser.source.clone());
    }
}

// ============================================================================
// CONFLAÇÃO DE LINHAS ENTRE PROVEDORES
// ============================================================================

/// Distância (blocos) abaixo da qual uma linha "corre junto" de outra. Medido
/// no Guará I+II (OSM × eixo de arruamento do GDF, amostras a cada 4 blocos):
/// a mesma rua fica a 1,2–2,0 blocos (mediana, residential a primary); vias
/// paralelas distintas (as duas pistas da EPTG, marginal × expressa) ficam a
/// 10+ blocos; EPTG/EPIA/serviço/calçadas, que não existem no GDF, a 20–50.
const LINE_TOLERANCE: f64 = 6.0;
/// Mesma ideia para linhas que não são vias (cercas, muros, cabos). Medido no
/// Guará (barreiras do OSM × "Cercas e Muros" do GDF): o mesmo muro fica a
/// 0–2 blocos (mediana 2,5); muros paralelos de lotes vizinhos podem estar a
/// poucos metros, então a tolerância é menor que a das vias.
const LINE_TOLERANCE_OTHER: f64 = 4.0;

fn line_tolerance(group: SemanticGroup) -> f64 {
    match group {
        SemanticGroup::Highway | SemanticGroup::Railway | SemanticGroup::Waterway => LINE_TOLERANCE,
        _ => LINE_TOLERANCE_OTHER,
    }
}
/// Fração mínima do comprimento da candidata junto de linhas aceitas para ela
/// ser a mesma (uma rua que só CRUZA outra fica junto dela em poucas amostras).
const LINE_MIN_COVERAGE: f64 = 0.7;
/// Fração mínima de uma linha aceita coberta pela duplicata para herdar dela.
const LINE_ENRICH_COVERAGE: f64 = 0.5;
const LINE_SAMPLE_STEP: f64 = 4.0;
const LINE_CELL: i32 = 32;

fn line_points(feature: &Feature) -> Option<&[XZPoint]> {
    match &feature.geometry {
        GeometryType::LineString(pts) if pts.len() >= 2 => Some(pts),
        _ => None,
    }
}

/// Ponte, túnel ou nível diferente do chão.
fn has_structure(feature: &Feature) -> bool {
    let tag = |k: &str| feature.get_tag(k).map(|s| s.as_str());
    matches!(tag("bridge"), Some(v) if v != "no")
        || matches!(tag("tunnel"), Some(v) if v != "no")
        || matches!(tag("layer"), Some(v) if v.trim() != "0")
}

fn samples_along(pts: &[XZPoint]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for pair in pts.windows(2) {
        let (ax, az) = (pair[0].x as f64, pair[0].z as f64);
        let (bx, bz) = (pair[1].x as f64, pair[1].z as f64);
        let len = ((bx - ax).powi(2) + (bz - az).powi(2)).sqrt();
        let n = ((len / LINE_SAMPLE_STEP) as usize).max(1);
        for k in 0..n {
            let t = k as f64 / n as f64;
            out.push((ax + t * (bx - ax), az + t * (bz - az)));
        }
    }
    let last = pts[pts.len() - 1];
    out.push((last.x as f64, last.z as f64));
    out
}

fn dist_to_segment((px, pz): (f64, f64), a: XZPoint, b: XZPoint) -> f64 {
    let (ax, az, bx, bz) = (a.x as f64, a.z as f64, b.x as f64, b.z as f64);
    let (dx, dz) = (bx - ax, bz - az);
    let len2 = dx * dx + dz * dz;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0)
    };
    ((px - (ax + t * dx)).powi(2) + (pz - (az + t * dz)).powi(2)).sqrt()
}

fn cells_around(a: XZPoint, b: XZPoint, pad: i32) -> impl Iterator<Item = (i32, i32)> {
    let min_cx = (a.x.min(b.x) - pad).div_euclid(LINE_CELL);
    let max_cx = (a.x.max(b.x) + pad).div_euclid(LINE_CELL);
    let min_cz = (a.z.min(b.z) - pad).div_euclid(LINE_CELL);
    let max_cz = (a.z.max(b.z) + pad).div_euclid(LINE_CELL);
    (min_cx..=max_cx).flat_map(move |cx| (min_cz..=max_cz).map(move |cz| (cx, cz)))
}

enum LineMatch {
    Distinct,
    /// Linhas aceitas tocadas e quanto de cada uma a candidata cobre.
    Duplicate(Vec<(usize, f64)>),
}

/// Segmentos das linhas aceitas, por célula de 32 blocos.
#[derive(Default)]
struct LineIndex {
    cells: HashMap<(i32, i32), Vec<(usize, usize)>>,
}

impl LineIndex {
    fn insert(&mut self, idx: usize, feature: &Feature) {
        if let Some(pts) = line_points(feature) {
            for (seg, w) in pts.windows(2).enumerate() {
                for cell in cells_around(w[0], w[1], 0) {
                    self.cells.entry(cell).or_default().push((idx, seg));
                }
            }
        }
    }

    /// Mesma regra de precedência das áreas (outra fonte, prioridade
    /// estritamente maior, mesmo grupo), com cobertura medida ao longo da linha
    /// e somada sobre TODAS as linhas aceitas — uma via do OSM costuma correr
    /// sobre vários trechos curtos do eixo do GDF.
    fn conflate(
        &self,
        candidate: &Feature,
        accepted: &[Feature],
        removed: &HashSet<usize>,
    ) -> LineMatch {
        let pts = line_points(candidate).expect("conflate só recebe LineString");
        let samples = samples_along(pts);
        let tolerance = line_tolerance(candidate.semantic_group);
        let pad = tolerance.ceil() as i32;
        let mut covered = 0usize;
        let mut touched: HashSet<usize> = HashSet::new();
        for &(sx, sz) in &samples {
            let p = XZPoint::new(sx.round() as i32, sz.round() as i32);
            let mut hit = false;
            for cell in cells_around(p, p, pad) {
                for &(idx, seg) in self.cells.get(&cell).into_iter().flatten() {
                    let other = &accepted[idx];
                    if removed.contains(&idx)
                        || other.source == candidate.source
                        || other.priority >= candidate.priority
                        || other.semantic_group != candidate.semantic_group
                        || !same_object_kind(other, candidate)
                    {
                        continue;
                    }
                    let opts = line_points(other).expect("índice só guarda linhas");
                    if dist_to_segment((sx, sz), opts[seg], opts[seg + 1]) <= tolerance {
                        hit = true;
                        touched.insert(idx);
                    }
                }
            }
            covered += usize::from(hit);
        }
        if (covered as f64) < LINE_MIN_COVERAGE * samples.len() as f64 {
            return LineMatch::Distinct;
        }
        let targets = touched
            .into_iter()
            .map(|idx| {
                let target_samples = samples_along(line_points(&accepted[idx]).unwrap());
                let near = target_samples
                    .iter()
                    .filter(|&&s| {
                        pts.windows(2)
                            .any(|w| dist_to_segment(s, w[0], w[1]) <= tolerance)
                    })
                    .count();
                (idx, near as f64 / target_samples.len() as f64)
            })
            .collect();
        LineMatch::Duplicate(targets)
    }
}

/// `inherit_missing_semantics` para linhas, com uma exceção: a classe
/// `highway` que o tradutor do GDF INFERE do número de faixas (`gdf:nrfaixas`
/// presente) é trocada pela classe curada da duplicata, e a inferida fica
/// registrada em `gdf:highway_por_nrfaixas`. Classe inferida não é dado.
fn inherit_line_semantics(winner: &mut Feature, loser: &Feature) {
    if winner.attributes.contains_key("gdf:nrfaixas") {
        if let Some(curated) = loser.attributes.get("highway") {
            if let Some(inferred) = winner
                .attributes
                .insert("highway".to_string(), curated.clone())
            {
                if inferred != *curated {
                    winner
                        .attributes
                        .insert("gdf:highway_por_nrfaixas".to_string(), inferred);
                }
            }
        }
    }
    inherit_missing_semantics(winner, loser);
}

fn should_supersede(accepted: &Feature, candidate: &Feature) -> bool {
    if accepted.source == candidate.source
        || accepted.priority >= candidate.priority
        || accepted.semantic_group != candidate.semantic_group
        || !accepted.intersects_aabb(candidate)
        || !same_object_kind(accepted, candidate)
    {
        return false;
    }
    match (&accepted.geometry, &candidate.geometry) {
        // Ponto só substitui ponto (um poste não some por estar dentro da
        // caixa de um estacionamento).
        (GeometryType::Point(a), GeometryType::Point(b)) => {
            (a.x - b.x).abs() <= 2 && (a.z - b.z).abs() <= 2
        }
        (a, b) if is_area(a) && is_area(b) => {
            // A caixa é só o filtro barato; a decisão é a sobreposição REAL dos
            // polígonos — um lote em L ou diagonal tem a caixa bem maior que ele.
            aabb_coverage_ratio(candidate.aabb, accepted.aabb) >= MIN_COVERAGE_TO_SUPERSEDE
                && polygon_coverage(candidate, accepted) >= MIN_COVERAGE_TO_SUPERSEDE
        }
        _ => false,
    }
}

fn is_area(geometry: &GeometryType) -> bool {
    matches!(
        geometry,
        GeometryType::Polygon(_) | GeometryType::MultiPolygon { .. }
    )
}

/// Chaves que dizem QUE objeto uma feature de infraestrutura é. O grupo
/// `Infrastructure` junta estacionamento, escola, cerca e poste; duas features
/// só são "o mesmo objeto" se a primeira dessas chaves presente tiver o mesmo
/// valor nas duas (estacionamento com estacionamento, muro com muro).
const OBJECT_KIND_KEYS: &[&str] = &["amenity", "barrier", "power", "man_made"];

fn same_object_kind(a: &Feature, b: &Feature) -> bool {
    if a.semantic_group != SemanticGroup::Infrastructure {
        return true;
    }
    OBJECT_KIND_KEYS
        .iter()
        .find_map(|k| a.get_tag(k).map(|v| (k, v)))
        .is_none_or(|(k, v)| b.get_tag(k) == Some(v))
}

fn outer_rings(geometry: &GeometryType) -> Vec<&[XZPoint]> {
    match geometry {
        GeometryType::Polygon(ring) => vec![ring.as_slice()],
        GeometryType::MultiPolygon { outer, .. } => outer.iter().map(|r| r.as_slice()).collect(),
        _ => Vec::new(),
    }
}

fn point_in_rings(x: f64, z: f64, rings: &[&[XZPoint]]) -> bool {
    rings.iter().any(|ring| {
        let mut inside = false;
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + n - 1) % n]);
            let (ax, az, bx, bz) = (a.x as f64, a.z as f64, b.x as f64, b.z as f64);
            if (az > z) != (bz > z) && x < (bx - ax) * (z - az) / (bz - az) + ax {
                inside = !inside;
            }
        }
        inside
    })
}

/// Fração da área da candidata que cai dentro dos polígonos da aceita,
/// amostrada numa grade de até 24×24 pontos sobre a caixa da candidata.
fn polygon_coverage(candidate: &Feature, accepted: &Feature) -> f64 {
    let own = outer_rings(&candidate.geometry);
    let other = outer_rings(&accepted.geometry);
    let (min_x, max_x, min_z, max_z) = candidate.aabb;
    let steps = 24;
    let (mut inside_own, mut inside_both) = (0usize, 0usize);
    for i in 0..steps {
        for j in 0..steps {
            let x = min_x as f64 + (i as f64 + 0.5) * (max_x - min_x + 1) as f64 / steps as f64;
            let z = min_z as f64 + (j as f64 + 0.5) * (max_z - min_z + 1) as f64 / steps as f64;
            if point_in_rings(x, z, &own) {
                inside_own += 1;
                if point_in_rings(x, z, &other) {
                    inside_both += 1;
                }
            }
        }
    }
    if inside_own == 0 {
        // Polígono degenerado (fino demais para a grade): cai na medida da caixa.
        return aabb_coverage_ratio(candidate.aabb, accepted.aabb);
    }
    inside_both as f64 / inside_own as f64
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
    fn superseded_feature_lends_its_missing_semantics_to_the_winner() {
        use crate::coordinate_system::cartesian::XZPoint;
        let square = |x0: i32, z0: i32, w: i32| {
            GeometryType::Polygon(vec![
                XZPoint::new(x0, z0),
                XZPoint::new(x0 + w, z0),
                XZPoint::new(x0 + w, z0 + w),
                XZPoint::new(x0, z0 + w),
                XZPoint::new(x0, z0),
            ])
        };
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let lidar = Feature::new(
            1,
            SemanticGroup::Building,
            tags(&[
                ("building", "yes"),
                ("source", "GDF_LiDAR_Cloud"),
                ("height", "12"),
            ]),
            square(0, 0, 20),
            "lidar".to_string(),
            1,
        );
        let osm = Feature::new(
            2,
            SemanticGroup::Building,
            tags(&[
                ("building", "retail"),
                ("name", "Feira do Guará"),
                ("building:levels", "2"),
                ("shop", "mall"),
                ("source", "survey"),
            ]),
            square(2, 2, 18),
            "osm".to_string(),
            10,
        );
        let mut manager = ProviderManager::new();
        let out = manager.resolve_collisions(vec![osm, lidar]);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.source, "lidar");
        assert_eq!(
            f.attributes.get("building").map(String::as_str),
            Some("yes")
        );
        assert_eq!(f.attributes.get("height").map(String::as_str), Some("12"));
        assert_eq!(
            f.attributes.get("name").map(String::as_str),
            Some("Feira do Guará")
        );
        assert_eq!(
            f.attributes.get("building:levels").map(String::as_str),
            Some("2")
        );
        assert_eq!(f.attributes.get("shop").map(String::as_str), Some("mall"));
        assert_eq!(
            f.attributes.get("source").map(String::as_str),
            Some("GDF_LiDAR_Cloud")
        );
        assert_eq!(
            f.attributes.get("merged:source").map(String::as_str),
            Some("osm")
        );
        let _ = &mut manager;
    }

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
    fn uso_column_means_lot_unless_structure_is_present() {
        assert_eq!(uso_to_tag("Residencial", false), ("landuse", "residential"));
        assert_eq!(uso_to_tag("Comercial", false), ("landuse", "retail"));
        assert_eq!(uso_to_tag("Inst EP", false), ("landuse", "institutional"));
        assert_eq!(uso_to_tag("Residencial", true), ("building", "residential"));
        assert_eq!(
            uso_to_tag("Edificação Comercial", false),
            ("building", "commercial")
        );
        assert_eq!(uso_to_tag("Industrial", true), ("building", "industrial"));
    }

    #[test]
    fn canonical_semantic_groups_align_providers() {
        let t = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            semantic_group_from_tags(&t(&[("railway", "subway")])),
            SemanticGroup::Railway
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("highway", "residential")])),
            SemanticGroup::Highway
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("natural", "water")])),
            SemanticGroup::Waterway
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("natural", "wood")])),
            SemanticGroup::Natural
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("landuse", "residential")])),
            SemanticGroup::Landuse
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("building", "yes"), ("shop", "x")])),
            SemanticGroup::Building
        );
        assert_eq!(
            semantic_group_from_tags(&t(&[("name", "x")])),
            SemanticGroup::Other
        );
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

#[cfg(test)]
mod line_conflation_tests {
    use super::*;

    fn line(
        id: u64,
        source: &str,
        priority: u8,
        pts: &[(i32, i32)],
        tags: &[(&str, &str)],
    ) -> Feature {
        Feature::new(
            id,
            SemanticGroup::Highway,
            tags.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            GeometryType::LineString(pts.iter().map(|&(x, z)| XZPoint::new(x, z)).collect()),
            source.to_string(),
            priority,
        )
    }

    fn resolve(features: Vec<Feature>) -> Vec<Feature> {
        ProviderManager::new().resolve_collisions(features)
    }

    /// A mesma rua no OSM (1–2 blocos ao lado, sobre DOIS trechos do eixo do
    /// GDF) sai uma vez só; os trechos do GDF recebem nome, sentido e a classe
    /// curada, sem perder as faixas medidas.
    #[test]
    fn osm_street_over_several_gdf_segments_is_one_street() {
        let out = resolve(vec![
            line(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (100, 0)],
                &[
                    ("highway", "tertiary"),
                    ("lanes", "2"),
                    ("gdf:nrfaixas", "2"),
                ],
            ),
            line(
                2,
                "GDF_GeoJSON",
                1,
                &[(100, 0), (200, 0)],
                &[
                    ("highway", "tertiary"),
                    ("lanes", "2"),
                    ("gdf:nrfaixas", "2"),
                ],
            ),
            line(
                3,
                "osm",
                10,
                &[(0, 2), (200, 1)],
                &[
                    ("highway", "residential"),
                    ("name", "QE 38"),
                    ("oneway", "yes"),
                    ("lanes", "3"),
                ],
            ),
        ]);
        assert_eq!(out.len(), 2);
        for gdf in &out {
            assert_eq!(gdf.get_tag("name").unwrap(), "QE 38");
            assert_eq!(gdf.get_tag("oneway").unwrap(), "yes");
            assert_eq!(gdf.get_tag("lanes").unwrap(), "2");
            assert_eq!(gdf.get_tag("highway").unwrap(), "residential");
            assert_eq!(gdf.get_tag("gdf:highway_por_nrfaixas").unwrap(), "tertiary");
        }
    }

    /// Regressão medida no Guará: a caixa de uma avenida DIAGONAL do GDF cobre
    /// o quarteirão inteiro, e a regra por AABB descartava a rua de serviço e a
    /// calçada do OSM que só estavam dentro da caixa.
    #[test]
    fn roads_inside_the_box_of_a_diagonal_avenue_survive() {
        let out = resolve(vec![
            line(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (300, 300)],
                &[("highway", "secondary")],
            ),
            line(
                2,
                "osm",
                10,
                &[(200, 40), (260, 40)],
                &[("highway", "service")],
            ),
            line(
                3,
                "osm",
                10,
                &[(60, 200), (60, 260)],
                &[("highway", "footway")],
            ),
        ]);
        assert_eq!(out.len(), 3);
        assert!(out[0].get_tag("service").is_none() && out[0].get_tag("footway").is_none());
    }

    #[test]
    fn parallel_carriageway_and_crossing_street_are_kept() {
        let out = resolve(vec![
            line(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (200, 0)],
                &[("highway", "tertiary")],
            ),
            line(2, "osm", 10, &[(0, 15), (200, 15)], &[("highway", "trunk")]),
            line(
                3,
                "osm",
                10,
                &[(100, -100), (100, 100)],
                &[("highway", "residential"), ("name", "Rua X")],
            ),
        ]);
        assert_eq!(out.len(), 3);
        assert!(out
            .iter()
            .find(|f| f.id == 1)
            .unwrap()
            .get_tag("name")
            .is_none());
    }

    #[test]
    fn osm_bridge_replaces_the_gdf_axis_it_covers() {
        let out = resolve(vec![
            line(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (80, 0)],
                &[("highway", "secondary")],
            ),
            line(
                2,
                "osm",
                10,
                &[(0, 1), (80, 1)],
                &[("highway", "secondary"), ("bridge", "yes"), ("layer", "1")],
            ),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get_tag("bridge").unwrap(), "yes");
    }

    /// Tags por nó são posicionais: herdadas por outra geometria, cairiam nos
    /// nós errados (a travessia do nó 3 do OSM no nó 3 do eixo do GDF).
    #[test]
    fn per_node_tags_are_never_inherited() {
        let out = resolve(vec![
            line(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (100, 0)],
                &[("highway", "tertiary")],
            ),
            line(
                2,
                "osm",
                10,
                &[(0, 1), (50, 1), (100, 1)],
                &[
                    ("highway", "residential"),
                    (NODE_TAGS_ATTR, "[null,{\"highway\":\"crossing\"},null]"),
                ],
            ),
        ]);
        assert_eq!(out.len(), 1);
        assert!(out[0].get_tag(NODE_TAGS_ATTR).is_none());
    }
}

#[cfg(test)]
mod same_object_merge_tests {
    use super::*;

    fn area(
        id: u64,
        source: &str,
        priority: u8,
        ring: &[(i32, i32)],
        tags: &[(&str, &str)],
    ) -> Feature {
        let mut pts: Vec<XZPoint> = ring.iter().map(|&(x, z)| XZPoint::new(x, z)).collect();
        pts.push(pts[0]);
        Feature::new(
            id,
            semantic_group_from_tags(
                &tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            tags.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            GeometryType::Polygon(pts),
            source.to_string(),
            priority,
        )
    }

    fn resolve(features: Vec<Feature>) -> Vec<u64> {
        let mut ids: Vec<u64> = ProviderManager::new()
            .resolve_collisions(features)
            .iter()
            .map(|f| f.id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// O mesmo estacionamento no GDF e no OSM (76 de 105 no Guará) vira um só.
    #[test]
    fn duplicated_parking_lot_is_merged() {
        let square = [(0, 0), (60, 0), (60, 40), (0, 40)];
        let ids = resolve(vec![
            area(1, "GDF_GeoJSON", 1, &square, &[("amenity", "parking")]),
            area(
                2,
                "osm",
                10,
                &[(2, 1), (59, 1), (59, 39), (2, 39)],
                &[("amenity", "parking")],
            ),
        ]);
        assert_eq!(ids, vec![1]);
    }

    /// Escola e estacionamento são do mesmo grupo (`Infrastructure`) mas não
    /// são o mesmo objeto.
    #[test]
    fn school_is_not_swallowed_by_parking_lot() {
        let ids = resolve(vec![
            area(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (60, 0), (60, 40), (0, 40)],
                &[("amenity", "parking")],
            ),
            area(
                2,
                "osm",
                10,
                &[(5, 5), (40, 5), (40, 30), (5, 30)],
                &[("amenity", "school")],
            ),
        ]);
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn lamp_inside_parking_lot_survives() {
        let lamp = Feature::new(
            3,
            SemanticGroup::Infrastructure,
            [("amenity".to_string(), "parking".to_string())]
                .into_iter()
                .collect(),
            GeometryType::Point(XZPoint::new(20, 20)),
            "osm".to_string(),
            10,
        );
        let ids = resolve(vec![
            area(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (60, 0), (60, 40), (0, 40)],
                &[("amenity", "parking")],
            ),
            lamp,
        ]);
        assert_eq!(ids, vec![1, 3]);
    }

    /// Lote diagonal: a caixa dele cobre a caixa do vizinho, o polígono não.
    #[test]
    fn diagonal_polygon_does_not_supersede_by_box_alone() {
        let ids = resolve(vec![
            area(
                1,
                "GDF_GeoJSON",
                1,
                &[(0, 0), (10, 0), (100, 90), (100, 100), (90, 100), (0, 10)],
                &[("amenity", "parking")],
            ),
            area(
                2,
                "osm",
                10,
                &[(60, 5), (95, 5), (95, 35), (60, 35)],
                &[("amenity", "parking")],
            ),
        ]);
        assert_eq!(ids, vec![1, 2]);
    }

    /// Muro do OSM a 2 blocos da "Cerca e Muro" do GDF é o mesmo muro; um muro
    /// paralelo a 8 blocos (lote vizinho) não é.
    #[test]
    fn walls_use_the_tighter_line_tolerance() {
        let wall = |id, src: &str, prio, z: i32| {
            Feature::new(
                id,
                SemanticGroup::Infrastructure,
                [("barrier".to_string(), "wall".to_string())]
                    .into_iter()
                    .collect(),
                GeometryType::LineString(vec![XZPoint::new(0, z), XZPoint::new(80, z)]),
                src.to_string(),
                prio,
            )
        };
        let ids = resolve(vec![
            wall(1, "GDF_GeoJSON", 1, 0),
            wall(2, "osm", 10, 2),
            wall(3, "osm", 10, 8),
        ]);
        assert_eq!(ids, vec![1, 3]);
    }
}
