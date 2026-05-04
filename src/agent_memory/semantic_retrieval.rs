//! Lightweight semantic retrieval via query expansion + TF-IDF cosine similarity.
//!
//! Pure Rust — no ONNX, no embeddings, no external model dependencies.
//! Compiles under any feature-flag combination (including the default set).
//!
//! # Flow
//! 1. Expand the raw query text into 1-N synonym variants.
//! 2. Issue a `store.search()` for each variant, de-duplicate by `memory_id`.
//! 3. Score each candidate hit with TF-IDF cosine similarity against the
//!    original query terms.
//! 4. Filter below `SEMANTIC_THRESHOLD`, tag the passing hits with
//!    [`SCORE_SIGNAL_SEMANTIC_SCORE_KEY`] in metadata, and return the
//!    top-k by descending similarity.

use std::collections::{HashMap, HashSet};

use super::adapters::memvid_store::MemoryStore;
use super::errors::Result;
use super::query_intent::expand_query;
use super::ranker::SCORE_SIGNAL_SEMANTIC_SCORE_KEY;
use super::schemas::{RetrievalHit, RetrievalQuery};

/// Minimum cosine similarity (inclusive) to include a candidate hit.
/// Hits below this threshold are discarded before returning to the retriever.
const SEMANTIC_THRESHOLD: f32 = 0.15;

/// Zero-size coordinator for the semantic retrieval path.
pub struct SemanticRetriever;

impl SemanticRetriever {
    /// Returns candidate hits scored by term-frequency cosine similarity.
    ///
    /// Calls `store.search()` for every expansion produced by [`expand_query`],
    /// deduplicates across expansions, and annotates each surviving hit with
    /// [`SCORE_SIGNAL_SEMANTIC_SCORE_KEY`] before handing results back to
    /// [`super::memory_retriever::MemoryRetriever::merge_semantic_hits`].
    pub fn semantic_hits<S: MemoryStore>(
        query_text: &str,
        store: &mut S,
        top_k: usize,
        base_query: &RetrievalQuery,
    ) -> Result<Vec<RetrievalHit>> {
        let expansions = expand_query(query_text);
        // Build vocab from ALL expansion variants so synonym-expanded hits still
        // score above the threshold even when the original term is absent.
        let all_expansion_terms: Vec<String> =
            expansions.iter().flat_map(|e| Self::tokenize(e)).collect();

        // Issue one `store.search()` per expansion and de-duplicate results.
        let mut seen_keys: HashSet<String> = HashSet::new();
        let mut candidates: Vec<RetrievalHit> = Vec::new();

        for expansion in &expansions {
            let mut expanded_query = base_query.clone();
            expanded_query.query_text = expansion.clone();
            expanded_query.top_k = top_k;

            let hits = store.search(&expanded_query)?;
            for hit in hits {
                let key = hit
                    .memory_id
                    .clone()
                    .unwrap_or_else(|| format!("{}|{}", hit.text.len(), hit.timestamp.timestamp()));
                if seen_keys.insert(key) {
                    candidates.push(hit);
                }
            }
        }

        // Score candidates and filter below threshold.
        let mut scored: Vec<(f32, RetrievalHit)> = candidates
            .into_iter()
            .filter_map(|mut hit| {
                let score = Self::tf_cosine(&all_expansion_terms, &hit.text);
                if score < SEMANTIC_THRESHOLD {
                    return None;
                }
                hit.metadata.insert(
                    SCORE_SIGNAL_SEMANTIC_SCORE_KEY.to_string(),
                    format!("{score:.6}"),
                );
                Some((score, hit))
            })
            .collect();

        scored.sort_by(|(a, _), (b, _)| b.total_cmp(a));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, hit)| hit).collect())
    }

    // ── internal helpers ─────────────────────────────────────────────────────

    /// Term-frequency cosine similarity between pre-tokenized query terms and
    /// raw document text.  Returns a value in `[0, 1]`.
    fn tf_cosine(query_terms: &[String], doc_text: &str) -> f32 {
        if query_terms.is_empty() {
            return 0.0;
        }
        let doc_terms = Self::tokenize(doc_text);
        if doc_terms.is_empty() {
            return 0.0;
        }

        let query_tf = Self::term_freq(query_terms);
        let doc_tf = Self::term_freq(&doc_terms);

        // Dot product over the shared vocabulary.
        let dot: f32 = query_tf
            .iter()
            .map(|(term, &q_count)| {
                let d_count = doc_tf.get(term.as_str()).copied().unwrap_or(0) as f32;
                q_count as f32 * d_count
            })
            .sum();

        let q_norm: f32 = query_tf
            .values()
            .map(|&c| (c as f32).powi(2))
            .sum::<f32>()
            .sqrt();
        let d_norm: f32 = doc_tf
            .values()
            .map(|&c| (c as f32).powi(2))
            .sum::<f32>()
            .sqrt();

        if q_norm == 0.0 || d_norm == 0.0 {
            return 0.0;
        }
        (dot / (q_norm * d_norm)).clamp(0.0, 1.0)
    }

    fn term_freq(terms: &[String]) -> HashMap<String, u32> {
        let mut tf: HashMap<String, u32> = HashMap::new();
        for term in terms {
            *tf.entry(term.clone()).or_insert(0) += 1;
        }
        tf
    }

    /// Splits text into lowercase alphabetic tokens of length >= 3 after
    /// removing common English stop words.
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphabetic())
            .filter(|tok| tok.len() >= 3)
            .map(|tok| tok.to_lowercase())
            .filter(|tok| !STOP_WORDS.contains(&tok.as_str()))
            .collect()
    }

    /// Re-orders `hits` by embedding cosine similarity to `query_text`.
    ///
    /// Falls back to the original ordering when no embedder is supplied or
    /// when embedding the query fails.  Per-hit embed failures silently
    /// score that hit at `0.0` (it sinks to the bottom of the ranking).
    pub fn contextual_rerank(
        hits: Vec<RetrievalHit>,
        query_text: &str,
        embedder: Option<&dyn super::embedding_provider::AgentEmbeddingProvider>,
    ) -> Vec<RetrievalHit> {
        let provider = match embedder {
            Some(p) => p,
            None => return hits,
        };
        use super::embedding_provider::cosine;
        let query_emb = match provider.embed(query_text) {
            Ok(e) => e,
            Err(_) => return hits,
        };
        let mut scored: Vec<(f32, RetrievalHit)> = hits
            .into_iter()
            .map(|hit| {
                let score = provider
                    .embed(&hit.text)
                    .map(|doc_emb| cosine(&query_emb, &doc_emb))
                    .unwrap_or(0.0);
                (score, hit)
            })
            .collect();
        scored.sort_by(|(a, _), (b, _)| b.total_cmp(a));
        scored.into_iter().map(|(_, hit)| hit).collect()
    }
}

