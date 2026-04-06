use super::enums::SourceType;

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
