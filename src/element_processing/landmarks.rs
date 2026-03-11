use crate::block_definitions::*;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use std::f64::consts::PI;

/// Escala global do mapa para referência matemática interna (Tier Governamental)
const V_SCALE: f64 = 1.15;
const H_SCALE: f64 = 1.33;

/// Estrutura Mestra de Interceptação de Marcos Urbanos (Landmarks) de Brasília.
/// Retorna `true` se o edifício for reconhecido e gerado por este módulo.
/// Retorna `false` para devolver o controle ao gerador procedural padrão.
pub fn generate_unique_landmark(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground_y: i32,
) -> bool {
    // ?? BESM-6 Tweak: A Guarda de Respeito Fotogramétrico
    // Se o elemento veio dos nossos provedores de ultra-precisão 3D (LOD2/LOD3/Mesh), 
    // nós ABORTAMOS a geração procedimental aproximada e deixamos o motor Voxel desenhar 
    // o modelo a laser exato que extraímos do governo no main.rs.
    let source = element.tags.get("source").map(|s| s.as_str()).unwrap_or("");
    if source == "GDF_CityGML_3D" || source == "GDF_Mesh_Voxel" || source == "Photogrammetry_Mesh" {
        return false; 
    }

    let name = element
        .tags
        .get("name")
        .or_else(|| element.tags.get("building:name"))
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    let is_station = element.tags.get("building").map(|s| s.as_str()) == Some("train_station")
        || element.tags.get("railway").map(|s| s.as_str()) == Some("station")
        || element.tags.get("station").map(|s| s.as_str()) == Some("subway");

    // =====================================================
    // 1. EIXO MONUMENTAL E PALÁCIOS (Niemeyer/Lucio Costa)
    // =====================================================

    if name.contains("congresso nacional")
        || name.contains("câmara dos deputados")
        || name.contains("senado federal")
    {
        generate_congresso(editor, element, ground_y);
        return true;
    }

    if name.contains("palácio do planalto") || name.contains("palacio do planalto") {
        generate_palacio_planalto(editor, element, ground_y);
        return true;
    }

    if name.contains("supremo tribunal federal") || name.contains("stf") {
        generate_stf(editor, element, ground_y);
        return true;
    }

    if name.contains("alvorada") {
        generate_palacio_alvorada(editor, element, ground_y);
        return true;
    }

    if name.contains("itamaraty") || name.contains("palácio dos arcos") {
        generate_itamaraty(editor, element, ground_y);
        return true;
    }

    if name.contains("justiça") && name.contains("palácio") {
        generate_palacio_justica(editor, element, ground_y);
        return true;
    }

    if name.contains("ministério da") || name.contains("ministerio da") || name.contains("ministério do") || name.contains("ministerio do") {
        generate_ministerio(editor, element, ground_y);
        return true;
    }

    if name.contains("catedral metropolitana") {
        generate_catedral(editor, element, ground_y);
        return true;
    }

    if name.contains("museu nacional") && name.contains("república") {
        generate_museu_nacional(editor, element, ground_y);
        return true;
    }

    if name.contains("teatro nacional") || name.contains("cláudio santoro") {
        generate_teatro_nacional(editor, element, ground_y);
        return true;
    }

    // =====================================================
    // 2. MARCOS URBANOS E INFRAESTRUTURA GIGANTE
    // =====================================================

    if name.contains("torre de tv") && !name.contains("digital") {
        generate_torre_tv(editor, element, ground_y);
        return true;
    }

    if name.contains("memorial jk") || name.contains("juscelino kubitschek") {
        generate_memorial_jk(editor, element, ground_y);
        return true;
    }

    if name.contains("rodoviária do plano piloto") || name.contains("rodoviaria do plano piloto") {
        generate_rodoviaria(editor, element, ground_y);
        return true;
    }

    if name.contains("estádio nacional") || name.contains("mané garrincha") || name.contains("mane garrincha") {
        generate_estadio_nacional(editor, element, ground_y);
        return true;
    }

    if name.contains("parque da cidade") && name.contains("pavil") {
        generate_pavilhao_parque_cidade(editor, element, ground_y);
        return true;
    }

    // =====================================================
    // 3. SETOR BANCÁRIO (Colossos Brutalistas)
    // =====================================================

    if name.contains("banco do brasil") && name.contains("sede") {
        generate_sede_bb(editor, element, ground_y);
        return true;
    }

    if (name.contains("caixa econômica") || name.contains("caixa economica")) && name.contains("matriz") {
        generate_sede_cef(editor, element, ground_y);
        return true;
    }

    if name.contains("banco central do brasil") || name.contains("bc") || name.contains("bacen") {
        generate_sede_bc(editor, element, ground_y);
        return true;
    }

    if (name.contains("banco de brasília") || name.contains("brb")) && name.contains("sede") {
        generate_sede_brb(editor, element, ground_y);
        return true;
    }

    // =====================================================
    // 4. SETOR DE HOTÉIS E COMÉRCIO
    // =====================================================

    if name.contains("conjunto nacional") {
        generate_conjunto_nacional(editor, element, ground_y);
        return true;
    }

    if name.contains("hotel nacional") {
        generate_hotel_nacional(editor, element, ground_y);
        return true;
    }

    // =====================================================
    // 5. MONUMENTOS DE BAIRRO
    // =====================================================

    if name.contains("igrejinha") || (name.contains("nossa senhora de fátima") && name.contains("igreja")) {
        generate_igrejinha(editor, element, ground_y);
        return true;
    }

    if is_station && (name.contains("guará") || name.contains("guara")) {
        generate_estacao_guara(editor, element, ground_y);
        return true;
    }

    if is_station && (name.contains("águas claras") || name.contains("aguas claras")) {
        generate_estacao_aguas_claras(editor, element, ground_y);
        return true;
    }

    false
}

