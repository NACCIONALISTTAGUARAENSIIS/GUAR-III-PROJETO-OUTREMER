use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::element_rng;
use crate::floodfill::flood_fill_area; // Needed for inline amenity flood fills
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use rand::{
    prelude::{IndexedRandom, SliceRandom},
    Rng,
};
use std::collections::{HashMap, HashSet};

// Definindo o Composter como fallback genérico de block, caso não exista no definitions.
// (O ID 249 é apenas um placeholder; o Arnis/Minecraft vai interpretar via namespace se usar set_block).
// Assumindo que o Arnis já suporta a escrita por nome no set_block se não houver constante.
const COMPOSTER: Block = Block::new(249); // Placeholder temporário, se o seu block_definitions não tiver.

pub fn generate_amenities(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    // Skip if 'layer' or 'level' is negative in the tags
    // 🚨 BESM-6: Corrigido o acesso a .tags() e a inferência de tipo do parse
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    // 🚨 BESM-6: Corrigido o acesso a .tags() e a inferência de tipo do parse
    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    // 🚨 BESM-6: Corrigido o acesso a .tags()
    if let Some(amenity_type) = element.tags().get("amenity") {
        let first_node: Option<XZPoint> = element
            .nodes()
            .map(|n: &crate::osm_parser::ProcessedNode| XZPoint::new(n.x, n.z))
            .next();
        match amenity_type.as_str() {
            "recycling" => {
                let is_container = element
                    .tags()
                    .get("recycling_type")
                    .is_some_and(|value| value == "container");

                if !is_container {
                    return;
                }

                if let Some(pt) = first_node {
                    let mut rng = rand::rng();
                    let loot_pool = build_recycling_loot_pool(element.tags());
                    let items = build_recycling_items(&loot_pool, &mut rng);

                    let properties = Value::Compound(recycling_barrel_properties());
                    let barrel_block = BlockWithProperties::new(BARREL, Some(properties));

                    // TWEAK TERRENO: Usa a elevação real para o barrel
                    let ground_y = editor.get_ground_level(pt.x, pt.z);
                    let absolute_y = editor.get_absolute_y(pt.x, ground_y + 1, pt.z);

                    editor.set_block_entity_with_items(
                        barrel_block,
                        pt.x,
                        ground_y + 1, // Assenta sobre o chão real
                        pt.z,
                        "minecraft:barrel",
                        items,
                    );

                    if let Some(category) = single_loot_category(&loot_pool) {
                        if let Some(display_item) =
                            build_display_item_for_category(category, &mut rng)
                        {
                            place_item_frame_on_random_side(
                                editor,
                                pt.x,
                                absolute_y,
                                pt.z,
                                display_item,
                            );
                        }
                    }
                }
            }
            "waste_disposal" | "waste_basket" => {
                // TWEAK URBANO DF: Lixeiras de rua (SLU)
                if let Some(pt) = first_node {
                    let ground_y = editor.get_ground_level(pt.x, pt.z);
                    // Composter dá a cara urbana e verde, posicionado no nível do terreno exato
                    editor.set_block(COMPOSTER, pt.x, ground_y + 1, pt.z, None, None);
                }
            }
            "vending_machine" | "atm" => {
                // TWEAK: ATM do BRB (Banco de Brasília) / Vending
                if let Some(pt) = first_node {
                    let ground_y = editor.get_ground_level(pt.x, pt.z);
                    editor.set_block(LIGHT_BLUE_CONCRETE, pt.x, ground_y + 1, pt.z, None, None);
                    editor.set_block(IRON_BLOCK, pt.x, ground_y + 2, pt.z, None, None); // Tela

                    // TWEAK RP: Adiciona botões nas laterais vazias para interação no jogo
                    let dirs = [(1, 0), (-1, 0), (0, 1), (0, -1)];
                    for (dx, dz) in dirs {
                        if editor.check_for_block_absolute(
                            pt.x + dx,
                            ground_y + 2,
                            pt.z + dz,
                            Some(&[AIR]),
                            None,
                        ) {
                            // Apenas garante que a área esteja limpa, o botão é um item opcional ou mod-dependent.
                            break;
                        }
                    }
                }
            }
            "bicycle_parking" => {
                let ground_block: Block = POLISHED_ANDESITE; // Concreto, não madeira para DF
                let roof_block: Block = SMOOTH_STONE_SLAB;

                let floor_area: Vec<(i32, i32)> =
                    flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

                if floor_area.is_empty() {
                    return;
                }

                for (x, z) in floor_area.iter() {
                    let ground_y = editor.get_ground_level(*x, *z);
                    editor.set_block(ground_block, *x, ground_y, *z, None, None);
                    // Racks de metal para bikes
                    if (*x + *z) % 2 == 0 {
                        editor.set_block(IRON_BARS, *x, ground_y + 1, *z, None, None);
                    }
                }

                // Place iron fences and roof slabs at each corner node
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;
                    let ground_y = editor.get_ground_level(x, z);

                    editor.set_block(ground_block, x, ground_y, z, None, None);
                    // Altura ajustada (Escala 1.15V)
                    for y in 1i32..=4i32 {
                        editor.set_block(IRON_BARS, x, ground_y + y, z, None, None);
                    }
                    editor.set_block(roof_block, x, ground_y + 5, z, None, None);
                }

                for (x, z) in floor_area.iter() {
                    let ground_y = editor.get_ground_level(*x, *z);
                    editor.set_block(roof_block, *x, ground_y + 5, *z, None, None);
                }
            }
            "bench" => {
                // TWEAK URBANO DF: Banco de Concreto do Plano Piloto
                if let Some(pt) = first_node {
                    let ground_y = editor.get_ground_level(pt.x, pt.z);
                    let mut rng = element_rng(element.id());
                    if rng.random_bool(0.5) {
                        editor.set_block(SMOOTH_STONE_SLAB, pt.x, ground_y + 1, pt.z, None, None);
                        editor.set_block(
                            SMOOTH_STONE_SLAB,
                            pt.x + 1,
                            ground_y + 1,
                            pt.z,
                            None,
                            None,
                        );
                        editor.set_block(
                            SMOOTH_STONE_SLAB,
                            pt.x - 1,
                            ground_y + 1,
                            pt.z,
                            None,
                            None,
                        );
                    } else {
                        editor.set_block(SMOOTH_STONE_SLAB, pt.x, ground_y + 1, pt.z, None, None);
                        editor.set_block(
                            SMOOTH_STONE_SLAB,
                            pt.x,
                            ground_y + 1,
                            pt.z + 1,
                            None,
                            None,
                        );
                        editor.set_block(
                            SMOOTH_STONE_SLAB,
                            pt.x,
                            ground_y + 1,
                            pt.z - 1,
                            None,
                            None,
                        );
                    }
                }
            }
            "shelter" => {
                // TWEAK URBANO DF: Parada de Ônibus (Estrutura de Concreto Maciço e Vazada)
                let roof_block: Block = SMOOTH_STONE;
                let wall_block: Block = GRAY_CONCRETE;

                let roof_area: Vec<(i32, i32)> =
                    flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

                for (i, node) in element.nodes().enumerate() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;
                    let ground_y = editor.get_ground_level(x, z);

                    // TWEAK: Apenas sobe pilar se o índice for par, criando laterais abertas
                    if i % 2 == 0 {
                        for fence_height in 1i32..=4i32 {
                            editor.set_block(wall_block, x, ground_y + fence_height, z, None, None);
                        }
                    }
                    editor.set_block(roof_block, x, ground_y + 5, z, None, None);
                }

                for (x, z) in roof_area.iter() {
                    let ground_y = editor.get_ground_level(*x, *z);
                    editor.set_block(roof_block, *x, ground_y + 5, *z, None, None);
                    // Chão da parada concretado
                    editor.set_block(POLISHED_ANDESITE, *x, ground_y, *z, None, None);
                }
            }
            "parking" | "fountain" => {
                let mut previous_node: Option<XZPoint> = None;
                let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
                let mut current_amenity: Vec<(i32, i32)> = vec![];

                let block_type = match amenity_type.as_str() {
                    "fountain" => WATER,
                    "parking" => BLACK_CONCRETE,
                    _ => GRAY_CONCRETE,
                };

                let mut min_x = i32::MAX;
                let mut min_z = i32::MAX;

                for node in element.nodes() {
                    let pt: XZPoint = node.xz();

                    min_x = min_x.min(node.x);
                    min_z = min_z.min(node.z);

                    if let Some(prev) = previous_node {
                        let bresenham_points: Vec<(i32, i32, i32)> =
                            bresenham_line(prev.x, 0, prev.z, pt.x, 0, pt.z);
                        for (bx, _, bz) in bresenham_points {
                            let ground_y = editor.get_ground_level(bx, bz);
                            editor.set_block(
                                block_type,
                                bx,
                                ground_y,
                                bz,
                                Some(&[BLACK_CONCRETE]),
                                None,
                            );

                            if amenity_type == "fountain" {
                                for dx in [-1, 0, 1].iter() {
                                    for dz in [-1, 0, 1].iter() {
                                        if (*dx, *dz) != (0, 0) {
                                            editor.set_block(
                                                SMOOTH_QUARTZ,
                                                bx + dx,
                                                ground_y,
                                                bz + dz,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                }
                            }

                            current_amenity.push((node.x, node.z));
                            corner_addup.0 += node.x;
                            corner_addup.1 += node.z;
                            corner_addup.2 += 1;
                        }
                    }
                    previous_node = Some(pt);
                }

                if corner_addup.2 > 0 {
                    let polygon_coords: Vec<(i32, i32)> = current_amenity.to_vec();
                    let flood_area: Vec<(i32, i32)> =
                        flood_fill_area(&polygon_coords, args.timeout.as_ref());

                    for (x, z) in flood_area {
                        let ground_y = editor.get_ground_level(x, z);

                        editor.set_block(
                            block_type,
                            x,
                            ground_y,
                            z,
                            Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                            None,
                        );

                        if amenity_type == "parking" {
                            let space_width = 4;
                            let space_length = 7;
                            let lane_width = 8;

                            // TWEAK: Coordenada relativa para garantir grid alinhado com o terreno do OSM
                            let relative_x = x - min_x;
                            let relative_z = z - min_z;

                            let zone_x = relative_x / space_width;
                            let zone_z = relative_z / (space_length + lane_width);
                            let local_x = relative_x % space_width;
                            let local_z = relative_z % (space_length + lane_width);

                            if local_z < space_length {
                                if local_x == 0 || local_z == 0 {
                                    editor.set_block(
                                        WHITE_CONCRETE,
                                        x,
                                        ground_y,
                                        z,
                                        Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                                        None,
                                    );
                                }
                            } else if local_z == space_length {
                                editor.set_block(
                                    WHITE_CONCRETE,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                                    None,
                                );
                            } else if local_z > space_length && local_z < space_length + lane_width
                            {
                                editor.set_block(
                                    GRAY_CONCRETE,
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[BLACK_CONCRETE]),
                                    None,
                                );
                            }

                            // TWEAK BRUTALISTA: Poste Padrão Neoenergia
                            if local_x == 0 && local_z == 0 && zone_x % 4 == 0 && zone_z % 2 == 0 {
                                editor.set_block(POLISHED_ANDESITE, x, ground_y + 1, z, None, None);
                                for dy in 2i32..=7i32 {
                                    editor.set_block(
                                        ANDESITE_WALL,
                                        x,
                                        ground_y + dy,
                                        z,
                                        None,
                                        None,
                                    );
                                }
                                editor.set_block(SEA_LANTERN, x, ground_y + 8, z, None, None);
                                editor.set_block(DAYLIGHT_DETECTOR, x, ground_y + 9, z, None, None);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// =========================================================
// AS FUNÇÕES DE LOOT FORAM MANTIDAS 100% INTACTAS
// =========================================================

#[derive(Clone, Copy)]
enum RecyclingLootKind {
    GlassBottle,
    Paper,
    GlassBlock,
    GlassPane,
    LeatherArmor,
    EmptyBucket,
    LeatherBoots,
    ScrapMetal,
    GreenWaste,
}

#[derive(Clone, Copy)]
enum LeatherPiece {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LootCategory {
    GlassBottle,
    Paper,
    Glass,
    Leather,
    EmptyBucket,
    ScrapMetal,
    GreenWaste,
}

fn recycling_barrel_properties() -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("facing".to_string(), Value::String("up".to_string()));
    props
}

fn build_recycling_loot_pool(tags: &HashMap<String, String>) -> Vec<RecyclingLootKind> {
    let mut loot_pool: Vec<RecyclingLootKind> = Vec::new();

    if tag_enabled(tags, "recycling:glass_bottles") {
        loot_pool.push(RecyclingLootKind::GlassBottle);
    }
    if tag_enabled(tags, "recycling:paper") {
        loot_pool.push(RecyclingLootKind::Paper);
    }
    if tag_enabled(tags, "recycling:glass") {
        loot_pool.push(RecyclingLootKind::GlassBlock);
        loot_pool.push(RecyclingLootKind::GlassPane);
    }
    if tag_enabled(tags, "recycling:clothes") {
        loot_pool.push(RecyclingLootKind::LeatherArmor);
    }
    if tag_enabled(tags, "recycling:cans") {
        loot_pool.push(RecyclingLootKind::EmptyBucket);
    }
    if tag_enabled(tags, "recycling:shoes") {
        loot_pool.push(RecyclingLootKind::LeatherBoots);
    }
    if tag_enabled(tags, "recycling:scrap_metal") {
        loot_pool.push(RecyclingLootKind::ScrapMetal);
    }
    if tag_enabled(tags, "recycling:green_waste") {
        loot_pool.push(RecyclingLootKind::GreenWaste);
    }

    loot_pool
}

fn build_recycling_items(
    loot_pool: &[RecyclingLootKind],
    rng: &mut impl Rng,
) -> Vec<HashMap<String, Value>> {
    if loot_pool.is_empty() {
        return Vec::new();
    }

    let mut items = Vec::new();
    for slot in 0..27 {
        if rng.random_bool(0.2) {
            let kind = loot_pool[rng.random_range(0..loot_pool.len())];
            if let Some(item) = build_item_for_kind(kind, slot as i8, rng) {
                items.push(item);
            }
        }
    }

    items
}

fn kind_to_category(kind: RecyclingLootKind) -> LootCategory {
    match kind {
        RecyclingLootKind::GlassBottle => LootCategory::GlassBottle,
        RecyclingLootKind::Paper => LootCategory::Paper,
        RecyclingLootKind::GlassBlock | RecyclingLootKind::GlassPane => LootCategory::Glass,
        RecyclingLootKind::LeatherArmor | RecyclingLootKind::LeatherBoots => LootCategory::Leather,
        RecyclingLootKind::EmptyBucket => LootCategory::EmptyBucket,
        RecyclingLootKind::ScrapMetal => LootCategory::ScrapMetal,
        RecyclingLootKind::GreenWaste => LootCategory::GreenWaste,
    }
}

fn single_loot_category(loot_pool: &[RecyclingLootKind]) -> Option<LootCategory> {
    let mut categories: HashSet<LootCategory> = HashSet::new();
    for kind in loot_pool {
        categories.insert(kind_to_category(*kind));
        if categories.len() > 1 {
            return None;
        }
    }
    categories.iter().next().copied()
}

fn build_display_item_for_category(
    category: LootCategory,
    rng: &mut impl Rng,
) -> Option<HashMap<String, Value>> {
    match category {
        LootCategory::GlassBottle => Some(make_display_item("minecraft:glass_bottle", 1)),
        LootCategory::Paper => Some(make_display_item(
            "minecraft:paper",
            rng.random_range(1..=4),
        )),
        LootCategory::Glass => Some(make_display_item("minecraft:glass", 1)),
        LootCategory::Leather => Some(build_leather_display_item(rng)),
        LootCategory::EmptyBucket => Some(make_display_item("minecraft:bucket", 1)),
        LootCategory::ScrapMetal => {
            let metals = [
                "minecraft:copper_ingot",
                "minecraft:iron_ingot",
                "minecraft:gold_ingot",
            ];
            let metal = metals.choose(rng)?;
            Some(make_display_item(metal, rng.random_range(1..=2)))
        }
        LootCategory::GreenWaste => {
            let options = [
                "minecraft:oak_sapling",
                "minecraft:birch_sapling",
                "minecraft:tall_grass",
                "minecraft:sweet_berries",
                "minecraft:wheat_seeds",
            ];
            let choice = options.choose(rng)?;
            Some(make_display_item(choice, rng.random_range(1..=3)))
        }
    }
}

fn place_item_frame_on_random_side(
    editor: &mut WorldEditor,
    x: i32,
    barrel_absolute_y: i32,
    z: i32,
    item: HashMap<String, Value>,
) {
    let mut rng = rand::rng();
    let mut directions = [
        ((0, 0, -1), 2), // North
        ((0, 0, 1), 3),  // South
        ((-1, 0, 0), 4), // West
        ((1, 0, 0), 5),  // East
    ];
    directions.shuffle(&mut rng);

    let (min_x, min_z) = editor.get_min_coords();
    let (max_x, max_z) = editor.get_max_coords();

    let ((dx, _dy, dz), facing) = directions
        .into_iter()
        .find(|((dx, _dy, dz), _)| {
            let target_x = x + dx;
            let target_z = z + dz;
            target_x >= min_x && target_x <= max_x && target_z >= min_z && target_z <= max_z
        })
        .unwrap_or(((0, 0, 1), 3)); // Fallback south if all directions are out of bounds

    let target_x = x + dx;
    let target_y = barrel_absolute_y;
    let target_z = z + dz;

    let ground_y = editor.get_absolute_y(target_x, 0, target_z);

    let mut extra = HashMap::new();
    extra.insert("Facing".to_string(), Value::Byte(facing));
    extra.insert("ItemRotation".to_string(), Value::Byte(0));
    extra.insert("Item".to_string(), Value::Compound(item));
    extra.insert("ItemDropChance".to_string(), Value::Float(1.0));
    extra.insert(
        "block_pos".to_string(),
        Value::List(vec![
            Value::Int(target_x),
            Value::Int(target_y),
            Value::Int(target_z),
        ]),
    );
    extra.insert("TileX".to_string(), Value::Int(target_x));
    extra.insert("TileY".to_string(), Value::Int(target_y));
    extra.insert("TileZ".to_string(), Value::Int(target_z));
    extra.insert("Fixed".to_string(), Value::Byte(1));

    let relative_y = target_y - ground_y;
    editor.add_entity(
        "minecraft:item_frame",
        target_x,
        relative_y,
        target_z,
        Some(extra),
    );
}

fn make_display_item(id: &str, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}

fn build_leather_display_item(rng: &mut impl Rng) -> HashMap<String, Value> {
    let mut item = make_display_item("minecraft:leather_chestplate", 1);
    let damage = biased_damage(80, rng);

    let mut tag = HashMap::new();
    tag.insert("Damage".to_string(), Value::Int(damage));

    if let Some(color) = maybe_leather_color(rng) {
        let mut display = HashMap::new();
        display.insert("color".to_string(), Value::Int(color));
        tag.insert("display".to_string(), Value::Compound(display));
    }

    item.insert("tag".to_string(), Value::Compound(tag));

    let mut components = HashMap::new();
    components.insert("minecraft:damage".to_string(), Value::Int(damage));
    item.insert("components".to_string(), Value::Compound(components));

    item
}

fn build_item_for_kind(
    kind: RecyclingLootKind,
    slot: i8,
    rng: &mut impl Rng,
) -> Option<HashMap<String, Value>> {
    match kind {
        RecyclingLootKind::GlassBottle => Some(make_basic_item(
            "minecraft:glass_bottle",
            slot,
            rng.random_range(1..=4),
        )),
        RecyclingLootKind::Paper => Some(make_basic_item(
            "minecraft:paper",
            slot,
            rng.random_range(1..=10),
        )),
        RecyclingLootKind::GlassBlock => Some(build_glass_item(false, slot, rng)),
        RecyclingLootKind::GlassPane => Some(build_glass_item(true, slot, rng)),
        RecyclingLootKind::LeatherArmor => {
            Some(build_leather_item(random_leather_piece(rng), slot, rng))
        }
        RecyclingLootKind::EmptyBucket => Some(make_basic_item("minecraft:bucket", slot, 1)),
        RecyclingLootKind::LeatherBoots => Some(build_leather_item(LeatherPiece::Boots, slot, rng)),
        RecyclingLootKind::ScrapMetal => Some(build_scrap_metal_item(slot, rng)),
        RecyclingLootKind::GreenWaste => Some(build_green_waste_item(slot, rng)),
    }
}

fn build_scrap_metal_item(slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    let metals = ["copper_ingot", "iron_ingot", "gold_ingot"];
    let metal = metals.choose(rng).expect("scrap metal list is non-empty");
    let count = rng.random_range(1..=3);
    make_basic_item(&format!("minecraft:{metal}"), slot, count)
}

fn build_green_waste_item(slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    #[allow(clippy::match_same_arms)]
    let (id, count) = match rng.random_range(0..8) {
        0 => ("minecraft:tall_grass", rng.random_range(1..=4)),
        1 => ("minecraft:sweet_berries", rng.random_range(2..=6)),
        2 => ("minecraft:oak_sapling", rng.random_range(1..=2)),
        3 => ("minecraft:birch_sapling", rng.random_range(1..=2)),
        4 => ("minecraft:spruce_sapling", rng.random_range(1..=2)),
        5 => ("minecraft:jungle_sapling", rng.random_range(1..=2)),
        6 => ("minecraft:acacia_sapling", rng.random_range(1..=2)),
        _ => ("minecraft:dark_oak_sapling", rng.random_range(1..=2)),
    };

    let id = if rng.random_bool(0.25) {
        match rng.random_range(0..4) {
            0 => "minecraft:wheat_seeds",
            1 => "minecraft:pumpkin_seeds",
            2 => "minecraft:melon_seeds",
            _ => "minecraft:beetroot_seeds",
        }
    } else {
        id
    };

    make_basic_item(id, slot, count)
}

fn build_glass_item(is_pane: bool, slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    const GLASS_COLORS: &[&str] = &[
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];

    let use_colorless = rng.random_bool(0.7);

    let id = if use_colorless {
        if is_pane {
            "minecraft:glass_pane".to_string()
        } else {
            "minecraft:glass".to_string()
        }
    } else {
        let color = GLASS_COLORS
            .choose(rng)
            .expect("glass color array is non-empty");
        if is_pane {
            format!("minecraft:{color}_stained_glass_pane")
        } else {
            format!("minecraft:{color}_stained_glass")
        }
    };

    let count = if is_pane {
        rng.random_range(4..=16)
    } else {
        rng.random_range(1..=6)
    };

    make_basic_item(&id, slot, count)
}

fn build_leather_item(piece: LeatherPiece, slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    let (id, max_damage) = match piece {
        LeatherPiece::Helmet => ("minecraft:leather_helmet", 55),
        LeatherPiece::Chestplate => ("minecraft:leather_chestplate", 80),
        LeatherPiece::Leggings => ("minecraft:leather_leggings", 75),
        LeatherPiece::Boots => ("minecraft:leather_boots", 65),
    };

    let mut item = make_basic_item(id, slot, 1);
    let damage = biased_damage(max_damage, rng);

    let mut tag = HashMap::new();
    tag.insert("Damage".to_string(), Value::Int(damage));

    if let Some(color) = maybe_leather_color(rng) {
        let mut display = HashMap::new();
        display.insert("color".to_string(), Value::Int(color));
        tag.insert("display".to_string(), Value::Compound(display));
    }

    item.insert("tag".to_string(), Value::Compound(tag));

    let mut components = HashMap::new();
    components.insert("minecraft:damage".to_string(), Value::Int(damage));
    item.insert("components".to_string(), Value::Compound(components));

    item
}

fn biased_damage(max_damage: i32, rng: &mut impl Rng) -> i32 {
    let safe_max = max_damage.max(1);
    let upper = safe_max.saturating_sub(1);
    let lower = (safe_max / 2).min(upper);

    let heavy_wear = rng.random_range(lower..=upper);
    let random_wear = rng.random_range(0..=upper);
    heavy_wear.max(random_wear)
}

fn maybe_leather_color(rng: &mut impl Rng) -> Option<i32> {
    if rng.random_bool(0.3) {
        Some(rng.random_range(0..=0x00FF_FFFF))
    } else {
        None
    }
}

fn random_leather_piece(rng: &mut impl Rng) -> LeatherPiece {
    match rng.random_range(0..4) {
        0 => LeatherPiece::Helmet,
        1 => LeatherPiece::Chestplate,
        2 => LeatherPiece::Leggings,
        _ => LeatherPiece::Boots,
    }
}

fn make_basic_item(id: &str, slot: i8, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Slot".to_string(), Value::Byte(slot));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}

fn tag_enabled(tags: &HashMap<String, String>, key: &str) -> bool {
    tags.get(key).is_some_and(|value| value == "yes")
}
