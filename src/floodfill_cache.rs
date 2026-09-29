//! Flood fill cache for polygon filling (Scanline Out-Of-Core Engine).
//!
//! 🚨 BESM-6 TWEAK: Pre-computation has been eradicated to prevent OOM.
//! The cache is now populated lazily on-demand per region and purged periodically
//! by the Scanline engine.
//! Integração total com o ComplexPolygon para suporte a furos e ilhas nativos O(1).

use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill::{
    extract_complex_polygon_from_element, flood_fill_area, scanline_fill_complex, ComplexPolygon,
};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
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
    // Guarda o preenchimento mapeado pelo ID do Elemento (seja Way ou Relation)
    way_cache: FnvHashMap<u64, Vec<(i32, i32)>>,
}

impl FloodFillCache {
    pub fn new() -> Self {
        Self {
            way_cache: FnvHashMap::default(),
        }
    }

    /// 🚨 BESM-6 TWEAK: Limpeza Absoluta do Cache (Evita OOM)
    /// Invocado pelo data_processing.rs na virada de Região MCA.
    pub fn clear_cache(&mut self) {
        self.way_cache.clear();
        self.way_cache.shrink_to_fit();
    }

    /// Executa o Floodfill on-demand para uma via simples.
    pub fn get_or_compute(
        &self,
        way: &ProcessedWay,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        if let Some(cached) = self.way_cache.get(&way.id) {
            return cached.clone();
        }

        let polygon_coords: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
        flood_fill_area(&polygon_coords, timeout)
    }

    /// 🚨 BESM-6 TWEAK: Delegação Geométrica Complexa.
    /// Para Elementos Genéricos (incluindo Relações com Furos).
    pub fn get_or_compute_element(
        &self,
        element: &ProcessedElement,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        let id = match element {
            ProcessedElement::Way(w) => w.id,
            ProcessedElement::Relation(r) => r.id,
            _ => return Vec::new(),
        };

        if let Some(cached) = self.way_cache.get(&id) {
            return cached.clone();
        }

        if let Some(complex_poly) = extract_complex_polygon_from_element(element) {
            scanline_fill_complex(&complex_poly, timeout)
        } else {
            Vec::new()
        }
    }

