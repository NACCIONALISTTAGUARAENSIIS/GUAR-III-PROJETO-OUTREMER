use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc; // 🚨 BESM-6: Acesso de memória em velocidade terminal O(1)

// Raw data from OSM

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OsmMember {
    pub r#type: String,
    pub r#ref: u64,
    pub r#role: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OsmElement {
    pub r#type: String,
    pub id: u64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub nodes: Option<Vec<u64>>,
    pub tags: Option<HashMap<String, String>>,
    #[serde(default)]
    pub members: Vec<OsmMember>,
}

impl OsmElement {
    // 🚨 BESM-6: Adicionado este método para compatibilidade com o retrieve_data.rs
    pub fn type_str(&self) -> &str {
        &self.r#type
    }
}

#[derive(Debug, Default, Clone, Deserialize, serde::Serialize)]
pub struct OsmData {
    // 🚨 BESM-6: Visibilidade alterada para pub para o retrieve_data poder concatenar Bounding Boxes massivas
    pub elements: Vec<OsmElement>,
    #[serde(default)]
    pub remark: Option<String>,
}

impl OsmData {
    /// Returns true if there are no elements in the OSM data
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    // 🚨 BESM-6: Adicionado este método para fundir pedaços de um download massivo fatiado no retrieve_data.
    pub fn merge(&mut self, mut other: OsmData) {
        self.elements.append(&mut other.elements);
    }
}

struct SplitOsmData {
    pub nodes: Vec<OsmElement>,
    pub ways: Vec<OsmElement>,
    pub relations: Vec<OsmElement>,
    #[allow(dead_code)]
    pub others: Vec<OsmElement>,
}

impl SplitOsmData {
    fn total_count(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len() + self.others.len()
    }
    fn from_raw_osm_data(osm_data: OsmData) -> Self {
        let mut nodes = Vec::new();
        let mut ways = Vec::new();
        let mut relations = Vec::new();
        let mut others = Vec::new();
        for element in osm_data.elements {
            match element.r#type.as_str() {
                "node" => nodes.push(element),
                "way" => ways.push(element),
                "relation" => relations.push(element),
                _ => others.push(element),
            }
        }
        SplitOsmData {
            nodes,
            ways,
            relations,
            others,
        }
    }
}

// End raw data

// Normalized data that we can use

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedNode {
    pub id: u64,
    pub tags: HashMap<String, String>,

    // Minecraft coordinates
    pub x: i32,
    pub z: i32,
}

