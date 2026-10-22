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

    /// Like [`ToneCurve::from_samples`], but smooths measurement noise first.
    ///
    /// Samples must be evenly spaced and start at drive 0. The curve is split
    /// into a power law fitted to the data and a slowly varying residual
    /// factor; only the residual is smoothed, so the steep shape near black is
    /// kept. Each sample is weighted by its expected noise, and the smoothing
    /// strength is chosen from the data by generalized cross-validation, so
    /// real detail in the response survives when it stands out of the noise.
    pub fn fit(xs: &[f64], ys: &[f64], noise: Noise) -> Result<ToneCurve, CurveError> {
        if noise.is_zero() || xs.len() < 4 || xs.len() != ys.len() || xs[0] != 0.0 {
            return ToneCurve::from_samples(xs, ys);
        }
        let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let Some(gamma) = power_law_exponent(xs, ys, 0.01 * y_max) else {
            return ToneCurve::from_samples(xs, ys);
        };
        let shape: Vec<f64> = xs[1..].iter().map(|x| x.powf(gamma)).collect();
        let residual: Vec<f64> = ys[1..].iter().zip(&shape).map(|(y, p)| y / p).collect();
        // The residual's variance is the reading's variance divided by y².
        let weights: Vec<f64> = shape
            .iter()
            .zip(&residual)
            .map(|(p, r)| 1.0 / noise.relative_variance(p * r.max(0.0)))
            .collect();
        let (smoothed, _) = smooth_auto(&residual, &weights);
        let mut fitted = vec![ys[0]];
        fitted.extend(smoothed.iter().zip(&shape).map(|(r, p)| r * p));
        ToneCurve::from_samples(xs, &fitted)
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

/// Least-squares exponent of `y = c·x^γ` over samples with `y > min_y` and `x > 0`.
fn power_law_exponent(xs: &[f64], ys: &[f64], min_y: f64) -> Option<f64> {
    let pts: Vec<(f64, f64)> = xs
        .iter()
        .zip(ys)
        .filter(|(x, y)| **x > 0.0 && **y > min_y)
        .map(|(x, y)| (x.ln(), y.ln()))
        .collect();
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = pts
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / n, b + y / n));
    let sxx: f64 = pts.iter().map(|(x, _)| (x - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    (sxx > 0.0)
        .then(|| sxy / sxx)
        .filter(|g| g.is_finite() && *g > 0.0)
}

/// Measurement noise model: a part proportional to the signal and a floor
/// independent of it, both as standard deviations.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Noise {
    pub relative: f64,
    pub floor: f64,
}

impl Noise {
    pub fn is_zero(&self) -> bool {
        self.relative <= 0.0 && self.floor <= 0.0
    }

    /// Variance of a reading of `y`.
    pub fn variance(&self, y: f64) -> f64 {
        (self.relative * y).powi(2) + self.floor.powi(2)
    }

    /// Variance of a reading of `y`, relative to `y`.
    pub fn relative_variance(&self, y: f64) -> f64 {
        let y = y.abs().max(1e-12);
        self.variance(y) / (y * y)
    }
}

/// Weighted Whittaker–Eilers smoother for evenly spaced samples: minimises
/// Σ wᵢ(yᵢ − zᵢ)² + λ Σ (Δ²zᵢ)². Large λ tends towards a straight line.
/// Weights should be inverse variances.
pub fn smooth(ys: &[f64], weights: &[f64], lambda: f64) -> Vec<f64> {
    if ys.len() < 3 || lambda <= 0.0 {
        return ys.to_vec();
    }
    whittaker(ys, weights, lambda).0
}

/// [`smooth`] with λ chosen by generalized cross-validation. Returns the
/// smoothed values and the λ used.
pub fn smooth_auto(ys: &[f64], weights: &[f64]) -> (Vec<f64>, f64) {
    if ys.len() < 4 {
        return (ys.to_vec(), 0.0);
    }
    let n = ys.len() as f64;
    let mut best = (ys.to_vec(), 0.0, f64::INFINITY);
    for e in -4..=12 {
        let lambda = 10f64.powf(e as f64 / 2.0);
        let (z, trace) = whittaker(ys, weights, lambda);
        let rss: f64 = (0..ys.len())
            .map(|i| weights[i] * (ys[i] - z[i]).powi(2))
            .sum();
        let score = n * rss / (n - trace).powi(2);
        if score < best.2 {
            best = (z, lambda, score);
        }
    }
    (best.0, best.1)
}

