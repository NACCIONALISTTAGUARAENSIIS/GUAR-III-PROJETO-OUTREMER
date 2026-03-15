use lru::LruCache;
use once_cell::sync::Lazy;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::borrow::Cow; // BESM-6: Zero-alloc efficiency

pub type RGBTuple = (u8, u8, u8);

/// BESM-6 Tweak: Contexto arquitet�nico robusto para gera��o procedimental determin�stica.
/// Expandido para suportar Intelig�ncia Demogr�fica, Identidade Regional e Materiais.
#[derive(Debug, Clone, Default)]
pub struct ColorContext<'a> {
    pub raw_color_tag: Option<&'a str>,
    pub building_type: Option<&'a str>,
    pub roof_material: Option<&'a str>,
    pub wall_material: Option<&'a str>, // ?? Injetado: Infer�ncia por material da parede
    pub element_id: u64,
    pub center_x: i32,
    pub center_z: i32,
    pub is_highway: bool,
    pub is_pipeline: bool,
    pub is_landmark: bool,
    
    // Identidade Arquitet�nica Local
    pub building_area: Option<f64>,  // Ajuda a diferenciar mans�es de casebres
    pub district_seed: u32,          // Define a paleta predominante do bairro
    pub distance_to_center: f64,     // Gradiente de conserva��o (0.0 a 1.0)
}

/// BESM-6 Tweak: Hashing Espacial (Spatial Seed) Aprimorado.
/// Usa dispers�o de bits estilo Murmur3 para evitar padr�es repetitivos 
/// em quarteir�es perfeitamente alinhados (Superquadras).
#[inline(always)]
pub fn spatial_seed(x: i32, z: i32, id: u64) -> u32 {
    let mut h = x.wrapping_mul(0xcc9e2d51u32 as i32) as u32;
    h = h.rotate_left(15).wrapping_mul(0x1b873593);
    h ^= z.wrapping_mul(0x85ebca6bu32 as i32) as u32;
    h = h.rotate_left(13).wrapping_mul(0xc2b2ae35);
    h ^= (id & 0xFFFFFFFF) as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h
}

// ============================================================================
// ?? MATEM�TICA DE CORES HSL (Tier Governamental) ??
// ============================================================================

#[inline(always)]
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r_f = r as f32 / 255.0;
    let g_f = g as f32 / 255.0;
    let b_f = b as f32 / 255.0;

    let max = r_f.max(g_f).max(b_f);
    let min = r_f.min(g_f).min(b_f);
    let delta = max - min;

    let l = (max + min) / 2.0;
    let s = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * l - 1.0).abs()) };
    
    let h = if delta == 0.0 {
        0.0
    } else if max == r_f {
        60.0 * (((g_f - b_f) / delta) % 6.0)
    } else if max == g_f {
        60.0 * (((b_f - r_f) / delta) + 2.0)
    } else {
        60.0 * (((r_f - g_f) / delta) + 4.0)
    };

    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, s, l)
}

#[inline(always)]
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> RGBTuple {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r + m) * 255.0).clamp(0.0, 255.0).round() as u8,
        ((g + m) * 255.0).clamp(0.0, 255.0).round() as u8,
        ((b + m) * 255.0).clamp(0.0, 255.0).round() as u8,
    )
}

/// Aplica desgaste urbano/ambiental (Weathering) na cor via espa�o HSL.
/// Resultado incrivelmente mais org�nico e natural, sem "acinzentar" artificialmente.
#[inline(always)]
pub fn apply_weathering(rgb: RGBTuple, seed: u32, is_west_facing: bool, distance: f64) -> RGBTuple {
    let (mut h, mut s, mut l) = rgb_to_hsl(rgb.0, rgb.1, rgb.2);
    
    let is_faded_by_sun = seed % 2 == 0 || is_west_facing; 
    
    // BESM-6 Tweak: Micro-variation injetado diretamente na base para quebrar uniformidade perfeitamente lisa
    let micro_noise = (seed % 5) as f32 / 255.0; 
    let base_variation = ((seed % 12) as f32 / 100.0) + micro_noise; 
    
    let conservation_modifier = 1.0 + (distance * 1.5).min(2.0) as f32; 
    let variation = base_variation * conservation_modifier;

    let sun_multiplier = if is_west_facing { 1.5 } else { 1.0 };
    
    // Poeira de terra vermelha (Hue = ~15 graus)
    let red_dust_intensity = (((seed / 7) % 6) as f32 * conservation_modifier) / 100.0;

    if is_faded_by_sun {
        l = (l + variation * sun_multiplier).clamp(0.0, 1.0);
        s = (s - variation * 0.5).clamp(0.0, 1.0);
    } else {
        l = (l - variation).clamp(0.0, 1.0);
        s = (s - variation * 0.3).clamp(0.0, 1.0);
    }

    // BESM-6 Tweak: Hue Wrap Seguro e Satura��o amortecida
    if red_dust_intensity > 0.0 {
        h = ((h * (1.0 - red_dust_intensity)) + (15.0 * red_dust_intensity)) % 360.0;
        s = (s + red_dust_intensity * 0.5).clamp(0.0, 1.0); // Clamp ap�s multiplicar para n�o explodir
    }

    hsl_to_rgb(h, s, l)
}

