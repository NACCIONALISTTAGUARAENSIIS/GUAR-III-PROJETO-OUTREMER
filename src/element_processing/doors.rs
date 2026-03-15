use crate::block_definitions::*;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;

/// Escala vertical do Tier Governamental (Rigor 1.15:1)
const GOV_V_SCALE: f64 = 1.15;

pub fn generate_doors(editor: &mut WorldEditor, element: &ProcessedNode) {
    // ?? BESM-6: Amplia��o para portas, port�es e barreiras de controle (Ground-Aware)
    if element.tags.contains_key("door") || element.tags.contains_key("entrance") || element.tags.contains_key("barrier") {
        
        // 1. Detec��o de N�vel (Subterr�neo vs Elevado)
        let mut level = 0;
        if let Some(level_str) = element.tags.get("level") {
            if let Ok(parsed_level) = level_str.parse::<i32>() {
                level = parsed_level;
            }
        }

        let x: i32 = element.x;
        let z: i32 = element.z;

        // 2. Ground-Aware Absoluto: Sincroniza��o com o relevo LiDAR/DEM
        let ground_y = if editor.get_ground().is_some() {
            editor.get_ground_level(x, z)
        } else {
            0
        };

        // 3. Aplicação do Rigor Governamental de Escala Vertical
        // Define o deslocamento vertical exato baseado no andar
        let level_offset = (level as f64 * 4.0 * GOV_V_SCALE).round() as i32;
        let final_y = ground_y + level_offset;

        // --- Tipologia Documental e Materiais de Bras�lia ---
        let door_type = element.tags.get("door").map(|s: &String| s.as_str()).unwrap_or("");
        let entrance_type = element.tags.get("entrance").map(|s: &String| s.as_str()).unwrap_or("");
        let barrier_type = element.tags.get("barrier").map(|s: &String| s.as_str()).unwrap_or("");
        let material = element.tags.get("material").map(|s: &String| s.as_str()).unwrap_or("");
        let access = element.tags.get("access").map(|s: &String| s.as_str()).unwrap_or("");

        let mut lower_block = SPRUCE_DOOR_LOWER; // Fallback: Apartamentos do Plano
        let mut upper_block = SPRUCE_DOOR_UPPER;
        let mut is_gate = false;
        let mut is_industrial = false;

        // HEUR�STICA DE SELE��O DE MATERIAL (Rigor GDF)
        if barrier_type == "gate" || entrance_type == "gate" {
            // Port�es de garagem em Sat�lites ou cercas de Superquadra
            is_gate = true;
            lower_block = OAK_FENCE_GATE;
            upper_block = AIR; 
        } else if material == "glass" || door_type == "glass" {
            // Com�rcios Locais, W3 e Pr�dios Espelhados
            lower_block = GLASS_PANE;
            upper_block = GLASS_PANE;
        } else if material == "iron" || material == "metal" || entrance_type == "service" || entrance_type == "garage" || level < 0 {
            // Infraestrutura Cr�tica: CAESB, CEB, Bunkers e Salas de M�quina
            is_industrial = true;
            lower_block = IRON_DOOR; 
            upper_block = IRON_DOOR; 
        } else if entrance_type == "main" || door_type == "double" || name_contains_royal(element) {
            // Pal�cios (Planalto, Alvorada) e Igrejas
            lower_block = DARK_OAK_DOOR_LOWER; 
            upper_block = DARK_OAK_DOOR_UPPER;
        } else if material == "wood" || door_type == "wood" {
            // Casas residenciais cl�ssicas
            lower_block = OAK_DOOR;
            upper_block = OAK_DOOR; 
        }

        // 4. Assentamento de Soleira (Padr�o Bras�lia: Andesito Polido)
        // Isso cria a transi��o perfeita entre a cal�ada e o interior
        editor.set_block_absolute(POLISHED_ANDESITE, x, final_y, z, None, None);
        
        // 5. Impress�o da Porta na Malha Voxel
        if is_gate {
            // Port�es geralmente t�m 2 blocos de altura para seguran�a
            editor.set_block_absolute(lower_block, x, final_y + 1, z, None, None);
            editor.set_block_absolute(lower_block, x, final_y + 2, z, None, None);
        } else if is_industrial {
            // Portas de ferro usam blocos inteiros para manter a hermeticidade visual
            editor.set_block_absolute(lower_block, x, final_y + 1, z, None, None);
            editor.set_block_absolute(upper_block, x, final_y + 2, z, None, None);
        } else {
            // Portas convencionais
            editor.set_block_absolute(lower_block, x, final_y + 1, z, None, None);
            editor.set_block_absolute(upper_block, x, final_y + 2, z, None, None);
        }

        // 6. Tweak de Acessibilidade: Rampas (Slabs) para entradas principais
        if entrance_type == "main" {
            let directions = [(1,0), (-1,0), (0,1), (0,-1)];
            for (dx, dz) in directions {
                // Se o bloco adjacente for ar, coloca uma rampa de andesito
                if editor.check_for_block_absolute(x + dx, final_y, z + dz, Some(&[AIR]), None) {
                    editor.set_block_absolute(SMOOTH_STONE_SLAB, x + dx, final_y, z + dz, None, None);
                }
            }
        }
    }
}

/// Helper para detectar se o nome do elemento sugere uma entrada monumental
fn name_contains_royal(node: &ProcessedNode) -> bool {
    let name = node.tags.get("name").map(|s: &String| s.to_lowercase()).unwrap_or_default();
    name.contains("pal�cio") || name.contains("minist�rio") || name.contains("catedral") || name.contains("teatro")
}