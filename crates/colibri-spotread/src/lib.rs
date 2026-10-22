//! colibri-spotread: readings from a Spyder4 through ArgyllCMS `spotread`,
//! one process per reading.
//!
//! This is a test stand-in until colibri can drive the sensor itself. It sits
//! in its own crate so it can be deleted when the real driver lands.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use colibri_core::math::Vec3;
use colibri_core::rig::{Instrument, MeasureError, Trust};

/// Environment variable with the path to `spotread`.
pub const PROGRAM_VAR: &str = "COLIBRI_SPOTREAD";
/// Environment variable with extra `spotread` arguments, such as `-y e`.
pub const ARGS_VAR: &str = "COLIBRI_SPOTREAD_ARGS";

/// The display type matters a lot for the Spyder4. On a MacBook M4 screen,
/// `-y e` (LCD, White LED Backlight) read 0.005 closer to D65 in x than `-y B`.
const DEFAULT_ARGS: &str = "-y e";

/// The Spyder4 can take several seconds on dark patches. Longer than this
/// means `spotread` is stuck, for example waiting for a sensor that was
/// unplugged.
const READING_TIMEOUT: Duration = Duration::from_secs(60);

/// How often `run_with_timeout` checks whether the process has exited.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The Spyder4 read through `spotread`.
pub struct SpotreadInstrument {
    pub program: PathBuf,
    pub extra_args: Vec<String>,
    pub timeout: Duration,
}

impl SpotreadInstrument {
    /// Settings from [`PROGRAM_VAR`] and [`ARGS_VAR`], with defaults for the
    /// ones not set.
    pub fn from_env() -> SpotreadInstrument {
        let program = std::env::var(PROGRAM_VAR).ok();
        let args = std::env::var(ARGS_VAR).ok();
        SpotreadInstrument::from_values(program.as_deref(), args.as_deref())
    }

    /// `spotread` from `PATH` and `-y e` when not given. An empty `args`
    /// means no extra arguments at all.
    pub fn from_values(program: Option<&str>, args: Option<&str>) -> SpotreadInstrument {
        let program = unquoted(program.unwrap_or("spotread"));
        let args = unquoted(args.unwrap_or(DEFAULT_ARGS));
        SpotreadInstrument {
            program: PathBuf::from(program),
            extra_args: args.split_whitespace().map(String::from).collect(),
            timeout: READING_TIMEOUT,
        }
    }

    /// Runs `spotread -?` and returns the first Spyder port name.
    pub fn find_spyder(&self) -> Result<String, String> {
        let mut command = Command::new(&self.program);
        command.arg("-?");
        // Argyll exits with an error code after printing its usage, so only
        // the output counts.
        let usage = run_with_timeout(command, self.timeout)?;
        let Some(port) = spyder_ports(&usage).into_iter().next() else {
            let program = self.program.display();
            return Err(format!(
                "{program} lists no Spyder: check it is plugged in and uses Argyll's USB driver"
            ));
        };
        Ok(port)
    }
}

impl Instrument for SpotreadInstrument {
    fn read(&mut self) -> Result<Vec3, MeasureError> {
        let mut command = Command::new(&self.program);
        // -e: emissive mode, absolute XYZ in cd/m². -O: read straight away
        // without waiting for a key, and exit after one reading.
        command.args(["-e", "-O", "-x"]).args(&self.extra_args);
        let output = run_with_timeout(command, self.timeout).map_err(MeasureError::Instrument)?;
        let Some(xyz) = parse_xyz(&output) else {
            let tail = last_lines(&output, 5);
            return Err(MeasureError::Instrument(format!(
                "spotread gave no reading:\n{tail}"
            )));
        };
        Ok(xyz)
    }

    fn trust(&self) -> Trust {
        Trust::Absolute
    }
}

/// `value` without the double quotes around it. cmd keeps them as part of
/// the value in `set COLIBRI_SPOTREAD_ARGS="-y e"`.
fn unquoted(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
}

/// XYZ from the last `Result is XYZ: X Y Z, Yxy: ...` line of `spotread`
/// output.
pub fn parse_xyz(output: &str) -> Option<Vec3> {
    let line = output
        .lines()
        .rev()
        .find(|l| l.contains("Result is XYZ:"))?;
    let (_, after) = line.split_once("Result is XYZ:")?;
    let (numbers, _) = after.split_once(',')?;
    let values: Vec<f64> = numbers
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    values.try_into().ok()
}

/// Port names containing `Spyder`, from the `-c` list in `spotread -?`:
///
/// ```text
///     1 = 'usb:/bus0/dev1/ (Datacolor Spyder4)'
/// ```
pub fn spyder_ports(usage: &str) -> Vec<String> {
    let mut ports = Vec::new();
    for line in usage.lines() {
        let Some((number, name)) = line.trim().split_once(" = ") else {
            continue;
        };
        let is_port_line = number.parse::<u32>().is_ok();
        let name = name.trim_matches('\'');
        if is_port_line && name.contains("Spyder") {
            ports.push(name.to_string());
        }
    }
    ports
}

/// The last `n` lines of `text`, for error messages.
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Runs `command` and returns stdout then stderr, as one string, whatever its
/// exit code. A process still running after `timeout` is killed.
pub fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW, so no console window flashes up for each reading.
        // See: https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
        command.creation_flags(0x0800_0000);
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|e| format!("can't start {program}: {e}"))?;
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let output = collect(stdout) + &collect(stderr);
    if timed_out {
        let seconds = timeout.as_secs_f64();
        let tail = last_lines(&output, 5);
        return Err(format!("{program} timed out after {seconds}s:\n{tail}"));
    }
    Ok(output)
}

