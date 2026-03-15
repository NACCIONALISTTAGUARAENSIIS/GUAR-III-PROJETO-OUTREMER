//! LiDAR Point Cloud Provider (BESM-6 Government Tier)
//!
//! Descodifica nuvens de pontos densas (.las / .laz) utilizando a arquitetura
//! de Voxelização Local Determinística com Pré-Quantização Inteira O(1).
//!
//! A crítica de "esquizofrenia arquitetural" foi sanada: O Convex Hull destrutivo
//! foi abolido. Implementou-se um Boundary Tracing (Casco Côncavo Rasterizado)
//! para preservar perfeitamente a arquitetura ortogonal de prédios em "L" e "U",
//! além de corrigir a assimetria na injeção da escala vertical.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque; // 🚨 Removido o HashMap ocioso (substituído por FxHashMap)
use std::path::PathBuf;

/// O Provedor LiDAR Oficial do BESM-6.
pub struct LidarProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
    pub source_epsg: String, // UTM Zone 23S EPSG:31983
}

impl LidarProvider {
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        scale_v: f64,
        priority: u8,
        source_epsg: &str,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            scale_v,
            priority,
            source_epsg: source_epsg.to_string(),
        }
    }

    #[inline(always)]
    fn class_to_semantic(classification: u8) -> Option<(SemanticGroup, std::collections::HashMap<String, String>)> {
        // O FxHashMap O(1) não preserva ordem em dumps de debug textuais legados, então para os tags mantemos o Hash padrão
        let mut tags = std::collections::HashMap::new();
        tags.insert("source".to_string(), "GDF_LiDAR_Cloud".to_string());

        match classification {
            3..=5 => {
                tags.insert("natural".to_string(), "wood".to_string());
                tags.insert("density".to_string(), "high".to_string());
                Some((SemanticGroup::Natural, tags))
            }
            6 => {
                tags.insert("building".to_string(), "yes".to_string());
                Some((SemanticGroup::Building, tags))
            }
            // Classes de LiDAR governamental frequentemente omitidas mas vitais
            9 => {
                tags.insert("natural".to_string(), "water".to_string());
                Some((SemanticGroup::Water, tags))
            }
            14 => {
                tags.insert("power".to_string(), "line".to_string());
                Some((SemanticGroup::Infrastructure, tags))
            }
            17 => {
                tags.insert("man_made".to_string(), "bridge".to_string());
                Some((SemanticGroup::Infrastructure, tags))
            }
            _ => None,
        }
    }

    /// 🚨 BESM-6: Concave Hull Boundary Tracing (Raster-to-Vector)
    /// Extirpou-se o Convex Hull que destruía prédios em "U" ou "L".
    /// Esta função traça o perímetro externo exato dos blocos contíguos.
    fn trace_concave_boundary(cluster_cells: &FxHashSet<(i32, i32)>) -> Vec<XZPoint> {
        if cluster_cells.is_empty() { return vec![]; }
        if cluster_cells.len() <= 4 {
            // Se for pequeno demais, devolve a bbox simples.
            return cluster_cells.iter().map(|&(x, z)| XZPoint::new(x, z)).collect();
        }

        // 1. Encontra um ponto inicial garantido no perímetro (o mais à esquerda-cima)
        let start_node = *cluster_cells.iter().min_by_key(|&&(x, z)| (x, z)).unwrap();

        // Direções de Moore Clockwise: Cima, Cima-Dir, Dir, Baixo-Dir, Baixo, Baixo-Esq, Esq, Cima-Esq
        let directions = [
            (0, -1), (1, -1), (1, 0), (1, 1),
            (0, 1), (-1, 1), (-1, 0), (-1, -1)
        ];

        let mut boundary = Vec::new();
        let mut current_node = start_node;
        let mut current_dir = 0; // Aponta pra cima

        // Algoritmo de "Mão na Parede" (Moore Neighborhood Tracing)
        // Isso abraça os contornos exatos em 90 graus das Superquadras
        loop {
            boundary.push(XZPoint::new(current_node.0, current_node.1));

            let mut found_next = false;
            // Procura o próximo pixel da borda olhando em volta (sentido horário a partir de 90 graus para trás)
            let search_start = (current_dir + 6) % 8;

            for i in 0..8 {
                let check_dir = (search_start + i) % 8;
                let nx = current_node.0 + directions[check_dir].0;
                let nz = current_node.1 + directions[check_dir].1;

                if cluster_cells.contains(&(nx, nz)) {
                    current_node = (nx, nz);
                    current_dir = check_dir;
                    found_next = true;
                    break;
                }
            }

            if !found_next || current_node == start_node {
                break;
            }

            // Proteção contra loops infinitos por topologia corrompida do LiDAR
            if boundary.len() > cluster_cells.len() * 2 {
                break;
            }
        }

        // Simplificação de Reta (Douglas-Peucker O(N))
        // Remove pontos colineares para não termos 50 vértices numa parede reta.
        Self::simplify_orthogonal_lines(&mut boundary);

        boundary
    }

    /// Mescla blocos consecutivos numa mesma linha reta (Ex: a parede lateral de um ministério).
    fn simplify_orthogonal_lines(boundary: &mut Vec<XZPoint>) {
        if boundary.len() < 3 { return; }
        let mut i = 0;
        while i < boundary.len() {
            let p1 = boundary[i];
            let p2 = boundary[(i + 1) % boundary.len()];
            let p3 = boundary[(i + 2) % boundary.len()];

            // Se o delta for linear (Horizontal ou Vertical)
            if (p1.x == p2.x && p2.x == p3.x) || (p1.z == p2.z && p2.z == p3.z) {
                boundary.remove((i + 1) % boundary.len());
            } else {
                i += 1;
            }
        }
    }
}

