//! Urban ground detection and generation based on building clusters.
//!
//! This module computes urban areas by analyzing building density and clustering,
//! then generates appropriate ground blocks (smooth stone) for those areas.
//!
//! ?? BESM-6: A arquitetura agora usa um GRID GLOBAL ABSOLUTO.
//! Nao h� depend�ncia da Bounding Box do mapa gerado no momento.
//! Isso garante determinismo perfeito entre execu��es de sat�lites diferentes.
//! Expans�o baseada em Raio Euclidiano para evitar "escadas" na borda do Cerrado.

use crate::coordinate_system::cartesian::XZBBox;
use rustc_hash::{FxHashMap, FxHashSet}; // BESM-6: Hashing ultrarr�pido
use std::collections::VecDeque;

/// Configuration for urban ground detection.
#[derive(Debug, Clone)]
pub struct UrbanGroundConfig {
    /// Grid cell size for density analysis (in blocks).
    /// Deve ser 16 (1 Chunk) para garantir o shift de bit O(1).
    pub cell_size: i32,

    /// Minimum elements (buildings/roads) per cell to consider it potentially urban.
    pub min_elements_per_cell: u16,

    /// Minimum total elements in a connected cluster to be considered urban.
    pub min_elements_for_cluster: u16,

    /// Base number of cells to expand the urban region.
    pub cell_expansion: i32,
}

impl Default for UrbanGroundConfig {
    fn default() -> Self {
        Self {
            // BRAS�LIA TWEAK ELITE: Finer granularity (16 blocks = 1 Chunk).
            // Garante o alinhamento com a grade do Minecraft (>> 4).
            cell_size: 16,
            min_elements_per_cell: 1,
            // ?? BESM-6: Elevado para 3 para ignorar cabanas/ranchos rurais isolados no Cerrado,
            // mas baixo o suficiente para pegar postos de gasolina/com�rcios locais.
            min_elements_for_cluster: 3,
            // ?? BESM-6: Expans�o suavizada. O ch�o de concreto envolver� as edifica��es.
            cell_expansion: 2,
        }
    }
}

/// Represents a detected urban cluster with its computed boundary.
#[derive(Debug)]
#[allow(dead_code)]
pub struct UrbanCluster {
    /// Grid cells that belong to this cluster
    cells: Vec<(i32, i32)>,
    /// Total number of elements in the cluster
    element_count: u64,
}

/// A compact lookup structure for checking if a coordinate is in an urban area.
///
/// Instead of storing millions of individual coordinates, this stores only
/// the cell indices (thousands) and performs O(1) lookups.
#[derive(Debug, Clone)]
pub struct UrbanGroundLookup {
    /// Set of cell indices (cx, cz) that are urban (Global Coordinates)
    urban_cells: FxHashSet<(i32, i32)>,
}

impl UrbanGroundLookup {
    /// Creates an empty lookup (no urban areas).
    pub fn empty() -> Self {
        Self {
            urban_cells: FxHashSet::default(),
        }
    }

    /// Returns true if the given absolute world coordinate is in an urban area.
    /// ?? BESM-6 Tweak: Utiliza Bit-Shift O(1) PURO e GLOBAL.
    #[inline(always)]
    pub fn is_urban(&self, x: i32, z: i32) -> bool {
        if self.urban_cells.is_empty() {
            return false;
        }

        // Fast path matem�tico O(1) via Arithmetic Right Shift.
        // O Rust lida perfeitamente com valores negativos no >> 4 (que equivale ao floor division).
        let cx = x >> 4;
        let cz = z >> 4;

        self.urban_cells.contains(&(cx, cz))
    }

    /// Returns the number of urban cells.
    #[allow(dead_code)]
    pub fn cell_count(&self) -> usize {
        self.urban_cells.len()
    }

    /// Returns true if there are no urban areas.
    pub fn is_empty(&self) -> bool {
        self.urban_cells.is_empty()
    }
}

/// Computes urban ground areas from building locations and roads.
pub struct UrbanGroundComputer {
    config: UrbanGroundConfig,
    // ?? BESM-6: Mem�ria Optimizada O(1).
    // Em vez de guardar as exatas 100 mil coordenadas dos pr�dios, guardamos apenas
    // a c�lula onde eles ca�ram e incrementamos o contador de densidade.
    density_grid: FxHashMap<(i32, i32), u16>,
    total_anchors: u64, // ?? Prote��o contra Continental Overflow (u64 em vez de u32)
}

impl UrbanGroundComputer {
    /// Creates a new urban ground computer with the given configuration.
    pub fn new(config: UrbanGroundConfig) -> Self {
        Self {
            config,
            density_grid: FxHashMap::default(),
            total_anchors: 0,
        }
    }

