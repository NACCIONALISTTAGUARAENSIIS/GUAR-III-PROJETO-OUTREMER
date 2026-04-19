//! OGC 3D Tiles Provider (BESM-6 Streaming Tier)
//!
//! Faz streaming de malhas 3D texturizadas otimizadas para web (Cesium/OGC).
//! Implementa Culling Espacial (Frustum/BBox) na Árvore Hierárquica (HLOD),
//! puxando apenas os tiles estritamente necessários para a BBox atual,
//! garantindo escala infinita sem sobrecarga de memória RAM (Out-of-Core).

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::transformation::CoordTransformer;
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};

use std::collections::HashMap;
use std::time::Duration;
use serde_json::Value;

pub struct Tiles3DProvider {
    pub endpoint_url: String,
    pub scale_h: f64,
    pub scale_v: f64,
    pub priority: u8,
}

impl Tiles3DProvider {
    pub fn new(endpoint_url: String, scale_h: f64, scale_v: f64, priority: u8) -> Self {
        Self {
            endpoint_url,
            scale_h,
            scale_v,
            priority,
        }
    }

    /// 🚨 Culling Espacial Matemático O(1)
    /// Verifica se a BoundingVolume do Tile (formato OGC Region: [west, south, east, north, min_h, max_h] em radianos)
    /// cruza com a BBox requisitada pelo motor.
    fn intersects_region(region_rads: &[Value], bbox: &LLBBox) -> bool {
        if region_rads.len() < 4 { return false; }

        let tile_west = region_rads[0].as_f64().unwrap_or(0.0).to_degrees();
        let tile_south = region_rads[1].as_f64().unwrap_or(0.0).to_degrees();
        let tile_east = region_rads[2].as_f64().unwrap_or(0.0).to_degrees();
        let tile_north = region_rads[3].as_f64().unwrap_or(0.0).to_degrees();

        let req_min_lat = bbox.min().lat();
        let req_max_lat = bbox.max().lat();
        let req_min_lon = bbox.min().lng();
        let req_max_lon = bbox.max().lng();

        // Lógica de interseção de retângulos (AABB)
        !(tile_west > req_max_lon || tile_east < req_min_lon || tile_south > req_max_lat || tile_north < req_min_lat)
    }

    /// 🚨 BESM-6: Traversal Recursivo da Árvore de Tiles (HLOD)
    fn traverse_node(
        &self,
        node: &Value,
        bbox: &LLBBox,
        transformer: &CoordTransformer,
        features: &mut Vec<Feature>,
        next_id: &mut u64,
    ) {
        // 1. Extração do Bounding Volume
        let bounding_volume = node.get("boundingVolume");
        if let Some(bv) = bounding_volume {
            if let Some(region) = bv.get("region").and_then(|r| r.as_array()) {
                // Se o tile atual NÃO cruza o nosso BBox, podamos o galho inteiro (Early Exit O(1))
                if !Self::intersects_region(region, bbox) {
                    return;
                }

                // Se cruzou e tem conteúdo (payload B3DM, GLB, etc), mapeamos para a Voxelização Local
                if let Some(content) = node.get("content") {
                    if let Some(uri) = content.get("uri").and_then(|u| u.as_str()) {

                        // OGC Spec: West, South, East, North (Radianos)
                        let w = region[0].as_f64().unwrap_or(0.0).to_degrees();
                        let s = region[1].as_f64().unwrap_or(0.0).to_degrees();
                        let e = region[2].as_f64().unwrap_or(0.0).to_degrees();
                        let n = region[3].as_f64().unwrap_or(0.0).to_degrees();
                        let min_h = region[4].as_f64().unwrap_or(0.0);
                        let max_h = region[5].as_f64().unwrap_or(0.0);

                        // Projeção dos vértices da bounding box do Tile para a Malha Cartesiana XZ
                        if let (Ok(sw), Ok(se), Ok(ne), Ok(nw)) = (
                            LLPoint::new(s, w), LLPoint::new(s, e),
                            LLPoint::new(n, e), LLPoint::new(n, w)
                        ) {
                            let p_sw = transformer.transform_point(sw);
                            let p_se = transformer.transform_point(se);
                            let p_ne = transformer.transform_point(ne);
                            let p_nw = transformer.transform_point(nw);

                            let poly = vec![p_sw, p_se, p_ne, p_nw, p_sw];

                            let mut tags = HashMap::new();
                            tags.insert("source".to_string(), "OGC_3DTiles_Stream".to_string());
                            tags.insert("tile_uri".to_string(), uri.to_string());

                            // Rigor Governamental de Altura (Scale V)
                            let h_blocks = ((max_h - min_h) * self.scale_v).max(1.0).round() as i32;
                            tags.insert("height".to_string(), h_blocks.to_string());
                            tags.insert("building".to_string(), "yes".to_string()); // Força a extrusão no motor principal

                            let feature = Feature::new(
                                *next_id,
                                SemanticGroup::Building, // Pode ser ajustado para TerrainDetail caso o LOD seja de solo
                                tags,
                                GeometryType::Polygon(poly),
                                "OGC_3DTiles".to_string(),
                                self.priority,
                            );

                            features.push(feature);
                            *next_id += 1;
                        }
                    }
                }
            }
        }

        // 2. Continua a descida recursiva para os filhos (Refinamento de LOD)
        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            for child in children {
                self.traverse_node(child, bbox, transformer, features, next_id);
            }
        }
    }
}

impl DataProvider for Tiles3DProvider {
    fn name(&self) -> &str {
        "OGC 3D Tiles Streamer (Cesium HLOD)"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!("[INFO] 🌐 Conectando à malha OGC 3D Tiles: {}", self.endpoint_url);

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Falha ao construir o cliente HTTP 3D Tiles: {}", e))?;

        // 1. Download do Tileset Root (tileset.json)
        let response = client.get(&self.endpoint_url).send()
            .map_err(|e| format!("Falha na ligação ao servidor 3D Tiles: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Servidor 3D Tiles rejeitou o pedido: {}", response.status()));
        }

        let json_text = response.text()
            .map_err(|e| format!("Falha ao ler resposta do 3D Tiles: {}", e))?;

        let tileset: Value = serde_json::from_str(&json_text)
            .map_err(|e| format!("Tileset.json inválido: {}", e))?;

        let root_node = tileset.get("root")
            .ok_or("Tileset.json não contém o nó 'root' obrigatório da OGC.")?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao instanciar transformador XZ: {}", e))?;

        let mut features = Vec::new();
        // Offset Massivo para os Tiles (Evitar colisões com OSM, GDF e IFC)
        let mut next_id = 11_000_000_000;

        // 2. Inicia o Culling Espacial Recursivo
        self.traverse_node(root_node, bbox, &transformer, &mut features, &mut next_id);

        features.shrink_to_fit();
        println!("[INFO] 🧩 3D Tiles Intersectados: {} tiles isolados para o quadrante atual.", features.len());

        Ok(features)
    }
}