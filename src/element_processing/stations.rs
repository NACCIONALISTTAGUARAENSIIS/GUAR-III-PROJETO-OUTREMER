//! Estações de metrô/trem, acessos e terminais de ônibus.
//!
//! 🚨 CORREÇÃO DE QUALIDADE: o dispatcher não tinha NENHUM branch para nós
//! `railway=*` — as três estações do Metrô-DF no recorte do Guará (Feira,
//! Guará, Shopping), seus cinco acessos (`railway=subway_entrance`) e as
//! paradas eram descartados em silêncio. Somado ao enterro indevido de toda
//! via `railway=subway` (ver `railways.rs`), o metrô simplesmente não existia
//! no mundo gerado.
//!
//! Uma estação no OSM é normalmente um **nó** (`railway=station` +
//! `public_transport=station`, com `name` e `level`), sem polígono. A geometria
//! é derivada da própria linha: o nó é projetado sobre o trilho mais próximo
//! (`RailIndex`), a plataforma nasce alinhada ao eixo da via, no MESMO nível do
//! leito (`railways::vertical_offset`), com o padrão do Metrô-DF: plataformas
//! laterais, piso tátil amarelo na borda, bancos, pilares, cobertura metálica
//! (superfície/viaduto) ou caixa escavada com iluminação (subterrâneo), placas
//! com o nome e acessos ligando o nível da rua ao nível da plataforma.

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::railways::{is_elevated_way, is_tunnel_way, vertical_offset};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Um segmento de via férrea com o que a estação precisa saber sobre ele.
#[derive(Clone, Copy, Debug)]
pub struct RailSegment {
    pub x1: i32,
    pub z1: i32,
    pub x2: i32,
    pub z2: i32,
    pub layer: i32,
    pub tunnel: bool,
    pub elevated: bool,
    pub metro: bool,
    pub double_track: bool,
}

impl RailSegment {
    /// Deslocamento vertical do leito em relação ao solo (ver `railways.rs`).
    pub fn offset(&self) -> i32 {
        vertical_offset(self.layer, self.tunnel, self.elevated)
    }

    /// Meio-gabarito do leito (mesma regra de `railways.rs`).
    pub fn radius(&self) -> i32 {
        if self.double_track {
            6
        } else {
            4
        }
    }
}

/// Índice de trilhos e acessos, construído uma vez por geração.
#[derive(Default)]
pub struct RailIndex {
    pub segments: Vec<RailSegment>,
    /// Nós `railway=subway_entrance` / `train_station_entrance` (posição).
    pub entrances: Vec<(i32, i32)>,
    /// Nós `railway=station|halt` (posição) — para um acesso saber se já
    /// pertence a alguma estação.
    pub stations: Vec<(i32, i32)>,
}

