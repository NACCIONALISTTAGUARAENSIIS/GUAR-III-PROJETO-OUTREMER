use crate::args::Args;
use crate::block_definitions::{
    AIR, BEDROCK, BRICK, COARSE_DIRT, COPPER_BLOCK, CYAN_TERRACOTTA, DIRT, GRASS_BLOCK, GRAVEL,
    POLISHED_ANDESITE, RED_TERRACOTTA, SMOOTH_STONE, STONE, WATER,
};
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::ground::Ground;
use crate::master_control::BesmSignal; // 🚨 A Ponte com a Telemetria
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedWay};
use crate::progress::{emit_gui_progress_update, emit_open_mcworld_file};
use crate::urban_ground;
use crate::world_editor::{WorldEditor, WorldFormat};
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

pub const MIN_Y: i32 = -64;
// 🚨 BESM-6: Distorção Afim Governamental para Compensação Geométrica
pub const H_SCALE: f64 = 1.33;

/// Generation options that can be passed separately from CLI Args
#[derive(Clone)]
pub struct GenerationOptions {
    pub path: PathBuf,
    pub format: WorldFormat,
    pub level_name: Option<String>,
    pub spawn_point: Option<(i32, i32)>,
    // 🚨 BESM-6: Features governamentais diretas (CAESB, CityGML, IFC) + Advertising
    // (agnóstica de provedor — ver `is_provider_specific` em main.rs)
    pub provider_features: Vec<crate::providers::Feature>,
    // 🚨 Terreno definitivo do mundo inteiro (SRTM/LiDAR + DEM local + DSM +
    // bioma), montado UMA vez em `main.rs` via `Ground::assemble` e compartilhado
    // (Arc) por todas as regiões — substitui o antigo fatiamento por região, que
    // além de custoso populava a cache com chaves absolutas e a consultava com
    // chaves relativas (o mundo saía plano mesmo com `--terrain`). Ver `ground.rs`.
    pub ground: Arc<Ground>,
    // 🚨 RECONEXÃO: Liga a floresta ambiente procedural (tree::generate_chunk) no
    // Scanline. Calculado em main.rs a partir de `--terrain` e `--no-ambient-forest`.
    pub ambient_forest: bool,
    // 🚨 BESM-6: Canal de telemetria opcional para GUI/MasterControl
    pub telemetry_tx: Option<mpsc::Sender<BesmSignal>>,
}

// ============================================================================
// 🚨 INFRAESTRUTURA SUBTERRÂNEA (WFS) - GERAÇÃO 🚨
// ============================================================================

pub fn generate_underground_infrastructure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    _args: &Args,
) {
    let man_made = element.tags.get("man_made").map(|s: &String| s.as_str());
    let power = element.tags.get("power").map(|s: &String| s.as_str());

    if man_made != Some("pipeline") && power != Some("cable") && power != Some("line") {
        return;
    }

    // 🚨 Aceita tanto `width` (convenção OSM comum) quanto `diameter` (convenção
    // usada pelos dados WFS/CAESB), o que faltava aqui e existia só no pipeline órfão.
    let width_str = element
        .tags
        .get("width")
        .or_else(|| element.tags.get("diameter"))
        .map(|s: &String| s.as_str())
        .unwrap_or("1");

    // 🚨 BESM-6: Correção Matemática de Rigor (Distorção do Raio)
    let base_radius = (width_str.parse::<f64>().unwrap_or(1.0) / 2.0).max(1.0);

    // O eixo X/Z no Minecraft foi escalado por 1.33, logo o diâmetro horizontal da tubulação
    // deve ser espandido afim de manter a topologia circular do tubo no plano fatiado.
    let rx = (base_radius * H_SCALE).round() as i32;
    let ry = base_radius.round() as i32; // O eixo Y (profundidade) não sofre distorção horizontal

    let layer_val = element
        .tags
        .get("layer")
        .and_then(|s: &String| s.parse::<i32>().ok())
        .unwrap_or(-1);
    let depth_offset = layer_val * 6;

    let substance = element
        .tags
        .get("substance")
        .map(|s: &String| s.as_str())
        .unwrap_or("");
    // 🚨 Também aceita a tag `utility` (convenção CAESB/WFS: "sewer"/"water", em
    // português "esgoto"/"agua") — antes só existia no pipeline órfão de man_made.rs.
    let utility = element
        .tags
        .get("utility")
        .map(|s: &String| s.to_lowercase())
        .unwrap_or_default();
    let is_sewage =
        substance == "sewage" || utility.contains("sewer") || utility.contains("esgoto");
    let is_water = substance == "water" || utility.contains("water") || utility.contains("agua");
    let is_power = power == Some("cable") || power == Some("line");

    let (wall_block, fluid_block) = if is_sewage {
        (BRICK, Some(WATER))
    } else if is_power {
        (COPPER_BLOCK, None)
    } else if is_water {
        (CYAN_TERRACOTTA, Some(WATER))
    } else {
        (SMOOTH_STONE, None)
    };

    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let current_node = (node.x, node.z);

        if let Some(prev) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, 0, prev.1, current_node.0, 0, current_node.1);

            for (bx, _, bz) in bresenham_points {
                let local_ground = editor.get_ground_level(bx, bz);
                let pipe_center_y = local_ground + depth_offset;

                if pipe_center_y < MIN_Y + 5 {
                    continue;
                }

                // 🚨 CILINDRO ELÍPTICO: Preenchimento da Tubulação (Equação da Elipse)
                for wx in -rx..=rx {
                    for wy in -ry..=ry {
                        // Equação da Elipse: (x^2 / a^2) + (y^2 / b^2) <= 1
                        // Adicionamos 0.5 para suavizar a quantização dos blocos
                        let a = rx as f64 + 0.5;
                        let b = ry as f64 + 0.5;

                        let val =
                            (wx as f64 * wx as f64) / (a * a) + (wy as f64 * wy as f64) / (b * b);

                        if val <= 1.0 {
                            // Shell Thickness Check (Aproximação heurística de parede)
                            let inner_a = (rx - 1).max(0) as f64 + 0.5;
                            let inner_b = (ry - 1).max(0) as f64 + 0.5;
                            let inner_val = if inner_a > 0.5 && inner_b > 0.5 {
                                (wx as f64 * wx as f64) / (inner_a * inner_a)
                                    + (wy as f64 * wy as f64) / (inner_b * inner_b)
                            } else {
                                2.0 // Força ser parede se o tubo for muito pequeno
                            };

                            let is_shell = inner_val > 1.0;

                            let set_x = bx + wx;
                            let set_y = pipe_center_y + wy;
                            let set_z = bz;

                            if is_shell {
                                editor.set_block_absolute(
                                    wall_block,
                                    set_x,
                                    set_y,
                                    set_z,
                                    Some(&[DIRT, STONE, COARSE_DIRT, GRAVEL]),
                                    None,
                                );
                            } else {
                                let core_block = if wy <= -ry + (ry / 2).max(1) {
                                    fluid_block.unwrap_or(AIR) // Preenche água só na metade de baixo
                                } else {
                                    AIR
                                };
                                editor.set_block_absolute(
                                    core_block, set_x, set_y, set_z, None, None,
                                );
                            }
                        }
                    }
                }
            }
        }
        previous_node = Some(current_node);
    }
}

