//! Quadras e campos esportivos (`leisure=pitch`) em escala de roleplay.
//!
//! 🚨 CORREÇÃO DE QUALIDADE: `leisure=pitch` era só um preenchimento liso de
//! terracota verde — nenhuma linha, trave, cesta, rede, alambrado ou poste. No
//! Guará (87 `leisure=pitch` no recorte real, a maioria quadras poliesportivas
//! das QEs/QIs, o equipamento público mais característico da cidade) isso
//! lia como um gramado verde qualquer.
//!
//! Cada quadra agora é desenhada num **referencial próprio** (retângulo de área
//! mínima que envolve o polígono, eixo `u` no comprimento e `v` na largura),
//! e recebe, conforme `sport=*`:
//!
//! | modalidade | piso | marcação | equipamento |
//! |---|---|---|---|
//! | `soccer` (campo) | grama (ou `surface`) | perímetro, meio, círculo central, grandes/pequenas áreas | traves com rede |
//! | `multi` / sem `sport` (poliesportiva) | concreto pintado verde, faixa lateral cinza | futsal + basquete | traves de futsal, tabelas |
//! | `basketball` | concreto pintado | perímetro, meio, círculo, garrafões, arcos de 3 pontos | tabelas com aro |
//! | `tennis` | saibro (ou `paved` → verde) | duplas, simples, saque, linha central | rede baixa com postes |
//! | `volleyball` / `beachvolleyball` | areia | perímetro, meio, linhas de ataque | rede alta com postes |
//! | `skateboard` | concreto | — | rampas e caixotes |
//!
//! Quadras (não campos grandes) ganham **alambrado** de 3 blocos no perímetro,
//! com portão, e **postes de iluminação** nos quatro cantos — o padrão NOVACAP.

use crate::block_definitions::*;
use crate::element_processing::amenities::place_neoenergia_pole;
use crate::element_processing::barriers;
use crate::element_processing::oriented_frame::OrientedFrame;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SportKind {
    Soccer,
    Multi,
    Basketball,
    Tennis,
    Volleyball,
    BeachVolleyball,
    Skate,
    Other,
}

/// Classifica a modalidade a partir de `sport=*` (primeiro valor de uma lista
/// `a;b;c`) e do tamanho: sem tag, área grande é campo de futebol, área
/// pequena é quadra poliesportiva (o caso típico do Guará).
pub fn classify(way: &ProcessedWay, area_blocks: usize) -> SportKind {
    let sport = way
        .tags
        .get("sport")
        .map(|s| s.split(';').next().unwrap_or("").trim().to_lowercase())
        .unwrap_or_default();
    match sport.as_str() {
        "soccer" | "football" | "american_football" | "rugby" => SportKind::Soccer,
        "basketball" => SportKind::Basketball,
        "tennis" | "padel" => SportKind::Tennis,
        "volleyball" => SportKind::Volleyball,
        "beachvolleyball" | "beach_volleyball" | "footvolley" => SportKind::BeachVolleyball,
        "skateboard" | "skate" | "bmx" => SportKind::Skate,
        "multi" | "futsal" | "handball" => SportKind::Multi,
        "" => {
            if area_blocks > 3500 {
                SportKind::Soccer
            } else {
                SportKind::Multi
            }
        }
        _ => SportKind::Other,
    }
}

/// Piso padrão da modalidade quando o OSM não traz `surface`.
pub fn default_surface(kind: SportKind, way: &ProcessedWay) -> Block {
    match kind {
        SportKind::Soccer => GRASS_BLOCK,
        SportKind::Multi | SportKind::Basketball => GREEN_CONCRETE,
        SportKind::Tennis => {
            if way
                .tags
                .get("surface")
                .is_some_and(|s| s == "paved" || s == "asphalt" || s == "concrete")
            {
                GREEN_CONCRETE
            } else {
                RED_TERRACOTTA
            }
        }
        SportKind::Volleyball => GREEN_CONCRETE,
        SportKind::BeachVolleyball => SAND,
        SportKind::Skate => LIGHT_GRAY_CONCRETE,
        SportKind::Other => GREEN_TERRACOTTA,
    }
}

