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


INVALID_TYPE_RE = re.compile(r"Invalid type: (\S+)")

# A `valids` flow the PINNED IMAGE refuses may be either of two very different
# things, and only one of them is evidence for Task 8.  Neither may be inferred
# from the absence of something in the error message: a bucket that collects by
# default is not a measurement, and this is precisely the bucket Task 8 is told
# to ignore, so a mis-file into it is invisible by construction.
#
# Both tables are therefore CURATED, each entry carrying the checkout line that
# proves the claim.  Anything not in them and not matching `Invalid type:` gets
# `unclassified-rejection` with the raw message, which is an open question rather
# than a dismissal.

# The checkout has a feature the pinned artifact does not.  NOT evidence for
# Task 8: the flow is valid, this Kestra is just older.
CHECKOUT_DRIFT = {
    "valids/flow-trigger-fire-once-true-flow-listen":
        "The trigger sets `fireOnce`, which the checkout declares at "
        "core/src/main/java/io/kestra/core/models/triggers/Window.java:79 "
        "(`@Builder.Default @PluginProperty @NotNull private boolean fireOnce = false;`) "
        "and v2.0.0-rc13 does not know: `Unrecognized field \"fireOnce\"`.",
}

# A flow that lives under valids/ but that the CHECKOUT refuses too.  This IS
# evidence for Task 8 -- egret must refuse it -- despite the directory it sits in.
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


def rejected_class(entry, why):
    """Why a 4xx publish happened.  Four different things wear the same status.

    Each class is entered on POSITIVE evidence only:

      invalid-flow             the flow is in invalids/            (evidence: the directory)
      task-type-not-in-image   the message says `Invalid type: X`  (evidence: the message)
      checkout-drift           curated, CHECKOUT_DRIFT             (evidence: a checkout line)
      refused-by-both          curated, REFUSED_BY_BOTH            (evidence: a checkout line)
      unclassified-rejection   everything else -- an open question, not a bucket

    Task 8 may take `invalid-flow` and `refused-by-both` as evidence that egret
    must refuse a flow.  It may take neither `checkout-drift` nor
    `unclassified-rejection`: the first is a valid flow, the second is unknown.
    """
    if entry["kind"] == "invalids":
        return "invalid-flow"
    if entry["id"] in CHECKOUT_DRIFT:
        return "checkout-drift"
    if entry["id"] in REFUSED_BY_BOTH:
        return "refused-by-both"
    if INVALID_TYPE_RE.search(why):
        return "task-type-not-in-image"
    return "unclassified-rejection"


TASK8_EVIDENCE = ("invalid-flow", "refused-by-both")


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
                root, sub_states, observe(doc))

    return "runs", doc["state"]["current"], root, sub_states, observe(doc)


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
                        ("REFUSED_BY_BOTH", REFUSED_BY_BOTH)):
        for k in table:
            if k not in ids:
                problems.append(f"{name}[{k!r}] names no corpus flow")
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
    "valids/pause-behavior":
        "MEASURED: on v2.0.0-rc13 all four documented behaviours -- FAIL, RESUME, WARN, "
        "CANCEL -- produce the identical execution (FAILED, 1 task run, histories "
        "CREATED/RUNNING/PAUSED/RESTARTED/RUNNING/FAILED).  The behavior property, "
        "rendered from an input, has no observable effect on this artifact, though "
        "PauseTest.java:374-380 asserts RESUME -> SUCCESS with 2 task runs.  So this "
        "row's `agrees` is a coincidence, not evidence that the input landed, and Task 2 "
        "must not build a golden for this flow from the checkout's semantics.",
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
        verdict = None
        for alt in usable:
            body, ctype = multipart(alt["inputs"])
            status, resp = http("POST", f"{BASE}/{TENANT}/executions/"
                                        f"{e['namespace']}/{e['flow_id']}", body, ctype)
            if status != 200:
                verdict = {"probed": True, "differs": True,
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": f"the alternative input is REFUSED ({status}: "
                                  f"{detail(resp)[:120]}), so a swap is visible"}
                break
            eid = json.loads(resp)["id"]
            doc, err = await_terminal(eid, EXEC_TIMEOUT)
            if doc is None:
                verdict = {"probed": True, "differs": True,
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": f"the alternative input does not terminate ({err}), "
                                  "so a swap is visible"}
                break
            bad = check_row(row, observe(doc))
            tried.append(json.dumps(alt["inputs"]))
            if bad:
                verdict = {"probed": True, "differs": True,
                           "alt_inputs": alt["inputs"], "alt_from": alt["from"],
                           "why": "; ".join(bad)}
                break
        if verdict is None:
            verdict = {"probed": True, "differs": False,
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


# --------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-down", action="store_true",
                    help="skip the `down -v`; the pre-flight empty-Kestra assertion still runs")
    ap.add_argument("--no-up", action="store_true", help="assume the stack is already running")
    ap.add_argument("--down", action="store_true", help="docker compose down -v and exit")
    ap.add_argument("--rewrite-runnable", action="store_true",
                    help="regenerate runnable.txt from committed corpus.json + probe.json; no Kestra needed")
    args = ap.parse_args()

    if args.rewrite_runnable:
        with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as f:
            out = json.load(f)
        out = out if isinstance(out, list) else out.get("entries", [])
        with open(os.path.join(HERE, "probe.json"), encoding="utf-8") as f:
            probe = json.load(f)
        write_runnable(out, probe)
        print("rewrote runnable.txt from corpus.json + probe.json")
        return

    if args.down:
        compose("down", "-v")
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


def write_runnable(out, probe):
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
    # DERIVED from the data, never a hand-written class list.  An earlier
    # version enumerated three names it expected: it printed a 0 for a class
    # that no longer exists and omitted `checkout-drift` and `refused-by-both`
    # entirely, so the table summed to 49 while `rejected` said 51.  Two flows
    # were invisible in the one file this package calls its honest denominator.
    # A summary that lists what its author expected is not a measurement.
    lines.append("rejected, by class")
    by_class = Counter(r.get("rejected_class") or "unclassified-rejection" for r in rej)
    for cls, n in sorted(by_class.items(), key=lambda kv: (-kv[1], kv[0])):
        lines.append(f"  {cls:<44} {n}")
    assert sum(by_class.values()) == len(rej), (
        f"class table {sum(by_class.values())} != rejected {len(rej)}")
    lines.append(f"  {'TOTAL':<44} {sum(by_class.values())}")
    lines.append("")
    lines.append("  Only `invalid-flow` is evidence that egret must refuse a flow.")
    lines.append("  Every other class is the pinned image or the checkout differing")
    lines.append("  from each other, which Task 8 must NOT read as a refusal.")
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
    for cid, verdict, why, asserts in rows:
        lines.append(f"  {verdict:<8} {cid:<52} [{asserts}]")
        lines.append(f"           {why}")
    lines.append("")
    lines.append(f"  agrees {agree}   differs {differ}   not exercised {skipped}"
                 f"   (of {len(EXPECTED)} rows)")
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

    with open(os.path.join(HERE, "runnable.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


if __name__ == "__main__":
    main()
