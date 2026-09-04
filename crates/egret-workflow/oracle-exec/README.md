# W3a execution oracle — the corpus and where it came from

`oracle-exec/` stands up a pinned Kestra, publishes a corpus of flows, executes
them, and records what happened. This document covers **the corpus and its
provenance**: which Kestra, which flows, how the input values were recovered,
and — the point of Task 1 — **how much of the corpus actually runs standalone**,
which is a measured result and not an assumption.

The normalisation rules, `golden.json` and the ratchet arrive in Task 2. This
file will grow those sections; nothing here is superseded by them.

Every number below is a number `harvest.py` prints. If this file and
`runnable.txt` ever disagree, `runnable.txt` is right and this file is stale —
it has been wrong that way once already.

## Requirements

Docker (measured on 28.4.0) and `python3`. No JVM, no Maven, no Kestra
checkout at run time — the flows are copied in.

## Running it

```bash
cd crates/egret-workflow/oracle-exec
python3 harvest.py                    # down -v, up, publish, execute, probe, write both outputs
python3 harvest.py --rewrite-runnable # regenerate runnable.txt offline from corpus.json + probe.json
python3 harvest.py --self-test        # mutate the committed data, prove every guard fires
python3 harvest.py --down             # docker compose down -v
```

A full harvest takes roughly 35 minutes, most of it the seven flows that pause
and are waited out to their timeout, plus the discrimination probe's second
execution of every flow that has an alternative input.

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

Following `parentId` recovered **133 sub-executions** that
`/executions/search` does not list.

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
| `task-type-not-in-image` | 22 | no | the body's **only** violation is `Invalid type: X`, and `X` is shaped like a Kestra test-source class. The flow is fine; the image has no such plugin. |
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
- **`Invalid type:` must be the *only* violation.** A 422 body carries every
  constraint violation, newline-separated — `invalids/invalid-parallel` returns
  eight. Matching the phrase anywhere in the message would let a `valids` flow
  with an unknown type *plus* a real structural violation file as the lenient
  class, and the structural violation would vanish; a task `description`
  containing the literal text would match too. The classifier now requires a
  single violation matching `^Invalid type: (\S+)$`, and requires the class to
  look like test source (a `.test.` package segment, or nesting in a `*Test`
  outer class). All 22 current members were resolved by hand to files under
  `src/test/java` in the checkout; `io.kestra.core.runners.test.trigger.Alias`
  is an `aliases =` declaration on
  `core/src/test/java/io/kestra/core/runners/test/TriggerWithAlias.java`.

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
  `CREATED/RUNNING/PAUSED/RESTARTED/RUNNING/FAILED`), although
  `PauseTest.java:374-380` asserts `RESUME → SUCCESS` with 2 task runs. The
  `behavior` property, rendered from an input, has no observable effect on this
  artifact. So that row's `agrees` is a **coincidence**, not evidence the input
  landed, and Task 2 must not build a golden for this flow from the checkout's
  semantics.
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
  produces (otherwise `corpus.json` is stale in a way no count would reveal).

Guards `raise` and never `assert`: `python3 -O` strips assert statements, so a
guard written as an assert is a guard that disappears under a flag anyone may
pass. `--self-test` mutates the committed data eleven ways and requires every
guard to fire; it needs no Kestra and takes about a second.

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
