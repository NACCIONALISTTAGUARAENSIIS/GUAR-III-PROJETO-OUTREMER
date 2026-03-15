use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use crate::deterministic_rng::coord_rng;
use rand::Rng;

// ?? BESM-6: Constantes de Escala Governamental
const V_SCALE: f64 = 1.15;
const _H_SCALE: f64 = 1.33; // Usada em larguras de vias, muros s�o linhas grossura 1.

// ?? BESM-6 Tweak: Matriz Culling Avançada O(1)
// Muros nunca devem destruir rodovias, cal�adas nobres, �gua ou �rvores do cerrado.
const URBAN_CLASH_BLACKLIST: &[Block] = &[
    BLACK_CONCRETE,    // Asfalto Monumental e Rodovias
    POLISHED_BASALT,   // Asfalto da W3
    WHITE_CONCRETE,    // Faixas de pedestre e rua
    YELLOW_CONCRETE,   // Ciclovias/Faixas centrais
    RED_CONCRETE,      // Ciclovia do Guar�/Parque da Cidade
    POLISHED_ANDESITE, // Cal�adas VIP e Passarelas
    SMOOTH_STONE_SLAB, // Cal�ad�o
    WATER,             // N�o ergue muros DENTRO do lago parano�, a n�o ser que seja ponte
    OAK_LOG,           // ?? Org�nico: Respeita �rvores nativas do cerrado
    DARK_OAK_LOG,
    JUNGLE_LOG,
    ACACIA_LOG,
    SPRUCE_LOG,
];

