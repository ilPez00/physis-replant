//! v0.3 features: near-match review, compensation, explicit undo, and MCP.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const SCRIPT: &str = r#"import argparse,json,pathlib,sys
p=argparse.ArgumentParser(); p.add_argument("mode",choices=("increment","verify","decrement")); p.add_argument("root",type=pathlib.Path); a=p.parse_args()
v=json.load(sys.stdin); c=a.root/"counter"; prev=a.root/"previous"
if a.mode=="increment":
    assert v=={"amount":1}; b=int(c.read_text()) if c.exists() else 0; prev.write_text(str(b)); c.write_text(str(b+1)); print(json.dumps({"ok":True,"count":b+1}))
elif a.mode=="decrement":
    assert v=={}; b=int(c.read_text()) if c.exists() else 0; c.write_text(str(max(0,b-1))); print(json.dumps({"ok":True,"count":max(0,b-1)}))
else:
    assert v=={}; n=int(c.read_text()) if c.exists() else None; b=int(prev.read_text()) if prev.exists() else None
    ok=n is not None and b is not None and n==b+1 and not (a.root/"fail").exists()
    (a.root/"checks.jsonl").open("a").write(json.dumps({"ok":ok,"count":n})+"\n"); print(json.dumps({"ok":ok,"count":n}))