/// ?? O RESOLVEDOR MESTRE ??
/// Pega o contexto do elemento e cospe uma cor realista e desgastada.
pub fn resolve_wall_color(ctx: &ColorContext) -> RGBTuple {
    let seed = spatial_seed(ctx.center_x, ctx.center_z, ctx.element_id);
    let is_west_facing = ctx.center_x % 2 == 0; 

    // 0. Prote��o Mestre: Landmarks (Garante M�rmore/Branco Imaculado sem sujeira para Pal�cios)
    if ctx.is_landmark {
        return (255, 255, 255); 
    }

    // 1. Tratamento para Vias e Infraestrutura (WFS / Highways)
    if ctx.is_highway {
        let base = match seed % 4 {
            0 => (80, 80, 85),  // Asfalto novo
            1 => (110, 110, 110), // Asfalto desgastado
            _ => (90, 90, 95),  // Asfalto padr�o
        };
        return apply_weathering(base, seed, false, ctx.distance_to_center);
    }
    
    if ctx.is_pipeline {
        let base = match seed % 3 {
            0 => (160, 82, 45), // Ferrugem escura
            1 => (120, 120, 125), // Concreto armado sujo
            _ => (180, 100, 50), // Cobre oxidado
        };
        return apply_weathering(base, seed, false, ctx.distance_to_center);
    }

    // 2. Prioridade M�xima: O dado bruto tem uma cor exata mapeada? (Com parsing composto via Token)
    if let Some(text) = ctx.raw_color_tag {
        if let Some(rgb) = color_text_to_rgb_tuple(text) {
            return apply_weathering(rgb, seed, is_west_facing, ctx.distance_to_center);
        }
    }

    // 2.5 Infer�ncia por Material da Parede
    if let Some(mat) = ctx.wall_material {
        if let Some(rgb) = semantic_material_to_rgb_tuple(mat) {
            return apply_weathering(rgb, seed, is_west_facing, ctx.distance_to_center);
        }
    }

    // 3. Inteligencia Probabilistica e Regional (Arquitetura por Bairro e �rea)
    let btype = ctx.building_type.unwrap_or("yes");
    let district_bias = ctx.district_seed % 3;

    let base_color = if btype.eq_ignore_ascii_case("residential") 
        || btype.eq_ignore_ascii_case("house") 
        || btype.eq_ignore_ascii_case("apartments") 
        || btype.eq_ignore_ascii_case("detached") 
        || btype.eq_ignore_ascii_case("terrace") 
    {
        if let Some(area) = ctx.building_area {
            if area > 800.0 {
                match seed % 3 {
                    0 => (240, 240, 235), // Branco Vidro
                    1 => (185, 185, 180), // Concreto Polido
                    _ => (235, 220, 190), // Creme Neutro
                }
            } else if area < 60.0 {
                match seed % 4 {
                    0 => (150, 70, 50),   // Tijolo � vista aparente
                    1 => (200, 180, 160), // Reboco r�stico
                    2 => (240, 230, 150), // Amarelo descascado
                    _ => (235, 220, 190), // Bege sujo
                }
            } else {
                apply_district_bias(seed, district_bias)
            }
        } else {
            apply_district_bias(seed, district_bias)
        }
    } else if btype.eq_ignore_ascii_case("industrial") || btype.eq_ignore_ascii_case("warehouse") || btype.eq_ignore_ascii_case("manufacture") {
        match seed % 3 {
            0 => (140, 140, 135), // Concreto encardido
            1 => (160, 160, 165), // Zinco/Metal sujo
            _ => (185, 185, 180), // Cimento base
        }
    } else if btype.eq_ignore_ascii_case("civic") || btype.eq_ignore_ascii_case("government") || btype.eq_ignore_ascii_case("public") || btype.eq_ignore_ascii_case("hospital") {
        match seed % 5 {
            0 => (185, 185, 180), // Concreto Monumental Bruto
            _ => (240, 240, 235), // Branco Institucional
        }
    } else if btype.eq_ignore_ascii_case("commercial") || btype.eq_ignore_ascii_case("retail") {
        match seed % 4 {
            0 => (140, 140, 135), // Concreto Pintado
            1 => (0, 120, 255),   // Fachada de Vidro (Azul)
            2 => (240, 240, 235), // Branco Comercial
            _ => (235, 220, 190), // Bege Comercial
        }
    } else {
        apply_district_bias(seed, district_bias)
    };

    apply_weathering(base_color, seed, is_west_facing, ctx.distance_to_center)
}

