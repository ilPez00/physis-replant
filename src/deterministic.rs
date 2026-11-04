// SPDX-License-Identifier: Apache-2.0
// Extracted from physis-next; modified candidate validation. See NOTICE.
//! Tier-0 deterministic engines (pure, no IO; world data arrives through the
//! [`ExperienceOracle`] provider trait so this module never touches the
//! observation log or any store directly).

use std::sync::Arc;

use crate::compile::CompiledRule;
use crate::{Capability, Criterion, Decision, DecisionEngine, OptionCandidate, Proposition, State};

/// Reads prior experience from the observation log (like `physis.predict` for
/// `run:<argv>` success counts). All methods are pure over whatever the
/// Runtime feeds in.
pub trait ExperienceOracle: Send + Sync {
    /// action key → success count (scans observations for past transitions).
    fn previous_transitions(&self, state: &State) -> Vec<(String, u32)>;
    /// state → (action key, preference weight) descending.
    fn preferences(&self, state: &State) -> Vec<(String, u32)>;
    /// exact-match cache: returns a previously chosen candidate id, if any.
    fn cached_result(&self, state: &State, candidates: &[OptionCandidate]) -> Option<String>;
}

/// Normalise raw success counts into pseudo-probabilities that sum to ≈ 1.0.
fn normalize_counts_over(
    counts: &[(String, u32)],
    options: &[OptionCandidate],
) -> Vec<(String, f32)> {
    let restricted: Vec<(String, u32)> = counts
        .iter()
        .filter(|(id, _)| options.iter().any(|o| o.id == *id))
        .cloned()
        .collect();
    normalize_counts(&restricted)
}

fn normalize_counts(counts: &[(String, u32)]) -> Vec<(String, f32)> {
    let total: u32 = counts.iter().map(|(_, c)| *c).sum();
    if total == 0 {
        return counts.iter().map(|(id, _)| (id.clone(), 0.0)).collect();
    }
    counts
        .iter()
        .map(|(id, c)| (id.clone(), *c as f32 / total as f32))
        .collect()
}

pub struct TransitionCacheEngine {
    pub oracle: Arc<dyn ExperienceOracle>,
    pub accept: f32,
}

impl TransitionCacheEngine {
    pub fn new(oracle: Arc<dyn ExperienceOracle>, accept: f32) -> Self {
        Self { oracle, accept }
    }
}

impl DecisionEngine for TransitionCacheEngine {
    fn id(&self) -> &'static str {
        "deterministic:transitions"
    }

    fn cost_class(&self) -> u8 {
        0
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Choose, Capability::Score]
    }

    fn choose(
        &self,
        state: &State,
        options: &[OptionCandidate],
    ) -> anyhow::Result<Option<Decision>> {
        let start = std::time::Instant::now();

        // 1. Exact-match cache first.
        if let Some(cached_id) = self.oracle.cached_result(state, options) {
            let probs: Vec<(String, f32)> = options
                .iter()
                .map(|o| {
                    let p = if o.id == cached_id { 1.0 } else { 0.0 };
                    (o.id.clone(), p)
                })
                .collect();
            return Ok(Some(Decision {
                selected: Some(cached_id),
                probabilities: probs,
                confidence: 1.0,
                backend: self.id().to_string(),
                latency_ms: start.elapsed().as_millis(),
            }));
        }

        // 2. Rank by prior transition success counts, normalised over the
        //    candidates actually offered for THIS decision. Normalising over
        //    every action the log has ever seen would let unrelated history
        //    depress a candidate's confidence below the accept floor.
        let transitions = self.oracle.previous_transitions(state);
        let scores = normalize_counts_over(&transitions, options);

        let mut probs: Vec<(String, f32)> = Vec::with_capacity(options.len());
        let mut best_score: f32 = -1.0;
        let mut best_id: Option<String> = None;
        for opt in options {
            let score = scores
                .iter()
                .find(|(id, _)| *id == opt.id)
                .map(|(_, p)| *p)
                .unwrap_or(0.0);
            if score > best_score {
                best_score = score;
                best_id = Some(opt.id.clone());
            }
            probs.push((opt.id.clone(), score));
        }

        let confidence = best_score.max(0.0);
        if confidence >= self.accept {
            Ok(Some(Decision {
                selected: best_id,
                probabilities: probs,
                confidence,
                backend: self.id().to_string(),
                latency_ms: start.elapsed().as_millis(),
            }))
        } else {
            Ok(None)
        }
    }

    fn score(&self, _state: &State, _criterion: &Criterion) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }

    fn check(&self, _state: &State, _proposition: &Proposition) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }
}

