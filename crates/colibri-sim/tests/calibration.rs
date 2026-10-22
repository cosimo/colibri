//! End-to-end calibration against simulated hardware, checked with exact colorimetry.

use colibri_core::calibrate::{calibrate, CalibrateError, Options, Target, WhitePoint};
use colibri_core::color::D65;
use colibri_core::curve::TargetCurve;
use colibri_sim::{check_grays, SimDisplay, SimRig, SimSensor};

const LEVELS: [f64; 9] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];

const NATIVE_22: Target = Target {
    white: WhitePoint::Native,
    curve: TargetCurve::Gamma(2.2),
};

fn worst_delta_e(display: &SimDisplay, curve: TargetCurve) -> f64 {
    check_grays(display, curve, &LEVELS)
        .iter()
        .map(|c| c.delta_e)
        .fold(0.0, f64::max)
}

/// ΔE2000 below 1 is not visible side by side.
const INVISIBLE: f64 = 1.0;

fn calibrated(rig: SimRig, target: Target) -> SimRig {
    calibrated_with(rig, target, Options::default())
}

fn calibrated_with(mut rig: SimRig, target: Target, options: Options) -> SimRig {
    let cal = calibrate(&mut rig, target, options).expect("calibration succeeds");
    assert!(cal.characterization.additivity_error < 0.01);
    rig.display.curves = Some(cal.curves);
    rig
}

#[test]
fn uncalibrated_panel_is_visibly_off() {
    // Guards against the other tests passing trivially.
    assert!(worst_delta_e(&SimDisplay::wide_gamut(), TargetCurve::Gamma(2.2)) > 3.0);
}

#[test]
fn colorimeter_calibrates_grays_at_native_white() {
    let rig = calibrated(
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1)),
        NATIVE_22,
    );
    let worst = worst_delta_e(&rig.display, TargetCurve::Gamma(2.2));
    assert!(worst < INVISIBLE, "worst ΔE2000 {worst}");
}

#[test]
fn phone_camera_calibrates_grays_at_native_white() {
    // The camera's colour space is unknown to the algorithm; it only needs to be linear.
    let rig = calibrated(
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::phone_camera(2)),
        NATIVE_22,
    );
    let worst = worst_delta_e(&rig.display, TargetCurve::Gamma(2.2));
    assert!(worst < INVISIBLE, "worst ΔE2000 {worst}");
}

#[test]
fn aged_colorimeter_calibrates_grays_at_native_white() {
    // An old sensor is noisier, so average a few readings per patch.
    let options = Options {
        repeats: 4,
        ..Options::default()
    };
    let rig = calibrated_with(
        SimRig::new(SimDisplay::srgb_ips(), SimSensor::aged_colorimeter(3)),
        Target {
            white: WhitePoint::Native,
            curve: TargetCurve::Srgb,
        },
        options,
    );
    let worst = worst_delta_e(&rig.display, TargetCurve::Srgb);
    assert!(worst < INVISIBLE, "worst ΔE2000 {worst}");
}

#[test]
fn averaging_repeated_readings_reduces_noise() {
    let options = Options {
        repeats: 16,
        ..Options::default()
    };
    let rig = calibrated_with(
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(8)),
        NATIVE_22,
        options,
    );
    let worst = worst_delta_e(&rig.display, TargetCurve::Gamma(2.2));
    assert!(worst < 0.25, "worst ΔE2000 {worst}");
}

#[test]
fn noise_free_calibration_is_exact() {
    let sensor = SimSensor::phone_camera(9).with_noise(0.0, 0.0);
    let rig = calibrated(SimRig::new(SimDisplay::wide_gamut(), sensor), NATIVE_22);
    let worst = worst_delta_e(&rig.display, TargetCurve::Gamma(2.2));
    assert!(worst < 0.02, "worst ΔE2000 {worst}");
}

#[test]
fn colorimeter_reaches_d65() {
    let target = Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Gamma(2.2),
    };
    let rig = calibrated(
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(4)),
        target,
    );
    let white = rig.display.true_xyz([1.0; 3]).chromaticity();
    assert!(white.distance(D65) < 0.001, "white {white:?}");
    let native = SimDisplay::wide_gamut().true_xyz([1.0; 3]);
    let kept = rig.display.true_xyz([1.0; 3]).y / native.y;
    assert!(kept > 0.8, "kept only {kept} of native luminance");
    assert!(worst_delta_e(&rig.display, TargetCurve::Gamma(2.2)) < INVISIBLE);
}

#[test]
fn relative_instrument_refuses_absolute_white_before_measuring() {
    let mut rig = SimRig::new(SimDisplay::wide_gamut(), SimSensor::phone_camera(5));
    let target = Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Gamma(2.2),
    };
    let err = calibrate(&mut rig, target, Options::default()).unwrap_err();
    assert_eq!(err, CalibrateError::NeedsAbsoluteInstrument);
    assert_eq!(rig.measurements, 0);
}

#[test]
fn aged_colorimeter_trusted_as_absolute_misses_d65() {
    // Why trust levels exist: the aged sensor still claims to report XYZ.
    let target = Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Gamma(2.2),
    };
    let rig = calibrated(
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::aged_colorimeter(6)),
        target,
    );
    let white = rig.display.true_xyz([1.0; 3]).chromaticity();
    assert!(white.distance(D65) > 0.005, "white {white:?}");
}

#[test]
fn link_bit_depth_limits_shadow_accuracy() {
    // Rounding calibrated drive values shifts gray balance on its own: at 8 bits
    // that is close to visible, at 10 bits it is not. The sensor is noise-free
    // so only rounding is measured.
    let worst_at = |bits| {
        let mut display = SimDisplay::wide_gamut();
        display.output_bits = Some(bits);
        let sensor = SimSensor::colorimeter(7).with_noise(0.0, 0.0);
        let rig = calibrated(SimRig::new(display, sensor), NATIVE_22);
        worst_delta_e(&rig.display, TargetCurve::Gamma(2.2))
    };
    let (eight, ten) = (worst_at(8), worst_at(10));
    assert!(eight > 0.5, "8-bit worst ΔE2000 {eight}");
    assert!(ten < 0.3, "10-bit worst ΔE2000 {ten}");
}
