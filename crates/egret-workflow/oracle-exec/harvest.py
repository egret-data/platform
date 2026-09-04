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
import os
import re
import subprocess
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
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
    "allow-failure": {
        "inputs": {"crash": "1"},
        "from": J + "plugin/core/flow/AllowFailureTest.java:41 failed()",
        "alt": [{"inputs": {}, "from": J + "plugin/core/flow/AllowFailureTest.java:29 success() (@ExecuteFlow, no inputs)"}],
    },
    "condition_with_input": {
        "inputs": {"condition": "fail"},
        "from": W + ":742 replayExecutionWithNewInputs()",
        "alt": [{"inputs": {"condition": "success"}, "from": W + ":762 replayExecutionWithNewInputs() replay body"}],
    },
    "exit": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:16 shouldExitTheExecution() (@ExecuteFlow, no inputs)"},
    "exit-killed": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:24 shouldExitAndKillTheExecution() (@ExecuteFlow, no inputs)"},
    "exit-nested": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:33 shouldExitAndFailNestedIf() (@ExecuteFlow, no inputs)"},
    "exit-cancelled": {"inputs": {}, "from": J + "plugin/core/execution/ExitTest.java:45 shouldExitWithCancelled() (@ExecuteFlow, no inputs)"},
    "fail-on-switch": {"inputs": {"param": "fail"}, "from": J + "plugin/core/execution/FailTest.java:31 failOnSwitch()"},
    "fail-on-condition": {
        "inputs": {"param": "fail"},
        "from": J + "plugin/core/execution/FailTest.java:44 failOnCondition()",
        "alt": [{"inputs": {"param": "success"}, "from": J + "plugin/core/execution/FailTest.java:57 dontFailOnCondition()"}],
    },
    "finally-sequential": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:59 sequentialWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:42 sequentialWithoutErrors()"}],
    },
    "finally-sequential-error": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:105 sequentialErrorBlockWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:76 sequentialErrorBlockWithoutErrors()"}],
    },
    "finally-allowfailure": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:141 allowFailureWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:124 allowFailureWithoutErrors()"}],
    },
    "finally-parallel": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/ParallelTest.java:48 errors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:160 parallelWithoutErrors()"}],
    },
    "finally-loop": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:216 loopWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:196 loopWithoutErrors()"}],
    },
    "finally-loop-parallel": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:266 loopParallelWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:246 loopParallelWithoutErrors()"}],
    },
    "finally-dag": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/DagTest.java:86 errors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:297 dagWithoutErrors()"}],
    },
    "finally-flow": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:350 flowWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:333 flowWithoutErrors()"}],
    },
    "finally-flow-error": {
        "inputs": {"failed": True},
        "from": J + "plugin/core/flow/FinallyTest.java:384 flowErrorBlockWithErrors()",
        "alt": [{"inputs": {"failed": False}, "from": J + "plugin/core/flow/FinallyTest.java:367 flowErrorBlockWithoutErrors()"}],
    },
    "if": {
        "inputs": {"if1": True, "if2": False, "if3": True},
        "from": J + "core/runners/AbstractRunnerTest.java:670 multipleIf()",
    },
    "if-condition": {
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
    "if-without-else": {
        "inputs": {"param": True},
        "from": J + "plugin/core/flow/IfTest.java:118 ifWithoutElse()",
        "alt": [{"inputs": {"param": False}, "from": J + "plugin/core/flow/IfTest.java:127 ifWithoutElse()"}],
    },
    "if-in-flowable": {
        "inputs": {"param": True},
        "from": J + "plugin/core/flow/IfTest.java:139 ifInFlowable()",
    },
    "input-log-secret": {
        "inputs": {"nested.key": "pass"},
        "from": J + "core/runners/InputsTest.java:425 shouldMaskSecretInputInLogs()",
    },
    "json-input-schema-validation": {
        "inputs": {"jsonWithSchema": {"name": "kestra"}, "jsonWithoutSchema": {"any": "shape"}},
        "from": J + "core/models/flows/input/JsonInputSchemaFlowIntegrationTest.java:35 shouldRunFlowWhenJsonInputMatchesSchema()",
    },
    "parallel-fail-with-flowable": {
        "inputs": {},
        "from": J + "plugin/core/flow/ParallelTest.java:67 parallelFailWithFlowable() (@ExecuteFlow, no inputs)",
    },
    "pause-behavior": {
        "inputs": {"behavior": "FAIL"},
        "from": J + "plugin/core/flow/PauseTest.java:117 runDurationWithBehaviorFail() -> Pause.Behavior.FAIL",
        "alt": [
            {"inputs": {"behavior": "RESUME"}, "from": J + "plugin/core/flow/PauseTest.java:111 -> Pause.Behavior.RESUME"},
            {"inputs": {"behavior": "WARN"}, "from": J + "plugin/core/flow/PauseTest.java:123 -> Pause.Behavior.WARN"},
            {"inputs": {"behavior": "CANCEL"}, "from": J + "plugin/core/flow/PauseTest.java:129 -> Pause.Behavior.CANCEL"},
        ],
    },
    "pause-duration-from-input": {
        "inputs": {},
        "from": J + "plugin/core/flow/PauseTest.java:206 runDurationFromInput() (passes null inputs; the DURATION default PT1S applies)",
    },
    "replay-loop": {
        "inputs": {"failed": "FIRST"},
        "from": J + "core/runners/ExecutionServiceTest.java:139 restartLoop()",
    },
    "sla-execution-condition": {
        "inputs": {"string": "CANCEL"},
        "from": J + "core/runners/SLATestCase.java:62 executionConditionSLAShouldCancel()",
        "alt": [
            {"inputs": {"string": "LABEL"}, "from": J + "core/runners/SLATestCase.java:86 executionConditionSLAShouldLabel()"},
            {"inputs": {}, "from": J + "core/runners/SLATestCase.java:56 executionConditionSLAShouldPass() (no inputs)"},
        ],
    },
    "subflow-function-child": {
        "inputs": {"region": "us"},
        "from": J + "core/runners/pebble/functions/SubflowFunctionTest.java:31 shouldRunSubflowAndExposeFlowOutputs()",
        "alt": [{"inputs": {}, "from": J + "core/runners/pebble/functions/SubflowFunctionTest.java:96 shouldUseSubflowInputDefaultsWhenNotProvided()"}],
    },
    "subflow-nullable-input-parent": {
        "inputs": {"integerValue": ""},
        "from": J + "plugin/core/flow/SubflowRunnerTest.java:155 shouldPassNullableInputFromParentToSubflow()",
    },
    "switch": {
        "inputs": {"string": "FIRST"},
        "from": J + "plugin/core/flow/SwitchTest.java:39 switchFirst()",
        "alt": [
            {"inputs": {"string": "SECOND"}, "from": J + "plugin/core/flow/SwitchTest.java:57 switchSecond()"},
            {"inputs": {"string": "THIRD"}, "from": J + "plugin/core/flow/SwitchTest.java:74 switchThird()"},
            {"inputs": {"string": "DEFAULT"}, "from": J + "plugin/core/flow/SwitchTest.java:92 switchDefault()"},
        ],
    },
    "switch-impossible": {
        "inputs": {"string": "impossible"},
        "from": J + "plugin/core/flow/SwitchTest.java:108 switchImpossible()",
    },
    "task-flow": {
        "inputs": {"string": "THIRD"},
        "from": J + "plugin/core/flow/FlowCaseTest.java:44 run() via waitFailed()",
        "alt": [{"inputs": {"string": "OK"}, "from": J + "plugin/core/flow/FlowCaseTest.java:44 run() via waitSuccess()"}],
    },
    "task-flow-inherited-labels": {
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
    "working-directory": {
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
    "task-flow-dynamic":
        "FlowGraphTest.java:295 hands runOne a RAW inputs map "
        "({namespace: f.getNamespace(), flowId: 'switch'}) instead of going through "
        "flowIO.readExecutionInputs, so it bypasses typed-input validation entirely and "
        "never supplies the flow's one DECLARED input, the required STRING `string`.  "
        "Over the API that is `422 Missing required input:string`, and there is no value "
        "for `string` anywhere in the checkout to recover.",
    "inputs":
        "FILE input `file` is a classpath resource path resolved by the JVM "
        "(InputsTest.inputs -> application-test.yml) and json2/yaml2 are Java Map "
        "objects; the values are computed, not literal.",
    "inputs-large":
        "RunContextTest.java:141 builds the `string` input from a 16 KiB generated "
        "char array and reuses InputsTest.inputs' classpath FILE path; computed, not literal.",
    "inputs-small-files":
        "ExecutionControllerRunnerTest.java:394 uploads the classpath resource "
        "application-test.yml as a multipart FILE part; the value is a JVM resource path.",
    "restart_with_inputs":
        "ExecutionControllerRunnerTest.java:176 supplies FILE input `file` as the "
        "classpath resource data/hello.txt; the value is a JVM resource path.",
    "secret-input-validation":
        "InputsTest.java:493 secretInputValidation() never executes the flow -- it "
        "calls flowIO.readExecutionInputs and asserts it throws.  No test drives an execution.",
    "scheduled-inputs-defaults":
        "Only FlowValidationTest.java:231 references it, and only to parse().  No test executes it.",
    "kv":
        "No Java test drives this flow.  Both inputs have YAML defaults, so a bare "
        "execution would silently take them and produce a golden for a path no Kestra test asserts.",
    "trigger-flow-listener":
        "Created only by its own io.kestra.plugin.core.trigger.Flow trigger, from an "
        "execution of trigger-flow (FlowTriggerCaseTest.java:48); its inputs render "
        "from trigger variables that do not exist on a direct execution.",
    "trigger-flow-listener-namespace-condition":
        "Same as trigger-flow-listener: created only by a Flow trigger "
        "(FlowTriggerCaseTest.java:62).",
    "trigger-flow-listener-invalid":
        "Nothing in the Kestra checkout references this flow at all -- no Java test, "
        "no other flow.  There is no test case to recover.",
    "flow-trigger-invalid-inputs-flow-listen":
        "Created only by a Flow trigger on flow-trigger-invalid-inputs-flow-a "
        "(MultipleConditionTriggerCaseTest.java:282); its inputs deliberately reference "
        "trigger outputs that do not exist, and the test asserts the TRIGGER produces a "
        "FAILED execution with no task runs.",
    "webhook-inputs":
        "Executed only through POST /executions/webhook/{ns}/webhook-inputs/{key} with a "
        "JSON body (ExecutionControllerRunnerTest.java:1491); its inputs come from the "
        "webhook body, not from an execution POST.",
    "subflow-to-retry":
        "Only ever created as a subflow of subflow-parent-retry "
        "(SubflowRunnerTest.java:118); no test posts inputs to it directly.",
    "subflow-nullable-input-child":
        "Only ever created as a subflow of subflow-nullable-input-parent "
        "(SubflowRunnerTest.java:144); no test posts inputs to it directly.",
    "subflow-function-recursive":
        "SubflowFunctionTest.java:132 drives it through the subflow() Pebble function and "
        "asserts the call THROWS on the recursion-depth cap.  Its input default calls "
        "subflow() on its own id, so there is no input map to recover.",
    "subflow-function-mutual-a":
        "SubflowFunctionTest.java:147 drives it through the subflow() Pebble function and "
        "asserts the call THROWS on the recursion-depth cap (a -> b -> a).",
    "subflow-function-mutual-b":
        "Half of the a -> b -> a mutual-recursion pair at SubflowFunctionTest.java:147; "
        "only ever reached through subflow-function-mutual-a.",
}

# --------------------------------------------------------------------------
# The cross-check that makes the input recovery FALSIFIABLE.
#
# The failure mode this whole table exists to prevent is silent: a wrong input
# does not error, it runs the flow's other branch.  So for every flow whose input
# was recovered, record the terminal state the SAME Java test asserts, and check
# the harvested execution against it.  A wrong `finally-parallel` input would show
# up here as SUCCESS where Kestra's test asserts FAILED.
#
# Only flows whose Java test asserts an execution-level state are listed; tests
# that assert only task counts or outputs are not (they belong to Task 2's trace
# comparison, not to a state check).
# --------------------------------------------------------------------------
EXPECTED_STATE = {
    "valids/allow-failure": ("FAILED", "AllowFailureTest.java:54 failed()"),
    "valids/condition_with_input": ("FAILED", "ExecutionControllerRunnerTest.java:747 replayExecutionWithNewInputs()"),
    "valids/exit": ("WARNING", "ExitTest.java:18 shouldExitTheExecution()"),
    "valids/exit-killed": ("KILLED", "ExitTest.java:26 shouldExitAndKillTheExecution()"),
    "valids/exit-nested": ("FAILED", "ExitTest.java:35 shouldExitAndFailNestedIf()"),
    "valids/exit-cancelled": ("CANCELLED", "ExitTest.java:47 shouldExitWithCancelled()"),
    "valids/fail-on-switch": ("FAILED", "FailTest.java:37 failOnSwitch()"),
    "valids/fail-on-condition": ("FAILED", "FailTest.java:50 failOnCondition()"),
    "valids/finally-sequential": ("FAILED", "FinallyTest.java:64 sequentialWithErrors()"),
    "valids/finally-sequential-error": ("FAILED", "FinallyTest.java:110 sequentialErrorBlockWithErrors()"),
    "valids/finally-allowfailure": ("WARNING", "FinallyTest.java:146 allowFailureWithErrors()"),
    "valids/finally-parallel": ("FAILED", "ParallelTest.java:53 errors()"),
    "valids/finally-loop": ("FAILED", "FinallyTest.java:221 loopWithErrors()"),
    "valids/finally-dag": ("FAILED", "DagTest.java:93 errors()"),
    "valids/finally-flow": ("FAILED", "FinallyTest.java:355 flowWithErrors()"),
    "valids/finally-flow-error": ("FAILED", "FinallyTest.java:389 flowErrorBlockWithErrors()"),
    "valids/if": ("SUCCESS", "AbstractRunnerTest.java:675 multipleIf()"),
    "valids/if-condition": ("SUCCESS", "IfTest.java:47 ifTruthy()"),
    "valids/if-without-else": ("SUCCESS", "IfTest.java:124 ifWithoutElse()"),
    "valids/json-input-schema-validation": ("SUCCESS", "JsonInputSchemaFlowIntegrationTest.java:50"),
    "valids/parallel-fail-with-flowable": ("FAILED", "ParallelTest.java:69 parallelFailWithFlowable()"),
    "valids/replay-loop": ("FAILED", "ExecutionServiceTest.java:140 restartLoop()"),
    "valids/sla-execution-condition": ("CANCELLED", "SLATestCase.java:64 executionConditionSLAShouldCancel()"),
    "valids/switch": ("SUCCESS", "SwitchTest.java:39 switchFirst()"),
    "valids/switch-impossible": ("FAILED", "SwitchTest.java:117 switchImpossible()"),
    "valids/task-flow": ("FAILED", "FlowCaseTest.java:33 waitFailed()"),
    "valids/working-directory": ("FAILED", "WorkingDirectoryTest.java:186 failed()"),
    "valids/subflow-nullable-input-parent": ("SUCCESS", "SubflowRunnerTest.java:161"),
    "valids/pause-behavior": ("FAILED", "PauseTest.java:376 runDurationWithBehavior(FAIL)"),
    "invalids/loop-switch-failed": ("FAILED", "RunContextTest.java:344 failedTasksVariable()"),
}


SUBEXEC_RE = re.compile(r'\[\[link execution="([^"]+)"\]\]')


def key(entry):
    """Look inputs up by base name, but only for `valids`.

    `inputs.yaml` exists in BOTH valids/ and invalids/ under the same base name;
    keying on the base alone would hand the invalid one the valid one's table entry.
    """
    return entry["base"] if entry["kind"] == "valids" else entry["id"]


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
            if not name.endswith(".yaml"):
                continue
            path = os.path.join(d, name)
            src = open(path, encoding="utf-8").read()
            fid = re.search(r"^id:\s*(\S+)", src, re.M)
            ns = re.search(r"^namespace:\s*(\S+)", src, re.M)
            entries.append({
                "id": f"{kind}/{name[:-5]}",
                "path": f"corpus/{kind}/{name}",
                "kind": kind,
                "base": name[:-5],
                "flow_id": fid.group(1).strip('"') if fid else None,
                "namespace": ns.group(1).strip('"') if ns else None,
                "declares_inputs": bool(re.search(r"^inputs:", src, re.M)),
                "source": src,
            })
    return entries


INVALID_TYPE_RE = re.compile(r"Invalid type: (\S+)")


def rejected_class(entry, why):
    """Why a 4xx publish happened -- three different things wear the same status.

    Task 8 ("refuse what Kestra refuses") may only take `invalid-flow` as its
    evidence.  A `valids` flow the pinned image refuses is NOT a flow egret should
    refuse: it is a gap between the artifact and the checkout, and treating it as
    an `invalids` would teach egret to reject a valid flow.
    """
    if entry["kind"] == "invalids":
        return "invalid-flow"
    m = INVALID_TYPE_RE.search(why)
    if m:
        return "task-type-not-in-image"
    return "valid-in-checkout-refused-by-pinned-image"


def detail(body):
    try:
        d = json.loads(body)
        return d.get("detail") or d.get("message") or body[:300]
    except Exception:
        return body[:300]


def descendants(root_id, pg):
    """Sub-executions are HIDDEN from /executions/search.

    Two independent discoveries, unioned:
      1. the parent's log lines, which carry `in sub-execution [[link execution="..."]]`
      2. Kestra's Postgres, walking the generated `parent_id` column transitively
    Neither alone is trusted: the log line only appears for tasks that log it, and
    Postgres is the storage layer rather than the API.
    """
    found = set()

    status, body = http("GET", f"{BASE}/{TENANT}/logs/{root_id}")
    if status == 200:
        found.update(SUBEXEC_RE.findall(body))

    sql = (
        "WITH RECURSIVE d AS ("
        f"  SELECT id FROM executions WHERE parent_id = '{root_id}'"
        "  UNION ALL"
        "  SELECT e.id FROM executions e JOIN d ON e.parent_id = d.id"
        ") SELECT id FROM d;"
    )
    r = subprocess.run(
        ["docker", "exec", pg, "psql", "-U", "kestra", "-d", "kestra", "-tAc", sql],
        capture_output=True, text=True,
    )
    if r.returncode == 0:
        found.update(x.strip() for x in r.stdout.split() if x.strip())

    found.discard(root_id)
    return found


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


def run_flow(entry, pg):
    """POST the execution, wait for it and everything it spawned."""
    ns, fid = entry["namespace"], entry["flow_id"]
    spec = INPUTS.get(key(entry), {})
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
            return "rejected", f"publish -> 200; POST execution -> {status}: {detail(resp)}", None, []
        return "not-standalone", f"POST execution -> {status}: {detail(resp)}", None, []

    root = json.loads(resp)["id"]
    doc, err = await_terminal(root, EXEC_TIMEOUT)
    if doc is None:
        reason = f"execution {root} {err}"
        if "still PAUSED" in err:
            src = entry["source"]
            if "core.flow.Pause" in src and "pauseDuration" not in src:
                reason = (
                    f"reached PAUSED and stayed there for the full {EXEC_TIMEOUT:.0f}s: the flow's "
                    "io.kestra.plugin.core.flow.Pause task declares no pauseDuration, so it waits "
                    "for an explicit resume that a standalone harvest does not issue.  Kestra's own "
                    "tests drive these with runOneUntilPaused(...) followed by "
                    "executionService.resume(...).  "
                    f"(execution {root})"
                )
        return "not-standalone", reason, root, []

    subs = descendants(root, pg)
    sub_states = []
    sub_deadline = time.time() + SUB_TIMEOUT
    pending = set(subs)
    while pending and time.time() < sub_deadline:
        for sid in list(pending):
            s, b = http("GET", f"{BASE}/{TENANT}/executions/{sid}")
            if s != 200:
                pending.discard(sid)
                continue
            st = json.loads(b).get("state", {}).get("current")
            if st in TERMINAL:
                sub_states.append({"id": sid, "state": st})
                pending.discard(sid)
        if pending:
            time.sleep(POLL)
            subs |= descendants(root, pg)
            pending |= (subs - {x["id"] for x in sub_states})

    if pending:
        return ("not-standalone",
                f"{len(pending)} sub-execution(s) never reached a terminal state",
                root, sub_states)

    return "runs", doc["state"]["current"], root, sub_states


# --------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-up", action="store_true", help="assume the stack is already running")
    ap.add_argument("--down", action="store_true", help="docker compose down -v and exit")
    args = ap.parse_args()

    if args.down:
        compose("down", "-v")
        return

    if not args.no_up:
        compose("up", "-d")
    wait_ready()
    pg = pg_container()

    entries = read_corpus()
    print(f"[corpus] {len(entries)} flows "
          f"({sum(1 for e in entries if e['kind'] == 'valids')} valids, "
          f"{sum(1 for e in entries if e['kind'] == 'invalids')} invalids)")

    out = []
    for i, e in enumerate(entries, 1):
        rec = {
            "id": e["id"],
            "path": e["path"],
            "flow_id": e["flow_id"],
            "namespace": e["namespace"],
            "declares_inputs": e["declares_inputs"],
        }

        status, body = http(
            "POST", f"{BASE}/{TENANT}/flows",
            e["source"].encode("utf-8"), "application/x-yaml",
        )
        rec["publish_status"] = status

        if 400 <= status < 500:
            rec["disposition"] = "rejected"
            rec["reason"] = f"publish -> {status}: {detail(body)}"
            rec["rejected_class"] = rejected_class(e, detail(body))
        elif status != 200:
            rec["disposition"] = "unclassified"
            rec["reason"] = f"publish -> {status}: {detail(body)}"
        elif key(e) in NOT_STANDALONE_INPUTS:
            rec["disposition"] = "not-standalone"
            rec["reason"] = NOT_STANDALONE_INPUTS[key(e)]
            rec["inputs"] = {}
            rec["inputs_from"] = "not recoverable -- see reason"
        else:
            spec = INPUTS.get(key(e))
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

            disp, why, root, subs = run_flow(e, pg)
            rec["disposition"] = disp
            rec["reason"] = why
            if disp == "rejected":
                rec["rejected_class"] = rejected_class(e, why)
            if e["kind"] == "invalids" and disp != "rejected":
                # Kestra's own repo files this under invalids/, and this Kestra ACCEPTS it.
                # Task 8 must not treat it as a flow egret has to refuse.
                rec["accepted_though_invalid"] = True
            if root:
                rec["execution_id"] = root
            rec["sub_executions"] = len(subs)

        out.append(rec)
        print(f"[{i:3}/{len(entries)}] {rec['id']:<62} {rec['disposition']:<15} {str(rec.get('reason'))[:90]}")

    with open(os.path.join(HERE, "corpus.json"), "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, sort_keys=False)
        f.write("\n")

    write_runnable(out)
    print("\n[done] corpus.json + runnable.txt written")


def write_runnable(out):
    buckets = {"runs": [], "rejected": [], "not-standalone": [], "unclassified": []}
    for r in out:
        buckets.setdefault(r["disposition"], []).append(r)

    lines = []
    lines.append("W3a execution oracle -- how much of Kestra's own corpus runs standalone")
    lines.append("=" * 72)
    lines.append("")
    lines.append("Measured by oracle-exec/harvest.py against kestra/kestra:v2.0.0-rc13.")
    lines.append("Corpus: kestra @ 96893b589a02e5d5d85c722855f76c08e79085af,")
    lines.append("core/src/test/resources/flows/{valids,invalids}.")
    lines.append("")
    lines.append(f"total flows                {len(out)}")
    for k in ("runs", "rejected", "not-standalone", "unclassified"):
        lines.append(f"  {k:<24} {len(buckets.get(k, []))}")
    lines.append("")
    v = [r for r in out if r["id"].startswith("valids/")]
    iv = [r for r in out if r["id"].startswith("invalids/")]
    for label, group in (("valids", v), ("invalids", iv)):
        lines.append(f"{label} ({len(group)})")
        for k in ("runs", "rejected", "not-standalone", "unclassified"):
            lines.append(f"  {k:<24} {sum(1 for r in group if r['disposition'] == k)}")
        lines.append("")

    rej = buckets.get("rejected", [])
    lines.append("rejected, by class")
    for cls in ("invalid-flow", "task-type-not-in-image",
                "valid-in-checkout-refused-by-pinned-image"):
        lines.append(f"  {cls:<44} {sum(1 for r in rej if r.get('rejected_class') == cls)}")
    lines.append("")
    lines.append("  Only `invalid-flow` is evidence that egret must refuse a flow.")
    lines.append("  The other two are the pinned image lacking something the checkout has.")
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

    for k in ("not-standalone", "rejected", "unclassified"):
        rows = buckets.get(k, [])
        lines.append("")
        lines.append(f"{k} -- {len(rows)}")
        lines.append("-" * 72)
        for r in sorted(rows, key=lambda x: x["id"]):
            cls = f"  [{r['rejected_class']}]" if r.get("rejected_class") else ""
            lines.append(f"{r['id']}{cls}")
            lines.append(f"    {r.get('reason', '')}")
        lines.append("")

    lines.append("")
    lines.append(f"runs -- {len(buckets['runs'])}")
    lines.append("-" * 72)
    for r in sorted(buckets["runs"], key=lambda x: x["id"]):
        extra = f"  (+{r['sub_executions']} sub)" if r.get("sub_executions") else ""
        lines.append(f"{r['id']:<62} {r.get('reason', '')}{extra}")
    lines.append("")

    lines.append("")
    lines.append("")
    lines.append("input recovery cross-check")
    lines.append("-" * 72)
    lines.append("Each recovered input is checked against the terminal state the SAME Java")
    lines.append("test asserts.  A wrong input does not error -- it runs the other branch --")
    lines.append("so this is what makes the recovery falsifiable rather than asserted.")
    lines.append("")
    by_id = {r["id"]: r for r in out}
    agree = differ = skipped = 0
    for cid, (want, where) in sorted(EXPECTED_STATE.items()):
        r = by_id.get(cid)
        if r is None:
            lines.append(f"  MISSING  {cid}")
            differ += 1
            continue
        if r["disposition"] != "runs":
            lines.append(f"  n/a      {cid:<52} not exercised: {r['disposition']}")
            skipped += 1
            continue
        got = r.get("reason")
        if got == want:
            agree += 1
            lines.append(f"  agrees   {cid:<52} {got:<10} {where}")
        else:
            differ += 1
            lines.append(f"  DIFFERS  {cid:<52} got {got}, Kestra asserts {want} ({where})")
    lines.append("")
    lines.append(f"  agrees {agree}   differs {differ}   not exercised {skipped}"
                 f"   (of {len(EXPECTED_STATE)})")
    lines.append("")

    with open(os.path.join(HERE, "runnable.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


if __name__ == "__main__":
    main()
