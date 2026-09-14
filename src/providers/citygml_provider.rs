use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Projeção ECEF Oficial
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::path::PathBuf;

use geo::{ConvexHull, MultiPoint, Point, Polygon};
use proj::Proj;
use quick_xml::events::Event;
use quick_xml::Reader;

/// Provedor de Modelos 3D Urbanos (CityGML) com suporte a LOD3.
/// Utiliza arquitetura SAX Streaming (Zero-RAM Bloat) para ler arquivos gigantescos (>10GB).
/// Extrai pegadas 2D (Footprints), alturas brutas (LOD1/LOD2) e Mapeamento de Fachada (LOD3 Janelas/Portas).
pub struct CityGmlProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
}

#[derive(Debug, Clone, PartialEq)]
enum GmlSurfaceType {
    None,
    Wall,
    Roof,
    Ground,
    Window, // LOD3
    Door,   // LOD3
}

impl CityGmlProvider {
    // 🚨 BESM-6: Alinhado para 3 argumentos, removendo stream_mode desnecessário (já é SAX nativo)
    pub fn new(file_path: PathBuf, scale_h: f64, priority: u8) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
        }
    }

    /// Parseia uma string de floats separada por espaços (padrão GML posList)
    /// em tuplas 3D (X, Y, Z).
    #[inline(always)]
    fn parse_pos_list(text: &str) -> Vec<(f64, f64, f64)> {
        let mut coords = Vec::new();
        // 🚨 CORREÇÃO CRÍTICA DO ITERADOR: &str nativo, sem coerção para String
        let mut floats = text
            .split_whitespace()
            .filter_map(|s| s.parse::<f64>().ok());

        // CityGML posList geralmente é 3D (srsDimension="3")
        while let (Some(x), Some(y), Some(z)) = (floats.next(), floats.next(), floats.next()) {
            coords.push((x, y, z));
        }

        coords
    }

    /// Computa a base (Footprint) e a Altura de um conjunto massivo de pontos 3D
    fn compute_building_footprint_and_height(
        points_3d: &[(f64, f64, f64)],
        proj: &Proj,
        transformer: &CoordTransformer,
        bbox: &LLBBox,
    ) -> Option<(Vec<XZPoint>, f64, bool)> {
        if points_3d.is_empty() {
            return None;
        }

        let mut mc_points = Vec::with_capacity(points_3d.len());
        let mut min_z_elev = f64::MAX;
        let mut max_z_elev = f64::MIN;
        let mut is_completely_outside = true;

        for &(x, y, z) in points_3d {
            // CityGML Z é a elevação real.
            if z < min_z_elev {
                min_z_elev = z;
            }
            if z > max_z_elev {
                max_z_elev = z;
            }

            // 1. Reprojeta UTM 23S (SIRGAS/GDF) para WGS84 Lat/Lon
            if let Ok((lon, lat)) = proj.convert((x, y)) {
                if let Ok(llpoint) = LLPoint::new(lat, lon) {
                    if bbox.contains(&llpoint) {
                        is_completely_outside = false;
                    }
                    // 2. Transforma para a malha cartesiana do Minecraft
                    mc_points.push(transformer.transform_point(llpoint));
                }
            }
        }

        if mc_points.len() < 3 || is_completely_outside {
            return None;
        }

        // BESM-6 Tweak: Como a malha LOD2 é um sólido 3D composto por várias faces (paredes, telhados),
        // achatar tudo e extrair o Convex Hull (Envoltória Convexa) garante um footprint 2D
        // perfeito e intransponível para o motor do Minecraft extrudar a base.
        let geo_points: Vec<Point<f64>> = mc_points
            .iter()
            .map(|pt| Point::new(pt.x as f64, pt.z as f64))
            .collect();

        let multi_point = MultiPoint(geo_points);
        let hull: Polygon<f64> = multi_point.convex_hull();

        // Extrai os vértices do casco convexo
        let mut footprint = Vec::new();
        for coord in hull.exterior().coords() {
            footprint.push(XZPoint::new(coord.x.round() as i32, coord.y.round() as i32));
        }

        let height_meters = (max_z_elev - min_z_elev).max(3.0); // No mínimo 3 metros (1 andar)

        Some((footprint, height_meters, is_completely_outside))
    }

    /// Extrator rápido de Bounding Box 3D para Janelas e Portas LOD3 (Espaço Minecraft)
    fn compute_element_aabb(
        points_3d: &[(f64, f64, f64)],
        proj: &Proj,
        transformer: &CoordTransformer,
    ) -> Option<(i32, i32, i32, i32, i32, i32)> {
        if points_3d.is_empty() {
            return None;
        }

        let mut min_mc_x = i32::MAX;
        let mut max_mc_x = i32::MIN;
        let mut min_mc_z = i32::MAX;
        let mut max_mc_z = i32::MIN;
        let mut min_mc_y = f64::MAX;
        let mut max_mc_y = f64::MIN; // Elev

        for &(x, y, z) in points_3d {
            if let Ok((lon, lat)) = proj.convert((x, y)) {
                if let Ok(llpoint) = LLPoint::new(lat, lon) {
                    let mc_pt = transformer.transform_point(llpoint);
                    if mc_pt.x < min_mc_x {
                        min_mc_x = mc_pt.x;
                    }
                    if mc_pt.x > max_mc_x {
                        max_mc_x = mc_pt.x;
                    }
                    if mc_pt.z < min_mc_z {
                        min_mc_z = mc_pt.z;
                    }
                    if mc_pt.z > max_mc_z {
                        max_mc_z = mc_pt.z;
                    }
                    if z < min_mc_y {
                        min_mc_y = z;
                    }
                    if z > max_mc_y {
                        max_mc_y = z;
                    }
                }
            }
        }

        // Aplica o Rigor de Escala Vertical BESM-6 (1.15) na elevação da Janela
        Some((
            min_mc_x,
            max_mc_x,
            (min_mc_y * 1.15).round() as i32,
            (max_mc_y * 1.15).round() as i32,
            min_mc_z,
            max_mc_z,
        ))
    }
}