// ============================================================================
// HELPER GEOMÉTRICO (Rigor de Rotação e Orientação)
// ============================================================================

struct OrientedBounds {
    min_x: i32, max_x: i32, min_z: i32, max_z: i32,
    cx: i32, cz: i32,
    angle_rad: f64, // O ângulo de rotação da planta baixa no mapa real
}

/// Extrai a bounding box E calcula o ângulo de inclinação do maior segmento do prédio.
/// ?? BESM-6 Tweak: Aplica o alargamento horizontal (H_SCALE) no esqueleto do prédio, 
/// empurrando as bordas para longe do centro de massa para acompanhar as rodovias.
fn get_oriented_bounds(element: &ProcessedWay) -> OrientedBounds {
    let raw_min_x = element.nodes.iter().map(|n| n.x).min().unwrap_or(0);
    let raw_max_x = element.nodes.iter().map(|n| n.x).max().unwrap_or(0);
    let raw_min_z = element.nodes.iter().map(|n| n.z).min().unwrap_or(0);
    let raw_max_z = element.nodes.iter().map(|n| n.z).max().unwrap_or(0);
    let cx = (raw_min_x + raw_max_x) / 2;
    let cz = (raw_min_z + raw_max_z) / 2;

    // Alarga a base 2D a partir do centro com proteção de precisão
    let half_w = (((raw_max_x - cx) as f64) * H_SCALE).round() as i32;
    let half_l = (((raw_max_z - cz) as f64) * H_SCALE).round() as i32;

    let min_x = cx - half_w;
    let max_x = cx + half_w;
    let min_z = cz - half_l;
    let max_z = cz + half_l;

    let mut longest_segment = 0.0;
    let mut main_angle = 0.0;

    for i in 0..element.nodes.len().saturating_sub(1) {
        let dx = (element.nodes[i+1].x - element.nodes[i].x) as f64;
        let dz = (element.nodes[i+1].z - element.nodes[i].z) as f64;
        let len = dx*dx + dz*dz;
        if len > longest_segment {
            longest_segment = len;
            main_angle = dz.atan2(dx); // atan2 retorna o ângulo do vetor
        }
    }

    OrientedBounds { min_x, max_x, min_z, max_z, cx, cz, angle_rad: main_angle }
}