    /// Coleta footprints para O(1) Block Check e popula o cache temporário na RAM.
    /// Agora respeita nativamente os anéis interiores de pátios.
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
                        let polygon_coords: Vec<(i32, i32)> =
                            way.nodes.iter().map(|n| (n.x, n.z)).collect();
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
                        || rel.tags.get("type").map(|t: &String| t.as_str()) == Some("building");
                    if is_building {
                        // 🚨 BESM-6 Tweak: Lida com a Relação inteira, subtraindo furos perfeitamente
                        if let Some(complex_poly) = extract_complex_polygon_from_element(element) {
                            let filled = scanline_fill_complex(&complex_poly, None);
                            for &(x, z) in &filled {
                                footprints.set(x, z);
                            }
                            // Agora mapeamos a Relação pela sua PRÓPRIA ID global
                            self.way_cache.insert(rel.id, filled);
                        }
                    }
                }
                _ => {}
            }
        }
        footprints
    }

    /// Bitmap de TODA área mapeada (uso do solo, lazer, amenidade, natureza,
    /// água, prédios, pátios, aeródromos...) — tudo o que já tem um gerador
    /// próprio decidindo o que existe naquele chão.
    ///
    /// Consumido por `tree::generate_chunk` (floresta ambiente do Cerrado): a
    /// mata procedural só nasce onde NENHUM dado diz o que há. Antes, ela
    /// brotava em qualquer bloco cuja superfície não fosse asfalto/concreto —
    /// ou seja, dentro de quadras residenciais, quintais, estacionamentos de
    /// terra, campos de futebol, cemitérios e pátios industriais inteiros.
    ///
    /// Rasteriza direto no bitmap, recortado ao bbox (`rasterize_polygon_into`):
    /// polígonos enormes (um lago mantido sem recorte, uma mata de 100 km²)
    /// não alocam um vetor de pontos do tamanho da área deles.
    pub fn collect_mapped_area_coverage(
        &self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
    ) -> CoordinateBitmap {
        let mut coverage = CoordinateBitmap::new(xzbbox);

        for element in elements {
            if !is_mapped_area(element) {
                continue;
            }
            if let Some(complex_poly) = extract_complex_polygon_from_element(element) {
                rasterize_polygon_into(&mut coverage, &complex_poly, xzbbox);
            }
        }
        coverage
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
                        || rel.tags.get("type").map(|t: &String| t.as_str()) == Some("building");
                    if is_building {
                        // 🚨 Como o footprint já guardou a Relação pela ID dela, o lookup é O(1) e imune a erros.
                        if let Some(cached) = self.way_cache.get(&rel.id) {
                            if let Some(centroid) = Self::compute_centroid(cached) {
                                centroids.push(centroid);
                            }
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

    /// Invalida uma entrada única do cache de footprints por way. Nenhum
    /// chamador hoje — `suppressed_building_outlines` (que exclui ways do
    /// dispatch genérico) filtra no ponto de uso em vez de invalidar o cache
    /// que já foi populado; ver tarefa de acompanhamento se isso se mostrar
    /// necessário para evitar footprints obsoletos entre regiões.
    #[allow(dead_code)]
    pub fn remove_way(&mut self, way_id: u64) {
        self.way_cache.remove(&way_id);
    }

    /// Invalida múltiplas entradas de uma vez (ver `remove_way`).
    #[allow(dead_code)]
    pub fn remove_relation_ways(&mut self, way_ids: &[u64]) {
        for &id in way_ids {
            self.way_cache.remove(&id);
        }
    }
}

/// Rasteriza um polígono (com furos, regra par-ímpar) diretamente num
/// `CoordinateBitmap`, linha a linha, recortando cada varredura ao `xzbbox`.
/// Custo O(linhas × arestas), memória zero além do próprio bitmap — ao
/// contrário de `scanline_fill_complex`, que materializa `Vec<(x, z)>` da área
/// inteira (proibitivo para polígonos que se estendem muito além do bbox).
///
/// Convenção: o bloco `(x, z)` é coberto se o seu centro `(x + 0.5, z + 0.5)`
/// está dentro do polígono.
pub fn rasterize_polygon_into(
    bitmap: &mut CoordinateBitmap,
    polygon: &ComplexPolygon,
    xzbbox: &XZBBox,
) {
    let mut edges: Vec<((i32, i32), (i32, i32))> = Vec::new();
    let mut push_ring = |ring: &[(i32, i32)]| {
        if ring.len() < 3 {
            return;
        }
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            if a.1 != b.1 {
                edges.push((a, b));
            }
        }
    };
    push_ring(&polygon.outer);
    for inner in &polygon.inners {
        push_ring(inner);
    }
    if edges.is_empty() {
        return;
    }

    let poly_min_z = edges.iter().map(|(a, b)| a.1.min(b.1)).min().unwrap();
    let poly_max_z = edges.iter().map(|(a, b)| a.1.max(b.1)).max().unwrap();
    let z_start = poly_min_z.max(xzbbox.min_z());
    let z_end = poly_max_z.min(xzbbox.max_z());
    if z_start > z_end {
        return;
    }

    let mut crossings: Vec<f64> = Vec::new();
    for z in z_start..=z_end {
        let zc = z as f64 + 0.5;
        crossings.clear();
        for &((x1, z1), (x2, z2)) in &edges {
            let (z1f, z2f) = (z1 as f64, z2 as f64);
            if (z1f <= zc) != (z2f <= zc) {
                let t = (zc - z1f) / (z2f - z1f);
                crossings.push(x1 as f64 + t * (x2 as f64 - x1 as f64));
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in crossings.chunks_exact(2) {
            let x_from = (pair[0] - 0.5).ceil() as i64;
            let x_to = (pair[1] - 0.5).floor() as i64;
            let x_from = x_from.max(xzbbox.min_x() as i64);
            let x_to = x_to.min(xzbbox.max_x() as i64);
            for x in x_from..=x_to {
                bitmap.set(x as i32, z);
            }
        }
    }
}

/// Chaves de tag cuja presença numa geometria FECHADA significa "este chão já
/// está descrito por um dado real" — a floresta ambiente não entra aqui.
const MAPPED_AREA_KEYS: &[&str] = &[
    "building",
    "building:part",
    "landuse",
    "leisure",
    "amenity",
    "natural",
    "water",
    "waterway",
    "aeroway",
    "man_made",
    "power",
    "military",
    "tourism",
    "shop",
    "historic",
    "parking",
    "public_transport",
    "area:highway",
];

/// Verdadeiro para polígonos fechados (ou relações multipolígono) que carregam
/// uma tag de área conhecida — ver `MAPPED_AREA_KEYS`. Vias abertas (ruas,
/// rios, cercas) não contam: são linhas, não chão.
pub fn is_mapped_area(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    let has_area_tag = MAPPED_AREA_KEYS.iter().any(|k| tags.contains_key(*k))
        || (tags.contains_key("highway") && tags.get("area").map(|v| v.as_str()) == Some("yes"));
    if !has_area_tag {
        return false;
    }
    match element {
        ProcessedElement::Way(w) => {
            w.nodes.len() >= 4
                && w.nodes.first().map(|n| (n.x, n.z)) == w.nodes.last().map(|n| (n.x, n.z))
        }
        ProcessedElement::Relation(r) => !r.members.is_empty(),
        ProcessedElement::Node(_) => false,
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

    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(target_threads)
        .build_global();
}

#[cfg(test)]
mod mapped_area_tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn way(id: u64, tags: &[(&str, &str)], pts: &[(i32, i32)]) -> ProcessedElement {
        ProcessedElement::Way(Arc::new(ProcessedWay {
            id,
            nodes: pts
                .iter()
                .map(|&(x, z)| ProcessedNode {
                    id: 0,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }))
    }

    const SQUARE: &[(i32, i32)] = &[(10, 10), (30, 10), (30, 30), (10, 30), (10, 10)];

    #[test]
    fn residential_landuse_polygon_is_mapped_area_and_covers_interior() {
        let xzbbox = XZBBox::new(0, 63, 0, 63);
        let cache = FloodFillCache::new();
        let elements = vec![way(1, &[("landuse", "residential")], SQUARE)];
        assert!(is_mapped_area(&elements[0]));
        let coverage = cache.collect_mapped_area_coverage(&elements, &xzbbox);
        assert!(coverage.contains(20, 20));
        assert!(coverage.contains(10, 10));
        assert!(!coverage.contains(40, 40));
        assert!(!coverage.contains(5, 20));
    }

    #[test]
    fn open_highway_is_not_a_mapped_area_but_highway_area_is() {
        let road = way(2, &[("highway", "residential")], &[(0, 0), (50, 0)]);
        assert!(!is_mapped_area(&road));
        let plaza = way(3, &[("highway", "pedestrian"), ("area", "yes")], SQUARE);
        assert!(is_mapped_area(&plaza));
    }

    #[test]
    fn unclosed_polygon_with_area_tag_is_ignored() {
        let open = way(4, &[("natural", "wood")], &[(10, 10), (30, 10), (30, 30)]);
        assert!(!is_mapped_area(&open));
        let closed = way(5, &[("natural", "wood")], SQUARE);
        assert!(is_mapped_area(&closed));
    }

    #[test]
    fn rasterizer_handles_holes_and_clips_to_bbox() {
        let xzbbox = XZBBox::new(0, 31, 0, 31);
        let mut bitmap = CoordinateBitmap::new(&xzbbox);
        // Quadrado maior que o bbox, com um furo interno: nada aloca, tudo recorta.
        let poly = ComplexPolygon {
            outer: vec![(-100, -100), (100, -100), (100, 100), (-100, 100)],
            inners: vec![vec![(10, 10), (20, 10), (20, 20), (10, 20)]],
        };
        rasterize_polygon_into(&mut bitmap, &poly, &xzbbox);
        assert!(bitmap.contains(0, 0));
        assert!(bitmap.contains(31, 31));
        assert!(bitmap.contains(5, 15));
        assert!(
            !bitmap.contains(15, 15),
            "furo interno deveria ficar descoberto"
        );
        assert!(!bitmap.contains(40, 40), "fora do bbox nunca é marcado");
        // 32×32 = 1024 blocos menos o furo 10×10 = 924.
        assert_eq!(bitmap.count(), 1024 - 100);
    }

    #[test]
    fn rasterizer_matches_scanline_fill_for_a_triangle() {
        let xzbbox = XZBBox::new(-50, 50, -50, 50);
        let mut bitmap = CoordinateBitmap::new(&xzbbox);
        let poly = ComplexPolygon {
            outer: vec![(-30, -20), (25, -10), (0, 30)],
            inners: vec![],
        };
        rasterize_polygon_into(&mut bitmap, &poly, &xzbbox);
        let reference = scanline_fill_complex(&poly, None);
        assert!(!reference.is_empty());
        let mut agree = 0usize;
        for &(x, z) in &reference {
            if bitmap.contains(x, z) {
                agree += 1;
            }
        }
        // Convenções de borda podem divergir num anel de 1 bloco; o interior
        // tem que coincidir (≥ 90% dos pontos de referência cobertos e a
        // contagem total na mesma ordem de grandeza).
        assert!(
            agree * 10 >= reference.len() * 9,
            "{agree}/{}",
            reference.len()
        );
        assert!(bitmap.count() * 10 <= reference.len() * 12);
    }

    #[test]
    fn untagged_or_unknown_tag_polygon_is_not_mapped() {
        let fence = way(6, &[("barrier", "fence")], SQUARE);
        assert!(!is_mapped_area(&fence));
    }
}
