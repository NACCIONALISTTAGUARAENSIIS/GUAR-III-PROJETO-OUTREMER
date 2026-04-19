//! BIM / IFC Provider (BESM-6 LOD4/LOD5 Tier)
//!
//! Responsável pela leitura de modelos arquitetônicos Industry Foundation Classes (.ifc).
//! Traz o rigor da arquitetura de Oscar Niemeyer (Rampas, Pilares em V, Lajes curvas)
//! para o motor Voxel, utilizando uma âncora geodésica para converter o Cartesiano Local
//! do CAD/Revit para o Sistema Global WGS84 -> Minecraft XZ.

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::f64::consts::PI;

pub struct IfcProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
    // Âncora Geodésica: Onde o ponto (0,0,0) do Revit/CAD "pousa" no mundo real.
    pub anchor_lat: f64,
    pub anchor_lon: f64,
    // Rotação em graus do Norte do Projeto para o Norte Verdadeiro
    pub rotation_rad: f64,
}

impl IfcProvider {
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        scale_v: f64,
        priority: u8,
        anchor_lat: f64,
        anchor_lon: f64,
        rotation_deg: f64,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            scale_v,
            priority,
            anchor_lat,
            anchor_lon,
            rotation_rad: rotation_deg * (PI / 180.0),
        }
    }

    /// 🚨 BESM-6: Motor de Translação e Rotação Afim (Local CAD -> Global WGS84 -> Minecraft XZ)
    #[inline(always)]
    fn transform_local_to_global(
        &self,
        local_x: f64,
        local_y: f64,
        transformer: &CoordTransformer,
    ) -> Option<XZPoint> {
        // 1. Aplica a Rotação do Norte do Projeto
        let rot_x = local_x * self.rotation_rad.cos() - local_y * self.rotation_rad.sin();
        let rot_y = local_x * self.rotation_rad.sin() + local_y * self.rotation_rad.cos();

        // 2. Converte metros locais para graus decimais aproximados (WGS84)
        // 1 grau de latitude = ~111.320 metros
        // 1 grau de longitude = ~111.320 metros * cos(latitude)
        const METERS_PER_DEGREE_LAT: f64 = 111_320.0;
        let meters_per_degree_lon = METERS_PER_DEGREE_LAT * self.anchor_lat.to_radians().cos();

        let global_lat = self.anchor_lat + (rot_y / METERS_PER_DEGREE_LAT);
        let global_lon = self.anchor_lon + (rot_x / meters_per_degree_lon);

        // 3. Projeta para a Malha Cartesiana Voxel do BESM-6
        if let Ok(llpoint) = LLPoint::new(global_lat, global_lon) {
            Some(transformer.transform_point(llpoint))
        } else {
            None
        }
    }

    /// Extrator Rápido de Coordenadas de uma linha STEP/IFC (O(1) String Parser)
    fn extract_coordinates(line: &str) -> Vec<f64> {
        let mut coords = Vec::new();
        if let Some(start) = line.find('(') {
            if let Some(end) = line.rfind(')') {
                let inner = &line[start + 1..end];
                for part in inner.split(',') {
                    if let Ok(val) = part.trim().parse::<f64>() {
                        coords.push(val);
                    }
                }
            }
        }
        coords
    }
}

impl DataProvider for IfcProvider {
    fn name(&self) -> &str {
        "BIM IFC Provider (LOD4/LOD5 Niemeyer Architecture)"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 🏗️ Iniciando scanner BIM/IFC de ultra-detalhe: {}", self.file_path.display());

        let file = File::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo IFC: {}", e))?;
        let reader = BufReader::new(file);

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador para o IFC: {}", e))?;

        let mut features = Vec::new();
        let mut next_id = 9_000_000_000; // Offset Massivo para o LOD4/LOD5

        // Variáveis de Estado do Scanner Lexical (STEP Format)
        let mut entity_cache: HashMap<String, String> = HashMap::new();
        let mut points_cache: HashMap<String, (f64, f64, f64)> = HashMap::new();

        let mut elements_extracted = 0;

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(_) => continue,
            };

            let line = line.trim();
            if line.is_empty() || line.starts_with("/*") || line.starts_with("ISO") {
                continue;
            }

