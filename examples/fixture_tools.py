"""Trusted counter tools for local examples/tests. Not a model or production tool."""
import argparse
import json
import pathlib
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("increment", "verify", "decrement"))
    parser.add_argument("root", type=pathlib.Path)
    args = parser.parse_args()
    value = json.load(sys.stdin)
    counter = args.root / "counter"
    previous = args.root / "previous"
    if args.mode == "increment":
        if value != {"amount": 1}:
            raise ValueError("fixture accepts only amount=1")
        before = int(counter.read_text()) if counter.exists() else 0
        previous.write_text(str(before))
        counter.write_text(str(before + 1))
        result = {"ok": True, "count": before + 1}
    elif args.mode == "decrement":
        if value != {}:
            raise ValueError("undo input must be an empty object")
        count = int(counter.read_text()) if counter.exists() else 0
        counter.write_text(str(max(0, count - 1)))
        result = {"ok": True, "count": max(0, count - 1)}
    else:
        if value != {}:
            raise ValueError("check input must be an empty object")
        count = int(counter.read_text()) if counter.exists() else None
        before = int(previous.read_text()) if previous.exists() else None
        result = {
            "ok": count is not None and before is not None
            and count == before + 1 and not (args.root / "fail").exists(),
            "count": count,
        }
        with (args.root / "checks.jsonl").open("a") as stream:
            stream.write(json.dumps(result) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
