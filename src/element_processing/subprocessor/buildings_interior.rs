use crate::block_definitions::*;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::world_editor::WorldEditor;
use rand::Rng;

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

/// Ruído orgânico de textura para parede interna — sem isso, TODA parede interna
/// gerada pelo motor (quarto, banheiro, corredor, escritório, banco, loja) era um
/// branco-concreto uniforme e pixel-idêntico em todo prédio, independente do prédio
/// ou da posição: zero individuação entre uma superquadra e outra, ou mesmo entre dois
/// cômodos do mesmo apartamento. ~12% de chance de uma variante sutil (terracota
/// branca ou quartzo liso) por posição exata do bloco — determinístico via
/// `coord_rng`, então estável entre execuções, mas nunca repetitivo de parede a
/// parede. Efeito pretendido é textura de reboco/pintura envelhecida, não um padrão
/// xadrez perceptível.
fn parede_organica(x: i32, y: i32, z: i32, id: u64) -> Block {
    let mut rng = coord_rng(x, y, z, id);
    let roll: f64 = rng.random();
    if roll < 0.06 {
        WHITE_TERRACOTTA
    } else if roll < 0.12 {
        SMOOTH_QUARTZ
    } else {
        WHITE_CONCRETE
    }
}

/// Mesma ideia de `parede_organica`, para a paleta cinza-claro das partições de vidro
/// de escritório/banco (`generate_office_layout`, `generate_banco_sede_layout`) — sem
/// isso, toda sala de escritório/agência do motor tinha a MESMA cor de partição, sala
/// após sala, prédio após prédio.
fn particao_organica(x: i32, y: i32, z: i32, id: u64) -> Block {
    let mut rng = coord_rng(x, y, z, id);
    if rng.random_bool(0.1) {
        GRAY_TERRACOTTA
    } else {
        LIGHT_GRAY_TERRACOTTA
    }
}

//
// =====================================================
// ENUMS DE CONTEXTO (BAIRRO, TIPOLOGIA E EDIFÍCIO ÚNICO)
// =====================================================
//

#[derive(PartialEq, Clone, Copy)]
#[allow(clippy::upper_case_acronyms)]
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
#[allow(clippy::upper_case_acronyms)]
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

/// Casas isoladas/geminadas/sobrados nunca têm pilotis — pilotis é um traço específico
/// dos blocos de apartamento das superquadras (Lúcio Costa/Niemeyer: prédio erguido sobre
/// pilares, térreo aberto sem cômodos). Uma casa térrea ou sobrado em condomínio horizontal
/// de Águas Claras/Taguatinga/Ceilândia/Samambaia tem sala, cozinha e quartos JÁ no térreo.
/// Lista de valores de `building` alinhada com `BuildingCategory::House` em `buildings.rs`.
fn eh_casa_terrea_ou_sobrado(element: &crate::osm_parser::ProcessedWay) -> bool {
    matches!(
        element.tags.get("building").map(|s| s.as_str()),
        Some("house")
            | Some("detached")
            | Some("semidetached_house")
            | Some("terrace")
            | Some("bungalow")
            | Some("villa")
            | Some("cabin")
            | Some("hut")
    )
}

/// Uso misto real: prédio residencial (bloco/torre, nunca casa) cujo térreo é loja/
/// comércio de rua e os andares de cima são apartamentos — o padrão urbano mais comum
/// nos centros comerciais das satélites (Taguatinga Centro, Ceilândia Centro,
/// Sobradinho, Gama, Planaltina) e presente também em trechos da W3 Sul/Norte do Plano
/// Piloto. O sinal disponível no OSM não é um valor dedicado de `building`, mas a
/// combinação de uma tag residencial (`building=apartments|residential`) com uma tag de
/// comércio (`shop=*`/`office=*`) no MESMO way — sinal imperfeito, mas real e
/// determinístico, em vez de arriscar um heurístico sem base na tag.
/// Testeiras/letreiros de loja: a paleta única do motor para identidade visual
/// de comércio — usada nas galerias internas dos shoppings (abaixo) e nas
/// fachadas de loja de rua (`buildings.rs`, faixa acima da vitrine).
pub const VITRINE_COLORS: [Block; 6] = [
    ORANGE_TERRACOTTA,
    RED_TERRACOTTA,
    YELLOW_TERRACOTTA,
    CYAN_TERRACOTTA,
    BLUE_TERRACOTTA,
    GREEN_TERRACOTTA,
];

pub fn eh_uso_misto_comercio_terreo(element: &crate::osm_parser::ProcessedWay) -> bool {
    let building_residencial = matches!(
        element.tags.get("building").map(|s| s.as_str()),
        Some("residential") | Some("apartments")
    );
    building_residencial
        && (element.tags.contains_key("shop") || element.tags.contains_key("office"))
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
    } else if name.contains("shopping")
        || name.contains("conjunto nacional")
        || element.tags.get("shop").map(|s| s.as_str()) == Some("mall")
    {
        // `shop=mall` é o sinal real e independente de nome — sem ele, shoppings
        // reais do DF sem "shopping" no nome (Pátio Brasil, Iguatemi Brasília)
        // nunca recebiam o átrio/galeria de `generate_shopping_layout`, só o
        // corredor genérico de escritório/comércio.
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

    // Iluminação do saguão: sem isso, todo andar de todo prédio com núcleo (qualquer
    // torre acima de 2 pavimentos) teria uma caixa de pedra completamente escura diante
    // das portas do elevador — nenhum saguão real do DF é assim, e no motor original
    // isso valia para cada andar de cada torre gerada.
    editor.set_block_absolute(SEA_LANTERN, cx, ceiling - 1 + offset, cz + 1, None, None);
}