const LINE: Block = WHITE_CONCRETE;
const LINE_TOL: f64 = 0.55;

#[inline]
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= LINE_TOL
}

/// Desenha marcação, equipamento, alambrado e iluminação sobre um `leisure=pitch`
/// já preenchido (`filled_area`) por `leisure.rs`.
pub fn generate_pitch(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    filled_area: &[(i32, i32)],
    kind: SportKind,
) {
    let ring: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let Some(frame) = OrientedFrame::from_polygon(&ring) else {
        return;
    };
    // Muito pequeno para marcação legível
    if frame.half_len < 6.0 || frame.half_wid < 4.0 {
        return;
    }
    let l = frame.half_len - 1.0; // linha de fundo, 1 bloco para dentro
    let w = frame.half_wid - 1.0;
    let lit = way.tags.get("lit").is_some_and(|v| v == "yes");
    let is_big_field = kind == SportKind::Soccer && filled_area.len() > 3500;

    // ---------- 1. Marcação horizontal ----------
    for &(x, z) in filled_area {
        let (u, v) = frame.local(x, z);
        let (au, av) = (u.abs(), v.abs());
        if au > l + LINE_TOL || av > w + LINE_TOL {
            continue;
        }
        let ground_y = editor.get_ground_level(x, z);
        let mark = match kind {
            SportKind::Soccer | SportKind::Multi => {
                let center_r = (w * 0.3).min(9.0);
                let pen_d = (l * 0.16).max(4.0);
                let pen_w = (w * 0.62).max(5.0);
                let goal_d = (l * 0.055).max(2.0);
                let goal_w = (w * 0.3).max(3.0);
                let perimeter = near(au, l) || near(av, w);
                let mid = near(au, 0.0);
                let circle = near((u * u + v * v).sqrt(), center_r);
                let pen_box =
                    (near(au, l - pen_d) && av <= pen_w) || (near(av, pen_w) && au >= l - pen_d);
                let goal_box = (near(au, l - goal_d) && av <= goal_w)
                    || (near(av, goal_w) && au >= l - goal_d);
                let mut m = perimeter || mid || circle || pen_box || goal_box;
                if kind == SportKind::Multi {
                    // Marcação de basquete sobreposta: garrafão e arco de 3 pontos
                    let key_d = l * 0.19;
                    let key_w = w * 0.34;
                    let key = (near(au, l - key_d) && av <= key_w)
                        || (near(av, key_w) && au >= l - key_d);
                    let basket_u = l - 1.5;
                    let arc_r = (w * 0.8).min(l * 0.3);
                    let du = au - basket_u;
                    let arc = near((du * du + v * v).sqrt(), arc_r) && au < basket_u;
                    m = m || key || arc;
                }
                m
            }
            SportKind::Basketball => {
                let key_d = l * 0.19;
                let key_w = w * 0.34;
                let perimeter = near(au, l) || near(av, w);
                let mid = near(au, 0.0);
                let circle = near((u * u + v * v).sqrt(), (w * 0.22).min(5.0));
                let key =
                    (near(au, l - key_d) && av <= key_w) || (near(av, key_w) && au >= l - key_d);
                let basket_u = l - 1.5;
                let arc_r = (w * 0.8).min(l * 0.3);
                let du = au - basket_u;
                let arc = near((du * du + v * v).sqrt(), arc_r) && au < basket_u;
                perimeter || mid || circle || key || arc
            }
            SportKind::Tennis => {
                let singles = w * 0.82;
                let service = l * 0.55;
                let perimeter = near(au, l) || near(av, w);
                let singles_line = near(av, singles);
                let service_line = near(au, service) && av <= singles;
                let center_line = near(av, 0.0) && au <= service;
                perimeter || singles_line || service_line || center_line
            }
            SportKind::Volleyball | SportKind::BeachVolleyball => {
                let attack = l * 0.33;
                near(au, l) || near(av, w) || near(au, 0.0) || near(au, attack)
            }
            SportKind::Skate | SportKind::Other => false,
        };
        if mark {
            editor.set_block_absolute(LINE, x, ground_y, z, None, None);
        } else if kind == SportKind::Multi || kind == SportKind::Basketball {
            // Faixa de escape lateral em cinza fora das linhas de jogo (padrão das
            // quadras públicas: área de jogo colorida, borda neutra).
            if au > l - 0.5 || av > w - 0.5 {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, ground_y, z, None, None);
            }
        }
    }

    // ---------- 2. Equipamento ----------
    match kind {
        SportKind::Soccer | SportKind::Multi => {
            let goal_half = if kind == SportKind::Soccer {
                (w * 0.15).clamp(3.0, 5.0)
            } else {
                2.0
            };
            let height = if kind == SportKind::Soccer { 3 } else { 2 };
            for end in [-1.0_f64, 1.0] {
                place_goal(editor, &frame, end * l, goal_half, height);
            }
        }
        SportKind::Basketball => {
            for end in [-1.0_f64, 1.0] {
                place_hoop(editor, &frame, end * (l - 1.5), end);
            }
        }
        SportKind::Tennis => place_net(editor, &frame, w, 1),
        SportKind::Volleyball | SportKind::BeachVolleyball => place_net(editor, &frame, w, 2),
        SportKind::Skate => place_skate_features(editor, &frame, l, w),
        SportKind::Other => {}
    }
    if kind == SportKind::Multi {
        for end in [-1.0_f64, 1.0] {
            place_hoop(editor, &frame, end * (l - 1.5), end);
        }
    }

    // ---------- 3. Alambrado (quadras) e iluminação ----------
    if !is_big_field {
        place_fence(editor, way, 3);
    }
    if lit || !is_big_field {
        for (su, sv) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            let (px, pz) = frame.world(su * (frame.half_len + 1.0), sv * (frame.half_wid + 1.0));
            place_light_pole(editor, px, pz, if is_big_field { 9 } else { 7 });
        }
    }
}

