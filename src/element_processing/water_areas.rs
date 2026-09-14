//! Geração de corpos d'água representados como relações OSM (multipolígono).
//!
//! 🚨 BESM-6 — CORREÇÃO ESTRUTURAL: este arquivo antes continha um protótipo
//! obsoleto de geração de floresta por chunk (`generate_chunk`/`generate_branch`/
//! `generate_undergrowth`, com um `enum Block` próprio e uma interface de
//! callback abstrata) — nunca chamado por nada no motor, superado havia tempo
//! pela reimplementação real em `element_processing/tree.rs::generate_chunk`
//! (mesma arquitetura de ruído, porém usando o `WorldEditor`/`Block` de
//! verdade). Foi removido daqui.
//!
//! O nome do arquivo, no entanto, é usado por um caminho que **existe e é
//! chamado**: `data_processing.rs` despacha toda `ProcessedElement::Relation`
//! cuja tag `water=*` ou `natural=water/bay` bata para
//! `water_areas::generate_water_areas_from_relation` — uma função que nunca
//! havia sido escrita (o crate não compilava). Isto cobre lagos/represas
//! desenhados como multipolígono (contorno externo + ilhas internas), como o
//! Lago Paranoá — o caso que `natural::generate_natural_from_relation` (usado
//! para os demais `natural=*`) não intercepta, porque `natural=water`/`bay`
//! é desviado para cá antes de chegar naquele branch.

use crate::block_definitions::WATER;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill::{scanline_fill_complex, ComplexPolygon};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation};
use crate::world_editor::WorldEditor;

/// Desenha um corpo d'água a partir de uma relação multipolígono (anel externo
/// + anéis internos/ilhas). Reaproveita o rasterizador Scanline já usado por
/// `landuse`/`leisure`/`natural` (via `FloodFillCache`) — mas chamado direto,
/// sem cache, já que este caminho não recebe `Args`/`FloodFillCache` (só
/// `editor`, a relação e o `XZBBox` de recorte, conforme o `dispatch_element`
/// em `data_processing.rs`). A regra par-ímpar do Scanline já exclui as ilhas
/// automaticamente — não precisamos de uma máscara de exclusão separada.
pub fn generate_water_areas_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    xzbbox: &XZBBox,
) {
    let mut outer_ring: Vec<(i32, i32)> = Vec::new();
    let mut inner_rings: Vec<Vec<(i32, i32)>> = Vec::new();

    for member in &rel.members {
        let ring: Vec<(i32, i32)> = member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
        match member.role {
            ProcessedMemberRole::Outer => outer_ring.extend(ring),
            ProcessedMemberRole::Inner => inner_rings.push(ring),
            ProcessedMemberRole::Part => {}
        }
    }

    if outer_ring.len() < 3 {
        return;
    }

    let complex_polygon = ComplexPolygon {
        outer: outer_ring,
        inners: inner_rings,
    };

    // Sem `Args` disponível aqui, não há timeout configurável — o próprio
    // Scanline já tem um teto de segurança de área (30M blocos, ver
    // `floodfill.rs`), suficiente mesmo para o Lago Paranoá inteiro.
    let filled_area = scanline_fill_complex(&complex_polygon, None);

    for (x, z) in filled_area {
        if !xzbbox.contains(&XZPoint::new(x, z)) {
            continue;
        }

        let ground_y = editor.get_ground_level(x, z);
        editor.set_block_absolute(WATER, x, ground_y, z, None, None);
    }
}
