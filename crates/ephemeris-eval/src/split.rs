//! The development / held-out split of real accounts (paper/PLAN.md §2, §8).
//!
//! Membership is a fixed function of the account id: a SplitMix64 hash of
//! the id XOR a salt, so the split never depends on the data, its order, or
//! a random seed, and anyone can recompute it. 20% of accounts are
//! development; the rest are held out. Ablations during development use a
//! further 25% of the development accounts, chosen the same way with a
//! second salt.
//!
//! The salts were fixed on 2026-11-13, before any forecast was run on real
//! data. Changing them would re-draw the split, so a test pins the split's
//! first members.

/// Salt of the dev / held-out split.
pub const SPLIT_SALT: u64 = 0x5048_4153_4536_2d31; // "PHASE6-1"
/// Salt of the 25% ablation subsample of development accounts.
pub const ABLATION_SALT: u64 = 0x5048_4153_4536_2d32; // "PHASE6-2"

/// SplitMix64's finalizer (Steele, Lea and Flood, 2014).
pub fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Dev,
    HeldOut,
}

pub fn split_of(account_id: u64) -> Split {
    if splitmix64(SPLIT_SALT ^ account_id) % 100 < 20 {
        Split::Dev
    } else {
        Split::HeldOut
    }
}

/// Whether a development account is in the 25% ablation subsample.
pub fn in_ablation_sample(account_id: u64) -> bool {
    split_of(account_id) == Split::Dev && splitmix64(ABLATION_SALT ^ account_id).is_multiple_of(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_reference_values() {
        // The first outputs of the SplitMix64 generator seeded with 0 are
        // splitmix64(0), splitmix64(γ), … with γ = 0x9e3779b97f4a7c15.
        assert_eq!(splitmix64(0), 0xe220_a839_7b1d_cdaf);
        assert_eq!(splitmix64(0x9e37_79b9_7f4a_7c15), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn about_a_fifth_is_development_and_the_split_is_pinned() {
        let dev: Vec<u64> = (1..=20_000)
            .filter(|&a| split_of(a) == Split::Dev)
            .collect();
        let share = dev.len() as f64 / 20_000.0;
        assert!((share - 0.2).abs() < 0.01, "{share}");
        // Pinned: a changed salt or hash would re-draw the split.
        assert_eq!(&dev[..8], &PINNED_FIRST_DEV);
        let abl = dev.iter().filter(|&&a| in_ablation_sample(a)).count() as f64;
        assert!((abl / dev.len() as f64 - 0.25).abs() < 0.03);
    }

    const PINNED_FIRST_DEV: [u64; 8] = [4, 9, 21, 24, 30, 33, 35, 37];
}
