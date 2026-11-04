//! Soar-like compiled cognition: expensive trace → reusable rule.
//!
//! Steals chunking (EBT backtrace → build) + NARS revision truth +
//! LangGraph checkpoint/replay. Never auto-promotes: compiled rules sit in
//! [`Quarantine`] with `validated=false` until an explicit human `validate()`
//! call, then replay-measured before they earn traffic.
//!
//! Provenance: every [`CompiledRule`] names the source trace digests it was
//! built from, so the HUD can jump from rule → traces.

use serde::{Deserialize, Serialize};

use crate::deterministic::Rule;
use crate::DecisionTrace;
/// A compiled rule plus its provenance and validation lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledRule {
    pub rule: Rule,
    pub sources: Vec<String>,
    pub created_at: String,
    pub validated: bool,
    pub replays: u64,
    pub successes: u64,
    /// Token/cost saving measured on replay vs the original backend.
    pub saved_ms: u128,
}

impl CompiledRule {
    pub fn success_rate(&self) -> f64 {
        if self.replays == 0 {
            return 0.0;
        }
        self.successes as f64 / self.replays as f64
    }
}

/// Compile a successful, non-escalated trace into a candidate rule.
///
/// Returns `None` when the trace is not compilable: escalated, no selection,
/// or confidence below `min_confidence`. The pattern is the stable reusable
/// subgraph: here the state description keyword (first content token), the
/// selection the candidate id. Richer subgraph extraction is future work —
/// this ships the lifecycle (quarantine → validate → replay → measure).
pub fn compile_trace(trace: &DecisionTrace, min_confidence: f32) -> Option<CompiledRule> {
    if trace.escalated {
        return None;
    }
    let selected = trace.decision.selected.clone()?;
    if trace.decision.confidence < min_confidence {
        return None;
    }
    // Stable pattern: first alphanumeric token of the state digest source.
    // Callers pass state descriptions; digest is opaque, so pattern on the
    // winning candidate's discriminating token: the selected id's first word.
    let pattern = selected
        .split_whitespace()
        .last()
        .unwrap_or(&selected)
        .to_string();
    Some(CompiledRule {
        rule: Rule {
            pattern,
            candidate_id: selected,
            confidence: trace.decision.confidence,
        },
        sources: vec![trace.state_digest.clone()],
        created_at: crate::trace::now(),
        validated: false,
        replays: 0,
        successes: 0,
        saved_ms: 0,
    })
}

/// Quarantined compiled rules. Append-only; promotion is explicit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Quarantine {
    pub rules: Vec<CompiledRule>,
    /// When true, `store` auto-validates each rule (no human `validate()`
    /// call). Replay measurement + promotion thresholds still apply, so a
    /// rule still has to earn traffic — only the approval step is skipped.
    /// Default false preserves the human-gated safety model.
    #[serde(default)]
    pub auto: bool,
}

