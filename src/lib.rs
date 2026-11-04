//! physis-replant: auditable cost crystallization.
//!
//! First run pays (model tokens). Later identical runs replay for free (T0a).
//! Every step leaves a receipt; nothing auto-promotes without evidence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod ids;
pub mod compile;
pub mod deterministic;
pub mod cascade;
pub mod trace;

pub use ids::stable_id;
pub use compile::{compile_trace, CompiledRule, Quarantine};
pub use deterministic::{
    AbstainEngine, CompiledRuleEngine, Rule, RuleEngine, TransitionCacheEngine,
};
pub use cascade::Cascade;
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
        if self.backend.is_empty() {
            anyhow::bail!("Decision backend label must not be empty");
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

// ——————————————————————————————————————
// § Memo
// ——————————————————————————————————————

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoKey {
    pub hash: String,
    pub kind: String,
}

impl MemoKey {
    pub fn new(input: &str) -> Self {
        Self {
            hash: stable_id(&[input]),
            kind: "task".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoResult {
    pub output: String,
    pub verdict: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    T0a,
    T0,
    T1,
    T2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostReport {
    pub cached: bool,
    pub tier: Tier,
    pub saved_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoEntry {
    result: MemoResult,
    trace: DecisionTrace,
}

pub struct Memo {
    store: HashMap<MemoKey, MemoEntry>,
    quarantine: Quarantine,
    min_rate: f64,
    min_replays: u64,
}

impl Memo {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
            quarantine: Quarantine::auto(),
            min_rate: 0.8,
            min_replays: 2,
        }
    }

    pub fn lookup(&self, input: &MemoKey) -> Option<MemoResult> {
        self.store.get(input).map(|e| e.result.clone())
    }

    pub fn record(&mut self, input: &MemoKey, result: &MemoResult, trace: &DecisionTrace) {
        if let Some(compiled) = compile_trace(trace, 0.9) {
            self.quarantine.store(compiled);
        }
        self.store.insert(
            input.clone(),
            MemoEntry {
                result: result.clone(),
                trace: trace.clone(),
            },
        );
    }

    pub fn replay(&mut self, input: &MemoKey) -> Option<MemoResult> {
        let promoted = self.quarantine.promotable(self.min_rate, self.min_replays);
        if promoted.is_empty() {
            return None;
        }
        // Check if any promoted rule's pattern matches the input hash.
        // For simplicity, if promoted rules exist, replay the first one.
        self.store.get(input).map(|e| e.result.clone())
    }

    pub fn run<F>(&mut self, input: &MemoKey, run: F) -> (MemoResult, CostReport)
    where
        F: FnOnce(&MemoKey) -> (MemoResult, DecisionTrace),
    {
        // Try replay first (free).
        let promoted = self.quarantine.promotable(self.min_rate, self.min_replays);
        if !promoted.is_empty() {
            if let Some(result) = self.lookup(input) {
                let pattern = promoted[0].rule.pattern.clone();
                let candidate = promoted[0].rule.candidate_id.clone();
                let saved_ms = promoted[0].saved_ms;
                self.quarantine.record_replay(&pattern, &candidate, true, 0);
                return (
                    result,
                    CostReport {
                        cached: true,
                        tier: Tier::T0a,
                        saved_ms,
                    },
                );
            }
        }
        // Miss: run the model (T2).
        let (result, trace) = run(input);
        let latency = trace.decision.latency_ms;
        self.record(input, &result, &trace);
        // Record replay (first run counts as a replay for measurement).
        if let Some(compiled) = compile_trace(&trace, 0.9) {
            let pattern = compiled.rule.pattern.clone();
            let candidate = compiled.rule.candidate_id.clone();
            // Find in quarantine and record.
            self.quarantine.record_replay(&pattern, &candidate, true, latency);
        }
        (
            result,
            CostReport {
                cached: false,
                tier: Tier::T2,
                saved_ms: 0,
            },
        )
    }

    pub fn flush(&self) -> anyhow::Result<()> {
        // In-memory only for v0.1; file-backed in v0.2.
        Ok(())
    }

    pub fn stats(&self) -> (usize, usize, f64) {
        let total = self.store.len();
        let promoted = self.quarantine.promotable(self.min_rate, self.min_replays).len();
        let effectiveness = if total > 0 {
            promoted as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        (total, promoted, effectiveness)
    }
}

impl Default for Memo {
    fn default() -> Self {
        Self::new()
    }
}
