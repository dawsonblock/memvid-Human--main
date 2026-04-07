use serde::{Deserialize, Serialize};

/// Internal bounded-memory layer used for routing and storage policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    Trace,
    Episode,
    Belief,
    GoalState,
    SelfModel,
    Procedure,
}

/// High-level governed memory types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Trace,
    Episode,
    Fact,
    Preference,
    GoalState,
}

/// Current status of an explicit belief.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefStatus {
    Active,
    Disputed,
    Stale,
    Retracted,
}

/// Retrieval-facing interpretation status derived from belief state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefViewStatus {
    Active,
    Contested,
    Superseded,
    Retracted,
}

/// Intent classes for governed retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryIntent {
    CurrentFact,
    HistoricalFact,
    PreferenceLookup,
    TaskState,
    EpisodicRecall,
    SemanticBackground,
}

/// Trust source of a memory observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Chat,
    File,
    Tool,
    System,
    External,
}

/// Visibility / applicability scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Private,
    Task,
    Project,
    Shared,
}

/// Result of the promotion gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDecision {
    Reject,
    StoreTrace,
    Promote,
}

/// Belief state transition kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefAction {
    Reinforce,
    Update,
    Dispute,
    Retract,
}

/// Lifecycle state for active goal-state memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Inactive,
    Blocked,
    WaitingOnUser,
    WaitingOnSystem,
    Stale,
    Completed,
}

/// Narrow categories for durable self-model records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfModelKind {
    Preference,
    ResponseStyle,
    RiskTolerance,
    ToolPreference,
    ProjectNorm,
    Constraint,
    Value,
    WorkPattern,
    CapabilityLimit,
}

/// Stability class for durable self-model records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfModelStabilityClass {
    StableDirective,
    FlexiblePreference,
}

/// Required update path for changing a durable self-model record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfModelUpdateRequirement {
    TrustedOrCorroborated,
    ReinforcementAllowed,
}

/// Lifecycle state for learned procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Active,
    CoolingDown,
    Retired,
}

/// External task outcome feedback recorded against a memory or workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeFeedbackKind {
    Positive,
    Negative,
}

impl MemoryLayer {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Episode => "episode",
            Self::Belief => "belief",
            Self::GoalState => "goal_state",
            Self::SelfModel => "self_model",
            Self::Procedure => "procedure",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "trace" => Some(Self::Trace),
            "episode" => Some(Self::Episode),
            "belief" => Some(Self::Belief),
            "goal_state" | "goalstate" => Some(Self::GoalState),
            "self_model" | "selfmodel" => Some(Self::SelfModel),
            "procedure" => Some(Self::Procedure),
            _ => None,
        }
    }
}

impl MemoryType {
    #[must_use]
    pub const fn memory_layer(self) -> MemoryLayer {
        match self {
            Self::Trace => MemoryLayer::Trace,
            Self::Episode => MemoryLayer::Episode,
            Self::Fact => MemoryLayer::Belief,
            Self::Preference => MemoryLayer::SelfModel,
            Self::GoalState => MemoryLayer::GoalState,
        }
    }
}

impl GoalStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Blocked => "blocked",
            Self::WaitingOnUser => "waiting_on_user",
            Self::WaitingOnSystem => "waiting_on_system",
            Self::Stale => "stale",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub fn from_text(value: &str, raw_text: &str) -> Self {
        let lower = format!("{value} {raw_text}").to_lowercase();
        if lower.contains("waiting on user") || lower.contains("awaiting user") {
            Self::WaitingOnUser
        } else if lower.contains("waiting on system")
            || lower.contains("awaiting system")
            || lower.contains("pending system")
        {
            Self::WaitingOnSystem
        } else if lower.contains("blocked") {
            Self::Blocked
        } else if lower.contains("stale") || lower.contains("outdated") {
            Self::Stale
        } else if lower.contains("complete") || lower.contains("done") {
            Self::Completed
        } else if lower.contains("inactive") || lower.contains("paused") {
            Self::Inactive
        } else {
            Self::Active
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            "blocked" => Some(Self::Blocked),
            "waiting_on_user" => Some(Self::WaitingOnUser),
            "waiting_on_system" => Some(Self::WaitingOnSystem),
            "stale" => Some(Self::Stale),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

impl BeliefStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disputed => "disputed",
            Self::Stale => "stale",
            Self::Retracted => "retracted",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disputed" => Some(Self::Disputed),
            "stale" => Some(Self::Stale),
            "retracted" => Some(Self::Retracted),
            _ => None,
        }
    }

    #[must_use]
    pub const fn view_status(self) -> BeliefViewStatus {
        match self {
            Self::Active => BeliefViewStatus::Active,
            Self::Disputed => BeliefViewStatus::Contested,
            Self::Stale => BeliefViewStatus::Superseded,
            Self::Retracted => BeliefViewStatus::Retracted,
        }
    }
}

