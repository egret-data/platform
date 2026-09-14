#!/usr/bin/env python3
"""W8 Task 5's mutation harness -- every verdict in the task report, regenerable.

**Committed because a table nobody else can rerun is testimony rather than
evidence.** The review of this task could not verify 15 of its 17 kills, for the
one reason that matters: the harness lived in a scratchpad. This file is the
harness, and `python3 crates/egret-workflow/mutations/w8-task5-allow-failure.py`
reproduces the whole table from a clean tree.

Usage
-----

    export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
    export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
    export EGRET_TEST_STARROCKS_URL=mysql://localhost:9030
    python3 crates/egret-workflow/mutations/w8-task5-allow-failure.py           # all
    python3 crates/egret-workflow/mutations/w8-task5-allow-failure.py M7 M11    # some
    python3 crates/egret-workflow/mutations/w8-task5-allow-failure.py --control # no mutation

Run it on a CLEAN tree. Each mutation is applied, run, and restored, and the
restore is verified by byte comparison before the next one starts; a crash
between the two leaves ONE edit in the working tree, which `git diff` shows.

What it prints, and why each part is there
------------------------------------------

* **`applied=True` before the verdict, out of band.** On this branch a mutation
  that silently fails to land prints `test result: ok. N passed; 0 failed` --
  byte-identical to the control -- and one that fails to COMPILE prints nothing
  at all where a result line goes. Neither is distinguishable from a surviving
  mutant by reading the result line, so both are asserted here instead.
* **`compiled`**, for the same reason: no result line and no failure is a
  compile error, not a kill.
* **`failures`**, the test NAMES that died, so a dying mutant can be asked
  WHICH assertion killed it and whether that assertion predates the change
  being proved.
* **`named`**, the golden ids the ratchet printed in its diff, so a bank is
  attributed by a running program rather than by prose.
* **`runs`**, the number of times each verdict was taken. One run is a coin
  flip reported as a measurement wherever a verdict can vary; these are
  compile-and-run differences rather than timing ones, so 1 is honest here --
  raise `--runs` if that ever stops being true.

The suite is `--lib --test executor`, which is what carries every assertion this
task added plus the ratchet. A mutation that could reach `tests/worker.rs` would
need that target added here.
"""

import argparse
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
ROOT = os.path.dirname(os.path.dirname(CRATE))

# id -> (path relative to the crate, the exact text to replace, its replacement)
#
# Every entry must match EXACTLY ONCE. A mutation whose anchor has drifted is
# reported as `LANDED: false` with the occurrence count rather than skipped, so
# a stale anchor cannot be mistaken for a surviving mutant.
MUTATIONS = {
    # --- the mapping table, one mutation per arm ----------------------------
    "M1": ("src/executor/allow_failure.rs",
           "(RunState::Failed, true, false) => RunState::Warning,",
           "(RunState::Failed, true, false) => RunState::Failed,"),
    "M2": ("src/executor/allow_failure.rs",
           "(RunState::Failed, true, true) => RunState::Success,",
           "(RunState::Failed, true, true) => RunState::Warning,"),
    "M3": ("src/executor/allow_failure.rs",
           "(RunState::Warning, _, true) => RunState::Success,",
           "(RunState::Warning, _, true) => RunState::Warning,"),
    "M4": ("src/executor/allow_failure.rs",
           "        (true, false) => RunState::Warning,\n"
           "        (true, true) => RunState::Success,",
           "        (true, false) => RunState::Failed,\n"
           "        (true, true) => RunState::Success,"),
    "M5": ("src/executor/allow_failure.rs",
           "    if will_retry {\n        return outcome;\n    }",
           "    if will_retry && false {\n        return outcome;\n    }"),
    # --- the OPERANDS: where each flag came from ----------------------------
    "M6": ("src/executor/flowable/mod.rs",
           "crate::executor::allow_failure::soften(s, t.allow_failure, t.allow_warning)",
           "crate::executor::allow_failure::soften(s, t.allow_warning, t.allow_failure)"),
    "M7": ("src/executor/flowable/mod.rs",
           "let t = &plan.tasks[task];",
           "let t = &plan.tasks[0];"),
    "M8": ("src/executor/flowable/mod.rs",
           "Some(RunState::Warning) if out != RunState::Failed => out = RunState::Warning,",
           "Some(RunState::Warning) if out != RunState::Failed => out = RunState::Success,"),
    "M9": ("src/executor/flowable/pause.rs",
           "crate::executor::allow_failure::fail(task.allow_failure, task.allow_warning),",
           "crate::executor::allow_failure::fail(false, false),"),
    "M10": ("src/worker/subflow.rs",
            "true => crate::executor::allow_failure::soften(child, allow_failure, allow_warning),",
            "true => crate::executor::allow_failure::soften(child, false, false),"),
    "M11": ("src/executor/flowable/loop_until.rs",
            "!matches!(s, RunState::Failed | RunState::Killed)",
            "!matches!(s, RunState::Failed | RunState::Killed | RunState::Warning)"),
    # --- the PROVENANCE chain, end to end ----------------------------------
    "M12": ("src/executor/plan.rs",
            "        allow_failure: task.allow_failure,\n"
            "        allow_warning: task.allow_warning,",
            "        allow_failure: false,\n        allow_warning: false,"),
    "M13": ("src/models/task.rs",
            '        let allow_failure: bool = with_default(&mut map, "allowFailure", false)?;',
            "        let allow_failure: bool = false;"),
    "M14": ("src/executor/allow_failure.rs",
            "    soften(outcome, task.allow_failure, task.allow_warning)",
            "    soften(outcome, task.allow_warning, task.allow_failure)"),
    "M15": ("src/executor/flowable/pause.rs",
            "crate::executor::allow_failure::fail(task.allow_failure, task.allow_warning),",
            "crate::executor::allow_failure::fail(task.allow_warning, task.allow_failure),"),
    "M16": ("src/worker/subflow.rs",
            "true => crate::executor::allow_failure::soften(child, allow_failure, allow_warning),",
            "true => crate::executor::allow_failure::soften(child, allow_warning, allow_failure),"),
    "M17": ("src/executor/state.rs",
            "                a.history.push(*attempt);",
            "                let _ = &a.history;"),
    # --- fix round 1: the `Subflow` trigger set ----------------------------
    #
    # The disjunct restored when `guess_state` was found answering SUCCESS for
    # a WARNING child where the artifact keeps WARNING. The ratchet cannot
    # score it, so its only killer is the unit row added with it.
    "M18": ("src/worker/subflow.rs",
            "RunState::Failed | RunState::Killed | RunState::Warning",
            "RunState::Failed | RunState::Killed"),
    # --- fix round 1: the terminality guard --------------------------------
    #
    # `is_terminal` losing `Warning`. Reported here beside M18 because the
    # guard NAMED for terminality was silent about it until fix round 1.
    "M19": ("src/executor/state.rs",
            "                | RunState::Warning\n        )",
            "        )"),
    # --- fix round 2: the class `RunState::Warning` falsified -------------
    #
    # M20/M21 are the second shipped defect and its coupled site. M22/M23 are
    # the two guard gaps the round-1 fix left behind: one was already covered
    # and one was not, which is the measurement that makes "one member of a
    # class is not the class" a fact here rather than a slogan.
    "M20": ("src/executor/flowable/parallel.rs",
            ".any(|&ix| latest(state, ix) == Some(RunState::Failed));",
            ".any(|&ix| latest(state, ix).is_some_and(|s| s.is_terminal()\n"
            "            && !matches!(s, RunState::Success | RunState::Skipped)));"),
    "M21": ("src/executor/flowable/dag.rs",
            "run_for(dep).is_some_and(|ix| latest(state, ix).is_some_and(RunState::is_terminal))",
            "run_for(dep).is_some_and(|ix| {\n"
            "                    matches!(\n"
            "                        latest(state, ix),\n"
            "                        Some(RunState::Success) | Some(RunState::Skipped)\n"
            "                    )\n"
            "                })"),
    "M22": ("src/worker/subflow.rs",
            "            RunState::Failed | RunState::Killed | RunState::Warning\n        ) {",
            "            RunState::Failed | RunState::Killed | RunState::Warning | RunState::Skipped\n        ) {"),
    "M23": ("src/worker/subflow.rs",
            "            RunState::Failed | RunState::Killed | RunState::Warning\n        ) {",
            "            RunState::Failed | RunState::Killed | RunState::Warning | RunState::Retried\n        ) {"),
    # `PAUSED` becomes terminal. The trigger-set omission in `guess_state` is
    # SOUND ONLY because a paused child never reaches it, and that soundness
    # rested on this line rather than on anything in `subflow.rs`.
    "M24": ("src/executor/state.rs",
            "                | RunState::Warning\n        )",
            "                | RunState::Warning\n                | RunState::Paused\n        )"),
}

