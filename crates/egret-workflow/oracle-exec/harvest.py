#!/usr/bin/env python3
"""W3a execution oracle -- harvester.

Stands up a pinned Kestra (docker compose -p w3oracle), publishes every flow in
corpus/, executes the ones that can run standalone, follows `parentId` to the
sub-executions the search API hides, and writes corpus.json + runnable.txt.

Requires nothing but python3 and docker. See README.md for the rules.

    python3 harvest.py            # up, publish, run, write corpus.json/runnable.txt
    python3 harvest.py --no-up    # assume the stack is already up
    python3 harvest.py --down     # tear the stack down (down -v) and exit
"""

import argparse
import base64
import json
from collections import Counter
import os
import re
import subprocess
import time
import urllib.error
import urllib.request

def bail(msg):
    """Guards raise; they never `assert`.

    `python3 -O` strips assert statements, so a guard written as an assert is a
    guard that silently disappears under a flag anyone may pass.
    """
    raise SystemExit("harvest.py guard failed: " + msg)


def load_task2():
    """golden.json + stability.json, Task 2's committed outputs.

    `runnable.txt` is the file this package calls its honest denominator, so the
    happens-before numbers belong in it and not only in a script's stdout.  Both
    files are committed, so `--rewrite-runnable` still needs no Kestra.
    """
    out = {}
    for name in ("golden.json", "stability.json"):
        path = os.path.join(HERE, name)
        if not os.path.exists(path):
            bail(f"{name} is missing; run `python3 normalise.py` first "
                 "(it needs traces.json, which `--traces` writes)")
        with open(path, encoding="utf-8") as f:
            out[name] = json.load(f)
    return out["golden.json"], out["stability.json"]


def pairs_in(node):
    """How many ordered pairs one normalised trace HAS, counting sub-executions.

    Derived from golden.json.  The point is that it is a SECOND derivation:
    stability.json says how many pairs it kept and dropped, golden.json says how
    many runs there are, and n*(n-1)/2 must reconcile the two.  A stability file
    that has drifted from the golden -- a re-normalisation that wrote one and not
    the other -- fails here instead of printing a table that adds up with itself.
    """
    n = len(node.get("runs") or [])
    return n * (n - 1) // 2 + sum(pairs_in(sub) for sub in node.get("spawned") or [])


def order_in(node):
    """How many order pairs one normalised trace CARRIES, sub-executions included.

    The counterpart to `pairs_in`: that one says how many pairs a trace has,
    this one how many survived the filter.  Both must reconcile with
    stability.json, from opposite directions.
    """
    return len(node.get("order") or []) + sum(
        order_in(sub) for sub in node.get("spawned") or [])


# The whole relation vocabulary.  A pair carrying anything else is a hard
# failure rather than a row nobody prints -- the same rule KNOWN_DISPOSITIONS and
# KNOWN_REJECTED_CLASSES follow.
KNOWN_RELATIONS = ("before", "after", "overlapping")


def count_relations(cid, node, counter):
    """Tally golden.json's order relations, sub-executions included."""
    for pair in node.get("order") or []:
        rel = pair[2]
        if rel not in KNOWN_RELATIONS:
            bail(f"{cid}: order pair {pair[:2]} carries relation {rel!r}, which is not "
                 f"a known relation {list(KNOWN_RELATIONS)}")
        counter[rel] += 1
    for sub in node.get("spawned") or []:
        count_relations(cid, sub, counter)


def order_table(golden, stab):
    """The happens-before relation: kept, and what had to be dropped.

    Kestra's own tests read `assertThat(a2.startDate.isAfter(a1.endDate)).isTrue()`
    FOUR times, in ParallelTest.errors() and DagTest.errors() -- counted at
    checkout 96893b589, not the 39 the plan claims.  That is a claim about
    SERIALISATION, and a normaliser that drops all clocks cannot see a flowable
    that stops serialising -- so the relation is derived before the clocks go,
    and kept only where all five rounds agreed.
    """
    lines = ["happens-before pairs, over the five harvest rounds"]
    by_id = {e["id"]: e for e in golden if e.get("verdict") == "runs"}
    missing = sorted(set(by_id) - set(stab["flows"]))
    if missing:
        bail(f"{len(missing)} golden entr(y/ies) have no stability record, e.g. "
             f"{missing[:5]}; the two files describe different harvests")

    kept = unstable = unmeasurable = 0
    by_relation = Counter()
    for cid, entry in sorted(by_id.items()):
        count_relations(cid, entry, by_relation)
        rec = stab["flows"][cid]
        have = (rec["pairs_kept"] + rec["pairs_dropped_unstable"]
                + rec["pairs_dropped_unmeasurable"])
        want = pairs_in(entry)
        if have != want:
            bail(f"{cid}: stability.json accounts for {have} pair(s) but golden.json's "
                 f"run counts give {want}; one of the two files is stale")
        written = order_in(entry)
        if written != rec["pairs_kept"]:
            bail(f"{cid}: golden.json carries {written} order pair(s) across the whole "
                 f"trace but stability.json kept {rec['pairs_kept']}; the stability "
                 "filter and the golden writer disagree")
        kept += rec["pairs_kept"]
        unstable += rec["pairs_dropped_unstable"]
        unmeasurable += rec["pairs_dropped_unmeasurable"]

    lines.append(f"  {'kept (identical in all 5 rounds)':<44} {kept}")
    lines.append(f"  {'dropped, relation flipped between rounds':<44} {unstable}")
    lines.append(f"  {'dropped, a run had no start or no end':<44} {unmeasurable}")
    lines.append(f"  {'TOTAL':<44} {kept + unstable + unmeasurable}")
    lines.append("")
    # Rows for every relation PRESENT, never a hand-written list, and derived
    # from golden.json's `order` entries -- a SECOND derivation of the number
    # stability.json reports as `pairs_kept`, which the per-flow check above
    # reconciles against it.
    lines.append("  of the kept pairs, by relation")
    for rel in KNOWN_RELATIONS:
        if rel in by_relation:
            lines.append(f"    {rel:<42} {by_relation[rel]}")
    lines.append(f"    {'TOTAL':<42} {sum(by_relation.values())}")
    lines.append("")
    lines.append("  `after` is UNREACHABLE BY CONSTRUCTION on Kestra's side, not measured")
    lines.append("  to be absent: pairs are enumerated (i, j) with i < j in taskRunList")
    lines.append("  order, and a run's left endpoint is its creation instant, which is")
    lines.append("  monotonic in that order in 4215 of 4215 captured runs.  It stays in the")
    lines.append("  vocabulary because it is a live error signal on EGRET's side, where")
    lines.append("  nothing guarantees that monotonicity.")
    lines.append("")
    lines.append("  A relation observed once is a coincidence, and genuinely parallel")
    lines.append("  branches flip.  Only the pairs that came out the same in all five")
    lines.append("  rounds are in golden.json; the rest are dropped and counted here.")
    lines.append("")
    lines.append(f"traces that did NOT reproduce across all 5 rounds: "
                 f"{len(stab['unstable_traces'])}")
    for cid in stab["unstable_traces"]:
        rec = stab["flows"][cid]
        lines.append(f"  {cid}  (rounds differing from round 1: "
                     f"{rec['rounds_differing_from_round_1']})")
    lines.append("  These get NO golden entry and are never compared.  A trace the oracle")
    lines.append("  itself does not reproduce five times running cannot judge egret: an")
    lines.append("  implementation matching round 1 would be right by luck.")
    lines.append("")
    return lines


HERE = os.path.dirname(os.path.abspath(__file__))

# Everything after this line in `runnable.txt` is written by hand, and
# `--rewrite-runnable` PRESERVES it instead of deleting it.
#
# W3b Task 7 measured the loss rather than assuming it: a copy of this script
# plus the committed inputs, run with `--rewrite-runnable` in a scratch
# directory, took `runnable.txt` from 1074 lines to 755 and removed every task
# note appended to it -- including the sections README.md tells a reader to
# believe over itself ("if this file and `runnable.txt` ever disagree,
# `runnable.txt` is right"). A documented command that deletes the authoritative
# half of the authoritative file is a landmine, not a regeneration.
HANDWRITTEN_SENTINEL = (
    "=====  hand-written notes below -- `--rewrite-runnable` PRESERVES them  ====="
)

# The artifact the golden was harvested from, and the honest shape of that
# claim. `v2.0.0-rc13` is a MUTABLE tag over a nightly build (the image's own
# labels read `version=nightly`, `revision=169ca7f2...`), and it was pulled with
# `pull_policy: always` for the whole of W3a, W3c and W3b Tasks 1-3. The digest
# below is what `docker-compose.yml` pins TODAY, read off the running container
# on 2026-09-06 by W3b Task 3; the golden predates the pin, so its provenance is
# the tag as it stood, and nothing establishes the two are the same image.
IMAGE_TAG = "kestra/kestra:v2.0.0-rc13"
IMAGE_DIGEST_PINNED_NOW = (
    "sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd"
)

PROJECT = "w3oracle"
BASE = "http://localhost:8080/api/v1"
TENANT = "main"
USER = "admin@kestra.io"
PASSWORD = "Admin1234!"

# Kestra's own State.Type.isTerminated(), read from
# core/src/main/java/io/kestra/core/models/flows/State.java at checkout
# 96893b589a02e5d5d85c722855f76c08e79085af.  The brief names four of these; the
# corpus reaches CANCELLED (exit-cancelled.yaml) and SKIPPED too, and treating
# those as "still running" would have mis-filed them as not-standalone.
TERMINAL = {
    "FAILED", "WARNING", "SUCCESS", "KILLED",
    "CANCELLED", "RETRIED", "SKIPPED", "RESUBMITTED",
}

EXEC_TIMEOUT = 90.0      # seconds to wait for one top-level execution
SUB_TIMEOUT = 60.0       # extra seconds to wait for its descendants
POLL = 0.5

# --------------------------------------------------------------------------
# Recovered inputs.
#
# 53 of the 261 `valids` declare `inputs:`.  The YAML declares that an input
# exists; the VALUE lives in the Java test beside it.  Every entry below was
# read out of the Kestra checkout at 96893b589a; `from` is the file and method
# it came from, verbatim.  Nothing here is invented: a flow whose value is
# computed by the JVM (a classpath resource path, a 16 KiB generated string, a
# Java Map object), or which no Java test drives at all, is listed in
# NOT_STANDALONE instead with the reason.
#
# `alt` records the other input maps the same flow is driven with, so nothing
# measured is thrown away.  The primary is the map that drives the flow's
# failure / error branch where there is one, because the error path exercises
# `errors:` and `finally:` handling that the success path does not.
# --------------------------------------------------------------------------
J = "core/src/test/java/io/kestra/"
W = "webserver/src/test/java/io/kestra/webserver/controllers/api/ExecutionControllerRunnerTest.java"

