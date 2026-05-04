use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::MemoryLayer;
use super::errors::Result;
use super::memory_retriever::MemoryRetriever;
use super::schemas::{MemoryContextPacket, RetrievalHit, RetrievalQuery};

/// Wraps [`MemoryRetriever`] and partitions its hits into a structured
/// [`MemoryContextPacket`] organised by memory layer and status.
#[derive(Debug, Clone)]
pub struct HybridRetriever {
    retriever: MemoryRetriever,
}

impl HybridRetriever {
    pub fn new(retriever: MemoryRetriever) -> Self {
        Self { retriever }
    }

    /// Retrieve and partition memories into a [`MemoryContextPacket`].
    ///
    /// Internally widens `top_k` to `max(top_k, 50)` and enables
    /// `include_expired` so that every bucket can be populated; each bucket
    /// is then truncated back to the caller-requested `top_k`.
    pub fn retrieve_context<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        clock: &dyn Clock,
    ) -> Result<MemoryContextPacket> {
        // Widen the query so every bucket has a chance to be populated.
        let mut wide_query = query.clone();
        wide_query.top_k = query.top_k.max(50);
        wide_query.include_expired = true;

        let all_hits = self.retriever.retrieve(store, &wide_query, clock)?;

        let mut packet = MemoryContextPacket {
            query_text: query.query_text.clone(),
            ..Default::default()
        };

        for hit in all_hits {
            classify_hit(hit, &mut packet);
        }

        // Truncate each bucket to the caller-requested top_k.
        let k = query.top_k;
        packet.direct_facts.truncate(k);
        packet.relevant_episodes.truncate(k);
        packet.user_preferences.truncate(k);
        packet.procedures.truncate(k);
        packet.corrections.truncate(k);
        packet.disputed_items.truncate(k);
        packet.stale_items.truncate(k);

        Ok(packet)
    }
}

/// Classify a single [`RetrievalHit`] into the appropriate bucket of
/// `packet`.  The priority order is:
///
/// 1. **Stale** — expired hits always go to `stale_items`.
/// 2. **Corrections** — Correction layer or metadata tag.
/// 3. **Disputed** — contested/retracted belief status.
/// 4. **Direct facts** — from the Belief layer.
/// 5. **Procedures** — Procedure layer.
/// 6. **Preferences** — SelfModel layer.
/// 7. **Episodes** — Episode or GoalState layer; Trace is the fallback.
fn classify_hit(hit: RetrievalHit, packet: &mut MemoryContextPacket) {
    // 1. Stale wins regardless of layer.
    if hit.expired {
        packet.stale_items.push(hit);
        return;
    }

    // 2. Correction layer or belief_relation metadata tag (Phase A boost).
    if hit.memory_layer == Some(MemoryLayer::Correction)
        || hit
            .metadata
            .get("belief_relation")
            .map(String::as_str)
            == Some("correction")
    {
        packet.corrections.push(hit);
        return;
    }

    // 3. Disputed: belief retrieval status is contested/superseded/retracted.
    let retrieval_status = hit
        .metadata
        .get("belief_retrieval_status")
        .map(String::as_str)
        .unwrap_or("");
    if matches!(
        retrieval_status,
        "contested" | "superseded" | "retracted"
    ) || hit.metadata.contains_key("time_in_dispute_seconds")
    {
        packet.disputed_items.push(hit);
        return;
    }

    // 4. Direct facts — belief layer or from_belief flag.
    if hit.from_belief || hit.memory_layer == Some(MemoryLayer::Belief) {
        packet.direct_facts.push(hit);
        return;
    }

    // 5. Procedures.
    if hit.memory_layer == Some(MemoryLayer::Procedure) {
        packet.procedures.push(hit);
        return;
    }

    // 6. Preferences / self-model.
    if hit.memory_layer == Some(MemoryLayer::SelfModel) {
        packet.user_preferences.push(hit);
        return;
    }

    // 7. Episodes (GoalState counts as episodic context) + Trace fallback.
    packet.relevant_episodes.push(hit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_memory::enums::{MemoryLayer, MemoryType};
    use crate::agent_memory::schemas::RetrievalHit;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn make_hit(layer: Option<MemoryLayer>, expired: bool) -> RetrievalHit {
        RetrievalHit {
            memory_id: None,
            belief_id: None,
            entity: None,
            slot: None,
            value: None,
            text: "test".into(),
            memory_layer: layer,
            memory_type: Some(MemoryType::Fact),
            score: 1.0,
            timestamp: Utc::now(),
            scope: None,
            source: None,
            from_belief: false,
            expired,
            metadata: BTreeMap::new(),
        }
    }

    fn make_hit_with_meta(
        layer: Option<MemoryLayer>,
        expired: bool,
        meta_key: &str,
        meta_val: &str,
    ) -> RetrievalHit {
        let mut hit = make_hit(layer, expired);
        hit.metadata.insert(meta_key.into(), meta_val.into());
        hit
    }

    #[test]
    fn expired_goes_to_stale() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Belief), true), &mut packet);
        assert_eq!(packet.stale_items.len(), 1);
        assert!(packet.direct_facts.is_empty());
    }

    #[test]
    fn correction_layer_goes_to_corrections() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Correction), false), &mut packet);
        assert_eq!(packet.corrections.len(), 1);
    }

    #[test]
    fn belief_relation_meta_goes_to_corrections() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(
            make_hit_with_meta(Some(MemoryLayer::Trace), false, "belief_relation", "correction"),
            &mut packet,
        );
        assert_eq!(packet.corrections.len(), 1);
    }

    #[test]
    fn contested_belief_goes_to_disputed() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(
            make_hit_with_meta(
                Some(MemoryLayer::Belief),
                false,
                "belief_retrieval_status",
                "contested",
            ),
            &mut packet,
        );
        assert_eq!(packet.disputed_items.len(), 1);
        assert!(packet.direct_facts.is_empty());
    }

    #[test]
    fn belief_layer_goes_to_direct_facts() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Belief), false), &mut packet);
        assert_eq!(packet.direct_facts.len(), 1);
    }

    #[test]
    fn from_belief_goes_to_direct_facts() {
        let mut packet = MemoryContextPacket::default();
        let mut hit = make_hit(Some(MemoryLayer::Trace), false);
        hit.from_belief = true;
        classify_hit(hit, &mut packet);
        assert_eq!(packet.direct_facts.len(), 1);
    }

    #[test]
    fn procedure_layer_goes_to_procedures() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Procedure), false), &mut packet);
        assert_eq!(packet.procedures.len(), 1);
    }

    #[test]
    fn self_model_goes_to_user_preferences() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::SelfModel), false), &mut packet);
        assert_eq!(packet.user_preferences.len(), 1);
    }

    #[test]
    fn episode_layer_goes_to_relevant_episodes() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Episode), false), &mut packet);
        assert_eq!(packet.relevant_episodes.len(), 1);
    }

    #[test]
    fn goal_state_goes_to_relevant_episodes() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::GoalState), false), &mut packet);
        assert_eq!(packet.relevant_episodes.len(), 1);
    }

    #[test]
    fn trace_falls_back_to_relevant_episodes() {
        let mut packet = MemoryContextPacket::default();
        classify_hit(make_hit(Some(MemoryLayer::Trace), false), &mut packet);
        assert_eq!(packet.relevant_episodes.len(), 1);
    }
}
