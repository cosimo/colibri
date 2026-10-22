//! Calibrate the simulated wide-gamut panel to D65 / sRGB and write an ICC v4
//! profile with vcgt and an MHC2 tag that emulates sRGB.
//!
//!     cargo run -p colibri-sim --example write_profile -- out.icc

use colibri_core::calibrate::{calibrate, Options, Target, WhitePoint};
use colibri_core::color::D65;
use colibri_core::curve::TargetCurve;
use colibri_core::icc::{Colorimetry, DisplayProfile, Mhc2, Mhc2Gamut, SRGB};
use colibri_sim::{SimDisplay, SimRig, SimSensor};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "colibri-sim.icc".into());
    let target = Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Srgb,
    };
    let mut rig = SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1));
    let cal = calibrate(&mut rig, target, Options::default()).expect("calibration");
    let colorimetry = Colorimetry::calibrated(&cal.characterization, target.white).unwrap();
    let gamut = Mhc2Gamut::Emulate {
        source: SRGB,
        adapt_white: true,
    };
    let mhc2 = Mhc2::new(&colorimetry, &cal.curves, gamut, 1024).unwrap();
    let profile = DisplayProfile {
        description: "colibri simulated wide-gamut panel (sRGB emulation)".into(),
        copyright: "No copyright, use freely".into(),
        created: [2026, 10, 22, 0, 0, 0],
        colorimetry,
        tone: target.curve,
        calibration: Some(cal.curves),
        mhc2: Some(mhc2),
    };
    let bytes = profile.to_bytes().expect("profile");
    std::fs::write(&path, &bytes).expect("write profile");
    println!("wrote {} bytes to {path}", bytes.len());
}