/// Helper para converter coordenadas locais (do desenho geométrico puro) para globais,
/// aplicando a matriz de rotação descoberta do mapa real.
#[inline(always)]
fn rot_x(lx: f64, lz: f64, angle: f64, cx: i32) -> i32 {
    cx + (lx * angle.cos() - lz * angle.sin()).round() as i32
}

#[inline(always)]
fn rot_z(lx: f64, lz: f64, angle: f64, cz: i32) -> i32 {
    cz + (lx * angle.sin() + lz * angle.cos()).round() as i32
}

// ============================================================================
// MATEMÁTICA DOS MONUMENTOS (Rigor Escala Real e Matriz de Rotação)
// ============================================================================

fn generate_congresso(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    // 1. Laje Monumental (Baseada no polígono real, independente de rotação)
    for x in bounds.min_x..=bounds.max_x {
        for z in bounds.min_z..=bounds.max_z {
            editor.set_block_absolute(WHITE_CONCRETE, x, ground_y + 1, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x, ground_y + 2, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x, ground_y + 3, z, None, None);
        }
    }

    // 2. Torres Gêmeas (Perfil H) - Usando espaço local (lx, lz) antes da rotação
    let tower_height = (100.0 * V_SCALE) as i32; // Realidade: 100 metros de altura
    let rx = (13.0 * H_SCALE) as i32;
    let rz = (15.0 * H_SCALE) as i32;
    let h_gap = (6.0 * H_SCALE) as i32; // Vão central ampliado

    for y in (ground_y + 4)..=(ground_y + tower_height) {
        for lx in -rx..=rx {
            for lz in -rz..=rz {
                if lx >= -h_gap && lx <= h_gap { continue; } // O Vazio do H

                let px = rot_x(lx as f64, lz as f64, angle, cx);
                let pz = rot_z(lx as f64, lz as f64, angle, cz);

                // Fachadas de vidro na frente/trás, concreto cego nos lados
                if lz == -rz || lz == rz {
                    editor.set_block_absolute(BLACK_STAINED_GLASS, px, y, pz, None, None);
                } else if lx == -rx || lx == rx || lx == -h_gap - 1 || lx == h_gap + 1 {
                    editor.set_block_absolute(WHITE_CONCRETE, px, y, pz, None, None);
                } else {
                    editor.set_block_absolute(SMOOTH_STONE, px, y, pz, None, None);
                }
            }
        }
    }

    // 3. Cúpula Convexa (Senado) - Calota Esférica Clássica
    let senado_lx = -(35.0 * H_SCALE) as i32; // Deslocado para a esquerda na planta local
    let radius = (18.0 * H_SCALE) as i32;
    for lx in (senado_lx - radius)..=(senado_lx + radius) {
        for lz in -radius..=radius {
            let dist_sq = (lx - senado_lx).pow(2) + lz.pow(2);
            if dist_sq <= radius * radius {
                // BESM-6 Tweak: Proteção Math pura .max(0.0)
                let diff = ((radius * radius - dist_sq) as f64).max(0.0);
                let h = diff.sqrt() as i32;
                let flat_h = (h as f64 * 0.6 * V_SCALE) as i32; // Achatamento da calota
                let px = rot_x(lx as f64, lz as f64, angle, cx);
                let pz = rot_z(lx as f64, lz as f64, angle, cz);

                for y in (ground_y + 4)..=(ground_y + 4 + flat_h) {
                    let is_surface = y == ground_y + 4 + flat_h;
                    let block = if is_surface { SMOOTH_QUARTZ } else { WHITE_CONCRETE };
                    editor.set_block_absolute(block, px, y, pz, None, None);
                }
            }
        }
    }

    // 4. Cúpula Côncava (Câmara) - Bacia/Parábola Invertida ( y = x² + z² )
    let camara_lx = (35.0 * H_SCALE) as i32; // Deslocado para a direita na planta local
    let radius_camara = (22.0 * H_SCALE) as i32;
    let max_h = (12.0 * V_SCALE) as i32;
    for lx in (camara_lx - radius_camara)..=(camara_lx + radius_camara) {
        for lz in -radius_camara..=radius_camara {
            let dist_sq = (lx - camara_lx).pow(2) + lz.pow(2);
            if dist_sq <= radius_camara * radius_camara {
                let norm_dist = (dist_sq as f64).sqrt() / (radius_camara as f64);
                let h = (norm_dist * norm_dist * max_h as f64) as i32; // Parábola pura

                let px = rot_x(lx as f64, lz as f64, angle, cx);
                let pz = rot_z(lx as f64, lz as f64, angle, cz);

                for y in (ground_y + 4)..=(ground_y + 4 + max_h - h) {
                    let is_surface = y == ground_y + 4 + max_h - h || y == ground_y + 4;
                    let block = if is_surface { SMOOTH_QUARTZ } else { WHITE_CONCRETE };
                    editor.set_block_absolute(block, px, y, pz, None, None);
                }
            }
        }
    }
}

