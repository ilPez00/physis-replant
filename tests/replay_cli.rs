use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    tools: Value,
    request: Value,
    record: Value,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("tools.py");
        fs::write(&script, r#"import json,sys,pathlib,time,os,subprocess
x=json.load(sys.stdin)
root=pathlib.Path(sys.argv[2])
mode=sys.argv[1]
counter=root/'counter'
if mode=='wait_act':
    (root/'ready').write_text('ready')
    while not (root/'release').exists():
        time.sleep(0.005)
    mode='act'
if mode=='act':
    n=int(counter.read_text())+1 if counter.exists() else 1
    counter.write_text(str(n))
    print(json.dumps({'ok':True,'count':n}))
elif mode=='check':
    print(json.dumps({'ok':not (root/'fail').exists() and counter.exists()}))
elif mode=='sleep':
    (root/'slow-pid').write_text(str(os.getpid()))
    time.sleep(30)
elif mode=='noise':
    print('x'*100000)
elif mode=='false':
    print(json.dumps({'ok':False}))
elif mode=='exit':
    sys.exit(7)
elif mode=='detach':
    child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
    (root/'detached-pid').write_text(str(child.pid))
    print(json.dumps({'ok':True}))
"#).unwrap();
        let tool = |kind: &str, mode: &str| {
            json!({
                "kind":kind, "program":"/usr/bin/python3",
                "args":[script,mode,dir.path()],
                "fingerprint_files":[script], "timeout_ms":1000
            })
        };
        // Tool program/argv are operator-owned; model data goes only to stdin.
        let tools = json!({
            "increment":tool("action","act"),
            "verify":tool("check","check"),
            "slow":tool("action","sleep"),
            "noisy":tool("action","noise"),
            "false_check":tool("check","false"),
            "bad_exit":tool("action","exit"),
            "detached":tool("action","detach"),
            "waiter":tool("action","wait_act")
        });
        fs::write(
            dir.path().join("tools.json"),
            serde_json::to_vec(&tools).unwrap(),
        )
        .unwrap();
        let request = json!({"prompt":"increment once", "input":{"item":1}, "context":{"workflow":"v1","tenant":"test"}});
        let record = json!({
            "request":request,
            "recipe":{"steps":[{"tool":"increment","input":{}}],"checks":[{"tool":"verify","input":{}}]},
            "trace":{
                "state_digest":"fixture", "candidates":[["increment",1.0]],
                "decision":{"selected":"increment","probabilities":[["increment",1.0]],"confidence":1.0,"backend":"fixture:local-planner-not-llm","latency_ms":0},
                "escalated":false,"escalations":[],"at":"0"
            }
        });
        Self {
            dir,
            tools,
            request,
            record,
        }
    }

    fn call(&self, verb: &str, input: &Value) -> (Output, Value) {
        call(self.dir.path(), verb, input)
    }

    fn count(&self) -> u32 {
        fs::read_to_string(self.dir.path().join("counter"))
            .unwrap()
            .parse()
            .unwrap()
    }
}

fn call(root: &Path, verb: &str, input: &Value) -> (Output, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .args([verb, "--store"])
        .arg(root.join("memory"))
        .arg("--tools")
        .arg(root.join("tools.json"))
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
    let out = child.wait_with_output().unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "expected JSON, got stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}

#[test]
fn records_then_replays_real_actions_across_processes() {
    let f = Fixture::new();
    let (_, miss) = f.call("run", &f.request);
    assert_eq!(miss["status"], "miss");
    let (out, learned) = f.call("record", &f.record);
    assert!(out.status.success(), "{learned}");
    assert_eq!(learned["status"], "succeeded");
    assert_eq!(f.count(), 1);
    for expected in 2..=4 {
        let (out, replay) = f.call("run", &f.request);
        assert!(out.status.success(), "{replay}");
        assert_eq!(replay["operation"], "replay");
        assert_eq!(replay["status"], "succeeded");
        assert_eq!(replay["provider_calls"], 0);
        assert_eq!(
            f.count(),
            expected,
            "must re-execute action, not return old response"
        );
    }
    let (_, show) = f.call("show", &f.request);
    assert_eq!(show["request"], f.request);
    assert_eq!(show["trace"], f.record["trace"]);
    let (_, stats) = f.call("stats", &json!({}));
    assert_eq!(stats["verified_records"], 1);
    assert_eq!(stats["verified_replays"], 3);
    assert_eq!(stats["replay_share_percent"], 75.0);
    let (_, history) = f.call("history", &json!({}));
    assert!(history
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["operation"] == "replay" && row["engine_version"].is_string()));
}

