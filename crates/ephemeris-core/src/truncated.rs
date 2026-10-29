//! The normal distribution truncated below at a floor (design §7).
//!
//! The spend rate `c` is Gaussian in the filter, but spending below zero is
//! not a plausible state for most people. The forecast samples `c` from its
//! Gaussian restricted to `c ≥ floor`, and the report states that
//! distribution's range. Sampling is by inverse CDF: one uniform per draw,
//! so a path's stream layout stays fixed, and it is exact in any tail
//! (rejection sampling would loop without bound when the floor is far
//! above the mean).
//!
//! The transcendental functions come from `libm` (pure Rust), so results
//! are bit-identical natively and in WASM (docs/phase1-design.md §4).

use std::f64::consts::FRAC_1_SQRT_2;

/// Standard normal CDF Φ, accurate in both tails (`erfc`, not `1 − erf`).
pub fn std_normal_cdf(x: f64) -> f64 {
    0.5 * libm::erfc(-x * FRAC_1_SQRT_2)
}

/// Standard normal quantile Φ⁻¹(p) for p in (0, 1); ±∞ at 0 and 1.
/// Wichura's AS241 (PPND16), relative error about 1e-16.
// The coefficients are AS241's as published, to be checkable against it;
// a few carry more digits than f64 keeps, which changes nothing.
#[allow(clippy::excessive_precision)]
pub fn std_normal_quantile(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180625 - q * q;
        return q
            * (((((((2_509.080_928_730_122_7 * r + 33_430.575_583_588_128) * r
                + 67_265.770_927_008_7)
                * r
                + 45_921.953_931_549_87)
                * r
                + 13_731.693_765_509_461)
                * r
                + 1_971.590_950_306_551_3)
                * r
                + 133.141_667_891_784_38)
                * r
                + 3.387_132_872_796_366_5)
            / (((((((5_226.495_278_852_545 * r + 28_729.085_735_721_943) * r
                + 39_307.895_800_092_71)
                * r
                + 21_213.794_301_586_597)
                * r
                + 5_394.196_021_424_751)
                * r
                + 687.187_007_492_057_9)
                * r
                + 42.313_330_701_600_91)
                * r
                + 1.0);
    }
    let tail = if q < 0.0 { p } else { 1.0 - p };
    let r = libm::sqrt(-libm::log(tail));
    let z = if r <= 5.0 {
        let r = r - 1.6;
        (((((((7.745_450_142_783_414e-4 * r + 0.022_723_844_989_269_184) * r
            + 0.241_780_725_177_450_6)
            * r
            + 1.270_458_252_452_368_4)
            * r
            + 3.647_848_324_763_204_5)
            * r
            + 5.769_497_221_460_691)
            * r
            + 4.630_337_846_156_546)
            * r
            + 1.423_437_110_749_683_5)
            / (((((((1.050_750_071_644_416_9e-9 * r + 5.475_938_084_995_345e-4) * r
                + 0.015_198_666_563_616_457)
                * r
                + 0.148_103_976_427_480_08)
                * r
                + 0.689_767_334_985_1)
                * r
                + 1.676_384_830_183_803_8)
                * r
                + 2.053_191_626_637_759)
                * r
                + 1.0)
    } else {
        let r = r - 5.0;
        (((((((2.010_334_399_292_288_1e-7 * r + 2.711_555_568_743_487_6e-5) * r
            + 0.001_242_660_947_388_078_4)
            * r
            + 0.026_532_189_526_576_124)
            * r
            + 0.296_560_571_828_504_9)
            * r
            + 1.784_826_539_917_291_3)
            * r
            + 5.463_784_911_164_114)
            * r
            + 6.657_904_643_501_103)
            / (((((((2.044_263_103_389_939_7e-15 * r + 1.421_511_758_316_446e-7) * r
                + 1.846_318_317_510_054_8e-5)
                * r
                + 7.868_691_311_456_133e-4)
                * r
                + 0.014_875_361_290_850_615)
                * r
                + 0.136_929_880_922_735_8)
                * r
                + 0.599_832_206_555_888)
                * r
                + 1.0)
    };
    if q < 0.0 { -z } else { z }
}

/// `N(mean, sd²)` conditioned on `x ≥ floor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TruncatedNormal {
    pub mean: f64,
    pub sd: f64,
    pub floor: f64,
}

impl TruncatedNormal {
    /// The `p`-quantile, `p` in (0, 1): `mean + sd·Φ⁻¹(Φ(a) + p·(1 − Φ(a)))`
    /// with `a = (floor − mean)/sd`. When `a > 0` the same point is computed
    /// from the upper tail, `−Φ⁻¹((1 − p)·Φ(−a))`, so a floor far above the
    /// mean keeps its precision. With `sd = 0`, or when no mass above the
    /// floor is representable, the answer is `max(mean, floor)`.
    pub fn quantile(&self, p: f64) -> f64 {
        if self.sd <= 0.0 {
            return self.mean.max(self.floor);
        }
        let a = (self.floor - self.mean) / self.sd;
        let z = if a <= 0.0 {
            let below = std_normal_cdf(a);
            std_normal_quantile(below + p * (1.0 - below))
        } else {
            let above = std_normal_cdf(-a);
            if above <= 0.0 {
                return self.floor;
            }
            -std_normal_quantile((1.0 - p) * above)
        };
        // Rounding can land a hair below the floor; the support ends there.
        (self.mean + self.sd * z).max(self.floor)
    }

    /// Probability mass of the untruncated Gaussian below the floor.
    pub fn mass_below(&self) -> f64 {
        if self.sd <= 0.0 {
            return if self.mean < self.floor { 1.0 } else { 0.0 };
        }
        std_normal_cdf((self.floor - self.mean) / self.sd)
    }
}

