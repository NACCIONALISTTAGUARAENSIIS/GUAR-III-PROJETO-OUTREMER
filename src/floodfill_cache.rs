//! Flood fill cache for polygon filling (Scanline Out-Of-Core Engine).
//!
//! ?? BESM-6 TWEAK: Pre-computation has been eradicated to prevent OOM.
//! The cache is now populated lazily on-demand per region and purged periodically 
//! by the Scanline engine.

use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedWay};
use fnv::FnvHashMap;
use std::time::Duration;

/// A memory-efficient bitmap for storing coordinates.
///
/// Instead of storing each coordinate individually (~24 bytes per entry in a HashSet),
/// this uses 1 bit per coordinate in the world bounds, reducing memory usage by ~200x.
pub struct CoordinateBitmap {
    bits: Vec<u8>,
    min_x: i32,
    min_z: i32,
    width: usize,
    #[allow(dead_code)]
    height: usize,
    count: usize,
}

impl CoordinateBitmap {
    pub fn new(xzbbox: &XZBBox) -> Self {
        let min_x = xzbbox.min_x();
        let min_z = xzbbox.min_z();
        
        let width = (i64::from(xzbbox.max_x()) - i64::from(min_x) + 1) as usize;
        let height = (i64::from(xzbbox.max_z()) - i64::from(min_z) + 1) as usize;

        let total_bits = width
            .checked_mul(height)
            .expect("CoordinateBitmap: world size too large (width * height overflowed)");
        let num_bytes = total_bits.div_ceil(8);

        Self {
            bits: vec![0u8; num_bytes],
            min_x,
            min_z,
            width,
            height,
            count: 0,
        }
    }

    #[inline]
    fn coord_to_index(&self, x: i32, z: i32) -> Option<usize> {
        let local_x = i64::from(x) - i64::from(self.min_x);
        let local_z = i64::from(z) - i64::from(self.min_z);

        if local_x < 0 || local_z < 0 {
            return None;
        }

        let local_x = local_x as usize;
        let local_z = local_z as usize;

        if local_x >= self.width || local_z >= self.height {
            return None;
        }

        Some(local_z * self.width + local_x)
    }

    #[inline]
    pub fn set(&mut self, x: i32, z: i32) {
        if let Some(bit_index) = self.coord_to_index(x, z) {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;

            let mask = 1u8 << bit_offset;
            if self.bits[byte_index] & mask == 0 {
                self.bits[byte_index] |= mask;
                self.count += 1;
            }
        }
    }

