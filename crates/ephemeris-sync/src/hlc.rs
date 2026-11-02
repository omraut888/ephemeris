//! The hybrid logical clock that orders every write (design §2.5).
//!
//! A stamp is `(l, c, replica)`, compared in that order. `l` is the largest
//! physical time (ms) seen, `c` orders events within one `l`, and the
//! replica id breaks ties, so two distinct writes never compare equal.
//! Wall time is only ever a lower bound for `l`. It is never compared on its
//! own, so a skewed clock cannot reorder writes that causally follow each
//! other.

use serde::{Deserialize, Serialize};

/// One replica: a random 64-bit id per install.
pub type ReplicaId = u64;

/// A hybrid logical clock stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    /// Milliseconds: the largest physical time seen when this was stamped.
    pub l: u64,
    /// Orders stamps that share `l`.
    pub c: u32,
    /// The replica that stamped it.
    pub r: ReplicaId,
}

impl Hlc {
    /// The stamp for a local write by `replica` at physical time `now_ms`,
    /// after `seen`: the largest stamp the replica has issued or merged.
    ///
    /// `l' = max(l, now)`; `c' = c + 1` when `l` did not move, else 0. At
    /// `c == u32::MAX` the counter cannot grow, so `l` advances 1 ms instead
    /// (it may run ahead of physical time; that is allowed).
    pub fn tick(seen: Option<Hlc>, replica: ReplicaId, now_ms: u64) -> Hlc {
        let Some(seen) = seen else {
            return Hlc {
                l: now_ms,
                c: 0,
                r: replica,
            };
        };
        if now_ms > seen.l {
            Hlc {
                l: now_ms,
                c: 0,
                r: replica,
            }
        } else if seen.c == u32::MAX {
            Hlc {
                l: seen.l + 1,
                c: 0,
                r: replica,
            }
        } else {
            Hlc {
                l: seen.l,
                c: seen.c + 1,
                r: replica,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ReplicaId = 1;
    const B: ReplicaId = 2;
    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 24 * HOUR;

    fn h(l: u64, c: u32, r: ReplicaId) -> Hlc {
        Hlc { l, c, r }
    }

    #[test]
    fn first_stamp_is_physical_time() {
        assert_eq!(Hlc::tick(None, A, 1000), h(1000, 0, A));
    }

    #[test]
    fn physical_time_ahead_resets_the_counter() {
        assert_eq!(Hlc::tick(Some(h(1000, 7, A)), A, 1001), h(1001, 0, A));
    }

    #[test]
    fn same_or_earlier_physical_time_counts_up() {
        assert_eq!(Hlc::tick(Some(h(1000, 7, A)), A, 1000), h(1000, 8, A));
        assert_eq!(Hlc::tick(Some(h(1000, 7, A)), A, 10), h(1000, 8, A));
    }

    #[test]
    fn counter_overflow_advances_l() {
        assert_eq!(
            Hlc::tick(Some(h(1000, u32::MAX, A)), A, 1000),
            h(1001, 0, A)
        );
    }

    #[test]
    fn order_is_l_then_c_then_replica() {
        assert!(h(1, 9, 9) < h(2, 0, 0));
        assert!(h(1, 1, 9) < h(1, 2, 0));
        assert!(h(1, 1, A) < h(1, 1, B));
    }

    /// Guarantee 3: a clock that jumps back never makes stamps go backward.
    #[test]
    fn a_backward_jump_keeps_stamps_increasing() {
        let mut last = Hlc::tick(None, A, 10 * DAY);
        for now in [10 * DAY + 5, 3 * DAY, 3 * DAY + 1, 0, 10 * DAY + 5] {
            let next = Hlc::tick(Some(last), A, now);
            assert!(next > last, "{next:?} after {last:?}");
            last = next;
        }
    }

    /// Guarantee 1, by hand: B's clock is an hour (or a day) behind A's.
    /// B merges A's write, then writes; B's write is later in HLC order.
    #[test]
    fn causality_beats_a_slow_clock() {
        for skew in [HOUR, DAY] {
            // A writes at true time T; B (replica id smaller, so a tie would
            // go to A) writes a minute later by true time, reading T + 1 min - skew.
            let a = Hlc::tick(None, B, 100 * DAY);
            let b = Hlc::tick(Some(a), A, 100 * DAY + 60_000 - skew);
            assert_eq!(b, h(100 * DAY, 1, A));
            assert!(b > a, "skew {skew}: {b:?} vs {a:?}");
        }
    }

    /// Guarantee 1, the other way round: the *earlier* writer's clock is
    /// fast, so its stamp is far in the future; a later causal write still
    /// wins (by the counter, not by time).
    #[test]
    fn causality_beats_a_fast_clock() {
        let fast = Hlc::tick(None, A, 100 * DAY + DAY);
        let after = Hlc::tick(Some(fast), B, 100 * DAY + 1);
        assert_eq!(after, h(101 * DAY, 1, B));
        assert!(after > fast);
    }

    /// Guarantee 2: concurrent writes at the same physical time are ordered
    /// by replica id, and identically from both sides.
    #[test]
    fn concurrent_ties_break_on_replica() {
        let a = Hlc::tick(None, A, 5000);
        let b = Hlc::tick(None, B, 5000);
        assert!(a < b);
        assert_eq!(a.max(b), b.max(a));
    }
}
