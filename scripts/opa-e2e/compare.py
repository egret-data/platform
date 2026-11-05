#!/usr/bin/env python3
"""Layer 2 of the OPA target (governance SP2b, Task 21): compare trino-opa with trino-ranger.

Run with `python3 -I scripts/opa-e2e/compare.py RESULTS.jsonl [--explained FILE] [--report FILE]`.

RESULTS.jsonl is what scripts/opa-e2e/layer2.sh writes (through collect.py): one JSON object
per statement run,

    {"scenario": "G06", "user": "ann", "sid": "q03", "via": "direct" | "impersonate",
     "engine": "opa" | "ranger" | "ref", "statement": "...",
     "ok": true, "columns": ["email"], "rows": [["..."]],
     "error": null | "message", "error_name": null | "PERMISSION_DENIED" | ..., "harness": false}

`opa` is trino-opa (the OPA plugin, answered by egret), `ranger` is trino-ranger (the Ranger
plugin on the same profile's Ranger download: the oracle, S8). `ref` is the same SELECT run by
the platform admin on trino-ranger in the scenario's POLICY-FREE state (after the reset, before
the setup: no mask, filter, deny or classification exists), so its values are the clear data.

For each (scenario, user, sid, via) the two sides are compared in this order: the sha256 of the
sorted rows; then the columns, the row counts and the per-column values; then, when both fail,
the error class (`errorName`) and the normalised message (norm_error()). Kinds:

  explainable (with a matching layer-2 matcher of explained.toml)
    opa-refuses          OPA fails the statement, Ranger serves it
    opa-hides-rows       same columns, OPA's rows are a proper non-empty sub-multiset of Ranger's
    opa-hides-all-rows   same columns, OPA returns no row where Ranger returns some
    opa-hides-columns    OPA's columns are a proper subset of Ranger's
    values-differ        same columns and row count, some column's values differ
    rows-differ          same columns, OPA has fewer rows, but not a subset
    errors-differ        both fail, with another errorName or another normalised message

  never explainable (OPA shows more; fails whatever explained.toml says)
    opa-serves           OPA serves what Ranger refuses
    opa-more-columns     OPA returns a column Ranger does not
    opa-more-rows        OPA returns more rows than Ranger
    opa-clearer-values   some column holds a value on OPA that Ranger masks or does not return
                         (shows_more_values(): checked on every path, whatever the row counts)

A never-explainable difference is an evaluator or a harness bug (the Task 18 rule). An entry of
explained.toml tagged with layer 2 (`layer = [.., 2]`; a missing `layer` means `[1]`) explains a
case through one of its `[[difference.layer2]]` matchers. A matcher no case used is stale. A
harness error (`harness: true`: no response, not JSON, a curl failure, a kill target that never
ran), a case with only one side, an unexplained or a never-explainable difference, or a stale
matcher exits 1.
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
    "opa-hides-all-rows",
    "opa-hides-columns",
    "values-differ",
    "rows-differ",
    "errors-differ",
}
NEVER = {"opa-serves", "opa-more-rows", "opa-more-columns", "opa-clearer-values"}
HEX64 = re.compile(r"^[0-9a-fA-F]{64}$")
DEFAULT_EXPLAINED = (
    Path(__file__).resolve().parents[2]
    / "crates/egret-perm-compile/tests/opa_differential/explained.toml"
)


def canon(v) -> str:
    return json.dumps(v, sort_keys=True)


def rows_sha(rows: list[list]) -> str:
    return hashlib.sha256("\n".join(sorted(canon(r) for r in rows)).encode()).hexdigest()


def error_name(r: dict) -> str:
    """The Trino errorName; for a record without one, the `NAME: ` prefix of its message."""
    if r.get("error_name"):
        return r["error_name"]
    m = re.match(r"([A-Z][A-Z0-9_]+): ", r.get("error") or "")
    return m.group(1) if m else "UNKNOWN"


def norm_error(msg: str | None) -> str:
    """A denial or failure as both plugins mean it, without the wording they differ in.

    Measured on both engines: Ranger names the bare relation (`t1`) where OPA names it
    qualified (`lake.s1.t1`); OPA's SelectFromColumns refusal lists the columns masks and
    filters read as well; impersonation reads "Principal A cannot become user B" on Ranger and
    "User A cannot impersonate user B" on OPA. Query ids are dropped too.
    """
    s = (msg or "").strip().splitlines()[0] if (msg or "").strip() else ""
    s = re.sub(r"^[A-Z][A-Z0-9_]+: ", "", s)
    s = re.sub(r"^Access Denied: ", "", s)
    s = re.sub(
        r"(?i)\b(?:principal|user) (\S+) cannot (?:become|impersonate) user (\S+)",
        r"impersonate \1 -> \2",
        s,
    )
    s = re.sub(r"\[[^\]]*\]", "[]", s)
    s = re.sub(r"\b\d{8}_\d{6}_\d{5}_[a-z0-9]{5}\b", "<query>", s)
    s = re.sub(r"[\w$]+(?:\.[\w$]+)+", lambda m: m.group(0).rsplit(".", 1)[-1], s)
    return re.sub(r"\s+", " ", s).strip().lower()


def column(res: dict, name: str) -> list:
    i = res["columns"].index(name)
    return [r[i] for r in res["rows"]]


def shows_more_values(opa: dict, rgr: dict, ref: dict | None) -> tuple[str, str] | None:
    """(column, why) where OPA holds a value Ranger masks or does not return; else None.

    With the policy-free reference: a value counts as clear when the reference column holds it;
    OPA shows more when it returns a clear value more often than Ranger does. That catches a
    CUSTOM or other distinctness-keeping mask, a dropped mask on fewer rows, and a column OPA
    keeps clear next to one it hides. Without a reference (the statement failed there, or it
    is no SELECT) the value-shape tests: Ranger all NULL where OPA is not; more distinct values
    on OPA; Ranger all 64-hex hashes where OPA is not.
    """
    shared = [c for c in rgr["columns"] if c in opa["columns"]]
    ref_ok = ref is not None and ref.get("ok")
    for c in shared:
        o, r = column(opa, c), column(rgr, c)
        if Counter(map(canon, o)) == Counter(map(canon, r)):
            continue
        if ref_ok and c in ref["columns"]:
            clear = {canon(v) for v in column(ref, c)}
            co, cr = Counter(map(canon, o)), Counter(map(canon, r))
            extra = [v for v in co if v in clear and co[v] > cr[v]]
            if extra:
                return c, f"OPA returns the clear value {extra[0]} {co[extra[0]]}x, Ranger {cr[extra[0]]}x"
            continue
        if r and all(v is None for v in r) and any(v is not None for v in o):
            return c, "Ranger is all NULL and OPA is not"
        do, dr = len(set(map(canon, o))), len(set(map(canon, r)))
        if do > dr:
            return c, f"OPA has {do} distinct values, Ranger {dr}"
        hashed = [v for v in r if v is not None]
        if hashed and all(isinstance(v, str) and HEX64.match(v) for v in hashed) and any(
            v is not None and not (isinstance(v, str) and HEX64.match(v)) for v in o
        ):
            return c, "Ranger's values are hashes and OPA's are not"
    return None


def classify(opa: dict, rgr: dict, ref: dict | None) -> dict:
    """The verdict of one case: {"kind": "same" | <kind>, "columns": [...], "detail": str}."""
    if opa["ok"] and rgr["ok"]:
        oc, rc = opa["columns"], rgr["columns"]
        if oc == rc and rows_sha(opa["rows"]) == rows_sha(rgr["rows"]):
            return {"kind": "same", "columns": [], "detail": ""}
        extra = sorted(set(oc) - set(rc))
        if extra:
            return {"kind": "opa-more-columns", "columns": extra, "detail": f"OPA also returns {extra}"}
        n_o, n_r = len(opa["rows"]), len(rgr["rows"])
        if n_o > n_r:
            return {"kind": "opa-more-rows", "columns": [], "detail": f"OPA {n_o} rows, Ranger {n_r}"}
        more = shows_more_values(opa, rgr, ref)
        if more:
            return {"kind": "opa-clearer-values", "columns": [more[0]], "detail": more[1]}
        if set(oc) < set(rc):
            hidden = sorted(set(rc) - set(oc))
            return {"kind": "opa-hides-columns", "columns": hidden, "detail": f"Ranger also returns {hidden}"}
        # Same column set (perhaps in another order): OPA's rows in Ranger's column order.
        o_rows = Counter(canon([r[oc.index(c)] for c in rc]) for r in opa["rows"])
        r_rows = Counter(canon(r) for r in rgr["rows"])
        if o_rows == r_rows:
            return {"kind": "same", "columns": [], "detail": "same rows, other column order"}
        differing = [c for c in rc if sorted(map(canon, column(opa, c))) != sorted(map(canon, column(rgr, c)))]
        if not (o_rows - r_rows):
            kind = "opa-hides-all-rows" if n_o == 0 else "opa-hides-rows"
            return {"kind": kind, "columns": [], "detail": f"OPA {n_o} rows, a subset of Ranger's {n_r}"}
        if n_o == n_r:
            return {"kind": "values-differ", "columns": differing, "detail": f"columns {differing} differ"}
        return {
            "kind": "rows-differ",
            "columns": differing,
            "detail": f"OPA {n_o} rows, Ranger {n_r}, not a subset; columns {differing} differ",
        }
    if not opa["ok"] and rgr["ok"]:
        return {"kind": "opa-refuses", "columns": [], "detail": first_line(opa["error"])}
    if opa["ok"] and not rgr["ok"]:
        return {"kind": "opa-serves", "columns": [], "detail": first_line(rgr["error"])}
    eo, er = error_name(opa), error_name(rgr)
    no, nr = norm_error(opa["error"]), norm_error(rgr["error"])
    if eo == er and no == nr:
        return {"kind": "same", "columns": [], "detail": f"both {eo}: {no}"}
    return {"kind": "errors-differ", "columns": [], "detail": f"opa {eo}: {no} | ranger {er}: {nr}"}


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


def matches(m: dict, case: dict, any_kind: bool = False) -> bool:
    return (
        fnmatch.fnmatchcase(case["scenario"], m["scenario"])
        and fnmatch.fnmatchcase(case["user"], m["user"])
        and re.search(m["statement"], case["statement"]) is not None
        and (any_kind or m["kind"] == case["kind"])
    )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("results", type=Path)
    ap.add_argument("--explained", type=Path, default=DEFAULT_EXPLAINED)
    ap.add_argument("--report", type=Path, default=Path("layer2-report.json"))
    args = ap.parse_args(argv)

    entries = load_entries(args.explained)
    runs: dict[tuple, dict[str, dict]] = {}
    harness = []
    for n, line in enumerate(args.results.read_text().splitlines(), 1):
        if not line.strip():
            continue
        r = json.loads(line)
        key = (r["scenario"], r["user"], r["sid"], r.get("via", "direct"))
        if r["engine"] in runs.setdefault(key, {}):
            raise SystemExit(f"{args.results}:{n}: {key} {r['engine']} is listed twice")
        runs[key][r["engine"]] = r
        if r.get("harness") or (r.get("error") or "").startswith("layer2 harness"):
            harness.append({"case": list(key), "engine": r["engine"], "error": first_line(r["error"])})

    cases, incomplete = [], []
    for key in sorted(runs):
        sides = runs[key]
        if "opa" not in sides or "ranger" not in sides:
            incomplete.append({"case": list(key), "has": sorted(sides)})
            continue
        v = classify(sides["opa"], sides["ranger"], sides.get("ref"))
        cases.append({
            "scenario": key[0],
            "user": key[1],
            "sid": key[2],
            "via": key[3],
            "statement": sides["opa"]["statement"],
            **v,
            "entry": None,
            "opa": summary(sides["opa"]),
            "ranger": summary(sides["ranger"]),
        })

    used: Counter = Counter()  # (entry id, matcher index)
    forbidden, unexplained = [], []
    for c in cases:
        if c["kind"] == "same":
            continue
        claimed = [
            (e["id"], i) for e in entries for i, m in enumerate(e["layer2"]) if matches(m, c)
        ]
        if c["kind"] in NEVER:
            # Reported, never honoured: entries whose matchers name this case, of any kind.
            c["claimed_by"] = sorted(
                {e["id"] for e in entries for m in e["layer2"] if matches(m, c, any_kind=True)}
            )
            forbidden.append(c)
            continue
        if claimed:
            c["entry"] = claimed[0][0]
            for k in claimed:
                used[k] += 1
        else:
            unexplained.append(c)
    stale = [
        f"{e['id']} layer2[{i}] ({m['scenario']} {m['user']} {m['kind']} /{m['statement']}/)"
        for e in entries
        for i, m in enumerate(e["layer2"])
        if not used[(e["id"], i)]
    ]
    by_entry = Counter()
    for c in cases:
        if c["entry"]:
            by_entry[c["entry"]] += 1

    tally = Counter(c["kind"] for c in cases)
    report = {
        "cases_total": len(cases),
        "tally": dict(sorted(tally.items())),
        "by_scenario": {
            s: dict(sorted(Counter(c["kind"] for c in cases if c["scenario"] == s).items()))
            for s in sorted({c["scenario"] for c in cases})
        },
        "entries_used": dict(sorted(by_entry.items())),
        "forbidden": forbidden,
        "unexplained": unexplained,
        "stale_matchers": stale,
        "harness_errors": harness,
        "incomplete": incomplete,
        "cases": cases,
    }
    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    print(f"layer2: {len(cases)} cases; {dict(sorted(tally.items()))}")
    for eid, n in sorted(by_entry.items()):
        print(f"layer2: entry {eid}: {n} case(s)")
    for h in harness:
        print(f"layer2: HARNESS {h}")
    for c in forbidden:
        extra = f" (explained.toml entries {c['claimed_by']} claim it; they cannot)" if c["claimed_by"] else ""
        print(f"layer2: FORBIDDEN {describe(c)}{extra}")
    for c in unexplained:
        print(f"layer2: UNEXPLAINED {describe(c)}")
    for s in stale:
        print(f"layer2: STALE layer-2 matcher no case used: {s}")
    for i in incomplete:
        print(f"layer2: INCOMPLETE {i}")
    failed = bool(harness or forbidden or unexplained or stale or incomplete)
    print(f"layer2: {'FAIL' if failed else 'PASS'} (report: {args.report})")
    return 1 if failed else 0


def summary(r: dict) -> dict:
    if not r["ok"]:
        return {"error": first_line(r["error"]), "error_name": error_name(r)}
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