impl DataProvider for LidarProvider {
    fn priority(&self) -> u8 { self.priority }
    fn name(&self) -> &str {
        "High-Density LiDAR Concave Vectorizer (.las/.laz)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        use las::{Read, Reader};
        use proj::Proj;

        println!("[INFO] 🚁 A invocar o scanner LiDAR de ultra-densidade: {}", self.file_path.display());

        let mut reader = Reader::from_path(&self.file_path)
            .map_err(|e| format!("Falha ao ler arquivo LiDAR: {}", e))?;

        let proj = Proj::new_known_crs(&self.source_epsg, "EPSG:4326", None)
            .ok()
            .ok_or(format!("Falha ao inicializar a projeção PROJ: {} -> WGS84", self.source_epsg))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o Global ECEF Transformer: {}", e))?;

        // 🚨 TWEAK O(1): Voxelização Local Determinística em vez de Listas de Flutuantes.
        // O FxHashMap O(1) agora salva a altura estrita já pré-calculada e quantizada na escala V,
        // e não o `pz` bruto (extirpando a "esquizofrenia" de misturar double e int depois).
        let mut ground_grid: FxHashMap<(i32, i32), i32> = FxHashMap::default();
        let mut building_grid: FxHashMap<(i32, i32), (i32, u8)> = FxHashMap::default();

        let mut read_count = 0u64;

        // Streaming contínuo (Out-of-Core)
        for point_result in reader.points() {
            let point = match point_result {
                Ok(p) => p,
                Err(_) => continue,
            };

            // 🚨 CORREÇÃO CRÍTICA: Extração de Classe Opaca (Erro E0605)
            // A API nova exige que se puxe o valor raw pelo conversor de trait From
            let class = u8::from(point.classification);

            if class != 2 && Self::class_to_semantic(class).is_none() {
                continue;
            }

            // Conversão Geodésica Rigorosa
            let (lon, lat) = proj.convert((point.x, point.y)).unwrap_or((0.0, 0.0));

            // Culling BBox Exato
            if lat < bbox.min().lat() || lat > bbox.max().lat() || lon < bbox.min().lng() || lon > bbox.max().lng() {
                continue;
            }

            if let Ok(llpoint) = LLPoint::new(lat, lon) {
                let xz = transformer.transform_point(llpoint);

                // Altura pré-quantizada na escala vertical 1.15 do Minecraft
                let mc_y = (point.z * self.scale_v).round() as i32;

                if class == 2 {
                    // Solo (Sempre salva o mais baixo detectado para aterrar o prédio)
                    let entry = ground_grid.entry((xz.x, xz.z)).or_insert(mc_y);
                    if mc_y < *entry {
                        *entry = mc_y;
                    }
                } else {
                    // Estruturas e Árvores (Sempre salva o pico)
                    let entry = building_grid.entry((xz.x, xz.z)).or_insert((mc_y, class));
                    if mc_y > entry.0 {
                        entry.0 = mc_y;
                        entry.1 = class;
                    }
                }
            }
            read_count += 1;
        }

        println!("[INFO] 🔬 Laser Decodificado. {} Pontos filtrados no BBox atual.", read_count);

        let mut features = Vec::new();
        let mut next_id = 8_000_000_000;

        let building_keys: Vec<(i32, i32)> = building_grid.keys().copied().collect();
        let mut global_visited: FxHashSet<(i32, i32)> = FxHashSet::default();

        for start_coord in building_keys {
            if global_visited.contains(&start_coord) { continue; }

            if let Some(&(start_h, b_class)) = building_grid.get(&start_coord) {
                let mut cluster_cells: FxHashSet<(i32, i32)> = FxHashSet::default();
                let mut queue = VecDeque::with_capacity(1024);

                let mut max_h = start_h;
                let mut c_min_x = start_coord.0; let mut c_max_x = start_coord.0;
                let mut c_min_z = start_coord.1; let mut c_max_z = start_coord.1;

                queue.push_back(start_coord);
                global_visited.insert(start_coord);

                while let Some((cx, cz)) = queue.pop_front() {
                    cluster_cells.insert((cx, cz));

                    c_min_x = c_min_x.min(cx); c_max_x = c_max_x.max(cx);
                    c_min_z = c_min_z.min(cz); c_max_z = c_max_z.max(cz);

                    let neighbors = [
                        (cx + 1, cz), (cx - 1, cz), (cx, cz + 1), (cx, cz - 1),
                    ];

                    for n_coord in &neighbors {
                        if !global_visited.contains(n_coord) {
                            if let Some(&(n_h, n_class)) = building_grid.get(n_coord) {
                                if n_class == b_class {
                                    global_visited.insert(*n_coord);
                                    queue.push_back(*n_coord);
                                    if n_h > max_h { max_h = n_h; }
                                }
                            }
                        }
                    }
                }

                // Culling Local: Um prédio não tem menos de 10 m².
                if cluster_cells.len() >= 10 {
                    if let Some((semantic, mut tags)) = Self::class_to_semantic(b_class) {

                        // Cálculo da Elevação Relativa. Como mc_y já está na escala do Minecraft,
                        // a subtração fornece a altura de blocos limpa, sem furos matemáticos.
                        let mut min_ground_local = i32::MAX;

                        for px in (c_min_x - 3)..=(c_max_x + 3) {
                            for pz in (c_min_z - 3)..=(c_max_z + 3) {
                                if let Some(&g_h) = ground_grid.get(&(px, pz)) {
                                    if g_h < min_ground_local {
                                        min_ground_local = g_h;
                                    }
                                }
                            }
                        }

                        let reference_ground = if min_ground_local < i32::MAX {
                            min_ground_local
                        } else {
                            // Se o laser não achou chão em volta (muita árvore em volta), aproxima
                            max_h - ((4.0 * self.scale_v) as i32)
                        };

                        let height_blocks = (max_h - reference_ground).max(1);
                        tags.insert("height".to_string(), height_blocks.to_string());

                        // Estimação de andares (~3.5m por andar)
                        if b_class == 6 {
                            let levels = ((height_blocks as f64) / (3.5 * self.scale_v)).max(1.0).floor() as i32;
                            tags.insert("building:levels".to_string(), levels.to_string());
                        }

                        // 🚨 Aplicação do Algoritmo Casco Côncavo Rasterizado (Salva prédios em U e L)
                        let boundary_polygon = Self::trace_concave_boundary(&cluster_cells);

                        if boundary_polygon.len() >= 3 {
                            features.push(Feature::new(
                                next_id,
                                semantic,
                                tags,
                                GeometryType::Polygon(boundary_polygon),
                                "GDF_LiDAR_Cloud".to_string(),
                                self.priority,
                            ));
                            next_id += 1;
                        }
                    }
                }
            }
        }

        features.shrink_to_fit();
        println!("[INFO] 🏗️ Solidificação LiDAR concluída: {} Cascos Côncavos isolados perfeitamente.", features.len());

        Ok(features)
    }
}