use chrono::{Duration, Utc};

use super::super::schemas::CandidateMemory;

/// Resolves relative temporal expressions in a candidate's raw text to an
/// absolute `event_at` timestamp.
///
/// Supported phrases: `"yesterday"`, `"last week"`, `"last month"`,
/// `"this morning"`, `"today"`.  If `event_at` is already set the candidate
/// is left unchanged.
#[derive(Debug, Default, Clone)]
pub struct TemporalNormalizer;

impl TemporalNormalizer {
    /// Attempt to fill `candidate.event_at` from relative time words in its
    /// `raw_text`.  No-op if `event_at` is already populated.
    pub fn normalize(&self, candidate: &mut CandidateMemory) {
        if candidate.event_at.is_some() {
            return;
        }
        let lower = candidate.raw_text.to_lowercase();
        let now = Utc::now();

        if lower.contains("yesterday") {
            candidate.event_at = Some(now - Duration::days(1));
        } else if lower.contains("last week") {
            candidate.event_at = Some(now - Duration::days(7));
        } else if lower.contains("last month") {
            candidate.event_at = Some(now - Duration::days(30));
        } else if lower.contains("last year") {
            candidate.event_at = Some(now - Duration::days(365));
        } else if lower.contains("this morning") || lower.contains("today") {
            candidate.event_at = Some(now);
        }
    }
}
