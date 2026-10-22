//! The calibration run, on a thread of its own, reporting progress back to
//! the UI over a channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use colibri_core::calibrate::{
    calibrate, measurement_count, Calibration, Options, Target, WhitePoint,
};
use colibri_core::color::D65;
use colibri_core::curve::TargetCurve;
use colibri_core::math::Vec3;
use colibri_core::rig::{InstrumentRig, MeasureError, Rgb, Rig, Trust};
use colibri_sim::{SimDisplay, SimRig, SimSensor};
use colibri_spotread::SpotreadInstrument;
use colibri_windows::{Monitor, PatchWindow};

use crate::flow::progress_text;

/// The panel needs a moment after the patch changes. spotread then takes a
/// second or more to open the sensor before reading, so this can be short.
const SETTLE: Duration = Duration::from_millis(200);

/// Side of the patch, as a fraction of the screen height.
const PATCH_SIZE: f64 = 0.5;

/// The simulator reads in no time. Slowing it down a little lets the
/// Measuring screen show its progress, about a second in all.
const SIMULATED_READING_TIME: Duration = Duration::from_millis(25);

/// What the patch window says while it waits for Space.
pub const START_PROMPT: &str = "Put the sensor on the square and press Space";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorChoice {
    Spotread,
    Simulated,
}

#[derive(Debug)]
pub enum WorkerMsg {
    /// The patch window is up, waiting for Space.
    WaitingForStart,
    /// The first reading is under way.
    Started,
    Progress {
        done: usize,
        total: usize,
    },
    /// The last message of every run. The calibration is boxed, it is much
    /// bigger than the other messages.
    Finished(Result<Box<Calibration>, String>),
}

/// D65 white and the sRGB tone curve, the only target for now.
pub fn target() -> Target {
    Target {
        white: WhitePoint::Chromaticity(D65),
        curve: TargetCurve::Srgb,
    }
}

/// Counts readings, reports each one, and stops when `cancel` is set.
pub struct ProgressRig<R> {
    inner: R,
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
    on_progress: Box<dyn FnMut(usize, usize) + Send>,
}

impl<R: Rig> ProgressRig<R> {
    pub fn new(
        inner: R,
        total: usize,
        cancel: Arc<AtomicBool>,
        on_progress: Box<dyn FnMut(usize, usize) + Send>,
    ) -> ProgressRig<R> {
        ProgressRig {
            inner,
            done: 0,
            total,
            cancel,
            on_progress,
        }
    }
}

impl<R: Rig> Rig for ProgressRig<R> {
    fn measure(&mut self, patch: Rgb) -> Result<Vec3, MeasureError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(MeasureError::Cancelled);
        }
        let reading = self.inner.measure(patch)?;
        self.done += 1;
        (self.on_progress)(self.done, self.total);
        Ok(reading)
    }

    fn trust(&self) -> Trust {
        self.inner.trust()
    }

    fn quantize(&self, patch: Rgb) -> Rgb {
        self.inner.quantize(patch)
    }
}

/// Sends messages to the UI, and wakes it up to read them.
#[derive(Clone)]
struct Messenger {
    tx: Sender<WorkerMsg>,
    // A Mutex makes the callback shareable between the progress callback
    // and the final message.
    wake: Arc<Mutex<Box<dyn Fn() + Send>>>,
}

impl Messenger {
    fn send(&self, msg: WorkerMsg) {
        // Nobody listening means the window is closing, and there is no one
        // left to tell.
        let _ = self.tx.send(msg);
        if let Ok(wake) = self.wake.lock() {
            wake();
        }
    }
}

/// Runs a calibration on a new thread. Every run ends with exactly one
/// [`WorkerMsg::Finished`], with errors as text.
pub fn spawn_calibration(
    sensor: SensorChoice,
    monitor: Option<Monitor>,
    cancel: Arc<AtomicBool>,
    tx: Sender<WorkerMsg>,
    wake: Box<dyn Fn() + Send>,
) -> JoinHandle<()> {
    let messenger = Messenger {
        tx,
        wake: Arc::new(Mutex::new(wake)),
    };
    std::thread::spawn(move || {
        let result = match sensor {
            SensorChoice::Simulated => run_simulated(cancel, messenger.clone()),
            SensorChoice::Spotread => run_spotread(monitor, cancel, messenger.clone()),
        };
        messenger.send(WorkerMsg::Finished(result.map(Box::new)));
    })
}

