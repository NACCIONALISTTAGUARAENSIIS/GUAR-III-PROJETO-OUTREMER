//! Enriquecimento de prédios com os POIs que eles contêm.
//!
//! 🚨 CORREÇÃO DE QUALIDADE: no Guará real, 24.410 dos 24.600 prédios são
//! `building=yes` sem nenhuma tag de uso; o uso está nos **nós** dentro deles
//! (`amenity=restaurant`, `shop=clothes`, `amenity=place_of_worship`,
//! `office=*`, `healthcare=*`...). O motor tratava cada um desses prédios como
//! casa genérica e descartava o nó (o dispatcher não desenha nada para
//! `amenity=restaurant` solto) — o comércio inteiro sumia.
//!
//! Este pré-passe injeta no way do prédio as tags do POI dominante que cai
//! dentro do seu polígono (só chaves que o prédio ainda não tem), ANTES do
//! dispatch. Assim toda a lógica já existente passa a valer sem código novo:
//! `BuildingCategory::from_element` (loja, escritório, igreja, escola...),
//! `buildings_interior::detect_tipologia` (layout de loja/escola/hospital),
//! `eh_uso_misto_comercio_terreo` (loja embaixo, apartamentos em cima) e a
//! fachada de vitrine do térreo.
//!
//! Custo: índice espacial dos POIs em células de 32 blocos + teste
//! ponto-em-polígono (ray casting) só para os candidatos da caixa do prédio.

use crate::osm_parser::{ProcessedElement, ProcessedWay};
use std::collections::HashMap;
use std::sync::Arc;

/// Chaves de nó que descrevem o USO de um prédio.
const POI_KEYS: &[&str] = &[
    "amenity",
    "shop",
    "office",
    "craft",
    "healthcare",
    "tourism",
];

/// Quanto menor, mais o POI define a identidade do prédio.
fn poi_rank(key: &str, value: &str) -> u8 {
    match (key, value) {
        ("amenity", "place_of_worship")
        | ("amenity", "school")
        | ("amenity", "hospital")
        | ("amenity", "university")
        | ("amenity", "college")
        | ("amenity", "kindergarten")
        | ("amenity", "clinic")
        | ("amenity", "townhall")
        | ("amenity", "police")
        | ("amenity", "fire_station") => 0,
        ("shop", _) => 1,
        ("amenity", _) => 2,
        ("office", _) => 3,
        ("healthcare", _) => 4,
        ("craft", _) => 5,
        _ => 6,
    }
}

/// Centróide (média dos vértices) de uma ÁREA de uso que não é prédio: loja
/// indoor (`indoor=*`/`level=*`) ou terreno institucional (escola, hospital,
/// igreja...). Estacionamentos e áreas sem chave de uso ficam de fora.
fn poi_area_centroid(w: &ProcessedWay) -> Option<(i32, i32)> {
    if w.nodes.len() < 4
        || w.tags.contains_key("building")
        || w.tags.contains_key("building:part")
        || !POI_KEYS.iter().any(|k| w.tags.contains_key(*k))
    {
        return None;
    }
    let indoor_unit = w.tags.contains_key("indoor") || w.tags.contains_key("level");
    let institutional =
        crate::element_processing::amenities::institutional_landuse_style(&w.tags).is_some();
    if !indoor_unit && !institutional {
        return None;
    }
    let n = w.nodes.len() as i64;
    let sx: i64 = w.nodes.iter().map(|p| p.x as i64).sum();
    let sz: i64 = w.nodes.iter().map(|p| p.z as i64).sum();
    Some(((sx / n) as i32, (sz / n) as i32))
}