"#;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("tools.py");
        fs::write(&script, SCRIPT).unwrap();
        let tool = |kind: &str, mode: &str| {
            json!({"kind":kind,"program":"/usr/bin/python3","args":[script,mode,dir.path()],
                   "fingerprint_files":[script],"timeout_ms":2000})
        };
        let tools = json!({
            "increment": tool("action","increment"),
            "verify": tool("check","verify"),
            "decrement": tool("undo","decrement")
        });
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
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            panic!(
                "expected JSON: stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out, value)
    }

    fn counter(&self) -> u32 {
        fs::read_to_string(self.dir.path().join("counter"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}

fn request(context: &str, prompt: &str) -> Value {
    json!({"prompt":prompt,"input":{"amount":1},"context":{"workflow":context}})
}

fn record(context: &str, prompt: &str, undo: bool) -> Value {
    let mut recipe = json!({
        "steps":[{"tool":"increment","input":{"amount":1}}],
        "checks":[{"tool":"verify","input":{}}]
    });
    if undo {
        recipe["undo"] = json!([{"tool":"decrement","input":{}}]);
    }
    json!({"request":request(context,prompt),"recipe":recipe,"trace":{"provider":"v03-test"}})
}

#[test]
fn explicit_undo_does_not_require_removed_action_or_check_tools() {
    let f = Fixture::new();
    let (out, learned) = f.call(
        "record",
        &record("undo-retired-tools", "increment once", true),
    );
    assert!(out.status.success(), "{learned}");
    assert_eq!(f.counter(), 1);
    let tools_path = f.dir.path().join("tools.json");
    let mut tools: Value = serde_json::from_slice(&fs::read(&tools_path).unwrap()).unwrap();
    tools.as_object_mut().unwrap().remove("increment");
    tools.as_object_mut().unwrap().remove("verify");
    fs::write(&tools_path, serde_json::to_vec(&tools).unwrap()).unwrap();
    let (out, undone) = f.call("undo", &json!({"id":learned["id"]}));
    assert!(
        out.status.success(),
        "undo only needs its currently authorized tools: {undone}"
    );
    assert_eq!(f.counter(), 0);
    assert_eq!(undone["calls"].as_array().unwrap().len(), 1);
    assert_eq!(undone["calls"][0]["kind"], "undo");
}

#[test]
fn explicit_undo_preflights_every_undo_permission_before_recovery_effects() {
    let f = Fixture::new();
    let tools_path = f.dir.path().join("tools.json");
    let mut tools: Value = serde_json::from_slice(&fs::read(&tools_path).unwrap()).unwrap();
    tools["second_undo"] = tools["decrement"].clone();
    fs::write(&tools_path, serde_json::to_vec(&tools).unwrap()).unwrap();
    let mut input = record("undo-revoked-role", "increment once", true);
    input["recipe"]["undo"] = json!([
        {"tool":"decrement","input":{}},
        {"tool":"second_undo","input":{}}
    ]);
    let (out, learned) = f.call("record", &input);
    assert!(out.status.success(), "{learned}");
    assert_eq!(f.counter(), 1);
    tools["decrement"]["kind"] = json!("action");
    tools.as_object_mut().unwrap().remove("increment");
    tools.as_object_mut().unwrap().remove("verify");
    fs::write(&tools_path, serde_json::to_vec(&tools).unwrap()).unwrap();
    let (out, refused) = f.call("undo", &json!({"id":learned["id"]}));
    assert!(!out.status.success());
    assert!(refused["reason"]
        .as_str()
        .unwrap()
        .contains("not registered as Undo"));
    assert_eq!(
        f.counter(),
        1,
        "the authorized reverse-first undo must not run before later authorization is checked"
    );
}

#[test]
fn near_match_is_reviewed_and_only_explicit_approval_executes() {
    let f = Fixture::new();
    let (out, learned) = f.call("record", &record("review-v1", "increment once", false));
    assert!(out.status.success(), "{learned}");
    assert_eq!(f.counter(), 1);
    let candidate = learned["id"].as_str().unwrap().to_string();

    let near = request("review-v1", "increment once ");
    let (out, review) = f.call("run", &near);
    assert!(out.status.success(), "{review}");
    assert_eq!(review["status"], "review");
    assert_eq!(review["candidates"][0]["id"], candidate);
    assert_eq!(f.counter(), 1, "review must not execute");

    let (out, approved) = f.call("approve", &json!({"request":near,"candidate":candidate}));
    assert!(out.status.success(), "{approved}");
    assert_eq!(approved["operation"], "approve");
    assert_eq!(f.counter(), 2);
    let (_, replay) = f.call("run", &near);
    assert_eq!(replay["operation"], "replay");
    assert_eq!(f.counter(), 3);
}

#[test]
fn rejected_candidate_is_not_proposed_again() {
    let f = Fixture::new();
    let (_, learned) = f.call("record", &record("reject-v1", "increment once", false));
    let candidate = learned["id"].as_str().unwrap().to_string();
    let near = request("reject-v1", "increment once ");
    let (_, review) = f.call("run", &near);
    assert_eq!(review["status"], "review");
    let (out, rejected) = f.call("reject", &json!({"request":near,"candidate":candidate}));
    assert!(out.status.success(), "{rejected}");
    assert_eq!(rejected["status"], "rejected");
    let (out, after) = f.call("run", &near);
    assert!(out.status.success(), "{after}");
    assert_eq!(after["status"], "miss");
    assert_eq!(f.counter(), 1);
}

#[test]
fn failed_check_compensates_and_explicit_undo_blocks_replay() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("fail"), "fail").unwrap();
    let (out, failed) = f.call("record", &record("undo-v1", "increment once", true));
    assert!(!out.status.success());
    assert_eq!(failed["status"], "failed");
    assert_eq!(f.counter(), 0, "declared undo must restore the counter");
    assert!(failed["calls"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["phase"] == "undo" && c["ok"] == true));

    fs::remove_file(f.dir.path().join("fail")).unwrap();
    let (out, learned) = f.call("record", &record("undo-v1", "increment once", true));
    assert!(out.status.success(), "{learned}");
    assert_eq!(f.counter(), 1);
    let id = learned["id"].as_str().unwrap();

    let (out, undone) = f.call("undo", &json!({"id":id}));
    assert!(out.status.success(), "{undone}");
    assert_eq!(undone["operation"], "undo");
    assert_eq!(f.counter(), 0);
    let (out, blocked) = f.call("run", &request("undo-v1", "increment once"));
    assert!(!out.status.success());
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(f.counter(), 0);
}

