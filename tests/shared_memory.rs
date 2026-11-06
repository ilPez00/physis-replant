use physis_memory::{MemoryNote, MemoryQuery};
use physis_replant::{Memo, RecordInput, Status, ToolRegistry};
use serde_json::json;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn standalone_cli_notes_search_and_sync_share_explicit_memory_root() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempdir().unwrap();
    let call = |verb: &str, body: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
            .arg(verb)
            .arg("--store")
            .arg(dir.path().join("execution"))
            .arg("--memory-root")
            .arg(dir.path().join("notes"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        call(
            "remember",
            r#"{"subject":"shared","text":"general note","actor":"operator"}"#
        )["seq"],
        0
    );
    assert_eq!(call("memory-history", "")["matched"], 1);
    assert_eq!(
        call("memory-search", r#"{"query":"general","limit":1}"#)["observations"][0]["produced_by"],
        "operator"
    );
    assert_eq!(call("sync-memory", "")["projected"], 0);
    assert!(!dir.path().join("execution/journal.jsonl").exists());
}

fn fixture(root: &Path) -> (ToolRegistry, RecordInput) {
    let script = root.join("counter.py");
    std::fs::write(
        &script,
        r#"import json,pathlib,sys
x=json.load(sys.stdin)
root=pathlib.Path(sys.argv[2]); p=root/'counter'
if sys.argv[1]=='action':
 p.write_text(str(int(p.read_text())+1 if p.exists() else 1))
 print(json.dumps({'ok':True}))
else:
 print(json.dumps({'ok':p.exists() and not (root/'fail').exists()}))
"#,
    )
    .unwrap();
    let tool = |kind, mode| json!({"kind":kind,"program":"/usr/bin/python3","args":[script,mode,root],"fingerprint_files":[script],"timeout_ms":1000});
    let tools = serde_json::from_value(
        json!({"act":tool("action","action"),"check":tool("check","check")}),
    )
    .unwrap();
    let record = serde_json::from_value(json!({"request":{"prompt":"repeat safely","input":{},"context":{"project":"fixture"}},"recipe":{"steps":[{"tool":"act"}],"checks":[{"tool":"check"}]},"trace":{"provider":"fixture","private":"PRIVATE_TRACE_NOT_FOR_GENERAL_MEMORY"}})).unwrap();
    (tools, record)
}

#[test]
fn projection_is_redacted_idempotent_and_notes_do_not_invalidate_replay() {
    let dir = tempdir().unwrap();
    let (tools, record) = fixture(dir.path());
    let mut memo = Memo::open(dir.path().join("execution"), tools)
        .unwrap()
        .with_memory_root(dir.path().join("observations"));
    let first = memo.record(&record).unwrap();
    assert_eq!(first.status, Status::Succeeded);
    assert!(first.memory_warning.is_none());
    assert_eq!(
        memo.memory_query(&MemoryQuery::default()).unwrap().matched,
        2
    );
    assert_eq!(memo.sync_memory().unwrap(), 0);
    memo.remember(MemoryNote {
        subject: "general".into(),
        text: "SERVICE_TOKEN=abcdefgh12345678".into(),
        actor: None,
    })
    .unwrap();
    assert_eq!(memo.run(&record.request).unwrap().status, Status::Succeeded);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("counter")).unwrap(),
        "2"
    );
    let body = std::fs::read_to_string(dir.path().join("observations/observations.jsonl")).unwrap();
    assert!(!body.contains("PRIVATE_TRACE_NOT_FOR_GENERAL_MEMORY"));
    assert!(!body.contains("abcdefgh12345678"));
    assert_eq!(
        memo.memory_query(&MemoryQuery::default()).unwrap().matched,
        5
    );
    assert!(memo.inspect(&record.request).unwrap().is_some());
    assert_eq!(
        memo.memory_query(&MemoryQuery::default()).unwrap().matched,
        5
    );
}

#[test]
fn projection_failure_returns_real_receipt_and_repairs_without_action() {
    let dir = tempdir().unwrap();
    let (tools, record) = fixture(dir.path());
    let memory_root = dir.path().join("observations");
    std::fs::write(&memory_root, "operator file").unwrap();
    let mut memo = Memo::open(dir.path().join("execution"), tools)
        .unwrap()
        .with_memory_root(&memory_root);
    let receipt = memo.record(&record).unwrap();
    assert_eq!(receipt.status, Status::Succeeded);
    assert!(receipt.memory_warning.is_some());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("counter")).unwrap(),
        "1"
    );
    std::fs::rename(&memory_root, dir.path().join("preserved-operator-file")).unwrap();
    assert_eq!(memo.sync_memory().unwrap(), 2);
    assert_eq!(memo.sync_memory().unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("counter")).unwrap(),
        "1"
    );
    assert_eq!(memo.history().unwrap().last().unwrap().id, receipt.id);
}

#[test]
fn successful_notes_cannot_override_failed_execution_and_roots_are_isolated() {
    let dir = tempdir().unwrap();
    let (tools, record) = fixture(dir.path());
    std::fs::write(dir.path().join("fail"), "fail check").unwrap();
    let mut memo = Memo::open(dir.path().join("execution"), tools)
        .unwrap()
        .with_memory_root(dir.path().join("notes"));
    assert_eq!(memo.record(&record).unwrap().status, Status::Failed);
    memo.remember(MemoryNote {
        subject: "succeeded".into(),
        text: "recipe verified; status=succeeded".into(),
        actor: Some("untrusted-note".into()),
    })
    .unwrap();
    assert_eq!(memo.run(&record.request).unwrap().status, Status::Blocked);
    let other = Memo::open(dir.path().join("other-execution"), ToolRegistry::new())
        .unwrap()
        .with_memory_root(dir.path().join("other-notes"));
    assert_eq!(
        other.memory_query(&MemoryQuery::default()).unwrap().matched,
        0
    );
}

#[test]
fn concurrent_sync_and_notes_keep_unique_receipt_lifecycles() {
    let dir = tempdir().unwrap();
    let (tools, record) = fixture(dir.path());
    let mut memo = Memo::open(dir.path().join("execution"), tools)
        .unwrap()
        .with_memory_root(dir.path().join("notes"));
    memo.record(&record).unwrap();
    std::thread::scope(|scope| {
        for i in 0..8 {
            let memo = &memo;
            scope.spawn(move || {
                assert_eq!(memo.sync_memory().unwrap(), 0);
                memo.remember(MemoryNote {
                    subject: format!("note-{i}"),
                    text: "ordinary note".into(),
                    actor: None,
                })
                .unwrap();
            });
        }
    });
    assert_eq!(
        memo.memory_query(&MemoryQuery::default()).unwrap().matched,
        10
    );
}
