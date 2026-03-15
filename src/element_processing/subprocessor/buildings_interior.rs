use crate::block_definitions::*;
use crate::world_editor::WorldEditor;

//
// =====================================================
// ESCALA E DIMENSÕES GLOBAIS
// =====================================================
//

const H_SCALE: f64 = 1.33;
const V_SCALE: f64 = 1.15;
const PE_DIREITO_METROS: f64 = 2.60;

fn pe_direito_blocos() -> i32 {
    (PE_DIREITO_METROS * V_SCALE).round() as i32
}

//
// =====================================================
// ENUMS DE CONTEXTO (BAIRRO, TIPOLOGIA E EDIFÍCIO ÚNICO)
// =====================================================
//

#[derive(PartialEq, Clone, Copy)]
enum Bairro {
    SQS,
    SQN,
    AguasClaras,
    Guara,
    Samambaia,
    Comercial,
    Condominio,
    Outro,
}

#[derive(PartialEq, Clone, Copy)]
enum Tipologia {
    Residencial,
    Escola,
    Hospital,
    Corporativo,
    Comercial,
    Religioso,
    MetroSubterraneo,
    MetroElevado,
    Generico,
}

#[derive(PartialEq, Clone, Copy)]
enum EdificioBrasilia {
    CongressoNacional,
    PalacioPlanalto,
    STF,
    Itamaraty,
    HospitalBase,
    SedeBancaria,
    Shopping,
    CatedralMetropolitana,
    Nenhum,
}

fn detect_bairro(element: &crate::osm_parser::ProcessedWay) -> Bairro {
    let suburb = element
        .tags
        .get("addr:suburb")
        .unwrap_or(&"".to_string())
        .to_lowercase();

    if suburb.contains("sqs") {
        Bairro::SQS
    } else if suburb.contains("sqn") {
        Bairro::SQN
    } else if suburb.contains("aguas claras") {
        Bairro::AguasClaras
    } else if suburb.contains("guará") || suburb.contains("guara") {
        Bairro::Guara
    } else if suburb.contains("samambaia") {
        Bairro::Samambaia
    } else if suburb.contains("comercial") || suburb.contains("setor de autarquias") {
        Bairro::Comercial
    } else if suburb.contains("condom") {
        Bairro::Condominio
    } else {
        Bairro::Outro
    }
}

fn detect_tipologia(element: &crate::osm_parser::ProcessedWay) -> Tipologia {
    let amenity = element
        .tags
        .get("amenity")
        .map(|s| s.as_str())
        .unwrap_or("");
    let building = element
        .tags
        .get("building")
        .map(|s| s.as_str())
        .unwrap_or("");
    let railway = element
        .tags
        .get("railway")
        .map(|s| s.as_str())
        .unwrap_or("");
    let station = element
        .tags
        .get("station")
        .map(|s| s.as_str())
        .unwrap_or("");
    let location = element
        .tags
        .get("location")
        .map(|s| s.as_str())
        .unwrap_or("");
    let layer = element
        .tags
        .get("layer")
        .map_or(0, |l| l.parse::<i32>().unwrap_or(0));

    let is_metro = station == "subway"
        || element.tags.get("subway").map(|s| s.as_str()) == Some("yes")
        || (railway == "station" && (location == "underground" || layer < 0));

    if is_metro {
        if location == "underground" || layer < 0 {
            return Tipologia::MetroSubterraneo;
        } else {
            return Tipologia::MetroElevado;
        }
    }

    match amenity {
        "school" | "university" | "college" | "kindergarten" => return Tipologia::Escola,
        "hospital" | "clinic" | "doctors" => return Tipologia::Hospital,
        "bank" | "police" | "courthouse" | "townhall" => return Tipologia::Corporativo,
        "place_of_worship" => return Tipologia::Religioso,
        _ => {}
    }

    match building {
        "school" | "university" => Tipologia::Escola,
        "hospital" | "clinic" => Tipologia::Hospital,
        "office" | "government" => Tipologia::Corporativo,
        "retail" | "commercial" | "supermarket" => Tipologia::Comercial,
        "residential" | "apartments" | "house" | "detached" | "terrace" => Tipologia::Residencial,
        "church" | "cathedral" | "chapel" | "temple" | "mosque" => Tipologia::Religioso,
        _ => Tipologia::Generico,
    }
}

fn detect_edificio_especifico(element: &crate::osm_parser::ProcessedWay) -> EdificioBrasilia {
    let name = element
        .tags
        .get("name")
        .or_else(|| element.tags.get("building:name"))
        .or_else(|| element.tags.get("operator"))
        .unwrap_or(&"".to_string())
        .to_lowercase();

    if name.contains("congresso nacional")
        || name.contains("câmara dos deputados")
        || name.contains("senado federal")
    {
        EdificioBrasilia::CongressoNacional
    } else if name.contains("palácio do planalto") || name.contains("palacio do planalto") {
        EdificioBrasilia::PalacioPlanalto
    } else if name.contains("supremo tribunal federal") || name.contains("stf") {
        EdificioBrasilia::STF
    } else if name.contains("itamaraty") || name.contains("relações exteriores") {
        EdificioBrasilia::Itamaraty
    } else if name.contains("hospital de base")
        || name.contains("santa lúcia")
        || name.contains("santa lucia")
    {
        EdificioBrasilia::HospitalBase
    } else if name.contains("banco do brasil")
        || name.contains("caixa econômica")
        || name.contains("banco central")
        || name.contains("brb")
    {
        EdificioBrasilia::SedeBancaria
    } else if name.contains("shopping") || name.contains("conjunto nacional") {
        EdificioBrasilia::Shopping
    } else if name.contains("catedral metropolitana") {
        EdificioBrasilia::CatedralMetropolitana
    } else {
        EdificioBrasilia::Nenhum
    }
}

//
// =====================================================
// ESTRUTURAS INTERNAS COMPARTILHADAS E LAJES
// =====================================================
//