// ── Embedding-based semantic retrieval (vec feature only) ────────────────────

#[cfg(feature = "vec")]
impl SemanticRetriever {
    /// Returns candidate hits scored by dense embedding cosine similarity.
    ///
    /// Fetches a wider candidate pool via the existing [`MemoryStore::search`]
    /// lexical path, reranks candidates by embedding cosine similarity against
    /// the query, filters below [`EMBEDDING_THRESHOLD`], and annotates each
    /// surviving hit with [`super::ranker::SCORE_SIGNAL_EMBEDDING_KEY`].
    pub fn semantic_hits_with_embedder<S: MemoryStore>(
        query_text: &str,
        store: &mut S,
        top_k: usize,
        base_query: &RetrievalQuery,
        provider: &dyn super::embedding_provider::AgentEmbeddingProvider,
        cache: &mut super::embedding_provider::EmbeddingCache,
    ) -> Result<Vec<RetrievalHit>> {
        use super::embedding_provider::cosine;
        use super::ranker::SCORE_SIGNAL_EMBEDDING_KEY;

        // Minimum cosine similarity to surface an embedding hit.
        const EMBEDDING_THRESHOLD: f32 = 0.50;

        // Embed the query (or retrieve from cache on repeated calls).
        let query_embedding = cache.get_or_embed(query_text, provider)?;

        // Fetch a wider pool of candidates via the existing lexical search.
        let mut search_q = base_query.clone();
        search_q.query_text = query_text.to_string();
        search_q.top_k = (top_k * 3).max(12);
        let candidates = store.search(&search_q)?;

        let mut scored: Vec<(f32, RetrievalHit)> = Vec::new();
        for mut hit in candidates {
            let doc_embedding = match cache.get_or_embed(&hit.text, provider) {
                Ok(e) => e,
                // Skip hits whose text we cannot embed (e.g. model not loaded).
                Err(_) => continue,
            };
            let sim = cosine(&query_embedding, &doc_embedding);
            if sim < EMBEDDING_THRESHOLD {
                continue;
            }
            hit.metadata
                .insert(SCORE_SIGNAL_EMBEDDING_KEY.to_string(), format!("{sim:.6}"));
            scored.push((sim, hit));
        }

        scored.sort_by(|(a, _), (b, _)| b.total_cmp(a));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, hit)| hit).collect())
    }
}

/// Common English stop words filtered during tokenisation.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was",
    "one", "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old",
    "see", "two", "way", "who", "its", "did", "let", "put", "too", "use", "com", "www", "that",
    "this", "with", "they", "from", "have", "will", "into", "then", "more", "also", "been", "were",
    "than", "what", "when", "your", "said", "each", "time", "like", "just", "some", "over", "such",
    "even", "most", "very", "about", "there", "their", "other", "would", "which", "these", "those",
    "could",
];
