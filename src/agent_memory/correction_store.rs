use super::schemas::CorrectionRecord;

/// In-memory store for correction records.
///
/// Corrections are append-only within a session. They can be queried by the
/// memory ID they supersede or by entity+slot to find all corrections that
/// have touched a semantic slot.
#[derive(Debug, Default, Clone)]
pub struct CorrectionStore {
    records: Vec<CorrectionRecord>,
}

impl CorrectionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a new correction record. Returns the correction ID.
    pub fn write_correction(&mut self, record: CorrectionRecord) -> String {
        let id = record.correction_id.clone();
        self.records.push(record);
        id
    }

    /// All corrections that reference a specific durable memory ID.
    #[must_use]
    pub fn get_corrections_for(&self, memory_id: &str) -> Vec<&CorrectionRecord> {
        self.records
            .iter()
            .filter(|r| r.corrects_memory_id.as_deref() == Some(memory_id))
            .collect()
    }

    /// All corrections that have touched the given entity + slot combination.
    #[must_use]
    pub fn get_corrections_by_entity_slot(
        &self,
        entity: &str,
        slot: &str,
    ) -> Vec<&CorrectionRecord> {
        self.records
            .iter()
            .filter(|r| r.entity == entity && r.slot == slot)
            .collect()
    }

    /// All correction records in insertion order.
    #[must_use]
    pub fn all_corrections(&self) -> &[CorrectionRecord] {
        &self.records
    }

    /// Number of stored corrections.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
