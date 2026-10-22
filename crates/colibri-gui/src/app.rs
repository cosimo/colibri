//! The colibri window and its screens: Ready, Measuring, Compare, and Kept
//! after Keep.
//!
//! This layer stays thin. What the screens offer comes from `flow`, the
//! profile switching from `session`, and the calibration run from `worker`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use colibri_core::calibrate::{measurement_count, Calibration, Options};
use colibri_core::icc::{calibrated_profile, Mhc2Gamut};
use colibri_spotread::SpotreadInstrument;
use colibri_windows::Monitor;
use eframe::egui::{self, Color32, Key, RichText, Sense, Ui, ViewportCommand};

use crate::flow::{
    data_dir, default_monitor, monitor_label, profile_description, profile_file_name,
    progress_text, show_display_choice, spotread_blocker, utc_from_unix, SpyderSearch,
};
use crate::session::{recover, DisplaySession, RestoreFile, Showing, WindowsProfiles};
use crate::worker::{spawn_calibration, target, SensorChoice, WorkerMsg, START_PROMPT};

/// The sensor dropdown, in order.
const SENSORS: [(SensorChoice, &str); 2] = [
    (SensorChoice::Spotread, "Spyder4 (through spotread, test)"),
    (SensorChoice::Simulated, "Simulated"),
];

const TARGET_TEXT: &str = "D65 white, sRGB curve";

const READY_REMINDER: &str =
    "Turn off Night light and HDR, and let the monitor warm up for about 30 minutes.";

const AUTO_COLOR_REMINDER: &str = "When you're done, turn Auto Color Management back on in \
     Settings > System > Display. It maps the colours of ordinary apps to the new profile.";

const NO_PROFILE: &str = "no profile";

const SIMULATED_NOTE: &str = "(simulated: made-up correction)";

/// The name profiles get when there is no display, as with the simulator
/// off Windows.
const SIMULATED_DISPLAY_NAME: &str = "simulated panel";

/// Steps in the Compare screen's gray ramp.
const GRAY_STEPS: usize = 32;

/// Red, green and blue, then skin, sky and foliage, which eyes are quick to
/// judge.
const SWATCHES: [[u8; 3]; 6] = [
    [200, 40, 40],
    [40, 160, 60],
    [40, 70, 200],
    [225, 175, 150],
    [110, 160, 210],
    [90, 130, 60],
];

pub struct App {
    profiles: WindowsProfiles,
    restore: RestoreFile,
    monitors: Vec<Monitor>,
    selected_monitor: usize,
    sensor: SensorChoice,
    spyder: SpyderSearch,
    spyder_rx: Receiver<Result<String, String>>,
    /// For the Ready screen: a restore after a crash, or why the last run
    /// stopped.
    message: Option<String>,
    screen: Screen,
    /// Close the window once the running calibration has stopped.
    close_after_run: bool,
}

enum Screen {
    Ready,
    Measuring(Run),
    Compare(Comparison),
    Kept(Kept),
}

/// A calibration in progress.
struct Run {
    sensor: SensorChoice,
    monitor: Option<Monitor>,
    session: Option<DisplaySession>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<WorkerMsg>,
    waiting_for_start: bool,
    /// When the first reading started.
    started: Instant,
    done: usize,
    total: usize,
}

/// A new profile, saved and shown, waiting for Keep or Discard.
struct Comparison {
    sensor: SensorChoice,
    session: Option<DisplaySession>,
    path: PathBuf,
    description: String,
    /// Why the last switch failed.
    switch_error: Option<String>,
}

struct Kept {
    path: PathBuf,
    /// Whether the profile became the display's default, or was only saved.
    installed: bool,
}

/// What `save_and_install()` made.
struct NewProfile {
    path: PathBuf,
    description: String,
}

