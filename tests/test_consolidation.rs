mod common;

use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, SourceType};
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{durable, ts};

#[test]
fn consolidation_records_repeated_self_model_preferences() {
    let mut store = InMemoryMemoryStore::default();
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses during repo work",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_060),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&first, Some("episode-1"))
            .expect("first self-model stored");
        self_model_store
            .save_memory(&second, Some("episode-2"))
            .expect("second self-model stored");
    }

    let outcomes = ConsolidationEngine
        .consolidate(
            &mut store,
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_120)),
        )
        .expect("consolidation succeeds");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].record.target_layer, MemoryLayer::SelfModel);
    assert!(outcomes[0].trace_id.starts_with("trace") || !outcomes[0].trace_id.is_empty());
}