/// Reads a child's pipe on its own thread, so neither pipe can fill up and
/// block the child while the other is being read.
fn read_in_background<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn collect(reader: JoinHandle<String>) -> String {
    reader.join().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Instant;

    // The samples follow the printf formats in Argyll 3.5.0
    // spectro/spotread.c. They should be replaced by output captured from
    // the Spyder4 on Windows.
    const READING: &str = "\
Place instrument on spot to be measured,
Hit ESC or Q to exit, any other key to take a reading:

 Result is XYZ: 532.130374 562.465032 591.506450, Yxy: 562.465032 0.315598 0.333589
";

    const USAGE: &str = "\
usage: spotread [-options] [logfile]
 -v                   Verbose mode
 -c listno            Set instrument port from the following list (default 1)
    1 = 'usb:/bus0/dev1/ (Datacolor Spyder4)'
    2 = 'COM3'
 -t                   Use transmission measurement mode
";

    #[test]
    fn parse_xyz_reads_result_line() {
        assert_eq!(
            parse_xyz(READING),
            Some([532.130374, 562.465032, 591.506450])
        );
    }

    #[test]
    fn parse_xyz_accepts_black() {
        let black = " Result is XYZ: 0.000000 0.000000 0.000000, Yxy: 0.000000 0.000000 0.000000\n";
        assert_eq!(
            parse_xyz(black),
            Some([0.0, 0.0, 0.0]),
            "black is a valid reading"
        );
    }

    #[test]
    fn parse_xyz_none_without_result() {
        assert_eq!(
            parse_xyz("spotread: Error - No instrument at port 1\n"),
            None
        );
    }

    #[test]
    fn spyder_ports_lists_only_spyders() {
        assert_eq!(
            spyder_ports(USAGE),
            vec!["usb:/bus0/dev1/ (Datacolor Spyder4)"]
        );
    }

    #[test]
    fn from_values_defaults_and_splits() {
        let default = SpotreadInstrument::from_values(None, None);
        assert_eq!(default.program, PathBuf::from("spotread"));
        assert_eq!(default.extra_args, ["-y", "e"]);
        assert_eq!(default.timeout, Duration::from_secs(60));

        let custom = SpotreadInstrument::from_values(
            Some(r"D:\prg\argyll\bin\spotread.exe"),
            Some("-y B  -Q 1964_10"),
        );
        assert_eq!(
            custom.program,
            PathBuf::from(r"D:\prg\argyll\bin\spotread.exe")
        );
        assert_eq!(custom.extra_args, ["-y", "B", "-Q", "1964_10"]);
    }

    #[test]
    fn from_values_drops_quotes_kept_by_cmd() {
        // cmd's `set COLIBRI_SPOTREAD_ARGS="-y e"` keeps the quotes in the
        // value, and spotread took `"-y` for a log file name.
        let quoted = SpotreadInstrument::from_values(
            Some(r#""C:\Program Files\Argyll\bin\spotread.exe""#),
            Some(r#""-y e""#),
        );

        assert_eq!(
            quoted.program,
            PathBuf::from(r"C:\Program Files\Argyll\bin\spotread.exe")
        );
        assert_eq!(
            quoted.extra_args,
            ["-y", "e"],
            "quoted args reached spotread"
        );
    }

    #[test]
    fn last_lines_keeps_the_tail() {
        assert_eq!(last_lines("a\nb\nc\nd\n", 2), "c\nd");
    }

    /// A fake `spotread` that only answers when called with the one-shot
    /// arguments and the default display type.
    #[cfg(unix)]
    fn fake_spotread(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("colibri-spotread-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join(name);
        let body = r#"[ "$*" = "-e -O -x -y e" ] || { echo "unexpected args: $*" >&2; exit 1; }
echo ' Result is XYZ: 1.500000 2.500000 3.500000, Yxy: 2.500000 0.200000 0.333333'"#;
        // The script is written by a child process, so no file descriptor
        // open for writing can leak into the processes other tests fork,
        // which would make running it fail with "text file busy".
        let status = Command::new("sh")
            .args([
                "-c",
                "printf '#!/bin/sh\\n%s\\n' \"$1\" > \"$2\" && chmod +x \"$2\"",
            ])
            .args(["sh", body, script.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        script
    }

    #[cfg(unix)]
    #[test]
    fn read_runs_one_shot_spotread() {
        let script = fake_spotread("one-shot");
        let mut sensor = SpotreadInstrument::from_values(script.to_str(), None);

        assert_eq!(sensor.read().unwrap(), [1.5, 2.5, 3.5]);
    }

    #[cfg(unix)]
    #[test]
    fn read_reports_output_without_result() {
        let script = fake_spotread("wrong-args");
        let mut sensor = SpotreadInstrument::from_values(script.to_str(), Some("-y B"));

        let err = sensor.read().unwrap_err().to_string();

        assert!(err.contains("spotread gave no reading"), "{err}");
        assert!(err.contains("unexpected args: -e -O -x -y B"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_kills_hung_process() {
        let started = Instant::now();
        let mut sleep = Command::new("sleep");
        sleep.arg("5");
        let err = run_with_timeout(sleep, Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "process was not killed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_returns_stdout_and_stderr() {
        let mut sh = Command::new("sh");
        sh.args(["-c", "echo out; echo err >&2"]);
        let output = run_with_timeout(sh, Duration::from_secs(5)).unwrap();
        assert!(output.contains("out") && output.contains("err"), "{output}");
    }
}
