//! DSM (Digital Surface Model) Provider (BESM-6 Government Tier)
//!
//! Diferente do DEM (Bare Earth), o DSM captura a primeira superf�cie refletora do laser/radar,
//! englobando copas de �rvores, telhados de pr�dios, pontes e viadutos.
//! � utilizado primariamente para o c�lculo de altimetria de extrus�o:
//! Altura do Prédio = (DSM_Y - DEM_Y).
//!
//! Emprega a Voxeliza��o Local Determin�stica com Pr�-Quantiza��o Inteira em O(1).

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use rustc_hash::FxHashMap;
use std::fs::File;
use std::path::PathBuf;
use tiff::decoder::{Decoder, DecodingResult};

/// Provedor de Modelos Digitais de Superf�cie (DSM).
pub struct DsmProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub scale_v: f64,
    pub ground_level_offset: i32,
    pub top_left_lat: f64,
    pub top_left_lon: f64,
    pub pixel_size_degrees_x: f64,
    pub pixel_size_degrees_y: f64,
    pub nodata_value: f32,
}

impl DsmProvider {
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        scale_v: f64,
        ground_level_offset: i32,
        top_left_lat: f64,
        top_left_lon: f64,
        pixel_size_degrees_x: f64,
        pixel_size_degrees_y: f64,
        nodata_value: f32,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            scale_v,
            ground_level_offset,
            top_left_lat,
            top_left_lon,
            pixel_size_degrees_x,
            pixel_size_degrees_y,
            nodata_value,
        }
    }

    /// Extrai a matriz quantizada da superf�cie (Canopy/Roofs) para uma Bounding Box espec�fica.
    /// Retorna um FxHashMap mapeando a coordenada absoluta (X, Z) para a altura Y de pico (Voxelizada).
    pub fn fetch_quantized_surface(
        &self,
        bbox: &LLBBox,
    ) -> Result<FxHashMap<(i32, i32), i32>, String> {
        println!(
            "[INFO] ??? Iniciando varredura do Modelo de Superf�cie (DSM) em: {}",
            self.file_path.display()
        );

        let file = File::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo DSM GeoTIFF: {}", e))?;

        let mut decoder = Decoder::new(file)
            .map_err(|e| format!("Falha ao inicializar descodificador DSM GeoTIFF: {}", e))?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| format!("Falha ao ler dimens�es do DSM GeoTIFF: {}", e))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o Global ECEF Transformer: {}", e))?;

        // ?? BESM-6: Matriz Virtual de Alta Performance O(1) para a Superf�cie
        let mut quantization_grid: FxHashMap<(i32, i32), i32> = FxHashMap::default();
        let mut read_count = 0u64;
        let mut mapped_count = 0u64;

        // Limites de Culling (O Scanline Engine define a BBox da regi�o atual)
        let start_x =
            ((bbox.min().lng() - self.top_left_lon) / self.pixel_size_degrees_x).max(0.0) as u32;
        let end_x = ((bbox.max().lng() - self.top_left_lon) / self.pixel_size_degrees_x)
            .min(width as f64) as u32;

        let start_y =
            ((self.top_left_lat - bbox.max().lat()) / self.pixel_size_degrees_y).max(0.0) as u32;
        let end_y = ((self.top_left_lat - bbox.min().lat()) / self.pixel_size_degrees_y)
            .min(height as f64) as u32;

        if start_x >= width || start_y >= height || start_x >= end_x || start_y >= end_y {
            println!("[AVISO] O DSM GeoTIFF est� fora da Bounding Box atual. Retornando grid de superf�cie vazio.");
            return Ok(quantization_grid);
        }

        match decoder.read_image() {
            Ok(DecodingResult::F32(image_data)) => {
                // Precision Floating Point (Comum em DSMs fotogram�tricos)
                for y in start_y..end_y {
                    for x in start_x..end_x {
                        read_count += 1;
                        let pixel_index = (y * width + x) as usize;
                        let elevation = image_data[pixel_index];

                        if (elevation - self.nodata_value).abs() < f32::EPSILON
                            || elevation.is_nan()
                        {
                            continue;
                        }

                        let lat = self.top_left_lat - (y as f64 * self.pixel_size_degrees_y);
                        let lon = self.top_left_lon + (x as f64 * self.pixel_size_degrees_x);

                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            if bbox.contains(&llpoint) {
                                let xz_point = transformer.transform_point(llpoint);

                                // Voxeliza��o: Arredondamento do teto (A superf�cie mais alta � a que dita o fim do pr�dio)
                                let voxel_y = self.ground_level_offset
                                    + (elevation as f64 * self.scale_v).round() as i32;

                                // O(1) Hash Map: No caso da superf�cie, garantimos sempre a reten��o do ponto mais alto no metro quadrado
                                let entry = quantization_grid
                                    .entry((xz_point.x, xz_point.z))
                                    .or_insert(voxel_y);
                                if voxel_y > *entry {
                                    *entry = voxel_y;
                                }
                                mapped_count += 1;
                            }
                        }
                    }
                }
            }
            Ok(DecodingResult::U16(image_data)) => {
                for y in start_y..end_y {
                    for x in start_x..end_x {
                        read_count += 1;
                        let pixel_index = (y * width + x) as usize;
                        let elevation = image_data[pixel_index] as f32;

                        if (elevation - self.nodata_value).abs() < f32::EPSILON {
                            continue;
                        }

                        let lat = self.top_left_lat - (y as f64 * self.pixel_size_degrees_y);
                        let lon = self.top_left_lon + (x as f64 * self.pixel_size_degrees_x);

                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            if bbox.contains(&llpoint) {
                                let xz_point = transformer.transform_point(llpoint);
                                let voxel_y = self.ground_level_offset
                                    + (elevation as f64 * self.scale_v).round() as i32;

                                let entry = quantization_grid
                                    .entry((xz_point.x, xz_point.z))
                                    .or_insert(voxel_y);
                                if voxel_y > *entry {
                                    *entry = voxel_y;
                                }
                                mapped_count += 1;
                            }
                        }
                    }
                }
            }
            Ok(DecodingResult::I16(image_data)) => {
                for y in start_y..end_y {
                    for x in start_x..end_x {
                        read_count += 1;
                        let pixel_index = (y * width + x) as usize;
                        let elevation = image_data[pixel_index] as f32;

                        if (elevation - self.nodata_value).abs() < f32::EPSILON {
                            continue;
                        }

                        let lat = self.top_left_lat - (y as f64 * self.pixel_size_degrees_y);
                        let lon = self.top_left_lon + (x as f64 * self.pixel_size_degrees_x);

                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            if bbox.contains(&llpoint) {
                                let xz_point = transformer.transform_point(llpoint);
                                let voxel_y = self.ground_level_offset
                                    + (elevation as f64 * self.scale_v).round() as i32;

                                let entry = quantization_grid
                                    .entry((xz_point.x, xz_point.z))
                                    .or_insert(voxel_y);
                                if voxel_y > *entry {
                                    *entry = voxel_y;
                                }
                                mapped_count += 1;
                            }
                        }
                    }
                }
            }
            _ => {
                return Err("Formato de codifica��o do DSM GeoTIFF n�o suportado. Esperado matriz F32, I16 ou U16.".to_string());
            }
        }

        // Garbage Collection Antecipado
        quantization_grid.shrink_to_fit();

        println!(
            "[INFO] ? Matriz de Superf�cie (DSM) quantizada. {} pixels processados, {} topos de estrutura ancorados.",
            read_count, mapped_count
        );

        Ok(quantization_grid)
    }
}
