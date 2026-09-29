//! Ground & Elevation Management (BESM-6 Government Tier)
//!
//! Este módulo é o Roteador Topográfico do mundo (Eixo Y): responde, em O(1),
//! qual é a cota do terreno nu, da superfície (telhados/copas, quando há DSM) e
//! do bioma oficial (MapBiomas/SICAR) em qualquer coluna (x, z) do mundo.
//!
//! # Contrato de coordenadas (a fonte do defeito corrigido aqui)
//!
//! Havia DUAS convenções misturadas no motor:
//!
//! * `Ground::level(XZPoint)` sempre foi documentado/consumido (herança do
//!   Arnis: `WorldEditor::get_ground_level`, `gui.rs`, `bedrock.rs`,
//!   `buildings.rs`) como recebendo coordenadas **relativas** ao canto mínimo
//!   do `XZBBox` (`x - min_x`, `z - min_z`).
//! * A cache de terreno, porém, era populada em `data_processing.rs` com chaves
//!   **absolutas** `(x, z)` do Minecraft.
//!
//! Como o `XZBBox` deste projeto é ancorado no Marco Zero de Brasília (Praça
//! dos Três Poderes) — e não em (0, 0) como no Arnis original — `min_x`/`min_z`
//! nunca são zero (Guará fica a ~15.000 blocos a oeste). Resultado: **toda**
//! consulta de elevação errava a chave, caía no fallback plano e o mundo saía
//! chato mesmo com `--terrain` e SRTM/LiDAR baixados com sucesso.
//!
//! Agora o `Ground` guarda a **origem** do grid e converte internamente:
//! `level`/`surface_level` continuam aceitando coordenadas relativas (contrato
//! legado, sem quebrar chamadores), e `level_abs`/`surface_level_abs`/
//! `get_biome` aceitam coordenadas absolutas. Não existe mais como errar a
//! convenção: as duas famílias convergem para o mesmo índice.
//!
//! # Grade única para o mundo inteiro (não mais fatiada por região)
//!
//! A elevação vem de `ElevationData` (grade densa do bbox inteiro, já em RAM).
//! Fatiá-la por região de 512×512 num `FxHashMap` — como era feito — custava
//! ~262 mil inserções de hash por região e, pior, fazia qualquer geometria que
//! "vazasse" de uma região para a vizinha (o Halo Cache) consultar um chão que
//! não existia naquela fatia, produzindo degraus/penhascos exatamente nas
//! bordas de região. Agora o `Ground` referencia a grade inteira (`Arc`, sem
//! cópia) e responde para qualquer coluna do bbox; fora dele, devolve a cota
//! da borda mais próxima (clamp), que é uma estimativa melhor do que "plano".

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::elevation_data::ElevationData;
use rustc_hash::FxHashMap;
use std::sync::Arc;

// 🚨 BESM-6: Importação do marcador de bioma nulo para o fallback seguro
use crate::providers::vegetation_provider::BIOME_NONE;

/// Grade densa de cotas (row-major: índice = row * width + col), relativa ao
/// canto mínimo do `XZBBox` (col 0 = `min_x`, row 0 = `min_z`).
#[derive(Clone)]
struct HeightGrid {
    origin_x: i32,
    origin_z: i32,
    width: usize,
    height: usize,
    heights: Arc<Vec<i32>>,
}

impl HeightGrid {
    /// Cota na coluna absoluta `(x, z)`, com clamp para a borda mais próxima
    /// quando a coluna cai fora do grid.
    #[inline(always)]
    fn sample_abs(&self, x: i32, z: i32) -> i32 {
        let col = (x as i64 - self.origin_x as i64).clamp(0, self.width as i64 - 1) as usize;
        let row = (z as i64 - self.origin_z as i64).clamp(0, self.height as i64 - 1) as usize;
        self.heights[row * self.width + col]
    }
}

/// Represents terrain data, stratified into Bare Earth and Surface Canopy.
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    pub ground_level: i32,

    /// Terreno nu (SRTM/LiDAR/DEM), grade densa do bbox inteiro. `None` quando
    /// a topografia está desligada ou nenhuma fonte de elevação foi obtida.
    bare_earth: Option<HeightGrid>,

    /// Superfície (DSM: telhados/copas), esparsa, chaves ABSOLUTAS `(x, z)`.
    canopy_surface_cache: Arc<FxHashMap<(i32, i32), i32>>,

    /// 🚨 BESM-6: Cache O(1) para a Verdade Biológica do MapBiomas/SICAR,
    /// chaves ABSOLUTAS `(x, z)`.
    biome_cache: Arc<FxHashMap<(i32, i32), u16>>,
}

