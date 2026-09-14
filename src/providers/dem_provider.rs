//! DEM (Digital Elevation Model) Provider (BESM-6 Government Tier)
//!
//! Especializado na leitura de dados matriciais de eleva��o do terreno nu (Bare Earth),
//! suportando formatos como GeoTIFF (Copernicus DEM, ALOS AW3D, SRTM, ANADEM).
//! Emprega a Voxelização Local Determin�stica com Pr�-Quantiza��o Inteira, mapeando
//! diretamente as cotas geogr�ficas para o Grid Global Absoluto em O(1).

use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use rustc_hash::FxHashMap;
use std::fs::File;
use std::path::PathBuf;
use tiff::decoder::{Decoder, DecodingResult};

/// Provedor de Modelos Digitais de Eleva��o (DEM).
/// Diferente dos provedores vetoriais (OSM/CityGML), o DEM n�o gera Features (Pol�gonos),
/// mas sim um mapa de altura (Heightmap) absoluto e quantizado para ancoragem do motor Scanline.
pub struct DemProvider {
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

impl DemProvider {
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

    /// Extrai a matriz quantizada de eleva��o para uma Bounding Box espec�fica.
    /// Retorna um FxHashMap mapeando a coordenada absoluta (X, Z) para a altura Y (Voxelizada),
    /// impedindo o colapso da RAM com arrays 2D gigantes e vazios.
    pub fn fetch_quantized_elevation(
        &self,
        bbox: &LLBBox,
    ) -> Result<FxHashMap<(i32, i32), i32>, String> {
        println!(
            "[INFO] ?? Iniciando leitura de DEM (Copernicus/ANADEM/SRTM) em: {}",
            self.file_path.display()
        );

        let file = File::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir arquivo DEM GeoTIFF: {}", e))?;

        let mut decoder = Decoder::new(file)
            .map_err(|e| format!("Falha ao inicializar descodificador DEM GeoTIFF: {}", e))?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| format!("Falha ao ler dimens�es do DEM GeoTIFF: {}", e))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o Global ECEF Transformer: {}", e))?;

        // ?? BESM-6: Matriz Virtual de Alta Performance (Preven��o OOM)
        let mut quantization_grid: FxHashMap<(i32, i32), i32> = FxHashMap::default();
        let mut read_count = 0u64;
        let mut mapped_count = 0u64;

        // Limites de Culling (Apenas processa a caixa geogr�fica estrita do motor)
        let start_x =
            ((bbox.min().lng() - self.top_left_lon) / self.pixel_size_degrees_x).max(0.0) as u32;
        let end_x = ((bbox.max().lng() - self.top_left_lon) / self.pixel_size_degrees_x)
            .min(width as f64) as u32;

        let start_y =
            ((self.top_left_lat - bbox.max().lat()) / self.pixel_size_degrees_y).max(0.0) as u32;
        let end_y = ((self.top_left_lat - bbox.min().lat()) / self.pixel_size_degrees_y)
            .min(height as f64) as u32;

        if start_x >= width || start_y >= height || start_x >= end_x || start_y >= end_y {
            println!("[AVISO] O DEM GeoTIFF est� fora da Bounding Box atual. Retornando grid de eleva��o vazio.");
            return Ok(quantization_grid);
        }

        match decoder.read_image() {
            Ok(DecodingResult::F32(image_data)) => {
                // Suporte para DEMs de precis�o em ponto flutuante 32-bit (Copernicus DEM, ALOS)
                for y in start_y..end_y {
                    for x in start_x..end_x {
                        read_count += 1;
                        let pixel_index = (y * width + x) as usize;
                        let elevation = image_data[pixel_index];

                        // Ignora pixels corrompidos ou marcados como NoData (Oceanos, falhas de sensor)
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

                                // Voxeliza��o Local Determin�stica com Pr�-Quantiza��o Inteira
                                let voxel_y = self.ground_level_offset
                                    + (elevation as f64 * self.scale_v).round() as i32;

                                // O(1) Hash Map: Sobrescreve mantendo sempre o pico geod�sico daquele bloco de 1x1m
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
                // Suporte para DEMs em 16-bit Unsigned (comum em varia��es do ALOS AW3D)
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
                // Suporte para DEMs em 16-bit Signed (Padr�o ouro do SRTM)
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
                return Err("Formato de codifica��o do DEM GeoTIFF n�o suportado. Esperado matriz F32, I16 ou U16.".to_string());
            }
        }

        // Limpeza de capacidade excedente antes de devolver � thread principal
        quantization_grid.shrink_to_fit();

        println!(
            "[INFO] ? Matriz DEM Bare Earth quantizada. {} pixels processados, {} blocos de terreno ancorados no Grid Absoluto.",
            read_count, mapped_count
        );

        Ok(quantization_grid)
    }
}
