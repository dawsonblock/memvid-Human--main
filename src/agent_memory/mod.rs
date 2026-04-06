//! Governed agent-memory layer built on top of the memvid kernel.

pub mod adapters;
pub mod audit;
pub mod belief_store;
pub mod belief_updater;
pub mod clock;
pub mod consolidation_engine;
pub mod enums;
pub mod episode_store;
pub mod errors;
pub mod goal_state_store;
pub mod memory_classifier;
pub mod memory_compactor;
pub mod memory_controller;
pub mod memory_decay;
pub mod memory_intake;
pub mod memory_promoter;
pub mod memory_retriever;
pub mod policy;
pub mod procedure_store;
pub mod query_intent;
pub mod ranker;
pub mod retention;
pub mod schemas;
pub mod self_model_store;
pub mod source_trust;

pub use memory_controller::MemoryController;
