#!/usr/bin/env python3
"""W3b Task 1 -- how much of the corpus a real handler can actually reproduce.

`runnable.txt` is a pure function of `corpus.json` + `probe.json`.  This file is
the same shape: `handler-surface.txt` is a pure function of

    corpus/            the 351 flow sources, as published to Kestra
    golden.json        what Kestra did with them
    baseline.txt       what egret does NOT reproduce today
    task-kinds.json    every corpus task type resolved to its Kestra interface

and nothing else.  It needs no Kestra, no docker and no third-party module --
only python3, the same rule `harvest.py` is written to.

    python3 handler_surface.py                # rewrite handler-surface.txt
    python3 handler_surface.py --check        # regenerate and diff; exit 1 on drift
    python3 handler_surface.py --self-test    # every guard must fire, and say why
    python3 handler_surface.py --refresh-kinds  # re-derive task-kinds.json

`task-kinds.json` is the only input that comes from the sibling Kestra checkout,
and it is COMMITTED for the same reason `test-source-classes.json` is: a reader
regenerating this file should not need a 1.2 GB checkout to do it.  Membership
of a Kestra interface is *evidence* about a task type; the module it lives in is
not -- `core.flow.Sleep` is a `RunnableTask` (Sleep.java:47) despite its
package, and classifying by module is exactly the error this file's population
figure had to correct.
"""

import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus")
GOLDEN = os.path.join(HERE, "golden.json")
BASELINE = os.path.join(HERE, "baseline.txt")
KINDS_FILE = os.path.join(HERE, "task-kinds.json")
OUT = os.path.join(HERE, "handler-surface.txt")
KESTRA_CHECKOUT = os.path.abspath(os.path.join(HERE, "..", "..", "..", "..", "kestra"))


def bail(msg):
    """Guards raise; they never `assert`.

    `python3 -O` strips assert statements, so a guard written as an assert is a
    guard that silently disappears under a flag anyone may pass.
    """
    raise SystemExit("handler_surface.py guard failed: " + msg)


# --------------------------------------------------------------------------
# The flow sources.  A YAML subset parser, because the corpus is YAML and this
# script may not import one.
# --------------------------------------------------------------------------
#
# The subset is the one Kestra's own test flows are written in: block mappings,
# block sequences, block scalars, flow scalars.  It is deliberately STRICT --
# a line it cannot account for is an error, not a line it skips -- because a
# lenient parser that drops a `tasks:` list would under-count the population and
# look exactly like a smaller population.
#
# It is validated against `golden.json` rather than trusted: every task name
# Kestra recorded a run for must be a task id this parser found.  That check
# reads 0 today over all 266 comparable `runs` entries, and it is what makes
# this parser evidence instead of an assumption.

KEY = re.compile(r"^(?P<key>(?:\"[^\"]*\"|'[^']*'|[^\s\"'#\[\]{},][^:#]*?))\s*:(?:\s+(?P<val>.*?))?\s*$")
BLOCK_SCALAR = "\x00block-scalar\x00"


class YamlError(Exception):
    pass


def _unquote(s):
    s = s.strip()
    if len(s) >= 2 and s[0] == s[-1] and s[0] in "\"'":
        return s[1:-1]
    return s


def parse_yaml(text, where):
    lines = text.splitlines()
    pos = [0]

    def ind_of(ln):
        return len(ln) - len(ln.lstrip(" "))

    def skippable(ln):
        s = ln.strip()
        return not s or s.startswith("#") or s == "---"

    def peek():
        while pos[0] < len(lines) and skippable(lines[pos[0]]):
            pos[0] += 1
        return lines[pos[0]] if pos[0] < len(lines) else None

    def eat_block_scalar(ind):
        while pos[0] < len(lines):
            ln = lines[pos[0]]
            if ln.strip() and ind_of(ln) <= ind:
                break
            pos[0] += 1

    def value_block(key_ind):
        ln = peek()
        if ln is None:
            return None
        cur, body = ind_of(ln), ln.lstrip(" ")
        if (body == "-" or body.startswith("- ")) and cur >= key_ind:
            return parse_seq(cur)
        if cur > key_ind:
            return parse_map(cur)
        return None

    def parse_seq(ind):
        out = []
        while True:
            ln = peek()
            if ln is None or ind_of(ln) != ind:
                break
            body = ln.lstrip(" ")
            if not (body == "-" or body.startswith("- ")):
                break
            rest = ln[ind + 1:]
            if not rest.strip():
                pos[0] += 1
                out.append(value_block(ind))
                continue
            lead = len(rest) - len(rest.lstrip(" "))
            item = rest.lstrip(" ")
            if KEY.match(item):
                # A `- key: value` opens a mapping whose members line up under
                # the key, not under the dash.  Rewriting the line is how this
                # parser stays one function instead of two.
                lines[pos[0]] = " " * (ind + 1 + lead) + item
                out.append(parse_map(ind + 1 + lead))
            else:
                pos[0] += 1
                out.append(_unquote(item))
        return out

    def parse_map(ind):
        out = {}
        while True:
            ln = peek()
            if ln is None or ind_of(ln) < ind:
                break
            if ind_of(ln) > ind:
                raise YamlError(f"{where}:{pos[0]+1}: over-indented inside a mapping: {ln!r}")
            body = ln.lstrip(" ")
            if body == "-" or body.startswith("- "):
                break
            m = KEY.match(body)
            if not m:
                raise YamlError(f"{where}:{pos[0]+1}: not a mapping key: {ln!r}")
            key, val = _unquote(m.group("key")), m.group("val")
            pos[0] += 1
            if val is None or val == "":
                out[key] = value_block(ind)
            elif val[0] in "|>":
                eat_block_scalar(ind)
                out[key] = BLOCK_SCALAR
            else:
                out[key] = _unquote(val)
        return out

    doc = value_block(-1)
    if peek() is not None:
        raise YamlError(f"{where}:{pos[0]+1}: trailing content {lines[pos[0]]!r}")
    return doc


# Key paths under which a `type:` is NOT a task's.  `inputs`, `outputs` and
# `onResume` declare INPUT types (`STRING`, `BOOL`); `triggers` declares
# triggers, which no handler ever runs; `retry` declares a backoff
# (`constant`, `exponential`); `sla` declares an SLA type.  Sweeping these in is
# how a quick textual pass turns 27 trigger-declaring flows into non-members of
# the handler population -- see *Reconciling with the plan's 194*.
NON_TASK_PATHS = {"triggers", "inputs", "outputs", "conditions",
                  "listeners", "onResume", "retry", "sla"}
# A plugin PROPERTY that itself carries a `type:`.  It has no `id:` and no run.
NON_TASK_PROPERTY = "additionalPlugin"


def collect_tasks(node, path, out):
    """Every task declaration in a parsed flow, as (id, type, key path)."""
    if isinstance(node, dict):
        ty = node.get("type")
        if (isinstance(ty, str)
                and not (set(path) & NON_TASK_PATHS)
                and NON_TASK_PROPERTY not in path):
            out.append((node.get("id"), ty, tuple(path)))
        for k, v in node.items():
            collect_tasks(v, path + [k], out)
    elif isinstance(node, list):
        for v in node:
            collect_tasks(v, path, out)


# End hooks A4 (2026-10-27): the executor runs a `finally:` on these kinds by
# Kestra's list rule (`flowable::current_list`), and since its fix round the
# flow's own `finally:` too (the worker finds its leaves through
# `spec::path::find_task`). Still unported: `finally:` on `Loop`, `LoopUntil`,
# `WorkingDirectory` (A5) and `Pause` (refused, #9). `Loop` since A6
# (2026-10-27): its iterations run the Loop's own `errors:`/`finally:`.
PORTED_FINALLY = {"allowfailure", "sequential", "parallel", "if", "switch", "dag", "loop"}


def unported_finally(node):
    """True if some `finally:` is one egret does not run yet.

    The AllowFailure flowable runs its own `finally:` (spec §5.2 of its design),
    and since end hooks A4 so do the kinds in PORTED_FINALLY and the flow
    document itself (a node with no `type`).
    """
    if isinstance(node, dict):
        ty = node.get("type")
        short = ty.rsplit(".", 1)[-1].lower() if isinstance(ty, str) else None
        if "finally" in node and short is not None and short not in PORTED_FINALLY:
            return True
        return any(unported_finally(v) for v in node.values())
    if isinstance(node, list):
        return any(unported_finally(v) for v in node)
    return False


def collect_keys(node, acc):
    if isinstance(node, dict):
        for k, v in node.items():
            acc.add(k)
            collect_keys(v, acc)
    elif isinstance(node, list):
        for v in node:
            collect_keys(v, acc)


def parse_flow(raw, fid, where):
    """`parse_yaml` plus the two guards, on the one path every caller uses.

    A self-test that exercises a COPY of a guard proves the copy fires. Both
    the harvester and `--self-test` come through here.
    """
    try:
        doc = parse_yaml(raw, where)
    except YamlError as e:
        bail(f"{e}. The corpus YAML subset parser accounts for every line of "
             f"every flow or it does not run at all; a line it skipped would "
             f"shrink the population and look exactly like a smaller population")
    if not isinstance(doc, dict) or "id" not in doc:
        bail(f"{fid}: parsed to {type(doc).__name__} with no `id`; that is not a "
             f"flow and the parser has lost the document")
    return doc


def read_corpus():
    flows = {}
    for root, _dirs, files in os.walk(CORPUS):
        for f in sorted(files):
            if not f.endswith((".yaml", ".yml")):
                continue
            path = os.path.join(root, f)
            with open(path, encoding="utf-8") as fh:
                raw = fh.read()
            fid = os.path.splitext(os.path.relpath(path, CORPUS))[0]
            doc = parse_flow(raw, fid, os.path.relpath(path, HERE))
            tasks = []
            collect_tasks(doc, [], tasks)
            keys = set()
            collect_keys(doc, keys)
            flows[fid] = {"doc": doc, "raw": raw, "tasks": tasks, "keys": keys,
                          "by_id": {t[0]: (t[1], t[2]) for t in tasks if t[0]}}
    # 345 -> 351 with the six `working-directory-real-*` flows (WorkingDirectory
    # Task 8); `runnable.txt` says `total flows 351` after the same harvest.
    if len(flows) != 351:
        bail(f"corpus/ holds {len(flows)} flows, not the 351 `runnable.txt` "
             f"counts. Re-derive both or neither")
    return flows


# --------------------------------------------------------------------------
# task-kinds.json -- every corpus task type resolved to its Kestra interface
# --------------------------------------------------------------------------

KNOWN_KINDS = ("RunnableTask", "FlowableTask", "ExecutableTask", "ExecutionUpdatableTask")


