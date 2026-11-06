//! Double-submit protection: fingerprint a submission and refuse an identical one inside a window.
//!
//! A [`Fingerprint`] is a single SHA-256 digest over a per-process random [`Salt`], the host, the
//! form's structural fingerprint (`ostler_forms` form fingerprint) and the field values. Only the
//! digest is retained, so the guard holds nothing sensitive; because the salt never leaves the
//! process, digests are not comparable across restarts (the guard is in-memory by design).
//!
//! Limits: values are matched exactly, so a retry that changes any value (a fresh timestamp or
//! CSRF token in a field) is "fresh". The guard cannot know whether the first submit succeeded:
//! call [`Guard::forget`] when it failed validation so the retry is not blocked, and pass `force`
//! when the user insists.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::Read;

use sha2::{Digest, Sha256};

/// Per-process secret mixed into fingerprints.
#[derive(Clone)]
pub struct Salt([u8; 32]);

impl std::fmt::Debug for Salt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Salt(..)")
    }
}

impl Salt {
    /// Fresh random salt: 32 bytes from the OS (`/dev/urandom` where it exists), always mixed
    /// with the standard library's randomly keyed hasher, the time and the process id so a
    /// missing `/dev/urandom` degrades to weaker but still per-process randomness.
    pub fn random() -> Salt {
        let mut os = [0u8; 32];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut os);
        }
        let mut h = Sha256::new();
        h.update(os);
        for _ in 0..4 {
            let mut x = RandomState::new().build_hasher();
            x.write_u64(0x6f73_746c_6572);
            h.update(x.finish().to_le_bytes());
        }
        h.update(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
                .to_le_bytes(),
        );
        h.update(std::process::id().to_le_bytes());
        Salt(h.finalize().into())
    }

    /// Fixed salt, for tests.
    pub fn from_bytes(b: [u8; 32]) -> Salt {
        Salt(b)
    }
}

