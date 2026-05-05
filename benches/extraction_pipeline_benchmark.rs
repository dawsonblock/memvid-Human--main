//! Criterion benchmarks for the extraction pipeline subsystem.
//!
//! Run with:
//!   cargo bench --bench extraction_pipeline_benchmark
//!
//! Individual groups:
//!   cargo bench --bench extraction_pipeline_benchmark -- preference_extractor
//!   cargo bench --bench extraction_pipeline_benchmark -- claim_extractor
//!   cargo bench --bench extraction_pipeline_benchmark -- pipeline_short
//!   cargo bench --bench extraction_pipeline_benchmark -- pipeline_medium
//!   cargo bench --bench extraction_pipeline_benchmark -- deduplication

use std::collections::BTreeMap;

use criterion::{Criterion, criterion_group, criterion_main};
use memvid_core::agent_memory::enums::{MemoryType, Scope, SourceType};
use memvid_core::agent_memory::extraction::{
    RawInputProcessor, claim_extractor::ClaimExtractor, entity_resolver::EntityResolver,
    preference_extractor::PreferenceExtractor, provider::MergedExtractionValidator,
};
use memvid_core::agent_memory::schemas::{CandidateMemory, IngestContext, Provenance};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_candidate(entity: &str, slot: &str, value: &str) -> CandidateMemory {
    CandidateMemory {
        candidate_id: format!("{entity}-{slot}-{value}"),
        observed_at: chrono::Utc::now(),
        entity: if entity.is_empty() {
            None
        } else {
            Some(entity.to_string())
        },
        slot: if slot.is_empty() {
            None
        } else {
            Some(slot.to_string())
        },
        value: if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        },
        raw_text: String::new(),
        source: Provenance {
            source_type: SourceType::Chat,
            source_id: "bench".to_string(),
            source_label: None,
            observed_by: None,
            trust_weight: 0.8,
        },
        memory_type: MemoryType::Fact,
        confidence: 0.5,
        salience: 0.4,
        scope: Scope::Private,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        is_retraction: false,
        thread_id: None,
        parent_memory_id: None,
    }
}

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_preference_extractor(c: &mut Criterion) {
    let input =
        "I prefer dark mode in my editor. I like Rust for systems work. I hate boilerplate.";
    let resolver = EntityResolver::new(Some("user".to_string()));

    c.bench_function("preference_extractor", |b| {
        b.iter(|| {
            let _ = PreferenceExtractor::extract(
                std::hint::black_box(input),
                std::hint::black_box(&resolver),
            );
        });
    });
}

fn bench_claim_extractor(c: &mut Criterion) {
    let input = "Alice is a software engineer. Bob has a computer. language: Rust. \
                 From now on always use tabs. Don't use Comic Sans. Carol is a designer.";
    let resolver = EntityResolver::new(None);

    c.bench_function("claim_extractor", |b| {
        b.iter(|| {
            let _ = ClaimExtractor::extract(
                std::hint::black_box(input),
                std::hint::black_box(&resolver),
            );
        });
    });
}

fn bench_pipeline_short(c: &mut Criterion) {
    let input = "I prefer dark mode.";
    let pipeline = RawInputProcessor::new();
    let ctx = IngestContext::default();

    c.bench_function("pipeline_short", |b| {
        b.iter(|| {
            let _ = pipeline.process(std::hint::black_box(input), std::hint::black_box(&ctx));
        });
    });
}

fn bench_pipeline_medium(c: &mut Criterion) {
    let input = "Alice is a software engineer at Acme Corp. \
                 I prefer dark mode for all editors. \
                 From now on use four spaces for indentation. \
                 Don't use tabs. Bob has a computer and a monitor. \
                 language: Rust. framework: Tokio.";
    let pipeline = RawInputProcessor::new();
    let ctx = IngestContext::default();

    c.bench_function("pipeline_medium", |b| {
        b.iter(|| {
            let _ = pipeline.process(std::hint::black_box(input), std::hint::black_box(&ctx));
        });
    });
}

fn bench_deduplication(c: &mut Criterion) {
    // Build a vec with 25 copies of the same candidate (same dedup key) plus
    // 25 unique entries, to stress the dedup map.
    let validator = MergedExtractionValidator;
    let duplicated = {
        let mut v: Vec<CandidateMemory> = (0..25)
            .map(|_| make_candidate("alice", "prefers", "dark mode"))
            .collect();
        let unique: Vec<CandidateMemory> = (0..25_u32)
            .map(|i| make_candidate("user", "prefers", &format!("item-{i}")))
            .collect();
        v.extend(unique);
        v
    };

    c.bench_function("deduplication", |b| {
        b.iter(|| {
            let _ = validator.deduplicate(std::hint::black_box(duplicated.clone()));
        });
    });
}

// ── Groups & entry point ─────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_preference_extractor,
    bench_claim_extractor,
    bench_pipeline_short,
    bench_pipeline_medium,
    bench_deduplication,
);
criterion_main!(benches);
