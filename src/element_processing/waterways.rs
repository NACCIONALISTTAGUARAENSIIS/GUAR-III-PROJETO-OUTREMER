use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use noise::{NoiseFn, OpenSimplex};
use once_cell::sync::Lazy;

// Lazy initialization do Simplex com sementes diferentes (Macro e Micro ruído)
static RIVER_WIDTH_NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(8080));
static RIVER_SHIFT_NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(9090));
static BANK_SEDIMENT_NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(4040));

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(waterway_type) = element.tags.get("waterway") {
        let (mut waterway_width, waterway_depth) = get_waterway_dimensions(waterway_type);

        // Check for custom width in tags
        if let Some(width_str) = element.tags.get("width") {
            waterway_width = width_str.parse::<i32>().unwrap_or_else(|_| {
                width_str
                    .parse::<f32>()
                    .map(|f: f32| f as i32)
                    .unwrap_or(waterway_width)
            });
        }

        // Skip layers below the ground level
        if matches!(
            element.tags.get("layer").map(|s: &String| s.as_str()),
            Some("-1") | Some("-2") | Some("-3")
        ) {
            return;
        }

        // Process consecutive node pairs to create waterways
        // Use windows(2) to avoid connecting last node back to first
        for nodes_pair in element.nodes.windows(2) {
            let prev_node = nodes_pair[0].xz();
            let current_node = nodes_pair[1].xz();

            // Draw a line between the current and previous node
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                // Create water channel with geomorphological realism
                create_water_channel(editor, bx, bz, waterway_width, waterway_depth);
            }
        }
    }
}

/// Determines width and depth based on waterway type
fn get_waterway_dimensions(waterway_type: &str) -> (i32, i32) {
    match waterway_type {
        // TWEAK URBANÍSTICO & RP: Escala 1.33H com foco em imponência orgânica
        // Rios principais do DF (Descoberto, São Bartolomeu): Largos e fundos
        "river" => (24, 5),
        "canal" => (12, 3),    // Canais urbanos
        "stream" => (7, 2),    // Córregos (A grande maioria da hidrografia do DF)
        "fairway" => (20, 5),  // Rotas de navegação (Paranoá)
        "flowline" => (3, 1),  // Linhas de fluxo de chuva
        "brook" => (4, 1),     // Riachos rasos
        "ditch" => (3, 1),     // Valetas
        "drain" => (2, 1),     // Drenagem pluvial
        _ => (6, 2),           // Default seguro orgânico
    }
}

