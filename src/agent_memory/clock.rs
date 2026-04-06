use chrono::{DateTime, Utc};

/// Clock abstraction used for deterministic tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// System UTC clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Fixed clock for tests.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: DateTime<Utc>,
}

impl FixedClock {
    #[must_use]
    pub fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.now
    }
}
