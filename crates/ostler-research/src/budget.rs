//! Crawl budgets and per-host politeness.
//!
//! [`BudgetTracker`] reports *which* limit was hit and remembers the first one (sticky); it never
//! truncates silently: callers must surface [`LimitHit`] to the agent. [`HostLimiter`] bounds
//! concurrent requests per host and enforces a minimum delay between request starts.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// Limits for one research session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Maximum pages fetched.
    pub max_pages: usize,
    /// Maximum total bytes downloaded.
    pub max_bytes: u64,
    /// Maximum wall-clock seconds from tracker creation.
    pub max_secs: u64,
    /// Maximum simultaneous requests to one host.
    pub per_host_concurrency: usize,
    /// Minimum milliseconds between request starts to one host.
    pub per_host_delay_ms: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_pages: 20,
            max_bytes: 20 * 1024 * 1024,
            max_secs: 120,
            per_host_concurrency: 2,
            per_host_delay_ms: 1000,
        }
    }
}

/// Which limit stopped the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    /// `max_pages` reached.
    Pages,
    /// `max_bytes` exceeded.
    Bytes,
    /// `max_secs` elapsed.
    Time,
}

impl fmt::Display for LimitHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pages => "page budget exhausted",
            Self::Bytes => "byte budget exhausted",
            Self::Time => "time budget exhausted",
        })
    }
}

/// Point-in-time usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Pages started.
    pub pages: u64,
    /// Bytes recorded.
    pub bytes: u64,
    /// Seconds since the tracker started.
    pub elapsed_secs: u64,
    /// First limit hit, if any.
    pub hit: Option<LimitHit>,
}

/// Thread-safe usage counter against a [`Budget`].
#[derive(Debug)]
pub struct BudgetTracker {
    budget: Budget,
    started: Instant,
    pages: AtomicU64,
    bytes: AtomicU64,
    hit: Mutex<Option<LimitHit>>,
}

impl BudgetTracker {
    /// Start tracking now.
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            started: Instant::now(),
            pages: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            hit: Mutex::new(None),
        }
    }

    /// The configured budget.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    fn mark(&self, h: LimitHit) -> LimitHit {
        if let Ok(mut g) = self.hit.lock() {
            g.get_or_insert(h);
        }
        h
    }

    /// First limit that was hit, if any (sticky).
    pub fn hit(&self) -> Option<LimitHit> {
        self.hit.lock().ok().and_then(|g| *g)
    }

    /// Check time and bytes without consuming anything.
    pub fn check(&self) -> Result<(), LimitHit> {
        if self.started.elapsed() >= Duration::from_secs(self.budget.max_secs) {
            return Err(self.mark(LimitHit::Time));
        }
        if self.bytes.load(Ordering::SeqCst) >= self.budget.max_bytes {
            return Err(self.mark(LimitHit::Bytes));
        }
        Ok(())
    }

    /// Atomically reserve one page. `Err` names the limit that refused it.
    pub fn try_start_page(&self) -> Result<(), LimitHit> {
        self.check()?;
        let max = self.budget.max_pages as u64;
        let mut cur = self.pages.load(Ordering::SeqCst);
        loop {
            if cur >= max {
                return Err(self.mark(LimitHit::Pages));
            }
            match self
                .pages
                .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Ok(()),
                Err(actual) => cur = actual,
            }
        }
    }

    /// Bytes still available; use it to cap a download so truncation is explicit.
    pub fn remaining_bytes(&self) -> u64 {
        self.budget
            .max_bytes
            .saturating_sub(self.bytes.load(Ordering::SeqCst))
    }

    /// Count downloaded bytes. `Err(Bytes)` if the total now exceeds the budget (the bytes are
    /// still counted; the caller must mark that page truncated or over budget).
    pub fn record_bytes(&self, n: u64) -> Result<(), LimitHit> {
        let total = self.bytes.fetch_add(n, Ordering::SeqCst).saturating_add(n);
        if total > self.budget.max_bytes {
            Err(self.mark(LimitHit::Bytes))
        } else {
            Ok(())
        }
    }

    /// Current usage.
    pub fn usage(&self) -> Usage {
        Usage {
            pages: self.pages.load(Ordering::SeqCst),
            bytes: self.bytes.load(Ordering::SeqCst),
            elapsed_secs: self.started.elapsed().as_secs(),
            hit: self.hit(),
        }
    }
}

struct HostState {
    sem: Arc<Semaphore>,
    last_start: tokio::sync::Mutex<Option<Instant>>,
}

/// Per-host concurrency cap and minimum spacing between request starts.
pub struct HostLimiter {
    concurrency: usize,
    delay: Duration,
    hosts: Mutex<HashMap<String, Arc<HostState>>>,
}