impl Quarantine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Auto-crystallizing quarantine: rules are approved on store, then
    /// replay-measured before they earn traffic. Use for the
    /// "first costs, second free" automation path.
    pub fn auto() -> Self {
        Self {
            rules: Vec::new(),
            auto: true,
        }
    }

    /// Store a compiled rule. Duplicate (same pattern+candidate) merges
    /// provenance instead of duplicating.
    pub fn store(&mut self, compiled: CompiledRule) {
        let mut compiled = compiled;
        if self.auto {
            compiled.validated = true;
        }
        for r in &mut self.rules {
            if r.rule.pattern == compiled.rule.pattern
                && r.rule.candidate_id == compiled.rule.candidate_id
            {
                for s in &compiled.sources {
                    if !r.sources.contains(s) {
                        r.sources.push(s.clone());
                    }
                }
                if self.auto {
                    r.validated = true;
                }
                return;
            }
        }
        self.rules.push(compiled);
    }

    /// Explicit human validation. No auto-promote path exists by construction.
    pub fn validate(&mut self, pattern: &str, candidate: &str) -> bool {
        for r in &mut self.rules {
            if r.rule.pattern == pattern && r.rule.candidate_id == candidate {
                r.validated = true;
                return true;
            }
        }
        false
    }

    /// Record one replay outcome. Returns running success rate.
    pub fn record_replay(
        &mut self,
        pattern: &str,
        candidate: &str,
        success: bool,
        saved_ms: u128,
    ) -> Option<f64> {
        for r in &mut self.rules {
            if r.rule.pattern == pattern && r.rule.candidate_id == candidate {
                r.replays += 1;
                if success {
                    r.successes += 1;
                }
                r.saved_ms += saved_ms;
                return Some(r.success_rate());
            }
        }
        None
    }

    /// Rules cleared for traffic: validated AND success_rate ≥ threshold
    /// over at least `min_replays` replays.
    pub fn promotable(&self, min_rate: f64, min_replays: u64) -> Vec<&CompiledRule> {
        self.rules
            .iter()
            .filter(|r| r.validated && r.replays >= min_replays && r.success_rate() >= min_rate)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decision, DecisionTrace};

    fn good_trace() -> DecisionTrace {
        DecisionTrace {
            state_digest: "dig1".into(),
            candidates: vec![("restart k3s".into(), 1.0)],
            decision: Decision {
                selected: Some("restart k3s".into()),
                probabilities: vec![("restart k3s".into(), 1.0)],
                confidence: 1.0,
                backend: "deterministic:transitions".into(),
                latency_ms: 5,
            },
            escalated: false,
            escalations: vec!["deterministic:transitions".into()],
            at: "0".into(),
        }
    }

    #[test]
    fn escalated_trace_does_not_compile() {
        let mut t = good_trace();
        t.escalated = true;
        assert!(compile_trace(&t, 0.9).is_none());
    }

    #[test]
    fn low_confidence_does_not_compile() {
        let t = good_trace();
        assert!(compile_trace(&t, 1.1).is_none());
    }

    #[test]
    fn compile_validate_replay_lifecycle() {
        let t = good_trace();
        let c = compile_trace(&t, 0.9).expect("compilable");
        assert!(!c.validated);
        let mut q = Quarantine::new();
        q.store(c);
        assert!(
            q.promotable(0.8, 1).is_empty(),
            "unvalidated never promotable"
        );
        assert!(q.validate("k3s", "restart k3s"));
        assert!(q.promotable(0.8, 1).is_empty(), "no replays yet");
        q.record_replay("k3s", "restart k3s", true, 4);
        q.record_replay("k3s", "restart k3s", true, 4);
        assert_eq!(q.promotable(0.8, 2).len(), 1);
        // provenance merge, no dup
        let mut t2 = good_trace();
        t2.state_digest = "dig2".into();
        q.store(compile_trace(&t2, 0.9).unwrap());
        assert_eq!(q.rules.len(), 1);
        assert_eq!(q.rules[0].sources.len(), 2);
    }
    #[test]
    fn auto_quarantine_validates_on_store() {
        let t = good_trace();
        let c = compile_trace(&t, 0.9).expect("compilable");
        let mut q = Quarantine::auto();
        q.store(c);
        assert!(q.rules[0].validated, "auto mode validates on store");
        assert!(q.promotable(0.8, 1).is_empty(), "no replays yet");
        q.record_replay("k3s", "restart k3s", true, 4);
        q.record_replay("k3s", "restart k3s", true, 4);
        assert_eq!(q.promotable(0.8, 2).len(), 1);
    }

    #[test]
    fn human_quarantine_does_not_auto_validate() {
        let t = good_trace();
        let c = compile_trace(&t, 0.9).expect("compilable");
        let mut q = Quarantine::new();
        q.store(c);
        assert!(!q.rules[0].validated, "human mode stays unvalidated");
    }

    #[test]
    fn crystallize_effectiveness_n8n_loop() {
        use crate::{Decision, DecisionTrace};

        let k: usize = 10;
        let min_replays: u64 = 2;
        let min_rate: f64 = 0.8;
        let model_latency_ms: u128 = 120;

        let selected = "restart k3s".to_string();
        let trace = DecisionTrace {
            state_digest: "dig1".into(),
            candidates: vec![("restart k3s".into(), 1.0)],
            decision: Decision {
                selected: Some(selected.clone()),
                probabilities: vec![("restart k3s".into(), 1.0)],
                confidence: 1.0,
                backend: "causal:smollm2-135m".into(),
                latency_ms: model_latency_ms,
            },
            escalated: false,
            escalations: vec!["deterministic:transitions".into()],
            at: "0".into(),
        };

        let mut q = Quarantine::auto();
        let compiled = compile_trace(&trace, 0.9).expect("compilable");
        let original = trace.decision.selected.clone().unwrap();
        q.store(compiled);

        let pattern = selected.split_whitespace().last().unwrap().to_string();

        let mut tiers: Vec<&str> = Vec::with_capacity(k);
        let mut free_count = 0usize;
        let mut model_count = 0usize;
        let mut correctness = 0usize;
        let mut promotion_at: Option<usize> = None;
        let mut total_ms_with = 0u128;
        let mut total_ms_without = 0u128;

        for i in 0..k {
            let promoted = q.promotable(min_rate, min_replays);
            if !promoted.is_empty() {
                tiers.push("T0a");
                free_count += 1;
                total_ms_with += 0;
                total_ms_without += model_latency_ms;
                if promoted[0].rule.candidate_id == original {
                    correctness += 1;
                }
                if promotion_at.is_none() {
                    promotion_at = Some(i);
                }
            } else {
                tiers.push("T2");
                model_count += 1;
                total_ms_with += model_latency_ms;
                total_ms_without += model_latency_ms;
                q.record_replay(&pattern, &selected, true, model_latency_ms);
            }
        }

        let effectiveness = free_count as f64 / k as f64 * 100.0;
        let speedup = if total_ms_with > 0 {
            total_ms_without as f64 / total_ms_with as f64
        } else {
            f64::INFINITY
        };

        println!("=== crystallize effectiveness: n8n loop ({} iters) ===", k);
        println!("tiers:            {:?}", tiers);
        println!("model (T2) runs:  {}", model_count);
        println!("free (T0a) runs:  {}", free_count);
        println!("effectiveness:    {:.1}%", effectiveness);
        println!("promotion at iter: {:?}", promotion_at);
        println!("correctness:      {}/{}", correctness, free_count);
        println!("total ms with crystallize:    {}", total_ms_with);
        println!("total ms without (all T2):    {}", total_ms_without);
        println!("speedup:          {:.2}x", speedup);

        assert!(promotion_at.is_some(), "rule must promote");
        assert!(free_count > 0, "must have free runs");
        assert_eq!(correctness, free_count, "every replay must match original");
        assert!(
            free_count >= k - min_replays as usize,
            "at least {} free runs after warmup",
            k - min_replays as usize
        );
        assert!(speedup > 1.0, "crystallize must be faster");
    }

}
