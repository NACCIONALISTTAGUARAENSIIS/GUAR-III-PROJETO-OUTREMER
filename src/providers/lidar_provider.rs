//! LiDAR Point Cloud Provider (BESM-6 Government Tier)
//!
//! Descodifica nuvens de pontos densas (.las / .laz) utilizando a arquitetura
//! de Voxelização Local Determinística com Pré-Quantização Inteira.
//! Emprega Agrupamento CCL e Monotone Chain Convex Hull para converter o caos
//! de milhões de pontos num conjunto coeso de polígonos arquitetônicos leves.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use rustc_hash::{FxHashMap, FxHashSet}; // BESM-6: Hash de extrema performance O(1)
use std::collections::VecDeque;
use std::path::PathBuf;

/// O Provedor LiDAR Oficial do BESM-6.
pub struct LidarProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
    // CRS EPSG de Origem do LiDAR (Geralmente SIRGAS 2000 UTM 23S para o DF: "EPSG:31983")
    pub source_epsg: String, 
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

    /// O Rosetta Stone do LiDAR (ASPRS Standard Point Classes)
    #[inline(always)]
    fn class_to_semantic(classification: u8) -> Option<(SemanticGroup, HashMap<String, String>)> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_LiDAR_Cloud".to_string());

        match classification {
            // Classe 3, 4, 5: Vegetação (Baixa, Média, Alta)
            3..=5 => {
                tags.insert("natural".to_string(), "wood".to_string());
                tags.insert("density".to_string(), "high".to_string()); // O LiDAR só reflete copas densas
                Some((SemanticGroup::Natural, tags))
            }
            // Classe 6: Construções / Edificações (Buildings)
            6 => {
                tags.insert("building".to_string(), "yes".to_string());
                Some((SemanticGroup::Building, tags))
            }
            // Classe 9: Água
            9 => {
                tags.insert("natural".to_string(), "water".to_string());
                Some((SemanticGroup::Waterway, tags))
            }
            // Ignoramos o Chão (Classe 2) porque a elevação base já cuida do terreno (elevation_data.rs),
            // e ignoramos ruídos (Classe 0, 1, 7).
            _ => None,
        }
    }

    /// ?? Algoritmo BESM-6: Monotone Chain Convex Hull
    /// Traça um polígono matemático perfeito ao redor de um cluster de voxels LiDAR.
    /// É O(N log N) e garante que o motor de construção (buildings.rs) receba paredes
    /// retas, sólidas e sem auto-intersecções complexas.
    fn compute_convex_hull(points: &mut [(i32, i32)]) -> Vec<XZPoint> {
        if points.len() <= 3 {
            return points.iter().map(|&(x, z)| XZPoint::new(x, z)).collect();
        }

        // Ordenação lexical (X ascendente, Z ascendente)
        points.sort_unstable();

        // Produto vetorial para determinar a direção da curva (Z-coordinate of cross product)
        let cross = |o: &(i32, i32), a: &(i32, i32), b: &(i32, i32)| -> i64 {
            let dx1 = a.0 as i64 - o.0 as i64;
            let dz1 = a.1 as i64 - o.1 as i64;
            let dx2 = b.0 as i64 - o.0 as i64;
            let dz2 = b.1 as i64 - o.1 as i64;
            dx1 * dz2 - dz1 * dx2
        };

        // Casco Inferior
        let mut lower = Vec::with_capacity(points.len() / 2);
        for p in points.iter() {
            while lower.len() >= 2 && cross(&lower[lower.len() - 2], &lower[lower.len() - 1], p) <= 0 {
                lower.pop();
            }
            lower.push(*p);
        }

        // Casco Superior
        let mut upper = Vec::with_capacity(points.len() / 2);
        for p in points.iter().rev() {
            while upper.len() >= 2 && cross(&upper[upper.len() - 2], &upper[upper.len() - 1], p) <= 0 {
                upper.pop();
            }
            upper.push(*p);
        }

        // A fusão dos cascos cria o polígono perfeito fechado
        lower.pop();
        upper.pop();
        lower.extend(upper);

        lower.into_iter().map(|(x, z)| XZPoint::new(x, z)).collect()
    }
}