impl App {
    /// Puts back a display left changed by a run that never finished, and
    /// starts looking for the Spyder4.
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        let mut profiles = WindowsProfiles;
        let restore = RestoreFile {
            path: data_dir().join("restore.json"),
        };
        let monitors = colibri_windows::monitors().unwrap_or_default();
        let message = recover(&mut profiles, &restore, &monitors);
        let sensor = if monitors.is_empty() {
            SensorChoice::Simulated
        } else {
            SensorChoice::Spotread
        };
        App {
            profiles,
            restore,
            selected_monitor: default_monitor(&monitors),
            monitors,
            sensor,
            spyder: SpyderSearch::Looking,
            spyder_rx: search_for_spyder(cc.egui_ctx.clone()),
            message,
            screen: Screen::Ready,
            close_after_run: false,
        }
    }

    fn poll_spyder(&mut self) {
        if self.spyder != SpyderSearch::Looking {
            return;
        }
        self.spyder = match self.spyder_rx.try_recv() {
            Ok(Ok(port)) => SpyderSearch::Found(port),
            Ok(Err(why)) => SpyderSearch::Missing(why),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                SpyderSearch::Missing("the search for the Spyder4 stopped".into())
            }
        };
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let Screen::Measuring(run) = &mut self.screen else {
            return;
        };
        let Some(result) = read_worker_messages(run) else {
            return;
        };
        if let Screen::Measuring(run) = std::mem::replace(&mut self.screen, Screen::Ready) {
            self.finish_run(run, result, ctx);
        }
    }

    /// Closing while measuring stops the run first. Closing in Compare counts
    /// as Discard.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        match &mut self.screen {
            Screen::Measuring(run) if !run.waiting_for_start => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                run.cancel.store(true, Ordering::Relaxed);
                self.close_after_run = true;
            }
            // Nothing is measured yet, and the patch window only listens to
            // Esc while it waits for Space. The display can go back now, and
            // the patch window goes with the process.
            Screen::Measuring(_) => {
                if let Screen::Measuring(run) = std::mem::replace(&mut self.screen, Screen::Ready) {
                    // A failure leaves the restore file for the next start.
                    let _ = self.restore_display(run.session);
                }
            }
            Screen::Compare(_) => self.discard(),
            Screen::Ready | Screen::Kept(_) => {}
        }
    }

    fn start_run(&mut self, ctx: &egui::Context) {
        let monitor = self.monitors.get(self.selected_monitor).cloned();
        let prepare_for_measuring = self.sensor == SensorChoice::Spotread;
        let mut session = None;
        if let Some(monitor) = &monitor {
            let begun = DisplaySession::begin(
                &mut self.profiles,
                &self.restore,
                monitor.clone(),
                prepare_for_measuring,
            );
            match begun {
                Ok(begun) => session = Some(begun),
                Err(e) => {
                    self.message = Some(format!("Couldn't get {} ready: {e}", monitor.name));
                    return;
                }
            }
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let wake = Box::new(move || repaint.request_repaint());
        spawn_calibration(self.sensor, monitor.clone(), cancel.clone(), tx, wake);
        self.message = None;
        self.screen = Screen::Measuring(Run {
            sensor: self.sensor,
            monitor,
            session,
            cancel,
            rx,
            waiting_for_start: false,
            started: Instant::now(),
            done: 0,
            total: measurement_count(&Options::default()),
        });
    }

    fn finish_run(
        &mut self,
        run: Run,
        result: Result<Box<Calibration>, String>,
        ctx: &egui::Context,
    ) {
        let Run {
            sensor,
            monitor,
            mut session,
            ..
        } = run;
        let saved = result.and_then(|calibration| {
            let monitor = monitor.as_ref();
            save_and_install(&mut self.profiles, &calibration, monitor, session.as_mut())
        });

        // Closing the window while measuring counts as Discard.
        if self.close_after_run {
            // A failure leaves the restore file for the next start.
            let _ = self.restore_display(session);
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        match saved {
            Ok(new_profile) => {
                self.screen = Screen::Compare(Comparison {
                    sensor,
                    session,
                    path: new_profile.path,
                    description: new_profile.description,
                    switch_error: None,
                });
            }
            Err(e) => {
                let stopped = format!("Calibration stopped: {e}");
                self.message = Some(match self.restore_display(session) {
                    Ok(()) => stopped,
                    Err(restore_error) => format!("{stopped}\n{restore_error}"),
                });
            }
        }
    }

    /// Puts the display back as it was before the run, if there is one.
    fn restore_display(&mut self, session: Option<DisplaySession>) -> Result<(), String> {
        let Some(session) = session else {
            return Ok(());
        };
        session
            .restore(&mut self.profiles, &self.restore)
            .map_err(|e| {
                format!(
                    "Couldn't put the display back: {e}. colibri tries again at the next start."
                )
            })
    }

    fn keep(&mut self) {
        let Screen::Compare(comparison) = std::mem::replace(&mut self.screen, Screen::Ready) else {
            return;
        };
        let installed = comparison.session.is_some();
        if let Some(session) = comparison.session {
            if let Err(e) = session.keep(&mut self.profiles, &self.restore) {
                self.message = Some(format!("Couldn't keep the new profile: {e}"));
                return;
            }
        }
        self.screen = Screen::Kept(Kept {
            path: comparison.path,
            installed,
        });
    }

    fn discard(&mut self) {
        let Screen::Compare(comparison) = std::mem::replace(&mut self.screen, Screen::Ready) else {
            return;
        };
        self.message = self.restore_display(comparison.session).err();
    }

    fn ready_screen(&mut self, ui: &mut Ui) {
        ui.heading("colibri");
        ui.add_space(8.0);
        egui::Grid::new("ready")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Display");
                self.display_choice(ui);
                ui.end_row();
                ui.label("Sensor");
                self.sensor_choice(ui);
                ui.end_row();
                ui.label("Target");
                ui.label(TARGET_TEXT);
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.weak(READY_REMINDER);
        ui.add_space(8.0);

        if let Some(message) = &self.message {
            ui.colored_label(ui.visuals().warn_fg_color, message);
        }
        let blocker = self.calibrate_blocker();
        if let Some(blocker) = &blocker {
            ui.colored_label(ui.visuals().warn_fg_color, blocker);
        }
        ui.add_space(8.0);
        let calibrate = egui::Button::new(RichText::new("Calibrate").size(20.0))
            .min_size(egui::vec2(200.0, 48.0));
        if ui.add_enabled(blocker.is_none(), calibrate).clicked() {
            self.start_run(ui.ctx());
        }
    }

    /// Why Calibrate is disabled. Only the Spyder4 has requirements. The
    /// colour mode is asked again on every repaint, so turning off Auto
    /// Color Management shows up as soon as the mouse is back.
    fn calibrate_blocker(&self) -> Option<String> {
        if self.sensor == SensorChoice::Simulated {
            return None;
        }
        let monitor = self.monitors.get(self.selected_monitor);
        let color_mode = monitor.and_then(|m| colibri_windows::color_mode(m).ok());
        spotread_blocker(&self.spyder, monitor, color_mode)
    }

    fn display_choice(&mut self, ui: &mut Ui) {
        if self.monitors.is_empty() {
            ui.label("none found");
            return;
        }
        if !show_display_choice(&self.monitors) {
            ui.label(monitor_label(&self.monitors[0]));
            return;
        }
        let selected = monitor_label(&self.monitors[self.selected_monitor]);
        egui::ComboBox::from_id_salt("display")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (i, monitor) in self.monitors.iter().enumerate() {
                    ui.selectable_value(&mut self.selected_monitor, i, monitor_label(monitor));
                }
            });
    }

    fn sensor_choice(&mut self, ui: &mut Ui) {
        let selected = SENSORS
            .iter()
            .find(|(sensor, _)| *sensor == self.sensor)
            .map_or("", |(_, label)| label);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("sensor")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (sensor, label) in SENSORS {
                        ui.selectable_value(&mut self.sensor, sensor, label);
                    }
                });
            ui.weak(self.sensor_status());
        });
    }

    fn sensor_status(&self) -> String {
        if self.sensor == SensorChoice::Simulated {
            return "a made-up wide-gamut panel".into();
        }
        match &self.spyder {
            SpyderSearch::Looking => "looking for the Spyder4…".into(),
            SpyderSearch::Found(port) => format!("found {port}"),
            SpyderSearch::Missing(_) => "not found".into(),
        }
    }

    fn measuring_screen(&mut self, ui: &mut Ui) {
        let Screen::Measuring(run) = &self.screen else {
            return;
        };
        ui.heading("Measuring");
        ui.add_space(8.0);
        if run.waiting_for_start {
            ui.label(format!("{START_PROMPT}. Esc in the patch window cancels."));
            return;
        }

        ui.label(progress_text(run.done, run.total, run.started.elapsed()));
        let fraction = run.done as f32 / run.total.max(1) as f32;
        ui.add(egui::ProgressBar::new(fraction));
        ui.add_space(8.0);
        if run.cancel.load(Ordering::Relaxed) {
            ui.label("Stopping after this reading…");
            return;
        }
        let cancel_pressed = ui.input(|i| i.key_pressed(Key::Escape));
        if ui.button("Cancel").clicked() || cancel_pressed {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn compare_screen(&mut self, ui: &mut Ui) {
        let Screen::Compare(comparison) = &mut self.screen else {
            return;
        };
        ui.heading("Compare");
        ui.add_space(8.0);

        let showing = comparison
            .session
            .as_ref()
            .map_or(Showing::After, |s| s.showing);
        let before = comparison
            .session
            .as_ref()
            .and_then(|s| s.before.as_deref());
        let before_text = format!("Before: {}", before.unwrap_or(NO_PROFILE));
        let mut after_text = format!("After: {}", comparison.description);
        if comparison.sensor == SensorChoice::Simulated {
            after_text = format!("{after_text} {SIMULATED_NOTE}");
        }

        let can_switch = comparison.session.is_some();
        let mut wanted = None;
        // One above the other: profile descriptions are too long to share a
        // line.
        ui.add_enabled_ui(can_switch, |ui| {
            ui.vertical(|ui| {
                if ui
                    .selectable_label(showing == Showing::Before, before_text)
                    .clicked()
                {
                    wanted = Some(Showing::Before);
                }
                if ui
                    .selectable_label(showing == Showing::After, after_text)
                    .clicked()
                {
                    wanted = Some(Showing::After);
                }
            });
        });
        let space_pressed =
            !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(Key::Space));
        if can_switch && space_pressed {
            wanted = Some(match showing {
                Showing::Before => Showing::After,
                Showing::After => Showing::Before,
            });
        }
        if let (Some(wanted), Some(session)) = (wanted, comparison.session.as_mut()) {
            let switched = session.show(&mut self.profiles, wanted);
            comparison.switch_error = switched.err().map(|e| format!("Couldn't switch: {e}"));
        }

        if can_switch {
            ui.weak(
                "Space switches too. The whole display changes, so any window can be compared.",
            );
        } else {
            ui.weak("No Windows display to switch: the profile is only saved.");
        }
        if let Some(error) = &comparison.switch_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(8.0);
        paint_samples(ui);
        ui.add_space(8.0);

        let mut keep = false;
        let mut discard = false;
        ui.horizontal(|ui| {
            keep = ui.button("Keep new profile").clicked();
            discard = ui.button("Discard").clicked();
        });
        if can_switch {
            ui.add_space(8.0);
            ui.weak(AUTO_COLOR_REMINDER);
        }

        if keep {
            self.keep();
        } else if discard {
            self.discard();
        }
    }

    fn kept_screen(&mut self, ui: &mut Ui) {
        let Screen::Kept(kept) = &self.screen else {
            return;
        };
        ui.heading("Kept");
        ui.add_space(8.0);
        if kept.installed {
            ui.label("The new profile is the display's default now. The file is in:");
        } else {
            ui.label("The display wasn't changed. The profile is saved in:");
        }
        ui.monospace(kept.path.display().to_string());
        if kept.installed {
            ui.add_space(8.0);
            ui.weak(AUTO_COLOR_REMINDER);
        }
        ui.add_space(8.0);
        if ui.button("Calibrate again").clicked() {
            self.screen = Screen::Ready;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_spyder();
        self.poll_worker(&ctx);
        self.handle_close_request(&ctx);
        egui::CentralPanel::default_margins().show(ui, |ui| match self.screen {
            Screen::Ready => self.ready_screen(ui),
            Screen::Measuring(_) => self.measuring_screen(ui),
            Screen::Compare(_) => self.compare_screen(ui),
            Screen::Kept(_) => self.kept_screen(ui),
        });
    }
}

/// Runs `spotread -?` on a thread of its own, since it can take seconds.
fn search_for_spyder(ctx: egui::Context) -> Receiver<Result<String, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(SpotreadInstrument::from_env().find_spyder());
        ctx.request_repaint();
    });
    rx
}

