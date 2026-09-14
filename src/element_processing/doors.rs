use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

// ============================================================================
// 🚨 PROVIDERS QUE CHAMAM ESTE MÓDULO (Injection Points):
// ============================================================================
//
// 1. **IFC Provider** (src/providers/ifc_provider.rs)
//    - Detecta IfcDoor e IfcWindow em modelos BIM
//    - Calcula largura/altura real da esquadria em milímetros
//    - Converte para blocos usando scale_h e scale_v
//    - Invoca: carve_and_place_door(editor, x, y, z, width, height, facing, tags)
//
// 2. **Buildings Processor** (src/element_processing/buildings.rs)
//    - Processa building=* do OSM com entrance=* nodes
//    - Detecta entrance=main/service/emergency nas paredes extrudadas
//    - Calcula facing baseado na normal da parede mais próxima
//    - Invoca: carve_and_place_door(editor, x, ground_y, z, 1, 2, facing, &tags)
//
// 3. **CityGML Provider** (src/providers/citygml_provider.rs)
//    - Extrai <door> e <opening> de Building LOD3/LOD4
//    - Geometria explícita de vãos em coordenadas 3D
//    - Converte polígono de abertura para retângulo AABB (width x height)
//    - Invoca: carve_and_place_door(editor, x, y, z, width, height, facing, tags)
//
// 4. **Indoor Utility Provider** (src/providers/indoor_utility_provider.rs)
//    - Processa indoor=door do Simple Indoor Tagging (SIT)
//    - Mapeia door=hinged/sliding/revolving para tipos de Block
//    - Detecta access=private/employees para selecionar materiais (ferro vs madeira)
//    - Invoca: carve_and_place_door(editor, x, level_y, z, 1, 2, facing, tags)
//
// 5. **GeoPackage Provider** (src/providers/gpkg_provider.rs)
//    - Lê tabelas de edificações com coluna "tipo_entrada" ou "door_type"
//    - Atributos como "largura_porta", "material_esquadria" viram tags
//    - Invoca: carve_and_place_door(editor, x, y, z, width, 2, facing, tags)
//
// 6. **PostGIS Provider** (src/providers/postgis_provider.rs)
//    - Query: SELECT * FROM edificacoes_entradas WHERE tipo='porta_principal'
//    - Colunas "largura_m", "altura_m", "material" traduzidas para width/height/tags
//    - Invoca: carve_and_place_door(editor, x, y, z, width_blocks, height_blocks, facing, tags)
//
// 7. **GeoJSON Provider** (src/providers/geojson_provider.rs)
//    - Features com properties.entrance = "yes" e geometry tipo Point
//    - properties.door:type, properties.width_m viram tags e dimensões
//    - Facing calculado pela direção da LineString da parede mais próxima
//    - Invoca: carve_and_place_door(editor, x, y, z, width, height, facing, tags)
//
// ============================================================================

/// Enumeração de Direções Cardeais para assentamento correto dos Block States (NBT)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DoorFacing {
    North,
    South,
    East,
    West,
}

impl DoorFacing {
    pub fn as_str(&self) -> &'static str {
        match self {
            DoorFacing::North => "north",
            DoorFacing::South => "south",
            DoorFacing::East => "east",
            DoorFacing::West => "west",
        }
    }
}