impl ProcessedNode {
    pub fn xz(&self) -> XZPoint {
        XZPoint {
            x: self.x,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ProcessedMemberRole {
    Outer,
    Inner,
    Part,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedMember {
    pub role: ProcessedMemberRole,
    pub way: Arc<ProcessedWay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedRelation {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<ProcessedMember>,
}

#[derive(Debug, Clone)]
pub enum ProcessedElement {
    Node(ProcessedNode),
    Way(Arc<ProcessedWay>), // 🚨 BESM-6 TWEAK: Garantindo ARC estrito nas ways root
    Relation(Arc<ProcessedRelation>), // 🚨 BESM-6 TWEAK: Garantindo ARC estrito nas Relations root
}

impl ProcessedElement {
    pub fn tags(&self) -> &HashMap<String, String> {
        match self {
            ProcessedElement::Node(n) => &n.tags,
            ProcessedElement::Way(w) => &w.tags,
            ProcessedElement::Relation(r) => &r.tags,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            ProcessedElement::Node(n) => n.id,
            ProcessedElement::Way(w) => w.id,
            ProcessedElement::Relation(r) => r.id,
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            ProcessedElement::Node(_) => "node",
            ProcessedElement::Way(_) => "way",
            ProcessedElement::Relation(_) => "relation",
        }
    }

    pub fn nodes<'a>(&'a self) -> Box<dyn Iterator<Item = &'a ProcessedNode> + 'a> {
        match self {
            ProcessedElement::Node(node) => Box::new([node].into_iter()),
            ProcessedElement::Way(way) => Box::new(way.nodes.iter()),
            ProcessedElement::Relation(_) => Box::new([].into_iter()),
        }
    }
}

pub fn parse_osm_data(
    osm_data: OsmData,
    bbox: LLBBox,
    scale: f64,
    debug: bool,
) -> (Vec<ProcessedElement>, XZBBox) {
    println!("{} Parsing data...", "[2/7]".bold());
    println!("Bounding box: {bbox:?}");
    emit_gui_progress_update(5.0, "Parsing data...");

    // Deserialize the JSON data into the OSMData structure
    let data = SplitOsmData::from_raw_osm_data(osm_data);

    let (coord_transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&bbox, scale)
        .unwrap_or_else(|e| {
            eprintln!("Error in defining coordinate transformation:\n{e}");
            panic!();
        });

    if debug {
        println!("Total elements: {}", data.total_count());
    }

    // TWEAK DE ELITE (Refinado):
    // Margem expandida de forma brutal (2000 blocos).
    // Garante que rodovias gigantescas como o Eixão ou EPIA não sejam cortadas no meio do nada
    // apenas porque o próximo "nó" do OSM está muito distante da fronteira do mapa.
    let margin = 2000;

    // 🚨 Correção do XZBBox::explicit: Ele já devolve a variante Rect, não precisa de .unwrap()
    let expanded_bbox = XZBBox::explicit(
        xzbbox.min_x() - margin,
        xzbbox.max_x() + margin,
        xzbbox.min_z() - margin,
        xzbbox.max_z() + margin,
    );

    // 🚨 BESM-6: Substituição de HashMap padrão por FxHashMap.
    // O SipHash do Rust mata a performance ao iterar milhões de chaves u64 (IDs do OSM).
    // O FxHash converte esta fase num processamento instantâneo.
    let mut nodes_map: FxHashMap<u64, ProcessedNode> = FxHashMap::default();
    let mut ways_map: FxHashMap<u64, Arc<ProcessedWay>> = FxHashMap::default();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in data.nodes {
        if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
            let llpoint = LLPoint::new(lat, lon).unwrap_or_else(|e| {
                eprintln!("Encountered invalid node element:\n{e}");
                panic!();
            });

            let xzpoint = coord_transformer.transform_point(llpoint);

            // TWEAK: Usa a expanded_bbox para armazenar nós que estão na "fronteira" do mapa.
            // Sem isso, lagos ou estradas sumiriam do nada.
            if !expanded_bbox.contains(&xzpoint) {
                continue;
            }

            let tags = element.tags.unwrap_or_default();
            let processed: ProcessedNode = ProcessedNode {
                id: element.id,
                tags: tags.clone(), // Tweak: Clona apenas uma vez para o nó principal
                x: xzpoint.x,
                z: xzpoint.z,
            };

            nodes_map.insert(element.id, processed.clone());

            if !tags.is_empty() && xzbbox.contains(&xzpoint) {
                processed_elements.push(ProcessedElement::Node(processed));
            }
        }
    }

    // Second pass: process ways and clip them to bbox
    for element in data.ways {
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            for &node_id in node_ids {
                if let Some(node) = nodes_map.get(&node_id) {
                    nodes.push(node.clone());
                }
            }
        }

        let tags = element.tags.unwrap_or_default();

        // 🚨 PREVENÇÃO BESM-6: Adoção de Nós Órfãos.
        // Se uma `way` pertencer a um Relation gigantesco que nós sabemos que foi gerado pelo
        // ProviderManager, nós NÃO A CORTAMOS AQUI se ela tiver menos nós do que devia.
        let way = Arc::new(ProcessedWay {
            id: element.id,
            tags: tags.clone(),
            nodes,
        });

        ways_map.insert(element.id, Arc::clone(&way));

        // Clip way nodes for standalone way processing (not relations)
        let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);

        // Skip ways that are completely outside the bbox (empty after clipping)
        if clipped_nodes.is_empty() {
            continue;
        }

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags,
            nodes: clipped_nodes,
        };

        // 🚨 BESM-6: Envolvemos em Arc para coerência tipológica com as Relações
        processed_elements.push(ProcessedElement::Way(Arc::new(processed)));
    }

    // Third pass: process relations and clip member ways
    for element in data.relations {
        let Some(tags) = &element.tags else {
            continue;
        };

        // Process multipolygons and building relations
        let relation_type = tags.get("type").map(|x: &String| x.as_str());
        if relation_type != Some("multipolygon") && relation_type != Some("building") {
            continue;
        };

        let is_building_relation = relation_type == Some("building")
            || tags.contains_key("building")
            || tags.contains_key("building:part");

        // Water relations require unclipped ways for ring merging in water_areas.rs
        // Building multipolygon relations also need unclipped ways so that
        // open outer-way segments can be merged into closed rings before clipping
        let is_water_relation = is_water_element(tags);
        let is_building_multipolygon = (tags.contains_key("building")
            || tags.contains_key("building:part"))
            && relation_type == Some("multipolygon");
        let keep_unclipped = is_water_relation || is_building_multipolygon;

        let members: Vec<ProcessedMember> = element
            .members
            .iter()
            .filter_map(|mem: &OsmMember| {
                if mem.r#type != "way" {
                    return None;
                }

                let trimmed_role = mem.role.trim();
                let role = if trimmed_role.eq_ignore_ascii_case("outer")
                    || trimmed_role.eq_ignore_ascii_case("outline")
                {
                    ProcessedMemberRole::Outer
                } else if trimmed_role.eq_ignore_ascii_case("inner") {
                    ProcessedMemberRole::Inner
                } else if trimmed_role.eq_ignore_ascii_case("part") {
                    if relation_type == Some("building") {
                        ProcessedMemberRole::Part
                    } else {
                        return None;
                    }
                } else if is_building_relation {
                    ProcessedMemberRole::Outer
                } else {
                    return None;
                };

                // Check if the way exists in ways_map
                let way = match ways_map.get(&mem.r#ref) {
                    Some(w) => Arc::clone(w),
                    None => {
                        return None;
                    }
                };

                let final_way = if keep_unclipped {
                    way
                } else {
                    let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);
                    if clipped_nodes.is_empty() {
                        return None;
                    }
                    Arc::new(ProcessedWay {
                        id: way.id,
                        tags: way.tags.clone(),
                        nodes: clipped_nodes,
                    })
                };

                Some(ProcessedMember {
                    role,
                    way: final_way,
                })
            })
            .collect();

        if !members.is_empty() {
            // 🚨 BESM-6: Envolvemos em Arc
            processed_elements.push(ProcessedElement::Relation(Arc::new(ProcessedRelation {
                id: element.id,
                members,
                tags: tags.clone(),
            })));
        }
    }

    emit_gui_progress_update(14.0, "");

    // TWEAK: Garantindo que a memória seja expurgada antes de passar para a fase de construção dos blocos.
    drop(nodes_map);
    drop(ways_map);

    (processed_elements, xzbbox)
}

