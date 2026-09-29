#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::colors::RGBTuple;

/// Tabela nome → bloco, montada uma vez varrendo todos os ids definidos.
static NAME_TO_BLOCK: Lazy<HashMap<&'static str, Block>> = Lazy::new(|| {
    let mut map = HashMap::new();
    for id in 0..4096u16 {
        let block = Block::new(id);
        if let Some(name) = block.name_opt() {
            map.entry(name).or_insert(block);
        }
    }
    map
});

// Enums for stair properties
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum StairFacing {
    North,
    East,
    South,
    West,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum StairShape {
    Straight,
    InnerLeft,
    InnerRight,
    OuterLeft,
    OuterRight,
}

impl StairFacing {
    #[inline(always)]
    pub fn as_str(&self) -> &'static str {
        match self {
            StairFacing::North => "north",
            StairFacing::East => "east",
            StairFacing::South => "south",
            StairFacing::West => "west",
        }
    }
}

impl StairShape {
    #[inline(always)]
    pub fn as_str(&self) -> &'static str {
        match self {
            StairShape::Straight => "straight",
            StairShape::InnerLeft => "inner_left",
            StairShape::InnerRight => "inner_right",
            StairShape::OuterLeft => "outer_left",
            StairShape::OuterRight => "outer_right",
        }
    }
}

// Type definitions for better readability
type ColorTuple = (u8, u8, u8);
type BlockOptions = &'static [Block];
type ColorBlockMapping = (ColorTuple, BlockOptions);

#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash, Debug)]
pub struct Block {
    // 🚨 BESM-6: Campo tornado PÚBLICO para acesso direto e veloz no map_renderer e exportadores
    pub id: u16, // Aumentado para u16 para garantir espaço futuro
}

// Extended block with dynamic properties
#[derive(Clone, Debug)]
pub struct BlockWithProperties {
    pub block: Block,
    pub properties: Option<Value>,
}

impl BlockWithProperties {
    pub fn new(block: Block, properties: Option<Value>) -> Self {
        Self { block, properties }
    }

    pub fn simple(block: Block) -> Self {
        Self {
            block,
            properties: None,
        }
    }
}

impl Block {
    #[inline(always)]
    pub const fn new(id: u16) -> Self {
        Self { id }
    }

    #[inline(always)]
    pub fn id(&self) -> u16 {
        self.id
    }

    #[inline(always)]
    pub fn namespace(&self) -> &str {
        "minecraft"
    }

    pub fn name(&self) -> &str {
        self.name_opt()
            .unwrap_or_else(|| panic!("Invalid id: {}", self.id))
    }

