//! Small injectable host boundaries shared by control-plane domains.

use time::OffsetDateTime;

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