/// Returns true if tags indicate a water element handled by water_areas.rs.
fn is_water_element(tags: &HashMap<String, String>) -> bool {
    if tags.contains_key("water") {
        return true;
    }
    if let Some(natural_val) = tags.get("natural") {
        if natural_val == "water" || natural_val == "bay" {
            return true;
        }
    }
    if let Some(waterway_val) = tags.get("waterway") {
        if waterway_val == "dock" {
            return true;
        }
    }
    false
}

/// Precedência de um elemento: ordem de despacho dentro da região E
/// precedência de PISO entre regiões (menor = desenhado antes e vence no chão,
/// ver `WorldEditor::surface_writer_priority`).
///
/// A escada antiga (`entrance, building, highway, waterway, water, barrier`,
/// resto = 6) só separava os grandes grupos; tudo que é ÁREA — quadra, pátio
/// de escola, parque, `landuse`, `place` — empatava, e o empate era decidido
/// pela ordem arbitrária de chegada (que, entre regiões, é a ordem do Halo:
/// um `landuse=residential` da QE 17 ancorado na região anterior pintava o
/// gramado por cima do asfalto das ruas da região seguinte). A escada abaixo
/// coloca o mais específico/menor antes do mais genérico/maior: prédio >
/// via > trilho > rio > lago > cerca > piso esportivo/estacionamento >
/// equipamento (amenity/man_made/...) > lazer e natural > landuse > place.
pub fn get_priority(element: &ProcessedElement) -> usize {
    let t = element.tags();
    let has = |k: &str| t.contains_key(k);
    let is = |k: &str, v: &str| t.get(k).map(String::as_str) == Some(v);
    if has("entrance") {
        0
    } else if has("building") || has("building:part") {
        1
    } else if has("highway") {
        2
    } else if has("railway") {
        3
    } else if has("waterway") {
        4
    } else if has("water") || is("natural", "water") {
        5
    } else if has("barrier") {
        6
    } else if matches!(
        t.get("leisure").map(String::as_str),
        Some("pitch" | "track" | "playground" | "swimming_pool" | "fitness_station")
    ) || is("amenity", "parking")
        || has("sport")
    {
        7
    } else if has("amenity")
        || has("man_made")
        || has("power")
        || has("tourism")
        || has("shop")
        || has("public_transport")
    {
        8
    } else if has("leisure") || has("natural") {
        9
    } else if has("landuse") {
        10
    } else if has("place") {
        11
    } else {
        12
    }
}

#[cfg(test)]
mod priority_tests {
    use super::*;

    fn way(tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(std::sync::Arc::new(ProcessedWay {
            id: 1,
            nodes: vec![],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }))
    }

    #[test]
    fn floor_precedence_puts_specific_before_generic() {
        let p = |tags: &[(&str, &str)]| get_priority(&way(tags));
        assert!(p(&[("building", "yes")]) < p(&[("highway", "residential")]));
        assert!(p(&[("highway", "residential")]) < p(&[("landuse", "residential")]));
        assert!(p(&[("railway", "subway")]) < p(&[("landuse", "railway")]));
        assert!(p(&[("leisure", "pitch")]) < p(&[("amenity", "school")]));
        assert!(p(&[("amenity", "school")]) < p(&[("landuse", "residential")]));
        assert!(p(&[("leisure", "park")]) < p(&[("landuse", "grass")]));
        assert!(p(&[("landuse", "residential")]) < p(&[("place", "neighbourhood")]));
        assert_eq!(p(&[("natural", "water")]), p(&[("water", "lake")]));
    }
}