pub fn generate_barriers(editor: &mut WorldEditor, element: &ProcessedElement) {
    let mut barrier_material: Block = COBBLESTONE_WALL;
    let mut barrier_height: f64 = 2.0; 
    
    // Flags de Tipologia Arquitet�nica do DF
    let mut is_cobogo = false;         
    let mut is_guardrail = false;      
    let mut is_green_railing = false;  
    let mut is_hedge = false;          
    let mut is_chainlink = false;      
    let mut is_sound_wall = false;     
    let mut is_rural_wire = false;     
    let mut is_security_wall = false;  
    let mut is_institutional_fence = false; // Embaixadas, Quart�is, Itamaraty
    let mut is_median_barrier = false; // Canteiros centrais baixos

    let tags = element.tags();
    
    let is_residential = tags.get("residential").is_some() || tags.get("security").is_some();
    let name = tags.get("name").map(|s: &String| s.to_lowercase()).unwrap_or_default();
    
    // An�lise Sem�ntica Fina
    let is_embassy_or_gov = name.contains("embaixada") || name.contains("minist�rio") || name.contains("pal�cio") || tags.get("amenity") == Some(&"embassy".to_string());
    let is_school_or_military = tags.get("amenity") == Some(&"school".to_string()) || tags.get("landuse") == Some(&"military".to_string());

    match tags.get("barrier").map(|s: &String| s.as_str()) {
        Some("bollard") => {
            barrier_material = DIORITE_WALL; 
            barrier_height = 0.8;
        }
        Some("kerb") => { return; }
        Some("hedge") => {
            barrier_material = AZALEA_LEAVES; 
            barrier_height = 1.5;
            is_hedge = true;
        }
        Some("guard_rail" | "crash_barrier" | "jersey_barrier") => {
            barrier_material = ANDESITE_WALL; 
            barrier_height = 1.0;
            is_guardrail = true;
        }
        Some("sound_wall" | "acoustic_barrier") => {
            barrier_material = CYAN_STAINED_GLASS; 
            barrier_height = 4.0;
            is_sound_wall = true;
        }
        Some("fence") => {
            match tags.get("fence_type").map(|s: &String| s.as_str()) {
                Some("railing" | "bars" | "krest") => {
                    barrier_material = IRON_BARS; 
                    barrier_height = 2.0;
                    if is_embassy_or_gov {
                        is_institutional_fence = true;
                        barrier_height = 2.5;
                    } else {
                        is_green_railing = true; 
                    }
                }
                Some("chain_link" | "metal" | "wire" | "metal_bars") => {
                    barrier_material = IRON_BARS;
                    barrier_height = 3.0; 
                    is_chainlink = true;
                    if is_school_or_military { barrier_height = 4.0; is_security_wall = true; }
                }
                Some("barbed_wire" | "electric") => {
                    barrier_material = IRON_BARS; // Usar iron_bars com concertina por cima � mais seguro pra IA dos mobs
                    barrier_height = 2.0;
                    is_rural_wire = true;
                }
                Some("wood" | "split_rail" | "panel" | "pole") => {
                    barrier_material = SPRUCE_FENCE; 
                    barrier_height = 1.5;
                }
                Some("concrete" | "stone") => {
                    barrier_material = STONE_BRICK_WALL;
                    barrier_height = 2.0;
                }
                Some("glass") => {
                    barrier_material = GLASS_PANE;
                    barrier_height = 1.5;
                }
                Some("masonry") => {
                    barrier_material = BRICK;
                    barrier_height = 2.5;
                    is_security_wall = is_residential;
                }
                _ => {
                    if tags.get("landuse").map(|s: &String| s.as_str()) == Some("farmland") {
                        barrier_material = OAK_FENCE;
                        barrier_height = 1.5;
                        is_rural_wire = true;
                    } else if is_embassy_or_gov {
                        barrier_material = IRON_BARS;
                        is_institutional_fence = true;
                        barrier_height = 2.5;
                    } else {
                        barrier_material = IRON_BARS; 
                        barrier_height = 2.0;
                    }
                }
            }
        }
        Some("wall") | Some("retaining_wall") | Some("city_wall") => {
            if let Some(wall_type) = tags.get("wall_type") {
                if wall_type == "hollow" || wall_type == "perforated" || wall_type == "cobogo" {
                    is_cobogo = true;
                }
            }
            
            barrier_material = STONE_BRICKS;
            
            if tags.get("barrier") == Some(&"retaining_wall".to_string()) {
                barrier_material = COBBLESTONE; 
                barrier_height = 3.0;
            } else {
                barrier_height = 2.5;
                is_security_wall = true; 
            }
        }
        Some("retaining_wall" | "ditch") => {
            is_median_barrier = true;
            barrier_material = SMOOTH_STONE;
            barrier_height = 0.5;
        }
        _ => {}
    }

    // Override Expl�cito de Altura (Padr�o OSM e Governo)
    if let Some(h_str) = tags.get("height").or_else(|| tags.get("height:barrier")) {
        if let Ok(h) = h_str.trim_end_matches('m').trim().parse::<f64>() {
            barrier_height = h;
        }
    }

    // Override Expl�cito de Material
    if let Some(barrier_mat) = tags.get("material").or_else(|| tags.get("surface")) {
        match barrier_mat.as_str() {
            "brick" => barrier_material = BRICK,
            "concrete" => barrier_material = LIGHT_GRAY_CONCRETE,
            "metal" => barrier_material = IRON_BARS,
            "stone" => barrier_material = STONE_BRICKS,
            "wood" => barrier_material = SPRUCE_PLANKS,
            "glass" => barrier_material = GLASS_PANE,
            "plaster" => barrier_material = WHITE_CONCRETE, 
            _ => {}
        }
    }

    // Tweak para Muros e Grades Brancas/Verdes 
    if let Some(color) = tags.get("colour") {
        if color == "white" {
            if barrier_material == STONE_BRICKS || barrier_material == BRICK {
                barrier_material = SMOOTH_QUARTZ; 
            } else if barrier_material == IRON_BARS {
                is_green_railing = false; 
            }
        } else if color == "green" && barrier_material == IRON_BARS {
            is_green_railing = true;
        }
    }

    if let ProcessedElement::Way(way) = element {
        // Altura V_SCALE
        let scaled_height = barrier_height * V_SCALE;
        let mut wall_height_blocks = scaled_height.floor() as i32;
        wall_height_blocks = wall_height_blocks.max(1);

        // Se o valor decimal for significativo, preparamos o topo da parede para receber Slab
        let has_half_slab_top = (scaled_height - wall_height_blocks as f64) > 0.2;

        let is_solid_block = barrier_material == BRICK 
            || barrier_material == LIGHT_GRAY_CONCRETE 
            || barrier_material == STONE_BRICKS 
            || barrier_material == QUARTZ_BRICKS
            || barrier_material == SMOOTH_QUARTZ
            || barrier_material == WHITE_CONCRETE
            || barrier_material == OAK_PLANKS
            || barrier_material == SPRUCE_PLANKS
            || barrier_material == COBBLESTONE;

        // ?? BESM-6: Controle de Duplicatas do Bresenham para evitar Overdraw e Pilares Duplos
        let mut last_point: Option<(i32, i32)> = None;

        // Processa todos os segmentos de reta do muro
        for i in 1..way.nodes.len() {
            let prev = &way.nodes[i - 1];
            let cur = &way.nodes[i];

            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);
            
            // ?? RESET por Segmento (Corrige o espa�amento irregular de postes longos)
            let mut segment_distance = 0; 

            for (bx, _, bz) in bresenham_points {
                if let Some((lx, lz)) = last_point {
                    if lx == bx && lz == bz { continue; } // Pula duplicata exata nas esquinas
                }
                last_point = Some((bx, bz));
                segment_distance += 1;
                
                // Hash Espacial Org�nico O(1)
                let mut rng = coord_rng(bx, bz, way.id);

                // ?? TOPOGRAFIA SUAVE: Evita degraus absurdos sob muros em ladeiras
                // Pega a m�dia de altura local em um raio 3x3 se houver terreno ativo
                let mut local_y_sum = 0;
                let mut count = 0;
                for ox in -1i32..=1i32 {
                    for oz in -1i32..=1i32 {
                        local_y_sum += editor.get_ground_level(bx + ox, bz + oz);
                        count += 1;
                    }
                }
                let avg_ground_y = (local_y_sum as f64 / count as f64).round() as i32;
                let exact_ground_y = editor.get_ground_level(bx, bz);

                // Culling de Intersec��o: Se bateu num asfalto monumental ou �gua, pula.
                if editor.check_for_block_absolute(bx, exact_ground_y, bz, Some(URBAN_CLASH_BLACKLIST), None) {
                    continue; 
                }

                // ?? FUNDA��O EST�VEL E FLUIDA
                // Para evitar buracos de ar debaixo de muros em subidas, o muro "afunda" 1 bloco ou mais
                // dependendo da m�dia local, consolidando o aterro.
                let foundation_y = avg_ground_y.min(exact_ground_y) - 1;
                let is_foundation_safe = !editor.check_for_block_absolute(bx, foundation_y, bz, Some(&[IRON_BLOCK, AIR]), None);
                
                if is_foundation_safe {
                    let base_mat = if is_solid_block { barrier_material } else { SMOOTH_STONE };
                    for fy in foundation_y..=exact_ground_y {
                        editor.set_block_absolute(base_mat, bx, fy, bz, Some(&[DIRT, GRASS_BLOCK, SAND, PODZOL, COARSE_DIRT, WATER]), None);
                    }
                }

                // SUBIDA VERTICAL DO MURO
                for dy in 0..=wall_height_blocks {
                    let absolute_y = exact_ground_y + dy;

                    // 1. Cercas Institucionais do Plano Piloto (Embaixadas, Base de pedra + Postes Grossos)
                    if is_institutional_fence {
                        if dy == 0 {
                            editor.set_block_absolute(STONE_BRICKS, bx, absolute_y, bz, None, None);
                        } else if rng.random_bool(0.2) { // Espa�amento org�nico
                            editor.set_block_absolute(STONE_BRICK_WALL, bx, absolute_y, bz, None, None);
                        } else {
                            editor.set_block_absolute(IRON_BARS, bx, absolute_y, bz, None, None);
                        }
                    }
                    // 2. L�gica do COBOG�
                    else if is_cobogo && is_solid_block && dy > 1 {
                        let is_hole = (bx + absolute_y + bz) % 2 != 0;
                        if !is_hole {
                            editor.set_block_absolute(barrier_material, bx, absolute_y, bz, None, None);
                        }
                    } 
                    // 3. Muros Maci�os (Weathering Realista e Pilares)
                    else if is_solid_block && !is_cobogo {
                        let mut final_mat = barrier_material;
                        
                        if dy <= 1 && rng.random_bool(0.25) && (barrier_material == WHITE_CONCRETE || barrier_material == SMOOTH_QUARTZ) {
                            final_mat = WHITE_TERRACOTTA; 
                        } else if dy > 0 && rng.random_bool(0.12) {
                            final_mat = match barrier_material {
                                BRICK => BRICK_STAIRS, 
                                STONE_BRICKS => CRACKED_STONE_BRICKS,
                                COBBLESTONE => MOSSY_COBBLESTONE, 
                                SMOOTH_QUARTZ | WHITE_CONCRETE => DIORITE, 
                                _ => barrier_material,
                            };
                        }
                        
                        // Quebra a monotonia colocando pilares em muros longos
                        if segment_distance % 6 == 0 && (barrier_material == BRICK || barrier_material == STONE_BRICKS) {
                            final_mat = POLISHED_ANDESITE;
                        }

                        editor.set_block_absolute(final_mat, bx, absolute_y, bz, None, None);
                    } 
                    // 4. L�gica de GRADES VERDES (NOVACAP) e ALAMBRADOS
                    else if barrier_material == IRON_BARS {
                        let is_post = segment_distance % 5 == 0;

                        if dy == 0 && (is_chainlink || is_green_railing) {
                            editor.set_block_absolute(SMOOTH_STONE, bx, absolute_y, bz, None, None);
                        } else if is_green_railing {
                            let pipe = if is_post { DARK_OAK_FENCE } else { IRON_BARS };
                            editor.set_block_absolute(pipe, bx, absolute_y, bz, None, None);
                        } else if is_chainlink && is_post {
                            editor.set_block_absolute(IRON_BLOCK, bx, absolute_y, bz, None, None);
                        } else {
                            editor.set_block_absolute(IRON_BARS, bx, absolute_y, bz, None, None);
                        }
                    }
                    // 5. L�gica de CERCA RURAL COM MOUR�ES 
                    else if is_rural_wire {
                        let is_post = segment_distance % 4 == 0;
                        if is_post {
                            editor.set_block_absolute(SPRUCE_LOG, bx, absolute_y, bz, None, None);
                        } else if dy > 0 && dy % 2 == 0 {
                            editor.set_block_absolute(IRON_BARS, bx, absolute_y, bz, None, None);
                        }
                    }
                    // 6. L�gica das BARREIRAS AC�STICAS DA EPTG
                    else if is_sound_wall {
                        if dy <= 2 {
                            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, bx, absolute_y, bz, None, None); 
                        } else {
                            if segment_distance % 4 == 0 {
                                editor.set_block_absolute(IRON_BLOCK, bx, absolute_y, bz, None, None); 
                            } else {
                                editor.set_block_absolute(barrier_material, bx, absolute_y, bz, None, None); 
                            }
                        }
                    }
                    // 7. L�gica de CERCA VIVA ORG�NICA
                    else if is_hedge {
                        let leaf_mat = if rng.random_bool(0.15) { FLOWERING_AZALEA } else { barrier_material };
                        editor.set_block_absolute(leaf_mat, bx, absolute_y, bz, None, None);
                        
                        if dy == 0 && segment_distance % 3 == 0 {
                            editor.set_block_absolute(OAK_LOG, bx, absolute_y, bz, None, None);
                        }
                    }
                    // 8. Canteiro Central Baixo
                    else if is_median_barrier {
                        if dy == 0 { editor.set_block_absolute(SMOOTH_STONE_SLAB, bx, absolute_y, bz, None, None); }
                    }
                    else {
                        editor.set_block_absolute(barrier_material, bx, absolute_y, bz, None, None);
                    }
                }

                // ============================================
                // C�PULA DA BARREIRA E SEGURAN�A NO TOPO
                // ============================================
                
                let mut top_y = exact_ground_y + wall_height_blocks + 1;
                
                // Se foi adicionado um half slab no topo, ajustamos o Y da concertina pra n�o flutuar
                let mut has_slab_now = false;
                
                if has_half_slab_top && is_solid_block && !is_cobogo {
                    let top_mat = match barrier_material {
                        BRICK => BRICK_SLAB,
                        STONE_BRICKS => STONE_BRICK_SLAB,
                        WHITE_CONCRETE | SMOOTH_QUARTZ => QUARTZ_SLAB_BOTTOM,
                        OAK_PLANKS => OAK_SLAB,
                        SPRUCE_PLANKS => SPRUCE_SLAB,
                        _ => SMOOTH_STONE_SLAB,
                    };
                    editor.set_block_absolute(top_mat, bx, top_y, bz, None, None);
                    has_slab_now = true;
                    top_y += 1; // Pr�ximo bloco (se houver seguran�a) sobe
                }

                if is_guardrail {
                    if !has_slab_now { editor.set_block_absolute(SMOOTH_STONE_SLAB, bx, top_y, bz, None, None); }
                } 
                else if is_sound_wall {
                    editor.set_block_absolute(IRON_TRAPDOOR, bx, top_y, bz, None, None);
                }
                else if is_security_wall && wall_height_blocks >= 2 && is_solid_block {
                    // Clusteriza��o da Concertina (Hash Espacial: Forma aglomerados de 3-5 blocos e pula)
                    let cluster_noise = ((bx as f64 * 0.5).sin() + (bz as f64 * 0.5).cos()).abs();
                    if cluster_noise > 0.4 {
                        let security_mat = if rng.random_bool(0.4) { COBWEB } else { IRON_BARS }; // Mistura visual agressiva
                        editor.set_block_absolute(security_mat, bx, top_y, bz, None, None);
                    }
                }
                else if is_chainlink && wall_height_blocks >= 3 {
                    // Farpado inclinado no topo de alambrado
                    editor.set_block_absolute(IRON_BARS, bx, top_y, bz, None, None);
                }
            }
        }
    }
}

