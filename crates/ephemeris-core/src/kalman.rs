//! Generic linear Kalman filter over `N` states with scalar measurements
//! (design §1, "Numerics").
//!
//! Measurements are applied one scalar at a time, which is equivalent to a
//! joint update when R is diagonal and needs no matrix inverse. The covariance
//! update uses the Joseph form and is re-symmetrized after every step.

use nalgebra::{SMatrix, SVector};

use crate::error::EngineError;
use crate::linalg::{is_psd, symmetrize};

/// The residual of one scalar update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Innovation {
    /// `z − h·x⁻`.
    pub nu: f64,
    /// Innovation variance `h·P⁻·hᵀ + r`.
    pub s: f64,
    /// Normalized innovation squared `ν²/s`, χ²₁-distributed when the model holds.
    pub nis: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Kf<const N: usize> {
    pub x: SVector<f64, N>,
    pub p: SMatrix<f64, N, N>,
}

impl<const N: usize> Kf<N> {
    /// A filter at prior mean `x` with covariance `p`, which must be symmetric PSD.
    pub fn new(x: SVector<f64, N>, p: SMatrix<f64, N, N>) -> Result<Self, EngineError> {
        if !x.iter().all(|v| v.is_finite()) {
            return Err(EngineError::InvalidInput("prior mean is not finite"));
        }
        if !is_psd(&p) {
            return Err(EngineError::InvalidInput(
                "prior covariance is not symmetric PSD",
            ));
        }
        Ok(Self { x, p })
    }

    /// Time update `x⁻ = A x + B u`, `P⁻ = A P Aᵀ + Q`.
    pub fn predict(
        &mut self,
        a: &SMatrix<f64, N, N>,
        b: &SVector<f64, N>,
        u: f64,
        q: &SMatrix<f64, N, N>,
    ) -> Result<(), EngineError> {
        if !u.is_finite() {
            return Err(EngineError::InvalidInput("control input is not finite"));
        }
        if !is_psd(q) {
            return Err(EngineError::InvalidInput(
                "process noise Q is not symmetric PSD",
            ));
        }
        self.x = a * self.x + b * u;
        self.p = symmetrize(&(a * self.p * a.transpose() + q));
        debug_assert!(is_psd(&self.p));
        Ok(())
    }

    /// The innovation an update with `(h, r, z)` would see, without applying it.
    pub fn innovation(
        &self,
        h: &SVector<f64, N>,
        r: f64,
        z: f64,
    ) -> Result<Innovation, EngineError> {
        if !z.is_finite() {
            return Err(EngineError::InvalidInput("measurement is not finite"));
        }
        if !(r.is_finite() && r > 0.0) {
            return Err(EngineError::InvalidInput(
                "measurement variance must be positive and finite",
            ));
        }
        let s = h.dot(&(self.p * h)) + r;
        if !(s.is_finite() && s > 0.0) {
            return Err(EngineError::InvalidInput(
                "innovation variance is not positive",
            ));
        }
        let nu = z - h.dot(&self.x);
        Ok(Innovation {
            nu,
            s,
            nis: nu * nu / s,
        })
    }