/// A deterministic rule: if `state.description` contains `pattern`, select
/// `candidate_id` with `confidence`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub candidate_id: String,
    pub confidence: f32,
}

/// Tier-0a: replay of compiled rules from quarantined `CompiledRule`s.
///
/// Fires only rules that a human explicitly validated. Replay evidence is
/// measured after activation: a rule with at least `min_replays` replays and a
/// success rate below `min_rate` stops firing (measured demotion). Requiring
/// `min_replays` *before* the first fire would deadlock — the counters can
/// only move when the rule is allowed to fire once. Backend id
/// `compiled:<pattern>` is stable for the HUD footer + telemetry.
pub struct CompiledRuleEngine {
    rules: Vec<CompiledRule>,
    min_rate: f64,
    min_replays: u64,
}

impl CompiledRuleEngine {
    pub fn new(rules: Vec<CompiledRule>, min_rate: f64, min_replays: u64) -> Self {
        Self {
            rules,
            min_rate,
            min_replays,
        }
    }

    /// Return (pattern, candidate_id) for the first promotable rule whose
    /// pattern matches `state.description`, recording a replay hit. Callers
    /// do NOT mutate counters — this engine is rebuilt per-decision and the
    /// persistence side (`Runtime::record_compiled_replay`) owns writes.
    pub fn match_state(&self, state: &State) -> Option<(&str, &str)> {
        for r in self.rules.iter().filter(|r| {
            r.validated && (r.replays < self.min_replays || r.success_rate() >= self.min_rate)
        }) {
            if state.description.contains(&r.rule.pattern) {
                return Some((&r.rule.pattern, &r.rule.candidate_id));
            }
        }
        None
    }
}

impl DecisionEngine for CompiledRuleEngine {
    fn id(&self) -> &'static str {
        "deterministic:compiled"
    }

    fn cost_class(&self) -> u8 {
        0
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Choose]
    }

    fn choose(
        &self,
        state: &State,
        options: &[OptionCandidate],
    ) -> anyhow::Result<Option<Decision>> {
        let start = std::time::Instant::now();
        if let Some((pattern, candidate)) = self.match_state(state) {
            if !options.iter().any(|option| option.id == candidate) {
                return Ok(None);
            }
            let probs: Vec<(String, f32)> = options
                .iter()
                .map(|o| {
                    let p = if o.id == candidate { 1.0 } else { 0.0 };
                    (o.id.clone(), p)
                })
                .collect();
            return Ok(Some(Decision {
                selected: Some(candidate.to_string()),
                probabilities: probs,
                confidence: 1.0,
                backend: format!("compiled:{pattern}"),
                latency_ms: start.elapsed().as_millis(),
            }));
        }
        Ok(None)
    }

    fn score(&self, _state: &State, _criterion: &Criterion) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }

    fn check(&self, _state: &State, _proposition: &Proposition) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }
}

/// Tier-0: explicit user preferences. First-match wins, confidence 1.0 when
/// applied.
pub struct RuleEngine {
    rules: Vec<Rule>,
}

impl RuleEngine {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }
}

impl DecisionEngine for RuleEngine {
    fn id(&self) -> &'static str {
        "deterministic:rules"
    }

    fn cost_class(&self) -> u8 {
        0
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Choose]
    }

    fn choose(
        &self,
        state: &State,
        options: &[OptionCandidate],
    ) -> anyhow::Result<Option<Decision>> {
        let start = std::time::Instant::now();
        for rule in &self.rules {
            if state.description.contains(&rule.pattern) {
                let probs: Vec<(String, f32)> = options
                    .iter()
                    .map(|o| {
                        let p = if o.id == rule.candidate_id {
                            rule.confidence
                        } else {
                            0.0
                        };
                        (o.id.clone(), p)
                    })
                    .collect();
                return Ok(Some(Decision {
                    selected: Some(rule.candidate_id.clone()),
                    probabilities: probs,
                    confidence: rule.confidence,
                    backend: self.id().to_string(),
                    latency_ms: start.elapsed().as_millis(),
                }));
            }
        }
        Ok(None)
    }

    fn score(&self, _state: &State, _criterion: &Criterion) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }

    fn check(&self, _state: &State, _proposition: &Proposition) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }
}