    /// Nome do bloco (sem o namespace), ou `None` para um id sem definição.
    /// Base de `Block::from_name`, que precisa varrer todos os ids possíveis
    /// sem estourar em pânico nos buracos da numeração.
    pub fn name_opt(&self) -> Option<&'static str> {
        match self.id {
            0 => Some("acacia_planks"),
            1 => Some("air"),
            2 => Some("andesite"),
            3 => Some("birch_leaves"),
            4 => Some("birch_log"),
            5 => Some("black_concrete"),
            6 => Some("blackstone"),
            7 => Some("blue_orchid"),
            8 => Some("blue_terracotta"),
            9 => Some("bricks"),
            10 => Some("cauldron"),
            11 => Some("chiseled_stone_bricks"),
            12 => Some("cobblestone_wall"),
            13 => Some("cobblestone"),
            14 => Some("polished_blackstone_bricks"),
            15 => Some("cracked_stone_bricks"),
            16 => Some("crimson_planks"),
            17 => Some("cut_sandstone"),
            18 => Some("cyan_concrete"),
            19 => Some("dark_oak_planks"),
            20 => Some("deepslate_bricks"),
            21 => Some("diorite"),
            22 => Some("dirt"),
            23 => Some("end_stone_bricks"),
            24 => Some("farmland"),
            25 => Some("glass"),
            26 => Some("glowstone"),
            27 => Some("granite"),
            28 => Some("grass_block"),
            29 => Some("short_grass"),
            30 => Some("gravel"),
            31 => Some("gray_concrete"),
            32 => Some("gray_terracotta"),
            33 => Some("green_terracotta"),
            34 => Some("green_wool"),
            35 => Some("hay_block"),
            36 => Some("iron_bars"),
            37 => Some("iron_block"),
            38 => Some("jungle_planks"),
            39 => Some("ladder"),
            40 => Some("light_blue_concrete"),
            41 => Some("light_blue_terracotta"),
            42 => Some("light_gray_concrete"),
            43 => Some("moss_block"),
            44 => Some("mossy_cobblestone"),
            45 => Some("mud_bricks"),
            46 => Some("nether_bricks"),
            47 => Some("netherite_block"),
            48 => Some("oak_fence"),
            49 => Some("oak_leaves"),
            50 => Some("oak_log"),
            51 => Some("oak_planks"),
            52 => Some("oak_slab"),
            53 => Some("orange_terracotta"),
            54 => Some("podzol"),
            55 => Some("polished_andesite"),
            56 => Some("polished_basalt"),
            57 => Some("quartz_block"),
            58 => Some("polished_blackstone"),
            59 => Some("polished_deepslate"),
            60 => Some("polished_diorite"),
            61 => Some("polished_granite"),
            62 => Some("prismarine"),
            63 => Some("purpur_block"),
            64 => Some("purpur_pillar"),
            65 => Some("quartz_bricks"),
            66 => Some("rail"),
            67 => Some("poppy"),
            68 => Some("red_nether_bricks"),
            69 => Some("red_terracotta"),
            70 => Some("red_wool"),
            71 => Some("sand"),
            72 => Some("sandstone"),
            73 => Some("scaffolding"),
            74 => Some("smooth_quartz"),
            75 => Some("smooth_red_sandstone"),
            76 => Some("smooth_sandstone"),
            77 => Some("smooth_stone"),
            78 => Some("sponge"),
            79 => Some("spruce_log"),
            80 => Some("spruce_planks"),
            81 => Some("stone_slab"),
            82 => Some("stone_brick_slab"),
            83 => Some("stone_bricks"),
            84 => Some("stone"),
            85 => Some("terracotta"),
            86 => Some("warped_planks"),
            87 => Some("water"),
            88 => Some("white_concrete"),
            89 => Some("azure_bluet"),
            90 => Some("white_stained_glass"),
            91 => Some("white_terracotta"),
            92 => Some("white_wool"),
            93 => Some("yellow_concrete"),
            94 => Some("dandelion"),
            95 => Some("yellow_wool"),
            96 => Some("lime_concrete"),
            97 => Some("cyan_wool"),
            98 => Some("blue_concrete"),
            99 => Some("purple_concrete"),
            100 => Some("red_concrete"),
            101 => Some("magenta_concrete"),
            102 => Some("brown_wool"),
            103 => Some("oxidized_copper"),
            104 => Some("yellow_terracotta"),
            105 => Some("carrots"),
            106 => Some("dark_oak_door"),
            107 => Some("dark_oak_door"),
            108 => Some("potatoes"),
            109 => Some("wheat"),
            110 => Some("bedrock"),
            111 => Some("snow_block"),
            112 => Some("snow"),
            113 => Some("oak_sign"),
            114 => Some("andesite_wall"),
            115 => Some("stone_brick_wall"),
            116..=125 => Some("rail"),
            126 => Some("coarse_dirt"),
            127 => Some("iron_ore"),
            128 => Some("coal_ore"),
            129 => Some("gold_ore"),
            130 => Some("copper_block"),
            131 => Some("clay"),
            132 => Some("dirt_path"),
            133 => Some("ice"),
            134 => Some("packed_ice"),
            135 => Some("mud"),
            136 => Some("dead_bush"),
            137..=138 => Some("tall_grass"),
            139 => Some("crafting_table"),
            140 => Some("furnace"),
            141 => Some("white_carpet"),
            142 => Some("bookshelf"),
            143 => Some("oak_pressure_plate"),
            144 => Some("oak_stairs"),
            155 => Some("chest"),
            156 => Some("red_carpet"),
            157 => Some("anvil"),
            158 => Some("note_block"),
            159 => Some("oak_door"),
            160 => Some("brewing_stand"),
            161 => Some("red_bed"),
            162 => Some("red_bed"),
            163 => Some("red_bed"),
            164 => Some("red_bed"),
            165 => Some("red_bed"),
            166 => Some("red_bed"),
            167 => Some("red_bed"),
            168 => Some("red_bed"),
            169 => Some("gray_stained_glass"),
            170 => Some("light_gray_stained_glass"),
            171 => Some("brown_stained_glass"),
            172 => Some("tinted_glass"),
            173 => Some("oak_trapdoor"),
            174 => Some("brown_concrete"),
            175 => Some("black_terracotta"),
            176 => Some("brown_terracotta"),
            177 => Some("stone_brick_stairs"),
            178 => Some("mud_brick_stairs"),
            179 => Some("polished_blackstone_brick_stairs"),
            180 => Some("brick_stairs"),
            181 => Some("polished_granite_stairs"),
            182 => Some("end_stone_brick_stairs"),
            183 => Some("polished_diorite_stairs"),
            184 => Some("smooth_sandstone_stairs"),
            185 => Some("quartz_stairs"),
            186 => Some("polished_andesite_stairs"),
            187 => Some("nether_brick_stairs"),
            188 => Some("barrel"),
            189 => Some("fern"),
            190 => Some("cobweb"),
            191 => Some("chiseled_bookshelf"),
            192 => Some("chiseled_bookshelf"),
            193 => Some("chiseled_bookshelf"),
            194 => Some("chiseled_bookshelf"),
            195 => Some("chipped_anvil"),
            196 => Some("damaged_anvil"),
            197 => Some("large_fern"),
            198 => Some("large_fern"),
            199 => Some("chain"),
            200 => Some("end_rod"),
            201 => Some("lightning_rod"),
            202 => Some("gold_block"),
            203 => Some("sea_lantern"),
            204 => Some("orange_concrete"),
            205 => Some("orange_wool"),
            206 => Some("blue_wool"),
            207 => Some("green_concrete"),
            208 => Some("brick_wall"),
            209 => Some("redstone_block"),
            210 => Some("chain"),
            211 => Some("chain"),
            212 => Some("spruce_door"),
            213 => Some("spruce_door"),
            214 => Some("smooth_stone_slab"),
            215 => Some("glass_pane"),
            216 => Some("light_gray_terracotta"),
            217 => Some("oak_slab"),
            218 => Some("oak_door"),
            219 => Some("dark_oak_log"),
            220 => Some("dark_oak_leaves"),
            221 => Some("jungle_log"),
            222 => Some("jungle_leaves"),
            223 => Some("acacia_log"),
            224 => Some("acacia_leaves"),
            225 => Some("spruce_leaves"),
            226 => Some("cyan_stained_glass"),
            227 => Some("blue_stained_glass"),
            228 => Some("light_blue_stained_glass"),
            229 => Some("daylight_detector"),
            230 => Some("red_stained_glass"),
            231 => Some("yellow_stained_glass"),
            232 => Some("purple_stained_glass"),
            233 => Some("orange_stained_glass"),
            234 => Some("magenta_stained_glass"),
            235 => Some("potted_poppy"),
            236 => Some("oak_trapdoor"),
            237 => Some("oak_trapdoor"),
            238 => Some("oak_trapdoor"),
            239 => Some("oak_trapdoor"),
            240 => Some("quartz_slab"),
            241 => Some("dark_oak_trapdoor"),
            242 => Some("spruce_trapdoor"),
            243 => Some("birch_trapdoor"),
            244 => Some("mud_brick_slab"),
            245 => Some("brick_slab"),
            246 => Some("potted_red_tulip"),
            247 => Some("potted_dandelion"),
            248 => Some("potted_blue_orchid"),
            249 => Some("black_stained_glass"),
            250 => Some("copper_grate"), // 🚨 BESM-6 Tweak: Cobogó Element para UnB e Fachadas Antigas
            251 => Some("diorite_wall"),
            252 => Some("azalea_leaves"),
            253 => Some("spruce_fence"),
            254 => Some("dark_oak_fence"),
            255 => Some("flowering_azalea"),
            256 => Some("quartz_slab"),
            257 => Some("spruce_slab"),
            258 => Some("iron_trapdoor"),
            259 => Some("iron_door"),
            260 => Some("oak_fence_gate"),
            261 => Some("oak_leaves"),
            262 => Some("stone_stairs"),
            263 => Some("tall_grass"),
            264 => Some("moss_carpet"),
            265 => Some("copper_ore"),
            266 => Some("green_terracotta"),
            267 => Some("light_weighted_pressure_plate"),
            268 => Some("grindstone"),
            269 => Some("cactus"),
            270 => Some("lily_pad"),
            271 => Some("pink_tulip"),
            272 => Some("allium"),
            273 => Some("red_tulip"),
            274 => Some("orange_tulip"),
            275 => Some("dark_oak_slab"),
            276 => Some("cyan_terracotta"),
            277 => Some("acacia_fence"),
            278 => Some("jungle_fence"),
            _ => None,
        }
    }

    /// Inverso de `name()`: resolve `"minecraft:oak_stairs"`/`"oak_stairs"`
    /// para o `Block` correspondente. Usado ao RELER uma região já gravada em
    /// disco (segunda passada do Halo, ver `WorldEditor::flush_pending_halo`).
    pub fn from_name(name: &str) -> Option<Block> {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        NAME_TO_BLOCK.get(bare).copied()
    }

    pub fn properties(&self) -> Option<Value> {
        match self.id {
            3 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            49 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            105 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            106 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            107 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            108 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            109 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            113 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("rotation".to_string(), Value::String("6".to_string()));
                map.insert(
                    "waterlogged".to_string(),
                    Value::String("false".to_string()),
                );
                map
            })),
            159 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            116 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("north_south".to_string()),
                );
                map
            })),
            117 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("east_west".to_string()));
                map
            })),
            118 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_east".to_string()),
                );
                map
            })),
            119 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_west".to_string()),
                );
                map
            })),
            120 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_north".to_string()),
                );
                map
            })),
            121 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_south".to_string()),
                );
                map
            })),
            122 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("north_east".to_string()));
                map
            })),
            123 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("north_west".to_string()));
                map
            })),
            124 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("south_east".to_string()));
                map
            })),
            125 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("south_west".to_string()));
                map
            })),
            137 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            138 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            161 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            162 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            163 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            164 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            165 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            166 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            167 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            168 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            173 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            191 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map
            })),
            192 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map
            })),
            193 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map
            })),
            194 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map
            })),
            197 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            198 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            210 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("axis".to_string(), Value::String("x".to_string()));
                map
            })),
            211 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("axis".to_string(), Value::String("z".to_string()));
                map
            })),
            212 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            213 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            214 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("bottom".to_string()));
                map
            })),
            217 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("top".to_string()));
                map
            })),
            218 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            220 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            222 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            224 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            225 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            240 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("top".to_string()));
                map
            })),
            236 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("open".to_string(), Value::String("true".to_string()));
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            237 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("open".to_string(), Value::String("true".to_string()));
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            238 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("open".to_string(), Value::String("true".to_string()));
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            239 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("open".to_string(), Value::String("true".to_string()));
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            256 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("bottom".to_string()));
                map
            })),
            _ => None,
        }
    }
}

