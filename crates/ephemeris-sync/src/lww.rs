//! Last-writer-wins registers and day-interval sets: the two join
//! semilattices `SyncState` is built from (design §2.4).

use ephemeris_core::Day;
use serde::{Deserialize, Serialize};

use crate::hlc::Hlc;

/// A value and the stamp of the write that set it. Join keeps the larger
/// stamp. Stamps are unique per write, so equal stamps must carry equal
/// values; anything else is a corrupt state and the join refuses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lww<T> {
    pub at: Hlc,
    pub value: T,
}

impl<T: Clone + PartialEq> Lww<T> {
    pub fn new(at: Hlc, value: T) -> Self {
        Self { at, value }
    }

    /// `self ⊔ other`.
    pub fn join(&mut self, other: &Lww<T>) -> Result<(), String> {
        if other.at > self.at {
            *self = other.clone();
        } else if other.at == self.at && other.value != self.value {
            return Err(format!(
                "two different writes share the stamp {:?}",
                other.at
            ));
        }
        Ok(())
    }
}

/// A set of days stored as sorted, disjoint, non-adjacent inclusive ranges.
/// Join is set union, so the representation of a set is unique and two
/// replicas holding the same days encode them identically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DayIntervals(Vec<[Day; 2]>);

impl DayIntervals {
    /// The ranges, in order.
    pub fn ranges(&self) -> &[[Day; 2]] {
        &self.0
    }

    /// Whether the ranges are in the unique normal form.
    pub fn is_normal(&self) -> bool {
        self.0.iter().all(|[a, b]| a <= b)
            && self
                .0
                .windows(2)
                .all(|w| i64::from(w[0][1]) + 1 < i64::from(w[1][0]))
    }

    /// Add the days `start..=end` (ignored when `start > end`).
    pub fn insert(&mut self, start: Day, end: Day) {
        if start > end {
            return;
        }
        self.0.push([start, end]);
        self.normalize();
    }

    /// `self ⊔ other`: the union.
    pub fn join(&mut self, other: &DayIntervals) {
        self.0.extend_from_slice(&other.0);
        self.normalize();
    }

    /// The first day of the unbroken run of covered days that contains
    /// `day`, if `day` is covered.
    pub fn run_containing(&self, day: Day) -> Option<Day> {
        self.0
            .iter()
            .find(|[a, b]| *a <= day && day <= *b)
            .map(|[a, _]| *a)
    }

    fn normalize(&mut self) {
        self.0.sort_unstable();
        let mut out: Vec<[Day; 2]> = Vec::with_capacity(self.0.len());
        for &[a, b] in &self.0 {
            match out.last_mut() {
                Some(last) if i64::from(a) <= i64::from(last[1]) + 1 => {
                    last[1] = last[1].max(b);
                }
                _ => out.push([a, b]),
            }
        }
        self.0 = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(l: u64, r: u64) -> Hlc {
        Hlc { l, c: 0, r }
    }

    #[test]
    fn lww_keeps_the_larger_stamp_from_either_side() {
        let old = Lww::new(h(1, 1), "old");
        let new = Lww::new(h(2, 1), "new");
        let mut x = old.clone();
        x.join(&new).unwrap();
        assert_eq!(x, new);
        let mut y = new.clone();
        y.join(&old).unwrap();
        assert_eq!(y, new);
    }

    #[test]
    fn lww_join_is_idempotent_and_refuses_a_reused_stamp() {
        let a = Lww::new(h(1, 1), 5);
        let mut x = a.clone();
        x.join(&a).unwrap();
        assert_eq!(x, a);
        assert!(x.join(&Lww::new(h(1, 1), 6)).is_err());
    }

    fn set(r: &[[Day; 2]]) -> DayIntervals {
        let mut s = DayIntervals::default();
        for [a, b] in r {
            s.insert(*a, *b);
        }
        s
    }

    #[test]
    fn intervals_merge_overlapping_and_adjacent_ranges() {
        let s = set(&[[10, 20], [21, 25], [30, 40], [35, 50], [0, 2]]);
        assert_eq!(s.ranges(), &[[0, 2], [10, 25], [30, 50]]);
        assert!(s.is_normal());
        let s = set(&[[5, 1]]);
        assert!(s.ranges().is_empty());
    }

    #[test]
    fn intervals_union_and_runs() {
        let mut a = set(&[[0, 9], [20, 29]]);
        a.join(&set(&[[10, 15], [40, 41]]));
        assert_eq!(a.ranges(), &[[0, 15], [20, 29], [40, 41]]);
        assert_eq!(a.run_containing(12), Some(0));
        assert_eq!(a.run_containing(29), Some(20));
        assert_eq!(a.run_containing(17), None);
    }

    #[test]
    fn non_normal_forms_are_detected() {
        assert!(!DayIntervals(vec![[3, 1]]).is_normal());
        assert!(!DayIntervals(vec![[0, 5], [6, 9]]).is_normal());
        assert!(!DayIntervals(vec![[6, 9], [0, 2]]).is_normal());
        assert!(DayIntervals(vec![[0, 5], [7, 9]]).is_normal());
    }
}
