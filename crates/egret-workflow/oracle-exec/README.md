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

Task 7 (`retry`, `errors:`) changed six, and a reader should take these over
everything below and over Task 6's note: `PlanTask::retry` and `PlanTask::errors`
are **no longer dead fields** — Task 3's stated reason for the first was wrong
about Kestra and is corrected in place; **`timeout:` is not implemented and never
will be by `advance()`**, because the artifact enforces it in the *worker*; the
feed table moved again, to `575 fed / 55 spawner / 108 flowable / 102 unplanned`;
`RunState` gained `RETRIED` and now holds **8** of the golden's 13 states; the
sub-execution hole is **38 of 41**, not 33, because the four
`retry-new-execution-*` entries now reproduce everything except the replacement
execution; and the double `taskRunList` entry the spec recorded for a retried
task **does not reproduce** — 2050 captured execution records holding 4215
taskruns contain not one duplicate. See *What Task 7 had to change, and what it
banked*.

**Task 7's fix round 1 answered six mutations that killed nothing**, and a
reader should take these over the round-0 sentences they replace. Two guards
Kestra has and this executor cannot reach — `handleEnd`'s and the retry pass's
`isRetrying` early returns — were **deleted** rather than shipped as comments;
one was made **unrepresentable** (a `NonZeroU32` in place of an `attempts == 0`
check, so the mutation is unwritable rather than unkilled); three were kept and
made reachable by tests written for them; and one is *provably* equivalent on
this corpus and now carries the derivation that says so. The replay itself was
killed mid-mutation once, by a `head -20` on its own output, and it was the
harness's poison check rather than the discipline that caught the file it left
behind.

Task 6 (`loop`, `Command::SpawnExecution`) changed five, and a reader should
take these over everything below: the feed table gained a **fourth bucket**
(*spawner*) and moved to `565 fed / 55 spawner / 108 flowable / 112 unplanned`,
which is 231 runs and more movement than every earlier task combined; **a
`Loop`'s own terminal state is now FED, not compared**, and the sentence saying
otherwise is corrected in place; the sub-execution hole is still open and is now
stated with three numbers rather than one (**41 entries, 142 sub-executions, 33
of the 41 differing on nothing else**); the first batch of spawn *requests* IS
compared, corpus-wide, 52 of the 142; and `Loop` left *States and constructs no
W3a task claims* while `LoopUntil`/`WaitFor` gained a row there. See *What Task
6 had to change, and what it banked*.

**Task 6's fix round 1 corrected four of its own sentences**, and a reader
should take these over anything round 0 said. Two are in the implementation's
favour: **nothing moved from compared to fed** — a `Loop` was already an opaque
`Runnable`, so Task 6 relabelled 40 already-fed terminals and *removed* 40
spurious `SUBMITTED` emissions — and the loop's aggregation rule is **not**
"judged by nothing", only invisible to the ratchet; two mutations prove it and
both drop the hole test from 33 to 23. Two are against it: the two entries round
0 blamed wholly on the narrow render context are blocked **twice over**, the
second cause being **a third `values:` shape (an ION file URI) that `advance()`
does not implement at all** — a named gap in the executor, not a harness
limitation; and the STUCK population that justifies the feed is **35**, not the
37 round 0 wrote in prose, now derived and pinned.

Task 5 (`if`, `switch`) changed four, and a reader should take these over
this file's older sentences: the feed table below moved again, to
`424 fed / 73 flowable / 343 unplanned`; the oracle's environment grew a
**fourth tier** — the caller renders, because `advance()` cannot — and with it a
new and narrow dependency on W2's renderer; the "kept `before` may be
unreachable without a clock" hole gained its **second** named entry,
`valids/if`; and `If`/`Switch` left *States and constructs no W3a task claims*
while `core.execution.Exit` arrived in it. See *What Task 5 had to change, and
what it banked*.

**Task 5's fix round 1 corrected three sentences of its own**, and a reader
should take these over anything round 0 said: `is_truthy` trimmed with Rust's
`str::trim` where Kestra trims Java's `String.trim()` — **11 measured rows
disagree, in both directions** — so the predicate changed and 18 rows were added
to the committed table; the banked table claimed `valids/if-without-else` was
earned by an `else` branch, when its golden is the `then` branch and **no banked
entry takes an `else` at all**; and "both shapes of the render tier are
invisible to this oracle, 182 mismatching" was wrong twice — the count was
stale, and **both variants fail a committed oracle test** even though the
ratchet's count does not move.

Task 4 (`parallel`, `dag`) changed four more, and they are the ones a reader
should not take from this file's older sentences: the feed table below moved to
`428 fed / 48 flowable / 364 unplanned`, the ratchet now compares the ordered
pairs the **golden** carries rather than every pair egret can produce, the
"kept `before` may be unreachable without a clock" hole under *What this oracle
does not prove* stopped being hypothetical and is now one named pair, and the
design doc's sentence "one bad sibling failed all three" is measurably too wide.
See *What Task 4 had to change, and what it banked*.

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

**Fed since Task 6, and this is a CORRECTION to the sentence below: a `Loop`
run's terminal too.** The paragraph that follows used to name `Loop` among the
flowables whose state is compared, and on measurement it cannot be. A `Loop`'s
state is a function of its **sub-executions**, and this oracle runs none —
`Command::SpawnExecution` is emitted and nothing executes it until W3b/W6 — so
there is no decision `advance()` could take here for the oracle to judge.

**Task 6's own first account of that trade was too pessimistic, and the
correction is in the implementation's favour.** No terminal moved from
*compared* to *fed*. At Tasks 3-5 a `Loop` was planned as an opaque `Runnable`,
so `golden_outcome` **already** supplied its terminal — and `advance()` also
emitted a `SUBMITTED` on it, which Kestra never does for a flowable. So Task 6
relabelled terminals that were already fed and **removed 40 wrong emissions with
them**. `planning_the_loop_removed_forty_spurious_submitted_emissions` derives
the 40 by rebuilding Task 5's classification from today's plan, and is validated
by reproducing all four of Task 5's committed figures (`424 / 73 / 343`, 114
over-submitted) before it reads the number off.

The alternative to feeding was still not "compare it": it is `STUCK`, which the
baseline does not cover and cannot.
`the_loop_completion_tier_is_load_bearing_for_exactly_these_entries` pins that
population at **35** entries — 39 comparable entries hold a `Loop`, two never
start one for want of values and two more because their `values:` cannot be
resolved at all — and requires all 35 to be baselined today. (Task 6's first
report said 37, in prose, beside a derivation and pinned by nothing. It is 35.)

What it costs is narrower than the first account said too. **`transmitFailed`
and the rule by which a `Loop` aggregates its iterations are invisible to the
RATCHET, and two committed tests see them anyway** — so "judged by nothing in
W3a" was wrong. Two independent wrong-aggregation constructions —
`Step::Done(Success)` for a `Loop` (M66) and a tier-3 override feeding `Success`
with the spawn schedule left identical (M67) — each die to
`the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` and to
`what_the_order_restriction_removes_is_derived_and_egret_never_says_after`. The
first is the load-bearing one: its **33** falls when a loop's terminal stops
matching Kestra's, so that number is a **guard** and not only a report, and a
later task re-deriving or deleting it should know that.
`valids/loop-failed-no-transmit` — three iterations that all FAILED and a loop
Kestra ended `SUCCESS` — is still reproduced by feeding rather than by deciding,
and the 55 runs this covers are counted in a bucket of their own so they cannot
be read as compared.

**Not fed, and therefore compared:** which runs are created, in what order,
under which parent, with what `value`; every state in a run's history *except*
its last; the whole of a **flowable's** history including its last; the
execution's own history; the happens-before relation. **The sub-executions are
neither fed nor compared** — egret's side of `spawned` is empty, which is not
the same as agreeing; what *is* compared about them is the first batch of spawn
*requests*, 52 of the golden's 142. Feeding a flowable's state would be circular
in the way that matters — a `Sequential`, `Parallel`, `Dag`, `If` or `Switch`
run's state is the executor's decision and is exactly what this oracle exists to
judge. A `Loop`'s, in W3a, is not.

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
| **fed** | 575 | the run's task is a `Runnable` in the plan. For the **386** at top level its terminal is supplied and is not compared; the other **189** are inside a sub-execution egret never creates, so nothing is supplied to them and nothing is compared about them either |
| **spawner** | 55 | the run's task is a `Loop`. For the **40** at top level, planned and driven here with its terminal **fed anyway** — it is a function of sub-executions this oracle does not run; the other **15** are inside a sub-execution, as above. Task 6's, and the price of the sub-execution hole |
| **flowable** | 108 | the run's task is one the executor drives **and decides for** itself (`Sequential`, `Parallel`, `Dag`, `If`, `Switch`), so its state is compared |
| **unplanned** | 102 | the run's task has no `PlanTask` at all — it lives inside a flowable `build_plan` still plans opaque and so does not descend into |
| **TOTAL** | **840** | |

