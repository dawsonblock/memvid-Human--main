use super::enums::SourceType;

const CORROBORATION_BOOST_PER_MATCH: f32 = 0.08;
const CORROBORATION_BOOST_CAP: f32 = 0.25;
const CONTRADICTION_PENALTY_PER_MATCH: f32 = 0.10;
const CONTRADICTION_PENALTY_CAP: f32 = 0.30;

/// Deterministic source trust weights.
#[must_use]
pub const fn source_weight(source_type: SourceType) -> f32 {
    match source_type {
        SourceType::System => 1.0,
        SourceType::File => 0.9,
        SourceType::Chat => 0.75,
        SourceType::Tool => 0.6,
        SourceType::External => 0.45,
    }
}

#[must_use]
pub fn effective_source_weight(
    base_weight: f32,
    corroborating_evidence_count: usize,
    contradictory_evidence_count: usize,
    verified_source: bool,
) -> f32 {
    if verified_source {
        return 1.0;
    }

    let corroboration_boost = (corroborating_evidence_count as f32 * CORROBORATION_BOOST_PER_MATCH)
        .min(CORROBORATION_BOOST_CAP);
    let contradiction_penalty = (contradictory_evidence_count as f32
        * CONTRADICTION_PENALTY_PER_MATCH)
        .min(CONTRADICTION_PENALTY_CAP);

    (base_weight + corroboration_boost - contradiction_penalty).clamp(0.0, 1.0)
}
