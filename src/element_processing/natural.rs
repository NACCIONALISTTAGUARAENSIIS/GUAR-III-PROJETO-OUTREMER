use crate::args::Args;
use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::element_processing::tree::{Tree, TreeType}; // Atualizado para chamar o seu trees.rs governamental
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;

// 🚨 Importações Específicas do Cerrado (Corrigindo o Erro E0425)
use crate::element_processing::tree::SHORT_GRASS;

// 🚨 BESM-6: Trazendo os dados reais do MapBiomas/SICAR para a consciência
use crate::providers::vegetation_provider::{
    BIOME_CAMPO_LIMPO, BIOME_CAMPO_RUPESTRE, BIOME_CAMPO_SUJO, BIOME_CERRADAO, BIOME_CERRADO_SS,
    BIOME_MATA_GALERIA, BIOME_NONE, BIOME_VEREDA, MASK_APP_SICAR,
};

use rand::{prelude::IndexedRandom, Rng};
use std::collections::HashSet;
use std::sync::Arc;

// 🚨 BESM-6: Motor Biológico de Distribuição Espacial (Perlin Fake)
#[inline(always)]
fn organic_density_noise(x: i32, z: i32, scale: f64) -> f64 {
    let xf = x as f64 * scale;
    let zf = z as f64 * scale;
    ((xf.sin() * zf.cos()) + (xf * 0.5 + zf * 0.3).sin() * 0.5).abs() / 1.5
}

// 🚨 BESM-6: Topografia Biométrica (Gradiente 2D & Busca de Lençol Freático & Grade Real)
// O Cerrado possui declives e veios d'água que determinam a flora.
#[inline]
fn determine_cerrado_biome(x: i32, z: i32, ground_y: i32, editor: &WorldEditor) -> &'static str {
    // 0. A Verdade Absoluta: Consulta o MapBiomas/SICAR no motor de terreno
    // NOTA: Assumindo que seu editor.get_ground() expõe o método get_biome().
    // Se o seu ground usa outro nome para ler a matriz, ajuste a chamada abaixo.
    if let Some(ground) = editor.get_ground() {
        // 🚨 Correção: a máscara de APP (MASK_APP_SICAR) vem OR'd no bit alto do bioma.
        // Comparar o valor bruto contra as constantes BIOME_* nunca batia para área
        // protegida, e o motor caía sempre no fallback matemático abaixo mesmo quando
        // o dado real (Vereda/Mata de Galeria) já estava disponível.
        let real_biome = ground.get_biome(x, z) & !MASK_APP_SICAR;
        if real_biome != BIOME_NONE {
            if real_biome == BIOME_MATA_GALERIA {
                return "mata_galeria";
            }
            if real_biome == BIOME_CERRADAO {
                return "cerradao";
            }
            if real_biome == BIOME_CERRADO_SS {
                return "cerrado_ss";
            }
            if real_biome == BIOME_CAMPO_SUJO {
                return "campo_sujo";
            }
            if real_biome == BIOME_CAMPO_LIMPO {
                return "campo_limpo";
            }
            if real_biome == BIOME_VEREDA {
                return "vereda";
            }
            if real_biome == BIOME_CAMPO_RUPESTRE {
                return "campo_rupestre";
            }
        }
    }

    // 1. Fallback Matemático: Busca de água (Octogonal expandida para Veredas simuladas)
    // Usamos um raio maior para detectar corredores fluviais (Mata de Galeria)
    let water_radius = 8;
    let has_water =
        editor.check_for_block_absolute(x + water_radius, ground_y, z, Some(&[WATER]), None)
            || editor.check_for_block_absolute(x - water_radius, ground_y, z, Some(&[WATER]), None)
            || editor.check_for_block_absolute(x, ground_y, z + water_radius, Some(&[WATER]), None)
            || editor.check_for_block_absolute(x, ground_y, z - water_radius, Some(&[WATER]), None)
            || editor.check_for_block_absolute(x + 4, ground_y, z + 4, Some(&[WATER]), None)
            || editor.check_for_block_absolute(x - 4, ground_y, z - 4, Some(&[WATER]), None);

    if has_water {
        return "mata_galeria";
    }

    // 2. Calcula Slope 2D (Gradiente Topográfico) para detectar Campos Rupestres
    // O código legado olhava apenas Norte-Sul. Agora olhamos os eixos cardeais.
    let y_north = editor.get_ground_level(x, z - 4);
    let y_south = editor.get_ground_level(x, z + 4);
    let y_east = editor.get_ground_level(x + 4, z);
    let y_west = editor.get_ground_level(x - 4, z);

    let slope_z = (y_north - y_south).abs();
    let slope_x = (y_east - y_west).abs();

    // Gradiente resultante
    let total_slope = ((slope_x * slope_x + slope_z * slope_z) as f64).sqrt();

    // Se o morro cai mais de 3.5 blocos em 8 metros (Alta inclinação)
    if total_slope > 3.5 {
        "campo_rupestre" // Morros pedregosos e secos
    } else {
        "cerrado_ss" // Cerrado Stricto Sensu (Típico e Plano)
    }
}

