//! Covariance helpers. Every covariance in the engine is symmetric PSD; these
//! functions enforce and check that.

use nalgebra::{Cholesky, SMatrix};

/// Relative tolerance for symmetry and PSD checks, scaled by the largest entry.
pub const PSD_REL_TOL: f64 = 1e-9;

/// `(P + Pᵀ) / 2`, removing the asymmetry rounding accumulates.
pub fn symmetrize<const N: usize>(p: &SMatrix<f64, N, N>) -> SMatrix<f64, N, N> {
    (p + p.transpose()) * 0.5
}

fn scale<const N: usize>(p: &SMatrix<f64, N, N>) -> f64 {
    p.iter().fold(1.0_f64, |m, v| m.max(v.abs()))
}

pub fn is_symmetric<const N: usize>(p: &SMatrix<f64, N, N>) -> bool {
    let tol = PSD_REL_TOL * scale(p);
    (0..N).all(|i| (0..i).all(|j| (p[(i, j)] - p[(j, i)]).abs() <= tol))
}

/// Symmetric, finite, and no eigenvalue below `−tol` (relative to the largest
/// entry). Tested as: `P + tol·I` has a Cholesky factorization.
pub fn is_psd<const N: usize>(p: &SMatrix<f64, N, N>) -> bool {
    if !p.iter().all(|v| v.is_finite()) || !is_symmetric(p) {
        return false;
    }
    let tol = PSD_REL_TOL * scale(p);
    Cholesky::new(symmetrize(p) + SMatrix::<f64, N, N>::identity() * tol).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Matrix2;

    #[test]
    fn psd_hand_cases() {
        assert!(is_psd(&Matrix2::new(2.0, 1.0, 1.0, 2.0))); // eigenvalues 1, 3
        assert!(is_psd(&Matrix2::new(1.0, 1.0, 1.0, 1.0))); // eigenvalues 0, 2: semidefinite
        assert!(!is_psd(&Matrix2::new(1.0, 2.0, 2.0, 1.0))); // eigenvalues −1, 3
        assert!(!is_psd(&Matrix2::new(1.0, 0.5, 0.4, 1.0))); // asymmetric
        assert!(!is_psd(&Matrix2::new(f64::NAN, 0.0, 0.0, 1.0)));
    }

    #[test]
    fn symmetrize_averages_off_diagonals() {
        let s = symmetrize(&Matrix2::new(1.0, 0.5, 0.3, 2.0));
        assert_eq!(s, Matrix2::new(1.0, 0.4, 0.4, 2.0));
        assert!(is_symmetric(&s));
    }
}
