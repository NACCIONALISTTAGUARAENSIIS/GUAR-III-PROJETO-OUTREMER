// Top-down world map renderer for GUI preview.
//
// Generates a PNG image of the generated world,
// showing the topmost visible block at each position.
// ?? BESM-6 TWEAK: Prote��o Din�mica de Mem�ria (Downsampling) contra Mapas Governamentais Massivos.
//
// 🚨 Chamado por `main::run_generation_pipeline` só dentro de `#[cfg(feature = "gui")]`
// (o preview só faz sentido para quem tem uma janela pra exibi-lo). No build sem GUI
// este módulo inteiro fica genuinamente inalcançável — não uma lacuna de conexão.
#![cfg_attr(not(feature = "gui"), allow(dead_code))]

use fastanvil::Region;
use fastnbt::{from_bytes, Value};
use fnv::FnvHashMap;
use image::{Rgb, RgbImage};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

/// Limite regido da dimens�o m�xima da imagem na RAM (Largura ou Altura)
/// 32768 x 32768 x 3 bytes (RGB) = ~3 Gigabytes de RAM apenas para a foto.
const MAX_IMAGE_DIMENSION: u32 = 32768;

/// Pre-computed block colors for fast lookup
static BLOCK_COLORS: Lazy<FnvHashMap<&'static str, Rgb<u8>>> = Lazy::new(get_block_colors);

/// Renders a top-down view of the generated Minecraft world.
/// Returns the path to the saved image file.
pub fn render_world_map(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Result<std::path::PathBuf, String> {
    let raw_width = (max_x - min_x + 1).max(0) as u32;
    let raw_height = (max_z - min_z + 1).max(0) as u32;

    if raw_width == 0 || raw_height == 0 {
        return Err("Invalid world bounds".to_string());
    }

    // ?? BESM-6 Tweak: Auto-Downsampling (Prote��o contra OOM do O.S.)
    // Se a imagem passar de ~3GB na RAM (o que � trivial no DF inteiro), reduzimos a escala.
    let mut downsample_factor = 1;
    while raw_width / downsample_factor > MAX_IMAGE_DIMENSION
        || raw_height / downsample_factor > MAX_IMAGE_DIMENSION
    {
        downsample_factor *= 2;
    }

    let width = raw_width / downsample_factor;
    let height = raw_height / downsample_factor;

    println!(
        "[INFO] ?? Iniciando renderiza��o do minimapa: {}x{} pixels (Downsample: {}x)",
        width, height, downsample_factor
    );

    // Use Mutex for thread-safe image access
    let img = Mutex::new(RgbImage::from_pixel(width, height, Rgb([255, 255, 255])));

    // Calculate region range
    let min_region_x = min_x >> 9; // divide by 512 (32 chunks * 16 blocks)
    let max_region_x = max_x >> 9;
    let min_region_z = min_z >> 9;
    let max_region_z = max_z >> 9;

    let region_dir = world_dir.join("region");

    // Collect all region coordinates for parallel processing
    let region_coords: Vec<(i32, i32)> = (min_region_x..=max_region_x)
        .flat_map(|rx| (min_region_z..=max_region_z).map(move |rz| (rx, rz)))
        .collect();

    // Process regions in parallel
    region_coords.par_iter().for_each(|&(region_x, region_z)| {
        let region_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));

        if !region_path.exists() {
            return;
        }

        if let Ok(file) = File::open(&region_path) {
            if let Ok(mut region) = Region::from_stream(file) {
                // Collect all pixels from this region first
                let pixels = render_region_to_pixels(
                    &mut region,
                    region_x,
                    region_z,
                    min_x,
                    min_z,
                    max_x,
                    max_z,
                    downsample_factor, // Repassamos a taxa de escala
                );

                // Then batch-write to image under lock
                if !pixels.is_empty() {
                    let mut img_guard = img.lock().unwrap();
                    for (x, z, color) in pixels {
                        // Verifica limites para n�o crachar a matriz do `image` crate
                        if x < img_guard.width() && z < img_guard.height() {
                            img_guard.put_pixel(x, z, color);
                        }
                    }
                }
            }
        }
    });

    // Save the image
    let output_path = world_dir.join("arnis_world_map.png");
    println!(
        "[INFO] ?? Salvando preview cartogr�fico em: {}",
        output_path.display()
    );

    img.into_inner()
        .unwrap()
        .save(&output_path)
        .map_err(|e| format!("Failed to save map image: {}", e))?;

    Ok(output_path)
}

/// Computa uma grade de altura+cor (topo real de cada coluna) para uma fatia do
/// mundo — o mesmo escaneamento de NBT/paleta que `render_world_map` usa para o
/// minimapa 2D do GUI, mas devolvendo dados crus em vez de gravar um PNG.
///
/// 🚨 BESM-6: Usado pelo `world_viewer` (`--view-world`), que serve esses dados
/// como relevo 3D num navegador — não pelo preview do GUI. Ao contrário do
/// minimapa (uma imagem 2D de cor só), aqui cada célula carrega também a altura
/// (`world_y`) real do bloco, para o navegador desenhar um relevo de verdade
/// (superfície com relevo, não uma foto plana). `downsample_factor` é decidido
/// por quem chama (orçamento de células, não de pixels de imagem).
pub fn compute_heightfield(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    downsample_factor: u32,
) -> Vec<(i32, i32, i32, [u8; 3])> {
    let min_region_x = min_x >> 9;
    let max_region_x = max_x >> 9;
    let min_region_z = min_z >> 9;
    let max_region_z = max_z >> 9;

    let region_dir = world_dir.join("region");

    let region_coords: Vec<(i32, i32)> = (min_region_x..=max_region_x)
        .flat_map(|rx| (min_region_z..=max_region_z).map(move |rz| (rx, rz)))
        .collect();

    region_coords
        .par_iter()
        .flat_map(|&(region_x, region_z)| {
            render_region_to_heightcells(
                region_x,
                region_z,
                &region_dir,
                min_x,
                min_z,
                max_x,
                max_z,
                downsample_factor,
            )
        })
        .collect()
}

/// Espelha `render_region_to_pixels`, mas devolve `(x, z, y, cor)` cru — ver
/// `compute_heightfield`.
#[allow(clippy::too_many_arguments)]
fn render_region_to_heightcells(
    region_x: i32,
    region_z: i32,
    region_dir: &Path,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    downsample_factor: u32,
) -> VoxelList {
    let mut cells = Vec::new();
    let region_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));
    if !region_path.exists() {
        return cells;
    }

    let Ok(file) = File::open(&region_path) else {
        return cells;
    };
    let Ok(mut region) = Region::from_stream(file) else {
        return cells;
    };

    let region_base_x = region_x * 512;
    let region_base_z = region_z * 512;

    for chunk_local_x in 0..32 {
        for chunk_local_z in 0..32 {
            let chunk_base_x = region_base_x + chunk_local_x * 16;
            let chunk_base_z = region_base_z + chunk_local_z * 16;

            if chunk_base_x + 15 < min_x
                || chunk_base_x > max_x
                || chunk_base_z + 15 < min_z
                || chunk_base_z > max_z
            {
                continue;
            }

            let Ok(Some(chunk_data)) =
                region.read_chunk(chunk_local_x as usize, chunk_local_z as usize)
            else {
                continue;
            };

            let chunk: Value = match from_bytes(&chunk_data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let sections = get_sections_from_chunk(&chunk);
            if sections.is_empty() {
                continue;
            }
            let sorted_sections = get_sorted_sections(&sections);
            if sorted_sections.is_empty() {
                continue;
            }

            for local_x in 0..16 {
                for local_z in 0..16 {
                    let world_x = chunk_base_x + local_x;
                    let world_z = chunk_base_z + local_z;

                    if world_x < min_x || world_x > max_x || world_z < min_z || world_z > max_z {
                        continue;
                    }
                    if world_x.rem_euclid(downsample_factor as i32) != 0
                        || world_z.rem_euclid(downsample_factor as i32) != 0
                    {
                        continue;
                    }

                    if let Some((block_name, world_y)) =
                        find_top_block_sorted(&sorted_sections, local_x as usize, local_z as usize)
                    {
                        let short_name =
                            block_name.strip_prefix("minecraft:").unwrap_or(&block_name);
                        let base_color = BLOCK_COLORS
                            .get(short_name)
                            .copied()
                            .unwrap_or_else(|| get_fallback_color(&block_name));
                        let color = apply_elevation_shading(base_color, world_y);
                        cells.push((
                            world_x,
                            world_z,
                            world_y,
                            [color.0[0], color.0[1], color.0[2]],
                        ));
                    }
                }
            }
        }
    }

    cells
}

