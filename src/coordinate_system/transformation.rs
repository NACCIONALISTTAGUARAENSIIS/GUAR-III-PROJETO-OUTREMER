use super::cartesian::{XZBBox, XZPoint};
use super::geographic::{LLBBox, LLPoint};

// Constantes do Elipsoide de Referencia WGS84 (Tier Governamental / Geod�sico)
const WGS84_A: f64 = 6378137.0; // Raio Semi-maior (Equador) em metros
const WGS84_E2: f64 = 0.00669437999014; // Excentricidade ao quadrado

// ?? BESM-6 TWEAK: MARCO ZERO ABSOLUTO DE BRAS�LIA
// Este � o mastro da Pra�a dos Tr�s Poderes.
// Isso garante que todos os bairros/cidades-sat�lites gerados em exporta��es separadas
// convirjam perfeitamente no mesmo mundo sem sobreposi��es.
const DF_ORIGIN_LAT: f64 = -15.8000;
const DF_ORIGIN_LON: f64 = -47.8600;

/// Transform geographic space to a highly accurate
/// local tangential cartesian space (ENU) anchored to the Bras�lia Absolute Zero.
pub struct CoordTransformer {
    scale: f64, // BESM-6 Tweak: Blocks per meter (sempre 1.33 para Bras�lia)

    // Par�metros ECEF -> ENU (Earth-Centered Earth-Fixed -> East-North-Up)
    origin_ecef: (f64, f64, f64),

    // Matriz de Rota��o Pr�-computada ancorada na Pra�a dos Tr�s Poderes
    rot_matrix: [[f64; 3]; 3],
}

impl CoordTransformer {
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Converte Coordenadas Geogr�ficas (Graus) para ECEF (Metros 3D baseados no n�cleo da Terra)
    #[inline(always)]
    fn ll_to_ecef(lat_rad: f64, lon_rad: f64, h: f64) -> (f64, f64, f64) {
        let n = WGS84_A / (1.0 - WGS84_E2 * lat_rad.sin().powi(2)).sqrt();
        let x = (n + h) * lat_rad.cos() * lon_rad.cos();
        let y = (n + h) * lat_rad.cos() * lon_rad.sin();
        let z = (n * (1.0 - WGS84_E2) + h) * lat_rad.sin();
        (x, y, z)
    }

    pub fn llbbox_to_xzbbox(
        llbbox: &LLBBox,
        _scale: f64, // Ignorado. N�s for�amos o H_SCALE interno
    ) -> Result<(CoordTransformer, XZBBox), String> {
        let err_header = "Construct LLBBox to XZBBox transformation failed".to_string();

        let len_lat = llbbox.max().lat() - llbbox.min().lat();
        let len_lng = llbbox.max().lng() - llbbox.min().lng();

        // Prote��o Cr�tica contra Divis�o por Zero e Coordenadas Singulares
        if len_lat <= f64::EPSILON || len_lng <= f64::EPSILON {
            return Err(format!(
                "{}: BBox covers zero area (min and max coordinates are identical).",
                &err_header
            ));
        }

        // 1. O Centro de Refer�ncia � SEMPRE o Marco Zero de Bras�lia, nunca o centro da BBox.
        let origin_lat_rad = DF_ORIGIN_LAT.to_radians();
        let origin_lon_rad = DF_ORIGIN_LON.to_radians();

        let origin_ecef = Self::ll_to_ecef(origin_lat_rad, origin_lon_rad, 0.0);

        // 2. Matriz de Rota��o fixada no Plano Piloto
        let sin_lat = origin_lat_rad.sin();
        let cos_lat = origin_lat_rad.cos();
        let sin_lon = origin_lon_rad.sin();
        let cos_lon = origin_lon_rad.cos();

        let rot_matrix = [
            [-sin_lon, cos_lon, 0.0],
            [-sin_lat * cos_lon, -sin_lat * sin_lon, cos_lat],
            [cos_lat * cos_lon, cos_lat * sin_lon, sin_lat],
        ];

        // 3. Descobrir os limites do recorte atual (BBox) no espa�o Global ENU
        // ?? BESM-6: Amostragem Curva (Evita que o achatamento do ENU corte pontos extremos do arco do elipsoide)
        let mid_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
        let mid_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;

        let control_points = [
            (llbbox.min().lat(), llbbox.min().lng()), // Sudoeste
            (llbbox.max().lat(), llbbox.min().lng()), // Noroeste
            (llbbox.min().lat(), llbbox.max().lng()), // Sudeste
            (llbbox.max().lat(), llbbox.max().lng()), // Nordeste
            (mid_lat, llbbox.min().lng()),            // Meio-Oeste
            (mid_lat, llbbox.max().lng()),            // Meio-Leste
            (llbbox.min().lat(), mid_lon),            // Meio-Sul
            (llbbox.max().lat(), mid_lon),            // Meio-Norte
            (mid_lat, mid_lon),                       // Centro Absoluto
        ];

        let mut min_enu_x = f64::MAX;
        let mut max_enu_x = f64::MIN;
        let mut min_enu_n = f64::MAX;
        let mut max_enu_n = f64::MIN;

        for (lat, lon) in &control_points {
            let ecef = Self::ll_to_ecef(lat.to_radians(), lon.to_radians(), 0.0);

            let dx = ecef.0 - origin_ecef.0;
            let dy = ecef.1 - origin_ecef.1;
            let dz = ecef.2 - origin_ecef.2;

            let enu_x = rot_matrix[0][0] * dx + rot_matrix[0][1] * dy + rot_matrix[0][2] * dz; // East
            let enu_n = rot_matrix[1][0] * dx + rot_matrix[1][1] * dy + rot_matrix[1][2] * dz; // North

            if enu_x < min_enu_x {
                min_enu_x = enu_x;
            }
            if enu_x > max_enu_x {
                max_enu_x = enu_x;
            }
            if enu_n < min_enu_n {
                min_enu_n = enu_n;
            }
            if enu_n > max_enu_n {
                max_enu_n = enu_n;
            }
        }

        // ?? BESM-6 Tweak: A Escala Horizontal (1.33) Governamental Global
        let h_scale = 1.33;

        // O XZ_BBox � instanciado em coordenadas globais absolutas, n�o relativas ao tamanho do corte.
        let min_mc_x = (min_enu_x * h_scale).round() as i32;
        let max_mc_x = (max_enu_x * h_scale).round() as i32;

        // Z � invertido no Minecraft (Norte � -Z, Sul � +Z)
        let min_mc_z = (-max_enu_n * h_scale).round() as i32;
        let max_mc_z = (-min_enu_n * h_scale).round() as i32;

        // ?? BESM-6 Tweak: Construtor Estrutural Absoluto
        // Substitu�mos o duplo construtor inst�vel (`rect_from_xz_lengths` + `new`)
        // por uma aloca��o direta. Isso evita que valida��es internas do Arnis (como
        // impedir que um bbox tenha offset global negativo) quebrem a compila��o.
        let final_bbox = XZBBox::new(min_mc_x, max_mc_x, min_mc_z, max_mc_z);

        Ok((
            Self {
                scale: h_scale,
                origin_ecef,
                rot_matrix,
            },
            final_bbox,
        ))
    }