    /// Measurement update for `z = h·x + v`, `v ~ N(0, r)`.
    pub fn update_scalar(
        &mut self,
        h: &SVector<f64, N>,
        r: f64,
        z: f64,
    ) -> Result<Innovation, EngineError> {
        let inn = self.innovation(h, r, z)?;
        let k = self.p * h / inn.s;
        self.x += k * inn.nu;
        // Joseph form: (I − K hᵀ) P (I − K hᵀ)ᵀ + K r Kᵀ stays PSD under rounding.
        let i_kh = SMatrix::<f64, N, N>::identity() - k * h.transpose();
        self.p = symmetrize(&(i_kh * self.p * i_kh.transpose() + k * k.transpose() * r));
        debug_assert!(is_psd(&self.p));
        Ok(inn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::{Matrix1, Matrix2, Vector1, Vector2};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
    }

    fn assert_mat2(p: &Matrix2<f64>, expected: [[f64; 2]; 2]) {
        for i in 0..2 {
            for j in 0..2 {
                assert!(
                    close(p[(i, j)], expected[i][j]),
                    "P[{i}{j}] = {} ≠ {}",
                    p[(i, j)],
                    expected[i][j]
                );
            }
        }
    }

    /// Scalar random walk: A = 1, B = 0, Q = 1, H = 1, R = 2.
    #[test]
    fn one_dimensional_by_hand() {
        let a = Matrix1::new(1.0);
        let b = Vector1::new(0.0);
        let q = Matrix1::new(1.0);
        let h = Vector1::new(1.0);
        let mut kf = Kf::new(Vector1::new(0.0), Matrix1::new(1.0)).unwrap();

        // Step 1: P⁻ = 1 + 1 = 2; S = 2 + 2 = 4; K = 1/2.
        kf.predict(&a, &b, 0.0, &q).unwrap();
        assert_eq!(kf.p[0], 2.0);
        let inn = kf.update_scalar(&h, 2.0, 1.0).unwrap();
        assert_eq!((inn.nu, inn.s, inn.nis), (1.0, 4.0, 0.25));
        // x = 0 + ½·1 = 0.5; P = (1 − ½)²·2 + ¼·2 = 1.
        assert!(close(kf.x[0], 0.5) && close(kf.p[0], 1.0));

        // Step 2: P⁻ = 2; z = 3 → ν = 2.5, S = 4, K = ½ → x = 1.75, P = 1.
        kf.predict(&a, &b, 0.0, &q).unwrap();
        let inn = kf.update_scalar(&h, 2.0, 3.0).unwrap();
        assert!(close(inn.nu, 2.5) && close(inn.s, 4.0));
        assert!(close(kf.x[0], 1.75) && close(kf.p[0], 1.0));

        // Step 3, R = 6: P⁻ = 2, S = 8, K = ¼; z = 1.75 → ν = 0, x unchanged.
        // P = (¾)²·2 + (¼)²·6 = 9/8 + 3/8 = 1.5.
        kf.predict(&a, &b, 0.0, &q).unwrap();
        let inn = kf.update_scalar(&h, 6.0, 1.75).unwrap();
        assert!(close(inn.nu, 0.0) && close(inn.s, 8.0));
        assert!(close(kf.x[0], 1.75) && close(kf.p[0], 1.5));
    }

    /// The balance/spend-rate block of design §1: A = [[1, −1], [0, 1]], B = [1, 0].
    #[test]
    fn two_dimensional_by_hand() {
        let a = Matrix2::new(1.0, -1.0, 0.0, 1.0);
        let b = Vector2::new(1.0, 0.0);
        let q = Matrix2::new(2.0, 0.0, 0.0, 0.5);
        let h = Vector2::new(1.0, 0.0);
        let mut kf = Kf::new(Vector2::new(100.0, 10.0), Matrix2::new(4.0, 0.0, 0.0, 1.0)).unwrap();

        // Predict with u = 5: x⁻ = [100 − 10 + 5, 10] = [95, 10].
        // P⁻ = [[p_b + p_c + σ², −p_c], [−p_c, p_c + q_c]] = [[7, −1], [−1, 1.5]].
        kf.predict(&a, &b, 5.0, &q).unwrap();
        assert_eq!(kf.x, Vector2::new(95.0, 10.0));
        assert_mat2(&kf.p, [[7.0, -1.0], [-1.0, 1.5]]);

        // Observe b = 90 with r = 1: S = 8, K = [7/8, −1/8], ν = −5.
        // The balance fell more than predicted, so the spend rate rises.
        let inn = kf.update_scalar(&h, 1.0, 90.0).unwrap();
        assert_eq!((inn.nu, inn.s), (-5.0, 8.0));
        assert!(close(kf.x[0], 95.0 - 35.0 / 8.0));
        assert!(close(kf.x[1], 10.0 + 5.0 / 8.0));
        // P = P⁻ − K hᵀ P⁻ = [[7/8, −1/8], [−1/8, 11/8]].
        assert_mat2(&kf.p, [[7.0 / 8.0, -1.0 / 8.0], [-1.0 / 8.0, 11.0 / 8.0]]);
    }

    /// g predict-only steps against the closed form
    /// P_bb = p_b + g²p_c + gσ² + q_c·(g−1)g(2g−1)/6, P_bc = −g·p_c − q_c·g(g−1)/2,
    /// P_cc = p_c + g·q_c.
    #[test]
    fn predict_only_matches_closed_form() {
        let a = Matrix2::new(1.0, -1.0, 0.0, 1.0);
        let b = Vector2::new(1.0, 0.0);
        let q = Matrix2::new(2.0, 0.0, 0.0, 0.5);
        let mut kf = Kf::new(Vector2::new(100.0, 10.0), Matrix2::new(4.0, 0.0, 0.0, 1.0)).unwrap();
        for _ in 0..3 {
            kf.predict(&a, &b, 0.0, &q).unwrap();
        }
        // g = 3: 4 + 9 + 6 + 0.5·5 = 21.5; −3 − 0.5·3 = −4.5; 1 + 1.5 = 2.5.
        assert_mat2(&kf.p, [[21.5, -4.5], [-4.5, 2.5]]);
        assert_eq!(kf.x, Vector2::new(70.0, 10.0));

        for g in 1..=60_u32 {
            let mut kf = Kf::new(Vector2::new(0.0, 0.0), Matrix2::new(4.0, 0.0, 0.0, 1.0)).unwrap();
            for _ in 0..g {
                kf.predict(&a, &b, 0.0, &q).unwrap();
            }
            let g = f64::from(g);
            let bb = 4.0 + g * g + 2.0 * g + 0.5 * (g - 1.0) * g * (2.0 * g - 1.0) / 6.0;
            let bc = -g - 0.5 * g * (g - 1.0) / 2.0;
            assert_mat2(&kf.p, [[bb, bc], [bc, 1.0 + 0.5 * g]]);
        }
    }

    #[test]
    fn joseph_form_matches_standard_form() {
        let mut kf = Kf::new(Vector2::new(1.0, 2.0), Matrix2::new(3.0, 0.7, 0.7, 2.0)).unwrap();
        let h = Vector2::new(0.6, -1.3);
        let p0 = kf.p;
        let k = p0 * h / (h.dot(&(p0 * h)) + 0.4);
        let standard = (Matrix2::identity() - k * h.transpose()) * p0;
        kf.update_scalar(&h, 0.4, 5.0).unwrap();
        for (x, y) in kf.p.iter().zip(standard.iter()) {
            assert!(close(*x, *y));
        }
    }

    #[test]
    fn rejects_invalid_inputs() {
        let bad_prior = Kf::new(Vector1::new(0.0), Matrix1::new(-1.0));
        assert!(bad_prior.is_err());
        assert!(Kf::new(Vector1::new(f64::NAN), Matrix1::new(1.0)).is_err());

        let mut kf = Kf::new(Vector1::new(0.0), Matrix1::new(1.0)).unwrap();
        let (a, b, h) = (Matrix1::new(1.0), Vector1::new(1.0), Vector1::new(1.0));
        assert!(kf.predict(&a, &b, f64::NAN, &Matrix1::new(1.0)).is_err());
        assert!(kf.predict(&a, &b, 0.0, &Matrix1::new(-1.0)).is_err());
        assert!(kf.update_scalar(&h, 0.0, 1.0).is_err());
        assert!(kf.update_scalar(&h, 1.0, f64::INFINITY).is_err());
        // Failed calls leave the filter untouched.
        assert_eq!(kf, Kf::new(Vector1::new(0.0), Matrix1::new(1.0)).unwrap());
    }
}
