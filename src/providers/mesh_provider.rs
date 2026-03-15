use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use rustc_hash::FxHashMap; // BESM-6: Hash O(1) de extrema performance
use proj::Proj;
use tobj;

/// Provedor de Malhas 3D de Fotogrametria (Wavefront .obj).
/// Projetado para ler escaneamentos de drones (Monumentos, Est�tuas, Pontes complexas).
/// Aplica Voxelizaçao colunar (1x1m) baseada na densidade dos v�rtices para recriar
/// estruturas com tetos curvos e v�os livres no Minecraft.
pub struct MeshProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
    pub decimation_factor: f64, // Pula v�rtices para aliviar mem�ria se necess�rio
}

impl MeshProvider {
    pub fn new(file_path: PathBuf, scale_h: f64, scale_v: f64, priority: u8, decimation_factor: f64) -> Self {
        Self {
            file_path,
            scale_h,
            scale_v,
            priority,
            decimation_factor: decimation_factor.clamp(0.01, 1.0),
        }
    }
}

impl DataProvider for MeshProvider {
    fn priority(&self) -> u8 { self.priority }
    fn name(&self) -> &str {
        "Photogrammetry Mesh Voxelizer (.obj)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] ?? Iniciando Voxeliza��o de Fotogrametria 3D: {}", self.file_path.display());

        // 1. Carregar a malha 3D via TOBJ (Zero-copy whenever possible)
        let load_options = tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ignore_points: false,
            ignore_lines: true,
        };

        let (models, _) = tobj::load_obj(&self.file_path, &load_options)
            .map_err(|e| format!("Falha ao decodificar a malha OBJ: {}", e))?;

        // Inicializa o Pipeline Geod�sico (Assumimos que o drone exportou a nuvem em UTM 23S)
        let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
            .ok()
            .ok_or("Falha ao inicializar biblioteca PROJ para CRS 31983 -> 4326")?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        // ?? BESM-6 Tweak: Matriz Voxel Colunar
        // Em vez de criar um pol�gono 2D gigante (que perderia o v�o livre de uma ponte),
        // N�s agregamos os v�rtices em uma grade Minecraft de 1x1 bloco.
        // A chave � a coordenada (X, Z) do Minecraft. O valor � (min_y, max_y) da eleva��o.
        let mut voxel_grid: FxHashMap<(i32, i32), (f64, f64)> = FxHashMap::default();

        let step_skip = (1.0 / self.decimation_factor).round() as usize;
        let mut processed_vertices = 0;

        for model in models {
            let mesh = &model.mesh;
            let positions = &mesh.positions; // [x1, y1, z1, x2, y2, z2, ...]

            // O Wavefront OBJ usa coordenadas locais.
            // Padr�o Fotogram�trico: X = Easting (UTM), Z = Northing (UTM), Y = Eleva��o.
            for i in (0..positions.len()).step_by(3 * step_skip) {
                if i + 2 >= positions.len() { break; }

                let utm_x = positions[i] as f64;
                let elev_y = positions[i + 1] as f64;
                let utm_z = positions[i + 2] as f64;

                // Reprojeta UTM para Lat/Lon
                if let Ok((lon, lat)) = proj.convert((utm_x, utm_z)) {
                    if let Ok(llpoint) = LLPoint::new(lat, lon) {
                        
                        // Early-Z Culling BBox
                        if !bbox.contains(&llpoint) {
                            continue;
                        }

                        // Projeta para a malha cartesiana X/Z do Minecraft
                        let xz_point = transformer.transform_point(llpoint);
                        let mc_x = xz_point.x;
                        let mc_z = xz_point.z;

                        // Agrega na coluna Voxel
                        let entry = voxel_grid.entry((mc_x, mc_z)).or_insert((elev_y, elev_y));
                        if elev_y < entry.0 { entry.0 = elev_y; }
                        if elev_y > entry.1 { entry.1 = elev_y; }
                        
                        processed_vertices += 1;
                    }
                }
            }
        }

        println!("[INFO] ?? Malha fatiada: {} v�rtices colapsados em {} colunas Voxel.", processed_vertices, voxel_grid.len());

        let mut features = Vec::with_capacity(voxel_grid.len());
        let mut next_id = 7_000_000_000; // Offset dedicado para Mesh Voxel

        // 3. Converter as Colunas Voxel em Features de Alta Fidelidade
        for ((mc_x, mc_z), (min_elev, max_elev)) in voxel_grid {
            
            // Cria um pol�gono de 1x1 bloco exato para essa coluna
            let voxel_poly = vec![
                XZPoint::new(mc_x, mc_z),
                XZPoint::new(mc_x + 1, mc_z),
                XZPoint::new(mc_x + 1, mc_z + 1),
                XZPoint::new(mc_x, mc_z + 1),
            ];

            let mut tags = HashMap::new();
            tags.insert("source".to_string(), "GDF_Mesh_Voxel".to_string());
            tags.insert("building".to_string(), "yes".to_string()); // For�a o motor a extrudar
            
            // O Segredo da Geometria Complexa: Se houver diferen�a entre a base e o topo,
            // o Arnis vai criar um v�o livre de ar embaixo (min_height) e teto em (height).
            tags.insert("min_height".to_string(), format!("{:.2}", min_elev * self.scale_v));
            tags.insert("height".to_string(), format!("{:.2}", max_elev * self.scale_v));

            let feature = Feature::new(
                next_id,
                SemanticGroup::Building, // Tratamos o mesh como constru��o para sofrer extrus�o
                tags,
                GeometryType::Polygon(voxel_poly),
                "Photogrammetry_Mesh".to_string(),
                self.priority, // Prioridade Alt�ssima para perfurar e sobrepor pol�gonos normais
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        Ok(features)
    }
}