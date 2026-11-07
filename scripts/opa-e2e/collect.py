#!/usr/bin/env python3
"""Layer 2 (Task 21): turn trino-batch.sh output into records, and check expectations.

    python3 -I collect.py records OUT_FILE META_TSV SCENARIO   # JSON lines on stdout
    python3 -I collect.py expect  OUT_FILE EXPECT_TSV SCENARIO ENGINE  # exit 1 on any miss

`records`: one record per job but the positive controls (see compare.py for the shape). `meta.tsv` maps `user/via TAB sid`
to the statement as the report shows it. A job whose responses are missing or not JSON, or
whose error is the harness's own (`errorName` HARNESS), is a record with `"harness": true`.

`expect`: EXPECT_TSV lines are `label TAB sid TAB user TAB expected`, where expected is
`ok`, `denied` (errorName PERMISSION_DENIED) or the exact JSON rows. Every expectation is
checked for ENGINE (opa | ranger: the lines whose label names it) in that side's OUT_FILE; any miss, and any harness error, exits 1 and says which.
"""
from __future__ import annotations

import json
import sys


def parse(path: str) -> list[dict]:
    jobs, cur = [], None
    for line in open(path):
        line = line.rstrip("\n")
        if line.startswith("@@BEGIN "):
            label, sid = line.split(" ")[1:3]
            cur = {"label": label, "sid": sid, "pages": []}
        elif line.startswith("@@END "):
            if cur is None:
                raise SystemExit(f"{path}: @@END without @@BEGIN: {line}")
            jobs.append(result(cur))
            cur = None
        elif cur is not None and line:
            cur["pages"].append(line)
    if cur is not None:
        jobs.append({**result(cur), "harness": True, "ok": False,
                     "error": "layer2 harness: the batch ended inside this job", "error_name": "HARNESS"})
    return jobs


def result(cur: dict) -> dict:
    cols, rows, err, name, harness = [], [], None, None, False
    for page in cur["pages"]:
        try:
            d = json.loads(page)
        except ValueError:
            err, name, harness = f"layer2 harness: not JSON: {page[:200]}", "HARNESS", True
            continue
        if d.get("columns"):
            cols = [c["name"] for c in d["columns"]]
        rows += d.get("data") or []
        if d.get("error"):
            e = d["error"]
            name = e.get("errorName") or "UNKNOWN"
            err = e.get("message", "")
            harness = harness or name == "HARNESS"
    if not cur["pages"]:
        err, name, harness = "layer2 harness: no response", "HARNESS", True
    return {"label": cur["label"], "sid": cur["sid"], "ok": err is None,
            "columns": cols if err is None else [], "rows": rows if err is None else [],
            "error": err, "error_name": name if err is not None else None, "harness": harness}


def records(out_file: str, meta: str, scenario: str) -> int:
    shown = {}
    for line in open(meta):
        uv, sid, stmt = line.rstrip("\n").split("\t", 2)
        shown[(uv, sid)] = stmt
    for j in parse(out_file):
        engine, user, via = j["label"].split("/")
        if via == "expect":  # positive controls are checked by `expect`, never compared
            continue
        rec = {"scenario": scenario, "user": user, "sid": j["sid"], "via": via, "engine": engine,
               "statement": shown[(f"{user}/{via}", j["sid"])],
               **{k: j[k] for k in ("ok", "columns", "rows", "error", "error_name", "harness")}}
        print(json.dumps(rec))
    return 0


def expect(out_file: str, expect_tsv: str, scenario: str, engine: str) -> int:
    want = {}
    for line in open(expect_tsv):
        label, sid, user, expected = line.rstrip("\n").split("\t", 3)
        if label.split("/")[0] == engine:
            want[(label, sid)] = (user, expected)
    if not want:
        print(f"layer2: {scenario}: no control for {engine}", file=sys.stderr)
        return 1
    got = {(j["label"], j["sid"]): j for j in parse(out_file) if (j["label"], j["sid"]) in want}
    misses = []
    for key, (user, expected) in sorted(want.items()):
        j = got.get(key)
        if j is None:
            misses.append(f"{key}: no result")
            continue
        if j["harness"]:
            misses.append(f"{key} ({user}): harness error: {j['error']}")
        elif expected == "ok":
            if not j["ok"]:
                misses.append(f"{key} ({user}): expected success, got {j['error_name']}: {j['error']}")
        elif expected == "denied":
            if j["ok"] or j["error_name"] != "PERMISSION_DENIED":
                misses.append(f"{key} ({user}): expected PERMISSION_DENIED, got "
                              f"{'rows ' + json.dumps(j['rows'])[:200] if j['ok'] else j['error_name'] + ': ' + j['error']}")
        else:
            rows = json.loads(expected)
            if not j["ok"]:
                misses.append(f"{key} ({user}): expected {expected}, got {j['error_name']}: {j['error']}")
            elif j["rows"] != rows:
                misses.append(f"{key} ({user}): expected {expected}, got {json.dumps(j['rows'])[:300]}")
    for m in misses:
        print(f"layer2: {scenario}: CONTROL FAILED {m}", file=sys.stderr)
    if not misses:
        print(f"layer2: {scenario}: {len(want)} control(s) hold on {engine}", file=sys.stderr)
    return 1 if misses else 0


if __name__ == "__main__":
    mode, *rest = sys.argv[1:]
    sys.exit({"records": records, "expect": expect}[mode](*rest))