/// 🚨 BESM-6 (Arquitetura Governamental): O doors.rs não "caça" portas no mapa.
/// Ele é uma API chamada pelo gerador de paredes (`buildings.rs` ou `ifc_provider.rs`)
/// quando este detecta um nó de entrada (entrance) ou uma IfcDoor.
///
/// O gerador de paredes passa as coordenadas exatas, a orientação da parede e as dimensões.
/// O algoritmo `carve_and_place_door` abre o vão (void) na parede já existente e insere a porta com NBT perfeito.
pub fn carve_and_place_door(
    editor: &mut WorldEditor,
    x: i32,
    y_base: i32,
    z: i32,
    width: i32,
    height: i32,
    facing: DoorFacing,
    tags: &HashMap<String, String>,
) {
    // 1. Tipologia Documental e Materiais (Escala de Brasília)
    let door_type = tags.get("door").map(String::as_str).unwrap_or("");
    let entrance_type = tags.get("entrance").map(String::as_str).unwrap_or("");
    let barrier_type = tags.get("barrier").map(String::as_str).unwrap_or("");
    let material = tags.get("material").map(String::as_str).unwrap_or("wood");

    // Fallbacks base
    let mut base_door_id = SPRUCE_DOOR_LOWER.id; // 🚨 Acesso correto ao campo público .id
    let mut is_gate = false;
    let mut is_glass_pane = false;

    // HEURÍSTICA DE SELEÇÃO DE MATERIAL (Rigor GDF - Material Driven)
    if barrier_type == "gate" || entrance_type == "gate" {
        is_gate = true;
    } else if material == "glass" || door_type == "glass" || door_type == "sliding" || door_type == "revolving" {
        // Blindex Modernista: Oscar Niemeyer detestava portas rústicas
        is_glass_pane = true;
    } else if material == "iron" || material == "metal" || entrance_type == "service" || entrance_type == "garage" {
        base_door_id = IRON_DOOR.id;
    } else if entrance_type == "main" || door_type == "double" || name_contains_royal(tags) {
        base_door_id = DARK_OAK_DOOR_LOWER.id;
    } else if material == "wood" || door_type == "wood" {
        base_door_id = OAK_DOOR.id; // Porta de madeira genérica
    }

    // 2. Cálculo do Vão (Void Carving)
    // Uma porta em Brasília não tem 1x2m fixos. Ela obedece à largura e altura passadas pelo BIM ou OSM.
    // Usamos f32 para o cálculo simétrico perfeito (width/2) exigido pelo BESM-6
    let half_w = (width as f32 / 2.0).floor() as i32;

    // Calcula o vetor de extrusão da largura perpendicular à direção da porta (facing).
    // Ex: Se a porta olha para o Norte (Z-), a largura se espalha no eixo X.
    let (vec_w_x, vec_w_z) = match facing {
        DoorFacing::North | DoorFacing::South => (1, 0),
        DoorFacing::East | DoorFacing::West => (0, 1),
    };

    // Calcula o vetor de deslocamento da Rampa de Acessibilidade (apontando para fora da porta)
    let (ramp_x, ramp_z) = match facing {
        DoorFacing::North => (0, -1),
        DoorFacing::South => (0, 1),
        DoorFacing::East => (1, 0),
        DoorFacing::West => (-1, 0),
    };

    // O offset de correção lida com as portas pares (Ex: largura 2, vai de -1 a 0)
    // Se a largura for par, o loop iterará corretamente.
    let end_offset = if width % 2 == 0 { width - half_w - 1 } else { width - half_w };

    // 3. Execução da Escavação e Assentamento
    for w in -half_w..=end_offset.max(0) {
        let px = x + (w * vec_w_x);
        let pz = z + (w * vec_w_z);

        // 🚨 PRESERVAÇÃO ESTRUTURAL DO CHÃO:
        // O motor NUNCA destrói o chão (y_base) para colocar andesito indiscriminadamente.
        // Ele apenas limpa a parede (AIR) do y_base + 1 até o height.
        for h in 1..=height {
            editor.set_block_absolute(AIR, px, y_base + h, pz, None, None);
        }

        // 4. Injeção de Propriedades (Block States)
        if is_glass_pane {
            // Em portas de vidro ou portas giratórias gigantes do Plano Piloto,
            // não usamos portas do Minecraft. O vão vira uma passagem monumental livre de ar (AIR),
            // ou recebe um toldo/cobertura superior. Acessibilidade 100%.
            for h in 3..=height {
                editor.set_block_absolute(CYAN_STAINED_GLASS, px, y_base + h, pz, None, None);
            }
        } else if is_gate {
            // Portões de Garagem (Gigantes e Abertos)
            // Portões industriais são vazados e altos
            for h in 1..=height {
                if h % 2 == 0 {
                    editor.set_block_absolute(IRON_BARS, px, y_base + h, pz, None, None);
                } else {
                    editor.set_block_absolute(AIR, px, y_base + h, pz, None, None); // Fluxo de ar
                }
            }
        } else {
            // 🚨 PORTAS NATIVAS (Minecraft Block States)
            let max_h = height.min(2);

            // 🚨 CÁLCULO DE SIMETRIA MATEMÁTICA (Hinge Mirroring)
            // Se w < 0 (metade esquerda do vão), a dobradiça vai na esquerda.
            // Se w >= 0 (metade direita do vão), a dobradiça vai na direita.
            // Isso garante que portas pares e ímpares abram corretamente do centro para fora.
            let hinge_side = if w < 0 { "left" } else { "right" };

            // O Arnis lê os blockstates através do argumento genérico, mas a assinatura correta no WorldEditor
            // é: (block_type, x, y, z, filter_array_option, nbt_properties_option)

            // Bloco Inferior (Lower Half)
            let mut lower_props = HashMap::new();
            lower_props.insert("facing".to_string(), Value::String(facing.as_str().to_string()));
            lower_props.insert("half".to_string(), Value::String("lower".to_string()));
            lower_props.insert("hinge".to_string(), Value::String(hinge_side.to_string()));
            lower_props.insert("open".to_string(), Value::String("false".to_string()));

            // 🚨 Correção da assinatura: Usar set_block_with_properties_absolute para blocos com NBT
            let lower_door = BlockWithProperties::new(Block::new(base_door_id), Some(Value::Compound(lower_props)));
            editor.set_block_with_properties_absolute(lower_door, px, y_base + 1, pz, None, None);

            if max_h == 2 {
                // Bloco Superior (Upper Half)
                let mut upper_props = HashMap::new();
                upper_props.insert("facing".to_string(), Value::String(facing.as_str().to_string()));
                upper_props.insert("half".to_string(), Value::String("upper".to_string()));
                upper_props.insert("hinge".to_string(), Value::String(hinge_side.to_string()));
                upper_props.insert("open".to_string(), Value::String("false".to_string()));

                let upper_door = BlockWithProperties::new(Block::new(base_door_id), Some(Value::Compound(upper_props)));
                editor.set_block_with_properties_absolute(upper_door, px, y_base + 2, pz, None, None);
            }

            // Bandeira (Transom Window) acima da porta se a altura do vão for maior que 2
            if height > 2 {
                for h in 3..=height {
                    // O material da bandeira segue o material da porta
                    let transom_block = if base_door_id == IRON_DOOR.id { IRON_BARS } else { GLASS_PANE };
                    editor.set_block_absolute(transom_block, px, y_base + h, pz, None, None);
                }
            }
        }

        // 5. Tweak de Acessibilidade Vetorial Correta
        // Rampas do GDF só são inseridas se a frente e o lado não possuírem blocos opacos flutuantes
        if entrance_type == "main" && (is_glass_pane || base_door_id == IRON_DOOR.id) {
            let rampa_x = px + ramp_x;
            let rampa_z = pz + ramp_z;

            // Lógica Topográfica Real: Pega a altura do solo no pé da rampa
            let ramp_ground_y = editor.get_ground_level(rampa_x, rampa_z);

            // A rampa só é criada se o degrau para o prédio for de exatos 1 bloco.
            if y_base == ramp_ground_y + 1 {
                // Slab inferior na coordenada exata do chão à frente da porta
                editor.set_block_absolute(SMOOTH_STONE_SLAB, rampa_x, ramp_ground_y, rampa_z, None, None);
            } else if y_base == ramp_ground_y {
                // Se a porta e o chão externo estão no mesmo nível, trocamos o chão externo
                // por asfalto/concreto tátil (Acessibilidade) em frente à porta
                editor.set_block_absolute(POLISHED_ANDESITE, rampa_x, ramp_ground_y, rampa_z, Some(&[GRASS_BLOCK, DIRT, SAND]), None);
            }
        }
    }
}

/// Helper para detectar se o nome do elemento sugere uma entrada monumental (Governamental)
fn name_contains_royal(tags: &HashMap<String, String>) -> bool {
    let name = tags
        .get("name")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    name.contains("palácio") // 🚨 UTF-8 Curado
        || name.contains("ministério")
        || name.contains("catedral")
        || name.contains("teatro")
        || name.contains("monumento")
        || name.contains("supremo")
        || name.contains("tribunal")
}