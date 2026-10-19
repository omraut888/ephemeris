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
