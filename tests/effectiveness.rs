use physis_replant::{compile_trace, Quarantine, Decision, DecisionTrace};

#[test]
fn n8n_loop_effectiveness() {
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
        escalations: vec![],
        at: "0".into(),
    };

    let mut q = Quarantine::auto();
    let compiled = compile_trace(&trace, 0.9).expect("compilable");
    let original = trace.decision.selected.clone().unwrap();
    q.store(compiled);

    let pattern = selected.split_whitespace().last().unwrap().to_string();

    let mut free_count = 0usize;
    let mut model_count = 0usize;
    let mut correctness = 0usize;
    let mut promotion_at: Option<usize> = None;

    for i in 0..k {
        let promoted = q.promotable(min_rate, min_replays);
        if !promoted.is_empty() {
            free_count += 1;
            if promoted[0].rule.candidate_id == original {
                correctness += 1;
            }
            if promotion_at.is_none() {
                promotion_at = Some(i);
            }
        } else {
            model_count += 1;
            q.record_replay(&pattern, &selected, true, model_latency_ms);
        }
    }

    println!("n8n loop: {} iters, {} T2, {} T0a, promotion@{:?}, correctness {}/{}",
        k, model_count, free_count, promotion_at, correctness, free_count);

    assert!(promotion_at.is_some());
    assert!(free_count > 0);
    assert_eq!(correctness, free_count);
    assert!(free_count >= k - min_replays as usize);
}
