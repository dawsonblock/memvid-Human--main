# memvid-core

A Rust crate for crash-safe single-file `.mv2` storage with a governed `agent_memory` subsystem.
`memvid-core` packages frames, indices, and metadata into one portable file — no database and no
sidecar files.

[![Crates.io](https://img.shields.io/crates/v/memvid-core)](https://crates.io/crates/memvid-core)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)

---

## What it is

`memvid-core` is a Rust crate, not a CLI contract or hosted service contract. It exposes two
supported public surfaces:

- The storage kernel centered on `Memvid`, which owns a single `.mv2` file and its indices.
- The always-on `agent_memory` subsystem centered on `MemoryController`, which adds bounded,
  policy-governed agent memory semantics on top of that storage kernel.

Adjacent CLI and Docker assets live in this repository, but the crate contract is defined here.

The `.mv2` storage kernel contains:

- **Document frames** — arbitrary bytes (text, PDF text, markdown, audio transcripts, image embeddings)
- **Full-text index** — BM25 search via [Tantivy](https://github.com/quickwit-oss/tantivy) (feature `lex`, on by default)
- **Vector index** — approximate nearest-neighbour search via HNSW (feature `vec`, optional)
- **Time index** — chronological ordering and filtering
- **Crash-safe WAL** — all writes stage through an embedded write-ahead log; the file is always in a consistent state after a crash
- **Table of contents footer** — self-describing format; no external metadata files

The format is defined in [MV2_SPEC.md](MV2_SPEC.md).

## Supported Public Surfaces

| Surface | Entry points | Status | Notes |
|---|---|---|---|
| Storage kernel | `Memvid`, `PutOptions`, `SearchRequest`, `TimelineQuery`, verification/report types | Blocking-tested | `--no-default-features` keeps the single-file kernel available; the default profile adds the standard search and extraction stack. |
| Governed agent memory | `agent_memory::MemoryController`, `CandidateMemory`, `DurableMemory`, `RetrievalQuery`, `PolicySet` | Supported public subsystem | Always compiled. `MemoryController` is the canonical ingest/retrieval path; lower-level stores remain public for advanced integrations and test coverage. |

## Supported Profiles

| Profile | Features | Status | Notes |
|---|---|---|---|
| Minimal storage kernel | `--no-default-features` | Blocking-tested | Single-file storage, commit/reopen, manifests, and direct frame access without the default search or extraction stack. |
| Default crate profile | `lex,pdf_extract,simd` | Blocking-tested | Recommended cross-platform baseline and the profile used for the main docs, examples, and CI. |
| Search + local vectors | `lex,pdf_extract,simd,vec` | Best effort | Requires local ONNX model files and adds HNSW search plus local text embeddings. |
| Encryption | `lex,pdf_extract,simd,encryption` | Best effort | Password-protected `.mv2e` capsules layered around the storage kernel. |
| Media, platform, and specialist integrations | `clip`, `whisper`, `pdfium`, `extractous`, `pdf_oxide`, `logic_mesh`, `api_embed`, `temporal_*`, `parallel_segments`, `symspell_cleanup`, `replay` | Best effort or platform-sensitive | Available opt-ins, but not part of the blocking compatibility matrix in this pass. |

---

## Quick start

```toml
# Cargo.toml
[dependencies]
memvid-core = "2"
```

```rust
use memvid_core::{Memvid, PutOptions, Result, SearchRequest, TimelineQuery};

fn main() -> Result<()> {
    // Create a new memory file
    let mut mem = Memvid::create("notes.mv2")?;

    // Insert documents
    mem.put_bytes(b"Attention Is All You Need introduces the Transformer architecture.")?;

    let opts = PutOptions::builder()
        .title("Project brief")
        .uri("mv2://docs/brief.md")
        .tag("project", "alpha")
        .build();
    mem.put_bytes_with_options(b"Q3 goal: ship the alpha release.", opts)?;

    // Persist (flushes WAL, rebuilds indices)
    mem.commit()?;

    // Full-text search (requires feature `lex`, which is on by default)
    let results = mem.search(SearchRequest {
        query: "transformer architecture".to_string(),
        top_k: 5,
        snippet_chars: 200,
        ..Default::default()
    })?;

    for hit in results.hits {
        println!("[{}] {:.3} — {}", hit.frame_id, hit.score.unwrap_or(0.0), hit.text);
    }

    // Browse by insertion order
    let entries = mem.timeline(TimelineQuery::default())?;
    for e in entries {
        println!("{} — {}", e.frame_id, e.preview);
    }

    // Reopen later
    drop(mem);
    let mem = Memvid::open("notes.mv2")?;
    println!("{} frames", mem.stats()?.frame_count);

    Ok(())
}
```

---

## Core API

| Method | Description |
|---|---|
| `Memvid::create(path)` | Create a new `.mv2` file |
| `Memvid::open(path)` | Open an existing `.mv2` file |
| `mem.put_bytes(bytes)` | Insert raw bytes as a frame |
| `mem.put_bytes_with_options(bytes, opts)` | Insert with title, URI, and key/value tags |
| `mem.commit()` | Flush WAL and rebuild indices |
| `mem.search(request)` | Full-text search (requires `lex` feature) |
| `mem.ask(request, embedder)` | Context-retrieval for RAG (requires `lex` feature) |
| `mem.timeline(query)` | Chronological browse with optional filters |
| `mem.stats()` | Frame count, index presence, size |
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

| Feature | Default | Support | Description |
|---|---|---|---|
| `lex` | ✅ | Blocking-tested | BM25 full-text search via Tantivy. Required for `search` and `ask`. |
| `pdf_extract` | ✅ | Blocking-tested | Pure-Rust PDF text extraction used by the default profile. |
| `simd` | ✅ | Blocking-tested | SIMD-accelerated distance computation for search and embedding utilities. |
| `vec` | ❌ | Best effort | HNSW vector search with local ONNX embeddings. Requires manually downloaded model files. |
| `encryption` | ❌ | Best effort | AES-256-GCM `.mv2e` capsules with Argon2 key derivation. |
| `api_embed` | ❌ | Best effort | OpenAI-compatible API embeddings. Requires network access and provider credentials. |
| `temporal_track` | ❌ | Best effort | Natural-language date parsing and temporal filtering. |
| `temporal_enrich` | ❌ | Best effort | Post-ingest temporal enrichment of relative time references. |
| `parallel_segments` | ❌ | Best effort | Multi-threaded segment ingestion. |
| `symspell_cleanup` | ❌ | Best effort | Spell-correction pass for noisy PDF text. |
| `replay` | ❌ | Best effort | Time-travel replay of agent sessions and state changes. |
| `logic_mesh` | ❌ | Experimental | Entity-relationship graph extraction and hybrid retrieval helpers. |
| `clip` | ❌ | Platform-sensitive | CLIP visual embeddings for image search. Pulls in ONNX and image-runtime dependencies. |
| `whisper` | ❌ | Platform-sensitive | On-device audio transcription via Candle. Heavy dependency stack and optional GPU backends. |
| `pdf_oxide` | ❌ | Experimental | Alternate high-accuracy PDF extraction path with different tradeoffs than the default extractor. |
| `pdfium` | ❌ | Platform-sensitive | PDFium-backed PDF processing path. Useful for some rendering-heavy workflows, but not part of the blocking matrix. |
| `extractous` | ❌ | Platform-sensitive | GraalVM-backed rich document extraction for PDFs and office-style formats. |

Platform/backend and developer-only feature flags remain available but are not part of the main
support contract in this pass: `mmap`, `metal`, `cuda`, `accelerate`, and `hnsw_bench`.

Build with specific features:

```bash
cargo build --features "lex,vec,temporal_track"
```

---

## Document ingestion

`memvid-core` can ingest several document types beyond raw bytes:

- **Plain text and Markdown** — always available
- **PDF** — the default profile uses `pdf_extract` (pure Rust); `pdf_oxide`, `pdfium`, and `extractous` are alternate opt-in paths with different compatibility and platform tradeoffs
- **XLSX / spreadsheets** — structured extraction into rows
- **DOCX and broader rich document formats** — best effort via `extractous`
- **Audio transcripts** — via `whisper` feature (model downloaded on first use)
- **Images** — via `clip` feature (visual embedding, CLIP model)

For PDFs, the raw text is chunked and each chunk is stored as a separate frame so search results
point to specific passages rather than whole documents.

---

## Vector search setup

The `vec` feature requires downloading ONNX model weights before use. Example for the default
BGE-small model (384 dimensions, ~120 MB):

```bash
mkdir -p ~/.cache/memvid/text-models

curl -L 'https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx' \
  -o ~/.cache/memvid/text-models/bge-small-en-v1.5.onnx

curl -L 'https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json' \
  -o ~/.cache/memvid/text-models/bge-small-en-v1.5_tokenizer.json
```

Other supported models: `bge-base-en-v1.5` (768d), `nomic-embed-text-v1.5` (768d),
`gte-large` (1024d). See the example at `examples/text_embedding.rs`.

To prevent accidental model mixing, pin the model after first write:

```rust
mem.set_vec_model("bge-small-en-v1.5")?;
```

---

## Whisper audio transcription

The `whisper` feature runs speech-to-text locally using Candle. Weights are downloaded on first
use from Hugging Face.

```rust
use memvid_core::{WhisperConfig, WhisperTranscriber};

let config = WhisperConfig::default(); // whisper-small-en (244 MB FP32)
let transcriber = WhisperTranscriber::new(&config)?;
let result = transcriber.transcribe_file("interview.mp3")?;
println!("{}", result.text);
```

| Model | Size | Notes |
|---|---|---|
| `whisper-small-en` | 244 MB | Default; highest accuracy |
| `whisper-tiny-en` | 75 MB | Faster |
| `whisper-tiny-en-q8k` | 19 MB | Fastest; lowest accuracy |

---

## Encryption

The `encryption` feature wraps a `.mv2` file into a password-protected `.mv2e` capsule using
Argon2 key derivation and AES-256-GCM authenticated encryption.

---

## Agent memory subsystem

`agent_memory` is the governed memory subsystem built on top of the `Memvid` kernel. It is always
compiled and treated as a supported public subsystem in this crate. The canonical path is
`MemoryController`, which classifies, routes, promotes, consolidates, retrieves, and audits memory
records while keeping storage in the same `.mv2` kernel.

By "human memory" this repository means bounded operational memory for agents: evidence,
current facts, task state, self-model, and procedures. It does not claim to model human cognition
or consciousness.

Key public entry points live under `memvid_core::agent_memory`:

| Entry point | Role |
|---|---|
| `memory_controller::MemoryController` | Canonical ingest and retrieval authority |
| `schemas::CandidateMemory` | Ingest payload before routing and promotion |
| `schemas::DurableMemory` | Persisted memory representation used by stores and adapters |
| `schemas::RetrievalQuery` | Query contract for agent-memory retrieval |
| `policy::PolicySet` | Thresholds and semantic governance knobs |

The subsystem provides a **six-layer bounded memory architecture**:

| Layer | Purpose |
|---|---|
| `Trace` | Raw observations; high volume, short retention |
| `Episode` | Consolidated event records |
| `Belief` | Long-lived factual assertions with conflict detection |
| `GoalState` | Active task objectives and their status |
| `SelfModel` | Agent self-description and capability model |
| `Procedure` | Learned procedural knowledge with lifecycle governance |

Ingest is done through `MemoryController::ingest(candidate)`, which classifies, promotes, and
audits every memory candidate before storage. Typed retrieval through `RetrievalQuery` remains the
canonical read path; `MemoryController::retrieve_text(...)` and `RetrievalQuery::from_text(...)`
are convenience helpers for obvious text-only queries, not the authoritative semantic router.
Durable memories now preserve immutable ingest time separately from mutable update/access time.
Retrieval-touch persistence is explicit policy: `PolicySet` defaults `persist_retrieval_touches` to
enabled, so retrieval may append durable access-touch records and update effective
`retrieval_count`/`last_accessed_at`; callers can disable that policy without changing retrieval
results. Access-touch history remains append-only today — there is no logical rollup or compaction
pass for those records. `MemoryController::run_maintenance()` is the explicit, caller-driven
maintenance entrypoint: it lists current durable memories, runs `MemoryDecay`, emits a maintenance
audit event, returns expired ids, and reports `MemoryCompactor` as unsupported together with its
unsupported reason. See
[docs/agent_memory.md](docs/agent_memory.md) for details.

---

## File format

This section and [MV2_SPEC.md](MV2_SPEC.md) describe the storage-kernel contract only. They do not
define the `agent_memory` policy surface, CLI behavior, or Docker image semantics.

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

Invariants: no sidecar files; append-only frames; all integers little-endian;
Blake3 checksums per frame; Ed25519 signatures optional.

Full specification: [MV2_SPEC.md](MV2_SPEC.md)  
Architecture overview: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## Build and test

```bash
# Blocking support matrix
cargo test --no-default-features
cargo test --features "lex,pdf_extract,simd"

# Full default-path verification
cargo test
cargo test -- --nocapture
cargo test --test lifecycle

# Lint and format
cargo clippy
cargo fmt

# Example opt-in builds
cargo build --release --features "lex,vec,temporal_track"
```

**Minimum supported Rust version:** 1.85.0  
**Edition:** 2024

---

## Examples

```bash
cargo run --example basic_usage
cargo run --example pdf_ingestion
cargo run --example text_embedding           --features vec
cargo run --example openai_embedding         --features api_embed
cargo run --example clip_visual_search       --features clip
cargo run --example test_whisper --features whisper -- /path/to/audio.mp3
```

---

## License

[Apache License 2.0](LICENSE)