#[test]
fn changed_input_context_or_tool_implementation_never_reuses_recipe() {
    let f = Fixture::new();
    assert!(f.call("record", &f.record).0.status.success());
    for field in ["input", "context"] {
        let mut changed = f.request.clone();
        changed[field] = json!({"changed":true});
        assert_eq!(f.call("run", &changed).1["status"], "miss");
    }
    fs::write(f.dir.path().join("tools.py"), "raise SystemExit('changed')").unwrap();
    assert_eq!(f.call("run", &f.request).1["status"], "miss");
    assert_eq!(f.count(), 1);
}

#[test]
fn failed_checks_invalidate_recipe_and_do_not_claim_success() {
    let f = Fixture::new();
    assert!(f.call("record", &f.record).0.status.success());
    fs::write(f.dir.path().join("fail"), "").unwrap();
    let (out, failed) = f.call("run", &f.request);
    assert!(!out.status.success());
    assert_eq!(failed["status"], "failed");
    assert_eq!(f.count(), 2);
    assert_eq!(f.call("run", &f.request).1["status"], "blocked");
    assert_eq!(f.count(), 2, "failed recipe must not keep firing");
}

#[test]
fn failures_and_missing_checks_cannot_be_recorded_for_replay() {
    let f = Fixture::new();
    let mut bad = f.record.clone();
    bad["recipe"]["checks"] = json!([]);
    assert!(!f.call("record", &bad).0.status.success());
    for name in ["false_check", "increment"] {
        let mut bad = f.record.clone();
        bad["recipe"]["checks"][0]["tool"] = json!(name);
        assert!(!f.call("record", &bad).0.status.success());
    }
    assert_eq!(f.call("run", &f.request).1["status"], "blocked");
}