impl DataProvider for CityGmlProvider {
    fn priority(&self) -> u8 {
        self.priority
    }
    fn name(&self) -> &str {
        "CityGML 3D Provider (LOD3 SAX Stream with Facade Matrix)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] 🏢 Iniciando scanner SAX Streaming LOD3 no CityGML: {}",
            self.file_path.display()
        );

        // Inicializa bibliotecas de geolocalização (Assumimos SIRGAS 2000 UTM 23S para GDF)
        let proj = Proj::new_known_crs("EPSG:31983", "EPSG:4326", None)
            .ok()
            .ok_or("Falha ao inicializar biblioteca PROJ para CRS 31983 -> 4326")?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        // 🚨 BESM-6 Tweak: SAX Streaming Engine LOD3
        let mut reader = Reader::from_file(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo CityGML: {}", e))?;

        reader.trim_text(true);

        let mut buf = Vec::new();
        let mut features = Vec::new();
        let mut next_id = 4_000_000_000; // Offset dedicado para CityGML

        // Estado do parser XML Geral
        let mut in_building = false;
        let mut current_building_points: Vec<(f64, f64, f64)> = Vec::new();

        // Estado do parser XML LOD3 (Fachadas e Vãos)
        let mut current_surface = GmlSurfaceType::None;
        let mut current_surface_points: Vec<(f64, f64, f64)> = Vec::new();
        let mut window_count = 0;
        let mut door_count = 0;
        let mut building_tags = HashMap::new(); // Tags do prédio atual sendo processado

        let mut capture_text = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.as_ref());