INPUTS = {
    "valids/allow-failure": {
        "inputs": {"crash": "1"},
        "from": J + "plugin/core/flow/AllowFailureTest.java:41 failed()",
        "alt": [{"inputs": {}, "from": J + "plugin/core/flow/AllowFailureTest.java:29 success() (@ExecuteFlow, no inputs)"}],
    },
    "valids/condition_with_input": {
        "inputs": {"condition": "fail"},
        "from": W + ":742 replayExecutionWithNewInputs()",
        "alt": [{"inputs": {"condition": "success"}, "from": W + ":762 replayExecutionWithNewInputs() replay body"}],
    },
    "valids/exit": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:16 shouldExitTheExecution() (@ExecuteFlow, no inputs)"},
    "valids/exit-killed": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:24 shouldExitAndKillTheExecution() (@ExecuteFlow, no inputs)"},
    "valids/exit-nested": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:33 shouldExitAndFailNestedIf() (@ExecuteFlow, no inputs)"},
    "valids/exit-cancelled": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:45 shouldExitWithCancelled() (@ExecuteFlow, no inputs)"},
    "valids/fail-on-switch": {"inputs": {"param": "fail"}, "from": J + "plugin/core/execution/FailTest.java:31 failOnSwitch()"},
    "valids/fail-on-condition": {
        "inputs": {"param": "fail"},
        "from": J + "plugin/core/execution/FailTest.java:44 failOnCondition()",
        "alt": [{"inputs": {"param": "success"}, "from": J + "plugin/core/execution/FailTest.java:57 dontFailOnCondition()"}],
    },
    "valids/finally-sequential": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:59 sequentialWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:42 sequentialWithoutErrors()"}],
    },
    "valids/finally-sequential-error": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:105 sequentialErrorBlockWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:76 sequentialErrorBlockWithoutErrors()"}],
    },
    "valids/finally-allowfailure": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:141 allowFailureWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:124 allowFailureWithoutErrors()"}],
    },
    "valids/finally-parallel": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/ParallelTest.java:48 errors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:160 parallelWithoutErrors()"}],
    },
    "valids/finally-loop": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:216 loopWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:196 loopWithoutErrors()"}],
    },
    "valids/finally-loop-parallel": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:266 loopParallelWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:246 loopParallelWithoutErrors()"}],
    },
    "valids/finally-dag": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/DagTest.java:86 errors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:297 dagWithoutErrors()"}],
    },
    "valids/finally-flow": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:350 flowWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:333 flowWithoutErrors()"}],
    },
    "valids/finally-flow-error": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:384 flowErrorBlockWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:367 flowErrorBlockWithoutErrors()"}],
    },
    "valids/if": {
        "inputs": {"if1": True, "if2": False, "if3": True},
        "from": J + "core/runners/AbstractRunnerTest.java:670 multipleIf()",
    },
    "valids/if-condition": {
        "inputs": {"param": True},
        "from": J + "plugin/core/flow/IfTest.java:41 ifTruthy()",
        "alt": [
            {"inputs": {"param": "true"}, "from": J + "plugin/core/flow/IfTest.java:54 ifTruthy()"},
            {"inputs": {"param": 1}, "from": J + "plugin/core/flow/IfTest.java:63 ifTruthy()"},
            {"inputs": {"param": False}, "from": J + "plugin/core/flow/IfTest.java:76 ifFalsy()"},
            {"inputs": {"param": "false"}, "from": J + "plugin/core/flow/IfTest.java:85 ifFalsy()"},
            {"inputs": {"param": 0}, "from": J + "plugin/core/flow/IfTest.java:94 ifFalsy()"},
        ],
    },
    "valids/if-without-else": {
        "inputs": {"param": True},
        "from": J + "plugin/core/flow/IfTest.java:118 ifWithoutElse()",
        "alt": [{"inputs": {"param": False}, "from": J + "plugin/core/flow/IfTest.java:127 ifWithoutElse()"}],
    },
    "valids/if-in-flowable": {
        "inputs": {"param": True},
        "from": J + "plugin/core/flow/IfTest.java:139 ifInFlowable()",
    },
    "valids/input-log-secret": {
        "inputs": {"nested.key": "pass"},
        "from": J + "core/runners/InputsTest.java:425 shouldMaskSecretInputInLogs()",
    },
    "valids/json-input-schema-validation": {
        "inputs": {"jsonWithSchema": {"name": "kestra"}, "jsonWithoutSchema": {"any": "shape"}},
        "from": J + "core/models/flows/input/JsonInputSchemaFlowIntegrationTest.java:35 shouldRunFlowWhenJsonInputMatchesSchema()",
    },
    "valids/parallel-fail-with-flowable": {
        "inputs": {},
        "from": J + "plugin/core/flow/ParallelTest.java:67 parallelFailWithFlowable() (@ExecuteFlow, no inputs)",
    },
    "valids/pause-behavior": {
        "inputs": {"behavior": "FAIL"},
        "from": J + "plugin/core/flow/PauseTest.java:117 runDurationWithBehaviorFail() -> Pause.Behavior.FAIL",
        "alt": [
            {"inputs": {"behavior": "RESUME"}, "from": J + "plugin/core/flow/PauseTest.java:111 -> Pause.Behavior.RESUME"},
            {"inputs": {"behavior": "WARN"}, "from": J + "plugin/core/flow/PauseTest.java:123 -> Pause.Behavior.WARN"},
            {"inputs": {"behavior": "CANCEL"}, "from": J + "plugin/core/flow/PauseTest.java:129 -> Pause.Behavior.CANCEL"},
        ],
    },
    "valids/pause-duration-from-input": {
        "inputs": {},
        "from": J + "plugin/core/flow/PauseTest.java:206 runDurationFromInput() (passes null inputs; the DURATION default PT1S applies)",
    },
    "valids/replay-loop": {
        "inputs": {"failed": "FIRST"},
        "from": J + "core/runners/ExecutionServiceTest.java:139 restartLoop()",
    },
    "valids/sla-execution-condition": {
        "inputs": {"string": "CANCEL"},
        "from": J + "core/runners/SLATestCase.java:62 executionConditionSLAShouldCancel()",
        "alt": [
            {"inputs": {"string": "LABEL"}, "from": J + "core/runners/SLATestCase.java:86 executionConditionSLAShouldLabel()"},
            {"inputs": {}, "from": J + "core/runners/SLATestCase.java:56 executionConditionSLAShouldPass() (no inputs)"},
        ],
    },
    "valids/subflow-function-child": {
        "inputs": {"region": "us"},
        "from": J + "core/runners/pebble/functions/SubflowFunctionTest.java:31 shouldRunSubflowAndExposeFlowOutputs()",
        "alt": [{"inputs": {}, "from": J + "core/runners/pebble/functions/SubflowFunctionTest.java:96 shouldUseSubflowInputDefaultsWhenNotProvided()"}],
    },
    "valids/subflow-nullable-input-parent": {
        "inputs": {"integerValue": ""},
        "from": J + "plugin/core/flow/SubflowRunnerTest.java:155 shouldPassNullableInputFromParentToSubflow()",
    },
    "valids/switch": {
        "inputs": {"string": "FIRST"},
        "from": J + "plugin/core/flow/SwitchTest.java:39 switchFirst()",
        "alt": [
            {"inputs": {"string": "SECOND"}, "from": J + "plugin/core/flow/SwitchTest.java:57 switchSecond()"},
            {"inputs": {"string": "THIRD"}, "from": J + "plugin/core/flow/SwitchTest.java:74 switchThird()"},
            {"inputs": {"string": "DEFAULT"}, "from": J + "plugin/core/flow/SwitchTest.java:92 switchDefault()"},
        ],
    },
    "valids/switch-impossible": {
        "inputs": {"string": "impossible"},
        "from": J + "plugin/core/flow/SwitchTest.java:108 switchImpossible()",
    },
    "valids/task-flow": {
        "inputs": {"string": "THIRD"},
        "from": J + "plugin/core/flow/FlowCaseTest.java:44 run() via waitFailed()",
        "alt": [{"inputs": {"string": "OK"}, "from": J + "plugin/core/flow/FlowCaseTest.java:44 run() via waitSuccess()"}],
    },
    "valids/task-flow-inherited-labels": {
        "inputs": {"string": "OK"},
        "from": J + "plugin/core/flow/FlowCaseTest.java:44 run() via noLabels()",
    },
    # An `invalids` flow that Kestra REFUSES AT THE EXECUTION SEAM, not at publish:
    # the MULTISELECT constraint is checked when the input is read, so the publish
    # returns 200 and the execution POST returns 422.
    "invalids/inputs-with-multiple-constraint-violations": {
        "inputs": {"multi": ["F", "H"]},
        "from": J + "core/runners/InputsTest.java:479 multipleConstraintViolations()",
    },
    # --- the four `valids/*.yml` flows that declare inputs (finding A) ---------
    "valids/empty-variables": {
        "inputs": {"emptyKey": "{ \"foo\": \"\" }",
                   "emptySubObject": "{\"json\":{\"someEmptyObject\":{}}}"},
        "from": J + "core/runners/EmptyVariablesTest.java:35 emptyVariables() "
                    "(JSON inputs are passed as JSON TEXT, not as objects)",
    },
    "valids/primitive-labels-flow": {
        "inputs": {"intLabel": 42, "boolLabel": True, "floatLabel": 3.14},
        "from": J + "plugin/core/flow/RuntimeLabelsTest.java:105 primitiveTypeLabels() "
                    "(floatLabel is the Java float literal 3.14f)",
        "alt": [{"inputs": {"intLabel": 42, "boolLabel": True, "floatLabel": 3.14},
                 "from": J + "plugin/core/flow/RuntimeLabelsTest.java:148 "
                             "primitiveTypeLabelsOverrideExistingLabels() -- same inputs, "
                             "different execution LABELS"}],
    },
    "valids/labels-update-task": {
        "inputs": {"labelsJson": "{\"keyFromJson\": \"valueFromJson\"}",
                   "labelsMapKey": "keyFromMap", "labelsMapValue": "valueFromMap",
                   "labelsListKey": "keyFromList", "labelsListValue": "valueFromList"},
        "from": J + "plugin/core/flow/RuntimeLabelsTest.java:39 update()",
    },
    "valids/task-runif-workingdirectory": {
        "inputs": {},
        "from": J + "core/runners/TaskWithRunIfTest.java:31 runIfWorkingDirectory() "
                    "(@ExecuteFlow, no inputs; the MULTISELECT default [\"orders\"] applies)",
    },
    "valids/working-directory": {
        "inputs": {"failed": "true"},
        "from": J + "plugin/core/flow/WorkingDirectoryTest.java:181 failed()",
        "alt": [{"inputs": {"failed": "false"}, "from": J + "plugin/core/flow/WorkingDirectoryTest.java:170 success()"}],
    },
}

# Declared inputs whose value cannot be recovered as a literal, or which no Java
# test drives.  These are marked not-standalone WITHOUT being executed: running
# them would either 422 or silently take a YAML default and produce a golden for
# a path no Kestra test asserts, which is the failure mode this table exists to
# prevent.
NOT_STANDALONE_INPUTS = {
    "valids/task-flow-dynamic":
        "FlowGraphTest.java:295 hands runOne a RAW inputs map "
        "({namespace: f.getNamespace(), flowId: 'switch'}) instead of going through "
        "flowIO.readExecutionInputs, so it bypasses typed-input validation entirely and "
        "never supplies the flow's one DECLARED input, the required STRING `string`.  "
        "Over the API that is `422 Missing required input:string`, and there is no value "
        "for `string` anywhere in the checkout to recover.",
    "valids/inputs":
        "FILE input `file` is a classpath resource path resolved by the JVM "
        "(InputsTest.inputs -> application-test.yml) and json2/yaml2 are Java Map "
        "objects; the values are computed, not literal.",
    "valids/inputs-large":
        "RunContextTest.java:141 builds the `string` input from a 16 KiB generated "
        "char array and reuses InputsTest.inputs' classpath FILE path; computed, not literal.",
    "valids/inputs-small-files":
        "ExecutionControllerRunnerTest.java:394 uploads the classpath resource "
        "application-test.yml as a multipart FILE part; the value is a JVM resource path.",
    "valids/restart_with_inputs":
        "ExecutionControllerRunnerTest.java:176 supplies FILE input `file` as the "
        "classpath resource data/hello.txt; the value is a JVM resource path.",
    "valids/secret-input-validation":
        "InputsTest.java:493 secretInputValidation() never executes the flow -- it "
        "calls flowIO.readExecutionInputs and asserts it throws.  No test drives an execution.",
    "valids/scheduled-inputs-defaults":
        "Only FlowValidationTest.java:231 references it, and only to parse().  No test executes it.",
    "valids/kv":
        "No Java test drives this flow.  Both inputs have YAML defaults, so a bare "
        "execution would silently take them and produce a golden for a path no Kestra test asserts.",
    "valids/trigger-flow-listener":
        "Created only by its own io.kestra.plugin.core.trigger.Flow trigger, from an "
        "execution of trigger-flow (FlowTriggerCaseTest.java:48); its inputs render "
        "from trigger variables that do not exist on a direct execution.",
    "valids/trigger-flow-listener-namespace-condition":
        "Same as trigger-flow-listener: created only by a Flow trigger "
        "(FlowTriggerCaseTest.java:62).",
    "valids/trigger-flow-listener-invalid":
        "Nothing in the Kestra checkout references this flow at all -- no Java test, "
        "no other flow.  There is no test case to recover.",
    "valids/flow-trigger-invalid-inputs-flow-listen":
        "Created only by a Flow trigger on flow-trigger-invalid-inputs-flow-a "
        "(MultipleConditionTriggerCaseTest.java:282); its inputs deliberately reference "
        "trigger outputs that do not exist, and the test asserts the TRIGGER produces a "
        "FAILED execution with no task runs.",
    "valids/webhook-inputs":
        "Executed only through POST /executions/webhook/{ns}/webhook-inputs/{key} with a "
        "JSON body (ExecutionControllerRunnerTest.java:1491); its inputs come from the "
        "webhook body, not from an execution POST.",
    "valids/subflow-to-retry":
        "Only ever created as a subflow of subflow-parent-retry "
        "(SubflowRunnerTest.java:118); no test posts inputs to it directly.",
    "valids/subflow-nullable-input-child":
        "Only ever created as a subflow of subflow-nullable-input-parent "
        "(SubflowRunnerTest.java:144); no test posts inputs to it directly.",
    "valids/subflow-function-recursive":
        "SubflowFunctionTest.java:132 drives it through the subflow() Pebble function and "
        "asserts the call THROWS on the recursion-depth cap.  Its input default calls "
        "subflow() on its own id, so there is no input map to recover.",
    "valids/subflow-function-mutual-a":
        "SubflowFunctionTest.java:147 drives it through the subflow() Pebble function and "
        "asserts the call THROWS on the recursion-depth cap (a -> b -> a).",
    "valids/subflow-function-mutual-b":
        "Half of the a -> b -> a mutual-recursion pair at SubflowFunctionTest.java:147; "
        "only ever reached through subflow-function-mutual-a.",
}

# --------------------------------------------------------------------------
# The cross-check that makes the input recovery FALSIFIABLE.
#
# The failure mode this whole table exists to prevent is silent: a wrong input
# does not error, it runs the flow's other branch.  A terminal STATE alone is a
# weak witness -- six of these flows end SUCCESS on either branch, so a state
# check on them passes with or without the right input.  So each row also carries
# whatever DISCRIMINATING assertion the same Java test makes:
#
#   state       execution.getState().getCurrent()
#   taskruns    execution.getTaskRunList() hasSize(N)
#   task_at     getTaskRunList().get(i).getTaskId() -- position-sensitive
#   task_states findTaskRunsByTaskId(x).getFirst().getState().getCurrent()
#               (a task absent from the harvest fails the row, which is exactly
#                what a branch flip produces)
#
# Only assertions the Java test actually makes are recorded.  Nothing is inferred
# from the harvest, or the check would be checking itself.
#
# `harvest.py` then runs a DISCRIMINATION PROBE (see probe_discrimination): for
# every row whose flow has a documented alternative input, it executes the flow
# again with that alternative and requires the observed tuple to DIFFER.  A row
# that survives its own alternative is reported as non-discriminating rather than
# counted as a pass.
# --------------------------------------------------------------------------
EXPECTED = {
    "valids/allow-failure": {
        "state": "FAILED", "taskruns": 10,
        "task_states": {"global-error": "SUCCESS", "switch": "FAILED", "crash": "FAILED"},
        "from": "AllowFailureTest.java:50-55 failed()"},
    "valids/condition_with_input": {
        "state": "FAILED",
        "from": "ExecutionControllerRunnerTest.java:747 replayExecutionWithNewInputs()"},
    "valids/exit": {
        "state": "WARNING", "taskruns": 2,
        "from": "ExitTest.java:18-20 shouldExitTheExecution()"},
    "valids/exit-killed": {
        "state": "KILLED", "taskruns": 2,
        "from": "ExitTest.java:26-29 shouldExitAndKillTheExecution()"},
    "valids/exit-nested": {
        "state": "FAILED", "taskruns": 4,
        "task_states": {"if_some_bool": "FAILED", "nested_bool_check": "FAILED",
                        "nested_was_false": "FAILED"},
        "from": "ExitTest.java:35-41 shouldExitAndFailNestedIf()"},
    "valids/exit-cancelled": {
        "state": "CANCELLED", "taskruns": 2,
        "from": "ExitTest.java:47-50 shouldExitWithCancelled()"},
    "valids/fail-on-switch": {
        "state": "FAILED", "taskruns": 1, "task_states": {"switch": "FAILED"},
        "from": "FailTest.java:35-37 failOnSwitch()"},
    "valids/fail-on-condition": {
        "state": "FAILED", "taskruns": 2, "task_states": {"fail": "FAILED"},
        "from": "FailTest.java:48-50 failOnCondition()"},
    "valids/finally-sequential": {
        "state": "FAILED", "taskruns": 5,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS"},
        "from": "FinallyTest.java:63-67 sequentialWithErrors()"},
    "valids/finally-sequential-error": {
        "state": "FAILED", "taskruns": 7,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS",
                        "e1": "SUCCESS", "e2": "SUCCESS"},
        "from": "FinallyTest.java:109-115 sequentialErrorBlockWithErrors()"},
    "valids/finally-allowfailure": {
        "state": "WARNING", "taskruns": 7,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS",
                        "e1": "SUCCESS", "e2": "SUCCESS"},
        "from": "FinallyTest.java:145-151 allowFailureWithErrors()"},
    "valids/finally-parallel": {
        "state": "FAILED", "taskruns": 10,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS",
                        "e1": "SUCCESS", "e2": "SUCCESS"},
        "from": "ParallelTest.java:52-58 errors()"},
    "valids/finally-loop": {
        "state": "FAILED", "taskruns": 1,
        "from": "FinallyTest.java:220-221 loopWithErrors()"},
    "valids/finally-loop-parallel": {
        "state": "FAILED", "taskruns": 1,
        "from": "FinallyTest.java:271-272 loopParallelWithErrors()"},
    "valids/finally-dag": {
        "state": "FAILED", "taskruns": 9,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS",
                        "e1": "SUCCESS", "e2": "SUCCESS"},
        "from": "DagTest.java:92-98 errors()"},
    "valids/finally-flow": {
        "state": "FAILED", "taskruns": 4,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS"},
        "from": "FinallyTest.java:354-358 flowWithErrors()"},
    "valids/finally-flow-error": {
        "state": "FAILED", "taskruns": 6,
        "task_states": {"ko": "FAILED", "a1": "SUCCESS", "a2": "SUCCESS",
                        "e1": "SUCCESS", "e2": "SUCCESS"},
        "from": "FinallyTest.java:388-394 flowErrorBlockWithErrors()"},
    "valids/if": {
        "state": "SUCCESS", "taskruns": 12,
        "from": "AbstractRunnerTest.java:673-674 multipleIf()"},
    "valids/if-condition": {
        "state": "SUCCESS", "taskruns": 2, "task_states": {"when-true": "SUCCESS"},
        "from": "IfTest.java:45-47 ifTruthy()"},
    "valids/if-without-else": {
        "state": "SUCCESS", "taskruns": 2, "task_states": {"when-true": "SUCCESS"},
        "from": "IfTest.java:122-124 ifWithoutElse()"},
    "valids/if-in-flowable": {
        "taskruns": 1,
        "from": "IfTest.java:145 ifInFlowable() (no execution-state assertion; "
                "the state assertions are on the sub-executions)"},
    "valids/input-log-secret": {
        "state": "SUCCESS", "taskruns": 1,
        "from": "InputsTest.java:429-430 shouldMaskSecretInputInLogs()"},
    "valids/json-input-schema-validation": {
        "state": "SUCCESS", "taskruns": 2,
        "from": "JsonInputSchemaFlowIntegrationTest.java:50-51"},
    "valids/parallel-fail-with-flowable": {
        "state": "FAILED", "taskruns": 5,
        "from": "ParallelTest.java:69-70 parallelFailWithFlowable()"},
    "valids/pause-behavior": {
        "state": "FAILED", "taskruns": 1,
        "from": "PauseTest.java:376-380 runDurationWithBehavior(FAIL)"},
    "valids/pause-duration-from-input": {
        "state": "SUCCESS", "taskruns": 2,
        "from": "PauseTest.java:200-202 runDurationFromInput()"},
    "valids/replay-loop": {
        "state": "FAILED", "taskruns": 1,
        "from": "ExecutionServiceTest.java:140-141 restartLoop()"},
    "valids/sla-execution-condition": {
        "state": "CANCELLED",
        "from": "SLATestCase.java:64 executionConditionSLAShouldCancel()"},
    "valids/switch": {
        "task_at": {1: "t1"},
        "from": "SwitchTest.java:47 switchFirst() -- getTaskRunList().get(1).getTaskId()"},
    "valids/switch-impossible": {
        "state": "FAILED",
        "from": "SwitchTest.java:117 switchImpossible()"},
    "valids/task-flow": {
        "state": "FAILED", "taskruns": 1,
        "from": "FlowCaseTest.java:33 waitFailed() -> run(..., FAILED, ..., 4, ...)"},
    "valids/task-flow-inherited-labels": {
        "state": "SUCCESS", "taskruns": 1,
        "from": "FlowCaseTest.java:37 noLabels() -> run(\"OK\", SUCCESS, ..., false, ...)"},
    "valids/subflow-nullable-input-parent": {
        "state": "SUCCESS",
        "from": "SubflowRunnerTest.java:161 shouldPassNullableInputFromParentToSubflow()"},
    "valids/working-directory": {
        "state": "FAILED", "taskruns": 3,
        "from": "WorkingDirectoryTest.java:185-188 failed()"},
    "valids/empty-variables": {
        "state": "SUCCESS", "taskruns": 3,
        "from": "EmptyVariablesTest.java:39-41 emptyVariables()"},
    "valids/primitive-labels-flow": {
        "state": "SUCCESS", "taskruns": 1,
        "from": "RuntimeLabelsTest.java:115-116 primitiveTypeLabels()"},
    "valids/labels-update-task": {
        "state": "SUCCESS", "taskruns": 4,
        "from": "RuntimeLabelsTest.java:54-55 update()"},
    "valids/task-runif-workingdirectory": {
        "state": "SUCCESS", "taskruns": 3,
        "task_states": {"log_orders": "SUCCESS", "log_test": "SKIPPED"},
        "from": "TaskWithRunIfTest.java:33-36 runIfWorkingDirectory()"},
    "invalids/loop-switch-failed": {
        "state": "FAILED", "taskruns": 1,
        "from": "RunContextTest.java:344-345 failedTasksVariable()"},
}