/// Solve (W + λ·DᵀD) z = W·y, with D the second-difference matrix. Also
/// returns the trace of the hat matrix (the effective number of parameters).
fn whittaker(ys: &[f64], weights: &[f64], lambda: f64) -> (Vec<f64>, f64) {
    let n = ys.len();
    let mut a = vec![vec![0.0; n]; n];
    for (i, row) in a.iter_mut().enumerate() {
        row[i] = weights[i];
    }
    for k in 0..n - 2 {
        let d = [(k, 1.0), (k + 1, -2.0), (k + 2, 1.0)];
        for &(i, di) in &d {
            for &(j, dj) in &d {
                a[i][j] += lambda * di * dj;
            }
        }
    }
    let inv = invert(a);
    let wy: Vec<f64> = (0..n).map(|i| weights[i] * ys[i]).collect();
    let z = inv
        .iter()
        .map(|row| row.iter().zip(&wy).map(|(a, b)| a * b).sum())
        .collect();
    let trace = (0..n).map(|i| inv[i][i] * weights[i]).sum();
    (z, trace)
}

/// Gauss–Jordan inverse of a small, well-conditioned matrix.
fn invert(mut a: Vec<Vec<f64>>) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap();
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let p = a[col][col];
        for k in 0..n {
            a[col][k] /= p;
            inv[col][k] /= p;
        }
        for row in 0..n {
            let f = a[row][col];
            if row != col && f != 0.0 {
                for k in 0..n {
                    a[row][k] -= f * a[col][k];
                    inv[row][k] -= f * inv[col][k];
                }
            }
        }
    }
    inv
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
    fn smoothing_keeps_lines_and_reduces_noise() {
        let line: Vec<f64> = (0..20).map(|i| 0.5 + 0.1 * i as f64).collect();
        let w = vec![1.0; 20];
        for (a, b) in smooth(&line, &w, 1e3).iter().zip(&line) {
            assert!((a - b).abs() < 1e-9, "a straight line is left alone");
        }
        let noisy: Vec<f64> = (0..20)
            .map(|i| if i % 2 == 0 { 1.01 } else { 0.99 })
            .collect();
        let out = smooth(&noisy, &w, 10.0);
        // Ends have fewer neighbours to average with.
        assert!(
            out[3..17].iter().all(|v| (v - 1.0).abs() < 0.001),
            "{out:?}"
        );
        assert!(out.iter().all(|v| (v - 1.0).abs() < 0.005), "{out:?}");
    }

    #[test]
    fn fit_recovers_gamma_through_noise() {
        let n = 33;
        let xs: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        // Deterministic ±0.5% alternating error.
        let ys: Vec<f64> = xs
            .iter()
            .enumerate()
            .map(|(i, x)| x.powf(2.4) * if i % 2 == 0 { 1.005 } else { 0.995 })
            .collect();
        let raw = ToneCurve::from_samples(&xs, &ys).unwrap();
        let noise = Noise {
            relative: 0.005,
            floor: 0.0,
        };
        let fitted = ToneCurve::fit(&xs, &ys, noise).unwrap();
        let err = |c: &ToneCurve| {
            // Above 0.1: below that, interpolation between samples dominates.
            (10..100)
                .map(|i| {
                    let x = i as f64 / 100.0;
                    (c.eval(x) / x.powf(2.4) - 1.0).abs()
                })
                .fold(0.0, f64::max)
        };
        assert!(
            err(&fitted) < 0.35 * err(&raw),
            "{} vs {}",
            err(&fitted),
            err(&raw)
        );
    }

    #[test]
    fn fit_keeps_detail_that_stands_out_of_the_noise() {
        // A 3% ripple measured with 0.1% noise must not be smoothed away.
        let n = 33;
        let xs: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        let truth = |x: f64| x.powf(2.2) * (1.0 + 0.03 * (8.0 * std::f64::consts::PI * x).sin());
        let ys: Vec<f64> = xs
            .iter()
            .enumerate()
            .map(|(i, &x)| truth(x) * if i % 2 == 0 { 1.001 } else { 0.999 })
            .collect();
        let noise = Noise {
            relative: 0.001,
            floor: 0.0,
        };
        let fitted = ToneCurve::fit(&xs, &ys, noise).unwrap();
        for &x in &xs[8..] {
            let rel = fitted.eval(x) / truth(x) - 1.0;
            assert!(rel.abs() < 0.004, "x={x}: {rel}");
        }
    }

    #[test]
    fn srgb_target_is_continuous() {
        let lo = TargetCurve::Srgb.eval(0.04045);
        let hi = TargetCurve::Srgb.eval(0.04045 + 1e-9);
        assert!((lo - hi).abs() < 1e-7);
        assert_eq!(TargetCurve::Srgb.eval(1.0), 1.0);
    }
}