impl OutcomeFeedbackKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "positive" | "success" => Some(Self::Positive),
            "negative" | "failure" | "failed" => Some(Self::Negative),
            _ => None,
        }
    }
}

impl BeliefViewStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Contested => "contested",
            Self::Superseded => "superseded",
            Self::Retracted => "retracted",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "contested" => Some(Self::Contested),
            "superseded" => Some(Self::Superseded),
            "retracted" => Some(Self::Retracted),
            _ => None,
        }
    }
}

impl SelfModelKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::ResponseStyle => "response_style",
            Self::RiskTolerance => "risk_tolerance",
            Self::ToolPreference => "tool_preference",
            Self::ProjectNorm => "project_norm",
            Self::Constraint => "constraint",
            Self::Value => "value",
            Self::WorkPattern => "work_pattern",
            Self::CapabilityLimit => "capability_limit",
        }
    }

    #[must_use]
    pub fn from_slot_strict(slot: &str) -> Option<Self> {
        let lower = slot.trim().to_lowercase();
        if lower.is_empty() {
            return None;
        }

        if lower.contains("style") || lower.contains("tone") || lower.contains("verbosity") {
            Some(Self::ResponseStyle)
        } else if lower.contains("risk") {
            Some(Self::RiskTolerance)
        } else if lower.contains("tool") || lower.contains("editor") {
            Some(Self::ToolPreference)
        } else if lower.contains("norm") || lower.contains("convention") {
            Some(Self::ProjectNorm)
        } else if lower.contains("constraint") || lower.contains("limit") {
            Some(Self::Constraint)
        } else if lower.contains("value") || lower.contains("priority") {
            Some(Self::Value)
        } else if lower.contains("pattern") || lower.contains("workflow") {
            Some(Self::WorkPattern)
        } else if lower.contains("capability") || lower.contains("strength") {
            Some(Self::CapabilityLimit)
        } else if lower.contains("prefer")
            || lower.contains("preference")
            || lower.contains("favorite")
            || lower.contains("like")
            || lower.contains("dislike")
        {
            Some(Self::Preference)
        } else {
            None
        }
    }

    #[must_use]
    pub fn from_slot(slot: &str) -> Self {
        Self::from_slot_strict(slot).unwrap_or(Self::Preference)
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "preference" => Some(Self::Preference),
            "response_style" => Some(Self::ResponseStyle),
            "risk_tolerance" => Some(Self::RiskTolerance),
            "tool_preference" => Some(Self::ToolPreference),
            "project_norm" => Some(Self::ProjectNorm),
            "constraint" => Some(Self::Constraint),
            "value" => Some(Self::Value),
            "work_pattern" => Some(Self::WorkPattern),
            "capability_limit" => Some(Self::CapabilityLimit),
            _ => None,
        }
    }

    #[must_use]
    pub const fn stability_class(self) -> SelfModelStabilityClass {
        match self {
            Self::Constraint | Self::Value | Self::CapabilityLimit => {
                SelfModelStabilityClass::StableDirective
            }
            Self::Preference
            | Self::ResponseStyle
            | Self::RiskTolerance
            | Self::ToolPreference
            | Self::ProjectNorm
            | Self::WorkPattern => SelfModelStabilityClass::FlexiblePreference,
        }
    }

    #[must_use]
    pub const fn update_requirement(self) -> SelfModelUpdateRequirement {
        match self.stability_class() {
            SelfModelStabilityClass::StableDirective => {
                SelfModelUpdateRequirement::TrustedOrCorroborated
            }
            SelfModelStabilityClass::FlexiblePreference => {
                SelfModelUpdateRequirement::ReinforcementAllowed
            }
        }
    }
}

impl SelfModelStabilityClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StableDirective => "stable_directive",
            Self::FlexiblePreference => "flexible_preference",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "stable_directive" => Some(Self::StableDirective),
            "flexible_preference" => Some(Self::FlexiblePreference),
            _ => None,
        }
    }
}

impl SelfModelUpdateRequirement {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrustedOrCorroborated => "trusted_or_corroborated",
            Self::ReinforcementAllowed => "reinforcement_allowed",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "trusted_or_corroborated" => Some(Self::TrustedOrCorroborated),
            "reinforcement_allowed" => Some(Self::ReinforcementAllowed),
            _ => None,
        }
    }
}

impl ProcedureStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::CoolingDown => "cooling_down",
            Self::Retired => "retired",
        }
    }

    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "cooling_down" => Some(Self::CoolingDown),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }
}