/// Held while a request to a host is in flight; dropping it frees the slot.
#[derive(Debug)]
pub struct HostPermit {
    _permit: Option<OwnedSemaphorePermit>,
}

impl fmt::Debug for HostLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostLimiter")
            .field("concurrency", &self.concurrency)
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

impl HostLimiter {
    /// Limiter with the given per-host concurrency (minimum 1) and spacing.
    pub fn new(concurrency: usize, delay: Duration) -> Self {
        Self {
            concurrency: concurrency.max(1),
            delay,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Limiter from a [`Budget`]'s per-host fields.
    pub fn from_budget(b: &Budget) -> Self {
        Self::new(
            b.per_host_concurrency,
            Duration::from_millis(b.per_host_delay_ms),
        )
    }

    /// Wait for a slot on `host` (case-insensitive) and for the minimum delay since the previous
    /// request start to that host.
    pub async fn acquire(&self, host: &str) -> HostPermit {
        let state = {
            let mut m = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
            m.entry(host.to_ascii_lowercase())
                .or_insert_with(|| {
                    Arc::new(HostState {
                        sem: Arc::new(Semaphore::new(self.concurrency)),
                        last_start: tokio::sync::Mutex::new(None),
                    })
                })
                .clone()
        };
        // The semaphore is never closed, so this is always Some; if it ever were, proceed without a slot.
        let permit = state.sem.clone().acquire_owned().await.ok();
        let mut last = state.last_start.lock().await;
        if let Some(prev) = *last {
            let earliest = prev + self.delay;
            if earliest > Instant::now() {
                tokio::time::sleep_until(earliest).await;
            }
        }
        *last = Some(Instant::now());
        HostPermit { _permit: permit }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(pages: usize, bytes: u64, secs: u64) -> Budget {
        Budget {
            max_pages: pages,
            max_bytes: bytes,
            max_secs: secs,
            ..Budget::default()
        }
    }

    #[test]
    fn pages_limit_is_reported_and_sticky() {
        let t = BudgetTracker::new(b(2, 1000, 60));
        assert!(t.try_start_page().is_ok());
        assert!(t.try_start_page().is_ok());
        assert_eq!(t.try_start_page(), Err(LimitHit::Pages));
        assert_eq!(t.hit(), Some(LimitHit::Pages));
        assert_eq!(t.usage().pages, 2);
    }

    #[test]
    fn bytes_limit_counts_overshoot() {
        let t = BudgetTracker::new(b(10, 100, 60));
        assert!(t.record_bytes(60).is_ok());
        assert_eq!(t.remaining_bytes(), 40);
        assert_eq!(t.record_bytes(60), Err(LimitHit::Bytes));
        assert_eq!(t.remaining_bytes(), 0);
        assert_eq!(t.try_start_page(), Err(LimitHit::Bytes));
        assert_eq!(t.hit(), Some(LimitHit::Bytes));
        assert_eq!(t.usage().bytes, 120);
    }

    #[tokio::test(start_paused = true)]
    async fn time_limit() {
        let t = BudgetTracker::new(b(10, 1000, 5));
        assert!(t.check().is_ok());
        tokio::time::advance(Duration::from_secs(6)).await;
        assert_eq!(t.check(), Err(LimitHit::Time));
        assert_eq!(t.try_start_page(), Err(LimitHit::Time));
        assert_eq!(t.usage().hit, Some(LimitHit::Time));
    }

    #[test]
    fn first_hit_wins_and_concurrent_pages_never_exceed() {
        let t = Arc::new(BudgetTracker::new(b(50, 1000, 60)));
        let ok = Arc::new(AtomicU64::new(0));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (t, ok) = (t.clone(), ok.clone());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        if t.try_start_page().is_ok() {
                            ok.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(ok.load(Ordering::SeqCst), 50);
        let _ = t.record_bytes(5000);
        assert_eq!(t.hit(), Some(LimitHit::Pages), "first hit stays");
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_spaces_requests_per_host() {
        let l = HostLimiter::new(4, Duration::from_millis(500));
        let start = Instant::now();
        let _a = l.acquire("Example.com").await;
        assert_eq!(start.elapsed(), Duration::ZERO);
        let _b = l.acquire("example.com").await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
        let _c = l.acquire("other.com").await;
        assert_eq!(
            start.elapsed(),
            Duration::from_millis(500),
            "other host independent"
        );
        let _d = l.acquire("example.com").await;
        assert_eq!(start.elapsed(), Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_caps_concurrency() {
        let l = Arc::new(HostLimiter::new(2, Duration::ZERO));
        let p1 = l.acquire("h").await;
        let _p2 = l.acquire("h").await;
        let l2 = l.clone();
        let third = tokio::spawn(async move { l2.acquire("h").await });
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!third.is_finished(), "third waits for a free slot");
        drop(p1);
        let _p3 = third.await.unwrap();
    }
}
