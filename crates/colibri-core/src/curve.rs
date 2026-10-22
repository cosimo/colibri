//! Tone curves: measured channel responses and target transfer functions.

/// A measured, monotonic channel response: drive value (0–1) to linear output.
///
/// Built from noisy samples. Noise is removed by isotonic regression, so the
/// curve never decreases, and the samples are joined by monotone cubic
/// (Fritsch–Carlson) interpolation, so the curve is smooth between them
/// without overshooting.
#[derive(Clone, Debug, PartialEq)]
pub struct ToneCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    slopes: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveError {
    TooFewPoints,
    /// Drive values must be strictly increasing.
    UnsortedInput,
    /// Output does not increase from first to last sample.
    NoResponse,
}

impl std::fmt::Display for CurveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CurveError::TooFewPoints => "a tone curve needs at least two samples",
            CurveError::UnsortedInput => "tone curve drive values must be strictly increasing",
            CurveError::NoResponse => "channel output does not increase with drive",
        })
    }
}

impl std::error::Error for CurveError {}

impl ToneCurve {
    pub fn from_samples(xs: &[f64], ys: &[f64]) -> Result<ToneCurve, CurveError> {
        if xs.len() < 2 || xs.len() != ys.len() {
            return Err(CurveError::TooFewPoints);
        }
        if xs.windows(2).any(|w| w[1] <= w[0]) {
            return Err(CurveError::UnsortedInput);
        }
        let ys = isotonic(ys);
        if ys[ys.len() - 1] <= ys[0] {
            return Err(CurveError::NoResponse);
        }
        let slopes = monotone_slopes(xs, &ys);
        Ok(ToneCurve {
            xs: xs.to_vec(),
            ys,
            slopes,
        })
    }

    pub fn min_output(&self) -> f64 {
        self.ys[0]
    }

    pub fn max_output(&self) -> f64 {
        self.ys[self.ys.len() - 1]
    }

    /// Output for drive `x`, clamped to the measured range.
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|&v| v <= x) - 1;
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.slopes[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.slopes[i + 1]
    }

    /// Smallest drive value producing output `y`, clamped to the measured range.
    pub fn inverse(&self, y: f64) -> f64 {
        let n = self.xs.len();
        if y <= self.ys[0] {
            return self.xs[0];
        }
        if y >= self.ys[n - 1] {
            return self.xs[n - 1];
        }
        // First segment whose end reaches y; the curve is monotone inside it.
        let i = self.ys.partition_point(|&v| v < y).max(1) - 1;
        let (mut lo, mut hi) = (self.xs[i], self.xs[i + 1]);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if self.eval(mid) < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

/// Pool-adjacent-violators: the closest non-decreasing sequence in least squares.
fn isotonic(ys: &[f64]) -> Vec<f64> {
    // Blocks of (mean, count).
    let mut blocks: Vec<(f64, usize)> = Vec::with_capacity(ys.len());
    for &y in ys {
        blocks.push((y, 1));
        while blocks.len() > 1 && blocks[blocks.len() - 2].0 > blocks[blocks.len() - 1].0 {
            let (m2, n2) = blocks.pop().unwrap();
            let (m1, n1) = blocks.pop().unwrap();
            let n = n1 + n2;
            blocks.push(((m1 * n1 as f64 + m2 * n2 as f64) / n as f64, n));
        }
    }
    blocks
        .into_iter()
        .flat_map(|(m, n)| std::iter::repeat_n(m, n))
        .collect()
}

/// Fritsch–Carlson tangents for monotone cubic Hermite interpolation.
fn monotone_slopes(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let delta: Vec<f64> = (0..n - 1)
        .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
        .collect();
    let mut m = vec![0.0; n];
    m[0] = delta[0];
    m[n - 1] = delta[n - 2];
    for i in 1..n - 1 {
        m[i] = if delta[i - 1] * delta[i] <= 0.0 {
            0.0
        } else {
            (delta[i - 1] + delta[i]) / 2.0
        };
    }
    for i in 0..n - 1 {
        if delta[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let a = m[i] / delta[i];
        let b = m[i + 1] / delta[i];
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * delta[i];
            m[i + 1] = t * b * delta[i];
        }
    }
    m
}

/// The tone response a calibration aims for, as relative luminance 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetCurve {
    /// Pure power law.
    Gamma(f64),
    /// IEC 61966-2-1 piecewise sRGB curve.
    Srgb,
}

impl TargetCurve {
    pub fn eval(self, v: f64) -> f64 {
        let v = v.clamp(0.0, 1.0);
        match self {
            TargetCurve::Gamma(g) => v.powf(g),
            TargetCurve::Srgb => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gamma_samples(n: usize, g: f64) -> (Vec<f64>, Vec<f64>) {
        let xs: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        let ys = xs.iter().map(|x| x.powf(g)).collect();
        (xs, ys)
    }

    #[test]
    fn interpolates_a_gamma_curve_from_17_points() {
        let (xs, ys) = gamma_samples(17, 2.2);
        let c = ToneCurve::from_samples(&xs, &ys).unwrap();
        for i in 0..=100 {
            let x = i as f64 / 100.0;
            assert!((c.eval(x) - x.powf(2.2)).abs() < 2e-3, "x={x}");
        }
    }

    #[test]
    fn inverse_undoes_eval() {
        let (xs, ys) = gamma_samples(9, 2.4);
        let c = ToneCurve::from_samples(&xs, &ys).unwrap();
        for i in 1..100 {
            let x = i as f64 / 100.0;
            assert!((c.inverse(c.eval(x)) - x).abs() < 1e-9);
        }
        assert_eq!(c.inverse(-1.0), 0.0);
        assert_eq!(c.inverse(2.0), 1.0);
    }

    #[test]
    fn noisy_samples_become_monotone() {
        let xs = [0.0, 0.25, 0.5, 0.75, 1.0];
        let ys = [0.0, 0.10, 0.08, 0.5, 1.0];
        let c = ToneCurve::from_samples(&xs, &ys).unwrap();
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=200 {
            let y = c.eval(i as f64 / 200.0);
            assert!(y >= prev - 1e-12);
            prev = y;
        }
        assert!((c.eval(0.25) - 0.09).abs() < 1e-12 && (c.eval(0.5) - 0.09).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            ToneCurve::from_samples(&[0.0], &[0.0]),
            Err(CurveError::TooFewPoints)
        );
        assert_eq!(
            ToneCurve::from_samples(&[0.0, 0.0], &[0.0, 1.0]),
            Err(CurveError::UnsortedInput)
        );
        assert_eq!(
            ToneCurve::from_samples(&[0.0, 1.0], &[1.0, 1.0]),
            Err(CurveError::NoResponse)
        );
    }

    #[test]
    fn srgb_target_is_continuous() {
        let lo = TargetCurve::Srgb.eval(0.04045);
        let hi = TargetCurve::Srgb.eval(0.04045 + 1e-9);
        assert!((lo - hi).abs() < 1e-7);
        assert_eq!(TargetCurve::Srgb.eval(1.0), 1.0);
    }
}
