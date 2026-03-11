use geo::orient::{Direction, Orient};
use geo::{Contains, LineString, Point, Polygon};
use itertools::Itertools;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum bounding box area (in blocks) for flood fill.
/// Aumentado para acomodar o Lago Paranoá sem ser cortado (30M blocks)
const MAX_FLOOD_FILL_AREA: i64 = 30_000_000;

/// A compact bitmap for visited-coordinate tracking during flood fill.
struct FloodBitmap {
    bits: Vec<u8>,
    min_x: i32,
    min_z: i32,
    width: usize,
}

impl FloodBitmap {
    #[inline]
    fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1) as usize;
        let height = (max_z - min_z + 1) as usize;
        let num_bytes = (width * height).div_ceil(8);
        Self {
            bits: vec![0u8; num_bytes],
            min_x,
            min_z,
            width,
        }
    }

    #[inline]
    fn insert(&mut self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        let mask = 1u8 << bit;
        if self.bits[byte] & mask != 0 {
            false
        } else {
            self.bits[byte] |= mask;
            true
        }
    }

    #[inline]
    fn contains(&self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        (self.bits[byte] >> bit) & 1 == 1
    }
}

pub fn flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    if polygon_coords.len() < 3 {
        return vec![];
    }

    let (min_x, max_x) = polygon_coords
        .iter()
        .map(|&(x, _)| x)
        .minmax()
        .into_option()
        .unwrap();
    let (min_z, max_z) = polygon_coords
        .iter()
        .map(|&(_, z)| z)
        .minmax()
        .into_option()
        .unwrap();

    let area = (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64;

    if area > MAX_FLOOD_FILL_AREA {
        // Ignora polígonos astronomicamente grandes (proteção contra dados corrompidos do OSM)
        return vec![];
    }

    // Aumentado de 50k para 100k devido à nossa escala Híbrida 1.33
    if area < 100_000 {
        optimized_scanline_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    } else {
        original_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    }
}

/// ?? BESM-6 Tweak: SCANLINE RASTERIZATION ??
/// Substitui o BFS (Busca em Largura) tradicional por um algoritmo de varredura por linha (Scanline).
/// Reduz a sobrecarga de memória (Zero Queue) e preenche polígonos perfeitamente em O(Area).
fn optimized_scanline_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    _min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();
    
    // BESM-6 Tweak: Capacidade Estimada com Limite Superior Seguro (Evita OOM)
    let bbox_area = (max_x - _min_x + 1) as i64 * (max_z - min_z + 1) as i64;
    let cap = (bbox_area / 4).min(5_000_000) as usize;
    let mut filled_area = Vec::with_capacity(cap); 
    
    // Preparar as arestas do polígono para verificação rápida (ignora arestas perfeitamente horizontais)
    let mut edges = Vec::with_capacity(polygon_coords.len());
    let len = polygon_coords.len();
    for i in 0..len {
        let j = (i + 1) % len;
        let (x1, z1) = polygon_coords[i];
        let (x2, z2) = polygon_coords[j];
        
        if z1 != z2 {
            // Guarda sempre com y_min primeiro para facilitar a checagem (Técnica clássica top-vertex)
            if z1 < z2 {
                edges.push(((x1 as f64, z1 as f64), (x2 as f64, z2 as f64)));
            } else {
                edges.push(((x2 as f64, z2 as f64), (x1 as f64, z1 as f64)));
            }
        }
    }

    let mut last_timeout_check = 0;

    // Scanline (Varrer de Cima para Baixo)
    for z in min_z..=max_z {
        
        // Proteção BESM-6: Controle de timeout com menor overhead
        if filled_area.len() - last_timeout_check > 5000 {
            if let Some(timeout) = timeout {
                if start_time.elapsed() > *timeout {
                    return filled_area;
                }
            }
            last_timeout_check = filled_area.len();
        }

        let z_f64 = z as f64;
        
        // BESM-6 Tweak: Vetor limpo e não pré-alocado pequeno para segurar complexos do OSM
        let mut intersections = Vec::new(); 

        // Encontra interseções da linha de varredura (Scanline) com as arestas do polígono
        for ((ex1, ez1), (ex2, ez2)) in &edges {
            // Condição top-vertex: evita contar a mesma quina (vértice) duas vezes se duas arestas se ligam ali
            if z_f64 >= *ez1 && z_f64 < *ez2 { 
                let intersect_x = ex1 + (z_f64 - ez1) / (ez2 - ez1) * (ex2 - ex1);
                intersections.push(intersect_x);
            }
        }

        // Ordena as interseções da esquerda para a direita
        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Regra Even-Odd (Preenche entre os pares de interseções)
        let mut idx = 0;
        while idx + 1 < intersections.len() {
            // BESM-6 Tweak: Conversão grid segura.
            let start_x = intersections[idx].ceil() as i32;
            let end_x = intersections[idx + 1].floor() as i32;

            for x in start_x..=end_x {
                filled_area.push((x, z));
            }
            idx += 2;
        }
    }

    filled_area.shrink_to_fit();
    filled_area
}

fn original_flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();
    let mut filled_area: Vec<(i32, i32)> = Vec::with_capacity(5000);
    let mut visited = FloodBitmap::new(min_x, max_x, min_z, max_z);

    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]).orient(Direction::Default);

    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x: i32 = (width / 8).clamp(1, 24);
    let step_z: i32 = (height / 8).clamp(1, 24);

    let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(4096);

    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            
            // Tweak: Como esta função lida com lagos de 30M, o syscall do relógio deve ser muito raro
            if filled_area.len() % 50_000 == 0 {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        return filled_area;
                    }
                }
            }

            if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                continue;
            }

            queue.clear();
            queue.push_back((x, z));
            visited.insert(x, z);

            while let Some((curr_x, curr_z)) = queue.pop_front() {
                // A GRANDE CORREÇÃO: O bloco já foi verificado antes de entrar na fila. Não testamos novamente aqui.
                filled_area.push((curr_x, curr_z));

                let neighbors = [
                    (curr_x - 1, curr_z),
                    (curr_x + 1, curr_z),
                    (curr_x, curr_z - 1),
                    (curr_x, curr_z + 1),
                ];

                for &(nx, nz) in &neighbors {
                    if nx >= min_x
                        && nx <= max_x
                        && nz >= min_z
                        && nz <= max_z
                        && visited.insert(nx, nz)
                    {
                        // Teste de Contenção blindado ANTES de colocar na fila (Impede vazamento de RAM)
                        if polygon.contains(&Point::new(nx as f64, nz as f64)) {
                            queue.push_back((nx, nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
}