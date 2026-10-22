//! A spectral model of an uncalibrated display.

use colibri_core::calibrate::CalibrationCurves;
use colibri_core::color::Xyz;
use colibri_core::math::Vec3;
use colibri_core::rig::Rgb;
use colibri_core::spectral::{self, Spectrum};

/// Additive display: black leakage plus three primaries, each with its own
/// power-law response. Optional calibration curves play the role of the GPU
/// lookup table the OS loads.
#[derive(Clone, Debug)]
pub struct SimDisplay {
    /// Radiance of each primary at full drive, W/(sr·m²·nm).
    pub primaries: [Spectrum; 3],
    /// Radiance at zero drive.
    pub black: Spectrum,
    pub gamma: Vec3,
    /// Amplitude of a ripple on each channel's response, as panels with
    /// coarse firmware lookup tables have: output is multiplied by
    /// `1 + ripple·sin(8π·drive)`. Must stay below about 0.08 to keep the
    /// response increasing.
    pub ripple: Vec3,
    /// Channel interaction: each channel's output drops by this fraction of
    /// the mean output of the other two, as with power limiting on OLEDs or
    /// crosstalk on LCDs. Makes the display non-additive.
    pub crosstalk: f64,
    /// Curves applied to every input before it reaches the panel.
    pub curves: Option<CalibrationCurves>,
    /// Bit depth of the link to the panel. Drive values are rounded to it.
    pub output_bits: Option<u32>,
    cmf: [Spectrum; 3],
}

/// One primary: peak wavelength, width (nm) and relative strength.
#[derive(Clone, Copy, Debug)]
pub struct Primary {
    pub peak: f64,
    pub width: f64,
    pub gain: f64,
}

impl SimDisplay {
    /// Build a display whose full white has luminance `white_cd_m2` and whose
    /// black is `1 / contrast` of white with the same spectrum (backlight leakage).
    pub fn new(
        primaries: [Primary; 3],
        gamma: Vec3,
        white_cd_m2: f64,
        contrast: f64,
    ) -> SimDisplay {
        let cmf = spectral::cie1931_approx();
        let shapes = primaries.map(|p| Spectrum::gaussian(p.peak, p.width, p.width).scaled(p.gain));
        let white = shapes[0]
            .add_scaled(&shapes[1], 1.0)
            .add_scaled(&shapes[2], 1.0);
        let k = white_cd_m2 / spectral::to_xyz(&cmf, &white).y;
        SimDisplay {
            black: white.scaled(k / contrast),
            primaries: shapes.map(|s| s.scaled(k)),
            gamma,
            ripple: [0.0; 3],
            crosstalk: 0.0,
            curves: None,
            output_bits: None,
            cmf,
        }
    }

    /// Narrow-primary wide-gamut panel (QD/OLED-like) with a cool native white
    /// and mismatched channel gammas.
    pub fn wide_gamut() -> SimDisplay {
        SimDisplay::new(
            [
                Primary {
                    peak: 630.0,
                    width: 9.0,
                    gain: 1.00,
                },
                Primary {
                    peak: 530.0,
                    width: 13.0,
                    gain: 0.80,
                },
                Primary {
                    peak: 455.0,
                    width: 8.0,
                    gain: 0.95,
                },
            ],
            [2.05, 2.30, 2.55],
            160.0,
            1000.0,
        )
    }

    /// Broad-primary sRGB-class IPS panel.
    pub fn srgb_ips() -> SimDisplay {
        SimDisplay::new(
            [
                Primary {
                    peak: 610.0,
                    width: 25.0,
                    gain: 1.0,
                },
                Primary {
                    peak: 545.0,
                    width: 30.0,
                    gain: 1.0,
                },
                Primary {
                    peak: 450.0,
                    width: 12.0,
                    gain: 1.0,
                },
            ],
            [2.2, 2.25, 2.35],
            120.0,
            1200.0,
        )
    }

    /// Drive values that reach the panel for `input`.
    pub fn drive(&self, input: Rgb) -> Rgb {
        let mut d = match &self.curves {
            Some(curves) => curves.apply(input),
            None => input.map(|v| v.clamp(0.0, 1.0)),
        };
        if let Some(bits) = self.output_bits {
            let max = ((1u64 << bits) - 1) as f64;
            d = d.map(|v| (v * max).round() / max);
        }
        d
    }

    /// Emitted radiance for `input`.
    pub fn spectrum(&self, input: Rgb) -> Spectrum {
        let d = self.drive(input);
        let lin = [0, 1, 2].map(|c| {
            let ripple = 1.0 + self.ripple[c] * (8.0 * std::f64::consts::PI * d[c]).sin();
            d[c].powf(self.gamma[c]) * ripple
        });
        let total: f64 = lin.iter().sum();
        (0..3).fold(self.black.clone(), |acc, c| {
            let others = (total - lin[c]) / 2.0;
            acc.add_scaled(&self.primaries[c], lin[c] * (1.0 - self.crosstalk * others))
        })
    }

    /// Ground-truth colorimetry of `input`, with no instrument error.
    pub fn true_xyz(&self, input: Rgb) -> Xyz {
        spectral::to_xyz(&self.cmf, &self.spectrum(input))
    }
}
