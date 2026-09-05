# W3a execution oracle — the corpus and where it came from

`oracle-exec/` stands up a pinned Kestra, publishes a corpus of flows, executes
them, and records what happened. This document covers **the corpus and its
provenance**: which Kestra, which flows, how the input values were recovered,
and — the point of Task 1 — **how much of the corpus actually runs standalone**,
which is a measured result and not an assumption.

Task 2 added the rest and superseded nothing: **the normalised trace**, the
happens-before relation, `golden.json`, `stability.json` and the ratchet in
`crates/egret-workflow/tests/executor.rs`. Those sections start at *The
normalised trace*.

Task 3 is the first task to write executor code, and it changed four things this
file states: the circularity figure under *What the oracle feeds in* is now
measured rather than quoted (and the quoted one was impossible), `RunState`
gained `SUBMITTED`, the elapsed-time hole's second cause is closed, and
`timeout:` turns out not to be `advance()`'s to implement at all. It also added
*States and constructs no W3a task claims*, which names five things — `Pause`,
flow concurrency, `disabled:`, `ExecutionUpdatableTask` and **`Subflow`** — that
strand baselined entries with no owner. See *What Task 3 had to change, and what
it banked*.

Every number below is a number `harvest.py` prints. If this file and
`runnable.txt` ever disagree, `runnable.txt` is right and this file is stale —
it has been wrong that way once already.

## Requirements

Docker (measured on 28.4.0) and `python3`. No JVM, no Maven, no Kestra
checkout at run time — the flows are copied in.

## Running it

```bash
cd crates/egret-workflow/oracle-exec
python3 harvest.py 2>&1 | tee harvest-run.txt   # down -v, up, publish, execute, probe, write both outputs
python3 harvest.py --traces 5         # down -v, up, publish, execute every `runs` flow 5x -> traces.json
python3 harvest.py --verify-traces    # re-read every captured execution; requires the SAME Kestra still up
python3 normalise.py                  # traces.json + corpus.json -> golden.json + stability.json
python3 harvest.py --rewrite-runnable # regenerate runnable.txt offline from the committed files
python3 harvest.py --self-test        # mutate the committed data, prove every guard fires
python3 harvest.py --refresh-test-source-classes   # re-resolve test-source-classes.json
python3 normalise.py --self-test      # the same, for the normaliser's guards
python3 harvest.py --down             # docker compose down -v
```

`--traces` and the full harvest are two different measurements of the same
Kestra and are run separately: the harvest asks *which flows run at all* and
writes `corpus.json`, `--traces` asks *what exactly did they do, five times* and
writes `traces.json`. `normalise.py` never talks to Kestra, so a defect in the
normalisation costs a re-run of one second and not of an hour.

A full harvest takes roughly 35 minutes, most of it the seven flows that pause
and are waited out to their timeout, plus the discrimination probe's second
execution of every flow that has an alternative input.

**The run log is committed as `harvest-run.txt`, and the extension matters.**
It was previously written to `harvest.log` and was never actually committed:
a global `*.log` ignore swallowed it silently, so two rounds of review believed
a file was there that `git ls-files` says never was. `harvest-run.txt` is the
stdout of the harvest that produced the committed `corpus.json`, `probe.json`
and `runnable.txt`.

**Every flow is published before any flow is executed.** Publishing and
executing flow-by-flow makes the result depend on the alphabet: **ten** corpus
flows name another corpus flow that sorts later, so their `Subflow` target did
not exist yet when they ran. It cost **four** wrong terminal states, not one —
`valids/loop-with-subflow`, `valids/subflow-child`,
`valids/subflow-parent-retry` and
`valids/flow-concurrency-parallel-subflow-kill-child` were all recorded `FAILED`
where they in fact `SUCCESS`, and `loop-with-subflow` recorded one of its two
loop iterations because the failure stopped the loop.

That was found by measurement, not review: Task 2's capture publishes everything
first, so the two harvests disagreed on exactly those four flows.
`traces_agreement_problems()` now makes that disagreement a hard failure rather
than something a reader might notice, and with publish-first all **268** `runs`
flows agree with Task 2's five-round capture on terminal state.

**The clean start is structural, not documented.** The default invocation runs
`docker compose down -v` before `up -d`, and then `assert_clean()` refuses to
proceed if any *corpus* flow is already published. Re-publishing an existing
flow returns `422 Flow id already exists`, which would be filed `rejected`: a
full re-run against leftover state fails loudly, but a **partial** leftover
produces a plausible `corpus.json` with a quietly wrong denominator. "Empty"
cannot mean zero rows — the image seeds 9 flows of its own into the `tutorial`
namespace on first boot — so the assertion is the precise one.

## `docker-compose.yml` — exactly two edits

Copied verbatim from `kestra/docker-compose.yml` at the checkout below. There
are exactly two changes, and `diff` against the upstream file shows only these:

1. **`image: kestra/kestra:latest` → `kestra/kestra:v2.0.0-rc13`.**
   `latest` is a moving tag, and a moving tag is a moving golden. This is the
   same artifact the W2 oracle pins (`io.kestra:core:2.0.0-rc13`), for the same
   reason. Digest observed at the time of harvest:
   `sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`.

2. **basic-auth enabled** (the block upstream ships commented out, plus the
   `enabled: true` line it does not ship):

   ```yaml
         kestra:
           server:
             basic-auth:
               enabled: true
               username: admin@kestra.io
               password: Admin1234!
   ```

   Kestra 2.x refuses unauthenticated API calls outright — every call in
   `harvest.py` carries HTTP basic auth. Without this the harness cannot publish
   a single flow.

A **third** edit is allowed and has deliberately not been taken:
`kestra.encryption.secret-key`. It costs exactly one corpus entry — see *What
the two-edit rule costs*.

## The corpus

Copied from the sibling Kestra checkout:

| | |
|---|---|
| source | `kestra/core/src/test/resources/flows/{valids,invalids}/` |
| checkout | `96893b589a02e5d5d85c722855f76c08e79085af` |
| `valids/*.yaml` | 261 |
| `valids/*.yml` | 56 |
| `valids` total | **317** |
| `invalids/*.yaml` | 28 |
| total | **345** |

**Both extensions, and that was not always so.** An earlier revision of this
harness copied only `*.yaml` and this file argued for it. It was wrong, and the
cost was concentrated: eight of the 56 excluded files are the entire retry
behaviour matrix —
`retry-{failed,new-execution}-{task,flow}-{attempts,duration}.yml` — which is
Task 7's whole subject. Their names alone settle that `RetryBehavior` has two
variants, and the harvest settles their observable difference: the four
`retry-failed-*` flows terminate `FAILED`, the four `retry-new-execution-*`
terminate **`RETRIED`**. The extension split costs nothing except the belief
that it is only a file-naming detail; `read_corpus()` now reads both, and the
corpus id strips the extension so `valids/sleep` is one key whatever it is
spelled. There are no basename collisions between the two extensions, or
between `valids` and `invalids`, except `inputs`, which exists in both
directories and is why keys carry their directory.

These are Kestra's own test resources, not flows written for this exercise.
That is the whole point: a corpus written here would prove only that the flows
someone thought of agree.

**The artifact is authoritative for behaviour; the checkout is authoritative
only for reading an implementation.** They have drifted, and the harvest found
where — see `checkout-drift` below.

## The measured result

`runnable.txt` carries the counts and, for every flow that is not `runs`, the
specific reason. The headline, from `corpus.json`:

| disposition | total | valids | invalids |
|---|---|---|---|
| `runs` | 268 | 267 | 1 |
| `rejected` | 51 | 24 | 27 |
| `not-standalone` | 26 | 26 | 0 |
| `unclassified` | 0 | 0 | 0 |
| **TOTAL** | **345** | **317** | **28** |

**267 of the 317 `valids` run standalone.** That is the honest denominator for
every coverage claim the rest of W3a makes. It is not rounded up, and no flow
was dropped for being inconvenient: `unclassified` exists as a bucket precisely
so that a flow which fits nowhere shows up as a number rather than as an
omission, and it is empty because every one of the 345 was classified.

Following `parentId` recovered **142 sub-executions** that
`/executions/search` does not list — the same number Task 2's independent
five-round capture reports, and `check_outputs()` fails if the two ever disagree
on any flow's terminal state.

Every table in `runnable.txt` is derived from the data and carries a `TOTAL`
row, and the writer refuses to emit a table whose rows do not account for every
record. A hand-written bucket list once printed rows summing to 338 of 345 and
exited 0 — seven flows invisible in the file this package calls its honest
denominator.

### `runs` does not mean "correct"

`runs` means the flow published and reached a state Kestra's own
`State.Type.isTerminated()` calls terminal. It says nothing about whether the
trace is right; that is Task 2's job. And agreeing on 267 flows does not bound
behaviour on the 268th. The ratchet prevents regression; it does not prove
coverage.

## The four dispositions

| disposition | meaning |
|---|---|
| `runs` | published, and it and every sub-execution it spawned reached a terminal state |
| `rejected` | Kestra refused it — 4xx at publish, or 4xx at the execution POST for an `invalids` flow whose validation happens when its inputs are read |
| `not-standalone` | published, but cannot reach a terminal state without what the Java test brings: a resume, a trigger, an uploadable file, a mocked worker, or an input value that is computed rather than literal |
| `unclassified` | anything that fits none of the above. Its own bucket with its own reason, never a silent omission |

`KNOWN_DISPOSITIONS` is the whole vocabulary, and a record carrying anything
else is a hard failure rather than a row nobody prints.

### Terminal states

`harvest.py` uses Kestra's own `State.Type.isTerminated()`, read from
`core/src/main/java/io/kestra/core/models/flows/State.java` at the checkout:

```
FAILED WARNING SUCCESS KILLED CANCELLED RETRIED SKIPPED RESUBMITTED
```

The plan's shorter list (`SUCCESS`/`FAILED`/`WARNING`/`KILLED`) is a subset, and
using it would have cost five flows for two different reasons:
`exit-cancelled.yaml` terminates in `CANCELLED`, and all four
`retry-new-execution-*` flows terminate in `RETRIED`. Each would have been
polled to its timeout and filed `not-standalone`.

### `rejected` is four different things wearing one status code

`runnable.txt` splits it, and generates the "what may Task 8 use" prose from
`TASK8_EVIDENCE` rather than beside it:

| class | count | Task 8 evidence? | what it is |
|---|---|---|---|
| `invalid-flow` | 27 | **yes** | an `invalids` flow, refused. |
| `refused-by-both` | 1 | **yes** | a `valids` flow the *checkout* refuses too, with the checkout line that proves it. |
| `task-type-not-in-image` | 22 | no | the body's only violation is `Invalid type: X`, and `X` is in the checkout-resolved test-source set. **This does not say the flow is valid** — see the short-circuit below. |
| `checkout-drift` | 1 | no | curated: the checkout has a feature the pinned artifact does not. |
| `unclassified-rejection` | 0 | no | **unknown** — not shown to be a refusal and not shown to be drift. Read the raw message. |

