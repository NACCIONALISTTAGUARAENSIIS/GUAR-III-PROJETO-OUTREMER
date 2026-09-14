//! Deterministic random number generation for consistent element processing (BESM-6).
//!
//! Este módulo provê um RNG cravado que garante que o mesmo elemento, originário de qualquer
//! provedor governamental (WFS, GDF, OSM), produza os mesmos valores aleatórios sempre.
//! Foi reescrito para utilizar uma função de Hash Pseudo-SplitMix64 (Efeito Avalanche real)
//! e o motor Xoshiro256++ (PRNG não-criptográfico de altíssimo desempenho), mitigando
//! colisões de bits e eliminando o gargalo do Ponto Zero (Zero Collapse).

// 🚨 Apenas o estritamente necessário. Sem implementações falsas de traits.
use rand::{RngCore, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;

// Constantes Mágicas de Propagação (Derivadas da Razão Áurea e Primos longos)
const GOLDEN_RATIO_64: u64 = 0x9E3779B97F4A7C15;
const MIX_CONST_1: u64 = 0xBF58476D1CE4E5B9;
const MIX_CONST_2: u64 = 0x94D049BB133111EB;

// ============================================================================
// 🚨 BESM-6 NEWTYPE PATTERN (O Isolador de Dependências)
// Pub struct simples e direta.
// A trait Rng (que provê gen_bool, gen_range, etc.) é implementada
// automaticamente pelo Rust em qualquer struct que implemente RngCore.
// Logo, quem importar PincelRng só precisa fazer `use rand::Rng;` localmente.
// ============================================================================

pub struct PincelRng(pub Xoshiro256PlusPlus);

impl RngCore for PincelRng {
    #[inline(always)]
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    #[inline(always)]
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    #[inline(always)]
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest)
    }

    #[inline(always)]
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.try_fill_bytes(dest)
    }
}

// ============================================================================
// LÓGICA DE HASH E SEMENTES
// ============================================================================

/// 🚨 BESM-6: Função de Avalanche Real (Inspirada no SplitMix64).
/// Ela recebe um state, soma a constante da razão áurea (garantindo que
/// a entrada 0 NUNCA resulte em 0) e depois mistura vigorosamente.
#[inline(always)]
fn avalanche_hash(mut x: u64) -> u64 {
    // A Injeção de Entropia Basal (O colapso do ponto zero é matematicamente impossível aqui)
    x = x.wrapping_add(GOLDEN_RATIO_64);
    x = (x ^ (x >> 30)).wrapping_mul(MIX_CONST_1);
    x = (x ^ (x >> 27)).wrapping_mul(MIX_CONST_2);
    x ^ (x >> 31)
}

/// Construtor de Matriz 256-bits O(1) SEGURO
#[inline(always)]
fn build_256bit_seed(h1: u64, h2: u64, h3: u64, h4: u64) -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[0..8].copy_from_slice(&h1.to_le_bytes());
    seed[8..16].copy_from_slice(&h2.to_le_bytes());
    seed[16..24].copy_from_slice(&h3.to_le_bytes());
    seed[24..32].copy_from_slice(&h4.to_le_bytes());
    seed
}

/// Creates a deterministic RNG seeded from an element ID.
#[inline]
pub fn element_rng(element_id: u64) -> PincelRng {
    let h1 = avalanche_hash(element_id);
    let h2 = avalanche_hash(h1);
    let h3 = avalanche_hash(h2);
    let h4 = avalanche_hash(h3);

    PincelRng(Xoshiro256PlusPlus::from_seed(build_256bit_seed(h1, h2, h3, h4)))
}

/// Creates a deterministic RNG seeded from an element ID with an additional salt.
#[inline]
#[allow(dead_code)]
pub fn element_rng_salted(element_id: u64, salt: u64) -> PincelRng {
    // Mistura o ID com o Salt usando um bitwise XOR forte
    let initial_state = element_id ^ salt.rotate_left(32);
    let h1 = avalanche_hash(initial_state);
    let h2 = avalanche_hash(h1);
    let h3 = avalanche_hash(h2);
    let h4 = avalanche_hash(h3);

    PincelRng(Xoshiro256PlusPlus::from_seed(build_256bit_seed(h1, h2, h3, h4)))
}

/// Creates a deterministic RNG seeded from 3D coordinates.
///
/// 🚨 BESM-6 TWEAK: Rotação de Bits para Não-Comutatividade Espacial.
/// Garante que a coordenada (10, 0, 20) seja matematicamente distinta de (20, 0, 10).
#[inline]
pub fn coord_rng(x: i32, y: i32, z: i32, element_id: u64) -> PincelRng {
    // Conversão segura de complemento de dois
    let ux = x as u32 as u64;
    let uy = y as u32 as u64;
    let uz = z as u32 as u64;

    // XOR assimétrico com rotações estritas para destruir a comutatividade e simetria dos eixos
    let spatial_mix = ux
        ^ uy.rotate_left(21)
        ^ uz.rotate_left(42);

    let initial_state = spatial_mix ^ element_id;

    let h1 = avalanche_hash(initial_state);
    let h2 = avalanche_hash(h1);
    let h3 = avalanche_hash(h2);
    let h4 = avalanche_hash(h3);

    PincelRng(Xoshiro256PlusPlus::from_seed(build_256bit_seed(h1, h2, h3, h4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng; // Importa o trait nativo do Rand para testar o gerador

    #[test]
    fn test_element_rng_deterministic() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng(12345);

        for _ in 0..100 {
            assert_eq!(rng1.gen::<u64>(), rng2.gen::<u64>());
        }
    }

    #[test]
    fn test_different_elements_different_values() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng(12346);

        let v1: u64 = rng1.gen();
        let v2: u64 = rng2.gen();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_salted_rng_different_from_base() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng_salted(12345, 1);

        let v1: u64 = rng1.gen();
        let v2: u64 = rng2.gen();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_coord_rng_deterministic() {
        let mut rng1 = coord_rng(100, 64, 200, 12345);
        let mut rng2 = coord_rng(100, 64, 200, 12345);

        assert_eq!(rng1.gen::<u64>(), rng2.gen::<u64>());
    }

    #[test]
    fn test_coord_rng_negative_coordinates() {
        let mut rng1 = coord_rng(-100, 10, -200, 12345);
        let mut rng2 = coord_rng(-100, 10, -200, 12345);

        assert_eq!(rng1.gen::<u64>(), rng2.gen::<u64>());

        let mut rng3 = coord_rng(-100, 10, -200, 12345);
        let mut rng4 = coord_rng(-101, 10, -200, 12345);

        assert_ne!(rng3.gen::<u64>(), rng4.gen::<u64>());
    }

    #[test]
    fn test_coord_rng_zero_collapse_prevention() {
        // 🚨 O Teste Mestre: Coord (0,0,0) com ID 0.
        // O gerador agora deve cuspir entropia caótica verdadeira em vez de colapsar.
        let mut rng_zero = coord_rng(0, 0, 0, 0);
        let v1: u64 = rng_zero.gen();
        let v2: u64 = rng_zero.gen();

        assert_ne!(v1, 0);
        assert_ne!(v2, 0);
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_coord_rng_non_commutative() {
        // Prova que (x, y, z) invertido não colide
        let mut rng1 = coord_rng(10, 0, 20, 999);
        let mut rng2 = coord_rng(20, 0, 10, 999);

        assert_ne!(rng1.gen::<u64>(), rng2.gen::<u64>());
    }
}