/// Renders all chunks within a region and returns pixel data
#[allow(clippy::too_many_arguments)]
fn render_region_to_pixels(
    region: &mut Region<File>,
    region_x: i32,
    region_z: i32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    downsample_factor: u32,
) -> Vec<(u32, u32, Rgb<u8>)> {
    let mut pixels = Vec::new();
    let region_base_x = region_x * 512;
    let region_base_z = region_z * 512;

    for chunk_local_x in 0..32 {
        for chunk_local_z in 0..32 {
            let chunk_base_x = region_base_x + chunk_local_x * 16;
            let chunk_base_z = region_base_z + chunk_local_z * 16;

            // Skip chunks outside our bounds
            if chunk_base_x + 15 < min_x
                || chunk_base_x > max_x
                || chunk_base_z + 15 < min_z
                || chunk_base_z > max_z
            {
                continue;
            }

            if let Ok(Some(chunk_data)) =
                region.read_chunk(chunk_local_x as usize, chunk_local_z as usize)
            {
                render_chunk_to_pixels(
                    &chunk_data,
                    &mut pixels,
                    chunk_base_x,
                    chunk_base_z,
                    min_x,
                    min_z,
                    max_x,
                    max_z,
                    downsample_factor,
                );
            }
        }
    }

    pixels
}

/// Renders a single chunk and appends pixel data
#[allow(clippy::too_many_arguments)]
fn render_chunk_to_pixels(
    chunk_data: &[u8],
    pixels: &mut Vec<(u32, u32, Rgb<u8>)>,
    chunk_base_x: i32,
    chunk_base_z: i32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    downsample_factor: u32,
) {
    // Parse chunk NBT - look for Level.sections or sections depending on format
    let chunk: Value = match from_bytes(chunk_data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Try to get sections from the chunk data
    let sections = get_sections_from_chunk(&chunk);
    if sections.is_empty() {
        return;
    }

    // Pre-sort sections by Y (descending) once per chunk, not per column
    let sorted_sections = get_sorted_sections(&sections);
    if sorted_sections.is_empty() {
        return;
    }

    // For each column in the chunk
    for local_x in 0..16 {
        for local_z in 0..16 {
            let world_x = chunk_base_x + local_x;
            let world_z = chunk_base_z + local_z;

            // Skip if outside our bounds
            if world_x < min_x || world_x > max_x || world_z < min_z || world_z > max_z {
                continue;
            }

            // ?? Pulo de Downsampling (Economiza CPU no mapa enorme)
            // Renderizamos apenas 1 pixel a cada X blocos definidos pelo fator.
            if world_x.rem_euclid(downsample_factor as i32) != 0
                || world_z.rem_euclid(downsample_factor as i32) != 0
            {
                continue;
            }

            // Find topmost non-air block using pre-sorted sections
            if let Some((block_name, world_y)) =
                find_top_block_sorted(&sorted_sections, local_x as usize, local_z as usize)
            {
                // Strip minecraft: prefix for lookup
                let short_name = block_name.strip_prefix("minecraft:").unwrap_or(&block_name);

                let base_color = BLOCK_COLORS
                    .get(short_name)
                    .copied()
                    .unwrap_or_else(|| get_fallback_color(&block_name));

                // Apply elevation shading
                let color = apply_elevation_shading(base_color, world_y);

                let img_x = ((world_x - min_x) as u32) / downsample_factor;
                let img_z = ((world_z - min_z) as u32) / downsample_factor;

                pixels.push((img_x, img_z, color));
            }
        }
    }
}

/// Applies elevation-based shading to a color
/// Higher elevations are brighter, lower are darker
#[inline]
fn apply_elevation_shading(color: Rgb<u8>, y: i32) -> Rgb<u8> {
    // Base brightness boost of 10%, plus elevation shading
    // Shading range: -20% darker to +20% brighter (asymmetric, more bright than dark)

    // Normalize Y to a -1.0 to 1.0 range (roughly)
    // y=0 -> -0.5, y=0 -> 0, y=200 -> +1.0
    let normalized = (y as f32 / 100.0).clamp(-1.0, 1.0);

    // Base 10% brightness boost + asymmetric elevation shading
    let elevation_adjust = if normalized >= 0.0 {
        // Above sea level: up to +20% brighter
        normalized * 0.20
    } else {
        // Below sea level: up to -20% darker
        normalized * 0.20
    };

    let multiplier = 1.10 + elevation_adjust;

    Rgb([
        (color.0[0] as f32 * multiplier).clamp(0.0, 255.0) as u8,
        (color.0[1] as f32 * multiplier).clamp(0.0, 255.0) as u8,
        (color.0[2] as f32 * multiplier).clamp(0.0, 255.0) as u8,
    ])
}

/// Extracts sections from chunk data (handles both old and new formats)
fn get_sections_from_chunk(chunk: &Value) -> Vec<&Value> {
    let mut sections = Vec::new();

    // Try new format (1.18+): directly in chunk
    if let Value::Compound(map) = chunk {
        if let Some(Value::List(secs)) = map.get("sections") {
            for sec in secs {
                sections.push(sec);
            }
            return sections;
        }

        // Try via Level wrapper (older format)
        if let Some(Value::Compound(level)) = map.get("Level") {
            if let Some(Value::List(secs)) = level.get("sections") {
                for sec in secs {
                    sections.push(sec);
                }
            }
        }
    }

    sections
}

/// Pre-sorts sections by Y coordinate (descending) - called once per chunk
/// Returns Vec of (section_y, section_value) for Y tracking
fn get_sorted_sections<'a>(sections: &[&'a Value]) -> Vec<(i8, &'a Value)> {
    let mut sorted: Vec<(i8, &Value)> = sections
        .iter()
        .filter_map(|s| {
            if let Value::Compound(map) = s {
                if let Some(Value::Byte(y)) = map.get("Y") {
                    return Some((*y, *s));
                }
            }
            None
        })
        .collect();

    sorted.sort_by_key(|item| std::cmp::Reverse(item.0));
    sorted
}

/// Finds the topmost non-air block using pre-sorted sections
/// Returns (block_name, world_y) where world_y is the actual Y coordinate
fn find_top_block_sorted(
    sorted_sections: &[(i8, &Value)],
    local_x: usize,
    local_z: usize,
) -> Option<(String, i32)> {
    for (section_y, section) in sorted_sections {
        if let Some((block_name, local_y)) = get_block_at_section(section, local_x, local_z) {
            if !is_transparent_block(&block_name) {
                // Calculate world Y: section_y * 16 + local_y
                let world_y = (*section_y as i32) * 16 + local_y as i32;
                return Some((block_name, world_y));
            }
        }
    }

    None
}

