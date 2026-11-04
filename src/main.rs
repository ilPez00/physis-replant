use physis_replant::{Memo, MemoKey};

fn main() {
    let mut memo = Memo::new();
    let inputs = ["restart k3s", "restart k3s", "restart k3s", "restart k3s"];

    for (i, input) in inputs.iter().enumerate() {
        let key = MemoKey::new(input);
        let (result, report) = memo.run(&key, |k| {
            // Simulated model run (T2).
            let trace = physis_replant::DecisionTrace {
                state_digest: physis_replant::state_digest(&physis_replant::State {
                    description: k.hash.clone(),
                    refs: vec![],
                    metrics: std::collections::BTreeMap::new(),
                }),
                candidates: vec![("restart k3s".into(), 1.0)],
                decision: physis_replant::Decision {
                    selected: Some("restart k3s".into()),
                    probabilities: vec![("restart k3s".into(), 1.0)],
                    confidence: 1.0,
                    backend: "causal:smollm2-135m".into(),
                    latency_ms: 120,
                },
                escalated: false,
                escalations: vec![],
                at: "0".into(),
            };
            (
                physis_replant::MemoResult {
                    output: "restarted".into(),
                    verdict: "pass".into(),
                },
                trace,
            )
        });
        println!(
            "iter {} | tier={:?} | cached={} | saved_ms={} | result={}",
            i, report.tier, report.cached, report.saved_ms, result.output
        );
    }

    let (total, promoted, eff) = memo.stats();
    println!("stats: total={} promoted={} effectiveness={:.1}%", total, promoted, eff);
}
