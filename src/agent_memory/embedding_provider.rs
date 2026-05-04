//! Dense-embedding provider trait for the agent-memory retrieval path.
//!
//! The trait itself (`AgentEmbeddingProvider`) is always compiled so that
//! `MemoryRetriever` can hold an `Option<Arc<dyn AgentEmbeddingProvider>>`
//! without any feature-gate in its type definition.
//!
//! The only *bundled* implementation (`LocalEmbeddingAdapter`) wraps
//! [`crate::text_embed::LocalTextEmbedder`] and is compiled only when the
//! `vec` feature is active (ONNX Runtime + ndarray are required).
//!
//! # Typical usage (`vec` feature)
//!
//! ```ignore
//! use memvid_core::text_embed::{LocalTextEmbedder, TextEmbedConfig};
//! use memvid_core::agent_memory::embedding_provider::LocalEmbeddingAdapter;
//! use memvid_core::agent_memory::MemoryController;
//!
//! let embedder = LocalTextEmbedder::new(TextEmbedConfig::default())?;
//! let adapter  = LocalEmbeddingAdapter::new(embedder);
//! let controller = MemoryController::new(...)
//!     .with_embedder(adapter);
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::errors::Result;

// ── Core trait ───────────────────────────────────────────────────────────────

/// Object-safe trait for computing dense text embeddings.
///
/// Implementors are expected to produce L2-normalised vectors of a fixed
/// dimension (returned by [`AgentEmbeddingProvider::dim`]).  All impls must
/// be `Send + Sync + fmt::Debug` so they can be stored behind an `Arc<dyn …>`
/// and printed during debug output.
pub trait AgentEmbeddingProvider: Send + Sync + fmt::Debug {
    /// Embed `text` into a dense float vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Fixed output dimension for all vectors produced by this provider.
    fn dim(&self) -> usize;
}

// ── Cosine similarity helper ─────────────────────────────────────────────────

/// Cosine similarity between two equal-length vectors.
///
/// Returns `0.0` if either vector has zero norm (rather than NaN).
/// Output is clamped to `[-1.0, 1.0]`.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        (dot / (na * nb)).clamp(-1.0, 1.0)
    }
}

// ── In-session embedding cache ───────────────────────────────────────────────

/// Bounded in-memory cache for text embeddings within a single retrieval
/// session.
///
/// Eviction is approximate (the oldest-inserted entry is removed when the
/// cache is full); this is intentional — LRU precision is not worth the
/// overhead in the governed-memory hot path.
pub struct EmbeddingCache {
    cache: HashMap<String, Vec<f32>>,
    capacity: usize,
}

impl EmbeddingCache {
    /// Create a new cache with the given maximum number of entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: HashMap::new(),
            capacity,
        }
    }

    /// Return a cached embedding for `text`, computing and caching it on miss.
    pub fn get_or_embed(
        &mut self,
        text: &str,
        provider: &dyn AgentEmbeddingProvider,
    ) -> Result<Vec<f32>> {
        if let Some(v) = self.cache.get(text) {
            return Ok(v.clone());
        }
        let embedding = provider.embed(text)?;
        if self.cache.len() >= self.capacity {
            // Evict one entry to stay within capacity.
            if let Some(key) = self.cache.keys().next().cloned() {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(text.to_string(), embedding.clone());
        Ok(embedding)
    }
}

// ── LocalEmbeddingAdapter (vec feature only) ─────────────────────────────────

/// Adapter that wraps [`crate::text_embed::LocalTextEmbedder`] (ONNX-backed)
/// and exposes it as an [`AgentEmbeddingProvider`].
///
/// Requires the `vec` feature flag.
#[cfg(feature = "vec")]
pub struct LocalEmbeddingAdapter {
    inner: crate::text_embed::LocalTextEmbedder,
    dim: usize,
}

#[cfg(feature = "vec")]
impl LocalEmbeddingAdapter {
    /// Wrap an existing [`crate::text_embed::LocalTextEmbedder`].
    #[must_use]
    pub fn new(inner: crate::text_embed::LocalTextEmbedder) -> Self {
        use crate::types::embedding::EmbeddingProvider as _;
        let dim = inner.dimension();
        Self { inner, dim }
    }
}

#[cfg(feature = "vec")]
impl fmt::Debug for LocalEmbeddingAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LocalEmbeddingAdapter(dim={})", self.dim)
    }
}

#[cfg(feature = "vec")]
impl AgentEmbeddingProvider for LocalEmbeddingAdapter {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        use crate::types::embedding::EmbeddingProvider as _;
        self.inner
            .embed_text(text)
            .map_err(super::errors::AgentMemoryError::Memvid)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

// ── EmbedderSlot — Arc wrapper with derive-compatible Debug+Clone ─────────────

/// Arc-wrapped optional provider stored inside [`super::memory_retriever::MemoryRetriever`].
///
/// The wrapper provides manual `Debug` and `Clone` implementations so that
/// `MemoryRetriever` can continue to derive both traits even with the embedded
/// provider field.
#[derive(Clone)]
pub struct EmbedderSlot(pub Option<Arc<dyn AgentEmbeddingProvider>>);

impl fmt::Debug for EmbedderSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => write!(f, "EmbedderSlot(None)"),
            Some(p) => write!(f, "EmbedderSlot(Some({p:?}))"),
        }
    }
}

impl Default for EmbedderSlot {
    fn default() -> Self {
        Self(None)
    }
}

impl EmbedderSlot {
    /// Returns `true` when an embedding provider is installed.
    #[must_use]
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }

    /// Borrow the inner provider if present.
    #[must_use]
    pub fn as_deref(&self) -> Option<&dyn AgentEmbeddingProvider> {
        self.0.as_deref()
    }
}