fn generate_laje(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    offset: i32,
    has_core: bool,
    is_atrium: bool,
) {
    if max_x - min_x < 3 || max_z - min_z < 3 {
        return;
    }

    let grid_span = (5.0 * H_SCALE).round() as i32;
    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    let largura = max_x - min_x;
    let prof = max_z - min_z;

    let void_min_x = min_x + (largura as f64 * 0.32) as i32;
    let void_max_x = max_x - (largura as f64 * 0.32) as i32;
    let void_min_z = min_z + (prof as f64 * 0.32) as i32;
    let void_max_z = max_z - (prof as f64 * 0.32) as i32;

    let valid_void = is_atrium && (void_max_x - void_min_x > 2) && (void_max_z - void_min_z > 2);

    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            if has_core && x >= cx - 1 && x <= cx + 2 && z >= cz - 1 && z <= cz + 2 {
                continue;
            }

            if valid_void
                && x >= void_min_x
                && x <= void_max_x
                && z >= void_min_z
                && z <= void_max_z
            {
                continue;
            }

            // Geometria relativa ao prédio (x - min_x)
            let is_beam = (x - min_x) % grid_span == 0 || (z - min_z) % grid_span == 0;
            let bloco = if is_beam {
                SMOOTH_STONE
            } else {
                POLISHED_ANDESITE
            };
            editor.set_block_absolute(bloco, x, y + offset, z, None, None);
        }
    }
}

fn generate_elevator_core(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    floor_y: i32,
    ceiling: i32,
    offset: i32,
    tipologia: Tipologia,
) {
    let largura = max_x - min_x;
    let profundidade = max_z - min_z;

    let largura_metros = largura as f64 / H_SCALE;
    let prof_metros = profundidade as f64 / H_SCALE;

    if largura_metros < 6.5 || prof_metros < 6.5 {
        return;
    }

    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    let core_x_max = if tipologia == Tipologia::Hospital || tipologia == Tipologia::Corporativo {
        cx + 3
    } else {
        cx + 2
    };

    for x in cx - 1..=core_x_max {
        for z in cz - 1..=cz + 2 {
            for y in floor_y..ceiling {
                let is_air = ((x == cx || x == cx + 1 || (x == cx + 2 && core_x_max > cx + 2))
                    && (z == cz || z == cz + 1))
                    || (z == cz + 2 && (x == cx || x == cx + 1));

                if is_air {
                    editor.set_block_absolute(AIR, x, y + offset, z, None, None);
                } else {
                    editor.set_block_absolute(STONE_BRICKS, x, y + offset, z, None, None);
                }
            }
        }
    }

    editor.set_block_absolute(IRON_DOOR, cx, floor_y + 1 + offset, cz - 1, None, None);
    editor.set_block_absolute(IRON_DOOR, cx + 1, floor_y + 1 + offset, cz - 1, None, None);
    if core_x_max > cx + 2 {
        editor.set_block_absolute(IRON_DOOR, cx + 2, floor_y + 1 + offset, cz - 1, None, None);
    }

    for y in floor_y..ceiling {
        editor.set_block_absolute(STONE_BRICKS, cx, y + offset, cz + 3, None, None);
        editor.set_block_absolute(STONE_BRICKS, cx + 1, y + offset, cz + 3, None, None);
    }
}

fn generate_corridor(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    floor_y: i32,
    ceiling: i32,
    offset: i32,
    bairro: Bairro,
    tipologia: Tipologia,
) {
    let largura = max_x - min_x;
    if max_z - min_z < 5 {
        return;
    }

    let center = (min_z + max_z) / 2;

    let (cz_start, cz_end) = match tipologia {
        Tipologia::Hospital | Tipologia::Escola => (center - 2, center + 2),
        Tipologia::Corporativo => (center - 1, center + 2),
        _ => match bairro {
            Bairro::Comercial => (center - 1, center + 1),
            _ => (center, center + 1),
        },
    };

    for x in (min_x + 1)..(max_x - 1) {
        for y in floor_y..ceiling {
            for z in cz_start..=cz_end {
                editor.set_block_absolute(AIR, x, y + offset, z, None, None);
            }

            if (largura as f64 / H_SCALE) >= 6.0 {
                let wall_block = if tipologia == Tipologia::Hospital {
                    SMOOTH_QUARTZ
                } else {
                    WHITE_CONCRETE
                };
                editor.set_block_absolute(wall_block, x, y + offset, cz_end + 1, None, None);
                editor.set_block_absolute(wall_block, x, y + offset, cz_start - 1, None, None);
            }
        }
    }
}

//
// =====================================================
// MOTORES INFRAESTRUTURAIS E ARQUITETÔNICOS
// =====================================================
//