/// Helpers para Identidade Regional (Bias de Cores)
fn apply_district_bias(seed: u32, bias: u32) -> RGBTuple {
    match bias {
        0 => { 
            match seed % 10 {
                0..=6 => (235, 220, 190),  // Bege
                7..=8 => (240, 240, 235),  // Branco
                _ => (150, 70, 50),        // Tijolo
            }
        },
        1 => { 
            match seed % 10 {
                0..=5 => (240, 240, 235),  // Branco
                6..=8 => (185, 185, 180),  // Concreto Monumental
                _ => (235, 220, 190),      // Bege
            }
        },
        _ => { 
            match seed % 10 {
                0..=3 => (240, 230, 150),  // Amarelo Pastel
                4..=6 => (150, 70, 50),    // Tijolo
                7..=8 => (200, 180, 160),  // Reboco
                _ => (235, 220, 190),      // Bege
            }
        }
    }
}

/// O RESOLVEDOR DE TELHADOS
pub fn resolve_roof_color(ctx: &ColorContext) -> RGBTuple {
    let seed = spatial_seed(ctx.center_x, ctx.center_z, ctx.element_id) ^ 0x9E3779B9; 

    if let Some(mat) = ctx.roof_material {
        let mut has_tile = false;
        let mut has_metal = false;
        let mut has_concrete = false;

        for token in mat.split(|c: char| !c.is_alphanumeric()) {
            if token.eq_ignore_ascii_case("tile") || token.eq_ignore_ascii_case("telha") || token.eq_ignore_ascii_case("clay") || token.eq_ignore_ascii_case("ceramica") {
                has_tile = true;
            }
            if token.eq_ignore_ascii_case("metal") || token.eq_ignore_ascii_case("zinco") {
                has_metal = true;
            }
            if token.eq_ignore_ascii_case("concrete") || token.eq_ignore_ascii_case("laje") {
                has_concrete = true;
            }
        }

        if has_tile { return apply_weathering((180, 80, 45), seed, false, ctx.distance_to_center); }
        if has_metal { return apply_weathering((160, 160, 165), seed, false, ctx.distance_to_center); }
        if has_concrete { return apply_weathering((140, 140, 135), seed, false, ctx.distance_to_center); }
    }

    if let Some(text) = ctx.raw_color_tag {
        if let Some(rgb) = color_text_to_rgb_tuple(text) {
            return apply_weathering(rgb, seed, false, ctx.distance_to_center);
        }
    }

    let btype = ctx.building_type.unwrap_or("yes");
    let base_color = if btype.eq_ignore_ascii_case("residential") || btype.eq_ignore_ascii_case("house") || btype.eq_ignore_ascii_case("detached") {
        let is_small = ctx.building_area.unwrap_or(100.0) < 150.0;
        if is_small {
            if seed % 10 < 8 { (180, 80, 45) } else { (160, 160, 165) }
        } else {
            if seed % 2 == 0 { (140, 140, 135) } else { (180, 80, 45) }
        }
    } else if btype.eq_ignore_ascii_case("industrial") || btype.eq_ignore_ascii_case("warehouse") {
        (160, 160, 165) // Zinco
    } else if btype.eq_ignore_ascii_case("civic") || btype.eq_ignore_ascii_case("government") || btype.eq_ignore_ascii_case("commercial") || btype.eq_ignore_ascii_case("apartments") {
        if seed % 2 == 0 { (140, 140, 135) } else { (185, 185, 180) } // Laje Plana
    } else {
        if seed % 2 == 0 { (180, 80, 45) } else { (140, 140, 135) }
    };

    apply_weathering(base_color, seed, false, ctx.distance_to_center)
}

