mod common;

use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::enums::{GoalStatus, MemoryType, SourceType};
use memvid_core::agent_memory::goal_state_store::GoalStateStore;

use common::{durable, ts};

#[test]
fn goal_state_store_persists_and_lists_active_goals() {
    let mut store = InMemoryMemoryStore::default();
    let goal_memory = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    let goal_id = {
        let mut goal_store = GoalStateStore::new(&mut store);
        goal_store
            .save_memory(&goal_memory, Some("episode-1"))
            .expect("goal stored")
    };

    let active_goals = {
        let mut goal_store = GoalStateStore::new(&mut store);
        goal_store.list_active().expect("active goals listed")
    };

    assert_eq!(goal_id, goal_memory.memory_id);
    assert_eq!(active_goals.len(), 1);
    assert_eq!(active_goals[0].status, GoalStatus::Blocked);
    assert_eq!(active_goals[0].supporting_episode_ids, vec!["episode-1"]);
}
