mod common;

use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, PromotionDecision, SourceType};
use memvid_core::agent_memory::memory_promoter::MemoryPromoter;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::schemas::PromotionContext;

use common::{candidate, ts};

#[test]
fn corroboration_can_lift_a_tool_source_into_trusted_singleton_path() {
    let promoter = MemoryPromoter::new(PolicySet::default());
    let mut candidate = candidate(
        "user",
        "location",
        "Berlin",
        "The deployment tool reports the user location as Berlin.",
    );
    candidate.memory_type = MemoryType::Fact;
    candidate.source.source_type = SourceType::Tool;
    candidate.source.trust_weight = 0.6;

    let result = promoter.promote_with_context(
        &candidate,
        &PromotionContext {
            corroborating_evidence_count: 3,
            contradictory_evidence_count: 0,
            ..PromotionContext::default()
        },
        &FixedClock::new(ts(1_700_000_000)),
    );

    assert_eq!(result.decision, PromotionDecision::Promote);
    assert_eq!(
        result.details.get("route_basis").map(String::as_str),
        Some("corroborated_trusted_source")
    );
}

#[test]
fn contradictions_reduce_effective_trust_and_fail_closed() {
    let promoter = MemoryPromoter::new(PolicySet::default());
    let mut candidate = candidate(
        "user",
        "location",
        "Berlin",
        "The deployment tool reports the user location as Berlin.",
    );
    candidate.memory_type = MemoryType::Fact;
    candidate.source.source_type = SourceType::Tool;
    candidate.source.trust_weight = 0.6;

    let result = promoter.promote_with_context(
        &candidate,
        &PromotionContext {
            corroborating_evidence_count: 3,
            contradictory_evidence_count: 3,
            ..PromotionContext::default()
        },
        &FixedClock::new(ts(1_700_000_000)),
    );

    assert_ne!(result.decision, PromotionDecision::Promote);
    assert_eq!(
        result.details.get("route_basis").map(String::as_str),
        Some("insufficient_evidence")
    );
}