def load_kinds():
    with open(KINDS_FILE, encoding="utf-8") as f:
        doc = json.load(f)
    kinds = doc.get("types") or {}
    if not kinds:
        bail(f"{KINDS_FILE} carries no types; run `handler_surface.py "
             f"--refresh-kinds` against the Kestra checkout")
    for ty, rec in kinds.items():
        if rec["kind"] not in KNOWN_KINDS:
            bail(f"{ty} is classified `{rec['kind']}`, which is not in "
                 f"KNOWN_KINDS {KNOWN_KINDS}")
    return kinds, doc.get("_checkout")


def scan_kinds(checkout, wanted):
    """Resolve each wanted FQCN to a Kestra task interface, transitively.

    `Loop extends AbstractBranch` and `AbstractBranch implements FlowableTask`;
    `WorkingDirectory extends Sequential`.  A one-level regex answers neither,
    so this builds the whole checkout's class graph and walks it.
    """
    decl = re.compile(
        r"\b(?:public\s+|final\s+|abstract\s+|static\s+)*(class|interface)\s+(\w+)"
        r"\s*(<[^{]*?>)?\s*(extends\s+[^{]*?)?\s*(implements\s+[^{]*?)?\s*\{", re.S)
    alias_re = re.compile(r'aliases\s*=\s*\{?\s*"([^"]+)"')
    decls, by_simple, aliases = {}, {}, {}
    for root, _dirs, files in os.walk(checkout):
        rp = root.replace(os.sep, "/")
        if "/node_modules" in rp or "/build/" in rp or "/.git" in rp or "/.gradle" in rp:
            continue
        for f in files:
            if not f.endswith(".java"):
                continue
            path = os.path.join(root, f)
            with open(path, encoding="utf-8", errors="replace") as fh:
                src = fh.read()
            m = re.search(r"^package\s+([\w.]+);", src, re.M)
            if not m:
                continue
            pkg, outer = m.group(1), f[:-5]
            rel = os.path.relpath(path, checkout)
            for cm in decl.finditer(src):
                name = cm.group(2)
                supers = []
                for group in (cm.group(4), cm.group(5)):
                    if not group:
                        continue
                    body = re.sub(r"^(extends|implements)\s+", "", group.strip())
                    depth, cur = 0, ""
                    for ch in body:
                        if ch == "<":
                            depth += 1
                        elif ch == ">":
                            depth -= 1
                        if ch == "," and depth == 0:
                            supers.append(cur)
                            cur = ""
                        else:
                            cur += ch
                    supers.append(cur)
                supers = [re.sub(r"<.*", "", s).split(".")[-1].strip() for s in supers if s.strip()]
                fq = f"{pkg}.{outer}" if name == outer else f"{pkg}.{outer}${name}"
                line = src[:cm.start()].count("\n") + 1
                decls[fq] = {"supers": supers, "file": rel, "line": line}
                by_simple.setdefault(name, []).append(fq)
            for a in alias_re.findall(src):
                aliases[a] = f"{pkg}.{outer}"

    def resolve(fq, seen):
        if fq in seen:
            return set()
        seen.add(fq)
        rec = decls.get(fq)
        if not rec:
            return set()
        found = set()
        for s in rec["supers"]:
            if s in KNOWN_KINDS:
                found.add(s)
            for cand in by_simple.get(s, []):
                found |= resolve(cand, seen)
        return found

    out = {}
    for ty in sorted(wanted):
        target = aliases.get(ty, ty)
        found = sorted(resolve(target, set()))
        rec = decls.get(target)
        if not rec:
            continue
        if len(found) != 1:
            continue
        entry = {"kind": found[0], "file": rec["file"], "line": rec["line"]}
        if target != ty:
            entry["alias_of"] = target
        out[ty] = entry
    return out


