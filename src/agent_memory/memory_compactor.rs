/// Placeholder for future archive compaction.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCompactor;

impl MemoryCompactor {
    /// Memvid already owns low-level segment compaction; governed memory only exposes a stub.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        false
    }

    /// Governed memory does not currently run a separate logical compaction pass.
    #[must_use]
    pub const fn status(&self) -> &'static str {
        "unsupported"
    }

    /// Honest user-facing explanation for the governed-memory maintenance surface.
    #[must_use]
    pub const fn unsupported_reason(&self) -> &'static str {
        "governed memory does not expose a separate logical compaction pass"
    }
}
