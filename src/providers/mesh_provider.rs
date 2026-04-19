use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use proj::Proj;
use rustc_hash::FxHashMap; // BESM-6: Hash O(1) de extrema performance
use tobj;

// 🚨 BESM-6: Utilizado para gerar um offset de ID único por malha
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Provedor de Malhas 3D de Fotogrametria (Wavefront .obj).
/// Projetado para ler escaneamentos de drones (Monumentos, Estátuas, Pontes complexas).
/// 🚨 BESM-6: Aplica Voxelização Tridimensional real (X, Y, Z) para preservar topologia
/// e vãos livres, injetando as cores reais dos materiais originais no mundo.
pub struct MeshProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
    // 🚨 BESM-6: Flexibilidade Geodésica Rigorosa
    pub crs_source: Option<String>,
    pub offset_x: f64,
    pub offset_y: f64,
    pub offset_z: f64,
}

impl MeshProvider {
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        scale_v: f64,
        priority: u8,
        crs_source: Option<String>,
        offset_x: f64,
        offset_y: f64,
        offset_z: f64,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            scale_v,
            priority,
            crs_source,
            offset_x,
            offset_y,
            offset_z,
        }
    }

    /// 🚨 BESM-6: Resolve o Ponto Cego de Colisão de IDs.
    /// Cada malha recebe um base_id bilionário único, derivado do nome do arquivo.
    fn generate_base_id(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.file_path.hash(&mut hasher);
        // Garante que o ID fique no escopo de 7 a 9 bilhões para não colidir com OSM
        7_000_000_000 + (hasher.finish() % 2_000_000_000)
    }
}

impl DataProvider for MeshProvider {
    fn priority(&self) -> u8 {
        self.priority
    }

    fn name(&self) -> &str {
        "Photogrammetry Mesh Voxelizer 3D (.obj)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] 🗿 Iniciando Voxelização de Fotogrametria 3D: {}",
            self.file_path.display()
        );

        // 1. Carregar a malha 3D e materiais via TOBJ
        let load_options = tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ignore_points: false,
            ignore_lines: true,
        };

        let (models, materials_result) = tobj::load_obj(&self.file_path, &load_options)
            .map_err(|e| format!("Falha ao decodificar a malha OBJ: {}", e))?;

        // 🚨 Suporte a Texturas/Cores (Mapeia o arquivo .mtl associado)
        let materials = materials_result.unwrap_or_default();

        // 🚨 Pipeline Geodésico Opcional Dinâmico
        // Se a malha já vem em coordenadas reais (UTM), projetamos para WGS84.
        // Se crs_source for None, assumimos Plano Tangente Local (Centro = 0,0,0).
        let proj = if let Some(crs) = &self.crs_source {
            Some(Proj::new_known_crs(crs, "EPSG:4326", None)
                .ok()
                .ok_or(format!("Falha ao inicializar PROJ para CRS {} -> 4326", crs))?)
        } else {
            None
        };

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        // 🚨 Voxelização Tridimensional Real O(1)
        // Agregamos vértices numa grade 3D. A tabela hash fará a decimação espacial natural.
        // Chave: (X, Y, Z) exatos no Minecraft. Valor: Cor RGB em HEX extraída da textura.
        let mut voxel_grid: FxHashMap<(i32, i32, i32), String> = FxHashMap::default();

        for model in models {
            let mesh = &model.mesh;
            let positions = &mesh.positions; // [x1, y1, z1, x2, y2, z2, ...]

            // Extração de Cor do Material baseada na face/grupo
            let mut hex_color = String::from("#888888"); // Concreto Brutalista (Fallback)
            if let Some(mat_id) = mesh.material_id {
                if let Some(mat) = materials.get(mat_id) {
                    if let Some(diffuse) = mat.diffuse {
                        let r = (diffuse[0] * 255.0) as u8;
                        let g = (diffuse[1] * 255.0) as u8;
                        let b = (diffuse[2] * 255.0) as u8;
                        hex_color = format!("#{:02X}{:02X}{:02X}", r, g, b);
                    }
                }
            }

            // O Wavefront OBJ usa coordenadas locais.
            // O laço não pula mais vértices destrutivamente. Processamos todos e a grade 1x1x1 absorve a redundância.
            for i in (0..positions.len()).step_by(3) {
                let raw_x = positions[i] as f64 + self.offset_x;
                let raw_y = positions[i + 1] as f64 + self.offset_y;
                let raw_z = positions[i + 2] as f64 + self.offset_z;

                let (mc_x, mc_z) = if let Some(ref p) = proj {
                    // Trata X e Z como Coordenadas Georreferenciadas (Ex: UTM)
                    if let Ok((lon, lat)) = p.convert((raw_x, raw_z)) {
                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            // Early-Z Culling Espacial
                            if !bbox.contains(&llpoint) {
                                continue;
                            }
                            let xz = transformer.transform_point(llpoint);
                            (xz.x, xz.z)
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    // Trata como Plano Cartesiano Local (Origem no Centro do Modelo)
                    let mc_x = (raw_x * self.scale_h).round() as i32;
                    let mc_z = (raw_z * self.scale_h).round() as i32;
                    (mc_x, mc_z)
                };

                let mc_y = (raw_y * self.scale_v).round() as i32;

                // Insere na Grade 3D
                // Múltiplos vértices no mesmo metro cúbico colidem na mesma chave (Decimação Topológica Natural)
                voxel_grid.insert((mc_x, mc_y, mc_z), hex_color.clone());
            }
        }

        println!(
            "[INFO] 🧱 Malha processada: {} voxels 3D sólidos e coloridos gerados.",
            voxel_grid.len()
        );

        let mut features = Vec::with_capacity(voxel_grid.len());

        // 🚨 O ID dinâmico e seguro gerado pela hash do filepath
        let mut next_id = self.generate_base_id();

        // 3. Converter os Voxels em Features Pontuais (Pulando extrusão 2.5D)
        for ((mc_x, mc_y, mc_z), color_hex) in voxel_grid {
            let mut tags = HashMap::new();
            tags.insert("source".to_string(), "GDF_Mesh_Voxel3D".to_string());

            // Repassamos a altura e cor exatas. O compilador do Minecraft usará a cor HEX
            // para escolher o bloco (Lã, Terracota, Concreto) mais parecido visualmente.
            tags.insert("elevation".to_string(), mc_y.to_string());
            tags.insert("color".to_string(), color_hex);
            tags.insert("material".to_string(), "photogrammetry".to_string());

            let feature = Feature::new(
                next_id,
                SemanticGroup::TerrainDetail, // 🚨 Cessa a dependência do Building. Voxels são tratados como pontos no espaço.
                tags,
                GeometryType::Point(XZPoint::new(mc_x, mc_z)),
                "Photogrammetry_Mesh".to_string(),
                self.priority,
            );

            features.push(feature);
            next_id += 1;
        }

        features.shrink_to_fit();
        Ok(features)
    }
}