//! Semantic conflict resolution for agent memory beliefs.
//!
//! Classifies the relationship between two candidate values for the same
//! (entity, slot). All detection is rule-based; no ML models are used.

use super::enums::SourceType;

/// Classification of how an incoming value relates to an existing belief value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeliefConflictResolution {
    /// Values are semantically identical (normalised equality).
    Same,
    /// Incoming strongly overlaps with existing — reinforces without dispute.
    Reinforces,
    /// Incoming is contextually scoped ("for this project", "when X") rather
    /// than a universal override — treat as a compatible variant.
    CompatibleContextualVariant,
    /// Incoming explicitly supersedes the existing value ("from now on", "actually").
    Supersedes,
    /// Incoming is an explicit contradiction ("not X", "that is wrong").
    Contradicts,
    /// Incoming is a strict token-subset of existing — narrows the scope.
    NarrowsScope,
    /// Incoming is a strict token-superset of existing — broadens the scope.
    BroadensScope,
    /// Incoming is qualified as temporary ("for now", "just this once").
    TemporaryOverride,
    /// Unable to classify — caller should fall back to trust-weighted logic.
    Ambiguous,
}

/// Additional signals for conflict resolution beyond the raw string values.
pub struct ConflictContext {
    pub existing: String,
    pub incoming: String,
    pub source_type: SourceType,
}

/// Stateless resolver — call [`BeliefConflictResolver::resolve`] directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct BeliefConflictResolver;

impl BeliefConflictResolver {
    /// Classify the relationship between `existing` and `incoming` values.
    pub fn resolve(
        existing: &str,
        incoming: &str,
        _context: &ConflictContext,
    ) -> BeliefConflictResolution {
        let norm_existing = normalize(existing);
        let norm_incoming = normalize(incoming);

        // 1. Identical after normalisation.
        if norm_existing == norm_incoming {
            return BeliefConflictResolution::Same;
        }

        let inc_lower = incoming.to_lowercase();

        // 2. Explicit supersedes markers.
        if is_supersedes_statement(&inc_lower) {
            return BeliefConflictResolution::Supersedes;
        }

        // 3. Temporary-override markers.
        if has_temporary_marker(&inc_lower) {
            return BeliefConflictResolution::TemporaryOverride;
        }

        // 4. Explicit contradiction.
        if is_explicit_contradiction(&norm_existing, &inc_lower) {
            return BeliefConflictResolution::Contradicts;
        }

        // 5. High token overlap → reinforce.
        if jaccard_token_overlap(&norm_existing, &norm_incoming) >= 0.8 {
            return BeliefConflictResolution::Reinforces;
        }

        // 6. Contextual scope qualifier → compatible variant.
        if has_scope_qualifier(&inc_lower) {
            return BeliefConflictResolution::CompatibleContextualVariant;
        }

        // 7. Token subset / superset → scope narrowing or broadening.
        let ex_set = token_set(&norm_existing);
        let inc_set = token_set(&norm_incoming);
        let ex_len = ex_set.len();
        let inc_len = inc_set.len();
        if inc_len > 0 && ex_len > 0 {
            let shared = ex_set.intersection(&inc_set).count();
            if shared == inc_len && inc_len < ex_len {
                return BeliefConflictResolution::NarrowsScope;
            }
            if shared == ex_len && ex_len < inc_len {
                return BeliefConflictResolution::BroadensScope;
            }
        }

        BeliefConflictResolution::Ambiguous
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_set(s: &str) -> std::collections::BTreeSet<&str> {
    s.split_whitespace().collect()
}

fn jaccard_token_overlap(a: &str, b: &str) -> f32 {
    let a_set: std::collections::BTreeSet<&str> = a.split_whitespace().collect();
    let b_set: std::collections::BTreeSet<&str> = b.split_whitespace().collect();
    let union_len = a_set.union(&b_set).count();
    if union_len == 0 {
        return 1.0;
    }
    a_set.intersection(&b_set).count() as f32 / union_len as f32
}

fn is_supersedes_statement(lower: &str) -> bool {
    lower.starts_with("now ")
        || lower.starts_with("instead")
        || lower.starts_with("actually ")
        || lower.starts_with("from now on")
        || lower.starts_with("going forward")
        || lower.starts_with("changed my mind")
        || lower.starts_with("i changed my mind")
}

fn has_temporary_marker(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "for now",
        "just this once",
        "just for now",
        "temporarily",
        "this time only",
        "just today",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn is_explicit_contradiction(norm_existing: &str, inc_lower: &str) -> bool {
    if inc_lower.contains("that is wrong")
        || inc_lower.contains("that's wrong")
        || inc_lower.contains("that was wrong")
        || inc_lower.contains("is incorrect")
        || inc_lower.contains("was incorrect")
    {
        return true;
    }
    // "not <key-token>" where the key-token appears in the existing value.
    for token in norm_existing.split_whitespace() {
        if token.len() > 3 {
            let negated = format!("not {token}");
            if inc_lower.contains(&negated) {
                return true;
            }
        }
    }
    false
}

fn has_scope_qualifier(lower: &str) -> bool {
    const QUALIFIERS: &[&str] = &[
        "for this ",
        "in this ",
        "when working",
        "unless ",
        "except when",
        "only when",
        "only for",
        "in that case",
    ];
    QUALIFIERS.iter().any(|q| lower.contains(q))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(existing: &str, incoming: &str) -> ConflictContext {
        ConflictContext {
            existing: existing.to_string(),
            incoming: incoming.to_string(),
            source_type: SourceType::Chat,
        }
    }

    #[test]
    fn same_after_normalisation() {
        assert_eq!(
            BeliefConflictResolver::resolve("Rust", "rust", &ctx("Rust", "rust")),
            BeliefConflictResolution::Same
        );
    }

    #[test]
    fn same_exact_equality() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "dark mode",
                "dark mode",
                &ctx("dark mode", "dark mode")
            ),
            BeliefConflictResolution::Same
        );
    }