/// Calibrates the simulated wide-gamut panel. The real display is left
/// alone.
fn run_simulated(cancel: Arc<AtomicBool>, messenger: Messenger) -> Result<Calibration, String> {
    let options = Options::default();
    let total = measurement_count(&options);
    let sim = SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1));
    messenger.send(WorkerMsg::Started);
    let on_progress = move |done, total| {
        std::thread::sleep(SIMULATED_READING_TIME);
        messenger.send(WorkerMsg::Progress { done, total });
    };
    let mut rig = ProgressRig::new(sim, total, cancel, Box::new(on_progress));
    calibrate(&mut rig, target(), options).map_err(|e| e.to_string())
}

/// Calibrates `monitor` with the Spyder4 through spotread: a patch window
/// on the display, a wait for Space, then the readings, with the progress
/// in the patch window's status line.
fn run_spotread(
    monitor: Option<Monitor>,
    cancel: Arc<AtomicBool>,
    messenger: Messenger,
) -> Result<Calibration, String> {
    let Some(monitor) = monitor else {
        return Err("calibrating with the Spyder4 needs a display to measure".into());
    };
    let mut window = PatchWindow::open(&monitor, PATCH_SIZE).map_err(|e| e.to_string())?;
    window
        .show_rgb8([255, 255, 255])
        .map_err(|e| e.to_string())?;
    messenger.send(WorkerMsg::WaitingForStart);
    window
        .wait_for_start(START_PROMPT)
        .map_err(|e| e.to_string())?;

    let options = Options::default();
    let total = measurement_count(&options);
    let status = window.status_line();
    status.set(&progress_text(0, total, Duration::ZERO));
    messenger.send(WorkerMsg::Started);
    let started = Instant::now();
    let on_progress = move |done, total| {
        status.set(&progress_text(done, total, started.elapsed()));
        messenger.send(WorkerMsg::Progress { done, total });
    };
    let spotread = InstrumentRig {
        display: window,
        instrument: SpotreadInstrument::from_env(),
        settle: SETTLE,
    };
    let mut rig = ProgressRig::new(spotread, total, cancel, Box::new(on_progress));
    calibrate(&mut rig, target(), options).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    use colibri_core::calibrate::CalibrateError;

    use super::*;

    fn sim_rig() -> SimRig {
        SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1))
    }

    #[test]
    fn progress_rig_reports_every_reading() {
        let total = measurement_count(&Options::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let mut rig = ProgressRig::new(
            sim_rig(),
            total,
            Arc::new(AtomicBool::new(false)),
            Box::new(move |done, total| sink.lock().unwrap().push((done, total))),
        );
        calibrate(&mut rig, target(), Options::default()).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 44);
        assert_eq!(seen.last(), Some(&(44, 44)));
    }

    #[test]
    fn progress_rig_stops_when_cancelled() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut rig = ProgressRig::new(sim_rig(), 44, cancel, Box::new(|_, _| {}));
        let err = calibrate(&mut rig, target(), Options::default()).unwrap_err();
        assert_eq!(err, CalibrateError::Measure(MeasureError::Cancelled));
    }

    #[test]
    fn simulated_run_finishes_with_a_calibration() {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_calibration(SensorChoice::Simulated, None, cancel, tx, Box::new(|| {}))
            .join()
            .unwrap();
        let messages: Vec<WorkerMsg> = rx.try_iter().collect();
        let progress = messages
            .iter()
            .filter(|m| matches!(m, WorkerMsg::Progress { .. }))
            .count();
        assert_eq!(progress, 44);
        assert!(matches!(messages.last(), Some(WorkerMsg::Finished(Ok(_)))));
    }

    #[test]
    fn spotread_run_needs_a_display() {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_calibration(SensorChoice::Spotread, None, cancel, tx, Box::new(|| {}))
            .join()
            .unwrap();
        let last = rx.try_iter().last();
        assert!(
            matches!(&last, Some(WorkerMsg::Finished(Err(e))) if e.contains("display")),
            "{last:?}"
        );
    }

    #[test]
    fn simulated_run_says_when_measuring_starts() {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_calibration(SensorChoice::Simulated, None, cancel, tx, Box::new(|| {}))
            .join()
            .unwrap();
        let first = rx.try_iter().next();
        assert!(matches!(first, Some(WorkerMsg::Started)), "{first:?}");
    }
}