fn place_goal(editor: &mut WorldEditor, frame: &OrientedFrame, u: f64, half: f64, height: i32) {
    let (gx, gz) = frame.world(u, 0.0);
    let base_y = editor.get_ground_level(gx, gz);
    let n = half.round() as i32;
    for v in -n..=n {
        let (px, pz) = frame.world(u, v as f64);
        let is_post = v == -n || v == n;
        for dy in 1..=height {
            if is_post || dy == height {
                editor.set_block_absolute(WHITE_CONCRETE, px, base_y + dy, pz, None, None);
            }
        }
        // Rede atrás da trave
        let (nx, nz) = frame.world(u + u.signum(), v as f64);
        for dy in 1..height {
            editor.set_block_if_absent_absolute(IRON_BARS, nx, base_y + dy, nz);
        }
    }
}

fn place_hoop(editor: &mut WorldEditor, frame: &OrientedFrame, u: f64, end_sign: f64) {
    let (px, pz) = frame.world(u, 0.0);
    let base_y = editor.get_ground_level(px, pz);
    for dy in 1..=3 {
        editor.set_block_absolute(IRON_BARS, px, base_y + dy, pz, None, None);
    }
    // Tabela (3 de largura, 2 de altura) virada para a quadra
    for v in -1..=1 {
        let (bx, bz) = frame.world(u, v as f64);
        editor.set_block_absolute(WHITE_CONCRETE, bx, base_y + 4, bz, None, None);
        editor.set_block_absolute(WHITE_CONCRETE, bx, base_y + 5, bz, None, None);
    }
    // Aro laranja projetado para dentro da quadra
    let (hx, hz) = frame.world(u - end_sign, 0.0);
    editor.set_block_absolute(ORANGE_CONCRETE, hx, base_y + 4, hz, None, None);
}

fn place_net(editor: &mut WorldEditor, frame: &OrientedFrame, w: f64, net_height: i32) {
    let n = (w + 1.0).round() as i32;
    for v in -n..=n {
        let (px, pz) = frame.world(0.0, v as f64);
        let base_y = editor.get_ground_level(px, pz);
        let is_post = v == -n || v == n;
        if is_post {
            for dy in 1..=(net_height + 1) {
                editor.set_block_absolute(IRON_BARS, px, base_y + dy, pz, None, None);
            }
        } else {
            for dy in 1..=net_height {
                editor.set_block_if_absent_absolute(IRON_BARS, px, base_y + dy, pz);
            }
        }
    }
}

