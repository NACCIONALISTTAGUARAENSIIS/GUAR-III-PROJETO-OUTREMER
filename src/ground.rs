#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::args::Args;
use crate::{
    coordinate_system::{cartesian::XZPoint, geographic::LLBBox},
    progress::emit_gui_progress_update,
};
use crate::elevation_data::{fetch_elevation_data, ElevationData};
use colored::Colorize;
use image::{Rgb, RgbImage};
use std::sync::Arc;

/// Represents terrain data and elevation settings
/// ?? BESM-6: Otimizado para Memória Contígua (1D Array) e Coordenadas Absolutas
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>,
    
    // Offsets Absolutos do Grid (Onde ele começa no mundo Minecraft)
    min_x: i32,
    min_z: i32,
}

impl Ground {
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            ground_level,
            elevation_data: None,
            min_x: 0,
            min_z: 0,
        }
    }

    pub fn new_enabled(
        bbox: &LLBBox,
        scale_h: f64,
        scale_v: f64,
        ground_level: i32,
        local_lidar: Option<&std::path::PathBuf>,
        // ?? Precisamos do X/Z mínimo global para alinhar o raster
        global_min_x: i32, 
        global_min_z: i32,
    ) -> Self {
        match fetch_elevation_data(bbox, scale_h, scale_v, ground_level, local_lidar) {
            Ok(elevation_data) => Self {
                elevation_enabled: true,
                ground_level,
                elevation_data: Some(elevation_data),
                min_x: global_min_x,
                min_z: global_min_z,
            },
            Err(e) => {
                eprintln!("Failed to fetch elevation data: {}", e);
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Warning,
                    "Elevation unavailable, using flat ground",
                );
                // Graceful fallback: disable elevation and keep provided ground_level
                Self {
                    elevation_enabled: false,
                    ground_level,
                    elevation_data: None,
                    min_x: global_min_x,
                    min_z: global_min_z,
                }
            }
        }
    }

    /// Returns the ground level at the given ABSOLUTE Minecraft coordinates
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled {
            return self.ground_level;
        }

        if let Some(data) = &self.elevation_data {
            // ?? BESM-6 Tweak: Fim do Ratio Relativo. Mapeamento 1:1 Direto
            // Calcula o deslocamento do bloco atual em relação ao início do Grid
            let local_x = (coord.x - self.min_x) as f64;
            let local_z = (coord.z - self.min_z) as f64;

            self.interpolate_height_absolute(local_x, local_z, data)
        } else {
            self.ground_level
        }
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn min_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).min()
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn max_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).max()
    }

    /// Interpolates height value using Smootherstep Bilinear Interpolation
    /// on a contiguous 1D Array (Flat Vector) for massive memory performance.
    #[inline(always)]
    fn interpolate_height_absolute(&self, local_x: f64, local_z: f64, data: &ElevationData) -> i32 {
        // Clamp nas bordas (Segurança contra consultas fora do bloco do DF)
        let max_x = (data.width.saturating_sub(1)) as f64;
        let max_z = (data.height.saturating_sub(1)) as f64;

        let gx = local_x.clamp(0.0, max_x);
        let gz = local_z.clamp(0.0, max_z);

        // Pegar os índices dos 4 pixels (blocos) ao redor do nosso ponto
        let x1 = gx.floor() as usize;
        let x2 = (x1 + 1).min(data.width - 1);
        let z1 = gz.floor() as usize;
        let z2 = (z1 + 1).min(data.height - 1);

        // Distância fracionária do ponto x1 e z1 originais
        let mut tx = gx - x1 as f64;
        let mut tz = gz - z1 as f64;

        // O TWEAK DE ELITE: Smootherstep (Ken Perlin)
        tx = tx * tx * tx * (tx * (tx * 6.0 - 15.0) + 10.0);
        tz = tz * tz * tz * (tz * (tz * 6.0 - 15.0) + 10.0);

        // ?? BESM-6: Leitura em Matriz 1D (z * width + x)
        let w = data.width;
        let h00 = data.heights[z1 * w + x1] as f64;
        let h10 = data.heights[z1 * w + x2] as f64;
        let h01 = data.heights[z2 * w + x1] as f64;
        let h11 = data.heights[z2 * w + x2] as f64;

        // Interpolação no eixo X (Horizontal)
        let h0 = h00 * (1.0 - tx) + h10 * tx;
        let h1 = h01 * (1.0 - tx) + h11 * tx;

        // Interpolação final no eixo Z (Vertical/Profundidade)
        let final_height = h0 * (1.0 - tz) + h1 * tz;

        final_height.round() as i32
    }

    fn save_debug_image(&self, filename: &str) {
        let data = match &self.elevation_data {
            Some(d) => d,
            None => return,
        };
            
        if data.heights.is_empty() || data.width == 0 {
            return;
        }

        let height = data.height;
        let width = data.width;
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(width as u32, height as u32);

        let mut min_height: i32 = i32::MAX;
        let mut max_height: i32 = i32::MIN;

        for &h in &data.heights {
            min_height = min_height.min(h);
            max_height = max_height.max(h);
        }

        // Tweak de Segurança: Evita divisão por zero se o terreno for completamente plano
        let range = max_height - min_height;
        let safe_range = if range == 0 { 1.0 } else { range as f64 };

        for y in 0..height {
            for x in 0..width {
                let h = data.heights[y * width + x];
                let normalized: u8 =
                    (((h - min_height) as f64 / safe_range) * 255.0) as u8;
                img.put_pixel(
                    x as u32,
                    y as u32,
                    Rgb([normalized, normalized, normalized]),
                );
            }
        }

        // Ensure filename has .png extension
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };

        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save debug image: {e}");
        }
    }
}

pub fn generate_ground_data(args: &Args, xzbbox: &crate::coordinate_system::cartesian::XZBBox) -> Ground {
    if args.terrain {
        println!("{} Fetching elevation (LiDAR/SRTM)...", "[3/7]".bold());
        #[cfg(feature = "gui")]
        emit_gui_progress_update(14.0, "Fetching elevation...");
        
        let ground = Ground::new_enabled(
            &args.bbox,
            args.scale_h,
            args.scale_v,
            args.ground_level,
            args.local_lidar.as_ref(), // Passa o caminho do LiDAR se existir (Gov-Tier)
            xzbbox.min_x(), // Injeta o offset absoluto X
            xzbbox.min_z(), // Injeta o offset absoluto Z
        );
        
        if args.debug {
            ground.save_debug_image("elevation_debug");
        }
        return ground;
    }
    Ground::new_flat(args.ground_level)
}