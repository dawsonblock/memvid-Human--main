# memvid-core

A Rust library for single-file AI agent memory. `memvid-core` packages documents, full-text and
vector search indices, and metadata into a portable `.mv2` file — no database, no sidecar files.

[![Crates.io](https://img.shields.io/crates/v/memvid-core)](https://crates.io/crates/memvid-core)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)

---

## What it is

`memvid-core` is a Rust library (not a CLI or server) that provides a read/write handle to a
single `.mv2` binary file. The file contains:

- **Document frames** — arbitrary bytes (text, PDF text, markdown, audio transcripts, image embeddings)
- **Full-text index** — BM25 search via [Tantivy](https://github.com/quickwit-oss/tantivy) (feature `lex`, on by default)
- **Vector index** — approximate nearest-neighbour search via HNSW (feature `vec`, optional)
- **Time index** — chronological ordering and filtering
- **Crash-safe WAL** — all writes stage through an embedded write-ahead log; the file is always in a consistent state after a crash
- **Table of contents footer** — self-describing format; no external metadata files

The format is defined in [MV2_SPEC.md](MV2_SPEC.md).

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

| Feature | Default | Description |
|---|---|---|
| `lex` | ✅ | BM25 full-text search (Tantivy). Required for `search` and `ask`. |
| `pdf_extract` | ✅ | PDF text extraction (pdfium-based). |
| `simd` | ✅ | SIMD-accelerated vector distance computation. |
| `vec` | ❌ | HNSW vector search with local ONNX embeddings. Requires manually downloaded model files. |
| `api_embed` | ❌ | OpenAI-compatible API embeddings (`OPENAI_API_KEY` must be set). |
| `clip` | ❌ | CLIP visual embeddings for image search. |
| `whisper` | ❌ | On-device audio transcription via Candle. |
| `encryption` | ❌ | AES-256-GCM encrypted `.mv2e` capsules (Argon2 key derivation). |
| `temporal_track` | ❌ | Natural-language date parsing and temporal filtering. |
| `temporal_enrich` | ❌ | Async enrichment of temporal metadata after ingest. |
| `parallel_segments` | ❌ | Multi-threaded segment ingestion. |
| `symspell_cleanup` | ❌ | Spell-correction pass for noisy PDF text. |
| `logic_mesh` | ❌ | Entity-relationship NER graph for hybrid retrieval. |
| `replay` | ❌ | Time-travel session replay. |

Build with specific features:

```bash
cargo build --features "lex,vec,temporal_track"
```

---

## Document ingestion

`memvid-core` can ingest several document types beyond raw bytes:

- **Plain text and Markdown** — always available
- **PDF** — enabled by `pdf_extract`, `pdf_oxide`, `extractous`, or `pdfium` features
- **XLSX / spreadsheets** — structured extraction into rows
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

`agent_memory` is a higher-level module built on top of the core `Memvid` kernel. It provides
a **six-layer bounded memory architecture** governed by a single `MemoryController`:

| Layer | Purpose |
|---|---|
| `Trace` | Raw observations; high volume, short retention |
| `Episode` | Consolidated event records |
| `Belief` | Long-lived factual assertions with conflict detection |
| `GoalState` | Active task objectives and their status |
| `SelfModel` | Agent self-description and capability model |
| `Procedure` | Learned procedural knowledge with lifecycle governance |

Ingest is done through `MemoryController::ingest(candidate)`, which classifies, promotes, and
audits every memory candidate before storage. See [docs/agent_memory.md](docs/agent_memory.md)
for details.

---

## File format

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
# Build
cargo build
cargo build --release
cargo build --release --features "lex,vec,temporal_track"

# Test
cargo test
cargo test -- --nocapture
cargo test --test lifecycle

# Lint and format
cargo clippy
cargo fmt
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
