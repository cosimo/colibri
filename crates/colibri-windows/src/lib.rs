//! Windows integration: monitor list, a full-screen patch window for
//! measurements, and the GPU gamma ramp.
//!
//! Measurements need the panel's raw response. Before measuring, reset the
//! gamma ramp to identity and turn off Auto Color Management and any active
//! MHC2 profile for that display, or Windows will transform the patches.
//!
//! On other systems everything returns [`Error::Unsupported`].

use colibri_core::calibrate::CalibrationCurves;

#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

#[cfg(windows)]
pub use win::{
    associate_profile, color_mode, default_profile, dissociate_profile, gamma_ramp,
    install_profile, monitors, set_gamma_ramp, PatchWindow, StatusLine,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// Position in [`monitors`] order.
    pub index: usize,
    /// GDI device name, such as `\\.\DISPLAY1`.
    pub device: String,
    /// Friendly name from the display configuration, such as
    /// `ASUS PA328CGV`, or `device` when there is none.
    pub name: String,
    /// How the colour profile APIs identify this display.
    pub source: Option<DisplaySource>,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

/// A display path from `QueryDisplayConfig`: the adapter's LUID, split in
/// its two halves, the source id and the target id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplaySource {
    pub adapter_low: u32,
    pub adapter_high: i32,
    /// The colour profile APIs take the source id.
    pub id: u32,
    /// The advanced colour queries take the target id.
    pub target_id: u32,
}

/// How Windows treats the colours of a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Sdr,
    AutoColorManagement,
    Hdr,
    /// Windows didn't say. Older than Windows 11 24H2, maybe.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    /// A Windows call failed: the function name and `GetLastError` code.
    Os(&'static str, u32),
    /// The patch window did not respond in time.
    Timeout,
    /// The display has no [`DisplaySource`].
    UnknownDisplay,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => f.write_str("only supported on Windows"),
            Error::Os(call, code) => write!(f, "{call} failed (error {code})"),
            Error::Timeout => f.write_str("the patch window stopped responding"),
            Error::UnknownDisplay => {
                f.write_str("display not found in the Windows display configuration")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A GPU gamma ramp: 256 entries per channel, 0–65535.
pub type GammaRamp = [[u16; 256]; 3];

pub fn identity_ramp() -> GammaRamp {
    let channel: [u16; 256] = std::array::from_fn(|i| (i * 257) as u16);
    [channel; 3]
}

/// Sample calibration curves into a gamma ramp.
pub fn ramp_from_curves(curves: &CalibrationCurves) -> GammaRamp {
    let drives: Vec<[f64; 3]> = (0..256)
        .map(|i| {
            let x = i as f64 / 255.0;
            curves.apply([x, x, x])
        })
        .collect();
    std::array::from_fn(|c| {
        std::array::from_fn(|i| (drives[i][c].clamp(0.0, 1.0) * 65535.0).round() as u16)
    })
}

#[cfg(not(windows))]
pub fn monitors() -> Result<Vec<Monitor>, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn color_mode(_monitor: &Monitor) -> Result<ColorMode, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn install_profile(_path: &std::path::Path) -> Result<String, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn default_profile(_monitor: &Monitor) -> Result<Option<String>, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn associate_profile(
    _monitor: &Monitor,
    _profile: &str,
    _make_default: bool,
) -> Result<(), Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn dissociate_profile(_monitor: &Monitor, _profile: &str) -> Result<(), Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn gamma_ramp(_monitor: &Monitor) -> Result<GammaRamp, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn set_gamma_ramp(_monitor: &Monitor, _ramp: &GammaRamp) -> Result<(), Error> {
    Err(Error::Unsupported)
}

/// Full-screen window showing a patch. Unavailable on this system.
#[cfg(not(windows))]
pub struct PatchWindow(());

#[cfg(not(windows))]
impl PatchWindow {
    pub fn open(_monitor: &Monitor, _size: f64) -> Result<PatchWindow, Error> {
        Err(Error::Unsupported)
    }

    pub fn show_rgb8(&mut self, _rgb: [u8; 3]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    pub fn cancelled(&self) -> bool {
        false
    }

    pub fn status_line(&self) -> StatusLine {
        StatusLine(())
    }

    pub fn wait_for_start(&self, _prompt: &str) -> Result<(), colibri_core::rig::MeasureError> {
        Err(colibri_core::rig::MeasureError::Instrument(
            Error::Unsupported.to_string(),
        ))
    }
}

/// The patch window's corner status text. Unavailable on this system.
#[cfg(not(windows))]
#[derive(Clone)]
pub struct StatusLine(());

#[cfg(not(windows))]
impl StatusLine {
    pub fn set(&self, _text: &str) {}
}

#[cfg(not(windows))]
impl colibri_core::rig::PatchDisplay for PatchWindow {
    fn show(
        &mut self,
        _patch: colibri_core::rig::Rgb,
    ) -> Result<(), colibri_core::rig::MeasureError> {
        Err(colibri_core::rig::MeasureError::Instrument(
            Error::Unsupported.to_string(),
        ))
    }

    fn bits(&self) -> u32 {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    pub(crate) fn monitor() -> Monitor {
        Monitor {
            index: 0,
            device: r"\\.\DISPLAY1".into(),
            name: "ASUS PA328CGV".into(),
            source: None,
            left: 0,
            top: 0,
            width: 2560,
            height: 1440,
            primary: true,
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn stubs_report_unsupported() {
        assert_eq!(monitors(), Err(Error::Unsupported));
        assert_eq!(color_mode(&monitor()), Err(Error::Unsupported));
        let path = std::path::Path::new("colibri.icc");
        assert_eq!(install_profile(path), Err(Error::Unsupported));
        assert_eq!(default_profile(&monitor()), Err(Error::Unsupported));
        let associated = associate_profile(&monitor(), "colibri.icc", true);
        assert_eq!(associated, Err(Error::Unsupported));
        let dissociated = dissociate_profile(&monitor(), "colibri.icc");
        assert_eq!(dissociated, Err(Error::Unsupported));
    }

    #[test]
    fn identity_curves_give_identity_ramp() {
        assert_eq!(
            ramp_from_curves(&CalibrationCurves::identity(1024)),
            identity_ramp()
        );
        let ramp = identity_ramp();
        assert_eq!((ramp[0][0], ramp[1][128], ramp[2][255]), (0, 32896, 65535));
    }
}
