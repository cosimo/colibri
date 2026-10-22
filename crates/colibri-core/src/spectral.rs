//! Spectra sampled on a fixed 380–780 nm grid at 1 nm.

use crate::color::Xyz;

pub const START_NM: f64 = 380.0;
pub const STEP_NM: f64 = 1.0;
pub const LEN: usize = 401;

/// Maximum luminous efficacy (lm/W): converts radiance to cd/m².
pub const KM: f64 = 683.0;

/// Spectral values (radiance, sensitivity, ...) on the shared grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum(pub Vec<f64>);

pub fn wavelength(i: usize) -> f64 {
    START_NM + i as f64 * STEP_NM
}

impl Spectrum {
    pub fn zero() -> Spectrum {
        Spectrum(vec![0.0; LEN])
    }

    pub fn from_fn(f: impl Fn(f64) -> f64) -> Spectrum {
        Spectrum((0..LEN).map(|i| f(wavelength(i))).collect())
    }

    /// Asymmetric Gaussian peaking at 1.0, with separate widths below and above `peak`.
    pub fn gaussian(peak: f64, sigma_below: f64, sigma_above: f64) -> Spectrum {
        Spectrum::from_fn(|wl| {
            let s = if wl < peak { sigma_below } else { sigma_above };
            (-0.5 * ((wl - peak) / s).powi(2)).exp()
        })
    }

    pub fn scaled(&self, k: f64) -> Spectrum {
        Spectrum(self.0.iter().map(|v| v * k).collect())
    }

    /// `self + other * k`.
    pub fn add_scaled(&self, other: &Spectrum, k: f64) -> Spectrum {
        Spectrum(
            self.0
                .iter()
                .zip(&other.0)
                .map(|(a, b)| a + b * k)
                .collect(),
        )
    }

    /// ∫ self(λ)·other(λ) dλ: the response of a sensor with sensitivity `self` to light `other`.
    pub fn integrate(&self, other: &Spectrum) -> f64 {
        self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum::<f64>() * STEP_NM
    }
}

/// CIE 1931 2° colour matching functions x̄, ȳ, z̄.
///
/// Uses the multi-lobe analytic fit of Wyman, Sloan & Shirley (2013), which
/// tracks the tabulated functions closely. Good enough for the simulator;
/// anything that reports colorimetry to users must use the CIE tables.
pub fn cie1931_approx() -> [Spectrum; 3] {
    let g = |wl: f64, mu: f64, s1: f64, s2: f64| {
        let s = if wl < mu { s1 } else { s2 };
        (-0.5 * ((wl - mu) / s).powi(2)).exp()
    };
    [
        Spectrum::from_fn(|wl| {
            1.056 * g(wl, 599.8, 37.9, 31.0) + 0.362 * g(wl, 442.0, 16.0, 26.7)
                - 0.065 * g(wl, 501.1, 20.4, 26.2)
        }),
        Spectrum::from_fn(|wl| 0.821 * g(wl, 568.8, 46.9, 40.5) + 0.286 * g(wl, 530.9, 16.3, 31.1)),
        Spectrum::from_fn(|wl| 1.217 * g(wl, 437.0, 11.8, 36.0) + 0.681 * g(wl, 459.0, 26.0, 13.8)),
    ]
}

/// XYZ of a radiance spectrum in W/(sr·m²·nm), giving luminance in cd/m².
pub fn to_xyz(cmf: &[Spectrum; 3], radiance: &Spectrum) -> Xyz {
    Xyz::new(
        KM * cmf[0].integrate(radiance),
        KM * cmf[1].integrate(radiance),
        KM * cmf[2].integrate(radiance),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_energy_white_is_near_e() {
        let cmf = cie1931_approx();
        let xy = to_xyz(&cmf, &Spectrum::from_fn(|_| 1.0)).chromaticity();
        // Illuminant E is (1/3, 1/3); the analytic fit lands within a few thousandths.
        assert!(
            (xy.x - 1.0 / 3.0).abs() < 0.005 && (xy.y - 1.0 / 3.0).abs() < 0.005,
            "{xy:?}"
        );
    }

    #[test]
    fn y_bar_peaks_near_555nm() {
        let cmf = cie1931_approx();
        let (i, _) = cmf[1]
            .0
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!((wavelength(i) - 556.0).abs() <= 3.0);
    }
}