def refresh_kinds():
    if not os.path.isdir(KESTRA_CHECKOUT):
        bail(f"no Kestra checkout at {KESTRA_CHECKOUT}")
    flows = read_corpus()
    wanted = {ty for f in flows.values() for _i, ty, _p in f["tasks"]}
    found = scan_kinds(KESTRA_CHECKOUT, wanted)
    sha = subprocess.run(["git", "-C", KESTRA_CHECKOUT, "rev-parse", "HEAD"],
                         capture_output=True, text=True, check=True).stdout.strip()
    old = {}
    if os.path.exists(KINDS_FILE):
        with open(KINDS_FILE, encoding="utf-8") as f:
            old = (json.load(f).get("types") or {})
    doc = {
        "_": "Every task type declared by a corpus flow, resolved to the Kestra "
             "interface its class implements -- transitively, because "
             "`Loop extends AbstractBranch implements FlowableTask` and "
             "`WorkingDirectory extends Sequential`. Written by "
             "`handler_surface.py --refresh-kinds`; do not hand-edit. A type "
             "with NO entry is one the checkout does not declare, which is "
             "itself the answer for the test-source classes.",
        "_checkout": sha,
        "types": found,
    }
    with open(KINDS_FILE, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")
    added = sorted(set(found) - set(old))
    gone = sorted(set(old) - set(found))
    changed = sorted(t for t in set(found) & set(old) if found[t] != old[t])
    print(f"task-kinds.json: {len(found)} of {len(wanted)} corpus task types "
          f"resolved at checkout {sha[:9]}")
    print(f"  added {len(added)}, removed {len(gone)}, changed {len(changed)}")
    for t in added + gone + changed:
        print("   ", t)


# --------------------------------------------------------------------------
# What W3b delivers, and what it does not
# --------------------------------------------------------------------------

# The four handlers this package writes.  `core.flow.Sleep` is here because it
# is a `RunnableTask` (Sleep.java:47), not because of its package.
#
# **And, since W9a Task 11, the four core storage tasks** (`worker::handlers::
# storage`), which the ratchet's own registry (`driver::registry`) runs, and
# since W9a Task 12 the five whole-file ones, and since W9b Tasks 6 and 7
# `core.namespace.{UploadFiles, DownloadFiles, DeleteFiles, PurgeFiles}` (no
# corpus flow declares any of them, so the surface is unchanged by them). The
# name is kept for the reason given for `W3A_FLOWABLES` below; what it means
# is "the handlers the oracle's driver runs".
W3B_HANDLERS = {
    "io.kestra.plugin.core.log.Log",
    "io.kestra.plugin.core.debug.Return",
    "io.kestra.plugin.core.execution.Fail",
    "io.kestra.plugin.core.flow.Sleep",
    "io.kestra.plugin.core.storage.Write",
    "io.kestra.plugin.core.storage.Size",
    # Group C1 Task 2 (`worker::handlers::output_values`).
    "io.kestra.plugin.core.output.OutputValues",
    # Group C1 Task 3 (`worker::handlers::assert`).
    "io.kestra.plugin.core.execution.Assert",
    # Group C1 Task 4 (`worker::handlers::purge_logs`).
    "io.kestra.plugin.core.log.PurgeLogs",
    "io.kestra.plugin.core.storage.Delete",
    "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles",
    "io.kestra.plugin.core.storage.Concat",
    "io.kestra.plugin.core.storage.Split",
    "io.kestra.plugin.core.storage.Reverse",
    "io.kestra.plugin.core.storage.DeduplicateItems",
    "io.kestra.plugin.core.storage.FilterItems",
    "io.kestra.plugin.core.namespace.UploadFiles",
    "io.kestra.plugin.core.namespace.DownloadFiles",
    "io.kestra.plugin.core.namespace.DeleteFiles",
    "io.kestra.plugin.core.namespace.PurgeFiles",
    # WorkingDirectory Task 8: the script task the `working-directory-real-*`
    # flows run, through `worker::handlers::script` on the Docker runner (the
    # driver registers it, `driver::registry`). Egret's own script types are
    # `egret.script.*`/`egret.container`, which the corpus does not write.
    "io.kestra.plugin.scripts.shell.Commands",
}

# The flowables `advance()` already drives and decides for (W3a Tasks 3-6).
# `Loop` is here because W3b's `SpawnExecution` executor is what completes it,
# and `Subflow` is here since W6 Task 3 for the same shape: `build_plan` gives
# it a `TaskKind::Executable`, `advance()` starts it and waits, and
# `worker::subflow` creates the child execution and reports its terminal back.
# It is NOT a `W3B_HANDLERS` entry, because no worker ever claims it -- a
# `TaskKind::Executable` gets no `task_runs.type_id`.
#
# **The name is no longer only W3a's, and it stopped being so before W6.** It
# is kept because `oracle-exec/README.md` cites it verbatim; what it means is
# "every task type this executor drives itself", which is what both use sites
# below actually ask.
W3A_FLOWABLES = {
    "io.kestra.plugin.core.flow.Sequential",
    "io.kestra.plugin.core.flow.Parallel",
    "io.kestra.plugin.core.flow.Dag",
    "io.kestra.plugin.core.flow.If",
    "io.kestra.plugin.core.flow.Switch",
    "io.kestra.plugin.core.flow.Loop",
    "io.kestra.plugin.core.flow.Subflow",
    # W8 Task 3, the same shape a third time: `build_plan` gives a plain
    # bounded-or-unbounded `Pause` a `TaskKind::Pause`, `advance()` drives the
    # whole of it, and `claim::persist` writes it no `task_runs.type_id` -- so
    # like `Subflow` it is not a `W3B_HANDLERS` entry and no worker ever claims
    # it.
    #
    # **A `Pause` that declares `behavior:`, `onPause:`, its own `tasks:` or an
    # `errors:` is NOT this**, and this set cannot say so: it is keyed by TYPE
    # and that distinction is per TASK (`executor::plan::pause_gap`). The four
    # entries it concerns are all still baselined and all still blocked --
    # `pause-behavior`, `pause_on_pause`,
    # `pause-errors-finally-after-execution` by `finally:`/`afterExecution:`
    # rows that already exist, and `pause-timeout-allow-failure` by its
    # `allowFailure:` row -- so the coarser key costs nothing today. It would
    # cost something the moment one of those four banked, and the `blocked`
    # guard below is what would say so.
    "io.kestra.plugin.core.flow.Pause",
    # The AllowFailure flowable (spec §5.2): `executor::flowable::allow_failure`
    # drives the block, so it is not a handler and no worker claims it. Its
    # `allowFailure:` FLAG on a child is a different construct and stays a row.
    "io.kestra.plugin.core.flow.AllowFailure",
    # WorkingDirectory Task 3: `build_plan` gives it a `TaskKind::WorkingDirectory`
    # and `executor::flowable::working_directory` drives its children and its two
    # claims. Its children are `Runnable`s the worker claims, which is why it is
    # here (a flowable the executor drives) and the children are not.
    "io.kestra.plugin.core.flow.WorkingDirectory",
    # W8 Task 4, and the pair is ONE Kestra class: `LoopUntil.java` carries
    # `@Plugin(aliases = "io.kestra.plugin.core.flow.WaitFor")`, so both
    # spellings now reach `models::TaskKind::LoopUntil` and `build_plan`
    # flattens the children of either. Both are listed because this set is
    # keyed by the string a flow writes, and seven of the ten corpus flows of
    # this construct write the alias.
    #
    # **The same per-TASK caveat the `Pause` note above states applies here**:
    # a `LoopUntil` whose `checkFrequency:` bound is an expression is planned
    # opaque (`executor::plan::loop_until_gap`) and this set cannot say so. No
    # corpus flow writes one -- all nine comparable entries write literal
    # bounds -- so the coarser key costs nothing today.
    "io.kestra.plugin.core.flow.LoopUntil",
    "io.kestra.plugin.core.flow.WaitFor",
}

# Kill-and-exit Task 3: types `advance()` runs itself that are NOT flowables in
# Kestra. `Exit` is an `ExecutionUpdatableTask`; `build_plan` gives it a
# `TaskKind::Exit` and `advance()` decides it (the run, its ancestors and the
# execution), so no worker claims it. Kept apart from `W3A_FLOWABLES` so that
# set keeps meaning what its name says; every reader takes the union
# (`EXECUTOR_DRIVEN`).
#
# Group C2 Task 5 (2026-10-18): `SetVariables`, `UnsetVariables` and `Labels`
# are `ExecutionUpdatableTask`s too; `build_plan` gives each a
# `TaskKind::ExecutionUpdate` and the worker's render pass computes it, so no worker
# claims them either (`spec::registry::EXECUTOR_LEVEL`).
W3A_EXECUTOR_LEVEL = {
    "io.kestra.plugin.core.execution.Exit",
    "io.kestra.plugin.core.execution.SetVariables",
    "io.kestra.plugin.core.execution.UnsetVariables",
    "io.kestra.plugin.core.execution.Labels",
}

EXECUTOR_DRIVEN = W3A_FLOWABLES | W3A_EXECUTOR_LEVEL

# `executor::RunState::ALL`, which is 12 of the golden's 13 states.  A trace
# carrying anything else cannot be represented, whatever the handler does.
# PAUSED and RESTARTED joined it at W8 Task 2 and WARNING at W8 Task 5; the one
# that remains outside is CANCELLED.  This set is a COPY of the Rust enum and so can
# disagree with it -- and it DID, by two variants, until W8 Task 2.  The guard
# is `tests/executor.rs`'s `handler_surfaces_run_state_copy_matches_the_enum`,
# which parses this literal and compares it to `RunState::ALL` in both
# directions.  It is NOT
# `the_state_vocabulary_this_enum_holds_is_derived_from_the_golden`, which this
# comment used to name: that one derives the vocabulary from `RunState::ALL`
# and never reads this file, so it stayed green through the whole staleness.
RUN_STATE = {"CREATED", "SUBMITTED", "RUNNING", "SUCCESS", "FAILED",
             "RETRYING", "RETRIED", "KILLED", "SKIPPED",
             "PAUSED", "RESTARTED", "WARNING", "CANCELLED", "KILLING"}

# `State.Type`'s seventeen, i.e. `models::State`.  Used ONLY by the self-test,
# to pick a state that is real to Kestra and absent from RUN_STATE above --
# see `blocker_rule_too_wide` for why that choice cannot be a literal.
KESTRA_STATES = {"CREATED", "SUBMITTED", "RUNNING", "PAUSED", "RESTARTED",
                 "KILLING", "SUCCESS", "WARNING", "FAILED", "KILLED",
                 "CANCELLED", "QUEUED", "RETRYING", "RETRIED", "SKIPPED",
                 "BREAKPOINT", "RESUBMITTED"}

# Expression markers that read something only a task that RAN can produce.
# The plan's Step 2 names these three.  They are kept apart in the report
# because they are not the same dependency: `outputs.` and `tasksWithState()`
# read ANOTHER task's result, `taskrun.` reads the current run's own metadata.
OUTPUT_MARKERS = (
    ("outputs", re.compile(r"\boutputs\s*[.\[]")),
    ("taskrun", re.compile(r"\btaskrun\s*[.\[]")),
    ("tasksWithState", re.compile(r"\btasksWithState\s*\(")),
)
EXPR = re.compile(r"\{\{.*?\}\}|\{%.*?%\}", re.S)


def expressions(raw):
    """Every pebble expression in a flow source, with whole-line comments cut.

    One corpus comment writes `tasksWithState()` in prose
    (`valids/task-runif-taskswithstate.yml:4`), and counting it would make a
    flow depend on an output because someone explained the flow.
    """
    body = "\n".join(ln for ln in raw.splitlines() if not ln.lstrip().startswith("#"))
    return EXPR.findall(body)


def output_markers(raw):
    hit = set()
    for expr in expressions(raw):
        for name, pattern in OUTPUT_MARKERS:
            if pattern.search(expr):
                hit.add(name)
    return hit


def golden_runs(entry):
    """Every run in a golden entry, top level and inside every sub-execution."""
    out = [(r, False) for r in entry.get("runs") or []]
    for sub in entry.get("spawned") or []:
        out += [(r, True) for r, _ in golden_runs(sub)]
    return out


def golden_states(entry):
    st = set(entry.get("execution") or [])
    for run in entry.get("runs") or []:
        st |= set(run["state"])
        for att in run.get("attempts") or []:
            st |= set(att)
    for sub in entry.get("spawned") or []:
        st |= golden_states(sub)
    return st


def run_blockers(entry, flows, kinds):
    """What the golden RECORDS that a perfect W3b driver still cannot produce.

    Run-driven: it asks what Kestra actually ran, not what the flow happens to
    declare.  A construct declared and never exercised does not block a trace
    that never shows it -- and that distinction is load-bearing, because
    `valids/sla-max-duration-ok` declares an `sla:` that never fires (PT10S over
    a PT0.5S sleep) and reproduces anyway.

    The clause used to read "an `sla:` nothing evaluates".  W8 Task 6 made that
    false for one of `SLA.Type`'s two subtypes -- `EXECUTION_ASSERTION` is
    evaluated now -- and false in a way that would have mattered here, because
    the example is precisely an entry whose blocker rule must NOT fire.  The
    reason it reproduces was never "nothing evaluates an sla", it was "this
    sla is not violated", and that is what the sentence says now.
    """
    out = set()
    fid = entry["id"]
    if entry["verdict"] != "runs":
        return out
    by_id = flows[fid]["by_id"]
    for run, _in_sub in golden_runs(entry):
        info = by_id.get(run["task"])
        if info is None:
            out.add(("construct", "a run of a task declared in ANOTHER flow"))
            continue
        ty, path = info
        if ty not in W3B_HANDLERS and ty not in EXECUTOR_DRIVEN:
            kind = kinds.get(ty, {}).get("kind", "unresolved")
            out.add(("handler" if kind == "RunnableTask" else "construct", ty))
        if "finally" in path and unported_finally(flows[fid]["doc"]):
            out.add(("construct", "finally:"))
        # `afterExecution:` is ported since end hooks #2b Task B1 (2026-10-27):
        # `advance` runs it once the execution is terminal (spec D8).
    for state in sorted(golden_states(entry) - RUN_STATE):
        out.add(("state", f"{state}, which RunState cannot hold"))
    return out


def declared_blockers(entry, flows, kinds):
    """What the flow DECLARES that no W3b task owns.

    Applied only to entries egret does not reproduce today.  For an entry that
    already matches, a declared construct is proven inert by the match itself;
    for one that does not, it is the leading explanation and a ceiling may not
    assume it away.
    """
    out = set()
    fid = entry["id"]
    if entry["verdict"] != "runs":
        return out
    flow = flows[fid]
    doc, keys, raw = flow["doc"], flow["keys"], flow["raw"]
    for _tid, ty, _path in flow["tasks"]:
        if ty in W3B_HANDLERS or ty in EXECUTOR_DRIVEN:
            continue
        kind = kinds.get(ty, {}).get("kind", "unresolved")
        out.add(("handler" if kind == "RunnableTask" else "construct", ty))
    if unported_finally(doc):
        out.add(("construct", "finally:"))
    # `afterExecution:` is not a blocker since end hooks #2b Task B1
    # (2026-10-27): it runs (spec D8).
    if "sla" in doc:
        out.add(("construct", "flow sla:"))
    if "concurrency" in doc:
        out.add(("construct", "flow concurrency:"))
    if "outputs" in doc:
        out.add(("construct", "flow-level outputs:"))
    if "CREATE_NEW_EXECUTION" in raw:
        out.add(("construct", "CREATE_NEW_EXECUTION retry"))
    if re.search(r"allowFailure:\s*true", raw):
        out.add(("construct", "allowFailure:"))
    pause_gaps(doc, out)
    loop_until_gaps(doc, raw, out)
    loop_store_output_gaps(doc, out)
    return out


# The two things a `LoopUntil` can carry that W8 Task 4 implemented CORRECTLY
# and still cannot reproduce.  Both are rows for the same reason the
# `PAUSE_GAP_KEYS` rows are: the construct itself is ported, so `W3A_FLOWABLES`
# stops blocking, and without these the two entries land in the CEILING -- a
# bankable list naming an entry nothing can bank, which this file's own rules
# call the defect that sends the next task hunting.
#
# 1. `checkFrequency.maxDuration`.  `reachedMaximums` compares the loop
#    taskrun's creation plus the bound against `now`, and the golden's iteration
#    count therefore carries the ARTIFACT'S delay-poller latency: on the
#    captured round `valids/loop-until-restart`'s two `PT1S` pauses took 1.17 s
#    and 1.92 s, putting its third iteration past a `PT3S` bound that
#    `driver.rs::pass_time` -- which moves the clock to the earliest deadline
#    and never further -- does not reach until the fourth.  The corroboration
#    that this is the construct and not the entry: `valids/waitfor-max-duration`
#    is one of only two corpus flows Kestra itself could not reproduce across
#    five rounds, so it has no golden entry at all.
# (2. was a relational operator against a QUOTED operand,
#    `valids/loopuntil-failed-flowable`'s `{{ outputs.loop.iterationCount <
#    "5" }}`: Pebble throws and minijinja answered. Package #5a Task A2 (spec
#    D1) made egret's relational operators Kestra's and the entry banked, so
#    the blocker is gone.)
LOOP_UNTIL_TYPES = {
    "io.kestra.plugin.core.flow.LoopUntil",
    "io.kestra.plugin.core.flow.WaitFor",
    "core.loopuntil",
    "core.waitfor",
}
# End hooks A4 fix round (2026-10-27): Pebble's `empty` test (`x is empty`,
# `x is not empty`). egret's renderer leaves it an unknown test on purpose
# (`runners::env`'s fence test), so the expression fails to render where the
# artifact answers.  The corpus has one entry, `valids/task-runif-taskswithstate`,
# whose two flow-level `finally:` `runIf:`s use it; it reached this file's
# CEILING once that `finally:` became ported.
EMPTY_TEST = re.compile(r"\{\{[^{}]*\bis\s+(not\s+)?empty\b")


def loop_until_gaps(node, raw, out):
    """The `LoopUntil` properties W8 Task 4 ported and cannot reproduce."""
    if EMPTY_TEST.search(raw):
        out.add(("render", "Pebble's `empty` test, which egret's renderer "
                           "leaves unknown"))

    def walk(n):
        if isinstance(n, dict):
            if n.get("type") in LOOP_UNTIL_TYPES:
                cf = n.get("checkFrequency")
                if isinstance(cf, dict) and cf.get("maxDuration"):
                    out.add(("clock", "checkFrequency.maxDuration; the golden's "
                                      "iteration count carries the artifact's "
                                      "delay-poller latency"))
            for v in n.values():
                walk(v)
        elif isinstance(n, list):
            for v in n:
                walk(v)

    walk(node)
    return out


LOOP_TYPES = {"io.kestra.plugin.core.flow.Loop", "core.loop"}
# A `values:` read from a FILE: the rendered text of an `outputs.<task>.uri`.
URI_VALUES = re.compile(r"\.uri\b")


def loop_store_output_gaps(node, out):
    """A `Loop` over a FILE that declares its own `outputs:` (W9a Task 11).

    Kestra's `fetchType` AUTO is STORE for values read from a URI, and STORE
    writes one Ion file per iteration at `executions/{iteration}/{n}.ion`;
    egret does not produce them (out of W9a by the user's ruling of
    2026-10-03, spec SS15).  W9a Task 11 made such a `values:` resolve, so
    this, not the render surface, is what blocks `valids/loop-outputs-auto`.
    """
    def walk(n):
        if isinstance(n, dict):
            values = n.get("values")
            if (n.get("type") in LOOP_TYPES and n.get("outputs")
                    and isinstance(values, str) and URI_VALUES.search(values)):
                out.add(("construct", "Loop outputs: over a file (STORE mode, "
                                      "executions/{iteration}/{n}.ion)"))
            for v in n.values():
                walk(v)
        elif isinstance(n, list):
            for v in n:
                walk(v)

    walk(node)
    return out


PAUSE_TYPES = {"io.kestra.plugin.core.flow.Pause", "core.pause"}

# The four `Pause` properties `executor::plan::pause_gap` refuses to plan, in
# its own order.  A `Pause` declaring any of them is planned an opaque
# `TaskKind::Runnable`, so it is exactly as blocked as an unported construct --
# and `W3A_FLOWABLES` above CANNOT say so, because that set is keyed by task
# TYPE and this is a property of one task.
#
# Without these rows the four entries concerned land in "nothing found here
# blocks it", i.e. in the CEILING, which is the one direction this file's own
# rules call a defect: a ceiling that is too low hides work, and a bankable
# list naming an entry nothing can bank is what sends the next task hunting.
PAUSE_GAP_KEYS = ("tasks", "errors", "behavior", "onPause")


def pause_gaps(node, out):
    """Every `Pause` property in this document that `build_plan` will not plan.

    Walks the parsed doc rather than the raw text on purpose: `behavior:` is
    also a `concurrency:` key (`behavior: CANCEL`), and a regex over the source
    would blame six flows that declare no `Pause` at all.
    """
    if isinstance(node, dict):
        if node.get("type") in PAUSE_TYPES:
            for k in PAUSE_GAP_KEYS:
                if node.get(k):
                    out.add(("construct", f"Pause {k}:"))
        for v in node.values():
            pause_gaps(v, out)
    elif isinstance(node, list):
        for v in node:
            pause_gaps(v, out)
    return out


def refusal_blockers(entry, baseline):
    if entry["verdict"] == "refused" and entry["id"] in baseline:
        return {("refusal", "egret over-accepts it -- W1's validation, not a handler")}
    return set()


# --------------------------------------------------------------------------
# The handler output shapes, read from Kestra's own classes
# --------------------------------------------------------------------------
#
# Every citation is `file:line` at checkout 96893b589a, and `--refresh-kinds`
# re-derives the class line for each of the four so a moved class is caught.

HANDLER_SHAPES = [
    {
        "type": "io.kestra.plugin.core.debug.Return",
        "returns": "Return.Output",
        "shape": '{"value": <the rendered `format`, or null>}',
        "cites": [
            ("core/src/main/java/io/kestra/plugin/core/debug/Return.java", 87,
             "class Return extends Task implements RunnableTask<Return.Output>"),
            ("core/src/main/java/io/kestra/plugin/core/debug/Return.java", 99,
             "render = runContext.render(format).as(String.class).orElse(null)"),
            ("core/src/main/java/io/kestra/plugin/core/debug/Return.java", 108,
             "return Output.builder().value(render).build()"),
            ("core/src/main/java/io/kestra/plugin/core/debug/Return.java", 119,
             "private String value;  -- the ONE field of Return.Output"),
        ],
        "notes": [
            "`format` is optional and is rendered with `.orElse(null)`, so a "
            "`Return` with no `format` SUCCEEDS with `value: null`. It does not "
            "fail, and it does not produce an absent output.",
            "It also emits two metrics -- `length` (Counter) and `duration` "
            "(Timer), :105-:106. Nothing in `golden.json` records metrics, so "
            "they are outside what the ratchet can see.",
            "The name says `Return`; the output field is `value`, not `return` "
            "and not `format`. `{{ outputs.<id>.value }}` is what six corpus "
            "flows write.",
        ],
    },
    {
        "type": "io.kestra.plugin.core.log.Log",
        "returns": "VoidOutput",
        "shape": "no output at all -- `run` returns Java `null`",
        "cites": [
            ("core/src/main/java/io/kestra/plugin/core/log/Log.java", 68,
             "class Log extends Task implements RunnableTask<VoidOutput>"),
            ("core/src/main/java/io/kestra/plugin/core/log/Log.java", 109,
             "return null;  -- not `new VoidOutput()`"),
            ("core/src/main/java/io/kestra/core/models/tasks/VoidOutput.java", 3,
             "class VoidOutput implements Output { }  -- no fields"),
            ("core/src/main/java/io/kestra/plugin/core/log/Log.java", 79,
             "private Object message;  -- a String OR a Collection<String>"),
            ("core/src/main/java/io/kestra/plugin/core/log/Log.java", 85,
             "level defaults to Property.ofValue(Level.INFO)"),
            ("core/src/main/java/io/kestra/plugin/core/log/Log.java", 106,
             "message of any other type throws IllegalArgumentException"),
        ],
        "notes": [
            "`run` returns `null`, so `outputs.<id>` is ABSENT rather than an "
            "empty map. An expression reading `{{ outputs.<log-id>.anything }}` "
            "is a render failure in Kestra, not a null.",
            "`message` is `@NotNull` and accepts a list; 1 corpus flow writes a "
            "list. A handler that accepts only a String fails those flows at "
            "run time, not at publish.",
        ],
    },
    {
        "type": "io.kestra.plugin.core.execution.Fail",
        "returns": "VoidOutput",
        "shape": "no output; it throws -- OR returns null and SUCCEEDS",
        "cites": [
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 118,
             "class Fail extends Task implements RunnableTask<VoidOutput>"),
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 123,
             "private Property<String> condition;  -- OPTIONAL"),
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 127,
             'errorMessage defaults to Property.ofValue("Task failure")'),
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 135,
             'condition truthy -> throw new Exception("Fail on a condition")'),
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 137,
             "condition FALSY -> return null;  -- the task SUCCEEDS"),
            ("core/src/main/java/io/kestra/plugin/core/execution/Fail.java", 140,
             "no condition -> throw new Exception(<rendered errorMessage>)"),
            ("core/src/main/java/io/kestra/core/utils/TruthUtils.java", 6,
             'FALSE_VALUES = List.of("false", "0", "-0", "")  -- trimmed, :12'),
        ],
        "notes": [
            "**A `Fail` does not always fail.** With a `condition:` that renders "
            "falsy it returns null and the run is SUCCESS. A handler written as "
            "`Fail always throws` is wrong on every flow that uses the "
            "condition form.",
            "The two throwing paths carry DIFFERENT messages: with a condition "
            "the thrown message is the literal `Fail on a condition` and the "
            "rendered `errorMessage` is only LOGGED (:134); with no condition "
            "the rendered `errorMessage` is the thrown message (:140).",
            "Nothing in `golden.json` records a failure message, so the ratchet "
            "cannot see which of the two a handler produces. That is a hole "
            "this package's own tests have to cover, not the oracle.",
        ],
    },
    {
        "type": "io.kestra.plugin.core.flow.Sleep",
        "returns": "VoidOutput",
        "shape": "no output at all -- `run` returns Java `null`",
        "cites": [
            ("core/src/main/java/io/kestra/plugin/core/flow/Sleep.java", 47,
             "class Sleep extends Task implements RunnableTask<VoidOutput>"),
            ("core/src/main/java/io/kestra/plugin/core/flow/Sleep.java", 52,
             "@NotNull private Property<Duration> duration;  -- and NOTHING else"),
            ("core/src/main/java/io/kestra/plugin/core/flow/Sleep.java", 56,
             "runContext.render(this.duration).as(Duration.class).orElseThrow()"),
            ("core/src/main/java/io/kestra/plugin/core/flow/Sleep.java", 62,
             "return null;"),
        ],
        "notes": [
            "It is a `RunnableTask` despite living in `core.flow` -- a HANDLER, "
            "not a flowable. Classifying it by module is what this file's "
            "population figure had to correct.",
            "**The brief's `PT0S is refused by the artifact` is NOT visible in "
            "this class.** `duration` carries `@NotNull` (:52) and no positivity "
            "constraint, and `TimeUnit.MILLISECONDS.sleep(0)` does not throw. If "
            "the refusal is real it comes from somewhere this task did not find, "
            "and no corpus flow writes `PT0S` -- so nothing here can settle it. "
            "Task 3 should measure it against the running artifact before "
            "implementing a positivity check egret's users would then be held to.",
        ],
    },
]


