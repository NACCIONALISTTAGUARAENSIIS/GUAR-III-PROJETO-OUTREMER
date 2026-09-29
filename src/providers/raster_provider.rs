use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer; // BESM-6: Motor ECEF
use crate::providers::{DataProvider, Feature, GeometryType, SemanticGroup};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use tiff::decoder::{Decoder, DecodingResult};

/// Provedor de Dados Matriciais (Raster / GeoTIFF).
/// Projetado para ler classificaçoes de sat�lite (MapBiomas, SISDIA, NDWI para rios e NDVI para vegeta��o).
/// Varre ficheiros TIFF onde 1 pixel = 1 metro e converte a natureza org�nica em matrizes Voxel exatas.
pub struct RasterProvider {
    pub file_path: PathBuf,
    pub scale_h: f64,
    pub priority: u8,
    // Par�metros de georreferencia��o do TIFF (Assumindo WGS84 ou UTM fornecido externamente)
    pub top_left_lat: f64,
    pub top_left_lon: f64,
    pub pixel_size_degrees: f64, // Resolu��o do pixel em graus (ex: 0.00001 para ~1 metro)
}

impl RasterProvider {
    /// 🚨 STATUS DE CONEXÃO: nenhum chamador hoje. Classifica raster genérico
    /// (NDVI/NDWI) em Natural/Waterway/Terrain — sobrepõe em parte o que
    /// `VegetationProvider` (já conectado, MapBiomas-específico) cobre para
    /// vegetação; não conectei para não arriscar dupla-classificação sem
    /// entender se este é complementar ou um provider anterior superado por
    /// aquele. Ver tarefa de acompanhamento sugerida nesta sessão.
    #[allow(dead_code)]
    pub fn new(
        file_path: PathBuf,
        scale_h: f64,
        priority: u8,
        top_left_lat: f64,
        top_left_lon: f64,
        pixel_size_degrees: f64,
    ) -> Self {
        Self {
            file_path,
            scale_h,
            priority,
            top_left_lat,
            top_left_lon,
            pixel_size_degrees,
        }
    }

    /// O "Rosetta Stone" Natural: Converte o valor num�rico do pixel (Classifica��o do Sat�lite)
    /// numa tag sem�ntica org�nica para o motor de gera��o.
    #[inline(always)]
    fn pixel_value_to_tags(pixel_val: u8) -> Option<(SemanticGroup, HashMap<String, String>)> {
        let mut tags = HashMap::new();
        tags.insert("source".to_string(), "GDF_Raster_GeoTIFF".to_string());

        // Tabela de Classifica��o Padr�o (Exemplo adaptado do MapBiomas / IBAMA)
        match pixel_val {
            1..=5 => {
                // Forma��es Florestais (Mata de Galeria, Cerrad�o)
                tags.insert("natural".to_string(), "wood".to_string());
                tags.insert("leaf_type".to_string(), "broadleaved".to_string());
                tags.insert("density".to_string(), "high".to_string()); // Floresta densa
                Some((SemanticGroup::Natural, tags))
            }
            10..=13 => {
                // Forma��es Sav�nicas e Campestres (Cerrado Ralo)
                tags.insert("natural".to_string(), "scrub".to_string());
                Some((SemanticGroup::Natural, tags))
            }
            33 => {
                // Corpos d'�gua (Rios, Lago Parano�, Ribeir�es)
                tags.insert("natural".to_string(), "water".to_string());
                tags.insert("water".to_string(), "river".to_string());
                Some((SemanticGroup::Waterway, tags))
            }
            34 => {
                // Zonas H�midas / V�rzeas
                tags.insert("natural".to_string(), "wetland".to_string());
                Some((SemanticGroup::Terrain, tags))
            }
            _ => None, // Classes urbanas (24, 25) s�o ignoradas aqui pois os vetores s�o melhores
        }
    }
}

impl DataProvider for RasterProvider {
    fn priority(&self) -> u8 {
        self.priority
    }
    fn describe_sources(&self) -> Vec<String> {
        vec![self.file_path.display().to_string()]
    }
    fn name(&self) -> &str {
        "Nature Raster GeoTIFF (MapBiomas / SISDIA)"
    }

