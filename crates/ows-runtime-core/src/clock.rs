//! Injectable clocks for deterministic execution.
//!
//! Business logic must never call `std::time::Instant::now()` directly. Instead
//! it goes through the [`Clock`] trait so that tests can drive time explicitly.

use std::time::{Duration, Instant};

/// A clock used by the runtime to measure time and produce timestamps.
#[async_trait::async_trait]
pub trait Clock: Send + Sync {
    /// Returns the current monotonic instant.
    fn now(&self) -> Instant;

    /// Returns the current wall-clock time as an epoch seconds value.
    fn epoch_seconds(&self) -> i64;

    /// Sleeps for the given duration asynchronously.
    async fn sleep(&self, duration: Duration);

    /// Returns whether the clock is a real (non-deterministic) clock.
    fn is_deterministic(&self) -> bool {
        false
    }
}

/// The system clock backed by the real OS clock.
#[derive(Debug, Clone)]
pub struct SystemClock {
    epoch: std::time::SystemTime,
}

impl SystemClock {
    /// Creates a new system clock.
    pub fn new() -> Self {
        Self {
            epoch: std::time::UNIX_EPOCH,
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn epoch_seconds(&self) -> i64 {
        self.epoch
            .elapsed()
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// A deterministic clock that can be advanced manually.
///
/// Real sleeping is replaced by a virtual time counter that tests drive
/// explicitly, so workflow timing logic is fully deterministic.
#[derive(Debug)]
pub struct DeterministicClock {
    start: Instant,
    elapsed: std::sync::atomic::AtomicU64,
}

impl DeterministicClock {
    /// Creates a new deterministic clock starting at epoch zero.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            elapsed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Returns the current virtual elapsed time.
    pub fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.elapsed.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Advances the virtual clock by the given duration.
    pub fn advance(&self, duration: Duration) {
        self.elapsed.fetch_add(
            duration.as_nanos() as u64,
            std::sync::atomic::Ordering::SeqCst,
        );
    }
}

impl Default for DeterministicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DeterministicClock {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            elapsed: std::sync::atomic::AtomicU64::new(
                self.elapsed.load(std::sync::atomic::Ordering::SeqCst),
            ),
        }
    }
}

#[async_trait::async_trait]
impl Clock for DeterministicClock {
    fn now(&self) -> Instant {
        self.start + self.elapsed()
    }

    fn epoch_seconds(&self) -> i64 {
        self.elapsed().as_secs() as i64
    }

    async fn sleep(&self, duration: Duration) {
        // Deterministic clocks do not actually sleep; time is virtual.
        self.advance(duration);
    }

    fn is_deterministic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deterministic_clock_advances() {
        let clock = DeterministicClock::new();
        assert_eq!(clock.epoch_seconds(), 0);
        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.epoch_seconds(), 5);
        clock.sleep(Duration::from_secs(2)).await;
        assert_eq!(clock.epoch_seconds(), 7);
        assert!(clock.is_deterministic());
    }
}