/// Cotas de topografia nunca ultrapassam este teto: o escritor de chunks
/// (`world_editor::common`) trunca qualquer Y acima de 319 (limite do formato
/// Java atual), e ainda é preciso sobrar espaço vertical para os prédios em
/// cima do relevo.
pub const TERRAIN_MAX_Y: i32 = crate::world_editor::WORLD_MAX_Y - TERRAIN_HEADROOM;
/// Espaço vertical reservado acima do ponto mais alto do relevo para
/// edificações/torres (blocos).
pub const TERRAIN_HEADROOM: i32 = 100;

impl Ground {
    /// O Mundo Estéril (Usado se a topografia for desativada via CLI)
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            ground_level,
            bare_earth: None,
            canopy_surface_cache: Arc::new(FxHashMap::default()),
            biome_cache: Arc::new(FxHashMap::default()),
        }
    }

    /// Monta o `Ground` definitivo do mundo a partir de todas as fontes reais
    /// disponíveis — chamado UMA vez por geração (não por região):
    ///
    /// * `elevation` — SRTM/LiDAR (`elevation_data::fetch_elevation_data`),
    ///   já em blocos, com o ponto mais baixo em `ground_level`.
    /// * `dem_override` — DEM GeoTIFF local (`DemProvider`), em blocos mas num
    ///   datum ABSOLUTO (`ground_level + metros × scale_v`). Antes era fundido
    ///   cru sobre o SRTM (datum relativo), o que colocava o terreno do DEM
    ///   ~1.000+ blocos acima do resto (cota real do Planalto Central), acima
    ///   do teto do mundo — virava um platô truncado em Y=319. Aqui é
    ///   re-baseado: quando há SRTM, pelo deslocamento mediano nas colunas em
    ///   comum; sem SRTM, pelo próprio mínimo do DEM → `ground_level`.
    /// * `surface` — DSM (`DsmProvider`), mesmo datum absoluto do DEM; recebe
    ///   o mesmo re-baseamento (ou, sem DEM, `ground_level - min(dsm)`).
    /// * `biome` — MapBiomas/IBGE/SICAR, chaves absolutas, usado como está.
    pub fn assemble(
        ground_level: i32,
        xzbbox: &XZBBox,
        elevation: Option<&ElevationData>,
        dem_override: Option<&FxHashMap<(i32, i32), i32>>,
        surface: Option<Arc<FxHashMap<(i32, i32), i32>>>,
        biome: Option<Arc<FxHashMap<(i32, i32), u16>>>,
    ) -> Self {
        let origin_x = xzbbox.min_x();
        let origin_z = xzbbox.min_z();
        let width = (xzbbox.max_x() as i64 - origin_x as i64 + 1).max(1) as usize;
        let height = (xzbbox.max_z() as i64 - origin_z as i64 + 1).max(1) as usize;

        let mut base: Option<HeightGrid> = elevation.map(|e| {
            if e.width() == width && e.height() == height {
                HeightGrid {
                    origin_x,
                    origin_z,
                    width,
                    height,
                    heights: e.shared_heights(),
                }
            } else {
                // Grade de outra dimensão (fonte externa): reamostra por
                // vizinho mais próximo para o tamanho exato do bbox.
                let mut resampled = vec![ground_level; width * height];
                for (row, chunk) in resampled.chunks_mut(width).enumerate() {
                    let src_row = ((row as f64 + 0.5) * e.height() as f64 / height as f64) as usize;
                    for (col, cell) in chunk.iter_mut().enumerate() {
                        let src_col =
                            ((col as f64 + 0.5) * e.width() as f64 / width as f64) as usize;
                        *cell = e.get_clamped(src_col, src_row);
                    }
                }
                HeightGrid {
                    origin_x,
                    origin_z,
                    width,
                    height,
                    heights: Arc::new(resampled),
                }
            }
        });

        // Deslocamento aplicado ao datum absoluto do DEM/DSM (blocos).
        let mut absolute_datum_shift: Option<i32> = None;

        if let Some(dem) = dem_override.filter(|d| !d.is_empty()) {
            let in_bbox = |&(&(x, z), _): &(&(i32, i32), &i32)| {
                x >= origin_x && x <= xzbbox.max_x() && z >= origin_z && z <= xzbbox.max_z()
            };

            let shift = match &base {
                Some(grid) => {
                    let mut diffs: Vec<i32> = dem
                        .iter()
                        .filter(in_bbox)
                        .map(|(&(x, z), &h)| grid.sample_abs(x, z) - h)
                        .collect();
                    if diffs.is_empty() {
                        0
                    } else {
                        let mid = diffs.len() / 2;
                        *diffs.select_nth_unstable(mid).1
                    }
                }
                None => {
                    let min = dem
                        .iter()
                        .filter(in_bbox)
                        .map(|(_, &h)| h)
                        .min()
                        .unwrap_or(ground_level);
                    ground_level - min
                }
            };
            absolute_datum_shift = Some(shift);

            let mut merged: Vec<i32> = match &base {
                Some(grid) => (*grid.heights).clone(),
                None => vec![ground_level; width * height],
            };
            for (&(x, z), &h) in dem.iter() {
                if x < origin_x || x > xzbbox.max_x() || z < origin_z || z > xzbbox.max_z() {
                    continue;
                }
                let col = (x - origin_x) as usize;
                let row = (z - origin_z) as usize;
                merged[row * width + col] = (h + shift).clamp(ground_level, TERRAIN_MAX_Y);
            }
            base = Some(HeightGrid {
                origin_x,
                origin_z,
                width,
                height,
                heights: Arc::new(merged),
            });
        }

        let canopy_surface_cache = match surface {
            Some(dsm) if !dsm.is_empty() => {
                let shift = absolute_datum_shift.unwrap_or_else(|| {
                    let min = dsm.values().copied().min().unwrap_or(ground_level);
                    ground_level - min
                });
                if shift == 0 {
                    dsm
                } else {
                    Arc::new(
                        dsm.iter()
                            .map(|(&k, &h)| {
                                (
                                    k,
                                    (h + shift)
                                        .clamp(ground_level, crate::world_editor::WORLD_MAX_Y),
                                )
                            })
                            .collect(),
                    )
                }
            }
            Some(dsm) => dsm,
            None => Arc::new(FxHashMap::default()),
        };

        Self {
            elevation_enabled: base.is_some(),
            ground_level,
            bare_earth: base,
            canopy_surface_cache,
            biome_cache: biome.unwrap_or_else(|| Arc::new(FxHashMap::default())),
        }
    }

    /// Retorna a altura do Terreno Nu (Bare Earth) na coordenada RELATIVA ao
    /// canto mínimo do `XZBBox` (contrato legado de `WorldEditor::get_ground_level`,
    /// `gui.rs`, `bedrock.rs`, `buildings.rs`).
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        match &self.bare_earth {
            Some(grid) => grid.sample_abs(
                grid.origin_x.saturating_add(coord.x),
                grid.origin_z.saturating_add(coord.z),
            ),
            None => self.ground_level,
        }
    }

    /// Mesma consulta de `level`, em coordenadas ABSOLUTAS do Minecraft.
    #[inline(always)]
    pub fn level_abs(&self, x: i32, z: i32) -> i32 {
        match &self.bare_earth {
            Some(grid) => grid.sample_abs(x, z),
            None => self.ground_level,
        }
    }

    /// Retorna a altura absoluta da Superfície (Telhados, copas de árvores,
    /// pontes) na coordenada RELATIVA. Sem DSM naquela coluna, cai para o chão nu.
    /// Contraparte relativa de `surface_level_abs` (mesmo contrato de `level`);
    /// os consumidores de produção usam a variante absoluta.
    #[cfg_attr(not(test), allow(dead_code))]
    #[inline(always)]
    pub fn surface_level(&self, coord: XZPoint) -> i32 {
        match &self.bare_earth {
            Some(grid) => self.surface_level_abs(
                grid.origin_x.saturating_add(coord.x),
                grid.origin_z.saturating_add(coord.z),
            ),
            None => self.ground_level,
        }
    }

    /// Mesma consulta de `surface_level`, em coordenadas ABSOLUTAS.
    #[inline(always)]
    pub fn surface_level_abs(&self, x: i32, z: i32) -> i32 {
        if let Some(surface_y) = self.canopy_surface_cache.get(&(x, z)) {
            return *surface_y;
        }
        self.level_abs(x, z)
    }

    /// 🚨 BESM-6: Consulta O(1) da tipologia do bioma oficial (MapBiomas/SICAR),
    /// coordenadas ABSOLUTAS.
    #[inline(always)]
    pub fn get_biome(&self, x: i32, z: i32) -> u16 {
        *self.biome_cache.get(&(x, z)).unwrap_or(&BIOME_NONE)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox() -> XZBBox {
        // Guará-like: origem bem longe de (0,0), como no mundo real deste motor.
        XZBBox::new(-15000, -14997, 4000, 4002)
    }

    fn elevation_4x3(values: [[i32; 4]; 3]) -> ElevationData {
        ElevationData::from_rows(values.iter().map(|r| r.to_vec()).collect())
    }

    /// Regressão do defeito central: chave absoluta na cache, consulta relativa.
    #[test]
    fn relative_and_absolute_lookups_agree_with_non_zero_origin() {
        let elevation = elevation_4x3([[10, 11, 12, 13], [20, 21, 22, 23], [30, 31, 32, 33]]);
        let ground = Ground::assemble(-62, &bbox(), Some(&elevation), None, None, None);

        assert!(ground.elevation_enabled);
        assert_eq!(ground.level(XZPoint::new(0, 0)), 10);
        assert_eq!(ground.level_abs(-15000, 4000), 10);
        assert_eq!(ground.level(XZPoint::new(3, 2)), 33);
        assert_eq!(ground.level_abs(-14997, 4002), 33);
        assert_eq!(ground.level(XZPoint::new(1, 1)), 21);
    }

    #[test]
    fn out_of_grid_lookup_clamps_to_nearest_edge_instead_of_flat() {
        let elevation = elevation_4x3([[10, 11, 12, 13], [20, 21, 22, 23], [30, 31, 32, 33]]);
        let ground = Ground::assemble(-62, &bbox(), Some(&elevation), None, None, None);
        assert_eq!(ground.level_abs(-15010, 3990), 10);
        assert_eq!(ground.level_abs(-14990, 4010), 33);
        assert_eq!(ground.level(XZPoint::new(-5, 1)), 20);
    }

    #[test]
    fn flat_ground_without_sources() {
        let ground = Ground::assemble(-62, &bbox(), None, None, None, None);
        assert!(!ground.elevation_enabled);
        assert_eq!(ground.level(XZPoint::new(2, 2)), -62);
        assert_eq!(ground.level_abs(-14998, 4001), -62);
        assert_eq!(ground.surface_level_abs(-14998, 4001), -62);
    }

    /// O DEM local chega em datum absoluto (cota real × scale_v): precisa ser
    /// re-baseado para o datum relativo do SRTM, não fundido cru.
    #[test]
    fn dem_override_is_rebased_onto_the_srtm_datum() {
        let elevation = elevation_4x3([[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let mut dem = FxHashMap::default();
        // Cota real ~1.200 m já escalada: 1.000 blocos acima do datum relativo.
        dem.insert((-15000, 4000), 1000);
        dem.insert((-14999, 4000), 1000);
        dem.insert((-14998, 4000), 1005);
        let ground = Ground::assemble(-62, &bbox(), Some(&elevation), Some(&dem), None, None);
        assert_eq!(ground.level_abs(-15000, 4000), 0);
        assert_eq!(ground.level_abs(-14998, 4000), 5);
        // Colunas sem DEM mantêm o SRTM.
        assert_eq!(ground.level_abs(-14997, 4002), 0);
    }

    #[test]
    fn dem_only_world_rebases_its_minimum_to_ground_level() {
        let mut dem = FxHashMap::default();
        dem.insert((-15000, 4000), 1300);
        dem.insert((-14999, 4001), 1310);
        let ground = Ground::assemble(-62, &bbox(), None, Some(&dem), None, None);
        assert!(ground.elevation_enabled);
        assert_eq!(ground.level_abs(-15000, 4000), -62);
        assert_eq!(ground.level_abs(-14999, 4001), -52);
        // Buraco sem dado: chão base.
        assert_eq!(ground.level_abs(-14997, 4002), -62);
    }

    #[test]
    fn surface_level_uses_dsm_where_present_and_ground_elsewhere() {
        let elevation = elevation_4x3([[5, 5, 5, 5], [5, 5, 5, 5], [5, 5, 5, 5]]);
        let mut dsm = FxHashMap::default();
        dsm.insert((-14999, 4001), 40); // datum já relativo (mínimo será re-baseado)
        dsm.insert((-15000, 4000), 5);
        let ground = Ground::assemble(
            -62,
            &bbox(),
            Some(&elevation),
            None,
            Some(Arc::new(dsm)),
            None,
        );
        // Sem DEM, o DSM é re-baseado por ground_level - min(dsm) = -67.
        assert_eq!(ground.surface_level_abs(-14999, 4001), 40 - 67);
        assert_eq!(ground.surface_level_abs(-14998, 4002), 5);
        assert_eq!(ground.surface_level(XZPoint::new(1, 1)), 40 - 67);
    }

    #[test]
    fn elevation_grid_of_different_size_is_resampled_to_bbox() {
        // Grade 2x2 para um bbox 4x3.
        let elevation = ElevationData::from_rows(vec![vec![1, 2], vec![3, 4]]);
        let ground = Ground::assemble(-62, &bbox(), Some(&elevation), None, None, None);
        assert_eq!(ground.level(XZPoint::new(0, 0)), 1);
        assert_eq!(ground.level(XZPoint::new(3, 0)), 2);
        assert_eq!(ground.level(XZPoint::new(0, 2)), 3);
        assert_eq!(ground.level(XZPoint::new(3, 2)), 4);
    }
}