    #[test]
    fn supersedes_from_now_on() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "use tabs",
                "from now on use spaces",
                &ctx("use tabs", "from now on use spaces"),
            ),
            BeliefConflictResolution::Supersedes
        );
    }

    #[test]
    fn supersedes_actually() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "prefers Python",
                "actually prefers Rust",
                &ctx("prefers Python", "actually prefers Rust"),
            ),
            BeliefConflictResolution::Supersedes
        );
    }

    #[test]
    fn temporary_for_now() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "use tabs",
                "for now use spaces",
                &ctx("use tabs", "for now use spaces"),
            ),
            BeliefConflictResolution::TemporaryOverride
        );
    }

    #[test]
    fn temporary_just_this_once() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "serif fonts",
                "just this once use sans-serif",
                &ctx("serif fonts", "just this once use sans-serif"),
            ),
            BeliefConflictResolution::TemporaryOverride
        );
    }

    #[test]
    fn contradicts_that_is_wrong() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "prefer dark mode",
                "that is wrong, use light mode",
                &ctx("prefer dark mode", "that is wrong, use light mode"),
            ),
            BeliefConflictResolution::Contradicts
        );
    }

    #[test]
    fn contradicts_negation_token() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "prefer tabs",
                "not tabs, use spaces",
                &ctx("prefer tabs", "not tabs, use spaces"),
            ),
            BeliefConflictResolution::Contradicts
        );
    }

    #[test]
    fn contextual_variant_in_this() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "use tabs",
                "in this project use spaces",
                &ctx("use tabs", "in this project use spaces"),
            ),
            BeliefConflictResolution::CompatibleContextualVariant
        );
    }

    #[test]
    fn narrows_scope() {
        // "dark" is a subset of "prefer dark mode" tokens
        assert_eq!(
            BeliefConflictResolver::resolve(
                "prefer dark mode",
                "dark mode",
                &ctx("prefer dark mode", "dark mode"),
            ),
            BeliefConflictResolution::NarrowsScope
        );
    }

    #[test]
    fn ambiguous_unrelated() {
        assert_eq!(
            BeliefConflictResolver::resolve(
                "use Python",
                "use Java",
                &ctx("use Python", "use Java"),
            ),
            BeliefConflictResolution::Ambiguous
        );
    }
}