            // O formato STEP é: #ID = NOME_DA_CLASSE(ATRIBUTOS...);
            if let Some(eq_idx) = line.find('=') {
                let id = line[..eq_idx].trim().to_string();
                let content = line[eq_idx + 1..].trim();

                // Armazenamos pontos cartesianos brutos na memória RAM temporária
                if content.starts_with("IFCCARTESIANPOINT") {
                    let coords = Self::extract_coordinates(content);
                    if coords.len() >= 3 {
                        points_cache.insert(id, (coords[0], coords[1], coords[2]));
                    } else if coords.len() == 2 {
                        points_cache.insert(id, (coords[0], coords[1], 0.0));
                    }
                    continue;
                }

                // 🚨 LEXICAL ROUTING: Interceptação das Entidades Construtivas de Niemeyer
                let mut is_target_entity = false;
                let mut semantic = SemanticGroup::BuildingPart;
                let mut part_type = "";

                if content.starts_with("IFCWALL") {
                    is_target_entity = true;
                    part_type = "wall";
                } else if content.starts_with("IFCCOLUMN") {
                    is_target_entity = true;
                    semantic = SemanticGroup::SupportStructure;
                    part_type = "column";
                } else if content.starts_with("IFCSLAB") {
                    is_target_entity = true;
                    semantic = SemanticGroup::Indoor; // Pisos e tetos
                    part_type = "slab";
                } else if content.starts_with("IFCRAMP") {
                    is_target_entity = true;
                    semantic = SemanticGroup::Indoor;
                    part_type = "ramp";
                } else if content.starts_with("IFCWINDOW") {
                    is_target_entity = true;
                    part_type = "window";
                } else if content.starts_with("IFCDOOR") {
                    is_target_entity = true;
                    part_type = "door";
                } else if content.starts_with("IFCROOF") {
                    is_target_entity = true;
                    part_type = "roof";
                }

                if is_target_entity {
                    // Como não estamos carregando a hierarquia topológica completa do IFC (que exige gigabytes de RAM),
                    // utilizamos um bypass analítico: procuramos referências cruzadas na própria string
                    // para estimar o Bounding Box ou a Coordenada Local baseada nos pontos em cache.

                    let mut local_x = 0.0;
                    let mut local_y = 0.0;
                    let mut local_z = 0.0;
                    let mut points_found = 0;

                    // Busca IDs de referência na linha (ex: #123)
                    for part in content.split(&[',', '(', ')'][..]) {
                        let p = part.trim();
                        if p.starts_with('#') {
                            if let Some(&(px, py, pz)) = points_cache.get(p) {
                                local_x += px;
                                local_y += py;
                                local_z += pz;
                                points_found += 1;
                            }
                        }
                    }

                    if points_found > 0 {
                        // Faz a média para achar o centroide aproximado do elemento BIM
                        local_x /= points_found as f64;
                        local_y /= points_found as f64;
                        local_z /= points_found as f64;

                        // Mapeia o Cartesiano Local (X, Y) para o Global e gera a coordenada Minecraft XZ
                        if let Some(xz_point) = self.transform_local_to_global(local_x, local_y, &transformer) {

                            // Cria as tags estruturais
                            let mut tags = HashMap::new();
                            tags.insert("source".to_string(), "BIM_IFC_Model".to_string());
                            tags.insert("building:part".to_string(), part_type.to_string());

                            // O Z do IFC é a elevação. Aplicamos o rigor vertical do BESM-6.
                            let mc_height = (local_z * self.scale_v).round() as i32;
                            tags.insert("elevation".to_string(), mc_height.to_string());
                            tags.insert("material".to_string(), "concrete".to_string()); // Padrão Niemeyer

                            // Definimos o tamanho estimado do voxel (Para LOD4 geramos uma Bounding Box de 1 bloco de espessura)
                            let half_size = if part_type == "column" { 1 } else { 2 };

                            let mut poly = Vec::new();
                            poly.push(XZPoint::new(xz_point.x - half_size, xz_point.z - half_size));
                            poly.push(XZPoint::new(xz_point.x + half_size, xz_point.z - half_size));
                            poly.push(XZPoint::new(xz_point.x + half_size, xz_point.z + half_size));
                            poly.push(XZPoint::new(xz_point.x - half_size, xz_point.z + half_size));
                            poly.push(XZPoint::new(xz_point.x - half_size, xz_point.z - half_size)); // Fecha o anel

                            let feature = Feature::new(
                                next_id,
                                semantic,
                                tags,
                                GeometryType::Polygon(poly),
                                "IFC_LOD4".to_string(),
                                self.priority,
                            );

                            features.push(feature);
                            next_id += 1;
                            elements_extracted += 1;
                        }
                    }
                }
            }
        }

        features.shrink_to_fit();
        println!("[INFO] 📐 Parsing BIM/IFC concluído: {} micro-estruturas (LOD4/LOD5) injetadas na matriz.", elements_extracted);
        Ok(features)
    }
}