/// LRU Cache Seguro Limitado a 256 Entradas.
static COLOR_CACHE: Lazy<Mutex<LruCache<String, RGBTuple>>> = Lazy::new(|| {
    Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap()))
});

/// Pipeline mestre de cores originais. Extrai, limpa, entende semanticamente usando tokeniza��o.
pub fn color_text_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    // TWEAK BESM-6: Substitui hifens por espa�os ANTES do parsing para n�o quebrar 
    // tags compostas do OSM (ex: "dark-red" vira "dark red") - Cr�tica 9.2
    let text_norm = text.replace('-', " ");
    let clean_text = text_norm.trim().to_ascii_lowercase();

    if let Ok(mut cache) = COLOR_CACHE.lock() {
        if let Some(&rgb) = cache.get(&clean_text) {
            return Some(rgb);
        }
    }

    let mut hex_buffer = String::new();
    
    // TWEAK BESM-6: Usando Cow (Clone-on-Write) ou refer�ncias diretas (Cr�tica 9.1).
    // Evita a aloca��o pesada de mem�ria ao parsear milhares de cores.
    let mut parse_text: Cow<str> = Cow::Borrowed(&clean_text);
    
    if !parse_text.starts_with('#') && parse_text.chars().all(|c| c.is_ascii_hexdigit()) {
        if parse_text.len() == 3 || parse_text.len() == 6 || parse_text.len() == 8 {
            hex_buffer.push('#');
            hex_buffer.push_str(&parse_text);
            parse_text = Cow::Owned(hex_buffer);
        }
    }

    let mut modifier_l = 0.0;
    let mut modifier_s = 0.0;
    
    let mut base_rgb = None;
    
    for token in parse_text.split(|c: char| !c.is_alphanumeric()) {
        match token {
            "light" | "pale" | "claro" => { modifier_l += 0.15; modifier_s -= 0.05; },
            "dark" | "escuro" => { modifier_l -= 0.15; modifier_s += 0.05; },
            "dirty" | "sujo" => { modifier_l -= 0.10; modifier_s -= 0.15; },
            "bright" | "vivid" => { modifier_l += 0.05; modifier_s += 0.20; },
            _ => {
                if base_rgb.is_none() {
                    base_rgb = exact_color_name_to_rgb_tuple(token)
                        .or_else(|| semantic_material_to_rgb_tuple(token));
                }
            }
        }
    }

    let mut result = full_hex_color_to_rgb_tuple(&parse_text)
        .or_else(|| short_hex_color_to_rgb_tuple(&parse_text))
        .or(base_rgb);

    if let Some(rgb) = result {
        if modifier_l != 0.0 || modifier_s != 0.0 {
            let (h, s, l) = rgb_to_hsl(rgb.0, rgb.1, rgb.2);
            let new_l = (l + modifier_l).clamp(0.0, 1.0);
            let new_s = (s + modifier_s).clamp(0.0, 1.0);
            result = Some(hsl_to_rgb(h, new_s, new_l));
        }
    }

    if let (Some(rgb), Ok(mut cache)) = (result, COLOR_CACHE.lock()) {
        cache.put(clean_text, rgb);
    }

    result
}

/// Aplica uma leve "sujeira" org�nica � cor (Mantida para Retrocompatibilidade).
pub fn apply_micro_variation(rgb: RGBTuple, seed: u32) -> RGBTuple {
    let v = (seed % 7) as i8 - 3;
    let r = (rgb.0 as i16 + v as i16).clamp(0, 255) as u8;
    let g = (rgb.1 as i16 + v as i16).clamp(0, 255) as u8;
    let b = (rgb.2 as i16 + v as i16).clamp(0, 255) as u8;
    (r, g, b)
}

fn full_hex_color_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    if !text.starts_with('#') {
        return None;
    }
    let is_valid_hex = text.chars().skip(1).all(|c| c.is_ascii_hexdigit());
    if is_valid_hex && (text.len() == 7 || text.len() == 9) {
        let r: u8 = u8::from_str_radix(&text[1..3], 16).ok()?;
        let g: u8 = u8::from_str_radix(&text[3..5], 16).ok()?;
        let b: u8 = u8::from_str_radix(&text[5..7], 16).ok()?;
        return Some((r, g, b));
    }
    None
}

