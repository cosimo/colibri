//! Simulated instruments: three linear channels with sensitivity curves and noise.

use colibri_core::math::Vec3;
use colibri_core::rig::Trust;
use colibri_core::spectral::{self, Spectrum, LEN};

use crate::rng::Rng;

#[derive(Clone, Debug)]
pub struct SimSensor {
    pub sensitivities: [Spectrum; 3],
    /// Multiplies every reading (683 makes CIE sensitivities report cd/m²).
    pub gain: f64,
    /// Standard deviation of noise proportional to the signal.
    pub relative_noise: f64,
    /// Standard deviation of noise independent of the signal, in reading units.
    pub noise_floor: f64,
    pub trust: Trust,
    rng: Rng,
}

impl SimSensor {
    pub fn new(sensitivities: [Spectrum; 3], gain: f64, trust: Trust, seed: u64) -> SimSensor {
        SimSensor {
            sensitivities,
            gain,
            relative_noise: 0.0,
            noise_floor: 0.0,
            trust,
            rng: Rng::new(seed),
        }
    }

    pub fn with_noise(mut self, relative: f64, floor: f64) -> SimSensor {
        self.relative_noise = relative;
        self.noise_floor = floor;
        self
    }

    pub fn with_trust(mut self, trust: Trust) -> SimSensor {
        self.trust = trust;
        self
    }

    /// A good colorimeter: exact CIE 1931 sensitivities, 0.1% noise,
    /// 0.005 cd/m² noise floor.
    pub fn colorimeter(seed: u64) -> SimSensor {
        SimSensor::new(
            spectral::cie1931_approx(),
            spectral::KM,
            Trust::Absolute,
            seed,
        )
        .with_noise(0.001, 0.005)
    }

    /// A colorimeter whose dye filters have drifted with age: every lobe moved
    /// to longer wavelengths and the blue filter darkened. It still believes
    /// it reports XYZ, so it claims [`Trust::Absolute`].
    pub fn aged_colorimeter(seed: u64) -> SimSensor {
        let [x, y, z] = spectral::cie1931_approx();
        SimSensor::new(
            [shifted(&x, 6), shifted(&y, 4), shifted(&z, 8).scaled(0.85)],
            spectral::KM,
            Trust::Absolute,
            seed,
        )
        .with_noise(0.002, 0.01)
    }

    /// A phone camera in RAW mode: broad, overlapping sensitivities that are
    /// no linear combination of the CIE functions.
    pub fn phone_camera(seed: u64) -> SimSensor {
        SimSensor::new(
            [
                Spectrum::gaussian(600.0, 35.0, 45.0),
                Spectrum::gaussian(535.0, 40.0, 40.0),
                Spectrum::gaussian(465.0, 25.0, 35.0),
            ],
            spectral::KM,
            Trust::Relative,
            seed,
        )
        .with_noise(0.002, 0.01)
    }

    pub fn read(&mut self, radiance: &Spectrum) -> Vec3 {
        [0, 1, 2].map(|c| {
            let v = self.gain * self.sensitivities[c].integrate(radiance);
            v * (1.0 + self.relative_noise * self.rng.normal())
                + self.noise_floor * self.rng.normal()
        })
    }
}

/// `s` moved `nm` nanometres towards longer wavelengths.
fn shifted(s: &Spectrum, nm: usize) -> Spectrum {
    let steps = (nm as f64 / spectral::STEP_NM).round() as usize;
    Spectrum(
        (0..LEN)
            .map(|i| if i >= steps { s.0[i - steps] } else { 0.0 })
            .collect(),
    )
}
