//! Ground & Elevation Management (BESM-6 Government Tier)
//!
//! Este módulo atua como o Roteador Topográfico do mundo (Eixo Y).
//! Para impedir o colapso de 50GB de RAM por alocações matriciais contíguas
//! numa escala do Distrito Federal, ele não guarda dados globais.
//! Ele delega as consultas de altimetria em tempo constante O(1) diretamente
//! para os provedores de DEM e DSM, que operam isoladamente na janela do Scanline.

use crate::coordinate_system::cartesian::XZPoint;
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// Represents terrain data, stratified into Bare Earth and Surface Canopy.
/// 🚨 BESM-6: Matrizes globais vetoriais (`Vec<i32>`) foram extintas.
/// Apenas o extrato quantizado O(1) da região atual (Scanline) é referenciado aqui.
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    pub ground_level: i32,

    // Caches locais do quadrante Scanline atual injetados pelo orquestrador
    bare_earth_cache: Arc<FxHashMap<(i32, i32), i32>>,
    canopy_surface_cache: Arc<FxHashMap<(i32, i32), i32>>,
}

impl Ground {
    /// O Mundo Estéril (Usado se a topografia for desativada via CLI)
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            ground_level,
            bare_earth_cache: Arc::new(FxHashMap::default()),
            canopy_surface_cache: Arc::new(FxHashMap::default()),
        }
    }

    /// O Construtor Orgânico Scanline: Recebe apenas as tabelas hash da região atual.
    /// Evita a alocação de ~50GB de RAM de um array global do tamanho do DF.
    pub fn new_enabled(
        ground_level: i32,
        bare_earth_cache: Arc<FxHashMap<(i32, i32), i32>>,
        canopy_surface_cache: Arc<FxHashMap<(i32, i32), i32>>,
    ) -> Self {
        Self {
            elevation_enabled: true,
            ground_level,
            bare_earth_cache,
            canopy_surface_cache,
        }
    }

    /// Retorna a altura do Terreno Nu (Bare Earth) na coordenada absoluta.
    /// Delega a consulta direta à tabela hash pré-quantizada.
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled {
            return self.ground_level;
        }

        // Se o provedor DEM não encontrou dados para esta coordenada exata
        // (por falta de pontos do LiDAR ou buraco no raster), assumimos o ground level global.
        // O algoritmo de Blur e Preenchimento O(1) do dem_provider deve cobrir >99% dos casos.
        *self
            .bare_earth_cache
            .get(&(coord.x, coord.z))
            .unwrap_or(&self.ground_level)
    }

    /// Retorna a altura absoluta da Superfície (Telhados, copas de árvores, pontes).
    /// Usado primariamente para extração do pé-direito de extrusão: (Surface Y - Bare Y)
    #[inline(always)]
    pub fn surface_level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled {
            return self.ground_level;
        }

        // Tenta puxar a superfície. Se não existir DSM, fallback para o DEM (Chão).
        if let Some(surface_y) = self.canopy_surface_cache.get(&(coord.x, coord.z)) {
            *surface_y
        } else {
            self.level(coord) // Se não tem teto, o teto é o chão
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
}

// 🚨 O Gerador Original (generate_ground_data) foi DELETADO e movido para a
// lógica de Scanline dentro de `data_processing.rs`, pois ele não pode mais
// ser rodado globalmente antes do motor começar a andar pelas regiões.
