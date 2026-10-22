//! Display calibration: measure each channel's tone response and build the
//! per-channel 1D curves (what the OS loads into the GPU) that give the
//! target tone curve with every gray at the target white.
//!
//! The method works with relative instruments too. A display emits
//! `black + R(r)·P_red + G(g)·P_green + B(b)·P_blue`, and any sensor that is
//! linear in light sees that through a fixed 3x3 matrix. Measuring full red,
//! green and blue gives that matrix, and inverting it turns every later reading
//! into the panel's linear channel outputs, whatever the sensor's own colour
//! space. Only a white point given as a chromaticity needs an absolute
//! instrument.

use crate::color::Xy;
use crate::curve::{CurveError, TargetCurve, ToneCurve};
use crate::math::{add, scale, sub, Mat3, Vec3};
use crate::rig::{MeasureError, Rgb, Rig, Trust};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WhitePoint {
    /// Keep the white the panel produces at full drive. Works with any instrument.
    Native,
    /// Reach this chromaticity, giving up as little luminance as possible.
    /// Needs a [`Trust::Absolute`] instrument.
    Chromaticity(Xy),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub white: WhitePoint,
    pub curve: TargetCurve,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Gray ramp patches, including full white. More steps resolve the shadows better.
    pub steps: usize,
    /// Readings averaged per patch. Instrument noise is the main error source
    /// once there are enough steps, and it falls with the square root of this.
    pub repeats: usize,
    /// Entries per channel in the output curves.
    pub lut_size: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            steps: 33,
            repeats: 1,
            lut_size: 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalibrateError {
    Measure(MeasureError),
    /// The target white point is a chromaticity but the rig is only relative.
    NeedsAbsoluteInstrument,
    /// Full red, green and blue readings are not independent, so the sensor
    /// cannot separate the channels (wrong patch shown, sensor saturated, ...).
    DegenerateChannels,
    /// The panel cannot produce the target white.
    WhiteOutOfGamut,
    Curve(CurveError),
    InvalidOptions(&'static str),
}

impl std::fmt::Display for CalibrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalibrateError::Measure(e) => e.fmt(f),
            CalibrateError::NeedsAbsoluteInstrument => f.write_str(
                "this white point needs an absolute instrument; use the native white point instead",
            ),
            CalibrateError::DegenerateChannels => {
                f.write_str("red, green and blue readings are not independent")
            }
            CalibrateError::WhiteOutOfGamut => {
                f.write_str("the display cannot produce the target white point")
            }
            CalibrateError::Curve(e) => e.fmt(f),
            CalibrateError::InvalidOptions(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for CalibrateError {}

impl From<MeasureError> for CalibrateError {
    fn from(e: MeasureError) -> CalibrateError {
        CalibrateError::Measure(e)
    }
}

impl From<CurveError> for CalibrateError {
    fn from(e: CurveError) -> CalibrateError {
        CalibrateError::Curve(e)
    }
}

/// What the display does before calibration, in the rig's own units.
#[derive(Clone, Debug, PartialEq)]
pub struct Characterization {
    pub trust: Trust,
    /// Reading for drive (0, 0, 0).
    pub black: Vec3,
    /// Columns are the readings for full red, green and blue, minus black.
    pub channel_matrix: Mat3,
    /// Per-channel drive → linear output, where 1.0 is the channel's output
    /// when shown on its own at full drive.
    pub curves: [ToneCurve; 3],
    /// How far full white differs from black + red + green + blue, relative to
    /// white. Near 0 for a well-behaved panel; large values mean the channels
    /// interact and a 1D calibration will be less accurate.
    pub additivity_error: f64,
}

/// Per-channel 1D curves: input level (evenly spaced over 0–1) → drive value.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationCurves {
    pub channels: [Vec<f64>; 3],
}

impl CalibrationCurves {
    pub fn identity(size: usize) -> CalibrationCurves {
        let ramp: Vec<f64> = (0..size).map(|i| i as f64 / (size - 1) as f64).collect();
        CalibrationCurves {
            channels: [ramp.clone(), ramp.clone(), ramp],
        }
    }

    pub fn len(&self) -> usize {
        self.channels[0].len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drive values for input `rgb`, interpolating linearly between entries.
    pub fn apply(&self, rgb: Rgb) -> Rgb {
        let mut out = [0.0; 3];
        for (c, value) in out.iter_mut().enumerate() {
            let lut = &self.channels[c];
            let pos = rgb[c].clamp(0.0, 1.0) * (lut.len() - 1) as f64;
            let i = (pos.floor() as usize).min(lut.len() - 2);
            let t = pos - i as f64;
            *value = lut[i] * (1.0 - t) + lut[i + 1] * t;
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    pub characterization: Characterization,
    pub curves: CalibrationCurves,
}

/// Measure the display. The display must be showing its uncalibrated
/// response, so reset the GPU curves to identity first.
pub fn characterize<R: Rig + ?Sized>(
    rig: &mut R,
    options: &Options,
) -> Result<Characterization, CalibrateError> {
    let steps = options.steps;
    let trust = rig.trust();
    if steps < 2 {
        return Err(CalibrateError::InvalidOptions("steps must be at least 2"));
    }
    if options.repeats == 0 {
        return Err(CalibrateError::InvalidOptions("repeats must be at least 1"));
    }
    let mut measure = |patch: Rgb| -> Result<Vec3, MeasureError> {
        let mut sum = [0.0; 3];
        for _ in 0..options.repeats {
            sum = add(sum, rig.measure(patch)?);
        }
        Ok(scale(sum, 1.0 / options.repeats as f64))
    };
    let black = measure([0.0, 0.0, 0.0])?;
    let r = sub(measure([1.0, 0.0, 0.0])?, black);
    let g = sub(measure([0.0, 1.0, 0.0])?, black);
    let b = sub(measure([0.0, 0.0, 1.0])?, black);
    let channel_matrix = Mat3::from_columns(r, g, b);
    let to_linear = channel_matrix
        .inverse()
        .ok_or(CalibrateError::DegenerateChannels)?;

    let mut drives = vec![0.0];
    let mut linear: Vec<Vec3> = vec![[0.0; 3]];
    let mut white = black;
    for i in 1..steps {
        let s = i as f64 / (steps - 1) as f64;
        let reading = measure([s, s, s])?;
        drives.push(s);
        linear.push(to_linear * sub(reading, black));
        white = reading;
    }

    let predicted = add(add(black, r), add(g, b));
    let additivity_error = norm(sub(white, predicted)) / norm(sub(white, black));

    let curve = |c: usize| {
        let ys: Vec<f64> = linear.iter().map(|l| l[c]).collect();
        ToneCurve::from_samples(&drives, &ys)
    };
    Ok(Characterization {
        trust,
        black,
        channel_matrix,
        curves: [curve(0)?, curve(1)?, curve(2)?],
        additivity_error,
    })
}

/// Linear output each channel must reach at full white for `white`.
pub fn white_channel_amounts(
    ch: &Characterization,
    white: WhitePoint,
) -> Result<Vec3, CalibrateError> {
    let max = ch.curves.each_ref().map(|c| c.max_output());
    let Xy { x, y } = match white {
        WhitePoint::Native => return Ok(max),
        WhitePoint::Chromaticity(xy) => xy,
    };
    if ch.trust != Trust::Absolute {
        return Err(CalibrateError::NeedsAbsoluteInstrument);
    }
    let to_linear = ch
        .channel_matrix
        .inverse()
        .ok_or(CalibrateError::DegenerateChannels)?;
    // White = black + M·a must be proportional to the target T:
    // a = k·u − v with u = M⁻¹·T and v = M⁻¹·black. Pick the largest k
    // that keeps every channel within its range.
    let u = to_linear * Xy::new(x, y).to_xyz(1.0).to_vec();
    let v = to_linear * ch.black;
    if u.iter().any(|&uc| uc <= 0.0) {
        return Err(CalibrateError::WhiteOutOfGamut);
    }
    let k = (0..3)
        .map(|c| (max[c] + v[c]) / u[c])
        .fold(f64::INFINITY, f64::min);
    let a = sub(scale(u, k), v);
    if a.iter().any(|&ac| ac <= 0.0) {
        return Err(CalibrateError::WhiteOutOfGamut);
    }
    Ok(a)
}

/// Build the calibration curves for `target` from a characterization.
pub fn build_curves(
    ch: &Characterization,
    target: Target,
    lut_size: usize,
) -> Result<CalibrationCurves, CalibrateError> {
    if lut_size < 2 {
        return Err(CalibrateError::InvalidOptions(
            "lut_size must be at least 2",
        ));
    }
    let amounts = white_channel_amounts(ch, target.white)?;
    let channels = [0, 1, 2].map(|c| {
        (0..lut_size)
            .map(|i| {
                let level = target.curve.eval(i as f64 / (lut_size - 1) as f64);
                ch.curves[c].inverse(amounts[c] * level)
            })
            .collect()
    });
    Ok(CalibrationCurves { channels })
}

/// Characterize the display and build calibration curves in one go.
pub fn calibrate<R: Rig + ?Sized>(
    rig: &mut R,
    target: Target,
    options: Options,
) -> Result<Calibration, CalibrateError> {
    if target.white != WhitePoint::Native && rig.trust() != Trust::Absolute {
        // Fail before spending time on measurements.
        return Err(CalibrateError::NeedsAbsoluteInstrument);
    }
    let characterization = characterize(rig, &options)?;
    let curves = build_curves(&characterization, target, options.lut_size)?;
    Ok(Calibration {
        characterization,
        curves,
    })
}

fn norm(v: Vec3) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Additive display with per-channel gammas, seen through a fixed sensor matrix.
    struct ToyRig {
        gamma: Vec3,
        sensor: Mat3,
        black: Vec3,
        trust: Trust,
    }

    impl Rig for ToyRig {
        fn measure(&mut self, p: Rgb) -> Result<Vec3, MeasureError> {
            let lin = [0, 1, 2].map(|c| p[c].powf(self.gamma[c]));
            Ok(add(self.sensor * lin, self.black))
        }
        fn trust(&self) -> Trust {
            self.trust
        }
    }

    fn toy(trust: Trust) -> ToyRig {
        ToyRig {
            gamma: [2.0, 2.3, 2.6],
            sensor: Mat3([[40.0, 35.0, 18.0], [21.0, 70.0, 7.0], [2.0, 12.0, 95.0]]),
            black: [0.1, 0.1, 0.12],
            trust,
        }
    }

    #[test]
    fn recovers_channel_gammas() {
        let ch = characterize(&mut toy(Trust::Relative), &Options::default()).unwrap();
        assert!(ch.additivity_error < 1e-12);
        for (c, g) in [2.0, 2.3, 2.6].into_iter().enumerate() {
            let measured = ch.curves[c].eval(0.5);
            assert!((measured - 0.5_f64.powf(g)).abs() < 1e-3, "channel {c}");
        }
    }

    #[test]
    fn native_white_curves_linearize_to_target() {
        let mut rig = toy(Trust::Relative);
        let target = Target {
            white: WhitePoint::Native,
            curve: TargetCurve::Gamma(2.2),
        };
        let cal = calibrate(&mut rig, target, Options::default()).unwrap();
        for v in [0.25, 0.5, 0.75, 1.0] {
            let drive = cal.curves.apply([v, v, v]);
            for (c, g) in [2.0, 2.3, 2.6].into_iter().enumerate() {
                let lin = drive[c].powf(g);
                assert!((lin - v.powf(2.2)).abs() < 2e-3, "v={v} channel {c}: {lin}");
            }
        }
    }

    #[test]
    fn absolute_white_needs_absolute_rig() {
        let target = Target {
            white: WhitePoint::Chromaticity(crate::color::D65),
            curve: TargetCurve::Srgb,
        };
        let err = calibrate(&mut toy(Trust::Relative), target, Options::default()).unwrap_err();
        assert_eq!(err, CalibrateError::NeedsAbsoluteInstrument);
    }

    #[test]
    fn identity_curves_pass_through() {
        let id = CalibrationCurves::identity(256);
        let out = id.apply([0.0, 0.3, 1.0]);
        assert!((out[1] - 0.3).abs() < 1e-12 && out[0] == 0.0 && out[2] == 1.0);
    }
}