/// Verifica se a coordenada está dentro de uma Área de Preservação Permanente (SICAR).
/// Usado para adensar sutilmente a vegetação em zonas legalmente protegidas, já que
/// hoje a máscara é calculada mas nunca se refletia visualmente no mundo gerado.
#[inline]
fn is_app_protected(x: i32, z: i32, editor: &WorldEditor) -> bool {
    editor
        .get_ground()
        .map(|ground| ground.get_biome(x, z) & MASK_APP_SICAR != 0)
        .unwrap_or(false)
}

#[inline]
fn termite_mound_block(rng: &mut impl Rng) -> Block {
    match rng.random_range(0..10) {
        0..=4 => RED_TERRACOTTA,
        5..=7 => BROWN_TERRACOTTA,
        8 => TERRACOTTA,
        _ => COARSE_DIRT,
    }
}

/// Cupinzeiro: elemento onipresente do Cerrado real, ausente do gerador até agora.
/// Montículo baixo e irregular de terra/argila avermelhada, mais estreito no topo.
fn generate_termite_mound(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    rng: &mut impl Rng,
) {
    let radius: i32 = rng.random_range(1..=2);
    let height: i32 = rng.random_range(2..=4);

    for h in 0..height {
        let layer_radius = (radius - (h / 2)).max(0);
        for dx in -layer_radius..=layer_radius {
            for dz in -layer_radius..=layer_radius {
                if dx * dx + dz * dz > layer_radius * layer_radius + 1 {
                    continue;
                }
                let block = termite_mound_block(rng);
                if h == 0 {
                    editor.set_block_absolute(
                        block,
                        x + dx,
                        ground_y,
                        z + dz,
                        Some(&[
                            GRASS_BLOCK,
                            DIRT,
                            PODZOL,
                            COARSE_DIRT,
                            SHORT_GRASS,
                            TALL_GRASS,
                            DEAD_BUSH,
                        ]),
                        None,
                    );
                } else {
                    editor.set_block_if_absent_absolute(block, x + dx, ground_y + h, z + dz);
                }
            }
        }
    }
}

/// Murundu: micro-relevo típico de campos úmidos e veredas — montículo raso de terra
/// coberto pela própria touceira de vegetação, formando os campos de murundus reais.
fn generate_murundu(editor: &mut WorldEditor, x: i32, ground_y: i32, z: i32, rng: &mut impl Rng) {
    let radius: i32 = rng.random_range(1..=3);
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx * dx + dz * dz > radius * radius {
                continue;
            }
            editor.set_block_absolute(
                COARSE_DIRT,
                x + dx,
                ground_y,
                z + dz,
                Some(&[GRASS_BLOCK, DIRT, MUD, PODZOL, SHORT_GRASS]),
                None,
            );
            if rng.random_bool(0.7) {
                editor.set_block_if_absent_absolute(SHORT_GRASS, x + dx, ground_y + 1, z + dz);
            }
        }
    }
    if rng.random_bool(0.5) {
        editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
    }
}

