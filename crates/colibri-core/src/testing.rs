//! Rigs shared by the unit tests of several modules.

use crate::math::{add, Mat3, Vec3};
use crate::rig::{MeasureError, Rgb, Rig, Trust};

/// Additive display with per-channel gammas, seen through a fixed sensor matrix.
pub(crate) struct ToyRig {
    gamma: Vec3,
    sensor: Mat3,
    black: Vec3,
    trust: Trust,
}

impl Rig for ToyRig {
    fn measure(&mut self, p: Rgb) -> Result<Vec3, MeasureError> {
        let lin = [0, 1, 2].map(|c| p[c].powf(self.gamma[c]));
        Ok(add(self.sensor * lin, self.black))
    }
    fn trust(&self) -> Trust {
        self.trust
    }
}

pub(crate) fn toy(trust: Trust) -> ToyRig {
    ToyRig {
        gamma: [2.0, 2.3, 2.6],
        sensor: Mat3([[40.0, 35.0, 18.0], [21.0, 70.0, 7.0], [2.0, 12.0, 95.0]]),
        black: [0.1, 0.1, 0.12],
        trust,
    }
}