/// Gets the topmost non-air block in a section at the given x,z
/// Returns (block_name, local_y) where local_y is 0-15 within the section
fn get_block_at_section(
    section: &Value,
    local_x: usize,
    local_z: usize,
) -> Option<(String, usize)> {
    let section_map = match section {
        Value::Compound(m) => m,
        _ => return None,
    };

    let block_states = match section_map.get("block_states") {
        Some(Value::Compound(bs)) => bs,
        _ => return None,
    };

    let palette = match block_states.get("palette") {
        Some(Value::List(p)) => p,
        _ => return None,
    };

    // If palette has only one block, that's the block for the entire section
    if palette.len() == 1 {
        // Return with local_y=15 (top of section) for single-block sections
        return get_block_name_from_palette(&palette[0]).map(|name| (name, 15));
    }

    let data = match block_states.get("data") {
        Some(Value::LongArray(d)) => d,
        _ => return None,
    };

    // Calculate bits per block
    let bits_per_block = std::cmp::max(4, (palette.len() as f64).log2().ceil() as usize);
    let blocks_per_long = 64 / bits_per_block;
    let mask = (1u64 << bits_per_block) - 1;

    // Search from top (y=15) to bottom (y=0) within this section
    for local_y in (0..16).rev() {
        let block_index = local_y * 256 + local_z * 16 + local_x;
        let long_index = block_index / blocks_per_long;
        let bit_offset = (block_index % blocks_per_long) * bits_per_block;

        if long_index >= data.len() {
            continue;
        }

        let palette_index = ((data[long_index] as u64 >> bit_offset) & mask) as usize;

        if palette_index < palette.len() {
            if let Some(name) = get_block_name_from_palette(&palette[palette_index]) {
                if !is_transparent_block(&name) {
                    return Some((name, local_y));
                }
            }
        }
    }

    None
}

/// Decodifica o bloco EXATO em `(local_x, local_y, local_z)` dentro de uma
/// section — ao contrário de `get_block_at_section` (que escaneia de cima
/// pra baixo procurando só o topo visível), isto devolve o bloco em
/// qualquer altura específica. Necessário para `compute_voxel_crop`: um
/// recorte voxel completo precisa de TODO bloco (paredes, interiores,
/// fundações), não só a superfície de cima que basta pro minimapa 2D.
fn get_block_at_exact_y(
    section: &Value,
    local_x: usize,
    local_y: usize,
    local_z: usize,
) -> Option<String> {
    let section_map = match section {
        Value::Compound(m) => m,
        _ => return None,
    };

    let block_states = match section_map.get("block_states") {
        Some(Value::Compound(bs)) => bs,
        _ => return None,
    };

    let palette = match block_states.get("palette") {
        Some(Value::List(p)) => p,
        _ => return None,
    };

    if palette.len() == 1 {
        return get_block_name_from_palette(&palette[0]);
    }

    let data = match block_states.get("data") {
        Some(Value::LongArray(d)) => d,
        _ => return None,
    };

    let bits_per_block = std::cmp::max(4, (palette.len() as f64).log2().ceil() as usize);
    let blocks_per_long = 64 / bits_per_block;
    let mask = (1u64 << bits_per_block) - 1;

    let block_index = local_y * 256 + local_z * 16 + local_x;
    let long_index = block_index / blocks_per_long;
    let bit_offset = (block_index % blocks_per_long) * bits_per_block;

    if long_index >= data.len() {
        return None;
    }

    let palette_index = ((data[long_index] as u64 >> bit_offset) & mask) as usize;
    if palette_index < palette.len() {
        get_block_name_from_palette(&palette[palette_index])
    } else {
        None
    }
}

/// Teto de segurança pra `compute_voxel_crop`, mesmo espírito do
/// `TARGET_CELLS` do `world_viewer`: um recorte voxel completo (todo bloco,
/// não só o topo) cresce em O(dx*dy*dz), então sem um limite um pedido
/// grande demais (ex.: o Guará I+II inteiro) estouraria a RAM tentando
/// guardar uma grade 3D densa. Calibrado pra caber uma área generosa pra
/// inspeção de perto (ex.: ~300×300 blocos por ~65 de altura).
const MAX_VOXEL_CELLS: u64 = 6_000_000;

/// `(x, y, z, [r, g, b])` por voxel exposto — alias só pra clippy não
/// reclamar de "tipo complexo demais" no `Result<Vec<...>, String>` de
/// `compute_voxel_crop`.
type VoxelList = Vec<(i32, i32, i32, [u8; 3])>;

