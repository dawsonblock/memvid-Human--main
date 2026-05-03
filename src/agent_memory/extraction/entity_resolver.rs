/// Resolves entity references and normalises entity names for the extraction pipeline.
pub struct EntityResolver {
    entity_hint: Option<String>,
}

impl EntityResolver {
    #[must_use]
    pub fn new(hint: Option<String>) -> Self {
        Self { entity_hint: hint }
    }

    /// Resolve a subject token to a canonical entity name.
    ///
    /// First-person pronouns (`I`, `me`, `my`, `mine`) are mapped to the entity
    /// hint when one is available.  Returns `None` when the subject is blank or
    /// unresolvable.
    #[must_use]
    pub fn resolve_subject(&self, subject: &str) -> Option<String> {
        let lower = subject.trim().to_lowercase();
        if matches!(lower.as_str(), "i" | "me" | "my" | "mine") {
            return self.entity_hint.clone();
        }
        let trimmed = subject.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Normalise an entity name: trim surrounding whitespace and collapse
    /// internal whitespace runs to a single space.
    #[must_use]
    pub fn normalize(&self, name: &str) -> String {
        name.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
