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
