#!/usr/bin/env python3
"""The non-exhaustive-`RunState`-pattern census — W8 Task 5 fix round 3.

**Why this exists.** Adding a variant to a state enum silently falsifies every
predicate that enumerates a SUBSET of it, and the compiler cannot help: you can
ask it to flag *uses* of a name (`#[deprecated]`), and these sites are defined
by an **omission**. W8 Task 5 shipped two such defects — `guess_state`'s trigger
set and `parallel::drive_concurrent`'s rule 1 — so the instrument is the
deliverable, not the number.

**Run it from the repository root:**

    python3 crates/egret-workflow/mutations/w8-task5-state-census.py

# The two figures, and why they differ

`--window N` reproduces fix round 2's first instrument: a line-oriented scan
that keeps a hit only if the token `RunState` appears within N lines. That scan
**missed `guess_state`'s own `matches!`** — the site fix round 1 had just
repaired — because its pattern sits four lines below the `matches!` with a
comment between, and it missed **this task's own module**, `allow_failure.rs`,
whose `soften` and `fail` are subset-matches with `_` catch-alls.

The default is window-free: comments and string literals are blanked, `mod
tests` is stripped by brace matching, and `matches!(…)` / `match …` bodies are
extracted by balanced delimiters. A site is found because the parser *reaches*
it, not because a token happened to be near enough. **A syntactic census is only
as wide as its window, and the window is invisible in the number** — which is
why both figures are printed.

# What it cannot see, and these are acquitted rather than open

* **The string plane.** A state compared as text carries no `RunState` token:
  `tests/executor.rs`'s `const TERMINAL: &[&str]`, `handler_surface.py`'s
  `RUN_STATE`, and the SQL vocabularies in migrations 0141/0142/0143. Checked by
  hand at fix round 3 and all current: `TERMINAL` is derived from the golden and
  already holds `WARNING`, `RUN_STATE` gained it at fix round 0, 0143 added
  `TERMINAL:WARNING` and is exercised by `tests/worker.rs`.
* **Collection membership** — a predicate held as data rather than as a pattern.
  Measured empty in fact: production code builds no `Vec<RunState>` it later
  `.contains`-es.
* **The crate boundary.** Measured empty in fact: the only other `RunState` in
  the workspace is `egret-automation`'s unrelated `Success/Working/Error` enum,
  a name collision.
* **Derivations that are not patterns** — `loop_until::iteration()` counts
  occurrences of `Running`. Still open, and unreachable by any pattern scan.
"""

import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
SRC = os.path.join(CRATE, "src")


def blank_comments_and_strings(src: str) -> str:
    """Replace comment and string bytes with spaces, keeping offsets stable."""
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        elif c == "/" and i + 1 < n and src[i + 1] == "*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            for k in range(i, j):
                if src[k] != "\n":
                    out[k] = " "
            i = j
        elif c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            for k in range(i, min(j, n)):
                if src[k] != "\n":
                    out[k] = " "
            i = j
        else:
            i += 1
    return "".join(out)


def strip_test_modules(src: str) -> str:
    """Blank every `mod tests { … }` body by brace matching."""
    out = list(src)
    for m in re.finditer(r"mod\s+tests\s*\{", src):
        depth, i = 0, m.end() - 1
        while i < len(src):
            if src[i] == "{":
                depth += 1
            elif src[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        for k in range(m.start(), min(i + 1, len(src))):
            if src[k] != "\n":
                out[k] = " "
    return "".join(out)


def balanced(src: str, start: int, open_ch: str, close_ch: str) -> int:
    depth, i = 0, start
    while i < len(src):
        if src[i] == open_ch:
            depth += 1
        elif src[i] == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return len(src)


def sites(path: str, window: int | None):
    """Yield (line, kind, text) for every candidate in one file."""
    raw = open(path).read()
    clean = strip_test_modules(blank_comments_and_strings(raw))
    lines = raw.split("\n")

    def line_of(off):
        return clean[:off].count("\n") + 1

    found = []
    if window is not None:
        # Fix round 2's instrument, reproduced exactly.
        pats = [
            ("matches!", re.compile(r"matches!\s*\(")),
            ("eq/ne", re.compile(r"[!=]=\s*(Some\()?\s*&?\s*RunState::")),
            ("iflet", re.compile(r"if\s+let\s+.*RunState::")),
            ("contains", re.compile(r"\.contains\(")),
            ("arraylit", re.compile(r"\[\s*RunState::")),
            ("is_term", re.compile(r"\bis_terminal\b|\bis_terminated")),
        ]
        stripped = clean.split("\n")
        for i, l in enumerate(stripped, 1):
            for name, rx in pats:
                if rx.search(l):
                    ctx = "\n".join(stripped[max(0, i - 1 - window) : i + window])
                    if "RunState" in ctx or name == "is_term":
                        found.append((i, name, lines[i - 1].strip()))
                    break
        return found

    # Window-free. A `matches!` or `match` is a site when its EXTRACTED body
    # mentions a variant -- reached by the parser, not by proximity.
    for m in re.finditer(r"\bmatches!\s*\(", clean):
        end = balanced(clean, m.end() - 1, "(", ")")
        if "RunState::" in clean[m.end() : end]:
            found.append((line_of(m.start()), "matches!", lines[line_of(m.start()) - 1].strip()))
    for m in re.finditer(r"\bmatch\b", clean):
        brace = clean.find("{", m.end())
        if brace < 0 or brace - m.end() > 200:
            continue
        end = balanced(clean, brace, "{", "}")
        body = clean[brace:end]
        if "RunState::" in body:
            kind = "match+_" if re.search(r"\n\s*_\s*=>", body) else "match"
            found.append((line_of(m.start()), kind, lines[line_of(m.start()) - 1].strip()))
    for rx, name in [
        (r"[!=]=\s*(?:Some\(\s*)?&?\s*RunState::", "eq/ne"),
        (r"if\s+let\s+[^;\n]*RunState::", "iflet"),
        (r"\[\s*RunState::", "arraylit"),
        (r"\bis_terminal\b|\bis_terminated\w*", "is_term"),
    ]:
        for m in re.finditer(rx, clean):
            found.append((line_of(m.start()), name, lines[line_of(m.start()) - 1].strip()))
    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--window", type=int, default=None,
                    help="reproduce fix round 2's line-oriented scan with this window")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    total, per_file = 0, {}
    for root, _, files in os.walk(SRC):
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(root, f)
            hits = sites(p, args.window)
            if hits:
                rel = os.path.relpath(p, CRATE)
                per_file[rel] = hits
                total += len(hits)
    for rel in sorted(per_file):
        print(f"{len(per_file[rel]):4d}  {rel}")
        if args.verbose:
            for ln, kind, text in sorted(per_file[rel]):
                print(f"        {ln:5d}  {kind:9s} {text[:100]}")
    label = "window-free" if args.window is None else f"window +/-{args.window}"
    print(f"TOTAL ({label}): {total} sites in {len(per_file)} files")
    # The instrument must reach this task's own module, which is how fix round
    # 2's scan failed: `allow_failure.rs` was absent from its table entirely.
    if args.window is None and "src/executor/allow_failure.rs" not in per_file:
        print("FAIL: the census does not reach src/executor/allow_failure.rs", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
