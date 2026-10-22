//! Colorimetry: CIE XYZ, chromaticity, CIELAB, ΔE2000, chromatic adaptation and
//! RGB colour space matrices.

use crate::math::{Mat3, Vec3};

/// CIE 1931 tristimulus value. `y` is luminance (cd/m² for absolute readings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xyz {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// CIE 1931 xy chromaticity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

/// CIELAB (1976) colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// CIE standard illuminant D65, 2° observer.
pub const D65: Xy = Xy {
    x: 0.3127,
    y: 0.3290,
};

/// CIE standard illuminant D50, 2° observer (the ICC profile connection space white).
pub const D50: Xy = Xy {
    x: 0.3457,
    y: 0.3585,
};

impl Xyz {
    pub const fn new(x: f64, y: f64, z: f64) -> Xyz {
        Xyz { x, y, z }
    }

    pub fn from_vec(v: Vec3) -> Xyz {
        Xyz::new(v[0], v[1], v[2])
    }

    pub fn to_vec(self) -> Vec3 {
        [self.x, self.y, self.z]
    }

    pub fn scaled(self, k: f64) -> Xyz {
        Xyz::new(self.x * k, self.y * k, self.z * k)
    }

    pub fn chromaticity(self) -> Xy {
        let sum = self.x + self.y + self.z;
        Xy {
            x: self.x / sum,
            y: self.y / sum,
        }
    }

    /// CIELAB relative to `white`. Both values must use the same luminance scale.
    pub fn to_lab(self, white: Xyz) -> Lab {
        fn f(t: f64) -> f64 {
            const D: f64 = 6.0 / 29.0;
            if t > D * D * D {
                t.cbrt()
            } else {
                t / (3.0 * D * D) + 4.0 / 29.0
            }
        }
        let fx = f(self.x / white.x);
        let fy = f(self.y / white.y);
        let fz = f(self.z / white.z);
        Lab {
            l: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }
}

impl Xy {
    pub const fn new(x: f64, y: f64) -> Xy {
        Xy { x, y }
    }

    /// The tristimulus value with this chromaticity and luminance `y_lum`.
    pub fn to_xyz(self, y_lum: f64) -> Xyz {
        Xyz::new(
            self.x / self.y * y_lum,
            y_lum,
            (1.0 - self.x - self.y) / self.y * y_lum,
        )
    }

    pub fn distance(self, other: Xy) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

impl Lab {
    pub const fn new(l: f64, a: f64, b: f64) -> Lab {
        Lab { l, a, b }
    }
}

/// CIEDE2000 colour difference, following Sharma, Wu & Dalal (2005).
pub fn delta_e_2000(c1: Lab, c2: Lab) -> f64 {
    use std::f64::consts::PI;
    let deg = |r: f64| r * 180.0 / PI;
    let rad = |d: f64| d * PI / 180.0;
    const P25_7: f64 = 6_103_515_625.0; // 25^7

    let c1_ab = c1.a.hypot(c1.b);
    let c2_ab = c2.a.hypot(c2.b);
    let c_mean7 = ((c1_ab + c2_ab) / 2.0).powi(7);
    let g = 0.5 * (1.0 - (c_mean7 / (c_mean7 + P25_7)).sqrt());
    let a1 = (1.0 + g) * c1.a;
    let a2 = (1.0 + g) * c2.a;
    let cp1 = a1.hypot(c1.b);
    let cp2 = a2.hypot(c2.b);
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            deg(b.atan2(a)).rem_euclid(360.0)
        }
    };
    let h1 = hue(c1.b, a1);
    let h2 = hue(c2.b, a2);

    let d_l = c2.l - c1.l;
    let d_c = cp2 - cp1;
    let chroma_zero = cp1 * cp2 == 0.0;
    let mut dh = h2 - h1;
    if chroma_zero {
        dh = 0.0;
    } else if dh > 180.0 {
        dh -= 360.0;
    } else if dh < -180.0 {
        dh += 360.0;
    }
    let d_h = 2.0 * (cp1 * cp2).sqrt() * rad(dh / 2.0).sin();

    let l_mean = (c1.l + c2.l) / 2.0;
    let cp_mean = (cp1 + cp2) / 2.0;
    let h_mean = if chroma_zero {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };

    let t = 1.0 - 0.17 * rad(h_mean - 30.0).cos()
        + 0.24 * rad(2.0 * h_mean).cos()
        + 0.32 * rad(3.0 * h_mean + 6.0).cos()
        - 0.20 * rad(4.0 * h_mean - 63.0).cos();
    let l50 = (l_mean - 50.0).powi(2);
    let s_l = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let s_c = 1.0 + 0.045 * cp_mean;
    let s_h = 1.0 + 0.015 * cp_mean * t;
    let cp_mean7 = cp_mean.powi(7);
    let r_c = 2.0 * (cp_mean7 / (cp_mean7 + P25_7)).sqrt();
    let d_theta = 30.0 * (-((h_mean - 275.0) / 25.0).powi(2)).exp();
    let r_t = -r_c * rad(2.0 * d_theta).sin();