# Recovered-input flows for which the checkout makes NO execution-level assertion
# this harness can check.  Listed rather than omitted, so the honest denominator
# in runnable.txt is over all 39 recovered inputs and not only the checkable ones.
NO_ASSERTION = {
    "valids/subflow-function-child":
        "SubflowFunctionTest.java:31 asserts the RENDERED STRING the subflow() Pebble "
        "function returns (\"dc-us\"), never an execution document.",
    "invalids/inputs-with-multiple-constraint-violations":
        "InputsTest.java:476 asserts that reading the inputs THROWS; there is no "
        "execution to check.  The refusal itself is the `rejected` disposition.",
}


SUBEXEC_RE = re.compile(r'\[\[link execution="([^"]+)"\]\]')


# --------------------------------------------------------------------------
# HTTP
# --------------------------------------------------------------------------
def _auth():
    return "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode()


def http(method, url, body=None, content_type=None, timeout=60):
    req = urllib.request.Request(url, data=body, method=method)
    req.add_header("Authorization", _auth())
    if content_type:
        req.add_header("Content-Type", content_type)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        return 0, str(e)


def multipart(fields):
    """Kestra 2.x accepts execution inputs ONLY as multipart/form-data.

    Measured against the running artifact: a JSON body returns
    415 "Content Type [application/json] not allowed. Allowed types:
    [multipart/form-data]".  Scalars go over the wire as their string form;
    JSON/ARRAY inputs go as compact JSON text.
    """
    boundary = "----w3oracle" + str(int(time.time() * 1000))
    out = []
    for k, v in fields.items():
        if isinstance(v, bool):
            s = "true" if v else "false"
        elif isinstance(v, (dict, list)):
            s = json.dumps(v, separators=(",", ":"))
        else:
            s = str(v)
        out.append(f"--{boundary}\r\n")
        out.append(f'Content-Disposition: form-data; name="{k}"\r\n\r\n')
        out.append(s + "\r\n")
    out.append(f"--{boundary}--\r\n")
    return "".join(out).encode(), f"multipart/form-data; boundary={boundary}"


# --------------------------------------------------------------------------
# stack
# --------------------------------------------------------------------------
def compose(*args, check=True):
    cmd = ["docker", "compose", "-p", PROJECT, *args]
    return subprocess.run(cmd, cwd=HERE, check=check)


def image_tag():
    """The pinned image, read from docker-compose.yml.

    Restating the tag in a second place is how a "pinned" artifact quietly stops
    being the one the container ran.  A moving tag is a moving golden, and a
    provenance line that names the wrong image is worse than none.
    """
    src = open(os.path.join(HERE, "docker-compose.yml"), encoding="utf-8").read()
    # Accepts the digest pin the file uses today (`kestra/kestra@sha256:...`) as
    # well as a tag. The tag-only form made `--traces` fail after five rounds,
    # at the final write, because the file had moved to a digest pin.
    m = re.search(r"^\s*image:\s*(kestra/kestra(?::\S+|@sha256:[0-9a-f]{64}))\s*$", src, re.M)
    if not m:
        bail("docker-compose.yml has no `image: kestra/kestra:<tag>` or `@sha256:` line")
    if m.group(1).endswith(":latest"):
        bail(f"docker-compose.yml pins {m.group(1)}: a moving tag is a moving golden")
    return m.group(1)


def pg_container():
    r = subprocess.run(
        ["docker", "compose", "-p", PROJECT, "ps", "-q", "postgres"],
        cwd=HERE, capture_output=True, text=True, check=True,
    )
    return r.stdout.strip().splitlines()[0]


def wait_ready(deadline=300.0):
    """Probe the EXACT authenticated URL and accept only 200.

    A wrong path returns 405 and an unauthenticated call returns 401.  Both look
    exactly like "still starting", so any readiness rule looser than
    `status == 200 on this URL` reports ready when it is not, or never at all.
    """
    url = f"{BASE}/{TENANT}/flows/search?size=1"
    start = time.time()
    while time.time() - start < deadline:
        status, _ = http("GET", url, timeout=10)
        if status == 200:
            print(f"[ready] {url} -> 200 after {time.time() - start:.1f}s")
            return
        time.sleep(3)
    raise SystemExit(f"Kestra never returned 200 on {url} within {deadline}s")


# --------------------------------------------------------------------------
# corpus
# --------------------------------------------------------------------------
def read_corpus():
    entries = []
    for kind in ("valids", "invalids"):
        d = os.path.join(HERE, "corpus", kind)
        for name in sorted(os.listdir(d)):
            # BOTH extensions.  valids/ holds 261 .yaml and 56 .yml, and the 56
            # include the entire retry behaviour matrix
            # (retry-{failed,new-execution}-{task,flow}-{attempts,duration}.yml).
            # Reading only .yaml silently excludes a fifth of the corpus.
            if not (name.endswith(".yaml") or name.endswith(".yml")):
                continue
            path = os.path.join(d, name)
            src = open(path, encoding="utf-8").read()
            fid = re.search(r"^id:\s*(\S+)", src, re.M)
            ns = re.search(r"^namespace:\s*(\S+)", src, re.M)
            entries.append({
                "id": f"{kind}/{os.path.splitext(name)[0]}",
                "path": f"corpus/{kind}/{name}",
                "kind": kind,
                "base": os.path.splitext(name)[0],
                "flow_id": fid.group(1).strip('"') if fid else None,
                "namespace": ns.group(1).strip('"') if ns else None,
                "declares_inputs": bool(re.search(r"^inputs:", src, re.M)),
                "source": src,
            })
    return entries


# rc13 SHORT-CIRCUITS on an unknown task type.  Measured directly against the
# running artifact: an unknown type plus a bad namespace, plus an empty
# `Parallel.tasks`, plus a blank task id, and two unknown types together, ALL
# return exactly one violation -- the first `Invalid type:` -- while each
# co-violation alone is a hard 422 on its own.  `POST /flows/validate` behaves
# identically.
#
# Two consequences, both load-bearing:
#
#   1. Counting violation lines proves NOTHING about co-violations.  An earlier
#      revision required `len(violations) == 1` and this file called that the
#      disambiguation; it never was, because the body can never contain a second
#      line when the first is `Invalid type:`.  The check is kept as a shape
#      check on the message and is no longer described as more.
#   2. `task-type-not-in-image` therefore means "the FIRST violation is a
#      missing test-source class".  It does NOT mean the flow is otherwise
#      valid -- rc13 cannot tell us that, and nothing here may claim it.
INVALID_TYPE_LINE = re.compile(r"^Invalid type: (\S+)$")

PUBLISH_PREFIX = re.compile(r"^publish -> \d+: |^publish -> 200; POST execution -> \d+: ")

TEST_SOURCE_FILE = os.path.join(HERE, "test-source-classes.json")
KESTRA_CHECKOUT = os.path.abspath(os.path.join(HERE, "..", "..", "..", "..", "kestra"))


def load_test_source_classes():
    """FQCN -> the checkout file that declares it.

    This replaces a NAMING HEURISTIC.  A regex over the package name accepted
    `com.acme.totally.made.up.test.NeverExisted` -- measured, it publishes with
    exactly the same `Invalid type:` body -- so any invented FQCN could file into
    a bucket Task 8 ignores.  Membership of a set resolved from the checkout is
    evidence; looking like a test class is not.

    The set is COMMITTED (`test-source-classes.json`) so a harvest needs no
    checkout, and `--refresh-test-source-classes` regenerates it and reports the
    diff.
    """
    with open(TEST_SOURCE_FILE, encoding="utf-8") as f:
        doc = json.load(f)
    classes = doc.get("classes") or {}
    if not classes:
        bail(f"{TEST_SOURCE_FILE} carries no classes; run "
             "`harvest.py --refresh-test-source-classes` against the checkout")
    return classes, doc.get("_checkout")


def scan_test_source_classes(checkout):
    """Resolve every plugin type declared under the checkout's src/test/java.

    A plugin type is a class that extends a Kestra Task/Trigger base or
    implements one of its interfaces, plus any `aliases = "..."` it declares --
    `io.kestra.core.runners.test.trigger.Alias` exists only as an alias on
    core/src/test/java/io/kestra/core/runners/test/TriggerWithAlias.java.
    """
    bases = re.compile(
        r"\b(?:extends\s+(?:Task|AbstractTrigger|Sequential|Parallel|Dag|Subflow|"
        r"WorkingDirectory|AbstractExecutionCondition|\w*Task\b)|"
        r"implements\s+[\w<>, ]*\b(?:RunnableTask|FlowableTask|ExecutableTask|"
        r"PollingTriggerInterface|TriggerInterface|RealtimeTriggerInterface)\b)")
    alias_re = re.compile(r'aliases\s*=\s*\{?\s*"([^"]+)"')
    found = {}
    for root, _dirs, files in os.walk(checkout):
        rp = root.replace(os.sep, "/")
        if "node_modules" in rp or "/build/" in rp or "/.git" in rp:
            continue
        if "/src/test/java/" not in rp:
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
            for cm in re.finditer(r"class\s+(\w+)([^{]*)\{", src):
                name, tail = cm.group(1), cm.group(2)
                if not bases.search(tail):
                    continue
                found[f"{pkg}.{outer}" if name == outer
                      else f"{pkg}.{outer}${name}"] = rel
            for a in alias_re.findall(src):
                found[a] = rel
    return found


