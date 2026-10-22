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
use crate::curve::{smooth_auto, CurveError, Noise, TargetCurve, ToneCurve};
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
    /// Smooth measurement noise out of the tone curves and refinement
    /// corrections. Costs `NOISE_PROBE` × 2 extra patches to measure the
    /// instrument's noise; the strength is then chosen from the data.
    pub smoothing: bool,
    /// Closed-loop passes after the first calibration: show the calibrated
    /// grays, measure them and correct the curves. Each pass costs `steps`
    /// patches (times `repeats`).
    pub refine_passes: usize,
    /// Entries per channel in the output curves.
    pub lut_size: usize,
}

const SMOOTHING_DEFAULT: bool = true;
const REFINE_DEFAULT: usize = 0;

/// Readings of white and of black taken to estimate the instrument's noise.
pub const NOISE_PROBE: usize = 4;

impl Default for Options {
    fn default() -> Options {
        Options {
            steps: 33,
            repeats: 1,
            smoothing: SMOOTHING_DEFAULT,
            refine_passes: REFINE_DEFAULT,
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
    /// Estimated noise of one averaged ramp reading, per channel, in the
    /// same units as `curves`. Zero when smoothing is off.
    pub noise: [Noise; 3],
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
    let measure = |rig: &mut R, patch: Rgb| measure_mean(rig, patch, options.repeats);
    let black = measure(rig, [0.0, 0.0, 0.0])?;
    let red = measure(rig, [1.0, 0.0, 0.0])?;
    let green = measure(rig, [0.0, 1.0, 0.0])?;
    let blue = measure(rig, [0.0, 0.0, 1.0])?;
    let (r, g, b) = (sub(red, black), sub(green, black), sub(blue, black));
    let channel_matrix = Mat3::from_columns(r, g, b);
    let to_linear = channel_matrix
        .inverse()
        .ok_or(CalibrateError::DegenerateChannels)?;
    let noise = if options.smoothing {
        probe_noise(rig, &to_linear, options.repeats)?
    } else {
        [Noise::default(); 3]
    };

    let mut drives = vec![0.0];
    let mut linear: Vec<Vec3> = vec![[0.0; 3]];
    let mut white = black;
    for i in 1..steps {
        let s = i as f64 / (steps - 1) as f64;
        let shown = rig.quantize([s, s, s]);
        if shown[0] <= drives[drives.len() - 1] {
            continue; // more steps than the display has levels
        }
        let reading = measure(rig, shown)?;
        drives.push(shown[0]);
        linear.push(to_linear * sub(reading, black));
        white = reading;
    }

    let additivity_error = additivity_error(black, red, green, blue, white);

    let curve = |c: usize| {
        let ys: Vec<f64> = linear.iter().map(|l| l[c]).collect();
        ToneCurve::fit(&drives, &ys, noise[c])
    };
    Ok(Characterization {
        trust,
        black,
        channel_matrix,
        curves: [curve(0)?, curve(1)?, curve(2)?],
        additivity_error,
        noise,
    })
}

/// How far the `white` reading differs from what `red`, `green` and `blue`
/// add up to, relative to white. All five are raw readings, black included,
/// so black is subtracted once from each channel.
pub fn additivity_error(black: Vec3, red: Vec3, green: Vec3, blue: Vec3, white: Vec3) -> f64 {
    let predicted = sub(add(add(red, green), blue), scale(black, 2.0));
    norm(sub(white, predicted)) / norm(sub(white, black))
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

/// Show the grays produced by `curves`, measure them and return corrected
/// curves. This removes errors the characterization cannot see: channels that
/// interact, detail lost to smoothing, wiggles in the panel's own response.
/// Like [`characterize`], it needs the GPU curves reset to identity, because
/// it shows the calibrated drive values directly.
pub fn refine<R: Rig + ?Sized>(
    rig: &mut R,
    ch: &Characterization,
    curves: &CalibrationCurves,
    target: Target,
    options: &Options,
) -> Result<CalibrationCurves, CalibrateError> {
    let steps = options.steps;
    if steps < 2 || options.repeats == 0 {
        return Err(CalibrateError::InvalidOptions(
            "steps must be at least 2 and repeats at least 1",
        ));
    }
    let amounts = white_channel_amounts(ch, target.white)?;
    let to_linear = ch
        .channel_matrix
        .inverse()
        .ok_or(CalibrateError::DegenerateChannels)?;

    // Drive correction per channel at each ramp level; black stays black.
    let mut deltas = [
        vec![0.0; steps - 1],
        vec![0.0; steps - 1],
        vec![0.0; steps - 1],
    ];
    let mut weights = [
        vec![0.0; steps - 1],
        vec![0.0; steps - 1],
        vec![0.0; steps - 1],
    ];
    for i in 1..steps {
        let v = i as f64 / (steps - 1) as f64;
        let drive = curves.apply([v, v, v]);
        let shown = rig.quantize(drive);
        let linear = to_linear * sub(measure_mean(rig, shown, options.repeats)?, ch.black);
        let level = target.curve.eval(v);
        for c in 0..3 {
            let curve = &ch.curves[c];
            let wanted = curve.eval(shown[c]) + amounts[c] * level - linear[c];
            deltas[c][i - 1] = curve.inverse(wanted) - drive[c];
            // A reading error σ moves the drive correction by σ / slope.
            let h = 1e-3;
            let slope = (curve.eval(shown[c] + h) - curve.eval(shown[c] - h)) / (2.0 * h);
            let var = ch.noise[c].variance(amounts[c] * level) / slope.max(1e-6).powi(2);
            weights[c][i - 1] = 1.0 / var.max(1e-18);
        }
    }

    let channels = [0, 1, 2].map(|c| {
        let mut d = vec![0.0];
        if options.smoothing && !ch.noise[c].is_zero() {
            d.extend(smooth_auto(&deltas[c], &weights[c]).0);
        } else {
            d.extend(&deltas[c]);
        }
        let lut = &curves.channels[c];
        let n = lut.len();
        let mut out = Vec::with_capacity(n);
        let mut prev = 0.0_f64;
        for (j, &old) in lut.iter().enumerate() {
            let pos = j as f64 / (n - 1) as f64 * (steps - 1) as f64;
            let k = (pos.floor() as usize).min(steps - 2);
            let t = pos - k as f64;
            let delta = d[k] * (1.0 - t) + d[k + 1] * t;
            // Keep the curve inside 0–1 and non-decreasing.
            prev = (old + delta).clamp(0.0, 1.0).max(prev);
            out.push(prev);
        }
        out
    });
    Ok(CalibrationCurves { channels })
}

/// Estimate per-channel noise of an averaged reading, from repeated single
/// readings of white (proportional part) and black (floor).
fn probe_noise<R: Rig + ?Sized>(
    rig: &mut R,
    to_linear: &Mat3,
    repeats: usize,
) -> Result<[Noise; 3], MeasureError> {
    let mut read = |patch: Rgb| -> Result<Vec<Vec3>, MeasureError> {
        (0..NOISE_PROBE)
            .map(|_| rig.measure(patch).map(|v| *to_linear * v))
            .collect()
    };
    let white = read([1.0; 3])?;
    let black = read([0.0; 3])?;
    let stats = |samples: &[Vec3], c: usize| {
        let n = samples.len() as f64;
        let mean = samples.iter().map(|s| s[c]).sum::<f64>() / n;
        let var = samples.iter().map(|s| (s[c] - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (mean, var.sqrt())
    };
    // Averaging `repeats` readings divides the standard deviation by √repeats.
    let k = 1.0 / (repeats as f64).sqrt();
    Ok([0, 1, 2].map(|c| {
        let (w_mean, w_sd) = stats(&white, c);
        let (_, floor) = stats(&black, c);
        let relative = (w_sd.powi(2) - floor.powi(2)).max(0.0).sqrt() / w_mean.abs().max(1e-12);
        Noise {
            relative: relative * k,
            floor: floor * k,
        }
    }))
}

fn measure_mean<R: Rig + ?Sized>(
    rig: &mut R,
    patch: Rgb,
    repeats: usize,
) -> Result<Vec3, MeasureError> {
    let mut sum = [0.0; 3];
    for _ in 0..repeats {
        sum = add(sum, rig.measure(patch)?);
    }
    Ok(scale(sum, 1.0 / repeats as f64))
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
    let mut curves = build_curves(&characterization, target, options.lut_size)?;
    for _ in 0..options.refine_passes {
        curves = refine(rig, &characterization, &curves, target, &options)?;
    }
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
    fn additivity_error_subtracts_black_once_per_channel() {
        let black = [1.0, 1.0, 1.0];
        let red = [41.0, 21.0, 3.0];
        let green = [36.0, 71.0, 12.0];
        let blue = [19.0, 8.0, 96.0];
        let additive_white = [94.0, 98.0, 109.0];
        let error = additivity_error(black, red, green, blue, additive_white);
        assert!(error < 1e-12, "additive panel reported {error}");

        // White 10% short of black + the three channels.
        let short_white = add(black, scale([93.0, 97.0, 108.0], 0.9));
        let error = additivity_error(black, red, green, blue, short_white);
        assert!((error - 0.1 / 0.9).abs() < 1e-12, "{error}");
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
