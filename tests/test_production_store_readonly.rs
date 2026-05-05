//! Verifies that the default [`MemvidStore`] does **not** write access-touch
//! frames to disk even when the retrieval policy explicitly requests touch
//! persistence.  This is the production-safety guarantee: a caller must
//! opt-in at **both** the store level *and* the policy level before any
//! access-touch side-effects land in the `.mv2` file.
#![cfg(feature = "lex")]

mod common;

use std::sync::Arc;

use memvid_core::Memvid;
use memvid_core::agent_memory::adapters::memvid_store::{MemoryStore, MemvidStore};
use memvid_core::agent_memory::audit::{AuditLogger, InMemoryAuditSink};
use memvid_core::agent_memory::belief_updater::BeliefUpdater;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_classifier::MemoryClassifier;
use memvid_core::agent_memory::memory_controller::MemoryController;
use memvid_core::agent_memory::memory_promoter::MemoryPromoter;
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::RetrievalQuery;
use tempfile::tempdir;

use common::{durable, ts};

/// Entity label used internally by [`MemvidStore`] to store access-touch cards.
const ACCESS_ENTITY: &str = "__agent_memory_access__";

/// Returns the number of access-touch cards persisted for `memory_id` by
/// reopening the file from disk.
fn access_touch_count(path: &std::path::Path, memory_id: &str) -> usize {
    let memvid = Memvid::open(path).expect("memvid reopened for touch count");
    memvid.memories().get_cards(ACCESS_ENTITY, memory_id).len()
}

/// Build a `MemoryController<MemvidStore>` using the supplied store and policy.
fn make_controller(store: MemvidStore, policy: PolicySet) -> MemoryController<MemvidStore> {
    let now = ts(1_700_000_000);
    let clock = Arc::new(FixedClock::new(now));
    MemoryController::new(
        store,
        clock.clone(),
        AuditLogger::new(clock.clone(), Arc::new(InMemoryAuditSink::default())),
        MemoryClassifier,
        MemoryPromoter::new(policy.clone()),
        BeliefUpdater,
        MemoryRetriever::new(Ranker, RetentionManager::new(policy)),
    )
}

// ---------------------------------------------------------------------------
// Core invariant: default MemvidStore never persists touches
// ---------------------------------------------------------------------------

/// `MemvidStore::new` must advertise `persists_access_touches() == false`.
#[test]
fn default_memvid_store_reports_touch_persistence_disabled() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("touch-flag.mv2");
    let memvid = Memvid::create(&path).expect("create");
    let store = MemvidStore::new(memvid);
    assert!(
        !store.persists_access_touches(),
        "MemvidStore::new must default to persists_access_touches = false"
    );
}

/// A full controller round-trip — import a memory, retrieve it with a policy
/// that requests touch persistence — must write **zero** access-touch frames
/// when the store was created with the default constructor.
#[test]
fn controller_with_default_store_writes_no_touch_frames_on_retrieve() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("prod-readonly.mv2");

    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    // --- write phase ---------------------------------------------------------
    {
        let memvid = Memvid::create(&path).expect("create");
        // Default store: persist_access_touches = false.
        let store = MemvidStore::new(memvid);
        // Policy explicitly requests touch persistence — the store must veto it.
        let policy = PolicySet::default().with_persist_retrieval_touches(true);
        let mut controller = make_controller(store, policy);

        let _id = controller
            .apply_governed_memory_for_import(memory.clone(), None)
            .expect("import succeeds");

        // Retrieve: retrieval_touch_persistence_enabled() should be false
        // because store.persists_access_touches() == false, even though the
        // policy says true.
        let hits = controller
            .retrieve(RetrievalQuery {
                query_text: "vim editor preference".to_string(),
                intent: QueryIntent::PreferenceLookup,
                entity: Some("user".to_string()),
                slot: Some("favorite_editor".to_string()),
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
                namespace_strict: false,
                user_id: None,
                project_id: None,
                task_id: None,
                thread_id: None,
            })
            .expect("retrieve succeeds");

        assert!(
            !hits.is_empty(),
            "at least one hit expected for the imported memory"
        );
        // controller (and its MemvidStore) are dropped here, flushing to disk
    }

    // --- verify phase --------------------------------------------------------
    // Reopen the file and count access-touch cards for this memory.
    assert_eq!(
        access_touch_count(&path, &memory.memory_id),
        0,
        "default MemvidStore must not write any access-touch frames to disk"
    );
}

// ---------------------------------------------------------------------------
// Opt-in sanity-check: when both store AND policy enable touches, frames land
// ---------------------------------------------------------------------------

/// Confirms the positive case: opting in at both levels does produce touch
/// frames.  This keeps the negative test above from being a vacuous proof.
#[test]
fn controller_with_touch_enabled_store_writes_touch_frames_on_retrieve() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("prod-touch-enabled.mv2");

    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    {
        let memvid = Memvid::create(&path).expect("create");
        // Explicit opt-in at store level.
        let store = MemvidStore::with_access_touch_persistence(memvid, true);
        // Explicit opt-in at policy level.
        let policy = PolicySet::default().with_persist_retrieval_touches(true);
        let mut controller = make_controller(store, policy);

        let _id = controller
            .apply_governed_memory_for_import(memory.clone(), None)
            .expect("import succeeds");

        controller
            .retrieve(RetrievalQuery {
                query_text: "vim editor preference".to_string(),
                intent: QueryIntent::PreferenceLookup,
                entity: Some("user".to_string()),
                slot: Some("favorite_editor".to_string()),
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
                namespace_strict: false,
                user_id: None,
                project_id: None,
                task_id: None,
                thread_id: None,
            })
            .expect("retrieve succeeds");
        // drop → flush
    }

    assert!(
        access_touch_count(&path, &memory.memory_id) > 0,
        "opt-in MemvidStore must persist at least one access-touch frame"
    );
}