    /// Creates a new urban ground computer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(UrbanGroundConfig::default())
    }

    /// Adds an anchor point to be considered for urban area detection.
    /// A coordenada fornecida deve ser a global do mundo.
    #[inline]
    pub fn add_anchor(&mut self, x: i32, z: i32) {
        let cell_x = x >> 4;
        let cell_z = z >> 4;

        let counter = self.density_grid.entry((cell_x, cell_z)).or_insert(0);

        // Evita overflow se houverem milhares de pr�dios na mesma c�lula
        if *counter < u16::MAX {
            *counter += 1;
            self.total_anchors += 1;
        }
    }

    /// Adds multiple anchors from an iterator.
    pub fn add_anchors<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (i32, i32)>,
    {
        for (x, z) in iter {
            self.add_anchor(x, z);
        }
    }

    /// Returns the number of anchors added.
    #[allow(dead_code)]
    pub fn anchor_count(&self) -> u64 {
        self.total_anchors
    }

    /// Computes urban ground and returns a compact lookup structure.
    pub fn compute_lookup(&self) -> UrbanGroundLookup {
        if self.total_anchors < self.config.min_elements_for_cluster as u64 {
            return UrbanGroundLookup::empty();
        }

        let clusters = self.find_urban_clusters();

        if clusters.is_empty() {
            return UrbanGroundLookup::empty();
        }

        let mut urban_cells = FxHashSet::default();
        for cluster in clusters {
            urban_cells.extend(cluster.cells.iter().copied());
        }

        UrbanGroundLookup { urban_cells }
    }

    /// Finds connected clusters of urban cells.
    fn find_urban_clusters(&self) -> Vec<UrbanCluster> {
        let dense_cells: FxHashSet<(i32, i32)> = self
            .density_grid
            .iter()
            .filter(|(_, &count)| count >= self.config.min_elements_per_cell)
            .map(|(&cell, _)| cell)
            .collect();

        if dense_cells.is_empty() {
            return Vec::new();
        }

        // Expans�o Radial/Euclidiana Fixa. Como o motor agora � Scanline, a densidade
        // local � a �nica que importa. O "falso diagn�stico" global foi desmantelado.
        let expanded_cells = self.expand_cells_radial(&dense_cells, self.config.cell_expansion);

        let mut visited = FxHashSet::default();
        let mut clusters = Vec::new();

        for &cell in &expanded_cells {
            if visited.contains(&cell) {
                continue;
            }

            let mut component_cells = Vec::new();
            let mut queue = VecDeque::new();
            queue.push_back(cell);
            visited.insert(cell);

            // BFS para agrupar o bairro
            while let Some(current) = queue.pop_front() {
                component_cells.push(current);

                for dz in -1i32..=1i32 {
                    for dx in -1i32..=1i32 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        let neighbor = (current.0 + dx, current.1 + dz);
                        if expanded_cells.contains(&neighbor) && !visited.contains(&neighbor) {
                            visited.insert(neighbor);
                            queue.push_back(neighbor);
                        }
                    }
                }
            }

            let mut element_count: u64 = 0;
            for &c in &component_cells {
                if let Some(&count) = self.density_grid.get(&c) {
                    element_count += count as u64;
                }
            }

            if element_count >= self.config.min_elements_for_cluster as u64 {
                clusters.push(UrbanCluster {
                    cells: component_cells,
                    element_count,
                });
            }
        }

        clusters
    }

    /// ?? BESM-6 TWEAK: Expans�o Euclidiana.
    /// Em vez de iterar em um quadrado perfeito (for dx, for dz) que gera quinas em 90 graus
    /// (Efeito Escada), filtramos os Chunks pela dist�ncia radial circular.
    fn expand_cells_radial(
        &self,
        cells: &FxHashSet<(i32, i32)>,
        expansion: i32,
    ) -> FxHashSet<(i32, i32)> {
        if expansion <= 0 {
            return cells.clone();
        }

        let mut expanded = cells.clone();
        let max_dist_sq = expansion * expansion;

        for &(cx, cz) in cells {
            for dz in -expansion..=expansion {
                for dx in -expansion..=expansion {
                    // Filtro Euclidiano (Arredondamento org�nico de bordas de asfalto)
                    if dx * dx + dz * dz <= max_dist_sq {
                        expanded.insert((cx + dx, cz + dz));
                    }
                }
            }
        }

        expanded
    }

    #[allow(dead_code)]
    fn expand_cells(&self, cells: &FxHashSet<(i32, i32)>) -> FxHashSet<(i32, i32)> {
        self.expand_cells_radial(cells, self.config.cell_expansion)
    }
}

#[allow(dead_code)]
pub fn compute_urban_ground_lookup(
    anchors: Vec<(i32, i32)>,
    _xzbbox: &XZBBox, // Mantido na assinatura para compatibilidade com o data_processing
) -> UrbanGroundLookup {
    let mut computer = UrbanGroundComputer::with_defaults();
    computer.add_anchors(anchors);
    computer.compute_lookup()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_buildings() {
        let computer = UrbanGroundComputer::with_defaults();
        let result = computer.compute_lookup();
        assert!(result.is_empty());
    }

    #[test]
    fn test_few_scattered_buildings() {
        let mut computer = UrbanGroundComputer::with_defaults();
        computer.add_anchor(100, 100);
        computer.add_anchor(500, 500);
        computer.add_anchor(900, 900);

        let result = computer.compute_lookup();
        assert!(
            result.is_empty(),
            "Scattered buildings should not form urban area"
        );
    }

    #[test]
    fn test_dense_cluster() {
        let mut computer = UrbanGroundComputer::with_defaults();

        for i in 0..30 {
            for j in 0..30 {
                if (i + j) % 3 == 0 {
                    computer.add_anchor(100 + i * 10, 100 + j * 10);
                }
            }
        }

        let result = computer.compute_lookup();
        assert!(
            !result.is_empty(),
            "Dense cluster should produce urban area"
        );
    }

    #[test]
    fn test_lookup_empty() {
        let lookup = UrbanGroundLookup::empty();
        assert!(lookup.is_empty());
        assert!(!lookup.is_urban(100, 100));
        assert_eq!(lookup.cell_count(), 0);
    }

    #[test]
    fn test_lookup_membership() {
        let mut computer = UrbanGroundComputer::with_defaults();

        for x in 0..10 {
            for z in 0..10 {
                computer.add_anchor(100 + x * 10, 100 + z * 10);
            }
        }

        let lookup = computer.compute_lookup();
        assert!(!lookup.is_empty());

        assert!(
            lookup.is_urban(150, 150),
            "Center of cluster should be urban"
        );

        assert!(
            !lookup.is_urban(900, 900),
            "Point far from cluster should not be urban"
        );
    }
}
