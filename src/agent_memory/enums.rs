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

/// Lifecycle state for learned procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Active,
    CoolingDown,
    Retired,
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
            "completed" => Some(Self::Completed),
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
    pub fn from_slot(slot: &str) -> Self {
        let lower = slot.to_lowercase();
        if lower.contains("style") || lower.contains("tone") || lower.contains("verbosity") {
            Self::ResponseStyle
        } else if lower.contains("risk") {
            Self::RiskTolerance
        } else if lower.contains("tool") || lower.contains("editor") {
            Self::ToolPreference
        } else if lower.contains("norm") || lower.contains("convention") {
            Self::ProjectNorm
        } else if lower.contains("constraint") || lower.contains("limit") {
            Self::Constraint
        } else if lower.contains("value") || lower.contains("priority") {
            Self::Value
        } else if lower.contains("pattern") || lower.contains("workflow") {
            Self::WorkPattern
        } else if lower.contains("capability") || lower.contains("strength") {
            Self::CapabilityLimit
        } else {
            Self::Preference
        }
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
