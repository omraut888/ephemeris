//! Every method's predictive distribution, scored one way (paper/PLAN.md §2).
//!
//! A forecast is a [`Dist`] for the balance on one day. Coverage of the
//! central intervals, CRPS, PIT and the median all come from it, so the Monte
//! Carlo cone, the Gaussian baselines, the empirical ones and the learned
//! quantile model are compared on identical terms.

use ephemeris_core::metrics::{crps_gaussian, crps_quantiles, midpoint_levels};
use ephemeris_core::truncated::{std_normal_cdf, std_normal_quantile};

/// Central intervals checked.
pub const INTERVALS: [f64; 4] = [0.50, 0.80, 0.90, 0.95];
/// Quantile grid for the CRPS of a quantile forecast (midpoint rule).
pub const CRPS_GRID: usize = 200;

/// The quantile levels a Monte Carlo cone must report so that [`Dist`] can
/// score it: every interval end, the median, and the CRPS grid. Sorted.
pub fn mc_levels() -> Vec<f64> {
    let mut v: Vec<f64> = INTERVALS
        .iter()
        .flat_map(|&c| [(1.0 - c) / 2.0, (1.0 + c) / 2.0])
        .chain([0.5])
        .chain(midpoint_levels(CRPS_GRID))
        .collect();
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

#[derive(Debug, Clone, PartialEq)]
pub enum Dist {
    /// Quantiles `values` at increasing `levels` in (0, 1). Linear in between;
    /// below the first level the first value, above the last the last.
    Quantiles {
        levels: Vec<f64>,
        values: Vec<f64>,
    },
    Gaussian {
        mean: f64,
        sd: f64,
    },
    /// An empirical distribution: equal mass on each (sorted) sample.
    Sample(Vec<f64>),
    /// All mass on one value.
    Point(f64),
}

impl Dist {
    /// A quantile forecast; values are sorted, so crossing quantiles (from a
    /// learned model) are repaired by rearrangement.
    pub fn quantiles(levels: Vec<f64>, mut values: Vec<f64>) -> Self {
        assert_eq!(levels.len(), values.len());
        assert!(levels.windows(2).all(|w| w[0] < w[1]));
        values.sort_by(f64::total_cmp);
        Dist::Quantiles { levels, values }
    }

    pub fn sample(mut xs: Vec<f64>) -> Self {
        assert!(!xs.is_empty());
        xs.sort_by(f64::total_cmp);
        Dist::Sample(xs)
    }

    /// The `p` quantile.
    pub fn quantile(&self, p: f64) -> f64 {
        match self {
            Dist::Point(x) => *x,
            Dist::Gaussian { mean, sd } => mean + sd * std_normal_quantile(p),
            Dist::Sample(xs) => {
                // Inverse of the empirical CDF (the smallest x with F(x) ≥ p).
                let n = xs.len();
                let i = ((p * n as f64).ceil() as usize).clamp(1, n) - 1;
                xs[i]
            }
            Dist::Quantiles { levels, values } => {
                let k = levels.partition_point(|&l| l < p);
                if k == 0 {
                    values[0]
                } else if k == levels.len() {
                    values[k - 1]
                } else if levels[k] == p {
                    values[k]
                } else {
                    let (l0, l1) = (levels[k - 1], levels[k]);
                    let w = (p - l0) / (l1 - l0);
                    values[k - 1] + w * (values[k] - values[k - 1])
                }
            }
        }
    }

    pub fn median(&self) -> f64 {
        self.quantile(0.5)
    }

    /// The central `level` interval.
    pub fn interval(&self, level: f64) -> (f64, f64) {
        (
            self.quantile((1.0 - level) / 2.0),
            self.quantile((1.0 + level) / 2.0),
        )
    }

    /// CRPS at `y`: exact for Gaussian, point and sample forecasts; from the
    /// [`CRPS_GRID`] midpoint quantiles for a quantile forecast.
    pub fn crps(&self, y: f64) -> f64 {
        match self {
            Dist::Point(x) => (y - x).abs(),
            Dist::Gaussian { mean, sd } => crps_gaussian(*mean, *sd, y).expect("finite"),
            Dist::Sample(xs) => crps_sample(xs, y),
            Dist::Quantiles { .. } => {
                let q: Vec<f64> = midpoint_levels(CRPS_GRID)
                    .into_iter()
                    .map(|p| self.quantile(p))
                    .collect();
                crps_quantiles(&q, y).expect("finite, sorted")
            }
        }
    }

    /// The probability integral transform F(y). For a point or sample it is
    /// randomized-free: the mid-point of the jump, `(F(y−) + F(y))/2`.
    pub fn pit(&self, y: f64) -> f64 {
        match self {
            Dist::Point(x) => {
                if y < *x {
                    0.0
                } else if y > *x {
                    1.0
                } else {
                    0.5
                }
            }
            Dist::Gaussian { mean, sd } => {
                if *sd > 0.0 {
                    std_normal_cdf((y - mean) / sd)
                } else {
                    Dist::Point(*mean).pit(y)
                }
            }
            Dist::Sample(xs) => {
                let below = xs.partition_point(|&x| x < y);
                let at_or_below = xs.partition_point(|&x| x <= y);
                (below + at_or_below) as f64 / (2 * xs.len()) as f64
            }
            Dist::Quantiles { levels, values } => {
                // Invert the piecewise-linear quantile function.
                let n = values.len();
                if y < values[0] {
                    return 0.0;
                }
                if y > values[n - 1] {
                    return 1.0;
                }
                let k = values.partition_point(|&v| v < y);
                if values[k] == y {
                    // A flat stretch: the middle of the levels that sit on y.
                    let hi = values.partition_point(|&v| v <= y) - 1;
                    return (levels[k] + levels[hi]) / 2.0;
                }
                let (v0, v1) = (values[k - 1], values[k]);
                levels[k - 1] + (y - v0) / (v1 - v0) * (levels[k] - levels[k - 1])
            }
        }
    }
}

/// Exact CRPS of an empirical distribution: `E|X − y| − ½E|X − X'|`, in
/// O(n) on sorted samples.
fn crps_sample(xs: &[f64], y: f64) -> f64 {
    let n = xs.len() as f64;
    let mean_abs = xs.iter().map(|x| (x - y).abs()).sum::<f64>() / n;
    // Σ_{i<j} (x_j − x_i) = Σ_i x_i (2i − n + 1) for sorted x (0-based).
    let pair: f64 = xs
        .iter()
        .enumerate()
        .map(|(i, x)| x * (2.0 * i as f64 - n + 1.0))
        .sum();
    mean_abs - pair / (n * n)
}

/// One scored forecast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub crps: f64,
    /// Whether `y` is inside each of [`INTERVALS`] (closed interval).
    pub inside: [bool; 4],
    pub width90: f64,
    pub median: f64,
    pub abs_err: f64,
    pub pit: f64,
}