fn generate_palacio_planalto(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (20.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;

                if y == ground_y + 1 || y == ground_y + height {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if is_edge && (x + z) % 6 == 0 {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if !is_edge && (x > bounds.min_x + 2 && x < bounds.max_x - 2 && z > bounds.min_z + 2 && z < bounds.max_z - 2) {
                    let is_inner_edge = x == bounds.min_x + 3 || x == bounds.max_x - 3 || z == bounds.min_z + 3 || z == bounds.max_z - 3;
                    if is_inner_edge {
                        editor.set_block_absolute(BLACK_STAINED_GLASS, x, y, z, None, None);
                    } else if y % 4 == 0 {
                        editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_stf(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (15.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;

                if y == ground_y + 1 || y == ground_y + height {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if is_edge && (x + z) % 5 == 0 {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if !is_edge && (x > bounds.min_x + 2 && x < bounds.max_x - 2 && z > bounds.min_z + 2 && z < bounds.max_z - 2) {
                    let is_inner_edge = x == bounds.min_x + 3 || x == bounds.max_x - 3 || z == bounds.min_z + 3 || z == bounds.max_z - 3;
                    if is_inner_edge {
                        editor.set_block_absolute(TINTED_GLASS, x, y, z, None, None);
                    } else if y % 5 == 0 {
                        editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_palacio_alvorada(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (12.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;

                if y == ground_y + height {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if is_edge && (x + z) % 8 == 0 {
                    editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None);
                } else if !is_edge && (x > bounds.min_x + 1 && x < bounds.max_x - 1 && z > bounds.min_z + 1 && z < bounds.max_z - 1) {
                    let is_glass = x == bounds.min_x + 2 || x == bounds.max_x - 2 || z == bounds.min_z + 2 || z == bounds.max_z - 2;
                    if is_glass {
                        editor.set_block_absolute(GLASS, x, y, z, None, None);
                    } else if y == ground_y + 1 {
                        editor.set_block_absolute(POLISHED_DIORITE, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_itamaraty(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (24.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;

                if y == ground_y + 1 {
                    editor.set_block_absolute(WATER, x, y, z, None, None);
                } else if y == ground_y + height {
                    editor.set_block_absolute(SMOOTH_STONE, x, y, z, None, None);
                } else if is_edge && (x + z) % 6 == 0 {
                    editor.set_block_absolute(SMOOTH_STONE, x, y, z, None, None);
                } else if !is_edge && (x > bounds.min_x + 3 && x < bounds.max_x - 3 && z > bounds.min_z + 3 && z < bounds.max_z - 3) {
                    let is_glass = x == bounds.min_x + 4 || x == bounds.max_x - 4 || z == bounds.min_z + 4 || z == bounds.max_z - 4;
                    if is_glass {
                        editor.set_block_absolute(BLACK_STAINED_GLASS, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_palacio_justica(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (22.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;

                if y == ground_y || y == ground_y + height {
                    editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
                } else if is_edge {
                    if (x + z) % 8 == 0 {
                        editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
                    } else if y < ground_y + 10 && z == bounds.max_z {
                        editor.set_block_absolute(WATER, x, y, z, None, None); // Cascatas
                    } else {
                        editor.set_block_absolute(TINTED_GLASS, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_ministerio(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    let rx = (((bounds.max_x - bounds.min_x) / 2) as f64).max(10.0) as i32;
    let rz = (((bounds.max_z - bounds.min_z) / 2) as f64).max(15.0) as i32;
    let height = (46.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for lx in -rx..=rx {
            for lz in -rz..=rz {
                let px = rot_x(lx as f64, lz as f64, angle, cx);
                let pz = rot_z(lx as f64, lz as f64, angle, cz);

                let is_x_wall = lx == -rx || lx == rx;
                let is_z_wall = lz == -rz || lz == rz;

                if y == ground_y || y == ground_y + height {
                    editor.set_block_absolute(WHITE_CONCRETE, px, y, pz, None, None);
                } else if is_x_wall {
                    editor.set_block_absolute(WHITE_CONCRETE, px, y, pz, None, None); // Empena
                } else if is_z_wall {
                    editor.set_block_absolute(CYAN_STAINED_GLASS, px, y, pz, None, None); // Fachada vidro
                } else if y % 5 == 0 {
                    editor.set_block_absolute(SMOOTH_STONE, px, y, pz, None, None); // Lajes internas
                }
            }
        }
    }
}

fn generate_catedral(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let radius = 25.0 * H_SCALE;
    let height = (40.0 * V_SCALE) as i32;
    let pool_radius = (30.0 * H_SCALE) as i32;

    for x in (cx - pool_radius)..=(cx + pool_radius) {
        for z in (cz - pool_radius)..=(cz + pool_radius) {
            let dist = (((x - cx).pow(2) + (z - cz).pow(2)) as f64).sqrt();
            if dist <= pool_radius as f64 - 2.0 { editor.set_block_absolute(WATER, x, ground_y, z, None, None); }
            if dist <= radius { editor.set_block_absolute(SMOOTH_QUARTZ, x, ground_y + 1, z, None, None); }
        }
    }

    // Hiperboloide de uma folha regrado matematicamente para a curvatura perfeita dos 16 pilares
    for y in 0..=height {
        let normalized_y = (y as f64 / height as f64) * 2.0 - 1.0;
        let current_radius = radius * (0.4 + 0.6 * normalized_y.powi(2));

        for angle_deg in 0..360 {
            let angle = (angle_deg as f64) * PI / 180.0;
            let px = cx + (current_radius * angle.cos()).round() as i32;
            let pz = cz + (current_radius * angle.sin()).round() as i32;

            if angle_deg % 22 == 0 { // 360 / 16 pilares = 22.5 graus de passo
                editor.set_block_absolute(WHITE_CONCRETE, px, ground_y + 1 + y, pz, None, None);
            } else {
                let glass_color = if y % 3 == 0 { BLUE_STAINED_GLASS } else { LIGHT_BLUE_STAINED_GLASS };
                editor.set_block_absolute(glass_color, px, ground_y + 1 + y, pz, None, None);
            }
        }
    }
}

fn generate_museu_nacional(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let radius = (22.0 * H_SCALE) as i32;

    for x in (cx - radius)..=(cx + radius) {
        for z in (cz - radius)..=(cz + radius) {
            let dist_sq = (x - cx).pow(2) + (z - cz).pow(2);
            if dist_sq <= radius * radius {
                let diff = ((radius * radius - dist_sq) as f64).max(0.0);
                let max_h = diff.sqrt() as i32;
                for y in (ground_y + 1)..=(ground_y + max_h) {
                    let is_surface = y == ground_y + max_h;
                    let block = if is_surface { WHITE_CONCRETE } else { AIR };
                    editor.set_block_absolute(block, x, y, z, None, None);
                }
            }
        }
    }
}

fn generate_teatro_nacional(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    let rx = (((bounds.max_x - bounds.min_x) / 2) as f64).max(15.0) as i32;
    let rz = (((bounds.max_z - bounds.min_z) / 2) as f64).max(25.0) as i32;
    let height = (35.0 * V_SCALE) as i32; // Frusto piramidal de 35 metros

    for y in 0..height {
        let shrink_x = y / 2;
        let shrink_z = y / 3;

        for lx in (-rx + shrink_x)..=(rx - shrink_x) {
            for lz in (-rz + shrink_z)..=(rz - shrink_z) {
                let px = rot_x(lx as f64, lz as f64, angle, cx);
                let pz = rot_z(lx as f64, lz as f64, angle, cz);

                let is_edge = lx == -rx + shrink_x || lx == rx - shrink_x || lz == -rz + shrink_z || lz == rz - shrink_z;
                if is_edge {
                    let block = if (px + pz + y) % 3 == 0 { QUARTZ_BRICKS } else { CHISELED_STONE_BRICKS };
                    editor.set_block_absolute(block, px, ground_y + y, pz, None, None);
                }
            }
        }
    }
}

fn generate_sede_bb(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (80.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_cross = (x - bounds.cx).abs() < 5 || (z - bounds.cz).abs() < 5;
                if is_cross {
                    let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;
                    if is_edge {
                        editor.set_block_absolute(BLACK_STAINED_GLASS, x, y, z, None, None);
                    } else if y % 4 == 0 {
                        editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_sede_cef(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (75.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;
                if is_edge {
                    if (x + z) % 8 < 2 { editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None); }
                    else { editor.set_block_absolute(TINTED_GLASS, x, y, z, None, None); }
                }
            }
        }
    }
}

fn generate_sede_bc(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (65.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;
                if is_edge {
                    if y > ground_y + 10 { editor.set_block_absolute(POLISHED_DEEPSLATE, x, y, z, None, None); }
                    else if (x + z) % 4 == 0 { editor.set_block_absolute(STONE_BRICKS, x, y, z, None, None); }
                }
            }
        }
    }
}

fn generate_sede_brb(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    generate_ministerio(editor, element, ground_y);
}

fn generate_conjunto_nacional(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (25.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;
                if is_edge {
                    if y > ground_y + 5 {
                        editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
                        if y == ground_y + height - 2 && (x % 5 == 0) { editor.set_block_absolute(RED_CONCRETE, x, y, z, None, None); }
                    } else if (x + z) % 6 == 0 {
                        editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
                    }
                }
            }
        }
    }
}

fn generate_hotel_nacional(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (50.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let is_edge = x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z;
                if is_edge {
                    let is_window = (x + z) % 3 != 0 && y % 4 != 0;
                    let block = if is_window { GLASS_PANE } else { YELLOW_TERRACOTTA };
                    editor.set_block_absolute(block, x, y, z, None, None);
                }
            }
        }
    }
}

fn generate_igrejinha(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    let rx = (((bounds.max_x - bounds.min_x) / 2) as f64).max(8.0) as i32;
    let rz = (((bounds.max_z - bounds.min_z) / 2) as f64).max(12.0) as i32;

    for lx in -rx..=rx {
        for lz in -rz..=rz {
            let px = rot_x(lx as f64, lz as f64, angle, cx);
            let pz = rot_z(lx as f64, lz as f64, angle, cz);

            let roof_y = ground_y + 8 + (lx * lx) as i32 / 5;
            editor.set_block_absolute(WHITE_CONCRETE, px, roof_y, pz, None, None);

            let is_edge = lz == -rz || lz == rz;
            if is_edge {
                for y in ground_y..roof_y {
                    let block = if (px + y) % 2 == 0 { LIGHT_BLUE_TERRACOTTA } else { WHITE_TERRACOTTA };
                    editor.set_block_absolute(block, px, y, pz, None, None);
                }
            }
        }
    }
}

fn generate_estacao_guara(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    let rx = (((bounds.max_x - bounds.min_x) / 2) as f64) as i32;
    let rz = (((bounds.max_z - bounds.min_z) / 2) as f64) as i32;

    for lx in -rx..=rx {
        for lz in -rz..=rz {
            let px = rot_x(lx as f64, lz as f64, angle, cx);
            let pz = rot_z(lx as f64, lz as f64, angle, cz);

            let dx = lx as f64;
            let diff = ((rx * rx) as f64 - dx * dx).max(0.0);
            let h = diff.sqrt() as i32;

            editor.set_block_absolute(SMOOTH_STONE, px, ground_y + h, pz, None, None);
            if lz == -rz || lz == rz {
                for y in ground_y..=(ground_y + h) {
                    editor.set_block_absolute(POLISHED_ANDESITE, px, y, pz, None, None);
                }
            }
        }
    }
}

fn generate_estacao_aguas_claras(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let angle = bounds.angle_rad;

    let rx = (((bounds.max_x - bounds.min_x) / 2) as f64) as i32;
    let rz = (((bounds.max_z - bounds.min_z) / 2) as f64) as i32;

    for lx in -rx..=rx {
        for lz in -rz..=rz {
            let px = rot_x(lx as f64, lz as f64, angle, cx);
            let pz = rot_z(lx as f64, lz as f64, angle, cz);

            for y in (ground_y - 15)..=ground_y {
                let is_inside_station = lx > -rx + 1 && lx < rx - 1 && lz > -rz + 1 && lz < rz - 1;
                if is_inside_station { editor.set_block_absolute(AIR, px, y, pz, None, None); }
                let is_wall = lx == -rx || lx == rx || lz == -rz || lz == rz;
                if is_wall { editor.set_block_absolute(SMOOTH_STONE, px, y, pz, None, None); }
            }
            let dx = lx as f64;
            let roof_h = 5 - (dx * 0.2).abs() as i32;
            editor.set_block_absolute(IRON_BLOCK, px, ground_y + roof_h.max(1), pz, None, None);
        }
    }
}

fn generate_torre_tv(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let max_h = (224.0 * V_SCALE) as i32;

    for y in 0..=max_h {
        let current_y = ground_y + y;
        let radius = (20.0 * H_SCALE) as i32 - (y / 8);
        if radius < 1 { break; }

        for x in (cx - radius)..=(cx + radius) {
            for z in (cz - radius)..=(cz + radius) {
                let dx = (x - cx).abs();
                let dz = (z - cz).abs();
                let is_leg = dx + dz == radius || (dx == 0 && dz == radius) || (dz == 0 && dx == radius);

                if is_leg && y % 3 == 0 { editor.set_block_absolute(IRON_BLOCK, x, current_y, z, None, None); }
                else if is_leg { editor.set_block_absolute(IRON_BARS, x, current_y, z, None, None); }
            }
        }
        let mirante_y = (75.0 * V_SCALE) as i32;
        if y >= mirante_y && y <= mirante_y + 2 {
            let deck_radius = (15.0 * H_SCALE) as i32;
            for x in (cx - deck_radius)..=(cx + deck_radius) {
                for z in (cz - deck_radius)..=(cz + deck_radius) {
                    if y == mirante_y { editor.set_block_absolute(SMOOTH_STONE, x, current_y, z, None, None); }
                    let is_deck_edge = x == cx - deck_radius || x == cx + deck_radius || z == cz - deck_radius || z == cz + deck_radius;
                    if is_deck_edge && y > mirante_y { editor.set_block_absolute(GLASS_PANE, x, current_y, z, None, None); }
                }
            }
        }
    }
}

fn generate_memorial_jk(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;

    let base_radius = (40.0 * H_SCALE) as i32;
    let tower_height = (28.0 * V_SCALE) as i32;

    for x in (cx - base_radius)..=(cx + base_radius) {
        for z in (cz - 10)..=(cz + 10) {
            editor.set_block_absolute(WHITE_CONCRETE, x, ground_y + 1, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x, ground_y + 2, z, None, None);
        }
    }

    for y in (ground_y + 3)..=(ground_y + tower_height) {
        for x in (cx - 2)..=(cx + 2) {
            for z in (cz - 2)..=(cz + 2) {
                editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
            }
        }
    }

    let foice_y = ground_y + tower_height + 1;
    for x in (cx - 6)..=(cx + 6) {
        for z in (cz - 2)..=(cz + 2) {
            editor.set_block_absolute(SMOOTH_QUARTZ, x, foice_y, z, None, None);
            editor.set_block_absolute(SMOOTH_QUARTZ, x, foice_y + 1, z, None, None);
        }
    }
}

fn generate_rodoviaria(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (15.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                if y == ground_y + 1 || y == ground_y + height { editor.set_block_absolute(SMOOTH_STONE, x, y, z, None, None); }
                else if (x + z) % 15 == 0 { editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None); }
                else if y == ground_y + 2 && x % 4 == 0 { editor.set_block_absolute(YELLOW_CONCRETE, x, y, z, None, None); }
            }
        }
    }
}

fn generate_estadio_nacional(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let cx = bounds.cx;
    let cz = bounds.cz;
    let raw_radius = (bounds.max_x - cx).max(bounds.max_z - cz);
    let radius = raw_radius;
    let height = (50.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in (cx - radius)..=(cx + radius) {
            for z in (cz - radius)..=(cz + radius) {
                let dist = (((x - cx).pow(2) + (z - cz).pow(2)) as f64).sqrt() as i32;
                if dist == radius && (x + z) % 4 == 0 { editor.set_block_absolute(SMOOTH_QUARTZ, x, y, z, None, None); }
                else if dist < radius - 5 && dist > radius - 25 {
                    let seat_height = (radius - 5 - dist) / 2;
                    if y == ground_y + seat_height { editor.set_block_absolute(RED_CONCRETE, x, y, z, None, None); }
                }
                else if dist <= radius - 25 && y == ground_y + 1 { editor.set_block_absolute(GRASS_BLOCK, x, y, z, None, None); }
                else if y == ground_y + height && dist <= radius && dist >= radius - 20 { editor.set_block_absolute(WHITE_STAINED_GLASS, x, y, z, None, None); }
            }
        }
    }
}

fn generate_pavilhao_parque_cidade(editor: &mut WorldEditor, element: &ProcessedWay, ground_y: i32) {
    let bounds = get_oriented_bounds(element);
    let height = (15.0 * V_SCALE) as i32;

    for y in ground_y..=(ground_y + height) {
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let dx = (x - bounds.cx) as f64;
                let wave_y = ground_y + 10 + ((dx * 0.2).sin() * 3.0 * V_SCALE) as i32;
                if y == wave_y { editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None); }
                else if y < wave_y && (x == bounds.min_x || x == bounds.max_x || z == bounds.min_z || z == bounds.max_z) {
                    if (x + z) % 6 == 0 { editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None); }
                    else { editor.set_block_absolute(GLASS_PANE, x, y, z, None, None); }
                }
            }
        }
    }
}