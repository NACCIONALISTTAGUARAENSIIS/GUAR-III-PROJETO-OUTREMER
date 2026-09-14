//! Algoritmos de Rasterização (Voxelização Planialtimétrica BESM-6)
//!
//! Este módulo substitui o arcaico e engasgado Flood Fill (BFS Inundação)
//! por um Scanline Rasterizer determinístico O(Y * Arestas).
//! A Voxelização Scanline elimina os bilhões de testes de intersecção na CPU
//! e suporta anéis interiores (pátios, ilhas no Lago Paranoá).

use crate::osm_parser::ProcessedElement;
use itertools::Itertools;
use std::time::{Duration, Instant};

/// Maximum bounding box area (in blocks) for safety cut-off.
/// Aumentado para 30M blocks para suportar o Lago Paranoá.
const MAX_FLOOD_FILL_AREA: i64 = 30_000_000;

/// Estrutura para acomodar polígonos complexos (com furos/ilhas)
pub struct ComplexPolygon {
    pub outer: Vec<(i32, i32)>,
    pub inners: Vec<Vec<(i32, i32)>>,
}

/// Interface unificada e protegida para o BESM-6 Scanline Rasterizer.
/// Ignora a velha separação por "Area < 100_000" do Arnis original.
/// A varredura Scanline otimizada é mais rápida para teto ou lago de 30M.
pub fn flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    if polygon_coords.len() < 3 {
        return vec![];
    }

    // Tratamos o polígono simples (sem furos fornecidos pela assinatura velha)
    // como um caso de ComplexPolygon com inners vazios.
    let complex_poly = ComplexPolygon {
        outer: polygon_coords.to_vec(),
        inners: Vec::new(),
    };

    scanline_fill_complex(&complex_poly, timeout)
}

/// 🚨 BESM-6 Tweak: SCANLINE RASTERIZATION PARA POLÍGONOS COMPLEXOS (COM FUROS) 🚨
/// Reduz a sobrecarga de memória (Zero Queue) e preenche polígonos perfeitamente em O(Area).
/// Substitui o uso mortífero de `polygon.contains()` (Ray-Casting na CPU).
pub fn scanline_fill_complex(
    polygon: &ComplexPolygon,
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();

    if polygon.outer.len() < 3 {
        return vec![];
    }

    // Determina o Bounding Box master
    let (min_x, max_x) = polygon
        .outer
        .iter()
        .map(|&(x, _)| x)
        .minmax()
        .into_option()
        .unwrap();
    let (min_z, max_z) = polygon
        .outer
        .iter()
        .map(|&(_, z)| z)
        .minmax()
        .into_option()
        .unwrap();

    let area = (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64;
    if area > MAX_FLOOD_FILL_AREA {
        return vec![];
    }

    let cap = (area / 4).min(5_000_000) as usize;
    let mut filled_area = Vec::with_capacity(cap);

    // Preparação geométrica de todas as arestas (Exteriores + Interiores)
    let mut edges = Vec::new();

    // Closure para extrair arestas de um anel
    let mut add_edges_from_ring = |ring: &[(i32, i32)]| {
        let len = ring.len();
        for i in 0..len {
            let j = (i + 1) % len;
            let (x1, z1) = ring[i];
            let (x2, z2) = ring[j];

            // Ignoramos retas perfeitamente horizontais, pois elas não cruzam o scanline (z_f64) em um único ponto,
            // elas coexistem na linha, e a regra Par-Ímpar (Even-Odd) não precisa delas para contar "paredes".
            if z1 != z2 {
                // Guarda sempre com y_min primeiro (Técnica Top-Vertex)
                if z1 < z2 {
                    edges.push(((x1 as f64, z1 as f64), (x2 as f64, z2 as f64)));
                } else {
                    edges.push(((x2 as f64, z2 as f64), (x1 as f64, z1 as f64)));
                }
            }
        }
    };

    add_edges_from_ring(&polygon.outer);
    for inner_ring in &polygon.inners {
        add_edges_from_ring(inner_ring);
    }

    let mut last_timeout_check = 0;

    // A Varredura (Scanline)
    for z in min_z..=max_z {
        // Timeout culling (Verifica a cada 10.000 pixels para não asfixiar o I/O)
        if filled_area.len() - last_timeout_check > 10_000 {
            if let Some(timeout) = timeout {
                if start_time.elapsed() > *timeout {
                    return filled_area;
                }
            }
            last_timeout_check = filled_area.len();
        }

        let z_f64 = z as f64;
        let mut intersections = Vec::new();

        // Cruzamento Ray-Casting Horizontal
        for ((ex1, ez1), (ex2, ez2)) in &edges {
            // A condição Top-Vertex (z_f64 >= ez1 && z_f64 < ez2) é vital.
            // Ela previne o bug da contagem dupla quando a Scanline bate exatamente
            // no vértice onde duas arestas se encontram (fazendo o raio passar "pelo meio" da quina).
            if z_f64 >= *ez1 && z_f64 < *ez2 {
                let intersect_x = ex1 + (z_f64 - ez1) / (ez2 - ez1) * (ex2 - ex1);
                intersections.push(intersect_x);
            }
        }

        // Ordenação da esquerda para a direita (Regra de Winding)
        // Se pegou uma aresta do exterior e uma do furo, elas vão parear.
        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Regra Even-Odd: A cada par de vértices interceptados, estamos "dentro" do material.
        let mut idx = 0;
        while idx + 1 < intersections.len() {
            // Voxelização Segura: Arredondamento para preservar a forma contínua do Minecraft
            let start_x = intersections[idx].ceil() as i32;
            let end_x = intersections[idx + 1].floor() as i32;

            for x in start_x..=end_x {
                filled_area.push((x, z));
            }
            idx += 2;
        }
    }

    filled_area.shrink_to_fit();
    filled_area
}

/// Extrai a geometria complexa de Polígonos baseados em Relations (OSM multipolygons),
/// garantindo o reconhecimento estrito de pátios internos e ilhas.
pub fn extract_complex_polygon_from_element(element: &ProcessedElement) -> Option<ComplexPolygon> {
    match element {
        ProcessedElement::Way(w) => {
            let outer: Vec<(i32, i32)> = w.nodes.iter().map(|n| (n.x, n.z)).collect();
            Some(ComplexPolygon {
                outer,
                inners: Vec::new(),
            })
        }
        ProcessedElement::Relation(rel) => {
            let mut outer_ring = Vec::new();
            let mut inner_rings = Vec::new();

            for member in &rel.members {
                let ring: Vec<(i32, i32)> = member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
                if member.role == crate::osm_parser::ProcessedMemberRole::Outer {
                    outer_ring.extend(ring);
                } else if member.role == crate::osm_parser::ProcessedMemberRole::Inner {
                    inner_rings.push(ring);
                }
            }

            if outer_ring.is_empty() {
                None
            } else {
                Some(ComplexPolygon {
                    outer: outer_ring,
                    inners: inner_rings,
                })
            }
        }
        _ => None,
    }
}
