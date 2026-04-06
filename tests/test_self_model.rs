mod common;

use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, SelfModelKind, SourceType};
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{durable, ts};

#[test]
fn self_model_store_persists_and_filters_matching_preferences() {
    let mut store = InMemoryMemoryStore::default();
    let preference_memory = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&preference_memory, Some("episode-1"))
            .expect("self-model stored");
    }

    let all_records = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };
    let matching_records = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .matching_values("user", "response_style", "concise")
            .expect("matching self-model records listed")
    };

    assert_eq!(all_records.len(), 1);
    assert_eq!(all_records[0].kind, SelfModelKind::ResponseStyle);
    assert_eq!(all_records[0].status, BeliefStatus::Active);
    assert_eq!(matching_records.len(), 1);
}
