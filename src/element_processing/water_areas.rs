#![allow(unused)]

use noise::{NoiseFn, OpenSimplex};
use once_cell::sync::Lazy;
use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;
use std::f64::consts::PI;

// =====================================================
// ESCALAS FIXAS (Frequência do Ruído)
// =====================================================
const HORIZONTAL_SCALE: f64 = 1.0 / 33.0;
const VERTICAL_SCALE: f64 = 1.0 / 15.0;

// =====================================================
// RUÍDO GLOBAL (LAZY - PERFORMANCE)
// =====================================================
static NOISE_TERRAIN: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(4242));
static NOISE_DENSITY: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(8888));
static NOISE_SPECIES: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(1919));
static NOISE_JITTER: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(7777));
static NOISE_BARK: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(3131));

// =====================================================
// ESTRUTURA DE BLOCO SIMPLIFICADA
// =====================================================
#[derive(Clone, Copy, PartialEq)]
pub enum Block {
    Air,
    Log,      // Madeira (Acácia para Cerrado)
    Leaf,     // Folha comum (Copa rala)
    LeafDense,// Folha fechada (Vereda/Mata de Galeria)
    Shrub,    // Arbusto
    Grass,    // Capim dourado/seco
    Litter,   // Serapilheira (folhas secas no chão)
    BurnMark, // Tronco carbonizado
    DeadBush, // Arbusto seco
}

// =====================================================
// FUNÇÃO PRINCIPAL DE GERAÇÃO POR CHUNK
// =====================================================
pub fn generate_chunk(
    chunk_x: i32,
    chunk_z: i32,
    get_ground_level: &dyn Fn(i32, i32) -> i32, // TWEAK: Importamos o leitor de terreno real
    world: &mut dyn FnMut(i32, i32, i32, Block),
) {
    // -------------------------------------------------
    // LOOP 16x16 (Rigor de Performance Rust)
    // -------------------------------------------------
    for lx in 0..16 {
        for lz in 0..16 {
            let wx = chunk_x * 16 + lx;
            let wz = chunk_z * 16 + lz;

            let sx = wx as f64 * HORIZONTAL_SCALE;
            let sz = wz as f64 * HORIZONTAL_SCALE;

            // =====================================================
            // TOPOGRAFIA + UMIDADE (Microclima)
            // =====================================================
            let topo = NOISE_TERRAIN.get([sx * 0.8, sz * 0.8]);
            let moisture = ((-topo + 0.5).clamp(0.0, 1.0));

            // TWEAK DE INTEGRAÇÃO MAIOR: Altura base agora é o SRTM real do Minecraft.
            // As árvores não vão mais flutuar ou nascer soterradas.
            let base_height = get_ground_level(wx, wz);

            // Se o terreno for muito baixo (água/lago), aborta a árvore
            if base_height < 62 {
                continue;
            }

            // =====================================================
            // DENSIDADE (Cerrado Stricto Sensu vs Campo Sujo)
            // =====================================================
            let density = NOISE_DENSITY.get([sx * 0.6, sz * 0.6]);

            // TWEAK DF: Espaçamento muito maior. O threshold é rígido.
            // Mais umidade (vales) = threshold menor (mais denso). Seca = threshold alto (esparso).
            let spawn_threshold = 0.45 - (moisture * 0.5);

            // =====================================================
            // RNG LOCAL DETERMINÍSTICO
            // =====================================================
            let seed = ((wx as i64) << 32) ^ (wz as i64);
            let mut rng = SmallRng::seed_from_u64(seed as u64);

            if density < spawn_threshold {
                // Se não nasceu árvore, gera a savana/campo sujo
                generate_undergrowth(wx, base_height, wz, moisture, &mut rng, world);
                continue;
            }

            // =====================================================
            // VARIAÇÃO ESPACIAL (JITTER)
            // =====================================================
            let jitter_x = NOISE_JITTER.get([sx * 3.0, sz * 3.0]) * 1.5;
            let jitter_z = NOISE_JITTER.get([sz * 3.0, sx * 3.0]) * 1.5;

            let tx = wx + jitter_x as i32;
            let tz = wz + jitter_z as i32;

            // Recalcula a altura real para o ponto exato da árvore (pós-jitter)
            let actual_base_height = get_ground_level(tx, tz);

            // =====================================================
            // ESCOLHA DE ESPÉCIE (Rigor Botânico do DF)
            // =====================================================
            let (trunk_base, is_twisted, leaf_type) = match moisture {
                m if m > 0.7 => (12, false, Block::LeafDense), // Vereda/Mata de Galeria (Alta, densa)
                m if m > 0.4 => (7, true, Block::Leaf),        // Cerrado Típico (Pau-Terra, torto)
                _ => (5, true, Block::Leaf),                   // Seca severa (Pequizeiro, muito torto)
            };

            let trunk_height = trunk_base + rng.gen_range(0..4);

            // =====================================================
            // CONSTRUÇÃO DO TRONCO COM TORTUOSIDADE
            // =====================================================
            let mut current_x = tx;
            let mut current_z = tz;

            for y in 0..trunk_height {
                let wy = actual_base_height + y;

                // Efeito de Tronco Retorcido (Cerrado Clássico)
                if is_twisted && y > 0 && rng.gen_bool(0.35) {
                    current_x += rng.gen_range(-1..=1);
                    current_z += rng.gen_range(-1..=1);
                }

                // CICATRIZES DE FOGO (Bark Noise)
                let bark = NOISE_BARK.get([
                    current_x as f64 * 0.4,
                    wy as f64 * 0.8,
                    current_z as f64 * 0.4
                ]);

                // Casca grossa resistente ao fogo
                let wood_type = if bark > 0.45 && y < 4 { Block::BurnMark } else { Block::Log };
                world(current_x, wy, current_z, wood_type);

                // BIFURCAÇÃO (Geralmente baixa no Cerrado)
                if is_twisted && y == (trunk_height / 2) {
                    generate_branch(current_x, wy, current_z, &mut rng, world);
                }
            }

            // =====================================================
            // COPA (Canopy)
            // =====================================================
            // TWEAK: Copas horizontais e assimétricas (Típico do Cerrado)
            let canopy_radius_x = 2 + rng.gen_range(0..=2) + (moisture * 1.5) as i32;
            let canopy_radius_z = 2 + rng.gen_range(0..=2) + (moisture * 1.5) as i32;
            let canopy_y = actual_base_height + trunk_height;

            for dx in -canopy_radius_x..=canopy_radius_x {
                for dz in -canopy_radius_z..=canopy_radius_z {
                    for dy in -1..=1 { // Copa muito achatada (pouca altura)

                        // Elipse achatada para a forma da copa
                        let dist = ((dx*dx) as f64 / (canopy_radius_x*canopy_radius_x) as f64) +
                            ((dz*dz) as f64 / (canopy_radius_z*canopy_radius_z) as f64) +
                            ((dy*dy * 3) as f64); // Peso vertical alto para amassar a copa

                        if dist < 1.0 {
                            // Esparsidade: a copa do cerrado deixa a luz passar
                            if leaf_type == Block::Leaf && rng.gen_bool(0.15) { continue; }
                            world(current_x + dx, canopy_y + dy, current_z + dz, leaf_type);
                        }
                    }
                }
            }
        }
    }
}

