//! Calibrate, write an MHC2 profile, read it back, run desktop colours through
//! the emulated Windows pipeline and check what the simulated panel shows.

use colibri_core::calibrate::{calibrate, Options, Target, WhitePoint};
use colibri_core::color::{delta_e_2000, Xyz, D65};
use colibri_core::curve::TargetCurve;
use colibri_core::icc::{Colorimetry, DisplayProfile, Mhc2, Mhc2Gamut, ParsedProfile, SRGB};
use colibri_core::rig::Rgb;
use colibri_sim::windows::sdr_pipeline;
use colibri_sim::{SimDisplay, SimRig, SimSensor};

const TARGET: Target = Target {
    white: WhitePoint::Chromaticity(D65),
    curve: TargetCurve::Srgb,
};

const PATCHES: [Rgb; 9] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.5, 0.5, 0.5],
    [0.2, 0.2, 0.2],
    [0.8, 0.4, 0.2],
    [0.2, 0.6, 0.9],
    [0.9, 0.8, 0.3],
];

/// Calibrate the wide-gamut panel and return it with its MHC2 tag, after a
/// round trip through a complete profile.
fn profiled(gamut: Mhc2Gamut) -> (SimDisplay, Mhc2) {
    let mut rig = SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(11));
    let options = Options {
        repeats: 4,
        ..Options::default()
    };
    let cal = calibrate(&mut rig, TARGET, options).unwrap();
    let colorimetry = Colorimetry::calibrated(&cal.characterization, TARGET.white).unwrap();
    let mhc2 = Mhc2::new(&colorimetry, &cal.curves, gamut, 1024).unwrap();
    let profile = DisplayProfile {
        description: "simulated wide-gamut panel".into(),
        copyright: "No copyright".into(),
        created: [2026, 10, 22, 0, 0, 0],
        colorimetry,
        tone: TARGET.curve,
        calibration: Some(cal.curves),
        mhc2: Some(mhc2),
    };
    let bytes = profile.to_bytes().unwrap();
    let parsed = ParsedProfile::parse(&bytes).unwrap();
    assert!(parsed.profile_id_matches());
    let mhc2 = Mhc2::parse(parsed.tag(b"MHC2").unwrap()).unwrap();
    // Windows applies the MHC2 LUT itself; nothing is loaded into the GPU ramp.
    (rig.display, mhc2)
}

/// What the panel shows for desktop pixel `rgb`.
fn shown(display: &SimDisplay, mhc2: &Mhc2, rgb: Rgb) -> Xyz {
    display.true_xyz(sdr_pipeline(mhc2, rgb))
}

/// What an sRGB pixel should look like: sRGB colorimetry scaled to the
/// calibrated white, on top of the black.
fn srgb_expected(display: &SimDisplay, mhc2: &Mhc2, rgb: Rgb) -> Xyz {
    let white = shown(display, mhc2, [1.0; 3]);
    let black = shown(display, mhc2, [0.0; 3]);
    let lin = rgb.map(|v| TargetCurve::Srgb.eval(v));
    let xyz = SRGB.rgb_to_xyz().unwrap() * lin;
    let k = white.y - black.y;
    Xyz::new(
        black.x + xyz[0] * k,
        black.y + xyz[1] * k,
        black.z + xyz[2] * k,
    )
}

fn delta_e(display: &SimDisplay, mhc2: &Mhc2, rgb: Rgb) -> f64 {
    let white = shown(display, mhc2, [1.0; 3]);
    let expected = srgb_expected(display, mhc2, rgb);
    delta_e_2000(
        shown(display, mhc2, rgb).to_lab(white),
        expected.to_lab(white),
    )
}

#[test]
fn srgb_emulation_shows_srgb_colours_on_a_wide_gamut_panel() {
    let (display, mhc2) = profiled(Mhc2Gamut::Emulate {
        source: SRGB,
        adapt_white: true,
    });
    let white = shown(&display, &mhc2, [1.0; 3]).chromaticity();
    assert!(white.distance(D65) < 0.002, "white {white:?}");
    for rgb in PATCHES {
        let de = delta_e(&display, &mhc2, rgb);
        assert!(de < 1.0, "{rgb:?}: ΔE2000 {de}");
    }
}

#[test]
fn without_emulation_srgb_colours_are_oversaturated() {
    // Guards the test above: the panel's native gamut really is far from sRGB.
    let (display, mhc2) = profiled(Mhc2Gamut::Native);
    let worst = PATCHES
        .iter()
        .map(|&rgb| delta_e(&display, &mhc2, rgb))
        .fold(0.0, f64::max);
    assert!(worst > 10.0, "worst ΔE2000 {worst}");
}

#[test]
fn native_gamut_still_calibrates_grays() {
    let (display, mhc2) = profiled(Mhc2Gamut::Native);
    for v in [0.1, 0.25, 0.5, 0.75] {
        let de = delta_e(&display, &mhc2, [v; 3]);
        assert!(de < 1.0, "gray {v}: ΔE2000 {de}");
    }
}
