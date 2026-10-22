//! Small fixed-size linear algebra: everything colour science needs is 3x3.

use std::ops::Mul;

/// A 3-component vector: a tristimulus value, a sensor reading or an RGB triplet.
pub type Vec3 = [f64; 3];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, k: f64) -> Vec3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// Row-major 3x3 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn from_columns(c0: Vec3, c1: Vec3, c2: Vec3) -> Mat3 {
        Mat3([
            [c0[0], c1[0], c2[0]],
            [c0[1], c1[1], c2[1]],
            [c0[2], c1[2], c2[2]],
        ])
    }

    pub fn diagonal(d: Vec3) -> Mat3 {
        Mat3([[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]])
    }

    pub fn column(&self, i: usize) -> Vec3 {
        [self.0[0][i], self.0[1][i], self.0[2][i]]
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Inverse, or `None` when the matrix is (numerically) singular.
    pub fn inverse(&self) -> Option<Mat3> {
        let m = &self.0;
        let det = self.determinant();
        let norm = m.iter().flatten().fold(0.0_f64, |a, v| a.max(v.abs()));
        if norm == 0.0 || det.abs() <= 1e-12 * norm.powi(3) {
            return None;
        }
        let inv = 1.0 / det;
        Some(Mat3([
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    pub fn transpose(&self) -> Mat3 {
        let m = &self.0;
        Mat3([
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ])
    }

    /// Largest absolute element-wise difference, for approximate comparisons.
    pub fn max_abs_diff(&self, other: &Mat3) -> f64 {
        let mut d = 0.0_f64;
        for r in 0..3 {
            for c in 0..3 {
                d = d.max((self.0[r][c] - other.0[r][c]).abs());
            }
        }
        d
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }
}

impl Mul for Mat3 {
    type Output = Mat3;
    fn mul(self, o: Mat3) -> Mat3 {
        let mut out = [[0.0; 3]; 3];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.0[r][k] * o.0[k][c]).sum();
            }
        }
        Mat3(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_round_trips() {
        let m = Mat3([[2.0, 1.0, 0.5], [0.3, 4.0, 1.0], [0.1, 0.2, 3.0]]);
        let i = m.inverse().unwrap();
        assert!((m * i).max_abs_diff(&Mat3::IDENTITY) < 1e-12);
        assert!((i * m).max_abs_diff(&Mat3::IDENTITY) < 1e-12);
    }

    #[test]
    fn singular_has_no_inverse() {
        let m = Mat3([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]]);
        assert!(m.inverse().is_none());
        assert!(Mat3([[0.0; 3]; 3]).inverse().is_none());
    }

    #[test]
    fn columns_and_vectors() {
        let m = Mat3::from_columns([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]);
        assert_eq!(m.column(1), [4.0, 5.0, 6.0]);
        assert_eq!(m * [1.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        assert_eq!(m.transpose().column(0), [1.0, 4.0, 7.0]);
    }
}