// ============================================================================
// 🚨 BESM-6 SCANLINE ENGINE (OUT-OF-CORE SPATIAL ROUTER) 🚨
// ============================================================================

/// Margem (blocos) além do nó mais a noroeste de um elemento que o desenho dele
/// pode alcançar: brush de via (até 26 de pista + 12 de faixa verde no Eixo
/// Monumental), copa de árvore, beiral de telhado, pilar de ponte. Entra no
/// cálculo da região-âncora para que NENHUM bloco do elemento caia numa região
/// já varrida (ver `anchor_region`).
const ELEMENT_REACH_MARGIN: i32 = 64;

/// Região que EXECUTA o desenho de um elemento no Scanline.
///
/// 🚨 CORREÇÃO DE QUALIDADE (defeito sistêmico): antes o elemento era executado
/// na região do seu CENTROIDE. Como a varredura é `rz` crescente e, dentro da
/// linha, `rx` crescente, tudo o que o elemento pintasse em regiões
/// "anteriores" (à esquerda ou acima do centroide) ia para o Halo de uma
/// região já selada em disco — e nunca mais era escrito. Na prática: ruas
/// cortadas ao meio na borda de região, metade de um prédio ausente, lagos com
/// um quadrante faltando. Pior: um elemento grande cujo centroide caísse FORA
/// do bbox (um lago ou rodovia parcialmente dentro do recorte) não era
/// executado em região nenhuma.
///
/// Agora a âncora é a região do canto MÍNIMO do AABB do elemento, recuado por
/// `ELEMENT_REACH_MARGIN` e recortado ao intervalo varrido. Toda coordenada que
/// o elemento possa tocar tem `z >= min_z - margem` (logo `rz >= âncora.rz`) e,
/// na mesma linha de regiões, `x >= min_x - margem` (logo `rx >= âncora.rx`):
/// todo vazamento é sempre "para frente", para uma região que AINDA vai ser
/// varrida e que replaya o Halo antes de selar. Elementos sem geometria caem
/// na primeira região (não pintam nada de qualquer forma).
fn anchor_region(
    element: &ProcessedElement,
    (min_rx, max_rx): (i32, i32),
    (min_rz, max_rz): (i32, i32),
) -> (i32, i32) {
    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut consider = |x: i32, z: i32| {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
    };
    match element {
        ProcessedElement::Node(n) => consider(n.x, n.z),
        ProcessedElement::Way(w) => w.nodes.iter().for_each(|n| consider(n.x, n.z)),
        ProcessedElement::Relation(r) => r
            .members
            .iter()
            .flat_map(|m| m.way.nodes.iter())
            .for_each(|n| consider(n.x, n.z)),
    }
    if min_x == i32::MAX {
        return (min_rx, min_rz);
    }
    anchor_region_for_min_corner(min_x, min_z, (min_rx, max_rx), (min_rz, max_rz))
}

/// Mesma regra de `anchor_region`, a partir do canto mínimo já conhecido
/// (usada também para `Feature`s de provedor, que carregam o AABB em cache).
fn anchor_region_for_min_corner(
    min_x: i32,
    min_z: i32,
    (min_rx, max_rx): (i32, i32),
    (min_rz, max_rz): (i32, i32),
) -> (i32, i32) {
    let rx = (min_x.saturating_sub(ELEMENT_REACH_MARGIN) >> 9).clamp(min_rx, max_rx);
    let rz = (min_z.saturating_sub(ELEMENT_REACH_MARGIN) >> 9).clamp(min_rz, max_rz);
    (rx, rz)
}

