//! Forecast scoring (design §5, "Forecast calibration").
//!
//! The continuous ranked probability score
//! `CRPS(F, y) = ∫ (F(x) − 1{x ≥ y})² dx` is in currency units, lower is better,
//! and is a proper score for the whole predictive distribution. A point
//! forecast's CRPS is its absolute error, so deterministic and probabilistic
//! forecasts compare on one scale.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use crate::error::EngineError;

/// Standard normal CDF Φ.
pub fn normal_cdf(x: f64) -> f64 {
    0.5 * erfc(-x * FRAC_1_SQRT_2)
}

/// Complementary error function, Chebyshev fit with fractional error below
/// 1.2e-7 everywhere (Numerical Recipes `erfcc`).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98
                                + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let r = t * poly.exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// CRPS of `N(mean, sd²)` at `y`, in closed form:
/// `sd·[z(2Φ(z) − 1) + 2φ(z) − 1/√π]` with `z = (y − mean)/sd`.
/// `sd = 0` gives the absolute error.
pub fn crps_gaussian(mean: f64, sd: f64, y: f64) -> Result<f64, EngineError> {
    if !(mean.is_finite() && y.is_finite() && sd.is_finite() && sd >= 0.0) {
        return Err(EngineError::InvalidInput(
            "CRPS needs finite inputs, sd >= 0",
        ));
    }
    if sd == 0.0 {
        return Ok((y - mean).abs());
    }
    let z = (y - mean) / sd;
    let pdf = (-0.5 * z * z).exp() / (2.0 * PI).sqrt();
    Ok(sd * (z * (2.0 * normal_cdf(z) - 1.0) + 2.0 * pdf - 1.0 / PI.sqrt()))
}

/// The `k` midpoint levels `(i + ½)/k`, `i = 0..k`, that
/// [`crps_quantiles`] expects.
pub fn midpoint_levels(k: usize) -> Vec<f64> {
    (0..k).map(|i| (i as f64 + 0.5) / k as f64).collect()
}