use std::sync::Mutex;
type StairCacheKey = (u16, StairFacing, StairShape);
type StairCacheMap = Mutex<HashMap<StairCacheKey, BlockWithProperties>>;
static STAIR_CACHE: Lazy<StairCacheMap> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn create_stair_with_properties(
    base_stair_block: Block,
    facing: StairFacing,
    shape: StairShape,
) -> BlockWithProperties {
    let cache_key = (base_stair_block.id(), facing, shape);
    {
        let cache = STAIR_CACHE.lock().unwrap();
        if let Some(cached_block) = cache.get(&cache_key) {
            return cached_block.clone();
        }
    }
    let mut map = HashMap::new();
    map.insert(
        "facing".to_string(),
        Value::String(facing.as_str().to_string()),
    );
    if !matches!(shape, StairShape::Straight) {
        map.insert(
            "shape".to_string(),
            Value::String(shape.as_str().to_string()),
        );
    }
    let properties = Value::Compound(map);
    let block_with_props = BlockWithProperties::new(base_stair_block, Some(properties));
    {
        let mut cache = STAIR_CACHE.lock().unwrap();
        cache.insert(cache_key, block_with_props.clone());
    }
    block_with_props
}

// Block constants
pub const ACACIA_PLANKS: Block = Block::new(0);
pub const AIR: Block = Block::new(1);
pub const ANDESITE: Block = Block::new(2);
pub const BIRCH_LEAVES: Block = Block::new(3);
pub const BIRCH_LOG: Block = Block::new(4);
pub const BLACK_CONCRETE: Block = Block::new(5);
pub const BLACKSTONE: Block = Block::new(6);
pub const BLUE_FLOWER: Block = Block::new(7);
pub const BLUE_TERRACOTTA: Block = Block::new(8);
pub const BRICK: Block = Block::new(9);
pub const CAULDRON: Block = Block::new(10);
pub const CHISELED_STONE_BRICKS: Block = Block::new(11);
pub const COBBLESTONE_WALL: Block = Block::new(12);
pub const COBBLESTONE: Block = Block::new(13);
pub const POLISHED_BLACKSTONE_BRICKS: Block = Block::new(14);
pub const CRACKED_STONE_BRICKS: Block = Block::new(15);
pub const CRIMSON_PLANKS: Block = Block::new(16);
pub const CUT_SANDSTONE: Block = Block::new(17);
pub const CYAN_CONCRETE: Block = Block::new(18);
pub const DARK_OAK_PLANKS: Block = Block::new(19);
pub const DEEPSLATE_BRICKS: Block = Block::new(20);
pub const DIORITE: Block = Block::new(21);
pub const DIRT: Block = Block::new(22);
pub const END_STONE_BRICKS: Block = Block::new(23);
pub const FARMLAND: Block = Block::new(24);
pub const GLASS: Block = Block::new(25);
pub const GLOWSTONE: Block = Block::new(26);
pub const GRANITE: Block = Block::new(27);
pub const GRASS_BLOCK: Block = Block::new(28);
pub const GRASS: Block = Block::new(29);
pub const GRAVEL: Block = Block::new(30);
pub const GRAY_CONCRETE: Block = Block::new(31);
pub const GRAY_TERRACOTTA: Block = Block::new(32);
pub const GREEN_STAINED_HARDENED_CLAY: Block = Block::new(33);
pub const GREEN_WOOL: Block = Block::new(34);
pub const HAY_BALE: Block = Block::new(35);
pub const IRON_BARS: Block = Block::new(36);
pub const IRON_BLOCK: Block = Block::new(37);
pub const JUNGLE_PLANKS: Block = Block::new(38);
pub const LADDER: Block = Block::new(39);
pub const LIGHT_BLUE_CONCRETE: Block = Block::new(40);
pub const LIGHT_BLUE_TERRACOTTA: Block = Block::new(41);
pub const LIGHT_GRAY_CONCRETE: Block = Block::new(42);
pub const MOSS_BLOCK: Block = Block::new(43);
pub const MOSSY_COBBLESTONE: Block = Block::new(44);
pub const MUD_BRICKS: Block = Block::new(45);
pub const NETHER_BRICK: Block = Block::new(46);
pub const NETHERITE_BLOCK: Block = Block::new(47);
pub const OAK_FENCE: Block = Block::new(48);
pub const OAK_LEAVES: Block = Block::new(49);
pub const OAK_LOG: Block = Block::new(50);
pub const OAK_PLANKS: Block = Block::new(51);
pub const OAK_SLAB: Block = Block::new(52);
pub const ORANGE_TERRACOTTA: Block = Block::new(53);
pub const PODZOL: Block = Block::new(54);
pub const POLISHED_ANDESITE: Block = Block::new(55);
pub const POLISHED_BASALT: Block = Block::new(56);
pub const QUARTZ_BLOCK: Block = Block::new(57);
pub const POLISHED_BLACKSTONE: Block = Block::new(58);
pub const POLISHED_DEEPSLATE: Block = Block::new(59);
pub const POLISHED_DIORITE: Block = Block::new(60);
pub const POLISHED_GRANITE: Block = Block::new(61);
pub const PRISMARINE: Block = Block::new(62);
pub const PURPUR_BLOCK: Block = Block::new(63);
pub const PURPUR_PILLAR: Block = Block::new(64);
pub const QUARTZ_BRICKS: Block = Block::new(65);
pub const RAIL: Block = Block::new(66);
pub const RED_FLOWER: Block = Block::new(67);
pub const RED_NETHER_BRICK: Block = Block::new(68);
pub const RED_TERRACOTTA: Block = Block::new(69);
pub const RED_WOOL: Block = Block::new(70);
pub const SAND: Block = Block::new(71);
pub const SANDSTONE: Block = Block::new(72);
pub const SCAFFOLDING: Block = Block::new(73);
pub const SMOOTH_QUARTZ: Block = Block::new(74);
pub const SMOOTH_RED_SANDSTONE: Block = Block::new(75);
pub const SMOOTH_SANDSTONE: Block = Block::new(76);
pub const SMOOTH_STONE: Block = Block::new(77);
pub const SPONGE: Block = Block::new(78);
pub const SPRUCE_LOG: Block = Block::new(79);
pub const SPRUCE_PLANKS: Block = Block::new(80);
pub const STONE_BLOCK_SLAB: Block = Block::new(81);
pub const STONE_BRICK_SLAB: Block = Block::new(82);
pub const STONE_BRICKS: Block = Block::new(83);
pub const STONE: Block = Block::new(84);
pub const TERRACOTTA: Block = Block::new(85);
pub const WARPED_PLANKS: Block = Block::new(86);
pub const WATER: Block = Block::new(87);
pub const WHITE_CONCRETE: Block = Block::new(88);
pub const WHITE_FLOWER: Block = Block::new(89);
pub const WHITE_STAINED_GLASS: Block = Block::new(90);
pub const WHITE_TERRACOTTA: Block = Block::new(91);
pub const WHITE_WOOL: Block = Block::new(92);
pub const YELLOW_CONCRETE: Block = Block::new(93);
pub const YELLOW_FLOWER: Block = Block::new(94);
pub const YELLOW_WOOL: Block = Block::new(95);
pub const LIME_CONCRETE: Block = Block::new(96);
pub const CYAN_WOOL: Block = Block::new(97);
pub const BLUE_CONCRETE: Block = Block::new(98);
pub const PURPLE_CONCRETE: Block = Block::new(99);
pub const RED_CONCRETE: Block = Block::new(100);
pub const MAGENTA_CONCRETE: Block = Block::new(101);
pub const BROWN_WOOL: Block = Block::new(102);
pub const OXIDIZED_COPPER: Block = Block::new(103);
pub const YELLOW_TERRACOTTA: Block = Block::new(104);
pub const SNOW_BLOCK: Block = Block::new(111);
pub const SNOW_LAYER: Block = Block::new(112);
pub const SIGN: Block = Block::new(113);
pub const ANDESITE_WALL: Block = Block::new(114);
pub const STONE_BRICK_WALL: Block = Block::new(115);
pub const CARROTS: Block = Block::new(105);
pub const DARK_OAK_DOOR_LOWER: Block = Block::new(106);
pub const DARK_OAK_DOOR_UPPER: Block = Block::new(107);
pub const POTATOES: Block = Block::new(108);
pub const WHEAT: Block = Block::new(109);
pub const BEDROCK: Block = Block::new(110);
pub const RAIL_NORTH_SOUTH: Block = Block::new(116);
pub const RAIL_EAST_WEST: Block = Block::new(117);
pub const RAIL_ASCENDING_EAST: Block = Block::new(118);
pub const RAIL_ASCENDING_WEST: Block = Block::new(119);
pub const RAIL_ASCENDING_NORTH: Block = Block::new(120);
pub const RAIL_ASCENDING_SOUTH: Block = Block::new(121);
pub const RAIL_NORTH_EAST: Block = Block::new(122);
pub const RAIL_NORTH_WEST: Block = Block::new(123);
pub const RAIL_SOUTH_EAST: Block = Block::new(124);
pub const RAIL_SOUTH_WEST: Block = Block::new(125);
pub const COARSE_DIRT: Block = Block::new(126);
pub const IRON_ORE: Block = Block::new(127);
pub const COAL_ORE: Block = Block::new(128);
pub const GOLD_ORE: Block = Block::new(129);
pub const COPPER_BLOCK: Block = Block::new(130);
pub const CLAY: Block = Block::new(131);
pub const DIRT_PATH: Block = Block::new(132);
pub const ICE: Block = Block::new(133);
pub const PACKED_ICE: Block = Block::new(134);
pub const MUD: Block = Block::new(135);
pub const DEAD_BUSH: Block = Block::new(136);
pub const TALL_GRASS_BOTTOM: Block = Block::new(137);
pub const TALL_GRASS_TOP: Block = Block::new(138);
pub const CRAFTING_TABLE: Block = Block::new(139);
pub const FURNACE: Block = Block::new(140);
pub const WHITE_CARPET: Block = Block::new(141);
pub const BOOKSHELF: Block = Block::new(142);
pub const OAK_PRESSURE_PLATE: Block = Block::new(143);
pub const OAK_STAIRS: Block = Block::new(144);
pub const CHEST: Block = Block::new(155);
pub const RED_CARPET: Block = Block::new(156);
pub const ANVIL: Block = Block::new(157);
pub const NOTE_BLOCK: Block = Block::new(158);
pub const OAK_DOOR: Block = Block::new(159);
pub const BREWING_STAND: Block = Block::new(160);
pub const RED_BED_NORTH_HEAD: Block = Block::new(161);
pub const RED_BED_NORTH_FOOT: Block = Block::new(162);
pub const RED_BED_EAST_HEAD: Block = Block::new(163);
pub const RED_BED_EAST_FOOT: Block = Block::new(164);
pub const RED_BED_SOUTH_HEAD: Block = Block::new(165);
pub const RED_BED_SOUTH_FOOT: Block = Block::new(166);
pub const RED_BED_WEST_HEAD: Block = Block::new(167);
pub const RED_BED_WEST_FOOT: Block = Block::new(168);
pub const GRAY_STAINED_GLASS: Block = Block::new(169);
pub const LIGHT_GRAY_STAINED_GLASS: Block = Block::new(170);
pub const BROWN_STAINED_GLASS: Block = Block::new(171);
pub const TINTED_GLASS: Block = Block::new(172);
pub const OAK_TRAPDOOR: Block = Block::new(173);
pub const BROWN_CONCRETE: Block = Block::new(174);
pub const BLACK_TERRACOTTA: Block = Block::new(175);
pub const BROWN_TERRACOTTA: Block = Block::new(176);
pub const STONE_BRICK_STAIRS: Block = Block::new(177);
pub const MUD_BRICK_STAIRS: Block = Block::new(178);
pub const POLISHED_BLACKSTONE_BRICK_STAIRS: Block = Block::new(179);
pub const BRICK_STAIRS: Block = Block::new(180);
pub const POLISHED_GRANITE_STAIRS: Block = Block::new(181);
pub const END_STONE_BRICK_STAIRS: Block = Block::new(182);
pub const POLISHED_DIORITE_STAIRS: Block = Block::new(183);
pub const SMOOTH_SANDSTONE_STAIRS: Block = Block::new(184);
pub const QUARTZ_STAIRS: Block = Block::new(185);
pub const POLISHED_ANDESITE_STAIRS: Block = Block::new(186);
pub const NETHER_BRICK_STAIRS: Block = Block::new(187);
pub const BARREL: Block = Block::new(188);
pub const FERN: Block = Block::new(189);
pub const COBWEB: Block = Block::new(190);
pub const CHISELLED_BOOKSHELF_NORTH: Block = Block::new(191);
pub const CHISELLED_BOOKSHELF_EAST: Block = Block::new(192);
pub const CHISELLED_BOOKSHELF_SOUTH: Block = Block::new(193);
pub const CHISELLED_BOOKSHELF_WEST: Block = Block::new(194);
pub const CHISELLED_BOOKSHELF: Block = CHISELLED_BOOKSHELF_NORTH;
pub const CHIPPED_ANVIL: Block = Block::new(195);
pub const DAMAGED_ANVIL: Block = Block::new(196);
pub const LARGE_FERN_LOWER: Block = Block::new(197);
pub const LARGE_FERN_UPPER: Block = Block::new(198);
pub const CHAIN: Block = Block::new(199);
pub const END_ROD: Block = Block::new(200);
pub const LIGHTNING_ROD: Block = Block::new(201);
pub const GOLD_BLOCK: Block = Block::new(202);
pub const SEA_LANTERN: Block = Block::new(203);
pub const ORANGE_CONCRETE: Block = Block::new(204);
pub const ORANGE_WOOL: Block = Block::new(205);
pub const BLUE_WOOL: Block = Block::new(206);
pub const GREEN_CONCRETE: Block = Block::new(207);
pub const BRICK_WALL: Block = Block::new(208);
pub const REDSTONE_BLOCK: Block = Block::new(209);
pub const CHAIN_X: Block = Block::new(210);
pub const CHAIN_Z: Block = Block::new(211);
pub const SPRUCE_DOOR_LOWER: Block = Block::new(212);
pub const SPRUCE_DOOR_UPPER: Block = Block::new(213);
pub const SMOOTH_STONE_SLAB: Block = Block::new(214);
pub const GLASS_PANE: Block = Block::new(215);
pub const LIGHT_GRAY_TERRACOTTA: Block = Block::new(216);
pub const OAK_SLAB_TOP: Block = Block::new(217);
pub const OAK_DOOR_UPPER: Block = Block::new(218);
pub const DARK_OAK_LOG: Block = Block::new(219);
pub const DARK_OAK_LEAVES: Block = Block::new(220);
pub const JUNGLE_LOG: Block = Block::new(221);
pub const JUNGLE_LEAVES: Block = Block::new(222);
pub const ACACIA_LOG: Block = Block::new(223);
pub const ACACIA_LEAVES: Block = Block::new(224);
pub const SPRUCE_LEAVES: Block = Block::new(225);
pub const CYAN_STAINED_GLASS: Block = Block::new(226);
pub const BLUE_STAINED_GLASS: Block = Block::new(227);
pub const LIGHT_BLUE_STAINED_GLASS: Block = Block::new(228);
pub const DAYLIGHT_DETECTOR: Block = Block::new(229);
pub const RED_STAINED_GLASS: Block = Block::new(230);
pub const YELLOW_STAINED_GLASS: Block = Block::new(231);
pub const PURPLE_STAINED_GLASS: Block = Block::new(232);
pub const ORANGE_STAINED_GLASS: Block = Block::new(233);
pub const MAGENTA_STAINED_GLASS: Block = Block::new(234);
pub const FLOWER_POT: Block = Block::new(235);
pub const OAK_TRAPDOOR_OPEN_NORTH: Block = Block::new(236);
pub const OAK_TRAPDOOR_OPEN_SOUTH: Block = Block::new(237);
pub const OAK_TRAPDOOR_OPEN_EAST: Block = Block::new(238);
pub const OAK_TRAPDOOR_OPEN_WEST: Block = Block::new(239);
pub const QUARTZ_SLAB_TOP: Block = Block::new(240);
pub const DARK_OAK_TRAPDOOR: Block = Block::new(241);
pub const SPRUCE_TRAPDOOR: Block = Block::new(242);
pub const BIRCH_TRAPDOOR: Block = Block::new(243);
pub const MUD_BRICK_SLAB: Block = Block::new(244);
pub const BRICK_SLAB: Block = Block::new(245);
pub const POTTED_RED_TULIP: Block = Block::new(246);
pub const POTTED_DANDELION: Block = Block::new(247);
pub const POTTED_BLUE_ORCHID: Block = Block::new(248);
pub const BLACK_STAINED_GLASS: Block = Block::new(249);
pub const COPPER_GRATE: Block = Block::new(250); // 🚨 BESM-6 Tweak: Cobogó Element para UnB e Fachadas Antigas

