#!/usr/bin/env python3
"""Layer 2 of the OPA target (governance SP2b, Task 21): compare trino-opa with trino-ranger.

Run with `python3 -I scripts/opa-e2e/compare.py RESULTS.jsonl [--explained FILE] [--report FILE]`.

RESULTS.jsonl is what scripts/opa-e2e/layer2.sh writes: one JSON object per statement run,

    {"scenario": "G06", "user": "ann", "sid": "q03", "via": "direct" | "impersonate",
     "engine": "opa" | "ranger" | "raw", "statement": "...",
     "ok": true, "columns": ["email"], "rows": [["..."]], "error": null}

`opa` is trino-opa (the OPA plugin, answered by egret), `ranger` is trino-ranger (the Ranger
plugin on the same profile's Ranger download: the oracle, S8), and `raw` is the same statement
run by the platform admin on trino-ranger, an optional reference for "clearer values".

For each (scenario, user, sid, via) the two sides are compared in this order: the sha256 of the
sorted rows, then the columns and the per-column values, then the error class (`Access Denied`
or not). Every difference gets a kind:

  explainable   opa-refuses          OPA fails the statement, Ranger serves it
                opa-hides-rows       same columns, OPA's rows are a proper sub-multiset of Ranger's
                opa-hides-columns    OPA's columns are a proper subset of Ranger's
                values-differ        same columns and row count, some column's values differ
                rows-differ          same columns, OPA has no more rows, but not a subset
                error-class-differs  both fail, one with Access Denied and one without

  never         opa-serves           OPA serves what Ranger refuses
  explainable   opa-more-rows        OPA returns rows Ranger does not, and more of them
                opa-more-columns     OPA returns a column Ranger does not
                opa-clearer-values   a differing column is clearer on OPA (see clearer())

A never-explainable difference fails whatever explained.toml says (the Task 18 rule): it is an
evaluator or a harness bug. An explainable one must match a `layer2` matcher of an entry tagged
`layer = [.., 2]` (a missing `layer` means `[1]`). An entry tagged with layer 2 that no case
produced is stale. Any of those, or a case with only one side, exits 1.
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import re
import sys
import tomllib
from collections import Counter
from pathlib import Path

EXPLAINABLE = {
    "opa-refuses",
    "opa-hides-rows",
    "opa-hides-columns",
    "values-differ",
    "rows-differ",
    "error-class-differs",
}
NEVER = {"opa-serves", "opa-more-rows", "opa-more-columns", "opa-clearer-values"}
HEX64 = re.compile(r"^[0-9a-fA-F]{64}$")
DEFAULT_EXPLAINED = (
    Path(__file__).resolve().parents[2]
    / "crates/egret-perm-compile/tests/opa_differential/explained.toml"
)


def rows_sha(rows: list[list]) -> str:
    canon = sorted(json.dumps(r, sort_keys=True) for r in rows)
    return hashlib.sha256("\n".join(canon).encode()).hexdigest()


def error_class(err: str | None) -> str:
    return "access-denied" if err and "Access Denied" in err else "other-error"


def column(res: dict, name: str) -> list:
    i = res["columns"].index(name)
    return sorted((r[i] for r in res["rows"]), key=lambda v: json.dumps(v))


def clearer(opa_col: list, rgr_col: list, raw_col: list | None) -> str | None:
    """Why OPA's values of one column are clearer than Ranger's, or None.

    Masks collapse or replace values, so OPA is taken as clearer when: it equals the
    unrestricted reference where Ranger does not; Ranger is all NULL where OPA is not; OPA has
    more distinct values; or Ranger's values are all 64-hex hashes and OPA's are not.
    """
    if raw_col is not None and opa_col == raw_col and rgr_col != raw_col:
        return "OPA equals the admin's reference and Ranger does not"
    if rgr_col and all(v is None for v in rgr_col) and any(v is not None for v in opa_col):
        return "Ranger is all NULL and OPA is not"
    distinct_o = len({json.dumps(v) for v in opa_col})
    distinct_r = len({json.dumps(v) for v in rgr_col})
    if distinct_o > distinct_r:
        return f"OPA has {distinct_o} distinct values, Ranger {distinct_r}"
    hashed = [v for v in rgr_col if v is not None]
    if hashed and all(isinstance(v, str) and HEX64.match(v) for v in hashed) and any(
        v is not None and not (isinstance(v, str) and HEX64.match(v)) for v in opa_col
    ):
        return "Ranger's values are hashes and OPA's are not"
    return None


def classify(opa: dict, rgr: dict, raw: dict | None) -> dict:
    """The verdict of one case: {"kind": "same" | <kind>, "columns": [...], "detail": str}."""
    if opa["ok"] and rgr["ok"]:
        if opa["columns"] == rgr["columns"] and rows_sha(opa["rows"]) == rows_sha(rgr["rows"]):
            return {"kind": "same", "columns": [], "detail": ""}
        oc, rc = opa["columns"], rgr["columns"]
        if set(oc) - set(rc):
            extra = sorted(set(oc) - set(rc))
            return {"kind": "opa-more-columns", "columns": extra, "detail": f"OPA also returns {extra}"}
        if set(oc) < set(rc):
            hidden = sorted(set(rc) - set(oc))
            return {"kind": "opa-hides-columns", "columns": hidden, "detail": f"Ranger also returns {hidden}"}
        # Same column set (perhaps in another order): OPA's rows in Ranger's column order.
        o_rows = Counter(json.dumps([r[oc.index(c)] for c in rc]) for r in opa["rows"])
        r_rows = Counter(json.dumps(r) for r in rgr["rows"])
        if o_rows == r_rows:
            return {"kind": "same", "columns": [], "detail": "same rows, other column order"}
        n_o, n_r = sum(o_rows.values()), sum(r_rows.values())
        if not (o_rows - r_rows):
            return {
                "kind": "opa-hides-rows",
                "columns": [],
                "detail": f"OPA {n_o} rows, a subset of Ranger's {n_r}",
            }
        differing = [c for c in rc if column(opa, c) != column(rgr, c)]
        if n_o == n_r:
            for c in differing:
                raw_col = column(raw, c) if raw and raw.get("ok") and c in raw["columns"] else None
                why = clearer(column(opa, c), column(rgr, c), raw_col)
                if why:
                    return {"kind": "opa-clearer-values", "columns": [c], "detail": why}
            return {
                "kind": "values-differ",
                "columns": differing,
                "detail": f"columns {differing} differ",
            }
        if n_o > n_r:
            return {
                "kind": "opa-more-rows",
                "columns": differing,
                "detail": f"OPA {n_o} rows, Ranger {n_r}, not a subset",
            }
        return {
            "kind": "rows-differ",
            "columns": differing,
            "detail": f"OPA {n_o} rows, Ranger {n_r}, not a subset; columns {differing} differ",
        }
    if not opa["ok"] and rgr["ok"]:
        return {"kind": "opa-refuses", "columns": [], "detail": first_line(opa["error"])}
    if opa["ok"] and not rgr["ok"]:
        return {"kind": "opa-serves", "columns": [], "detail": first_line(rgr["error"])}
    eo, er = error_class(opa["error"]), error_class(rgr["error"])
    if eo == er:
        return {"kind": "same", "columns": [], "detail": f"both {eo}"}
    return {
        "kind": "error-class-differs",
        "columns": [],
        "detail": f"opa {eo}: {first_line(opa['error'])} | ranger {er}: {first_line(rgr['error'])}",
    }


def first_line(s: str | None) -> str:
    return (s or "").strip().splitlines()[0][:300] if (s or "").strip() else ""


def load_entries(path: Path) -> list[dict]:
    data = tomllib.loads(path.read_text())
    entries = []
    for e in data.get("difference", []):
        layer = e.get("layer", [1])
        if 2 not in layer:
            continue
        matchers = e.get("layer2", [])
        if not matchers:
            raise SystemExit(f"explained.toml: {e['id']} is tagged layer 2 but has no layer2 matcher")
        for m in matchers:
            if set(m) != {"scenario", "user", "statement", "kind"}:
                raise SystemExit(f"explained.toml: {e['id']}: a layer2 matcher has keys {sorted(m)}")
            if m["kind"] not in EXPLAINABLE:
                raise SystemExit(
                    f"explained.toml: {e['id']}: kind {m['kind']!r} is not explainable "
                    f"(explainable: {sorted(EXPLAINABLE)})"
                )
            re.compile(m["statement"])
        entries.append(e)
    return entries


def matches(m: dict, case: dict) -> bool:
    return (
        fnmatch.fnmatchcase(case["scenario"], m["scenario"])
        and fnmatch.fnmatchcase(case["user"], m["user"])
        and re.search(m["statement"], case["statement"]) is not None
        and m["kind"] == case["kind"]
    )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("results", type=Path)
    ap.add_argument("--explained", type=Path, default=DEFAULT_EXPLAINED)
    ap.add_argument("--report", type=Path, default=Path("layer2-report.json"))
    args = ap.parse_args(argv)

    entries = load_entries(args.explained)
    runs: dict[tuple, dict[str, dict]] = {}
    for n, line in enumerate(args.results.read_text().splitlines(), 1):
        if not line.strip():
            continue
        r = json.loads(line)
        key = (r["scenario"], r["user"], r["sid"], r.get("via", "direct"))
        if r["engine"] in runs.setdefault(key, {}):
            raise SystemExit(f"{args.results}:{n}: {key} {r['engine']} is listed twice")
        runs[key][r["engine"]] = r

    cases, incomplete = [], []
    for key in sorted(runs):
        sides = runs[key]
        if "opa" not in sides or "ranger" not in sides:
            incomplete.append({"case": list(key), "has": sorted(sides)})
            continue
        v = classify(sides["opa"], sides["ranger"], sides.get("raw"))
        case = {
            "scenario": key[0],
            "user": key[1],
            "sid": key[2],
            "via": key[3],
            "statement": sides["opa"]["statement"],
            **v,
            "entry": None,
            "opa": summary(sides["opa"]),
            "ranger": summary(sides["ranger"]),
        }
        cases.append(case)

    used: Counter = Counter()
    forbidden, unexplained = [], []
    for c in cases:
        if c["kind"] == "same":
            continue
        claimed = [e["id"] for e in entries if any(matches(m, c) for m in e["layer2"])]
        if c["kind"] in NEVER:
            c["claimed_by"] = claimed
            forbidden.append(c)
            continue
        if claimed:
            c["entry"] = claimed[0]
            used[claimed[0]] += 1
        else:
            unexplained.append(c)
    stale = [e["id"] for e in entries if not used[e["id"]]]

    tally = Counter(c["kind"] for c in cases)
    report = {
        "cases_total": len(cases),
        "tally": dict(sorted(tally.items())),
        "by_scenario": {
            s: dict(sorted(Counter(c["kind"] for c in cases if c["scenario"] == s).items()))
            for s in sorted({c["scenario"] for c in cases})
        },
        "entries_used": dict(sorted(used.items())),
        "forbidden": forbidden,
        "unexplained": unexplained,
        "stale_entries": stale,
        "incomplete": incomplete,
        "cases": cases,
    }
    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    print(f"layer2: {len(cases)} cases; {dict(sorted(tally.items()))}")
    for eid, n in sorted(used.items()):
        print(f"layer2: entry {eid}: {n} case(s)")
    for c in forbidden:
        extra = f" (explained.toml entries {c['claimed_by']} claim it; they cannot)" if c["claimed_by"] else ""
        print(f"layer2: FORBIDDEN {describe(c)}{extra}")
    for c in unexplained:
        print(f"layer2: UNEXPLAINED {describe(c)}")
    for s in stale:
        print(f"layer2: STALE explained.toml entry tagged layer 2 that no case produced: {s}")
    for i in incomplete:
        print(f"layer2: INCOMPLETE {i}")
    failed = bool(forbidden or unexplained or stale or incomplete)
    print(f"layer2: {'FAIL' if failed else 'PASS'} (report: {args.report})")
    return 1 if failed else 0


def summary(r: dict) -> dict:
    if not r["ok"]:
        return {"error": first_line(r["error"])}
    rows = r["rows"]
    return {"columns": r["columns"], "rows": len(rows), "sha256": rows_sha(rows), "head": rows[:3]}


def describe(c: dict) -> str:
    cols = f" columns {c['columns']}" if c["columns"] else ""
    return (
        f"{c['scenario']} {c['user']} {c['sid']} ({c['via']}) {c['kind']}{cols}: "
        f"{c['statement']} — {c['detail']}"
    )


if __name__ == "__main__":
    sys.exit(main())