fn short_hex_color_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    if !text.starts_with('#') {
        return None;
    }
    let is_valid_hex = text.chars().skip(1).all(|c| c.is_ascii_hexdigit());
    if is_valid_hex && (text.len() == 4 || text.len() == 5) {
        let r: u8 = u8::from_str_radix(&text[1..2], 16).ok()?;
        let r: u8 = r | (r << 4);
        let g: u8 = u8::from_str_radix(&text[2..3], 16).ok()?;
        let g: u8 = g | (g << 4);
        let b: u8 = u8::from_str_radix(&text[3..4], 16).ok()?;
        let b: u8 = b | (b << 4);
        return Some((r, g, b));
    }
    None
}

fn semantic_material_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    match text {
        "brick" | "tijolo" | "alvenaria" => Some((150, 70, 50)),
        "terracotta" | "clay" | "barro" | "tile" | "telha" | "ceramica" => Some((180, 80, 45)),
        "sand" | "areia" | "plaster" | "reboco" | "stucco" | "sandstone" => Some((230, 220, 170)),
        "concrete" | "concreto" | "cement" | "cimento" | "limestone" => Some((185, 185, 180)),
        "stone" | "pedra" | "granite" => Some((150, 150, 150)),
        "glass" | "vidro" | "mirror" | "espelhado" => Some((140, 180, 220)),
        "slate" | "lousa" | "zinc" | "zinco" | "amianto" | "fibrocimento" | "metal" | "iron" | "steel" | "a�o" => Some((160, 160, 165)),
        "wood" | "madeira" | "timber" => Some((135, 85, 55)),
        "asphalt" | "asfalto" | "tarmac" | "piche" | "pavimento" => Some((90, 90, 95)),
        "marble" | "marmore" => Some((240, 240, 240)),
        _ => None,
    }
}

fn exact_color_name_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    let color_to_match = text.split_whitespace().last().unwrap_or(text);

    Some(match color_to_match {
        "brown" | "saddlebrown" => (135, 55, 40),
        "chocolate" | "sienna" => (160, 82, 45),
        "orange" | "coral" => (180, 80, 45),
        "green" | "forestgreen" => (85, 95, 65),
        "olive" | "olivedrab" => (128, 128, 0),
        "gray" | "grey" | "cinza" => (90, 90, 95),
        "silver" | "prata" => (185, 185, 180),
        "dimgray" | "dimgrey" => (140, 140, 135),
        "white" | "snow" | "cream" | "branco" => (240, 240, 235),
        "beige" | "antiquewhite" | "bege" => (235, 220, 190),
        "ivory" | "cornsilk" | "wheat" => (240, 230, 150),
        "blue" | "royalblue" | "dodgerblue" | "azul" => (0, 120, 255),
        "navy" | "midnightblue" => (25, 35, 60),
        "skyblue" => (110, 170, 230),
        "yellow" | "gold" | "amarelo" => (255, 210, 0),
        "pink" | "hotpink" | "rosa" => (255, 105, 180),
        "magenta" | "purple" | "roxo" => (160, 32, 240),
        "black" | "preto" => (25, 25, 25),
        "red" | "vermelho" => (190, 35, 35),
        "lime" => (50, 205, 50),
        "cyan" | "aqua" => (0, 255, 255),
        "teal" => (0, 128, 128),
        "fuchsia" => (255, 0, 255),
        "maroon" => (128, 0, 0),
        
        // ?? Adi��es Cr�ticas do Padr�o CSS (Arquitetura)
        "tan" => (210, 180, 140),
        "khaki" => (240, 230, 140),
        "salmon" => (250, 128, 114),
        "tomato" => (255, 99, 71),
        "indianred" => (205, 92, 92),
        "peru" => (205, 133, 63),
        "burlywood" => (222, 184, 135),
        "darkkhaki" => (189, 183, 107),
        "darkgreen" => (0, 100, 0),
        "seagreen" => (46, 139, 87),
        "steelblue" => (70, 130, 180),
        "cadetblue" => (95, 158, 160),
        "slategray" | "slategrey" => (112, 128, 144),
        "lightslategray" | "lightslategrey" => (119, 136, 153),
        "gainsboro" => (220, 220, 220),
        "whitesmoke" => (245, 245, 245),
        
        _ => return None,
    })
}

/// BESM-6: Dist�ncia Euclidiana Quadrada. 
#[inline(always)]
pub fn rgb_distance(from: &RGBTuple, to: &RGBTuple) -> u32 {
    let dr = from.0 as i32 - to.0 as i32;
    let dg = from.1 as i32 - to.1 as i32;
    let db = from.2 as i32 - to.2 as i32;
    let distance = (dr * dr) + (dg * dg) + (db * db);
    distance as u32
}