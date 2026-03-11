pub mod osm_provider;
pub mod gdf_provider;
pub mod geojson_provider; // ?? INJETADO: GeoJSON governamental (CODEPLAN, IPEDF)
pub mod citygml_provider; // ?? INJETADO: 3D Models SAX Streamer (LOD2/LOD3)
pub mod wfs_provider;     // ?? INJETADO: Submundo WFS (Saneamento CAESB, Energia)
// ?? AS ÚLTIMAS INJEÇÕES DO REATOR BESM-6:
pub mod gpkg_provider;           // ?? INJETADO: GeoPackage SQLite (IBGE / IPEDF)
pub mod pbf_provider;            // ?? INJETADO: OSM PBF Local Binário O(1)
pub mod csv_provider;            // ?? INJETADO: Tabelas CSV Dados Abertos (Postes, Árvores)
pub mod raster_provider;         // ?? INJETADO: GeoTIFF Natureza e Hidrografia (MapBiomas)
pub mod indoor_utility_provider; // ?? INJETADO: Saneamento CAESB e Plantas Baixas (Rigor 1.15:1)
pub mod mesh_provider;           // ?? INJETADO: Fotogrametria 3D Voxelizada (.obj)

use crate::coordinate_system::geographic::LLBBox;
use crate::coordinate_system::cartesian::XZPoint;
use std::collections::{HashMap, HashSet};

// ============================================================================
// ADAPTER LAYER: Contrato Universal de Provedores de Dados (Tier Governamental)
// ============================================================================

/// Grupos Semânticos evitam falsos positivos na resolução de colisões.
/// Uma via (Highway) pode cruzar um rio (Waterway), mas dois provedores
/// diferentes não devem gerar o mesmo Building no mesmo lugar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticGroup {
    Building,
    Highway,
    Waterway,
    Landuse,
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
                    if p.x < min_x { min_x = p.x; }
                    if p.x > max_x { max_x = p.x; }
                    if p.z < min_z { min_z = p.z; }
                    if p.z > max_z { max_z = p.z; }
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
                        if p.x < min_x { min_x = p.x; }
                        if p.x > max_x { max_x = p.x; }
                        if p.z < min_z { min_z = p.z; }
                        if p.z > max_z { max_z = p.z; }
                    }
                }
                (min_x, max_x, min_z, max_z)
            }
        }
    }
}

