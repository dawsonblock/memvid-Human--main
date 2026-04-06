mod common;

use memvid_core::agent_memory::enums::MemoryType;
use memvid_core::agent_memory::memory_classifier::MemoryClassifier;

use common::candidate;

#[test]
fn structured_preference_becomes_preference() {
    let classifier = MemoryClassifier;
    let classified = classifier.classify(candidate(
        "user",
        "favorite_color",
        "blue",
        "The user's favorite color is blue.",
    ));

    assert_eq!(classified.memory_type, MemoryType::Preference);
}

#[test]
fn event_text_becomes_episode() {
    let classifier = MemoryClassifier;
    let mut input = candidate("", "", "", "Yesterday we completed the deployment.");
    input.entity = None;
    input.slot = None;
    input.value = None;

    let classified = classifier.classify(input);

    assert_eq!(classified.memory_type, MemoryType::Episode);
}

#[test]
fn unstructured_text_defaults_to_trace() {
    let classifier = MemoryClassifier;
    let mut input = candidate("", "", "", "Miscellaneous note without structure.");
    input.entity = None;
    input.slot = None;
    input.value = None;

    let classified = classifier.classify(input);

    assert_eq!(classified.memory_type, MemoryType::Trace);
}