def refresh_test_source_classes():
    if not os.path.isdir(KESTRA_CHECKOUT):
        bail(f"no Kestra checkout at {KESTRA_CHECKOUT}")
    found = scan_test_source_classes(KESTRA_CHECKOUT)
    sha = subprocess.run(["git", "-C", KESTRA_CHECKOUT, "rev-parse", "HEAD"],
                         capture_output=True, text=True, check=True).stdout.strip()
    old, old_sha = ({}, None)
    if os.path.exists(TEST_SOURCE_FILE):
        with open(TEST_SOURCE_FILE, encoding="utf-8") as f:
            d = json.load(f)
        old, old_sha = d.get("classes") or {}, d.get("_checkout")
    added = sorted(set(found) - set(old))
    removed = sorted(set(old) - set(found))
    doc = {
        "_checkout": sha,
        "_generated_by": "harvest.py --refresh-test-source-classes",
        "_what": "Kestra plugin types (Task/Trigger subclasses and their `aliases`) "
                 "declared in the checkout's src/test/java. A flow whose only publish "
                 "violation names one of these is refused because the shipped image has "
                 "no such plugin, not because the flow is malformed.",
        "classes": dict(sorted(found.items())),
    }
    with open(TEST_SOURCE_FILE, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    print(f"[test-source] {len(found)} plugin types from {sha}"
          f" (was {len(old)} from {old_sha})")
    for a in added:
        print(f"  + {a}")
    for r in removed:
        print(f"  - {r}")


def violations(why):
    """The individual constraint violations in one 422 body."""
    return [ln.strip() for ln in PUBLISH_PREFIX.sub("", why).splitlines() if ln.strip()]


TEST_SOURCE_CLASSES, TEST_SOURCE_CHECKOUT = load_test_source_classes()


CHECKOUT_DRIFT = {
    "valids/flow-trigger-fire-once-true-flow-listen":
        "The trigger sets `fireOnce`, which the checkout declares at "
        "core/src/main/java/io/kestra/core/models/triggers/Window.java:79 "
        "(`@Builder.Default @PluginProperty @NotNull private boolean fireOnce = false;`) "
        "and v2.0.0-rc13 does not know: `Unrecognized field \"fireOnce\"`.",
}

# A flow that lives under valids/ but that the CHECKOUT refuses too.  This IS
# evidence for Task 8 -- egret must refuse it -- despite the directory it sits in.
# Drift the harness sees at the EXECUTION seam rather than at publish.
# CHECKOUT_DRIFT only ever holds flows rc13 REFUSES, so a flow that publishes,
# runs, and behaves differently from the checkout's own passing test had nowhere
# to be recorded -- it was a comment on one probe row and nothing a later task
# could enumerate.
EXECUTION_DRIFT = {
    # EMPTY since W8 Task 3 fix round 3.  The bucket stays -- it is the only
    # place an execution-seam divergence could be recorded, and CHECKOUT_DRIFT
    # still holds publish-time refusals only -- but the single entry it ever
    # carried has been withdrawn.
    #
    # It claimed `valids/pause-behavior` drifts: literal `behavior` values
    # working while a Pebble expression fails at resume.  Re-measured with a
    # fresh, never-reused flow id per execution (3 runs each), the EXPRESSION
    # form gives FAIL -> FAILED/1, RESUME -> SUCCESS/2, WARN -> WARNING/2,
    # CANCEL -> CANCELLED/1 -- which is exactly what `PauseTest.java:374-380`
    # asserts.  There is no drift at this seam; the earlier reading was
    # contaminated by flow-id reuse, where `execution.inputs` still records the
    # posted value so "the input landed" cannot say which SOURCE ran.
    #
    # Outside the `Behavior` enum the artifact does something this table would
    # be the right home for if a corpus flow reached it: `NOPE`, lower-case
    # `fail` and an undefined `{{inputs.nosuch}}` each leave the execution
    # RUNNING past 120s, non-terminal.  No corpus flow writes one, so the
    # bucket stays empty rather than gaining a row nothing can exercise.
}

REFUSED_BY_BOTH = {
    "valids/invalid-draft-flow":
        "`draft: true` in the YAML body is not what makes a flow a draft: "
        "FlowController.java:350-351 reads `draft` from a QUERY PARAMETER "
        "(`@QueryValue(defaultValue = \"false\") boolean draft`), so a POST without it "
        "is draft=false and the `@NotEmpty` on Flow.java:71 `tasks` applies in the "
        "checkout exactly as in rc13.  Kestra's own "
        "ExecutionControllerRunnerTest.java:209-227 calls this file \"a draft saved with "
        "constraint violations (empty `tasks` violates @NotEmpty)\" and asserts 422.",
}

# The complete vocabulary rejected_class() may return.  write_runnable() refuses
# to print a table containing anything else, so a class added here-but-not-there
# (or a record whose class was dropped) is a hard failure rather than a row that
# quietly reads as drift.
KNOWN_REJECTED_CLASSES = (
    "invalid-flow",
    "refused-by-both",
    "checkout-drift",
    "task-type-not-in-image",
    "unclassified-rejection",
)


def rejected_class(entry, why):
    """Why a 4xx publish happened.  Five classes, each entered on POSITIVE evidence.

      invalid-flow            the flow is in invalids/      (evidence: the directory)
      checkout-drift          curated CHECKOUT_DRIFT        (evidence: a checkout line)
      refused-by-both         curated REFUSED_BY_BOTH       (evidence: a checkout line)
      task-type-not-in-image  the body's only violation is `Invalid type: X` and X is
                              in the checkout-resolved test-source set
      unclassified-rejection  everything else -- an OPEN QUESTION, not drift and not
                              a refusal.  Nothing lands here by default reasoning.

    Task 8 may take TASK8_EVIDENCE as evidence that egret must refuse a flow.
    `task-type-not-in-image` is not on that list, and it is ALSO not evidence
    that the flow is valid: see the short-circuit note above.
    """
    if entry["kind"] == "invalids":
        return "invalid-flow"
    if entry["id"] in CHECKOUT_DRIFT:
        return "checkout-drift"
    if entry["id"] in REFUSED_BY_BOTH:
        return "refused-by-both"
    vs = violations(why)
    if len(vs) == 1:
        m = INVALID_TYPE_LINE.match(vs[0])
        if m and m.group(1) in TEST_SOURCE_CLASSES:
            return "task-type-not-in-image"
    return "unclassified-rejection"


# The classes Task 8 may read as "egret must refuse this flow".  The prose in
# runnable.txt is generated from this tuple and from NON_EVIDENCE_REASON, so a
# class moving between them cannot leave a sentence behind that says the
# opposite -- which is exactly what happened once: the file told Task 8 to ignore
# `refused-by-both`, a class that holds a flow Kestra refuses in both places.
TASK8_EVIDENCE = ("invalid-flow", "refused-by-both")

NON_EVIDENCE_REASON = {
    "checkout-drift":
        "the flow is VALID; the pinned image is older than the checkout",
    "task-type-not-in-image":
        "the FIRST violation is a Kestra test-source class the image does not ship. "
        "rc13 short-circuits on an unknown type, so this says NOTHING about whether "
        "the flow is otherwise valid -- do not read it as `the flow is fine`",
    "unclassified-rejection":
        "UNKNOWN -- not shown to be a refusal and not shown to be drift; read the "
        "raw message before using it for anything",
}

# Every disposition write_runnable() will print.  Anything outside this
# vocabulary is a hard failure, so a record with a novel disposition can never
# be silently dropped from the header table.
KNOWN_DISPOSITIONS = ("runs", "rejected", "not-standalone", "unclassified")

# Every key an EXPECTED row may carry, and the ones that make it an assertion.
EXPECTED_ROW_KEYS = {"state", "taskruns", "task_at", "task_states", "from"}
ASSERTION_KEYS = ("state", "taskruns", "task_at", "task_states")

# Kestra's State.Type, read from the checkout.  Used to reject a typo'd state
# in an EXPECTED row rather than compare against a value nothing can produce.
STATE_TYPES = {
    "CREATED", "SUBMITTED", "RUNNING", "PAUSED", "RESTARTED", "KILLING", "SUCCESS",
    "WARNING", "FAILED", "KILLED", "CANCELLED", "QUEUED", "RETRYING", "RETRIED",
    "SKIPPED", "BREAKPOINT", "RESUBMITTED",
}


def detail(body):
    try:
        d = json.loads(body)
        return d.get("detail") or d.get("message") or body[:300]
    except Exception:
        return body[:300]


def descendants(root_id, pg):
    """Sub-executions are HIDDEN from /executions/search.

    **ONE discovery, not two.  This docstring said "two independent discoveries,
    unioned" and that was false** -- the design spec's §7 item 2, corrected by W6
    Task 8:

      1. the parent's log lines: `SUBEXEC_RE` matches
         `[[link execution="..."]]` and **nothing on the pinned artifact emits
         that shape**.  The live markup carries `flowId=` and `namespace=`
         before the closing `]]`
         (`[[link execution="5Z4dkYmmBkChEpPYNR1HLM" flowId="replay-in-loop"
         namespace="egret.probe4"]]`, seen again by W6 Task 4 while probing a
         `Loop`), so the regex matches neither the decoded message nor the raw
         JSON body, where the quotes are escaped as well.  Measured by running
         the regex over both shapes **with a positive control on the shape it
         was written for**: 0 matches, 0 matches, 1 match.  An arm that returned
         nothing on every input would look exactly like a dead arm, which is
         why the control is part of the measurement.
      2. Kestra's Postgres, walking the generated `parent_id` column
         transitively -- **the only arm that has ever found anything**, and the
         one all 142 recovered sub-executions came from.

    So "neither alone is trusted" describes an intent, not this function: arm 2
    is alone, which is why a psql failure below is fatal rather than degraded.

    **Arm 1 is left in place rather than deleted**, and that is a decision with a
    reason: it is dead *against this artifact's log markup*, which is a
    measurement about the artifact and not about this code, and removing it
    changes what a re-harvest requests -- a change nothing in W6 can run
    end-to-end, since W6 deliberately does not re-harvest.  It belongs to the
    package that makes triggers measurable (see README, *The coverage statement*
    -> the W6 section), together with the `trigger_execution_id` union that
    package needs anyway.  Until then this docstring, not the code shape, is
    what tells a reader how many arms there are.

    **And a dead arm is not the hole here.**  `parent_id` is NULL for a subflow
    child and for anything a trigger creates, so no arm keyed on it can see
    those at all -- README, *Sub-executions are hidden from /executions/search*.
    """
    found = set()

    # ARM 1, AND IT IS DEAD ON THIS ARTIFACT -- measured, see the docstring.
    # `SUBEXEC_RE` matches no shape this Kestra emits, so this GET has never
    # contributed an id.  It is not a fallback for the arm below.
    status, body = http("GET", f"{BASE}/{TENANT}/logs/{root_id}")
    if status == 200:
        found.update(SUBEXEC_RE.findall(body))

    # :'root' is a psql variable, not string interpolation -- the id never reaches
    # the SQL text.
    sql = (
        "WITH RECURSIVE d AS ("
        "  SELECT id FROM executions WHERE parent_id = :'root'"
        "  UNION ALL"
        "  SELECT e.id FROM executions e JOIN d ON e.parent_id = d.id"
        ") SELECT id FROM d;"
    )
    # The SQL goes over STDIN, not -c: psql does not interpolate :'var' into a -c
    # string, so the parameterisation would silently not happen (measured -- it
    # errors with `syntax error at or near ":"`, which is why this fails loudly).
    r = subprocess.run(
        ["docker", "exec", "-i", pg, "psql", "-U", "kestra", "-d", "kestra",
         "-v", "ON_ERROR_STOP=1", "-v", f"root={root_id}", "-tA"],
        input=sql, capture_output=True, text=True,
    )
    # A swallowed psql error degrades sub-execution recovery to the log-scrape arm
    # alone, silently, and the harvest still writes a plausible corpus.json.  Fail.
    if r.returncode != 0:
        raise SystemExit(
            f"psql failed while walking parent_id for {root_id} "
            f"(exit {r.returncode}): {r.stderr.strip()[:500]}"
        )
    found.update(x.strip() for x in r.stdout.split() if x.strip())

    found.discard(root_id)
    return found


def settle(root, pg, timeout):
    """Wait until the whole execution TREE is finished, not merely terminal.

    Returns (docs_by_id, error).  This is the single settle rule; Task 1's
    harvest and Task 2's capture both go through it, because two harvests with
    two rules is how `corpus.json` came to say 133 sub-executions while the
    verified capture said 142.

    **Terminal is not the same as finished.**  The obvious loop -- discover
    descendants, wait for the pending ones, stop when none are pending -- stops
    immediately when the first discovery returns nothing, so a row that lands a
    beat later is simply absent, silently, with a plausible count written.  That
    cost every `retry-new-execution-*` flow BOTH of its executions (four flows x
    two each = 8) and `valids/loop-with-subflow` one of its two iterations: nine.

    `RETRIED` is Kestra saying a replacement execution is coming.  Over 40
    observations (four flows x five rounds x two children) every `RETRIED`
    execution had exactly one child and every other terminal state had none, and
    the replacement's `startDate` ran 1.07 s to 5.92 s after the `RETRIED`
    execution's `endDate`.  So that arm is exact rather than a sleep: settle only
    when nothing is pending AND no `RETRIED` execution is childless.

    The THIRD arm is a quiescence condition and is labelled as one, because no
    exact signal exists for it.  A serial `Loop` creates iteration k+1 after k
    finishes, and nothing the API returns says how many iterations there will be:
    the parent's taskrun carries no count and `loopRun` carries only `index`.
    Measured, the race is real but narrow -- `valids/loop-with-subflow` recorded
    1 of its 2 iterations once, under the load of a 345-flow harvest, while 20
    quiet repeats had the second iteration visible 0.26 s to 0.62 s BEFORE
    everything looked terminal.  So settle also requires the (id, state) snapshot
    to be observed unchanged across two consecutive polls, which bounds the race
    at one poll interval instead of ending it.  It is not exact and is not
    described as exact.
    """
    deadline = time.time() + timeout
    last_snapshot = None
    while True:
        docs, pending = {}, set()
        for eid in {root} | descendants(root, pg):
            s, b = http("GET", f"{BASE}/{TENANT}/executions/{eid}")
            if s != 200:
                pending.add(eid)
                continue
            d = json.loads(b)
            if d.get("state", {}).get("current") in TERMINAL:
                docs[eid] = d
            else:
                pending.add(eid)
        parents = {d.get("parentId") for d in docs.values()}
        childless = {eid for eid, d in docs.items()
                     if d.get("state", {}).get("current") == "RETRIED" and eid not in parents}
        snapshot = frozenset((eid, d.get("state", {}).get("current"))
                             for eid, d in docs.items())
        if not pending and not childless and snapshot == last_snapshot:
            return docs, None
        last_snapshot = snapshot
        if time.time() >= deadline:
            return docs, (
                f"{len(pending)} sub-execution(s) never reached a terminal state"
                if pending else
                f"{len(childless)} RETRIED execution(s) never got the replacement "
                "execution that state promises")
        time.sleep(POLL)


def sub_order(doc):
    """Sort key for sibling sub-executions: `loopRun.index`, never creation instant.

    A concurrent loop creates its sub-executions in an order that flipped between
    rounds on nine corpus flows -- a fact about the scheduler, not about the flow.
    Anything with no `loopRun` sorts after, by id, so the order is total.
    """
    loop = doc.get("loopRun") or {}
    idx = loop.get("index")
    return (0, idx, doc.get("id")) if isinstance(idx, int) else (1, 0, doc.get("id"))


def await_terminal(eid, timeout):
    start = time.time()
    last = None
    while time.time() - start < timeout:
        status, body = http("GET", f"{BASE}/{TENANT}/executions/{eid}")
        if status != 200:
            return None, f"GET execution {eid} -> {status}"
        doc = json.loads(body)
        last = doc.get("state", {}).get("current")
        if last in TERMINAL:
            return doc, None
        time.sleep(POLL)
    return None, f"still {last} after {timeout:.0f}s"


def observe(doc):
    """The values EXPECTED rows are checked against, straight off the API document.

    `task_ids` is creation order, which spec 3.3 measured as stable; it is recorded
    so the discrimination probe can see a branch flip even when the state and the
    task count both survive it.
    """
    trl = doc.get("taskRunList") or []
    return {
        "state": doc.get("state", {}).get("current"),
        "taskruns": len(trl),
        "task_ids": [t.get("taskId") for t in trl],
        "task_states": {t.get("taskId"): t.get("state", {}).get("current") for t in trl},
    }


def check_row(row, obs):
    """Compare one EXPECTED row against an observation. Returns list of failures."""
    bad = []
    if "state" in row and obs["state"] != row["state"]:
        bad.append(f"state {obs['state']} != {row['state']}")
    if "taskruns" in row and obs["taskruns"] != row["taskruns"]:
        bad.append(f"taskruns {obs['taskruns']} != {row['taskruns']}")
    for i, tid in row.get("task_at", {}).items():
        got = obs["task_ids"][i] if i < len(obs["task_ids"]) else None
        if got != tid:
            bad.append(f"task[{i}] {got} != {tid}")
    for tid, st in row.get("task_states", {}).items():
        got = obs["task_states"].get(tid)
        if got != st:
            bad.append(f"task {tid} {got} != {st}")
    return bad


def run_flow(entry, pg):
    """POST the execution, wait for it and everything it spawned."""
    ns, fid = entry["namespace"], entry["flow_id"]
    spec = INPUTS.get(entry["id"], {})
    inputs = spec.get("inputs", {})

    if inputs:
        body, ctype = multipart(inputs)
    else:
        body, ctype = multipart({})
    status, resp = http("POST", f"{BASE}/{TENANT}/executions/{ns}/{fid}", body, ctype)
    if status != 200:
        # An `invalids` flow refused at the EXECUTION seam is still Kestra refusing it.
        # Publish-time and execution-time validation are two different gates; filing the
        # second as "not-standalone" would hide it from Task 8.
        if entry["kind"] == "invalids" and 400 <= status < 500:
            return "rejected", f"publish -> 200; POST execution -> {status}: {detail(resp)}", None, [], None
        return "not-standalone", f"POST execution -> {status}: {detail(resp)}", None, [], None

    root = json.loads(resp)["id"]
    doc, err = await_terminal(root, EXEC_TIMEOUT)
    if doc is None:
        reason = f"execution {root} {err}"
        if "still PAUSED" in err:
            src = entry["source"]
            # The rule is BOTH bounds, not just pauseDuration.  Three corpus flows
            # (pause-timeout, pause-timeout-allow-failure,
            # pause-errors-finally-after-execution) also declare no pauseDuration and
            # DO terminate -- they carry a `timeout:` on the Pause task instead.  A
            # reason that named only pauseDuration would be contradicted by its own
            # corpus.
            if ("core.flow.Pause" in src
                    and "pauseDuration" not in src and "timeout:" not in src):
                reason = (
                    f"reached PAUSED and stayed there for the full {EXEC_TIMEOUT:.0f}s: the flow's "
                    "io.kestra.plugin.core.flow.Pause task declares neither `pauseDuration` nor "
                    "`timeout`, so nothing bounds the pause and it ends only on an explicit resume, "
                    "which a standalone harvest does not issue.  Kestra's own tests drive these with "
                    "runOneUntilPaused(...) followed by executionService.resume(...).  "
                    f"(execution {root})"
                )
        return "not-standalone", reason, root, [], None

    docs, err = settle(root, pg, SUB_TIMEOUT)
    if err:
        # No `observed` on a non-`runs` record: an observation is the harness
        # saying "this finished", and record_invariant_problems enforces that a
        # flipped disposition contradicts it.
        return "not-standalone", err, root, [], None

    sub_states = [{"id": d.get("id"), "state": d.get("state", {}).get("current")}
                  for d in sorted((docs[k] for k in docs if k != root), key=sub_order)]

    return "runs", doc["state"]["current"], root, sub_states, observe(doc)


def expected_row_problems():
    """An EXPECTED row is DATA that decides what gets checked, so its shape has to
    be checked too.

    `"taskrun": 999` -- a typo of `taskruns` -- was silently ignored: check_row
    never looks for it, the row still printed `agrees`, and the `[state,taskruns]`
    label came from the same lookup that missed it.  That is the same
    invisible-by-construction shape as a bucket that collects by default.
    """
    bad = []
    for cid, row in sorted(EXPECTED.items()):
        unknown = set(row) - EXPECTED_ROW_KEYS
        if unknown:
            bad.append(f"EXPECTED[{cid!r}] has unknown key(s) {sorted(unknown)}; "
                       f"allowed: {sorted(EXPECTED_ROW_KEYS)}")
        if not isinstance(row.get("from"), str) or not row.get("from"):
            bad.append(f"EXPECTED[{cid!r}] has no `from` provenance string")
        if not any(k in row for k in ASSERTION_KEYS):
            bad.append(f"EXPECTED[{cid!r}] asserts nothing; it would always agree")
        if "state" in row and row["state"] not in STATE_TYPES:
            bad.append(f"EXPECTED[{cid!r}].state {row['state']!r} is not a State.Type")
        if "taskruns" in row and not (isinstance(row["taskruns"], int)
                                      and not isinstance(row["taskruns"], bool)
                                      and row["taskruns"] >= 0):
            bad.append(f"EXPECTED[{cid!r}].taskruns {row['taskruns']!r} is not a count")
        if "task_at" in row:
            if not isinstance(row["task_at"], dict):
                bad.append(f"EXPECTED[{cid!r}].task_at is not a dict")
            else:
                for i, tid in row["task_at"].items():
                    if not isinstance(i, int) or not isinstance(tid, str):
                        bad.append(f"EXPECTED[{cid!r}].task_at[{i!r}] is not int -> str")
        if "task_states" in row:
            if not isinstance(row["task_states"], dict):
                bad.append(f"EXPECTED[{cid!r}].task_states is not a dict")
            else:
                for tid, st in row["task_states"].items():
                    if not isinstance(tid, str) or st not in STATE_TYPES:
                        bad.append(f"EXPECTED[{cid!r}].task_states[{tid!r}] = {st!r} "
                                   "is not a task id -> State.Type")
    return bad


def validate_tables(entries):
    """Every table key must name a real corpus entry.

    The tables and the corpus are keyed the same way -- `valids/x`, `invalids/y`,
    extension stripped -- so a typo, a renamed flow, or a row added under an old
    key is a hard error here instead of a row that silently never fires.
    """
    ids = {e["id"] for e in entries}
    problems = []
    for name, table in (("INPUTS", INPUTS),
                        ("NOT_STANDALONE_INPUTS", NOT_STANDALONE_INPUTS),
                        ("EXPECTED", EXPECTED),
                        ("NO_ASSERTION", NO_ASSERTION),
                        ("CHECKOUT_DRIFT", CHECKOUT_DRIFT),
                        ("REFUSED_BY_BOTH", REFUSED_BY_BOTH),
                        # NON_DISCRIMINATING_NOTES was missing from this list, so a
                        # typo'd key there was a note that could never print.
                        ("NON_DISCRIMINATING_NOTES", NON_DISCRIMINATING_NOTES),
                        ("EXECUTION_DRIFT", EXECUTION_DRIFT)):
        for k in table:
            if k not in ids:
                problems.append(f"{name}[{k!r}] names no corpus flow")
    problems.extend(expected_row_problems())
    overlap = set(INPUTS) & set(NOT_STANDALONE_INPUTS)
    if overlap:
        problems.append(f"in both INPUTS and NOT_STANDALONE_INPUTS: {sorted(overlap)}")
    declared = {e["id"] for e in entries if e["declares_inputs"] and e["kind"] == "valids"}
    uncovered = declared - set(INPUTS) - set(NOT_STANDALONE_INPUTS)
    if uncovered:
        problems.append(f"declare inputs: but are in neither table: {sorted(uncovered)}")
    if problems:
        raise SystemExit("table/corpus mismatch:\n  " + "\n  ".join(problems))


def assert_clean(entries):
    """The numbers depend on a clean start, so assert it rather than document it.

    Re-publishing an existing flow returns `422 Flow id already exists`, which
    would be filed `rejected`.  A full re-run against leftover state fails
    loudly; a PARTIAL leftover produces a plausible corpus.json with a quietly
    wrong denominator.  That is the failure this refuses to allow.

    "Empty" cannot mean zero rows: the image seeds 9 flows of its own into the
    `tutorial` namespace on first boot (measured on v2.0.0-rc13).  So the
    assertion is the precise one -- no CORPUS flow may already be published --
    and the seeded flows are reported rather than tolerated silently.
    """
    status, body = http("GET", f"{BASE}/{TENANT}/flows/search?size=1000")
    if status != 200:
        raise SystemExit(f"pre-flight: /flows/search -> {status}")
    doc = json.loads(body)
    present = {(r["namespace"], r["id"]) for r in doc.get("results", [])}
    if doc.get("total", 0) > len(present):
        raise SystemExit(
            f"pre-flight: {doc['total']} flows exist but only {len(present)} were "
            "listed; raise the page size before trusting this check."
        )
    mine = {(e["namespace"], e["flow_id"]) for e in entries}
    clash = sorted(present & mine)
    if clash:
        raise SystemExit(
            f"pre-flight: {len(clash)} corpus flow(s) are ALREADY published, e.g. "
            f"{clash[:3]}.  This harvest's counts are only meaningful from a clean "
            "Kestra -- run `python3 harvest.py` without --no-down, or `--down` first."
        )
    print(f"[pre-flight] {len(present)} pre-existing flow(s), none of them corpus flows"
          + (f" (image-seeded namespaces: "
             f"{sorted({ns for ns, _ in present})})" if present else ""))


# Why a row still cannot discriminate after every documented alternative was
# executed.  Written only where the harvest itself showed the cause.
NON_DISCRIMINATING_NOTES = {
    # EMPTY since W8 Task 3 fix round 3, and empty is the honest state rather
    # than an omission.  Its one entry was `valids/pause-behavior`, asserting
    # that all four documented `behavior` values produce the identical
    # FAILED/1 execution and diagnosing a Pebble-render failure.  Both halves
    # are FALSE -- re-measured on the same pinned digest with a fresh,
    # never-reused flow id per execution, 3 runs each: FAIL -> FAILED/1,
    # RESUME -> SUCCESS/2, WARN -> WARNING/2, CANCEL -> CANCELLED/1.
    #
    # The note is DELETED rather than corrected because its consumer is gone:
    # this table is read only on the non-discriminating branch, and that row
    # discriminates now.  A note nothing can print is the shape this table's
    # own key-validation guard was added for.
    #
    # **Why it had to be edited HERE and not annotated elsewhere:**
    # `runnable.txt` is a pure function of `corpus.json` + `probe.json` through
    # `--rewrite-runnable`, and this table feeds the same generator.  Prose
    # beside a falsehood does not stop a regeneration re-emitting it; only
    # correcting the input does.
}


def probe_discrimination(entries, out, pg):
    """Re-run each checkable flow with a DOCUMENTED ALTERNATIVE input and require
    the observation to change.

    This is what turns "the cross-check agrees" into a claim worth making.  A row
    whose flow ends SUCCESS on either branch, and whose assertions cannot tell the
    branches apart, passes with or without the right input -- exactly the shape
    this programme keeps finding.  Here the alternative is actually executed and
    the two observations are compared on the row's own asserted fields.
    """
    by_id = {e["id"]: e for e in entries}
    recs = {r["id"]: r for r in out}
    result = {}
    for cid, row in sorted(EXPECTED.items()):
        rec = recs.get(cid)
        if rec is None or rec.get("disposition") != "runs" or not rec.get("observed"):
            continue
        alts = INPUTS.get(cid, {}).get("alt") or []
        primary = INPUTS.get(cid, {}).get("inputs")
        usable = [a for a in alts if a["inputs"] != primary]
        if not alts:
            result[cid] = {"probed": False,
                           "why": "no alternative input is documented for this flow"}
            continue
        if not usable:
            result[cid] = {"probed": False,
                           "why": f"the {len(alts)} documented alternative(s) pass the SAME "
                                  "input values (they vary something else, e.g. the execution "
                                  "labels), so none of them can move the observation"}
            continue

        # EVERY alternative, not just the first: `if-condition`'s first alternative
        # is {"param": "true"}, which is the same truthy branch as {"param": true}.
        # A probe that stopped there would report the row as unable to discriminate
        # when the corpus in fact contains {"param": false}, which flips the branch.
        e = by_id[cid]
        tried = []
        last_obs = None
        verdict = None
        for alt in usable:
            body, ctype = multipart(alt["inputs"])
            status, resp = http("POST", f"{BASE}/{TENANT}/executions/"
                                        f"{e['namespace']}/{e['flow_id']}", body, ctype)
            if status != 200:
                verdict = {"probed": True, "differs": True, "basis": "refused",
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": f"the alternative input is REFUSED ({status}: "
                                  f"{detail(resp)[:120]}), so a swap is visible"}
                break
            eid = json.loads(resp)["id"]
            doc, err = await_terminal(eid, EXEC_TIMEOUT)
            if doc is None:
                verdict = {"probed": True, "differs": True, "basis": "no-terminal",
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": f"the alternative input does not terminate ({err}), "
                                  "so a swap is visible"}
                break
            alt_obs = observe(doc)
            bad = check_row(row, alt_obs)
            tried.append(json.dumps(alt["inputs"]))
            last_obs = alt_obs
            if bad:
                # `alt_observed` is recorded so `differs` is RE-DERIVABLE.  It used
                # to be a raw boolean, and flipping every one of them true -> false
                # took "proven discriminating 17" to 0 at exit 0.
                verdict = {"probed": True, "differs": True, "basis": "observed",
                           "alt_observed": alt_obs,
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": "; ".join(bad)}
                break
        if verdict is None:
            verdict = {"probed": True, "differs": False, "basis": "observed",
                       "alt_observed": last_obs,
                       "alt_inputs": [a["inputs"] for a in usable],
                       "alt_from": "; ".join(a["from"] for a in usable),
                       "why": f"all {len(tried)} documented alternative(s) satisfy this row's "
                              f"assertions too: {', '.join(tried)}"}
            note = NON_DISCRIMINATING_NOTES.get(cid)
            if note:
                verdict["why"] += "  " + note
        result[cid] = verdict
        print(f"[probe] {cid:<58} "
              f"{'discriminates' if result[cid]['differs'] else 'CANNOT DISCRIMINATE'}")
    return result


def load_committed():
    """Read corpus.json + probe.json for the offline rewrite, and CHECK them.

    Regenerating runnable.txt offline is what stops a defect in the summary from
    costing a multi-hour re-harvest.  But the rewrite path never touches
    `corpus/`, so with 56 entries deleted from corpus.json it happily reproduced
    the pre-fix header -- `total flows 289 / runs 218` -- exit 0, no warning.  A
    reproducibility tool that can reproduce the defect is not one.
    """
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as f:
        out = json.load(f)
    out = out if isinstance(out, list) else out.get("entries", [])
    with open(os.path.join(HERE, "probe.json"), encoding="utf-8") as f:
        probe = json.load(f)

    entries = read_corpus()
    # The guards `--self-test` exercises must also run on the path that actually
    # WRITES the published file.  They did not: `--rewrite-runnable` called
    # neither validate_tables nor expected_row_problems.
    validate_tables(entries)

    corpus_ids = {e["id"] for e in entries}
    json_ids = {r["id"] for r in out}
    if corpus_ids != json_ids:
        missing = sorted(corpus_ids - json_ids)[:5]
        extra = sorted(json_ids - corpus_ids)[:5]
        bail(f"corpus.json covers {len(json_ids)} flows but corpus/ holds "
             f"{len(corpus_ids)}; missing from corpus.json e.g. {missing}, "
             f"not in corpus/ e.g. {extra}.  Re-harvest; do not rewrite.")

    check_outputs(out, probe)
    return out, probe


def self_test():
    """Prove each guard FIRES, and fires for the RIGHT REASON, by mutating the
    committed data.

    Two defects this rewrite fixes, both of which made the previous version look
    stronger than it was:

    * It drove its first cases through `write_runnable`, which validates AND
      writes `runnable.txt`.  With a guard neutered so the mutation did not fire,
      the run that reports a missing guard left the committed artifact corrupted
      -- `TOTAL 344`, one flow absent -- and then exited 1.  Every case now goes
      through `check_outputs`, which writes nothing.
    * `must_bail` accepted ANY SystemExit, so a case labelled for guard X passed
      when guard Y fired.  Deleting the vocabulary guard left case 1 still
      printing FIRES.  Each case now asserts on the message.
    """
    import copy
    out, probe = load_committed()
    results = []

    def case(name, expect, fn):
        try:
            fn()
        except SystemExit as e:
            msg = str(e)
            if expect in msg:
                hit = next((ln.strip() for ln in msg.splitlines() if expect in ln), msg)
                print(f"  FIRES   {name}")
                print(f"            {hit[:150]}")
                results.append(True)
            else:
                print(f"  WRONG   {name}")
                print(f"            expected {expect!r} in the message, got: "
                      f"{msg.splitlines()[0][:150]}")
                results.append(False)
            return
        print(f"  SILENT  {name}  <-- the guard did not fire")
        results.append(False)

    def mutated(fn):
        m = copy.deepcopy(out)
        fn(m)
        return m

    print("[self-test] every guard must fire, and name its own reason")

    # --- the disposition vocabulary and the record invariants -----------------
    case("disposition outside the vocabulary",
         "is not in KNOWN_DISPOSITIONS",
         lambda: check_outputs(mutated(lambda m: m[0].update(disposition="probably-fine")),
                               probe))

    def flip_to_runs(m, frm):
        r = next(x for x in m if x["disposition"] == frm)
        r["disposition"] = "runs"
        r.pop("rejected_class", None)
        return r
    case("not-standalone flipped to runs (moves the headline)",
         "disposition runs but no `observed`",
         lambda: check_outputs(mutated(lambda m: flip_to_runs(m, "not-standalone")), probe))
    case("rejected flipped to runs with its class dropped",
         "disposition runs but publish_status",
         lambda: check_outputs(mutated(lambda m: flip_to_runs(m, "rejected")), probe))
    case("sub_executions tripled",
         "sub_executions",
         lambda: check_outputs(mutated(lambda m: [
             r.update(sub_executions=r["sub_executions"] * 3)
             for r in m if r.get("sub_executions")][0] and None), probe))
    case("an observed state corrupted",
         "observed.state",
         lambda: check_outputs(mutated(lambda m: next(
             r for r in m if r.get("observed"))["observed"].update(state="NOPE")), probe))
    case("a non-rejected record carrying a stale rejected_class",
         "stale rejected_class",
         lambda: check_outputs(mutated(lambda m: next(
             r for r in m if r["disposition"] == "runs").update(
                 rejected_class="invalid-flow")), probe))

    # --- the rejected-class vocabulary and its re-derivation ------------------
    for label, mutate in (
        ("rejected_class = None", lambda r: r.update(rejected_class=None)),
        ("rejected_class deleted", lambda r: r.pop("rejected_class", None)),
        ("rejected_class = ''", lambda r: r.update(rejected_class="")),
        ("rejected_class unknown", lambda r: r.update(rejected_class="looks-fine")),
    ):
        case(label, "is not a known class",
             lambda mutate=mutate: check_outputs(mutated(lambda m: mutate(
                 next(r for r in m if r["disposition"] == "rejected"))), probe))
    case("a stored class the classifier disagrees with",
         "the classifier derives",
         lambda: check_outputs(mutated(lambda m: next(
             r for r in m if r.get("rejected_class") == "task-type-not-in-image").update(
                 rejected_class="checkout-drift")), probe))

    # --- the probe verdicts ---------------------------------------------------
    def flip_differs(pr, basis):
        return {k: dict(v, differs=False)
                if v.get("differs") and v.get("basis") == basis else v
                for k, v in pr.items()}
    case("every observed probe `differs` flipped true -> false",
         "re-deriving it from alt_observed",
         lambda: check_outputs(out, flip_differs(probe, "observed")))
    case("a refused-basis probe `differs` flipped true -> false",
         "must be discriminating",
         lambda: check_outputs(out, flip_differs(probe, "refused")))
    case("probe.json missing a row that ran",
         "has no record for",
         lambda: check_outputs(out, {k: v for k, v in list(probe.items())[5:]}))
    case("probe.json with a key naming no probed flow",
         "which is not a flow that ran",
         lambda: check_outputs(out, dict(probe, **{"valids/parallel": {"probed": False}})))

    case("a terminal state that disagrees with Task 2's capture",
         "rounds of traces.json say",
         lambda: check_outputs(mutated(lambda m: next(
             r for r in m if r["id"] == "valids/loop-with-subflow")["observed"].update(
                 state="FAILED")), probe))

    # --- corpus.json against corpus/ -----------------------------------------
    case("corpus.json short of corpus/",
         "corpus.json covers",
         lambda: _corpus_cover_check([r for r in out
                                      if not r["id"].endswith("-duration")]))

    # --- the EXPECTED row shapes ---------------------------------------------
    saved = dict(EXPECTED)
    try:
        cid = "valids/exit"
        EXPECTED[cid] = dict(saved[cid], taskrun=999)
        case("EXPECTED row with a typo'd key", "has unknown key(s)",
             lambda: _raise_if(expected_row_problems()))
        EXPECTED[cid] = dict(saved[cid], state="SUKCESS")
        case("EXPECTED row whose state is not a State.Type", "is not a State.Type",
             lambda: _raise_if(expected_row_problems()))
    finally:
        EXPECTED.clear()
        EXPECTED.update(saved)

    # --- Task 2's two artifacts against each other ----------------------------
    # Through `order_table`, which validates and writes nothing -- the same rule
    # this rewrite applied to every case above.
    golden, stab = load_task2()

    def mutated_golden(fn):
        g = copy.deepcopy(golden)
        fn(g)
        return g

    case("a golden entry with no stability record",
         "have no stability record",
         lambda: order_table(golden, {**stab, "flows": {
             k: v for k, v in stab["flows"].items()
             if k != next(e["id"] for e in golden if e["verdict"] == "runs")}}))
    case("golden run counts stability.json cannot account for",
         "run counts give",
         lambda: order_table(mutated_golden(lambda g: next(
             e for e in g if e["verdict"] == "runs" and e["runs"])["runs"].append(
                 {"task": "x", "value": None, "parent": None,
                  "state": ["CREATED"], "attempts": []})), stab))
    case("golden order pairs the stability filter did not keep",
         "the stability filter and the golden writer disagree",
         lambda: order_table(mutated_golden(lambda g: next(
             e for e in g if e["verdict"] == "runs" and e["order"])["order"].pop()), stab))
    case("an order relation outside the vocabulary",
         "is not a known relation",
         lambda: order_table(mutated_golden(lambda g: next(
             e for e in g if e["verdict"] == "runs" and e["order"])["order"][0].__setitem__(
                 2, "sort-of-before")), stab))

    # --- the curated tables ---------------------------------------------------
    NON_DISCRIMINATING_NOTES["valids/does-not-exist"] = "x"
    try:
        case("NON_DISCRIMINATING_NOTES key naming no flow", "names no corpus flow",
             lambda: validate_tables(read_corpus()))
    finally:
        NON_DISCRIMINATING_NOTES.pop("valids/does-not-exist", None)
    EXECUTION_DRIFT["valids/does-not-exist"] = "x"
    try:
        case("EXECUTION_DRIFT key naming no flow", "names no corpus flow",
             lambda: validate_tables(read_corpus()))
    finally:
        EXECUTION_DRIFT.pop("valids/does-not-exist", None)

    print(f"[self-test] {sum(results)}/{len(results)} guards fired for the right reason")
    if not all(results):
        raise SystemExit("[self-test] at least one guard did not fire correctly")


def _corpus_cover_check(out):
    corpus_ids = {e["id"] for e in read_corpus()}
    json_ids = {r["id"] for r in out}
    if corpus_ids != json_ids:
        bail(f"corpus.json covers {len(json_ids)} flows but corpus/ holds "
             f"{len(corpus_ids)}")


def _raise_if(problems):
    if problems:
        bail("; ".join(problems))


# --------------------------------------------------------------------------
# raw execution traces -- Task 2's input
# --------------------------------------------------------------------------
# corpus.json records `observed` (state, taskrun count, task ids, task states):
# enough to cross-check a recovered input, nowhere near enough to build a
# semantic trace.  Task 2 needs the histories, the attempts, the parent taskrun
# links and the sub-executions, so `--traces N` re-executes every `runs` flow N
# times and writes the REDUCED raw documents to traces.json.  normalise.py turns
# those into golden.json; nothing downstream talks to Kestra.
#
# N is 5 because the happens-before relation is only kept where it is stable
# across all five (plan Task 2 step 1 rule 2): a relation observed once is a
# coincidence, and genuinely parallel branches flip.
TRACE_ROUNDS = 5

# The taskrun-level state vocabulary the reduction keeps VERBATIM.  Measured on
# the artifact, and wider than the executor's draft `RunState`: a worker task
# passes through SUBMITTED, which no flowable does, and a retried one repeats
# CREATED.  Dropping SUBMITTED would be normalising away the one field that
# distinguishes a task the worker ran from a flowable the executor ran.
TRACE_STATE_KEYS = ("state", "date")


def reduce_state(st):
    """One `state` object: the current value, the history, and the two clocks.

    The clocks are kept HERE and dropped in normalise.py, on purpose.  The
    happens-before relation is derived from them, and a derivation whose input
    is not committed cannot be audited or re-run offline -- which is the same
    property `--rewrite-runnable` exists to give runnable.txt.
    """
    st = st or {}
    return {
        "current": st.get("current"),
        "histories": [[h.get("state"), h.get("date")] for h in st.get("histories") or []],
        "startDate": st.get("startDate"),
        "endDate": st.get("endDate"),
    }


def spawn_link(doc):
    """Which of the PARENT's taskruns spawned this sub-execution, and with what.

    A LOOP sub-execution's `loopRun` names all three things the trace needs:
    `taskRunId` (the flowable in the PARENT execution that spawned it), `index`
    (its position in the loop's values) and `value` (the iteration value itself).
    Measured on valids/loop-object:
    `{"taskId": "1_each", "taskRunId": "2uNX...", "index": 0, "value": "value 1"}`.

    `index` is the only stable order for siblings.  Creation instant is NOT: a
    concurrent loop creates its sub-executions in an order that flipped between
    rounds on nine corpus flows, which is a fact about the scheduler and not
    about the flow.

    The `parent.taskRunList` last entry says the same thing and is CROSS-CHECKED
    rather than used: reading a link off the wrong field would put
    sub-executions under the wrong run, and once the ids are gone that reads
    exactly like a measurement.  Anything with no `loopRun` returns None rather
    than a guess.
    """
    loop = doc.get("loopRun") or {}
    by = loop.get("taskRunId")
    trl = (loop.get("parent") or {}).get("taskRunList") or []
    if by and trl and trl[-1].get("id") != by:
        bail(f"execution {doc.get('id')}: loopRun.taskRunId is {by} but "
             f"loopRun.parent.taskRunList's last entry is {trl[-1].get('id')}; "
             "the two ways of reading the spawn link disagree")
    return by, loop.get("index"), loop.get("value")


def reduce_execution(doc):
    """Everything normalise.py reads, and nothing else.

    Outputs, labels, worker ids, revisions and the tenant are dropped at capture
    time: they are Kestra's record-keeping, not its semantics, and the Global
    Constraints say the comparison is on a semantic trace and never on Kestra's
    JSON layout.
    """
    runs = []
    for t in doc.get("taskRunList") or []:
        runs.append({
            "id": t.get("id"),
            "taskId": t.get("taskId"),
            "parentTaskRunId": t.get("parentTaskRunId"),
            "value": t.get("value"),
            "state": reduce_state(t.get("state")),
            "attempts": [reduce_state(a.get("state")) for a in t.get("attempts") or []],
        })
    by, index, value = spawn_link(doc)
    return {
        "id": doc.get("id"),
        "parentId": doc.get("parentId"),
        "kind": doc.get("kind"),
        "flowId": doc.get("flowId"),
        "namespace": doc.get("namespace"),
        "spawnedBy": by,
        "loopIndex": index,
        "loopValue": value,
        "state": reduce_state(doc.get("state")),
        "runs": runs,
    }


def execute_once(entry, inputs, pg):
    """One execution of one flow, with everything it spawned.

    Returns (root_reduced, [descendant_reduced], error).  `error` is a string
    whenever the root or anything under it did not settle inside the same bounds
    the Task 1 harvest used -- never a partial trace passed off as a whole one.

    **Terminal is not the same as finished.**  The obvious loop -- discover the
    descendants once, wait for them, stop when none are pending -- stops
    immediately when the first discovery returns nothing, and a sub-execution
    whose row lands a beat later is then simply absent, silently, with a
    plausible trace written.  That is measured, not feared: it cost every
    `retry-new-execution-*` flow BOTH of its executions in the first capture,
    and `--verify-traces` is what found it.

    The signal is exact.  `RETRIED` is Kestra saying a replacement execution is
    coming, and over 40 observations (four flows x five rounds x two children)
    **every** `RETRIED` execution had exactly one child and every other terminal
    state had none.  The replacement's `startDate` ran 1.07 s to 5.92 s after the
    `RETRIED` execution's `endDate`, so waiting for it needs a rule and not a
    sleep: settle when nothing is pending AND no `RETRIED` execution is
    childless.
    """
    ns, fid = entry["namespace"], entry["flow_id"]
    body, ctype = multipart(inputs or {})
    status, resp = http("POST", f"{BASE}/{TENANT}/executions/{ns}/{fid}", body, ctype)
    if status != 200:
        return None, [], f"POST execution -> {status}: {detail(resp)}"

    root = json.loads(resp)["id"]
    doc, err = await_terminal(root, EXEC_TIMEOUT)
    if doc is None:
        return None, [], f"execution {root} {err}"

    # One settle rule for the whole package -- see settle().  Task 1's harvest
    # and this capture used to have separate loops, which is how corpus.json came
    # to say 133 sub-executions while this capture said 142.
    docs, why = settle(root, pg, SUB_TIMEOUT)
    if why:
        return None, [], why

    subs = sorted((docs[k] for k in docs if k != root), key=sub_order)
    return reduce_execution(docs[root]), [reduce_execution(d) for d in subs], None


def capture_traces(rounds):
    """Execute every `runs` flow `rounds` times and write traces.json.

    Rounds are ROUND-major, not flow-major: round 1 runs all 268 flows before
    round 2 starts any.  Five back-to-back executions of one flow share a warm
    JVM, a warm page cache and a quiet queue, which is exactly the condition
    under which a race does not show -- and the stability filter this feeds is
    only worth anything if the five samples are not correlated by construction.
    """
    out, _ = load_committed()
    entries = {e["id"]: e for e in read_corpus()}
    validate_tables(list(entries.values()))
    assert_clean(list(entries.values()))

    wanted = [r for r in out if r.get("disposition") == "runs"]
    print(f"[traces] {len(wanted)} `runs` flows x {rounds} rounds")

    published = 0
    for e in entries.values():
        status, _ = http("POST", f"{BASE}/{TENANT}/flows",
                         e["source"].encode("utf-8"), "application/x-yaml")
        published += 1 if status == 200 else 0
    print(f"[traces] published {published}/{len(entries)} flows")

    pg = pg_container()
    flows = {r["id"]: [] for r in wanted}
    for rnd in range(1, rounds + 1):
        t0 = time.time()
        for i, rec in enumerate(wanted, 1):
            e = entries[rec["id"]]
            root, subs, err = execute_once(e, rec.get("inputs") or {}, pg)
            flows[rec["id"]].append(
                {"round": rnd, "error": err, "root": root, "descendants": subs})
            if err or i % 50 == 0:
                print(f"[traces r{rnd}] [{i:3}/{len(wanted)}] {rec['id']:<58} "
                      f"{err or root['state']['current']}")
        print(f"[traces] round {rnd} done in {time.time() - t0:.0f}s")

    doc = {"rounds": rounds, "image": image_tag(), "flows": flows}
    check_traces(doc, out)
    with open(os.path.join(HERE, "traces.json"), "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=1, sort_keys=False)
        f.write("\n")
    print(f"[traces] traces.json written: {len(flows)} flows x {rounds} rounds")


def verify_traces():
    """Re-ask Kestra for everything traces.json captured, long after the fact.

    The capture stops polling for sub-executions the moment none are pending,
    and a sub-execution whose row lands a beat later is then simply absent --
    silently, with a plausible trace written.  That is not hypothetical: Task 1's
    corpus.json records ONE sub-execution for `valids/loop-with-subflow` and this
    capture recorded TWO in every round, because the flow's `Loop` has two
    values.  An undercount by one is exactly the defect that reads like a
    measurement.

    So every captured execution is fetched again -- minutes to hours later, with
    the flows long finished -- reduced the same way, and required to be
    IDENTICAL.  A descendant that was going to appear has appeared by now.
    """
    with open(os.path.join(HERE, "traces.json"), encoding="utf-8") as f:
        doc = json.load(f)
    pg = pg_container()
    checked = grown = 0
    problems = []
    for cid in sorted(doc["flows"]):
        for r in doc["flows"][cid]:
            root_id = r["root"]["id"]
            now = descendants(root_id, pg)
            was = {d["id"] for d in r["descendants"]}
            if now != was:
                grown += 1
                problems.append(f"{cid} r{r['round']}: descendants were {sorted(was)}, "
                                f"are now {sorted(now)}")
            for captured in [r["root"], *r["descendants"]]:
                checked += 1
                status, body = http("GET", f"{BASE}/{TENANT}/executions/{captured['id']}")
                if status != 200:
                    problems.append(f"{cid} r{r['round']}: execution {captured['id']} "
                                    f"re-read -> {status}")
                    continue
                again = reduce_execution(json.loads(body))
                if again != captured:
                    problems.append(f"{cid} r{r['round']}: execution {captured['id']} "
                                    "re-reads differently than it was captured")
    if problems:
        bail(f"{len(problems)} captured execution(s) do not re-read the way they were "
             f"captured ({grown} grew a sub-execution):\n  " + "\n  ".join(problems[:20]))
    print(f"[verify] {checked} captured execution(s) re-read identically, and every "
          f"descendant set is unchanged")


def check_traces(doc, out):
    """traces.json must cover exactly the `runs` flows, `rounds` times, with no
    round that failed to run.

    The failure this prevents is the quiet one: a round that errored on 30 flows
    still produces a traces.json that normalise.py will happily read, and the
    golden it writes would then be built from whichever rounds happened to
    survive -- a denominator nobody stated.
    """
    want = {r["id"] for r in out if r.get("disposition") == "runs"}
    got = set(doc.get("flows", {}))
    if want != got:
        bail(f"traces.json covers {len(got)} flows but corpus.json has {len(want)} "
             f"with disposition `runs`; missing e.g. {sorted(want - got)[:5]}, "
             f"extra e.g. {sorted(got - want)[:5]}")
    rounds = doc.get("rounds")
    bad_len = [cid for cid, rs in doc["flows"].items() if len(rs) != rounds]
    if bad_len:
        bail(f"{len(bad_len)} flow(s) do not have all {rounds} rounds, "
             f"e.g. {bad_len[:5]}")
    errored = [f"{cid} r{r['round']}: {r['error']}"
               for cid, rs in sorted(doc["flows"].items()) for r in rs if r["error"]]
    if errored:
        bail(f"{len(errored)} capture(s) failed; a golden built from the rounds that "
             f"survived has a denominator nobody stated: {errored[:5]}")


# --------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-down", action="store_true",
                    help="skip the `down -v`; the pre-flight empty-Kestra assertion still runs")
    ap.add_argument("--no-up", action="store_true", help="assume the stack is already running")
    ap.add_argument("--down", action="store_true", help="docker compose down -v and exit")
    ap.add_argument("--rewrite-runnable", action="store_true",
                    help="regenerate runnable.txt from committed corpus.json + probe.json; no Kestra needed")
    ap.add_argument("--traces", type=int, metavar="N", nargs="?", const=TRACE_ROUNDS,
                    help=f"execute every `runs` flow N times (default {TRACE_ROUNDS}) and "
                         "write traces.json, the input normalise.py turns into golden.json")
    ap.add_argument("--verify-traces", action="store_true",
                    help="re-read every execution traces.json captured and require it to be "
                         "identical; needs the same Kestra still running")
    ap.add_argument("--refresh-test-source-classes", action="store_true",
                    help="re-resolve test-source-classes.json from the sibling Kestra checkout")
    ap.add_argument("--self-test", action="store_true",
                    help="mutate the committed data and prove every guard fires; no Kestra needed")
    args = ap.parse_args()

    if args.rewrite_runnable:
        out, probe = load_committed()
        write_runnable(out, probe)
        print("rewrote runnable.txt from corpus.json + probe.json")
        return

    if args.refresh_test_source_classes:
        refresh_test_source_classes()
        return

    if args.self_test:
        self_test()
        return

    if args.down:
        compose("down", "-v")
        return

    if args.verify_traces:
        wait_ready()
        verify_traces()
        return

    if args.traces:
        # The same structural clean start the full harvest has, for the same
        # reason: a partial leftover produces a plausible artifact with a quietly
        # wrong denominator.
        if not args.no_up:
            if not args.no_down:
                compose("down", "-v")
            compose("up", "-d")
        wait_ready()
        capture_traces(args.traces)
        return

    if not args.no_up:
        # Structural, not documented: the default invocation always starts clean.
        if not args.no_down:
            compose("down", "-v")
        compose("up", "-d")
    wait_ready()
    pg = pg_container()

    entries = read_corpus()
    validate_tables(entries)
    assert_clean(entries)
    print(f"[corpus] {len(entries)} flows "
          f"({sum(1 for e in entries if e['kind'] == 'valids')} valids, "
          f"{sum(1 for e in entries if e['kind'] == 'invalids')} invalids)")

    # PUBLISH EVERY FLOW FIRST, then execute.  Publishing and executing
    # flow-by-flow in sorted order makes the result depend on the alphabet: ten
    # corpus flows name another corpus flow that sorts LATER, so their Subflow
    # target did not exist yet when they ran.  Measured on
    # `valids/loop-with-subflow`, which calls `io.kestra.tests/minimal`: harvested
    # that way it FAILED with one loop iteration, and harvested after everything
    # was published it SUCCEEDS with two.  Task 2's capture published first and
    # got the second answer, which is the whole of the 141-vs-142 gap.
    publish = {}
    for i, e in enumerate(entries, 1):
        status, body = http(
            "POST", f"{BASE}/{TENANT}/flows",
            e["source"].encode("utf-8"), "application/x-yaml",
        )
        publish[e["id"]] = (status, detail(body))
    ok = sum(1 for st, _ in publish.values() if st == 200)
    print(f"[publish] {ok}/{len(entries)} flows published; "
          f"{len(entries) - ok} refused")

    out = []
    for i, e in enumerate(entries, 1):
        rec = {
            "id": e["id"],
            "path": e["path"],
            "flow_id": e["flow_id"],
            "namespace": e["namespace"],
            "declares_inputs": e["declares_inputs"],
        }

        status, body = publish[e["id"]]
        rec["publish_status"] = status

        if 400 <= status < 500:
            rec["disposition"] = "rejected"
            rec["reason"] = f"publish -> {status}: {body}"
            rec["rejected_class"] = rejected_class(e, body)
        elif status != 200:
            rec["disposition"] = "unclassified"
            rec["reason"] = f"publish -> {status}: {body}"
        elif e["id"] in NOT_STANDALONE_INPUTS:
            rec["disposition"] = "not-standalone"
            rec["reason"] = NOT_STANDALONE_INPUTS[e["id"]]
            rec["inputs"] = {}
            rec["inputs_from"] = "not recoverable -- see reason"
        else:
            spec = INPUTS.get(e["id"])
            if spec:
                rec["inputs"] = spec["inputs"]
                rec["inputs_from"] = spec["from"]
                if spec.get("alt"):
                    rec["inputs_alternatives"] = [
                        {"inputs": a["inputs"], "inputs_from": a["from"]} for a in spec["alt"]
                    ]
            elif e["declares_inputs"]:
                rec["disposition"] = "unclassified"
                rec["reason"] = "declares inputs: but no entry in INPUTS or NOT_STANDALONE_INPUTS"
                out.append({k: v for k, v in rec.items()})
                print(f"[{i:3}/{len(entries)}] {rec['id']:<62} {rec['disposition']}")
                continue

            disp, why, root, subs, obs = run_flow(e, pg)
            rec["disposition"] = disp
            rec["reason"] = why
            if obs:
                rec["observed"] = obs
            if disp == "rejected":
                rec["rejected_class"] = rejected_class(e, why)
            if e["kind"] == "invalids" and disp != "rejected":
                # Kestra's own repo files this under invalids/, and this Kestra ACCEPTS it.
                # Task 8 must not treat it as a flow egret has to refuse.
                rec["accepted_though_invalid"] = True
            if root:
                rec["execution_id"] = root
            rec["sub_executions"] = len(subs)
            if subs:
                rec["sub_execution_ids"] = [x["id"] for x in subs]

        out.append(rec)
        print(f"[{i:3}/{len(entries)}] {rec['id']:<62} {rec['disposition']:<15} {str(rec.get('reason'))[:90]}")

    # corpus.json first: the probe re-executes flows and can fail, and losing a
    # 35-minute harvest to a probe bug is a self-inflicted wound.
    with open(os.path.join(HERE, "corpus.json"), "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, sort_keys=False)
        f.write("\n")
    print("[done] corpus.json written; probing discrimination")

    probe = probe_discrimination(entries, out, pg)
    # Persist it.  `runnable.txt` is the file this package calls its honest
    # denominator, and until now regenerating it required a LIVE Kestra -- so a
    # defect in the SUMMARY could not be fixed without a multi-hour re-harvest,
    # which is pressure to hand-edit the very file whose worth is that it is a
    # function of the data.  `--rewrite-runnable` now reproduces it offline.
    with open(os.path.join(HERE, "probe.json"), "w", encoding="utf-8") as f:
        json.dump(probe, f, indent=2, sort_keys=True)
        f.write("\n")
    write_runnable(out, probe)
    print("\n[done] corpus.json + runnable.txt written")


def record_invariant_problems(out):
    """Invariants every record must satisfy, checked against the RECORD's own
    other fields rather than against the summary built from it.

    Five corruptions of `corpus.json` used to move a published number at exit 0:
    flipping a `not-standalone` to `runs` (the headline moves), flipping a
    `rejected` to `runs` and dropping its class (two numbers move, and it slips
    past the class re-derivation because that only inspects records still SAYING
    rejected), tripling `sub_executions`, and corrupting an `observed` state.
    Each of these now contradicts a field the corruption did not think to change.
    """
    bad = []
    for r in out:
        cid = r.get("id", "<no id>")
        disp = r.get("disposition")
        pub = r.get("publish_status")
        obs = r.get("observed")
        if disp not in KNOWN_DISPOSITIONS:
            bad.append(f"{cid}: disposition {disp!r} is not in KNOWN_DISPOSITIONS")
            continue
        if not r.get("reason"):
            bad.append(f"{cid}: no reason")
        # A flow that RAN was published, reached a terminal state, and left an
        # observation.  A flipped disposition contradicts all three at once.
        if disp == "runs":
            if pub != 200:
                bad.append(f"{cid}: disposition runs but publish_status {pub!r}")
            if not obs:
                bad.append(f"{cid}: disposition runs but no `observed`")
            elif obs.get("state") not in TERMINAL:
                bad.append(f"{cid}: runs but observed.state {obs.get('state')!r} "
                           "is not terminal")
            elif r.get("reason") != obs.get("state"):
                bad.append(f"{cid}: reason {r.get('reason')!r} != observed.state "
                           f"{obs.get('state')!r}")
            elif obs.get("taskruns") != len(obs.get("task_ids") or []):
                bad.append(f"{cid}: observed.taskruns {obs.get('taskruns')} != "
                           f"len(task_ids) {len(obs.get('task_ids') or [])}")
        else:
            if obs is not None:
                bad.append(f"{cid}: disposition {disp} but carries `observed`")
        if disp == "rejected":
            seam = "POST execution -> 4" in str(r.get("reason"))
            if not ((isinstance(pub, int) and 400 <= pub < 500) or seam):
                bad.append(f"{cid}: disposition rejected but publish_status {pub!r} "
                           "and no 4xx execution-seam evidence in the reason")
            if r.get("rejected_class") not in KNOWN_REJECTED_CLASSES:
                bad.append(f"{cid}: rejected but rejected_class "
                           f"{r.get('rejected_class')!r} is not a known class")
        elif "rejected_class" in r:
            bad.append(f"{cid}: disposition {disp} but carries a stale "
                       f"rejected_class {r['rejected_class']!r}")
        if disp == "not-standalone" and pub != 200:
            bad.append(f"{cid}: not-standalone but publish_status {pub!r}; it never "
                       "published, so it cannot be a standalone-execution finding")
        n = r.get("sub_executions")
        if n is not None:
            ids = r.get("sub_execution_ids")
            if not isinstance(n, int) or isinstance(n, bool) or n < 0:
                bad.append(f"{cid}: sub_executions {n!r} is not a count")
            elif n and disp != "runs":
                bad.append(f"{cid}: {n} sub_executions on a {disp} record")
            elif n and ids is None:
                # The count must be checkable against something.  Tripling a bare
                # integer moved a published number at exit 0; the ids are what
                # make that contradict itself.
                bad.append(f"{cid}: {n} sub_executions but no sub_execution_ids to "
                           "check the count against")
            elif ids is not None and (n != len(ids) or len(set(ids)) != len(ids)):
                bad.append(f"{cid}: sub_executions {n} != {len(ids)} distinct "
                           "sub_execution_ids")
    return bad


def probe_problems(out, probe):
    """`differs` decides the single most load-bearing sentence in this package --
    "proven discriminating N" -- and it used to be a raw copied boolean.

    Flipping every `differs` true -> false took `proven discriminating 17` to 0
    at exit 0.  So each probed row records the BASIS of its verdict and, where the
    basis is an observation, the observation itself; `differs` is then recomputed
    here from that observation and the EXPECTED row.
    """
    bad = []
    by_id = {r["id"]: r for r in out}
    ran = {cid for cid, r in by_id.items()
           if cid in EXPECTED and r.get("disposition") == "runs"}
    for cid in sorted(ran - set(probe)):
        bad.append(f"probe.json has no record for {cid}, which ran and has an "
                   "EXPECTED row; it would print as `unprobed`")
    for cid in sorted(set(probe) - ran):
        bad.append(f"probe.json has a record for {cid}, which is not a flow that "
                   "ran with an EXPECTED row")
    for cid in sorted(set(probe) & ran):
        pr = probe[cid]
        if not pr.get("probed"):
            if pr.get("differs"):
                bad.append(f"probe[{cid}] is unprobed but claims differs")
            continue
        basis = pr.get("basis")
        if basis == "observed":
            obs = pr.get("alt_observed")
            if not obs:
                bad.append(f"probe[{cid}] basis=observed but carries no alt_observed")
                continue
            recomputed = bool(check_row(EXPECTED[cid], obs))
            if recomputed != bool(pr.get("differs")):
                bad.append(f"probe[{cid}].differs is {pr.get('differs')} but "
                           f"re-deriving it from alt_observed gives {recomputed}")
        elif basis in ("refused", "no-terminal"):
            if not pr.get("differs"):
                bad.append(f"probe[{cid}] basis={basis} must be discriminating")
        else:
            bad.append(f"probe[{cid}] has no recognised basis ({basis!r})")
    return bad


def traces_agreement_problems(out):
    """corpus.json's terminal states, checked against Task 2's INDEPENDENT capture.

    Two harvests of the same artifact are two measurements, and where they
    disagree at least one is wrong.  They did, on four flows -- and the cause was
    not flakiness: the Task 1 harvest published and executed flow-by-flow in
    sorted order, so ten corpus flows whose `Subflow` target sorts LATER ran
    before that target existed.  `valids/loop-with-subflow`,
    `valids/subflow-child`, `valids/subflow-parent-retry` and
    `valids/flow-concurrency-parallel-subflow-kill-child` were all recorded
    FAILED where five rounds of Task 2's capture say SUCCESS.

    traces.json is optional here: it belongs to Task 2 and a Task 1-only
    checkout may not have it yet.  When it is present the check is not optional.
    """
    path = os.path.join(HERE, "traces.json")
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        flows = json.load(f).get("flows") or {}
    by_id = {r["id"]: r for r in out}
    bad = []
    for fid, rounds in flows.items():
        rec = by_id.get(fid)
        if not rec or rec.get("disposition") != "runs" or not rec.get("observed"):
            continue
        states = {r["root"]["state"]["current"] for r in rounds if r.get("root")}
        if len(states) != 1:
            continue
        want = states.pop()
        if rec["observed"]["state"] != want:
            bad.append(f"{fid}: corpus.json says {rec['observed']['state']} but all "
                       f"{len(rounds)} rounds of traces.json say {want}")
    return bad


def check_outputs(out, probe):
    """Every guard over the harvested data, in one place that WRITES NOTHING.

    `--self-test` used to drive its first cases through `write_runnable`, which
    both validates and writes the committed artifact: with a guard neutered so
    the mutation did not fire, the run that reports a missing guard left
    `runnable.txt` corrupted -- `TOTAL 344`, one flow absent -- and then exited 1,
    ready for the next `git add -A`.
    """
    problems = record_invariant_problems(out)
    problems += probe_problems(out, probe)
    problems += traces_agreement_problems(out)

    # The class table, re-derived rather than recounted.  `sum(Counter(rej)) ==
    # len(rej)` was the previous check and could not fail: both sides came from
    # iterating the same list once.  This recomputes each class from the record's
    # own directory and message, which a corrupted `rejected_class` contradicts.
    for r in out:
        if r.get("disposition") != "rejected":
            continue
        kind = r["id"].split("/", 1)[0]
        want = rejected_class({"kind": kind, "id": r["id"]}, r.get("reason", ""))
        if want != r.get("rejected_class"):
            problems.append(
                f"{r['id']}: stored rejected_class {r.get('rejected_class')!r} but the "
                f"classifier derives {want!r} from its directory and message")
    if problems:
        bail(f"{len(problems)} problem(s) in the harvested data:\n  "
             + "\n  ".join(problems[:20])
             + (f"\n  ... and {len(problems) - 20} more" if len(problems) > 20 else ""))


def disposition_table(records, label, total_label="TOTAL"):
    """Rows for every disposition PRESENT, never a hand-written list.

    A hand-written list next to `buckets.setdefault()` printed rows summing to
    338 of 345 and exited 0 -- seven flows invisible in the file this package
    calls its honest denominator.  Fixing one instance of that is not evidence
    the class is gone; this is the only place either table is built.
    """
    seen = Counter(r["disposition"] for r in records)
    unknown = sorted(set(seen) - set(KNOWN_DISPOSITIONS))
    if unknown:
        bail(f"{label}: disposition(s) {unknown} are outside KNOWN_DISPOSITIONS "
             f"{list(KNOWN_DISPOSITIONS)}; add them there or fix the record")
    names = [d for d in KNOWN_DISPOSITIONS if d in seen]
    rows = [f"  {d:<24} {seen[d]}" for d in names]
    printed = sum(seen[d] for d in names)
    if printed != len(records):
        bail(f"{label}: rows sum to {printed} but there are {len(records)} records")
    rows.append(f"  {total_label:<24} {printed}")
    return rows


def handwritten_tail_of(body):
    """The lines after [`HANDWRITTEN_SENTINEL`], or a guard failure.

    A missing sentinel is refused rather than treated as "no notes": that is
    exactly the state the file was in before W3b Task 7, and treating it as
    empty is what made `--rewrite-runnable` delete 319 lines without a word.
    """
    if HANDWRITTEN_SENTINEL not in body:
        bail("runnable.txt carries no hand-written sentinel, so a rewrite cannot "
             "tell the generated half from the notes appended after it and would "
             f"DELETE them. Put this line back before rewriting:\n  "
             f"{HANDWRITTEN_SENTINEL}")
    return body[body.index(HANDWRITTEN_SENTINEL) + 1:]


def handwritten_tail():
    path = os.path.join(HERE, "runnable.txt")
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return handwritten_tail_of(f.read().splitlines())


def write_runnable(out, probe, task2=None):
    check_outputs(out, probe)
    buckets = {}
    for r in out:
        buckets.setdefault(r["disposition"], []).append(r)

    lines = []
    lines.append("W3a execution oracle -- how much of Kestra's own corpus runs standalone")
    lines.append("=" * 72)
    lines.append("")
    lines.append(f"Measured by oracle-exec/harvest.py against {IMAGE_TAG} -- a MUTABLE tag")
    lines.append("over a NIGHTLY build (`version=nightly`, `revision=169ca7f2...`), pulled with")
    lines.append("`pull_policy: always` when this golden was harvested.  So the provenance of")
    lines.append("every number below is `the tag as it stood`, NOT a version and NOT a digest.")
    lines.append(f"docker-compose.yml pins {IMAGE_DIGEST_PINNED_NOW}")
    lines.append("since e72c83cc, which is AFTER the harvest: that digest is what a re-harvest")
    lines.append("would use, not what this one used.")
    lines.append("Corpus: kestra @ 96893b589a02e5d5d85c722855f76c08e79085af,")
    lines.append("core/src/test/resources/flows/{valids,invalids}, BOTH .yaml and .yml.")
    lines.append("")
    lines.append(f"total flows                {len(out)}")
    lines.extend(disposition_table(out, "overall"))
    lines.append("")
    v = [r for r in out if r["id"].startswith("valids/")]
    iv = [r for r in out if r["id"].startswith("invalids/")]
    if len(v) + len(iv) != len(out):
        bail(f"{len(out) - len(v) - len(iv)} record(s) are in neither valids/ nor invalids/")
    for label, group in (("valids", v), ("invalids", iv)):
        lines.append(f"{label} ({len(group)})")
        lines.extend(disposition_table(group, label))
        lines.append("")

    rej = buckets.get("rejected", [])
    # DERIVED from the data, never a hand-written class list, AND checked against
    # the vocabulary rejected_class() is the only producer of.  A record whose
    # class was dropped, blanked or renamed fails here instead of quietly reading
    # as one of the classes Task 8 ignores.
    lines.append("rejected, by class")
    by_class = Counter(r["rejected_class"] for r in rej)
    for cls, n in sorted(by_class.items(), key=lambda kv: (-kv[1], kv[0])):
        mark = "  <- Task 8 evidence" if cls in TASK8_EVIDENCE else ""
        lines.append(f"  {cls:<44} {n}{mark}")
    lines.append(f"  {'TOTAL':<44} {sum(by_class.values())}")
    lines.append("")
    # Generated from TASK8_EVIDENCE, not written beside it.  The hand-written
    # version said "only invalid-flow", which told Task 8 to ignore
    # `refused-by-both` -- a class holding a flow Kestra refuses in both places.
    lines.append("  Evidence that egret must refuse a flow -- and nothing else is:")
    for cls in TASK8_EVIDENCE:
        lines.append(f"    {cls:<42} {by_class.get(cls, 0)}")
    lines.append("  The remaining classes are NOT refusals:")
    for cls in KNOWN_REJECTED_CLASSES:
        if cls in TASK8_EVIDENCE:
            continue
        lines.append(f"    {cls:<42} {by_class.get(cls, 0)}  -- "
                     f"{NON_EVIDENCE_REASON[cls]}")
    lines.append("")

    accepted = [r for r in out if r.get("accepted_though_invalid")]
    lines.append(f"invalids this Kestra ACCEPTS: {len(accepted)}")
    for r in accepted:
        lines.append(f"  {r['id']}  ->  {r['disposition']} ({r.get('reason')})")
    if accepted:
        lines.append("  These are NOT evidence for Task 8.  Kestra's repo files a flow under")
        lines.append("  invalids/ when the EXECUTION is expected to fail as well as when the flow")
        lines.append("  is rejected; the pinned runtime distinguishes them and this is the split.")
    lines.append("")

    subs = sum(r.get("sub_executions", 0) for r in out)
    lines.append(f"sub-executions harvested by following parentId: {subs}")
    lines.append("")

    # Task 2's numbers.  Optional only so that a re-harvest can write
    # runnable.txt before normalise.py has seen the new traces; the default
    # path reads the committed files and fails if they are absent.
    golden, stab = task2 if task2 is not None else load_task2()
    lines.extend(order_table(golden, stab))

    # Same derivation as the header: every bucket present gets a detail section,
    # `runs` last because it is the long one.  The literal list this replaced gave
    # a novel disposition no header row AND no detail section, from the same
    # hand-written tuple in two places.
    detail_order = ([d for d in KNOWN_DISPOSITIONS if d in buckets and d != "runs"]
                    + (["runs"] if "runs" in buckets else []))
    if sum(len(buckets[d]) for d in detail_order) != len(out):
        bail(f"detail sections cover {sum(len(buckets[d]) for d in detail_order)} "
             f"of {len(out)} records")
    for k in detail_order:
        rows = buckets[k]
        lines.append("")
        lines.append(f"{k} -- {len(rows)}")
        lines.append("-" * 72)
        for r in sorted(rows, key=lambda x: x["id"]):
            if k == "runs":
                extra = f"  (+{r['sub_executions']} sub)" if r.get("sub_executions") else ""
                lines.append(f"{r['id']:<62} {r.get('reason', '')}{extra}")
            else:
                cls = f"  [{r['rejected_class']}]" if r.get("rejected_class") else ""
                lines.append(f"{r['id']}{cls}")
                lines.append(f"    {r.get('reason', '')}")
        lines.append("")

    lines.append("")
    lines.append("")
    lines.append("input recovery cross-check")
    lines.append("-" * 72)
    lines.append("Each recovered input is checked against what the SAME Java test asserts:")
    lines.append("the terminal state, the taskRunList size, a task id at a position, and")
    lines.append("per-task states.  A wrong input does not error -- it runs the other")
    lines.append("branch -- so a state-only check is not enough: several of these flows end")
    lines.append("SUCCESS either way.")
    lines.append("")
    by_id = {r["id"]: r for r in out}
    agree = differ = skipped = 0
    rows = []
    for cid, row in sorted(EXPECTED.items()):
        r = by_id.get(cid)
        asserts = ",".join(k for k in ("state", "taskruns", "task_at", "task_states")
                           if k in row)
        if r is None:
            rows.append((cid, "MISSING", "no corpus entry", asserts))
            differ += 1
            continue
        if r["disposition"] != "runs" or not r.get("observed"):
            rows.append((cid, "n/a", f"not exercised: {r['disposition']}", asserts))
            skipped += 1
            continue
        bad = check_row(row, r["observed"])
        if bad:
            rows.append((cid, "DIFFERS", "; ".join(bad), asserts))
            differ += 1
        else:
            rows.append((cid, "agrees", row["from"], asserts))
            agree += 1
    # A row is WEAK when its only assertion is the terminal state, that state is a
    # non-error one, and the probe never proved the row can move.  Such a row would
    # read `agrees` for any input that merely finishes successfully.  Derived, so a
    # new row of that shape is labelled without anyone remembering to.
    weak = []
    for cid, verdict, why, asserts in rows:
        pr = probe.get(cid) or {}
        is_weak = (verdict == "agrees" and asserts == "state"
                   and EXPECTED[cid].get("state") in ("SUCCESS", "WARNING")
                   and not pr.get("differs"))
        if is_weak:
            weak.append(cid)
        lines.append(f"  {verdict:<8} {cid:<52} [{asserts}]"
                     + ("   WEAK" if is_weak else ""))
        lines.append(f"           {why}")
    lines.append("")
    lines.append(f"  agrees {agree}   differs {differ}   not exercised {skipped}"
                 f"   (of {len(EXPECTED)} rows)")
    if weak:
        lines.append("")
        lines.append(f"  WEAK rows ({len(weak)}): state-only, non-error state, and not proven")
        lines.append("  discriminating.  Counted in `agrees` above, but they would agree with")
        lines.append("  any input that merely finishes successfully -- do not cite them.")
        for cid in weak:
            lines.append(f"    {cid}")
    lines.append("")

    lines.append("")
    lines.append("discrimination probe -- which rows CAN report a difference")
    lines.append("-" * 72)
    lines.append("A check that passes with or without the thing it verifies is not a check.")
    lines.append("For every row whose flow has a documented ALTERNATIVE input, the flow was")
    lines.append("executed again with that alternative and the row re-evaluated: the row")
    lines.append("discriminates only if the alternative FAILS it.  This is measured, not")
    lines.append("argued.")
    lines.append("")
    disc = nondisc = unprobed = 0
    for cid, row in sorted(EXPECTED.items()):
        pr = probe.get(cid)
        r = by_id.get(cid)
        if r is None or r.get("disposition") != "runs":
            continue
        if pr is None or not pr.get("probed"):
            unprobed += 1
            lines.append(f"  unprobed      {cid}")
            lines.append(f"                {(pr or {}).get('why', 'not probed')}")
            continue
        if pr["differs"]:
            disc += 1
            lines.append(f"  discriminates {cid}")
            lines.append(f"                with {json.dumps(pr['alt_inputs'])}: {pr['why']}")
        else:
            nondisc += 1
            lines.append(f"  CANNOT        {cid}")
            lines.append(f"                {pr['why']}")
    lines.append("")
    lines.append(f"  proven discriminating {disc}   proven non-discriminating {nondisc}"
                 f"   unprobed {unprobed}")
    lines.append("")
    lines.append("  `unprobed` means no ALTERNATIVE input is documented for that flow, so")
    lines.append("  there is nothing to swap in.  Such a row is not claimed to discriminate.")
    lines.append("")

    lines.append("")
    lines.append("recovered inputs with no checkable assertion")
    lines.append("-" * 72)
    for cid, why in sorted(NO_ASSERTION.items()):
        lines.append(f"  {cid}")
        lines.append(f"    {why}")
    recovered = sorted(INPUTS)
    lines.append("")
    lines.append(f"  recovered input values          {len(recovered)}")
    lines.append(f"    with an EXPECTED row          {len(set(recovered) & set(EXPECTED))}")
    lines.append(f"    with no checkable assertion   {len(set(recovered) & set(NO_ASSERTION))}")
    lines.append("")

    # The hand-written half, carried across the rewrite rather than deleted.
    tail = handwritten_tail()
    lines.append(HANDWRITTEN_SENTINEL)
    lines.extend(tail)

    path = os.path.join(HERE, "runnable.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
        f.write("\n")

    # Read it back and count what survived.  A guard that only inspects the
    # `lines` list it just built cannot fail for the reason this one exists --
    # the failure mode is a rewrite that drops the tail on the way to disk.
    with open(path, encoding="utf-8") as f:
        after = handwritten_tail_of(f.read().splitlines())
    if len(after) < len(tail):
        bail(f"the rewrite lost the hand-written half of runnable.txt: {len(tail)} "
             f"line(s) before, {len(after)} after. Those notes are not derivable "
             f"from corpus.json and probe.json and nothing else holds them.")


if __name__ == "__main__":
    main()