Two properties this table has to have, and once did not:

- **Nothing lands in a lenient class by default.** `checkout-drift` used to be
  the fallback branch: anything whose message lacked `Invalid type:` arrived
  there with no evidence that the flow was valid anywhere. That is the one
  bucket Task 8 is told to ignore, so a mis-file into it is invisible by
  construction. Both lenient classes are now curated tables carrying a checkout
  line each, and everything unrecognised goes to `unclassified-rejection` — an
  open question, not a dismissal.
- **Membership of a resolved set, not a naming heuristic.** An earlier revision
  accepted any FQCN matching `(^|\.)test\.|Test\$`. Measured: publishing a flow
  whose task type is `com.acme.totally.made.up.test.NeverExisted` returns exactly
  the same `Invalid type:` body, so an invented class filed into the bucket Task
  8 ignores. `test-source-classes.json` now holds the **44** plugin types
  (Task/Trigger subclasses and their `aliases`) declared under the checkout's
  `src/test/java`, each with the file that declares it;
  `harvest.py --refresh-test-source-classes` regenerates it and prints the diff.
  The set is committed so a harvest still needs no checkout.
  `io.kestra.core.runners.test.trigger.Alias` is in it only because it is an
  `aliases =` declaration on
  `core/src/test/java/io/kestra/core/runners/test/TriggerWithAlias.java`.

#### rc13 short-circuits on an unknown type, so this class claims less than its name