#[allow(clippy::too_many_arguments)]
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
    id: u64,
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
                let wall_a = if tipologia == Tipologia::Hospital {
                    SMOOTH_QUARTZ
                } else {
                    parede_organica(x, y + offset, cz_end + 1, id)
                };
                let wall_b = if tipologia == Tipologia::Hospital {
                    SMOOTH_QUARTZ
                } else {
                    parede_organica(x, y + offset, cz_start - 1, id)
                };
                editor.set_block_absolute(wall_a, x, y + offset, cz_end + 1, None, None);
                editor.set_block_absolute(wall_b, x, y + offset, cz_start - 1, None, None);
            }
        }
    }

    // Iluminação: sem isso, todo corredor de todo andar comercial/genérico seria um
    // túnel de concreto completamente escuro — nenhum corredor real de prédio
    // comercial/institucional do DF é assim. `center` está sempre dentro da faixa
    // vazada (`cz_start..=cz_end`) em todos os ramos acima.
    let modulo_luz = ((5.0 * H_SCALE).round() as i32).max(1);
    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % modulo_luz == 0 {
            editor.set_block_absolute(GLOWSTONE, x, ceiling - 1 + offset, center, None, None);
        }
    }
}

/// Escada de dois lances com patamar, ligando o piso `y` ao piso `ceiling` (nível de
/// laje do andar de cima). Reconectada a partir de uma escada que já existia dentro de
/// `generate_residential_layout`, mas com um gatilho errado (`total_floors > 1` do
/// prédio inteiro, não da unidade específica): isso fazia CADA apartamento de CADA
/// andar de QUALQUER torre residencial furar o piso do vizinho de cima com um vão de
/// escada privado — nunca deveria acontecer numa unidade que usa elevador/corredor
/// comum. Agora extraída como helper reutilizável, chamada só em dois cenários reais:
/// (1) dentro da própria unidade, quando é o térreo de um sobrado/casa de dois
/// pavimentos (`tem_andar_acima` em `generate_residential_layout`); e (2) como escada
/// COMUM de um bloco residencial sem elevador — muitíssimo comum no DF real (blocos
/// baixos e antigos de SQN/SQS, prédios de satélites de até 3-4 pavimentos), onde
/// antes desta reconexão não existia NENHUMA circulação vertical entre andares além do
/// núcleo de elevador — um bloco sem elevador simplesmente não tinha como um morador
/// chegar ao 2º andar.
#[allow(clippy::too_many_arguments)]
fn gerar_escada_interna(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    centralizada: bool,
) {
    let prof_m = (max_z - min_z) as f64 / H_SCALE;
    let alt_andar = ceiling - y;
    if alt_andar < 4 || prof_m <= 8.0 {
        return;
    }

    let forro_y = ceiling - 2;
    // Escada comum de bloco (`centralizada`): ancorada no mesmo ponto do núcleo de
    // elevador (`generate_elevator_core`), para que a zona de exclusão já calculada em
    // `gerar_terreo_condominio` (portaria/bicicletário/salão) continue valendo mesmo
    // quando é escada, e não elevador, ocupando o centro do térreo. Escada privada de
    // sobrado: no canto de entrada da própria unidade, como antes da extração.
    let (escada_x, escada_z) = if centralizada {
        ((min_x + max_x) / 2 - 1, (min_z + max_z) / 2 - 1)
    } else {
        (min_x + 2, min_z + 2)
    };
    let patamar_y = y + (alt_andar / 2);

    if escada_x + 3 >= max_x || escada_z + 5 >= max_z {
        return;
    }

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
                if mez_y < ground_y {
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

#[allow(clippy::too_many_arguments)]
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
    id: u64,
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
                    let bloco = parede_organica(x, h + offset, narthex_z, id);
                    editor.set_block_absolute(bloco, x, h + offset, narthex_z, None, None);
                }
            }
            editor.set_block_absolute(AIR, cx, y + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx, y + 1 + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx - 1, y + offset, narthex_z, None, None);
            editor.set_block_absolute(AIR, cx - 1, y + 1 + offset, narthex_z, None, None);

            let altar_z = max_z - 6;

            for x in (min_x + 1)..(max_x - 1) {
                for h in y..(ceiling - 2) {
                    let bloco = parede_organica(x, h + offset, altar_z + 2, id);
                    editor.set_block_absolute(bloco, x, h + offset, altar_z + 2, None, None);
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
                    let bloco = parede_organica(narthex_x, h + offset, z, id);
                    editor.set_block_absolute(bloco, narthex_x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(AIR, narthex_x, y + offset, cz, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + 1 + offset, cz, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + offset, cz - 1, None, None);
            editor.set_block_absolute(AIR, narthex_x, y + 1 + offset, cz - 1, None, None);

            let altar_x = max_x - 6;

            for z in (min_z + 1)..(max_z - 1) {
                for h in y..(ceiling - 2) {
                    let bloco = parede_organica(altar_x + 2, h + offset, z, id);
                    editor.set_block_absolute(bloco, altar_x + 2, h + offset, z, None, None);
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
                // Faixa de vitrine colorida (testeira/letreiro de loja) na primeira
                // fiada acima do piso — sem isso, a galeria inteira era um vidro liso
                // e uniforme, sem nada que distinguisse uma loja da vizinha. Nenhum
                // shopping real (Conjunto Nacional, ParkShopping, Taguatinga Shopping)
                // tem uma galeria de vidro homogêneo — cada loja tem sua própria
                // identidade visual na testeira.
                for h in y..ceiling {
                    let is_pillar = (x + z) % 8 == 0;
                    let block = if is_pillar {
                        WHITE_CONCRETE
                    } else if h == y + 1 {
                        let idx = (x as i64 + z as i64 * 3).rem_euclid(VITRINE_COLORS.len() as i64);
                        VITRINE_COLORS[idx as usize]
                    } else {
                        GLASS
                    };
                    editor.set_block_absolute(block, x, h + offset, z, None, None);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
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
    id: u64,
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
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 5)..(max_z - 1) {
                for h in y..ceiling {
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
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
#[allow(clippy::too_many_arguments)]
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
    id: u64,
) {
    let center_z = (min_z + max_z) / 2;
    let modulo_quarto = (4.0 * H_SCALE).round() as i32;

    for x in (min_x + 1)..(max_x - 1) {
        if (x - min_x) % modulo_quarto == 0 {
            for z in (min_z + 1)..(center_z - 2) {
                for h in y..ceiling {
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 3)..(max_z - 1) {
                for h in y..ceiling {
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z - 3, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z + 3, None, None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Sede bancária/agência: antes desta reconexão, era só uma fileira de partições de
/// vidro sem nenhuma porta (salas isoladas, inacessíveis a pé) e nada que lembrasse um
/// banco de verdade — nem guichê, nem cofre. Reconectada com vão de porta em cada
/// partição (circulação real entre salas) e, no meio de cada sala, um guichê de
/// atendimento (balcão + grade) alternando com uma porta reforçada de cofre — a dupla
/// imagem mais reconhecível de agência bancária real do DF (Banco do Brasil, Caixa,
/// Banco Central, BRB).
fn generate_banco_sede_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    id: u64,
) {
    let modulo = ((6.0 * H_SCALE).round() as i32).max(2);
    let meio_z = (min_z + max_z) / 2;

    for x in (min_x + 1)..(max_x - 1) {
        let rel = x - min_x;
        if rel % modulo == 0 {
            for z in (min_z + 1)..(max_z - 1) {
                for h in y..ceiling {
                    let block = if h == y + 2 {
                        GLASS_PANE
                    } else {
                        particao_organica(x, h + offset, z, id)
                    };
                    editor.set_block_absolute(block, x, h + offset, z, None, None);
                }
            }
            if meio_z > min_z + 1 && meio_z < max_z - 1 {
                editor.set_block_absolute(IRON_DOOR, x, y + 1 + offset, meio_z, None, None);
                editor.set_block_absolute(AIR, x, y + 2 + offset, meio_z, None, None);
            }
        } else if rel % modulo == modulo / 2 {
            let mut rng = element_rng(id.wrapping_add(x as u64));
            for &z in &[min_z + 2, max_z - 2] {
                if z > min_z + 1 && z < max_z - 2 {
                    editor.set_block_absolute(SMOOTH_STONE_SLAB, x, y + 1 + offset, z, None, None);
                    editor.set_block_absolute(IRON_BARS, x, y + 2 + offset, z, None, None);
                }
            }
            if rng.random_bool(0.3) && meio_z + 2 < max_z - 1 {
                editor.set_block_absolute(IRON_DOOR, x, y + 1 + offset, meio_z + 2, None, None);
            }
        }
    }
}

//
// =====================================================
// 🚨 RECONEXÃO: MOBILIÁRIO E CÔMODOS REAIS (Fidelidade Residencial DF)
// =====================================================
// Antes desta reconexão, `generate_residential_layout` só erguia paredes e portas —
// nenhum cômodo tinha cama, guarda-roupa, sofá, vaso sanitário ou área de serviço.
// Toda unidade residencial candanga real (apartamento de superquadra, casa de
// condomínio horizontal, sobrado de satélite) tem, no mínimo, um banheiro — isso
// nunca existia no motor. Os helpers abaixo são propositalmente pequenos e aditivos
// (só preenchem espaço de ar já dentro de cômodos existentes, ou usam cantos de
// entrada comprovadamente livres nas três faixas de largura) para não arriscar
// sobrepor as paredes já calculadas em `generate_residential_layout`.

#[derive(Clone, Copy)]
enum Direcao {
    Norte,
    Sul,
    Leste,
    Oeste,
}

/// Cama de casal (2 blocos: pé + cabeceira) — a mobília mais básica que faltava em
/// TODO quarto gerado pelo motor até aqui.
fn mobilia_cama(
    editor: &mut WorldEditor,
    foot_x: i32,
    y: i32,
    foot_z: i32,
    offset: i32,
    dir: Direcao,
) {
    let (foot, head, dx, dz) = match dir {
        Direcao::Norte => (RED_BED_NORTH_FOOT, RED_BED_NORTH_HEAD, 0, -1),
        Direcao::Sul => (RED_BED_SOUTH_FOOT, RED_BED_SOUTH_HEAD, 0, 1),
        Direcao::Leste => (RED_BED_EAST_FOOT, RED_BED_EAST_HEAD, 1, 0),
        Direcao::Oeste => (RED_BED_WEST_FOOT, RED_BED_WEST_HEAD, -1, 0),
    };
    editor.set_block_absolute(foot, foot_x, y + offset, foot_z, None, None);
    editor.set_block_absolute(head, foot_x + dx, y + offset, foot_z + dz, None, None);
}

/// Guarda-roupa embutido — quase universal em quartos brasileiros reais; nunca existia aqui.
fn mobilia_guarda_roupa(editor: &mut WorldEditor, x: i32, y: i32, z: i32, offset: i32) {
    editor.set_block_absolute(CHEST, x, y + 1 + offset, z, None, None);
}

/// Sofá simplificado (degrau + tapete) para a sala de estar.
fn mobilia_sofa(editor: &mut WorldEditor, x: i32, y: i32, z: i32, offset: i32, carpet: Block) {
    editor.set_block_absolute(OAK_STAIRS, x, y + 1 + offset, z, None, None);
    editor.set_block_absolute(carpet, x, y + offset, z, None, None);
}

/// Banheiro compacto (2x2), com vaso sanitário (aproximação via `CAULDRON`, já que o
/// Minecraft não tem um bloco de vaso dedicado) e piso de terracota diferenciado.
/// Presente em TODA unidade real — antes, nenhuma tinha.
fn gerar_banheiro(
    editor: &mut WorldEditor,
    x: i32,
    y: i32,
    z: i32,
    forro_y: i32,
    offset: i32,
    id: u64,
) {
    for dx in 0i32..=1 {
        for dz in 0i32..=1 {
            for h in y..forro_y {
                let is_wall = dx == 0 || dz == 0;
                if is_wall {
                    let bloco = parede_organica(x + dx, h + offset, z + dz, id);
                    editor.set_block_absolute(bloco, x + dx, h + offset, z + dz, None, None);
                }
            }
            editor.set_block_absolute(WHITE_TERRACOTTA, x + dx, y + offset, z + dz, None, None);
        }
    }
    editor.set_block_absolute(OAK_DOOR, x + 1, y + 1 + offset, z, None, None);
    editor.set_block_absolute(CAULDRON, x + 1, y + 1 + offset, z + 1, None, None);

    let mut rng = element_rng(id);
    if rng.random_bool(0.5) {
        // Espelho/armarinho sobre a pia — variação orgânica, nem todo banheiro tem.
        editor.set_block_absolute(IRON_TRAPDOOR, x, y + 2 + offset, z + 1, None, None);
    }
}

/// Área de serviço (tanque + máquina de lavar) encostada na parede externa da
/// cozinha — item característico do apartamento brasileiro e completamente ausente
/// do motor antes desta reconexão.
fn gerar_area_servico(editor: &mut WorldEditor, x: i32, y: i32, z: i32, offset: i32) {
    editor.set_block_absolute(BARREL, x, y + 1 + offset, z, None, None); // máquina de lavar
    editor.set_block_absolute(CAULDRON, x, y + offset, z + 1, None, None); // tanque
}

/// Área comum do térreo/pilotis de um condomínio vertical real do DF: portaria com
/// caixas de correio (presente mesmo nos blocos mais antigos de SQN/SQS — zeladoria é
/// item universal, não de condomínio moderno) e bicicletário; e, só quando o pilotis
/// sobra espaço de verdade além do núcleo de elevador, um pequeno salão de festas —
/// item mais recente (condomínios de Águas Claras e satélites, blocos reformados),
/// por isso condicionado ao tamanho real do térreo em vez de a um bairro específico,
/// no mesmo espírito da reconexão de `generate_residential_layout`. Antes desta
/// reconexão, todo pilotis de torre era só o poço do elevador cercado de piso vazio —
/// nenhuma torre residencial do DF real tem só isso no térreo.
///
/// Nunca chamada para casas/sobrados (`eh_casa_terrea_ou_sobrado`): pilotis e portaria
/// são traços de prédio de apartamentos, não de residência unifamiliar.
#[allow(clippy::too_many_arguments)]
fn gerar_terreo_condominio(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    offset: i32,
    id: u64,
) {
    let largura_m = (max_x - min_x) as f64 / H_SCALE;
    let prof_m = (max_z - min_z) as f64 / H_SCALE;

    if largura_m < 10.0 || prof_m < 8.0 {
        return;
    }

    // Pegada exata do poço do elevador OU da escada comum centralizada (mesma origem
    // `(cx-1, cz-1)` usada por ambas — ver `generate_elevator_core` e
    // `gerar_escada_interna` com `centralizada = true` — com folga extra), usada só
    // para garantir por construção que a mobília do térreo nunca sobreponha qualquer
    // que seja a circulação vertical do prédio — não para redesenhá-la. `core_max_z`
    // cobre até `cz+4` porque a escada (mais profunda que o poço do elevador) usa
    // `dz 0..=5` a partir de `cz-1`.
    let cx = (min_x + max_x) / 2;
    let cz = (min_z + max_z) / 2;
    let core_min_x = cx - 1;
    let core_max_x = cx + 3;
    let core_min_z = cz - 1;
    let core_max_z = cz + 4;

    let livre = |x: i32, z: i32| -> bool {
        x > min_x
            && x < max_x - 1
            && z > min_z
            && z < max_z - 1
            && !(x >= core_min_x && x <= core_max_x && z >= core_min_z && z <= core_max_z)
    };

    // Portaria/zeladoria: balcão com luminária no canto noroeste do pilotis.
    let px = min_x + 2;
    let pz = min_z + 2;
    if livre(px, pz) && livre(px + 1, pz) {
        editor.set_block_absolute(BOOKSHELF, px, y + 1 + offset, pz, None, None);
        editor.set_block_absolute(BOOKSHELF, px, y + 2 + offset, pz, None, None);
        editor.set_block_absolute(SMOOTH_STONE_SLAB, px + 1, y + 1 + offset, pz, None, None);
        editor.set_block_absolute(GLOWSTONE, px + 1, y + 2 + offset, pz, None, None);

        // Caixas de correio: fileira de alçapões na parede ao lado da portaria.
        for i in 0..3 {
            let mx = px + i;
            if livre(mx, pz + 1) {
                editor.set_block_absolute(IRON_TRAPDOOR, mx, y + 1 + offset, pz + 1, None, None);
            }
        }
    }

    // Bicicletário: fileira de cercas no canto nordeste, longe da portaria.
    let bike_x = max_x - 3;
    for dz in 0..3 {
        let bz = min_z + 2 + dz;
        if livre(bike_x, bz) {
            editor.set_block_absolute(OAK_FENCE, bike_x, y + 1 + offset, bz, None, None);
        }
    }

    // Salão de festas: só quando sobra espaço real além do núcleo/portaria/bicicletário.
    if largura_m >= 16.0 && prof_m >= 12.0 {
        let mut rng = element_rng(id.wrapping_add(11));
        let sx = max_x - 7;
        let sz = max_z - 7;
        for dx in 0..2 {
            for dz in 0..2 {
                let tx = sx + dx * 3;
                let tz = sz + dz * 3;
                if livre(tx, tz) && livre(tx + 1, tz) {
                    editor.set_block_absolute(
                        SMOOTH_STONE_SLAB,
                        tx,
                        y + 1 + offset,
                        tz,
                        None,
                        None,
                    );
                    let cadeira = if rng.random_bool(0.5) {
                        OAK_STAIRS
                    } else {
                        STONE_BRICK_STAIRS
                    };
                    editor.set_block_absolute(cadeira, tx + 1, y + 1 + offset, tz, None, None);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_residential_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    id: u64,
    tem_andar_acima: bool,
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
        // 🚨 RECONEXÃO: iluminação de teto — antes desta reconexão nenhuma unidade
        // residencial gerada pelo motor (apartamento OU casa) tinha uma única fonte
        // de luz; toda unidade era escura por dentro. A grade de luminárias no forro
        // é sempre segura: o forro cobre uniformemente todo o interior, então
        // qualquer ponto substituído aqui nunca invade parede ou mobília (que ficam
        // estritamente abaixo de `forro_y`, em `y..forro_y`).
        let modulo_luz = ((4.0 * H_SCALE).round() as i32).max(1);
        for x in (min_x + 1)..(max_x - 1) {
            for z in (min_z + 1)..(max_z - 1) {
                let is_luz = (x - min_x) % modulo_luz == 0 && (z - min_z) % modulo_luz == 0;
                let teto = if is_luz { GLOWSTONE } else { SMOOTH_QUARTZ };
                editor.set_block_absolute(teto, x, forro_y + offset, z, None, None);
            }
        }
    }

    let mut suite_z_bound = max_z;

    let is_fachada_z = largura_interna < profundidade;
    let varanda_z = if is_fachada_z { min_z } else { max_z - 1 };

    if largura_m < 6.0 {
        for z in (min_z + 1)..(max_z - 1) {
            for h in y..forro_y {
                let bloco = parede_organica(min_x + 3, h + offset, z, id);
                editor.set_block_absolute(bloco, min_x + 3, h + offset, z, None, None);
            }
        }
        editor.set_block_absolute(OAK_DOOR, min_x + 4, y + 1 + offset, min_z + 3, None, None);
        editor.set_block_absolute(OAK_DOOR, min_x + 4, y + 1 + offset, max_z - 3, None, None);

        for dx in 1i32..=2i32 {
            for dz in 1i32..=3i32 {
                for h in y..forro_y {
                    let is_wall = dx == 2 || dz == 3 || dz == 1;
                    let block = if is_wall {
                        parede_organica(min_x + 3 + dx, h + offset, meio_z + dz - 1, id)
                    } else {
                        AIR
                    };
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

        // 🚨 RECONEXÃO: unidade compacta (quitinete) — sem espaço seguro para uma
        // cama de casal sem risco de sobrepor a própria alcova; só o guarda-roupa.
        mobilia_guarda_roupa(editor, max_x - 2, y, meio_z, offset);
    } else if largura_m <= 9.0 {
        for x in (min_x + 1)..(max_x - 1) {
            for h in y..forro_y {
                let bloco = parede_organica(x, h + offset, meio_z, id);
                editor.set_block_absolute(bloco, x, h + offset, meio_z, None, None);
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
                        let bloco = parede_organica(min_x + 3 + dx, h + offset, min_z + dz, id);
                        editor.set_block_absolute(
                            bloco,
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
                let bloco = parede_organica(meio_x, h + offset, z, id);
                editor.set_block_absolute(bloco, meio_x, h + offset, z, None, None);
            }
        }
        editor.set_block_absolute(OAK_DOOR, meio_x + 1, y + 1 + offset, meio_z + 2, None, None);

        // 🚨 RECONEXÃO: quarto (oeste do corredor) mobiliado — antes era só parede e
        // porta, sem função visível. A cama cabe mesmo na dimensão mínima desta
        // faixa (6.0m); guarda-roupa só entra com folga extra de profundidade
        // comprovada, pra nunca arriscar sobrepor a cabeceira da cama. Direção
        // (cabeceira pro fundo ou pra entrada do quarto) varia por unidade —
        // nenhum apartamento vizinho decide igual.
        let cama_dir = if element_rng(id).random_bool(0.5) {
            Direcao::Sul
        } else {
            Direcao::Norte
        };
        let cama_z = if matches!(cama_dir, Direcao::Norte) {
            meio_z + 3
        } else {
            meio_z + 2
        };
        mobilia_cama(editor, min_x + 2, y, cama_z, offset, cama_dir);
        if prof_m > 6.5 {
            mobilia_guarda_roupa(editor, min_x + 2, y, max_z - 2, offset);
        }
    } else {
        let terco = largura_interna / 3;
        for i in 1..3 {
            let divisao = min_x + terco * i;
            for z in meio_z..(max_z - 1) {
                for h in y..forro_y {
                    let bloco = parede_organica(divisao, h + offset, z, id);
                    editor.set_block_absolute(bloco, divisao, h + offset, z, None, None);
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
                let bloco = parede_organica(x, h + offset, meio_z, id);
                editor.set_block_absolute(bloco, x, h + offset, meio_z, None, None);
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
                        let bloco = parede_organica(suite_x + dx, h + offset, max_z - dz - 1, id);
                        editor.set_block_absolute(
                            bloco,
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

        // 🚨 RECONEXÃO: unidade grande de 3 seções (sala + quarto + suíte) —
        // mobiliada de verdade pela primeira vez: cama e guarda-roupa na suíte
        // (cabem sempre — a suíte tem dimensão fixa, não escala com a unidade),
        // cama no quarto do meio, e sofá na sala só com folga extra de
        // profundidade comprovada (evita sobrepor o canto do banheiro de entrada).
        let (suite_cama_x, suite_cama_dir) = if element_rng(id.wrapping_add(3)).random_bool(0.5) {
            (suite_x + 1, Direcao::Leste)
        } else {
            (suite_x + 2, Direcao::Oeste)
        };
        mobilia_cama(editor, suite_cama_x, y, max_z - 3, offset, suite_cama_dir);
        mobilia_guarda_roupa(editor, suite_x + 2, y, max_z - 2, offset);
        mobilia_cama(
            editor,
            min_x + terco + 1,
            y,
            meio_z + 2,
            offset,
            Direcao::Leste,
        );
        if prof_m > 6.0 {
            let sofa_carpet = if element_rng(id).random_bool(0.5) {
                RED_CARPET
            } else {
                WHITE_CARPET
            };
            mobilia_sofa(editor, min_x + 1, y, meio_z - 1, offset, sofa_carpet);
        }
    }

    // 🚨 RECONEXÃO: banheiro no canto de entrada — presente em toda unidade real,
    // ausente de todas as faixas de largura até aqui. Canto (min_x+1..+2,
    // min_z+1..+2) é livre nas três faixas: a faixa 1 só constrói sua primeira
    // parede em min_x+3; as faixas 2 e 3 só mexem em min_x+3 em diante também.
    gerar_banheiro(
        editor,
        min_x + 1,
        y,
        min_z + 1,
        forro_y,
        offset,
        id.wrapping_add(7),
    );

    let kitchen_z = min_z + 1;
    let kitchen_x_start = max_x - 5;

    // 🚨 RECONEXÃO: o enclausuramento da cozinha (paredes de verdade, não só um
    // balcão flutuando no meio do cômodo) dependia do BAIRRO (só Guará/Samambaia)
    // em vez do TAMANHO da unidade — sem nenhuma razão arquitetônica real pra
    // isso. Agora qualquer unidade com profundidade suficiente ganha cozinha
    // fechada (com área de serviço anexa, item característico do apartamento
    // brasileiro que nunca existia aqui); unidades mais rasas mantêm o balcão
    // aberto simples, que ainda cabe.
    if prof_m > 7.0 {
        for dx in 0i32..=4i32 {
            for dz in 0i32..=4i32 {
                if kitchen_z + dz < suite_z_bound {
                    for h in y..forro_y {
                        let is_wall = dx == 0 || dz == 4;
                        if is_wall {
                            let bloco = parede_organica(
                                kitchen_x_start + dx,
                                h + offset,
                                kitchen_z + dz,
                                id,
                            );
                            editor.set_block_absolute(
                                bloco,
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

        // 🚨 RECONEXÃO: área de serviço — encostada na própria porta da cozinha,
        // do lado de fora da parede oeste (dx==0). Item quase universal no
        // apartamento brasileiro real, ausente do motor até aqui.
        gerar_area_servico(editor, kitchen_x_start - 1, y, kitchen_z + 2, offset);
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

    // 🚨 RECONEXÃO: varanda existia só para Guará/Samambaia — sem razão real,
    // já que sacada é um elemento praticamente universal em edifício residencial
    // do DF (inclusive nas superquadras, historicamente). Agora toda unidade tem.
    for x in (min_x + 1)..(max_x - 1) {
        editor.set_block_absolute(GLASS_PANE, x, y + offset, varanda_z, None, None);
        editor.set_block_absolute(AIR, x, y + 1 + offset, varanda_z, None, None);
        editor.set_block_absolute(AIR, x, y + 2 + offset, varanda_z, None, None);
    }

    // Escada interna privada — só existe de verdade quando esta unidade é o térreo de
    // um sobrado/casa de dois pavimentos (ver `tem_andar_acima` em
    // `generate_building_interior`). Uma unidade de apartamento comum, num prédio com
    // elevador ou escada COMUM, nunca teria uma escada furando o piso do vizinho de
    // cima — por isso a extração para `gerar_escada_interna` com o gatilho corrigido.
    if tem_andar_acima {
        gerar_escada_interna(
            editor, min_x, max_x, min_z, max_z, y, ceiling, offset, false,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_school_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    id: u64,
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
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
                }
            }
            for z in (center_z + 3)..(max_z - 1) {
                for h in y..ceiling {
                    let bloco = parede_organica(x, h + offset, z, id);
                    editor.set_block_absolute(bloco, x, h + offset, z, None, None);
                }
            }
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z - 3, None, None);
            editor.set_block_absolute(IRON_DOOR, x - 1, y + 1 + offset, center_z + 3, None, None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Escritório/ministério genérico (sem nome reconhecido — a grande maioria dos blocos
/// da Esplanada dos Ministérios e das agências bancárias comuns, que não têm um nome
/// que bata com `detect_edificio_especifico`). Antes desta reconexão, só o lado sul do
/// corredor recebia partição (o lado norte ficava um vão aberto sem nenhuma sala), e
/// nenhuma sala tinha mobília — um cubículo de vidro vazio. Reconectado com partição
/// simétrica dos dois lados do corredor e uma mesa de trabalho por sala.
fn generate_office_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    ceiling: i32,
    offset: i32,
    id: u64,
) {
    let modulo = ((4.0 * H_SCALE).round() as i32).max(2);
    let center_z = (min_z + max_z) / 2;

    for x in (min_x + 1)..(max_x - 1) {
        let rel = x - min_x;
        if rel % modulo == 0 {
            for &(z_start, z_end) in &[(min_z + 1, center_z - 1), (center_z + 2, max_z - 1)] {
                if z_start >= z_end {
                    continue;
                }
                for z in z_start..z_end {
                    for h in y..ceiling {
                        let block = if h == y + 2 {
                            GLASS_PANE
                        } else {
                            particao_organica(x, h + offset, z, id)
                        };
                        editor.set_block_absolute(block, x, h + offset, z, None, None);
                    }
                }
            }
        } else if rel % modulo == modulo / 2 {
            let mut rng = element_rng(id.wrapping_add(x as u64));
            let cadeira = if rng.random_bool(0.5) {
                OAK_STAIRS
            } else {
                STONE_BRICK_STAIRS
            };
            if center_z - 3 > min_z + 1 {
                editor.set_block_absolute(
                    SMOOTH_STONE_SLAB,
                    x,
                    y + 1 + offset,
                    center_z - 3,
                    None,
                    None,
                );
                editor.set_block_absolute(cadeira, x, y + offset, center_z - 3, None, None);
            }
            if center_z + 4 < max_z - 1 {
                editor.set_block_absolute(
                    SMOOTH_STONE_SLAB,
                    x,
                    y + 1 + offset,
                    center_z + 4,
                    None,
                    None,
                );
                editor.set_block_absolute(cadeira, x, y + offset, center_z + 4, None, None);
            }
        }
    }
}

/// Loja/comércio de rua genérico — a esmagadora maioria das lojas e pequenos
/// comércios das satélites, sem "Shopping" no nome (que ativaria
/// `generate_shopping_layout`). Antes desta reconexão, todo andar
/// `Tipologia::Comercial` recebia só o corredor vazio de `generate_corridor` — nem uma
/// prateleira, nem um balcão de caixa; a MESMA lacuna que existia em
/// `generate_residential_layout` antes de suas reconexões, só que nunca corrigida
/// aqui. Prateleiras ao longo de uma parede da faixa já vazada pelo corredor + um
/// balcão de caixa perto da entrada (`min_x`) — a composição mais reconhecível de loja
/// real, e a mesma faixa (`cz_start`) que `generate_corridor` já usa, então nunca
/// invade a parede externa.
#[allow(clippy::too_many_arguments)]
fn generate_loja_layout(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    y: i32,
    offset: i32,
    bairro: Bairro,
) {
    if max_x - min_x < 6 {
        return;
    }
    let center = (min_z + max_z) / 2;
    let cz_start = match bairro {
        Bairro::Comercial => center - 1,
        _ => center,
    };

    for x in (min_x + 4)..(max_x - 2) {
        if (x - min_x) % 2 == 0 {
            editor.set_block_absolute(BOOKSHELF, x, y + 1 + offset, cz_start, None, None);
        }
    }
    editor.set_block_absolute(
        SMOOTH_STONE_SLAB,
        min_x + 2,
        y + 1 + offset,
        cz_start,
        None,
        None,
    );
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
            element.id,
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
                    element.id,
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
                    element.id,
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
                            element.id,
                        );
                        generate_school_layout(
                            editor, min_x, max_x, min_z, max_z, floor_y, ceiling, offset,
                            element.id,
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
                            element.id,
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
                            element.id,
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
                            element.id,
                        );
                        generate_office_layout(
                            editor, min_x, max_x, min_z, max_z, floor_y, ceiling, offset,
                            element.id,
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
                        // 🚨 RECONEXÃO DE FIDELIDADE (Brasília/Satélites): as 6 combinações de
                        // bairro tinham CADA UMA sua própria regra ad-hoc — e na prática, SQS,
                        // SQN, Comercial e Águas Claras NUNCA chamavam
                        // `generate_residential_layout`, só corredor + núcleo de elevador. Ou
                        // seja: nenhuma superquadra do Plano Piloto (a tipologia residencial
                        // mais icônica de Brasília) e nenhuma torre de Águas Claras jamais
                        // recebia um apartamento de verdade — só um andar oco atrás das
                        // portas do corredor. Guará/Samambaia/Condomínio/Outro tinham a
                        // função certa, mas um teto artificial de altura (<=12 blocos, ~3
                        // andares) que descartava qualquer prédio residencial mais alto.
                        //
                        // Unificado numa regra única e arquitetonicamente real: o térreo é
                        // pilotis/hall (só o núcleo de elevador, sem apartamentos) — o traço
                        // definidor das superquadras de Lúcio Costa/Niemeyer (prédio erguido
                        // sobre pilares, térreo aberto), preservado do comportamento de SQS/
                        // SQN e agora estendido a todo bairro; do 1º andar em diante, todo
                        // andar residencial recebe layout completo mobiliado, sem teto de
                        // altura — uma torre de 20 andares em Águas Claras repete o mesmo
                        // apartamento 19 vezes, exatamente como no mundo real.
                        // Exceção de fidelidade: casas/sobrados não têm pilotis — o térreo
                        // É a sala/cozinha/quartos, não um hall vazio sobre pilares. Ver
                        // `eh_casa_terrea_ou_sobrado`. Só o térreo de blocos/torres de
                        // apartamento (SQN/SQS, Águas Claras, condomínios verticais) segue
                        // sendo tratado como pilotis/hall.
                        // Circulação vertical do térreo: núcleo de elevador quando o
                        // prédio tem (`has_core`); senão, se ainda é residencial com
                        // mais de um andar, uma escada COMUM real — antes desta
                        // reconexão, um bloco baixo sem elevador (maioria dos blocos
                        // antigos de SQN/SQS e prédios pequenos das satélites) não
                        // tinha absolutamente nenhuma forma de subir de andar.
                        if i == 0 && !eh_casa_terrea_ou_sobrado(element) {
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
                            } else if tipologia == Tipologia::Residencial && i + 1 < total_floors {
                                gerar_escada_interna(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    ceiling,
                                    offset,
                                    true,
                                );
                            }
                            // 🚨 RECONEXÃO: uso misto real — térreo de loja, andares de
                            // apartamento em cima. Ver `eh_uso_misto_comercio_terreo`.
                            // Sem isso, um prédio com `building=apartments`+`shop=*`
                            // (padrão de rua comercial de satélite: Taguatinga Centro,
                            // Ceilândia Centro, Sobradinho, Gama, W3 Sul/Norte) recebia
                            // pilotis vazio ou portaria de condomínio no térreo — nunca
                            // uma loja, mesmo com a tag de comércio bem ali no OSM.
                            if eh_uso_misto_comercio_terreo(element) {
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
                                    Tipologia::Comercial,
                                    element.id,
                                );
                                generate_loja_layout(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    offset,
                                    bairro,
                                );
                            } else if tipologia == Tipologia::Residencial {
                                gerar_terreo_condominio(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    offset,
                                    element.id,
                                );
                            }
                            continue;
                        }

                        if tipologia == Tipologia::Residencial {
                            // Só uma casa/sobrado (nunca uma unidade de apartamento em
                            // torre com elevador/escada comum) tem escada PRIVADA
                            // ligando este andar ao de cima — ver `gerar_escada_interna`.
                            let tem_andar_acima =
                                eh_casa_terrea_ou_sobrado(element) && i + 1 < total_floors;
                            generate_residential_layout(
                                editor,
                                min_x,
                                max_x,
                                min_z,
                                max_z,
                                floor_y,
                                ceiling,
                                offset,
                                element.id,
                                tem_andar_acima,
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
                                element.id,
                            );
                            if tipologia == Tipologia::Comercial {
                                generate_loja_layout(
                                    editor,
                                    min_x,
                                    max_x,
                                    min_z,
                                    max_z,
                                    floor_y + 1,
                                    offset,
                                    bairro,
                                );
                            }
                        }
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
                        } else if tipologia == Tipologia::Residencial
                            && !eh_casa_terrea_ou_sobrado(element)
                            && i + 1 < total_floors
                        {
                            gerar_escada_interna(
                                editor,
                                min_x,
                                max_x,
                                min_z,
                                max_z,
                                floor_y + 1,
                                ceiling,
                                offset,
                                true,
                            );
                        }
                    }
                }
            }
        }
    }
}