/// A Rota Individual de Geração (O "Bisturi" que o Orquestrador chama para cada forma)
#[allow(clippy::too_many_arguments)]
fn dispatch_element(
    element: ProcessedElement,
    editor: &mut WorldEditor,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
    flood_fill_cache: &mut FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    suppressed_building_outlines: &HashSet<u64>,
    xzbbox: &XZBBox,
    provenance: &mut crate::provenance::ProvenanceLedger,
) {
    // 🚨 BESM-6: Auditoria de proveniência — acumula, nesta chamada, TODO
    // módulo que efetivamente tratou o elemento (não só o primeiro branch que
    // bateu: a via de energia abaixo, por exemplo, passa tanto por
    // `power`/`man_made` QUANTO por `generate_underground_infrastructure` na
    // MESMA chamada — um `Vec`, não um `Option`, captura essa mistura real em
    // vez de escondê-la). Registrado no fim da função, via
    // `provenance.record_dispatch`, que só grava se a origem da feature
    // (provider/grupo semântico) foi registrada em `main.rs` antes da
    // conversão pra `ProcessedElement`.
    let mut dispatched_modules: Vec<&'static str> = Vec::new();

    match &element {
        ProcessedElement::Way(way) => {
            // 🚨 RECONEXÃO: `power=substation`/`power=plant` têm prioridade sobre
            // `building`, mesmo quando ambas as tags coexistem (padrão OSM comum:
            // subestações urbanas mapeadas como `building=yes` + `power=substation`
            // na MESMA way). Sem isto, o pátio de subestação nunca era desenhado —
            // caía sempre em `buildings::generate_buildings` como uma casa genérica.
            if matches!(
                way.tags.get("power").map(|s| s.as_str()),
                Some("substation") | Some("plant")
            ) {
                power::generate_power(editor, &element, args);
                dispatched_modules.push("power");
            } else if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                if !suppressed_building_outlines.contains(&way.id) {
                    buildings::generate_buildings(editor, way, args, None, None, flood_fill_cache);
                    dispatched_modules.push("buildings");
                }
            } else if way.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    &element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                    building_footprints,
                );
                dispatched_modules.push("highways");
            } else if way.tags.contains_key("landuse") {
                landuse::generate_landuse(editor, way, args, flood_fill_cache, building_footprints);
                dispatched_modules.push("landuse");
            } else if way.tags.contains_key("natural") {
                natural::generate_natural(
                    editor,
                    &element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    None,
                );
                dispatched_modules.push("natural");
            } else if way.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, &element, args, flood_fill_cache);
                dispatched_modules.push("amenities");
            } else if way.tags.contains_key("leisure") {
                leisure::generate_leisure(editor, way, args, flood_fill_cache, building_footprints);
                dispatched_modules.push("leisure");
            } else if way.tags.contains_key("barrier") {
                barriers::generate_barriers(editor, &element);
                dispatched_modules.push("barriers");
            } else if let Some(val) = way.tags.get("waterway") {
                if val == "dock" {
                    // water_areas::generate_water_area_from_way(editor, way, xzbbox);
                } else {
                    waterways::generate_waterways(editor, way);
                    dispatched_modules.push("waterways");
                }
            } else if way.tags.contains_key("railway") {
                railways::generate_railways(editor, way);
                dispatched_modules.push("railways");
            } else if way.tags.contains_key("roller_coaster") {
                railways::generate_roller_coaster(editor, way);
                dispatched_modules.push("railways::roller_coaster");
            } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway") {
                highways::generate_aeroway(editor, way, args);
                dispatched_modules.push("highways::aeroway");
            } else if way.tags.get("service") == Some(&"siding".to_string()) {
                highways::generate_siding(editor, way);
                dispatched_modules.push("highways::siding");
            } else if way.tags.get("tomb") == Some(&"pyramid".to_string()) {
                historic::generate_pyramid(editor, way, args, flood_fill_cache);
                dispatched_modules.push("historic::pyramid");
            } else if way.tags.contains_key("man_made")
                && way.tags.get("man_made") != Some(&"pipeline".to_string())
            {
                man_made::generate_man_made(editor, &element, args, flood_fill_cache);
                dispatched_modules.push("man_made");
            } else if way.tags.contains_key("power") {
                power::generate_power(editor, &element, args);
                dispatched_modules.push("power");
            } else if way.tags.contains_key("place") {
                landuse::generate_place(editor, way, args, flood_fill_cache);
                dispatched_modules.push("landuse::place");
            }

            // Infra Subterrânea WFS (Saneamento/Energia)
            if way.tags.contains_key("man_made") || way.tags.contains_key("power") {
                generate_underground_infrastructure(editor, way, args);
                dispatched_modules.push("underground_infrastructure");
            }
        }
        ProcessedElement::Node(node) => {
            // 🚨 BESM-6: `door`/`entrance` e `advertising` migraram para uma arquitetura
            // orientada a `Feature` (ver `doors::carve_and_place_door` e
            // `advertising::generate_advertising`). Um nó OSM solto não carrega o
            // contexto de parede/Ground que essas APIs agora exigem — portas são
            // talhadas pelo gerador de paredes (`buildings.rs`) e anúncios chegam
            // via `options.provider_features`, não por este dispatcher genérico.
            if node.tags.contains_key("natural")
                && node.tags.get("natural") == Some(&"tree".to_string())
            {
                natural::generate_natural(
                    editor,
                    &element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    None,
                );
                dispatched_modules.push("natural");
            } else if node.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, &element, args, flood_fill_cache);
                dispatched_modules.push("amenities");
            } else if node.tags.contains_key("barrier") {
                barriers::generate_barrier_nodes(editor, node);
                dispatched_modules.push("barriers");
            } else if node.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    &element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                    building_footprints,
                );
                dispatched_modules.push("highways");
            } else if node.tags.contains_key("tourism") {
                tourisms::generate_tourisms(editor, node);
                dispatched_modules.push("tourisms");
            } else if node.tags.contains_key("man_made") {
                man_made::generate_man_made_nodes(editor, node, args);
                dispatched_modules.push("man_made");
            } else if node.tags.contains_key("power") {
                power::generate_power_nodes(editor, node, args);
                dispatched_modules.push("power");
            } else if node.tags.contains_key("historic") {
                historic::generate_historic(editor, node);
                dispatched_modules.push("historic");
            } else if node.tags.contains_key("emergency") {
                emergency::generate_emergency(editor, node);
                dispatched_modules.push("emergency");
            }
            // 🚨 RECONEXÃO (Advertising): NÃO há branch `advertising` aqui de propósito.
            // Havia uma chamada `advertising::generate_advertising(editor, node)` neste
            // ponto, mas com assinatura incompatível com a função real (que exige
            // `&Feature` + `&Args` + `&Ground`, não um `&ProcessedNode` cru) — nunca
            // compilava corretamente com o contrato atual do módulo. Como
            // `SemanticGroup::Advertising` agora é roteado direto (ver `is_provider_specific`
            // em `main.rs` e o branch de `provider_features` acima em
            // `generate_world_with_options`), nenhuma feature de propaganda chega mais
            // como `ProcessedElement` genérico neste dispatcher — a Feature original
            // (com geometria e atributos intactos) é entregue a
            // `advertising::generate_advertising` antes de ser convertida.
        }
        ProcessedElement::Relation(rel) => {
            let is_building_relation = rel.tags.contains_key("building")
                || rel.tags.contains_key("building:part")
                || rel.tags.get("type").map(|t: &String| t.as_str()) == Some("building");
            if is_building_relation {
                buildings::generate_building_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    xzbbox,
                );
                dispatched_modules.push("buildings::from_relation");
            } else if rel.tags.contains_key("water")
                || rel
                    .tags
                    .get("natural")
                    .map(|val| val == "water" || val == "bay")
                    .unwrap_or(false)
            {
                water_areas::generate_water_areas_from_relation(editor, rel, xzbbox);
                dispatched_modules.push("water_areas");
            } else if rel.tags.contains_key("natural") {
                natural::generate_natural_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
                dispatched_modules.push("natural::from_relation");
            } else if rel.tags.contains_key("landuse") {
                landuse::generate_landuse_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
                dispatched_modules.push("landuse::from_relation");
            } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                leisure::generate_leisure_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
                dispatched_modules.push("leisure::from_relation");
            }
        }
    }

    provenance.record_dispatch(
        element.id(),
        dispatched_modules.into_iter().map(String::from).collect(),
    );
}

