//! colibri command-line tool.

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use colibri_core::calibrate::{additivity_error, calibrate, Options, Target, WhitePoint};
use colibri_core::color::{Xyz, D65};
use colibri_core::curve::TargetCurve;
use colibri_core::icc::{Colorimetry, DisplayProfile, Mhc2, Mhc2Gamut, SRGB};
use colibri_core::math::Vec3;
use colibri_core::rig::{InstrumentRig, MeasureError, Rig};
use colibri_sim::{check_grays, SimDisplay, SimRig, SimSensor};
use colibri_spotread::SpotreadInstrument;
use colibri_windows::{Monitor, PatchWindow};

const USAGE: &str = "\
usage:
  colibri displays
      List monitors.
  colibri patch [--monitor N] [--size F] [R G B]
      Open a patch window and show 8-bit colours typed on stdin as `R G B`
      (0-255), starting with R G B if given. An empty line or `q` quits.
      --size is the patch side as a fraction of the screen height (0.5).
  colibri gamma (show|reset) [--monitor N]
      Show or reset to identity the GPU gamma ramp.
  colibri check [--monitor N] [--size F]
      Measure white, black, gray, red, green, blue and white again with the
      Spyder4 through Argyll's spotread, and print the readings with the
      additivity and repeatability checks. COLIBRI_SPOTREAD sets the path
      to spotread (default: from PATH), COLIBRI_SPOTREAD_ARGS its extra
      arguments (default: -y e).
  colibri simulate [--out FILE]
      Calibrate a simulated wide-gamut panel to D65/sRGB, report the result
      and optionally write its ICC profile with an MHC2 tag.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("displays") => displays(),
        Some("patch") => patch(&args[1..]),
        Some("gamma") => gamma(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("simulate") => simulate(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Flags of the form `--name value`, and the remaining positional arguments.
struct Args {
    flags: Vec<(String, String)>,
    positional: Vec<String>,
}

impl Args {
    fn parse(args: &[String], known: &[&str]) -> Result<Args, String> {
        let mut flags = Vec::new();
        let mut positional = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if !known.contains(&name) {
                    return Err(format!("unknown option --{name}\n\n{USAGE}"));
                }
                let value = it.next().ok_or(format!("--{name} needs a value"))?;
                flags.push((name.to_string(), value.clone()));
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Args { flags, positional })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn get<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.value(name) {
            Some(v) => v.parse().map_err(|_| format!("invalid --{name}: {v}")),
            None => Ok(default),
        }
    }
}

fn monitor(args: &Args) -> Result<Monitor, String> {
    let list = colibri_windows::monitors().map_err(|e| e.to_string())?;
    let primary = list.iter().position(|m| m.primary).unwrap_or(0);
    let index: usize = args.get("monitor", primary)?;
    list.get(index)
        .cloned()
        .ok_or(format!("no monitor {index}; see `colibri displays`"))
}

fn displays() -> Result<(), String> {
    for m in colibri_windows::monitors().map_err(|e| e.to_string())? {
        println!(
            "{}: {} {}x{} at ({}, {}){}",
            m.index,
            m.device,
            m.width,
            m.height,
            m.left,
            m.top,
            if m.primary { " primary" } else { "" }
        );
    }
    Ok(())
}

fn parse_rgb(parts: &[&str]) -> Result<[u8; 3], String> {
    let values: Vec<u8> = parts
        .iter()
        .map(|p| {
            p.parse::<u8>()
                .map_err(|_| format!("not a 0-255 value: {p}"))
        })
        .collect::<Result<_, _>>()?;
    values
        .try_into()
        .map_err(|_| "expected three values: R G B".to_string())
}

fn patch(raw: &[String]) -> Result<(), String> {
    let args = Args::parse(raw, &["monitor", "size"])?;
    let monitor = monitor(&args)?;
    let mut window =
        PatchWindow::open(&monitor, args.get("size", 0.5)?).map_err(|e| e.to_string())?;
    let mut show = |rgb: [u8; 3]| -> Result<(), String> {
        window.show_rgb8(rgb).map_err(|e| e.to_string())?;
        println!("showing {} {} {}", rgb[0], rgb[1], rgb[2]);
        Ok(())
    };
    if !args.positional.is_empty() {
        let parts: Vec<&str> = args.positional.iter().map(String::as_str).collect();
        show(parse_rgb(&parts)?)?;
    }
    let stdin = std::io::stdin();
    loop {
        print!("R G B> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(());
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() || parts == ["q"] {
            return Ok(());
        }
        match parse_rgb(&parts) {
            Ok(rgb) => show(rgb)?,
            Err(e) => eprintln!("{e}"),
        }
    }
}

fn gamma(raw: &[String]) -> Result<(), String> {
    let args = Args::parse(raw, &["monitor"])?;
    let monitor = monitor(&args)?;
    match args.positional.first().map(String::as_str) {
        Some("show") => {
            let ramp = colibri_windows::gamma_ramp(&monitor).map_err(|e| e.to_string())?;
            let identity = colibri_windows::identity_ramp();
            for i in (0..256).step_by(32).chain([255]) {
                println!("{i:3}: {:5} {:5} {:5}", ramp[0][i], ramp[1][i], ramp[2][i]);
            }
            println!("identity: {}", ramp == identity);
            Ok(())
        }
        Some("reset") => {
            colibri_windows::set_gamma_ramp(&monitor, &colibri_windows::identity_ramp())
                .map_err(|e| e.to_string())?;
            println!("gamma ramp of {} reset to identity", monitor.device);
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// The patches of the patch window test in `docs/windows-spyder4-setup.md`,
/// in the order they are measured. White comes first and last, to see
/// whether the panel drifts during the run.
const CHECK_PATCHES: [(&str, [u8; 3]); 7] = [
    ("white", [255, 255, 255]),
    ("black", [0, 0, 0]),
    ("gray", [128, 128, 128]),
    ("red", [255, 0, 0]),
    ("green", [0, 255, 0]),
    ("blue", [0, 0, 255]),
    ("white", [255, 255, 255]),
];

/// The panel needs a moment after the patch changes. spotread then takes a
/// second or more to open the sensor before reading, so this can be short.
const SPOTREAD_SETTLE: Duration = Duration::from_millis(300);

fn check(raw: &[String]) -> Result<(), String> {
    let args = Args::parse(raw, &["monitor", "size"])?;
    let monitor = monitor(&args)?;
    let size = args.get("size", 0.5)?;
    let sensor = SpotreadInstrument::from_env();
    let port = sensor.find_spyder()?;
    let spotread_args = sensor.extra_args.join(" ");
    println!("sensor: {port}, spotread args: {spotread_args}");

    let ramp = colibri_windows::gamma_ramp(&monitor).map_err(|e| e.to_string())?;
    if ramp != colibri_windows::identity_ramp() {
        let index = monitor.index;
        eprintln!(
            "warning: monitor {index} has a gamma ramp loaded, so readings won't be the \
             panel's own; run `colibri gamma reset --monitor {index}`"
        );
    }

    println!(
        "{:<6} {:>3} {:>3} {:>3} {:>9} {:>8} {:>7} {:>7}",
        "patch", "R", "G", "B", "Y cd/m²", "of white", "x", "y"
    );
    let readings = measure_check_patches(&monitor, size, sensor).map_err(|e| e.to_string())?;

    let [white, black, _gray, red, green, blue, white_again] = readings;
    let additivity = additivity_error(black, red, green, blue, white) * 100.0;
    println!("additivity error {additivity:.2}%: red + green + blue against white");

    let drift = (white_again[1] / white[1] - 1.0) * 100.0;
    let first_xy = Xyz::from_vec(white).chromaticity();
    let last_xy = Xyz::from_vec(white_again).chromaticity();
    let xy_shift = (last_xy.x - first_xy.x).hypot(last_xy.y - first_xy.y);
    println!("white drift: Y {drift:+.2}%, xy {xy_shift:.4}");
    Ok(())
}

/// Shows each of [`CHECK_PATCHES`] in a patch window and reads it, printing
/// a table row per reading. The window closes before returning.
fn measure_check_patches(
    monitor: &Monitor,
    size: f64,
    sensor: SpotreadInstrument,
) -> Result<[Vec3; 7], MeasureError> {
    let mut window =
        PatchWindow::open(monitor, size).map_err(|e| MeasureError::Instrument(e.to_string()))?;
    window
        .show_rgb8([255, 255, 255])
        .map_err(|e| MeasureError::Instrument(e.to_string()))?;
    window.wait_for_start("Put the sensor on the square and press Space")?;
    let status = window.status_line();
    let mut rig = InstrumentRig {
        display: window,
        instrument: sensor,
        settle: SPOTREAD_SETTLE,
    };

    let mut readings = [[0.0; 3]; 7];
    for (i, (name, rgb)) in CHECK_PATCHES.iter().enumerate() {
        let total = CHECK_PATCHES.len();
        status.set(&format!("Patch {} of {total} · Esc cancels", i + 1));
        let reading = rig.measure(rgb.map(|v| v as f64 / 255.0))?;
        readings[i] = reading;

        let [r, g, b] = rgb;
        let luminance = reading[1];
        let of_white = luminance / readings[0][1] * 100.0;
        let (x, y) = chromaticity_text(reading);
        println!("{name:<6} {r:>3} {g:>3} {b:>3} {luminance:>9.3} {of_white:>7.1}% {x:>7} {y:>7}");
    }
    Ok(readings)
}

/// x and y with four decimals, or dashes for a reading with no light in it.
fn chromaticity_text(xyz: Vec3) -> (String, String) {
    if xyz.iter().sum::<f64>() <= 0.0 {
        return ("-".into(), "-".into());
    }
    let xy = Xyz::from_vec(xyz).chromaticity();
    (format!("{:.4}", xy.x), format!("{:.4}", xy.y))
}

fn simulate(raw: &[String]) -> Result<(), String> {
    let args = Args::parse(raw, &["out"])?;
    let target = Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Srgb,
    };
    let mut rig = SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1));
    let cal = calibrate(&mut rig, target, Options::default()).map_err(|e| e.to_string())?;
    println!(
        "measured {} patches; additivity error {:.2}%",
        rig.measurements,
        cal.characterization.additivity_error * 100.0
    );
    let mut shown = rig.display.clone();
    shown.curves = Some(cal.curves.clone());
    let white = shown.true_xyz([1.0; 3]);
    let xy = white.chromaticity();
    println!("white: x {:.4} y {:.4}, {:.1} cd/m²", xy.x, xy.y, white.y);
    for g in check_grays(&shown, target.curve, &[0.1, 0.25, 0.5, 0.75, 0.9]) {
        println!("gray {:.2}: ΔE2000 {:.2}", g.level, g.delta_e);
    }
    let Some(path) = args.value("out") else {
        return Ok(());
    };
    let colorimetry =
        Colorimetry::calibrated(&cal.characterization, target.white).map_err(|e| e.to_string())?;
    let gamut = Mhc2Gamut::Emulate {
        source: SRGB,
        adapt_white: true,
    };
    let mhc2 = Mhc2::new(&colorimetry, &cal.curves, gamut, 1024).map_err(|e| e.to_string())?;
    let profile = DisplayProfile {
        description: "colibri simulated wide-gamut panel".into(),
        copyright: "No copyright, use freely".into(),
        created: [2026, 1, 1, 0, 0, 0],
        colorimetry,
        tone: target.curve,
        calibration: Some(cal.curves),
        mhc2: Some(mhc2),
    };
    let bytes = profile.to_bytes().map_err(|e| e.to_string())?;
    std::fs::write(path, &bytes).map_err(|e| format!("{path}: {e}"))?;
    println!("wrote {path} ({} bytes)", bytes.len());
    Ok(())
}
