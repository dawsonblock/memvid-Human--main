mod common;

use memvid_core::agent_memory::adapters::memvid_store::MemoryStore;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::MemoryLayer;
use memvid_core::agent_memory::enums::{
    BeliefStatus, MemoryType, SelfModelKind, SelfModelStabilityClass, SelfModelUpdateRequirement,
    SourceType,
};
use memvid_core::agent_memory::policy::ReasonCode;
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{apply_durable, candidate, controller, durable, ts};

#[test]
fn repeated_stable_preference_reinforces_one_logical_self_model_entry() {
    let (mut controller, _) = controller(ts(1_700_000_100));
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
        "The user still prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        ts(1_700_000_100),
    );

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };
    let matching_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .matching_values("user", "response_style", "concise")
            .expect("matching self-model records listed")
    };

    assert_eq!(latest.kind, SelfModelKind::ResponseStyle);
    assert_eq!(
        latest.stability_class,
        SelfModelStabilityClass::FlexiblePreference
    );
    assert_eq!(
        latest.update_requirement,
        SelfModelUpdateRequirement::ReinforcementAllowed
    );
    assert_eq!(latest.status, BeliefStatus::Active);
    assert_eq!(latest.value, "concise");
    assert_eq!(latest.memory_id, first.memory_id);
    assert_eq!(
        latest
            .metadata
            .get("reinforcement_count")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(matching_records.len(), 2);
}

#[test]
fn one_off_preference_observation_stays_episode_evidence_until_repeated() {
    let (mut controller, _) = controller(ts(1_700_000_000));

    controller
        .ingest(candidate(
            "user",
            "response_style",
            "concise",
            "The user prefers concise responses",
        ))
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    let self_model_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };

    assert!(self_model_records.is_empty());
    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(
        controller.store().memories()[0].memory_layer().as_str(),
        "episode"
    );
}

#[test]
fn explicit_trusted_preference_statement_promotes_directly_to_self_model() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut trusted = candidate(
        "user",
        "favorite_editor",
        "vim",
        "The tool profile says the user always prefers vim for editing.",
    );
    trusted.source.source_type = SourceType::Tool;
    trusted.source.source_id = "tool-profile".to_string();
    trusted.source.trust_weight = 0.95;

    let memory_id = controller
        .ingest(trusted)
        .expect("ingest succeeds")
        .expect("self-model stored");

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "favorite_editor")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };

    assert_eq!(memory_id, latest.memory_id);
    assert_eq!(latest.value, "vim");
    assert_eq!(
        controller
            .store()
            .memories()
            .iter()
            .filter(|memory| memory.memory_layer() == MemoryLayer::SelfModel)
            .count(),
        1
    );
    assert_eq!(
        controller
            .store()
            .memories()
            .iter()
            .filter(|memory| memory.memory_layer() == MemoryLayer::Episode)
            .count(),
        1
    );

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion audit event present");
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("trusted_source")
    );
    assert_eq!(
        promotion_event
            .details
            .get("target_layer")
            .map(String::as_str),
        Some("self_model")
    );
}

#[test]
fn rerunning_same_self_model_stabilization_evidence_is_idempotent() {
    let (mut controller, _) = controller(ts(1_700_000_120));
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
        "The user still prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        ts(1_700_000_060),
    );

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let first_outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_120)),
        )
        .expect("consolidation succeeds");
    assert!(first_outcomes.iter().any(|outcome| {
        outcome.record.target_layer == MemoryLayer::SelfModel
            && outcome
                .record
                .metadata
                .get("consolidation_action")
                .map(String::as_str)
                == Some("promotion")
    }));

    let rerun_outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_121)),
        )
        .expect("rerun consolidation succeeds");
    assert!(rerun_outcomes.is_empty());
}

#[test]
fn stronger_contradictory_preference_updates_the_same_logical_trait() {
    let (mut controller, _) = controller(ts(1_700_000_100));
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.7,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "verbose",
        "The system profile now says the user prefers detailed responses",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_100),
    );

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };

    assert_eq!(latest.value, "verbose");
    assert_eq!(latest.status, BeliefStatus::Active);
    assert_eq!(latest.memory_id, first.memory_id);
    assert_eq!(
        latest
            .metadata
            .get("contradiction_resolution")
            .map(String::as_str),
        Some("updated")
    );
}

#[test]
fn weaker_contradictory_preference_is_disputed_without_replacing_active_trait() {
    let (mut controller, _) = controller(ts(1_700_000_100));
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "verbose",
        "One chat turn suggested a more verbose style",
        MemoryType::Preference,
        SourceType::Chat,
        0.55,
        ts(1_700_000_100),
    );

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };
    let all_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };

    assert_eq!(latest.value, "concise");
    assert!(
        all_records
            .iter()
            .any(|record| { record.value == "verbose" && record.status == BeliefStatus::Disputed })
    );
}

