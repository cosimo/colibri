//! Deterministic noise, so simulated runs are reproducible.

/// SplitMix64 generator with Box–Muller normal samples.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng {
            state: seed,
            spare: None,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal.
    pub fn normal(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let r = (-2.0 * self.uniform().ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * self.uniform();
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_has_unit_variance() {
        let mut rng = Rng::new(7);
        let n = 100_000;
        let samples: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
        let mean = samples.iter().sum::<f64>() / n as f64;
        let var = samples.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        assert!(
            mean.abs() < 0.02 && (var - 1.0).abs() < 0.02,
            "mean {mean} var {var}"
        );
    }
}