/// Applies the worker's messages to `run`, and returns the result once the
/// run has finished.
fn read_worker_messages(run: &mut Run) -> Option<Result<Box<Calibration>, String>> {
    for message in run.rx.try_iter() {
        match message {
            WorkerMsg::WaitingForStart => run.waiting_for_start = true,
            WorkerMsg::Started => {
                run.waiting_for_start = false;
                run.started = Instant::now();
            }
            WorkerMsg::Progress { done, total } => {
                run.done = done;
                run.total = total;
            }
            WorkerMsg::Finished(result) => return Some(result),
        }
    }
    None
}

/// Builds the profile and saves it under `data_dir()`. With a session it
/// also installs the profile and makes it the display's default.
fn save_and_install(
    profiles: &mut WindowsProfiles,
    calibration: &Calibration,
    monitor: Option<&Monitor>,
    session: Option<&mut DisplaySession>,
) -> Result<NewProfile, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let created = utc_from_unix(now.as_secs());
    let display_name = monitor.map_or(SIMULATED_DISPLAY_NAME, |m| m.name.as_str());
    let description = profile_description(display_name, created);
    let profile = calibrated_profile(
        calibration,
        target(),
        Mhc2Gamut::Native,
        &description,
        created,
    )
    .map_err(|e| e.to_string())?;
    let bytes = profile.to_bytes().map_err(|e| e.to_string())?;

    let dir = data_dir().join("profiles");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(profile_file_name(display_name, created));
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;

    if let Some(session) = session {
        session
            .install_new(profiles, &path)
            .map_err(|e| e.to_string())?;
    }
    Ok(NewProfile { path, description })
}