#[test]
fn repeated_contradiction_churn_preserves_one_effective_active_trait() {
    let (mut controller, _) = controller(ts(1_700_000_100));
    let stable = durable(
        "user",
        "response_style",
        "concise",
        "The system profile says the user prefers concise responses",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );

    apply_durable(&mut controller, &stable, Some("episode-1"));

    for (index, value) in ["verbose", "narrative", "chatty"].into_iter().enumerate() {
        let contradictory = durable(
            "user",
            "response_style",
            value,
            &format!("One chat turn suggested a more {value} style"),
            MemoryType::Preference,
            SourceType::Chat,
            0.55,
            ts(1_700_000_100 + index as i64),
        );
        let episode_id = format!("episode-{}", index + 2);
        apply_durable(&mut controller, &contradictory, Some(&episode_id));
    }

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };
    let all_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };
    let latest_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .list_latest_for_entity("user")
            .expect("latest self-model entries listed")
    };

    assert_eq!(latest.value, "concise");
    assert_eq!(latest.status, BeliefStatus::Active);
    assert_eq!(
        all_records
            .iter()
            .filter(|record| {
                record.slot == "response_style" && record.status == BeliefStatus::Active
            })
            .count(),
        1
    );
    assert_eq!(
        latest_records
            .iter()
            .filter(|record| record.slot == "response_style")
            .count(),
        1
    );
    assert!(
        all_records
            .iter()
            .filter(|record| {
                record.slot == "response_style" && record.status == BeliefStatus::Disputed
            })
            .count()
            >= 1
    );
}

#[test]
fn self_model_store_rejects_blank_structure_and_latest_valid_trait_survives_bypass_row() {
    let (mut controller, _) = controller(ts(1_700_000_020));
    let invalid = durable(
        "user",
        "response_style",
        "   ",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    assert!(controller.apply_durable_memory(invalid, None).is_err());

    let valid = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_010),
    );
    controller
        .apply_durable_memory(valid.clone(), None)
        .expect("valid self-model stored");

    let mut bypass_invalid = valid.clone();
    bypass_invalid.memory_id = "memory-user-response_style-invalid".to_string();
    bypass_invalid.stored_at = ts(1_700_000_020);
    bypass_invalid.value = "   ".to_string();
    controller
        .store_mut()
        .put_memory(&bypass_invalid)
        .expect("invalid bypass row stored");

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };

    assert_eq!(latest.memory_id, valid.memory_id);
    assert_eq!(latest.value, "concise");
}

#[test]
fn stable_directive_resists_weak_overwrite_and_records_policy_rejection() {
    let (mut controller, sink) = controller(ts(1_700_000_100));
    let stable = durable(
        "agent",
        "memory_constraint",
        "preserve_traceability",
        "Preserve traceability for durable memory changes.",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    controller
        .apply_durable_memory(stable.clone(), None)
        .expect("stable directive stored");

    let weak_conflict = durable(
        "agent",
        "memory_constraint",
        "relax_traceability",
        "Maybe provenance can be skipped when it is inconvenient.",
        MemoryType::Preference,
        SourceType::Chat,
        0.55,
        ts(1_700_000_100),
    );

    let error = controller
        .apply_durable_memory(weak_conflict, None)
        .expect_err("weak stable overwrite rejected");
    assert!(
        error
            .to_string()
            .contains("stable directives require a trusted update path or corroborated evidence")
    );

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("agent", "memory_constraint")
            .expect("latest self-model loaded")
            .expect("stable directive exists")
    };
    assert_eq!(latest.value, "preserve_traceability");
    assert_eq!(latest.kind, SelfModelKind::Constraint);
    assert_eq!(
        latest.stability_class,
        SelfModelStabilityClass::StableDirective
    );

    let rejection_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "policy_rejected")
        .expect("policy rejection event present");
    assert_eq!(
        rejection_event
            .details
            .get("reason_code")
            .map(String::as_str),
        Some(ReasonCode::StableDirectiveUpdateRejected.as_str())
    );
}

#[test]
fn stable_directive_can_update_through_trusted_path() {
    let (mut controller, _) = controller(ts(1_700_000_100));
    let stable = durable(
        "agent",
        "memory_constraint",
        "preserve_traceability",
        "Preserve traceability for durable memory changes.",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    controller
        .apply_durable_memory(stable, None)
        .expect("stable directive stored");

    let trusted_update = durable(
        "agent",
        "memory_constraint",
        "preserve_evidence_integrity",
        "Preserve evidence integrity before accepting reinterpretation.",
        MemoryType::Preference,
        SourceType::Tool,
        0.95,
        ts(1_700_000_100),
    );
    controller
        .apply_durable_memory(trusted_update, None)
        .expect("trusted directive update stored");

    let latest = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("agent", "memory_constraint")
            .expect("latest self-model loaded")
            .expect("stable directive exists")
    };
    assert_eq!(latest.value, "preserve_evidence_integrity");
    assert_eq!(latest.kind, SelfModelKind::Constraint);
    assert_eq!(
        latest.stability_class,
        SelfModelStabilityClass::StableDirective
    );
    assert_eq!(
        latest
            .metadata
            .get("stable_directive_update_path")
            .map(String::as_str),
        Some("trusted_source")
    );
}
