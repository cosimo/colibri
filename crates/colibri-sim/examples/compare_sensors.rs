//! Calibrate a simulated wide-gamut panel with three kinds of instrument and
//! report the result against ground truth.
//!
//!     cargo run -p colibri-sim --example compare_sensors

use colibri_core::calibrate::{calibrate, Options, Target, WhitePoint};
use colibri_core::color::D65;
use colibri_core::curve::TargetCurve;
use colibri_sim::{check_grays, SimDisplay, SimRig, SimSensor};

fn main() {
    let levels = [0.05, 0.1, 0.25, 0.5, 0.75, 0.9];
    let curve = TargetCurve::Gamma(2.2);

    print!("{:<34}", "gray level");
    levels.iter().for_each(|v| print!("{v:>7.2}"));
    println!("\n{:<34}  -- ΔE2000 vs ideal gray --", "");
    print_row("uncalibrated", &SimDisplay::wide_gamut(), curve, &levels);

    let sensors = [
        ("colorimeter", SimSensor::colorimeter(1)),
        ("aged colorimeter", SimSensor::aged_colorimeter(1)),
        ("phone camera (RAW)", SimSensor::phone_camera(1)),
    ];
    for (name, sensor) in &sensors {
        let mut rig = SimRig::new(SimDisplay::wide_gamut(), sensor.clone());
        let target = Target {
            white: WhitePoint::Native,
            curve,
        };
        let cal = calibrate(&mut rig, target, Options::default()).expect("calibration");
        rig.display.curves = Some(cal.curves);
        print_row(
            &format!("{name}, native white"),
            &rig.display,
            curve,
            &levels,
        );
    }

    println!("\nTargeting D65 (x 0.3127, y 0.3290):");
    for (name, sensor) in sensors {
        let mut rig = SimRig::new(SimDisplay::wide_gamut(), sensor);
        let target = Target {
            white: WhitePoint::Chromaticity(D65),
            curve,
        };
        match calibrate(&mut rig, target, Options::default()) {
            Ok(cal) => {
                rig.display.curves = Some(cal.curves);
                let xy = rig.display.true_xyz([1.0; 3]).chromaticity();
                println!(
                    "  {name:<20} true white x {:.4} y {:.4}  (off by {:.4})",
                    xy.x,
                    xy.y,
                    xy.distance(D65)
                );
            }
            Err(e) => println!("  {name:<20} refused: {e}"),
        }
    }
}

fn print_row(name: &str, display: &SimDisplay, curve: TargetCurve, levels: &[f64]) {
    print!("{name:<34}");
    for check in check_grays(display, curve, levels) {
        print!("{:>7.2}", check.delta_e);
    }
    println!();
}