/// Ray casting (par-ímpar) em inteiros de 64 bits.
pub fn point_in_polygon(px: i32, pz: i32, ring: &[(i32, i32)]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let (px, pz) = (px as i64, pz as i64);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, zi) = (ring[i].0 as i64, ring[i].1 as i64);
        let (xj, zj) = (ring[j].0 as i64, ring[j].1 as i64);
        if (zi > pz) != (zj > pz) {
            // x da interseção da aresta com a linha horizontal z = pz
            let x_cross = xi as f64 + (pz - zi) as f64 * (xj - xi) as f64 / (zj - zi) as f64;
            if (px as f64) < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Injeta nos prédios as tags do POI dominante contido neles. Devolve quantos
/// prédios foram enriquecidos.
pub fn inject_poi_tags_into_buildings(elements: &mut [ProcessedElement]) -> usize {
    // 1. Índice espacial dos POIs: nós com alguma chave de uso e também ÁREAS
    //    de uso sem `building` (lojas indoor do shopping/feira com `indoor=*`
    //    ou `level=*`, pátios institucionais), representadas pelo centróide.
    //    `pois` guarda (x, z, índice do elemento); a grade indexa `pois`.
    let mut pois: Vec<(i32, i32, usize)> = Vec::new();
    for (i, e) in elements.iter().enumerate() {
        match e {
            ProcessedElement::Node(n) => {
                if POI_KEYS.iter().any(|k| n.tags.contains_key(*k)) {
                    pois.push((n.x, n.z, i));
                }
            }
            ProcessedElement::Way(w) => {
                if let Some((cx, cz)) = poi_area_centroid(w) {
                    pois.push((cx, cz, i));
                }
            }
            ProcessedElement::Relation(_) => {}
        }
    }
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (k, &(x, z, _)) in pois.iter().enumerate() {
        grid.entry((x >> 5, z >> 5)).or_default().push(k);
    }
    if grid.is_empty() {
        return 0;
    }

    let mut enriched = 0usize;
    for i in 0..elements.len() {
        let (ring, candidates) = match &elements[i] {
            ProcessedElement::Way(w)
                if (w.tags.contains_key("building") || w.tags.contains_key("building:part"))
                    && w.nodes.len() >= 4 =>
            {
                let ring: Vec<(i32, i32)> = w.nodes.iter().map(|n| (n.x, n.z)).collect();
                let min_x = ring.iter().map(|p| p.0).min().unwrap();
                let max_x = ring.iter().map(|p| p.0).max().unwrap();
                let min_z = ring.iter().map(|p| p.1).min().unwrap();
                let max_z = ring.iter().map(|p| p.1).max().unwrap();
                let mut candidates = Vec::new();
                for cx in (min_x >> 5)..=(max_x >> 5) {
                    for cz in (min_z >> 5)..=(max_z >> 5) {
                        if let Some(list) = grid.get(&(cx, cz)) {
                            candidates.extend(list.iter().copied());
                        }
                    }
                }
                if candidates.is_empty() {
                    continue;
                }
                (ring, candidates)
            }
            _ => continue,
        };

        // 2. POI dominante entre os que caem dentro do polígono
        let mut best: Option<(u8, usize)> = None;
        let mut count = 0usize;
        for &k in &candidates {
            let (px, pz, ci) = pois[k];
            if ci == i || !point_in_polygon(px, pz, &ring) {
                continue;
            }
            let poi_tags = elements[ci].tags();
            count += 1;
            let rank = POI_KEYS
                .iter()
                .filter_map(|k| poi_tags.get(*k).map(|v| poi_rank(k, v)))
                .min()
                .unwrap_or(6);
            if best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, ci));
            }
        }
        let Some((_, poi_idx)) = best else {
            continue;
        };
        let poi_tags = elements[poi_idx].tags().clone();

        // 3. Reconstrói o way com as tags injetadas (só as que ele não tem)
        let ProcessedElement::Way(w) = &elements[i] else {
            continue;
        };
        let mut tags = w.tags.clone();
        let mut changed = false;
        for key in POI_KEYS {
            if let Some(v) = poi_tags.get(*key) {
                if !tags.contains_key(*key) {
                    tags.insert((*key).to_string(), v.clone());
                    changed = true;
                }
            }
        }
        if let Some(name) = poi_tags.get("name") {
            if !tags.contains_key("name") {
                tags.insert("name".to_string(), name.clone());
                changed = true;
            }
        }
        tags.insert("poi:count".to_string(), count.to_string());
        if changed {
            enriched += 1;
        }
        elements[i] = ProcessedElement::Way(Arc::new(ProcessedWay {
            id: w.id,
            nodes: w.nodes.clone(),
            tags,
        }));
    }
    enriched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;

    fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Node(ProcessedNode {
            id,
            x,
            z,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn building(id: u64, tags: &[(&str, &str)], ring: &[(i32, i32)]) -> ProcessedElement {
        ProcessedElement::Way(Arc::new(ProcessedWay {
            id,
            nodes: ring
                .iter()
                .map(|&(x, z)| ProcessedNode {
                    id: 0,
                    x,
                    z,
                    tags: HashMap::new(),
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }))
    }

    const SQUARE: &[(i32, i32)] = &[(-100, 50), (-80, 50), (-80, 70), (-100, 70), (-100, 50)];

    #[test]
    fn point_in_polygon_handles_negative_coordinates_and_edges() {
        assert!(point_in_polygon(-90, 60, SQUARE));
        assert!(!point_in_polygon(-70, 60, SQUARE));
        assert!(!point_in_polygon(-90, 80, SQUARE));
    }

    #[test]
    fn shop_node_inside_building_injects_shop_and_name() {
        let mut elements = vec![
            building(1, &[("building", "yes")], SQUARE),
            node(2, -90, 60, &[("shop", "bakery"), ("name", "Padaria Guará")]),
            node(3, -50, 60, &[("shop", "clothes")]), // fora
        ];
        assert_eq!(inject_poi_tags_into_buildings(&mut elements), 1);
        let ProcessedElement::Way(w) = &elements[0] else {
            panic!()
        };
        assert_eq!(w.tags.get("shop").map(String::as_str), Some("bakery"));
        assert_eq!(
            w.tags.get("name").map(String::as_str),
            Some("Padaria Guará")
        );
        assert_eq!(w.tags.get("poi:count").map(String::as_str), Some("1"));
    }

    #[test]
    fn institutional_poi_outranks_shop_and_existing_tags_are_kept() {
        let mut elements = vec![
            building(1, &[("building", "yes"), ("name", "Bloco A")], SQUARE),
            node(2, -95, 55, &[("shop", "kiosk")]),
            node(
                3,
                -85,
                65,
                &[("amenity", "place_of_worship"), ("name", "Paróquia")],
            ),
        ];
        inject_poi_tags_into_buildings(&mut elements);
        let ProcessedElement::Way(w) = &elements[0] else {
            panic!()
        };
        assert_eq!(
            w.tags.get("amenity").map(String::as_str),
            Some("place_of_worship")
        );
        assert_eq!(w.tags.get("name").map(String::as_str), Some("Bloco A"));
        assert_eq!(w.tags.get("poi:count").map(String::as_str), Some("2"));
    }

    #[test]
    fn indoor_shop_unit_area_enriches_enclosing_mall() {
        let mut elements = vec![
            building(1, &[("building", "retail")], SQUARE),
            building(
                2,
                &[("shop", "clothes"), ("indoor", "room"), ("level", "1")],
                &[(-95, 55), (-90, 55), (-90, 60), (-95, 60), (-95, 55)],
            ),
            // área de estacionamento sem indoor/level: não conta como POI
            building(
                3,
                &[("amenity", "parking")],
                &[(-99, 51), (-97, 51), (-97, 53), (-99, 53), (-99, 51)],
            ),
        ];
        assert_eq!(inject_poi_tags_into_buildings(&mut elements), 1);
        let ProcessedElement::Way(w) = &elements[0] else {
            panic!()
        };
        assert_eq!(w.tags.get("shop").map(String::as_str), Some("clothes"));
        assert_eq!(w.tags.get("poi:count").map(String::as_str), Some("1"));
    }

    #[test]
    fn building_without_pois_is_untouched() {
        let original = building(1, &[("building", "yes")], SQUARE);
        let mut elements = vec![original.clone(), node(2, 0, 0, &[("shop", "car")])];
        assert_eq!(inject_poi_tags_into_buildings(&mut elements), 0);
        assert_eq!(elements[0].tags(), original.tags());
    }
}