    fn fetch_features(&self, bbox: &LLBBox) -> Result<Vec<Feature>, String> {
        println!(
            "[INFO] ?? A abrir malha de sat�lite GeoTIFF (Natureza 1m): {}",
            self.file_path.display()
        );

        let file = File::open(&self.file_path)
            .map_err(|e| format!("Falha ao abrir ficheiro TIFF: {}", e))?;

        let mut decoder = Decoder::new(file)
            .map_err(|e| format!("Falha ao inicializar descodificador TIFF: {}", e))?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| format!("Falha ao ler dimens�es do TIFF: {}", e))?;

        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(bbox, self.scale_h)
            .map_err(|e| format!("Falha ao inicializar o transformador de coordenadas: {}", e))?;

        let mut features = Vec::new();
        let mut next_id = 9_000_000_000; // Offset dedicado para Natureza Rasterizada

        // ?? BESM-6 Tweak: Lemos a imagem nativamente. Para n�o rebentar a RAM com imagens
        // de sat�lite de 40.000 x 40.000 pix�is, o processamento deve ser estritamente matem�tico.
        if let Ok(DecodingResult::U8(image_data)) = decoder.read_image() {
            println!(
                "[INFO] TIFF carregado na mem�ria. Resolu��o: {}x{} pix�is.",
                width, height
            );

            // Calcula os limites dos pix�is que caem dentro da BBox do jogador
            // Early-Culling Matem�tico (Ignora loops desnecess�rios)
            let start_x =
                ((bbox.min().lng() - self.top_left_lon) / self.pixel_size_degrees).max(0.0) as u32;
            let end_x = ((bbox.max().lng() - self.top_left_lon) / self.pixel_size_degrees)
                .min(width as f64) as u32;

            // Latitude diminui � medida que descemos na imagem
            let start_y =
                ((self.top_left_lat - bbox.max().lat()) / self.pixel_size_degrees).max(0.0) as u32;
            let end_y = ((self.top_left_lat - bbox.min().lat()) / self.pixel_size_degrees)
                .min(height as f64) as u32;

            if start_x >= width || start_y >= height || start_x >= end_x || start_y >= end_y {
                println!("[AVISO] O GeoTIFF est� fora da Bounding Box atual. Saltando.");
                return Ok(features);
            }

            for y in start_y..end_y {
                for x in start_x..end_x {
                    let pixel_index = (y * width + x) as usize;
                    let pixel_val = image_data[pixel_index];

                    // Se for um pixel de natureza mapeada
                    if let Some((semantic_group, tags)) = Self::pixel_value_to_tags(pixel_val) {
                        let lat = self.top_left_lat - (y as f64 * self.pixel_size_degrees);
                        let lon = self.top_left_lon + (x as f64 * self.pixel_size_degrees);

                        if let Ok(llpoint) = LLPoint::new(lat, lon) {
                            if bbox.contains(&llpoint) {
                                let xz_point = transformer.transform_point(llpoint);

                                // Renderiza cada pixel natural de 1m como um bloco/pol�gono de 1x1
                                let voxel_poly = vec![
                                    XZPoint::new(xz_point.x, xz_point.z),
                                    XZPoint::new(xz_point.x + 1, xz_point.z),
                                    XZPoint::new(xz_point.x + 1, xz_point.z + 1),
                                    XZPoint::new(xz_point.x, xz_point.z + 1),
                                ];

                                features.push(Feature::new(
                                    next_id,
                                    semantic_group,
                                    tags,
                                    GeometryType::Polygon(voxel_poly),
                                    "GDF_Nature_Raster".to_string(),
                                    self.priority,
                                ));
                                next_id += 1;
                            }
                        }
                    }
                }
            }
        } else {
            eprintln!("[AVISO] Formato de cor do TIFF n�o � U8 nativo. Apenas matrizes de classifica��o suportadas.");
        }

        features.shrink_to_fit();
        println!(
            "[INFO] ? Processamento Raster conclu�do: {} voxels org�nicos injetados.",
            features.len()
        );
        Ok(features)
    }
}