/// Canela-de-ema (Vellozia): roseta endêmica do Campo Rupestre, caule lenhoso curto
/// coroado por uma touceira densa — aproximação de baixo custo com blocos existentes.
fn generate_canela_de_ema(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    rng: &mut impl Rng,
) {
    let stem_h: i32 = rng.random_range(1..=2);
    for h in 1..=stem_h {
        editor.set_block_if_absent_absolute(GRAY_TERRACOTTA, x, ground_y + h, z);
    }
    editor.set_block_if_absent_absolute(FERN, x, ground_y + stem_h + 1, z);
}

pub fn generate_natural(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    exclusion_mask: Option<&HashSet<(i32, i32)>>, // 🚨 BESM-6: O Veto Booleano (Inners)
) {
    if let Some(natural_type) = element.tags().get("natural") {
        if natural_type == "tree" {
            if let ProcessedElement::Node(node) = element {
                let x: i32 = node.x;
                let z: i32 = node.z;

                // Bloqueia a árvore se ela caiu exatamente dentro de um lago interno
                if let Some(mask) = exclusion_mask {
                    if mask.contains(&(x, z)) {
                        return;
                    }
                }

                let mut trees_ok_to_generate: Vec<TreeType> = vec![];
                if let Some(species) = element.tags().get("species") {
                    let species_lower = species.to_lowercase();
                    if species_lower.contains("ipê") || species_lower.contains("handroanthus") {
                        trees_ok_to_generate.push(TreeType::IpeAmarelo);
                    }
                    if species_lower.contains("copaíba") || species_lower.contains("copaifera") {
                        trees_ok_to_generate.push(TreeType::Copaiba);
                    }
                    if species_lower.contains("pequi") || species_lower.contains("caryocar") {
                        trees_ok_to_generate.push(TreeType::Pequi);
                    }
                    if species_lower.contains("angico") {
                        trees_ok_to_generate.push(TreeType::Angico);
                    }
                    if species_lower.contains("jatob") {
                        trees_ok_to_generate.push(TreeType::Jatoba);
                    }
                    if species_lower.contains("aroeira") {
                        trees_ok_to_generate.push(TreeType::Aroeira);
                    }
                    if species_lower.contains("baru") {
                        trees_ok_to_generate.push(TreeType::Baru);
                    }
                    if species_lower.contains("gameleira") || species_lower.contains("ficus") {
                        trees_ok_to_generate.push(TreeType::Gameleira);
                    }
                    if species_lower.contains("cagaita") {
                        trees_ok_to_generate.push(TreeType::Cagaita);
                    }
                    if species_lower.contains("barbatimão") || species_lower.contains("barbatimao")
                    {
                        trees_ok_to_generate.push(TreeType::Barbatimao);
                    }
                } else {
                    // Expurgagem de Espécies Exóticas: sorteia entre nativas do Cerrado
                    trees_ok_to_generate.push(TreeType::Sucupira);
                    trees_ok_to_generate.push(TreeType::IpeAmarelo);
                    trees_ok_to_generate.push(TreeType::Pequi);
                    trees_ok_to_generate.push(TreeType::Angico);
                    trees_ok_to_generate.push(TreeType::Jatoba);
                }

                if trees_ok_to_generate.is_empty() {
                    trees_ok_to_generate.push(TreeType::Acacia);
                }

                let mut rng = element_rng(element.id());
                let tree_type = *trees_ok_to_generate
                    .choose(&mut rng)
                    .unwrap_or(&TreeType::Acacia);

                // BESM-6 Tweak: GROUND AWARE (Árvores não voam nem ficam soterradas)
                let ground_y = editor.get_ground_level(x, z);

                // Protege estradas e águas de receberem árvores se o OSM errar 1 metro
                if !editor.check_for_block_absolute(
                    x,
                    ground_y,
                    z,
                    Some(&[
                        BLACK_CONCRETE,
                        POLISHED_BASALT,
                        YELLOW_CONCRETE,
                        RED_CONCRETE,
                        WATER,
                        POLISHED_ANDESITE,
                    ]),
                    None,
                ) {
                    Tree::create_of_type(
                        editor,
                        (x, ground_y + 1, z),
                        tree_type,
                        Some(building_footprints),
                    );
                }
            }
        } else {
            // BESM-6 Tweak: Correção na Inconsistência de Blocos orgânicos.
            let block_type: Block = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" | "heath" | "tree_row" => GRASS_BLOCK,
                "sand" | "dune" | "beach" | "shoal" => SAND,
                "water" | "reef" => WATER,
                "bare_rock" | "ridge" | "cliff" => STONE,
                "blockfield" | "mountain_range" | "saddle" => ANDESITE, // Rocha bruta
                "glacier" => PACKED_ICE,
                "mud" | "wetland" => MUD,
                "shrubbery" | "tundra" | "hill" => GRASS_BLOCK,
                _ => GRASS_BLOCK,
            };

            let ProcessedElement::Way(way) = element else {
                return;
            };

            let filled_area: Vec<(i32, i32)> =
                flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
            if filled_area.is_empty() {
                return;
            }

            let trees_ok_to_generate: Vec<TreeType> = vec![
                TreeType::Sucupira,
                TreeType::Acacia,
                TreeType::Pequi,
                TreeType::Angico,
                TreeType::Baru,
                TreeType::Aroeira,
            ];
            let mut rng = element_rng(way.id);

            // 🚨 BESM-6: Aplica Densidade de Vegetação baseada na Escala Híbrida (Fixado erro sintaxe)
            let scale_area_multiplier = args.scale_h * args.scale_h;
            let base_tree_chance = (6.0 / scale_area_multiplier).clamp(2.0, 10.0) as u32;

            // 🚨 PREENCHIMENTO DA ÁREA INTERNA (CORE E BORDAS via Scanline)
            for &(x, z) in &filled_area {
                // 🚨 O Veto Booleano (O Algo do Pintor Morreu Aqui)
                // Se a coordenada caiu num lago (Inner), o motor aborta instantaneamente.
                if let Some(mask) = exclusion_mask {
                    if mask.contains(&(x, z)) {
                        continue;
                    }
                }

                let ground_y = editor.get_ground_level(x, z);

                // Culling Híbrido: A natureza não sobrepõe a cidade. Se há concreto sob os pés, recua.
                if editor.check_for_block_absolute(
                    x,
                    ground_y,
                    z,
                    Some(&[
                        BLACK_CONCRETE,
                        POLISHED_BASALT,
                        YELLOW_CONCRETE,
                        RED_CONCRETE,
                        WHITE_CONCRETE,
                        POLISHED_ANDESITE,
                    ]),
                    None,
                ) {
                    continue;
                }

                let biome_class = determine_cerrado_biome(x, z, ground_y, editor);

                // Pedras e terras áridas nos morros (Campo Rupestre): canga ferruginosa real,
                // não apenas cascalho/pedra genéricos — usa tons avermelhados de quartzito/laterita.
                let final_block =
                    if biome_class == "campo_rupestre" && rng.random_range(0..100) < 55 {
                        match rng.random_range(0..10) {
                            0..=2 => GRANITE,
                            3..=4 => SMOOTH_RED_SANDSTONE,
                            5..=7 => COARSE_DIRT,
                            _ => GRAVEL,
                        }
                    } else if block_type == ANDESITE && rng.random_range(0..100) < 30 {
                        if rng.random_bool(0.5) {
                            STONE
                        } else {
                            GRAVEL
                        }
                    } else {
                        block_type
                    };

                editor.set_block_absolute(
                    final_block,
                    x,
                    ground_y,
                    z,
                    Some(&[
                        GRASS_BLOCK,
                        DIRT,
                        PODZOL,
                        COARSE_DIRT,
                        STONE,
                        GRAVEL,
                        GRANITE,
                        SMOOTH_RED_SANDSTONE,
                    ]),
                    None,
                );

                // Pula decoração se for água pura ou gelo
                if block_type == WATER || block_type == PACKED_ICE {
                    continue;
                }

                // 🚨 Cupinzeiros: elemento onipresente e icônico do Cerrado real, ausente
                // do gerador até agora. Raríssimos, espalhados pelas fisionomias abertas/semi-abertas.
                if matches!(biome_class, "cerrado_ss" | "campo_sujo" | "cerradao")
                    && rng.random_range(0..10_000) < 6
                {
                    generate_termite_mound(editor, x, ground_y, z, &mut rng);
                    continue;
                }

                // 🚨 Murundus: micro-relevo caracterísitico de campos úmidos/veredas,
                // montículos de terra com sua própria touceira de vegetação.
                if (biome_class == "vereda" || natural_type == "wetland")
                    && rng.random_range(0..1_000) < 4
                {
                    generate_murundu(editor, x, ground_y, z, &mut rng);
                    continue;
                }

                // Zonas de APP (SICAR) recebem sub-bosque mais denso: hoje a máscara legal
                // era calculada mas nunca se manifestava visualmente no mundo gerado.
                let is_app = is_app_protected(x, z, editor);

                // Noise Macro-Biológico para evitar florestas xadrez
                let bio_noise = organic_density_noise(x, z, 0.1);

                match natural_type.as_str() {
                    "grassland" | "heath" => {
                        if biome_class == "campo_limpo" {
                            // 🚨 Campo Limpo real: só gramíneas, tapete quase uniforme,
                            // praticamente sem arbustos secos ou solo exposto.
                            if rng.random_range(0..100) < 75 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            } else if rng.random_range(0..100) < 5 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                            }
                        } else if biome_class == "campo_rupestre" {
                            // 🚨 Sempre-vivas: rosetas endêmicas que marcam o Campo Rupestre real
                            if rng.random_range(0..100) < 12 {
                                editor.set_block_if_absent_absolute(ALLIUM, x, ground_y + 1, z);
                            } else if rng.random_range(0..100) < 40 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                            } else if rng.random_range(0..100) < 60 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            }
                        } else {
                            if bio_noise > 0.7 {
                                editor.set_block_absolute(
                                    PODZOL,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[GRASS_BLOCK]),
                                    None,
                                );
                            }
                            if rng.random_range(0..100) < 40 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            } else if rng.random_range(0..100) < 55 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                                // Seca do Cerrado
                            }
                        }
                    }
                    "scrub" => {
                        if biome_class == "campo_limpo" {
                            // Campo Limpo não tem arbustos: se o OSM marcou "scrub" aqui,
                            // a grade real de vegetação tem prioridade.
                            if rng.random_range(0..100) < 70 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            }
                        } else if biome_class == "campo_rupestre" {
                            if rng.random_range(0..100) < 15 {
                                generate_canela_de_ema(editor, x, ground_y, z, &mut rng);
                            } else if rng.random_range(0..100) < 30 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                            }
                        } else if biome_class == "campo_sujo" {
                            // 🚨 Reage à grade real (Mato baixo misturado com terra exposta)
                            if rng.random_range(0..100) < 15 {
                                editor.set_block_absolute(
                                    COARSE_DIRT,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[GRASS_BLOCK]),
                                    None,
                                );
                            } else if rng.random_range(0..100) < 35 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                            } else if rng.random_range(0..100) < 65 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            }
                        } else {
                            if rng.random_range(0..100) < 8 {
                                editor.set_block_absolute(
                                    COARSE_DIRT,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[GRASS_BLOCK]),
                                    None,
                                );
                            } else if rng.random_range(0..100) < 25 {
                                editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                            } else if rng.random_range(0..100) < 40 {
                                editor.set_block_if_absent_absolute(
                                    ACACIA_LEAVES,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            } else if rng.random_range(0..100) < 70 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            }
                        }
                    }
                    "wood" | "tree_row" => {
                        // Zonas legalmente protegidas (APP) recebem dossel mais cerrado
                        let app_density_bonus: u32 = if is_app { 2 } else { 1 };

                        // Campo Limpo real não tem árvores: é a fisionomia mais aberta do Cerrado
                        if biome_class == "campo_limpo" {
                            if bio_noise < 0.2 {
                                editor.set_block_if_absent_absolute(
                                    SHORT_GRASS,
                                    x,
                                    ground_y + 1,
                                    z,
                                );
                            }
                        } else if biome_class == "mata_galeria" || biome_class == "vereda" {
                            if bio_noise > 0.2
                                && rng.random_range(0..100)
                                    < (base_tree_chance * 3 * app_density_bonus)
                            {
                                let tree_type = match rng.random_range(0..10) {
                                    0..=2 => TreeType::Buriti,
                                    3..=6 => TreeType::Copaiba,
                                    // Gameleira é rara e majestosa: reforça a mata de galeria real
                                    7 if biome_class == "mata_galeria" => TreeType::Gameleira,
                                    _ => TreeType::Copaiba,
                                };
                                Tree::create_of_type(
                                    editor,
                                    (x, ground_y + 1, z),
                                    tree_type,
                                    Some(building_footprints),
                                );
                            }
                        } else if biome_class == "cerradao" {
                            // 🚨 Reage à grade real (Mata de dossel mais denso e escuro)
                            if bio_noise > 0.3
                                && rng.random_range(0..100)
                                    < (base_tree_chance * 4 * app_density_bonus)
                            {
                                let tree_type = match rng.random_range(0..10) {
                                    0..=3 => TreeType::DarkOak,
                                    4..=6 => TreeType::Sucupira,
                                    _ => TreeType::Jatoba,
                                };
                                Tree::create_of_type(
                                    editor,
                                    (x, ground_y + 1, z),
                                    tree_type,
                                    Some(building_footprints),
                                );
                            }
                        } else {
                            if bio_noise > 0.4
                                && rng.random_range(0..100)
                                    < (base_tree_chance * 2 * app_density_bonus)
                            {
                                let tree_type = *trees_ok_to_generate
                                    .choose(&mut rng)
                                    .unwrap_or(&TreeType::Sucupira);
                                Tree::create_of_type(
                                    editor,
                                    (x, ground_y + 1, z),
                                    tree_type,
                                    Some(building_footprints),
                                );
                            } else if bio_noise < 0.2 {
                                editor.set_block_if_absent_absolute(TALL_GRASS, x, ground_y + 1, z);
                            }
                        }
                    }
                    "sand" | "shoal" => {
                        if rng.random_range(0..100) < 8 {
                            editor.set_block_if_absent_absolute(DEAD_BUSH, x, ground_y + 1, z);
                        }
                    }
                    "wetland" => {
                        let base_block = if rng.random_bool(0.4) {
                            MOSS_BLOCK
                        } else {
                            MUD
                        };
                        editor.set_block_absolute(
                            base_block,
                            x,
                            ground_y,
                            z,
                            Some(&[GRASS_BLOCK, MUD]),
                            None,
                        );
                        if rng.random_bool(0.3) {
                            editor.set_block_absolute(
                                WATER,
                                x,
                                ground_y,
                                z,
                                Some(&[MUD, MOSS_BLOCK]),
                                None,
                            );
                        } else if rng.random_bool(0.6) {
                            editor.set_block_if_absent_absolute(SHORT_GRASS, x, ground_y + 1, z);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn generate_natural_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if rel.tags.contains_key("natural") {
        // 🚨 BESM-6: Geometria de Exclusão (Inner Polygons)
        // Extrai a matemática dos buracos (Lagos, clareiras, pedreiras) antes de pintar o mato.
        let mut exclusion_mask: HashSet<(i32, i32)> = HashSet::new();

        for member in &rel.members {
            if member.role == ProcessedMemberRole::Inner {
                let inner_filled =
                    flood_fill_cache.get_or_compute(&member.way, args.timeout.as_ref());
                for coord in inner_filled {
                    exclusion_mask.insert(coord);
                }
            }
        }

        let exclusion_ref = if exclusion_mask.is_empty() {
            None
        } else {
            Some(&exclusion_mask)
        };

        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // 🚨 TWEAK: Usando Arc::new como requerido pela nossa nova assinatura do ProcessedElement
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_natural(
                    editor,
                    &ProcessedElement::Way(Arc::new(way_with_rel_tags)),
                    args,
                    flood_fill_cache,
                    building_footprints,
                    exclusion_ref, // Passa o Veto Booleano para o pintor do núcleo
                );
            }
        }
    }
}
