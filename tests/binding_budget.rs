//! Reject amplification during binding preflight, not after allocating the result.
#![cfg(unix)]
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

#[test]
fn oversized_selected_value_is_rejected_before_cloning() {
    assert_bounded_refusal(500_000, 3000, false);
}

#[test]
fn repeated_small_bindings_share_one_call_budget() {
    assert_bounded_refusal(16_000, 3000, false);
}

#[test]
fn excessive_call_count_is_rejected_before_binding_expansion() {
    assert_bounded_refusal(16_000, 6000, true);
}

fn assert_bounded_refusal(blob_len: usize, repetitions: usize, too_many_calls: bool) {
    // Each source request fits 1 MiB and each template fits 64 KiB. RLIMIT_AS
    // contains the old resolver's large allocations; no host OOM or core dump.
    let dir = tempfile::tempdir().unwrap();
    let effect = dir.path().join("must-not-execute");
    let tools_path = dir.path().join("tools.json");
    let action = json!({
        "kind":"action", "program":"/usr/bin/python3",
        "args":["-c", "import json,pathlib,sys;json.load(sys.stdin);pathlib.Path(sys.argv[1]).write_text('effect');print('{\"ok\":true}')", effect]
    });
    let mut check = action.clone();
    check["kind"] = json!("check");
    fs::write(
        &tools_path,
        serde_json::to_vec(&json!({"act":action,"check":check})).unwrap(),
    )
    .unwrap();
    let template = if too_many_calls {
        json!({"$input":"blob"})
    } else {
        Value::Array(vec![json!({"$input":"blob"}); repetitions])
    };
    assert!(serde_json::to_vec(&template).unwrap().len() <= 65_536);
    let step = json!({"tool":"act","input":template});
    let steps = vec![step; if too_many_calls { repetitions } else { 1 }];
    let record = json!({
        "request":{"prompt":"bounded binding amplification","input":{"blob":"x".repeat(blob_len)},"context":{}},
        "recipe":{"bind_inputs":true,"steps":steps,"checks":[{"tool":"check","input":{}}]},
        "trace":{"fixture":"binding-allocation-budget"}
    });
    let bytes = serde_json::to_vec(&record).unwrap();
    assert!(bytes.len() <= 1_048_576);
    let mut command = Command::new(env!("CARGO_BIN_EXE_replant"));
    command
        .arg("record")
        .arg("--tools")
        .arg(&tools_path)
        .arg("--store")
        .arg(dir.path().join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only async-signal-safe libc calls run between fork and exec. The limit
    // affects only Replant, not the test runner or concurrent system processes.
    unsafe {
        command.pre_exec(|| {
            let memory = libc::rlimit {
                rlim_cur: 64 * 1024 * 1024,
                rlim_max: 64 * 1024 * 1024,
            };
            let core = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                || libc::setrlimit(libc::RLIMIT_CORE, &core) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "must return a bounded refusal, not allocator abort: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let refused: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(refused["status"], "failed");
    let expected = if too_many_calls { "32 calls" } else { "64 KiB" };
    assert!(
        refused["reason"].as_str().unwrap().contains(expected),
        "{refused}"
    );
    assert!(!effect.exists());
    assert!(!dir.path().join("state/journal.jsonl").exists());
}