/// A uniform draw on the open interval (0, 1) from the top 52 random bits:
/// the midpoints `(k + ½)/2⁵²` are all exact in f64, so it is never 0 or 1
/// and the quantile is always finite.
pub fn open01(bits: u64) -> f64 {
    ((bits >> 12) as f64 + 0.5) * (1.0 / (1u64 << 52) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn standard_normal_known_values() {
        assert_eq!(std_normal_cdf(0.0), 0.5);
        assert!(close(std_normal_cdf(1.959_963_984_540_054), 0.975, 1e-15));
        // Deep tail, where 1 − Φ(x) would lose every digit.
        assert!(close(
            std_normal_cdf(-10.0) / 7.619_853_024_160_527e-24,
            1.0,
            1e-12
        ));
        assert_eq!(std_normal_quantile(0.5), 0.0);
        assert!(close(
            std_normal_quantile(0.975),
            1.959_963_984_540_054,
            1e-14
        ));
        assert!(close(
            std_normal_quantile(0.05),
            -1.644_853_626_951_472_2,
            1e-14
        ));
        // The two tail branches (r ≤ 5 and r > 5).
        assert!(close(
            std_normal_quantile(1e-10),
            -6.361_340_902_404_056,
            1e-12
        ));
        assert!(close(
            std_normal_quantile(1e-300),
            -37.047_096_299_361_2,
            1e-9
        ));
        // Reference values from an independent AS241 (Python's
        // statistics.NormalDist.inv_cdf), to catch a mistyped coefficient.
        for (p, z) in [
            (1e-12, -7.034_483_825_301_132),
            (1e-6, -4.753_424_308_822_899),
            (0.01, -2.326_347_874_040_840_8),
            (0.2, -0.841_621_233_572_914_2),
            (0.7, 0.524_400_512_708_040_7),
            (0.99, 2.326_347_874_040_840_8),
        ] {
            assert!(close(std_normal_quantile(p), z, 1e-14 * z.abs()), "{p}");
        }
        assert_eq!(std_normal_quantile(0.0), f64::NEG_INFINITY);
        assert_eq!(std_normal_quantile(1.0), f64::INFINITY);
        // Φ(Φ⁻¹(p)) = p across the range.
        for p in [1e-12, 1e-6, 0.01, 0.2, 0.5, 0.7, 0.99, 1.0 - 1e-9] {
            assert!(
                close(std_normal_cdf(std_normal_quantile(p)) / p, 1.0, 1e-9),
                "{p}"
            );
        }
    }

    /// The Plaid sandbox after its regime changes: ĉ = $67.04/day and the
    /// ±2σ band −$5.04..$139.11, so σ = $36.0375. Expected values computed
    /// separately (bisection on Φ, docs/phase1-design.md §7).
    #[test]
    fn sandbox_spend_band_by_hand() {
        let t = TruncatedNormal {
            mean: 67.04,
            sd: 36.0375,
            floor: 0.0,
        };
        assert!(close(t.mass_below(), 0.031_422_647_9, 1e-9));
        assert!(close(t.quantile(0.02275), 8.939_868_7, 1e-5));
        assert!(close(t.quantile(0.97725), 139.598_689_7, 1e-5));
        assert!(close(t.quantile(0.05), 16.368_714_9, 1e-5));
    }

    #[test]
    fn a_floor_far_below_changes_nothing() {
        let t = TruncatedNormal {
            mean: 60.0,
            sd: 5.0,
            floor: 0.0,
        };
        for p in [0.001, 0.02275, 0.5, 0.97725, 0.999] {
            let plain = 60.0 + 5.0 * std_normal_quantile(p);
            assert!(close(t.quantile(p), plain, 1e-9), "{p}");
        }
    }

    #[test]
    fn floor_above_the_mean_uses_the_upper_tail() {
        // a = 8: Φ(8) rounds to 1 in f64, so the lower-tail formula would
        // return the floor for every p. The upper-tail form keeps the spread.
        let t = TruncatedNormal {
            mean: 0.0,
            sd: 1.0,
            floor: 8.0,
        };
        let (lo, mid, hi) = (t.quantile(0.01), t.quantile(0.5), t.quantile(0.99));
        assert!(8.0 <= lo && lo < mid && mid < hi);
        // Above a high floor the tail is nearly exponential with rate a:
        // median ≈ a + ln 2 / a.
        assert!(close(mid, 8.0 + std::f64::consts::LN_2 / 8.0, 0.01));
        // Nothing representable above the floor: the floor itself.
        let far = TruncatedNormal { floor: 60.0, ..t };
        assert_eq!(far.quantile(0.5), 60.0);
    }

    #[test]
    fn degenerate_and_monotone() {
        let point = TruncatedNormal {
            mean: -3.0,
            sd: 0.0,
            floor: 0.0,
        };
        assert_eq!(point.quantile(0.5), 0.0);
        assert_eq!(point.mass_below(), 1.0);
        let t = TruncatedNormal {
            mean: 10.0,
            sd: 20.0,
            floor: 0.0,
        };
        let qs: Vec<f64> = (1..100).map(|i| t.quantile(f64::from(i) / 100.0)).collect();
        assert!(qs.windows(2).all(|w| w[0] < w[1]));
        assert!(qs[0] >= 0.0);
    }

    #[test]
    fn open01_stays_inside() {
        assert!(open01(0) > 0.0);
        assert!(open01(u64::MAX) < 1.0);
        assert!(close(open01(1 << 63), 0.5, 1e-15));
    }
}