/// 🚨 RECONEXÃO: Gera UMA região (.mca) isolada a partir de uma fatia de `Feature`
/// já pré-filtrada por bbox — usada pelo Master Control HUD (`master_control.rs`,
/// modo Tile Streaming interativo sem GUI) para baixar e desenhar região por região
/// sob demanda, em vez do Scanline global de `generate_world_with_options` abaixo.
/// `master_control.rs::dispatch_generation` chamava esta função, mas ela nunca havia
/// sido escrita — não compilava.
///
/// Deliberadamente reduzida frente ao Scanline principal: o HUD busca cada região
/// avulsa sob demanda, sem o pré-processamento único de `ElevationData`/bioma real
/// que `main.rs` faz para o bbox inteiro antes do Scanline — então aqui é sempre chão
/// plano (`Ground::new_flat`) e sem floresta ambiente. Aceitável para o preview
/// interativo do HUD; uma exportação final continua passando por
/// `generate_world_with_options`.
///
/// Só é chamada quando a feature `gui` está desligada (`master_control.rs` é o único
/// chamador, e ele só entra em cena via `#[cfg(not(feature = "gui"))]` em `main.rs`) —
/// daí o `allow(dead_code)` condicional, para não acusar código morto num build com
/// GUI que nunca deveria mesmo chamar esta função.
#[cfg_attr(feature = "gui", allow(dead_code))]
pub fn generate_region_from_global(
    editor: &mut WorldEditor,
    features: &[crate::providers::Feature],
    args: &Args,
    // Não usado: `features` e `editor` já estão em espaço Minecraft (XZ) — a
    // transformação geográfica já aconteceu em `master_control.rs` antes de chamar
    // esta função. Mantido na assinatura só para não forçar o chamador a descartá-lo.
    _transformer: &crate::coordinate_system::transformation::CoordTransformer,
) {
    let (min_x, min_z) = editor.get_min_coords();
    let (max_x, max_z) = editor.get_max_coords();
    let xzbbox = XZBBox::new(min_x, max_x, min_z, max_z);

    // 🚨 BESM-6: Ledger de proveniência DESCARTADO de propósito — este é o
    // modo HUD interativo (streaming região-por-região, só sem a feature
    // `gui`), chamado uma vez por região independente, sem um ponto final
    // único onde escrever um relatório coerente para o mundo inteiro. Ver o
    // comentário de módulo em `provenance.rs` ("Cobertura").
    let mut discarded_provenance = crate::provenance::ProvenanceLedger::new();

    // Mesma bifurcação de `main.rs::run_generation_pipeline` (`is_provider_specific`):
    // infra CAESB (Sanitation/Sewage/Utility/Power/Telecom/Indoor governamental),
    // Advertising e voxels de fotogrametria (`MeshProvider`) vão direto pro motor
    // especializado; o resto vira `ProcessedElement` pro dispatcher genérico.
    let mut provider_specific_features: Vec<&crate::providers::Feature> = Vec::new();
    let mut osm_elements: Vec<ProcessedElement> = Vec::new();

    for feature in features {
        let is_provider_specific = (matches!(
            feature.semantic_group,
            crate::providers::SemanticGroup::Sanitation
                | crate::providers::SemanticGroup::Sewage
                | crate::providers::SemanticGroup::Utility
                | crate::providers::SemanticGroup::Power
                | crate::providers::SemanticGroup::Telecom
                | crate::providers::SemanticGroup::Indoor
        ) && (feature.source.contains("CAESB")
            || feature.source.contains("CityGML")
            || feature.source.contains("IFC")
            || feature.source.contains("Indoor")))
            || feature.semantic_group == crate::providers::SemanticGroup::Advertising
            || (feature.semantic_group == crate::providers::SemanticGroup::TerrainDetail
                && feature.source.contains("Photogrammetry_Mesh"));

        if is_provider_specific {
            provider_specific_features.push(feature);
        } else {
            osm_elements.push(feature.clone().into_processed_element());
        }
    }

    let highway_connectivity = highways::build_highway_connectivity_map(&osm_elements);
    let mut flood_fill_cache = FloodFillCache::new();
    let building_footprints = flood_fill_cache.collect_building_footprints(&osm_elements, &xzbbox);

    let suppressed_building_outlines: HashSet<u64> = {
        let mut outlines = HashSet::new();
        for element in &osm_elements {
            if let ProcessedElement::Relation(rel) = element {
                let is_building_type =
                    rel.tags.get("type").map(|t: &String| t.as_str()) == Some("building");
                if is_building_type
                    && rel
                        .members
                        .iter()
                        .any(|m| m.role == ProcessedMemberRole::Part)
                {
                    for member in &rel.members {
                        if member.role == ProcessedMemberRole::Outer {
                            outlines.insert(member.way.id);
                        }
                    }
                }
            }
        }
        outlines
    };

    // Ver o comentário sobre `Arc<Ground>` independente em `generate_world_with_options`
    // logo abaixo: precisamos de `&Ground` E `&mut editor` na mesma chamada de
    // `advertising::generate_advertising`, e pegar o Ground emprestado do próprio
    // `editor` colidiria com esse empréstimo mutável.
    let ground_arc = Arc::new(Ground::new_flat(args.ground_level));
    editor.set_ground(ground_arc.clone());

    for element in osm_elements {
        dispatch_element(
            element,
            editor,
            args,
            &highway_connectivity,
            &mut flood_fill_cache,
            &building_footprints,
            &suppressed_building_outlines,
            &xzbbox,
            &mut discarded_provenance,
        );
    }

    for feature in provider_specific_features {
        // `generate_from_provider_feature` (man_made.rs) já sabe rotear internamente
        // por `semantic_group` (Sanitation/Utility/Sewage/Indoor/Power/Telecom/
        // TerrainDetail); só Advertising precisa do `&Ground` extra que só esta
        // função tem à mão.
        let is_specialized_engine_feature = matches!(
            feature.semantic_group,
            crate::providers::SemanticGroup::Sanitation
                | crate::providers::SemanticGroup::Utility
                | crate::providers::SemanticGroup::Sewage
                | crate::providers::SemanticGroup::Indoor
                | crate::providers::SemanticGroup::Power
                | crate::providers::SemanticGroup::Telecom
                | crate::providers::SemanticGroup::TerrainDetail
        );

        if is_specialized_engine_feature {
            man_made::generate_from_provider_feature(editor, feature, args);
        } else if feature.semantic_group == crate::providers::SemanticGroup::Advertising {
            advertising::generate_advertising(editor, feature, args, &ground_arc);
        }
    }
}