/// Tier-0: always abstains (`Ok(None)`). Useful for composition and tests
/// where you need to force cascade escalation.
pub struct AbstainEngine;

impl DecisionEngine for AbstainEngine {
    fn id(&self) -> &'static str {
        "deterministic:abstain"
    }

    fn cost_class(&self) -> u8 {
        0
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[]
    }

    fn choose(
        &self,
        _state: &State,
        _options: &[OptionCandidate],
    ) -> anyhow::Result<Option<Decision>> {
        Ok(None)
    }

    fn score(&self, _state: &State, _criterion: &Criterion) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }

    fn check(&self, _state: &State, _proposition: &Proposition) -> anyhow::Result<Option<f32>> {
        Ok(None)
    }
}

// —————————————— tests ——————————————

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiled_rule_cannot_select_an_unoffered_action() {
        let state = State {
            description: "do not restart k3s".into(),
            refs: vec![],
            metrics: Default::default(),
        };
        let rule = CompiledRule {
            rule: Rule {
                pattern: "k3s".into(),
                candidate_id: "restart k3s".into(),
                confidence: 1.0,
            },
            sources: vec![],
            created_at: "0".into(),
            validated: true,
            replays: 2,
            successes: 2,
            saved_ms: 0,
        };
        let engine = CompiledRuleEngine::new(vec![rule], 0.8, 2);
        let options = vec![OptionCandidate {
            id: "inspect logs".into(),
            semantic_description: "read only".into(),
            tags: vec![],
        }];
        assert!(engine.choose(&state, &options).unwrap().is_none());
    }
    use std::collections::BTreeMap;

    #[test]
    fn score_normalization_sums_to_one() {
        let counts = vec![
            ("restart k3s".to_string(), 3u32),
            ("redeploy k3s".to_string(), 1u32),
        ];
        let scores = normalize_counts(&counts);
        let sum: f32 = scores.iter().map(|(_, p)| *p).sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "normalised scores must sum to 1.0, got {sum}"
        );
        assert!(
            scores.iter().all(|(_, p)| *p >= 0.0 && *p <= 1.0),
            "all scores must be in [0, 1]"
        );
        assert!(
            scores[0].1 > scores[1].1,
            "higher count must produce higher score"
        );
    }

    #[test]
    fn score_normalization_zero_counts() {
        let scores = normalize_counts(&[("a".to_string(), 0u32), ("b".to_string(), 0u32)]);
        assert_eq!(scores.len(), 2);
        assert!(scores.iter().all(|(_, p)| *p == 0.0));
    }

    fn compiled_rule(
        pattern: &str,
        validated: bool,
        replays: u64,
        successes: u64,
    ) -> crate::CompiledRule {
        crate::CompiledRule {
            rule: Rule {
                pattern: pattern.into(),
                candidate_id: "restart k3s".into(),
                confidence: 1.0,
            },
            sources: vec![],
            created_at: String::new(),
            validated,
            replays,
            successes,
            saved_ms: 0,
        }
    }

    fn state(desc: &str) -> State {
        State {
            description: desc.into(),
            refs: vec![],
            metrics: BTreeMap::new(),
        }
    }

    #[test]
    fn activated_rule_fires_before_it_has_replay_evidence() {
        let engine = CompiledRuleEngine::new(vec![compiled_rule("k3s", true, 0, 0)], 0.8, 2);
        let hit = engine.match_state(&state("service(k3s)=down"));
        assert!(
            hit.is_some(),
            "activation is the gate, not the replay count"
        );
    }

    #[test]
    fn measured_failure_demotes_a_rule() {
        let engine = CompiledRuleEngine::new(vec![compiled_rule("k3s", true, 2, 0)], 0.8, 2);
        assert!(engine.match_state(&state("service(k3s)=down")).is_none());
        let healthy = CompiledRuleEngine::new(vec![compiled_rule("k3s", true, 2, 2)], 0.8, 2);
        assert!(healthy.match_state(&state("service(k3s)=down")).is_some());
    }

    #[test]
    fn unvalidated_rule_never_fires() {
        let engine = CompiledRuleEngine::new(vec![compiled_rule("k3s", false, 5, 5)], 0.0, 0);
        assert!(engine.match_state(&state("service(k3s)=down")).is_none());
    }

    #[test]
    fn confidence_is_normalised_over_the_offered_candidates_only() {
        let counts = vec![
            ("restart nginx".to_string(), 3u32),
            ("restart k3s".to_string(), 3u32),
            ("unrelated".to_string(), 94u32),
        ];
        let options = vec![
            OptionCandidate {
                id: "restart nginx".into(),
                semantic_description: "restart nginx".into(),
                tags: vec![],
            },
            OptionCandidate {
                id: "restart k3s".into(),
                semantic_description: "restart k3s".into(),
                tags: vec![],
            },
        ];
        let scores = normalize_counts_over(&counts, &options);
        let nginx = scores
            .iter()
            .find(|(id, _)| id == "restart nginx")
            .unwrap()
            .1;
        assert!(
            (nginx - 0.5).abs() < 1e-5,
            "unrelated history must not dilute a candidate offered here, got {nginx}"
        );
    }

    #[test]
    fn rule_engine_first_match_wins() {
        let engine = RuleEngine::new(vec![
            Rule {
                pattern: "svc=down".into(),
                candidate_id: "restart".into(),
                confidence: 1.0,
            },
            Rule {
                pattern: "svc".into(),
                candidate_id: "redeploy".into(),
                confidence: 1.0,
            },
        ]);
        let state = State {
            description: "svc=down on host-01".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        let opts = vec![
            OptionCandidate {
                id: "restart".into(),
                semantic_description: "r".into(),
                tags: vec![],
            },
            OptionCandidate {
                id: "redeploy".into(),
                semantic_description: "d".into(),
                tags: vec![],
            },
        ];
        let decision = engine
            .choose(&state, &opts)
            .unwrap()
            .expect("rule should fire");
        assert_eq!(decision.selected.as_deref(), Some("restart"));
        assert_eq!(decision.confidence, 1.0);
        assert_eq!(decision.backend, "deterministic:rules");
    }

    #[test]
    fn rule_engine_no_match_abstains() {
        let engine = RuleEngine::new(vec![]);
        let state = State {
            description: "nothing here".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        let opts = vec![OptionCandidate {
            id: "a".into(),
            semantic_description: "a".into(),
            tags: vec![],
        }];
        assert!(engine.choose(&state, &opts).unwrap().is_none());
    }

    #[test]
    fn abstain_engine_always_none() {
        let state = State {
            description: "x".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        let opts = vec![OptionCandidate {
            id: "a".into(),
            semantic_description: "a".into(),
            tags: vec![],
        }];
        assert!(AbstainEngine.choose(&state, &opts).unwrap().is_none());
        assert!(AbstainEngine
            .score(
                &state,
                &Criterion {
                    description: "c".into()
                }
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn transition_cache_engine_cached_result() {
        let oracle = NullOracle {
            cached: Some("hit".into()),
        };
        let engine = TransitionCacheEngine::new(Arc::new(oracle), 0.9);
        let state = State {
            description: "s".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        let opts = vec![
            OptionCandidate {
                id: "hit".into(),
                semantic_description: "h".into(),
                tags: vec![],
            },
            OptionCandidate {
                id: "miss".into(),
                semantic_description: "m".into(),
                tags: vec![],
            },
        ];
        let decision = engine.choose(&state, &opts).unwrap().expect("cache hit");
        assert_eq!(decision.selected.as_deref(), Some("hit"));
        assert_eq!(decision.confidence, 1.0);
    }

    struct NullOracle {
        cached: Option<String>,
    }
    impl ExperienceOracle for NullOracle {
        fn previous_transitions(&self, _s: &State) -> Vec<(String, u32)> {
            Vec::new()
        }
        fn preferences(&self, _s: &State) -> Vec<(String, u32)> {
            Vec::new()
        }
        fn cached_result(&self, _s: &State, _c: &[OptionCandidate]) -> Option<String> {
            self.cached.clone()
        }
    }
}
