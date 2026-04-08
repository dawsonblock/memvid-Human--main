use serde::{Deserialize, Serialize};

/// Machine-readable rejection codes for policy-governed ingest decisions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ReasonCode {
    ScoreBelowRejectionThreshold,
    TraceLayerArchivalOnly,
    ScoreBelowTraceThreshold,
    StructuredIdentityRequired,
    GoalStateSemanticsRequired,
    EvidenceThresholdNotMet,
    ProtectedSelfModelRejected,
    StableDirectiveUpdateRejected,
    ProcedureEvidenceRestricted,
    PromotionThresholdNotMet,
}

impl ReasonCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScoreBelowRejectionThreshold => "score_below_rejection_threshold",
            Self::TraceLayerArchivalOnly => "trace_layer_archival_only",
            Self::ScoreBelowTraceThreshold => "score_below_trace_threshold",
            Self::StructuredIdentityRequired => "structured_identity_required",
            Self::GoalStateSemanticsRequired => "goal_state_semantics_required",
            Self::EvidenceThresholdNotMet => "evidence_threshold_not_met",
            Self::ProtectedSelfModelRejected => "protected_self_model_rejected",
            Self::StableDirectiveUpdateRejected => "stable_directive_update_rejected",
            Self::ProcedureEvidenceRestricted => "procedure_evidence_restricted",
            Self::PromotionThresholdNotMet => "promotion_threshold_not_met",
        }
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