pub fn generate_world_with_options(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    args: &Args,
    options: GenerationOptions,
    provenance: &mut crate::provenance::ProvenanceLedger,
) -> Result<PathBuf, String> {
    let output_path = options.path.clone();
    let world_format = options.format;

    let mut editor: WorldEditor = WorldEditor::new_with_format_and_name(
        options.path,
        &xzbbox,
        llbbox,
        options.format,
        options.level_name.clone(),
        options.spawn_point,
    );

    println!("{} Building Global Constraints...", "[4/7]".bold());

    let highway_connectivity = highways::build_highway_connectivity_map(&elements);
    let mut flood_fill_cache = FloodFillCache::new();

    let building_footprints = flood_fill_cache.collect_building_footprints(&elements, &xzbbox);

    // Áreas já descritas por algum dado (uso do solo, lazer, amenidades,
    // natureza, água, prédios): a floresta ambiente do Cerrado só nasce fora
    // delas. Só custa algo quando a floresta está ligada.
    let mapped_areas = if options.ambient_forest {
        Some(flood_fill_cache.collect_mapped_area_coverage(&elements, &xzbbox))
    } else {
        None
    };

    let building_centroids = if args.city_boundaries {
        flood_fill_cache.collect_building_centroids(&elements)
    } else {
        Vec::new()
    };

    let urban_lookup = if args.city_boundaries && !building_centroids.is_empty() {
        urban_ground::compute_urban_ground_lookup(building_centroids, &xzbbox)
    } else {
        urban_ground::UrbanGroundLookup::empty()
    };
    let has_urban_ground = !urban_lookup.is_empty();

    let suppressed_building_outlines: HashSet<u64> = {
        let mut outlines = HashSet::new();
        for element in &elements {
            if let ProcessedElement::Relation(rel) = element {
                let is_building_type =
                    rel.tags.get("type").map(|t: &String| t.as_str()) == Some("building");
                if is_building_type
                    && rel
                        .members
                        .iter()
                        .any(|m| m.role == ProcessedMemberRole::Part)
                {
                    for member in &rel.members {
                        if member.role == ProcessedMemberRole::Outer {
                            outlines.insert(member.way.id);
                        }
                    }
                }
            }
        }
        outlines
    };

    // Delimitação da Matriz Global Scanline (Regiões do Minecraft: 512x512 blocos)
    let min_rx = xzbbox.min_x() >> 9;
    let max_rx = xzbbox.max_x() >> 9;
    let min_rz = xzbbox.min_z() >> 9;
    let max_rz = xzbbox.max_z() >> 9;
    let rx_range = (min_rx, max_rx);
    let rz_range = (min_rz, max_rz);

    // 🚨 BESM-6: Indexação Espacial — cada elemento é executado na sua
    // região-âncora (canto mínimo com margem, ver `anchor_region`), nunca na do
    // centroide, para que nenhum bloco caia numa região já selada.
    println!("{} Spatially Indexing Vectors...", "[5/7]".bold());
    let mut spatial_index: HashMap<(i32, i32), Vec<ProcessedElement>> = HashMap::new();

    for element in elements.into_iter() {
        let key = anchor_region(&element, rx_range, rz_range);
        spatial_index.entry(key).or_default().push(element);
    }

    // 🚨 RECONEXÃO: `osm_parser::get_priority` já existia (prioriza building >
    // highway > waterway > water > barrier, ver `PRIORITY_ORDER`) mas nada
    // ordenava os elementos por ela antes do dispatch — cada região era
    // processada na ordem arbitrária de inserção do parser. Ordenar aqui
    // garante que, por região, prédios sejam desenhados antes de vias/rios
    // que dependam deles (ex.: recuo de calçada), de forma determinística.
    for elements_in_region in spatial_index.values_mut() {
        elements_in_region.sort_by_key(crate::osm_parser::get_priority);
    }

    let total_regions = ((max_rx - min_rx + 1) * (max_rz - min_rz + 1)) as usize;
    let mut processed_regions = 0;

    editor.set_ground(Arc::clone(&options.ground));

    // 🚨 O MOTOR DE VARREDURA (SCANLINE) 🚨
    for rz in min_rz..=max_rz {
        for rx in min_rx..=max_rx {
            // Sinaliza a GUI (Barra de progresso clássica)
            let p = (processed_regions as f64 / total_regions as f64) * 100.0;
            emit_gui_progress_update(p, &format!("Sweeping Region r.{}.{}", rx, rz));

            // Informa ao editor qual cache ele deve ativar (O Core Router)
            editor.set_active_region(rx, rz);

            // 1. TOPOGRAFIA: o mesmo `Ground` global serve todas as regiões (ver
            // `GenerationOptions::ground`); `region_ground` é só um segundo Arc
            // para `advertising::generate_advertising`, que precisa de `&Ground` e
            // `&mut WorldEditor` ao mesmo tempo.
            let region_ground = Arc::clone(&options.ground);

            // 2. GERAÇÃO FÍSICA DO CHÃO NA REGIÃO
            let chunk_min_x = rx * 32;
            let chunk_max_x = (rx * 32) + 31;
            let chunk_min_z = rz * 32;
            let chunk_max_z = (rz * 32) + 31;

            for cx in chunk_min_x..=chunk_max_x {
                for cz in chunk_min_z..=chunk_max_z {
                    let min_x = (cx << 4).max(xzbbox.min_x());
                    let max_x = ((cx << 4) + 15).min(xzbbox.max_x());
                    let min_z = (cz << 4).max(xzbbox.min_z());
                    let max_z = ((cz << 4) + 15).min(xzbbox.max_z());

                    for x in min_x..=max_x {
                        for z in min_z..=max_z {
                            let ground_y = editor.get_ground_level(x, z);

                            let is_urban = has_urban_ground && urban_lookup.is_urban(x, z);

                            if !editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[STONE]),
                                None,
                            ) {
                                if is_urban {
                                    editor.set_block_if_absent_absolute(
                                        POLISHED_ANDESITE,
                                        x,
                                        ground_y,
                                        z,
                                    );
                                } else {
                                    editor.set_block_if_absent_absolute(
                                        GRASS_BLOCK,
                                        x,
                                        ground_y,
                                        z,
                                    );
                                }
                                editor.set_block_if_absent_absolute(
                                    COARSE_DIRT,
                                    x,
                                    ground_y - 1,
                                    z,
                                );
                                editor.set_block_if_absent_absolute(
                                    RED_TERRACOTTA,
                                    x,
                                    ground_y - 2,
                                    z,
                                );
                            }

                            if args.fillground {
                                editor.fill_column_absolute(
                                    STONE,
                                    x,
                                    z,
                                    MIN_Y + 1,
                                    ground_y - 3,
                                    true,
                                );
                            }
                            editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));
                        }
                    }
                }
            }

            // 3. INJEÇÃO DO HALO CACHE (Construções vizinhas que vazaram pra cá)
            editor.load_halo_to_core();

            // 4. PROCESSAMENTO VETORIAL (Prédios, Rios, Estradas desta BBox estrita)
            if let Some(region_elements) = spatial_index.remove(&(rx, rz)) {
                for element in region_elements {
                    dispatch_element(
                        element,
                        &mut editor,
                        args,
                        &highway_connectivity,
                        &mut flood_fill_cache,
                        &building_footprints,
                        &suppressed_building_outlines,
                        &xzbbox,
                        provenance,
                    );
                }
            }

            // 🚨 BESM-6: PROCESSAMENTO DE FEATURES GOVERNAMENTAIS (CAESB, CityGML, IFC)
            // Infraestrutura que requer preservação de metadados (diâmetro, profundidade, material)
            for feature in &options.provider_features {
                // 🚨 Executada UMA vez, na região-âncora do seu AABB (mesma regra
                // dos elementos OSM). Antes era executada em TODA região que o
                // AABB intersectasse: um duto CAESB de 2 km era desenhado 4-5
                // vezes (uma por região), cada passada re-pintando e re-vazando
                // para as vizinhas, com proveniência registrada em dobro.
                let (feat_min_x, _, feat_min_z, _) = feature.aabb;
                let anchored_here =
                    anchor_region_for_min_corner(feat_min_x, feat_min_z, rx_range, rz_range)
                        == (rx, rz);

                if anchored_here {
                    // 🚨 RECONEXÃO DO PIPELINE CAESB (água/esgoto/indoor subterrâneo)
                    //
                    // Antes desta correção, TODA feature de provedor — incluindo as de
                    // saneamento do IndoorUtilityProvider (CAESB) — passava por
                    // `into_processed_element()` e caía no dispatcher genérico de OSM
                    // abaixo. Isso funcionava, mas descartava metadados finos que só a
                    // Feature original carrega (a tag `utility` distinguindo água/esgoto,
                    // o `diameter` preciso, o `semantic_group`), e o motor especializado
                    // de `man_made::generate_from_provider_feature` — que desenha a seção
                    // transversal oca com líquido parcial, câmaras de inspeção com poço de
                    // acesso e desgaste orgânico determinístico — nunca era chamado.
                    //
                    // Por isso os grupos semânticos de infraestrutura têm prioridade aqui:
                    // se a feature pertence a Sanitation/Utility/Sewage/Indoor/Power/Telecom
                    // (os grupos que o IndoorUtilityProvider produz para a rede da CAESB),
                    // ela é desenhada pelo motor especializado e NUNCA passa pelo
                    // dispatcher genérico — evita processamento duplicado do mesmo elemento.
                    //
                    // 🚨 REVISÃO: Power/Telecom antes ficavam de fora daqui por engano —
                    // a suposição era "não são CAESB, são CEB/telecom solta". Mas o filtro
                    // em `main.rs` (`is_provider_specific`) só deixa uma feature chegar em
                    // `provider_features` se o `source` contiver "CAESB"/"CityGML"/"IFC"/
                    // "Indoor" — ou seja, todo Power/Telecom que passa por aqui já é dado
                    // de infraestrutura governamental do mesmo pacote, não CEB solta. O
                    // motor especializado (`generate_underground_cable`) desenha um cabo
                    // fino de 1 bloco com cor distinta por tipo (laranja/energia, azul/
                    // telecom), mais fiel que o duto genérico de `generate_underground_infrastructure`.
                    let is_caesb_infrastructure_feature = matches!(
                        feature.semantic_group,
                        crate::providers::SemanticGroup::Sanitation
                            | crate::providers::SemanticGroup::Utility
                            | crate::providers::SemanticGroup::Sewage
                            | crate::providers::SemanticGroup::Indoor
                            | crate::providers::SemanticGroup::Power
                            | crate::providers::SemanticGroup::Telecom
                    );

                    if is_caesb_infrastructure_feature {
                        man_made::generate_from_provider_feature(&mut editor, feature, args);
                        provenance
                            .record_direct(feature, "man_made::generate_from_provider_feature");
                        continue;
                    }

                    // 🚨 BESM-6 RECONEXÃO: mesma lógica acima, mas para voxels de
                    // fotogrametria (`MeshProvider`, `--local-mesh`). Chegam aqui como
                    // `SemanticGroup::TerrainDetail` com as tags `color`/`elevation`/
                    // `material=photogrammetry` — nenhuma reconhecida pelo dispatcher
                    // genérico de `ProcessedElement::Node` logo abaixo, que descartaria
                    // o voxel silenciosamente mesmo após `MeshProvider` já tê-lo
                    // decimado e posicionado corretamente.
                    if feature.semantic_group == crate::providers::SemanticGroup::TerrainDetail
                        && feature.source.contains("Photogrammetry_Mesh")
                    {
                        man_made::generate_from_provider_feature(&mut editor, feature, args);
                        provenance
                            .record_direct(feature, "man_made::generate_from_provider_feature");
                        continue;
                    }

                    // 🚨 RECONEXÃO (Advertising): totens, outdoors e painéis MUB.
                    //
                    // Antes desta correção, `SemanticGroup::Advertising` nunca era
                    // atribuído por nenhum provedor (`OSMProvider::determine_semantic_group`
                    // caía sempre no fallback `Other`), e mesmo que fosse, o filtro
                    // `is_provider_specific` de `main.rs` só deixava passar Sanitation/
                    // Utility/Sewage/Indoor/Power/Telecom vindos de CAESB/CityGML/IFC —
                    // então todo elemento `advertising=*` ia para `into_processed_element()`
                    // e caía no dispatcher genérico de `ProcessedElement` (`dispatch_element`
                    // abaixo), que chamava `advertising::generate_advertising(editor, node)`
                    // com a assinatura ERRADA (a função exige `&Feature` + `&Args` + `&Ground`
                    // para a extração PCA da geometria e o cálculo de nível do chão, não um
                    // `&ProcessedNode` cru) — ou seja, nenhum outdoor/totem real do DF jamais
                    // era desenhado, apesar do dado já vir baixado da Overpass
                    // (`retrieve_data.rs` já pede `nwr["advertising"]`).
                    //
                    // Com `SemanticGroup::Advertising` agora atribuído e incluído em
                    // `is_provider_specific`, a Feature chega aqui intacta (geometria +
                    // atributos brutos: `angle`/`direction`, `material`, `layer`, `level`)
                    // e é desenhada pelo motor especializado direto, sem passar pelo
                    // dispatcher genérico — o `region_ground` separado acima fornece o
                    // `&Ground` que essa função exige simultaneamente ao `&mut editor`.
                    if feature.semantic_group == crate::providers::SemanticGroup::Advertising {
                        advertising::generate_advertising(
                            &mut editor,
                            feature,
                            args,
                            &region_ground,
                        );
                        provenance.record_direct(feature, "advertising::generate_advertising");
                        continue;
                    }

                    // 🚨 TWEAK: Roteador Semântico de Features de Alta Precisão
                    // Re-registra a origem por segurança (idempotente — já foi
                    // registrada em `main.rs` pra todo `provider_specific_features`,
                    // mas outros chamadores de `generate_world_with_options` podem
                    // não ter feito isso) antes de `into_processed_element()`
                    // descartá-la.
                    provenance.register_origin(feature);
                    let processed_element = feature.clone().into_processed_element();

                    // Delega para os construtores baseados nas tags traduzidas do Shapefile/WFS
                    dispatch_element(
                        processed_element,
                        &mut editor,
                        args,
                        &highway_connectivity,
                        &mut flood_fill_cache,
                        &building_footprints,
                        &suppressed_building_outlines,
                        &xzbbox,
                        provenance,
                    );
                }
            }

            // 4.5 FLORESTA AMBIENTE (RECONEXÃO): `tree::generate_chunk` existia pronta
            // — ruído de Perlin, variação de espécie do Cerrado, troncos caídos,
            // sub-bosque — mas nenhum lugar do motor a chamava; só havia árvore onde
            // o OSM/GDF marcava `natural=tree/wood` explicitamente. Roda por chunk
            // (16×16, mesma grade de `chunk_min_x..chunk_max_x` usada na geração do
            // chão acima) DEPOIS de prédios/vias desta região já estarem desenhados,
            // para que o bloqueio de superfície urbana/viária em tree.rs (ver os
            // comentários de `POLISHED_ANDESITE`/`GRAY_CONCRETE`/`GRAY_TERRACOTTA` em
            // `element_processing/tree.rs`) veja o chão real e não brote árvore em
            // cima de rua recém-pavimentada. `--no-ambient-forest` desliga.
            if options.ambient_forest {
                for cx in chunk_min_x..=chunk_max_x {
                    for cz in chunk_min_z..=chunk_max_z {
                        tree::generate_chunk(
                            cx,
                            cz,
                            Some(&building_footprints),
                            mapped_areas.as_ref(),
                            &mut editor,
                        );
                    }
                }
            }

            // 5. FLUSH DIRETO PARA O DISCO O(1) E ANIQUILAÇÃO DA RAM
            editor.flush_active_region();
            flood_fill_cache.clear_cache();

            // 6. TELEMETRIA (Comunica ao HUD Master Control que o quadrante foi selado)
            if let Some(tx) = &options.telemetry_tx {
                // Se o HUD estiver ligado, ele receberá a mensagem instantaneamente sem que o motor precise esperar
                let _ = tx.send(BesmSignal::RegionSealed(rx, rz, 14_750_000));
            }

            processed_regions += 1;
        }
    }

    // Com o roteamento por região-âncora, nenhuma operação pode sobrar no Halo
    // (toda região que recebeu vazamentos foi varrida depois deles). Se sobrar,
    // é regressão: blocos perdidos na borda de região — avisa em vez de calar.
    let pending_halo = editor.pending_halo_ops();
    if pending_halo > 0 {
        eprintln!(
            "{} {} operações do Halo nunca foram aplicadas (blocos perdidos em bordas de região) — regressão em `anchor_region`?",
            "Aviso:".yellow().bold(),
            pending_halo
        );
    }

    // Salva Metadados Finais
    editor.save();

    emit_gui_progress_update(99.0, "Finalizing world...");

    if world_format == WorldFormat::BedrockMcWorld {
        if let Some(path_str) = output_path.to_str() {
            emit_open_mcworld_file(path_str);
        }
    }

    // Sinaliza ao HUD que a obra acabou
    if let Some(tx) = &options.telemetry_tx {
        let _ = tx.send(BesmSignal::GenerationComplete);
    }

    // 🚨 BESM-6: Escreve o relatório de auditoria de proveniência
    // (`provenance.ndjson` + `provenance_summary.json`) ao lado de
    // `metadata.json`. Só pra Java Anvil por ora — `output_path` do Bedrock
    // é o `.mcworld` já empacotado (zip), não uma pasta onde dá pra escrever
    // arquivos soltos; ver o comentário de módulo em `provenance.rs`.
    if world_format == WorldFormat::JavaAnvil {
        if let Err(e) = provenance.write_reports(&output_path) {
            eprintln!("Aviso: falha ao escrever o relatório de proveniência: {e}");
        }
    }

    Ok(output_path)
}

