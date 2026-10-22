//! The measurement interface every instrument backend implements.

use crate::math::Vec3;

/// Drive values for one patch, each channel 0–1.
pub type Rgb = [f64; 3];

/// How far an instrument's numbers can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Readings are CIE XYZ in cd/m², accurate enough to hit an absolute target
    /// such as a D65 white point.
    Absolute,
    /// Readings are linear in light but in an unknown colour space: a phone
    /// camera, or a colorimeter whose filters have aged. Good for tone curves
    /// and gray balance at the display's native white, not for absolute targets.
    Relative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeasureError {
    Instrument(String),
    Cancelled,
}

impl std::fmt::Display for MeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeasureError::Instrument(msg) => write!(f, "instrument error: {msg}"),
            MeasureError::Cancelled => f.write_str("measurement cancelled"),
        }
    }
}

impl std::error::Error for MeasureError {}

/// A display plus an instrument pointed at it.
///
/// A real rig shows the patch full-screen (with no colour management applied),
/// waits for the panel to settle and reads the instrument. The simulator does
/// the same with spectra.
pub trait Rig {
    /// Show `patch` and return three channels that are linear in light.
    /// For [`Trust::Absolute`] rigs these are XYZ in cd/m².
    fn measure(&mut self, patch: Rgb) -> Result<Vec3, MeasureError>;

    fn trust(&self) -> Trust;
}