fn place_skate_features(editor: &mut WorldEditor, frame: &OrientedFrame, l: f64, w: f64) {
    // Duas rampas (escadas de pedra) nas extremidades e um caixote central
    for end in [-1.0_f64, 1.0] {
        for v in -2..=2 {
            let (px, pz) = frame.world(end * (l - 3.0), v as f64);
            let y = editor.get_ground_level(px, pz);
            editor.set_block_absolute(STONE_STAIRS, px, y + 1, pz, None, None);
            let (qx, qz) = frame.world(end * (l - 2.0), v as f64);
            editor.set_block_absolute(SMOOTH_STONE, qx, y + 1, qz, None, None);
        }
    }
    let box_w = (w * 0.3).clamp(1.0, 3.0) as i32;
    for u in -3..=3 {
        for v in -box_w..=box_w {
            let (px, pz) = frame.world(u as f64, v as f64);
            let y = editor.get_ground_level(px, pz);
            editor.set_block_absolute(SMOOTH_STONE_SLAB, px, y + 1, pz, None, None);
        }
    }
}

/// Alambrado no perímetro da quadra: reutiliza o gerador de cercas do motor
/// (`barriers::generate_barriers`, tipologia `fence_type=chain_link` — o
/// alambrado NOVACAP com postes e arame no topo) sobre um way sintético com o
/// anel da quadra, em vez de uma segunda implementação de cerca.
/// Alambrado de tela (chain link) ao redor do contorno de `way`, desenhado
/// pelo módulo canônico de cercas (`barriers::generate_barriers`) através de
/// um way sintético `barrier=fence`. As tags `amenity`, `landuse` e `name` do
/// way original são copiadas para que `barriers` aplique a semântica que já
/// conhece (escola/militar → alambrado alto de segurança, embaixada → grade
/// institucional). Também usado pelos pátios institucionais (`amenities.rs`).
pub fn place_fence(editor: &mut WorldEditor, way: &ProcessedWay, height: i32) {
    let mut tags: HashMap<String, String> = HashMap::new();
    tags.insert("barrier".to_string(), "fence".to_string());
    tags.insert("fence_type".to_string(), "chain_link".to_string());
    tags.insert("height".to_string(), height.to_string());
    for key in ["amenity", "landuse", "name"] {
        if let Some(v) = way.tags.get(key) {
            tags.insert(key.to_string(), v.clone());
        }
    }
    let mut nodes: Vec<ProcessedNode> = way.nodes.clone();
    if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
        if (first.x, first.z) != (last.x, last.z) {
            let first = first.clone();
            nodes.push(first);
        }
    }
    let fence = ProcessedElement::Way(Arc::new(ProcessedWay {
        id: way.id ^ 0x5A5A_0000_0000_0001,
        nodes,
        tags,
    }));
    barriers::generate_barriers(editor, &fence);
}

/// Poste de iluminação da quadra: o mesmo poste padrão Neoenergia usado nos
/// estacionamentos (`amenities::place_neoenergia_pole`).
fn place_light_pole(editor: &mut WorldEditor, x: i32, z: i32, _height: i32) {
    let y = editor.get_ground_level(x, z);
    if editor.block_at_absolute(x, y + 1, z) {
        return;
    }
    place_neoenergia_pole(editor, x, y, z);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_defaults_by_size_and_reads_sport_lists() {
        use std::collections::HashMap;
        let mk = |sport: Option<&str>| ProcessedWay {
            id: 1,
            nodes: vec![],
            tags: sport
                .map(|s| HashMap::from([("sport".to_string(), s.to_string())]))
                .unwrap_or_default(),
        };
        assert_eq!(classify(&mk(None), 800), SportKind::Multi);
        assert_eq!(classify(&mk(None), 8000), SportKind::Soccer);
        assert_eq!(
            classify(&mk(Some("soccer;basketball;volleyball")), 800),
            SportKind::Soccer
        );
        assert_eq!(
            classify(&mk(Some("beachvolleyball")), 800),
            SportKind::BeachVolleyball
        );
        assert_eq!(default_surface(SportKind::BeachVolleyball, &mk(None)), SAND);
    }
}