// 🚨 BLOCOS ADICIONAIS NECESSÁRIOS PELO SISTEMA
pub const DIORITE_WALL: Block = Block::new(251);
pub const AZALEA_LEAVES: Block = Block::new(252);
pub const SPRUCE_FENCE: Block = Block::new(253);
pub const DARK_OAK_FENCE: Block = Block::new(254);
pub const FLOWERING_AZALEA: Block = Block::new(255);
pub const QUARTZ_SLAB_BOTTOM: Block = Block::new(256);
pub const SPRUCE_SLAB: Block = Block::new(257);
pub const IRON_TRAPDOOR: Block = Block::new(258);
pub const IRON_DOOR: Block = Block::new(259);
pub const OAK_FENCE_GATE: Block = Block::new(260);
pub const LEAVES: Block = Block::new(261);
pub const STONE_STAIRS: Block = Block::new(262);
pub const TALL_GRASS: Block = Block::new(263);
pub const MOSS_CARPET: Block = Block::new(264);
pub const COPPER_ORE: Block = Block::new(265);
pub const GREEN_TERRACOTTA: Block = Block::new(266);
pub const LIGHT_WEIGHTED_PRESSURE_PLATE: Block = Block::new(267);
pub const GRINDSTONE: Block = Block::new(268);
pub const CACTUS: Block = Block::new(269);
pub const LILY_PAD: Block = Block::new(270);
pub const PINK_TULIP: Block = Block::new(271);
pub const ALLIUM: Block = Block::new(272);
pub const RED_TULIP: Block = Block::new(273);
pub const ORANGE_TULIP: Block = Block::new(274);
pub const DARK_OAK_SLAB: Block = Block::new(275);
pub const CYAN_TERRACOTTA: Block = Block::new(276);
pub const ACACIA_FENCE: Block = Block::new(277);
pub const JUNGLE_FENCE: Block = Block::new(278);