# --------------------------------------------------------------------------
# The report
# --------------------------------------------------------------------------

def load_inputs():
    flows = read_corpus()
    kinds, checkout = load_kinds()
    with open(GOLDEN, encoding="utf-8") as f:
        golden = json.load(f)
    baseline = set()
    with open(BASELINE, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                baseline.add(line)
    return flows, kinds, golden, baseline, checkout


def check_inputs(flows, kinds, golden, baseline):
    """Everything downstream reads a number off these; they are checked first."""
    if len(golden) != 301:
        bail(f"golden.json holds {len(golden)} entries, not 301. The comparable "
             f"population moved and every count in handler-surface.txt with it")
    runs = [e for e in golden if e["verdict"] == "runs"]
    if len(runs) != 273:
        bail(f"golden.json holds {len(runs)} `runs` entries, not 273")
    # 143 through W5; 131 after W6 Task 3 banked the 12 `Subflow` entries; 127
    # after W6 Task 4 banked the four `retry-new-execution-*` replays; 119 after
    # W8 Task 3 banked eight `Pause` entries; 118 after W8 Task 4 banked
    # `valids/waitfor-nested`; 111 after W8 Task 5 banked seven -- the three
    # `valids/task-allow-warning-*`, the three `valids/task-allow-failure-*`
    # and `valids/pause-timeout-allow-failure`; 109 since W8 Task 6 banked the
    # two `sla:` entries whose `EXECUTION_ASSERTION` cancels the execution,
    # `valids/flow-concurrency-sla-assert-cancel` and
    # `valids/sla-execution-condition`; 108 since W9a Task 11 banked
    # `valids/loop-values-from-uri` (a `Loop` over an ION file); 107 since
    # no-silent-stall Task 1 banked `invalids/invalid-task` (egret refuses an
    # unknown task type now, as Kestra does); 104 since the AllowFailure
    # flowable banked `valids/allow-failure`, `valids/allow-failure-with-retry`
    # and `valids/finally-allowfailure`; 103 since WorkingDirectory Task 8 banked
    # `valids/working-directory-taskrun`, whose Kestra trace egret now reproduces;
    # 104 since the same WorkingDirectory pass baselined
    # `valids/retry-failed-task-duration` (Kestra's ~0.6 s per-retry scheduler
    # latency lets it stop at three attempts where egret makes four); 100 since
    # kill-and-exit Task 3 banked the four `valids/exit*` entries (the
    # executor runs `io.kestra.plugin.core.execution.Exit`); 89 since group C1
    # Task 2 banked the eleven entries `OutputValues` held (it has a handler);
    # 88 since group C1 Task 3 banked `valids/assert` (`Assert` has a handler);
    # 84 since group C1 Task 4 banked four `valids/purge_logs_*` (`PurgeLogs`
    # has a handler; `purge_logs_full_arguments` stays, by the scope rule);
    # 73 since group C2 Task 5 banked eleven entries `SetVariables`,
    # `UnsetVariables` and `Labels` held (executor-level now;
    # `task-runif-taskswithstate` stays, on the flow-level `finally:`).
    # 72 since no-silent-drops Task 8b (2026-10-26) banked
    # `invalids/invalid-property` (refused at save, `Seam::Save`).
    # 68 since end hooks A4 (2026-10-27) banked `valids/finally-dag`,
    # `valids/finally-parallel`, `valids/finally-sequential` and
    # `valids/finally-sequential-error` (flowable `finally:` runs); 63 since
    # its fix round (the same day) banked `valids/finally-flow`,
    # `-flow-error`, `-flow-error-first`, `-sequential-error-first` and
    # `valids/restart-with-finally` (the worker finds a flow-level `finally:`
    # leaf). 60 since end hooks A6 (2026-10-27) banked
    # `invalids/loop-switch-failed`, `valids/finally-loop` and
    # `valids/finally-loop-parallel` (a `Loop` iteration runs the Loop's own
    # `errors:`/`finally:`). 52 since end hooks #2b Task B1 (2026-10-27)
    # banked `valids/after-execution-error`, `-finally`, `-flowable`,
    # `valids/flow-concurrency-queue-after-execution`,
    # `valids/flow-concurrency-queue-killed-after-execution`,
    # `valids/restart-with-after-execution`, `valids/sleep-long` and
    # `valids/sleep-long-after-execution-flowable` (`afterExecution:` runs
    # once the execution is terminal). 50 since workflow-inputs Task A6
    # (2026-10-28) banked `invalids/inputs-bad-validator-syntax` (refused at
    # save, `Seam::Save`) and `invalids/inputs-with-multiple-constraint-
    # violations` (refused at start, the new `Seam::Start`). 49 since package
    # #5a Task A2 (2026-10-28) banked `valids/loopuntil-failed-flowable` (a
    # relational operator against a quoted operand now fails as Kestra's does).
    # 45 since package #5a Task A3 (2026-10-28) banked `valids/restart-child`,
    # `-loop`, `-parent` and `-parent-loop` (`1 / 0` fails as Java's does).
    # 44 since package #5a Task A5 (2026-10-29) banked `valids/webhook-plaintext`
    # (`??` reads through a missing member chain, spec D6).
    # 42 since package #5a Task A6 (2026-10-29) banked `valids/if-in-flowable`
    # (`equals`) and `valids/task-runif-workingdirectory` (`contains`), spec D5.
    if len(baseline) != 42:
        bail(f"baseline.txt holds {len(baseline)} ids, not 42; the split "
             f"between what egret reproduces and what it does not has moved")
    missing = sorted(e["id"] for e in golden if e["id"] not in flows)
    if missing:
        bail(f"golden.json names {len(missing)} flows corpus/ does not hold, "
             f"e.g. {missing[:3]}")
    stray = sorted(i for i in baseline if not any(e["id"] == i for e in golden))
    if stray:
        bail(f"baseline.txt names {len(stray)} ids golden.json does not compare, "
             f"e.g. {stray[:3]}")
    # The parser, validated against Kestra rather than trusted.
    unknown = []
    for entry in runs:
        by_id = flows[entry["id"]]["by_id"]
        for run in entry.get("runs") or []:
            if run["task"] not in by_id:
                unknown.append((entry["id"], run["task"]))
    if unknown:
        bail(f"{len(unknown)} runs Kestra recorded name a task the YAML subset "
             f"parser did not find, e.g. {unknown[:3]}. The parser has lost part "
             f"of a flow and every population figure below is short by whatever "
             f"it lost")
    # Every task type a comparable flow declares must have a resolved kind, or
    # be a test-source class the pinned image never had.
    unresolved = sorted({ty for e in golden for _i, ty, _p in flows[e["id"]]["tasks"]
                         if ty not in kinds})
    # A type declared by a flow Kestra RAN must exist -- Kestra instantiated it.
    # A type declared only by a flow Kestra REFUSED may not: that is what
    # `invalids/invalid-task` is a test of, and its two types
    # (`core.debug.MissingTwo`, `MissingThree`) are declared nowhere in the
    # checkout. So the guard is over the `runs` half only; over all 294 it fires
    # on the corpus as committed and would have to be weakened to something that
    # never fires at all.
    #
    # Two types are exempt, and each is exempt for a reason the checkout states:
    #   `io.kestra.plugin.scripts.shell.Commands` is a shell plugin task whose
    #     class is in a plugin artifact, not in this core checkout (`git ls-files`
    #     holds no `scripts/shell/Commands.java`); the pinned image loads it.
    #   `io.kestra.plugin.scripts.runner.docker.Docker` is a `taskRunner:` type,
    #     declared as a `TaskRunner` (script/.../runner/docker/Docker.java), not a
    #     task, so it has no task kind. The corpus parser reads every `type:`
    #     line, the runner's included.
    # Added at WorkingDirectory Task 8, when the six `working-directory-real-*`
    # flows became the first corpus flows to run a script task.
    OUTSIDE_THE_CHECKOUT = {
        "io.kestra.plugin.scripts.shell.Commands",
        "io.kestra.plugin.scripts.runner.docker.Docker",
    }
    ran = sorted({ty for e in runs for _i, ty, _p in flows[e["id"]]["tasks"]
                  if ty not in kinds and ty not in OUTSIDE_THE_CHECKOUT})
    if ran:
        bail(f"{len(ran)} task types are declared by a flow Kestra RAN and have "
             f"no entry in task-kinds.json, e.g. {ran[:3]}; run --refresh-kinds")
    return runs, unresolved


def derive(flows, kinds, golden, baseline):
    runs, unresolved = check_inputs(flows, kinds, golden, baseline)
    d = {"runs": runs, "unresolved": unresolved}

    # -- the type census, by kind -------------------------------------------
    entries_with = Counter()
    occurrences = Counter()
    for entry in runs:
        seen = set()
        for _tid, ty, _p in flows[entry["id"]]["tasks"]:
            occurrences[ty] += 1
            seen.add(ty)
        for ty in seen:
            entries_with[ty] += 1
    d["entries_with"] = entries_with
    d["occurrences"] = occurrences

    def kind_of(ty):
        return kinds.get(ty, {}).get("kind", "not-in-the-checkout")

    d["kind_of"] = kind_of

    # -- the population ------------------------------------------------------
    population, others = [], []
    for entry in runs:
        types = {ty for _i, ty, _p in flows[entry["id"]]["tasks"]}
        non_flowable = {ty for ty in types if kind_of(ty) != "FlowableTask"}
        (population if non_flowable <= W3B_HANDLERS else others).append(entry["id"])
    d["population"] = population
    d["outside_population"] = others
    if len(population) + len(others) != len(runs):
        bail(f"the population split does not account for every comparable `runs` "
             f"entry: {len(population)} + {len(others)} != {len(runs)}")

    # -- the plan's 194, reconstructed --------------------------------------
    # A quick textual pass sweeps EVERY `type:` line, `triggers:` included, and
    # allows anything under `core.flow.*` -- classifying by module.  Both errors
    # are reproduced here so the difference is a derivation and not a story.
    type_line = re.compile(r"^\s*-?\s*type:\s*(\S+)\s*$", re.M)
    by_module = W3B_HANDLERS | {ty for ty in occurrences
                                if ty.startswith("io.kestra.plugin.core.flow.")}
    naive = [e["id"] for e in runs
             if {t for t in type_line.findall(flows[e["id"]]["raw"]) if "." in t} <= by_module]
    d["naive"] = naive
    d["naive_only"] = sorted(set(naive) - set(population))
    d["population_only"] = sorted(set(population) - set(naive))
    d["naive_only_types"] = Counter()
    for i in d["naive_only"]:
        for ty in {t[1] for t in flows[i]["tasks"]}:
            if ty not in W3B_HANDLERS and kind_of(ty) != "FlowableTask":
                d["naive_only_types"][ty] += 1

    # -- the outputs dependency ---------------------------------------------
    marker_of = {i: output_markers(flows[i]["raw"]) for i in population}
    d["marker_of"] = marker_of
    d["depends"] = sorted(i for i in population if marker_of[i])
    d["independent"] = sorted(i for i in population if not marker_of[i])
    if len(d["depends"]) + len(d["independent"]) != len(population):
        bail(f"the outputs split does not account for every member of the "
             f"population: {len(d['depends'])} + {len(d['independent'])} != "
             f"{len(population)}")
    d["by_marker"] = Counter()
    for i in population:
        for m in marker_of[i]:
            d["by_marker"][m] += 1
    d["foreign_only"] = sorted(i for i in population
                               if marker_of[i] & {"outputs", "tasksWithState"})
    d["two_markers"] = sorted(i for i in population if len(marker_of[i]) > 1)

    # -- the ceiling ---------------------------------------------------------
    safe, at_risk, bankable, blocked = [], {}, [], {}
    for entry in golden:
        eid = entry["id"]
        if eid in baseline:
            b = (run_blockers(entry, flows, kinds)
                 | declared_blockers(entry, flows, kinds)
                 | refusal_blockers(entry, baseline))
            if b:
                blocked[eid] = sorted(b)
            else:
                bankable.append(eid)
        else:
            # An entry that MATCHES today has proven every construct it declares
            # inert.  What the real driver takes away from it is the FEED: the
            # harness supplies the terminal of every `Runnable` run from the
            # golden, so an entry whose runs include a task type W3b has no
            # handler for matches today for a reason that is about to go away.
            b = run_blockers(entry, flows, kinds)
            if b:
                at_risk[eid] = sorted(b)
            else:
                safe.append(eid)
    d.update(safe=safe, at_risk=at_risk, bankable=bankable, blocked=blocked)

    if len(safe) + len(at_risk) + len(bankable) + len(blocked) != len(golden):
        bail(f"the ceiling's four buckets do not account for every comparable "
             f"entry: {len(safe)} + {len(at_risk)} + {len(bankable)} + "
             f"{len(blocked)} != {len(golden)}")
    if len(safe) + len(at_risk) != len(golden) - len(baseline):
        bail(f"safe + at-risk must be the entries egret reproduces today: "
             f"{len(safe)} + {len(at_risk)} != {len(golden) - len(baseline)}")
    if len(bankable) + len(blocked) != len(baseline):
        bail(f"bankable + blocked must be the baseline: {len(bankable)} + "
             f"{len(blocked)} != {len(baseline)}")
    for eid, b in at_risk.items():
        if not all(cause == "handler" for cause, _ in b):
            bail(f"{eid} matches today and is blocked by {b}. A construct or a "
                 f"state on an entry that ALREADY reproduces means the blocker "
                 f"rules are wrong, not that the entry is at risk -- only the "
                 f"fed terminal of an unhandled task type can be taken away by "
                 f"re-pointing the oracle")
    d["ceiling_feed_removed"] = len(safe) + len(bankable)
    d["ceiling_feed_kept"] = len(safe) + len(at_risk) + len(bankable)
    # W3a's own named remainder, and where each of its members lands here.
    # Derived rather than described: the first draft of this section said four
    # of them were bankable and one of the four was not.
    d["named_here"] = []
    for eid, what in W3A_NAMED_REMAINDER:
        if eid in bankable:
            where = "BANKABLE"
        elif eid in blocked:
            where = "blocked: " + "; ".join(f"{c}/{w}" for c, w in blocked[eid])
        elif eid in at_risk:
            where = "at risk"
        else:
            where = "reproduced today"
        d["named_here"].append((eid, what, where))
    d["full_keys"] = sorted(flows["valids/full"]["doc"].keys())
    # Last, deliberately: it is the narrowest guard here and every earlier one
    # is about a bigger quantity. A guard that fires FIRST on an unrelated
    # mutation is a guard whose self-test case is really someone else's.
    no_trigger = [i for i in d["population_only"] if "triggers" not in flows[i]["doc"]]
    if no_trigger:
        bail(f"{len(no_trigger)} entries the naive pass drops declare no "
             f"`triggers:`, e.g. {no_trigger[:3]}; the reconciliation section of "
             f"handler-surface.txt says every one of them does")
    return d


# W3a's own named remainder -- the baselined entries no construct explains,
# from `tests/executor.rs::the_residual_baseline_is_attributed_to_a_construct_or_named`
# at bcf4ab7a. Quoted, because two of its causes -- an unreachable ordered pair
# and an output the handler cannot produce -- are invisible in a flow's YAML and
# in the golden's states, so this derivation cannot find them on its own. Where
# each one LANDS here is derived.
W3A_NAMED_REMAINDER = [
    ("invalids/dag-invalid-subtask", "over-accepted: a @NotNull on a plugin property"),
    ("invalids/inputs-bad-validator-syntax", "over-accepted: a java.util.regex dialect"),
    ("invalids/inputs-with-multiple-constraint-violations",
     "over-accepted: refused at a different seam"),
    ("invalids/invalid-property", "over-accepted: an unrecognised plugin property"),
    ("invalids/invalid-task", "over-accepted: a type not on Kestra's classpath"),
    ("valids/dag", "one `order` pair a clockless machine cannot reach"),
    ("valids/flow-with-outputs-failed", "flow-level `outputs:`, which nothing evaluates"),
    ("valids/full", "flow-level `outputs:` and `variables:`, same"),
    ("valids/if", "three `order` pairs, same cause"),
    ("valids/parallel-disabled-tasks", "one `order` pair, same cause"),
    ("valids/retry-failed-flow-duration", "maxDuration with no elapsed time"),
    ("valids/retry-success", "WARNING, which RunState cannot hold"),
    ("valids/subflow-secret-output-child", "a SECRET output; no encryption key"),
]


def by_cause(blocked):
    counts = Counter()
    for causes in blocked.values():
        for cause, what in causes:
            counts[(cause, what)] += 1
    return counts


def render(d, checkout):
    L = []
    w = L.append
    runs = d["runs"]
    kind_of = d["kind_of"]

    w("W3b Task 1 -- the handler surface, and the ceiling it puts on this package")
    w("=" * 78)
    w("")
    w("GENERATED by `python3 handler_surface.py`. Do not hand-edit: every number")
    w("below is a function of corpus/, golden.json, baseline.txt and")
    w(f"task-kinds.json (Kestra checkout {(checkout or '?')[:9]}), and")
    w("`--check` fails if this file and those inputs disagree.")
    w("")
    w("The question: the corpus was harvested from a Kestra that RAN tasks, and")
    w("egret's handlers are not Kestra's. Wherever the golden's outcome depends")
    w("on what a task DID rather than that it ran, `the handler ran` is not")
    w("enough. This file says how far four handlers can possibly go.")
    w("")

    # ---- 1. the census ----------------------------------------------------
    w("")
    w("1. EVERY TASK TYPE THE 273 COMPARABLE `runs` ENTRIES DECLARE")
    w("-" * 78)
    w("")
    w("Kind is the Kestra interface the type's class implements, resolved")
    w("transitively from the checkout -- not guessed from the package. A")
    w("`RunnableTask` needs a HANDLER; a `FlowableTask` is the executor's own")
    w("work; an `ExecutableTask` spawns another execution; an")
    w("`ExecutionUpdatableTask` mutates the execution and runs nowhere.")
    w("")
    w(f"  {'entries':>7}  {'runs':>5}  {'kind':<24} type")
    types = sorted(d["entries_with"], key=lambda t: (-d["entries_with"][t], t))
    for ty in types:
        mark = "*" if ty in W3B_HANDLERS else " "
        w(f"  {d['entries_with'][ty]:7}  {d['occurrences'][ty]:5}  "
          f"{kind_of(ty):<24} {mark}{ty}")
    w(f"  {'-'*7}  {'-'*5}")
    w(f"  {'TYPES':>7}  {len(types):5}  distinct task types over {len(runs)} entries")
    w(f"  {'TASKS':>7}  {sum(d['occurrences'].values()):5}  task declarations in those entries")
    w("")
    w("  * = a handler W3b writes.")
    w("")
    w("`entries` is the number of comparable `runs` entries that declare the")
    w("type at least once; the rows OVERLAP and do not sum to 273. `runs` is")
    w("raw declarations and does sum.")
    if d["unresolved"]:
        w("")
        w("Types with no entry in task-kinds.json -- the checkout does not")
        w("declare them, which is the answer for a test-source class:")
        for ty in d["unresolved"]:
            w(f"  - {ty}")
    w("")
    spec3 = {"io.kestra.plugin.core.debug.Return": 115,
             "io.kestra.plugin.core.log.Log": 82,
             "io.kestra.plugin.core.execution.Fail": 55,
             "io.kestra.plugin.core.output.OutputValues": 23,
             "io.kestra.plugin.core.flow.Sleep": 20}
    disagree = {t: (n, d["entries_with"][t]) for t, n in spec3.items()
                if d["entries_with"][t] != n}
    w("The spec's SS3 table is these five rows:")
    for ty, n in sorted(spec3.items(), key=lambda kv: -kv[1]):
        w(f"    {n:5}  {ty}  ->  {d['entries_with'][ty]} here")
    if disagree:
        w("")
        w(f"  {len(disagree)} of the five DISAGREE with this derivation, and the")
        w("  disagreement is the finding: re-derive before quoting either.")
    else:
        w("")
        w("  All five reproduce exactly, which is what makes this generator's")
        w("  YAML reader evidence rather than a second opinion.")
    w("")
    tally = Counter(kind_of(t) for t in types)
    w("  What SS3 did NOT carry is the rest of the column:")
    for k, n in sorted(tally.items(), key=lambda kv: (-kv[1], kv[0])):
        extra = n - sum(1 for t in spec3 if kind_of(t) == k)
        w(f"    {n:5}  {k}  ({extra} the spec's table does not name)")
    w("")
    w("  A real driver meets every one of them; a table of the top five hides")
    w("  them, and that is the shape trap 3 of this task's brief names.")
    w("")

    # ---- 2. the population -------------------------------------------------
    w("")
    w("2. THE POPULATION -- ENTRIES WHOSE NON-FLOWABLE TASKS ARE ONLY THE FOUR")
    w("-" * 78)
    w("")
    w("Definition, stated because the answer depends entirely on it: a")
    w("comparable `runs` entry is IN the population when every task type it")
    w("declares whose Kestra class is not a `FlowableTask` is one of")
    w("`core.log.Log`, `core.debug.Return`, `core.execution.Fail`,")
    w("`core.flow.Sleep`.")
    w("")
    w(f"  {len(d['population']):5}  in the population")
    w(f"  {len(d['outside_population']):5}  declare some other non-flowable task")
    w(f"  {'-'*5}")
    w(f"  {len(d['population']) + len(d['outside_population']):5}  TOTAL, the comparable `runs` entries")
    w("")
    w("  Reconciling with the plan's 194")
    w("  " + "-" * 32)
    w("")
    w("  The plan's Step 1 says 194 and asks for it to be re-derived. It is")
    w(f"  {len(d['population'])}. The 194 is reproducible and it comes from two errors in the")
    w("  same quick pass, both of which this file has to avoid:")
    w("")
    w("  (a) it swept `triggers:` in. A textual pass over `type:` lines cannot")
    w("      tell a trigger from a task, and a trigger is not a task: no handler")
    w("      ever runs one.")
    w("  (b) it classified by MODULE. Everything under `core.flow.*` counted as")
    w("      flowable -- which is what makes `core.flow.Sleep` look like a")
    w("      flowable when it is a `RunnableTask` (Sleep.java:47), and what lets")
    w("      `Subflow`, `WorkingDirectory`, `Pause`, `AllowFailure` and")
    w("      `LoopUntil` in. Measured, only one of the five changes the answer.")
    w("")
    w("  Reproducing both errors over the same corpus gives:")
    w("")
    w(f"    {len(d['naive']):5}  entries whose every `type:` line, triggers included, is one")
    w("           of the four or lives under `core.flow.*`")
    w(f"    {len(d['naive_only']):5}  of those are NOT in the population above")
    w(f"    {len(d['population_only']):5}  population members the naive pass drops")
    w(f"    {len(d['naive']) - len(d['naive_only']) + len(d['population_only']):5}  = "
      f"{len(d['naive'])} - {len(d['naive_only'])} + {len(d['population_only'])}, the population")
    w("")
    w(f"  Every one of the {len(d['population_only'])} the naive pass drops declares a `triggers:` --")
    w("  derived, and the generator refuses to write this file if one does not.")
    w(f"  The {len(d['naive_only'])} it admits are the module classification's, and this is what")
    w("  they declare that a handler cannot run (the rows overlap):")
    w("")
    for ty, n in sorted(d["naive_only_types"].items(), key=lambda kv: (-kv[1], kv[0])):
        w(f"    {n:5}  {kind_of(ty):<24} {ty}")
    w("")
    w("  Believe this file's figure over the plan's, and note WHICH way the")
    w("  error ran: the brief's 194 is not conservative, it is a different set.")
    w(f"  It admits {len(d['naive_only'])} entries the population excludes and excludes {len(d['population_only'])} it admits.")
    w("")

    # ---- 3. the outputs dependency ----------------------------------------
    w("")
    w("3. HOW MANY OF THEM NEED AN OUTPUT, NOT JUST A RUN")
    w("-" * 78)
    w("")
    w("An expression that reads `{{ outputs... }}`, `{{ taskrun... }}` or")
    w("`tasksWithState()` needs a task to have DONE something. `inputs`, `vars`,")
    w("`item`, `flow`, `execution`, `trigger` and `labels` do not. Whole-line")
    w("comments are cut before scanning -- one corpus comment writes")
    w("`tasksWithState()` in prose.")
    w("")
    w(f"  {len(d['depends']):5}  read at least one of the three")
    w(f"  {len(d['independent']):5}  read none of them")
    w(f"  {'-'*5}")
    w(f"  {len(d['depends']) + len(d['independent']):5}  TOTAL, the population")
    w("")
    w("  By marker. The rows CAN overlap -- one flow may write both -- and")
    if d["two_markers"]:
        w(f"  {len(d['two_markers'])} of them do, so these do not sum to {len(d['depends'])}:")
    else:
        w(f"  measured, NONE does, so they sum to {len(d['depends'])} exactly:")
    w("")
    for name, _p in OUTPUT_MARKERS:
        w(f"    {d['by_marker'][name]:5}  {name}")
    w(f"    {'-'*5}")
    w(f"    {sum(d['by_marker'].values()):5}  TOTAL")
    w("")
    w("  The three are NOT one dependency, and lumping them overstates the")
    w("  problem. `taskrun.` reads the CURRENT run's own metadata -- its id,")
    w("  `startDate`, `value`, `attemptsCount` -- which the driver knows without")
    w("  any handler producing anything, and it is the overwhelming majority")
    w("  here. `outputs.` and `tasksWithState()` read ANOTHER task's result, and")
    w("  those are the entries where an exact handler output is load-bearing:")
    w("")
    w(f"    {len(d['foreign_only']):5}  read another task's result (`outputs.` or `tasksWithState()`)")
    w("")
    w("  This is what W3c's `runIf:` fix banks nothing against: W3a's harness")
    w("  render context reaches `inputs` and `vars` and stops. The real driver")
    w("  has outputs, so these entries become live -- and spec SS4.4 is right that")
    w("  every one of them must be attributed to the render surface rather than")
    w("  to durability. (`loop-outputs-auto` was named here until W9a Task 11:")
    w("  its `values:` resolves now, and what blocks it is the Loop's STORE")
    w("  `outputs:`, which W9a leaves out -- section 3's construct list.)")
    w("")

    # ---- 4. the handler shapes --------------------------------------------
    w("")
    w("4. WHAT EACH HANDLER MUST PRODUCE, FROM KESTRA'S OWN CLASS")
    w("-" * 78)
    w("")
    w(f"All citations are file:line at checkout {(checkout or '?')[:9]}.")
    for h in HANDLER_SHAPES:
        w("")
        w(f"  {h['type']}")
        w(f"    returns : {h['returns']}")
        w(f"    output  : {h['shape']}")
        for path, line, what in h["cites"]:
            w(f"    {path}:{line}")
            w(f"        {what}")
        for note in h["notes"]:
            w("")
            for i, chunk in enumerate(_wrap(note, 68)):
                w(f"    {'- ' if i == 0 else '  '}{chunk}")
    w("")
    w("  Three of the four produce NOTHING, and that is the useful half of this")
    w("  section: `outputs.<id>` is absent for a `Log`, a `Fail` and a `Sleep`,")
    w("  not an empty map. Only `Return` puts anything in the render context,")
    w("  and the field is `value`.")
    w("")

    # ---- 5. the ceiling ---------------------------------------------------
    w("")
    w("5. THE CEILING")
    w("-" * 78)
    w("")
    w("How many of the 301 comparable entries could agree with the golden if")
    w("W3b's driver were perfect and its four handlers exact. It is an UPPER")
    w("BOUND and no later task may report above it without moving it here.")
    w("")
    w("Two rules, and they are deliberately different:")
    w("")
    w("  - For an entry egret ALREADY reproduces, a construct it declares is")
    w("    proven inert by the match itself. What re-pointing the oracle takes")
    w("    away from it is the FEED -- W3a's harness supplies the terminal of")
    w("    every `Runnable` run from the golden (575 of the golden's 840 run")
    w("    terminals; see *What the oracle feeds in*). An entry whose runs")
    w("    include a task type W3b writes no handler for is reproduced TODAY by")
    w("    the feed and by nothing else.")
    w("  - For an entry egret does NOT reproduce, a declared construct no W3b")
    w("    task owns is the leading explanation and a ceiling may not assume it")
    w("    away.")
    w("")
    w("  The second rule is deliberately STRICT, and the direction of its error")
    w("  is the point. A construct declared and never exercised blocks an entry")
    w("  that does not match today, so this number can be too LOW and cannot be")
    w("  too high. A ceiling that is too low costs a later task one edit to this")
    w("  file, with the entry named and the reason given; a ceiling that is too")
    w("  high costs nothing at all, which is why it would not be a ceiling.")
    w("")
    w(f"  {len(d['safe']):5}  reproduced today, and by nothing the driver removes")
    w(f"  {len(d['at_risk']):5}  reproduced today ONLY because the feed supplies a terminal")
    w("         for a task type W3b has no handler for")
    w(f"  {len(d['bankable']):5}  not reproduced today, and nothing found here blocks it")
    w(f"  {len(d['blocked']):5}  not reproduced today, and blocked -- by cause below")
    w(f"  {'-'*5}")
    w(f"  {len(d['safe']) + len(d['at_risk']) + len(d['bankable']) + len(d['blocked']):5}  TOTAL, the comparable entries")
    w("")
    w(f"  CEILING, feed removed  = {len(d['safe'])} + {len(d['bankable'])} = "
      f"**{d['ceiling_feed_removed']}**")
    w(f"  CEILING, feed retained = {len(d['safe'])} + {len(d['at_risk'])} + "
      f"{len(d['bankable'])} = {d['ceiling_feed_kept']}")
    w("")
    w(f"  Today the ratchet matches {len(d['safe']) + len(d['at_risk'])}. The two ceilings are the same")
    w("  quantity under the two answers Task 5 may give to `does the driver")
    w("  still feed a terminal for a task type it has no handler for?`. They are")
    w("  NOT the same set with a different name, and the composition is printed")
    w("  above so a reader cannot take one for the other.")
    w("")
    w(f"  **{d['ceiling_feed_removed']} is the bound this package should be held to**, because removing")
    w("  the feed is the stated intent of Task 5 step 4. If Task 5 keeps the")
    w("  feed for unhandled types it must say so, and then the bound is")
    w(f"  {d['ceiling_feed_kept']} -- reported as a decision, not as progress.")
    w("")
    w(f"  The {len(d['at_risk'])} at risk, with the type whose terminal is fed:")
    for eid, b in sorted(d["at_risk"].items()):
        w(f"    {eid:<52} {', '.join(what.rsplit('.', 1)[-1] for _c, what in b)}")
    w("")
    w(f"  The {len(d['bankable'])} nothing here blocks:")
    for eid in sorted(d["bankable"]):
        w(f"    {eid}")
    w("")
    w("  What blocks the rest, by cause. The rows OVERLAP -- an entry may be")
    w("  blocked several times over -- so they do not sum to the bucket:")
    w("")
    counts = by_cause(d["blocked"])
    for (cause, what), n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
        w(f"    {n:5}  {cause:<10} {what}")
    w(f"    {'-'*5}")
    w(f"    {len(d['blocked']):5}  DISTINCT entries blocked (the rows above overlap)")
    w("")
    w("  The four causes the plan names, and where each landed:")
    w("")
    for cause, label in (("handler", "a missing handler"),
                         ("construct", "a construct no W3b task owns"),
                         ("state", "a state RunState cannot hold"),
                         ("refusal", "an over-acceptance -- W1's validation")):
        n = len({e for e, b in d["blocked"].items() if any(c == cause for c, _ in b)})
        w(f"    {n:5}  {label}")
    w("")
    w("  `an unreachable ordered pair` and `an output the handler cannot")
    w("  produce` are the two causes the plan names that this derivation CANNOT")
    w("  see: neither is visible in a flow's YAML or in the golden's states. W3a")
    w("  measured both, in")
    w("  `tests/executor.rs::the_residual_baseline_is_attributed_to_a_construct_or_named`.")
    w("  Its thirteen named entries, with where each one lands HERE -- derived,")
    w("  because the first draft of this paragraph named four bankable and one")
    w("  of the four was not:")
    w("")
    still = []
    for eid, what, where in d["named_here"]:
        w(f"    {eid}")
        w(f"        W3a : {what}")
        w(f"        here: {where}")
        if where == "BANKABLE":
            still.append((eid, what))
    w("")
    w(f"  {len(still)} of the thirteen are bankable by this file's rules and carry a W3a")
    w("  cause this file cannot re-derive. They are the shortfall between the")
    w("  ceiling and what W3b should EXPECT:")
    w("")
    for eid, what in still:
        w(f"    {eid:<40} {what}")
    w("")
    w(f"    working expectation = {d['ceiling_feed_removed']} - {len(still)} = "
      f"{d['ceiling_feed_removed'] - len(still)}")
    w("")
    w("  Two of them deserve a sentence each:")
    w("")
    w("  - The `order` pair entries are the ones spec SS4.3 says the real driver")
    w("    makes WORSE, not better. A message-driven runner applies a batch")
    w("    partially and produces the `CREATED` sibling W3a's Task 4 deleted a")
    w("    guard for, naming its trigger as *a caller that applies a batch")
    w("    partially -- W4's worker*. That caller is W3b's.")
    w("  - `valids/retry-failed-flow-duration` is the ONE entry here whose")
    w("    blocker durability removes rather than adds: `maxDuration` in an")
    w("    environment with no elapsed time, and a real driver has elapsed time.")
    w("")
    w("  And one W3a attribution is wrong, found by this derivation putting it")
    w("  in the bankable bucket with nothing against it:")
    w("")
    w("    valids/full is attributed to flow-level `outputs:` and `variables:`.")
    w(f"    Its top-level keys are {', '.join('`' + k + '`' for k in d['full_keys'])} --")
    w("    it has NEITHER. Whatever holds that entry, it is not the two things")
    w("    W3a's table says, and W3b must not plan around them.")
    w("")
    return "\n".join(L) + "\n"


def _wrap(text, width):
    out, line = [], ""
    for word in text.split():
        if line and len(line) + 1 + len(word) > width:
            out.append(line)
            line = word
        else:
            line = f"{line} {word}".strip()
    if line:
        out.append(line)
    return out


def build():
    flows, kinds, golden, baseline, checkout = load_inputs()
    d = derive(flows, kinds, golden, baseline)
    return render(d, checkout), d


# --------------------------------------------------------------------------
# --self-test: every guard fires, and names its own reason
# --------------------------------------------------------------------------

def self_test():
    """Prove each guard FIRES, and fires for the RIGHT REASON, by mutating the
    committed inputs in memory.

    `must_bail` accepting any SystemExit is how a self-test passes a case for
    guard X because guard Y fired -- `harvest.py` records that defect and this
    one asserts on the message for the same reason. Nothing here writes a file:
    a self-test that corrupts the artifact it validates is worse than none.
    """
    import copy
    flows, kinds, golden, baseline, _ck = load_inputs()
    results = []

    def case(name, expect, fn):
        try:
            fn()
        except SystemExit as e:
            msg = str(e)
            if expect in msg:
                hit = next((ln.strip() for ln in msg.splitlines() if expect in ln), msg)
                print(f"  FIRES   {name}")
                print(f"            {hit[:160]}")
                results.append(True)
            else:
                print(f"  WRONG   {name}")
                print(f"            expected {expect!r}, got: {msg.splitlines()[0][:160]}")
                results.append(False)
            return
        print(f"  SILENT  {name}  <-- the guard did not fire")
        results.append(False)

    def run(flows_=None, kinds_=None, golden_=None, baseline_=None):
        derive(flows_ or flows, kinds_ or kinds, golden_ or golden, baseline_ or baseline)

    print("[self-test] every guard must fire, and name its own reason")

    # -- the YAML subset parser ---------------------------------------------
    case("a flow line the parser cannot account for",
         "not a mapping key",
         lambda: parse_flow("id: x\nnamespace: y\nthis is not a mapping\n", "M", "M"))
    case("a flow whose members are over-indented",
         "over-indented inside a mapping",
         lambda: parse_flow("id: x\n  namespace: y\n", "M", "M"))
    case("a document that parses to something that is not a flow",
         "the parser has lost the document",
         lambda: parse_flow("- a\n- b\n", "M", "M"))

    # -- the parser, validated against Kestra --------------------------------
    def lose_a_task():
        m = copy.deepcopy(flows)
        entry = next(e for e in golden if e["verdict"] == "runs" and e.get("runs"))
        m[entry["id"]]["by_id"].pop(entry["runs"][0]["task"])
        run(flows_=m)
    case("the parser loses one task Kestra ran",
         "did not find",
         lose_a_task)

    # -- the input denominators ---------------------------------------------
    case("golden.json is no longer 301",
         "not 301",
         lambda: run(golden_=golden[:-1]))
    first_run = next(e for e in golden if e["verdict"] == "runs")
    case("the `runs` half is no longer 273",
         "not 273",
         lambda: run(golden_=[{"id": e["id"], "verdict": "refused"} if e is first_run
                              else e for e in golden]))
    case("baseline.txt is no longer 42",
         "not 42",
         lambda: run(baseline_=set(list(baseline)[:-1])))
    # Swapped, not added: the size check comes first and would otherwise be the
    # guard this case actually exercises -- which is how a self-test passes a
    # case for guard X because guard Y fired.
    case("baseline.txt names an id the golden does not compare",
         "golden.json does not compare",
         lambda: run(baseline_=set(list(baseline)[:-1]) | {"valids/not-a-flow"}))
    case("golden.json names a flow corpus/ does not hold",
         "corpus/ does not hold",
         lambda: run(golden_=[{"id": "valids/ghost", "verdict": "refused"}
                              if e is golden[0] else e for e in golden]))

    # -- the kind vocabulary -------------------------------------------------
    case("a task kind outside the vocabulary",
         "not in KNOWN_KINDS",
         lambda: _with_kind(kinds, "io.kestra.plugin.core.debug.Return", "ProbablyFine"))
    def drop_kind():
        m = copy.deepcopy(kinds)
        m.pop("io.kestra.plugin.core.output.OutputValues")
        run(kinds_=m)
    case("a `plugin.core` type with no resolved kind",
         "run --refresh-kinds",
         drop_kind)

    # -- the derivations' own arithmetic -------------------------------------
    def blocker_rule_too_wide():
        # An entry that MATCHES today, carrying a run state `RunState` cannot
        # hold: `run_blockers` returns a `state` blocker for it, and the guard
        # says every blocker on a matching entry must be a `handler` one. Either
        # the entry does not really match or the state vocabulary is wrong, and
        # both are contradictions this file must not write out.
        #
        # **The mutation this case used to make cannot reach the guard any
        # more, and W3b Task 7 measured that rather than assuming it.** It flipped
        # `OutputValues` to a `FlowableTask` so the six entries it held would
        # acquire a `construct` blocker while still matching -- and W3b Task 5
        # dropped the feed, so those six stopped matching and the mutation went
        # somewhere else entirely: the case failed with the TRIGGER
        # reconciliation guard's message ("15 entries the naive pass drops
        # declare no `triggers:`"), which is a different guard firing for a
        # different reason. Measured over the whole golden afterwards, **no
        # entry that matches today runs a task type outside `W3B_HANDLERS` and
        # `W3A_FLOWABLES` at all** -- which is nearly a tautology, since that is
        # most of why it matches -- so no `kinds` mutation can reach this guard
        # and only the `state` arm can.
        #
        # The victim is DERIVED, not named: the first matching `runs` entry with
        # a run. Naming one is how the old case rotted.
        #
        # **The STATE it is given is derived too, and W8 Task 5 is why.** This
        # line appended the literal `"WARNING"` until then, and `RunState`
        # gained `Warning` in that task -- so the mutation became a no-op, the
        # guard stayed silent, and the self-test failed for the right reason
        # rather than passing a case it was no longer exercising. A literal here
        # rots on the very event the guard exists to notice. What is appended
        # now is a state Kestra really has that `RUN_STATE` really lacks, chosen
        # at run time; `_SENTINEL` is the fallback for the day `RUN_STATE`
        # holds all seventeen, because a case that cannot mutate is worse than
        # one that mutates artificially.
        outside = sorted(KESTRA_STATES - RUN_STATE)
        m = copy.deepcopy(golden)
        victim = next(e for e in m
                      if e["verdict"] == "runs" and e["id"] not in baseline and e.get("runs"))
        victim["runs"][0]["state"] = list(victim["runs"][0]["state"]) + [
            outside[0] if outside else "_SENTINEL"]
        run(golden_=m)
    case("a blocker rule that fires on an entry egret already reproduces",
         "the blocker rules are wrong",
         blocker_rule_too_wide)

    def lose_a_trigger():
        m = copy.deepcopy(flows)
        victim = next(i for i in m if i.startswith("valids/webhook"))
        m[victim]["doc"].pop("triggers", None)
        run(flows_=m)
    case("a trigger flow that no longer declares a trigger",
         "says every one of them does",
         lose_a_trigger)

    ok = sum(results)
    print(f"[self-test] {ok}/{len(results)} guards fired for the right reason")
    if ok != len(results):
        raise SystemExit("[self-test] at least one guard did not fire correctly")


def _with_kind(kinds, ty, kind):
    """Round-trip a mutated task-kinds.json through the REAL loader.

    Writing the vocabulary check a second time here would prove that the copy
    rejects `ProbablyFine`, which is not the claim.
    """
    import copy
    import tempfile
    m = copy.deepcopy(kinds)
    m[ty]["kind"] = kind
    global KINDS_FILE
    original = KINDS_FILE
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
        json.dump({"_checkout": "mutant", "types": m}, fh)
        KINDS_FILE = fh.name
    try:
        load_kinds()
    finally:
        KINDS_FILE = original
        os.unlink(fh.name)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true",
                    help="regenerate and diff against the committed file")
    ap.add_argument("--self-test", action="store_true",
                    help="prove every guard fires, and for the right reason")
    ap.add_argument("--refresh-kinds", action="store_true",
                    help="re-derive task-kinds.json from the Kestra checkout")
    args = ap.parse_args()

    if args.refresh_kinds:
        refresh_kinds()
        return
    if args.self_test:
        self_test()
        return

    text, d = build()
    if args.check:
        with open(OUT, encoding="utf-8") as f:
            committed = f.read()
        if committed != text:
            import difflib
            diff = list(difflib.unified_diff(committed.splitlines(), text.splitlines(),
                                             "committed", "derived", lineterm="", n=1))
            print("\n".join(diff[:80]))
            raise SystemExit(f"handler-surface.txt is stale: "
                             f"{len(diff)} diff lines. Re-run `handler_surface.py`")
        print(f"handler-surface.txt is up to date "
              f"(ceiling {d['ceiling_feed_removed']}, population {len(d['population'])})")
        return

    with open(OUT, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"wrote {os.path.relpath(OUT, HERE)}: population {len(d['population'])}, "
          f"ceiling {d['ceiling_feed_removed']} (feed removed) / "
          f"{d['ceiling_feed_kept']} (feed retained)")


if __name__ == "__main__":
    main()