/// A "Feature" é a unidade universal de dados do motor.
/// Um provedor OSM, Shapefile ou GeoJSON irá cuspir Features.
#[derive(Debug, Clone)]
pub struct Feature {
    /// ID Único gerado pelo provedor para evitar colisões
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
            match provider.fetch_features(bbox) {
                Ok(mut features) => {
                    println!(" -> {} features extraídas de {}.", features.len(), provider.name());
                    all_features.append(&mut features);
                }
                Err(e) => {
                    eprintln!("[AVISO] Timeout ou Falha Crítica no provedor {}: {}", provider.name(), e);
                }
            }
        }

        println!("[INFO] Merge Intelligence: Resolvendo colisões espaciais (Tier Governamental)...");
        let merged_features = self.resolve_collisions(all_features);
        println!("[INFO] Dados governamentais e públicos fundidos com sucesso. Total: {} features.", merged_features.len());

        Ok(merged_features)
    }

    /// Lógica de Resolução de Colisões Espaciais (Spatial Sweeper Otimizado O(N))
    fn resolve_collisions(&self, mut features: Vec<Feature>) -> Vec<Feature> {
        // Ordena garantindo que os dados de Shapefile do GDF(priority 1) sejam processados primeiro.
        features.sort_by(|a, b| a.priority.cmp(&b.priority));
        
        let mut accepted_features: Vec<Feature> = Vec::with_capacity(features.len());
        
        // Grid de indexação espacial (Baldes de 256x256 blocos)
        const GRID_SIZE: i32 = 256;
        let mut spatial_grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

        for new_feature in features {
            let mut is_collision = false;

            if new_feature.semantic_group != SemanticGroup::Terrain 
            && new_feature.semantic_group != SemanticGroup::Infrastructure {
                
                // Determina em quais baldes o Bounding Box desta nova feature cai
                let (min_x, max_x, min_z, max_z) = new_feature.aabb;
                let min_grid_x = min_x / GRID_SIZE;
                let max_grid_x = max_x / GRID_SIZE;
                let min_grid_z = min_z / GRID_SIZE;
                let max_grid_z = max_z / GRID_SIZE;

                // Checa colisões APENAS com features que estão nos mesmos baldes
                'collision_check: for gx in min_grid_x..=max_grid_x {
                    for gz in min_grid_z..=max_grid_z {
                        if let Some(cell_indices) = spatial_grid.get(&(gx, gz)) {
                            for &idx in cell_indices {
                                let accepted = &accepted_features[idx];
                                
                                if new_feature.semantic_group == accepted.semantic_group 
                                && new_feature.intersects_aabb(accepted) {
                                    is_collision = true;
                                    break 'collision_check;
                                }
                            }
                        }
                    }
                }
            }

            // Se sobreviveu à checagem de colisão, nós a aceitamos e registramos no Grid
            if !is_collision {
                let accepted_idx = accepted_features.len();
                
                if new_feature.semantic_group != SemanticGroup::Terrain 
                && new_feature.semantic_group != SemanticGroup::Infrastructure {
                    let (min_x, max_x, min_z, max_z) = new_feature.aabb;
                    let min_grid_x = min_x / GRID_SIZE;
                    let max_grid_x = max_x / GRID_SIZE;
                    let min_grid_z = min_z / GRID_SIZE;
                    let max_grid_z = max_z / GRID_SIZE;

                    for gx in min_grid_x..=max_grid_x {
                        for gz in min_grid_z..=max_grid_z {
                            spatial_grid.entry((gx, gz)).or_default().push(accepted_idx);
                        }
                    }
                }
                
                accepted_features.push(new_feature);
            }
        }

        accepted_features.shrink_to_fit();
        accepted_features
    }
}

// ============================================================================
// PONTE BESM-6 (Tradução Reversa para Compatibilidade Legada)
// ============================================================================
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay, ProcessedRelation, ProcessedMember, ProcessedMemberRole};

impl Feature {
    /// Tradução Reversa: Converte a Feature Otimizada do Motor de volta para o formato 
    /// legado do Arnis. Isso impede que os módulos de geração originais quebrem.
    pub fn into_processed_element(self) -> ProcessedElement {
        let mut fake_node_id = self.id.wrapping_mul(1000);

        match self.geometry {
            GeometryType::Point(pt) => {
                ProcessedElement::Node(ProcessedNode {
                    id: self.id,
                    x: pt.x,
                    z: pt.z,
                    tags: self.attributes,
                })
            },
            GeometryType::LineString(pts) | GeometryType::Polygon(pts) => {
                let nodes = pts.into_iter().map(|pt| {
                    fake_node_id = fake_node_id.wrapping_add(1);
                    ProcessedNode {
                        id: fake_node_id,
                        x: pt.x,
                        z: pt.z,
                        tags: HashMap::new(), 
                    }
                }).collect();
                
                ProcessedElement::Way(ProcessedWay {
                    id: self.id,
                    nodes,
                    tags: self.attributes,
                })
            },
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
                        way: ProcessedWay {
                            id: way_id,
                            nodes,
                            tags: HashMap::new(),
                        }
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
                        way: ProcessedWay {
                            id: way_id,
                            nodes,
                            tags: HashMap::new(),
                        }
                    });
                }

                ProcessedElement::Relation(ProcessedRelation {
                    id: self.id,
                    members,
                    tags: self.attributes,
                })
            }
        }
    }
}