This is the correction that matters most in this section, because the previous
revision of this file stated the opposite as measured fact ("The flow is fine;
the image cannot run it") and the classifier carried a condition that could not
fail.

Measured directly against the running artifact — each co-violation is a hard 422
on its own, and each disappears entirely behind an unknown type:

| flow | body |
|---|---|
| unknown type alone | `Invalid type: io.kestra.core.tasks.test.Encrypted` |
| unknown type + `namespace: BADNS_UPPER` | `Invalid type: …Encrypted` |
| `namespace: BADNS_UPPER` alone | `namespace: must match "^[a-z0-9][a-z0-9._-]*"` |
| unknown type + empty `Parallel.tasks` | `Invalid type: …Encrypted` |
| unknown type + blank task id | `Invalid type: …Encrypted` |
| two unknown types | `Invalid type: …Encrypted` (only the first) |

`POST /flows/validate` behaves identically, so there is no second opinion to
ask. Consequences:

- Requiring `Invalid type:` to be the **only** violation proves nothing about
  co-violations: the body can never contain a second line when the first is
  `Invalid type:`. The condition is kept as a shape check on the message and is
  no longer described as more than that.
- `task-type-not-in-image` therefore means **"the first violation is a missing
  test-source class"**. It is not evidence that egret must refuse the flow, and
  it is equally not evidence that the flow is otherwise valid. Anything that
  needs the latter has to establish it some other way.

The two curated entries, both worth knowing before writing "Kestra does" about
anything:

- `valids/flow-trigger-fire-once-true-flow-listen` — `checkout-drift`.
  `Unrecognized field "fireOnce" (class io.kestra.core.models.triggers.Window)`.
  The checkout's `Window.java:79` declares
  `@Builder.Default @PluginProperty @NotNull private boolean fireOnce = false;`.
  The pinned artifact does not.
- `valids/invalid-draft-flow` — `refused-by-both`, and therefore **evidence for
  Task 8**. `draft: true` in the YAML body is not what makes a flow a draft:
  `FlowController.java:350-351` reads `draft` from a query parameter
  (`@QueryValue(defaultValue = "false") boolean draft`), so a POST without it is
  `draft=false` and the `@NotEmpty` on `Flow.java:71` `tasks` applies in the
  checkout exactly as in rc13. Kestra's own
  `ExecutionControllerRunnerTest.java:209-227` calls the file "a draft saved
  with constraint violations (empty `tasks` violates @NotEmpty)" and asserts
  422. This entry was in the lenient bucket for one revision, which is the
  concrete reason that bucket now needs evidence.

### Drift at the execution seam, not at publish

`CHECKOUT_DRIFT` only ever holds flows rc13 **refuses**, so a flow that
publishes, runs, and behaves differently from the checkout's own passing test
had nowhere to be recorded. `EXECUTION_DRIFT` is that bucket, and it currently
holds one entry.

`valids/pause-behavior` writes `behavior: "{{inputs.behavior}}"` and, harvested,
gives the identical execution for every documented input value — `FAILED`, one
task run. That looked like "Pause's `behavior` has no effect on rc13". It is
not. Publishing four copies of the same flow with a **literal** behavior and
running each measures the opposite:

| literal `behavior` | state | task runs | `PauseTest.java:374-380` expects |
|---|---|---|---|
| `RESUME` | SUCCESS | 2 | SUCCESS, 2 |
| `WARN` | WARNING | 2 | WARNING, 2 |
| `FAIL` | FAILED | 1 | FAILED, 1 |
| `CANCEL` | CANCELLED | 1 | CANCELLED, 1 |

Every literal value behaves exactly as the checkout's test asserts. What fails
is **rendering a Pebble expression into `Property<Behavior>`**, silently, at the
moment the pause ends. So the drift is in dynamic-property resolution, not in
`Pause` semantics — Task 7 must not read this flow as evidence about pause
behaviour, and Task 2 must not build its golden from the checkout's semantics.

### One `invalids` flow that this Kestra ACCEPTS

`invalids/loop-switch-failed` publishes with 200, runs, and reaches `FAILED`
with 2 sub-executions.

It is not an invalid flow at all. Kestra's own
`RunContextTest.failedTasksVariable` executes it with
`@ExecuteFlow("flows/invalids/loop-switch-failed.yaml")` and asserts exactly
that: `FAILED`, one task run, two loop sub-executions. It lives under
`invalids/` because the *execution* is expected to fail, not because the flow is
rejected.

So "the 28 `invalids` are flows Kestra refuses" is **27 of 28**, and that flow
carries `"accepted_though_invalid": true` in `corpus.json` so Task 8 cannot pick
it up by directory name.

### One `invalids` flow refused at a different seam

`invalids/inputs-with-multiple-constraint-violations` publishes with 200 and is
refused when its inputs are read — `MULTISELECT` values outside the declared
set. Publish-time and execution-time validation are two different gates. It is
recorded as `rejected` because Kestra does refuse it; the reason string names
both statuses so the seam is visible.

## The 57 flows whose inputs live in Java, not in YAML

**57 of the 317 `valids` declare `inputs:`** — 53 `.yaml` and 4 `.yml`. The YAML
declares that an input exists; the *value* lives in the Java test beside it.
`ParallelTest.errors()` drives `finally-parallel.yaml` with
`Map.of("failed", true)` — and running that flow without the input does not fail
loudly. It runs the other branch and produces a golden that looks fine and tests
nothing.

Every one of the 57 was resolved by reading the Java test that references it.
The table lives in `harvest.py` (`INPUTS`, `NOT_STANDALONE_INPUTS`) and every
entry carries the file, line and method it came from. **Nothing was invented.**

| | count |
|---|---|
| value recovered from a Java test | 39 |
| `not-standalone`, value not recoverable | 18 |
| total `valids` declaring `inputs:` | **57** |

`INPUTS` also carries one `invalids` entry
(`inputs-with-multiple-constraint-violations`), so the table has 40 rows.

`validate_tables()` fails the run if any table key names no corpus flow, if a
flow declares inputs and appears in neither table, or if a flow appears in both.

### The wire format, measured

Kestra 2.x accepts execution inputs **only** as `multipart/form-data`. Measured
against the running artifact:

```
POST /api/v1/main/executions/{ns}/{id}  Content-Type: application/json
  -> 415 Content Type [application/json] not allowed.
     Allowed types: [multipart/form-data]
```

Scalars go over the wire as their string form (`true`, `42`); `JSON` and `ARRAY`
inputs go as compact JSON text. A `BOOL` input sent as the form field
`failed=true` comes back on the execution document as `"inputs": {"failed": true}`
— checked before the harvester was written around it.

### Which value, when a flow is driven with several

Twenty-one flows in the table are driven with more than one input map (twenty
reach `corpus.json` with the alternatives attached; `working-directory` is
rejected at publish before its inputs are ever used). The entry records one as
`inputs`/`inputs_from` and **all the others under `inputs_alternatives`**, each
with its own provenance, so nothing measured is discarded. The primary is the
map that drives the flow's failure or error branch where there is one, because
the error path exercises `errors:` and `finally:` handling that the success path
does not. `if-condition` alone carries five alternatives — `true`, `"true"`,
`1`, `false`, `"false"`, `0` — which is Kestra's own truthiness matrix.

### The cross-check, and what it can and cannot catch

A wrong input value does not raise; it runs the other branch. So each recovered
input has an `EXPECTED` row carrying **whatever the same Java test asserts**:

| field | from |
|---|---|
| `state` | `execution.getState().getCurrent()` |
| `taskruns` | `assertThat(execution.getTaskRunList()).hasSize(N)` |
| `task_at` | `getTaskRunList().get(i).getTaskId()` |
| `task_states` | `findTaskRunsByTaskId(x).getFirst().getState().getCurrent()` |

A state alone is a weak witness: several of these flows end `SUCCESS` on either
branch, so a state-only check on them passes with or without the right input.
The row shapes are validated too — `"taskrun": 999`, a typo of `taskruns`, was
once silently ignored while the row still printed `agrees` and still labelled
itself `[state,taskruns]`, which is the same invisible-by-construction shape as
a bucket that collects by default.

Current result, from `runnable.txt`:

```
agrees 37   differs 0   not exercised 2   (of 39 rows)
```

The two not exercised are `valids/working-directory`, whose
`WorkingDirectoryTest$LocalFiles` task type is not in the image, and
`valids/input-log-secret`, which needs the third compose edit.

### The discrimination probe — which rows can actually report a difference

`agrees 37 / differs 0` is a weaker claim than it reads, so the harness measures
the strength rather than asserting it. For every row whose flow has a
**documented alternative** input, `probe_discrimination()` executes the flow
again with that alternative and requires the row to **fail**. Every alternative
is tried, not just the first: `if-condition`'s first alternative is
`{"param": "true"}`, the same truthy branch as `{"param": true}`, and a probe
that stopped there would have called the row unable to discriminate when the
corpus in fact contains `{"param": false}`.

```
proven discriminating 17   proven non-discriminating 1   unprobed 19
```

- **17** rows are proven falsifiable by measurement.
- **1** is proven *not* falsifiable, and finding it is the reason the probe
  exists: `valids/pause-behavior`. On v2.0.0-rc13 all four documented behaviours
  — `FAIL`, `RESUME`, `WARN`, `CANCEL` — produce the identical execution
  (`FAILED`, 1 task run, histories
  `CREATED/RUNNING/PAUSED/RESTARTED/RUNNING/FAILED`).

  **The cause is not that `behavior` is unimplemented on this artifact**, and an
  earlier revision of this file said so, which would have sent whoever
  implements `pause` hunting for a feature that is already there. Published with
  **literal** values on a fresh `rc13`, the same flow gives `RESUME → SUCCESS`
  with 2 task runs, `WARN → WARNING` with 2, `FAIL → FAILED` with 1, `CANCEL →
  CANCELLED` with 1, and the property omitted → `SUCCESS` with 2 — the
  checkout's semantics reproduced exactly, `PauseTest.java:374-380` included.

  What fails is narrower. The corpus flow writes
  `behavior: "{{ inputs.behavior }}"`, and **rendering a Pebble expression into
  a `Property<Behavior>` fails at resume time and takes the task down with it**,
  for every value including the one that would have failed anyway. The execution
  document does record `inputs: {"behavior": "FAIL"}`, so the input landed; the
  task dies rendering it.

  So that row's `agrees` is a coincidence in the **strong** sense — the flow
  never reaches the `FAIL` behaviour at all — and Task 2 builds this flow's
  golden from what the pinned image did.
- **19** are unprobed because no alternative input is documented for the flow —
  there is nothing to swap in. Seventeen of them carry a `taskruns`, `task_at`
  or `task_states` assertion that a branch flip would move, so they are strong;
  they are simply not *proven*.

One row is labelled **WEAK** in `runnable.txt`, and the label is derived rather
than remembered: a row whose only assertion is a non-error terminal state and
which the probe never proved can move.
`valids/subflow-nullable-input-parent` is the one — it would agree with any
input that merely finishes successfully. Do not cite it.

Two recovered inputs have no checkable execution-level assertion at all and are
listed rather than omitted: `valids/subflow-function-child`, whose test asserts
the string the `subflow()` Pebble function returns, and
`invalids/inputs-with-multiple-constraint-violations`, whose test asserts that
reading the inputs throws.

### The 18 that could not be recovered, by cause

Each is listed individually with its full reason in `runnable.txt`; the causes
group as:

- **The value is computed by the JVM, not literal** (4) — a classpath resource
  path (`inputs`, `inputs-small-files`, `restart_with_inputs`), a 16 KiB
  generated string (`inputs-large`), or a Java `Map` object. Guessing one
  produces a golden for a flow nobody tests.
- **The flow is never executed by any test** (2) — `secret-input-validation` is
  only handed to `readExecutionInputs` to assert it throws;
  `scheduled-inputs-defaults` is only `parse()`d.
- **Created only by a trigger or as a subflow** (5) — `trigger-flow-listener`,
  `trigger-flow-listener-namespace-condition`,
  `flow-trigger-invalid-inputs-flow-listen`, `subflow-to-retry`,
  `subflow-nullable-input-child`. Their inputs render from trigger variables
  that do not exist on a direct execution.
- **Driven only through the `subflow()` Pebble function, and asserted to throw**
  (3) — `subflow-function-recursive` and the `subflow-function-mutual-a/b` pair.
- **Reached only through the webhook endpoint** (1) — `webhook-inputs`.
- **No Java test drives it at all** (2) — `kv`, whose two inputs both have YAML
  defaults, so a bare execution would silently take them: exactly the
  looks-fine-tests-nothing golden this table exists to prevent; and
  `trigger-flow-listener-invalid`, which nothing in the checkout references —
  no Java test, no other flow. Both are left as measured gaps rather than
  counted as runnable.
- **The Java test bypasses typed inputs** (1) — `task-flow-dynamic`.
  `FlowGraphTest.java:295` hands `runOne` a raw map
  (`{namespace: …, flowId: "switch"}`) instead of going through
  `flowIO.readExecutionInputs`, so it never supplies the flow's one *declared*
  input, the required `STRING string`. Over the API that is
  `422 Missing required input:string`, and no value for `string` exists anywhere
  in the checkout to recover.

## What the two-edit rule costs

`valids/input-log-secret` declares two `SECRET` inputs. Its execution POST
returns:

```
422  Unable to use a `SECRET` input/output as encryption is not configured
```

Kestra's own `core/src/test/resources/application-test.yml:21` sets
`kestra.encryption.secret-key`. Our compose file does not. This is a **known,
priced cost of exactly one corpus entry**, not an accident: a task that wants it
back should add the key as an explicit third edit and amend the compose section
above.

## Two traps this harness is built around

### The readiness probe

A wrong path returns **405** and an unauthenticated call returns **401**, and
both look exactly like "still starting". `wait_ready()` probes the exact
authenticated URL and accepts **only 200**. Measured on the running container:

| request | status |
|---|---|
| `GET /api/v1/main/flows/search?size=1` with basic auth | 200 |
| the same URL with no auth | 401 |
| `GET /api/v1/main/flows` with basic auth (wrong path) | 405 |

Kestra becomes ready roughly 15–30 s after `docker compose up -d`.

### Sub-executions are hidden from `/executions/search`

A sub-execution is fetchable by id and carries `parentId`, but does **not**
appear in the search listing. A harvester that enumerates executions silently
loses every loop iteration and every subflow.

`descendants()` therefore unions two independent discoveries and trusts neither
alone:

1. the parent's log lines, which carry `in sub-execution [[link execution="…"]]`
   — only present for the tasks that log it;
2. a recursive walk of Kestra's Postgres over the generated `parent_id` column,
   which is the storage layer rather than the API.

The SQL goes over psql's **stdin**, not `-c`, and the id is a psql variable
(`:'root'`): psql does not interpolate variables into a `-c` string, so the
parameterisation would silently not happen. A non-zero psql exit is fatal — a
swallowed error would degrade sub-execution recovery to the log-scrape arm
alone, silently, and the harvest would still write a plausible `corpus.json`.

### Terminal is not the same as finished

Discovering the descendants, waiting for the pending ones, and stopping when
none are pending is the obvious loop, and it is wrong: it stops the instant the
first discovery returns nothing, so a row that lands a beat later is simply
absent — silently, with a plausible count written. That is measured, not feared.
It cost **nine** sub-executions: two each for the four `retry-new-execution-*`
flows (Task 7's entire subject, which recorded **zero**), and one of
`valids/loop-with-subflow`'s two loop iterations.

`RETRIED` is Kestra saying a replacement execution is coming. Over 40
observations (four flows × five rounds × two children) every `RETRIED` execution
had exactly one child and every other terminal state had none, and the
replacement's `startDate` ran **1.07 s to 5.92 s** after the `RETRIED`
execution's `endDate`. So the rule is exact rather than a sleep:

> settle when nothing is pending **and** no `RETRIED` execution is childless.

`settle()` is the single implementation, used by both the Task 1 harvest and
Task 2's capture. Two harvests with two rules is how `corpus.json` came to say
133 sub-executions while the verified capture said 142.

A **third** arm is a quiescence condition, and it is labelled as one because no
exact signal exists for it. A serial `Loop` creates iteration k+1 after k
finishes, and nothing the API returns says how many iterations there will be:
the parent's taskrun carries no count, and `loopRun` carries only `index`.
Measured, the race is real but narrow — `valids/loop-with-subflow` recorded 1 of
its 2 iterations once, under the load of a 345-flow harvest, while 20 quiet
repeats had the second iteration visible **0.26 s to 0.62 s before** everything
looked terminal. So `settle()` also requires the `(id, state)` snapshot to be
observed unchanged across two consecutive polls. That bounds the race at one
poll interval; it does not end it, and nothing here claims otherwise.

**Siblings are ordered by `loopRun.index`, never by creation instant.** A
concurrent loop creates its sub-executions in an order that flipped between
rounds on nine corpus flows — a fact about the scheduler, not about the flow.

## Reproducing the summary without a Kestra

`runnable.txt` is a pure function of `corpus.json` and `probe.json`, both
committed, so `--rewrite-runnable` regenerates it offline. Without that, fixing
a defect in the summary would cost a 35-minute re-harvest — which is pressure to
hand-edit the one file whose worth is that nobody edits it.

That path is guarded, because it once cheerfully reproduced the very defect it
exists to fix: with the 56 `.yml` entries deleted from `corpus.json` it
regenerated the pre-fix header verbatim (`total flows 289 / runs 218`) and
exited 0, since it never reads `corpus/`. `load_committed()` now fails unless

- `corpus.json`'s id set equals `read_corpus()`'s, and
- every row that ran and has an `EXPECTED` entry has a `probe.json` record
  (dropping five silently degraded those rows to `unprobed`), and
- every stored `rejected_class` still matches what the current classifier
  produces (otherwise `corpus.json` is stale in a way no count would reveal),
  and
- `validate_tables()` and `check_outputs()` both pass — the path that actually
  writes the published file used to run neither.

Guards `raise` and never `assert`: `python3 -O` strips assert statements, so a
guard written as an assert is a guard that disappears under a flag anyone may
pass.

### What a guard has to be able to do

Removing the `assert` keyword is not the same as making a check able to fail,
and this file said the finding was closed when only the keyword had gone. The
replacement condition — `if sum(by_class.values()) != len(rej)` — was still
vacuous: `by_class` is a `Counter` built by iterating `rej` exactly once, so
both sides are the same number by construction. It is gone. Two properties every
guard here now has:

- **It compares against something derived independently.** The class table is
  re-derived by running `rejected_class()` again over each record's own
  directory and message, which a corrupted stored class contradicts. `differs`
  in `probe.json` is recomputed from the alternative's recorded observation
  (`alt_observed`) and the `EXPECTED` row, because it used to be a raw copied
  boolean: flipping every one of them true → false took **`proven
  discriminating 17` → `0`** at exit 0, and that sentence is the most
  load-bearing one in the package.
- **Where a second, independent measurement exists, it is checked.**
  `traces_agreement_problems()` compares every `runs` record's terminal state
  against Task 2's five-round capture in `traces.json` and fails on any
  disagreement. It is what turned "the two harvests differ by nine
  sub-executions" into "these four flows have the wrong terminal state, and here
  is why".
- **A corruption contradicts a field it did not think to change.**
  `record_invariant_problems()` holds the invariants that make that true, all of
  which hold on every record today: `runs ⇒ publish_status == 200`,
  `runs ⇒ an `observed` whose state is terminal and whose `taskruns` equals its
  `task_ids` length`, `non-runs ⇒ no `observed``, `rejected ⇒ 4xx evidence`,
  `non-rejected ⇒ no stale `rejected_class``, and `sub_executions` must equal
  the number of distinct `sub_execution_ids` recorded beside it.

Five corruptions of `corpus.json` used to move a published number at exit 0:
flipping a `not-standalone` to `runs`, flipping a `rejected` to `runs` with its
class dropped (which also slipped past the class re-derivation, since that only
inspects records still *saying* rejected), flipping every probe `differs`,
tripling `sub_executions`, and corrupting an `observed` state. Each now fails.

`--self-test` mutates the committed data **21** ways and requires every guard to
fire **and to name its own reason** — an earlier version accepted any
`SystemExit`, so a case labelled for guard X passed when guard Y fired, proven
by deleting the vocabulary guard and watching case 1 still print `FIRES`. Every
case runs through `check_outputs()`, which writes nothing: the previous version
drove its first cases through `write_runnable`, so the run that reports a
missing guard left `runnable.txt` corrupted — `TOTAL 344`, one flow absent —
and then exited 1, ready for the next `git add -A`. It needs no Kestra and takes
about a second.

## The normalised trace

`golden.json` holds one entry per **comparable** corpus id, written by
`normalise.py` from `traces.json`. The shape:

`valids/parallel`, abridged to its first two of eight runs and first two of 28
order pairs — the rest is more of the same:

```json
{
  "id": "valids/parallel",
  "verdict": "runs",
  "execution": ["CREATED", "RUNNING", "SUCCESS"],
  "runs": [
    {"task": "parent", "value": null, "parent": null,
     "state": ["CREATED", "RUNNING", "SUCCESS"],
     "attempts": [["CREATED", "RUNNING", "SUCCESS"]]},
    {"task": "t1", "value": null, "parent": 0,
     "state": ["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"],
     "attempts": [["CREATED", "RUNNING", "SUCCESS"]]}
  ],
  "order": [[0, 1, "overlapping"], [0, 2, "overlapping"]],
  "spawned": []
}
```

A `refused` entry carries no trace at all — `{"id", "verdict": "refused",
"rejected_class"}` — because the only thing compared for it is that egret's plan
build **fails**.

The rules, each traceable to a measurement:

1. **Every generated id is replaced by its ordinal in first-appearance order**,
   and `parent` is that ordinal rather than a ksuid. `normalise.py` numbers by
   index into `taskRunList` and **checks** that every `parentTaskRunId` resolves
   to an *earlier* index — if it did not, ordinal-by-index and
   ordinal-by-first-appearance would be two different numberings, and the golden
   would silently be using whichever one the script happened to implement.

2. **Every clock is dropped — but the happens-before relation is derived
   first.** `startDate`, `endDate`, `duration`, every `histories[].date` and
   `metadata.originalCreatedDate` are gone from the golden. Durations are
   dropped, not bucketed: a duration bucket is a timing assertion wearing a
   correctness costume. See *The happens-before relation*.

3. **`runs` keeps Kestra's `taskRunList` order**, which is creation order.

4. **`histories[].state` sequences are kept verbatim.** See *The state
   vocabulary, measured* for what that turns out to include.

5. **`spawned` holds sub-execution traces of the same shape**, each carrying
   the `parent_run` ordinal that spawned it and the `value` it was spawned with,
   **ordered by parent run and then by `loopRun.index`**. See *What the
   sub-executions turned out to carry*.

### What the sub-executions turned out to carry

134 of the 142 sub-executions this capture recovered are
`kind: "LOOP"`, and each carries a `loopRun` object naming all three things the
trace needs:

```json
{"taskId": "1_each", "taskRunId": "2uNX…", "index": 0, "value": "value 1"}
```

Three measurements came out of reading it, and each of the three changed the
normaliser.

- **Order by `index`, not by when it was created.** The first pass ordered
  siblings by creation instant, and **nine** flows then failed the five-round
  stability check with the *same* sub-executions in a *different* sequence —
  `loop-object`, `loop-object-in-list`, `if-in-parallel`,
  `switch-in-concurrent-loop` among them. A concurrent loop creates its
  iterations in whatever order the scheduler picks; that is a fact about the
  scheduler, not about the flow. `index` is the flow's own numbering and is what
  `Command::SpawnExecution` will be judged on. Ksuids are not a tiebreak either:
  a ksuid orders at one-second resolution, so siblings created in the same
  second sort at random.

- **`TaskRun.value` is null everywhere in this corpus** — all 840 runs in
  `golden.json`, in all five rounds. The plan's normalised shape has a `value`
  on each run
  "for a loop iteration's value", and on this artifact that is not where it
  lives: the iteration value is on the sub-execution's `loopRun.value`. The
  field is kept on runs, always null, and carried on `spawned` where the value
  actually is.

- **`parentTaskRunId` can cross the execution boundary.** Measured on
  `invalids/loop-switch-failed`: the LOOP sub-execution's top-level taskruns
  (`switch`, `errorforeach`) name the *root* execution's `foreach` taskrun as
  their parent. Inside their own execution they have no parent, and the link
  they express is the one `parent_run` already carries — so an id equal to this
  execution's own spawn link normalises to `null`. The exemption is exactly one
  id wide: anything else is still a hard failure, because "not found means null"
  would silently swallow a real broken link.

The spawn link itself is read from `loopRun.taskRunId` and **cross-checked**
against `loopRun.parent.taskRunList`'s last entry, which says the same thing;
the two disagreeing is a hard failure. Reading a link off the wrong field would
put sub-executions under the wrong run, and once the ids are gone that reads
exactly like a measurement.

The other 8 have no `loopRun` at all. They are what
`RetryBehavior: CREATE_NEW_EXECUTION` produces — a **replacement execution** for
the whole flow, not a task spawning a child — so they get `parent_run: null` and
sort by creation instant. `valids/retry-new-execution-task-attempts` reads:

```
root       CREATED RUNNING FAILED RETRIED
 └ child   CREATED RUNNING FAILED RETRIED
    └ …    CREATED RUNNING FAILED
```

### 142, and how `corpus.json` came to agree

The Task 1 harvest counted **133** sub-executions; this capture counted
**142** in every one of the five rounds. `corpus.json` now says 142 too: Task
1's harvest was re-run through the shared `settle()` and, like this capture,
now publishes every flow before executing any. The two numbers are one
measurement rather than two, and `check_outputs()` fails if they ever drift
apart. The nine that the first capture and Task 1's original both missed:

- **8** are the `retry-new-execution-*` flows' replacement executions, two each.
  All four of those flows had **zero** sub-executions recorded, and they are
  Task 7's entire subject.
- **1** is `valids/loop-with-subflow`, whose `Loop` has two values and therefore
  two iterations; the earlier captures recorded one.

The cause is structural, and it is worth writing down because the obvious loop
has it: the sub-execution poll stops the moment nothing is *pending*, so it
stops immediately when the first `descendants()` call returns nothing, and a row
that lands a beat later is simply absent — silently, with a plausible trace
written. **Terminal is not the same as finished.**

For the retry case the signal turns out to be exact. `RETRIED` is Kestra saying
a replacement execution is coming, and over 40 observations (four flows × five
rounds × two children) **every** `RETRIED` execution had exactly one child while
every other terminal state had none; the replacement's `startDate` ran 1.07 s to
5.92 s after the `RETRIED` execution's `endDate`. So the capture settles on a
rule rather than a sleep: nothing pending **and** no childless `RETRIED`.

`harvest.py --verify-traces` is what found all of this, and it is why
`traces.json` can be trusted rather than merely believed. With the same Kestra
still running it fetches **every** captured execution again — long after the
flows finished — and requires each to reduce **identically** and its descendant
set to be unchanged. Anything that was going to appear has appeared by then. It
is a separate command, not part of the capture, because a check that runs inside
the thing it checks shares its clock.

Current result: **2050 captured executions re-read identically, and every
descendant set unchanged.**

### What the oracle feeds in, what it costs, and where it IS circular

`advance()` does not decide whether a `Log` task succeeds — Kestra's executor
does not either; its *worker* does. So `tests/executor.rs` supplies exactly one
thing from the golden, and this section says precisely what, because an earlier
version of it overclaimed.

**Fed:** the last element of the `state` sequence of a run whose `PlanTask` is
`Runnable`, and of each of that run's attempts. Per attempt, not per run — four
runs in the golden have attempts that end differently (`retry-success`'s
`failed`: FAILED, FAILED, FAILED, SUCCESS) and a per-run outcome makes them
unreproducible.

**Not fed, and therefore compared:** which runs are created, in what order,
under which parent, with what `value`; every state in a run's history *except*
its last; the whole of a **flowable's** history including its last; the
execution's own history; the happens-before relation; and the sub-executions.
Feeding a flowable's state would be circular in the way that matters — a
`Sequential`, `Parallel`, `Dag`, `If`, `Switch` or `Loop` run's state is the
executor's decision and is exactly what this oracle exists to judge.

**The oracle is non-circular with respect to `advance()` — which is what it is
for — and circular with respect to `golden.json` for that one field.**

Demonstrating that needs an experiment that could have come out the other way,
and the first one here could not. It rewrote `valids/minimal2`'s terminal to
`KILLED` and observed the ratchet stay green — but `minimal2` cannot match under
*any* value, because its golden run history carries `SUBMITTED`, which
`RunState` cannot represent. The ratchet reported the same 294 mismatches before
and after. **A demonstration that survives deleting the mechanism it
demonstrates is not evidence**, and gutting `golden_outcome` to return `Failed`
unconditionally does leave `executor_matches_golden` green — while `advance()`
is a stub, nothing matches either way.

The discriminating version, on `valids/labels-update-task-deduplicate`, whose
golden uses only states `RunState` can hold. With a state machine that
reproduces it exactly:

| | golden | `golden_outcome` | the entry |
|---|---|---|---|
| **A** | as committed | as written | **matches** — `PROGRESS NOT BANKED … valids/labels-update-task-deduplicate` |
| **B** | run terminal **and** attempt terminal rewritten `SUCCESS → KILLED` | as written | **still matches** — the lie is absorbed on both sides |
| **C** | still `KILLED` | gutted to always `Failed` | **stops matching** |
| **D** | as committed | gutted to always `Failed` | **stops matching** |

**A → B is the circularity**: an outcome Kestra never produced for that flow was
written into the golden and the entry went on agreeing. **C and D are how we
know the experiment discriminates**: the mechanism under test is load-bearing
for this entry in both directions, so B's result is a fact about the feed and
not an artefact of an entry that could never match. The old `minimal2`
experiment has no C or D.

So a normalisation bug or a capture anomaly *in that one field* is invisible in
both directions.

**How much of the golden that covers — measured, Task 3.** The figure that stood
here was quoted rather than measured: the review's own plan builder put it at
**812 of the golden's 840 run terminals**, and this file said Task 3 should
replace the paragraph once `build_plan` could answer it. It can, and the answer
is not 812. **812 was not merely wrong, it was impossible**, and against this
file's own bracket: 120 of the 840 are the parent of another run and are
therefore certainly flowables whose terminals are never fed, so no split can put
the fed count above 720.

`tests/executor.rs::the_feed_absorbs_a_measured_share_of_the_golden_run_terminals`
derives it on every run, from `build_plan` and `golden.json`, over the golden's
run terminals **including the sub-executions'** — counting only the top-level
`runs` lists gives 570 and reads just as much like a measurement. Every terminal
is in exactly one bucket:

| bucket | count | what it is |
|---|---|---|
| **fed** | 390 | the run's task is a `Runnable` in the plan, so its terminal is supplied and is not compared |
| **flowable** | 23 | the run's task is a `TaskKind::Sequential`, whose state is the executor's own decision and is compared |
| **unplanned** | 427 | the run's task has no `PlanTask` at all — it lives inside a flowable Task 3 plans opaque and so does not descend into |
| **TOTAL** | **840** | |

Those three move as Tasks 4-8 teach `build_plan` more flowables, which is why
they are asserted in the test rather than only printed here: a task that changes
the split has to change this table in the same commit.

**The end state is 580, and it is a lower bound rather than the exact figure.**
`SUBMITTED` is Kestra handing a task to a *worker*, so a plan builder that knew
every flowable would feed at least the 580 run terminals whose golden history
carries it and compare at most the other 260 — independent of how complete
`build_plan` happens to be. It is a bound and not an equality because the
converse fails, measured: **2 runs of `io.kestra.plugin.core.log.Log`, an
unambiguous `RunnableTask`, carry no `SUBMITTED`** — Kestra never submitted them,
because `runIf` `SKIPPED` them or they failed before submission. The set is
asserted at exactly those 2, so if the exception grows this paragraph has to
weaken with it.

**Task 3's own over-feed is 135 of the 390, and it is not a single accident.**
The by-type breakdown is derived from the plan's own `type_id` and printed on
every run — **15 task types**, of which 11 are flowables Tasks 4-6 own (114
runs — but see the section below: `Subflow`'s 18 of those are claimed by nobody
either), 3 are `io.kestra.plugin.core.execution.*` (19 runs) that **no W3a task
claims**, and 1 is the `Log` exception above. See *States and constructs no W3a
task claims*.

**Nothing banked rests on any of this** — zero of the 134 banked runs sit in the
135-run over-fed bucket. See *What Task 3 had to change, and what it banked* for
the intersection that establishes it.

What makes that acceptable is not the argument, it is that `golden.json` is
machine-derived and re-derivable: `normalise.py` at HEAD reproduces both it and
`stability.json` byte-identically from the committed `traces.json`, and
`--verify-traces` re-reads every captured execution against the live Kestra. The
one field nobody can catch by comparison is the one field nobody hand-writes.

### The environment has three tiers, and tier 2 was an accident

The oracle is a *caller*, and a caller decides when the worker's answers arrive.
`tests/executor.rs` does it in three tiers, strictly in this order:

1. **`advance()` decides.** If it emits commands they are applied and it is
   asked again. Nothing else happens while it still has work.
2. **The worker finishes ONE task.** Only when `advance()` returns nothing: the
   earliest-created `Runnable` still `Running` reaches the terminal state the
   golden recorded **for its current attempt**.
3. **Time passes.** Only when `advance()` has nothing to do and nothing is in
   flight: `now` jumps to the earliest `not_before` still ahead of it.

**Tier 2 is a deliberate choice now and was an accident of loop order before.**
The first version completed every `Running` task at the top of the loop, *before*
every `advance()` call — so `advance()` was never once invoked with a task in
flight. That is not an inference: an `advance()` that panics the moment it sees
a `Runnable` in `Running` **does not fire at all** under the old order and fires
immediately under the new one. Every worker was zero-latency and no interleaving
was ever simulated, which put concurrency caps, `WaitFor` and timeouts — Tasks
5, 6 and 7's subjects — out of reach of the instrument built to judge them.

Completing **one** task rather than all of them is part of the same choice: "two
running, one finishes, `advance()` is asked what to do" is the state a
concurrency limit is about, and it does not exist if the environment finishes
everything at once. It is safe for the happens-before relation because a run's
left endpoint is its *creation* step, so siblings created in one round stay
`overlapping` whatever order they finish in.

Tier 3 is a rule, not a sleep: time moves only when nothing else can happen, and
only to a deadline something is already waiting for. So a `ScheduleRetry` cannot
deadlock the simulation, and a `timeout` cannot fire early. It also cannot fire
at all from elapsed work — see *what this oracle does not prove*, which now names
both causes of that, only one of which lives here.

**Settled and stuck are different answers.** After tier 3 finds no deadline, a
run left `Running` — a flowable, which tier 2 never picks up — used to fall
straight out of the loop as a *truncated trace*, reported as a mismatch: a wrong
answer a maintainer could baseline, when the rule is that an implementation
which cannot finish is a broken instrument. A `ScheduleRetry` whose `not_before`
was already past was dropped by the same filter. Both are now a `STUCK` error
naming the run and its deadline, reported through the `panics` arm. The
qualifier that keeps the stub honest is "started": a machine that creates
nothing and never leaves `[CREATED]` has not started, and its empty trace is the
legitimate mismatch the whole baseline is made of.

**One interleaving, not interleavings.** `in_flight` takes the *earliest-created*
run, so "the later-created one finishes first" never happens. It costs nothing
for the happens-before relation — a run's left endpoint is its creation step —
but Tasks 5 and 7 must know that worker return order is pinned to creation
order.

### None of this was guarded, and now it is

With `advance()` a stub, nothing committed ever reached the **second** iteration
of the simulation loop. Six sites were replaced with unconditional panics —
tier 1's command-application body, tier 2, tier 3's scan, `in_flight`'s closure,
`step_relation`'s body and the `order` assembly loop — and the suite still
reported `8 passed; 0 failed`. The endpoint fix that closed an over-acceptance
could have been reverted the next day and nothing would have said so.

The fix is the one this file already used twice: make the rule reachable.
`simulate_with()` takes the state machine as a parameter, `simulate()` passes
the real `advance`, and five committed tests pass their own. Re-run against the
same six mutations, every one of them now kills at least one test — and
reverting the left endpoint to `StartRun` fails
`an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended`
with `left: [(0, 1, "before")] right: [(0, 1, "overlapping")]`.

## The happens-before relation

Dropping the clocks outright would lose something Kestra's own tests assert.

**The count, measured.** The plan says "39 ordering assertions across 12 test
files"; that did not reproduce, and the honest figure is smaller and sharper.
At checkout `96893b589`, `getState().getStartDate().isAfter(` appears **4
times, in 2 files** — `ParallelTest.errors()` and `DagTest.errors()`, two
assertions each. (Widening to every `isAfter`/`isBefore` in a test that mentions
a date gives 35 lines in 15 files, but almost all of those are KV expiry and
repository timestamps, not run ordering.) The wider number is not what this
section rests on, so it is stated as what it is rather than repeated.

The four are all of one form:

```java
assertThat(execution.findTaskRunsByTaskId("a2").getFirst().getState().getStartDate()
    .isAfter(execution.findTaskRunsByTaskId("a1").getFirst().getState().getEndDate().orElseThrow()))
    .isTrue();
```

That is not a timing claim; it is the claim that `a2` **serialised after** `a1`,
which is exactly the semantics a `sequential` nested in a `parallel` exists to
provide. A normaliser that drops all clocks cannot see a flowable that stops
serialising — and four is enough for that to matter, because the four are the
only assertions in Kestra's suite that distinguish `parallel` and `dag` from
"the same tasks in some order".

**The golden reproduces all four.** They drive `finally-parallel` and
`finally-dag` with `{"failed": true}`, which is exactly the input Task 1
recovered for both. `golden.json` gives those two flows 10 and 9 runs — the
sizes the Java tests assert — and carries `a1 → a2 before` and `e1 → e2 before`
in each. Nothing was written to make that come out; it is what the harvest
measured, checked against the Java afterwards.

So for every ordered pair of runs in every execution, `normalise.py` computes
one of `before` / `after` / `overlapping` — strictly, matching the Java form, so
two runs sharing a timestamp are `overlapping` and never `before` — and only
then throws the timestamps away.

**The relation is kept only where it is stable.** `harvest.py --traces 5`
executes every `runs` flow five times, round-major — all 268 flows in round
1 before round 2 starts any — because five back-to-back executions of one flow
share a warm JVM, a warm page cache and a quiet queue, which is exactly the
condition under which a race does not show. A pair survives only if all five
rounds agreed:

```
  kept (identical in all 5 rounds)             1104
  dropped, relation flipped between rounds     13
  dropped, a run had no start or no end        0
  TOTAL                                        1117
```

The dropped pairs are not a defect; they are the measurement. `runnable.txt`
prints the same table over the same flows — the ones that reached the golden —
and `stability.json` lists every dropped pair with the five relations it was
seen as.

The kept pairs are **743 `before` and 361 `overlapping`**. There are no `after`
pairs, and that is **unreachable by construction, not a measurement**: pairs are
enumerated `(i, j)` with `i < j` in `taskRunList` order, and a run's left
endpoint is its creation instant, which is monotonic in that order in 4215 of
4215 captured runs — so `after` would need a run to end before it was created.
It is kept in the vocabulary because it is still a live error signal on egret's
side, where nothing guarantees that monotonicity; it is not evidence about
Kestra.

`tests/executor.rs` derives the same relation from egret with the same
predicate over **`advance()` round numbers** instead of timestamps. That is the
point: a pure state machine has no clock, and its rounds are exactly what
serialisation means to it.

**The same predicate has to mean the same endpoints, and once it did not.**
Kestra's left endpoint is a run's **creation** instant — `state.startDate`
equals `histories[0].date` in 4215 of 4215 captured runs, and that first history
state is `CREATED` in all 4215. egret's was recorded on `StartRun`, the
**RUNNING** transition. That makes egret's interval a strict sub-interval of
Kestra's, so `Kestra-before` implies `egret-before` and never the converse:
egret's `before` set was a **superset**, which is over-acceptance — the one
divergence class this programme forbids shipping.

It was not theoretical. `valids/labels-update-task-deduplicate` is two runs with
`order: [[0, 1, "before"]]`, which means Kestra did not create `from-list` until
`from-string` had terminated. An `advance()` that creates **both** runs in one
round and only then serialises them was **accepted** — the ratchet reported the
entry as matching and told the maintainer to bank it. With the left endpoint
moved to creation, the same `advance()` produces `overlapping` and the entry
correctly mismatches.

### Traces that did not reproduce

Five rounds also measure something the plan did not ask for and that turned out
to be worth publishing: whether the trace *itself* — runs, states, attempts,
sub-executions, everything but the order — is identical every time.

It is not, for 2 of the 268 flows that ran:

- `valids/retry-failed-task-duration` — rounds [3, 4] differed from round 1.
- `valids/waitfor-max-duration` — rounds [2, 4] differed from round 1.

Where it is not, the flow gets **no golden entry**, is listed in `runnable.txt`
and in `stability.json`, and is counted in the exclusion table below. A trace
the oracle does not reproduce five times running cannot judge egret: an
implementation matching round 1 would be right by luck and one matching round 3
would be marked wrong. Silently keeping round 1 is the version of this that
looks fine.

## What `golden.json` covers, and what it excludes

| | count |
|---|---|
| compared — `verdict: "runs"` | 266 |
| compared — `verdict: "refused"` | 28 |
| **golden.json total** | **294** |
| excluded — `not-standalone` | 26 |
| excluded — rejections that are not refusal evidence | 23 |
| excluded — traces that did not reproduce across five rounds | 2 |
| **excluded total** | **51** |
| **TOTAL** | **345** |

Every one of the 345 corpus flows is in exactly one row, and
`tests/executor.rs::every_corpus_flow_is_compared_or_excluded_by_name` fails
naming any flow that is in none of them. An entry that is silently skipped is
worse than one that is honestly excluded: the coverage claim keeps its
denominator and loses its numerator.

The second exclusion is the one worth being careful about. Task 1 measured that
`rejected` is four different things wearing one status code, and only
`invalid-flow` (27) and `refused-by-both` (1) are evidence
that egret must refuse a flow. `task-type-not-in-image` (22) and
`checkout-drift` (1) are the pinned image and the sibling checkout
differing. A golden that demanded a refusal for those would manufacture
23 defects out of a packaging accident, and Task 8 would implement them.

## The state vocabulary, measured

Rule 4 keeps history state sequences verbatim, and verbatim turns out to be
wider than the executor's draft `RunState`. Measured over every golden entry:

| state | run histories that contain it |
|---|---|
| `CREATED` | 840 |
| `RUNNING` | 828 |
| `SUCCESS` | 639 |
| `SUBMITTED` | 580 |
| `FAILED` | 184 |
| `PAUSED` | 20 |
| `RETRYING` | 20 |
| `WARNING` | 12 |
| `RETRIED` | 8 |
| `SKIPPED` | 6 |
| `KILLED` | 5 |
| `CANCELLED` | 2 |

Two of these are load-bearing:

- **`SUBMITTED`** appears on tasks the *worker* ran and never on a flowable the
  *executor* ran — measured on `valids/parallel`, where `parent` (the `Parallel`
  itself) goes `CREATED RUNNING SUCCESS` while each of `t1`..`t6` goes `CREATED
  SUBMITTED RUNNING SUCCESS`. It is the only thing in the record that separates
  the two, so dropping it would be normalising away a distinction rather than a
  representation.
- **`RETRYING`**, and a repeated **`CREATED`**, are how a retried task's history
  reads: `valids/retry-failed-task-attempts` gives one taskrun with four
  attempts and a history of `CREATED SUBMITTED RUNNING FAILED RETRYING` four
  times over.

The table above is **run** histories. `Command::SetExecutionState` carries a
`RunState` too, so the vocabulary `RunState` actually has to cover is the union
of both, and that union is **13**, not 12: `RESTARTED` appears 60 times in
execution histories and **never** in a run history. Going the other way,
`SKIPPED` and `SUBMITTED` appear only in run histories.

`executor::RunState` had six members when this was written, so 7 of the 13 were
unrepresentable. **Task 3 added `SUBMITTED` and no other**, so the enum has seven
and **6 of the 13 remain unrepresentable**: `PAUSED`, `RESTARTED`, `WARNING`,
`SKIPPED`, `RETRIED`, `CANCELLED`. Each is recorded here rather than worked
around in the normaliser, because a normaliser that quietly maps an
unrepresentable state onto a representable one is a normaliser that hides the gap
it was built to expose.

**What the one addition bought, and what the six still cost.** Of the 266 `runs`
entries, **35** use only the six members the plan drafted, and **229** use only
the seven — so `SUBMITTED` alone is what moved the reachable set by 194 entries,
and it was the single cheapest thing any task could do to this oracle. It is
also not optional for Task 3's own slice: `SUBMITTED` is in 580 of the golden's
840 run histories and on every task the worker ran, so a sequential flow of
`Return` tasks cannot match without it.

The other six, with the number of `runs` entries whose trace uses each and the
task that owns it:

| state | `runs` entries | owner |
|---|---|---|
| `PAUSED` | 18 | `Pause`, which no W3a task claims — see the hole list |
| `RESTARTED` | 12 | execution histories only; a restart is W3b's, not `advance()`'s |
| `WARNING` | 10 | Task 7 (`AllowFailure`, `warningOnRetry`) |
| `SKIPPED` | 4 | `disabled: true`, which no W3a task claims |
| `RETRIED` | 4 | Task 7 (`CREATE_NEW_EXECUTION`) |
| `CANCELLED` | 3 | flow concurrency, which no W3a task claims |

A later task adding one of these adds it here too; the counts are what
`normalise.py` measured over `golden.json`.

## What Task 2 had to add to the interfaces

The plan's Task 3 lists the types; Task 2 needs three things it does not
mention, and adding them quietly would be the seam defect this programme keeps
finding.

- **`build_plan(source) -> Result<Plan, PlanError>`.** The ratchet has to turn a
  corpus YAML into something `advance()` can be driven over, and Task 8 has to
  assert that a refusal *is* a refusal. Task 3's `plan_of(corpus_id)` is a
  crate-internal unit-test helper, and an integration test cannot see it.
- **`apply(&mut ExecutionState, &Command)`.** `advance()` decides; something has
  to record. It is in the crate rather than in the test because the oracle is a
  *caller* and W3b's worker will be another, and two callers writing their own
  bookkeeping is two chances for the thing compared to differ from the thing
  shipped. It carries no decision: every branch is a mechanical write of what
  the command already says.
- **`Command::ScheduleRetry { not_before: Tick }`.** The plan's draft
  `command.rs` spells this `Instant` while its own `state.rs` explains at length
  why `Tick` exists. `Tick` wins; the divergence is recorded here rather than
  left for a reader to rediscover.

## What Task 3 had to change, and what it banked

Task 3 owns the types, so three changes to the plan's draft are its to make and
to justify. All three are in the crate with a guard each; none was made quietly.

- **`RunState::Submitted`.** See *The state vocabulary, measured*: the plan drafted
  six members and 35 of the 266 `runs` entries could be reached with them. Seven
  reaches 229, and a sequential flow of `Return` tasks cannot match at all
  without it. `only_a_runnable_passes_through_submitted` is the guard; deleting
  the emission leaves a trace with the right runs in the right order and no
  matches.
- **`Command::StartRun { run, at: Tick }`** and **`Attempt::started_at`.** This is
  the answer to cause 2 of the elapsed-time hole below, and the shape is
  measured rather than guessed: Kestra takes a `RETRY_FAILED_TASK` retry's
  `maxDuration` from `this.attempts.getFirst().getState().minDate()`
  (`core/src/main/java/io/kestra/core/models/executions/TaskRun.java:337`) and a
  `CREATE_NEW_EXECUTION` one from `execution.getMetadata()
  .getOriginalCreatedDate()` (line 316). The first is the attempt's start; the
  second is the execution's creation, which is `Tick(0)` by the type's own
  definition. So one field on the attempt covers both, and `TaskRun::started_at()`
  derives the run's start from `attempts[0]` rather than storing a second copy
  that could disagree. The instant rides on the command rather than being a `now`
  parameter on `apply` so that there is exactly one value it can be — the one
  `advance()` saw. `an_attempt_records_the_tick_it_was_started_at` is the guard
  for the field.

  **The two accessors were interchangeable, and now are not.**
  `TaskRun::started_at()` reads `attempts.first()` and
  `attempt_started_at()` reads `attempts.last()`, and the whole justification for
  having both is that Kestra measures two different rules from the two different
  instants. Under the committed suite as first shipped, **swapping `.first()` for
  `.last()` in *either* one built with 0 errors and left the crate at 434 passed
  / 0 failed with the ratchet unmoved at 193** — because Task 3 never produces a
  run with more than one attempt, so
  `an_attempt_records_the_tick_it_was_started_at` pins the field and cannot tell
  the accessors apart. Two accessors argued from two Kestra lines and
  distinguishable by nothing is the third instance of the
  guard-that-cannot-fail pattern in this package, and the first two shipped.
  `the_run_start_and_the_attempt_start_are_different_instants` now opens a
  **second attempt through `apply` and a real `Command::StartRun`** — the command
  Task 7's retry will emit — so it pins the writer too, and only the decision to
  emit it is the test's. Both swaps now die (M10, M11).
- **`build_plan` refuses only what it cannot parse.** Not a change to a type, but
  the decision most likely to be undone by a later reader as an obvious
  improvement, so it is recorded with its measurement: see the mutation table
  below, row M8.

**Banked: 101 of the 294**, leaving 193 in `baseline.txt`. Every one of the 101
was shown red first, and not by argument: at `0e157363` — the commit before this
one — the same ratchet reports *294 golden entries, 294 mismatching, 294
baselined, 0 broken*. The composition, derived from the golden rather than
listed:

| | count |
|---|---|
| `runs` entries whose golden has a parented run — a real `Sequential` ran | 2 (`valids/sequential`, 11 runs and 3 nesting levels; `valids/sequential-sleep`) |
| `runs` entries that are the root task list only | 93, of which 77 are a single run |
| `refused` entries | 6 |
| **TOTAL** | **101** |

Across the 95 `runs` entries that banks 134 runs, 109 happens-before pairs (92
`before`, 17 `overlapping`) and 23 executions that terminate `FAILED` rather than
`SUCCESS` — so the stop-at-the-first-failure path is exercised by the corpus and
not only by a unit test. **No banked entry contains a flowable other than
`Sequential`**: the only other `io.kestra.plugin.core.flow.*` type in the 95 is
`Sleep`, which is a `RunnableTask` in Kestra too, so nothing here matches because
an opaque flowable happened to look like a worker task.

**Nothing banked rests on the one field this oracle feeds.** A review
intersected each mutation's victims with the 101: all 95 banked `runs` entries
die to three *independent* decision mutations (M1, M2, M9), and the only 4 banked
runs whose golden carries no `SUBMITTED` are exactly the 4 `Sequential` runs
(`1-seq`, `1-3_seq`, `1-3-2_seq`, `seq`) — re-derived here, not relayed. So zero
banked runs sit in the 135-run over-fed bucket.

### Refusal: 22 over-accepted, and 6 banked on the weakest evidence there is

**Over-acceptance first, because it is the class the programme forbids
shipping.** Of the golden's 28 `refused` entries, `build_plan` **over-accepts
22** — it plans a flow this Kestra refused. That is unchanged from BASE, honestly
baselined, and Task 8's whole subject; Task 3 refuses only what W1's parser
cannot read and deliberately does not narrow the gap (M8 below is what happens
when it tries). The count is here so that "101 banked" is never read as progress
against the divergence class that matters.

**The other 6 are banked on the weakest possible evidence, and Task 8 must
re-derive them.** They are `invalids/{inputs, inputs-bad-type, invalid,
invalid-parallel, switch-invalid, system-labels}`, and they match only because
the oracle asks one thing of a `refused` entry — that `build_plan` returns `Err`
— and W1's YAML parser happens to refuse them.

**Two of the six are right by accident, and the same rule is an OVER-REFUSAL that
reaches the HTTP API.** `models::Switch::defaults` and every other bare
`Vec<Task>` in `models/task.rs` reject an explicit `null` that Jackson accepts —
the bean-validation-is-not-parsing principle that file applies correctly to
`Parallel::concurrent` and every other scalar. Pinned to the token by
`executor/mod.rs::a_null_task_list_is_an_over_refusal_that_reaches_the_http_api`:

- `valids/switch-impossible` — a flow this Kestra published and **ran** — is
  refused, and **parses the moment `defaults: null` is deleted**.
- `invalids/switch-invalid` parses without that same line too, so its refusal
  says nothing about its empty `cases:`.
- In `invalids/invalid-parallel`, only `para4`'s `tasks: null` refuses: `para2`
  (no `tasks` key) and `para3` (`tasks: []`) both parse. The rule is exactly
  "explicit null", not "missing or empty".

**Blast radius: an API defect, not an oracle note.** `spec::parse` is called from
`routes.rs:302` (`draft_dto`), `routes.rs:335` (`parse_draft`, used by
`create_flow` and `update_flow`) and `routes.rs:640` (`validate_flow`). So `POST
/flows`, `PUT /flows/{id}` and `POST /flows/{id}/validate` return **400
InvalidInput** today for a flow this Kestra accepts and runs. W1's model owns the
fix; when it lands the committed test flips, and whoever fixes it must revisit
`baseline.txt` and Task 8's per-group message assertions in the same commit.
`valids/switch-impossible` stays baselined meanwhile as `egret REFUSED a flow
Kestra ran`, which is where a measured over-refusal belongs.

## The ratchet, and the four arms

`tests/executor.rs` is a ratchet, not a pass/fail. `advance()` was a stub at Task
2, so **every** comparable entry mismatched and all 294 were in `baseline.txt`;
Task 3 banked 101 of them and 193 remain. A test that is simply red for six tasks
measures nothing — a new mismatch would be invisible in the noise — so it fails
in **both** directions: an id not in the baseline that mismatches is a
regression, and an id in the baseline that now matches must have its line deleted
in the commit that earned it.

`assemble_failures` is a pure function carrying four arms, and each was **shown
red on purpose** before this was committed. A green run sits exactly at
baseline, so `stale`, `regressions` and `banked` are all empty on every passing
run and nothing else would ever construct a `Findings` — an arm nobody has seen
fail is an arm nobody has seen.

| arm | fails when | proven by |
|---|---|---|
| `panics` | the state machine panicked, or would not settle inside the step bound | neutering the arm (`if false && …`) kills `a_panic_fails_even_when_the_ratchet_is_satisfied` and nothing else |
| `stale` | a baseline line names an id `golden.json` does not have | neutering kills `a_stale_baseline_line_fails_and_names_it`; and a bogus line added to the real file fails the real ratchet, naming it |
| `regressions` | an id **not** in the baseline mismatches | neutering kills `a_regression_outside_the_baseline_fails_and_carries_its_detail`; and deleting a real baseline line fails the real ratchet, naming it with its detail |
| `banked` | an id **in** the baseline now matches, **and did not panic** | neutering kills `progress_that_is_not_banked_fails_and_names_it`; and making `build_plan` refuse one flow fails the real ratchet, naming the line to delete. Deleting the `!panicked` exclusion kills `a_baselined_id_that_panicked_is_not_reported_as_banked` |

**A panic is never covered by the baseline.** A baselined id means "egret does
not reproduce this trace yet"; it never means "egret may crash here". Not
settling inside the step bound is reported the same way and for the same reason:
an implementation that spins is a broken instrument, not a wrong answer.

**And a panicked id is not "progress" either**, which took a second pass to get
right. `banked` was `baseline ∩ golden \ mismatched`, and a panicking id lands
in `panicked`, never in `mismatched` — so with `build_plan` panicking on three
flows the ratchet reported the `PANIC` arm **and**
`PROGRESS NOT BANKED: 3 entr(y/ies) … now MATCH. Delete these lines…`, naming
three entries that could not run at all. A ratchet that hands out a free pass
for a crash is the failure this file exists to prevent. `classify()` now
excludes them, and `a_baselined_id_that_panicked_is_not_reported_as_banked`
dies if that exclusion is deleted.

### Task 3's mutations

Twelve, each built first and asserted to compile (`cargo build -p egret-workflow
--tests` reporting **0 errors**) before being run, because a mutation that does
not build looks exactly like one no test caught. Each was run against the
**whole crate** — `cargo test -p egret-workflow --no-fail-fast`, 436 lib tests
plus six integration binaries — and not against a name-filtered subset, which
can only confirm the expectation it was filtered by. The crate is green at 0
failures without them.

**The exact edit is recorded, not a description of it.** A review re-ran these
from the descriptions and got different numbers twice — M3 killing 5 unit tests
where the table said 4, and an M8 formulation giving 0 regressions / 6 bogus
banks where this one gives 12 / 8 — both resolved only by matching the exact
text. A mutation table that cannot be replayed is a claim, not a record.

| # | file | edit (`-` → `+`) | what died, and the message |
|---|---|---|---|
| M1 | `sequential.rs` | `- cmds.push(Command::StartRun { run, at: now });` → `+ let _ = (run, now);` | ratchet + **6** unit tests. `PANIC: the state machine panicked or would not settle on 285 entr(y/ies)` … `STUCK after 2 step(s) at now=Tick(0): nothing to do, nothing in flight, no deadline ahead … run 0 (dag) is Some(Submitted)` |
| M2 | `sequential.rs` | delete the whole `if matches!(… Runnable …) { cmds.push(SetRunState { state: Submitted }) }` block, replaced by `let _ = (plan, task);` | ratchet + **2**. `runnable 1-1 must pass through SUBMITTED — left: ["CREATED","RUNNING","SUCCESS"] right: ["CREATED","SUBMITTED","RUNNING","SUCCESS"]`. Also **21 baselined entries start matching** — see *States and constructs no W3a task claims* |
| M3 | `sequential.rs` | the single `CreateRun` for the first un-created child → `children.iter().filter(no run yet).map(CreateRun).collect()` | ratchet + **5**. `left: [CreateRun { task: 0, .. }, CreateRun { task: 10, .. }] right: [CreateRun { task: 0, .. }]` |
| M4 | `sequential.rs` | `- if *s != RunState::Success {` → `+ if false && *s != RunState::Success {` | ratchet + **2**. `the first task failed, so none of the other 4 was created — left: 5 right: 1` |
| M5 | `sequential.rs` | `- at: now` → `+ at: Tick(0)` on the `StartRun` push | `an_attempt_records_the_tick_it_was_started_at`. `left: Some(Tick(0)) right: Some(Tick(11))` |
| M6 | `plan.rs` | `- if depth > MAX_TASK_DEPTH {` → `+ if false && depth > MAX_TASK_DEPTH {` | `nesting_past_the_bound_is_refused_by_name`. `nesting past the bound must be refused; got a plan of 49 task(s)` |
| M7 | `plan.rs` | the `if let (ModelKind::Sequential(seq), SEQUENTIAL_TYPE)` arm → a `match` also taking `ModelKind::AllowFailure(a)` and `ModelKind::WorkingDirectory(w)` | `every_other_flowable_stays_opaque` **and** the feed test. ``AllowFailure is opaque, so `inner` must not be in the plan at all — left: 2 right: 1``; the split moves `(390,23,427,135)` → `(392,30,418,133)` |
| M8 | `plan.rs` | insert, before `let ix = out.len();`: `if task.r#type.starts_with("io.kestra.plugin.core.flow.") && task.r#type != SEQUENTIAL_TYPE { return Err(…"uses an unimplemented flowable") }` | ratchet **in both directions at once**, plus `every_other_flowable_stays_opaque` and the feed test. `REGRESSION: 12 entr(y/ies)` … `valids/flow-concurrency-cancel: egret REFUSED a flow Kestra ran: task id 'sleep' uses an unimplemented flowable`, **and** `PROGRESS NOT BANKED: 8 entr(y/ies)` naming `invalids/dag-cyclicdependency`, `invalids/recursive-flow` and six more. This is the whole argument for the restraint in `build_plan`'s doc, measured: the refusal is right for a reason that has nothing to do with the flow being invalid, and the ratchet cannot tell |
| M9 | `mod.rs` | `- if not_running_yet && (has_a_run \|\| terminating) {` → `+ if false && …` | ratchet + **3**. `left: ["CREATED","SUCCESS"] right: ["CREATED","RUNNING","SUCCESS"]` |
| M10 | `state.rs` | `TaskRun::started_at`: `- self.attempts.first()` → `+ self.attempts.last()` | `the_run_start_and_the_attempt_start_are_different_instants`. `started_at() is the FIRST attempt's instant — left: Some(Tick(4200)) right: Some(Tick(11))` |
| M11 | `state.rs` | `TaskRun::attempt_started_at`: `- self.attempts.last()` → `+ self.attempts.first()` | the same test. `attempt_started_at() is the LAST attempt's instant — left: Some(Tick(11)) right: Some(Tick(4200))` |
| M12 | `tests/executor.rs` | `- if has_children.contains(&ix) {` → `+ if has_children.contains(&ix) && run.parent.is_none() {` | the feed test. `120 golden runs are the parent of another run … That is the entire basis for the 720 ceiling — left: 86 right: 120` |

M6 also earned a change to the test rather than to the code: `expect_err` printed
the whole 49-task `Plan` and buried the finding, so the assertion prints the task
count instead. A guard that fires with an unreadable message has been seen fire
and not seen work.

**The restore path matters as much as the edit.** The driver restores with
`shutil.copyfile`, which copies content only and stamps the destination mtime at
write time, so cargo always rebuilds — verified, not assumed. A restore that
*preserves* mtimes (`tar xf`, `cp -p`, `rsync -a`) makes cargo rebuild nothing,
and a "pristine" control run then reproduces the mutation's failures exactly,
reading identically to a real result. The same family caught this task once
already: `cp` is aliased to `cp -i` in this shell, so a snapshot refresh silently
refused to overwrite and a stale file was restored over a fix.

## States and constructs no W3a task claims

The plan carries this table too (`docs/en/plans/2026-09-04-workflow-w3a-executor.md`,
*States and constructs no W3a task claims*); it is here because the entries it
strands are entries of `baseline.txt`, and a permanently baselined line with no
owner is how a suppression list starts.

| construct | what it strands | produced by | owner |
|---|---|---|---|
| `Pause` | `PAUSED`, 18 golden `runs` entries | `io.kestra.plugin.core.flow.Pause` | W8 |
| flow concurrency | `CANCELLED`, 3 entries | flow-level `concurrency:` | W8 |
| `disabled:` | `SKIPPED`, 4 entries | a task-level flag | **unclaimed — W3a's final review must assign it or defer it with a reason** |
| `ExecutionUpdatableTask` | 19 over-submitted runs across 16 baselined entries | `Labels`, `SetVariables`, `UnsetVariables`, `Exit` | **unclaimed** |
| `Subflow` | 22 baselined `runs` entries, 18 over-submitted runs | `io.kestra.plugin.core.flow.Subflow` | **unclaimed — the word does not appear in the plan** |

The fourth row is Task 3's, and it is wider than the name that surfaced it.
`io.kestra.plugin.core.execution.Labels` is `extends Task implements
ExecutionUpdatableTask` (`core/src/main/java/io/kestra/plugin/core/execution/Labels.java:79`)
— **neither a `RunnableTask` nor a flowable**. Kestra's *executor* calls
`update(execution, runContext)` on it and never submits it to a worker, so its
runs carry no `SUBMITTED`. The interface has four implementors in this checkout:
`Labels`, `SetVariables`, `UnsetVariables` and `Exit`.

`advance()` plans all four as opaque `Runnable`s and therefore emits `SUBMITTED`
on them, and the cost is exact rather than estimated. Under M2 — the mutation
that deletes the `SUBMITTED` emission — **21** baselined entries start matching,
which is the set whose *only* remaining mismatch is that emission. Six of the 21
are `Labels` flows (`labels-update-nested-value-list`,
`labels-update-nested-value-map`, `labels-update-task`,
`labels-update-task-deduplicate`, `npe-labels-update-task`,
`primitive-labels-flow`); of the other 15, twelve are `Subflow`, two are `If`
and one is `Switch`.

**And `Subflow` is the fifth unclaimed construct, which checking rather than
assuming is what found.** `If` and `Switch` are Task 5's and `Loop` is Task 6's,
but **the word `Subflow` does not appear anywhere in
`docs/en/plans/2026-09-04-workflow-w3a-executor.md`** — no task creates a
`TaskKind` for it, and `Command::SpawnExecution` arrives in Task 6 for a *loop
iteration*, which is a different construct (a `Loop` body never gets a run in the
parent execution; a `Subflow` task does). It is a `core.flow.*` class, so it
looks owned to anyone reading the flowable list, and it is the largest single
block left: **22 baselined `runs` entries** use it, 18 of the over-submitted runs
are its, and 12 of M2's 21.

**So those six can be banked by no W3a task as scheduled** — and one of them is
`valids/labels-update-task-deduplicate`, the entry this file's own circularity
demonstration is built on. The demonstration stands (it was run against a state
machine that reproduces the entry), but nothing in W3a will ever force a reader
back to it.

**M2 also shows the emission is a net protection, not a net cost.** Deleting it
does not merely fail to bank: it makes 21 entries match *by accident*, on flows
whose flowables egret does not implement at all. `SUBMITTED` is what stops that.

## What this oracle does not prove

The corpus is Kestra's own test suite, which is a sample of an infinite space.
The ratchet prevents regression; it does not bound behaviour on a flow nobody
wrote. Beyond that, the specific holes, each with its count:

- **26 `not-standalone` flows are never compared at all.** They have a
  corpus entry and no golden entry. Kestra itself cannot run them without what
  the Java test brings — a resume, a trigger, an uploadable file, a mocked
  worker, or an input value that is computed rather than literal.
- **23 rejections are not evidence of anything about the flow**, and are
  excluded for that reason. Nothing in this package tests what egret does with
  them.
- **2 flows did not reproduce across five rounds** and are excluded:
  `retry-failed-task-duration` (three attempts in some rounds, four in others —
  a *duration*-bounded retry counts whatever fits) and `waitfor-max-duration`.
  Both are bounded by wall-clock duration, so this is the flow being
  nondeterministic and not the harness.
- **13 ordered pairs were dropped**, all of them because the relation flipped
  between rounds; none because a run had no start or no end. The kept 1104 —
  743 `before`, 361 `overlapping` — are the whole of what this oracle asserts
  about serialisation.
- **A kept `before` is not always reachable by a clockless implementation.** The
  relation egret produces comes from `advance()` round numbers, so two branches
  a `parallel` starts together and finishes together are `overlapping` no matter
  how long each took. Where Kestra's scheduler happened to serialise two
  branches the same way five times running, the golden records `before` and
  egret will say `overlapping` — a mismatch that is about the instrument, not
  about the flowable. None is known to exist today because nothing is
  implemented yet; Tasks 4 and 6 are the ones that will find out, and the honest
  answer if it happens is to record the pairs, not to weaken the relation.
- **Nothing keyed on elapsed time is reachable, and there were TWO independent
  causes. Task 3 closed the second; the first stands.**
  1. *The environment, which stands.* `now` moves only to a `not_before`
     something is already waiting for, so no tier advances the clock because a
     task took a while. Tasks 5 and 7 could fix that inside this oracle. Until
     one does, an implementation whose `maxDuration` is correct and one whose
     `maxDuration` is ignored produce the same trace here.
  2. *The state type — **closed**.* `ExecutionState` and `TaskRun` carried no
     instant at all except `not_before`, so `advance()` had nothing to subtract
     `now` from. Task 3 added `Attempt::started_at`, written by
     `Command::StartRun { at }`; `TaskRun::started_at()` derives the run's start
     from the first attempt. See *What Task 3 had to change* for the two Kestra
     lines that fix which instant each rule is measured from.

  What that covers has also been corrected, and the correction is measured.
  `PlanTask::timeout` is **not** an executor concern in this artifact: Kestra
  enforces it in the **worker**, in
  `worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:58`,
  which wraps `task.run(runContext)` in `callWithTimeout`. Kestra's own validator
  says so out loud —
  `core/src/main/java/io/kestra/core/services/FlowService.java:574` emits "cannot
  use the 'timeout' property as it's only relevant for runnable tasks" for any
  non-`RunnableTask`, with `Pause` the single exception. So `timeout:` belongs to
  W4's worker, not to `advance()`, and the elapsed-time rules this oracle cannot
  reach are `RetryPolicy::max_duration`, `WaitFor`'s `maxDuration`, a `Pause`
  delay and a `Pause`'s own `timeout` — not `timeout:` in general.
- **`PlanTask::retry`, `PlanTask::timeout` and `PlanTask::errors` are dead
  fields today**, and for three different reasons. `build_plan` leaves the first
  two `None` because Kestra spells both as `Property<Duration>` — unrendered
  source — and rendering one needs a `RenderContext` a pure builder must not
  acquire; Task 7 owns where that render happens. `errors` is empty for a
  different and worse reason: W1's `models::Sequential` and every other
  `AbstractBranch` port in `models/task.rs` **drop** the Java class's
  `errors`/`finally` fields, so there is nothing in the parsed flow to read them
  from. Task 7 has to widen the model before it can implement `errors:` at all.
- **Sub-executions are harvested but not yet produced.** `Command::SpawnExecution`
  arrives in Task 6; until then egret's side of `spawned` is always empty, so
  every one of the 41 golden entries that carries a sub-execution
  mismatches on that field alone. Those entries look compared and are not, until
  Task 6 says otherwise — which is why the count is here.
- **`agrees` on `valids/pause-behavior` is a coincidence in the strong sense.**
  See the discrimination-probe section: the flow dies rendering
  `behavior: "{{ inputs.behavior }}"` into a `Property<Behavior>` and never
  reaches the behaviour at all.

## The API surface, measured

Everything below was checked against the running container; do not re-derive it.

| endpoint | note |
|---|---|
| `POST /api/v1/main/flows` | `Content-Type: application/x-yaml`, the whole file as the body |
| `GET /api/v1/main/flows/search` | **default page size is 10** — always pass `?size=` |
| `POST /api/v1/main/executions/{namespace}/{flowId}` | `multipart/form-data` only |
| `GET /api/v1/main/executions/{id}` | works for sub-executions too |
| `GET /api/v1/main/logs/{executionId}` | where the `[[link execution="…"]]` lines are |
| `GET /api/v1/plugins` | 200. **not** `/api/v1/main/plugins`, which is 404 |
| `GET /api/v1/version` | does not exist — 404 |

The tenant is `main`.

## `corpus.json`

One object per flow, in `valids` then `invalids` order:

```json
{
 "id": "valids/finally-parallel",
 "path": "corpus/valids/finally-parallel.yaml",
 "flow_id": "finally-parallel",
 "namespace": "io.kestra.tests",
 "declares_inputs": true,
 "publish_status": 200,
 "inputs": {"failed": true},
 "inputs_from": "core/src/test/java/io/kestra/plugin/core/flow/ParallelTest.java:48 errors()",
 "inputs_alternatives": [
  {"inputs": {"failed": false},
   "inputs_from": "core/src/test/java/io/kestra/plugin/core/flow/FinallyTest.java:160 parallelWithoutErrors()"}
 ],
 "disposition": "runs",
 "reason": "FAILED",
 "observed": {"state": "FAILED", "taskruns": 10, "task_ids": ["…"], "task_states": {"…": "…"}},
 "execution_id": "…",
 "sub_executions": 0
}
```

`reason` carries the terminal state for `runs` and the specific cause for
everything else. `observed` is what the cross-check compares against.
`rejected` entries also carry `rejected_class`; the one `invalids` flow this
Kestra accepts carries `accepted_though_invalid`.

`probe.json` maps a corpus id to the discrimination probe's verdict for it:
whether it was probed, whether the alternative moved the observation, which
alternative was used, and where that alternative came from.