    #[inline]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        if let Some(bit_index) = self.coord_to_index(x, z) {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            return (self.bits[byte_index] >> bit_offset) & 1 == 1;
        }
        false
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[inline]
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.count
    }

    #[inline]
    #[allow(dead_code)]
    pub fn count_contained<'a, I>(&self, coords: I) -> usize
    where
        I: Iterator<Item = &'a (i32, i32)>,
    {
        coords.filter(|(x, z)| self.contains(*x, *z)).count()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn count_in_range(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> (usize, usize) {
        let mut urban_count = 0usize;
        let mut total_count = 0usize;

        for z in min_z..=max_z {
            let local_z = i64::from(z) - i64::from(self.min_z);
            if local_z < 0 || local_z >= self.height as i64 {
                total_count += (i64::from(max_x) - i64::from(min_x) + 1) as usize;
                continue;
            }
            let local_z = local_z as usize;

            let local_min_x = (i64::from(min_x) - i64::from(self.min_x)).max(0) as usize;
            let local_max_x =
                ((i64::from(max_x) - i64::from(self.min_x)) as usize).min(self.width - 1);

            let x_start_offset = (i64::from(self.min_x) - i64::from(min_x)).max(0) as usize;
            let x_end_offset = (i64::from(max_x) - i64::from(self.min_x) - (self.width as i64 - 1))
                .max(0) as usize;
            total_count += x_start_offset + x_end_offset;

            if local_min_x > local_max_x {
                continue;
            }

            let row_start_bit = local_z * self.width + local_min_x;
            let row_end_bit = local_z * self.width + local_max_x;
            let num_bits = row_end_bit - row_start_bit + 1;
            total_count += num_bits;

            let start_byte = row_start_bit / 8;
            let end_byte = row_end_bit / 8;
            let start_bit_in_byte = row_start_bit % 8;
            let end_bit_in_byte = row_end_bit % 8;

            if start_byte == end_byte {
                let byte = self.bits[start_byte];
                let num_bits_in_mask = end_bit_in_byte - start_bit_in_byte + 1;
                let mask = if num_bits_in_mask >= 8 {
                    0xFFu8
                } else {
                    ((1u16 << num_bits_in_mask) - 1) as u8
                };
                let masked = (byte >> start_bit_in_byte) & mask;
                urban_count += masked.count_ones() as usize;
            } else {
                let first_byte = self.bits[start_byte];
                let first_mask = !((1u8 << start_bit_in_byte) - 1);
                urban_count += (first_byte & first_mask).count_ones() as usize;

                for byte_idx in (start_byte + 1)..end_byte {
                    urban_count += self.bits[byte_idx].count_ones() as usize;
                }

                let last_byte = self.bits[end_byte];
                let last_mask = if end_bit_in_byte >= 7 {
                    0xFFu8
                } else {
                    (1u8 << (end_bit_in_byte + 1)) - 1
                };
                urban_count += (last_byte & last_mask).count_ones() as usize;
            }
        }

        (urban_count, total_count)
    }
}

pub type BuildingFootprintBitmap = CoordinateBitmap;

/// Cache Dinâmico de Floodfill (Scanline Context)
pub struct FloodFillCache {
    // Agora modificado via Mutex de escopo interno, mas manteremos simple &mut onde for chamado
    // O lifetime dessa struct dita a existência do cache.
    way_cache: FnvHashMap<u64, Vec<(i32, i32)>>,
}

impl FloodFillCache {
    pub fn new() -> Self {
        Self {
            way_cache: FnvHashMap::default(),
        }
    }

    /// ?? BESM-6 TWEAK: Limpeza Absoluta do Cache (Evita OOM)
    /// Invocado pelo data_processing.rs na virada de Região MCA.
    pub fn clear_cache(&mut self) {
        self.way_cache.clear();
        self.way_cache.shrink_to_fit();
    }

    /// Executa o Floodfill on-demand (se não estiver no cache) e armazena o resultado.
    /// Como o código legado espera `&self` em alguns pontos, exigiremos `&mut` para gravar na RAM,
    /// ou usaremos a abordagem estrita de "calcula na hora e devolve".
    /// 
    /// Por compatibilidade com a assinatura imutável do Arnis, o cache em RAM é mutado
    /// internamente apenas quando injetado diretamente (ex: collect_building_footprints), 
    /// caso contrário, calcula e cospe (garantindo 0 vazamento de memória).
    pub fn get_or_compute(
        &self,
        way: &ProcessedWay,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        // Se já foi pré-calculado na rotina de Footprint, devolve O(1).
        if let Some(cached) = self.way_cache.get(&way.id) {
            return cached.clone();
        } 
        
        // Fallback Dinâmico (Scanline Voxelization Local)
        let polygon_coords: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
        flood_fill_area(&polygon_coords, timeout)
    }

    /// Gets cached flood fill result for a ProcessedElement (Way only).
    pub fn get_or_compute_element(
        &self,
        element: &ProcessedElement,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        match element {
            ProcessedElement::Way(way) => self.get_or_compute(way, timeout),
            _ => Vec::new(),
        }
    }

    /// Coleta footprints para O(1) Block Check e popula o cache temporário na RAM.
    /// Isso permite que o FloodFillCache retenha prédios massivos sem precisar
    /// recalculá-los quando a geometria passar.
    pub fn collect_building_footprints(
        &mut self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
    ) -> BuildingFootprintBitmap {
        let mut footprints = BuildingFootprintBitmap::new(xzbbox);

        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                        let polygon_coords: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                        let filled = flood_fill_area(&polygon_coords, None);
                        for &(x, z) in &filled {
                            footprints.set(x, z);
                        }
                        self.way_cache.insert(way.id, filled);
                    }
                }
                ProcessedElement::Relation(rel) => {
                    let is_building = rel.tags.contains_key("building")
                        || rel.tags.contains_key("building:part")
                        || rel.tags.get("type").map(|t| t.as_str()) == Some("building");
                    if is_building {
                        for member in &rel.members {
                            if member.role == ProcessedMemberRole::Outer {
                                let polygon_coords: Vec<(i32, i32)> = member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
                                let filled = flood_fill_area(&polygon_coords, None);
                                for &(x, z) in &filled {
                                    footprints.set(x, z);
                                }
                                self.way_cache.insert(member.way.id, filled);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        footprints
    }

    pub fn collect_building_centroids(&self, elements: &[ProcessedElement]) -> Vec<(i32, i32)> {
        let mut centroids = Vec::new();

        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                        if let Some(cached) = self.way_cache.get(&way.id) {
                            if let Some(centroid) = Self::compute_centroid(cached) {
                                centroids.push(centroid);
                            }
                        }
                    }
                }
                ProcessedElement::Relation(rel) => {
                    let is_building = rel.tags.contains_key("building")
                        || rel.tags.contains_key("building:part")
                        || rel.tags.get("type").map(|t| t.as_str()) == Some("building");
                    if is_building {
                        let mut all_coords = Vec::new();
                        for member in &rel.members {
                            if member.role == ProcessedMemberRole::Outer {
                                if let Some(cached) = self.way_cache.get(&member.way.id) {
                                    all_coords.extend(cached.iter().copied());
                                }
                            }
                        }
                        if let Some(centroid) = Self::compute_centroid(&all_coords) {
                            centroids.push(centroid);
                        }
                    }
                }
                _ => {}
            }
        }
        centroids
    }

    fn compute_centroid(coords: &[(i32, i32)]) -> Option<(i32, i32)> {
        if coords.is_empty() {
            return None;
        }
        let sum_x: i64 = coords.iter().map(|(x, _)| i64::from(*x)).sum();
        let sum_z: i64 = coords.iter().map(|(_, z)| i64::from(*z)).sum();
        let len = coords.len() as i64;
        Some(((sum_x / len) as i32, (sum_z / len) as i32))
    }

    pub fn remove_way(&mut self, way_id: u64) {
        self.way_cache.remove(&way_id);
    }

    pub fn remove_relation_ways(&mut self, way_ids: &[u64]) {
        for &id in way_ids {
            self.way_cache.remove(&id);
        }
    }
}

impl Default for FloodFillCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Configures the global Rayon thread pool with a CPU usage cap.
pub fn configure_rayon_thread_pool(cpu_fraction: f64) {
    let cpu_fraction = cpu_fraction.clamp(0.1, 1.0);

    let available_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let target_threads = ((available_cores as f64) * cpu_fraction).floor() as usize;
    let target_threads = target_threads.max(1);

    match rayon::ThreadPoolBuilder::new()
        .num_threads(target_threads)
        .build_global()
    {
        Ok(()) => {}
        Err(_) => {}
    }
}