fn generate_metro_underground_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    ground_y: i32,
    offset: i32,
) {
    let plat_y = ground_y - 14;
    let mez_y = ground_y - 7;

    let is_x_axis = (max_x - min_x) > (max_z - min_z);
    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    // TWEAK O(n³): Em vez de pintar o volume inteiro de ar, limpa apenas a cavidade interna
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            editor.set_block_absolute(SMOOTH_STONE, x, plat_y - 1 + offset, z, None, None);
            editor.set_block_absolute(SMOOTH_STONE, x, ground_y + offset, z, None, None);

            let is_wall = x == min_x || x == max_x || z == min_z || z == max_z;

            // Só executa o laço de altura para as paredes
            if is_wall {
                for h in plat_y..ground_y {
                    editor.set_block_absolute(SMOOTH_STONE, x, h + offset, z, None, None);
                }
            }
        }
    }

    // Limpa a cavidade interna para formar o túnel (Air)
    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            for h in plat_y..ground_y {
                editor.set_block_absolute(AIR, x, h + offset, z, None, None);
            }
        }
    }

    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            let is_mezzanine_hole = if is_x_axis {
                z >= cz - 3 && z <= cz + 3 && x >= cx - 8 && x <= cx + 8
            } else {
                x >= cx - 3 && x <= cx + 3 && z >= cz - 8 && z <= cz + 8
            };

            if !is_mezzanine_hole {
                editor.set_block_absolute(POLISHED_ANDESITE, x, mez_y + offset, z, None, None);
            } else {
                if mez_y + 1 <= ground_y {
                    let is_hole_edge = if is_x_axis {
                        (z == cz - 4 || z == cz + 4) && x >= cx - 8 && x <= cx + 8
                    } else {
                        (x == cx - 4 || x == cx + 4) && z >= cz - 8 && z <= cz + 8
                    };
                    if is_hole_edge {
                        editor.set_block_absolute(GLASS_PANE, x, mez_y + 1 + offset, z, None, None);
                    }
                }
            }

            let is_stair_zone = if is_x_axis {
                (x == cx - 8 || x == cx + 8) && (z == cz - 4 || z == cz + 4)
            } else {
                (z == cz - 8 || z == cz + 8) && (x == cx - 4 || x == cx + 4)
            };

            if is_stair_zone {
                for step in 0..=(mez_y - plat_y) {
                    let step_x = if is_x_axis {
                        if x < cx {
                            x + step
                        } else {
                            x - step
                        }
                    } else {
                        x
                    };
                    let step_z = if !is_x_axis {
                        if z < cz {
                            z + step
                        } else {
                            z - step
                        }
                    } else {
                        z
                    };

                    if step_x > min_x && step_x < max_x && step_z > min_z && step_z < max_z {
                        editor.set_block_absolute(
                            STONE_STAIRS,
                            step_x,
                            mez_y - step + offset,
                            step_z,
                            None,
                            None,
                        );
                        editor.set_block_absolute(
                            AIR,
                            step_x,
                            mez_y - step + 1 + offset,
                            step_z,
                            None,
                            None,
                        );
                        editor.set_block_absolute(
                            AIR,
                            step_x,
                            mez_y - step + 2 + offset,
                            step_z,
                            None,
                            None,
                        );
                    }
                }
            }

            let is_track_pit = if is_x_axis {
                z >= cz - 3 && z <= cz + 3
            } else {
                x >= cx - 3 && x <= cx + 3
            };

            if !is_track_pit {
                editor.set_block_absolute(POLISHED_DIORITE, x, plat_y + offset, z, None, None);

                let is_tactile_edge = if is_x_axis {
                    z == cz - 4 || z == cz + 4
                } else {
                    x == cx - 4 || x == cx + 4
                };
                if is_tactile_edge {
                    editor.set_block_absolute(YELLOW_CONCRETE, x, plat_y + offset, z, None, None);
                }
            } else {
                editor.set_block_absolute(GRAVEL, x, plat_y - 1 + offset, z, None, None);
            }

            let is_pillar = if is_x_axis {
                (x - min_x) % 8 == 0 && (z == cz - 4 || z == cz + 4)
            } else {
                (z - min_z) % 8 == 0 && (x == cx - 4 || x == cx + 4)
            };
            if is_pillar {
                for h in plat_y..=ground_y {
                    editor.set_block_absolute(SMOOTH_STONE, x, h + offset, z, None, None);
                }
            }
        }
    }

    if is_x_axis {
        for z in (cz - 3)..=(cz + 3) {
            for h in plat_y..=(plat_y + 5) {
                editor.set_block_absolute(AIR, min_x, h + offset, z, None, None);
                editor.set_block_absolute(AIR, max_x, h + offset, z, None, None);
            }
        }
    } else {
        for x in (cx - 3)..=(cx + 3) {
            for h in plat_y..=(plat_y + 5) {
                editor.set_block_absolute(AIR, x, h + offset, min_z, None, None);
                editor.set_block_absolute(AIR, x, h + offset, max_z, None, None);
            }
        }
    }
}

fn generate_metro_elevated_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    ground_y: i32,
    offset: i32,
) {
    let plat_y = ground_y + 8;
    let is_x_axis = (max_x - min_x) > (max_z - min_z);
    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let is_pillar = (x - min_x) % 10 == 0 && (z - min_z) % 10 == 0;
            if is_pillar {
                for h in ground_y..plat_y {
                    editor.set_block_absolute(SMOOTH_STONE, x, h + offset, z, None, None);
                }
            }

            if x > min_x && x < max_x && z > min_z && z < max_z {
                editor.set_block_absolute(POLISHED_ANDESITE, x, plat_y - 1 + offset, z, None, None);

                let is_track_pit = if is_x_axis {
                    z >= cz - 3 && z <= cz + 3
                } else {
                    x >= cx - 3 && x <= cx + 3
                };
                if is_track_pit {
                    editor.set_block_absolute(GRAVEL, x, plat_y - 1 + offset, z, None, None);
                } else {
                    editor.set_block_absolute(POLISHED_DIORITE, x, plat_y + offset, z, None, None);
                    let is_tactile = if is_x_axis {
                        z == cz - 4 || z == cz + 4
                    } else {
                        x == cx - 4 || x == cx + 4
                    };
                    if is_tactile {
                        editor.set_block_absolute(
                            YELLOW_CONCRETE,
                            x,
                            plat_y + offset,
                            z,
                            None,
                            None,
                        );
                    }
                }

                let is_roof = (x + z) % 2 == 0;
                let roof_block = if is_roof { IRON_BLOCK } else { GLASS };
                editor.set_block_absolute(roof_block, x, plat_y + 6 + offset, z, None, None);
            }
        }
    }
}

