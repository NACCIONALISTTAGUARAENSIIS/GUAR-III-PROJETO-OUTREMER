use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Bounding Box in minecraft XZ space with varied shapes.
#[derive(Clone, Debug)]
pub enum XZBBox {
    Rect(XZBBoxRect),
}

impl XZBBox {
    /// 🚨 BESM-6 TWEAK: Construtor Direto Determinístico
    /// Instancia uma Bounding Box Retangular a partir de limites absolutos inteiros.
    pub fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        Self::Rect(
            XZBBoxRect::new(
                XZPoint { x: min_x, z: min_z },
                XZPoint { x: max_x, z: max_z },
            ).expect("BESM-6 Erro Crítico: Coordenadas de Bounding Box inválidas (min > max).")
        )
    }

    /// 🚨 BESM-6 TWEAK: Alias Semântico Explícito
    /// Utilizado quando o pipeline exige clareza absoluta de fronteiras (ex: Map Rendering).
    pub fn explicit(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        Self::new(min_x, max_x, min_z, max_z)
    }

    /// Construct rectangle shape bbox from the x and z lengths of the world, originated at (0, 0)
    pub fn rect_from_xz_lengths(length_x: f64, length_z: f64) -> Result<Self, String> {
        if !length_x.is_finite() {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x not finite: {length_x}"
            ));
        }

        if !length_z.is_finite() {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z not finite: {length_z}"
            ));
        }

        if length_x < 0.0 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x should >=0 , but encountered {length_x}"
            ));
        }

        if length_z < 0.0 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z should >=0 , but encountered {length_z}"
            ));
        }

        let length_x_floor = length_x.floor();
        let length_z_floor = length_z.floor();

        if length_x_floor > i32::MAX as f64 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x too large for i32: {length_x}"
            ));
        }

        if length_z_floor > i32::MAX as f64 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z too large for i32: {length_z}"
            ));
        }

        Ok(Self::Rect(XZBBoxRect::new(
            XZPoint { x: 0, z: 0 },
            XZPoint {
                x: length_x_floor as i32,
                z: length_z_floor as i32,
            },
        )?))
    }

    /// Check whether an XZPoint is covered
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        match self {
            Self::Rect(r) => r.contains(xzpoint),
        }
    }

    /// Return the circumscribed rectangle of the current XZBBox shape
    pub fn bounding_rect(&self) -> XZBBoxRect {
        match self {
            Self::Rect(r) => *r,
        }
    }

    /// Return the min x in all covered blocks
    pub fn min_x(&self) -> i32 {
        self.bounding_rect().min().x
    }

    /// Return the max x in all covered blocks
    pub fn max_x(&self) -> i32 {
        self.bounding_rect().max().x
    }

    /// Return the min z in all covered blocks
    pub fn min_z(&self) -> i32 {
        self.bounding_rect().min().z
    }

    /// Return the max z in all covered blocks
    pub fn max_z(&self) -> i32 {
        self.bounding_rect().max().z
    }
}

impl fmt::Display for XZBBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rect(r) => write!(f, "XZBBox::{r}"),
        }
    }
}

impl Add<XZVector> for XZBBox {
    type Output = XZBBox;

    fn add(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r + other),
        }
    }
}

impl AddAssign<XZVector> for XZBBox {
    fn add_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r += other,
        }
    }
}

impl Sub<XZVector> for XZBBox {
    type Output = XZBBox;

    fn sub(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r - other),
        }
    }
}

impl SubAssign<XZVector> for XZBBox {
    fn sub_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r -= other,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_valid_inputs() {
        let obj = XZBBox::rect_from_xz_lengths(1.0, 1.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 2);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 2);
        assert_eq!(obj.bounding_rect().total_blocks(), 4);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 1);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 1);

        let obj = XZBBox::rect_from_xz_lengths(0.0, 1.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 1);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 2);

        let obj = XZBBox::rect_from_xz_lengths(1.0, 0.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 2);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 1);

        let obj = XZBBox::rect_from_xz_lengths(123.4, 322.5);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 124);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 323);
    }

    #[test]
    fn test_invalid_inputs() {
        assert!(XZBBox::rect_from_xz_lengths(-1.0, 1.5).is_err());
        assert!(XZBBox::rect_from_xz_lengths(0.2, i32::MAX as f64 + 10.0).is_err());
        assert!(XZBBox::rect_from_xz_lengths(f64::INFINITY, 10.0).is_err());
        assert!(XZBBox::rect_from_xz_lengths(f64::NAN, 10.0).is_err());
    }
}