TARGETS = ["--lib", "--test", "executor"]


def run_suite():
    out = subprocess.run(
        ["cargo", "test", "-p", "egret-workflow", *TARGETS,
         "--no-fail-fast", "-j", "3", "--", "--nocapture"],
        cwd=ROOT, capture_output=True, text=True,
    )
    txt = out.stdout + out.stderr
    return {
        "exit": out.returncode,
        "compiled": "error[E" not in txt and "could not compile" not in txt,
        "results": re.findall(r"^test result: .*$", txt, re.M),
        "failures": sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED$", txt, re.M))),
        "named": sorted(set(re.findall(r"^\s+((?:valids|invalids)/\S+):", txt, re.M))),
        "ratchet": (re.search(r"^oracle-exec ratchet: .*$", txt, re.M) or
                    re.search(r"^$", txt)).group(0),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ids", nargs="*", help="mutation ids; default all")
    ap.add_argument("--control", action="store_true",
                    help="run the suite with no mutation applied")
    ap.add_argument("--runs", type=int, default=1,
                    help="times to run each verdict (see the module doc)")
    args = ap.parse_args()

    if args.control:
        for i in range(args.runs):
            r = run_suite()
            print(f"[CONTROL run {i + 1}] " + json.dumps(r))
        return 0

    ids = args.ids or list(MUTATIONS)
    report = []
    for tag in ids:
        rel, old, new = MUTATIONS[tag]
        path = os.path.join(CRATE, rel)
        original = open(path).read()
        n = original.count(old)
        if n != 1:
            print(f"[{tag}] LANDED=False -- anchor matched {n} times in {rel}")
            report.append({"tag": tag, "LANDED": False, "occurrences": n})
            continue
        open(path, "w").write(original.replace(old, new, 1))
        after = open(path).read()
        landed = new in after and old not in after
        # Out of band, and BEFORE any verdict is read.
        print(f"[{tag}] applied={landed} file={rel}")
        try:
            for i in range(args.runs):
                r = run_suite()
                r.update(tag=tag, LANDED=landed, run=i + 1, runs=args.runs)
                report.append(r)
                print(f"[{tag}] " + json.dumps(
                    {k: r[k] for k in ("run", "LANDED", "compiled", "exit",
                                       "failures", "named")}))
        finally:
            open(path, "w").write(original)
            assert open(path).read() == original, f"restore of {rel} failed"
    json.dump(report, open(os.path.join(HERE, "w8-task5-verdicts.json"), "w"), indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