#[test]
fn mcp_stdio_lists_tools_and_routes_a_run() {
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("mcp")
        .arg("--store")
        .arg(f.dir.path().join("state"))
        .arg("--tools")
        .arg(f.dir.path().join("tools.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut exchange = |message: Value| {
        writeln!(stdin, "{}", serde_json::to_string(&message).unwrap()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let init = exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "physis-replant");
    let list = exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 8);
    let call = exchange(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"replant_run","arguments":{"request":request("mcp-v1","increment once")}}}));
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let receipt: Value = serde_json::from_str(text).unwrap();
    assert_eq!(receipt["status"], "miss");
    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn unsafe_candidates_cannot_be_proposed_or_approved() {
    for mode in [
        "failed",
        "unindexed",
        "drift",
        "engine",
        "undone",
        "superseded",
        "failed-replay",
        "interrupted",
        "tampered",
        "new-unindexed",
    ] {
        let f = Fixture::new();
        if mode == "failed" {
            fs::write(f.dir.path().join("fail"), "fail").unwrap();
        }
        let (_, learned) = f.call("record", &record("candidate-v1", "increment once", true));
        assert_eq!(
            learned["status"],
            if mode == "failed" {
                "failed"
            } else {
                "succeeded"
            }
        );
        let id = learned["id"].as_str().unwrap();
        let state = f.dir.path().join("state");
        match mode {
            "failed" => fs::remove_file(f.dir.path().join("fail")).unwrap(),
            "unindexed" => fs::remove_file(
                state
                    .join("entries")
                    .join(format!("{}.json", learned["key"].as_str().unwrap())),
            )
            .unwrap(),
            "drift" => {
                let script = f.dir.path().join("tools.py");
                fs::write(
                    &script,
                    format!("{}\n# changed", fs::read_to_string(&script).unwrap()),
                )
                .unwrap();
            }
            "engine" | "tampered" => {
                let path = state.join("records").join(format!("{id}.json"));
                let mut stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                if mode == "engine" {
                    stored["engine_version"] = json!("old");
                } else {
                    stored["recipe"]["steps"][0]["input"] = json!({"amount":2});
                }
                fs::write(path, serde_json::to_vec(&stored).unwrap()).unwrap();
            }
            "undone" => assert_eq!(f.call("undo", &json!({"id":id})).1["status"], "succeeded"),
            "superseded" => assert_eq!(
                f.call("record", &record("candidate-v1", "increment once", true))
                    .1["status"],
                "succeeded"
            ),
            "new-unindexed" => {
                assert_eq!(
                    f.call("record", &record("candidate-v1", "increment once", true))
                        .1["status"],
                    "succeeded"
                );
                let path = state
                    .join("entries")
                    .join(format!("{}.json", learned["key"].as_str().unwrap()));
                fs::write(path, serde_json::to_vec(id).unwrap()).unwrap();
            }
            "failed-replay" => {
                fs::write(f.dir.path().join("fail"), "fail").unwrap();
                assert_eq!(
                    f.call("run", &request("candidate-v1", "increment once")).1["status"],
                    "failed"
                );
                fs::remove_file(f.dir.path().join("fail")).unwrap();
            }
            "interrupted" => {
                let path = state.join("journal.jsonl");
                let journal = fs::read_to_string(&path).unwrap();
                let rows: Vec<_> = journal.lines().collect();
                fs::write(path, format!("{}\n", rows[..rows.len() - 1].join("\n"))).unwrap();
            }
            _ => unreachable!(),
        }
        let before = f.counter();
        let near = request("candidate-v1", "increment once ");
        let (_, review) = f.call("run", &near);
        assert!(
            review
                .get("candidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .all(|c| c["id"] != id),
            "{mode}: {review}"
        );
        let (out, approved) = f.call("approve", &json!({"request":near,"candidate":id}));
        assert!(!out.status.success(), "{mode}: {approved}");
        assert_eq!(
            f.counter(),
            before,
            "{mode}: unsafe approval executed an action"
        );
    }
}
