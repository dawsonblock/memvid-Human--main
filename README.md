<div align="center">

# memvid-core

**Crash-safe single-file `.mv2` storage with governed agent memory — in pure Rust**

[![Crates.io](https://img.shields.io/crates/v/memvid-core)](https://crates.io/crates/memvid-core)
[![docs.rs](https://img.shields.io/docsrs/memvid-core)](https://docs.rs/memvid-core)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)
[![CI](https://img.shields.io/github/actions/workflow/status/memvid/memvid/ci.yml?branch=main&label=CI)](https://github.com/memvid/memvid/actions)

[Quick Start](#quick-start) · [API Reference](#core-api) · [Features](#features) · [Agent Memory](#agent-memory-subsystem) · [File Format](#file-format) · [Examples](#examples)

</div>

---

`memvid-core` packages document frames, full-text and vector indices, and metadata into **one portable `.mv2` file** — no database, no sidecar files, crash-safe by design. It exposes two supported public surfaces:

| Surface | Entry point | Notes |
|---|---|---|
| **Storage kernel** | `Memvid` | WAL-backed writes, BM25 + HNSW search, timeline queries, encryption |
| **Agent memory** | `agent_memory::MemoryController` | Policy-governed seven-layer bounded memory built on the storage kernel |

The crate is synchronous, single-file first, and has no required network dependency. The `.mv2` format is fully specified in [MV2_SPEC.md](MV2_SPEC.md).

### What lives in a `.mv2` file

- **Document frames** — arbitrary bytes: text, PDF chunks, Markdown, audio transcripts, image embeddings
- **Full-text index** — BM25 via [Tantivy](https://github.com/quickwit-oss/tantivy) (`lex` feature, on by default)
- **Vector index** — approximate nearest-neighbour via HNSW (`vec` feature, optional)
- **Time index** — chronological ordering and range filtering
- **Crash-safe WAL** — all writes stage through an embedded write-ahead log; the file is always consistent after a crash
- **Self-describing footer** — table of contents with segment offsets; no external metadata

### Supported build profiles

| Profile | Feature flags | Status |
|---|---|---|
| Minimal kernel | `--no-default-features` | Blocking-tested |
| Default *(recommended)* | `lex,pdf_extract,simd` | Blocking-tested |
| + Local vector search | `lex,pdf_extract,simd,vec` | Best effort |
| + Encryption | `lex,pdf_extract,simd,encryption` | Best effort |
| Media / specialist | `clip`, `whisper`, `pdfium`, `extractous`, `logic_mesh`, … | Best effort / platform-sensitive |

---

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
memvid-core = "2"
```

```rust
use memvid_core::{Memvid, PutOptions, Result, SearchRequest, TimelineQuery};

fn main() -> Result<()> {
    // --- Write ---
    let mut mem = Memvid::create("notes.mv2")?;

    mem.put_bytes(b"Attention Is All You Need introduces the Transformer architecture.")?;

    let opts = PutOptions::builder()
        .title("Project brief")
        .uri("mv2://docs/brief.md")
        .tag("project", "alpha")
        .build();
    mem.put_bytes_with_options(b"Q3 goal: ship the alpha release.", opts)?;

    mem.commit()?; // flush WAL, rebuild indices

    // --- Search ---
    let results = mem.search(SearchRequest {
        query: "transformer architecture".to_string(),
        top_k: 5,
        snippet_chars: 200,
        ..Default::default()
    })?;

    for hit in &results.hits {
        println!("[{}] {:.3} — {}", hit.frame_id, hit.score.unwrap_or(0.0), hit.text);
    }

    // --- Browse chronologically ---
    let entries = mem.timeline(TimelineQuery::default())?;
    for e in &entries {
        println!("{} — {}", e.frame_id, e.preview);
    }

    // --- Reopen later ---
    drop(mem);
    let mem = Memvid::open("notes.mv2")?;
    println!("{} frames", mem.stats()?.frame_count);

    Ok(())
}
```

---

## Core API

### `Memvid` — storage kernel

| Method | Description |
|---|---|
| `Memvid::create(path)` | Create a new `.mv2` file |
| `Memvid::open(path)` | Open an existing `.mv2` file |
| `mem.put_bytes(bytes)` | Insert raw bytes as a frame |
| `mem.put_bytes_with_options(bytes, opts)` | Insert with title, URI, and key/value tags |
| `mem.commit()` | Flush WAL and rebuild indices |
| `mem.search(request)` | BM25 full-text search (requires `lex`) |
| `mem.ask(request, embedder)` | Context-retrieval for RAG (requires `lex`) |
| `mem.timeline(query)` | Chronological browse with optional filters |
| `mem.stats()` | Frame count, index presence, size on disk |
| `Memvid::verify(path, deep)` | Integrity check — returns a `VerificationReport` |
| `mem.doctor(opts)` | Diagnostic scan and repair plan |

### `PutOptions`

```rust
let opts = PutOptions::builder()
    .title("My doc")
    .uri("mv2://scope/name")
    .tag("key", "value")
    .build();
```

### `SearchRequest`

```rust
let req = SearchRequest {
    query: "transformer".to_string(),
    top_k: 10,
    snippet_chars: 300,
    scope: Some("mv2://docs/".to_string()), // optional URI prefix filter
    ..Default::default()
};
```

---

## Features

| Feature | Default | Tier | Description |
|---|:---:|---|---|
| `lex` | ✅ | Blocking-tested | BM25 full-text search via Tantivy. Required for `search` and `ask`. |
| `pdf_extract` | ✅ | Blocking-tested | Pure-Rust PDF text extraction. |
| `simd` | ✅ | Blocking-tested | SIMD-accelerated distance computation. |
| `vec` | ❌ | Best effort | HNSW vector search with local ONNX embeddings. Requires downloaded model files. |
| `encryption` | ❌ | Best effort | AES-256-GCM `.mv2e` capsules with Argon2 key derivation. |
| `api_embed` | ❌ | Best effort | OpenAI-compatible API embeddings. Requires network + credentials. |
| `temporal_track` | ❌ | Best effort | Natural-language date parsing and temporal filtering. |
| `temporal_enrich` | ❌ | Best effort | Post-ingest temporal enrichment of relative time references. |
| `parallel_segments` | ❌ | Best effort | Multi-threaded segment ingestion. |
| `symspell_cleanup` | ❌ | Best effort | Spell-correction pre-pass for noisy OCR/PDF text. |
| `replay` | ❌ | Best effort | Time-travel replay of agent sessions and state changes. |
| `logic_mesh` | ❌ | Experimental | Entity-relationship graph extraction and hybrid retrieval helpers. |
| `clip` | ❌ | Platform-sensitive | CLIP visual embeddings for image search (ONNX + image runtime). |
| `whisper` | ❌ | Platform-sensitive | On-device audio transcription via Candle (heavy deps, optional GPU). |
| `pdf_oxide` | ❌ | Experimental | Alternate high-accuracy PDF extraction path. |
| `pdfium` | ❌ | Platform-sensitive | PDFium-backed PDF processing for rendering-heavy workflows. |
| `extractous` | ❌ | Platform-sensitive | GraalVM-backed rich document extraction (PDFs, office formats). |

> Platform/backend and developer-only flags (`mmap`, `metal`, `cuda`, `accelerate`, `hnsw_bench`) are available but not part of the blocking support contract.

```bash
cargo build --features "lex,vec,temporal_track"
```

---

## Document ingestion

`memvid-core` can ingest several document types beyond raw bytes:

| Format | How |
|---|---|
| Plain text / Markdown | Always available |
| PDF | `pdf_extract` (default, pure Rust); or `pdf_oxide` / `pdfium` / `extractous` for alternate paths |
| XLSX / spreadsheets | Structured row extraction |
| DOCX and rich formats | Best effort via `extractous` |
| Audio | Transcription via `whisper` (model downloaded on first use) |
| Images | Visual embedding via `clip` (CLIP model) |

PDFs are chunked at ingest time — each chunk is its own frame, so search results point to specific passages rather than whole documents.

---

## Vector search

The `vec` feature adds HNSW vector search with local ONNX embeddings. Download model weights before use:

```bash
mkdir -p ~/.cache/memvid/text-models

# BGE-small (384d, ~120 MB) — default
curl -L 'https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx' \
  -o ~/.cache/memvid/text-models/bge-small-en-v1.5.onnx

curl -L 'https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json' \
  -o ~/.cache/memvid/text-models/bge-small-en-v1.5_tokenizer.json
```

Other supported models: `bge-base-en-v1.5` (768d), `nomic-embed-text-v1.5` (768d), `gte-large` (1024d). See [`examples/text_embedding.rs`](examples/text_embedding.rs).

Pin the model after first write to prevent accidental dimension mismatches:

```rust
mem.set_vec_model("bge-small-en-v1.5")?;
```

---

## Whisper audio transcription

The `whisper` feature runs speech-to-text locally via Candle. Weights are downloaded automatically on first use.

```rust
use memvid_core::{WhisperConfig, WhisperTranscriber};

let transcriber = WhisperTranscriber::new(&WhisperConfig::default())?;
let result = transcriber.transcribe_file("interview.mp3")?;
println!("{}", result.text);
```

| Model | Size | Notes |
|---|---|---|
| `whisper-small-en` | 244 MB | Default — highest accuracy |
| `whisper-tiny-en` | 75 MB | Faster |
| `whisper-tiny-en-q8k` | 19 MB | Fastest, lowest accuracy |

---

## Encryption

The `encryption` feature wraps a `.mv2` file into a password-protected `.mv2e` capsule using Argon2 key derivation and AES-256-GCM authenticated encryption.

---

## Agent memory subsystem

`agent_memory` is a policy-governed, bounded memory layer built on top of the `Memvid` kernel. It is always compiled and treated as a supported public subsystem. The canonical path is `MemoryController`, which classifies, routes, promotes, consolidates, retrieves, and audits memory records — all within the same `.mv2` file.

> **Terminology**: "agent memory" means bounded operational memory for software agents — evidence, current facts, task state, self-model, and learned procedures. It does not model human cognition.

### Public entry points (`memvid_core::agent_memory`)

| Type | Role |
|---|---|
| `MemoryController` | Canonical ingest and retrieval authority |
| `CandidateMemory` | Ingest payload before routing and promotion |
| `DurableMemory` | Persisted memory representation |
| `RetrievalQuery` | Query contract for agent-memory retrieval |
| `PolicySet` | Thresholds and semantic governance knobs |

### Seven-layer memory architecture

| Layer | Purpose |
|---|---|
| `Trace` | Raw observations — high volume, short retention |
| `Episode` | Consolidated event records |
| `Belief` | Long-lived factual assertions with conflict detection |
| `GoalState` | Active task objectives and their status |
| `SelfModel` | Agent self-description and capability model |
| `Procedure` | Learned procedural knowledge with lifecycle governance |
| `Correction` | Explicit corrections to prior beliefs; bypasses eligibility gate; 1.1× retrieval score; TTL 30 days, decay 0.04/day |

### Key behaviours

- **Ingest**: `MemoryController::ingest(candidate)` classifies, promotes, and audits every candidate before storage.
- **Retrieval**: `MemoryController::retrieve_text(...)` and `RetrievalQuery::from_text(...)` are convenience helpers; `RetrievalQuery` is the authoritative semantic router.
- **Immutable ingest time**: durable memories preserve ingest time separately from mutable update/access timestamps.
- **Read-only by default**: `PolicySet` defaults `persist_retrieval_touches` to **`false`** — retrieval never writes access-touch records or updates `retrieval_count` / `last_accessed_at` unless the caller opts in:

  ```rust
  // Opt in to touch persistence
  let policy = PolicySet::default().with_persist_retrieval_touches(true);
  // Also enable at the store level for the production store
  let store = MemvidStore::with_access_touch_persistence(memvid, true);
  ```

- **Historical queries are always read-only**: `RetrievalQuery { as_of: Some(t), .. }` never records touches regardless of policy.
- **Maintenance**: call `MemoryController::run_maintenance()` explicitly — it runs `MemoryDecay`, emits a maintenance audit event, and returns expired IDs.

See [docs/agent_memory.md](docs/agent_memory.md) for the full specification.

---

## File format

> This section and [MV2_SPEC.md](MV2_SPEC.md) describe the storage-kernel contract only. They do not define agent memory policy, CLI behaviour, or Docker image semantics.

```
┌────────────────────────────┐
│ Header (4 KB)              │  Magic MV2\0, version 2.1, capacity
├────────────────────────────┤
│ Embedded WAL (1–64 MB)     │  Crash-safe staging at byte 4096
├────────────────────────────┤
│ Data segments              │  Zstd/LZ4/raw compressed frames
├────────────────────────────┤
│ Lex index (Tantivy)        │  BM25 full-text
├────────────────────────────┤
│ Vec index (HNSW)           │  384d cosine similarity
├────────────────────────────┤
│ Time index                 │  Chronological frame ordering
├────────────────────────────┤
│ TOC footer (MVTC magic)    │  Segment offsets and manifests
└────────────────────────────┘
```

**Invariants:** no sidecar files · append-only frames · all integers little-endian · Blake3 checksums per frame · Ed25519 signatures optional

- Full specification: [MV2_SPEC.md](MV2_SPEC.md)
- Architecture overview: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## Build and test

```bash
# Blocking support matrix
cargo test --no-default-features
cargo test --features "lex,pdf_extract,simd"

# Full default profile
cargo test
cargo test -- --nocapture
cargo test --test lifecycle

# Lint and format
cargo clippy
cargo fmt

# Release builds
cargo build --release --features "lex,vec,temporal_track"
```

**Minimum supported Rust version:** 1.85.0 · **Edition:** 2024

---

## Examples

```bash
cargo run --example basic_usage
cargo run --example pdf_ingestion
cargo run --example text_embedding           --features vec
cargo run --example openai_embedding         --features api_embed
cargo run --example clip_visual_search       --features clip
cargo run --example test_whisper             --features whisper -- /path/to/audio.mp3
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Security issues: [SECURITY.md](SECURITY.md).

---

## License

[Apache License 2.0](LICENSE)
