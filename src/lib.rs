// SPDX-License-Identifier: Apache-2.0
// Cognition types extracted from physis-next; modified validation and module wiring.
//! physis-replant: auditable cost crystallization.
//!
//! Learn a recipe supplied by an external planner, verify it with trusted tools,
//! and re-execute its actions and checks without calling a model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod cascade;
pub mod compile;
pub mod deterministic;
pub mod ids;
pub mod mcp;
mod runner;
pub mod server;
mod store;
pub mod trace;
pub mod workflow;

pub use workflow::{
    similarity, LearnedRecord, Memo, Operation, Receipt, Recipe, RecordInput, Rejection, Request,
    RerecordInput, ReviewCandidate, ReviewInput, StaleRecord, Stats, Status, ToolCall, ToolKind,
    ToolOutcome, ToolRegistry, ToolSpec, REVIEW_THRESHOLD,
};

pub use cascade::Cascade;
pub use compile::{compile_trace, CompiledRule, Quarantine};
pub use deterministic::{
    AbstainEngine, CompiledRuleEngine, Rule, RuleEngine, TransitionCacheEngine,
};
pub use ids::stable_id;
pub use trace::state_digest;

// ——————————————————————————————————————
// § DecisionEngine contract
// ——————————————————————————————————————

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    Embed,
    Choose,
    Check,
    Score,
    Rerank,
    Generate,
    Vision,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionCandidate {
    pub id: String,
    pub semantic_description: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub description: String,
    pub refs: Vec<String>,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposition {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub selected: Option<String>,
    pub probabilities: Vec<(String, f32)>,
    pub confidence: f32,
    pub backend: String,
    pub latency_ms: u128,
}

impl Decision {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.backend.trim().is_empty() {
            anyhow::bail!("Decision backend label must not be empty");
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            anyhow::bail!("Decision confidence must be finite and in [0, 1]");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionTrace {
    pub state_digest: String,
    pub candidates: Vec<(String, f32)>,
    pub decision: Decision,
    pub escalated: bool,
    pub escalations: Vec<String>,
    pub at: String,
}

pub trait DecisionEngine: Send + Sync {
    fn id(&self) -> &'static str;
    fn cost_class(&self) -> u8;
    fn capabilities(&self) -> &'static [Capability];
    fn choose(
        &self,
        state: &State,
        options: &[OptionCandidate],
    ) -> anyhow::Result<Option<Decision>>;
    fn score(&self, state: &State, criterion: &Criterion) -> anyhow::Result<Option<f32>>;
    fn check(&self, state: &State, proposition: &Proposition) -> anyhow::Result<Option<f32>>;
}
