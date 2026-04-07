# Architecture

`memvid-core` has two supported public surfaces: the storage kernel centered on `Memvid`, and the
always-on `agent_memory` subsystem centered on `MemoryController`. This document describes the
internal design behind those surfaces.

The authoritative crate contract and supported-profile matrix live in [README.md](../README.md).
This document is intentionally narrower: it explains composition, internal module boundaries, and
file-format responsibilities. Optional integrations that appear in the module graph are not, by
themselves, a support guarantee.

---

## High-level module layout

The crate root exposes a broader set of low-level modules than the blocking support matrix. Treat
the README as the support contract, and this document as an explanation of how the crate is built.

```
src/
├── lib.rs                  # Public re-exports; all feature-gated items declared here
├── error.rs                # MemvidError enum + Result alias
├── constants.rs            # Magic bytes, base offsets, WAL sizes
├── types/                  # All public data-transfer types
│   ├── frame.rs            # Frame, Stats, TimelineEntry, TimelineQuery
│   ├── options.rs          # PutOptions, PutOptionsBuilder, PutManyOpts
│   ├── search.rs           # SearchRequest, SearchResponse, SearchHit
│   ├── ask.rs              # AskRequest, AskResponse, AskMode, AskStats
│   ├── manifest.rs         # Toc, Header, SegmentCatalog, and all segment manifests
│   ├── verification.rs     # DoctorReport, VerificationReport hierarchies
│   ├── acl.rs              # AclContext, AclEnforcementMode
│   └── ...                 # (temporal, embedding, logic_mesh, etc.)
├── memvid/                 # Memvid struct and all operation implementations
│   ├── lifecycle.rs        # create, open (file I/O, locking, WAL recovery)
│   ├── mutation.rs         # put_bytes, put_bytes_with_options, commit
│   ├── search/             # search() and engine orchestration
│   ├── ask.rs              # ask() — retrieval-augmented generation context
│   ├── timeline.rs         # timeline() — chronological browsing
│   ├── doctor.rs           # diagnostic scan and repair planning
│   ├── frame.rs            # BlobReader — random read access to frame payloads
│   └── ...
├── io/
│   ├── header.rs           # 4 KB file header codec
│   ├── wal.rs              # EmbeddedWal — read/write/replay of WAL entries
│   └── time_index.rs       # Chronological frame index
├── lex.rs                  # LexIndex — Tantivy BM25 wrapper (feature `lex`)
├── vec.rs                  # VecIndex — HNSW approximate nearest-neighbour (feature `vec`)
├── clip.rs                 # ClipIndex — CLIP visual embeddings (feature `clip`)
├── whisper.rs              # WhisperTranscriber — audio transcription (feature `whisper`)
├── reader/                 # DocumentReader trait and format-specific backends
├── agent_memory/           # Governed agent-memory subsystem (see docs/agent_memory.md)
├── pii.rs                  # PII detection utilities
├── signature.rs            # Ed25519 file signing
├── simd.rs                 # SIMD distance kernels (feature `simd`)
├── text_embed.rs           # LocalTextEmbedder ONNX wrapper (feature `vec`)
└── encryption/             # AES-256-GCM capsule wrapping (feature `encryption`)
```

---

## The `.mv2` file format

`MV2_SPEC.md` is the normative format document for this section. It covers storage-kernel
invariants only, not the higher-level `agent_memory` policy model or adjacent CLI/Docker tooling.

Version 2.1. Magic bytes: `4D 56 32 00` (`MV2\0`). All multi-byte integers are little-endian.

```
Offset        Size       Contents
─────────────────────────────────────────────────────────────
0             4 KB       Header (HeaderCodec)
4096          1–64 MB    Embedded WAL (EmbeddedWal) — capacity scaled by tier
var           var        Data segments — Zstd/LZ4/raw frames
var           var        Lex index segment (Tantivy on-disk, embedded; feature `lex`)
var           var        Vec index segment (HNSW bitpacked; feature `vec`)
var           var        Time index segment
var           var        TOC footer (magic MVTC) — segment offsets and manifests
```

### Header (4 KB)

| Field | Offset | Size | Description |
|---|---|---|---|
| Magic | 0 | 4 | `4D 56 32 00` |
| Version | 4 | 2 | Spec version (currently `0x0201` = 2.1) |
| Capacity tier | 6 | 1 | `Tiny`/`Small`/`Medium`/`Large` — controls WAL size |
| Reserved | 7 | 4089 | Zeroed; reserved for future header fields |

### Embedded WAL

The WAL sits at a fixed offset (byte 4096) and grows to at most 64 MB on the largest capacity
tier. Every `put_bytes` call appends a `WalRecord` without touching the data region. On `commit`,
the WAL is replayed into the data region and the indices are rebuilt atomically.

WAL checkpointing triggers when the WAL is ≥ 75% full or after 1,000 transactions.

**WAL record fields:** type tag, frame ID, payload length, encoding, metadata JSON (title, URI, tags,
timestamps), Blake3 payload hash, and the raw payload bytes.

### Data segments

Frames are stored in segments. Each segment starts with a `SegmentCommon` header (type, version,
compression algorithm, uncompressed length, Blake3 hash). Segment types:

| Type byte | Name | Contents |
|---|---|---|
| `0x01` | Data | Raw/compressed frame payloads |
| `0x02` | LexIndex | Embedded Tantivy index files |
| `0x03` | VecIndex | HNSW graph bytes |
| `0x04` | TimeIndex | Chronological frame records |

### TOC footer

The last bytes of the file contain a TOC block with magic `MVTC`. It holds the `Toc` struct
(serialized via `bincode`) which maps every segment type to its `(offset, length)` within the
file. On open, the library scans backward for the last valid footer; if the most recent footer is
corrupted, it falls back to the previous one (footer history is retained).

### URI addressing

Any frame can be given a logical address with a `mv2://` URI:

```
mv2://[track/][path/]name
mv2://meetings/2024-01-15
mv2://docs/api/reference.md
```

The `scope` filter in `SearchRequest` narrows results to a URI prefix.

---

## Write path

```
put_bytes_with_options(bytes, opts)
  │
  ├─ extract text (DocumentReader — PDF, DOCX, plain text, …)
  ├─ chunk into logical passages (plan_text_chunks / plan_document_chunks)
  ├─ auto-tag (AutoTagger)
  ├─ [optional] temporal tag (temporal_track feature)
  ├─ write WalRecord(s) to EmbeddedWal
  └─ update in-memory frame counter (pending_frame_inserts++)

commit()
  │
  ├─ replay WAL → compress payloads → write to data region
  ├─ rebuild lex index (Tantivy)   [feature lex]
  ├─ rebuild/update vec index (HNSW)  [feature vec]
  ├─ rebuild time index
  ├─ serialize + write TOC footer
  └─ truncate + reset WAL
```

Frames are immutable once written to the data region. Logical deletion (tombstoning) is supported
but physical removal requires compaction via `doctor`.

---

## Read / search path

### Full-text search (`lex` feature)

```
search(SearchRequest)
  │
  ├─ validate: lex_enabled, non-empty query
  ├─ [optional] temporal filter → set of matching frame IDs
  ├─ Tantivy BM25 query → ranked SearchHit list
  ├─ [optional] sketch pre-filter for candidate pruning
  ├─ fetch frame metadata + generate text snippets
  └─ return SearchResponse { hits, total_hits, elapsed_ms, … }
```

### RAG context retrieval (`ask`)

```
ask(AskRequest, embedder?)
  │
  ├─ classify question type (aggregation / recency / analytical)
  ├─ sanitize query for lexical search
  ├─ lexical search (BM25) ──┐
  ├─ vector search (HNSW) ───┤ optional RRF fusion
  └─ build AskResponse { context_fragments, citations, stats }
```

### Timeline

`timeline(TimelineQuery)` reads the time index segment directly and returns
`TimelineEntry` records sorted by insertion timestamp, with optional `after` / `before`
frame ID bounds.

---

## Indexing details

### Lexical index (Tantivy)

Each frame's text is tokenized and stored in an embedded Tantivy index segment.
The Tantivy data is written as raw bytes inside a `LexIndex` segment and reloaded on open via
an in-memory directory. Dirty Tantivy state is tracked separately (`tantivy_dirty`) to avoid
unnecessary reindexing on read-only opens.

### Vector index (HNSW)

Parameters: 384-dimensional cosine similarity HNSW (for the default BGE-small model), M=16,
ef-construction=200. The model is loaded from `~/.cache/memvid/text-models/` at runtime.
CLIP embeddings use a separate index (`clip_index`) with different dimensionality.

Model identity is embedded into the TOC manifest so that re-opening with a mismatched model
name returns `MemvidError::ModelMismatch` rather than silently producing wrong results.

---

## Crash safety and file locking

- **WAL**: All mutations are staged in the WAL before any data-region write. On open, any
  unplayed WAL entries are replayed automatically.
- **File locking**: `FileLock` (OS-level) prevents concurrent writers. The lock has a configurable
  timeout and stale-grace period.
- **Atomic commit**: The TOC footer is only updated after all segments are fully written. An
  interrupted commit leaves the previous footer valid.
- **Blake3 checksums**: Each frame records a hash of its payload. `Memvid::verify` validates all
  checksums; `mem.doctor()` reports and can repair inconsistencies.

---

## Encryption (`.mv2e`)

When the `encryption` feature is enabled, `EncryptionCapsule::seal(path, password)` reads a
`.mv2` file and writes a `.mv2e` fle containing:

1. Salt (random, 32 bytes)
2. Argon2id key derivation parameters
3. AES-256-GCM nonce (random, 12 bytes)
4. GCM authentication tag
5. Encrypted payload (the full `.mv2` contents)

Decryption requires the correct password; the authentication tag ensures integrity.

---

## Feature-gated compilation

The crate uses Cargo feature flags extensively. Features that add heavy dependencies (Tantivy,
ONNX runtime, Candle, pdfium) are isolated so the crate can be compiled with a minimal footprint
when only basic functionality is needed. The `[dependencies]` table in `Cargo.toml` uses
`optional = true` for all such dependencies.

The default feature set is `["lex", "pdf_extract", "simd"]`.