#[test]
fn unknown_tools_are_refused_before_any_side_effect() {
    let f = Fixture::new();
    let mut bad = f.record.clone();
    bad["recipe"]["steps"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tool":"unknown","input":{}}));
    assert!(!f.call("record", &bad).0.status.success());
    assert!(!f.dir.path().join("counter").exists());
}

#[test]
fn failing_exits_timeout_and_excess_output_are_bounded_failures() {
    let f = Fixture::new();
    for tool in ["slow", "noisy", "bad_exit"] {
        let mut bad = f.record.clone();
        bad["recipe"]["steps"][0]["tool"] = json!(tool);
        let started = std::time::Instant::now();
        let (out, report) = f.call("record", &bad);
        assert!(!out.status.success(), "{report}");
        assert_eq!(report["status"], "failed");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(f.call("run", &f.request).1["status"], "blocked");
    }
}

#[test]
fn corrupt_journal_is_an_error_not_a_clean_cache() {
    let f = Fixture::new();
    assert!(f.call("record", &f.record).0.status.success());
    fs::write(f.dir.path().join("memory/journal.jsonl"), "{torn").unwrap();
    assert!(!f.call("run", &f.request).0.status.success());
    assert_eq!(f.count(), 1);
}

#[test]
fn help_and_unknown_commands_are_not_the_demo() {
    let help = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("record"));
    let invalid = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("not-a-command")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn failed_first_record_blocks_automatic_retries_without_an_index() {
    let f = Fixture::new();
    let mut bad = f.record.clone();
    bad["recipe"]["checks"][0]["tool"] = json!("false_check");
    assert!(!f.call("record", &bad).0.status.success());
    let (out, blocked) = f.call("run", &f.request);
    assert!(
        !out.status.success(),
        "must not route a partial failure to an automatic AI miss branch"
    );
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(f.count(), 1);
}

#[test]
fn registry_change_invalidates_reuse_and_wrong_task_never_borrows_promotion() {
    let f = Fixture::new();
    assert!(f.call("record", &f.record).0.status.success());
    let mut other = f.request.clone();
    other["prompt"] = json!("another operation");
    assert_eq!(f.call("run", &other).1["status"], "miss");
    let mut tools = f.tools.clone();
    tools["increment"]["timeout_ms"] = json!(900);
    fs::write(
        f.dir.path().join("tools.json"),
        serde_json::to_vec(&tools).unwrap(),
    )
    .unwrap();
    assert_eq!(f.call("run", &f.request).1["status"], "miss");
    assert_eq!(f.count(), 1);
}

#[cfg(unix)]
#[test]
fn swapping_declared_symlink_targets_invalidates_the_binding() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let selected = f.dir.path().join("selected.py");
    let unused = f.dir.path().join("unused.py");
    let a = f.dir.path().join("a.py");
    let b = f.dir.path().join("b.py");
    let original = fs::read_to_string(f.dir.path().join("tools.py")).unwrap();
    for (file, label) in [(&a, "a"), (&b, "b")] {
        fs::write(
            file,
            original.replace(
                "'count':n}",
                &format!("'count':n,'implementation':'{label}'}}"),
            ),
        )
        .unwrap();
    }
    symlink(&a, &selected).unwrap();
    symlink(&b, &unused).unwrap();
    let mut tools = f.tools.clone();
    tools["increment"]["args"][0] = json!(selected);
    tools["increment"]["fingerprint_files"] = json!([selected]);
    tools["unused"] = tools["increment"].clone();
    tools["unused"]["args"][0] = json!(unused);
    tools["unused"]["fingerprint_files"] = json!([unused]);
    fs::write(
        f.dir.path().join("tools.json"),
        serde_json::to_vec(&tools).unwrap(),
    )
    .unwrap();
    let (out, record) = f.call("record", &f.record);
    assert!(out.status.success(), "{record}");
    assert_eq!(record["calls"][0]["output"]["implementation"], "a");
    // The canonical file set stays {a,b}; the tool/path bindings change.
    fs::remove_file(&selected).unwrap();
    fs::remove_file(&unused).unwrap();
    symlink(&b, &selected).unwrap();
    symlink(&a, &unused).unwrap();
    let (_, run) = f.call("run", &f.request);
    assert_eq!(
        run["status"], "miss",
        "content hash dedup must not erase declared-path bindings"
    );
    assert_eq!(f.count(), 1);
}

#[test]
fn precondition_failure_prevents_actions() {
    let f = Fixture::new();
    let mut record = f.record.clone();
    record["recipe"]["preconditions"] = json!([{"tool":"false_check","input":{}}]);
    assert!(!f.call("record", &record).0.status.success());
    assert!(!f.dir.path().join("counter").exists());
}

#[test]
fn full_cognition_trace_is_preserved_without_dropping_extra_spans() {
    let f = Fixture::new();
    let mut record = f.record.clone();
    record["trace"]["spans"] = json!([{"tool":"increment","reason":"chosen by external planner","metadata":{"observations":[1,2,3]}}]);
    assert!(f.call("record", &record).0.status.success());
    let (_, shown) = f.call("show", &f.request);
    assert_eq!(
        shown["trace"], record["trace"],
        "unknown trace fields must not be silently discarded"
    );
}

#[test]
fn index_publication_failure_is_not_a_fresh_miss_after_effects() {
    let f = Fixture::new();
    let (_, miss) = f.call("run", &f.request);
    let index = f
        .dir
        .path()
        .join("memory/entries")
        .join(format!("{}.json", miss["key"].as_str().unwrap()));
    fs::create_dir(&index).unwrap(); // Force atomic index rename to fail after execution.
    assert!(!f.call("record", &f.record).0.status.success());
    assert_eq!(f.count(), 1);
    fs::remove_dir(index).unwrap();
    let (out, row) = f.call("run", &f.request);
    assert!(
        !out.status.success(),
        "a committed effect must not silently enter the AI miss branch"
    );
    assert_eq!(row["status"], "blocked");
    assert_eq!(f.count(), 1);
}