Task 3's figures were `390 / 23 / 427`; Task 4 moved 63 runs out of *unplanned*,
38 of them into *fed* and 25 into *flowable*, reaching `428 / 48 / 364`. Task 5
took them to `424 / 73 / 343` — *flowable* **+25**, *unplanned* **−21**, *fed*
**−4**, which sums to zero as it must. `fed` moved by only 4 because two
opposite things happened to it at once: every `If` and `Switch` run left it (a
flowable's state is its own decision, so it is compared and not fed) while the
`Runnable` children of those same tasks arrived in it for the first time.

**Task 6 moved 231 runs, which is more than every earlier task put together**,
and the cause is not that `Loop` is a big flowable. It is that planning a `Loop`
gives every task **inside a loop body** a `PlanTask` for the first time, and
those tasks are what the golden's sub-execution runs are runs *of*. So
*unplanned* went **343 → 112**, *fed* **424 → 565**, *flowable* **73 → 108**, and
the new *spawner* bucket took **55**; 141 + 55 + 35 = 231, which is exactly the
fall in *unplanned*.

**None of those 231 runs is simulated by egret**, which is why the two fed-ish
descriptions above now carry a split rather than a single count: **189** of the
565 *fed* and **15** of the 55 *spawner* runs live inside a sub-execution egret
never creates, and for them nothing is fed and nothing is compared. This table
is a classification of Kestra's runs by egret's plan, not a claim about
coverage. That was always true of it; Task 6 is the first task large enough to
make it matter, and the pair `(189, 15)` is asserted so the descriptions stay
true of every run they cover.

**Task 7 moves exactly 10**, `unplanned` 112 → 102 and `fed` 565 → 575, and
every one of them is the run of a task written inside a **task-level `errors:`**
list — which had no `PlanTask` at all until Task 7 widened W1's model. The flow's
own `errors:` is not among them: `Plan::errors` has been populated since Task 3.
The over-feed does **not** move with them (79, unchanged): an `errors:` task is a
`Runnable` that carries `SUBMITTED`, so it belongs in `fed` on both readings.
`planning_the_loop_removed_forty_spurious_submitted_emissions` derives that 10
rather than folding it into its constants, so its reproduction of Task 5's four
committed figures is still a reproduction and not an agreement with itself.

These five move as Tasks 7-8 teach `build_plan` more, which is why they are
asserted in the test rather than only printed here: a task that changes the
split has to change this table in the same commit.

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

**The over-feed is 79 of the 565, and it is not a single accident.**
The by-type breakdown is derived from the plan's own `type_id` and printed on
every run — **11 task types** after Task 6 (12 after Task 5, 13 after Task 4, 15
before it; `core.flow.Parallel` and `core.flow.Dag` left in Task 4's commit,
`core.flow.If` and `core.flow.Switch` in Task 5's, `core.flow.Loop` in Task
6's), of which **6 are still flowables (54 runs)**, **4 are
`io.kestra.plugin.core.execution.*` (23 runs)** that **no W3a task claims**, and
1 is the `Log` exception above — 6 + 4 + 1 = 11 types and 54 + 23 + 2 = 79 runs,
asserted as arithmetic beside the parts rather than written next to them. See
*States and constructs no W3a task claims*.

The flow bucket's fall from 89 to 54 is **not** 35 runs of one type leaving, and
Task 6's first account of it named four types that did not move. Reconstructed
from today's plan and validated against Task 5's own totals, exactly three
counts changed:

| type | Task 5 | Task 6 | why |
|---|---|---|---|
| `core.flow.Loop` | 40 | — | left the set entirely: it has a `TaskKind` now |
| `core.flow.Subflow` | 22 | **25** | `valids/loop-with-subflow`'s `Subflow` is inside a loop body |
| `core.flow.WorkingDirectory` | 2 | **4** | the same, one level over |
| `AllowFailure` / `Pause` / `WaitFor` / `LoopUntil` | 4 / 12 / 6 / 3 | unchanged | already over-submitted at Task 5; Task 6 only *named* two of them here |

−40 + 3 + 2 = −35, and the +5 is the counter-example this file has now recorded
three times: **implementing a flowable enlarges the over-feed for whatever nests
inside it.**

**Implementing a flowable ENLARGES the over-feed for whatever nests inside it,
and it has now happened three times.** Task 4 measured `If` 12 → 19 and
`Subflow` 18 → 22 when `Parallel` and `Dag` landed. Task 5 is the same shape one
level down: `core.execution.Exit` was not in the set at all before it and
contributes 4 runs now, because `Exit` lives inside an `If` and planning the
`If` is what gives its children a `PlanTask`. Task 6 is the third: `Subflow`
22 → 25 and `WorkingDirectory` 2 → 4, five runs between them. A run
that was `unplanned` (its parent was opaque) becomes a fed `Runnable` the moment
its parent is planned. Tasks 7-8 should expect their own numbers to go up before
they come down.

**Nothing banked rests on any of this** — re-derived at Task 5, and it is still
exactly zero: of the **168** runs in the banked entries, **none** is a planned
`Runnable` whose golden history lacks `SUBMITTED`. See *What Task 3 had to
change, and what it banked* for the intersection Task 3 established.

**Task 6 did not re-derive the 168, and does not adjust it by argument.** It
banks two entries, so the figure has certainly moved; how far is a measurement
nobody has taken since Task 5, and an arithmetic guess here would be one more
number written down beside a derivation. Whoever next runs that test should take
its output over this sentence. What Task 6 *can* say without measuring is that
the property is unthreatened by its own two entries: a `Loop` run is no longer a
planned `Runnable` at all, so it cannot be in the set the property is about.

What makes that acceptable is not the argument, it is that `golden.json` is
machine-derived and re-derivable: `normalise.py` at HEAD reproduces both it and
`stability.json` byte-identically from the committed `traces.json`, and
`--verify-traces` re-reads every captured execution against the live Kestra. The
one field nobody can catch by comparison is the one field nobody hand-writes.

### The environment has four tiers, and the worker tier was an accident

The oracle is a *caller*, and a caller decides when the worker's answers arrive.
`tests/executor.rs` does it in four tiers, strictly in this order:

1. **`advance()` decides.** If it emits commands they are applied and it is
   asked again. Nothing else happens while it still has work.
2. **The caller renders** (Task 5). Only when `advance()` returns nothing: every
   planned `If`/`Switch` run that is `Running` and unrendered gets its
   expression rendered into `TaskRun::rendered`. A render that FAILS fails that
   run. See *What Task 5 had to change* for what the caller can render and what
   the narrowness costs.
3. **The worker finishes ONE task.** Only when tiers 1 and 2 have nothing to do:
   the earliest-created `Runnable` still `Running` reaches the terminal state the
   golden recorded **for its current attempt**.
4. **Time passes.** Only when nothing else can happen: `now` jumps to the
   earliest `not_before` still ahead of it.

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
unrepresentable. **Task 3 added `SUBMITTED`, Task 7 added `RETRIED`**, so the
enum has eight and **5 of the 13 remain unrepresentable**: `PAUSED`, `RESTARTED`,
`WARNING`, `SKIPPED`, `CANCELLED`. Each is recorded here rather than worked
around in the normaliser, because a normaliser that quietly maps an
unrepresentable state onto a representable one is a normaliser that hides the gap
it was built to expose.

**What each addition bought, and what the five still cost.** Of the 266 `runs`
entries, **35** use only the six members the plan drafted, **229** use only the
seven and **233** use only the eight — so `SUBMITTED` alone is what moved the
reachable set by 194 entries and `RETRIED` moves it by 4,
and it was the single cheapest thing any task could do to this oracle. It is
also not optional for Task 3's own slice: `SUBMITTED` is in 580 of the golden's
840 run histories and on every task the worker ran, so a sequential flow of
`Return` tasks cannot match without it.

**And `RETRIED`'s 4 buy no bank at all**, which is worth saying beside the
number: the four entries it covers are exactly the `retry-new-execution-*`
flows, and every one of them carries a **replacement execution** in `spawned`
that egret cannot create. The state was still worth adding — without it those
four are `FAILED` where Kestra recorded `RETRIED`, which is a wrong answer rather
than a missing one — but it moved them from *wrong on several fields* to *right
on everything except the field nothing produces*. See the sub-execution hole.

The other five, with the number of `runs` entries whose trace uses each and the
task that owns it:

| state | `runs` entries | owner |
|---|---|---|
| `PAUSED` | 18 | `Pause`, which no W3a task claims — see the hole list |
| `RESTARTED` | 12 | execution histories only; a restart is W3b's, not `advance()`'s |
| `WARNING` | 10 | `AllowFailure` and `warningOnRetry` — **not Task 7's**, and this row said it was. `warningOnRetry` turns a retried task's eventual SUCCESS into WARNING, so `valids/retry-success` and `valids/task-allow-failure-runnable` need it; `AllowFailure` is planned opaque and belongs to W8. Unclaimed |
| `SKIPPED` | 4 | `disabled: true`, which no W3a task claims |
| `CANCELLED` | 3 | flow concurrency, which no W3a task claims |

A later task adding one of these adds it here too; the counts are what
`tests/executor.rs::the_state_vocabulary_this_enum_holds_is_derived_from_the_golden`
derives over `golden.json` on every run — they were prose beside a derivation
until Task 7 had to move them.

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

## What Task 4 had to change, and what it banked

Task 4 implements `parallel` and `dag`. It banks **4** of the 193 entries Task 3
left, taking `baseline.txt` to **189**: `valids/parallel`,
`valids/parallel-nested`, and the two `invalids` `Dag` refusals egret used to
**over-accept** — `invalids/dag-cyclicdependency` and
`invalids/dag-notexist-task`.

The control was run first, in the working tree at `e82c8814` with `git status`
clean: `oracle-exec ratchet: 294 golden entries, 193 mismatching, 193 baselined,
0 broken`. Every line banked was one of those 193.

### The ratchet now compares the pairs the GOLDEN carries, not every pair egret can produce

This is a change to what the oracle compares, and it is the one thing in Task 4
a reader should check hardest.

`normalise.py` keeps an ordered pair only where all five Kestra rounds agreed,
and publishes the drops — 13 pairs flipped between rounds and are simply absent
from `golden.json`. egret has no such notion: a clock-free state machine
serialises in rounds and produces a relation for **every** pair. So the ratchet
was comparing a vector of 21 against a vector of 20 and calling it a mismatch on
a pair the oracle explicitly declines to assert.

Measured cost of not fixing it: `valids/parallel-nested` mismatched with
**identical runs, parents, histories and attempts**, and with all 52 kept
ordered pairs reproduced exactly. The only difference was 3 pairs the golden had
dropped. `valids/dag` and `valids/if` are in the same position for 1 and 9
pairs.

`Trace::restrict_order_to` removes, from egret's side only, the pairs the golden
has no relation for. **The predicate and its endpoints are untouched** — creation
on the left, termination on the right, strict on both sides, still pinned by
`an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended`
against `simulate_with`'s raw output — and a pair the golden *does* carry that
egret does not produce still mismatches, because the projection only ever
removes.

**Removing is equivalent to ACCEPTING, and that is what makes it not a
weakening.** egret's relation domain is measured, not assumed: over all
comparable entries it produced **501 ordered pairs and zero `after`** at Task 4
(555 after Task 6, **656** after Task 7; the zero has never moved). A pair is
dropped from the golden precisely because Kestra flipped on it between rounds,
which over a two-valued domain means Kestra was *observed producing both
values*. So "drop the pair" is today exactly "accept either relation Kestra was
seen to produce". And "egret must not produce one" is not a coherent
alternative, because egret emits a relation for every pair by construction. The
equivalence holds only while `after` stays unreachable — it is unreachable
because a run's left endpoint is its creation step and creation is monotonic in
run order — so it is asserted rather than argued. Task 6's spawned executions
were the thing named as able to break it, and they do not: a `Loop` creates no
run for its body at all, so it adds no creation that could land out of order.
The population is **656** after Task 7 (555 after Task 6), and the assertion of
zero `after` is unchanged. Task 7 adds 101 pairs and every one of them is made
by an `errors:` run — a run that exists is in a pair with every other run of its
execution — so the growth is a new *population*, not a new relation.

**The cost, stated and demonstrated rather than implied.** On the dropped pairs
egret may report *anything*. That is shown on `valids/parallel-nested`, a
**banked** entry: rewriting its relation on exactly those 3 pairs to the
impossible `after` leaves the comparison equal.

Three committed tests bound it:

- `restricting_the_order_cannot_hide_a_disagreement_on_a_kept_pair` — two
  machines that differ on a kept pair still mismatch; the control, the same pair
  dropped, does not.
- `the_golden_has_no_relation_for_exactly_these_pairs` — the golden-side
  population, derived: **13 pairs across 3 entries**, `valids/dag` 1,
  `valids/if` 9, `valids/parallel-nested` 3. That is the *same* 13
  `stability.json` records as dropped-for-flipping, in the same three flows;
  all three reached `golden.json`, so there is nothing to reconcile.
- `what_the_order_restriction_removes_is_derived_and_egret_never_says_after` —
  what the projection actually removes when the real `advance()` runs, which is
  **22 across 6 entries** and is the number the ratchet prints. The 9 beyond the
  golden's own 13 are on `valids/disable-simple` (2),
  `valids/parallel-disabled-tasks` (7) and `valids/sequential-with-disabled` (2)
  — all `disabled:` flows whose egret run set *differs* from Kestra's, so their
  pair indices do not denote the same pairs and such an entry can never bank
  anyway (`runs` is compared first and verbatim). The test asserts that
  separation, so an entry losing pairs while its runs DO match shows up as a
  changed map.

**An earlier version of this section said 12 and offered a reconciliation for
the missing one.** Both were wrong: the map sums to 13, and the test that
claimed to bound the projection never called it. The map was derived and the
total beside it was written down — this package's recurring defect, recorded
here as the fourth instance.

### `valids/dag` is NOT banked, and it is the hole this file predicted

Everything about it reproduces except **one ordered pair**, `(task3, task4)`.
The golden says `before`; egret says `overlapping`.

Kestra's `resolveDagNexts` filters on "all dependencies are terminated", and
`task4` depends only on `task2`. So Kestra's own code creates `task4` the moment
`task2` terminates, whether or not `task3` has. The five harvest rounds
nonetheless all recorded `task3` ending before `task4` was created — both are
`Log` tasks that finish in milliseconds, and Kestra's executor saw both
terminations in one evaluation. egret's environment completes exactly one
in-flight run per round, so it cannot: it sees `task2` finish, creates `task4`,
and only then sees `task3` finish.

This is *What this oracle does not prove*'s fifth bullet arriving: "Where
Kestra's scheduler happened to serialise two branches the same way five times
running, the golden records `before` and egret will say `overlapping` — a
mismatch that is about the instrument, not about the flowable. Tasks 4 and 6 are
the ones that will find out, and the honest answer if it happens is to record
the pairs, not to weaken the relation." It happened, it is one pair, and it is
recorded — here, and in `runnable.txt`. The alternative that would bank the
entry is a `Dag` that waits for every in-flight sibling before creating the next
batch, and that is a worse implementation, not a fix: it would make one slow
branch block an independent short branch's successor, which is observable and
wrong.

### "One bad sibling failed all three" is too wide, and the golden says so

`docs/en/specs/2026-09-04-workflow-w3a-executor-design.md` §4 records, from the
spike, that a validation error in one parallel branch (`c_fast`) left `a_slow`
and `b_mid` `FAILED` as well. The shape reproduces in the corpus:
`valids/parallel-fail-with-flowable`'s branch holding a `Sleep: PT1S` ends
`FAILED`, and `Sleep` succeeds in every other flow here, `PT30S` included
(`valids/sequential-sleep`).

But the sentence as written is false, and the same golden refutes it.
`valids/finally-parallel`, driven with `{"failed": true}`, is a `Parallel` whose
four branches end `SUCCESS, SUCCESS, SUCCESS, FAILED` — the `Parallel` itself is
`FAILED` and three siblings survived. `valids/finally-dag` is the same shape
(`t1` and `t2` `SUCCESS` beside a `FAILED` `t3`).

What separates them is *when*. A sibling that had already terminated keeps the
state it terminated with; a sibling still in flight does not survive. That is
the rule `parallel.rs::fail_the_rest` implements, and both halves are asserted
by `one_invalid_sibling_fails_every_sibling` — the spike's half, and the bound
`finally-parallel` puts on it.

**Neither half is reachable by this oracle**, and that is worth saying plainly:
the runs it touches are `Runnable`s whose terminal state is *fed* from the
golden. The unit test is the only thing holding it.

### A guard that was written and then deleted — and the reason is about the CALLER

`resolveParallelNexts` returns nothing at all while any existing child run is
still `CREATED` — Kestra's "first created, leave". It was ported, and then the
mutation that bypasses it (`if !any_created || true`) left the whole crate at
**447 passed / 0 failed** with the ratchet unmoved. A guard nobody has seen fail
is a guard nobody has seen, so it was deleted.

**Deleting it is right for today's only caller. It is not a property of
`advance`.** The first version of this section said the condition was
unreachable "in a round-driven executor", which reads as a fact about the state
machine, and `advance` is a pure function of the state it is handed — any caller
may hand it any state. Instrumenting the exact decision point (a batch about to
be created while an existing child run of the same branch is still `CREATED`)
over all 294 golden entries measures:

| driver | occurrences |
|---|---|
| the oracle's environment — never applies a worker result while `advance` has work | **0** |
| one worker result applied **between** `advance` calls | `valids/dag` 1 batch, `valids/if` 3 |

Nothing about a branch has to change for the second row; only the *delivery
discipline* does. So the trigger to watch for is **W4's worker**, which is
message-driven by construction — not a later flowable, which is what the first
version of this section wrongly named. Whoever gives `advance` a caller that can
interleave a completion with a decision has to bring the guard back, and bring
the test that makes it fire with it.

### Task 4's mutations

Each was applied, **built and asserted to compile with 0 errors**, then run
against the whole crate (`cargo test -p egret-workflow --no-fail-fast`) — never
a name-filtered subset. Restore is `shutil.copyfile`, which stamps a fresh
mtime; `tar xf`, `cp -p` and `rsync -a` all preserve mtimes and would leave
cargo rebuilding nothing, and `cp` is aliased to `cp -i` in this shell, which
silently refuses to overwrite. Restoration was verified with `cmp` against the
snapshot for all eight files.

**The literal edit is in the table, and the victim counts are re-derived from a
replay of the final tree.** The Task 3 section below says why, and Task 4's
first attempt reproduced the same defect it warns about: a table carrying only
descriptions was replayed by a review and gave a different number for M16 — the
description "fails every sibling" admits several edits and they do not kill the
same set. Every row is now the exact `-` → `+`.

Fifteen rows are counted here as thirteen replayed plus two recorded: **M14**,
whose anchor no longer exists because the guard it bypassed was deleted, and the
Task-4 re-run in which every count below was measured.

| # | file | `-` | `+` | what died |
|---|---|---|---|---|
| M13 | `parallel.rs` | `.take(room)` | `.take(room.min(1))` | 3 unit tests: `parallel_creates_every_child_before_starting_any`, `a_parallel_concurrency_cap_bounds_the_batch`, `one_invalid_sibling_fails_every_sibling`. ``a Parallel creates every child in ONE round … — left: [CreateRun { task: 1, .. }] right: [… six CreateRuns …]`` |
| M14 | `parallel.rs` | `if !any_created {` | `if !any_created \|\| true {` | **nothing** — 447 passed, 0 failed, ratchet unmoved. See *A guard that was written and then deleted*; the anchor is gone from the final tree, so this row is a record and not replayable |
| M15 | `parallel.rs` | `let cmds = fail_the_rest(state, &runs);` | `let cmds: Vec<Command> = Vec::new();` + a discarding call | 1: `one_invalid_sibling_fails_every_sibling`. ``a_slow and b_mid were still in flight when c_fast failed, so both end FAILED — left: [] right: [SetRunState { run: 1, state: Failed }, SetRunState { run: 2, state: Failed }]`` |
| M16 | `parallel.rs` | `.filter(\|&ix\| !is_terminal(state, ix))` | `.filter(\|&ix\| latest(state, ix) != Some(RunState::Failed))` | 2: `one_invalid_sibling_fails_every_sibling`, `a_parallel_takes_the_worst_of_its_children`. ``a_slow had already SUCCEEDED and keeps it … — left: [Failed, Failed, Failed, Failed] right: [Failed, Success, Failed, Failed]`` |
| M17 | `dag.rs` | `node.depends_on.iter().all(\|&dep\| { run_for(dep).is_some_and(…Success) })` | `true` | 3: `a_dag_creates_a_node_only_when_every_dependency_has_succeeded`, `a_dag_creates_nothing_downstream_of_a_failed_dependency`, `what_the_order_restriction_removes_…`. ``Kestra's taskRunList for valids/dag, verbatim — left: [… task4, task5, task6] right: [… task4, task6, task5]`` |
| M18 | `plan.rs` | `if !cyclic.is_empty() {` | `if false && !cyclic.is_empty() {` | 2: `a_cyclic_dag_is_refused_by_name` and **the ratchet**. ``a cyclic dag must be refused; got a plan of 6 task(s) rooted at 'dag'``; ``PANIC … invalids/dag-cyclicdependency: STUCK after 2 step(s) … run 0 (dag) is Some(Running)`` |
| M19 | `plan.rs` | `if !unknown.is_empty() {` | `if false && !unknown.is_empty() {` | 3: `a_dag_dependency_on_a_task_that_is_not_a_node_is_refused_by_name`, `build_plan_survives_every_corpus_flow`, **the ratchet**. ``flatten_dag resolves dependsOn only after the not-a-node check above has passed``; ``PANIC … invalids/dag-notexist-task`` |
| M20 | `parallel.rs` | `Some(cap) => (cap as usize).saturating_sub(in_flight),` | `Some(cap) => usize::MAX.saturating_sub(cap as usize * 0 + in_flight),` | 1: `a_parallel_concurrency_cap_bounds_the_batch`. ``\`concurrent: 2\` creates two of the three children in the first batch — left: 3 right: 2`` |
| M21 | `tests/executor.rs` | `self.order.retain(\|(i, j, _)\| keep.contains(&(*i, *j)));` | `self.order.clear();` | 3: `restricting_the_order_cannot_hide_a_disagreement_on_a_kept_pair`, `what_the_order_restriction_removes_…`, **the ratchet**. ``nothing to drop when the golden carries the pair — left: 1 right: 0``; ``REGRESSION: 20 entr(y/ies) mismatch that were NOT in baseline.txt`` |
| M22 | `parallel.rs` | *(end of rule 3's loop body)* | `if !cmds.is_empty() { break; }` | 1: `one_invalid_sibling_fails_every_sibling`. ``all three branches must be in flight before one of them fails: [Some(Running), Some(Running), Some(Created), Some(Created)]``. **It does NOT kill `a_parallel_drives_every_branch_in_the_same_round`** — see M26 |
| M23 | `parallel.rs` | `Some(Killed) => return Killed, Some(Failed) => out = Failed,` | both arms `=> out = RunState::Success` | 3: `a_parallel_takes_the_worst_of_its_children`, `one_invalid_sibling_fails_every_sibling`, `a_dag_creates_nothing_downstream_of_a_failed_dependency`. ``the Dag reports the failure instead of waiting for runs that will never be created — left: ["CREATED","RUNNING","SUCCESS"] right: ["CREATED","RUNNING","FAILED"]`` |
| M24 | `parallel.rs` | `if every_run_terminal && (created.len() == children.len() \|\| ended_badly) {` | `if every_run_terminal && created.len() == children.len() {` | 2: `a_dag_creates_nothing_downstream_of_a_failed_dependency`, **the ratchet**. ``left: ["CREATED","RUNNING"] right: ["CREATED","RUNNING","FAILED"]``; ``PANIC … invalids/dag-invalid-subtask: STUCK after 5 step(s)`` |
| M25 | `plan.rs` | `Ok(0) \| Err(_) => None,` | `Err(_) => None,` | **9**, including the ratchet. ``an absent \`concurrent:\` is Kestra's default 0, which is unlimited — left: Parallel { … concurrency: Some(0) } right: … concurrency: None``; ``PANIC … valids/finally-parallel: STUCK after 2 step(s)`` |
| M26 | `parallel.rs` | *(end of rule 3's loop body)* | `if !is_terminal(state, ix) { break; }` | 4: `a_parallel_drives_every_branch_in_the_same_round`, `one_invalid_sibling_fails_every_sibling`, `what_the_order_restriction_removes_…`, **the ratchet**. This — serialising the branches — is what guards the every-branch property; M22 is not |

**M22 and M26 look alike and are not.** M22 stops after the first branch that
*wants something*; the branches are still visited in order on every round, so a
branch catches up as soon as the one before it is waiting and the run list is
unchanged. M26 stops at the first branch that has not *terminated*, which runs
branch one to completion before touching branch two and changes
`taskRunList` to `parallel, sequence1, sequence2, task1, task2, task3, task4`.
Task 4's first report named M22 as the guard for
`a_parallel_drives_every_branch_in_the_same_round`, and that test survives M22.

M18 and M19's assertion messages were **rewritten because of what the mutation
showed**: `expect_err` printed the whole `Plan` and buried the finding — Task 3's
M6 lesson arrived at from the same direction. Both now print the task count and
root id.

### A finding Task 4 did not need but Task 8 does

`build_plan` calls `spec::parse` and **not** `spec::validate`, and W1's
`validate` already carries rules for everything Task 4 added to the builder —
`dag_cycle`, `dag_dependency_not_found`, `dag_node_task_required`. So egret's
HTTP publish path was already refusing `invalids/dag-cyclicdependency`; only the
executor's builder over-accepted it.

Measured over the 20 `refused` entries still baselined: **8 of them would be
refused today if `build_plan` ran `validate`** — `invalids/duplicate` and
`invalids/duplicate-parallel` (`duplicate_task_id`), `invalids/duplicate-inputs`
(`duplicate_input_id`), `invalids/empty` and `valids/invalid-draft-flow`
(`empty_tasks`), `invalids/if-without-condition` (`if_condition_required`),
`invalids/recursive-flow` (`self_subflow`), `invalids/workingdirectory-no-tasks`
(`working_directory_tasks_required`) — and the other 12 would not. That is
Task 8's decision, not this task's: it changes what the executor refuses for
every flow, and Task 3's M8 is the recorded argument for not widening refusal
casually.

## What Task 5 had to change, and what it banked

Task 5 implements `if` and `switch`. It banks **8** of the 189 entries Task 4
left, taking `baseline.txt` to **181**:

| banked | what earns it |
|---|---|
| `valids/if-condition` | `If`, the branch taken on a truthy render |
| `valids/if-without-else` | `If`, the branch taken on a **truthy** render — `corpus.json` posts `param: true` for it, so its golden is the `then` branch. (Fix round 1: this row said "falsy render → the empty `else`" and that is the one thing nothing banked earns; see *No banked entry takes an `else` branch*.) |
| `valids/switch` | `Switch`, the case whose key equals the rendered value |
| `valids/fail-on-switch` | `Switch` with no matching case and no `defaults`: `FAILED` |
| `valids/if-condition-fail` | the caller's render FAILS and the run fails with it |
| `valids/variables-invalid` | the same, on a `Switch` |
| `valids/parallel-fail-with-flowable` | Task 4's `fail_the_rest`, finally reachable — the `If`s in its two branches are planned, and the flow's declared input `defaults:` are what make their condition render |
| `invalids/if-without-condition` | an `If` with no `condition` cannot be planned — an **over-acceptance** fixed |

The control was run **before any code**, in the main tree at `a5069876` with
`git status --porcelain` empty:

```text
oracle-exec ratchet: 294 golden entries, 189 mismatching, 189 baselined, 0 broken,
  22 ordered pair(s) removed
```

and after:

```text
oracle-exec ratchet: 294 golden entries, 181 mismatching, 181 baselined, 0 broken,
  25 ordered pair(s) removed
```

Every one of the 8 was among the 189, measured mismatching immediately before
the code that earns it. The ratchet fails in both directions, so the 8 deletions
are neither more nor fewer than what changed.

### `If`'s truthiness, measured against the running artifact

The plan says to measure this rather than choose it. `TruthUtils.java` was read
first — the sibling checkout is authoritative for reading an implementation —
and then the rule was **measured against the running artifact before the
predicate was written**, because the checkout and the pinned image have drifted
before and a read is not a measurement.

**Twenty flows**, each an `If` whose `condition` is a **literal** — so the
rendered value is exactly the literal, and the measurement isolates the
predicate from the renderer — published to `kestra/kestra:v2.0.0-rc13` and
executed, recording which branch got a taskrun:

| rendered condition | branch Kestra took |
|---|---|
| `"true"`, `"1"`, `"00"`, `"0.0"`, `"abc"`, `"FALSE"`, `"False"`, `"null"`, `"no"`, `"[]"`, `"{}"`, `"true\n"`, `"  true  "` | `then` |
| `"false"`, `"0"`, `"-0"`, `""`, `"  false  "`, `" 0 "`, `"   "` | `else` |

So the rule is: **falsy is exactly one of the four strings `false`, `0`, `-0`,
`""`, compared after trimming; everything else is truthy.** It matches
`core/src/main/java/io/kestra/core/utils/TruthUtils.java` verbatim
(`condition != null && !List.of("false","0","-0","").contains(condition.trim())`),
which is what a reader would hope, but three GROUPS of rows are there because
nothing but a measurement settles them:

- `"00"` and `"0.0"` are **truthy**. Any predicate that parses a number gets
  these wrong.
- `"FALSE"`, `"False"`, `"no"`, `"null"`, `"[]"`, `"{}"` are **truthy**. It is
  not a case-insensitive match and not a "looks empty" test.
- `"  false  "`, `" 0 "` and `"   "` are **falsy**. The comparison is on the
  trimmed value, and `"   "` trims to `""`.

`kestra_truthiness_is_four_exact_strings_compared_after_trimming` in
`executor/mod.rs` carries all 38 rows as assertions, driven through
`valids/if-condition` — Kestra's own flow — so each row is the branch a real
corpus flow takes. Five mutations die to it (M27, M28, M29, M44, M45), and two
of them — M27, which swaps the trim back to `str::trim()`, and M45, which is
`parse::<bool>().unwrap_or(true)` — die to **almost nothing else**: M27 to
nothing at all, M45 to one further test.

**And Kestra's own Javadoc on `If.condition` is wrong**, which is a third reason
not to take the rule from a reading: it says "Boolean coercion allows 0, -0,
**null** and '' to evaluate to false", and a rendered `"null"` takes `then`.
There is no `null` in `FALSE_VALUES`.

#### `condition.trim()` is not `str::trim()` — 11 measured disagreements

The 20 rows above pin *that* there is a trim. **They cannot pin WHICH trim**,
because every padded row among them pads with an ASCII space, where every
candidate agrees — so round 0 transliterated `condition.trim()` to
`rendered.trim()` and shipped a silent wrong branch. A review asked the
question; 18 more flows were published and executed to answer it.

Java's `String.trim()` strips leading and trailing code units **`<= U+0020`**.
Rust's `str::trim` strips Unicode **`White_Space`**. Neither set contains the
other, so the two differ in **both** directions:

| rendered | Kestra | `str::trim()` would give |
|---|---|---|
| `"\u{20}false\u{20}"`, `"\u{9}…"`, `"\u{A}…"`, `"\u{D}…"`, `"\u{B}…"`, `"\u{C}…"` | `else` | `else` — **agree**, both sets hold these |
| `"\u{1}false\u{1}"`, `"\u{E}false\u{E}"`, `"\u{1F}false\u{1F}"`, `"\u{1}0\u{1}"` | `else` | `then` — Rust does not strip C0 controls |
| `"\u{A0}false\u{A0}"` (NBSP), `"\u{85}…"`, `"\u{3000}…"`, `"\u{2007}…"`, `"\u{2028}…"`, `"\u{A0}0\u{A0}"`, `"\u{A0}"` | `then` | `else` — Rust strips these, Java does not |
| `"\u{21}false\u{21}"` | `then` | `then` — **agree**, in neither set |

**11 of the 18 disagree** — 4 where Java trims and Rust does not, 7 the other
way. The predicate is now
`rendered.trim_matches(|c: char| c <= '\u{20}')`, which agrees on all 38 rows,
and the test asserts the **set** of 11 disagreeing rows by name rather than a
count, so a revert to `.trim()` fails there instead of taking a wrong branch on
an NBSP. `U+000B` and `U+000C` are in both sets and agree, which is why a
smaller probe would have missed this entirely.

Nothing in the corpus reaches it, so no baselined entry moves; an NBSP in a
rendered data value is not exotic, which is the whole reason it is fixed.

### A `Switch` FAILS where an `If` SUCCEEDS, and both halves are in the golden

The two flowables look like one rule and are not.

- An `If` whose chosen branch is empty ends **`SUCCESS`** with no child run.
  `valids/if`, driven with `if2: false`, carries `if-2` as
  `CREATED RUNNING SUCCESS` and no child; `valids/if-without-else`'s measured
  alternative (`param: false`, `probe.json`, `IfTest.java:127`) is one taskrun
  and `SUCCESS`. Java: `If.resolveState` wraps `childTasks()` in
  `ListUtils.emptyOnNull` and short-circuits an empty list into
  `guessFinalState(null, …)`.
- A `Switch` with no matching case and no `defaults` ends **`FAILED`**.
  `valids/fail-on-switch` (`param: fail`) and `valids/switch-impossible`
  (`string: impossible`) are each one run, `CREATED RUNNING FAILED`. Java:
  `Switch` has no `resolveState` override, `resolveTasks(null)` is `null`, and
  `FlowableUtils.resolveState` logs "No task found on flow" and answers
  `FAILED` for a null list.

**And an empty MATCHED case is a third thing: it hangs.** Measured live rather
than inferred — a flow with `cases: {MATCH: []}` and a non-empty `defaults`, run
with `value: "MATCH"`, was still `RUNNING` after 45 s with only its own taskrun.
An empty matched list is still a `findFirst()` hit, so `defaults` is never
reached, and an empty-but-non-null list reaches `resolveState`'s last branch and
returns `Optional.empty()`. `drive_switch` reproduces it as `Step::Waiting` for
ever; no corpus flow reaches it, so only a unit test holds it.

**What egret cannot represent, measured.** W1's `models::Switch::defaults` is a
bare `Vec<Task>`, so an absent `defaults:` and an explicit `defaults: []` arrive
identically — and on the artifact they differ: with no matching case,
`defaults: []` leaves the `Switch` `RUNNING` for ever (same probe) where an
absent `defaults:` ends it `FAILED`. Nothing in the corpus writes
`defaults: []`, and `defaults: null` is refused by W1's parser today (the
over-refusal `a_null_task_list_is_an_over_refusal_that_reaches_the_http_api`
pins), so the two spellings this crate can tell apart are the two that occur.
Whoever fixes that over-refusal owns this paragraph.

Two more things the same probe settled: `then: []` is **refused at publish**
(`422 tasks[if].then: must not be empty`), so only the `else` branch can be
empty in a published flow — and W1's `spec::validate` already has
`if_then_required`, so nothing is owed there; and an explicit `else: []` behaves
exactly as an absent `else`.

**A third null-versus-empty gap, found in fix round 1 and NOT closed.** A
`Switch` with a `value` and **no `cases` key at all** publishes, and its run
**FAILS** (`Switch.childTasks` dereferences a null `cases`); the same flow with
`cases: {}` runs `defaults` to `SUCCESS`. Both measured live. W1's
`models::Switch::cases` is a bare `BTreeMap`, so absent and `{}` arrive here
identically and egret runs `defaults` for both — right for the `{}` spelling and
wrong for the absent one. It is **not** implemented, and deliberately: making an
empty `cases` map answer `FAILED` would fix the absent spelling by breaking the
explicit-`{}` one, and egret cannot tell them apart. It is the same model gap as
`defaults: null` vs `defaults: []`, in the sibling field, and whoever closes one
closes both. No corpus flow is either shape.

### The environment grew a fourth tier, because `advance()` cannot render

`TaskKind::If` and `TaskKind::Switch` hold the author's **unrendered**
expression, and `advance()` reads the caller's answer from `TaskRun::rendered` —
the field Task 3 defined for this and that nothing wrote until now. `None` means
"not answered yet" and the only correct response is to emit nothing; reading it
as falsy would silently take the `else` of every `If` the caller had not reached
(M31 is that mutation, and it kills 8 tests).

That made a new tier unavoidable rather than optional: with `If` planned and
nothing supplying `rendered`, an `If` run waits for ever and the ratchet reports
`STUCK` — which is never covered by the baseline. Deleting the tier today
(M38) panics **19** entries.

**What the caller supplies, and where it comes from.** `inputs`, from
`corpus.json` — the values `harvest.py` posted for the very execution the golden
was captured from — plus the flow's own declared `defaults:` for every input the
harvest did not post; and `vars`, from the flow's `variables:`. The expression
itself comes from the flow's own text via `build_plan`. **Nothing about which
branch Kestra took is read**, so the branch decision stays compared rather than
fed; what it does add is a dependency on **W2's renderer** being right, which is
new and is stated in `tests/executor.rs`'s own feed section.

**The `defaults:` half was a real defect, not a simplification.** Without it,
**seven** planned `If` conditions across five entries (`valids/exit`,
`valids/exit-cancelled`, `valids/exit-killed`, `valids/exit-nested` ×2,
`valids/parallel-fail-with-flowable` ×2) failed to render on flows that post
`inputs: {}` and declare their input with a `defaults:` — 7 + 2 = 9 of the 26
failed; 2 fail with it. `valids/parallel-fail-with-flowable`, the flow Task 4's
whole `fail_the_rest` rule was argued from, banks **because** the defaults are
applied. M42 is the guard.

**A failed render FAILS the run**, which is Kestra's own answer:
`valids/if-condition-fail` renders `{{ inputs.run }}` on a flow declaring no
such input and its golden is one run, `CREATED RUNNING FAILED`, attempt
included. Two entries bank by that path.

### Two guards that could not fail, and what happened to them

- **`drive_if`'s empty-branch early return was deleted.** An
  `if chosen.is_empty() { return Step::Done(Success) }` was written first; then
  `sequential::drive` over an empty list turned out to fall out of its own loop
  with exactly `Done(Success)`, so a mutation removing the early return killed
  nothing at all. It went, and `drive_if`'s doc records why. That is the fourth
  instance of this pattern in the package and the first three shipped.
- **The two refusal tests stopped using `expect_err`.** It prints the whole
  `Plan` and buries the finding — Task 3's M6 and Task 4's M18/M19 arriving a
  third time. Both now print the task count and root id, which is why M35 and
  M36 read `got a plan of 2 task(s) rooted at 'if'` rather than a struct dump.

### `valids/if` is NOT banked, and it is this file's own hole arriving twice

`valids/if` reproduces **all 12 runs** in Kestra's `taskRunList` order, every
parent, every history, every attempt, and **54 of its 57 kept ordered pairs**.
The three it does not are `(if-2, if-3-log)`, `(if-3-log, log-parallel-4)` and
`(log-parallel-2, log-parallel-5)`, all `golden: before` / `egret: overlapping`
— the same class as `valids/dag`'s `(task3, task4)` and for the same reason: a
machine that completes one in-flight run per round lands a creation in the same
round as a termination Kestra had already seen. That the entry's runs are
identical to Kestra's is the measurement; the Kestra-side cycle is not. See
*What this oracle does not prove* and `runnable.txt`.

### Task 5's mutations

**Twenty-one after fix round 1** (sixteen in round 0, plus five the round
added: three that prove no banked entry takes an `else`, one for the
declared-`type:` gate and one for the render tier's batching). Each applied to a
pristine snapshot, **built first**
(`cargo build -p egret-workflow --tests`, **0 errors** asserted) and only then
run against the **whole crate** (`cargo test -p egret-workflow --no-fail-fast`),
never a name-filtered subset. Restore is `shutil.copyfile` (fresh mtime, so
cargo always rebuilds), verified with `filecmp.cmp` after every mutation and
with `cmp` against the snapshot for all five files at the end. **The victim
counts below are from a replay of the ROUND-1 tree**, not from memory and not
carried over from round 0: Task 4 dropped the literal-edit column and its M16
then replayed to 3 where its table said 2, and round 1 moved five of these rows
on its own (M27's anchor changed with the fix; M30, M31, M37 and M42 each gained
a victim from the two tests round 1 added). Every row was re-run.

| # | file | `-` | `+` | victims (replayed) |
|---|---|---|---|---|
| M27 | `branch.rs` | `rendered.trim_matches(\|c: char\| c <= '\u{20}'),` | `rendered.trim(),` | 1 |
| M28 | `branch.rs` | `"false" \| "0" \| "-0" \| ""` | `"false" \| ""` | 2 |
| M29 | `branch.rs` | the whole `!matches!(…)` | `.parse::<bool>().unwrap_or(!rendered.trim().is_empty())` | 2 |
| M30 | `branch.rs` | `if is_truthy(rendered) { then_ } else { else_ }` | `{ else_ } else { then_ }` | 9 (incl. ratchet) |
| M31 | `branch.rs` | `let Some(rendered) = … else { return Step::Waiting; };` | `let rendered = ….unwrap_or("");` | 9 (incl. ratchet) |
| M32 | `branch.rs` | `cases.iter().find(\|(key, _)\| key == rendered)` | `cases.first()` | 5 (incl. ratchet) |
| M33 | `branch.rs` | `Some((_, chosen)) if chosen.is_empty() => Step::Waiting,` | *(arm deleted)* | 1 |
| M34 | `branch.rs` | `None if default.is_empty() => Step::Done(RunState::Failed),` | `… RunState::Success),` | 2 (incl. ratchet) |
| M35 | `plan.rs` | `cond.condition.clone().ok_or_else(…)?` | `cond.condition.clone().unwrap_or_default()` | 2 (incl. ratchet) |
| M36 | `plan.rs` | `.ok_or_else(…)?` on `Switch::value` | `.unwrap_or_default()` | 1 |
| M37 | `flowable/mod.rs` | the `TaskKind::If` arm of `descend` | `TaskKind::If { .. } => return Step::Waiting,` | 10 (incl. ratchet PANIC on 15) |
| M38 | `tests/executor.rs` | `if !unrendered.is_empty() {` | `if false && !unrendered.is_empty() {` | 3 (incl. ratchet PANIC on 19) |
| M39 | `tests/executor.rs` | the failed-render arm's `apply(… Failed …)` + `ended[ix] = Some(step)` | `Err(_) => state.runs[ix].rendered = Some(String::new()),` | 2 (incl. ratchet) |
| M40 | `tests/executor.rs` | `flow.and_then(\|f\| to_value(f.variables).ok()).unwrap_or(Null)` | `serde_json::Value::Null,` | 2 |
| M41 | `tests/executor.rs` | `Some(TaskKind::Switch { value, .. }) => Some(value.clone()),` | `Some(TaskKind::Switch { .. }) => None,` | 2 (incl. ratchet PANIC on 3) |
| M42 | `tests/executor.rs` | `for input in &f.inputs {` | `for input in f.inputs.iter().take(0) {` | 4 (incl. ratchet) |
| M43 | `branch.rs` | `let chosen = if is_truthy(rendered) { then_ } else { else_ };` | `let chosen = { let _ = is_truthy(rendered); then_ };` | 6 **new** |
| M44 | `branch.rs` | the whole `!matches!(…)` | `!rendered.trim_matches(…).is_empty()` | 6 **new** |
| M45 | `branch.rs` | the whole `!matches!(…)` | `.parse::<bool>().unwrap_or(true)` | 2 **new** |
| M46 | `tests/executor.rs` | the `match input.r#type { … }` gate | `let typed = serde_json::from_str(raw).ok();` | 1 **new** |
| M47 | `executor/mod.rs` | `for ix in unrendered {` | `for ix in unrendered.into_iter().take(1) {` | 1 **new** |

Messages worth quoting, because they are the ones a maintainer will see:

- M27: ``condition rendered "  false  " -- measured live on kestra/kestra:v2.0.0-rc13, which took `when-false` -- left: ["if", "when-true"] right: ["if", "when-false"]``
- M33: ``an empty matched case neither runs `defaults` nor fails -- left: [SetRunState { run: 0, state: Success }] right: []``
- M34: ``a Switch with nothing to run FAILS -- an If in the same position SUCCEEDS -- left: ["CREATED", "RUNNING", "SUCCESS"] right: ["CREATED", "RUNNING", "FAILED"]``
- M38: ``a rendered If must settle -- STUCK here means the render tier is gone: "STUCK after 2 step(s) at now=Tick(0) …"``
- M42: ``17 of 26 branch expressions render`` — which is where the 9 above comes from.
- M27 (round 1): ``condition rendered "\u{1}false\u{1}" -- measured live on kestra/kestra:v2.0.0-rc13, which took `when-false```
- M46: ``a STRING input's `defaults: true` is the STRING "true" -- parsing it as JSON would make it a boolean and this comparison false``
- M47: ``valids/if's taskRunList from golden.json, verbatim``

**Five rows do NOT include the ratchet, and that is the finding rather than a
gap**: M43, M44 and M45 are the three mutants that prove no banked entry takes
an `else` branch — see *No banked entry takes an `else` branch* — and M46 and
M47 are guarded by the two tests round 1 added precisely because nothing else
could see them fail.

`cargo fmt` ran after the mutation set; nothing it touched changed by more than
whitespace.

## What Task 6 had to change, and what it banked

Task 6 implements `loop` and `Command::SpawnExecution`. It banks **2** of the
181 Task 5 left, taking `baseline.txt` to **179**:

| banked | what earns it |
|---|---|
| `valids/loop-empty` | `values: '[]'` — the `Loop` ends `SUCCESS` **without ever running**: `CREATED SUCCESS`, no `RUNNING`, no attempt, and the next root task runs |
| `valids/loop-null` | `values: '[1, null, {…}]'` — the values cannot be resolved and the `Loop` ends `CREATED FAILED`, also with no attempt, and the next root task is never created |

The control was run **before any code**, in the main tree at `6774e282` with
`git status --porcelain` empty:

```text
oracle-exec ratchet: 294 golden entries, 181 mismatching, 181 baselined, 0 broken,
  25 ordered pair(s) removed
```

and after:

```text
oracle-exec ratchet: 294 golden entries, 179 mismatching, 179 baselined, 0 broken,
  25 ordered pair(s) removed
```

**Two is the whole of what this task could bank, and the reason is the hole
rather than the implementation.** Those two are the *only* `Loop` entries in the
golden whose sub-execution list is empty. Every other one carries a `spawned`
that egret cannot produce — see below for the three numbers that say how much.

### A `Loop` does not iterate inside its execution

Measured, and it is the whole shape of the flowable. Each value produces a
separate **sub-execution** carrying `parentId`; the body's tasks never appear in
the parent's `taskRunList`. Over the comparable corpus, **39 entries hold a
`Loop`, with 82 tasks inside a loop body between them, and not one of those
tasks has a run in its parent** — on either side.
`no_run_is_ever_created_for_a_task_inside_a_loop` derives that from egret's
traces *and* from Kestra's own golden, because if Kestra ever did put one there,
`advance()` refusing to create one would be the defect rather than the rule.

`Plan::is_loop_body` is the predicate, and "inside" is the **whole subtree**, not
the direct children. Nothing forces that from the type — a narrow reading
compiles and passes everything else, because `advance()` never calls the method
at all — so the two readings are measured against each other: over all 345
corpus flows the wide one names **89** tasks and the narrow one **64**.

### Only the FIRST batch is spawned, and that is a derivation

`advance()` is a pure function of `(plan, state, now)` and `ExecutionState`
holds **nothing** about a sub-execution — no id, no state, no count. Two things
follow, and neither is a preference:

- **Every spawn must be emitted in one round.** A round that emitted some and
  left the rest would be asked again against a state recording neither and would
  emit the same ones for ever. So the emission is tied to the one transition
  that happens exactly once, `CREATED` → `RUNNING`.
- **There is no second batch.** Kestra starts `min(limit, size)` iterations and
  then one more each time an iteration terminates
  (`Loop.java:405-424`, `LoopExecutionEventMessageHandler`). The second half
  needs an iteration outcome, which is the thing this crate cannot represent.
  W3b owns it.

So `advance()` emits Kestra's own first batch, `Loop.java:422` verbatim:
`limit = concurrencyLimit == 0 ? size : min(concurrencyLimit, size)`.

### `concurrencyLimit` defaults to **1**, not 0 — and the corpus can tell

This is the one number in the task most likely to be got wrong, because
`Parallel::concurrent` is the same shape and defaults to the opposite:
`Parallel.java`'s `concurrent` is `0` (unlimited) by default, while
`Loop.java:220-233` is
`@PositiveOrZero @NotNull @Builder.Default private Integer concurrencyLimit = 1;`
— full serialisation. **`0` still means unlimited on both**; it is only the
default that differs.

The number is not taken from either reading. `the_default_concurrency_limit_is_
one_and_the_corpus_can_tell` derives it from Kestra's own traces: for each of
the **24** top-level `Loop`s that write no `concurrencyLimit`, it compares the
values the flow resolves against the sub-executions Kestra actually recorded.
Under an unlimited default the second could never be smaller than the first, and
on five of them it is:

| entry | values | sub-executions Kestra ran |
|---|---|---|
| `valids/all-flowable` `1-each` | 3 | 1 |
| `valids/loop-failed` `loop` | 3 | 1 |
| `valids/loop-outputs-failed-render` `loop` | 3 | 1 |
| `valids/restart-loop` `loop` | 3 | 1 |
| `valids/restart-parent-loop` `each` | 3 | 1 |

Under a default of `0`, egret would ask for every value on each of those and
`every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` would fail five
times. So the default is held by a corpus assertion, not by a line of Java.

**An explicit `concurrencyLimit: null` and a negative one are OVER-ACCEPTANCES**
and are Task 8's, not this task's. Kestra refuses both at publish
(`@NotNull`, `@PositiveOrZero`); W1 accepts them because bean validation is not
parsing, and `build_plan` is documented not to widen refusal. Egret plans them
with the class default of `1`. **That fallback was a guard nothing could see
fail** — the mutation flipping it to `0` killed nothing at all on the first pass,
because no corpus flow writes either spelling — and
`a_null_or_negative_concurrency_limit_takes_kestras_class_default` is what
closes it. Fourth instance of that pattern in this package; the first three
shipped.

### The values are Kestra's `resolveValues`, and two of its rules are not obvious

`iteration_values` is `FlowableUtils.resolveValues`'s **String** arm
(`FlowableUtils.java:418-455`), because a string is the only shape that reaches
egret: Kestra's `Loop.values` is a bare `Object`, so a YAML sequence arrives as a
`List` and a quoted JSON string as a `String`, while W1's
`Property<serde_json::Value>` flattens **both** to the same JSON text first.

- **A map iterates its VALUES**, not its keys and not its pairs — measured on
  `valids/loop-map`, whose `{key1: value 1, …}` produced sub-executions carrying
  `"value 1"`, `"value 2"`, `"value 3"`.
- **`distinct()` is on the array arm and NOT on the map arm** —
  `FlowableUtils.java:439` and `:461` against `:463-468`. No corpus flow has a
  duplicate value, so a unit test holds it rather than an entry.
- A `null` element throws, which is exactly what fails `valids/loop-null`.
- A non-string element is its **compact JSON**, with keys in **insertion**
  order: `valids/loop-object`'s golden value is
  `{"key":"my-complex","value":{"sub":1,"bool":true}}`, and `bool` sorts before
  `sub`. That works here only because this crate's `serde_json` resolution
  carries `preserve_order` — the feature `Cargo.toml` already pins for a
  different reason.

**Where egret is knowingly WIDER than Kestra**, and it cannot be narrowed here:
Kestra's `List` arm resolves elements through `valueAsString`, which accepts
`String`, `Number` and `Map` and **throws for a boolean or a nested list**,
while its `String` arm serialises any non-null element. On the artifact
`values: [true]` therefore fails and `values: '[true]'` succeeds. Egret takes
the `String` arm for both, because W1 flattened them into one text before this
code ran. It is the same class of model gap as `defaults: []` versus an absent
`defaults:`, in a different field, and closing it means widening W1's model.
No corpus flow is either shape.

### The sub-executions: what is compared, and what is not

**An uncompared golden that looks compared is worse than no golden**, so the
hole is stated with numbers and marked by a test rather than by a sentence.

| | |
|---|---|
| golden entries carrying a sub-execution | **41** |
| sub-executions they carry | **142** |
| of the 41, entries that now differ on **nothing but `spawned`** | **33** |
| sub-executions whose `(parent_run, value)` IS compared | **52** |
| sub-executions whose **trace** is compared | **0** |

`the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` asserts
the first three, requires all 41 to still be baselined, and names the 8 that
differ on more than `spawned` with the reason for each: four are the
`retry-new-execution-*` replacement executions (Task 7's), one is a `disabled:`
task (unclaimed), one is a `Loop` whose first iteration failed and whose
flow-level `errors:` then ran (Task 7's), and two are the `values: "{{
outputs.write.uri }}"` flows the harness cannot render.

`every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` is the part that
*can* be compared, and it is corpus-wide: over **36** loop runs it checks that
the `(parent_run, value)` pairs egret asks for are the **first** ones Kestra's
`loopRun.index` ordering recorded for the same run, that the command names the
`Loop` and not a body task, and that the run it names as parent is a run of that
same loop. **52** spawn requests are compared this way and on **16** of the 36
runs the first batch is the whole of what Kestra ran.

A prefix and not an equality, because there is no second batch. `valids/loop-serial`
ran 3 iterations and egret asks for 1; `valids/all-flowable` ran 1 of its 3
because the first failed, and egret asks for 1 there too.

### A `Loop`'s own terminal state is FED, which corrects this file

See *What the oracle feeds in* — the correction is recorded there, in place,
because it contradicts a sentence this file already carried. In short: a
`Loop`'s state is a function of its sub-executions, this oracle runs none, and
the alternative to feeding it is not "compare it" but `STUCK` on **35**
honestly-baselined entries (`the_loop_completion_tier_is_load_bearing_for_exactly_these_entries`
derives the 35 and requires every one of them to be baselined; Task 6's first
report said 37, in prose, pinned by nothing).

Two further corrections to that first account, both measured and both in the
implementation's favour: **nothing moved from compared to fed** — a `Loop` was
already an opaque `Runnable` at Tasks 3-5, so its terminal was already supplied,
and Task 6 relabelled 40 of those and dropped 40 spurious `SUBMITTED`
emissions with them; and **the aggregation rule is not "judged by nothing"** —
it is invisible to the *ratchet*, and two committed tests catch it anyway
(M66, M67).

### The render tier gained a third kind and a second run state

`TaskKind::Loop` holds an **unrendered** `values`, exactly as `If` holds an
unrendered `condition`, and the caller supplies the answer in
`TaskRun::rendered`. It is asked at **`CREATED`** and not at `RUNNING`, and that
is measured: `valids/loop-empty` and `valids/loop-null` both settle from
`CREATED` without a `RUNNING` or an attempt, so the values decide whether the
run starts at all. `renders_at` is the one place that difference lives, on each
side of the seam.

**The `item` ruling, and the number Task 5 forwarded.** Task 5 recorded a
review's measurement that `{{ item.value }}` and three variants return
`Err("undefined value")` here and that **16 more expressions across 15 entries**
would become planned once `Loop` landed, each failing its run. Re-derived on
this tree, the population is:

| | reachable | inside a loop body |
|---|---|---|
| `If`/`Switch` expressions | 26 | 12 |
| `Loop` `values:` | 40 | 6 |

**12, not 16 — and none of them is reachable.** A loop body's task never gets a
run in this execution, so tier 2 never renders its expression and none of them
fails anything. The reachable branch population is still exactly the **26** Task
5 measured, with the same 2 failures. Ten of the 18 loop-body expressions name
`item`; the other eight are six nested `Loop` `values:` literals and two `If`
conditions over `inputs:` that this context could render perfectly well — which
is what says the exclusion is about *no run existing*, not about the context
being too narrow.

**So no `item` is supplied**, deliberately. Kestra renders those expressions
inside the sub-execution with `item` from `loopRun`, and the sub-executions are
exactly what W3a does not run; inventing one here would put a made-up value
behind a branch decision and change nothing this oracle can see.

**Two entries are blocked, and they are blocked TWICE OVER — which Task 6's
first account got wrong by naming only the first half.**
`valids/loop-outputs-auto` and `valids/loop-values-from-uri` both write
`values: "{{ outputs.write.uri }}"`, and:

1. `render_context` carries no `outputs`, so the render fails here where
   Kestra's `RunContext` resolved it — the narrow-context cost, and Task 5 had
   none of these;
2. **and that expression renders to an ION file URI, which `advance()` cannot
   resolve either.** See *A third `values:` shape, named as a gap* below. With a
   perfect `outputs` context both entries would still fail, inside the executor.

`the_caller_can_render_every_expression_except_the_ones_named` holds them in a
list of their own — separate from the two Kestra could not render either — and
asserts three things: that the render fails, that `golden.json` says Kestra's
own runs for them ended `SUCCESS`, and that `iteration_values` refuses a
`kestra:///….ion` string. The third is the second cause, asserted rather than
described, because an attribution committed in a test is the kind that gets
believed. Both entries are baselined, and both are in the 8 that differ on more
than `spawned`.

### A third `values:` shape, named as a gap

`Loop.java:210-214` documents **three** shapes and `iteration_values` implements
two. The third is "an ION file URI, each line will be deserialized as an ION
object then coalesced to a string", and it never reaches `resolveValues` at all:
`FlowableUtils.resolveLoopValuesUri` (`FlowableUtils.java:519`) renders `values`,
asks `URIFetcher.supports` whether the result is a URI, and hands it to
`Loop.initFromUri` (`Loop.java:404`), which reads the file and counts its lines.

So this is **a gap in `advance()`**, not a limitation of the harness, and it is
recorded here as one rather than filed under the render context — where Task 6's
first account put the whole of it. It costs the two entries above.

**What closing it would take.** `advance()` performs no I/O and cannot fetch the
file. The caller would have to resolve `values` all the way to a JSON array —
fetching the ION file and coalescing its lines — and put *that* into
`TaskRun::rendered`, which is already what `rendered` means for an `If`: the
caller's answer, not the author's source. That is a decision about the W3b
contract and **it has not been taken**. Neither entry could bank on it alone:
this oracle has neither the `outputs` context nor the storage the file lives in.

### Task 6's mutations

**Twenty after fix round 1** (eighteen in round 0, plus the two that prove the
loop's aggregation is judged by something). Each applied to a pristine snapshot,
**built first**
(`cargo build -p egret-workflow --tests`, **0 errors** asserted) and only then
run against the **whole crate** (`cargo test -p egret-workflow --no-fail-fast`),
never a name-filtered subset. Restore is `shutil.copyfile` (fresh mtime, so
cargo always rebuilds), verified with `filecmp.cmp` after every mutation and
against the snapshot for all six files at the end. **Every victim count below is
from one replay of the round-1 tree**, not carried over from round 0: the two
tests round 1 added moved eight of the eighteen rows on their own.

**Round 1's first attempt at that replay was discarded, and the reason is worth
the paragraph.** It was killed by a command timeout part-way through a mutation,
so its restore never ran; the *next* run snapshotted the mutated file, and its
own restore-and-verify then compared the tree against a poisoned snapshot and
agreed with itself perfectly — printing `ALL FILES RESTORED AND VERIFIED` over
counts that had every one been measured against a mutated harness. **A restore
that verifies against a poisoned snapshot verifies nothing.** The harness now
refuses to snapshot if any mutation's `+` text is already present in its file
while its `-` text is gone, and says so before it starts; the table below is
from the run that check guarded. Two independent cross-checks say it is the
sound one: it reproduces round 0's counts wherever round 1 added no test, and
M63's ratchet line reads `35 broken`, which is the same 35
`the_loop_completion_tier_is_load_bearing_for_exactly_these_entries` derives by
a completely different route.

| # | file | `-` | `+` | victims (replayed) |
|---|---|---|---|---|
| M48 | `loop_.rs` | `if concurrency_limit == 0 { size } else { (concurrency_limit as usize).min(size) }` | `(concurrency_limit as usize).min(size)` | 3 |
| M49 | `loop_.rs` | the same expression | `let _ = concurrency_limit; size` | 4 |
| M50 | `plan.rs` | `_ => 1,` in `literal_concurrency_limit` | `_ => 0,` | 1 |
| M51 | `loop_.rs` | the whole `if values.is_empty() { … Success … }` block | *(deleted)* | 3 (incl. ratchet) |
| M52 | `loop_.rs` | `let Ok(values) = … else { … Failed … };` | `let values = iteration_values(rendered).unwrap_or_default();` | 3 (incl. ratchet) |
| M53 | `loop_.rs` | `let Some(rendered) = … else { return Step::Waiting; };` | `let rendered = ….unwrap_or("");` | 7 (incl. ratchet) |
| M54 | `loop_.rs` | `if !out.contains(&value) { out.push(value); }` | `out.push(value);` | 1 |
| M55 | `loop_.rs` | `.map(\|(_, value)\| match &value {` | `.map(\|(key, value)\| match &Value::String(key) {` | 2 |
| M56 | `loop_.rs` | the array arm's `Null => return Err(…)` | *(arm deleted)* | 6 (incl. ratchet) |
| M57 | `flowable/mod.rs` | `TaskKind::Loop { .. } => loop_::open(…),` in `open` | *(arm deleted)* | 9 (incl. ratchet; hole 33 → **35**) |
| M58 | `flowable/mod.rs` | `TaskKind::Loop { .. } => return Step::Waiting,` in `descend` | `TaskKind::Loop { tasks, .. } => sequential::drive(plan, state, tasks, Some(run), now),` | 6 (incl. ratchet, **4 broken**) |
| M59 | `plan.rs` | `frontier.extend(self.children_of(ix));` in `is_loop_body` | *(deleted — the narrow reading)* | 4 |
| M60 | `plan.rs` | `.ok_or_else(…)?` on `Loop::values` | `.unwrap_or_default()` | 1 |
| M61 | `loop_.rs` | `Command::SpawnExecution { task,` | `Command::SpawnExecution { task: task + 1,` | 1 |
| M62 | `tests/executor.rs` | `Some(TaskKind::Loop { .. }) => RunState::Created,` in `renders_at` | *(arm deleted)* | 5 (incl. ratchet, **39 broken**) |
| M63 | `tests/executor.rs` | `Some(TaskKind::Runnable { .. }) \| Some(TaskKind::Loop { .. })` in `in_flight` | `Some(TaskKind::Runnable { .. })` | 5 (incl. ratchet, **35 broken**) |
| M64 | `tests/executor.rs` | `Some(TaskKind::Loop { values, .. }) => Some(values.clone()),` | `Some(TaskKind::Loop { .. }) => None,` | 6 (incl. ratchet, **39 broken**) |
| M65 | `executor/mod.rs` | `TaskKind::Loop { .. } => Some(RunState::Created),` in the lib `renders_at` | *(arm deleted)* | 2 |
| M66 | `flowable/mod.rs` | `TaskKind::Loop { .. } => return Step::Waiting,` in `descend` | `TaskKind::Loop { .. } => Step::Done(RunState::Success),` | 2 (hole 33 → **23**) |
| M67 | `tests/executor.rs` | tier 3's `state: golden_outcome(golden, ix, attempt),` | forced to `RunState::Success` for a `Loop`, schedule untouched | 2 (hole 33 → **23**) |

Messages worth quoting, because they are the ones a maintainer will see:

- M49: ``four values, `concurrencyLimit: 2` -- Kestra's own first batch is `min(limit, size)` (Loop.java:422)``
- M51: ``no RUNNING and no attempt: an empty loop never starts -- left: ["CREATED", "SUCCESS"] right: ["CREATED", "RUNNING", "SUCCESS"]``
- M56: ``valids/loop-null: egret's `1_each` SUCCEEDs where Kestra FAILED it`` — the null-element rule is a whole banked entry on its own.
- M61: ``SpawnExecution must name the Loop itself, not a body task``
- M50: ``\`concurrencyLimit: null\` must plan as 1`` — the only victim, and the test exists **because** the first replay found none.
- M63: ``179 baselined, 35 broken`` — the completion tier, measured independently of the test that pins the same 35.

**Eleven rows do not include the ratchet, and that is a shape rather than a
gap.** (Task 6's first table said "six", which was wrong on its own 18 rows —
it was nine there and is eleven here. A count beside a derivation, again.)
They fall into three groups:

- **M48, M49, M50, M61 — the spawn schedule.** Invisible to
  `executor_matches_golden` for one reason: the ratchet compares `spawned`,
  egret's is empty, and every entry that could notice a wrong batch is baselined
  anyway. That is exactly the hole this section measures, and it is why
  `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` exists — it is
  the victim of all four.
- **M66, M67 — the loop's aggregation.** Two independent constructions that get
  a `Loop`'s terminal wrong: `Step::Done(Success)` inside `descend`, and a
  tier-3 override that forces `Success` while leaving the spawn schedule exactly
  as it is. Neither moves the ratchet, and **both drop
  `the_spawned_field_…_and_a_bank` from 33 to 23** and fail the order test. So
  "`transmitFailed` and the aggregation rule are judged by nothing", which Task
  6's first account said, is wrong: they are judged, by two tests, and that
  **33** is a guard as well as a report.
- **M54, M55, M59, M60, M65 — rules the corpus does not reach.** Held by unit
  tests, except that M59 is not: **two of its four victims are ORACLE tests**
  (`no_run_is_ever_created_for_a_task_inside_a_loop` and
  `the_caller_can_render_every_expression_except_the_ones_named`), and a fourth
  is `planning_the_loop_removed_forty_spurious_submitted_emissions`. The first
  table put M54 and M59 in one box and only M54 belongs in it.

`cargo fmt` ran after the mutation set; nothing it touched changed by more than
whitespace.

## What Task 7 had to change, and what it banked

Task 7 implements `retry:` and `errors:`. It banks **16** of the 179 Task 6
left, taking `baseline.txt` to **163** — the largest single bank since Task 3,
and the reason is that the two subjects are one subject: `errors:` is the list a
failure runs *instead of* `tasks:`, and a retry is the thing that decides
whether the failure is final.

| banked | what earns it |
|---|---|
| `valids/retry-failed-task-attempts` | `maxAttempts: 4` on the task. Four attempts, and the execution's `FAILED RETRYING RUNNING` three times over |
| `valids/retry-failed-flow-attempts` | the same shape with the retry on the **flow** — the third source in the precedence |
| `valids/retry-fail` | a retry that exhausts, then a flow-level `errors:`, and a second root task that is never created |
| `valids/retry-failed` | five attempts, `maxDuration: PT15S` that never bites, then `errors:` |
| `valids/retry-expo` | `type: exponential`, and the lower-case `PT0.25s` a case-sensitive duration parser drops silently |
| `valids/retry-random` | `type: random`, whose delay `advance()` cannot draw and does not need to |
| `valids/retry-flowable` | `retry:` written on a `Sequential`: the flowable gets **one** attempt, its child gets three |
| `valids/retry-flowable-child` | the same, one level of nesting apart |
| `valids/retry-flowable-nested-child` | two levels: `flowable` and `flowable2` both stay `RUNNING` for all three attempts |
| `valids/errors` | flow-level `errors:` with a four-deep `Parallel` chain inside it, and an execution that ends `FAILED` although every error task SUCCEEDED |
| `valids/change-state-errors` | a mid-list failure, an `errors:`, and the third root task never created |
| `valids/sequential-with-global-errors` | the error runs at the root (`parent: null`) |
| `valids/sequential-with-local-errors` | the identical flow with the identical `errors:` moved onto the inner `Sequential`: the error runs hang off **it** |
| `valids/replay-sequential-with-error-handler` | a task-level `errors:` on a `Sequential` inside a root list that has already run one task |
| `valids/restart_local_errors` | the same, plus a third root task that is never created because the `Sequential` ended `FAILED` |
| `valids/restart_always_failed` | a flow-level `errors:` on a flow whose only task fails |

The control was run **before any code**, in the main tree at `d8730508` with
`git status --porcelain` empty:

```text
oracle-exec ratchet: 294 golden entries, 179 mismatching, 179 baselined, 0 broken,
  25 ordered pair(s) removed
```

and after:

```text
oracle-exec ratchet: 294 golden entries, 163 mismatching, 163 baselined, 0 broken,
  30 ordered pair(s) removed
```

### `timeout:` is not the executor's, and the plan's own step said it was

The plan gives Task 7 "`retry`, `timeout` and `errors:`" and its Step 3 says
"`timeout` is a deadline compared against the `now` parameter". Measured against
the artifact, it is not one at all:

- `worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:58`
  wraps `task.run(runContext)` in `callWithTimeout` — the **worker** enforces it;
- `core/src/main/java/io/kestra/core/services/FlowService.java:574` emits
  "cannot use the 'timeout' property as it's only relevant for runnable tasks"
  for every non-`RunnableTask`, with `Pause` the single exception.

So the only `timeout:` an `advance()` will ever enforce is a `Pause`'s, and
`Pause` is W8's. `PlanTask::timeout` is still `None` and no line of Task 7
implements a deadline. What Task 7 *does* own of the elapsed-time rules is
`RetryPolicy::max_duration`.

### The double `taskRunList` entry does not reproduce

The plan's Step 2 records, from the spec, that Kestra's own record showed a
retried task as **two `taskRunList` entries**, and asks Task 7 to determine
before implementing whether that is a semantic fact or a serialisation artefact.

**It is neither. It does not occur on this artifact at all.** Measured two ways,
so that a capture anomaly in one could not hide it in the other:

- over `traces.json`, **2050 execution records** (268 flows × 5 rounds, roots and
  every descendant) holding **4215 taskruns**: zero have two entries with the
  same taskrun id, and zero have two with the same
  `(taskId, value, parentTaskRunId)`. The 4215 is the same population this file
  quotes for the happens-before endpoint measurement, reached by a different
  route. `no_captured_execution_has_a_duplicate_taskrun` re-derives it on every
  run;
- from the artifact, the path is closed by construction.
  `ExecutorService.java:611` marks the run `RETRYING` **in place**, and
  `ExecutionService.retryTask` (`ExecutionService.java:135`) installs the new
  attempt through `Execution.withTaskRun`, whose body is
  `Collections.replaceAll(newTaskRunList, findTaskRunByTaskRunId(id), taskRun)`
  followed by `throw new IllegalStateException` when the replace finds nothing.
  It replaces or it throws. There is no append.

So egret reproducing one entry with several attempts is not a trade-off against
Kestra's record shape — it *is* Kestra's record shape. Whatever produced the
spec's observation was not this image's `taskRunList`.

### One empty list, two answers, decided by who is asking

`Execution.findTaskDependingFlowState` returns `emptyList()` for two different
reasons and the artifact reads them **differently depending on the caller**:

| caller | guard | answer for the empty list |
|---|---|---|
| a flowable, `FlowableUtils.resolveState` | `if (execution.hasFailedNoRetry(...))` | keep running |
| the execution, `ExecutorService.handleEnd` | `execution.isTerminated(currentTasks)` | `0 == 0` — take a state |

That single difference is the whole of why `valids/retry-failed`'s execution
reads `CREATED RUNNING FAILED RETRYING RUNNING FAILED …`, one `FAILED` per
attempt, while `valids/retry-flowable`'s reads `CREATED RUNNING RETRYING RUNNING
RETRYING RUNNING FAILED` with no `FAILED` until the end. `flowable::Current` is
that four-way distinction as a type and `sequential::drive` keys on
`parent.is_none()` — the flow's own list — for the question they answer
differently.

The other half of the same function: **`guessFinalState` is passed `tasks`,
never the list actually driven**, in both `resolveState` and `onEnd`. That is why
`valids/errors`, whose two error tasks both SUCCEED, ends `FAILED`.

### The retry precedence, and the flowable that is never retried

Three sources, in `ExecutorService.java:576-599`'s order: the task's own
`retry:`, else the nearest **ancestor** with one, else the flow's — and when an
ancestor has one the flow's is **not** consulted. `retry::policy_for` is that
precedence in one place.

The pass itself is gated on `task instanceof RunnableTask<?> || task instanceof
Subflow`, so **a flowable is never retried whatever its own `retry:` says**.
`valids/retry-flowable` is the measurement: its `retry:` sits on a `Sequential`,
and the `Sequential` has **one** attempt while its child has three.

Kestra keeps that `Sequential` alive by a mechanism egret does not port —
`ExecutorService.java:615-618` filters the parent's own worker result out of the
batch whenever a child is being retried — because `hasFailedNoRetry` alone would
have ended it (it asks only the failing task's *own* `retry:`, and the child has
none). `flowable::current_list` asks once, with the full precedence, and reaches
the same answer for everything in this corpus. Where the two constructions could
differ is a retry owned by a grandparent, since Java's filter names only
`getParentTaskRunId()`; they do not differ in fact, because the grandparent's
state is a function of the parent's and the parent is already held — measured on
`valids/retry-flowable-nested-child`.

### `errors:` did NOT make `fail_the_rest` comparable

*What Task 4 had to change* records that `fail_the_rest` — a `Parallel` failing
its still-in-flight siblings while already-terminated ones keep their state — is
unfalsifiable by this oracle, and the plan predicted `errors:` would change that.
**It did not, and the reason is the corpus rather than the implementation.**

The two golden shapes the rule rests on are `valids/parallel-fail-with-flowable`
(an in-flight sibling, and **no `errors:`**) and `valids/finally-parallel` (an
`errors:` — and a `finally:` — whose three surviving siblings had **already
SUCCEEDED** before the failure: `p1`/`p2`/`p3` end at `01.248`–`01.261` in
`traces.json` and the failure is at `01.297`). No corpus `Parallel` has both an
`errors:` and a sibling still running when one branch fails, so the composition
of the two rules is written in `parallel::drive` and exercised by nothing. And
`valids/finally-parallel` cannot bank while `finally:` is unimplemented.

What *is* comparable, and was already: the rule's **bound**. M89 deletes the
`!is_terminal` filter — the part that says an already-finished sibling keeps its
state — and the ratchet reports it. M98 deletes the whole body.

### Task 7's mutations

**Twenty-six after fix round 1**, plus three that no longer exist as mutations —
see below. Each `-` → `+` is the literal edit, so the table can be replayed
rather than believed. Every one was applied to a file **anchored to
`git show HEAD:<path>`** (never to the working tree), **built first**
(`cargo build -p egret-workflow --tests`, 0 errors asserted) and only then run
against the **whole crate** (`cargo test -p egret-workflow --no-fail-fast`),
never a name-filtered subset. Restore is `shutil.copyfile` (fresh mtime, so
cargo always rebuilds), verified after each mutation and against the anchor for
all seven files at the end. **Every count is from one replay of the final
tree**, except M97's — see the note after the table.

The harness refuses to start if any mutation's `-` text is missing from its
file, or if any file differs from `HEAD`. That check earned its place inside
this task: round 0's first attempt was piped into `head -20`, which exited after
twenty lines and killed the replay with `SIGPIPE` **part-way through a
mutation**, leaving `sequential.rs` mutated in the tree. The next invocation
refused to run and named the file. That is the same failure Task 6 recorded, met
again in a new disguise, and it is the guard rather than the discipline that
caught it.

| # | file | `-` | `+` | victims (replayed) |
|---|---|---|---|---|
| M68 | `retry.rs` | `if policy.max_attempts.is_some_and(\|max\| count >= max) {` | `if false && …` | 6 (incl. ratchet, **9 broken**) |
| M69 | `retry.rs` | `if next.0 > origin.0.saturating_add(millis(&max)) {` | `if false && …` | 3 (incl. ratchet, 1 broken) |
| M70 | `retry.rs` | `RetryBehavior::RetryFailedTask => (attempts, task_run.started_at()?),` | `… => (attempts, Tick(0)),` | 1 |
| M71 | `retry.rs` | `RetryBehavior::CreateNewExecution => (1, Tick(0)),` | `… => (1, task_run.started_at()?),` | 1 |
| M72 | `retry.rs` | `let mut parent = state.runs[run].parent;` | `let mut parent: Option<RunIx> = None;` | 2 (incl. ratchet, 1 regression) |
| M73 | `retry.rs` | `plan.retry.as_ref()` closing `policy_for` | `None` | 3 (incl. ratchet, 1 regression) |
| M74 | `retry.rs` | the `if !matches!(… Runnable …) { continue; }` in `scan` | *(deleted)* | 4 (incl. ratchet, 1 regression) |
| M75 | `retry.rs` | `factor.powi(count as i32 - 1)` | `factor.powi(count as i32)` | 1 |
| M76 | `retry.rs` | `RetryDelay::Random { min_interval, .. } => *min_interval,` | `… { max_interval, .. } => *max_interval,` | 1 |
| M77 | `retry.rs` | `take_unit(&mut rest, ['S', 's'], &mut fields)?` | `take_unit(&mut rest, ['S', 'S'], …)?` | 4 (incl. ratchet, 1 regression) |
| M78 | `retry.rs` | `not_before.is_some_and(\|t\| t <= now)` | `not_before.is_some()` | 4 (incl. ratchet, 1 broken) |
| M79 | `retry.rs` | one `SetExecutionState { Running }` then the due runs | one of each, per due run | 1 |
| M80 | `state.rs` | the `Some(a) if !a.history.last().is_some_and(…is_terminal())` arm | `if let Some(a) = r.attempts.last_mut()` | 5 (incl. ratchet, **9 regressions**) |
| M81 | `state.rs` | `Success \| Failed \| Retried \| Killed` in `is_terminal` | `Success \| Failed \| Killed` | 2 (incl. ratchet, 4 broken) |
| M82 | `flowable/mod.rs` | the `return Current::RetryPending` block | `let _ = will_retry;` | 3 (incl. ratchet, 7 broken) |
| M83 | `flowable/mod.rs` | `if errors.is_empty() { Ended } else { Errors(errors) }` | `Current::Ended` | 5 (incl. ratchet, **9 regressions**) |
| M84 | `sequential.rs` | `None => Step::Done(aggregate(state, children, parent)),` | `None => Step::Done(RunState::Success),` | 4 (incl. ratchet, **9 regressions**) |
| M85 | `sequential.rs` | the `Current::RetryPending if is_flow` arm | *(deleted — both collapse to `Waiting`)* | **8** (incl. ratchet, 2 broken) |
| M88 | `mod.rs` | the `if !reopened.is_empty() { return reopened; }` block | `let reopened: Vec<Command> = Vec::new();` | 7 (incl. ratchet, **16 broken**) |
| M89 | `parallel.rs` | `.filter(\|&ix\| !is_terminal(state, ix))` in `fail_the_rest` | *(deleted)* | 5 (incl. ratchet, 5 broken) |
| M90 | `plan.rs` | `out[ix].errors = flatten_list(out, &seq.errors, depth + 1)?;` | `let _ = &seq.errors;` | 5 (incl. ratchet, 3 regressions) |
| M91 | `plan.rs` | `retry: super::retry::policy_of(task.retry.as_ref()),` | `retry: None,` | **10** (incl. ratchet, 8 regressions) |
| M92 | `plan.rs` | `retry: super::retry::policy_of(flow.retry.as_ref()),` | `retry: None,` | 4 (incl. ratchet, 1 regression) |
| M93 | `plan.rs` | the `own_errors` half of `children_of` | `let mut out: Vec<TaskIx> = Vec::new();` | **0 — and provably so; see below** |
| M95 | `sequential.rs` | `Some(s) if s.is_terminal() => {}` in `walk` | Task 3's `if *s != Success { return Done(*s) }` | 1 |
| M96 | `mod.rs` | `work.extend(retry::scan(plan, state, now));` | the scan's commands placed **before** the drive's | 7 (incl. ratchet, **12 broken**) |
| M97 | `retry.rs` | `ix = state.runs[cur].parent;` in the `markParents` walk | `ix = None;` | 1 |
| M98 | `parallel.rs` | `fail_the_rest`'s whole `doomed.sort_unstable(); …collect()` tail | `Vec::new()` | 1 |

Messages worth quoting, because they are the ones a maintainer will see:

- M68: `` `maxAttempts: 4` is four attempts in total, not four RETRIES: Kestra's guard is `attemptNumber() >= maxAttempts` `` — and 9 entries go `STUCK`, because a retry that never exhausts never lets its execution finish.
- M77: `PT0.25s must parse as Java's Duration.parse reads it` — a case-sensitive seconds designator drops `valids/retry-expo`'s whole policy in silence, because a policy that cannot be read is a task that does not retry.
- M80: ``attempt 0 must not carry the RETRYING that follows it`` — and **9 baselined entries stop matching**, which is the whole retry slice.
- M85: ``the execution takes FAILED on every round a retry is pending`` — the largest victim count in the table, and it is one `if` about who is asking.
- M91: ``21 corpus flows declare a readable `retry:` `` — ten victims for one field.

**Three mutations exist in round 0's table and not in this one, and none was
dropped for being inconvenient.**

- **M86 and M87** neutered Kestra's two `isRetrying` guards in `advance` —
  `handleEnd`'s early return, and the retry pass's own
  `!executor.getExecution().getState().isRetrying()`. Both killed **nothing**
  across the whole crate, so both guards were **deleted** rather than shipped
  with a comment nobody could check, which is the answer
  `parallel::drive_concurrent` already records for Kestra's "first created,
  leave". What makes them unreachable is a property of `retry::scan`: it writes
  `Retrying` onto the run and onto the execution in the same round, so no later
  round can see a `Failed` run while the execution is `RETRYING`. The trigger to
  watch for is a caller that applies a command batch **partially** — W3b's
  message-driven driver, the same trigger the `Parallel` guard names.
- **M94** neutered `if attempts == 0` in `next_retry_at`, and killed nothing.
  The condition is real — with zero attempts, `count >= max_attempts` is false
  and a `CREATE_NEW_EXECUTION` policy would schedule a retry for a run that
  never ran — but unreachable, because `scan` only offers `Failed` runs and
  every run this corpus can fail has started an attempt. So it is now a
  `NonZeroU32`, and the mutation is **unwritable** rather than unkilled: there
  is no line left to delete.

**M93 has no victims and that is a measurement rather than a gap.**
`Plan::children_of` walks `errors` because Java's
`AbstractBranch.allChildTasks()` is `concat(tasks, errors, _finally)`, and a
task hanging off an `errors:` inside a loop body is as absent from the parent's
`taskRunList` as one hanging off `tasks:`. The walk is kept — it is the
artifact's — and its cost here is derived rather than argued:
`an_errors_list_inside_a_loop_body_is_a_shape_the_corpus_does_not_reach` shows
that **not one of the 89 loop-body tasks is reached through an `errors:` list**,
so the two walks name the same set and the mutation is *equivalent* on this
corpus. A mutation that cannot change the answer cannot be killed; what the test
adds is the proof that it cannot, and the assertion that fails the day a corpus
makes it non-zero.

**M97's row is the one exception to "one replay of the final tree", and the
reason is that round 2's formulation was not a mutation at all.** It read
`while let Some(cur) = ix.take()`, and the loop body's very next
`ix = state.runs[cur].parent;` overwrites whatever `take()` left — the two trees
are identical in behaviour, which is why it reported 0 victims. It was
reformulated as `ix = None;` and re-run alone. That cannot disturb any other
row: a no-op mutation produces a tree byte-equivalent in behaviour to the
unmutated one, so no other row's victim set was ever computed against anything
but the final tree.

**Seven rows do not include the ratchet, and they fall into three groups.**

- **M70, M71, M75, M76, M79, M97 — rules the corpus does not reach.** The two
  `maxDuration` origins differ only for a run whose first attempt starts late,
  which the oracle's clock rule never produces; the exponential exponent and the
  random bound are invisible because the trace carries no clocks; the single
  `RUNNING` needs two retries due in one round; the `markParents` walk needs a
  `CREATE_NEW_EXECUTION` retry on a *nested* task, and all four
  `retry-new-execution-*` flows fail a root one. Each is held by a unit test
  written for it, and four of those tests exist **because** the first replay
  found the mutation unkilled.
- **M95 — the sequential walk's failure short-circuit.** Re-adding Task 3's
  `return Done(*s)` leaves the ratchet exactly where it is and fails only
  `what_the_order_restriction_removes_is_derived_and_egret_never_says_after`.
  The two readings agree on `FAILED` — which is the only non-success terminal
  the corpus puts mid-list — so what separates them is a `KILLED` or `RETRIED`
  child, and `current_list` switching the list out is what ends the sequence
  either way.
- **M98 — `fail_the_rest`'s body.** Deleting it outright is caught by **one**
  unit test, `one_invalid_sibling_fails_every_sibling`, and by nothing else.
  That is *What Task 4 had to change* confirmed rather than closed: the plan
  predicted `errors:` would make this rule comparable and it did not, for the
  reason recorded above — no corpus `Parallel` has both an `errors:` and a
  sibling still in flight. Its **bound** is comparable (M89 moves the ratchet);
  its **body** is not.

`cargo fmt` ran after the mutation set; nothing it touched changed by more than
whitespace.

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
| `ExecutionUpdatableTask` | **23** over-submitted runs at Task 5 and at Task 6 (19 at Task 3, across 16 baselined entries — the entry count has not been re-derived since) | `Labels`, `SetVariables`, `UnsetVariables`, `Exit` | **unclaimed** |
| `Subflow` | 22 baselined `runs` entries, **25** over-submitted runs after Task 6 (22 after Task 4, 18 before it) | `io.kestra.plugin.core.flow.Subflow` | **unclaimed — the word does not appear in the plan** |
| `LoopUntil`, and its alias `WaitFor` | **3** + **6** over-submitted runs | `io.kestra.plugin.core.flow.{LoopUntil,WaitFor}` | **W8** (spec §2.1) — the plan gained the row after Task 6 named it |
| `WARNING` | 10 golden `runs` entries, including `valids/retry-success` and `valids/task-allow-failure-runnable` | `AllowFailure` and `retry.warningOnRetry` | **unclaimed — and this table's state list said Task 7 owned it.** Task 7 reads `warningOnRetry` into `RetryPolicy` and does nothing with it, because `RunState` cannot hold `WARNING`; `AllowFailure` is planned opaque |
| `finally:` | the whole `finally-*` family — 11 baselined entries — plus the only two corpus flows that put an `errors:` on a `Dag` or a `Parallel` | `AbstractBranch._finally` and `Flow._finally` | **unclaimed — new at Task 7.** W1's model carries `Flow::finally_` and Task 7 deliberately did NOT add the task-level field beside `errors`, because no W3a task implements it and a field nothing reads is a dead field |
| the replay a `CREATE_NEW_EXECUTION` retry starts | 4 baselined entries (`retry-new-execution-*`), each right on every field but `spawned` | `AbstractRetry.Behavior.CREATE_NEW_EXECUTION` → `ExecutionService.replay` | **W6/W3b, with `Subflow` and the `Loop` iteration** — it is a third kind of sub-execution and `ExecutionState` has no field for any of them |

`Loop` left this table in Task 6's commit and one row was added in the same one.
**That row is not runs that arrived at Task 6, and Task 6's first draft of this
table said it was.** Measured against Task 5's reconstructed map: `LoopUntil`
was already 3 there and `WaitFor` already 6. They were over-submitted all along
and simply had no row here; *naming* them is the change, not the count.
`LoopUntil` is `AbstractBranch`'s other sibling, and W1's `models/task.rs`
carries it with a note saying W3 must walk its `tasks:` exactly as `Loop`'s.
Nothing in W3a does — and the spec's §2.1 gives it to W8, which is why the plan
now names it: the ownership existed and was unfindable from here, exactly as it
had been for `Subflow`.

`Loop` was on this list until Task 6; `If` and `Switch` until Task 5. The table
gained something in Task 5's commit:
`core.execution.Exit` was not over-submitted at all before Task 5 and
contributes 4 of the 23 now, because `Exit` lives inside an `If` and planning
the `If` is what gives its children a `PlanTask`. That is the fourth row growing
rather than shrinking, for the reason *What the oracle feeds in* records.

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

**That M2 measurement is Task 3's and has not been re-derived since.** Three of
the entries it counts are `If`/`Switch` flows, and both flowables have been
implemented since, so **21** is a figure to re-measure rather than to quote —
Task 5 did not re-run M2, and says so rather than adjusting the number by
argument. **Task 6 did not re-run it either**, and it has now aged further: 12
of the 21 are `Subflow` flows, and a `Subflow` inside a loop body is planned
today where it was not. Whoever next needs the number should measure it.

**And `Subflow` is the fifth unclaimed construct, which checking rather than
assuming is what found.** `If` and `Switch` were Task 5's and `Loop` was Task
6's, but **the word `Subflow` does not appear anywhere in
`docs/en/plans/2026-09-04-workflow-w3a-executor.md`** — no task creates a
`TaskKind` for it, and `Command::SpawnExecution` arrives in Task 6 for a *loop
iteration*, which is a different construct (a `Loop` body never gets a run in the
parent execution; a `Subflow` task does). It is a `core.flow.*` class, so it
looks owned to anyone reading the flowable list, and it is the largest single
block left: **22 baselined `runs` entries** use it, 25 of the over-submitted runs
are its (18 when this was written — a `Subflow` inside a loop body is planned
since Task 6), and 12 of M2's 21.

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
  about serialisation. **Since Task 4 the ratchet compares only those**: egret
  produces a relation for every pair, the golden does not, and comparing an
  assertion against an absence was costing `valids/parallel-nested` a bank on 3
  pairs the golden itself declines to state. Because egret's relation domain is
  measured at `{before, overlapping}` (**656** pairs after Task 7, 555 after
  Task 6, 557 after Task 5, zero `after`), a dropped pair
  is one Kestra was seen producing *both* values for, so dropping it is exactly
  accepting either. Derived and guarded by three tests — see *What Task 4 had to
  change, and what it banked*. Driven through the real `advance()` the
  projection removes **30 pairs across 9 entries** at Task 7 (25 across 7 at
  Tasks 5 and 6, 22 across 6 at Task 4). Task 7's 101 new pairs are all made by
  `errors:` runs — a run that exists is in a pair with every other run of its
  execution — and its two new entries are `valids/disable-error` (3) and
  `valids/exit-killed` (2), which reach an `errors:` list for the first time and
  whose run sets differ from Kestra's for reasons that are not Task 7's
  (`disabled:`, which belongs to no package in the programme spec, and `Exit`).
- **A kept `before` is not always reachable by a clockless implementation.
  Task 4 found the first; Task 5 found three more, on a second entry.** The
  relation egret produces comes from
  `advance()` round numbers, so two branches a `parallel` starts together and
  finishes together are `overlapping` no matter how long each took. Where
  Kestra's scheduler happened to serialise two branches the same way five times
  running, the golden records `before` and egret says `overlapping` — a mismatch
  about the instrument, not about the flowable.

  **It was one pair at Task 4 and is four across two entries at Task 5.** The
  three new ones are `valids/if`'s `(if-2, if-3-log)`,
  `(if-3-log, log-parallel-4)` and `(log-parallel-2, log-parallel-5)`.
  `valids/if` reproduces **all 12 runs** in Kestra's own `taskRunList` order
  with every parent, history and attempt identical, and **54 of its 57 kept
  ordered pairs**; it is a `Parallel` with `concurrent: 4` over nine branches, so
  its runs are created in batches as earlier branches terminate. Each of the
  three disagreements is a creation egret lands in the *same* round as a
  termination Kestra had already observed, which is what a machine that completes
  exactly one in-flight run per round does; the general mechanism is this
  bullet's, and the specific Kestra cycle was **not** measured. What was measured
  is that the entry's runs are identical to Kestra's, which is what says the
  disagreement is about the instrument rather than about the `If`. This section
  predicted one such entry and named Task 6 as the remaining candidate;
  **Task 5 found the second first.**

  The original, in detail: **`valids/dag`'s `(task3, task4)`.** `task4` depends
  only on `task2`, so Kestra's own filter ("all dependencies are terminated")
  creates it as soon as `task2` ends; all five rounds nonetheless saw `task3`
  end first, because both are `Log` tasks and one executor evaluation saw both
  terminations. egret's environment completes one in-flight run per round and
  cannot. That single pair is the whole reason `valids/dag` is still baselined —
  its runs, parents, histories, attempts and other 19 pairs all reproduce. The
  answer taken was the one this bullet prescribed: record the pair, do not
  weaken the relation, and do not make the `Dag` wait for quiescence to buy the
  bank — and the same answer was taken for `valids/if`.

  **Task 6 did not find a third, and could not have**, which is worth recording
  because this bullet named it as a candidate twice. A `Loop` creates **no run
  for its body at all** — every iteration is a separate sub-execution — so it
  adds no run whose creation could land in the same round as a termination
  Kestra had already seen. The order test says the same in the other direction:
  its removal map is unchanged at 25 pairs across the same 7 entries.

  **Task 7 was named as the next candidate and did not answer it**, which is
  recorded rather than glossed. It banked 16 entries and added 101 ordered pairs
  without a single one of them costing a bank — every entry it set out to bank
  banked whole — but "no entry is blocked *only* on an `order` pair" is a claim
  about the 163 that remain, and nobody has re-derived that population since
  Task 4 named `valids/dag` and Task 5 named `valids/if`. Whoever needs the
  answer should derive it rather than read this bullet's silence as one.
- **Nothing keyed on elapsed time is reachable, and there were TWO independent
  causes. Task 3 closed the second; the first stands.**
  1. *The environment, which stands, and Task 7 measured what it costs.* `now`
     moves only to a `not_before` something is already waiting for, so no tier
     advances the clock because a task took a while. Tasks 5 and 7 could have
     fixed that inside this oracle; neither did.

     The cost is now a number rather than a worry. **`valids/retry-failed-flow-duration`
     is bounded by `maxDuration: PT7S` and `interval: PT2S` with no
     `maxAttempts` at all.** Kestra ran **three** attempts; egret runs
     **four** — 0, 2000, 4000, 6000, and the fifth would be 8000, which is after
     0 + 7000. Every attempt here costs zero elapsed time, so more of them fit
     inside the same window, and the entry stays baselined for that and for
     nothing else. `a_retry_is_bounded_by_max_attempts_and_separately_by_max_duration`
     asserts the four rather than the three, with the reason on the assertion:
     an implementation cannot be judged against a number the instrument cannot
     produce. Its sibling `valids/retry-failed-task-duration` was already
     excluded from the golden for being nondeterministic across five harvests
     — three attempts in some rounds, four in others — which is the same
     mechanism seen from Kestra's side.

     So an implementation whose `maxDuration` is correct and one whose
     `maxDuration` is ignored still produce the same *banked* traces here. What
     separates them is `a_retry_is_bounded_by…`, which is a unit test, and the
     two mutations that delete each bound.
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
- **No banked entry takes an `else` branch, so the ratchet cannot see a
  predicate that is wrong in the truthy direction** (round 1, measured; still
  true after Task 7, and checked rather than assumed — **not one of Task 7's 16
  entries contains an `If` or a `Switch` at all**, so the population this bullet
  is about did not move). Of the
  eight entries Task 5 banks, no `else` branch and no `defaults` list is ever
  driven: `valids/switch` matches its `FIRST` case, `valids/fail-on-switch`
  matches nothing and has no `defaults`, two entries fail at the render before
  any branch is chosen, one is a refusal, and every remaining `If` renders
  truthy. Three mutants prove it rather than argue it —
  `chosen = then_` unconditionally, `is_truthy = !trim().is_empty()`, and
  `parse::<bool>().unwrap_or(true)` — and all three leave `executor_matches_golden`
  **passing**. What kills them is the lib unit tests plus the **falsy arm** of
  `the_caller_renders_and_a_failed_render_fails_the_run`, whose use of `"0"` is
  load-bearing and is the only assertion in the oracle binary that dies to all
  three. (The first two also fail
  `what_the_order_restriction_removes_…`; the `parse::<bool>` one does not, and
  even `an_if_takes_then_when_true_and_else_when_false` survives it.) So the
  dependency on W2's renderer that Task 5 introduces is one-directional: a
  renderer answering `"false"` where Kestra answered `"true"` is caught; the
  converse is not.

- **The caller's render context is narrow, and an expression outside it FAILS
  the run** (Task 5). `advance()` cannot render, so the oracle does; it supplies
  `inputs` (the values `corpus.json` records `harvest.py` as having posted, plus
  the flow's own declared `defaults:`) and `vars`, and nothing else. Kestra's
  `RunContext` also reaches task outputs, the loop `item`, `execution`,
  `taskrun`, `trigger`, `labels` and the KV store. Measured and asserted by
  `the_caller_can_render_every_expression_except_the_ones_named` (Task 5 spelled
  it `…_except_the_two_kestra_could_not`, before there were four): of the **26**
  planned `If` conditions and `Switch` values a run can exist for, **24 render**
  and the **2** that do not are exactly the two the artifact could not render
  either — the test reads each failing task's own run out of `golden.json` and
  requires it to end `FAILED`.

  **Task 6 widened the population and the outcome was the opposite of this
  file's prediction, measured.** The prediction was that a `{{ item.value }}`
  condition inside a `Loop` would become planned and fail. It became planned —
  **12** `If`/`Switch` expressions did, not the 16 a review had forwarded — and
  **none of them is rendered**, because a loop body's task never gets a run in
  this execution, so tier 2 never reaches it. The reachable branch population is
  still exactly 26 with the same 2 failures.

  What the narrow context *did* cost at Task 6 is two different entries:
  `valids/loop-outputs-auto` and `valids/loop-values-from-uri` both write
  `values: "{{ outputs.write.uri }}"`, `render_context` carries no `outputs`, and
  egret fails a `Loop` Kestra ran. They are asserted by name in a list of their
  own — separate from the two Kestra could not render either — and the test reads
  `golden.json` to require that Kestra's own runs for them ended **SUCCESS**.
  Both are baselined. A fifth failure would be a new one. Task 7 widens it
  again.

  **But those two are blocked twice over, and calling them the harness's fault
  was half right.** See the next bullet: the same expression renders to an ION
  file URI, which `advance()` cannot resolve either.

  **No `item` is supplied, deliberately.** Kestra renders those 12 inside the
  sub-execution with `item` from `loopRun`; inventing one here would put a
  made-up value behind a branch decision and change nothing this oracle can see,
  since no run for those tasks exists. Ten of the 18 loop-body expressions name
  `item`; the other eight are six nested `Loop` `values:` literals and two `If`
  conditions over `inputs:` this context could render perfectly well — which is
  what says the exclusion is about *no run existing* and not about the context
  being too narrow.

- **Two shapes of the render tier are invisible to the RATCHET and not to the
  oracle** — a distinction this file got wrong in round 0 and a review caught.
  Both variants were built on the final tree and run over the whole golden:

  | variant | the ratchet | the oracle binary |
  |---|---|---|
  | render one run per step, not all | **unmoved**, 181 mismatching | `what_the_order_restriction_removes_is_derived_and_egret_never_says_after` **FAILS** |
  | let the worker move before the render | **unmoved**, 181 mismatching | the same test **FAILS** |

  **Both rows are Task 5's measurement on Task 5's tree and were NOT re-derived
  at Task 6**, which added a third kind to that tier and a `Loop` that renders at
  `CREATED`. The counts are 181 because that was the baseline then; they are
  quoted as history, not as current behaviour.

  In both, `valids/if` joins that test's `runs_differ` list — its run **order**
  stops matching Kestra's `taskRunList`. So the shipped shape is the one that
  reproduces Kestra's creation order and a committed test says so; neither the
  ordering nor the batching is free. Round 0 said "the batching is free" and
  quoted **182**, which was the count from *before* the `defaults:` fix banked
  the eighth entry: the ratchet's count was read and the binary was not.

- **`PlanTask::timeout` is a dead field and will stay one; the other two are
  not, and the reason the first pair was ever grouped was wrong.** This bullet
  used to say `build_plan` left `retry` and `timeout` `None` "because Kestra
  spells both as `Property<Duration>`". Measured at the artifact, that is true
  of exactly one of them: `Task.java:59` is `protected Property<Duration>
  timeout;` and `Task.java:56` is `protected AbstractRetry retry;`, a plain
  POJO whose every field (`maxDuration`, `maxAttempts`, `Constant.interval`,
  `Exponential.interval`/`maxInterval`, `Random.minInterval`/`maxInterval`) is a
  plain `java.time.Duration` or `Integer`. **Kestra never renders a retry**, so
  a pure builder can read one, and Task 7's does. The `Property` wrapper around
  those durations is W1's own, added where Kestra has none.

  `timeout:` stays dead for a *different* reason, and it is not a reason a later
  W3 task can close: the **worker** enforces it
  (`WorkerTaskCallable.java:58` wraps `task.run(runContext)` in
  `callWithTimeout`), and Kestra's own validator refuses it on every
  non-`RunnableTask` (`FlowService.java:574`), with `Pause` the single
  exception. W4 owns it. The plan's Task 7 step said "`timeout` is a deadline
  compared against the `now` parameter"; it is not, and no line of Task 7
  implements one.

  `errors` was a model gap and is closed: W1's `Sequential`, `Parallel`, `If`,
  `Switch` and `Dag` now all carry `errors` (Java declares it on all five —
  `AbstractBranch`, `If.java:111`, `Switch.java:112`, `Dag.java:106`), and
  `build_plan` flattens it. **`finally` deliberately does not follow**: no W3a
  task implements it, and a field nothing reads is a second dead field rather
  than a smaller version of the same fix. That costs the whole `finally-*`
  family — `finally-parallel`, `finally-dag`, `finally-sequential`,
  `finally-loop`, `finally-flow` and their variants, all still baselined — and
  two of them, `valids/finally-dag` and `valids/finally-parallel`, are the ONLY
  corpus flows that put an `errors:` on a `Dag` or a `Parallel`, so those two
  arms of the implementation are reached by no comparable entry at all.
  `every_corpus_errors_list_is_on_a_flowable_egret_plans` derives that
  distribution: 17 flows write a flow-level `errors:` and 9 tasks write one, of
  which 7 are on a `Sequential`, 1 on a `Parallel` and 1 on a `Dag`; `If` and
  `Switch` carry the field because Java does and no corpus flow writes one
  there.
- **Sub-executions are ASKED FOR and still not produced, and Task 6 did not
  close this.** `Command::SpawnExecution` is emitted; nothing executes one until
  W3b/W6, and `ExecutionState` holds no field for a sub-execution. So egret's
  side of `spawned` is still **always empty** and every one of the **41** golden
  entries that carries a sub-execution mismatches on that field alone. **38 of
  the 41 now differ on nothing else** — they reproduce every run, parent,
  history, attempt and kept ordered pair Kestra recorded, and are baselined
  purely for a field egret does not fill. It was 33 at Task 6; the five Task 7
  adds are the four `retry-new-execution-*` entries and
  `valids/retry-flowable-parallel`. A `CREATE_NEW_EXECUTION` retry ends its
  execution `RETRIED` and Kestra then **replays the whole flow as a different
  execution**, which is a sub-execution of a second kind: it carries
  `parent_run: null` rather than a loop run, and nothing in `Command` can ask
  for one. The remaining three are `valids/disable-flowable` (`disabled:`,
  unclaimed) and `valids/loop-outputs-auto` / `valids/loop-values-from-uri`
  (the ION-file `values:` shape).

  Those entries look compared and are not.
  `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` is the
  marking the plan asked for: it derives all three numbers, requires all 41 to
  be baselined, and names the 8 that differ on more with a reason each. What
  **is** compared is the first batch of spawn *requests* — `(parent_run, value)`
  for **52** of the 142, over 36 loop runs, by
  `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`. **No
  sub-execution's TRACE is compared at all**, and none can be until something
  executes the command.

  Three further holes follow from the same one:
  - **There is no second batch.** `advance()` emits `min(limit, size)` and
    cannot schedule more, so an implementation that scheduled the rest wrongly
    is indistinguishable here.
  - **`concurrencyLimit` is judged only through that first batch.** The four
    mutations that get it wrong (M48, M49, M50, M61) are invisible to the
    ratchet and die only to the prefix test and two unit tests.
  - **`transmitFailed` and the loop's aggregation rule are invisible to the
    RATCHET**, because a `Loop`'s terminal state is fed — but not to this oracle:
    two independent wrong-aggregation constructions (M66, M67) each die to
    `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` and
    to the order test. Its **33** is a guard, not only a report. Task 6's first
    account said "judged by nothing", which was wrong in the pessimistic
    direction. See *What the oracle feeds in*.
- **One of Kestra's three `values:` shapes is not implemented, and it is a hole
  in `advance()` rather than in the harness.** `Loop.java:210-214` documents a
  string, an object/list, **and an ION file URI**; the third never reaches
  `resolveValues` at all (`FlowableUtils.resolveLoopValuesUri`,
  `Loop.initFromUri`), and `iteration_values` implements only the
  `resolveValues` arms. Two comparable entries are that shape —
  `valids/loop-outputs-auto` and `valids/loop-values-from-uri` — and they would
  fail inside the executor even with a perfect render context; Task 6's first
  account filed the whole of it under the narrow `outputs`-less context, which
  was half the reason. Closing it needs a decision nobody has taken: `advance()`
  performs no I/O, so the **caller** would have to fetch the ION file and put the
  coalesced lines into `TaskRun::rendered`. Neither entry could bank on that
  alone — this oracle has neither the `outputs` nor the storage.
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
