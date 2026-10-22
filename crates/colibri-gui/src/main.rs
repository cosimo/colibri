//! colibri-gui: pick a display, calibrate it, compare the new profile with
//! the previous one, and keep it or not.

// Release builds on Windows use the GUI subsystem, so no console opens.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// The screens that use it come later.
#[allow(dead_code)]
mod flow;
#[allow(dead_code)]
mod session;
#[allow(dead_code)]
mod worker;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions::default();
    eframe::run_native("colibri", options, Box::new(|_cc| Ok(Box::new(Empty))))
}

/// An empty window, until the screens are there.
struct Empty;

impl eframe::App for Empty {
    fn ui(&mut self, _ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {}
}
