# Workflow engine: Kestra parity roadmap

**Status:** order approved by the user on 2026-10-18. Measured 2026-10-18 on `feat/workflow-kestra-executor-tasks` (main `a2850644` + group C2 in progress) against the Kestra checkout `data-portal/kestra` at `96893b589` (2.0.0-SNAPSHOT, the programme's pin). Both measurements read code and oracle files only (no cargo, no docker); the evidence, with `file:line` for every row, is in Appendix A (oracle corpus) and Appendix B (feature surface).

**As of (2026-10-18, C2 final review):** §1 and the appendices are the 2026-10-18 measurement taken while C2 was in progress; C2 has since been built (73 baselined, LIVE 293) — see §2 row 0.

**Programme spec:** `docs/en/specs/2026-09-01-workflow-programme-design.md`. Every W-package in its §9 has merged; this roadmap orders what comes after group C (`docs/en/specs/2026-10-18-workflow-core-tasks-design.md`).

## 1. Where egret stands

**Oracle corpus** (Kestra's own test flows, `crates/egret-workflow/oracle-exec/`):

| | count | source |
|---|---|---|
| corpus flows | 351 (some `driver.rs` comments still say 345, which is stale) | `corpus.json` |
| published and live on egret | 281 | `driver.rs` `LIVE` |
| compared against Kestra's golden | 301 | `golden.json` |
| match the golden | 217 | 301 − 84 |
| mismatch, baselined with a cause | 84 | `baseline.txt` |
| Kestra runs them, egret refuses | 18: 12 group C2, 3 `Pause` properties and 1 `AllowFailure` block `retry:` (both decided), 2 over-refusals | `executor/mod.rs` `PUBLISHED_BUT_REFUSED` |

The 84 by cause: C2 12, `finally:` 11, `afterExecution:` 9, renderer divergences 8, `sla: MAX_DURATION` 5, `Loop` own `outputs:` 5, timing/poller latency 4 and order pairs a clockless driver cannot reach 4 (both mostly permanent harness limits), invalids over-accepted 4, unattributed `Loop` divergences 4, `Pause` 3, flow `outputs:`/SECRET 3, `WorkingDirectory`+`runIf` 3, Subflow retry 2, missing Pebble functions 2, single causes 4, deliberate 1.

**Feature surface** (Kestra 2.0 OSS, beyond the corpus):
- **Core tasks:** 49. 38 supported (11 of them partly), 3 in progress (C2), 8 missing: `http.Request`, `http.Download`, `http.SseRequest`, `PurgeExecutions`, `PurgeFlows`, `PurgeStorage`, `metric.Publish`, `TemplatedTask`.
- **Silent drops, the worst class:** flow-level `finally:` and `afterExecution:` are parsed and never planned (checked: `build_plan` reads neither; only a depth test does), and `finally:` on every flowable except `AllowFailure` is dropped too. `stopAfter` and `retry.warningOnRetry` are parsed and have no effect. No warning names any of them.
- **Registered but unserved:** `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack` have `TASK_TYPES` rows and no deployment wires a handler (checked: `runtime.rs` wires none of them), so publish refuses them with `task_type_not_served`.
- **Inputs:** `required`, type, `values`, `validator`, min/max and date bounds are not checked at manual, API, webhook or flow-trigger start; only backfill checks `required`.
- **Triggers:** `ScheduleOnDates` and `http.Trigger` missing; Schedule `when:` and webhook `wait`/`returnOutputs` refused.
- **Execution operations:** start, list, get, graph, logs, files, kill, resume (no inputs) and whole-execution replay exist. Restart in place, replay from a task, change-status, pause, delete, labels API, bulk operations, live streaming and the expression-eval endpoint are missing.
- **Expressions:** `render`/`renderOnce`/`errorLogs`/`subflow` refused; `max`/`min`, ten Pebble filters and the `null`/`empty`/`map`/`json` tests missing; `secret()` resolves only in `kestra.*` plugin tasks.
- **Plugins:** the plugin host runs `RunnableTask`s from 3 jars (9 JDBC classes), with a 1.6–3.6 s JVM start per attempt.
- **Runners:** `WorkingDirectory` runs on Docker only, which the chart refuses in production.

## 2. Approved order

Each package goes through the usual flow: spec (or a spec section) → plan → subagent-driven build → final review → gate → PR → merge. Sizes: S ≈ one task, M ≈ one package, L ≈ more than one package. "Banks" is the oracle's upper bound from Appendix A; the ratchet measures the real number.

| # | Package | Scope | Banks | Size |
|---|---|---|---|---|
| 0 | **Group C2** (built 2026-10-18, PR pending: 11 of 12 banked, 73 baselined, LIVE 293) | `SetVariables`, `UnsetVariables`, `Labels`; migration 0196 | 11 of up to 12 | M |
| 1 | **No silent drops** | Turn every parsed-but-ignored key into a named refusal or warning: flow and flowable `finally:`/`afterExecution:` (until #2 runs them), trigger `stopAfter`, `retry.warningOnRetry`, the Subflow keys egret does not model (`revision`, `labels`, `inheritLabels`, `scheduleDate`, `restartBehavior`), the input keys egret does not model. Fix the stale "345" figures in `driver.rs` comments | 0 (correctness) | S |
| 2 | **End hooks: `finally:` and `afterExecution:`** | Flow-level `finally:` and `afterExecution:`, and `finally:` on every flowable (`AllowFailure` already plans one, so the pattern exists); `Loop errors:` if it falls out cheaply | ~20 upper, 14–15 strict | M |
| 3 | **Inputs checked at start** | `required`, type coercion errors, SELECT/MULTISELECT `values`/`allowCustomValue`, STRING `validator`, INT/FLOAT min/max, date bounds, ARRAY items, at manual, API, webhook and flow-trigger start; the `validate inputs` endpoint | a few invalids | M |
| 4 | **HTTP and the unserved platform tasks** | `http.Request` and `http.Download` as worker handlers with an egress policy (same SSRF stance as `http()`); decide and wire `egret.sql` (needs a `QueryExecPort`) and the other `egret.*` rows, or remove the rows | 0 | M |
| 5 | **Renderer parity** | Division by zero (the `restart-*` Infinity case), undefined `taskrun.value`, number-vs-string comparison, `??` on a missing root; `render`/`renderOnce`, `errorLogs`, `max`/`min`, the missing filters and tests | ~8–10 | S–M |
| 6 | **`Loop` own `outputs:`** | Evaluate a `Loop`'s `outputs:` at its end (STORE/AUTO stay deferred by the 2026-10-03 ruling); name the cause of the 4 unattributed `Loop` entries first | 3 (+ up to 4) | M |
| 7 | **Execution operations** | Restart failed tasks in place, replay from a chosen task (with inputs), change-status, pause, delete, labels API; then bulk operations | 0 (operability) | M–L |
| 8 | **Flow `concurrency:`** | `limit` with QUEUE/CANCEL/FAIL, unqueue and force-run | few | M |
| 9 | **Later, in this order unless re-ranked** | Schedule `when:` + `stopAfter` + `ScheduleOnDates` (S–M); synchronous webhooks `wait`/`returnOutputs` (M); `secret()` outside plugin tasks, by injecting rather than rendering (M–L); `WorkingDirectory` on Kubernetes (M); `sla: MAX_DURATION` with a harness time-model change (M–L); Python `dependencies:`/uv and more script languages (S–M each); Subflow `labels`/`inheritLabels`/`revision`/`scheduleDate`, Pause `onResume` and resume-with-inputs (S–M each); `checks:`, `metric.Publish` and a metric store (L), `http.Trigger`, `PurgeExecutions`/`PurgeFlows`/`PurgeStorage`, `TemplatedTask`; SSE follow and the expression-eval endpoint | — | — |

**Expected after #0 and #2:** the baseline falls from 84 to about 52.

## 3. Not planned

- **Harness limits** (8 entries): timing and poller latency, and order pairs a clockless driver cannot reach. They stay baselined unless the driver gains a time model.
- **Deliberate divergences** (16, listed in Appendix B §8), among them: namespace-confined writes and purges, `http()` refused, `Process` runner refused, `PurgeLogs` scope, no metrics from `Assert`.
- **Decided refusals:** `Pause` `behavior:`/`errors:`/`onPause:` (ruling 2026-10-11) and `AllowFailure` block `retry:`, until a package takes them on; plugin triggers (W5b §11).
- **Not workflow-engine scope:** Kestra dashboards, previews, blueprints, AI, MCP, Apps, EE features.

## 4. Keeping this current

Re-measure when a package merges: the ratchet's printout gives the corpus counts, and `python3 handler_surface.py` the handler surface. Update §1, mark the package done in §2 with its PR, and leave the appendices as the dated record of 2026-10-18.

---

## Appendix A — egret vs Kestra oracle corpus: gap by cause (after PR #691, worktree egret-coretasks)

Sources: `crates/egret-workflow/oracle-exec/{corpus.json,golden.json,baseline.txt,driver.rs,README.md}`,
`src/executor/mod.rs` (`PUBLISHED_BUT_REFUSED`), `tests/executor.rs` (`UNATTRIBUTED`, `CONSTRUCTS`, `STILL_OVER_ACCEPTED*`).
No file edited, no cargo, no docker. Numbers below were recomputed from the files with python3 unless marked "(const)".

### 1. Population

| quantity | value | source / note |
|---|---|---|
| corpus flows | **351** (not 345) | `corpus.json` has 351 entries; `corpus/valids` 323 files, `corpus/invalids` 28. The brief's "345" is stale: it is the pre-WorkingDirectory-Task-8 figure, still in some driver.rs doc comments ("the reason LIVE is not 345"). `executor/mod.rs` sweep asserts `(published 301, entries 351)`. |
| Kestra publish_status | 200: 301 (299 valids + 2 invalids); 422: 50 (24 valids + 26 invalids) | `corpus.json` |
| `corpus.json` disposition | runs 274, rejected 51, not-standalone 26 | `corpus.json` |
| golden (comparable) entries | **301** = 273 `runs` + 28 `refused` | `golden.json`; the 50 other flows (26 not-standalone, ~24 rejected by harvest) are never compared |
| LIVE (published, version exists) | **281** | driver.rs `LIVE` (const) |
| refused at parse | 6 | `PARSE_REFUSED` (const) |
| name-contest losers (create refused) | 4 | `CREATE_REFUSED` (const) |
| refused at publish: unknown task type | 33 | `UNKNOWN_TYPE_REFUSED` (const) |
| refused at publish: egret validate | 27 | `VALIDATE_REFUSED` (const) |
| check | 281+6+4+33+27 = **351** | consistent |
| `PUBLISHED_BUT_REFUSED` rows | **18** (Kestra published with 200, egret refuses) | `executor/mod.rs:4219`; cross-cuts the stages above |
| baselined (mismatching golden) | **84** (79 valids/runs, 4 invalids/refused, 1 invalids/runs = `loop-switch-failed`); all 84 are in golden | `baseline.txt` (84 non-comment lines) |
| matching | **217** = 301 - 84 | derived |

Disagreements reported: (a) 351 vs 345, above. (b) 33 unknown-type: my YAML scan finds 37 flows naming a type absent from `src/`
(12 C2 + 23 Kestra-rejected test-only classes + 2 `fs.http.Trigger` flows that stay live); 12 + 21 = 33 so about 2 of the
Kestra-rejected ones die at parse/validate instead. Not resolvable without running the census. (c) The `tests/executor.rs`
`CONSTRUCTS` rows overlap (declaration needles), so their totals (finally 14, afterExecution 10, Loop 17 ...) are larger
than the one-cause counts below; the table below assigns each entry to ONE cause.

Refused-stage composition of the 18 `PUBLISHED_BUT_REFUSED`: 12 C2 (unknown type), 3 `Pause` property (`unsupported_pause_property`),
1 AllowFailure block `retry:` (`unsupported_allow_failure_retry`), 2 true over-refusals (`task-flow-dynamic`,
`trigger-flow-listener-namespace-condition`, both `not-standalone`, not in golden, not baselined). 16 of the 18 are also baselined.
The remaining ~60 refused-stage flows are mostly flows Kestra refused too (51 Kestra 422s) and match.

Unknown-type refusals, which types block which Kestra-published flows (all others in the 33 are Kestra-rejected test classes
that egret rightly refuses):

| missing type | flows blocked |
|---|---|
| `io.kestra.plugin.core.execution.Labels` | 7 (`labels-update-*` x5, `npe-labels-update-task`, `primitive-labels-flow`) |
| `io.kestra.plugin.core.execution.SetVariables` | 5 (`set-variables`, `set-variables-duplicate`, `task-runif-executionupdating`, `task-runif-taskswithstate`, `unset-variables`) |
| `io.kestra.plugin.core.execution.UnsetVariables` | 2 (`task-runif-executionupdating`, `unset-variables`; both also need SetVariables) |
| union | 12 flows = group C2 |

No other missing task type blocks a flow Kestra published. No trigger type blocks anything that is baselined (`io.kestra.plugin.fs.http.Trigger`
in `http-listen*` is not baselined). Webhook trigger flows: only `webhook-plaintext` is baselined, and for a renderer reason.

### 2. Every baselined entry, one cause

Cause text is from baseline.txt comments, `tests/executor.rs` `UNATTRIBUTED`/README unless marked (inferred).

**C2 in progress (12; all also refused at publish, `unknown_task_type`)**
labels-update-nested-value-list, labels-update-nested-value-map, labels-update-task, labels-update-task-deduplicate, labels-update-task-empty,
npe-labels-update-task, primitive-labels-flow, set-variables, set-variables-duplicate, task-runif-executionupdating,
task-runif-taskswithstate (also declares `finally:`), unset-variables.

**`afterExecution:` not run (9)** (`build_plan` flattens only tasks/errors/finally)
after-execution, after-execution-error, after-execution-finally (also `finally:`), after-execution-flowable,
flow-concurrency-queue-after-execution, flow-concurrency-queue-killed-after-execution, sleep-long, sleep-long-after-execution-flowable (inferred from needle),
restart-with-after-execution (README also notes a `runIf:` render cost in the harness).

**`finally:` not run (11)**, "unclaimed, no owner"
finally-dag, finally-flow, finally-flow-error, finally-flow-error-first, finally-parallel, finally-sequential, finally-sequential-error,
finally-sequential-error-first, restart-with-finally, finally-loop, finally-loop-parallel (the last two also need `Loop errors:`).

**Pause `behavior:` / `errors:` / `onPause:` (3)**, refused by user ruling 2026-10-11 until egret runs them
pause-behavior, pause_on_pause, pause-errors-finally-after-execution (also needs `finally:` and `afterExecution:`).

**`Loop` own `outputs:` not produced (5)**
loop-outputs, loop-outputs-failed-render, if-in-parallel (assert reads `outputs.for_each.outputs[i]`), loop-outputs-store and
loop-outputs-auto (STORE/AUTO file outputs, deferred by user ruling 2026-10-03, W9a spec 15).

**Renderer divergence, minijinja answers where Pebble throws/behaves differently (8)**
restart-child (`1/taskrun.attemptsCount` = Infinity), restart-parent, restart-loop, restart-parent-loop (all via restart-child), all-flowable and
disable-flowable (`{{taskrun.value}}` undefined in Kestra, FAILED there), loopuntil-failed-flowable (`iterationCount < "5"` number vs string),
webhook-plaintext (`??` over undefined ROOT).

**Missing Pebble functions (2)**: errors (`errorLogs()`, needs a task-log store), variables (`render()`).

**`sla: MAX_DURATION` (5)** sla-max-duration-fail, sla-subflow, flow-concurrency-sla-fail-child, sla-parent-flow, flow-concurrency-sla-fail-parent.
README calls these unbankable under this harness (`Sleep::capped(ZERO)` leaves no window); needs both an implementation and a harness change.

**Timing: Kestra scheduler/poller latency vs egret exact clock (4)**
retry-failed-task-duration (Kestra ~0.6 s/retry, 3 attempts vs 4; user chose to baseline), retry-failed-flow-duration (driver clock moves only to next deadline),
loop-until-restart (4 iterations vs 3, `maxDuration: PT3S`), waitfor-child-task-warning (one extra `WaitFor` iteration, same shape).

**Flow-level `outputs:` not evaluated / SECRET outputs (3)**: flow-with-outputs-failed, subflow-secret-output-parent (flow `outputs:` block),
subflow-secret-output-child (harness has no encryption key). Note `flow_outputs` appears in `src/worker/subflow.rs`/`render.rs` so the first two's cause text may be partly stale (inferred).

**Subflow retry accumulation (2)**: retry-subflow, subflow-parent-retry (`state::apply` rule 1: Kestra folds sub-execution retries into the parent attempt).

**Loop sub-execution diverges, not attributed (4, inferred)**: loop-object, loop-object-in-list, if-in-flowable (no cause text anywhere; all are `Loop`+`Switch/If` bodies
with `item.*` rendering; need a diff to name the field), retry-flowable-parallel (`retry:` on a `Loop` block; README: "held by the Loop", differs only inside `spawned`).

**`Loop errors:` (1)**: loop-switch-failed (`models::Loop` has no `errors` field; `Loop.java:272`).

**Order pairs a clockless driver cannot reach (4)**: dag, full, if, parallel-disabled-tasks (one to three `order` pairs only; harness limit).

**WorkingDirectory + `runIf:` render (3, not isolated)**: task-runif-workingdirectory, working-directory-invalid-runif (in `STILL_OVER_ACCEPTED_AT_RUNTIME`:
egret creates a FAILED run Kestra does not), working-directory-taskrun-nested (`{{workerTaskrun}}` render context suspect).

**Other single causes (3)**
parallel-fail-with-flowable (sibling still in flight when `Parallel` fails, no kill path), retry-success (`warningOnRetry: true` yields WARNING; inferred, the
UNATTRIBUTED text "RunState cannot hold WARNING" is stale since W8 Task 5), retry-with-flowable-errors (AllowFailure block-level `retry:`, decided non-goal, refused).

**Deliberate (1)**: purge_logs_full_arguments (PurgeLogs scope ruling 2026-10-18).

**Invalids over-accepted (4)**: dag-invalid-subtask (`@NotNull` on a plugin property), inputs-bad-validator-syntax (java.util.regex dialect),
invalid-property (unrecognised plugin property), inputs-with-multiple-constraint-violations (refused at a different seam).

Total: 12+9+11+3+5+8+2+5+4+3+2+4+1+4+3+3+1+4 = 84 (checked).

Not baselined but also gaps: the 2 `PUBLISHED_BUT_REFUSED` over-refusals (task-flow-dynamic, trigger-flow-listener-namespace-condition) and
2 publish-stage over-refusals (`valids/task-runif`, `valids/trigger-flow-listener-no-inputs`: Subflow/Flow-trigger reference to a flow absent from the corpus;
README "Known over-refusals at publish"). 14 `not-standalone`/10 rejected flows have no golden and are invisible to the ratchet.

### 3. Cause table

| cause | n | entries | kind | size | why |
|---|---|---|---|---|---|
| C2 SetVariables/UnsetVariables/Labels | 12 | see above | gap, **C2 in progress** | M | ExecutionUpdatableTask, new executor Command; `task-runif-taskswithstate` also needs `finally:` |
| `finally:` | 11 | finally-* family, restart-with-finally | gap | M | plan+advance after-block; 2 also need Loop `errors:`; AllowFailure already plans `finally` so pattern exists |
| `afterExecution:` | 9 | after-execution*, sleep-long*, flow-concurrency-queue-*after-execution, restart-with-after-execution | gap | M | flow-level post-terminal run list; sibling of `finally:` |
| renderer divergence (Pebble semantics) | 8 | restart-*, all-flowable, disable-flowable, loopuntil-failed-flowable, webhook-plaintext | gap, arguably decide-per-item | S each, M total | each is a distinct Pebble rule (div-by-zero, undefined `taskrun.value`, mixed compare, `??` on missing root); parent/child restart pairs bank together |
| `sla: MAX_DURATION` | 5 | sla-*, flow-concurrency-sla-fail-* | gap + harness limit | M-L | needs a monitoring loop AND driver time model change |
| `Loop` own `outputs:` | 5 | loop-outputs*, if-in-parallel | gap (STORE/AUTO 2 deferred by ruling) | M (S for 3 non-STORE) | evaluate `outputs:` at loop end; STORE needs W9a store |
| timing/latency | 4 | retry-failed-*-duration, loop-until-restart, waitfor-child-task-warning | permanent-ish (model Kestra latency) | L / not worth | would need a poller-latency model; user already chose to baseline retry-failed-task-duration |
| order-pair harness limit | 4 | dag, full, if, parallel-disabled-tasks | permanent (harness) | L | no clock; only a harness/driver change can bank |
| invalids over-accepted | 4 | dag-invalid-subtask, ... | gap | S each | validator rules (NotNull, regex dialect, unknown property, seam) |
| Loop unattributed (inferred) | 4 | loop-object, loop-object-in-list, if-in-flowable, retry-flowable-parallel | gap | S-M | needs a diff first; likely item/jq rendering or Loop-level retry |
| Pause behavior/errors/onPause | 3 | pause-behavior, pause_on_pause, pause-errors-... | decided refusal (gap) | M | `Pause` handler semantics, WARN/CANCEL, errors sub-run |
| flow `outputs:` / SECRET | 3 | flow-with-outputs-failed, subflow-secret-output-* | gap / harness key | S-M | parts may already exist in `worker/subflow.rs`; re-measure |
| WorkingDirectory + runIf render | 3 | task-runif-workingdirectory, ... | gap | S-M | render context inside a block, not isolated |
| Subflow retry accumulation | 2 | retry-subflow, subflow-parent-retry | gap | M | state machine rule 1 |
| missing Pebble functions | 2 | errors, variables | gap | M | `errorLogs()` needs log store; `render()` |
| Loop `errors:` | 1 | loop-switch-failed | gap | S-M | add field + sub-execution handling |
| parallel in-flight kill | 1 | parallel-fail-with-flowable | gap | M | kill path from `advance()` |
| retry `warningOnRetry` | 1 | retry-success | gap (inferred) | S | WARNING on retried success |
| AllowFailure block `retry:` | 1 | retry-with-flowable-errors | deliberate (decided non-goal) | M if reversed | spec 2 |
| PurgeLogs scope | 1 | purge_logs_full_arguments | deliberate, permanent | - | user ruling 2026-10-18 |

### 4. Unblock ranking (single construct -> entries banked, upper bound / strict)

1. **`finally:`** 11 upper (8 strict: finally-dag/flow x3/parallel/sequential x3 need nothing else; `finally-loop*` also need Loop `errors:`, `restart-with-finally` has a runIf render cost; 3 more secondary: after-execution-finally, task-runif-taskswithstate, pause-errors).
2. **`afterExecution:`** 9 upper (about 6-7 strict). **Build 1 and 2 as one "end hooks" package: about 20 upper, 14-15 strict (about 17-24% of the baseline)**, and it also unlocks 2 of C2/Pause combos.
3. **C2** 12 (in progress); 11 strict, `task-runif-taskswithstate` needs `finally:`.
4. Renderer divergences 8 (each its own small fix; 4 banked by fixing the Infinity case alone: restart-child/parent/loop/parent-loop).
5. `Loop` `outputs:` 5 (3 without STORE).
6. `sla:` 5, but harness-blocked, so low yield per effort.
Overlap note: after C2 + `finally:` + `afterExecution:` land, 84 would drop to roughly 84-12-20 = 52.

---

## Appendix B — egret workflow engine vs Kestra OSS feature surface (2026-10-18)

How this was measured: by reading the code only. No cargo or docker was run.

- **Kestra:** the `data-portal/kestra` checkout at `96893b589` (2.0.0-SNAPSHOT), which is the programme's source pin.
- **egret:** the `egret-coretasks` worktree. Paths are relative to `crates/egret-workflow/src/` unless another root is given.
- **Group C2** (`SetVariables`, `UnsetVariables`, `Labels`) is counted as **in progress**.

**Kestra 2.0 already dropped a lot.** At this pin, Kestra core has no `ForEach`, `ForEachItem`, `EachSequential`/`EachParallel`, `Template` or `Flow` (old subflow) task, and no condition classes at all: there is no `io.kestra.plugin.core.condition.*`. Flow-level `pluginDefaults` is gone too. Conditions became `when:` expressions plus the date functions (`isWeekend`, `isDayWeekInMonth`, ...). No class under `plugin/core` carries `@Deprecated`. The only aliases are `LoopUntil`'s `WaitFor` (`flow/LoopUntil.java:67`) and the `io.kestra.plugin.fs.http.*` names on the http classes.

**What "supported" means here.** Supported means it runs with Kestra semantics. Partial means it runs, but some properties or behaviours are refused or ignored. Missing means it is not implemented. Out means a decision was recorded to leave it out. In egret's own `spec/schema/property-treatment.json`, **"honoured" only means the key survives the parse; it does not mean the key executes.** That matters below for `finally` and `afterExecution`.

---

### 1. Core task types (`io.kestra.plugin.core.**`)

Out of the count are: abstract classes and interfaces; the purge-behaviour subtypes (`kv.Key`, `kv.Version`, `namespace.Version`, `flow.Version`), which egret handles inside `PurgeKV` and `PurgeFiles`; and helpers (`SchedulableExecutionFactory`, `WebhookInputRenderException`). The 20 dashboard chart and data classes and the 4 `preview.*` file renderers are UI-side. They are listed once, at the end.

**Tasks: 49.** 38 are supported (11 of those partially), 3 are in progress (C2) and 8 are missing.

| Kestra item | egret status | evidence |
|---|---|---|
| debug.Return | supported | spec/registry.rs:194, worker/handlers/ret.rs |
| execution.Assert | supported (C1). No metrics, by decision D1 | registry.rs:220, handlers/assert.rs |
| execution.Exit | supported (executor-level) | registry.rs:206, executor/exit.rs:92, plan.rs:1335 |
| execution.Fail | supported | registry.rs:199, handlers/fail.rs |
| execution.Labels | **in progress (C2)** | docs/en/specs/2026-10-18-workflow-core-tasks-design.md §3.3 |
| execution.SetVariables | **in progress (C2)**, migration 0196 | core-tasks spec §3.2 |
| execution.UnsetVariables | **in progress (C2)** | core-tasks spec §3 |
| execution.PurgeExecutions | **missing** | no row in TASK_TYPES (registry.rs:91-351) |
| flow.AllowFailure | supported. A block `retry:` is refused | plan.rs:1267, flowable/allow_failure.rs, validate `unsupported_allow_failure_retry` |
| flow.Dag | partial: no `finally:`. A disabled node is handled differently on purpose (plan.rs doc on flatten_list) | plan.rs:1368, models/task.rs:348 |
| flow.If | partial: no `finally:` | plan.rs:1288, models/task.rs:302 |
| flow.Loop | partial: no `errors:`/`finally:`, and STORE-mode `outputs` stays baselined | plan.rs:1345, models/task.rs:407; programme spec §9 W9a "What is still not true" |
| flow.LoopUntil (+ `WaitFor` alias) | supported. An unparseable `checkFrequency.*` or `failOnMaxReached` is refused | plan.rs:1415, plan.rs:1495-1515 (`loop_until_gap`) |
| flow.Parallel | partial: no `finally:` | plan.rs:1280, models/task.rs:274 |
| flow.Pause | partial: `tasks`, `errors`, `behavior` and `onPause` are refused; `onResume` (resume with inputs) is not supported; `finally` is absent | plan.rs:825-839 (`pause_gap`), flowable/pause.rs:52-58, routes.rs:2747 (resume takes no body) |
| flow.PurgeFlows | **missing** | not in TASK_TYPES |
| flow.Sequential | partial: no `finally:`, and the key is silently dropped by serde | plan.rs:1260, models/task.rs:253-261 |
| flow.Sleep | supported | registry.rs:235, handlers/sleep.rs |
| flow.Subflow | partial: `revision`, `labels`, `inheritLabels`, `scheduleDate` and `restartBehavior` are not modelled; `namespace`, `flowId`, `inputs`, `wait` and `transmitFailed` are | plan.rs:1340, models/task.rs:481-510 vs kestra flow/Subflow.java:87-144 |
| flow.Switch | partial: no `finally:` | plan.rs:1308, models/task.rs:326 |
| flow.WorkingDirectory | partial: **Docker runner only, refused on Kubernetes**, and the chart refuses Docker in production | plan.rs:1276; docs/en/specs/2026-10-16-workflow-working-directory-design.md:3,112 |
| http.Request | **missing**. A `core.http` registry row exists with no handler, so it is refused at publish and start as `task_type_not_served` | registry.rs:161; crates/egret-bin/src/main.rs:1861-1864 |
| http.Download | **missing** | not in TASK_TYPES |
| http.SseRequest | **missing** | not in TASK_TYPES |
| kv.Set / Get / GetKeys / Delete / Put / PurgeKV | supported (6). Divergence: writes only to the flow's own namespace | registry.rs:321-350, handlers/kv.rs; W9c spec §15 |
| log.Log | supported (alias `core.log`) | registry.rs:189 |
| log.PurgeLogs | supported (C1). Scoped to the flow's own namespace and its children | registry.rs:229, handlers/purge_logs.rs |
| metric.Publish | **missing**. egret has no metric store | core-tasks spec D1 |
| namespace.UploadFiles / DownloadFiles / DeleteFiles / PurgeFiles | supported (4) | registry.rs:296-316, handlers/namespace.rs |
| output.OutputValues | supported (C1) | registry.rs:213, handlers/output_values.rs |
| runner.Process (task runner) | **deliberately out**: refused, and only the Docker and Kubernetes runners exist | W5a spec; scripts/mod.rs:200 (`runner` refused) |
| storage.Write / Size / Concat / Split / Reverse / DeduplicateItems / FilterItems / Delete / PurgeCurrentExecutionFiles | supported (9) | registry.rs:245-290, handlers/storage.rs |
| storage.PurgeStorage | **missing** | not in TASK_TYPES |
| templating.TemplatedTask | **missing** | not in TASK_TYPES |
| dashboard.chart.* / dashboard.data.* (20), preview.* renderers (4) | out (Kestra UI dashboards and previews; nothing to do with flow execution) | n/a |

**egret-only task rows that are registered but have no handler (6 of them).** They publish nowhere, because `task_type_not_served` refuses them: `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack`. Evidence: egret-bin/src/main.rs:1835-1864. In practice the egret-native platform tasks do not run in production.

**Script tasks (separate Kestra plugin repositories).** These are supported: `scripts.shell.{Commands,Script}` and `scripts.python.{Commands,Script}` (registry.rs:116-135). Python's `dependencies`, `packageManager`, `pythonVersion` and uv auto-install are refused, and so are `targetOS` and most Docker options (scripts/mod.rs:200-247). node, R, Julia, Ruby, Go and the rest are missing; W5a spec §10 says they are "addable later".

---

### 2. Flow-level properties (`AbstractFlow`, `Flow`)

| Kestra item | egret status | evidence |
|---|---|---|
| id, namespace, description | supported | models/flow.rs:40-43 |
| inputs | partial (see the input table below) | models/flow.rs:49, models/input.rs |
| outputs | supported | models/flow.rs:51 |
| variables | supported (`vars`) | models/flow.rs:53 |
| labels | supported | models/flow.rs:46 |
| tasks, errors | supported | plan.rs:1148-1157 (`flatten_flow`) |
| **finally** | **missing, and silent**. It is parsed (flow.rs:82) but `flatten_flow` plans only `tasks` and `errors`; `Plan` has no finally list (plan.rs:20-28). There is no warning, and schema treatment calls it "honoured" | plan.rs:1148-1157; oracle-exec/README.md:3213 ("unclaimed") |
| **afterExecution** | **missing, and silent**: parsed (flow.rs:92), drawn by `graph/`, never planned | plan.rs:1148-1157; oracle-exec/README.md:3215 |
| retry (flow-level) | supported (Constant, Exponential, Random; RETRY_FAILED_TASK and CREATE_NEW_EXECUTION) | executor/retry.rs:21-27,133 |
| concurrency (limit + QUEUE/CANCEL/FAIL) | **missing, warned** (`not_yet_enforced`) | spec/validate.rs:173-176, 844 |
| sla: EXECUTION_ASSERTION | supported | sla.rs |
| sla: MAX_DURATION | **missing, warned** | sla.rs header; validate.rs:130-140 |
| checks (BLOCK/FAIL/CREATE_EXECUTION) | **missing, warned** | validate.rs:176, 927 |
| quotas (EE) | warned; Kestra itself treats this as EE-only | validate.rs:177, 897 |
| disabled | supported | routes.rs:2288, trigger/flow.rs:808, webhook_routes.rs:603 |
| triggers | partial (see section 4) | validate.rs:178-181 |
| pluginDefaults | refused at save. Kestra 2.0 removed it as well, so this agrees with Kestra | validate.rs:357-368 |
| workerSelector (formerly workerGroup) | ignored (dropped by serde) | models/flow.rs:30-31 |
| policyRefs (EE) | ignored | models/flow.rs:32-35 |
| revision, updated, deleted, tenantId | egret-managed (versions table) | models/flow.rs:26-29 |

**Input types (Kestra's `Type` enum has 20).** All 20 parse (models/input.rs:19-41), with `item_type` and `allowedFileExtensions`. What is wrong is how they are enforced:

| Kestra item | egret status | evidence |
|---|---|---|
| STRING, INT, FLOAT, BOOL, JSON, ARRAY, MULTISELECT | partial: structured types are JSON-coerced at render time (worker/render.rs:856-905). INT `min`/`max`, FLOAT bounds, STRING `validator` regex, SELECT/MULTISELECT `values`/`allowCustomValue` and ARRAY item validation are **not modelled**, so they are dropped silently | models/input.rs:53-90 (no such fields) |
| DATETIME, DATE, TIME, DURATION | partial: passed as strings, with no `after`/`before`/`min`/`max` validation | as above |
| FILE | supported (upload, `kestra://`, extension allowlist, which is stricter than Kestra) | storage/inputs.rs:107,246 |
| SECRET, URI, EMAIL, YAML, ION | partial: parse only, no type validation | models/input.rs |
| FORM, REUSABLE_INPUTS (EE) | parse only | models/input.rs:39-40 |
| `required` | **not enforced at manual, API, webhook or flow-trigger start**. Only backfill checks it (triggers/backfill.rs:311-315). The 400 list for start never mentions a missing required input (routes.rs:2396) | |
| `defaults` | supported, coerced by declared type | worker/render.rs:856-905 |
| `prefill`, `dependsOn`, `displayName` | parsed. `dependsOn` is not supported in the UI (W7a §13) | models/input.rs:62-68 |

---

### 3. Task-level properties (`Task.java`)

| Kestra item | egret status | evidence |
|---|---|---|
| id, type, description | supported | models/task.rs:25-27 |
| retry (Constant, Exponential, Random; maxAttempts, maxDuration, behavior) | supported | models/task.rs:667-690, executor/retry.rs |
| retry.warningOnRetry | **missing, silent**: parsed, read into the policy, no effect | executor/retry.rs:135 (only reader) |
| timeout | supported (worker) | models/task.rs:29, worker/timeout.rs |
| disabled | supported (removed at plan time, as Kestra does) | plan.rs:1160-1200 |
| runIf | supported (SKIPPED) | models/task.rs:52, executor/flowable |
| allowFailure, allowWarning | supported (WARNING state) | models/task.rs:66,72, executor/allow_failure.rs |
| logLevel | ignored on core tasks and flowables; **refused** on script and plugin tasks | scripts/mod.rs:194-207; plugins/mod.rs:225 |
| logToFile | same as logLevel | same |
| workerSelector | same (no worker routing) | same |
| taskCache | same | same |
| assets (lineage) | same | same |
| version (plugin version) | same | same |
| policyRefs (EE) | same | same |
| `finally:` on flowables (AbstractBranch) | only on AllowFailure. Sequential, Parallel, If, Switch, Dag, Loop, LoopUntil and Pause drop it silently | models/task.rs:253-565 |
| `{{ task.type }}` on a flowable | divergence: renders nothing | oracle-exec/README.md:3216 |

---

### 4. Triggers and conditions

| Kestra item | egret status | evidence |
|---|---|---|
| trigger.Schedule (cron, withSeconds, timezone, inputs) | supported | models/trigger.rs:110-160, scheduler/fire.rs |
| Schedule.recoverMissedSchedules | supported | scheduler/fire.rs:433 |
| Schedule.lateMaximumDelay | supported | trigger/schedule.rs:1368 |
| Schedule backfill (create/pause/delete, bulk) | supported | triggers/backfill_routes.rs:362-496 |
| Schedule `when:` (the 2.0 replacement for conditions) | **refused** (`UnsupportedWhenCondition`) | trigger/schedule.rs:1282,1354 |
| trigger `stopAfter` | **missing, silent**: parsed, nothing reads it | models/trigger.rs:52-60 (doc on `stop_after`) |
| trigger `labels` | supported on Flow, ignored on Schedule and Webhook | spec/schema/README.md:100-105 |
| trigger `disabled` + per-trigger enable/disable API | supported | triggers/switch.rs, triggers/routes.rs:987,1030 |
| trigger.ScheduleOnDates | **missing** (Leaf trigger, `not_yet_enforced` warning) | models/trigger.rs:75-80 |
| trigger.Webhook (key, inputs, fetchType, multipart) | supported. Anonymous route, concurrency guard | webhook_routes.rs:10,460 |
| Webhook `wait`, `returnOutputs`, `responseContentType`, `responseCode` | **refused** (409 on fire, removed from the editor) | trigger/webhook.rs:335-360 |
| trigger.Flow (states, inputs, dependsOn, window, mode, minSatisfied) | supported (W13a/b). Divergence: window rows cascade on listener delete; an expired window is replaced | models/trigger.rs:217-327, flow_trigger/, W13 spec:240,258 |
| http.Trigger (polling HTTP) | **missing** | Leaf trigger |
| trigger.McpToolTrigger | **missing** | Leaf trigger |
| polling and realtime plugin triggers (jdbc.Trigger, Kafka, ...) | **deliberately out** (W5b §11) | plugin-host EgretPluginHost.java:536 refuses AbstractTrigger |
| Conditions | Kestra 2.0 has no condition classes. Their replacement, `when:` with the date functions, is refused on Schedule (above). The functions themselves exist in egret | runners/functions.rs |
| Trigger unlock / restart (TriggerController) | missing. egret's claim model has no lock to release | – |

---

### 5. Execution operations (Kestra `ExecutionController` and friends vs egret REST)

egret routes are under `/api/ws/{ws}/workflows`.

| Kestra endpoint | egret status | evidence |
|---|---|---|
| POST /executions/{ns}/{id} (start, multipart inputs) | supported | routes.rs:2381 |
| POST /{ns}/{id}/validate (validate inputs) | missing | – |
| GET /search, GET /{id}, GET /{id}/flow | supported (list with state/namespace/flow/parent/time filters, detail) | routes.rs:1838,1913 |
| GET /{id}/graph | supported | routes.rs:1314 |
| GET /{id}/file, /file/metas, /file/preview | supported | storage/routes.rs:255,322,350 |
| logs GET /{id} | supported (paged) | routes.rs:2064 |
| logs follow (SSE), logs download, logs delete | missing (polling only) | W7b §10 |
| GET /{id}/follow, /follow-dependencies (SSE) | missing | W7b §10 |
| POST /{id}/actions/resume | partial: resumes all Pause runs; no `onResume` inputs; no resume/validate; no resume-from-breakpoint | routes.rs:2747 |
| POST /{id}/actions/pause | missing | – |
| DELETE /{id}/actions/kill (cascade) | supported (KILLING to KILLED, cascade default true) | routes.rs:3025; kill-and-exit spec D1-D6 |
| POST /{id}/actions/replay | partial: egret's "replay" re-runs the WHOLE execution from the frozen source. There is no replay from a chosen taskRunId, no replay-with-inputs and no revision choice | routes.rs:2822-2850 |
| POST /{id}/actions/restart (resume failed tasks in place) | **missing** | – |
| POST /{id}/actions/change-status, /actions/state (force a task-run state) | **missing** | – |
| POST /{id}/actions/labels | missing (C2 adds the task, not the API) | – |
| DELETE /{id} (delete execution + logs/metrics/storage) | **missing** | – |
| POST /{id}/actions/eval (expression debugger) | missing (`ExpressionController.render` is missing too) | – |
| unqueue, force-run (concurrency queue) | missing, because concurrency is not enforced | – |
| every `by-ids`/`by-query` bulk op (kill, restart, replay, resume, pause, labels, delete, change-status) | missing | W7b §10 |
| export CSV, distinct-field-values, average-duration, latest | missing | – |
| webhook endpoints | supported (anonymous) | webhook_routes.rs |
| OutputController / MetricController | outputs come back in the execution detail; **metrics missing** | – |
| Flow API: CRUD, validate, revisions, graph | supported (plus a draft/publish split, which is an egret design) | routes.rs:608-1283 |
| Flow API: import/export, bulk enable/disable/delete, search-in-source/replace, validate task/trigger, delete revisions | missing | W7a §13 |
| Namespace: list/get/dependencies, files (full), KV (full, plus revisions and inheritance) | supported | routes.rs:3159-3328, storage/nsfiles/routes.rs, storage/kv/routes.rs |
| Secrets API (namespace secrets, inherited-secrets) | missing in the workflow API. egret has its own typed-secrets store | – |
| Plugin docs API (schemas, icons, versions) | partial: `GET /schema` serves a trimmed flow JSON schema | routes.rs:932 |
| Concurrency-limit search, Blueprints, Dashboards, AI, MCP, Apps | missing or out | – |

On the frontend, the execution page offers Resume, Replay and Kill only (frontend/src/components/workflows/executions/actions.ts).

---

### 6. Pebble functions, filters and tests

The engine side was measured in `oracle/name-surface.txt`: 63 filters, 37 functions and 8 tests, from Kestra `Extension` plus Pebble core. Everything Kestra itself registers is ported (`runners/filters.rs` registers 35 filters; `runners/functions.rs`), except the items below.

**Functions, missing or refused**

- `render`, `renderOnce`: refused, "not yet". `{{ render(vars.x) }}` is very common in real flows (functions.rs:304-311).
- `errorLogs`: refused. It is common in `errors:` notification handlers (functions.rs:312).
- `subflow`: refused (functions.rs:313).
- `http`: deliberately refused (SSRF from the control plane) (functions.rs:237).
- `secret`: deliberately confined. **It resolves only on `kestra.*` plugin tasks.** Script tasks, Log, inputs, runIf and triggers all refuse it, because a rendered secret would sit in the pod spec (functions.rs:242-254). In practice every Kestra flow that does `env: {TOKEN: "{{ secret('X') }}"}` on a script fails.
- Pebble core `max`, `min`: missing. `i18n`: out.

**Filters, missing (all from Pebble core)**

`abbreviate`, `base64encode`, `base64decode`, `merge`, `numberformat`, `rsort`, `sha256`, `urlencode`, `raw`, `nl2br` (name-surface.txt:66-152). `escape` supports only the HTML strategy (`js`, `css` and `url_param` are refused).

**Tests, missing**

`null`, `empty`, `map`, `json`. `{{ x is null }}` is a Kestra idiom (especially `kv(..., errorOnMissing=false) is null`), and it fails with "unknown test" (name-surface.txt:215-240; W9c spec §15).

**Other divergences.** Chained comparisons evaluate eagerly. Names beginning `kestra_` are reserved. Exact-decimal arithmetic fails rather than going through BigDecimal (W9c §15).

---

### 7. Plugin ecosystem (`plugin-host/`)

| Kestra item | egret status | evidence |
|---|---|---|
| Interfaces run | `RunnableTask` only. Triggers (`AbstractTrigger`), flowables and dynamic-result tasks are refused | plugin-host/src/main/java/io/kestra/core/runners/EgretPluginHost.java:536-541 |
| Spelling | `type: kestra.<FQCN>`; the catalog is baked from `plugins.lock` | plugins/catalog.json |
| Jars | **3 jars, 9 classes**: jdbc-postgres (Query, Queries, Batch, CopyIn, CopyOut), jdbc-mysql (Query, Queries, Batch), jdbc-trino (Query), all 2.0.0 | plugin-host/plugins.lock |
| Governance | per-workspace feature `workflow_kestra_plugins` (off by default) plus `allowed-types` globs and the `workflow:use_secrets` permission; adding a jar is a reviewed PR | programme spec §11 Q3 |
| RunContext gaps (exit code 4) | `namespaceKv()`, `logFileURI()`, `inputAndOutput()`, `localPath()`, `encrypt`/`decrypt`, `acl()`, `sdk()`, `cloneForPlugin()` (nested task runners), `services()`, `getTaskProperty()`; namespace-file writes | ShimRunContext.java:296-395 |
| Rendering | `render*` is the identity: properties are rendered once before dispatch, so per-row or deferred rendering is wrong | ShimRunContext.java:43; programme spec §3.2 |
| Metrics | `metric()` writes a DEBUG log line only | ShimRunContext.java:45 |
| Storage | internal storage (STORE/FETCH) and read-only namespace files are bridged | W9a §9.1, W9b §10 |
| Cost and limits | 1.6-3.6 s of JVM start per attempt (so a short `timeout` never runs the plugin); outputs not masked; StarRocks queries outlive the task timeout | W5b spec §12 |

---

### 8. Deliberate divergences recorded in the specs

1. Containers and plugins sit behind workspace features (off by default) plus a global image/type allowlist. This is stricter than Kestra OSS (W5a, W5b).
2. The `Process` task runner is refused, and the Docker runner is refused by the chart in production (W5a §10-11).
3. `secret()` is only allowed in plugin tasks. `secret()` inside a KV `Set`/`Put` value is refused at publish (functions.rs:242; W9c §15).
4. `http()` is refused, to stop SSRF (starrocks-timeout-and-http spec §2).
5. A FILE input's `allowedFileExtensions` is enforced; Kestra does not enforce it (W9a §2).
6. KV, namespace-file writes, `PurgeKV`, `PurgeFiles` and `PurgeLogs` are confined to the flow's own namespace and its children. Kestra OSS has no check (W9b, W9c, core-tasks spec).
7. KV value size and density caps, and caps on numbers' digits (W9c §15).
8. A soft-deleted namespace file's history stays visible (W9b R1).
9. Flow-trigger windows cascade on listener delete, and an expired window is replaced rather than dropping the event (W13 spec:240,258).
10. Publish freezes references (a flow is rigid to renames) (programme spec §11 Q2).
11. Draft/publish split; no flow-level `pluginDefaults` (agrees with Kestra 2.0).
12. Kill on a terminal execution answers 409 in every case; a paused execution is killed without a worker (kill-and-exit D2, D3).
13. `Assert` emits no metrics; `PurgeLogs.purgeNonExecutionLogs` is a no-op (core-tasks D1, D2).
14. A Dag node that is disabled is dropped together with its edges; Kestra hangs in that case (plan.rs flatten_list doc).
15. Unknown and unsupported properties are refused by name on script and plugin tasks, but ignored on core tasks (scripts/mod.rs:194).
16. Log lines stay in Postgres with retention, not object storage (§11 Q1).

---

### 9. Missing that matters (ranked by impact on real-world flows)

| # | Gap | Why it matters | Size |
|---|---|---|---|
| 1 | **Flow-level `finally:` and `afterExecution:` do not run, and nothing tells the author.** Flowable-level `finally:` is also dropped on every flowable except AllowFailure | Cleanup and notifications are skipped with no warning, a silent-correctness hole of the same class the no-silent-stall work closed. At minimum this should be a refusal or warning today | M (planner lists plus state machine; warning first is S) |
| 2 | **`io.kestra.plugin.core.http.Request` (and Download)**, the most used core task after Log, plus the six unserved `egret.*`/`core.*` rows (`egret.sql` above all) | Most real flows call an API or run SQL. Today that needs a container script or the JDBC plugin host (off by default) | M (Request/Download as worker handlers with an egress policy); `egret.sql` M (needs a QueryExecPort wiring) |
| 3 | **`secret()` outside plugin tasks** (script env, Log, http) | Almost every production Kestra flow passes credentials to scripts this way. Today it is a hard refusal. Needs a design that injects secrets (env from a k8s Secret or a file mount) rather than rendering them into the pod spec | M-L |
| 4 | **Input validation at start**: `required`, type coercion errors, SELECT `values`, STRING `validator`, min/max, date bounds | Executions start with missing or invalid inputs and fail later, or worse, run wrong. Kestra rejects them up front | M |
| 5 | **Execution restart (retry failed tasks in place) and replay-from-task** (plus change-status) | This is the primary operator recovery action in Kestra. egret can only re-run the whole flow | M-L |
| 6 | **Flow `concurrency:` limits** (QUEUE/CANCEL/FAIL), still warned only | Very common on scheduled pipelines to prevent overlapping runs | M |
| 7 | **`render()`, `errorLogs()`, the `is null`/`empty` tests, `max`/`min`, and the base64/urlencode/merge/numberformat/sha256 filters** | Common expression idioms that break pasted flows at render time | S-M (each filter S; `render` M because of recursion bounds) |
| 8 | **Schedule `when:` conditions** (`isWeekend`, `isDayWeekInMonth`...) and **`stopAfter`** (silent today) | The Kestra 2.0 replacement for conditions; business-calendar schedules | S-M |
| 9 | **WorkingDirectory on Kubernetes** | Production runs on k8s, so WorkingDirectory is effectively unusable in production | M |
| 10 | **Webhook `wait`/`returnOutputs`** (synchronous webhooks) | API-style flows | M |
| 11 | **Python `dependencies:`/uv**, and more script languages (node, R...) | Kestra 2.0 python idiom; the workaround is `beforeCommands` | S-M each |
| 12 | Subflow `labels`, `inheritLabels`, `revision`, `scheduleDate`; Pause `onResume` inputs and resume-with-inputs; Loop `errors:` | Approval and parameterised-subflow patterns | S-M each |
| 13 | Execution delete, labels API, bulk ops, SSE follow, expression eval endpoint | Operability and UX; not engine correctness | M total |
| 14 | `checks:`, `sla` MAX_DURATION, `metric.Publish` / metrics store, `ScheduleOnDates`, `http.Trigger`, `PurgeExecutions`/`PurgeFlows`/`PurgeStorage`, `TemplatedTask`, `retry.warningOnRetry` (silent) | Lower frequency. `warningOnRetry` and `stopAfter` should at least warn | S each (metrics store L) |

**Quick wins (S, low risk).** Turn the silent drops into named refusals or warnings: flow-level and flowable `finally`/`afterExecution`, `stopAfter`, `warningOnRetry`, Subflow's unmodelled keys, and the input validator keys. That is the same rule the no-silent-stall package applied to tasks.