#[inline]
pub fn get_stair_block_for_material(material: Block) -> Block {
    match material {
        STONE_BRICKS => STONE_BRICK_STAIRS,
        MUD_BRICKS => MUD_BRICK_STAIRS,
        OAK_PLANKS => OAK_STAIRS,
        POLISHED_ANDESITE => STONE_BRICK_STAIRS,
        SMOOTH_STONE => POLISHED_ANDESITE_STAIRS,
        ANDESITE => STONE_BRICK_STAIRS,
        BLACK_TERRACOTTA => POLISHED_BLACKSTONE_BRICK_STAIRS,
        BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        BLUE_TERRACOTTA => MUD_BRICK_STAIRS,
        BRICK => BRICK_STAIRS,
        BROWN_CONCRETE => MUD_BRICK_STAIRS,
        BROWN_TERRACOTTA => MUD_BRICK_STAIRS,
        DEEPSLATE_BRICKS => STONE_BRICK_STAIRS,
        END_STONE_BRICKS => END_STONE_BRICK_STAIRS,
        GRAY_CONCRETE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        GRAY_TERRACOTTA => MUD_BRICK_STAIRS,
        NETHER_BRICK => NETHER_BRICK_STAIRS,
        POLISHED_BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_BLACKSTONE_BRICKS => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_GRANITE => POLISHED_GRANITE_STAIRS,
        SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        SMOOTH_SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        WHITE_CONCRETE => QUARTZ_STAIRS,
        WHITE_TERRACOTTA => MUD_BRICK_STAIRS,
        _ => STONE_BRICK_STAIRS,
    }
}

