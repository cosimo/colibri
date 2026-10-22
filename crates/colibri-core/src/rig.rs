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

    /// The drive values actually shown for `patch`, for displays that can
    /// only show a limited number of levels (an 8-bit patch window shows
    /// multiples of 1/255). Calibration uses these as the true drive values.
    fn quantize(&self, patch: Rgb) -> Rgb {
        patch
    }
}

/// A measuring instrument, independent of how patches get on screen.
pub trait Instrument {
    /// Take one reading of whatever the instrument is pointed at.
    fn read(&mut self) -> Result<Vec3, MeasureError>;

    fn trust(&self) -> Trust;
}

/// Something that can fill the measured area with one colour.
pub trait PatchDisplay {
    /// Show `patch` and return once it is on screen.
    fn show(&mut self, patch: Rgb) -> Result<(), MeasureError>;

    /// Bits per channel of the patches shown.
    fn bits(&self) -> u32;
}

/// A [`PatchDisplay`] and an [`Instrument`] used together as a [`Rig`].
pub struct InstrumentRig<D, I> {
    pub display: D,
    pub instrument: I,
    /// Wait after changing the patch, for the panel to settle.
    pub settle: std::time::Duration,
}

impl<D: PatchDisplay, I: Instrument> Rig for InstrumentRig<D, I> {
    fn measure(&mut self, patch: Rgb) -> Result<Vec3, MeasureError> {
        self.display.show(self.quantize(patch))?;
        std::thread::sleep(self.settle);
        self.instrument.read()
    }

    fn trust(&self) -> Trust {
        self.instrument.trust()
    }

    fn quantize(&self, patch: Rgb) -> Rgb {
        quantize(patch, self.display.bits())
    }
}

/// Round each channel of `patch` to the nearest level of a `bits`-bit display.
pub fn quantize(patch: Rgb, bits: u32) -> Rgb {
    let max = ((1u64 << bits.clamp(1, 32)) - 1) as f64;
    patch.map(|v| (v.clamp(0.0, 1.0) * max).round() / max)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder(Vec<Rgb>);

    impl PatchDisplay for Recorder {
        fn show(&mut self, patch: Rgb) -> Result<(), MeasureError> {
            self.0.push(patch);
            Ok(())
        }
        fn bits(&self) -> u32 {
            8
        }
    }

    struct Constant;

    impl Instrument for Constant {
        fn read(&mut self) -> Result<Vec3, MeasureError> {
            Ok([1.0, 2.0, 3.0])
        }
        fn trust(&self) -> Trust {
            Trust::Relative
        }
    }

    #[test]
    fn instrument_rig_shows_quantized_patches() {
        let mut rig = InstrumentRig {
            display: Recorder(Vec::new()),
            instrument: Constant,
            settle: std::time::Duration::ZERO,
        };
        assert_eq!(rig.measure([0.5, 0.0, 1.0]).unwrap(), [1.0, 2.0, 3.0]);
        assert_eq!(rig.display.0, vec![[128.0 / 255.0, 0.0, 1.0]]);
        assert_eq!(rig.trust(), Trust::Relative);
    }
}
