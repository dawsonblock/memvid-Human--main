/// Placeholder for future archive compaction.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCompactor;

impl MemoryCompactor {
    /// Memvid already owns low-level segment compaction; governed memory only exposes a stub.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        false
    }
}