pub static WINDOW_VARIATIONS: [Block; 7] = [
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
    WHITE_STAINED_GLASS,
    TINTED_GLASS,
];

pub static RESIDENTIAL_WINDOW_OPTIONS: [Block; 4] = [
    GLASS,
    WHITE_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
];

pub static INSTITUTIONAL_WINDOW_OPTIONS: [Block; 3] =
    [GLASS, WHITE_STAINED_GLASS, LIGHT_GRAY_STAINED_GLASS];

pub static HOSPITALITY_WINDOW_OPTIONS: [Block; 2] = [GLASS, WHITE_STAINED_GLASS];

pub static INDUSTRIAL_WINDOW_OPTIONS: [Block; 4] = [
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
];

pub fn get_window_block_for_building_type(building_type: &str) -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    get_window_block_for_building_type_with_rng(building_type, &mut rng)
}

pub fn get_window_block_for_building_type_with_rng(
    building_type: &str,
    rng: &mut impl rand::Rng,
) -> Block {
    match building_type {
        "residential" | "house" | "apartment" | "apartments" => {
            RESIDENTIAL_WINDOW_OPTIONS[rng.random_range(0..RESIDENTIAL_WINDOW_OPTIONS.len())]
        }
        "hospital" | "school" | "university" => {
            INSTITUTIONAL_WINDOW_OPTIONS[rng.random_range(0..INSTITUTIONAL_WINDOW_OPTIONS.len())]
        }
        "hotel" | "restaurant" => {
            HOSPITALITY_WINDOW_OPTIONS[rng.random_range(0..HOSPITALITY_WINDOW_OPTIONS.len())]
        }
        "industrial" | "warehouse" => {
            INDUSTRIAL_WINDOW_OPTIONS[rng.random_range(0..INDUSTRIAL_WINDOW_OPTIONS.len())]
        }
        _ => WINDOW_VARIATIONS[rng.random_range(0..WINDOW_VARIATIONS.len())],
    }
}