/// Opaque digest of a submission.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fingerprint({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl Fingerprint {
    /// Fingerprint a submission. `values` are `(field key, value)` pairs; order does not matter.
    /// Host is case-insensitive. Every part is length-prefixed so boundaries cannot be forged.
    pub fn new(
        salt: &Salt,
        host: &str,
        form_fingerprint: &str,
        values: &[(&str, &str)],
    ) -> Fingerprint {
        let mut pairs: Vec<(&str, &str)> = values.to_vec();
        pairs.sort();
        let mut h = Sha256::new();
        let mut part = |s: &str| {
            h.update((s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        part(&host.trim().to_lowercase());
        part(form_fingerprint);
        for (k, v) in pairs {
            part(k);
            part(v);
        }
        let mut out = Sha256::new();
        out.update(salt.0);
        out.update(h.finalize());
        Fingerprint(out.finalize().into())
    }
}

/// Answer of [`Guard::check_and_record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardVerdict {
    /// Not seen within the window; now recorded.
    Fresh,
    /// Seen `age_ms` ago (measured from the first accepted submit; duplicates do not extend it).
    Duplicate {
        /// Milliseconds since the original.
        age_ms: u64,
    },
}

/// Bounded in-memory duplicate detector.
#[derive(Debug)]
pub struct Guard {
    window_ms: u64,
    capacity: usize,
    seen: HashMap<Fingerprint, u64>,
    order: VecDeque<(Fingerprint, u64)>,
}

impl Guard {
    /// `window_ms`: how long a submission blocks an identical one. `capacity`: most fingerprints
    /// kept (at least 1); when full the oldest is evicted even if still inside the window, which
    /// weakens the guarantee only under extreme submit volume.
    pub fn new(window_ms: u64, capacity: usize) -> Guard {
        Guard {
            window_ms,
            capacity: capacity.max(1),
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Default: 5 minutes, 1024 entries.
    pub fn with_defaults() -> Guard {
        Guard::new(5 * 60 * 1000, 1024)
    }

    /// Number of fingerprints currently held.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// True when nothing is held.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn evict(&mut self, now: u64) {
        while let Some((f, t)) = self.order.front() {
            let (fp, ts) = (f.clone(), *t);
            let expired = now.saturating_sub(ts) >= self.window_ms;
            let over = self.order.len() > self.capacity;
            if !(expired || over) {
                break;
            }
            if self.seen.get(&fp) == Some(&ts) {
                self.seen.remove(&fp);
            }
            self.order.pop_front();
        }
    }

    /// Check, and record when fresh. `now_ms` is any monotonic millisecond clock.
    pub fn check_and_record(&mut self, fp: &Fingerprint, now_ms: u64) -> GuardVerdict {
        self.check_and_record_with(fp, now_ms, false)
    }

    /// Like [`Guard::check_and_record`]; with `force` the submission is always `Fresh` and
    /// (re)recorded, restarting its window.
    pub fn check_and_record_with(
        &mut self,
        fp: &Fingerprint,
        now_ms: u64,
        force: bool,
    ) -> GuardVerdict {
        self.evict(now_ms);
        if !force {
            if let Some(ts) = self.seen.get(fp) {
                let age = now_ms.saturating_sub(*ts);
                if age < self.window_ms {
                    return GuardVerdict::Duplicate { age_ms: age };
                }
            }
        }
        self.seen.insert(fp.clone(), now_ms);
        self.order.push_back((fp.clone(), now_ms));
        self.evict(now_ms);
        GuardVerdict::Fresh
    }

    /// Drop a fingerprint, e.g. because the submit was rejected by validation and a retry is legitimate.
    pub fn forget(&mut self, fp: &Fingerprint) {
        self.seen.remove(fp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salt() -> Salt {
        Salt::from_bytes([7; 32])
    }

    fn fp(v: &str) -> Fingerprint {
        Fingerprint::new(
            &salt(),
            "Shop.test",
            "abcd",
            &[("email", "a@b.c"), ("qty", v)],
        )
    }

    #[test]
    fn fingerprint_properties() {
        assert_eq!(fp("1"), fp("1"));
        assert_ne!(fp("1"), fp("2"));
        let a = Fingerprint::new(&salt(), "h", "f", &[("a", "1"), ("b", "2")]);
        let b = Fingerprint::new(&salt(), "H", "f", &[("b", "2"), ("a", "1")]);
        assert_eq!(a, b, "order and host case do not matter");
        // boundary forgery: ("ab","c") vs ("a","bc")
        assert_ne!(
            Fingerprint::new(&salt(), "h", "f", &[("ab", "c")]),
            Fingerprint::new(&salt(), "h", "f", &[("a", "bc")])
        );
        assert_ne!(
            Fingerprint::new(&salt(), "h", "f", &[("a", "1")]),
            Fingerprint::new(&salt(), "h2", "f", &[("a", "1")])
        );
        assert_ne!(
            Fingerprint::new(&salt(), "h", "f", &[("a", "1")]),
            Fingerprint::new(&Salt::from_bytes([8; 32]), "h", "f", &[("a", "1")])
        );
    }

    #[test]
    fn random_salts_differ_and_debug_hides() {
        let (a, b) = (Salt::random(), Salt::random());
        assert_ne!(a.0, b.0);
        assert_eq!(format!("{a:?}"), "Salt(..)");
        let d = format!("{:?}", fp("1"));
        assert!(d.len() < 30);
    }

    #[test]
    fn window_behaviour() {
        let mut g = Guard::new(1000, 10);
        let f = fp("1");
        assert_eq!(g.check_and_record(&f, 100), GuardVerdict::Fresh);
        assert_eq!(
            g.check_and_record(&f, 600),
            GuardVerdict::Duplicate { age_ms: 500 }
        );
        assert_eq!(
            g.check_and_record(&f, 1099),
            GuardVerdict::Duplicate { age_ms: 999 }
        );
        // duplicates did not extend the window
        assert_eq!(g.check_and_record(&f, 1100), GuardVerdict::Fresh);
        assert_eq!(
            g.check_and_record(&f, 1200),
            GuardVerdict::Duplicate { age_ms: 100 }
        );
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn different_submissions_are_independent() {
        let mut g = Guard::new(1000, 10);
        assert_eq!(g.check_and_record(&fp("1"), 0), GuardVerdict::Fresh);
        assert_eq!(g.check_and_record(&fp("2"), 1), GuardVerdict::Fresh);
    }

    #[test]
    fn force_and_forget() {
        let mut g = Guard::new(1000, 10);
        let f = fp("1");
        g.check_and_record(&f, 0);
        assert_eq!(g.check_and_record_with(&f, 500, true), GuardVerdict::Fresh);
        // forced record restarted the window
        assert_eq!(
            g.check_and_record(&f, 1400),
            GuardVerdict::Duplicate { age_ms: 900 }
        );
        g.forget(&f);
        assert_eq!(g.check_and_record(&f, 1401), GuardVerdict::Fresh);
    }

    #[test]
    fn memory_is_bounded() {
        let mut g = Guard::new(u64::MAX / 2, 5);
        for i in 0..100u64 {
            g.check_and_record(&fp(&i.to_string()), i);
        }
        assert!(g.len() <= 5);
        assert!(g.order.len() <= 6);
        // oldest evicted (documented), newest kept
        assert!(matches!(
            g.check_and_record(&fp("99"), 100),
            GuardVerdict::Duplicate { .. }
        ));
        assert_eq!(g.check_and_record(&fp("0"), 101), GuardVerdict::Fresh);
    }

    #[test]
    fn expired_entries_are_dropped() {
        let mut g = Guard::new(10, 100);
        for i in 0..50u64 {
            g.check_and_record(&fp(&i.to_string()), i);
        }
        g.check_and_record(&fp("x"), 1000);
        assert_eq!(g.len(), 1);
        assert!(!g.is_empty());
    }

    #[test]
    fn clock_going_backwards_is_duplicate_not_panic() {
        let mut g = Guard::new(1000, 10);
        let f = fp("1");
        g.check_and_record(&f, 5000);
        assert_eq!(
            g.check_and_record(&f, 10),
            GuardVerdict::Duplicate { age_ms: 0 }
        );
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn never_fresh_twice_in_window(
            window in 1u64..10_000,
            keys in proptest::collection::vec(0u8..6, 1..60),
            gaps in proptest::collection::vec(0u64..2_000, 60),
        ) {
            let salt = Salt::from_bytes([1; 32]);
            let mut g = Guard::new(window, 1024);
            let mut last_fresh: HashMap<u8, u64> = HashMap::new();
            let mut now = 0u64;
            for (i, k) in keys.iter().enumerate() {
                now += gaps[i];
                let f = Fingerprint::new(&salt, "h", "f", &[("k", &k.to_string())]);
                let v = g.check_and_record(&f, now);
                if let Some(prev) = last_fresh.get(k) {
                    if now - prev < window {
                        prop_assert!(matches!(v, GuardVerdict::Duplicate { .. }), "fresh twice within window");
                    }
                }
                if v == GuardVerdict::Fresh {
                    last_fresh.insert(*k, now);
                }
            }
        }

        #[test]
        fn fingerprint_ignores_order(mut vals in proptest::collection::vec(("[a-z]{1,4}", "[ -~]{0,8}"), 1..6)) {
            let salt = Salt::from_bytes([3; 32]);
            let a: Vec<(&str, &str)> = vals.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let fa = Fingerprint::new(&salt, "h", "f", &a);
            vals.reverse();
            let b: Vec<(&str, &str)> = vals.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            prop_assert_eq!(fa, Fingerprint::new(&salt, "h", "f", &b));
        }
    }
}