#[test]
fn implementation_drift_during_execution_cannot_be_verified_under_old_policy() {
    let f = Fixture::new();
    let mut record = f.record.clone();
    record["recipe"]["steps"][0]["tool"] = json!("waiter");
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("record")
        .arg("--store")
        .arg(f.dir.path().join("memory"))
        .arg("--tools")
        .arg(f.dir.path().join("tools.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    let start = std::time::Instant::now();
    while !f.dir.path().join("ready").exists()
        && start.elapsed() < std::time::Duration::from_secs(5)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(f.dir.path().join("ready").exists());
    let script = f.dir.path().join("tools.py");
    let mut body = fs::read_to_string(&script).unwrap();
    body.push_str("\n# implementation changed during the run\n");
    fs::write(script, body).unwrap();
    fs::write(f.dir.path().join("release"), "").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "changed implementation must not receive an old-policy success receipt"
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "failed");
}

#[test]
fn concurrent_processes_keep_every_effect_and_journal_entry() {
    let f = Fixture::new();
    assert!(f.call("record", &f.record).0.status.success());
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| f.call("run", &f.request)))
            .collect();
        for job in jobs {
            let (out, row) = job.join().unwrap();
            assert!(out.status.success(), "{row}");
        }
    });
    assert_eq!(f.count(), 5);
    let (_, stats) = f.call("stats", &json!({}));
    assert_eq!(stats["verified_records"], 1);
    assert_eq!(stats["verified_replays"], 4);
    assert_eq!(stats["pending"], 0);
}

#[cfg(unix)]
#[test]
fn killed_process_leaves_pending_evidence_and_blocks_replay() {
    let f = Fixture::new();
    let mut record = f.record.clone();
    record["recipe"]["steps"][0]["tool"] = json!("slow");
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("record")
        .arg("--store")
        .arg(f.dir.path().join("memory"))
        .arg("--tools")
        .arg(f.dir.path().join("tools.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    let start = std::time::Instant::now();
    let pid_file = f.dir.path().join("slow-pid");
    while !pid_file.exists() && start.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        pid_file.exists(),
        "tool must have started before crash injection"
    );
    let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    // The test owns this exact fixture process; do not leave its sleeper behind.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let (out, row) = f.call("run", &f.request);
    assert!(!out.status.success());
    assert_eq!(row["status"], "blocked");
    let (_, stats) = f.call("stats", &json!({}));
    assert_eq!(stats["pending"], 1);
}

#[cfg(unix)]
#[test]
fn detached_pipe_holder_cannot_hang_the_cli_forever() {
    let f = Fixture::new();
    let mut record = f.record.clone();
    record["recipe"]["steps"][0]["tool"] = json!("detached");
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("record")
        .arg("--store")
        .arg(f.dir.path().join("memory"))
        .arg("--tools")
        .arg(f.dir.path().join("tools.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    let start = std::time::Instant::now();
    let mut tool_started = None;
    let finished = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        // Time the pipe deadline after tool startup, not debug-build hashing
        // competing with all the other process fixtures on a loaded host.
        if f.dir.path().join("detached-pid").exists() && tool_started.is_none() {
            tool_started = Some(std::time::Instant::now());
        }
        if tool_started.is_some_and(|time: std::time::Instant| {
            time.elapsed() > std::time::Duration::from_secs(3)
        }) || start.elapsed() > std::time::Duration::from_secs(15)
        {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    if !finished {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    // Clean up the deliberately escaped fixture descendant, not arbitrary PIDs.
    if let Ok(pid) = fs::read_to_string(f.dir.path().join("detached-pid")) {
        unsafe {
            libc::kill(pid.parse().unwrap(), libc::SIGKILL);
        }
    }
    assert!(finished, "pipe readers must share the tool's deadline");
    assert!(!output.status.success());
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["status"], "failed");
}