                    if name_str.contains("Building") && !name_str.contains("BuildingPart") {
                        in_building = true;
                        current_building_points.clear();
                        building_tags.clear();
                        window_count = 0;
                        door_count = 0;
                    } else if in_building {
                        // Classificador de Superfície LOD3
                        if name_str.contains("Window") {
                            current_surface = GmlSurfaceType::Window;
                            current_surface_points.clear();
                        } else if name_str.contains("Door") {
                            current_surface = GmlSurfaceType::Door;
                            current_surface_points.clear();
                        } else if name_str.contains("WallSurface") {
                            current_surface = GmlSurfaceType::Wall;
                        }

                        if name_str.contains("posList") || name_str.contains("pos") {
                            capture_text = true;
                        }
                    }
                }
                Ok(Event::Text(e)) => {
                    if capture_text {
                        let text = e.unescape().unwrap_or_default();
                        let points = Self::parse_pos_list(&text);

                        // Adiciona ao volume mestre do prédio para calcular a AABB e o footprint geral
                        current_building_points.extend(points.clone());

                        // Se for uma janela ou porta, guarda temporariamente para fatiar as coordenadas
                        if current_surface == GmlSurfaceType::Window
                            || current_surface == GmlSurfaceType::Door
                        {
                            current_surface_points.extend(points);
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let name_str = String::from_utf8_lossy(name.as_ref());

                    if name_str.contains("posList") || name_str.contains("pos") {
                        capture_text = false;
                    } else if name_str.contains("Window") {
                        // 🚨 LOD3 Tweak: Extração Milimétrica da Janela
                        if let Some((min_x, max_x, min_y, max_y, min_z, max_z)) =
                            Self::compute_element_aabb(&current_surface_points, &proj, &transformer)
                        {
                            building_tags.insert(
                                format!("lod3:window_{}:min_x", window_count),
                                min_x.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:window_{}:max_x", window_count),
                                max_x.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:window_{}:min_y", window_count),
                                min_y.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:window_{}:max_y", window_count),
                                max_y.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:window_{}:min_z", window_count),
                                min_z.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:window_{}:max_z", window_count),
                                max_z.to_string(),
                            );
                            window_count += 1;
                        }
                        current_surface = GmlSurfaceType::Wall; // Volta pro estado de parede por padrão
                        current_surface_points.clear();
                    } else if name_str.contains("Door") {
                        // 🚨 LOD3 Tweak: Extração Milimétrica da Porta
                        if let Some((min_x, max_x, min_y, max_y, min_z, max_z)) =
                            Self::compute_element_aabb(&current_surface_points, &proj, &transformer)
                        {
                            building_tags.insert(
                                format!("lod3:door_{}:min_x", door_count),
                                min_x.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:door_{}:max_x", door_count),
                                max_x.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:door_{}:min_y", door_count),
                                min_y.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:door_{}:max_y", door_count),
                                max_y.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:door_{}:min_z", door_count),
                                min_z.to_string(),
                            );
                            building_tags.insert(
                                format!("lod3:door_{}:max_z", door_count),
                                max_z.to_string(),
                            );
                            door_count += 1;
                        }
                        current_surface = GmlSurfaceType::Wall;
                        current_surface_points.clear();
                    } else if name_str.contains("Building") && !name_str.contains("BuildingPart") {
                        // 🚨 FECHOU O PRÉDIO: Hora de processar, consolidar as tags e limpar a RAM
                        in_building = false;

                        if let Some((footprint, height, is_outside)) =
                            Self::compute_building_footprint_and_height(
                                &current_building_points,
                                &proj,
                                &transformer,
                                bbox,
                            )
                        {
                            if !is_outside {
                                building_tags
                                    .insert("source".to_string(), "GDF_CityGML_3D".to_string());
                                building_tags.insert("building".to_string(), "yes".to_string());

                                // O Arnis usa 'height' como valor escalar nas tags para definir a altura do Voxel
                                building_tags
                                    .insert("height".to_string(), format!("{:.1}", height));

                                // Registra quantos elementos de fachada o motor LOD3 detectou para iterar depois
                                if window_count > 0 {
                                    building_tags.insert(
                                        "lod3:windows".to_string(),
                                        window_count.to_string(),
                                    );
                                }
                                if door_count > 0 {
                                    building_tags
                                        .insert("lod3:doors".to_string(), door_count.to_string());
                                }

                                let feature = Feature::new(
                                    next_id,
                                    SemanticGroup::Building,
                                    building_tags.clone(),
                                    GeometryType::Polygon(footprint),
                                    "CityGML".to_string(),
                                    self.priority,
                                );

                                features.push(feature);
                                next_id += 1;
                            }
                        }

                        // Zera para o próximo prédio (Evita vazamento de memória - RAM continua plana O(1))
                        current_building_points.clear();
                        building_tags.clear();
                    }
                }
                Ok(Event::Eof) => break, // Fim do Arquivo XML
                Err(e) => {
                    eprintln!(
                        "[ALERTA] Erro de parsing SAX no CityGML na posição {}: {:?}",
                        reader.buffer_position(),
                        e
                    );
                    break;
                }
                _ => (), // Ignora comentários, doctypes, etc.
            }
            buf.clear();
        }

        features.shrink_to_fit();
        println!("[INFO] 🏢 SAX Streaming LOD3 finalizado: {} blocos 3D e fachadas matriciais injetadas.", features.len());
        Ok(features)
    }
}
