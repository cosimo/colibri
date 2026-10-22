//! colibri-core: colour science and calibration algorithms, independent of
//! any instrument, operating system or user interface.

pub mod calibrate;
pub mod color;
pub mod curve;
pub mod icc;
pub mod math;
mod md5;
pub mod rig;
pub mod spectral;
#[cfg(test)]
pub(crate) mod testing;
