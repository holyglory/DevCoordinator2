//! Small injectable host boundaries shared by control-plane domains.

use time::OffsetDateTime;

use std::time::Instant;

/// UTC time source. Domain code receives this interface instead of consulting
/// the host clock directly, which keeps restart and semantic fixtures exact.
pub trait Clock: Send + Sync + 'static {
    fn now_utc(&self) -> OffsetDateTime;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostClock;

impl Clock for HostClock {
    fn now_utc(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Monotonic elapsed-time source used for duration-based lifecycle decisions.
pub trait MonotonicClock: Send + Sync + 'static {
    fn seconds(&self) -> f64;
}

#[derive(Debug)]
pub struct HostMonotonicClock {
    origin: Instant,
}

impl Default for HostMonotonicClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl MonotonicClock for HostMonotonicClock {
    fn seconds(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// Cryptographic randomness source for opaque public identifiers.
pub trait RandomSource: Send + Sync + 'static {
    fn fill(&self, destination: &mut [u8]) -> Result<(), getrandom::Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostRandom;

impl RandomSource for HostRandom {
    fn fill(&self, destination: &mut [u8]) -> Result<(), getrandom::Error> {
        getrandom::fill(destination)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub OffsetDateTime);

#[cfg(test)]
impl Clock for FixedClock {
    fn now_utc(&self) -> OffsetDateTime {
        self.0
    }
}
