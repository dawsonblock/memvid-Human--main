mod common;

use memvid_core::agent_memory::enums::{MemoryType, SourceType};

use common::{apply_durable, controller, durable, ts};

/// A memory whose text uses the British spelling "summarise" is found when the
/// query uses the American spelling "summarize".  The lexical path scores 0
/// (different characters at position 7), so the hit comes entirely from the
/// semantic expansion path.
#[test]
fn semantic_near_miss_found() {
    let (mut ctrl, _) = controller(ts(1_700_001_000));
    let memory = durable(
        "alan",
        "pref",
        "brevity",
        "please summarise",
        MemoryType::Preference,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    apply_durable(&mut ctrl, &memory, None);

    let results = ctrl
        .retrieve_text("summarize answers")
        .expect("retrieval succeeds");

    assert!(
        !results.is_empty(),
        "semantic expansion (summarize → summarise) should surface the hit"
    );
    let returned_ids: Vec<&str> = results
        .iter()
        .filter_map(|h| h.memory_id.as_deref())
        .collect();
    assert!(
        returned_ids.contains(&memory.memory_id.as_str()),
        "expected memory_id '{}' in results; got {:?}",
        memory.memory_id,
        returned_ids
    );
}

/// The lexical path still returns exact-match memories after the semantic merge
/// is unconditionally wired into the retrieval pipeline.
#[test]
fn lexical_query_still_works() {
    let (mut ctrl, _) = controller(ts(1_700_001_000));
    let memory = durable(
        "user",
        "editor",
        "neovim",
        "user prefers neovim for coding",
        MemoryType::Preference,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    apply_durable(&mut ctrl, &memory, None);

    let results = ctrl
        .retrieve_text("neovim editor preference")
        .expect("retrieval succeeds");

    assert!(
        !results.is_empty(),
        "lexical match should be returned with semantic merge active"
    );
    let returned_ids: Vec<&str> = results
        .iter()
        .filter_map(|h| h.memory_id.as_deref())
        .collect();
    assert!(
        returned_ids.contains(&memory.memory_id.as_str()),
        "expected memory_id '{}' in results (got {:?})",
        memory.memory_id,
        returned_ids
    );
}

/// A memory found by BOTH the lexical path and a synonym expansion must appear
/// exactly once in the final result list.  `merge_semantic_hits` boosts the
/// existing lexical hit in-place rather than appending a duplicate, and
/// `dedup_hits` in the retriever provides a second layer of protection.
#[test]
fn dedup_merges_lexical_and_semantic() {
    let (mut ctrl, _) = controller(ts(1_700_001_000));
    // "verbose" is in the query → lexical path finds this memory.
    // SYNONYM_PAIRS entry ("verbose","detailed") means "detailed" expansion also finds it.
    let memory = durable(
        "user",
        "style",
        "verbose",
        "verbose detailed responses preferred",
        MemoryType::Preference,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    apply_durable(&mut ctrl, &memory, None);

    let results = ctrl
        .retrieve_text("verbose answers")
        .expect("retrieval succeeds");

    let hit_ids: Vec<&str> = results
        .iter()
        .filter_map(|h| h.memory_id.as_deref())
        .collect();

    assert!(!results.is_empty(), "memory should be present in results");

    let unique_count = hit_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(
        hit_ids.len(),
        unique_count,
        "each memory_id should appear exactly once; found {:?}",
        hit_ids
    );
}