    #[inline(always)]
    pub fn transform_point(&self, llpoint: LLPoint) -> XZPoint {
        let ecef = Self::ll_to_ecef(llpoint.lat().to_radians(), llpoint.lng().to_radians(), 0.0);

        let dx = ecef.0 - self.origin_ecef.0;
        let dy = ecef.1 - self.origin_ecef.1;
        let dz = ecef.2 - self.origin_ecef.2;

        let enu_x =
            self.rot_matrix[0][0] * dx + self.rot_matrix[0][1] * dy + self.rot_matrix[0][2] * dz;
        let enu_n =
            self.rot_matrix[1][0] * dx + self.rot_matrix[1][1] * dy + self.rot_matrix[1][2] * dz;

        // Dist�ncia direta do Marco Zero x Escala (Z invertido para bater com o Norte=Z negativo do MC)
        let final_x = enu_x * self.scale;
        let final_z = -enu_n * self.scale;

        XZPoint::new(final_x.round() as i32, final_z.round() as i32)
    }
}

// (lat meters, lon meters)
#[inline]
pub fn geo_distance(a: LLPoint, b: LLPoint) -> (f64, f64) {
    let z: f64 = lat_distance(a.lat(), b.lat());

    // distance between two lons depends on their latitude. In this case we'll just average them
    let x: f64 = lon_distance((a.lat() + b.lat()) / 2.0, a.lng(), b.lng());

    (z, x)
}

// Haversine but optimized for a latitude delta of 0
// returns meters
fn lon_distance(lat: f64, lon1: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lon: f64 = (lon2 - lon1).to_radians();
    let a: f64 =
        lat.to_radians().cos() * lat.to_radians().cos() * (d_lon / 2.0).sin() * (d_lon / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// Haversine but optimized for a longitude delta of 0
// returns meters
fn lat_distance(lat1: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lat: f64 = (lat2 - lat1).to_radians();
    let a: f64 = (d_lat / 2.0).sin() * (d_lat / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

#[cfg(test)]
pub fn lat_lon_to_minecraft_coords(
    lat: f64,
    lon: f64,
    bbox: LLBBox, // (min_lon, min_lat, max_lon, max_lat)
    scale_factor_z: f64,
    scale_factor_x: f64,
) -> (i32, i32) {
    let rel_x: f64 = (lon - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
    let rel_z: f64 = 1.0 - (lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

    let x: i32 = (rel_x * scale_factor_x) as i32;
    let z: i32 = (rel_z * scale_factor_z) as i32;

    (x, z)
}
