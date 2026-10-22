//! Emulation of Windows' SDR colour pipeline with an MHC2 profile, as
//! reverse-engineered by MHC2Gen (github.com/dantmnf/MHC2):
//! `LUT(regamma(XYZ→sRGB · M · sRGB→XYZ · degamma(input)))`, with sRGB
//! degamma and regamma. Lets profile maths be tested end to end; it is not
//! evidence of how a given Windows build behaves.

use colibri_core::curve::TargetCurve;
use colibri_core::icc::{Mhc2, SRGB};
use colibri_core::rig::Rgb;

/// Drive values Windows would send to the panel for desktop pixel `input`.
pub fn sdr_pipeline(mhc2: &Mhc2, input: Rgb) -> Rgb {
    let to_xyz = SRGB.rgb_to_xyz().expect("sRGB matrix");
    let from_xyz = to_xyz.inverse().expect("sRGB matrix inverse");
    let linear = input.map(|v| TargetCurve::Srgb.eval(v));
    let out = from_xyz * (mhc2.matrix3() * (to_xyz * linear));
    let encoded = out.map(|v| srgb_encode(v.clamp(0.0, 1.0)));
    [0, 1, 2].map(|c| lut(&mhc2.luts[c], encoded[c]))
}

fn srgb_encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear interpolation between LUT entries.
fn lut(table: &[f64], x: f64) -> f64 {
    let pos = x * (table.len() - 1) as f64;
    let i = (pos.floor() as usize).min(table.len() - 2);
    let t = pos - i as f64;
    table[i] * (1.0 - t) + table[i + 1] * t
}
