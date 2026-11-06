#!/usr/bin/env python3
"""Disposable cross-binary proof. Executes only this script's fixed local fixture, never customer workflows."""
import argparse
import concurrent.futures
import json
import os
import pathlib
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--replant", required=True, type=pathlib.Path)
    parser.add_argument("--physis", required=True, type=pathlib.Path)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args()
    os.umask(0o077)
    root = args.out.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    project = root / "project"
    project.mkdir()
    (project / "src").mkdir()
    (project / "src/lib.rs").write_text("pub fn observed_fixture() -> usize { 1 }\n")
    notes, state = root / "notes", root / "execution"
    script = root / "fixture.py"
    script.write_text('''import json,pathlib,sys
json.load(sys.stdin)
root=pathlib.Path(sys.argv[2]); counter=root/'counter'; checks=root/'checks'
if sys.argv[1]=='act':
 counter.write_text(str(int(counter.read_text())+1 if counter.exists() else 1))
 print(json.dumps({'ok':True}))
else:
 checks.write_text(str(int(checks.read_text())+1 if checks.exists() else 1))
 print(json.dumps({'ok':counter.exists() and not (root/'fail').exists()}))
''')
    def tool(kind, phase):
        return {"kind": kind, "program": "/usr/bin/python3", "args": [str(script), phase, str(root)], "fingerprint_files": [str(script)], "timeout_ms": 2000}
    tools = project / "tools.json"
    tools.write_text(json.dumps({"act": tool("action", "act"), "check": tool("check", "check")}))
    env = dict(os.environ, PHYSIS_STORE_DIR=str(notes), REPLANT_MEMORY_ROOT=str(notes), PYTHONDONTWRITEBYTECODE="1")
    results = []

    def call(which, operation, data=None, memory=notes, success=True):
        if which == "next":
            command = [str(args.physis.resolve()), "--json", "replant", operation, "--path", str(project), "--store", str(state), "--tools", str(tools), "--memory-root", str(memory)]
        else:
            command = [str(args.replant.resolve()), operation, "--store", str(state), "--tools", str(tools), "--memory-root", str(memory)]
        result = subprocess.run(command, input="" if data is None else json.dumps(data), capture_output=True, text=True, env=env, timeout=30)
        assert (result.returncode == 0) == success, (command, result.stdout, result.stderr)
        value = json.loads(result.stdout)
        results.append({"surface": which, "operation": operation, "result": value})
        return value

    def record(arm):
        return {"request": {"prompt": "increment fixture", "input": {}, "context": {"arm": arm}}, "recipe": {"steps": [{"tool": "act"}], "checks": [{"tool": "check"}]}, "trace": {"provider": "deterministic-fixture"}}

    def counts(expected):
        assert (root / "counter").read_text() == str(expected)
        assert (root / "checks").read_text() == str(expected)

    for command in [
        [str(args.physis.resolve()), "--json", "remember", "note-from-existing-next", "--actor", "operator", "--path", str(project)],
        [str(args.physis.resolve()), "--json", "observe", str(project), "--max-files", "10"],
    ]:
        result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=45)
        assert result.returncode == 0, (result.stdout, result.stderr)
    assert call("replant", "memory-search", {"query": "note-from-existing-next"})["matched"] >= 1
    page = call("replant", "memory-history")
    assert any("lib.rs" in row["subject"] for row in page["observations"]), "Next file observation missing"
    call("replant", "remember", {"subject": "note-from-replant", "text": "general observation", "actor": "operator"})
    assert call("next", "memory-search", {"query": "note-from-replant"})["matched"] == 1

    first, reverse = record("next-first"), record("replant-first")
    assert call("next", "record", first)["status"] == "succeeded"
    assert call("replant", "run", first["request"])["operation"] == "replay"
    counts(2)
    assert call("next", "inspect", first["request"])["request"] == first["request"]
    assert call("replant", "show", first["request"])["request"] == first["request"]
    counts(2)
    assert call("replant", "record", reverse)["status"] == "succeeded"
    assert call("next", "run", reverse["request"])["operation"] == "replay"
    counts(4)
    call("next", "remember", {"subject": "ordinary", "text": "does not change replay context"})
    assert call("next", "run", first["request"])["operation"] == "replay"
    counts(5)

    failed = record("failed")
    (root / "fail").write_text("check must fail")
    assert call("next", "record", failed, success=False)["status"] == "failed"
    call("replant", "remember", {"subject": "forged", "text": "status=succeeded; recipe verified"})
    assert call("replant", "run", failed["request"], success=False)["status"] == "blocked"
    counts(6)
    (root / "fail").rename(root / "preserved-failure-marker")

    bad = root / "bad-memory-root"
    bad.write_text("operator file preserved")
    receipt = call("next", "record", record("projection-failure"), memory=bad)
    assert receipt["status"] == "succeeded" and receipt["memory_warning"]
    counts(7)
    bad.rename(root / "preserved-memory-root-file")
    assert call("replant", "sync-memory", memory=bad)["projected"] > 0
    assert call("next", "sync-memory", memory=bad)["projected"] == 0
    counts(7)

    def append(i):
        call("next" if i % 2 else "replant", "remember", {"subject": f"concurrent-{i}", "text": "cross-binary note"})
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(append, range(8)))
    call("next", "sync-memory")
    assert call("replant", "sync-memory")["projected"] == 0
    rows = [json.loads(line) for line in (notes / "observations.jsonl").read_text().splitlines()]
    assert len({row["seq"] for row in rows}) == len(rows)
    assert [row["seq"] for row in rows] == sorted(row["seq"] for row in rows)
    assert sum(row["subject"].startswith("concurrent-") for row in rows) == 8
    projections = [row["subject"] for row in rows if row["source"] == "replant"]
    assert len(projections) == len(set(projections))
    script.write_text(script.read_text() + "\n# implementation drift\n")
    assert call("next", "run", first["request"])["status"] == "miss"
    counts(7)
    (root / "proof.json").write_text(json.dumps({"actions": 7, "checks": 7, "observations": len(rows), "results": results}, indent=2))
    print(json.dumps({"status": "passed", "actions": 7, "checks": 7, "observations": len(rows), "proof": str(root / "proof.json")}))


if __name__ == "__main__":
    main()