impl Dist {
    pub fn score(&self, y: f64) -> Score {
        let inside = INTERVALS.map(|c| {
            let (lo, hi) = self.interval(c);
            lo <= y && y <= hi
        });
        let (lo, hi) = self.interval(0.90);
        let median = self.median();
        Score {
            crps: self.crps(y),
            inside,
            width90: hi - lo,
            median,
            abs_err: (y - median).abs(),
            pit: self.pit(y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn sample_crps_by_hand() {
        // {0, 2} at y = 1: E|X − 1| = 1, E|X − X'| = (0 + 2 + 2 + 0)/4 = 1.
        assert!(close(crps_sample(&[0.0, 2.0], 1.0), 0.5, 1e-12));
        // A single sample is the absolute error.
        assert!(close(crps_sample(&[3.0], 1.0), 2.0, 1e-12));
        // {1, 2, 4} at y = 0: E|X| = 7/3; pairs 1,3,2 → E|X−X'| = 2·6/9.
        assert!(close(
            crps_sample(&[1.0, 2.0, 4.0], 0.0),
            7.0 / 3.0 - 6.0 / 9.0,
            1e-12
        ));
    }

    #[test]
    fn a_large_sample_matches_the_gaussian_closed_form() {
        let n = 4000;
        let xs: Vec<f64> = (0..n)
            .map(|i| std_normal_quantile((i as f64 + 0.5) / n as f64))
            .collect();
        let s = Dist::sample(xs);
        let g = Dist::Gaussian { mean: 0.0, sd: 1.0 };
        for y in [-2.0, -0.3, 0.0, 1.1] {
            assert!(close(s.crps(y), g.crps(y), 2e-3), "{y}");
            assert!(close(s.pit(y), g.pit(y), 1e-3), "{y}");
        }
    }

    #[test]
    fn quantile_forecast_interpolates_and_scores() {
        let levels = mc_levels();
        let values: Vec<f64> = levels
            .iter()
            .map(|&p| 10.0 + 2.0 * std_normal_quantile(p))
            .collect();
        let q = Dist::quantiles(levels, values);
        let g = Dist::Gaussian {
            mean: 10.0,
            sd: 2.0,
        };
        assert!(close(q.median(), 10.0, 1e-9));
        let (lo, hi) = q.interval(0.9);
        assert!(close(lo, 10.0 - 2.0 * 1.644_853_626_951_472_2, 1e-6));
        assert!(close(hi, 10.0 + 2.0 * 1.644_853_626_951_472_2, 1e-6));
        for y in [6.0, 9.5, 13.0] {
            assert!(close(q.crps(y), g.crps(y), 0.01), "{y}");
            assert!(close(q.pit(y), g.pit(y), 0.005), "{y}");
        }
        // Outside the levels: the end values.
        assert_eq!(q.pit(-100.0), 0.0);
        assert_eq!(q.pit(100.0), 1.0);
    }

    #[test]
    fn crossing_quantiles_are_rearranged() {
        let d = Dist::quantiles(vec![0.1, 0.5, 0.9], vec![3.0, 1.0, 2.0]);
        assert_eq!(d.quantile(0.1), 1.0);
        assert_eq!(d.median(), 2.0);
    }

    #[test]
    fn point_and_sample_coverage() {
        let p = Dist::Point(5.0);
        let s = p.score(5.0);
        assert_eq!(
            (s.crps, s.inside, s.width90, s.pit),
            (0.0, [true; 4], 0.0, 0.5)
        );
        assert_eq!(p.score(6.0).inside, [false; 4]);
        let d = Dist::sample((1..=20).map(f64::from).collect());
        // 90% interval: the 5% and 95% points of 1..20 are 1 and 19.
        assert_eq!(d.interval(0.9), (1.0, 19.0));
        assert!(close(d.pit(10.0), 19.0 / 40.0, 1e-12));
    }
}
