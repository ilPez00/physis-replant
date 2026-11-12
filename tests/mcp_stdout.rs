//! The executable, not only the MCP library loop, must own a clean protocol stream.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn mcp_stdout_contains_only_responses_and_no_shutdown_trailer() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools.json");
    fs::write(&tools, "{}").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("mcp")
        .arg("--tools")
        .arg(&tools)
        .arg("--store")
        .arg(dir.path().join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ] {
        writeln!(stdin, "{message}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "MCP stdout contains an extra non-response: {rows:?}"
    );
    for (row, expected_id) in rows.iter().zip([1, 2]) {
        assert_eq!(row["jsonrpc"], "2.0");
        assert_eq!(row["id"], expected_id);
        assert!(row.get("result").is_some());
    }
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 8);
}

#[test]
fn mcp_setup_failure_uses_stderr_and_does_not_pollute_protocol_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools.json");
    fs::write(&tools, "{broken").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_replant"))
        .arg("mcp")
        .arg("--tools")
        .arg(&tools)
        .arg("--store")
        .arg(dir.path().join("state"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "setup error is not a JSON-RPC frame"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid tool registry JSON"));
    assert!(!dir.path().join("state").exists());
}
