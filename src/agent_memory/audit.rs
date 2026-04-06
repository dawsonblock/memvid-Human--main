use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::clock::Clock;
use super::schemas::AuditEvent;

/// Append-only audit sink.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

/// In-memory audit sink used by tests.
#[derive(Debug, Default, Clone)]
pub struct InMemoryAuditSink {
    events: Arc<Mutex<Vec<AuditEvent>>>,
}

impl InMemoryAuditSink {
    #[must_use]
    pub fn events(&self) -> Vec<AuditEvent> {
        match self.events.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl AuditSink for InMemoryAuditSink {
    fn record(&self, event: AuditEvent) {
        match self.events.lock() {
            Ok(mut guard) => guard.push(event),
            Err(poisoned) => poisoned.into_inner().push(event),
        }
    }
}

/// Helper for emitting append-only audit events.
pub struct AuditLogger {
    clock: Arc<dyn Clock>,
    sink: Arc<dyn AuditSink>,
}

impl AuditLogger {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, sink: Arc<dyn AuditSink>) -> Self {
        Self { clock, sink }
    }

    pub fn emit(&self, mut event: AuditEvent) {
        if event.event_id.is_empty() {
            event.event_id = Uuid::new_v4().to_string();
        }
        event.occurred_at = self.clock.now();
        self.sink.record(event);
    }
}
