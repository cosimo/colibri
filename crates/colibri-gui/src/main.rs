//! colibri-gui: pick a display, calibrate it, compare the new profile with
//! the previous one, and keep it or not.

// Release builds on Windows use the GUI subsystem, so no console opens.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod flow;
mod session;
mod worker;

use eframe::egui;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([600.0, 520.0])
        .with_min_inner_size([440.0, 380.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "colibri",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
