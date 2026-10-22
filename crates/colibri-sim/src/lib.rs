//! colibri-sim: a spectral display and instrument simulator, so the whole
//! calibration pipeline can be tested without hardware.

pub mod display;
pub mod rng;
pub mod sensor;
pub mod windows;

use colibri_core::color::{delta_e_2000, Xyz};
use colibri_core::curve::TargetCurve;
use colibri_core::math::Vec3;
use colibri_core::rig::{MeasureError, Rgb, Rig, Trust};

pub use display::{Primary, SimDisplay};
pub use sensor::SimSensor;

/// A simulated display with a simulated instrument on it.
#[derive(Clone, Debug)]
pub struct SimRig {
    pub display: SimDisplay,
    pub sensor: SimSensor,
    /// Number of patches measured so far.
    pub measurements: usize,
    /// Bits per channel of the simulated patch window; `None` shows any value.
    pub patch_bits: Option<u32>,
}

impl SimRig {
    pub fn new(display: SimDisplay, sensor: SimSensor) -> SimRig {
        SimRig {
            display,
            sensor,
            measurements: 0,
            patch_bits: None,
        }
    }
}

impl Rig for SimRig {
    fn measure(&mut self, patch: Rgb) -> Result<Vec3, MeasureError> {
        self.measurements += 1;
        let shown = self.quantize(patch);
        Ok(self.sensor.read(&self.display.spectrum(shown)))
    }

    fn trust(&self) -> Trust {
        self.sensor.trust
    }

    fn quantize(&self, patch: Rgb) -> Rgb {
        match self.patch_bits {
            Some(bits) => colibri_core::rig::quantize(patch, bits),
            None => patch,
        }
    }
}

/// Ground-truth check of one gray level after calibration.
#[derive(Clone, Copy, Debug)]
pub struct GrayCheck {
    pub level: f64,
    /// ΔE2000 between the gray shown and the ideal gray: the display's white
    /// chromaticity at the target's relative luminance, black included.
    pub delta_e: f64,
    /// Effective gamma at this level, black subtracted.
    pub gamma: f64,
}

/// Compare grays shown on `display` (through its loaded curves) with the
/// ideal response for `curve`, using exact colorimetry.
pub fn check_grays(display: &SimDisplay, curve: TargetCurve, levels: &[f64]) -> Vec<GrayCheck> {
    let white = display.true_xyz([1.0; 3]);
    let black = display.true_xyz([0.0; 3]);
    let span = |a: f64, b: f64, t: f64| a + (b - a) * t;
    levels
        .iter()
        .map(|&v| {
            let t = curve.eval(v);
            let ideal = Xyz::new(
                span(black.x, white.x, t),
                span(black.y, white.y, t),
                span(black.z, white.z, t),
            );
            let got = display.true_xyz([v; 3]);
            GrayCheck {
                level: v,
                delta_e: delta_e_2000(ideal.to_lab(white), got.to_lab(white)),
                gamma: ((got.y - black.y) / (white.y - black.y)).ln() / v.ln(),
            }
        })
        .collect()
}