/// Extrai TODO bloco não-transparente dentro do recorte `[min_x,max_x] ×
/// [min_y,max_y] × [min_z,max_z]`, descartando (por economia de banda/RAM no
/// navegador) qualquer bloco totalmente cercado por outros blocos opacos —
/// só emite blocos com pelo menos uma face exposta a ar/transparente (ou à
/// borda do recorte). Ao contrário de `compute_heightfield` (que só vê o
/// topo de cada coluna), isto devolve o mundo real em blocos — paredes,
/// interiores, terreno — para inspeção de perto no estilo de um render
/// voxel completo.
pub(crate) fn compute_voxel_crop(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    min_y: i32,
    max_y: i32,
) -> Result<VoxelList, String> {
    if min_x > max_x || min_z > max_z || min_y > max_y {
        return Err("recorte inválido: cada eixo precisa ter min <= max".to_string());
    }

    let w = (max_x - min_x + 1) as usize;
    let d = (max_z - min_z + 1) as usize;
    let h = (max_y - min_y + 1) as usize;
    let total_cells = (w as u64) * (d as u64) * (h as u64);
    if total_cells > MAX_VOXEL_CELLS {
        return Err(format!(
            "recorte grande demais para um render voxel completo: {w}×{h}×{d} = {total_cells} \
             células (limite {MAX_VOXEL_CELLS}). Reduza a área (--crop) ou o intervalo de altura \
             (--min-y/--max-y) — para uma visão geral do mundo inteiro use --view-world em vez disso."
        ));
    }

    // Paleta local (índice u16, 0 = vazio/ar) em vez de guardar o nome do
    // bloco por célula — a mesma coluna repete os mesmos poucos nomes de
    // bloco milhares de vezes, então internar economiza RAM real num
    // recorte de milhões de células.
    let mut palette_names: Vec<String> = Vec::new();
    let mut palette_lookup: FnvHashMap<String, u16> = FnvHashMap::default();
    let mut grid: Vec<u16> = vec![0u16; w * d * h];

    let min_region_x = min_x >> 9;
    let max_region_x = max_x >> 9;
    let min_region_z = min_z >> 9;
    let max_region_z = max_z >> 9;
    let region_dir = world_dir.join("region");

    for region_x in min_region_x..=max_region_x {
        for region_z in min_region_z..=max_region_z {
            let region_path = region_dir.join(format!("r.{region_x}.{region_z}.mca"));
            if !region_path.exists() {
                continue;
            }
            let Ok(file) = File::open(&region_path) else {
                continue;
            };
            let Ok(mut region) = Region::from_stream(file) else {
                continue;
            };

            let region_base_x = region_x * 512;
            let region_base_z = region_z * 512;

            for chunk_local_x in 0..32 {
                for chunk_local_z in 0..32 {
                    let chunk_base_x = region_base_x + chunk_local_x * 16;
                    let chunk_base_z = region_base_z + chunk_local_z * 16;
                    if chunk_base_x + 15 < min_x
                        || chunk_base_x > max_x
                        || chunk_base_z + 15 < min_z
                        || chunk_base_z > max_z
                    {
                        continue;
                    }

                    let Ok(Some(chunk_data)) =
                        region.read_chunk(chunk_local_x as usize, chunk_local_z as usize)
                    else {
                        continue;
                    };
                    let chunk: Value = match from_bytes(&chunk_data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let sections = get_sections_from_chunk(&chunk);

                    for section in &sections {
                        let Value::Compound(section_map) = section else {
                            continue;
                        };
                        let Some(Value::Byte(section_y)) = section_map.get("Y") else {
                            continue;
                        };
                        let section_min_y = (*section_y as i32) * 16;
                        let section_max_y = section_min_y + 15;
                        if section_max_y < min_y || section_min_y > max_y {
                            continue;
                        }

                        for local_x in 0..16i32 {
                            let world_x = chunk_base_x + local_x;
                            if world_x < min_x || world_x > max_x {
                                continue;
                            }
                            for local_z in 0..16i32 {
                                let world_z = chunk_base_z + local_z;
                                if world_z < min_z || world_z > max_z {
                                    continue;
                                }
                                for local_y in 0..16i32 {
                                    let world_y = section_min_y + local_y;
                                    if world_y < min_y || world_y > max_y {
                                        continue;
                                    }
                                    let Some(name) = get_block_at_exact_y(
                                        section,
                                        local_x as usize,
                                        local_y as usize,
                                        local_z as usize,
                                    ) else {
                                        continue;
                                    };
                                    if is_transparent_block(&name) {
                                        continue;
                                    }

                                    let palette_index =
                                        *palette_lookup.entry(name.clone()).or_insert_with(|| {
                                            palette_names.push(name);
                                            // 0 é reservado pra "vazio"; blocos reais começam em 1.
                                            palette_names.len() as u16
                                        });

                                    let xi = (world_x - min_x) as usize;
                                    let zi = (world_z - min_z) as usize;
                                    let yi = (world_y - min_y) as usize;
                                    grid[(yi * d + zi) * w + xi] = palette_index;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(extract_exposed_voxels(
        &grid,
        w,
        d,
        h,
        min_x,
        min_y,
        min_z,
        &palette_names,
    ))
}

/// Passe de exposição: um bloco só entra no resultado se tiver pelo menos
/// um dos 6 vizinhos vazio (ar/transparente, já descartado da grade) ou
/// fora do recorte — blocos totalmente cercados por outros blocos opacos
/// nunca aparecem visualmente e só inflariam o payload. Extraída como
/// função pura (sobre a grade já montada) pra poder ser testada sem
/// depender de arquivos de mundo reais.
#[allow(clippy::too_many_arguments)]
fn extract_exposed_voxels(
    grid: &[u16],
    w: usize,
    d: usize,
    h: usize,
    min_x: i32,
    min_y: i32,
    min_z: i32,
    palette_names: &[String],
) -> VoxelList {
    let is_empty = |xi: i64, yi: i64, zi: i64| -> bool {
        if xi < 0 || yi < 0 || zi < 0 || xi as usize >= w || yi as usize >= h || zi as usize >= d {
            return true;
        }
        grid[(yi as usize * d + zi as usize) * w + xi as usize] == 0
    };

    let mut voxels = Vec::new();
    for yi in 0..h {
        for zi in 0..d {
            for xi in 0..w {
                let palette_index = grid[(yi * d + zi) * w + xi];
                if palette_index == 0 {
                    continue;
                }
                let (xi64, yi64, zi64) = (xi as i64, yi as i64, zi as i64);
                let exposed = is_empty(xi64 - 1, yi64, zi64)
                    || is_empty(xi64 + 1, yi64, zi64)
                    || is_empty(xi64, yi64 - 1, zi64)
                    || is_empty(xi64, yi64 + 1, zi64)
                    || is_empty(xi64, yi64, zi64 - 1)
                    || is_empty(xi64, yi64, zi64 + 1);
                if !exposed {
                    continue;
                }

                let name = &palette_names[(palette_index - 1) as usize];
                let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
                let color = BLOCK_COLORS
                    .get(short_name)
                    .copied()
                    .unwrap_or_else(|| get_fallback_color(name));
                voxels.push((
                    min_x + xi as i32,
                    min_y + yi as i32,
                    min_z + zi as i32,
                    [color.0[0], color.0[1], color.0[2]],
                ));
            }
        }
    }
    voxels
}

/// Extracts block name from a palette entry
fn get_block_name_from_palette(entry: &Value) -> Option<String> {
    if let Value::Compound(map) = entry {
        if let Some(Value::String(name)) = map.get("Name") {
            return Some(name.clone());
        }
    }
    None
}

/// Checks if a block should be considered transparent (look through it)
fn is_transparent_block(name: &str) -> bool {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(
        short_name,
        "air"
            | "cave_air"
            | "void_air"
            | "glass"
            | "glass_pane"
            | "white_stained_glass"
            | "gray_stained_glass"
            | "light_gray_stained_glass"
            | "brown_stained_glass"
            | "tinted_glass"
            | "barrier"
            | "light"
            | "short_grass"
            | "tall_grass"
            | "dead_bush"
            | "poppy"
            | "dandelion"
            | "blue_orchid"
            | "azure_bluet"
            | "iron_bars"
            | "ladder"
            | "scaffolding"
            | "rail"
            | "powered_rail"
            | "detector_rail"
            | "activator_rail"
    )
}

/// Returns a fallback color based on block name patterns
fn get_fallback_color(name: &str) -> Rgb<u8> {
    // Try to guess color from name
    if name.contains("stone") || name.contains("cobble") || name.contains("andesite") {
        return Rgb([128, 128, 128]);
    }
    if name.contains("dirt") || name.contains("mud") {
        return Rgb([139, 90, 43]);
    }
    if name.contains("sand") {
        return Rgb([219, 211, 160]);
    }
    if name.contains("grass") {
        return Rgb([86, 125, 70]);
    }
    if name.contains("water") {
        return Rgb([59, 86, 165]);
    }
    if name.contains("log") || name.contains("wood") {
        return Rgb([101, 76, 48]);
    }
    if name.contains("leaves") {
        return Rgb([55, 95, 36]);
    }
    if name.contains("planks") {
        return Rgb([162, 130, 78]);
    }
    if name.contains("brick") {
        return Rgb([150, 97, 83]);
    }
    if name.contains("concrete") {
        return Rgb([128, 128, 128]);
    }
    if name.contains("wool") || name.contains("carpet") {
        return Rgb([220, 220, 220]);
    }
    if name.contains("terracotta") {
        return Rgb([152, 94, 67]);
    }
    if name.contains("iron") {
        return Rgb([200, 200, 200]);
    }
    if name.contains("gold") {
        return Rgb([255, 215, 0]);
    }
    if name.contains("diamond") {
        return Rgb([97, 219, 213]);
    }
    if name.contains("emerald") {
        return Rgb([17, 160, 54]);
    }
    if name.contains("lapis") {
        return Rgb([38, 67, 156]);
    }
    if name.contains("redstone") {
        return Rgb([170, 0, 0]);
    }
    if name.contains("netherrack") || name.contains("nether") {
        return Rgb([111, 54, 53]);
    }
    if name.contains("end_stone") {
        return Rgb([219, 222, 158]);
    }
    if name.contains("obsidian") {
        return Rgb([15, 10, 24]);
    }
    if name.contains("deepslate") {
        return Rgb([72, 72, 73]);
    }
    if name.contains("blackstone") {
        return Rgb([42, 36, 41]);
    }
    if name.contains("quartz") {
        return Rgb([235, 229, 222]);
    }
    if name.contains("prismarine") {
        return Rgb([76, 128, 113]);
    }
    if name.contains("copper") {
        return Rgb([192, 107, 79]);
    }
    if name.contains("amethyst") {
        return Rgb([133, 97, 191]);
    }
    if name.contains("moss") {
        return Rgb([89, 109, 45]);
    }
    if name.contains("dripstone") {
        return Rgb([134, 107, 92]);
    }

    // Default gray for unknown blocks
    Rgb([160, 160, 160])
}

/// Returns a mapping of common block names to RGB colors (without minecraft: prefix)
fn get_block_colors() -> FnvHashMap<&'static str, Rgb<u8>> {
    FnvHashMap::from_iter([
        ("grass_block", Rgb([86, 125, 70])),
        ("short_grass", Rgb([86, 125, 70])),
        ("tall_grass", Rgb([86, 125, 70])),
        ("dirt", Rgb([139, 90, 43])),
        ("coarse_dirt", Rgb([119, 85, 59])),
        ("podzol", Rgb([91, 63, 24])),
        ("rooted_dirt", Rgb([144, 103, 76])),
        ("mud", Rgb([60, 57, 61])),
        ("stone", Rgb([128, 128, 128])),
        ("granite", Rgb([149, 108, 91])),
        ("polished_granite", Rgb([154, 112, 98])),
        ("diorite", Rgb([189, 188, 189])),
        ("polished_diorite", Rgb([195, 195, 195])),
        ("andesite", Rgb([136, 136, 137])),
        ("polished_andesite", Rgb([132, 135, 134])),
        ("deepslate", Rgb([72, 72, 73])),
        ("cobbled_deepslate", Rgb([77, 77, 80])),
        ("polished_deepslate", Rgb([72, 72, 73])),
        ("deepslate_bricks", Rgb([70, 70, 71])),
        ("deepslate_tiles", Rgb([54, 54, 55])),
        ("calcite", Rgb([223, 224, 220])),
        ("tuff", Rgb([108, 109, 102])),
        ("dripstone_block", Rgb([134, 107, 92])),
        ("sand", Rgb([219, 211, 160])),
        ("red_sand", Rgb([190, 102, 33])),
        ("gravel", Rgb([131, 127, 126])),
        ("clay", Rgb([160, 166, 179])),
        ("bedrock", Rgb([85, 85, 85])),
        ("water", Rgb([59, 86, 165])),
        ("ice", Rgb([145, 183, 253])),
        ("packed_ice", Rgb([141, 180, 250])),
        ("blue_ice", Rgb([116, 167, 253])),
        ("snow", Rgb([249, 254, 254])),
        ("snow_block", Rgb([249, 254, 254])),
        ("powder_snow", Rgb([248, 253, 253])),
        ("oak_log", Rgb([109, 85, 50])),
        ("oak_planks", Rgb([162, 130, 78])),
        ("oak_slab", Rgb([162, 130, 78])),
        ("oak_stairs", Rgb([162, 130, 78])),
        ("oak_fence", Rgb([162, 130, 78])),
        ("oak_door", Rgb([162, 130, 78])),
        ("spruce_log", Rgb([58, 37, 16])),
        ("spruce_planks", Rgb([115, 85, 49])),
        ("spruce_slab", Rgb([115, 85, 49])),
        ("spruce_stairs", Rgb([115, 85, 49])),
        ("spruce_fence", Rgb([115, 85, 49])),
        ("spruce_door", Rgb([115, 85, 49])),
        ("birch_log", Rgb([216, 215, 210])),
        ("birch_planks", Rgb([196, 179, 123])),
        ("birch_slab", Rgb([196, 179, 123])),
        ("birch_stairs", Rgb([196, 179, 123])),
        ("birch_fence", Rgb([196, 179, 123])),
        ("birch_door", Rgb([196, 179, 123])),
        ("jungle_log", Rgb([85, 68, 25])),
        ("jungle_planks", Rgb([160, 115, 81])),
        ("acacia_log", Rgb([103, 96, 86])),
        ("acacia_planks", Rgb([168, 90, 50])),
        ("dark_oak_log", Rgb([60, 46, 26])),
        ("dark_oak_planks", Rgb([67, 43, 20])),
        ("dark_oak_slab", Rgb([67, 43, 20])),
        ("dark_oak_stairs", Rgb([67, 43, 20])),
        ("dark_oak_fence", Rgb([67, 43, 20])),
        ("dark_oak_door", Rgb([67, 43, 20])),
        ("mangrove_log", Rgb([84, 66, 36])),
        ("mangrove_planks", Rgb([117, 54, 48])),
        ("cherry_log", Rgb([54, 33, 44])),
        ("cherry_planks", Rgb([226, 178, 172])),
        ("bamboo_block", Rgb([122, 129, 52])),
        ("bamboo_planks", Rgb([194, 175, 93])),
        ("crimson_stem", Rgb([92, 25, 29])),
        ("crimson_planks", Rgb([101, 48, 70])),
        ("warped_stem", Rgb([58, 58, 77])),
        ("warped_planks", Rgb([43, 104, 99])),
        ("oak_leaves", Rgb([55, 95, 36])),
        ("spruce_leaves", Rgb([61, 99, 61])),
        ("birch_leaves", Rgb([80, 106, 47])),
        ("jungle_leaves", Rgb([48, 113, 20])),
        ("acacia_leaves", Rgb([75, 104, 40])),
        ("dark_oak_leaves", Rgb([35, 82, 11])),
        ("mangrove_leaves", Rgb([69, 123, 38])),
        ("cherry_leaves", Rgb([228, 177, 197])),
        ("azalea_leaves", Rgb([71, 96, 37])),
        ("stone_bricks", Rgb([122, 122, 122])),
        ("stone_brick_slab", Rgb([122, 122, 122])),
        ("stone_brick_stairs", Rgb([122, 122, 122])),
        ("stone_brick_wall", Rgb([122, 122, 122])),
        ("mossy_stone_bricks", Rgb([115, 121, 105])),
        ("mossy_stone_brick_slab", Rgb([115, 121, 105])),
        ("mossy_stone_brick_stairs", Rgb([115, 121, 105])),
        ("mossy_stone_brick_wall", Rgb([115, 121, 105])),
        ("cracked_stone_bricks", Rgb([118, 117, 118])),
        ("chiseled_stone_bricks", Rgb([119, 119, 119])),
        ("cobblestone", Rgb([128, 127, 127])),
        ("cobblestone_slab", Rgb([128, 127, 127])),
        ("cobblestone_stairs", Rgb([128, 127, 127])),
        ("cobblestone_wall", Rgb([128, 127, 127])),
        ("mossy_cobblestone", Rgb([110, 118, 94])),
        ("mossy_cobblestone_slab", Rgb([110, 118, 94])),
        ("mossy_cobblestone_stairs", Rgb([110, 118, 94])),
        ("mossy_cobblestone_wall", Rgb([110, 118, 94])),
        ("stone_slab", Rgb([128, 128, 128])),
        ("stone_stairs", Rgb([128, 128, 128])),
        ("smooth_stone", Rgb([158, 158, 158])),
        ("smooth_stone_slab", Rgb([158, 158, 158])),
        ("bricks", Rgb([150, 97, 83])),
        ("brick_slab", Rgb([150, 97, 83])),
        ("brick_stairs", Rgb([150, 97, 83])),
        ("brick_wall", Rgb([150, 97, 83])),
        ("mud_bricks", Rgb([137, 103, 79])),
        ("mud_brick_slab", Rgb([137, 103, 79])),
        ("mud_brick_stairs", Rgb([137, 103, 79])),
        ("mud_brick_wall", Rgb([137, 103, 79])),
        ("terracotta", Rgb([152, 94, 67])),
        ("white_terracotta", Rgb([210, 178, 161])),
        ("orange_terracotta", Rgb([162, 84, 38])),
        ("magenta_terracotta", Rgb([149, 88, 109])),
        ("light_blue_terracotta", Rgb([113, 109, 138])),
        ("yellow_terracotta", Rgb([186, 133, 35])),
        ("lime_terracotta", Rgb([104, 118, 53])),
        ("pink_terracotta", Rgb([162, 78, 79])),
        ("gray_terracotta", Rgb([58, 42, 36])),
        ("light_gray_terracotta", Rgb([135, 107, 98])),
        ("cyan_terracotta", Rgb([87, 91, 91])),
        ("purple_terracotta", Rgb([118, 70, 86])),
        ("blue_terracotta", Rgb([74, 60, 91])),
        ("brown_terracotta", Rgb([77, 51, 36])),
        ("green_terracotta", Rgb([76, 83, 42])),
        ("red_terracotta", Rgb([143, 61, 47])),
        ("black_terracotta", Rgb([37, 23, 16])),
        ("white_concrete", Rgb([207, 213, 214])),
        ("orange_concrete", Rgb([224, 97, 0])),
        ("magenta_concrete", Rgb([169, 48, 159])),
        ("light_blue_concrete", Rgb([35, 137, 198])),
        ("yellow_concrete", Rgb([241, 175, 21])),
        ("lime_concrete", Rgb([94, 169, 24])),
        ("pink_concrete", Rgb([214, 101, 143])),
        ("gray_concrete", Rgb([55, 58, 62])),
        ("light_gray_concrete", Rgb([125, 125, 115])),
        ("cyan_concrete", Rgb([21, 119, 136])),
        ("purple_concrete", Rgb([100, 32, 156])),
        ("blue_concrete", Rgb([45, 47, 143])),
        ("brown_concrete", Rgb([96, 60, 32])),
        ("green_concrete", Rgb([73, 91, 36])),
        ("red_concrete", Rgb([142, 33, 33])),
        ("black_concrete", Rgb([8, 10, 15])),
        ("white_wool", Rgb([234, 236, 237])),
        ("orange_wool", Rgb([241, 118, 20])),
        ("magenta_wool", Rgb([190, 68, 179])),
        ("light_blue_wool", Rgb([58, 175, 217])),
        ("yellow_wool", Rgb([249, 198, 40])),
        ("lime_wool", Rgb([112, 185, 26])),
        ("pink_wool", Rgb([238, 141, 172])),
        ("gray_wool", Rgb([63, 68, 72])),
        ("light_gray_wool", Rgb([142, 142, 135])),
        ("cyan_wool", Rgb([21, 138, 145])),
        ("purple_wool", Rgb([122, 42, 173])),
        ("blue_wool", Rgb([53, 57, 157])),
        ("brown_wool", Rgb([114, 72, 41])),
        ("green_wool", Rgb([85, 110, 28])),
        ("red_wool", Rgb([161, 39, 35])),
        ("black_wool", Rgb([21, 21, 26])),
        ("sandstone", Rgb([223, 214, 170])),
        ("sandstone_slab", Rgb([223, 214, 170])),
        ("sandstone_stairs", Rgb([223, 214, 170])),
        ("sandstone_wall", Rgb([223, 214, 170])),
        ("chiseled_sandstone", Rgb([223, 214, 170])),
        ("cut_sandstone", Rgb([225, 217, 171])),
        ("cut_sandstone_slab", Rgb([225, 217, 171])),
        ("smooth_sandstone", Rgb([223, 214, 170])),
        ("smooth_sandstone_slab", Rgb([223, 214, 170])),
        ("smooth_sandstone_stairs", Rgb([223, 214, 170])),
        ("red_sandstone", Rgb([186, 99, 29])),
        ("red_sandstone_slab", Rgb([186, 99, 29])),
        ("red_sandstone_stairs", Rgb([186, 99, 29])),
        ("red_sandstone_wall", Rgb([186, 99, 29])),
        ("smooth_red_sandstone", Rgb([186, 99, 29])),
        ("netherrack", Rgb([111, 54, 53])),
        ("nether_bricks", Rgb([44, 21, 26])),
        ("nether_brick_slab", Rgb([44, 21, 26])),
        ("nether_brick_stairs", Rgb([44, 21, 26])),
        ("nether_brick_wall", Rgb([44, 21, 26])),
        ("nether_brick_fence", Rgb([44, 21, 26])),
        ("red_nether_bricks", Rgb([69, 7, 9])),
        ("red_nether_brick_slab", Rgb([69, 7, 9])),
        ("red_nether_brick_stairs", Rgb([69, 7, 9])),
        ("red_nether_brick_wall", Rgb([69, 7, 9])),
        ("soul_sand", Rgb([81, 62, 51])),
        ("soul_soil", Rgb([75, 57, 46])),
        ("basalt", Rgb([73, 72, 77])),
        ("polished_basalt", Rgb([88, 87, 91])),
        ("smooth_basalt", Rgb([72, 72, 78])),
        ("blackstone", Rgb([42, 36, 41])),
        ("blackstone_slab", Rgb([42, 36, 41])),
        ("blackstone_stairs", Rgb([42, 36, 41])),
        ("blackstone_wall", Rgb([42, 36, 41])),
        ("polished_blackstone", Rgb([53, 49, 56])),
        ("polished_blackstone_bricks", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_slab", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_stairs", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_wall", Rgb([48, 43, 50])),
        ("glowstone", Rgb([171, 131, 84])),
        ("shroomlight", Rgb([240, 146, 70])),
        ("crying_obsidian", Rgb([32, 10, 60])),
        ("obsidian", Rgb([15, 10, 24])),
        ("end_stone", Rgb([219, 222, 158])),
        ("end_stone_bricks", Rgb([218, 224, 162])),
        ("end_stone_brick_slab", Rgb([218, 224, 162])),
        ("end_stone_brick_stairs", Rgb([218, 224, 162])),
        ("end_stone_brick_wall", Rgb([218, 224, 162])),
        ("purpur_block", Rgb([170, 126, 170])),
        ("purpur_pillar", Rgb([171, 129, 171])),
        ("purpur_slab", Rgb([170, 126, 170])),
        ("purpur_stairs", Rgb([170, 126, 170])),
        ("coal_ore", Rgb([105, 105, 105])),
        ("iron_ore", Rgb([136, 130, 127])),
        ("copper_ore", Rgb([124, 125, 120])),
        ("gold_ore", Rgb([143, 140, 125])),
        ("redstone_ore", Rgb([133, 107, 107])),
        ("emerald_ore", Rgb([108, 136, 115])),
        ("lapis_ore", Rgb([99, 112, 135])),
        ("diamond_ore", Rgb([121, 141, 140])),
        ("coal_block", Rgb([16, 15, 15])),
        ("iron_block", Rgb([220, 220, 220])),
        ("copper_block", Rgb([192, 107, 79])),
        ("gold_block", Rgb([246, 208, 62])),
        ("redstone_block", Rgb([170, 0, 0])),
        ("emerald_block", Rgb([42, 203, 88])),
        ("lapis_block", Rgb([38, 67, 156])),
        ("diamond_block", Rgb([97, 219, 213])),
        ("netherite_block", Rgb([66, 61, 63])),
        ("amethyst_block", Rgb([133, 97, 191])),
        ("raw_iron_block", Rgb([166, 136, 107])),
        ("raw_copper_block", Rgb([154, 105, 79])),
        ("raw_gold_block", Rgb([221, 169, 46])),
        ("quartz_block", Rgb([235, 229, 222])),
        ("quartz_slab", Rgb([235, 229, 222])),
        ("quartz_stairs", Rgb([235, 229, 222])),
        ("smooth_quartz", Rgb([235, 229, 222])),
        ("smooth_quartz_slab", Rgb([235, 229, 222])),
        ("smooth_quartz_stairs", Rgb([235, 229, 222])),
        ("quartz_bricks", Rgb([234, 229, 221])),
        ("quartz_pillar", Rgb([235, 230, 224])),
        ("chiseled_quartz_block", Rgb([231, 226, 218])),
        ("prismarine", Rgb([76, 128, 113])),
        ("prismarine_slab", Rgb([76, 128, 113])),
        ("prismarine_stairs", Rgb([76, 128, 113])),
        ("prismarine_wall", Rgb([76, 128, 113])),
        ("prismarine_bricks", Rgb([99, 172, 158])),
        ("prismarine_brick_slab", Rgb([99, 172, 158])),
        ("prismarine_brick_stairs", Rgb([99, 172, 158])),
        ("dark_prismarine", Rgb([51, 91, 75])),
        ("dark_prismarine_slab", Rgb([51, 91, 75])),
        ("dark_prismarine_stairs", Rgb([51, 91, 75])),
        ("sea_lantern", Rgb([172, 199, 190])),
        ("exposed_copper", Rgb([161, 125, 103])),
        ("weathered_copper", Rgb([109, 145, 107])),
        ("oxidized_copper", Rgb([82, 162, 132])),
        ("cut_copper", Rgb([191, 106, 80])),
        ("cut_copper_slab", Rgb([191, 106, 80])),
        ("cut_copper_stairs", Rgb([191, 106, 80])),
        ("exposed_cut_copper", Rgb([154, 121, 101])),
        ("exposed_cut_copper_slab", Rgb([154, 121, 101])),
        ("exposed_cut_copper_stairs", Rgb([154, 121, 101])),
        ("weathered_cut_copper", Rgb([109, 145, 107])),
        ("weathered_cut_copper_slab", Rgb([109, 145, 107])),
        ("weathered_cut_copper_stairs", Rgb([109, 145, 107])),
        ("oxidized_cut_copper", Rgb([79, 153, 126])),
        ("oxidized_cut_copper_slab", Rgb([79, 153, 126])),
        ("oxidized_cut_copper_stairs", Rgb([79, 153, 126])),
        ("glass", Rgb([200, 220, 230])),
        ("glass_pane", Rgb([200, 220, 230])),
        ("white_stained_glass", Rgb([255, 255, 255])),
        ("white_stained_glass_pane", Rgb([255, 255, 255])),
        ("orange_stained_glass", Rgb([216, 127, 51])),
        ("orange_stained_glass_pane", Rgb([216, 127, 51])),
        ("magenta_stained_glass", Rgb([178, 76, 216])),
        ("magenta_stained_glass_pane", Rgb([178, 76, 216])),
        ("light_blue_stained_glass", Rgb([102, 153, 216])),
        ("light_blue_stained_glass_pane", Rgb([102, 153, 216])),
        ("yellow_stained_glass", Rgb([229, 229, 51])),
        ("yellow_stained_glass_pane", Rgb([229, 229, 51])),
        ("lime_stained_glass", Rgb([127, 204, 25])),
        ("lime_stained_glass_pane", Rgb([127, 204, 25])),
        ("pink_stained_glass", Rgb([242, 127, 165])),
        ("pink_stained_glass_pane", Rgb([242, 127, 165])),
        ("gray_stained_glass", Rgb([76, 76, 76])),
        ("gray_stained_glass_pane", Rgb([76, 76, 76])),
        ("light_gray_stained_glass", Rgb([153, 153, 153])),
        ("light_gray_stained_glass_pane", Rgb([153, 153, 153])),
        ("cyan_stained_glass", Rgb([76, 127, 153])),
        ("cyan_stained_glass_pane", Rgb([76, 127, 153])),
        ("purple_stained_glass", Rgb([127, 63, 178])),
        ("purple_stained_glass_pane", Rgb([127, 63, 178])),
        ("blue_stained_glass", Rgb([51, 76, 178])),
        ("blue_stained_glass_pane", Rgb([51, 76, 178])),
        ("brown_stained_glass", Rgb([102, 76, 51])),
        ("brown_stained_glass_pane", Rgb([102, 76, 51])),
        ("green_stained_glass", Rgb([102, 127, 51])),
        ("green_stained_glass_pane", Rgb([102, 127, 51])),
        ("red_stained_glass", Rgb([153, 51, 51])),
        ("red_stained_glass_pane", Rgb([153, 51, 51])),
        ("black_stained_glass", Rgb([25, 25, 25])),
        ("black_stained_glass_pane", Rgb([25, 25, 25])),
        ("bookshelf", Rgb([116, 89, 53])),
        ("hay_block", Rgb([166, 139, 12])),
        ("melon", Rgb([111, 145, 31])),
        ("pumpkin", Rgb([198, 118, 24])),
        ("jack_o_lantern", Rgb([213, 139, 42])),
        ("carved_pumpkin", Rgb([198, 118, 24])),
        ("tnt", Rgb([219, 68, 52])),
        ("sponge", Rgb([195, 192, 74])),
        ("wet_sponge", Rgb([171, 181, 70])),
        ("moss_block", Rgb([89, 109, 45])),
        ("moss_carpet", Rgb([89, 109, 45])),
        ("sculk", Rgb([12, 28, 36])),
        ("honeycomb_block", Rgb([229, 148, 29])),
        ("slime_block", Rgb([111, 192, 91])),
        ("honey_block", Rgb([251, 185, 52])),
        ("barrel", Rgb([140, 106, 60])),
        ("chest", Rgb([155, 113, 48])),
        ("trapped_chest", Rgb([155, 113, 48])),
        ("crafting_table", Rgb([144, 109, 67])),
        ("furnace", Rgb([110, 110, 110])),
        ("blast_furnace", Rgb([80, 80, 85])),
        ("smoker", Rgb([90, 80, 70])),
        ("anvil", Rgb([68, 68, 68])),
        ("lectern", Rgb([180, 140, 90])),
        ("composter", Rgb([100, 80, 45])),
        ("cauldron", Rgb([60, 60, 60])),
        ("hopper", Rgb([70, 70, 70])),
        ("jukebox", Rgb([130, 90, 70])),
        ("note_block", Rgb([120, 80, 65])),
        ("bell", Rgb([200, 170, 50])),
        ("dirt_path", Rgb([148, 121, 65])),
        ("farmland", Rgb([143, 88, 46])),
        ("mycelium", Rgb([111, 99, 107])),
        ("rail", Rgb([125, 108, 77])),
        ("powered_rail", Rgb([153, 126, 55])),
        ("detector_rail", Rgb([120, 97, 80])),
        ("activator_rail", Rgb([117, 85, 76])),
        ("redstone_wire", Rgb([170, 0, 0])),
        ("redstone_torch", Rgb([170, 0, 0])),
        ("redstone_lamp", Rgb([180, 130, 70])),
        ("lever", Rgb([100, 80, 60])),
        ("tripwire_hook", Rgb([120, 100, 80])),
        ("torch", Rgb([255, 200, 100])),
        ("wall_torch", Rgb([255, 200, 100])),
        ("lantern", Rgb([200, 150, 80])),
        ("soul_lantern", Rgb([80, 200, 200])),
        ("soul_torch", Rgb([80, 200, 200])),
        ("soul_wall_torch", Rgb([80, 200, 200])),
        ("campfire", Rgb([200, 100, 50])),
        ("soul_campfire", Rgb([80, 200, 200])),
        ("candle", Rgb([200, 180, 130])),
        ("dandelion", Rgb([255, 236, 85])),
        ("poppy", Rgb([200, 30, 30])),
        ("blue_orchid", Rgb([47, 186, 199])),
        ("allium", Rgb([190, 130, 200])),
        ("azure_bluet", Rgb([220, 230, 220])),
        ("red_tulip", Rgb([200, 50, 50])),
        ("orange_tulip", Rgb([230, 130, 50])),
        ("white_tulip", Rgb([230, 230, 220])),
        ("pink_tulip", Rgb([220, 150, 170])),
        ("oxeye_daisy", Rgb([230, 230, 200])),
        ("cornflower", Rgb([70, 90, 180])),
        ("lily_of_the_valley", Rgb([230, 230, 230])),
        ("wither_rose", Rgb([30, 30, 30])),
        ("sunflower", Rgb([255, 200, 50])),
        ("lilac", Rgb([200, 150, 200])),
        ("rose_bush", Rgb([180, 40, 40])),
        ("peony", Rgb([230, 180, 200])),
        ("fern", Rgb([80, 120, 60])),
        ("large_fern", Rgb([80, 120, 60])),
        ("dead_bush", Rgb([150, 120, 80])),
        ("seagrass", Rgb([40, 100, 60])),
        ("tall_seagrass", Rgb([40, 100, 60])),
        ("kelp", Rgb([50, 110, 60])),
        ("kelp_plant", Rgb([50, 110, 60])),
        ("sugar_cane", Rgb([140, 180, 100])),
        ("bamboo", Rgb([90, 140, 50])),
        ("vine", Rgb([50, 100, 40])),
        ("lily_pad", Rgb([40, 110, 40])),
        ("sweet_berry_bush", Rgb([60, 90, 50])),
        ("cactus", Rgb([85, 127, 52])),
        ("white_carpet", Rgb([234, 236, 237])),
        ("orange_carpet", Rgb([241, 118, 20])),
        ("magenta_carpet", Rgb([190, 68, 179])),
        ("light_blue_carpet", Rgb([58, 175, 217])),
        ("yellow_carpet", Rgb([249, 198, 40])),
        ("lime_carpet", Rgb([112, 185, 26])),
        ("pink_carpet", Rgb([238, 141, 172])),
        ("gray_carpet", Rgb([63, 68, 72])),
        ("light_gray_carpet", Rgb([142, 142, 135])),
        ("cyan_carpet", Rgb([21, 138, 145])),
        ("purple_carpet", Rgb([122, 42, 173])),
        ("blue_carpet", Rgb([53, 57, 157])),
        ("brown_carpet", Rgb([114, 72, 41])),
        ("green_carpet", Rgb([85, 110, 28])),
        ("red_carpet", Rgb([161, 39, 35])),
        ("black_carpet", Rgb([21, 21, 26])),
        ("oak_sign", Rgb([162, 130, 78])),
        ("oak_wall_sign", Rgb([162, 130, 78])),
        ("spruce_sign", Rgb([115, 85, 49])),
        ("spruce_wall_sign", Rgb([115, 85, 49])),
        ("birch_sign", Rgb([196, 179, 123])),
        ("birch_wall_sign", Rgb([196, 179, 123])),
        ("dark_oak_sign", Rgb([67, 43, 20])),
        ("dark_oak_wall_sign", Rgb([67, 43, 20])),
        ("white_bed", Rgb([234, 236, 237])),
        ("orange_bed", Rgb([241, 118, 20])),
        ("magenta_bed", Rgb([190, 68, 179])),
        ("light_blue_bed", Rgb([58, 175, 217])),
        ("yellow_bed", Rgb([249, 198, 40])),
        ("lime_bed", Rgb([112, 185, 26])),
        ("pink_bed", Rgb([238, 141, 172])),
        ("gray_bed", Rgb([63, 68, 72])),
        ("light_gray_bed", Rgb([142, 142, 135])),
        ("cyan_bed", Rgb([21, 138, 145])),
        ("purple_bed", Rgb([122, 42, 173])),
        ("blue_bed", Rgb([53, 57, 157])),
        ("brown_bed", Rgb([114, 72, 41])),
        ("green_bed", Rgb([85, 110, 28])),
        ("red_bed", Rgb([161, 39, 35])),
        ("black_bed", Rgb([21, 21, 26])),
        ("oak_trapdoor", Rgb([162, 130, 78])),
        ("spruce_trapdoor", Rgb([115, 85, 49])),
        ("birch_trapdoor", Rgb([196, 179, 123])),
        ("dark_oak_trapdoor", Rgb([67, 43, 20])),
        ("iron_trapdoor", Rgb([200, 200, 200])),
        ("iron_bars", Rgb([150, 150, 150])),
        ("ladder", Rgb([160, 130, 70])),
        ("wheat", Rgb([200, 180, 80])),
        ("carrots", Rgb([230, 140, 30])),
        ("potatoes", Rgb([180, 160, 80])),
        ("beetroots", Rgb([150, 50, 50])),
        ("pumpkin_stem", Rgb([120, 140, 70])),
        ("melon_stem", Rgb([120, 140, 70])),
    ])
}

#[cfg(test)]
mod voxel_crop_tests {
    use super::*;

    /// Grade 3×3×3 totalmente preenchida com o mesmo bloco: só o cubo do
    /// meio (1,1,1) tem os 6 vizinhos ocupados — é o único que deve ser
    /// descartado. Os outros 26 (a casca externa) têm pelo menos uma face
    /// tocando a borda do recorte (tratada como exposta) e devem sobrar.
    #[test]
    fn extract_exposed_voxels_culls_only_the_fully_buried_center() {
        let (w, d, h) = (3, 3, 3);
        let grid = vec![1u16; w * d * h];
        let palette = vec!["minecraft:stone".to_string()];

        let voxels = extract_exposed_voxels(&grid, w, d, h, 0, 0, 0, &palette);

        assert_eq!(
            voxels.len(),
            26,
            "só o bloco central (1,1,1) deve ser culled"
        );
        assert!(
            !voxels.iter().any(|&(x, y, z, _)| (x, y, z) == (1, 1, 1)),
            "o bloco central, cercado nas 6 faces, não deveria aparecer no resultado"
        );
    }

    /// Uma grade totalmente vazia (paleta índice 0 em toda célula) não deve
    /// produzir nenhum voxel — regressão simples pra garantir que o índice
    /// 0 = "vazio" é respeitado e não é interpretado como um bloco real.
    #[test]
    fn extract_exposed_voxels_empty_grid_yields_nothing() {
        let grid = vec![0u16; 4 * 4 * 4];
        let palette: Vec<String> = vec![];
        let voxels = extract_exposed_voxels(&grid, 4, 4, 4, -10, 5, 20, &palette);
        assert!(voxels.is_empty());
    }

    /// Um único bloco isolado (grade 1×1×1) não tem nenhum vizinho dentro
    /// do recorte — todos os 6 lados caem fora da grade e contam como
    /// "vazio" — então ele precisa aparecer exposto, nunca culled.
    #[test]
    fn extract_exposed_voxels_single_block_is_always_exposed() {
        let grid = vec![1u16];
        let palette = vec!["minecraft:oak_planks".to_string()];
        let voxels = extract_exposed_voxels(&grid, 1, 1, 1, 100, -64, 200, &palette);
        assert_eq!(voxels.len(), 1);
        assert_eq!((voxels[0].0, voxels[0].1, voxels[0].2), (100, -64, 200));
    }

    #[test]
    fn compute_voxel_crop_rejects_oversized_requests() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let err = compute_voxel_crop(tmp.path(), -1000, 1000, -1000, 1000, -64, 320)
            .expect_err("recorte gigante precisa ser rejeitado antes de tentar alocar a grade");
        assert!(err.contains("grande demais"), "mensagem inesperada: {err}");
    }

    #[test]
    fn compute_voxel_crop_rejects_inverted_bounds() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let err = compute_voxel_crop(tmp.path(), 10, -10, 0, 5, 0, 5)
            .expect_err("min_x > max_x precisa ser rejeitado");
        assert!(err.contains("inválido"));
    }
}
