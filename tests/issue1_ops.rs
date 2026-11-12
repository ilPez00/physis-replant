//! Issue #1 process-boundary and workflow regression tests.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
}
impl Fixture {
    fn new(action: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("tools.py");
        fs::write(
            &script,
            format!("import json,sys,time,os,signal,pathlib\nv=json.load(sys.stdin)\n{action}\n"),
        )
        .unwrap();
        let tool = |kind: &str, args: Value| json!({"kind":kind,"program":"/usr/bin/python3","args":args,"fingerprint_files":[script],"timeout_ms":1000});
        let tools = json!({"act":tool("action",json!([script])),"check":tool("check",json!(["-c","import json,sys;json.load(sys.stdin);print('{\"ok\":true}')"]))});
        fs::write(
            dir.path().join("tools.json"),
            serde_json::to_vec(&tools).unwrap(),
        )
        .unwrap();
        Self { dir }
    }
    fn call(&self, verb: &str, input: &Value) -> (Output, Value) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
            .args([verb, "--store"])
            .arg(self.dir.path().join("state"))
            .arg("--tools")
            .arg(self.dir.path().join("tools.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
        (out, value)
    }
    fn request(&self) -> Value {
        json!({"prompt":"execute exact task","input":{"days":7},"context":{"scope":"test"}})
    }
    fn record(&self) -> Value {
        json!({"request":self.request(),"recipe":{"steps":[{"tool":"act","input":{}}],"checks":[{"tool":"check","input":{}}]},"trace":{"source":"test"}})
    }
    fn tools(&self) -> Value {
        serde_json::from_slice(&fs::read(self.dir.path().join("tools.json")).unwrap()).unwrap()
    }
    fn set_tools(&self, tools: &Value) {
        fs::write(
            self.dir.path().join("tools.json"),
            serde_json::to_vec(tools).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn stale_lookup_and_explicit_rerecord_preserve_exact_template() {
    let f = Fixture::new("print(json.dumps({'ok':True,'input':v}))");
    let (_, original) = f.call("record", &f.record());
    let script = f.dir.path().join("tools.py");
    fs::write(
        &script,
        format!("{}\n# drift", fs::read_to_string(&script).unwrap()),
    )
    .unwrap();
    let (out, stale) = f.call("run", &f.request());
    assert!(out.status.success(), "{stale}");
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["stale_record"]["id"], original["id"]);
    let (_, stored) = f.call("show", &f.request());
    assert_eq!(stale["stale_record"]["recipe"], stored["recipe"]);
    assert_eq!(stale["calls"], json!([]));
    let (out, r) = f.call("rerecord", &json!({"id":original["id"]}));
    assert!(out.status.success(), "{r}");
    assert_eq!(r["operation"], "rerecord");
    assert_eq!(r["status"], "succeeded");
    assert_ne!(r["id"], original["id"]);
    let (_, replay) = f.call("run", &f.request());
    assert_eq!(replay["operation"], "replay");
    let (_, stats) = f.call("stats", &json!({}));
    assert_eq!(stats["stale"], 1);
    assert_eq!(stats["verified_records"], 2);
    assert!(!f
        .call("rerecord", &json!({"id":original["id"]}))
        .0
        .status
        .success());
}

#[test]
fn stale_and_rerecord_refuse_unsafe_sources() {
    for mode in [
        "missing-index",
        "unverified",
        "interrupted",
        "tampered",
        "request",
        "schema",
        "engine",
        "strategy",
        "undone",
        "failed-replay",
        "new-unindexed",
    ] {
        let f = Fixture::new("print('{\"ok\":true}')");
        let (_, r) = f.call("record", &f.record());
        let id = r["id"].as_str().unwrap();
        let state = f.dir.path().join("state");
        let journal = state.join("journal.jsonl");
        let stored = state.join("records").join(format!("{id}.json"));
        match mode {
            "missing-index" => fs::remove_file(
                state
                    .join("entries")
                    .join(format!("{}.json", r["key"].as_str().unwrap())),
            )
            .unwrap(),
            "unverified" => fs::write(&journal, "").unwrap(),
            "interrupted" => {
                let s = fs::read_to_string(&journal).unwrap();
                fs::write(&journal, format!("{}\n", s.lines().next().unwrap())).unwrap();
            }
            "tampered" | "request" | "schema" | "engine" | "strategy" => {
                let mut s: Value = serde_json::from_slice(&fs::read(&stored).unwrap()).unwrap();
                match mode {
                    "tampered" => s["recipe"]["steps"][0]["input"] = json!({"changed":true}),
                    "request" => s["request"]["input"] = json!({}),
                    "schema" => s["schema"] = json!(2),
                    "engine" => s["engine_version"] = json!("forged"),
                    _ => s["strategy"] = json!("forged"),
                };
                fs::write(stored, serde_json::to_vec(&s).unwrap()).unwrap();
            }
            "undone" | "failed-replay" => {
                let mut s = r.clone();
                s["id"] = json!("a".repeat(64));
                s["operation"] = json!(if mode == "undone" { "undo" } else { "replay" });
                s["status"] = json!("started");
                let mut rows = fs::read_to_string(&journal).unwrap();
                rows.push_str(&format!("{s}\n"));
                s["status"] = json!(if mode == "undone" {
                    "succeeded"
                } else {
                    "failed"
                });
                rows.push_str(&format!("{s}\n"));
                fs::write(&journal, rows).unwrap();
            }
            "new-unindexed" => {
                let (_, new) = f.call("record", &f.record());
                assert_ne!(new["id"], r["id"]);
                fs::write(
                    state
                        .join("entries")
                        .join(format!("{}.json", r["key"].as_str().unwrap())),
                    serde_json::to_vec(id).unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let script = f.dir.path().join("tools.py");
        fs::write(
            &script,
            format!("{}\n# drift", fs::read_to_string(&script).unwrap()),
        )
        .unwrap();
        let (_, lookup) = f.call("run", &f.request());
        assert!(lookup.get("stale_record").is_none(), "{mode}: {lookup}");
        let (out, re) = f.call("rerecord", &json!({"id":id}));
        assert!(!out.status.success(), "{mode}: {re}");
        assert!(re.get("calls").is_none(), "{mode}: {re}");
    }
}

#[test]
fn failed_rerecord_blocks_replay_and_does_not_replace_index() {
    let f = Fixture::new("print('{\"ok\":true}')");
    let (_, r) = f.call("record", &f.record());
    fs::write(
        f.dir.path().join("tools.py"),
        "import json,sys\njson.load(sys.stdin)\nprint('{\"ok\":false}')",
    )
    .unwrap();
    let (out, failed) = f.call("rerecord", &json!({"id":r["id"]}));
    assert!(!out.status.success());
    assert_eq!(failed["operation"], "rerecord");
    assert_eq!(failed["status"], "failed");
    assert_eq!(f.call("show", &f.request()).1["id"], r["id"]);
    assert_eq!(f.call("run", &f.request()).1["status"], "blocked");
}

#[test]
fn rerecord_http_and_mcp_projections_exercise_real_processes() {
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream};
    for transport in ["http", "mcp"] {
        let f = Fixture::new("print('{\"ok\":true}')");
        let (_, original) = f.call("record", &f.record());
        let script = f.dir.path().join("tools.py");
        fs::write(
            &script,
            format!("{}\n# drift", fs::read_to_string(&script).unwrap()),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_replant"));
        cmd.args([if transport == "http" { "serve" } else { "mcp" }, "--store"])
            .arg(f.dir.path().join("state"))
            .arg("--tools")
            .arg(f.dir.path().join("tools.json"));
        let address = if transport == "http" {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let a = listener.local_addr().unwrap();
            drop(listener);
            cmd.args(["--bind", &a.to_string()])
                .env("REPLANT_API_TOKEN", "issue-one-test-token");
            Some(a)
        } else {
            None
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(address) = address {
            let mut ready = false;
            for _ in 0..200 {
                if TcpStream::connect(address).is_ok() {
                    ready = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(ready);
            let http = |path: &str, body: Value, auth: bool| {
                let body = body.to_string();
                let mut s = TcpStream::connect(address).unwrap();
                s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let auth = if auth {
                    "Authorization: Bearer issue-one-test-token\r\n"
                } else {
                    ""
                };
                write!(
                    s,
                    "POST {path} HTTP/1.1\r\nHost: test\r\n{auth}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                let mut reply = String::new();
                s.read_to_string(&mut reply).unwrap();
                let (head, body) = reply.split_once("\r\n\r\n").unwrap();
                (
                    head.split_whitespace()
                        .nth(1)
                        .unwrap()
                        .parse::<u16>()
                        .unwrap(),
                    serde_json::from_str::<Value>(body).unwrap(),
                )
            };
            assert_eq!(
                http("/rerecord", json!({"id":original["id"]}), false).0,
                401
            );
            let (status, stale) = http("/run", f.request(), true);
            assert_eq!(status, 200);
            assert_eq!(stale["status"], "stale");
            let (status, r) = http("/rerecord", json!({"id":original["id"]}), true);
            assert_eq!(status, 200, "{r}");
            assert_eq!(r["operation"], "rerecord");
            assert_eq!(r["status"], "succeeded");
            child.kill().unwrap();
            child.wait().unwrap();
        } else {
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = BufReader::new(child.stdout.take().unwrap());
            let mut call = |name: &str, args: Value| {
                writeln!(stdin,"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
                stdin.flush().unwrap();
                let mut line = String::new();
                stdout.read_line(&mut line).unwrap();
                let reply: Value = serde_json::from_str(&line).unwrap();
                let result = reply["result"].clone();
                let text = result["content"][0]["text"].as_str().unwrap();
                (
                    result["isError"].clone(),
                    serde_json::from_str::<Value>(text).unwrap(),
                )
            };
            let (error, stale) = call("replant_run", json!({"request":f.request()}));
            assert_eq!(error, false);
            assert_eq!(stale["status"], "stale");
            let (error, r) = call("replant_rerecord", json!({"id":original["id"]}));
            assert_eq!(error, false, "{r}");
            assert_eq!(r["operation"], "rerecord");
            assert_eq!(r["status"], "succeeded");
            drop(stdin);
            child.wait().unwrap();
        }
    }
}

#[test]
fn explicit_role_sets_allow_only_declared_phases_and_report_actual_role() {
    let f = Fixture::new("print('{\"ok\":true}')");
    let mut tools = f.tools();
    tools["act"].as_object_mut().unwrap().remove("kind");
    tools["act"]["kinds"] = json!(["action", "undo"]);
    f.set_tools(&tools);
    let mut input = f.record();
    input["recipe"]["undo"] = json!([{"tool":"act","input":{}}]);
    input["recipe"]["preconditions"] = json!([{"tool":"check"}]);
    let (out, r) = f.call("record", &input);
    assert!(out.status.success(), "{r}");
    assert_eq!(r["calls"][0]["kind"], "check");
    assert_eq!(r["calls"][1]["kind"], "action");
    let (out, undo) = f.call("undo", &json!({"id":r["id"]}));
    assert!(out.status.success(), "{undo}");
    assert_eq!(undo["calls"][0]["kind"], "undo");
}

#[test]
fn registry_role_shapes_are_fail_closed_and_legacy_json_is_accepted() {
    for roles in [
        json!({}),
        json!({"kinds":[]}),
        json!({"kind":"action","kinds":["action"]}),
        json!({"kind":"action","kinds":[]}),
        json!({"kinds":["action","action"]}),
        json!({"kinds":["unknown"]}),
        json!({"kind":null,"kinds":["action"]}),
    ] {
        let f = Fixture::new("print('{\"ok\":true}')");
        let mut tools = f.tools();
        let spec = tools["act"].as_object_mut().unwrap();
        spec.remove("kind");
        spec.extend(roles.as_object().unwrap().clone());
        f.set_tools(&tools);
        let (out, r) = f.call("record", &f.record());
        assert!(!out.status.success(), "{roles}: {r}");
        assert!(r.get("calls").is_none());
    }
    let f = Fixture::new("print('{\"ok\":true}')");
    assert!(f.call("record", &f.record()).0.status.success());
}

#[test]
fn undeclared_late_phase_role_prevents_earlier_effects() {
    for phase in ["preconditions", "checks", "undo"] {
        let f=Fixture::new("pathlib.Path(__file__).with_suffix('.effect').write_text('effect');print('{\"ok\":true}')");
        let mut record = f.record();
        record["recipe"][phase] = json!([{"tool":"act"}]);
        let (out, r) = f.call("record", &record);
        assert!(!out.status.success(), "{phase}: {r}");
        assert!(!f.dir.path().join("tools.effect").exists());
    }
}

#[test]
fn operator_timeout_boundaries_and_default() {
    for timeout in [120001, 1800000, 0, 1800001] {
        let f = Fixture::new("print('{\"ok\":true}')");
        let mut tools = f.tools();
        tools["act"]["timeout_ms"] = json!(timeout);
        f.set_tools(&tools);
        let (out, r) = f.call("record", &f.record());
        assert_eq!(
            out.status.success(),
            (1..=1800000).contains(&timeout),
            "{timeout}: {r}"
        );
    }
    let f = Fixture::new("print('{\"ok\":true}')");
    let mut tools = f.tools();
    tools["act"].as_object_mut().unwrap().remove("timeout_ms");
    let registry: physis_replant::ToolRegistry = serde_json::from_value(tools).unwrap();
    assert_eq!(registry["act"].timeout_ms, 30000);
}

#[test]
fn diagnostics_preserve_nonzero_json_and_stderr() {
    let f = Fixture::new("print(json.dumps({'ok':False,'detail':'denied'}));sys.stderr.write('diagnostic');sys.exit(7)");
    let (out, r) = f.call("record", &f.record());
    assert!(!out.status.success(), "{r}");
    let c = &r["calls"][0];
    assert_eq!(c["exit_code"], 7, "{r}");
    assert_eq!(c["output"]["ok"], false);
    assert!(c["stdout"].as_str().unwrap().contains("denied"));
    assert_eq!(c["stderr"], "diagnostic");
}

#[test]
fn diagnostics_bound_prefix_and_tail_without_success() {
    let f = Fixture::new("sys.stdout.write('x'*70000);sys.stderr.write('a'*9000+'TAIL')");
    let (_, r) = f.call("record", &f.record());
    let c = &r["calls"][0];
    assert_eq!(r["status"], "failed");
    assert_eq!(c["stdout"].as_str().unwrap().len(), 65536);
    assert_eq!(c["stderr"].as_str().unwrap().len(), 8192);
    assert!(c["stderr"].as_str().unwrap().ends_with("TAIL"));
    assert_eq!(c["stdout_truncated"], true);
    assert_eq!(c["stderr_truncated"], true);
}

#[test]
fn diagnostics_timeout_keeps_partial_output() {
    let f = Fixture::new("print('{\"ok\":true}',flush=True);print('partial',file=sys.stderr,flush=True);time.sleep(5)");
    let mut t = f.tools();
    t["act"]["timeout_ms"] = json!(100);
    f.set_tools(&t);
    let (_, r) = f.call("record", &f.record());
    assert_eq!(r["status"], "failed");
    assert!(r["calls"][0]["stdout"].as_str().unwrap().contains("ok"));
    assert!(r["calls"][0]["stderr"]
        .as_str()
        .unwrap()
        .contains("partial"));
    assert_eq!(r["calls"][0]["output"]["ok"], true);
}

#[test]
fn diagnostics_malformed_nonobject_false_and_signal_never_succeed() {
    for action in [
        "print('not-json')",
        "print('[]')",
        "print('{\"ok\":false}')",
        "os.kill(os.getpid(),signal.SIGTERM)",
    ] {
        let f = Fixture::new(action);
        let (_, r) = f.call("record", &f.record());
        assert_eq!(r["status"], "failed", "{action}: {r}");
        assert!(r["calls"][0]["error"].is_string());
    }
}
