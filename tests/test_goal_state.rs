mod common;

use memvid_core::agent_memory::enums::{GoalStatus, MemoryType, SourceType};
use memvid_core::agent_memory::goal_state_store::GoalStateStore;

use common::{apply_durable, controller, durable, ts};

#[test]
fn goal_state_store_persists_and_lists_active_goals() {
    let (mut controller, _) = controller(ts(1_700_000_000));
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

    let goal_id = apply_durable(&mut controller, &goal_memory, Some("episode-1"));

    let active_goals = {
        let mut goal_store = GoalStateStore::new(controller.store_mut());
        goal_store.list_active().expect("active goals listed")
    };

    assert_eq!(goal_id, goal_memory.memory_id);
    assert_eq!(active_goals.len(), 1);
    assert_eq!(active_goals[0].status, GoalStatus::Blocked);
    assert_eq!(active_goals[0].supporting_episode_ids, vec!["episode-1"]);
}
