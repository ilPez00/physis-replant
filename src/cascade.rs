//! Escalation cascade: tries ordered engines, accepts the first whose best
//! probability ≥ `accept` threshold, otherwise records the escalation path and
//! (if confidence remains above `escalate_floor`) retains the best sub-threshold
//! decision with `escalated = true`. Tier3/4 generation handoff is wired in
//! WO-5; here the cascade only records what happened.

use std::sync::Arc;

use crate::trace;
use crate::{Decision, DecisionEngine, DecisionTrace, OptionCandidate, State};

/// Ordered engines + thresholds.
pub struct Cascade {
    pub engines: Vec<Arc<dyn DecisionEngine>>,
    pub accept: f32,
    pub escalate_floor: f32,
}

impl Cascade {
    pub fn new(engines: Vec<Arc<dyn DecisionEngine>>, accept: f32, escalate_floor: f32) -> Self {
        Self {
            engines,
            accept,
            escalate_floor,
        }
    }

    /// Walk `engines` in order. The first engine whose best probability
    /// ≥ `accept` wins immediately (`escalated = false`). Otherwise, if the
    /// best sub-threshold decision clears `escalate_floor`, it is retained but
    /// `escalated = true`. If nothing clears `escalate_floor` (or all abstain),
    /// `selected = None` and `escalated = true`.
    pub fn decide(&self, state: &State, options: &[OptionCandidate]) -> DecisionTrace {
        let state_digest = trace::state_digest(state);
        let at = trace::now();
        let mut escalations: Vec<String> = Vec::with_capacity(self.engines.len());
        let mut best: Option<Decision> = None;

        for engine in &self.engines {
            escalations.push(engine.id().to_string());
            match engine.choose(state, options) {
                Ok(Some(decision)) => {
                    if decision.confidence >= self.accept {
                        return DecisionTrace {
                            state_digest,
                            candidates: decision.probabilities.clone(),
                            decision,
                            escalated: false,
                            escalations,
                            at,
                        };
                    }
                    let better = best
                        .as_ref()
                        .map(|b| decision.confidence > b.confidence)
                        .unwrap_or(true);
                    if better {
                        best = Some(decision);
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }

        let best_confidence = best.as_ref().map(|d| d.confidence).unwrap_or(0.0);

        // No engine cleared `accept`. Retain best if it clears `escalate_floor`;
        // otherwise hard-abstain — but keep the candidate scores either way, so
        // the trace stays legible (§19: show actual machine state and scores).
        let decision = if best_confidence >= self.escalate_floor {
            best.unwrap_or_else(|| Decision {
                selected: None,
                probabilities: Vec::new(),
                confidence: 0.0,
                backend: "deterministic:none".to_string(),
                latency_ms: 0,
            })
        } else {
            match best {
                Some(d) => Decision {
                    selected: None,
                    probabilities: d.probabilities,
                    confidence: d.confidence,
                    backend: d.backend,
                    latency_ms: d.latency_ms,
                },
                None => Decision {
                    selected: None,
                    probabilities: Vec::new(),
                    confidence: best_confidence,
                    backend: "deterministic:none".to_string(),
                    latency_ms: 0,
                },
            }
        };

        DecisionTrace {
            state_digest,
            candidates: decision.probabilities.clone(),
            decision,
            escalated: best_confidence < self.accept,
            escalations,
            at,
        }
    }
}

// —————————————— tests ——————————————

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deterministic::{
        AbstainEngine, ExperienceOracle, Rule, RuleEngine, TransitionCacheEngine,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn stub_state() -> State {
        State {
            description: "service(k3s)=down, latency=4300ms".into(),
            refs: vec!["entity:k3s".into()],
            metrics: BTreeMap::new(),
        }
    }

    fn stub_options() -> Vec<OptionCandidate> {
        vec![
            OptionCandidate {
                id: "restart k3s".into(),
                semantic_description: "restart k3s".into(),
                tags: vec![],
            },
            OptionCandidate {
                id: "redeploy k3s".into(),
                semantic_description: "redeploy k3s".into(),
                tags: vec![],
            },
        ]
    }

    struct StubOracle {
        cached: Option<String>,
        transitions: Vec<(String, u32)>,
    }

    impl ExperienceOracle for StubOracle {
        fn previous_transitions(&self, _state: &State) -> Vec<(String, u32)> {
            self.transitions.clone()
        }
        fn preferences(&self, _state: &State) -> Vec<(String, u32)> {
            vec![]
        }
        fn cached_result(&self, _state: &State, _candidates: &[OptionCandidate]) -> Option<String> {
            self.cached.clone()
        }
    }

    #[test]
    fn cascade_deterministic_short_circuits() {
        let oracle = Arc::new(StubOracle {
            cached: Some("restart k3s".into()),
            transitions: vec![],
        });
        let det = Arc::new(TransitionCacheEngine::new(oracle, 0.9));
        let abstain = Arc::new(AbstainEngine);

        // Accept threshold 0.9 — cache hit gives confidence 1.0, so the first
        // engine should win and the AbstainEngine must never be tried.
        let cascade = Cascade::new(vec![det, abstain], 0.9, 0.0);
        let trace = cascade.decide(&stub_state(), &stub_options());

        assert!(!trace.escalated);
        assert_eq!(trace.decision.selected.as_deref(), Some("restart k3s"));
        assert_eq!(trace.decision.backend, "deterministic:transitions");
        assert_eq!(trace.escalations.len(), 1);
        assert_eq!(trace.escalations[0], "deterministic:transitions");
    }

    #[test]
    fn cascade_abstain_escalates() {
        let abstain = Arc::new(AbstainEngine);
        let cascade = Cascade::new(vec![abstain], 0.9, 0.0);

        let trace = cascade.decide(&stub_state(), &stub_options());

        assert!(trace.escalated);
        assert!(trace.decision.selected.is_none());
        assert_eq!(trace.decision.backend, "deterministic:none");
        assert_eq!(trace.escalations, vec!["deterministic:abstain".to_string()]);
    }

    #[test]
    fn cascade_subthreshold_above_floor_keeps_best() {
        // Transitions: "restart k3s" → 2, "redeploy k3s" → 8 → normalised 0.2 / 0.8
        let oracle = Arc::new(StubOracle {
            cached: None,
            transitions: vec![("restart k3s".into(), 2u32), ("redeploy k3s".into(), 8u32)],
        });
        let det = Arc::new(TransitionCacheEngine::new(oracle, 0.0));
        // engine accept=0.0 (proposes whenever data exists);
        // cascade accept=0.95 is the real gate; escalate_floor=0.5 (best=0.8 clears)
        let cascade = Cascade::new(vec![det], 0.95, 0.5);
        let trace = cascade.decide(&stub_state(), &stub_options());

        assert!(trace.escalated);
        // Best confidence 0.8 ≥ floor 0.5 → best decision retained.
        assert_eq!(trace.decision.selected.as_deref(), Some("redeploy k3s"));
        assert!((trace.decision.confidence - 0.8).abs() < 1e-5);
    }

    #[test]
    fn cascade_subthreshold_below_floor_hard_abstains() {
        let oracle = Arc::new(StubOracle {
            cached: None,
            transitions: vec![("restart k3s".into(), 1u32), ("redeploy k3s".into(), 1u32)],
        });
        let det = Arc::new(TransitionCacheEngine::new(oracle, 0.9));
        // Best = 0.5, floor = 0.6 → below floor → hard abstain.
        let cascade = Cascade::new(vec![det], 0.9, 0.6);
        let trace = cascade.decide(&stub_state(), &stub_options());

        assert!(trace.escalated);
        assert!(trace.decision.selected.is_none());
    }

    #[test]
    fn cascade_rule_engine_short_circuits_before_abstain() {
        let rules = vec![Rule {
            pattern: "k3s".into(),
            candidate_id: "restart k3s".into(),
            confidence: 1.0,
        }];
        let rules_engine = Arc::new(RuleEngine::new(rules));
        let abstain = Arc::new(AbstainEngine);

        let cascade = Cascade::new(vec![rules_engine, abstain], 0.9, 0.0);
        let trace = cascade.decide(&stub_state(), &stub_options());

        assert!(!trace.escalated);
        assert_eq!(trace.decision.selected.as_deref(), Some("restart k3s"));
        assert_eq!(trace.escalations.len(), 1);
    }
}