/// A gray ramp and a row of colour swatches, something to look at while
/// switching.
fn paint_samples(ui: &mut Ui) {
    let width = ui.available_width().min(480.0);
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 40.0), Sense::hover());
    let ramp = response.rect;
    let step_width = ramp.width() / GRAY_STEPS as f32;
    for step in 0..GRAY_STEPS {
        let left = ramp.left() + step as f32 * step_width;
        let rect = egui::Rect::from_min_size(
            egui::pos2(left, ramp.top()),
            egui::vec2(step_width + 0.5, ramp.height()),
        );
        let gray = (step * 255 / (GRAY_STEPS - 1)) as u8;
        painter.rect_filled(rect, 0.0, Color32::from_gray(gray));
    }

    ui.add_space(8.0);
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 48.0), Sense::hover());
    let row = response.rect;
    let swatch_width = row.width() / SWATCHES.len() as f32;
    for (i, [r, g, b]) in SWATCHES.into_iter().enumerate() {
        let left = row.left() + i as f32 * swatch_width;
        let rect = egui::Rect::from_min_size(
            egui::pos2(left, row.top()),
            egui::vec2(swatch_width - 4.0, row.height()),
        );
        painter.rect_filled(rect, 4.0, Color32::from_rgb(r, g, b));
    }
}