// =====================================================
// FUNÇÕES AUXILIARES DE SUPORTE (RIGOR TÉCNICO)
// =====================================================

/// Gera galhos laterais para árvores tortas
fn generate_branch(x: i32, y: i32, z: i32, rng: &mut SmallRng, world: &mut dyn FnMut(i32, i32, i32, Block)) {
    let dx = rng.gen_range(-2..=2);
    let dz = rng.gen_range(-2..=2);
    for i in 1..=3 {
        world(x + (dx * i / 3), y + i, z + (dz * i / 3), Block::Log);
    }
}

/// Gera a cobertura de solo (Undergrowth) do Cerrado
fn generate_undergrowth(x: i32, y: i32, z: i32, moisture: f64, rng: &mut SmallRng, world: &mut dyn FnMut(i32, i32, i32, Block)) {
    // Correção Ecológica: Apenas áreas secas recebem Dead Bush. Áreas úmidas recebem grama verde/arbustos.
    if moisture < 0.4 {
        // Campo Sujo (Seca): Muito Capim, Serapilheira e Arbustos Secos
        if rng.gen_bool(0.40) { world(x, y, z, Block::Grass); }
        if rng.gen_bool(0.08) { world(x, y, z, Block::Litter); }
        if rng.gen_bool(0.05) { world(x, y, z, Block::DeadBush); }
    } else {
        // Mata de Galeria/Vereda (Úmido): Arbustos vivos, pouca grama solta (sombreado)
        if rng.gen_bool(0.15) { world(x, y, z, Block::Shrub); }
        if rng.gen_bool(0.20) { world(x, y, z, Block::Grass); }
    }
}