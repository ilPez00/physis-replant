//! Real CLI processes and filesystem effects; no live model or n8n runtime.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn invoke(root: &Path, verb: &str, input: &Value, tools: bool) -> (Output, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_replant"));
    command.arg(verb).arg("--store").arg(root.join("memory"));
    if tools {
        command.arg("--tools").arg(root.join("tools.json"));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if matches!(verb, "run" | "record" | "show") {
        stdin
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI must emit JSON: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, value)
}

fn fixture(root: &Path) -> Value {
    let script = root.join("tools.py");
    fs::write(&script, include_str!("../examples/fixture_tools.py")).unwrap();
    let tool = |kind: &str, mode: &str| {
        json!({
            "kind":kind, "program":"/usr/bin/python3",
            "args":[script,mode,root], "fingerprint_files":[script], "timeout_ms":2000
        })
    };
    fs::write(
        root.join("tools.json"),
        serde_json::to_vec(&json!({
            "increment":tool("action","increment"),
            "verify":tool("check","verify")
        }))
        .unwrap(),
    )
    .unwrap();
    json!({"prompt":"increment once", "input":{"amount":1}, "context":{"tenant":"fixture","workflow":"v1"}})
}

fn local_planner(request: &Value) -> Value {
    json!({
        "request":request,
        "recipe":{"steps":[{"tool":"increment","input":{"amount":1}}],"checks":[{"tool":"verify","input":{}}]},
        "trace":{
            "state_digest":"fixture", "candidates":[["increment",1.0]],
            "decision":{"selected":"increment","probabilities":[["increment",1.0]],"confidence":1.0,
                        "backend":"fixture:local-planner-not-llm","latency_ms":0},
            "escalated":false,"escalations":[],"at":"0"
        }
    })
}

#[test]
fn process_loop_skips_planner_only_after_verified_execution() {
    // Separate stores and counters; every invocation starts a fresh binary.
    let baseline = TempDir::new().unwrap();
    let replay = TempDir::new().unwrap();
    let iterations = 5;
    for (root, use_replay) in [(baseline.path(), false), (replay.path(), true)] {
        let request = fixture(root);
        let mut planner_calls = 0;
        for expected in 1..=iterations {
            let receipt = if use_replay {
                let (output, receipt) = invoke(root, "run", &request, true);
                assert!(output.status.success(), "{receipt}");
                receipt
            } else {
                json!({"status":"miss"})
            };
            let receipt = if receipt["status"] == "miss" {
                planner_calls += 1;
                let (output, receipt) = invoke(root, "record", &local_planner(&request), true);
                assert!(output.status.success(), "{receipt}");
                assert_eq!(receipt["operation"], "record");
                receipt
            } else {
                assert_eq!(receipt["operation"], "replay");
                receipt
            };
            assert_eq!(receipt["status"], "succeeded");
            assert_eq!(receipt["provider_calls"], 0);
            let count: u32 = fs::read_to_string(root.join("counter"))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(count, expected, "each invocation must change actual state");
            let checks = fs::read_to_string(root.join("checks.jsonl")).unwrap();
            let checks: Vec<Value> = checks
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(checks.len(), expected as usize);
            assert_eq!(checks.last().unwrap()["ok"], true);
            assert_eq!(checks.last().unwrap()["count"], expected);
        }
        assert_eq!(planner_calls, if use_replay { 1 } else { iterations });
        // Read-only commands must not require executable tools.
        let (output, history) = invoke(root, "history", &json!({}), false);
        assert!(output.status.success(), "{history}");
        assert!(history.is_array());
        let (output, stats) = invoke(root, "stats", &json!({}), false);
        assert!(output.status.success(), "{stats}");
        assert_eq!(stats["verified_records"], planner_calls);
        let (output, learned) = invoke(root, "show", &request, false);
        assert!(output.status.success(), "{learned}");
        assert_eq!(learned["request"], request);
    }
}

#[test]
fn cli_rejects_ambiguous_arguments_and_invalid_stdin_as_json() {
    let root = TempDir::new().unwrap();
    for args in [
        vec![],
        vec!["run"],
        vec!["run", "--tools"],
        vec!["history", "extra"],
        vec!["history", "--unknown"],
        vec!["history", "--store", "one", "--store", "two"],
        vec!["--help", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_replant"))
            .args(&args)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(!output.status.success(), "must reject {args:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["status"], "failed");
    }
    fixture(root.path());
    for input in [
        b"{\"prompt\":\"x\"} {\"prompt\":\"y\"}".to_vec(),
        vec![b' '; 1024 * 1024 + 1],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
            .arg("run")
            .arg("--store")
            .arg(root.path().join("memory"))
            .arg("--tools")
            .arg(root.path().join("tools.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["status"], "failed");
    }
    assert!(!root.path().join("counter").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("stats")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stats: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stats["verified_records"], 0);
    assert!(root.path().join(".replant").is_dir());
}