pub static FLOOR_BLOCK_OPTIONS: [Block; 8] = [
    WHITE_CONCRETE,
    GRAY_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    POLISHED_ANDESITE,
    SMOOTH_STONE,
    STONE_BRICKS,
    MUD_BRICKS,
    OAK_PLANKS,
];

pub fn get_random_floor_block() -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    FLOOR_BLOCK_OPTIONS[rng.random_range(0..FLOOR_BLOCK_OPTIONS.len())]
}

pub fn get_floor_block_with_rng(rng: &mut impl rand::Rng) -> Block {
    FLOOR_BLOCK_OPTIONS[rng.random_range(0..FLOOR_BLOCK_OPTIONS.len())]
}

// --- BRASÍLIA DEFINED COLORS & MAPPING ---

static DEFINED_COLORS: &[ColorBlockMapping] = &[
    // 1. Terra Vermelha do Cerrado (Latossolo)
    ((135, 55, 40), &[RED_TERRACOTTA, RED_CONCRETE, COARSE_DIRT]),
    // 2. Telhados Coloniais / Barro (Laranja Fundo/Terracota)
    ((180, 80, 45), &[ORANGE_TERRACOTTA, BRICK, GRANITE]),
    // 3. Vegetação de Cerrado Seco / Áreas Verdes (Grass, Moss e Podzol para áreas secas)
    ((85, 95, 65), &[GRASS_BLOCK, MOSS_BLOCK, PODZOL]),
    // 4. Concreto Monumental (Off-white / Cinza Claro Poeira)
    (
        (185, 185, 180),
        &[POLISHED_ANDESITE, SMOOTH_STONE, LIGHT_GRAY_CONCRETE],
    ),
    // 5. Brutalismo Raiz (Concreto Envelhecido / Fachadas UnB)
    (
        (140, 140, 135),
        &[GRAY_TERRACOTTA, LIGHT_GRAY_TERRACOTTA, STONE],
    ),
    // 6. Asfalto Urbano (Eixos e EPTG)
    (
        (90, 90, 95),
        &[GRAY_CONCRETE, BLACK_CONCRETE, POLISHED_BASALT],
    ),
    // 7. Branco Institucional (Palácios / Catedral)
    (
        (240, 240, 235),
        &[WHITE_CONCRETE, QUARTZ_BLOCK, SMOOTH_QUARTZ],
    ),
    // 8. Casas do DF / Pastel Bege (Típico Residencial Guará/Taguatinga)
    (
        (235, 220, 190),
        &[SMOOTH_SANDSTONE, WHITE_TERRACOTTA, SANDSTONE],
    ),
    // 9. Casas Pastel Amarelo Clássico
    ((240, 230, 150), &[YELLOW_TERRACOTTA, END_STONE_BRICKS]),
    // 10. Ipê Amarelo
    ((255, 210, 0), &[YELLOW_CONCRETE, YELLOW_WOOL]),
    // 11. Ipê Rosa / Roxo
    ((255, 105, 180), &[MAGENTA_CONCRETE]),
    ((160, 32, 240), &[PURPLE_CONCRETE]),
    // --- PADRÕES TÉCNICOS ---
    ((233, 107, 57), &[BRICK, NETHER_BRICK]),
    ((159, 82, 36), &[BROWN_CONCRETE, MUD_BRICKS, BRICK]),
    ((255, 255, 255), &[WHITE_CONCRETE, QUARTZ_BLOCK]),
    ((0, 0, 0), &[BLACK_CONCRETE]),
    ((0, 120, 255), &[LIGHT_BLUE_CONCRETE, BLUE_TERRACOTTA]), // Vidros espelhados/Azul
];