fn generate_church_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    is_catedral: bool,
) {
    let largura = max_x - min_x;
    let prof = max_z - min_z;

    if largura < 7 || prof < 7 {
        return;
    }

    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            for h in y..ceiling {
                editor.set_block_absolute(AIR, x, h + offset, z, None, None);
            }
        }
    }

    if is_catedral {
        for dx in -3i32..=3i32 {
            for dz in -3i32..=3i32 {
                let dist = dx * dx + dz * dz;
                if dist <= 9 {
                    editor.set_block_absolute(
                        SMOOTH_QUARTZ,
                        cx + dx,
                        y + offset,
                        cz + dz,
                        None,
                        None,
                    );
                } else if dist <= 16 {
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        cx + dx,
                        y + offset,
                        cz + dz,
                        None,
                        None,
                    );
                }
            }
        }
        editor.set_block_absolute(QUARTZ_BLOCK, cx, y + 1 + offset, cz, None, None);
        editor.set_block_absolute(QUARTZ_BLOCK, cx + 1, y + 1 + offset, cz, None, None);
        editor.set_block_absolute(QUARTZ_BLOCK, cx - 1, y + 1 + offset, cz, None, None);
    } else {
        let is_z_axis = prof > largura;

        if is_z_axis {
            let narthex_z = min_z + 4;
            for x in (min_x + 1)..(max_x - 1) {
                for h in y..(ceiling - 2) {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, narthex_z, None, None);
                }
            }
            editor.set_block_absolute(AIR, cx, y + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx, y + 1 + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx - 1, y + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx - 1, y + 1 + offset, narthex_z, None, None);

            let altar_z = max_z - 6;

            for x in (min_x + 1)..(max_x - 1) {
                for h in y..(ceiling - 2) {
                    editor.set_block_absolute(
                        WHITE_CONCRETE,
                        x,
                        h + offset,
                        altar_z + 2,
                        None,
                        None,
                    );
                }
            }
            editor.set_block_absolute(OAK_DOOR, min_x + 3, y + 1 + offset, altar_z + 2, None, None);

            for z in (narthex_z + 1)..altar_z {
                editor.set_block_absolute(RED_CARPET, cx, y + offset, z, None, None);
                editor.set_block_absolute(RED_CARPET, cx - 1, y + offset, z, None, None);

                if (z - min_z) % 4 == 0 {
                    for h in y..ceiling {
                        editor.set_block_absolute(
                            SMOOTH_QUARTZ,
                            min_x + 3,
                            h + offset,
                            z,
                            None,
                            None,
                        );
                        editor.set_block_absolute(
                            SMOOTH_QUARTZ,
                            max_x - 3,
                            h + offset,
                            z,
                            None,
                            None,
                        );
                    }
                }

                if (z - min_z) % 2 == 0 {
                    for bx in (min_x + 4)..(cx - 1) {
                        editor.set_block_absolute(OAK_STAIRS, bx, y + offset, z, None, None);
                    }
                    for bx in (cx + 1)..(max_x - 4) {
                        editor.set_block_absolute(OAK_STAIRS, bx, y + offset, z, None, None);
                    }
                }
            }

            for x in (min_x + 2)..(max_x - 2) {
                editor.set_block_absolute(SMOOTH_QUARTZ, x, y + offset, altar_z, None, None);
            }
            editor.set_block_absolute(GOLD_BLOCK, cx, y + 1 + offset, altar_z, None, None);
        } else {
            let narthex_x = min_x + 4;
            for z in (min_z + 1)..(max_z - 1) {
                for h in y..(ceiling - 2) {
                    editor.set_block_absolute(WHITE_CONCRETE, narthex_x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(AIR, narthex_x, y + offset, cz, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + 1 + offset, cz, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + offset, cz - 1, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + 1 + offset, cz - 1, None, None);

            let altar_x = max_x - 6;

            for z in (min_z + 1)..(max_z - 1) {
                for h in y..(ceiling - 2) {
                    editor.set_block_absolute(
                        WHITE_CONCRETE,
                        altar_x + 2,
                        h + offset,
                        z,
                        None,
                        None,
                    );
                }
            }
            editor.set_block_absolute(OAK_DOOR, altar_x + 2, y + 1 + offset, min_z + 3, None, None);

            for x in (narthex_x + 1)..altar_x {
                editor.set_block_absolute(RED_CARPET, x, y + offset, cz, None, None);
                editor.set_block_absolute(RED_CARPET, x, y + offset, cz - 1, None, None);

                if (x - min_x) % 4 == 0 {
                    for h in y..ceiling {
                        editor.set_block_absolute(
                            SMOOTH_QUARTZ,
                            x,
                            h + offset,
                            min_z + 3,
                            None,
                            None,
                        );
                        editor.set_block_absolute(
                            SMOOTH_QUARTZ,
                            x,
                            h + offset,
                            max_z - 3,
                            None,
                            None,
                        );
                    }
                }

                if (x - min_x) % 2 == 0 {
                    for bz in (min_z + 4)..(cz - 1) {
                        editor.set_block_absolute(OAK_STAIRS, x, y + offset, bz, None, None);
                    }
                    for bz in (cz + 1)..(max_z - 4) {
                        editor.set_block_absolute(OAK_STAIRS, x, y + offset, bz, None, None);
                    }
                }
            }
            for z in (min_z + 2)..(max_z - 2) {
                editor.set_block_absolute(SMOOTH_QUARTZ, altar_x, y + offset, z, None, None);
            }
            editor.set_block_absolute(GOLD_BLOCK, altar_x, y + 1 + offset, cz, None, None);
        }
    }
}

fn generate_monumental_atrium_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    has_core: bool,
) {
    let largura = max_x - min_x;
    let prof = max_z - min_z;

    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;

    let void_min_x = min_x + (largura as f64 * 0.32) as i32;
    let void_max_x = max_x - (largura as f64 * 0.32) as i32;
    let void_min_z = min_z + (prof as f64 * 0.32) as i32;
    let void_max_z = max_z - (prof as f64 * 0.32) as i32;

    if void_max_x - void_min_x < 2 || void_max_z - void_min_z < 2 {
        return;
    }

    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            if has_core && x >= cx - 1 && x <= cx + 2 && z >= cz - 1 && z <= cz + 2 {
                continue;
            }

            let is_void_zone =
                x >= void_min_x && x <= void_max_x && z >= void_min_z && z <= void_max_z;

            // Limpa o ar no miolo e levanta as bordas de vidro/quartzo de forma limpa (Evita Redundância O(n²))
            if is_void_zone {
                for h in y..ceiling {
                    editor.set_block_absolute(AIR, x, h + offset, z, None, None);
                }
            } else {
                let is_void_edge = (x == void_min_x - 1 || x == void_max_x + 1)
                    && z >= void_min_z - 1
                    && z <= void_max_z + 1
                    || (z == void_min_z - 1 || z == void_max_z + 1)
                        && x >= void_min_x - 1
                        && x <= void_max_x + 1;

                if is_void_edge {
                    editor.set_block_absolute(GLASS_PANE, x, y + offset + 1, z, None, None);

                    if (x + z) % 6 == 0 {
                        for h in y..ceiling {
                            editor.set_block_absolute(SMOOTH_QUARTZ, x, h + offset, z, None, None);
                        }
                    }
                }
            }
        }
    }
}