#[cfg(test)]
mod anchor_tests {
    use super::*;
    use crate::osm_parser::{ProcessedMember, ProcessedNode, ProcessedRelation};

    fn node(x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id: 0,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn way(pts: &[(i32, i32)]) -> ProcessedElement {
        ProcessedElement::Way(Arc::new(ProcessedWay {
            id: 1,
            nodes: pts.iter().map(|&(x, z)| node(x, z)).collect(),
            tags: HashMap::new(),
        }))
    }

    /// Bbox cobrindo as regiões rx ∈ [-30, -28], rz ∈ [7, 9] (Guará fica a
    /// oeste do Marco Zero: X negativo é o caso normal deste motor).
    const RX: (i32, i32) = (-30, -28);
    const RZ: (i32, i32) = (7, 9);

    #[test]
    fn anchor_is_min_corner_not_centroid() {
        // Rua diagonal do canto noroeste da região (-29, 8) até (-28, 9): o
        // centroide cai em (-29, 8)/(−28, 9) dependendo do arredondamento; a
        // âncora tem que ser a região do canto mínimo, recuada pela margem.
        let w = way(&[
            (-29 * 512 + 100, 8 * 512 + 100),
            (-28 * 512 + 400, 9 * 512 + 400),
        ]);
        assert_eq!(anchor_region(&w, RX, RZ), (-29, 8));
    }

    #[test]
    fn margin_pulls_anchor_back_across_a_region_border() {
        // Nó mínimo a 10 blocos da borda oeste/norte da região (-28, 8): o brush
        // de uma avenida alcança a região anterior → âncora recua para (-29, 7).
        let w = way(&[
            (-28 * 512 + 10, 8 * 512 + 10),
            (-28 * 512 + 300, 8 * 512 + 300),
        ]);
        assert_eq!(anchor_region(&w, RX, RZ), (-29, 7));
    }

    #[test]
    fn anchor_is_clamped_into_the_swept_range() {
        // Lago que começa muito fora do bbox (a noroeste): o centroide antigo
        // cairia numa região nunca varrida e o lago sumia. Agora: primeira região.
        let w = way(&[(-40 * 512, 2 * 512), (-29 * 512 + 50, 8 * 512 + 50)]);
        assert_eq!(anchor_region(&w, RX, RZ), (-30, 7));
        // E a sudeste: última região varrida ainda executa o elemento.
        let w = way(&[
            (-27 * 512 + 900, 10 * 512 + 900),
            (-27 * 512 + 950, 10 * 512 + 950),
        ]);
        assert_eq!(anchor_region(&w, RX, RZ), (-28, 9));
    }

    #[test]
    fn relation_anchor_considers_all_members() {
        let outer = Arc::new(ProcessedWay {
            id: 2,
            nodes: vec![node(-28 * 512 + 200, 9 * 512 + 200)],
            tags: HashMap::new(),
        });
        let inner = Arc::new(ProcessedWay {
            id: 3,
            nodes: vec![node(-29 * 512 + 300, 8 * 512 + 300)],
            tags: HashMap::new(),
        });
        let rel = ProcessedElement::Relation(Arc::new(ProcessedRelation {
            id: 4,
            tags: HashMap::new(),
            members: vec![
                ProcessedMember {
                    role: ProcessedMemberRole::Outer,
                    way: outer,
                },
                ProcessedMember {
                    role: ProcessedMemberRole::Inner,
                    way: inner,
                },
            ],
        }));
        assert_eq!(anchor_region(&rel, RX, RZ), (-29, 8));
    }

    /// Propriedade central: a região-âncora nunca é posterior, na ordem da
    /// varredura (rz, depois rx), a qualquer região que o elemento (com margem)
    /// possa tocar.
    #[test]
    fn anchor_never_comes_after_any_touched_region_in_sweep_order() {
        let w = way(&[
            (-28 * 512 + 5, 8 * 512 + 5),
            (-28 * 512 + 500, 8 * 512 + 500),
        ]);
        let (arx, arz) = anchor_region(&w, RX, RZ);
        let sweep_index = |rx: i32, rz: i32| (rz - RZ.0) * (RX.1 - RX.0 + 1) + (rx - RX.0);
        for (x, z) in [
            (
                -28 * 512 + 5 - ELEMENT_REACH_MARGIN,
                8 * 512 + 5 - ELEMENT_REACH_MARGIN,
            ),
            (-28 * 512 + 5, 8 * 512 + 5),
            (-28 * 512 + 500, 8 * 512 + 500),
        ] {
            let (rx, rz) = ((x >> 9).clamp(RX.0, RX.1), (z >> 9).clamp(RZ.0, RZ.1));
            assert!(sweep_index(arx, arz) <= sweep_index(rx, rz));
        }
    }
}