pub fn get_building_wall_block_for_color(color: RGBTuple) -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    let closest_color = DEFINED_COLORS
        .iter()
        .min_by_key(|(defined_color, _)| crate::colors::rgb_distance(&color, defined_color));

    if let Some((_, options)) = closest_color {
        options[rng.random_range(0..options.len())]
    } else {
        get_fallback_building_block()
    }
}

pub fn get_fallback_building_block() -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    let fallback_options = [
        WHITE_CONCRETE,
        WHITE_TERRACOTTA,
        WHITE_TERRACOTTA,
        SMOOTH_SANDSTONE,
        LIGHT_GRAY_CONCRETE,
        LIGHT_GRAY_TERRACOTTA,
        POLISHED_ANDESITE,
        BRICK,
        MUD_BRICKS,
    ];
    fallback_options[rng.random_range(0..fallback_options.len())]
}

pub fn get_castle_wall_block() -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    let castle_wall_options = [
        STONE_BRICKS,
        CHISELED_STONE_BRICKS,
        COBBLESTONE,
        MOSSY_COBBLESTONE,
        DEEPSLATE_BRICKS,
        POLISHED_ANDESITE,
        ANDESITE,
        SMOOTH_STONE,
        BRICK,
    ];
    castle_wall_options[rng.random_range(0..castle_wall_options.len())]
}

// ============================================================================
// 🚨 BESM-6: TRADUTOR SEMÂNTICO DE MATERIAIS AVANÇADOS (Gov-Tier) 🚨
// ============================================================================

/// Recebe um conjunto de tags (como tipo, estilo, material e idade) e retorna o bloco exato.
/// Elimina a dependência exclusiva de cores mapeadas do OSM.
pub fn resolve_advanced_material(tags: &HashMap<String, String>, default_block: Block) -> Block {
    // 1. Verificamos a Era de Construção (Temporal Inference)
    let is_classic =
        if let Some(year_str) = tags.get("start_date").or_else(|| tags.get("year_built")) {
            // Se foi construído antes da inauguração de Brasília (1960) ou é taggeado como histórico
            year_str.parse::<i32>().unwrap_or(2000) < 1960 || tags.contains_key("historic")
        } else {
            tags.contains_key("historic")
        };

    if is_classic {
        // Arquitetura Colonial / Fazendas do Planalto Central pré-JK
        if tags.get("building") == Some(&"church".to_string()) {
            return SMOOTH_SANDSTONE; // Igrejinhas antigas
        }
        return BRICK; // Casas bandeirantes ou galpões velhos
    }

    // 2. Verificamos se há um material de construção explicitamente declarado no GDF ou OSM
    if let Some(material) = tags
        .get("building:material")
        .or_else(|| tags.get("material"))
    {
        let mat_lower = material.to_lowercase();

        if mat_lower.contains("glass") || mat_lower.contains("vidro") {
            return CYAN_STAINED_GLASS; // Prédios empresariais modernos
        } else if mat_lower.contains("brick") || mat_lower.contains("tijolo") {
            return BRICK;
        } else if mat_lower.contains("concrete") || mat_lower.contains("concreto") {
            return POLISHED_ANDESITE; // Modernismo de Niemeyer (Asas e Setores)
        } else if mat_lower.contains("wood") || mat_lower.contains("madeira") {
            return OAK_PLANKS;
        } else if mat_lower.contains("stone") || mat_lower.contains("pedra") {
            return SMOOTH_STONE; // Fundações e muradas
        }
    }

    // 3. Se não houver material explícito, usamos a Arquitetura por Tipologia
    let b_type = tags
        .get("building")
        .map(|s: &String| s.as_str())
        .unwrap_or("yes");

    if b_type == "hospital" || b_type == "clinic" {
        return WHITE_CONCRETE; // Sanitário e limpo
    } else if b_type == "industrial" || b_type == "warehouse" {
        return GRAY_TERRACOTTA; // Brutalismo utilitário (SIA/SCIA)
    } else if b_type == "commercial" || b_type == "office" {
        return LIGHT_GRAY_CONCRETE; // Base corporativa padrão (SCS/SBS)
    } else if b_type == "church" || b_type == "cathedral" {
        return SMOOTH_QUARTZ; // Monumental (Catedral de Brasília)
    } else if b_type == "ruins" {
        return MOSSY_COBBLESTONE;
    }

    // 4. Fallback: Devolve a cor calculada matematicamente pelo colors.rs
    default_block
}