fn generate_shopping_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    has_core: bool,
    start_y: i32,
) {
    generate_monumental_atrium_layout(
        editor, min_x, max_x, min_z, max_z, y, ceiling, offset, has_core,
    );

    let largura = max_x - min_x;
    let prof = max_z - min_z;

    let gallery_min_x = min_x + (largura as f64 * 0.15) as i32;
    let gallery_max_x = max_x - (largura as f64 * 0.15) as i32;
    let gallery_min_z = min_z + (prof as f64 * 0.15) as i32;
    let gallery_max_z = max_z - (prof as f64 * 0.15) as i32;

    for x in (min_x + 1)..(max_x - 1) {
        for z in (min_z + 1)..(max_z - 1) {
            // TWEAK: Condicional de piso corrigida. Só pinta se for laje ou piso elevado ao terreno.
            if y > start_y {
                editor.set_block_absolute(POLISHED_DIORITE, x, y + offset - 1, z, None, None);
            }

            let is_gallery_edge = (x == gallery_min_x || x == gallery_max_x)
                && z >= gallery_min_z
                && z <= gallery_max_z
                || (z == gallery_min_z || z == gallery_max_z)
                    && x >= gallery_min_x
                    && x <= gallery_max_x;

            if is_gallery_edge {
                for h in y..ceiling {
                    let is_pillar = (x + z) % 8 == 0;
                    let block = if is_pillar { WHITE_CONCRETE } else { GLASS };
                    editor.set_block_absolute(block, x, h + offset, z, None, None);
                }
            }
        }
    }
}

fn generate_hospital_base_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    has_core: bool,
) {
    let center_z = (min_z + max_z) / 2;
    let modulo_quarto = (6.0 * H_SCALE).round() as i32;

    let cx = (min_x + max_x) / 2;
    let _cz = (min_z + max_z) / 2;
    let core_x_max = cx + 3;

    editor.set_block_absolute(SMOOTH_QUARTZ, cx - 4, y + offset, center_z, None, None);
    editor.set_block_absolute(SMOOTH_QUARTZ, cx - 4, y + offset, center_z + 1, None, None);
    editor.set_block_absolute(SMOOTH_QUARTZ, cx - 4, y + offset, center_z - 1, None, None);

    for x in (min_x + 1)..(max_x - 1) {
        for z in (center_z - 3)..=(center_z + 3) {
            for h in y..ceiling {
                editor.set_block_absolute(AIR, x, h + offset, z, None, None);
            }
        }

        editor.set_block_absolute(SMOOTH_QUARTZ, x, y + offset, center_z - 4, None, None);
        editor.set_block_absolute(SMOOTH_QUARTZ, x, y + offset, center_z + 4, None, None);

        if (x - min_x) % modulo_quarto == 0 {
            let is_inside_core = has_core && x >= cx - 1 && x <= core_x_max;
            if is_inside_core {
                continue;
            }

            for z in (min_z + 1)..(center_z - 4) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 5)..(max_z - 1) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z - 4, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 2, y + 1 + offset, center_z - 4, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z + 4, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 2, y + 1 + offset, center_z + 4, None, None);
        }
    }
}

// Postos de Saúde e Clínicas genéricas
fn generate_generic_hospital_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    _has_core: bool,
) {
    let center_z = (min_z + max_z) / 2;
    let modulo_quarto = (4.0 * H_SCALE).round() as i32;

    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % modulo_quarto == 0 {
            for z in (min_z + 1)..(center_z - 2) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 3)..(max_z - 1) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z - 3, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z + 3, None, None);
        }
    }
}

fn generate_banco_sede_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
) {
    let modulo = (6.0 * H_SCALE).round() as i32;

    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % modulo == 0 {
            for z in (min_z + 1)..(max_z - 1) {
                for h in y..ceiling {
                    let block = if h == y + 2 {
                        GLASS_PANE
                    } else {
                        LIGHT_GRAY_TERRACOTTA
                    };
                    editor.set_block_absolute(block, x, h + offset, z, None, None);
                }
            }
        }
    }
}