pub fn generate_barrier_nodes(editor: &mut WorldEditor<'_>, node: &ProcessedNode) {
    let ground_y = editor.get_ground_level(node.x, node.z);

    match node.tags.get("barrier").map(|s: &String| s.as_str()) {
        Some("bollard") => {
            editor.set_block_absolute(DIORITE_WALL, node.x, ground_y + 1, node.z, None, None);
            editor.set_block_absolute(SMOOTH_STONE_SLAB, node.x, ground_y + 2, node.z, None, None);
        }
        Some("stile" | "gate" | "swing_gate" | "lift_gate" | "entrance") => {
            // ?? TWEAK DF: Resolve o bug de port�es n�o abrirem muros texturizados
            let replaceable_walls: &[Block] = &[
                COBBLESTONE_WALL, OAK_FENCE, STONE_BRICK_WALL, AZALEA_LEAVES, OAK_LEAVES,
                STONE_BRICK_SLAB, IRON_BARS, BRICK, LIGHT_GRAY_CONCRETE, GLASS_PANE,
                SMOOTH_STONE_SLAB, QUARTZ_BRICKS, STONE_BRICKS, WHITE_CONCRETE, SMOOTH_QUARTZ,
                DARK_OAK_FENCE, SPRUCE_FENCE, IRON_BLOCK, COBWEB, ANDESITE_WALL, DIORITE,
                WHITE_TERRACOTTA, CYAN_STAINED_GLASS, IRON_TRAPDOOR, CRACKED_STONE_BRICKS, MOSSY_COBBLESTONE,
                BRICK_STAIRS, POLISHED_ANDESITE, SMOOTH_STONE
            ];

            let is_metal_context = editor.check_for_block_absolute(node.x + 1, ground_y + 1, node.z, Some(&[IRON_BARS, STONE_BRICKS, BRICK, IRON_BLOCK, SMOOTH_STONE]), None) ||
                                   editor.check_for_block_absolute(node.x, ground_y + 1, node.z + 1, Some(&[IRON_BARS, STONE_BRICKS, BRICK, IRON_BLOCK, SMOOTH_STONE]), None);
            
            let gate_material = if is_metal_context { IRON_DOOR } else { OAK_DOOR };

            editor.set_block_absolute(
                gate_material, 
                node.x,
                ground_y + 1,
                node.z,
                Some(replaceable_walls),
                None,
            );
            
            for dy in 2i32..=3i32 {
                let fill = if dy == 2 { gate_material } else { AIR };
                editor.set_block_absolute(
                    fill,
                    node.x,
                    ground_y + dy,
                    node.z,
                    Some(replaceable_walls),
                    None,
                );
            }
        }
        Some("block") => {
            editor.set_block_absolute(STONE, node.x, ground_y + 1, node.z, None, None);
        }
        None => {}
        _ => {}
    }
}