impl DataProvider for LidarProvider {
    fn name(&self) -> &str {
        "High-Density LiDAR Cloud (.las/.laz)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        use las::{Read, Reader};
        use proj::Proj;

        println!("[INFO] ?? A invocar o scanner LiDAR de ultra-densidade: {}", self.file_path.display());

        let mut reader = Reader::from_path(&self.file_path)
            .map_err(|e| format!("Falha ao ler arquivo LiDAR: {}", e))?;

        let proj = Proj::new_known_crs(&self.source_epsg, "EPSG:4326", None)
            .ok()
            .ok_or(format!("Falha ao inicializar a projeção PROJ: {} -> WGS84", self.source_epsg))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o Global ECEF Transformer: {}", e))?;

        // ?? MATRIZ 2.5D VIRTUAL: Class -> Coordenada Inteira -> Altura Máxima (Y).
        // Isso impede a criação de milhões de objetos, achatando 500 pontos do telhado
        // num único pixel de valor absoluto. Memória estritamente constante.
        let mut quantization_grid: FxHashMap<u8, FxHashMap<(i32, i32), f64>> = FxHashMap::default();

        let mut read_count = 0u64;
        let mut mapped_count = 0u64;

        // Voxelização em Streaming de Disco (Zero Load na RAM inteira)
        for point_result in reader.points() {
            let point = match point_result {
                Ok(p) => p,
                Err(_) => continue, // Blindagem contra chunks de laser corrompidos
            };

            read_count += 1;

            // Filtro Precoce de Classificação (Otimização O(1))
            if Self::class_to_semantic(point.classification).is_none() {
                continue;
            }

            // Reprojeção do UTM para a Latitude/Longitude Global (WGS84)
            let (lon, lat) = proj.convert((point.x, point.y)).unwrap_or((0.0, 0.0));

            // Filtro Geométrico (Scanline Paging)
            if lat < bbox.min().lat()
                || lat > bbox.max().lat()
                || lon < bbox.min().lng()
                || lon > bbox.max().lng()
            {
                continue;
            }

            // A Pré-Quantização Inteira do BESM-6
            if let Ok(llpoint) = LLPoint::new(lat, lon) {
                let xz_point = transformer.transform_point(llpoint);
                
                // Extração dos limites máximos (Altura do Prédio/Árvore)
                let class_grid = quantization_grid.entry(point.classification).or_insert_with(FxHashMap::default);
                let current_y = class_grid.entry((xz_point.x, xz_point.z)).or_insert(f64::MIN);
                
                if point.z > *current_y {
                    *current_y = point.z;
                }

                mapped_count += 1;
            }
        }

        println!("[INFO] ?? Laser condensado. {} lidos, {} alocados no Grid Quantizado.", read_count, mapped_count);

        let mut features = Vec::new();
        let mut next_id = 8_000_000_000; // ID Range reservado para extrusões LiDAR

        // Fase 2: Connected Components Labeling (CCL) para forjar entidades vetoriais
        for (classification, grid) in quantization_grid {
            let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();

            for (&(start_x, start_z), &start_y) in &grid {
                if visited.contains(&(start_x, start_z)) {
                    continue;
                }

                // Expansão BFS (Breadth-First Search) para agrupar o prédio/floresta
                let mut cluster = Vec::new();
                let mut queue = VecDeque::new();
                let mut min_h = start_y;
                let mut max_h = start_y;

                queue.push_back((start_x, start_z));
                visited.insert((start_x, start_z));
                cluster.push((start_x, start_z));

                while let Some((cx, cz)) = queue.pop_front() {
                    // Von Neumann Neighborhood (Vizinhos ortogonais diretos)
                    let neighbors = [
                        (cx + 1, cz), (cx - 1, cz),
                        (cx, cz + 1), (cx, cz - 1),
                    ];

                    for &(nx, nz) in &neighbors {
                        if !visited.contains(&(nx, nz)) {
                            if let Some(&ny) = grid.get(&(nx, nz)) {
                                visited.insert((nx, nz));
                                queue.push_back((nx, nz));
                                cluster.push((nx, nz));

                                if ny < min_h { min_h = ny; }
                                if ny > max_h { max_h = ny; }
                            }
                        }
                    }
                }

                // Descartamos clusters muito pequenos (Ruído de passarinho voando ou poste isolado)
                // Um prédio real ou mancha de árvore requer pelo menos 5 blocos.
                if cluster.len() >= 5 {
                    if let Some((semantic, mut tags)) = Self::class_to_semantic(classification) {
                        
                        // Conversão e Escalonamento da Altura
                        let height_meters = max_h - min_h;
                        let height_blocks = (height_meters * self.scale_v).max(1.0).round() as i32;

                        tags.insert("height".to_string(), height_blocks.to_string());
                        
                        // Aproximação de andares (1 andar ~ 3.5 blocos/metros)
                        if classification == 6 {
                            let levels = (height_blocks as f64 / 3.5).floor().max(1.0) as i32;
                            tags.insert("building:levels".to_string(), levels.to_string());
                        }

                        // Traçagem Paramétrica do Contorno
                        let hull_polygon = Self::compute_convex_hull(&mut cluster);

                        if hull_polygon.len() >= 3 {
                            features.push(Feature::new(
                                next_id,
                                semantic,
                                tags,
                                GeometryType::Polygon(hull_polygon),
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
        println!("[INFO] ??? Solidificação LiDAR concluída: {} mega-estruturas/biomas extraídos e poligonizados.", features.len());
        
        Ok(features)
    }
}