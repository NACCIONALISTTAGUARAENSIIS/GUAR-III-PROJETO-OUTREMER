//! Referencial orientado de um polígono: o retângulo de **área mínima** que o
//! envolve (rotating calipers simplificado — a orientação ótima é sempre
//! paralela a uma aresta), com eixo `u` no comprimento e `v` na largura.
//!
//! É o ÚNICO helper de orientação do motor para polígonos: `sports.rs`
//! (marcação de quadras), `stations.rs` (plataformas) e `landmarks.rs`
//! (ângulo de implantação dos monumentos) usam este. `advertising.rs` mantém
//! a sua extração por PCA porque trabalha com geometrias lineares (painéis),
//! não com polígonos.

/// Referencial local do retângulo de área mínima que envolve um polígono.
#[derive(Clone, Copy, Debug)]
pub struct OrientedFrame {
    cx: f64,
    cz: f64,
    /// Eixo longo (unitário)
    ux: f64,
    uz: f64,
    /// Meia extensão ao longo de `u` (comprimento) e de `v` (largura)
    pub half_len: f64,
    pub half_wid: f64,
}

impl OrientedFrame {
    /// Retângulo envolvente de área mínima (rotating calipers simplificado: a
    /// orientação ótima é sempre paralela a uma das arestas do fecho).
    pub fn from_polygon(nodes: &[(i32, i32)]) -> Option<OrientedFrame> {
        let pts: Vec<(f64, f64)> = nodes.iter().map(|&(x, z)| (x as f64, z as f64)).collect();
        if pts.len() < 3 {
            return None;
        }
        let mut best: Option<(f64, OrientedFrame)> = None;
        for i in 0..pts.len() {
            let (x1, z1) = pts[i];
            let (x2, z2) = pts[(i + 1) % pts.len()];
            let (dx, dz) = (x2 - x1, z2 - z1);
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1.0 {
                continue;
            }
            let (ux, uz) = (dx / len, dz / len);
            let (vx, vz) = (-uz, ux);
            let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for &(px, pz) in &pts {
                let u = px * ux + pz * uz;
                let v = px * vx + pz * vz;
                umin = umin.min(u);
                umax = umax.max(u);
                vmin = vmin.min(v);
                vmax = vmax.max(v);
            }
            let area = (umax - umin) * (vmax - vmin);
            if best.as_ref().is_none_or(|(a, _)| area < *a) {
                let uc = (umin + umax) / 2.0;
                let vc = (vmin + vmax) / 2.0;
                let mut frame = OrientedFrame {
                    cx: uc * ux + vc * vx,
                    cz: uc * uz + vc * vz,
                    ux,
                    uz,
                    half_len: (umax - umin) / 2.0,
                    half_wid: (vmax - vmin) / 2.0,
                };
                if frame.half_wid > frame.half_len {
                    // Garante `u` = eixo longo
                    frame = OrientedFrame {
                        ux: vx,
                        uz: vz,
                        half_len: frame.half_wid,
                        half_wid: frame.half_len,
                        ..frame
                    };
                }
                best = Some((area, frame));
            }
        }
        best.map(|(_, f)| f)
    }

    /// Ângulo do eixo longo (radianos, `atan2(uz, ux)`).
    #[inline]
    pub fn angle(&self) -> f64 {
        self.uz.atan2(self.ux)
    }

    #[inline]
    pub fn local(&self, x: i32, z: i32) -> (f64, f64) {
        let dx = x as f64 - self.cx;
        let dz = z as f64 - self.cz;
        (dx * self.ux + dz * self.uz, -dx * self.uz + dz * self.ux)
    }

    #[inline]
    pub fn world(&self, u: f64, v: f64) -> (i32, i32) {
        let x = self.cx + u * self.ux - v * self.uz;
        let z = self.cz + u * self.uz + v * self.ux;
        (x.round() as i32, z.round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_of_axis_aligned_rectangle_has_long_axis_along_x() {
        let f = OrientedFrame::from_polygon(&[(0, 0), (40, 0), (40, 20), (0, 20)]).unwrap();
        assert!((f.half_len - 20.0).abs() < 1e-6);
        assert!((f.half_wid - 10.0).abs() < 1e-6);
        assert!(f.ux.abs() > 0.99);
        let (u, v) = f.local(40, 10);
        assert!((u.abs() - 20.0).abs() < 1e-6 && v.abs() < 1e-6);
    }

    #[test]
    fn frame_of_rotated_rectangle_recovers_true_dimensions() {
        // Retângulo 30×10 girado 45°: o AABB daria ~28×28; o retângulo mínimo, 30×10.
        let c = (2.0_f64).sqrt() / 2.0;
        let pts: Vec<(i32, i32)> = [(0.0, 0.0), (30.0, 0.0), (30.0, 10.0), (0.0, 10.0)]
            .iter()
            .map(|&(x, z)| {
                (
                    (x * c - z * c).round() as i32,
                    (x * c + z * c).round() as i32,
                )
            })
            .collect();
        let f = OrientedFrame::from_polygon(&pts).unwrap();
        assert!((f.half_len - 15.0).abs() < 1.0, "{}", f.half_len);
        assert!((f.half_wid - 5.0).abs() < 1.0, "{}", f.half_wid);
    }

    #[test]
    fn world_and_local_are_inverse() {
        let f = OrientedFrame::from_polygon(&[(10, 5), (50, 25), (40, 45), (0, 25)]).unwrap();
        let (x, z) = f.world(7.0, -3.0);
        let (u, v) = f.local(x, z);
        assert!((u - 7.0).abs() < 0.75 && (v + 3.0).abs() < 0.75);
    }
}