/// Creates a water channel with realistic meanders, variable depth, and floodplains
fn create_water_channel(
    editor: &mut WorldEditor,
    base_x: i32,
    base_z: i32,
    base_width: i32,
    base_depth: i32,
) {
    // 1. MACRO-GEOMORFOLOGIA (Frequência muito baixa para suavidade)
    let width_noise = RIVER_WIDTH_NOISE.get([base_x as f64 * 0.01, base_z as f64 * 0.01]);
    let shift_noise = RIVER_SHIFT_NOISE.get([base_z as f64 * 0.015, base_x as f64 * 0.015]);

    // 2. MEANDRAMENTO (Deslocamento do eixo do rio)
    // O canal se desloca até 30% da sua largura para os lados baseado no ruído
    let max_shift = (base_width as f64 * 0.3).round() as i32;
    let shift_x = (shift_noise * max_shift as f64) as i32;
    let shift_z = (shift_noise * 0.5 * max_shift as f64) as i32; // Ligeira assimetria direcional

    let center_x = base_x + shift_x;
    let center_z = base_z + shift_z;

    // 3. LARGURA E PROFUNDIDADE BASEADAS EM ENERGIA
    // Se o rio alarga (width_noise alto), ele fica mais raso. Se estreita, fica mais fundo.
    let width_variance = (base_width as f64 * 0.25 * width_noise).round() as i32;
    let final_width = (base_width + width_variance).max(2);

    // Profundidade inversamente proporcional à variação de largura
    let depth_variance = -(width_variance as f64 * 0.5).round() as i32;
    let final_depth = (base_depth + depth_variance).max(1);

    let half_width = final_width / 2;
    let half_width_sq = half_width * half_width;
    let outer_radius_sq = (half_width + 1) * (half_width + 1);

    // Zona Ripária (Planície de inundação): +2 a +4 blocos além do banco
    let riparian_radius_sq = (half_width + 4) * (half_width + 4);

    // A GRANDE CORREÇÃO: Nivelamento Baseado no Eixo
    // Pega a altura do centro do rio e usa ela como "superfície do espelho d'água"
    // para toda a largura transversal. Evita rios tortos em ladeiras.
    let river_surface_y = editor.get_ground_level(center_x, center_z);

    // Iteração sobre o Bounding Box local da seção do rio (incluindo planície)
    for x in (center_x - half_width - 4)..=(center_x + half_width + 4) {
        for z in (center_z - half_width - 4)..=(center_z + half_width + 4) {
            let dx = (x - center_x).abs();
            let dz = (z - center_z).abs();
            let distance_sq = dx * dx + dz * dz;

            // SEDIMENTAÇÃO GLOBAL DA COORDENADA
            let sediment_noise = BANK_SEDIMENT_NOISE.get([x as f64 * 0.04, z as f64 * 0.04]);

            // Descobrir onde o chão real está neste bloco lateral
            let local_ground_y = editor.get_ground_level(x, z);

            // --- ZONA 1: LEITO CENTRAL PROFUNDO ---
            if distance_sq <= half_width_sq {

                // Escava do solo local até o fundo do rio (river_surface_y - final_depth)
                // Se o morro for alto, ele rasga o morro. Se for baixo, ele constrói a margem.
                let bottom_y = river_surface_y - final_depth;
                let top_y = local_ground_y.max(river_surface_y);

                for y in bottom_y..=top_y {
                    if y <= river_surface_y {
                        editor.set_block_absolute(WATER, x, y, z, None, None);
                    } else {
                        // Limpa terra/pedra que estava no caminho do vale em U
                        editor.set_block_absolute(AIR, x, y, z, None, None);
                    }
                }

                // Fundo geológico
                let bed_block = if sediment_noise > 0.3 {
                    MUD // Lama em áreas largas/lentas
                } else if sediment_noise > -0.2 {
                    SAND // Assoreamento padrão
                } else {
                    GRAVEL // Cascalho em leito rápido
                };
                editor.set_block_absolute(bed_block, x, bottom_y - 1, z, None, None);

            // --- ZONA 2: BARRANCO INCLINADO E PRAIAS ---
            } else if distance_sq <= outer_radius_sq && final_depth > 1 {
                // A profundidade da margem cai pela metade
                let slope_depth = (final_depth / 2).max(1);
                let bottom_y = river_surface_y - slope_depth;
                let top_y = local_ground_y.max(river_surface_y);

                for y in bottom_y..=top_y {
                    if y <= river_surface_y {
                        editor.set_block_absolute(WATER, x, y, z, None, None);
                    } else {
                        editor.set_block_absolute(AIR, x, y, z, None, None);
                    }
                }

                // Margens assimétricas baseadas no sedimento
                let bank_block = if sediment_noise > 0.4 {
                    SAND // Banco de areia extenso
                } else if sediment_noise > 0.0 {
                    DIRT // Terra úmida
                } else {
                    COARSE_DIRT // Barranco seco erodido
                };
                editor.set_block_absolute(bank_block, x, bottom_y - 1, z, None, None);

            // --- ZONA 3: PLANÍCIE DE INUNDAÇÃO (MATA CILIAR) ---
            } else if distance_sq <= riparian_radius_sq {
                // Não colocamos água aqui, apenas alteramos o solo e a flora para simular umidade
                // O Y local dita a regra (não aplainamos a planície, ela acompanha o morro)
                if editor.check_for_block_absolute(x, local_ground_y, z, Some(&[GRASS_BLOCK, DIRT, COARSE_DIRT]), None) {

                    let soil_block = if sediment_noise > 0.2 {
                        MUD // Solo turfoso perto da margem
                    } else if sediment_noise > -0.2 {
                        PODZOL // Terra preta orgânica de mata ciliar
                    } else {
                        GRASS_BLOCK // Transição normal
                    };

                    editor.set_block_absolute(soil_block, x, local_ground_y, z, None, None);

                    // Vegetação ciliar densa (Samambaias e mato alto amam água)
                    if sediment_noise > 0.3 {
                        editor.set_block_absolute(FERN, x, local_ground_y + 1, z, Some(&[AIR]), None);
                    } else if sediment_noise > 0.0 {
                        editor.set_block_absolute(TALL_GRASS_BOTTOM, x, local_ground_y + 1, z, Some(&[AIR]), None);
                    }
                }
            }
        }
    }
}