/// CRPS from quantiles at the [`midpoint_levels`] of their count, using
/// `CRPS = 2∫₀¹ ρ_τ(y − q_τ) dτ` with the pinball loss
/// `ρ_τ(u) = u·(τ − 1{u < 0})`, integrated by the midpoint rule. The error
/// is O(1/k²) for a smooth distribution plus the tails beyond `1/(2k)`.
/// Equal quantiles (a point forecast) give the absolute error exactly.
pub fn crps_quantiles(quantiles: &[f64], y: f64) -> Result<f64, EngineError> {
    if quantiles.is_empty() || !y.is_finite() || !quantiles.iter().all(|q| q.is_finite()) {
        return Err(EngineError::InvalidInput(
            "CRPS needs a non-empty finite quantile grid",
        ));
    }
    if quantiles.windows(2).any(|w| w[1] < w[0]) {
        return Err(EngineError::InvalidInput(
            "quantiles must be non-decreasing",
        ));
    }
    let k = quantiles.len() as f64;
    let loss: f64 = quantiles
        .iter()
        .enumerate()
        .map(|(i, &q)| {
            let tau = (i as f64 + 0.5) / k;
            let u = y - q;
            if u < 0.0 { u * (tau - 1.0) } else { u * tau }
        })
        .sum();
    Ok(2.0 * loss / k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn normal_cdf_known_values() {
        assert!(close(normal_cdf(0.0), 0.5, 1e-7));
        assert!(close(normal_cdf(1.959_963_984_540_054), 0.975, 1e-7));
        assert!(close(normal_cdf(-1.0), 0.158_655_253_931_457, 1e-7));
        assert!(close(normal_cdf(1.0) + normal_cdf(-1.0), 1.0, 1e-12));
        assert!(close(normal_cdf(-9.0), 0.0, 1e-15));
    }

    #[test]
    fn crps_gaussian_by_hand() {
        // y = mean: 2φ(0) − 1/√π = 2/√(2π) − 1/√π.
        let at_mean = 0.797_884_560_802_865_4 - 0.564_189_583_547_756_3;
        assert!(close(crps_gaussian(0.0, 1.0, 0.0).unwrap(), at_mean, 1e-12));
        // Linear in scale and location: N(10, 2²) at 10.
        assert!(close(
            crps_gaussian(10.0, 2.0, 10.0).unwrap(),
            2.0 * at_mean,
            1e-12
        ));
        // z = 1: (2Φ(1) − 1) + 2φ(1) − 1/√π
        //      = 0.682689492 + 0.483941449 − 0.564189584 = 0.602441358.
        assert!(close(
            crps_gaussian(0.0, 1.0, 1.0).unwrap(),
            0.602_441_357,
            1e-6
        ));
        // Symmetric in y − mean.
        assert!(close(
            crps_gaussian(0.0, 1.0, -1.0).unwrap(),
            crps_gaussian(0.0, 1.0, 1.0).unwrap(),
            1e-6
        ));
        // Far from the mean it approaches |y − mean| − sd/√π.
        let far = crps_gaussian(0.0, 1.0, 50.0).unwrap();
        assert!(close(far, 50.0 - 1.0 / PI.sqrt(), 1e-9));
    }

    #[test]
    fn crps_gaussian_zero_sd_is_absolute_error() {
        assert_eq!(crps_gaussian(3.0, 0.0, -2.0), Ok(5.0));
    }

    #[test]
    fn crps_gaussian_rejects_bad_input() {
        assert!(crps_gaussian(0.0, -1.0, 0.0).is_err());
        assert!(crps_gaussian(f64::NAN, 1.0, 0.0).is_err());
        assert!(crps_gaussian(0.0, 1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn midpoint_levels_by_hand() {
        assert_eq!(midpoint_levels(4), vec![0.125, 0.375, 0.625, 0.875]);
    }

    #[test]
    fn crps_quantiles_point_forecast_is_absolute_error() {
        for y in [-3.0, 2.0, 7.5] {
            let c = crps_quantiles(&[2.0; 10], y).unwrap();
            assert!(close(c, (y - 2.0_f64).abs(), 1e-12));
        }
    }

    #[test]
    fn crps_quantiles_uniform_by_hand() {
        // U(0, 1) at y = ½: exact CRPS = E|X − y| − ½E|X − X'| = ¼ − ⅙ = 1/12.
        // k = 4, q = τ = ⅛, ⅜, ⅝, ⅞: each pinball loss is 3/64, so
        // CRPS ≈ (2/4)·(4·3/64) = 3/32.
        let q = midpoint_levels(4);
        assert!(close(crps_quantiles(&q, 0.5).unwrap(), 3.0 / 32.0, 1e-15));
        // The midpoint-rule error shrinks as 1/k².
        let q = midpoint_levels(1000);
        assert!(close(crps_quantiles(&q, 0.5).unwrap(), 1.0 / 12.0, 1e-6));
    }

    #[test]
    fn crps_quantiles_matches_gaussian_closed_form() {
        // Gaussian quantiles by bisection on Φ, at the grid the calibration uses.
        let inv = |p: f64| {
            let (mut lo, mut hi) = (-10.0, 10.0);
            for _ in 0..100 {
                let mid = 0.5 * (lo + hi);
                if normal_cdf(mid) < p {
                    lo = mid
                } else {
                    hi = mid
                }
            }
            0.5 * (lo + hi)
        };
        let q: Vec<f64> = midpoint_levels(200).into_iter().map(inv).collect();
        for y in [0.0, 0.7, -1.5, 3.0] {
            let exact = crps_gaussian(0.0, 1.0, y).unwrap();
            let grid = crps_quantiles(&q, y).unwrap();
            assert!(
                (grid - exact).abs() <= 0.005 * exact,
                "y={y}: {grid} vs {exact}"
            );
        }
    }

    #[test]
    fn crps_quantiles_rejects_bad_input() {
        assert!(crps_quantiles(&[], 0.0).is_err());
        assert!(crps_quantiles(&[1.0, 0.0], 0.0).is_err());
        assert!(crps_quantiles(&[0.0, f64::NAN], 0.0).is_err());
        assert!(crps_quantiles(&[0.0], f64::NAN).is_err());
    }
}