    let (tl, tc, th) = (d_l / s_l, d_c / s_c, d_h / s_h);
    (tl * tl + tc * tc + th * th + r_t * tc * th).sqrt()
}

/// Bradford chromatic adaptation matrix taking colours seen under `from` to
/// their corresponding colours under `to`.
pub fn bradford(from: Xyz, to: Xyz) -> Mat3 {
    const M: Mat3 = Mat3([
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ]);
    let src = M * from.to_vec();
    let dst = M * to.to_vec();
    let gain = Mat3::diagonal([dst[0] / src[0], dst[1] / src[1], dst[2] / src[2]]);
    M.inverse().expect("Bradford matrix is invertible") * gain * M
}

/// Matrix converting linear RGB to XYZ for a colour space with the given
/// primaries and white point, normalised so RGB (1,1,1) has luminance 1.
/// Returns `None` if the primaries are collinear.
pub fn rgb_to_xyz_matrix(red: Xy, green: Xy, blue: Xy, white: Xy) -> Option<Mat3> {
    let p = Mat3::from_columns(
        red.to_xyz(1.0).to_vec(),
        green.to_xyz(1.0).to_vec(),
        blue.to_xyz(1.0).to_vec(),
    );
    let s = p.inverse()? * white.to_xyz(1.0).to_vec();
    Some(p * Mat3::diagonal(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_e_2000_matches_sharma_reference_data() {
        // Pairs from Sharma, Wu & Dalal (2005), Table 1.
        let cases = [
            ((50.0, 2.6772, -79.7751), (50.0, 0.0, -82.7485), 2.0425),
            ((50.0, 3.1571, -77.2803), (50.0, 0.0, -82.7485), 2.8615),
            ((50.0, 2.8361, -74.0200), (50.0, 0.0, -82.7485), 3.4412),
            ((50.0, 0.0, 0.0), (50.0, -1.0, 2.0), 2.3669),
            ((50.0, 2.5, 0.0), (73.0, 25.0, -18.0), 27.1492),
            (
                (60.2574, -34.0099, 36.2677),
                (60.4626, -34.1751, 39.4387),
                1.2644,
            ),
        ];
        for ((l1, a1, b1), (l2, a2, b2), expected) in cases {
            let de = delta_e_2000(Lab::new(l1, a1, b1), Lab::new(l2, a2, b2));
            assert!((de - expected).abs() < 1e-4, "got {de}, want {expected}");
            let rev = delta_e_2000(Lab::new(l2, a2, b2), Lab::new(l1, a1, b1));
            assert!((de - rev).abs() < 1e-9, "ΔE2000 must be symmetric");
        }
    }

    #[test]
    fn white_is_lab_100_0_0() {
        let w = D65.to_xyz(80.0);
        let lab = w.to_lab(w);
        assert!((lab.l - 100.0).abs() < 1e-9 && lab.a.abs() < 1e-9 && lab.b.abs() < 1e-9);
    }

    #[test]
    fn chromaticity_round_trip() {
        let xyz = Xyz::new(41.0, 35.0, 12.0);
        let back = xyz.chromaticity().to_xyz(xyz.y);
        assert!((back.x - xyz.x).abs() < 1e-12 && (back.z - xyz.z).abs() < 1e-12);
    }

    #[test]
    fn srgb_matrix_matches_iec_61966_2_1() {
        let m = rgb_to_xyz_matrix(
            Xy::new(0.64, 0.33),
            Xy::new(0.30, 0.60),
            Xy::new(0.15, 0.06),
            D65,
        )
        .unwrap();
        let expected = Mat3([
            [0.4124, 0.3576, 0.1805],
            [0.2126, 0.7152, 0.0722],
            [0.0193, 0.1192, 0.9505],
        ]);
        assert!(m.max_abs_diff(&expected) < 1e-3, "{m:?}");
    }

    #[test]
    fn bradford_d65_to_d50_matches_lindbloom() {
        let d65 = Xyz::new(0.95047, 1.0, 1.08883);
        let d50 = Xyz::new(0.96422, 1.0, 0.82521);
        let m = bradford(d65, d50);
        let expected = Mat3([
            [1.0478112, 0.0228866, -0.0501270],
            [0.0295424, 0.9904844, -0.0170491],
            [-0.0092345, 0.0150436, 0.7521316],
        ]);
        assert!(m.max_abs_diff(&expected) < 1e-6, "{m:?}");
        let mapped = Xyz::from_vec(m * d65.to_vec());
        assert!((mapped.x - d50.x).abs() < 1e-9 && (mapped.z - d50.z).abs() < 1e-9);
    }
}
