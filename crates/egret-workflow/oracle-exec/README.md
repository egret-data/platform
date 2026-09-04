# W3a execution oracle — the corpus and where it came from

`oracle-exec/` stands up a pinned Kestra, publishes a corpus of flows, executes
them, and records what happened. This document covers **the corpus and its
provenance**: which Kestra, which flows, how the input values were recovered,
and — the point of Task 1 — **how much of the corpus actually runs standalone**,
which is a measured result and not an assumption.

The normalisation rules, `golden.json` and the ratchet arrive in Task 2. This
file will grow those sections; nothing here is superseded by them.

## Requirements

Docker (measured on 28.4.0) and `python3`. No JVM, no Maven, no Kestra
checkout at run time — the flows are copied in.

## Running it

```bash
cd crates/egret-workflow/oracle-exec
python3 harvest.py            # up, publish, execute, write corpus.json + runnable.txt
python3 harvest.py --no-up    # the stack is already running
python3 harvest.py --down     # docker compose down -v
```

A run takes roughly 25 minutes, most of it the seven flows that pause and are
waited out to their timeout. Always start from `--down` (i.e. `down -v`): the
harvest publishes into a real database and a half-populated one is not a
measurement.

## `docker-compose.yml` — exactly two edits

Copied verbatim from `kestra/docker-compose.yml` at the checkout below. There
are exactly two changes, and `diff` against the upstream file will show only
these:

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

Nothing else is changed, and in particular **the encryption key is not
configured**, which costs one corpus entry — see *What the two-edit rule costs*
below.

## The corpus

Copied from the sibling Kestra checkout:

| | |
|---|---|
| source | `kestra/core/src/test/resources/flows/{valids,invalids}/*.yaml` |
| checkout | `96893b589a02e5d5d85c722855f76c08e79085af` |
| `valids` | 261 |
| `invalids` | 28 |
| total | **289** |

`valids/` also contains 56 `.yml` files, which are **not** copied: the count the
spec works from is the 261 `.yaml`, and mixing the extensions would silently
change the denominator.

These are Kestra's own test resources, not flows written for this exercise.
That is the whole point: a corpus written here would prove only that the flows
someone thought of agree.

**The artifact is authoritative for behaviour; the checkout is authoritative
only for reading an implementation.** They have drifted, and the harvest found
where — see the `valid-in-checkout-refused-by-pinned-image` class below.

## The measured result

`runnable.txt` carries the counts and, for every flow that is not `runs`, the
specific reason. The headline, from `corpus.json`:

| disposition | total | valids | invalids |
|---|---|---|---|
| `runs` | 218 | 217 | 1 |
| `rejected` | 45 | 18 | 27 |
| `not-standalone` | 26 | 26 | 0 |
| `unclassified` | 0 | 0 | 0 |

**217 of the 261 `valids` run standalone.** That is the honest denominator for
every coverage claim the rest of W3a makes. It is not rounded up, and no flow
was dropped for being inconvenient: `unclassified` exists as a bucket precisely
so that a flow which fits nowhere shows up as a number rather than as an
omission, and it is empty because every one of the 289 was classified.

Following `parentId` recovered **131 sub-executions** that
`/executions/search` does not list.

Three full harvests were run from `down -v`, and the four counts were identical
in all three (the second run reclassified one entry because `harvest.py`
changed between runs, not because Kestra behaved differently).

### `runs` does not mean "correct"

`runs` means the flow published and reached a state Kestra's own
`State.Type.isTerminated()` calls terminal. It says nothing about whether the
trace is right; that is Task 2's job. And agreeing on 217 flows does not bound
behaviour on the 218th. The ratchet prevents regression; it does not prove
coverage.

## The four dispositions

| disposition | meaning |
|---|---|
| `runs` | published, and it and every sub-execution it spawned reached a terminal state |
| `rejected` | Kestra refused it — 4xx at publish, or 4xx at the execution POST for an `invalids` flow whose validation happens when its inputs are read |
| `not-standalone` | published, but cannot reach a terminal state without what the Java test brings: a resume, a trigger, an uploadable file, a mocked worker, or an input value that is computed rather than literal |
| `unclassified` | anything that fits none of the above. Its own bucket with its own reason, never a silent omission |

### Terminal states

`harvest.py` uses Kestra's own `State.Type.isTerminated()`, read from
`core/src/main/java/io/kestra/core/models/flows/State.java` at the checkout:

```
FAILED WARNING SUCCESS KILLED CANCELLED RETRIED SKIPPED RESUBMITTED
```

The plan's shorter list (`SUCCESS`/`FAILED`/`WARNING`/`KILLED`) is a subset.
`exit-cancelled.yaml` terminates in `CANCELLED`, so the shorter list would have
polled it to the timeout and filed a perfectly good flow as `not-standalone`.

### `rejected` is three different things wearing one status code

`runnable.txt` splits it, because Task 8 ("refuse what Kestra refuses") may take
only the first as evidence:

| class | count | what it is |
|---|---|---|
| `invalid-flow` | 27 | an `invalids` flow, refused. This is what Task 8 is for. |
| `task-type-not-in-image` | 16 | a `valids` flow whose task type is a **test-only class** compiled into Kestra's test source set (`io.kestra.core.tasks.test.*`, `io.kestra.core.runners.test.*`, `WorkingDirectoryTest$LocalFiles`, …). The shipped image has no such plugin. The flow is fine; the image cannot run it. |
| `valid-in-checkout-refused-by-pinned-image` | 2 | the checkout is ahead of `v2.0.0-rc13`. |

Treating either of the last two as an `invalids` would teach egret to reject a
valid flow — the exact over-acceptance mirror-image the programme says must not
ship, and harder to notice because it looks like strictness.

The two drift cases, both worth knowing before writing "Kestra does" about
anything:

- `valids/flow-trigger-fire-once-true-flow-listen` — `Unrecognized field
  "fireOnce" (class io.kestra.core.models.triggers.Window)`. The checkout's
  `Window.java:79` has `private boolean fireOnce = false;`. The pinned artifact
  does not.
- `valids/invalid-draft-flow` — `tasks: must not be empty`. The flow is
  `draft: true` with `tasks: []`; rc13 validates it anyway.

### One `invalids` flow that this Kestra ACCEPTS

`invalids/loop-switch-failed` publishes with 200, runs, and reaches `FAILED`
with 2 sub-executions.

It is not an invalid flow at all. Kestra's own
`RunContextTest.failedTasksVariable` executes it with
`@ExecuteFlow("flows/invalids/loop-switch-failed.yaml")` and asserts exactly
that: `FAILED`, one task run, two loop sub-executions. It lives under
`invalids/` because the *execution* is expected to fail, not because the flow is
rejected.

So the spec's "the 28 `invalids` are flows Kestra refuses" is **27 of 28**, and
that flow carries `"accepted_though_invalid": true` in `corpus.json` so Task 8
cannot pick it up by directory name.

### One `invalids` flow refused at a different seam

`invalids/inputs-with-multiple-constraint-violations` publishes with 200 and is
refused when its inputs are read — `MULTISELECT` values outside the declared
set. Publish-time and execution-time validation are two different gates. It is
recorded as `rejected` because Kestra does refuse it; the reason string names
both statuses so the seam is visible.

## The 53 flows whose inputs live in Java, not in YAML

**53 of the 261 `valids` declare `inputs:`.** The YAML declares that an input
exists; the *value* lives in the Java test beside it. `ParallelTest.errors()`
drives `finally-parallel.yaml` with `Map.of("failed", true)` — and running that
flow without the input does not fail loudly. It runs the other branch and
produces a golden that looks fine and tests nothing.

Every one of the 53 was resolved by reading the Java test that references it.
The table lives in `harvest.py` (`INPUTS`, `NOT_STANDALONE_INPUTS`) and every
entry carries the file, line and method it came from. **Nothing was invented.**

| | count |
|---|---|
| value recovered from a Java test | 35 |
| `not-standalone`, value not recoverable | 18 |
| total declaring `inputs:` | **53** |

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

Twenty flows are driven with more than one input map (19 of them reach
`corpus.json` with the alternatives attached; `working-directory` is rejected at
publish before its inputs are ever used). The entry records one as
`inputs`/`inputs_from` and **all the others under `inputs_alternatives`**, each
with its own provenance, so nothing measured is discarded. The primary is the
map that drives the flow's failure or error branch where there is one, because
the error path exercises `errors:` and `finally:` handling that the success path
does not. `if-condition` alone carries five alternatives — `true`, `"true"`,
`1`, `false`, `"false"`, `0` — which is Kestra's own truthiness matrix.

### The cross-check that makes the recovery falsifiable

A wrong input value does not raise; it runs the other branch. So for every flow
whose input was recovered and whose Java test asserts an execution-level state,
`harvest.py` records that state in `EXPECTED_STATE` and checks the harvested
execution against it. The section at the end of `runnable.txt` reports it:

```
agrees 29   differs 0   not exercised 1   (of 30)
```

The one not exercised is `valids/working-directory`, which never got as far as
an execution — its `WorkingDirectoryTest$LocalFiles` task type is not in the
image. A `finally-parallel` harvested with the wrong `failed` value would show
up here as `SUCCESS` where Kestra asserts `FAILED`, which is the whole point.

This is the check to re-run first if the input table is ever edited.

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
`kestra.encryption.secret-key`. Our compose file does not, because the brief
allows exactly two edits. This is a **known, priced cost of one corpus entry**,
not an accident; a later task that wants it back should add the key as an
explicit third edit and say so here.

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

Kestra becomes ready roughly 30 s after `docker compose up -d`.

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
 "execution_id": "…",
 "sub_executions": 0
}
```

`reason` carries the terminal state for `runs` and the specific cause for
everything else. `rejected` entries also carry `rejected_class`; the one
`invalids` flow this Kestra accepts carries `accepted_though_invalid`.