impl RailIndex {
    pub fn build(elements: &[ProcessedElement]) -> Self {
        let mut index = RailIndex::default();
        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    let Some(kind) = way.tags.get("railway") else {
                        continue;
                    };
                    if !matches!(
                        kind.as_str(),
                        "subway" | "rail" | "light_rail" | "tram" | "narrow_gauge" | "monorail"
                    ) {
                        continue;
                    }
                    let layer = way
                        .tags
                        .get("layer")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let tunnel = is_tunnel_way(&way.tags);
                    let elevated = !tunnel && is_elevated_way(&way.tags);
                    let metro = matches!(kind.as_str(), "subway" | "light_rail" | "monorail");
                    let tracks = way
                        .tags
                        .get("tracks")
                        .and_then(|t| t.parse::<i32>().ok())
                        .unwrap_or(if metro { 2 } else { 1 });
                    for pair in way.nodes.windows(2) {
                        index.segments.push(RailSegment {
                            x1: pair[0].x,
                            z1: pair[0].z,
                            x2: pair[1].x,
                            z2: pair[1].z,
                            layer,
                            tunnel,
                            elevated,
                            metro,
                            double_track: tracks >= 2,
                        });
                    }
                }
                ProcessedElement::Node(node) => {
                    match node.tags.get("railway").map(|s| s.as_str()) {
                        Some("subway_entrance") | Some("train_station_entrance") => {
                            index.entrances.push((node.x, node.z));
                        }
                        Some("station") | Some("halt") => index.stations.push((node.x, node.z)),
                        _ => {}
                    }
                }
                ProcessedElement::Relation(_) => {}
            }
        }
        index
    }

    /// Segmento mais próximo de `(x, z)` (até `max_dist`), com a projeção do
    /// ponto sobre ele e a direção unitária da via.
    pub fn nearest(&self, x: i32, z: i32, max_dist: f64) -> Option<NearestTrack> {
        let mut best: Option<NearestTrack> = None;
        for seg in &self.segments {
            let (ax, az) = (seg.x1 as f64, seg.z1 as f64);
            let (bx, bz) = (seg.x2 as f64, seg.z2 as f64);
            let (dx, dz) = (bx - ax, bz - az);
            let len2 = dx * dx + dz * dz;
            if len2 < 1.0 {
                continue;
            }
            let t = (((x as f64 - ax) * dx + (z as f64 - az) * dz) / len2).clamp(0.0, 1.0);
            let (px, pz) = (ax + t * dx, az + t * dz);
            let dist = ((x as f64 - px).powi(2) + (z as f64 - pz).powi(2)).sqrt();
            if dist <= max_dist && best.as_ref().is_none_or(|b| dist < b.dist) {
                let len = len2.sqrt();
                best = Some(NearestTrack {
                    segment: *seg,
                    px: px.round() as i32,
                    pz: pz.round() as i32,
                    ux: dx / len,
                    uz: dz / len,
                    dist,
                });
            }
        }
        best
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NearestTrack {
    pub segment: RailSegment,
    pub px: i32,
    pub pz: i32,
    pub ux: f64,
    pub uz: f64,
    pub dist: f64,
}

/// Meio-comprimento da plataforma (blocos). Metrô-DF: composições de 4 carros
/// (~90 m) em plataformas de ~120 m → 140 blocos na escala 1,33; usamos 60 de
/// cada lado para caber em estações mapeadas perto de curvas.
const PLATFORM_HALF_LEN: i32 = 60;
const PLATFORM_WIDTH: i32 = 5;
/// Pé-direito da plataforma até a cobertura/teto
const PLATFORM_CLEARANCE: i32 = 5;
/// Distância máxima do nó da estação até o trilho para considerar que é a linha dela
const STATION_TRACK_SEARCH: f64 = 90.0;
/// Acessos a até esta distância do centro da estação pertencem a ela
const ENTRANCE_SEARCH: f64 = 160.0;

/// Nós `railway=*`: estações, acessos, paradas.
pub fn generate_railway_node(editor: &mut WorldEditor, node: &ProcessedNode, rails: &RailIndex) {
    match node.tags.get("railway").map(|s| s.as_str()) {
        Some("station") | Some("halt") => generate_station(editor, node, rails),
        Some("subway_entrance") | Some("train_station_entrance") => {
            // Acessos perto de uma estação são desenhados (e ligados à plataforma)
            // pela própria estação; só os órfãos ganham um quiosque isolado.
            let near_station = rails.stations.iter().any(|&(sx, sz)| {
                (((sx - node.x) as f64).powi(2) + ((sz - node.z) as f64).powi(2)).sqrt()
                    <= ENTRANCE_SEARCH
            });
            if !near_station {
                generate_entrance_kiosk(editor, node.x, node.z, None);
            }
        }
        Some("buffer_stop") => {
            let y = editor.get_ground_level(node.x, node.z);
            for dx in -1..=1 {
                editor.set_block_absolute(RED_CONCRETE, node.x + dx, y + 1, node.z, None, None);
            }
        }
        _ => {}
    }
}

fn generate_station(editor: &mut WorldEditor, node: &ProcessedNode, rails: &RailIndex) {
    let name = node
        .tags
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Estação".to_string());
    let level_tag: Option<i32> = node.tags.get("level").and_then(|l| l.parse().ok());

    let Some(track) = rails.nearest(node.x, node.z, STATION_TRACK_SEARCH) else {
        // Sem linha por perto (dado incompleto): pelo menos um acesso sinalizado
        generate_entrance_kiosk(editor, node.x, node.z, Some(&name));
        return;
    };

    let seg = track.segment;
    // O `level` do nó desempata quando a linha muda de nível perto da estação
    let underground = seg.tunnel || level_tag.is_some_and(|l| l < 0);
    let elevated = !underground && (seg.elevated || level_tag.is_some_and(|l| l > 0));
    let layer = if underground {
        seg.layer.min(level_tag.unwrap_or(-1)).min(-1)
    } else if elevated {
        seg.layer.max(level_tag.unwrap_or(1)).max(1)
    } else {
        0
    };
    let offset = if level_tag.is_none() {
        seg.offset()
    } else {
        vertical_offset(layer, underground, elevated)
    };
    let radius = seg.radius();
    let operator_line = if seg.metro { "METRÔ-DF" } else { "Estação" };

    let (cx, cz) = (track.px, track.pz);
    let (ux, uz) = (track.ux, track.uz);
    let (nx, nz) = (-uz, ux);
    let ground_c = editor.get_ground_level(cx, cz);
    let track_y = if underground {
        (ground_c + offset)
            .min(ground_c - 5)
            .max(crate::data_processing::MIN_Y + 6)
    } else if elevated {
        (ground_c + offset).max(ground_c + 5)
    } else {
        ground_c + offset
    };
    let floor_y = track_y + 1; // piso da plataforma ~1 m acima do boleto
    let roof_y = floor_y + PLATFORM_CLEARANCE;

    let world = |u: i32, v: i32| -> (i32, i32) {
        (
            (cx as f64 + u as f64 * ux + v as f64 * nx).round() as i32,
            (cz as f64 + u as f64 * uz + v as f64 * nz).round() as i32,
        )
    };

    // ---------- 1. Caixa escavada (subterrâneo) ----------
    if underground {
        let half_w = radius + PLATFORM_WIDTH + 2;
        for u in -(PLATFORM_HALF_LEN + 2)..=(PLATFORM_HALF_LEN + 2) {
            for v in -half_w..=half_w {
                let (x, z) = world(u, v);
                let is_wall = v.abs() == half_w || u.abs() == PLATFORM_HALF_LEN + 2;
                for y in track_y - 1..=roof_y + 1 {
                    let block = if y == roof_y + 1 || y == track_y - 1 || is_wall {
                        SMOOTH_STONE
                    } else {
                        AIR
                    };
                    editor.set_block_absolute(block, x, y, z, None, None);
                }
                // Iluminação no teto
                if u.rem_euclid(8) == 0 && v.abs() == radius + 3 {
                    editor.set_block_absolute(GLOWSTONE, x, roof_y + 1, z, None, None);
                }
            }
        }
    }

    // ---------- 2. Plataformas laterais ----------
    for side in [-1, 1] {
        for u in -PLATFORM_HALF_LEN..=PLATFORM_HALF_LEN {
            for w in 1..=PLATFORM_WIDTH {
                let v = side * (radius + w);
                let (x, z) = world(u, v);
                // Base sólida até o solo (superfície/viaduto) ou só o piso (túnel)
                if !underground {
                    let g = editor.get_ground_level(x, z);
                    for y in (g + 1)..floor_y {
                        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, None);
                    }
                }
                let floor_block = if w == 1 {
                    YELLOW_CONCRETE
                } else {
                    SMOOTH_STONE
                };
                editor.set_block_absolute(floor_block, x, floor_y, z, None, None);
                for y in (floor_y + 1)..roof_y {
                    editor.set_block_absolute(AIR, x, y, z, None, None);
                }
                // Bancos
                if w == PLATFORM_WIDTH - 1 && u.rem_euclid(12) == 6 {
                    editor.set_block_absolute(SMOOTH_STONE_SLAB, x, floor_y + 1, z, None, None);
                }
                // Pilares da cobertura / da caixa
                if w == PLATFORM_WIDTH && u.rem_euclid(10) == 0 {
                    for y in (floor_y + 1)..roof_y {
                        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, None);
                    }
                }
                // Guarda-corpo no fundo da plataforma (superfície/viaduto)
                if w == PLATFORM_WIDTH && !underground {
                    let (bx, bz) = world(u, side * (radius + PLATFORM_WIDTH + 1));
                    editor.set_block_absolute(IRON_BARS, bx, floor_y + 1, bz, None, None);
                    if elevated {
                        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, bx, floor_y, bz, None, None);
                    }
                }
            }
        }
        // Rampas de acesso nas pontas (superfície): a plataforma desce até o solo
        if !underground && !elevated {
            for u_end in [-(PLATFORM_HALF_LEN + 1), PLATFORM_HALF_LEN + 1] {
                for w in 1..=PLATFORM_WIDTH {
                    let (x, z) = world(u_end, side * (radius + w));
                    let g = editor.get_ground_level(x, z);
                    editor.set_block_absolute(STONE_STAIRS, x, g + 1, z, None, None);
                }
            }
        }
    }

    // ---------- 3. Arquitetura: desenho próprio quando existe ----------
    // `landmarks.rs` já tem a Estação Guará (cobertura em abóbada de concreto)
    // e a Águas Claras (caixa metálica): entregamos a ele um polígono
    // retangular sintético alinhado à via, com as tags do nó, e só desenhamos
    // a cobertura genérica se nenhum monumento reivindicar a estação.
    let shell_half_w = radius + PLATFORM_WIDTH;
    let shell_ring = [
        world(-PLATFORM_HALF_LEN, -shell_half_w),
        world(PLATFORM_HALF_LEN, -shell_half_w),
        world(PLATFORM_HALF_LEN, shell_half_w),
        world(-PLATFORM_HALF_LEN, shell_half_w),
        world(-PLATFORM_HALF_LEN, -shell_half_w),
    ];
    let shell_way = ProcessedWay {
        id: node.id,
        nodes: shell_ring
            .iter()
            .map(|&(x, z)| ProcessedNode {
                id: 0,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect(),
        tags: node.tags.clone(),
    };
    let has_own_architecture = !underground
        && crate::element_processing::landmarks::generate_unique_landmark(
            editor, &shell_way, floor_y,
        );

    // ---------- 3b. Cobertura genérica (superfície e viaduto) ----------
    if !underground && !has_own_architecture {
        let half_w = radius + PLATFORM_WIDTH;
        for u in -PLATFORM_HALF_LEN..=PLATFORM_HALF_LEN {
            for v in -half_w..=half_w {
                let (x, z) = world(u, v);
                let edge = v.abs() == half_w;
                let block = if edge {
                    SMOOTH_STONE_SLAB
                } else {
                    LIGHT_GRAY_CONCRETE
                };
                editor.set_block_absolute(block, x, roof_y, z, None, None);
            }
        }
    }

    // ---------- 4. Placas com o nome ----------
    for u in [-PLATFORM_HALF_LEN + 10, 0, PLATFORM_HALF_LEN - 10] {
        for side in [-1, 1] {
            let (x, z) = world(u, side * (radius + PLATFORM_WIDTH));
            editor.set_sign(
                operator_line.to_string(),
                name.clone(),
                String::new(),
                String::new(),
                x,
                floor_y + 2 - editor.get_ground_level(x, z),
                z,
                0,
            );
        }
    }

    // ---------- 5. Acessos ----------
    let mut entrances: Vec<(i32, i32)> = rails
        .entrances
        .iter()
        .copied()
        .filter(|&(ex, ez)| {
            (((ex - cx) as f64).powi(2) + ((ez - cz) as f64).powi(2)).sqrt() <= ENTRANCE_SEARCH
        })
        .collect();
    if entrances.is_empty() {
        // Sem acesso mapeado: um em cada ponta, no lado do nó da estação
        let side = {
            let d = (node.x - cx) as f64 * nx + (node.z - cz) as f64 * nz;
            if d >= 0.0 {
                1
            } else {
                -1
            }
        };
        entrances.push(world(
            -PLATFORM_HALF_LEN + 6,
            side * (radius + PLATFORM_WIDTH + 4),
        ));
        entrances.push(world(
            PLATFORM_HALF_LEN - 6,
            side * (radius + PLATFORM_WIDTH + 4),
        ));
    }
    for (ex, ez) in entrances {
        // Ponto da plataforma mais próximo do acesso
        let du = (ex - cx) as f64 * ux + (ez - cz) as f64 * uz;
        let dv = (ex - cx) as f64 * nx + (ez - cz) as f64 * nz;
        let u = (du.round() as i32).clamp(-PLATFORM_HALF_LEN + 3, PLATFORM_HALF_LEN - 3);
        let side = if dv >= 0.0 { 1 } else { -1 };
        let (tx, tz) = world(u, side * (radius + PLATFORM_WIDTH));
        connect_entrance(editor, ex, ez, tx, tz, floor_y, underground, &name);
    }
}

/// Quiosque de acesso no nível da rua + escada (poço com escada de mão fechado
/// em vidro) até o nível da plataforma + corredor até a borda dela.
#[allow(clippy::too_many_arguments)]
fn connect_entrance(
    editor: &mut WorldEditor,
    ex: i32,
    ez: i32,
    tx: i32,
    tz: i32,
    floor_y: i32,
    underground: bool,
    name: &str,
) {
    let ground = editor.get_ground_level(ex, ez);
    generate_entrance_kiosk(editor, ex, ez, Some(name));

    // Poço vertical 3×3 com escada de mão na parede, do quiosque ao piso da plataforma
    let (lo, hi) = if floor_y < ground {
        (floor_y, ground)
    } else {
        (ground + 1, floor_y)
    };
    for y in lo..=hi {
        for dx in -1i32..=1 {
            for dz in -1i32..=1 {
                let is_wall = dx.abs() == 1 && dz.abs() == 1;
                let block = if is_wall { SMOOTH_STONE } else { AIR };
                editor.set_block_absolute(block, ex + dx, y, ez + dz, None, None);
            }
        }
        editor.set_block_absolute(SMOOTH_STONE, ex + 1, y, ez, None, None);
        editor.set_block_absolute(LADDER, ex, y, ez, None, None);
    }
    editor.set_block_absolute(SMOOTH_STONE, ex, lo - 1, ez, None, None);

    // Corredor no nível da plataforma até a borda dela (3 de largura)
    let path = bresenham_line(ex, 0, ez, tx, 0, tz);
    if path.len() > 120 {
        return;
    }
    for (px, _, pz) in path {
        for dx in -1..=1 {
            for dz in -1..=1 {
                editor.set_block_absolute(SMOOTH_STONE, px + dx, floor_y, pz + dz, None, None);
                for y in (floor_y + 1)..=(floor_y + 3) {
                    editor.set_block_absolute(AIR, px + dx, y, pz + dz, None, None);
                }
                if underground {
                    editor.set_block_absolute(
                        SMOOTH_STONE,
                        px + dx,
                        floor_y + 4,
                        pz + dz,
                        None,
                        None,
                    );
                }
            }
        }
        if !underground && floor_y > editor.get_ground_level(px, pz) + 1 {
            // Passarela elevada: guarda-corpo
            editor.set_block_absolute(IRON_BARS, px - 2, floor_y + 1, pz, None, None);
            editor.set_block_absolute(IRON_BARS, px + 2, floor_y + 1, pz, None, None);
        }
    }
}

/// Quiosque de acesso (5×5, vidro e concreto, com placa) no nível da rua.
pub fn generate_entrance_kiosk(editor: &mut WorldEditor, x: i32, z: i32, name: Option<&str>) {
    let y = editor.get_ground_level(x, z);
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            let edge = dx.abs() == 2 || dz.abs() == 2;
            editor.set_block_absolute(SMOOTH_STONE, x + dx, y, z + dz, None, None);
            if edge {
                let corner = dx.abs() == 2 && dz.abs() == 2;
                // Porta: vão no meio da face sul
                let is_door = dz == 2 && dx == 0;
                for dy in 1..=3 {
                    let block = if corner {
                        LIGHT_GRAY_CONCRETE
                    } else if is_door {
                        AIR
                    } else {
                        GLASS
                    };
                    editor.set_block_absolute(block, x + dx, y + dy, z + dz, None, None);
                }
            }
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x + dx, y + 4, z + dz, None, None);
        }
    }
    // Totem "M" azul do Metrô-DF
    editor.set_block_absolute(BLUE_CONCRETE, x + 3, y + 1, z + 3, None, None);
    editor.set_block_absolute(BLUE_CONCRETE, x + 3, y + 2, z + 3, None, None);
    editor.set_block_absolute(WHITE_CONCRETE, x + 3, y + 3, z + 3, None, None);
    editor.set_block_absolute(GLOWSTONE, x, y + 3, z, None, None);
    if let Some(n) = name {
        editor.set_sign(
            "Estação".to_string(),
            n.to_string(),
            String::new(),
            String::new(),
            x,
            2,
            z + 3,
            0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn rail_way(id: u64, tags: &[(&str, &str)], pts: &[(i32, i32)]) -> ProcessedElement {
        ProcessedElement::Way(Arc::new(ProcessedWay {
            id,
            nodes: pts
                .iter()
                .map(|&(x, z)| ProcessedNode {
                    id: 0,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }))
    }

    #[test]
    fn index_reads_level_from_tags_not_from_type() {
        let elements = vec![
            rail_way(1, &[("railway", "subway")], &[(0, 0), (100, 0)]),
            rail_way(
                2,
                &[("railway", "subway"), ("tunnel", "yes"), ("layer", "-1")],
                &[(0, 50), (100, 50)],
            ),
            rail_way(
                3,
                &[("railway", "subway"), ("bridge", "yes"), ("layer", "1")],
                &[(0, 100), (100, 100)],
            ),
            rail_way(4, &[("highway", "residential")], &[(0, 150), (100, 150)]),
        ];
        let idx = RailIndex::build(&elements);
        assert_eq!(idx.segments.len(), 3);
        let surface = idx.nearest(50, 3, 20.0).unwrap();
        assert!(!surface.segment.tunnel && !surface.segment.elevated && surface.segment.metro);
        assert_eq!(surface.segment.offset(), 0);
        let tunnel = idx.nearest(50, 52, 20.0).unwrap();
        assert!(tunnel.segment.tunnel);
        assert_eq!(tunnel.segment.offset(), -10);
        let viaduct = idx.nearest(50, 97, 20.0).unwrap();
        assert!(viaduct.segment.elevated);
        assert_eq!(viaduct.segment.offset(), 7);
        assert!(idx.nearest(50, 150, 20.0).is_none());
    }

    #[test]
    fn nearest_projects_onto_segment_and_reports_direction() {
        let idx = RailIndex::build(&[rail_way(1, &[("railway", "rail")], &[(0, 0), (0, 200)])]);
        let n = idx.nearest(15, 80, 30.0).unwrap();
        assert_eq!((n.px, n.pz), (0, 80));
        assert!(n.ux.abs() < 1e-9 && (n.uz - 1.0).abs() < 1e-9);
        assert!((n.dist - 15.0).abs() < 1e-9);
        assert!(!n.segment.double_track);
    }
}