fn generate_residential_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    bairro: Bairro,
    total_floors: usize,
) {
    let largura_interna = (max_x - min_x) - 2;
    let profundidade = (max_z - min_z) - 2;

    let largura_m = largura_interna as f64 / H_SCALE;
    let prof_m = profundidade as f64 / H_SCALE;

    if largura_m < 4.5 || prof_m < 4.5 {
        return;
    }

    let meio_z = min_z + (profundidade / 2);
    let meio_x = min_x + (largura_interna / 2);

    let forro_y = ceiling - 2;
    if forro_y > y + 2 {
        for x in (min_x + 1)..(max_x - 1) {
            for z in (min_z + 1)..(max_z - 1) {
                editor.set_block_absolute(SMOOTH_QUARTZ, x, forro_y + offset, z, None, None);
            }
        }
    }

    let mut suite_z_bound = max_z;

    let is_fachada_z = largura_interna < profundidade;
    let varanda_z = if is_fachada_z { min_z } else { max_z - 1 };

    if largura_m < 6.0 {
        for z in (min_z + 1)..(max_z - 1) {
            for h in y..forro_y {
                editor.set_block_absolute(WHITE_CONCRETE, min_x + 3, h + offset, z, None, None);
            }
        }
        editor.set_block_absolute(OAK_DOOR, min_x + 4, y + 1 + offset, min_z + 3, None, None);
        editor.set_block_absolute(OAK_DOOR, min_x + 4, y + 1 + offset, max_z - 3, None, None);

        for dx in 1i32..=2i32 {
            for dz in 1i32..=3i32 {
                for h in y..forro_y {
                    let is_wall = dx == 2 || dz == 3 || dz == 1;
                    let block = if is_wall { WHITE_CONCRETE } else { AIR };
                    editor.set_block_absolute(
                        block,
                        min_x + 3 + dx,
                        h + offset,
                        meio_z + dz - 1,
                        None,
                        None,
                    );
                }
            }
        }
        editor.set_block_absolute(OAK_DOOR, min_x + 5, y + 1 + offset, meio_z + 1, None, None);
    } else if largura_m <= 9.0 {
        for x in (min_x + 1)..(max_x - 1) {
            for h in y..forro_y {
                editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, meio_z, None, None);
            }
        }

        editor.set_block_absolute(AIR, meio_x, y + 1 + offset, meio_z, None, None);
        editor.set_block_absolute(AIR, meio_x, y + 2 + offset, meio_z, None, None);
        editor.set_block_absolute(AIR, meio_x - 1, y + 1 + offset, meio_z, None, None);
        editor.set_block_absolute(AIR, meio_x - 1, y + 2 + offset, meio_z, None, None);

        for dx in 0i32..=2i32 {
            for dz in 0i32..=3i32 {
                for h in y..forro_y {
                    let is_wall = dx == 0 || dx == 2 || dz == 3;
                    if is_wall {
                        editor.set_block_absolute(
                            WHITE_CONCRETE,
                            min_x + 3 + dx,
                            h + offset,
                            min_z + dz,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        editor.set_block_absolute(OAK_DOOR, min_x + 5, y + 1 + offset, min_z + 2, None, None);

        for z in meio_z..(max_z - 1) {
            for h in y..forro_y {
                editor.set_block_absolute(WHITE_CONCRETE, meio_x, h + offset, z, None, None);
            }
        }
        editor.set_block_absolute(OAK_DOOR, meio_x + 1, y + 1 + offset, meio_z + 2, None, None);
    } else {
        let terco = largura_interna / 3;
        for i in 1..3 {
            let divisao = min_x + terco * i;
            for z in meio_z..(max_z - 1) {
                for h in y..forro_y {
                    editor.set_block_absolute(WHITE_CONCRETE, divisao, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(
                OAK_DOOR,
                divisao + 1,
                y + 1 + offset,
                meio_z + 1,
                None,
                None,
            );
        }

        for x in (min_x + 1)..(max_x - 1) {
            for h in y..forro_y {
                editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, meio_z, None, None);
            }
        }
        for dx in -1i32..=1i32 {
            editor.set_block_absolute(AIR, meio_x + dx, y + 1 + offset, meio_z, None, None);
            editor.set_block_absolute(AIR, meio_x + dx, y + 2 + offset, meio_z, None, None);
        }

        let suite_x = min_x + terco * 2 + 1;
        suite_z_bound = max_z - 4;

        for dx in 0i32..=3i32 {
            for dz in 0i32..=3i32 {
                for h in y..forro_y {
                    let is_wall = dx == 0 || dx == 3 || dz == 0 || dz == 3;
                    if is_wall {
                        editor.set_block_absolute(
                            WHITE_CONCRETE,
                            suite_x + dx,
                            h + offset,
                            max_z - dz - 1,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        editor.set_block_absolute(OAK_DOOR, suite_x, y + 1 + offset, max_z - 2, None, None);
    }

    let kitchen_z = min_z + 1;
    let kitchen_x_start = max_x - 5;

    if bairro == Bairro::Guara || bairro == Bairro::Samambaia {
        if prof_m > 7.0 {
            for dx in 0i32..=4i32 {
                for dz in 0i32..=4i32 {
                    if kitchen_z + dz < suite_z_bound {
                        for h in y..forro_y {
                            let is_wall = dx == 0 || dz == 4;
                            if is_wall {
                                editor.set_block_absolute(
                                    WHITE_CONCRETE,
                                    kitchen_x_start + dx,
                                    h + offset,
                                    kitchen_z + dz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }
            }
            editor.set_block_absolute(
                OAK_DOOR,
                kitchen_x_start + 1,
                y + 1 + offset,
                kitchen_z + 2,
                None,
                None,
            );
            editor.set_block_absolute(
                POLISHED_ANDESITE,
                max_x - 2,
                y + 1 + offset,
                kitchen_z + 1,
                None,
                None,
            );
            editor.set_block_absolute(
                FURNACE,
                max_x - 2,
                y + 1 + offset,
                kitchen_z + 2,
                None,
                None,
            );
        }
    } else {
        for dz in 1i32..=3i32 {
            editor.set_block_absolute(
                SMOOTH_QUARTZ,
                kitchen_x_start,
                y + 1 + offset,
                kitchen_z + dz,
                None,
                None,
            );
        }
        editor.set_block_absolute(
            FURNACE,
            max_x - 2,
            y + 1 + offset,
            kitchen_z + 1,
            None,
            None,
        );
    }

    if bairro == Bairro::Guara || bairro == Bairro::Samambaia {
        for x in (min_x + 1)..(max_x - 1) {
            editor.set_block_absolute(GLASS_PANE, x, y + offset, varanda_z, None, None);
            editor.set_block_absolute(AIR, x, y + 1 + offset, varanda_z, None, None);
            editor.set_block_absolute(AIR, x, y + 2 + offset, varanda_z, None, None);
        }
    }

    let alt_andar = ceiling - y;
    if total_floors > 1 && alt_andar >= 4 && prof_m > 8.0 {
        let escada_x = min_x + 2;
        let escada_z = min_z + 2;
        let patamar_y = y + (alt_andar / 2);

        if escada_x + 3 < max_x && escada_z + 5 < max_z {
            for dx in 0i32..=2i32 {
                for dz in 0i32..=5i32 {
                    for h in forro_y..=(ceiling + 1) {
                        editor.set_block_absolute(
                            AIR,
                            escada_x + dx,
                            h + offset,
                            escada_z + dz,
                            None,
                            None,
                        );
                    }
                }
            }

            let degraus_lance1 = patamar_y - y;
            for i in 0..degraus_lance1 {
                if escada_z + i < max_z {
                    editor.set_block_absolute(
                        STONE_STAIRS,
                        escada_x,
                        y + i + offset,
                        escada_z + i,
                        None,
                        None,
                    );
                    editor.set_block_absolute(
                        STONE_STAIRS,
                        escada_x + 1,
                        y + i + offset,
                        escada_z + i,
                        None,
                        None,
                    );
                    editor.set_block_absolute(
                        GLASS_PANE,
                        escada_x + 2,
                        y + i + 1 + offset,
                        escada_z + i,
                        None,
                        None,
                    );

                    for h in y..(y + i) {
                        editor.set_block_absolute(
                            STONE_BRICKS,
                            escada_x,
                            h + offset,
                            escada_z + i,
                            None,
                            None,
                        );
                        editor.set_block_absolute(
                            STONE_BRICKS,
                            escada_x + 1,
                            h + offset,
                            escada_z + i,
                            None,
                            None,
                        );
                    }
                }
            }

            if escada_z + degraus_lance1 + 1 < max_z {
                for dx in 0i32..=1i32 {
                    for dz in 0i32..=1i32 {
                        editor.set_block_absolute(
                            SMOOTH_STONE_SLAB,
                            escada_x + dx,
                            patamar_y + offset,
                            escada_z + degraus_lance1 + dz,
                            None,
                            None,
                        );
                    }
                }
                editor.set_block_absolute(
                    GLASS_PANE,
                    escada_x + 2,
                    patamar_y + 1 + offset,
                    escada_z + degraus_lance1,
                    None,
                    None,
                );
            }

            let degraus_lance2 = ceiling - patamar_y;
            for i in 0..degraus_lance2 {
                let z_pos = escada_z + degraus_lance1 - 1 - i;
                if z_pos >= min_z {
                    editor.set_block_absolute(
                        STONE_STAIRS,
                        escada_x + 1,
                        patamar_y + i + offset,
                        z_pos,
                        None,
                        None,
                    );
                    editor.set_block_absolute(
                        GLASS_PANE,
                        escada_x,
                        patamar_y + i + 1 + offset,
                        z_pos,
                        None,
                        None,
                    );

                    for h in y..(patamar_y + i) {
                        editor.set_block_absolute(
                            STONE_BRICKS,
                            escada_x + 1,
                            h + offset,
                            z_pos,
                            None,
                            None,
                        );
                    }
                }
            }
        }
    }
}

fn generate_school_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
) {
    let largura_interna = max_x - min_x;
    let tamanho_sala = (7.0 * H_SCALE).round() as i32;

    if largura_interna < tamanho_sala * 2 {
        return;
    }

    let center_z = (min_z + max_z) / 2;

    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % tamanho_sala == 0 {
            for z in (min_z + 1)..(center_z - 2) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 3)..(max_z - 1) {
                for h in y..ceiling {
                    editor.set_block_absolute(WHITE_CONCRETE, x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z - 3, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z + 3, None, None);
        }
    }
}

fn generate_office_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
) {
    let modulo = (4.0 * H_SCALE).round() as i32;

    let center_z = (min_z + max_z) / 2;

    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % modulo == 0 {
            for z in (min_z + 1)..(center_z - 1) {
                for h in y..ceiling {
                    let block = if h == y + 2 {
                        GLASS_PANE
                    } else {
                        LIGHT_GRAY_TERRACOTTA
                    };
                    editor.set_block_absolute(block, x, h + offset, z, None, None);
                }
            }
        }
    }
}

//
// =====================================================
// FUNÇÃO PRINCIPAL: GERADOR HIERÁRQUICO
// =====================================================
//

#[allow(clippy::too_many_arguments)]
pub fn generate_building_interior(
    editor: &mut WorldEditor,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    start_y: i32,
    height: i32,
    floors: &[i32],
    element: &crate::osm_parser::ProcessedWay,
    offset: i32,
) {
    let edificio_unico = detect_edificio_especifico(element);
    let bairro = detect_bairro(element);
    let tipologia = detect_tipologia(element);
    let total_floors = floors.len();

    let altura_padrao = pe_direito_blocos();

    // --- INTERCEPTADORES MONUMENTAIS E DE INFRAESTRUTURA ---

    if tipologia == Tipologia::MetroSubterraneo {
        generate_metro_underground_layout(editor, min_x, max_x, min_z, max_z, start_y, offset);
        return;
    }

    if tipologia == Tipologia::MetroElevado {
        generate_metro_elevated_layout(editor, min_x, max_x, min_z, max_z, start_y, offset);
        return;
    }

    if tipologia == Tipologia::Religioso {
        // TWEAK PROTEÇÃO (Evita Panic caso floors esteja vazio no OSM)
        if floors.is_empty() {
            return;
        }

        let is_catedral = edificio_unico == EdificioBrasilia::CatedralMetropolitana;
        let ceiling = start_y + height;
        generate_church_layout(
            editor,
            min_x,
            max_x,
            min_z,
            max_z,
            floors[0] + 1,
            ceiling,
            offset,
            is_catedral,
        );
        return;
    }

    for i in 0..total_floors {
        let floor_y = floors[i];

        let mut ceiling = if i < total_floors - 1 {
            floors[i + 1]
        } else {
            start_y + height
        };

        if ceiling - floor_y < altura_padrao - 1 {
            ceiling = floor_y + altura_padrao;
        }

        let is_tipologia_vertical = tipologia == Tipologia::Hospital
            || tipologia == Tipologia::Corporativo
            || tipologia == Tipologia::Comercial;
        let has_core = (total_floors > 2) || (height > 12 && is_tipologia_vertical);

        let is_atrium = matches!(
            edificio_unico,
            EdificioBrasilia::CongressoNacional
                | EdificioBrasilia::STF
                | EdificioBrasilia::PalacioPlanalto
                | EdificioBrasilia::Itamaraty
                | EdificioBrasilia::Shopping
        );

        if i > 0 {
            generate_laje(
                editor, min_x, max_x, min_z, max_z, floor_y, offset, has_core, is_atrium,
            );
        }

        match edificio_unico {
            EdificioBrasilia::CongressoNacional
            | EdificioBrasilia::STF
            | EdificioBrasilia::PalacioPlanalto
            | EdificioBrasilia::Itamaraty => {
                generate_monumental_atrium_layout(
                    editor,
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    floor_y + 1,
                    ceiling,
                    offset,
                    has_core,
                );
                if has_core {
                    generate_elevator_core(
                        editor,
                        min_x,
                        max_x,
                        min_z,
                        max_z,
                        floor_y + 1,
                        ceiling,
                        offset,
                        tipologia,
                    );
                }
            }
            EdificioBrasilia::Shopping => {
                generate_shopping_layout(
                    editor,
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    floor_y + 1,
                    ceiling,
                    offset,
                    has_core,
                    start_y,
                );
                if has_core {
                    generate_elevator_core(
                        editor,
                        min_x,
                        max_x,
                        min_z,
                        max_z,
                        floor_y + 1,
                        ceiling,
                        offset,
                        tipologia,
                    );
                }
            }
            EdificioBrasilia::HospitalBase => {
                // Design Exclusivo do Hospital de Base (Maior da América Latina)
                generate_hospital_base_layout(
                    editor,
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    floor_y + 1,
                    ceiling,
                    offset,
                    has_core,
                );
                if has_core {
                    generate_elevator_core(
                        editor,
                        min_x,
                        max_x,
                        min_z,
                        max_z,
                        floor_y + 1,
                        ceiling,
                        offset,
                        tipologia,
                    );
                }
            }
            EdificioBrasilia::SedeBancaria => {
                generate_banco_sede_layout(
                    editor,
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    floor_y + 1,
                    ceiling,
                    offset,
                );
                if has_core {
                    generate_elevator_core(
                        editor,
                        min_x,
                        max_x,
                        min_z,
                        max_z,
                        floor_y + 1,
                        ceiling,
                        offset,
                        tipologia,
                    );
                }
            }
            EdificioBrasilia::CatedralMetropolitana => {}
            EdificioBrasilia::Nenhum => {
                match tipologia {
                    Tipologia::Escola => {
                        generate_corridor(
                            editor,
                            min_x,
                            max_x,
                            min_z,
                            max_z,
                            floor_y + 1,
                            ceiling,
                            offset,
                            bairro,
                            tipologia,
                        );
                        generate_school_layout(
                            editor, min_x, max_x, min_z, max_z, floor_y, ceiling, offset,
                        );
                        if has_core {
                            generate_elevator_core(
                                editor,
                                min_x,
                                max_x,
                                min_z,
                                max_z,
                                floor_y + 1,
                                ceiling,
                                offset,
                                tipologia,
                            );
                        }
                    }
                    Tipologia::Hospital => {
                        // Hospitais Genéricos e Postos de Saúde (Clínicas Candangas)
                        generate_corridor(
                            editor,
                            min_x,
                            max_x,
                            min_z,
                            max_z,
                            floor_y + 1,
                            ceiling,
                            offset,
                            bairro,
                            tipologia,
                        );
                        generate_generic_hospital_layout(
                            editor,
                            min_x,
                            max_x,
                            min_z,
                            max_z,
                            floor_y + 1,
                            ceiling,
                            offset,
                            has_core,
                        );
                        if has_core {
                            generate_elevator_core(
                                editor,
                                min_x,
                                max_x,
                                min_z,
                                max_z,
                                floor_y + 1,
                                ceiling,
                                offset,
                                tipologia,
                            );
                        }
                    }
                    Tipologia::Corporativo => {
                        generate_corridor(
                            editor,
                            min_x,
                            max_x,
                            min_z,
                            max_z,
                            floor_y + 1,
                            ceiling,
                            offset,
                            bairro,
                            tipologia,
                        );
                        generate_office_layout(
                            editor, min_x, max_x, min_z, max_z, floor_y, ceiling, offset,
                        );
                        if has_core {
                            generate_elevator_core(
                                editor,
                                min_x,
                                max_x,
                                min_z,
                                max_z,
                                floor_y + 1,
                                ceiling,
                                offset,
                                tipologia,
                            );
                        }
                    }
                    Tipologia::MetroSubterraneo
                    | Tipologia::MetroElevado
                    | Tipologia::Religioso => {}
                    Tipologia::Residencial | Tipologia::Comercial | Tipologia::Generico => {
                        match bairro {
                            Bairro::SQS => {
                                if i == 0 {
                                    continue;
                                }
                                generate_corridor(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    ceiling,
                                    offset,
                                    bairro,
                                    tipologia,
                                );
                                if has_core {
                                    generate_elevator_core(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        tipologia,
                                    );
                                }
                            }
                            Bairro::SQN => {
                                if i == 0 && has_core {
                                    generate_elevator_core(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        tipologia,
                                    );
                                    continue;
                                }
                                generate_corridor(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    ceiling,
                                    offset,
                                    bairro,
                                    tipologia,
                                );
                                if has_core {
                                    generate_elevator_core(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        tipologia,
                                    );
                                }
                            }
                            Bairro::Comercial | Bairro::AguasClaras => {
                                generate_corridor(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    ceiling,
                                    offset,
                                    bairro,
                                    tipologia,
                                );
                                if has_core {
                                    generate_elevator_core(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        tipologia,
                                    );
                                }
                            }
                            Bairro::Guara | Bairro::Samambaia | Bairro::Condominio => {
                                if height <= 12 && tipologia == Tipologia::Residencial {
                                    generate_residential_layout(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y,
                                        ceiling,
                                        offset,
                                        bairro,
                                        total_floors,
                                    );
                                } else {
                                    generate_corridor(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        bairro,
                                        tipologia,
                                    );
                                    if has_core {
                                        generate_elevator_core(
                                            editor,
                                            min_x,
                                            max_x,
                                            min_z,
                                            max_z,
                                            floor_y + 1,
                                            ceiling,
                                            offset,
                                            tipologia,
                                        );
                                    }
                                }
                            }
                            Bairro::Outro => {
                                if has_core {
                                    generate_corridor(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        bairro,
                                        tipologia,
                                    );
                                    generate_elevator_core(
                                        editor,
                                        min_x,
                                        max_x,
                                        min_z,
                                        max_z,
                                        floor_y + 1,
                                        ceiling,
                                        offset,
                                        tipologia,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
