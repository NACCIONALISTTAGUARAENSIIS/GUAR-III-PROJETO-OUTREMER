use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::element_processing::tree::Tree;
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Escala vertical rigorosa (1.15) e horizontal (1.33) aplicada ao lazer e relevo
const V_SCALE: f64 = 1.15;
const H_SCALE: f64 = 1.33;

/// ?? BESM-6: Motor de Ru�do Org�nico (Pseudo-Perlin Noise O(1))
/// Usado para criar maci�os florestais e canteiros fluidos de Burle Marx,
/// substituindo a distribui��o aleat�ria e irrealista.
#[inline(always)]
fn organic_noise(x: i32, z: i32, scale: f64) -> f64 {
    let xf = x as f64 * scale;
    let zf = z as f64 * scale;
    // Padr�o de interfer�ncia de ondas para criar "ilhas" e "clareiras"
    ((xf.sin() * zf.cos()) + (xf * 0.5 + zf * 0.3).sin() * 0.5).abs() / 1.5
}

pub fn generate_leisure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let name = element
            .tags
            .get("name")
            .map(|s: &String| s.to_lowercase())
            .unwrap_or_default();
        let _source = element
            .tags
            .get("source")
            .map(|s: &String| s.as_str())
            .unwrap_or("");

        // ?? BESM-6: Detec��o de Patrim�nio Cultural e Lazer do Distrito Federal
        let is_parque_da_cidade =
            name.contains("parque da cidade") || name.contains("sarah kubitschek");
        let is_ana_lidia = name.contains("ana l�dia") || name.contains("ana lidia");
        // Burle Marx projetou os jardins do Itamaraty, Jaburu, TCU, Superquadras antigas e Pra�a dos Cristais
        let is_burle_marx = name.contains("burle marx")
            || name.contains("cristais")
            || name.contains("itamaraty")
            || name.contains("justi�a")
            || name.contains("jaburu")
            || name.contains("tribunal de contas");
        let is_cristais = name.contains("cristais");
        let is_unb = name.contains("unb") || name.contains("universidade de bras�lia");
        let is_guara = name.contains("guar�") || name.contains("guara");

        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);

        // Defini��o de materiais r�gida (Bras�lia Architectural Specs)
        let block_type: Block = match leisure_type.as_str() {
            "park" | "nature_reserve" | "garden" | "disc_golf_course" | "golf_course" => {
                GRASS_BLOCK
            }
            "schoolyard" => LIGHT_GRAY_CONCRETE,
            "track" => {
                let surface = element
                    .tags
                    .get("surface")
                    .map(|s: &String| s.as_str())
                    .unwrap_or("");
                // ?? TWEAK DF: O GDF pinta as ciclovias (Guar�, Eix�o, W3) de vermelho.
                // As pistas de cooper do Parque da Cidade tamb�m s�o avermelhadas (emborrachadas).
                if surface == "asphalt"
                    || surface == "paved"
                    || surface == "tartan"
                    || surface == "rubber"
                {
                    RED_CONCRETE // Ciclovias GDF e Pistas de Cooper
                } else {
                    RED_TERRACOTTA
                }
            }
            "fitness_station" => SMOOTH_STONE, // Base de concreto dos PECs do GDF
            "playground" | "recreation_ground" | "pitch" | "beach_resort" | "dog_park" => {
                if let Some(surface) = element.tags.get("surface") {
                    match surface.as_str() {
                        "clay" => COARSE_DIRT,
                        "sand" => SAND,
                        "tartan" | "rubber" => RED_CONCRETE,
                        "grass" => GRASS_BLOCK,
                        "dirt" => DIRT,
                        "asphalt" => GRAY_CONCRETE,
                        "concrete" => LIGHT_GRAY_CONCRETE,
                        _ => LIGHT_BLUE_CONCRETE,
                    }
                } else {
                    if leisure_type == "pitch" {
                        GREEN_TERRACOTTA
                    } else if leisure_type == "playground" {
                        SAND
                    } else {
                        LIGHT_BLUE_CONCRETE
                    }
                }
            }
            "swimming_pool" | "swimming_area" => WATER,
            "bathing_place" => SMOOTH_SANDSTONE,
            "outdoor_seating" => POLISHED_ANDESITE,
            "water_park" | "slipway" => LIGHT_BLUE_TERRACOTTA,
            "ice_rink" => PACKED_ICE,
            _ => GRASS_BLOCK,
        };

        // Renderiza��o de Bordas e Conten��o
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, 0, prev.1, node.x, 0, node.z);

                for (bx, _, bz) in bresenham_points {
                    let ground_y = if args.terrain {
                        editor.get_ground_level(bx, bz)
                    } else {
                        0
                    };

                    let edge_block = if leisure_type == "track" {
                        WHITE_CONCRETE // Faixa lateral de seguran�a das ciclovias
                    } else if block_type == WATER {
                        STONE_BRICKS // Borda de piscina/espelho d'�gua
                    } else {
                        block_type
                    };

                    // Prote��o de Malha: N�o sobrep�e asfalto monumental ou cal�adas de pedra
                    if !editor.check_for_block_absolute(
                        bx,
                        ground_y,
                        bz,
                        Some(&[
                            BLACK_CONCRETE,
                            POLISHED_BASALT,
                            YELLOW_CONCRETE,
                            POLISHED_ANDESITE,
                        ]),
                        None,
                    ) {
                        editor.set_block_absolute(
                            edge_block,
                            bx,
                            ground_y,
                            bz,
                            Some(&[GRASS_BLOCK, DIRT, SAND, STONE, GRAVEL]),
                            None,
                        );
                    }
                }

                corner_addup.0 += node.x;
                corner_addup.1 += node.z;
                corner_addup.2 += 1;
            }
            previous_node = Some((node.x, node.z));
        }

        // Preenchimento de �rea (Flood-fill)
        if corner_addup != (0, 0, 0) {
            let filled_area: Vec<(i32, i32)> =
                flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

            let _rng = element_rng(element.id);

            // Centro de massa da �rea de lazer (�til para ancorar monumentos �nicos)
            let (cx, cz) = if !filled_area.is_empty() {
                let (sum_x, sum_z) = filled_area.iter().fold((0i64, 0i64), |acc, &(x, z)| {
                    (acc.0 + x as i64, acc.1 + z as i64)
                });
                let len = filled_area.len() as i64;
                ((sum_x / len) as i32, (sum_z / len) as i32)
            } else {
                (0, 0)
            };
            let cy = if args.terrain {
                editor.get_ground_level(cx, cz)
            } else {
                0
            };

            // ?? MONUMENTO: O FOGUETINHO E CASTELINHO (Parque Ana L�dia - 1969)
            if is_ana_lidia && !filled_area.is_empty() {
                // Foguetinho (Ancorado no centro do pol�gono)
                let rocket_h = (10.0 * V_SCALE).round() as i32;
                let rocket_w = (2.0 * H_SCALE).round() as i32;

                // Base Vermelha (Os 4 p�s estabilizadores do Astro City Slide)
                for lx in (cx - rocket_w)..=(cx + rocket_w) {
                    for lz in (cz - rocket_w)..=(cz + rocket_w) {
                        if (lx - cx).abs() == rocket_w && (lz - cz).abs() == rocket_w {
                            for dy in 1i32..=3i32 {
                                editor.set_block_absolute(
                                    RED_CONCRETE,
                                    lx,
                                    cy + dy,
                                    lz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
                // Fuselagem e Ponta do Foguete
                for dy in 4..rocket_h {
                    let is_nose_cone = dy > rocket_h - 3;
                    let b = if is_nose_cone {
                        RED_CONCRETE
                    } else if dy % 3 == 0 {
                        YELLOW_CONCRETE
                    } else {
                        WHITE_CONCRETE
                    };

                    editor.set_block_absolute(b, cx, cy + dy, cz, None, None);

                    if !is_nose_cone {
                        editor.set_block_absolute(b, cx + 1, cy + dy, cz, None, None);
                        editor.set_block_absolute(b, cx - 1, cy + dy, cz, None, None);
                        editor.set_block_absolute(b, cx, cy + dy, cz + 1, None, None);
                        editor.set_block_absolute(b, cx, cy + dy, cz - 1, None, None);
                    }
                }
                editor.set_block_absolute(LIGHTNING_ROD, cx, cy + rocket_h, cz, None, None);

                // Castelinho (Deslocado 15 blocos a oeste)
                let cast_x = cx - (15.0 * H_SCALE) as i32;
                let cast_z = cz;
                let cast_y = if args.terrain {
                    editor.get_ground_level(cast_x, cast_z)
                } else {
                    0
                };
                let cast_radius = (4.0 * H_SCALE).round() as i32;

                for lx in (cast_x - cast_radius)..=(cast_x + cast_radius) {
                    for lz in (cast_z - cast_radius)..=(cast_z + cast_radius) {
                        let is_wall = (lx - cast_x).abs() == cast_radius
                            || (lz - cast_z).abs() == cast_radius;
                        if is_wall {
                            for dy in 1i32..=5i32 {
                                editor.set_block_absolute(BRICK, lx, cast_y + dy, lz, None, None);
                            }
                            // Ameias do castelo
                            if (lx + lz) % 2 == 0 {
                                editor.set_block_absolute(BRICK, lx, cast_y + 6, lz, None, None);
                            }
                        }
                    }
                }
            }

            for &(x, z) in &filled_area {
                let ground_y = if args.terrain {
                    editor.get_ground_level(x, z)
                } else {
                    0
                };

                // Bloqueio de colis�o com asfalto monumental, rodovias e passeios reais
                if editor.check_for_block_absolute(
                    x,
                    ground_y,
                    z,
                    Some(&[
                        BLACK_CONCRETE,
                        POLISHED_BASALT,
                        YELLOW_CONCRETE,
                        WHITE_CONCRETE,
                        POLISHED_ANDESITE,
                        SMOOTH_STONE_SLAB,
                    ]),
                    None,
                ) {
                    continue;
                }

                editor.set_block_absolute(
                    block_type,
                    x,
                    ground_y,
                    z,
                    Some(&[GRASS_BLOCK, DIRT, SAND, STONE, GRAVEL, PODZOL]),
                    None,
                );

                // L�gica de profundidade para Piscinas e Lagos (Impede a �gua de quebrar se o ch�o afundar)
                if block_type == WATER {
                    editor.set_block_absolute(DIRT, x, ground_y - 1, z, None, None);
                    editor.set_block_absolute(WATER, x, ground_y, z, None, None);
                    continue;
                }

                // ?? CICLOVIAS E PISTAS (Escala 1.33 e Rigor Governamental)
                if leisure_type == "track" {
                    let stripe_mod = (4.0 * H_SCALE).round() as i32;
                    // Faixa amarela cont�nua no meio (m�o dupla) para ciclovias do Guar� e Plano
                    let is_center_line = if is_guara {
                        (x - cx).abs() % 4 == 0
                    } else {
                        false
                    };

                    if is_center_line {
                        editor.set_block_absolute(YELLOW_CONCRETE, x, ground_y, z, None, None);
                    } else if (x % stripe_mod == 0) && (z % 5 != 0) {
                        // Tracejado branco gen�rico
                        editor.set_block_absolute(WHITE_CONCRETE, x, ground_y, z, None, None);
                    }
                }

                // ??? PEC (Ponto de Encontro Comunit�rio) - Padr�o Literal da NOVACAP
                if leisure_type == "fitness_station" && !building_footprints.contains(x, z) {
                    let local_x = x.abs() % 14;
                    let local_z = z.abs() % 14;

                    // Aparelhos Met�licos de Gin�stica GDF (Verde e Amarelo representados por Ferro/Pedra)
                    if local_x == 2 && local_z == 2 {
                        // Simulador de Caminhada
                        editor.set_block_absolute(IRON_BARS, x, ground_y + 1, z, None, None);
                        editor.set_block_absolute(IRON_BARS, x, ground_y + 2, z, None, None);
                        editor.set_block_absolute(
                            LIGHT_WEIGHTED_PRESSURE_PLATE,
                            x,
                            ground_y + 3,
                            z,
                            None,
                            None,
                        );
                    } else if local_x == 8 && local_z == 2 {
                        // Rota��o Dupla Vertical (Volante)
                        editor.set_block_absolute(IRON_BARS, x, ground_y + 1, z, None, None);
                        editor.set_block_absolute(GRINDSTONE, x, ground_y + 2, z, None, None);
                    } else if local_x == 2 && local_z == 8 {
                        // Press�o de Pernas
                        editor.set_block_absolute(STONE_STAIRS, x, ground_y + 1, z, None, None);
                        editor.set_block_absolute(IRON_BARS, x + 1, ground_y + 1, z, None, None);
                    } else if local_x > 10 && local_z > 10 {
                        // Pergolado de Sombreamento e Bancos (Cl�ssico dos PECs de Bras�lia)
                        if (local_x == 11 || local_x == 13) && (local_z == 11 || local_z == 13) {
                            editor.set_block_absolute(OAK_FENCE, x, ground_y + 1, z, None, None);
                            editor.set_block_absolute(OAK_FENCE, x, ground_y + 2, z, None, None);
                            editor.set_block_absolute(OAK_FENCE, x, ground_y + 3, z, None, None);
                        }
                        if local_x >= 11 && local_x <= 13 && local_z >= 11 && local_z <= 13 {
                            editor.set_block_absolute(OAK_SLAB, x, ground_y + 4, z, None, None);
                        }
                        if local_x == 12 && local_z == 12 {
                            editor.set_block_absolute(OAK_STAIRS, x, ground_y + 1, z, None, None);
                            // Banco embaixo da sombra
                        }
                    }
                }

                // ?? PAISAGISMO ORG�NICO (Burle Marx, UnB, Parque da Cidade)
                if matches!(leisure_type.as_str(), "park" | "garden" | "nature_reserve") {
                    let bm_noise = organic_noise(x, z, 0.05); // Densidade macro (Canteiros/Bosques)
                    let micro_noise = organic_noise(x, z, 0.2); // Densidade fina (Flores/�rvores isoladas)

                    let mut tile_rng = coord_rng(x, z, element.id);
                    let random_roll = tile_rng.random_range(0..1000);

                    if is_cristais {
                        // Pra�a dos Cristais: Cact�ceas, areia e lagos angulares
                        if bm_noise > 0.6 && !building_footprints.contains(x, z) {
                            editor.set_block_absolute(SAND, x, ground_y, z, None, None);
                            if micro_noise > 0.8 {
                                editor.set_block_absolute(CACTUS, x, ground_y + 1, z, None, None);
                            }
                        } else if bm_noise < 0.15 && !building_footprints.contains(x, z) {
                            editor.set_block_absolute(DIRT, x, ground_y - 1, z, None, None);
                            editor.set_block_absolute(WATER, x, ground_y, z, None, None);
                            if random_roll < 50 {
                                editor.set_block_absolute(LILY_PAD, x, ground_y + 1, z, None, None);
                                // Vit�rias-r�gias
                            }
                        }
                    } else if is_burle_marx && bm_noise > 0.5 {
                        // Maci�os de Burle Marx (Ilhas curvas de cor intensa)
                        if micro_noise > 0.4 && !building_footprints.contains(x, z) {
                            let flower = if (x ^ z) % 4 == 0 {
                                PINK_TULIP
                            } else if (x ^ z) % 4 == 1 {
                                ALLIUM
                            } else if (x ^ z) % 4 == 2 {
                                RED_TULIP
                            } else {
                                ORANGE_TULIP
                            };
                            editor.set_block_absolute(
                                flower,
                                x,
                                ground_y + 1,
                                z,
                                Some(&[AIR]),
                                None,
                            );
                        }
                    } else if is_unb {
                        // Campus da UnB: Gramados abertos (Minhoc�o), terra vermelha, Ip�s esparsos
                        if bm_noise > 0.7
                            && micro_noise > 0.8
                            && !building_footprints.contains(x, z)
                        {
                            Tree::create(editor, (x, ground_y + 1, z), Some(building_footprints));
                        } else if bm_noise < 0.2 {
                            editor.set_block_absolute(
                                COARSE_DIRT,
                                x,
                                ground_y,
                                z,
                                Some(&[GRASS_BLOCK]),
                                None,
                            ); // Caminhos de terra
                        }
                    } else if is_parque_da_cidade {
                        // Parque da Cidade: Bosques de pinheiros e grandes gramados
                        if bm_noise > 0.6
                            && micro_noise > 0.85
                            && !building_footprints.contains(x, z)
                        {
                            Tree::create(editor, (x, ground_y + 1, z), Some(building_footprints));
                        }
                    } else {
                        // Parques Gen�ricos e �reas Verdes de Superquadra
                        // Substitui o "random_roll < 3" antigo por uma l�gica de bosque (Noise)
                        if bm_noise > 0.75 {
                            // Zona densa (Bosque)
                            if micro_noise > 0.6 && !building_footprints.contains(x, z) {
                                Tree::create(
                                    editor,
                                    (x, ground_y + 1, z),
                                    Some(building_footprints),
                                );
                            } else {
                                editor.set_block_absolute(
                                    PODZOL,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[GRASS_BLOCK]),
                                    None,
                                );
                                editor.set_block_absolute(
                                    TALL_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                    Some(&[AIR]),
                                    None,
                                );
                            }
                        } else if bm_noise < 0.3 && micro_noise > 0.9 {
                            // �rvores solit�rias no gramado
                            if !building_footprints.contains(x, z) {
                                Tree::create(
                                    editor,
                                    (x, ground_y + 1, z),
                                    Some(building_footprints),
                                );
                            }
                        }
                    }
                }

                // ?? PARQUINHOS DE SUPERQUADRA E SAT�LITES
                if matches!(leisure_type.as_str(), "playground" | "recreation_ground")
                    && !is_ana_lidia
                {
                    let mut tile_rng = coord_rng(x, z, element.id);
                    let play_roll = tile_rng.random_range(0..5000);

                    match play_roll {
                        0..5 => {
                            // Gangorra cl�ssica de madeira
                            for dx in -1i32..=1i32 {
                                editor.set_block_absolute(
                                    DARK_OAK_SLAB,
                                    x + dx,
                                    ground_y + 1,
                                    z,
                                    None,
                                    None,
                                );
                            }
                            editor.set_block_absolute(OAK_FENCE, x, ground_y, z, None, None);
                        }
                        6..10 => {
                            // Trepa-trepa (Gaiola de Ferro cl�ssica)
                            for dy in 1i32..=3i32 {
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x,
                                    ground_y + dy,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x + 1,
                                    ground_y + dy,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x,
                                    ground_y + dy,
                                    z + 1,
                                    None,
                                    None,
                                );
                            }
                        }
                        11..12 => {
                            // Banco de concreto cl�ssico do DF ao redor do parquinho
                            editor.set_block_absolute(
                                SMOOTH_STONE_SLAB,
                                x,
                                ground_y + 1,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                SMOOTH_STONE_SLAB,
                                x + 1,
                                ground_y + 1,
                                z,
                                None,
                                None,
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub fn generate_leisure_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if let Some(leisure) = rel.tags.get("leisure") {
        if matches!(
            leisure.as_str(),
            "park" | "nature_reserve" | "garden" | "recreation_ground" | "pitch" | "track"
        ) {
            for member in &rel.members {
                if member.role == ProcessedMemberRole::Outer {
                    let way_with_rel_tags = ProcessedWay {
                        id: member.way.id,
                        nodes: member.way.nodes.clone(),
                        tags: rel.tags.clone(), // Repassa as tags da rela��o (nome do parque, etc)
                    };
                    generate_leisure(
                        editor,
                        &way_with_rel_tags,
                        args,
                        flood_fill_cache,
                        building_footprints,
                    );
                }
            }
        }
    }
}
