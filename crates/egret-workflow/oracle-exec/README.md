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

> **Read *The coverage statement* first — it is the last section, and after
> W3b it is the only one whose numbers are current.** W3b Task 5 replaced the
> test harness behind this ratchet with the **real driver** (a claim loop over
> Postgres, five handlers, a spawner), so what the ratchet judges is the
> shipping system rather than four tiers of test code. Everything from *The
> normalised trace* down to *W3b Task 4* is a record of what a task measured on
> the day it ran, and several of those numbers are now false. The five a reader
> is most likely to pick up by accident:
>
> - **the sub-execution hole** is written three times in this file as *33*, *38*
>   and *39* "of the 41 differing on nothing but `spawned`", each true at a
>   different moment. It is now **16 of the 41 MATCH, 6 differ only *inside*
>   `spawned`, 19 differ on more** — a different sentence, because the field is
>   filled rather than empty;
> - **`Command::SpawnExecution` has an executor** (`worker::spawn`) and this
>   oracle drives it. Two places still say "nothing executes one until W3b/W6";
> - **nothing is fed from `golden.json` any more** — 630 run terminals were,
>   575 `Runnable` + 55 `Loop`, and the circular field is gone;
> - **"the kept 1104 are the whole of what this oracle asserts about
>   serialisation"** is wrong for the second time. **354** are; the other 750 sit
>   on baselined entries and cannot fail, which was as true under W3a as it is
>   now;
> - **`egret.sql` ships with no oracle coverage at all** — 0 of 345 corpus flows
>   and 0 of 294 golden entries — and its own suite self-skips without an engine.
>
> The coverage statement carries the full list, the derivation of every count,
> and what a green ratchet still does not prove.

**W3c** is a follow-on package, not a W3a task, and it closes the last
over-acceptance W3a named but left unowned: **`runIf:`**. A task whose `runIf:`
renders falsy is *created* by Kestra and then not executed — `CREATED SKIPPED`,
no `SUBMITTED`, no `RUNNING` — and egret ran it. That is the same class as
`disabled:`, which W3a's final review closed. Four sentences below are now
stale and a reader should take these over them: **`RunState` holds 9 of the
golden's 13 states**, not 8; `SKIPPED` is no longer in the missing list, so
**four** remain, not five; `runIf:` is gone from *States and constructs no W3a
task claims* and from the residual-baseline table; and the order-pair population
is **642**, not 647. **The ratchet did not move: 144 mismatching before and
after, 0 banked and 0 regressions**, and *What W3c had to change, and what it did
NOT bank* is the section that says why a correct fix banks nothing here.

**W3b** is the package after W3c, and its Task 1 adds `handler-surface.txt` and
`handler_surface.py`: the measurement that bounds what a real driver can bank
here. Three things in it a reader should take over anything below. The plan's
**194** is **201**, and 194 counts triggers as tasks and classifies task types by
their Java package. `advance()`'s four handlers are `Log`, `Return`, `Fail` and
`Sleep`, and **three of the four produce no output at all** while a `Fail` with a
falsy `condition:` does not fail. And **the ratchet can go DOWN when this oracle
is re-pointed**: 11 of the 150 entries it counts as matching agree only because
the harness feeds a terminal for a task type W3b writes no handler for. See
*W3b Task 1 — the handler surface, and the ceiling it puts on W3b*.

Task 8 (**refusal**) changed four, and a reader should take these over
everything below. **`build_plan` now runs `spec::validate`**, so egret has one
answer to "is this flow acceptable" instead of two — the largest single cause of
the over-acceptance was a missing *call*, not a missing rule. The `refused`
count in *Refusal: 22 over-accepted…* is superseded: **23 of the 28 are refused
and 5 are named, with an owner each**, and the five are asserted to be
over-accepted so that one becoming a refusal fails a test rather than passing
quietly. The rejection classification is re-derived here at **51 rejections, 28
of them evidence** — the plan's own Task 8 step says 45 and 18 and is stale.
And running `validate` brings **two pre-existing over-refusals** with it, on
flows this Kestra published with 200 and which have **no golden entry at all**,
so the ratchet cannot see either; they are a committed test instead. See *What
Task 8 had to change, and what it banked*, and *The coverage statement*, which
now closes this file.

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

**Task 7's fix round 1 answered six mutations that killed nothing, and round 2
answered a seventh guard that could not fail — the first in this package that
was hiding something.** `every_corpus_errors_list_is_on_a_flowable_egret_plans`
matched on W1's parsed kind, so a type with no `errors` field scored zero by
construction; reading the raw YAML instead raises the count from 9 tasks on
three types to **18 on seven**, and surfaces a `Loop`'s own `errors:` as
unclaimed debt (4 baselined entries). The same round corrected a `plan.rs` doc
that its own commit contradicted (`errors` went onto **five** model types, not
two, and `If`/`Switch`/`Dag` are not opaque), moved the
`WorkerTaskCallable.java` citation from `:58` to `:61`, and withdrew
"unwritable" to "there is no line left to delete".

**Round 1 answered six mutations that killed nothing**, and a
reader should take these over the round-0 sentences they replace. Two guards
Kestra has and this executor cannot reach — `handleEnd`'s and the retry pass's
`isRetrying` early returns — were **deleted** rather than shipped as comments;
one was made **unrepresentable** (a `NonZeroU32` in place of an `attempts == 0`
check, so there is no line left to delete); three were kept and
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

> **Nothing in this section is current. It is the derivation of the number
> W3b Task 5 took to zero, and it is kept for that.** This oracle feeds **no**
> run terminal now: `oracle-exec/driver.rs` takes a flow source and the posted
> inputs, runs the execution through `worker::tick` against Postgres, and reads
> the trace back out; nothing on egret's side reads `golden.json`. The **630**
> terminals this section counts — 575 `Runnable` runs and 55 `Loop` runs — are
> what W3a's harness supplied and the whole of why the oracle was circular for
> one field. See *The coverage statement*, which also carries the demonstration:
> corrupting a terminal on an entry that **matches** now breaks it, and the
> control that says why the demonstration needs a matching entry.

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
| **flowable** | 109 | the run's task is one the executor drives **and decides for** itself (`Sequential`, `Parallel`, `Dag`, `If`, `Switch`), so its state is compared |
| **unplanned** | 101 | the run's task has no `PlanTask` at all — it lives inside a flowable `build_plan` still plans opaque and so does not descend into |
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
**The null-list fix moves exactly one**, `unplanned` 102 → 101 and `flowable`
108 → 109, and it is not a flowable the executor learned. It is
`valids/switch-impossible`'s single `impossible` run: `build_plan` **refused**
that flow, and a flow with no plan has every run of it *unplanned* whatever its
tasks are. An over-refusal is invisible in this table except as an entry in the
one bucket that means "egret has nothing to say about this" — which is the
second thing wrong with one, after the 400 it returned on the HTTP API.

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
weaken with it. (W3c implements `runIf:`, so egret no longer submits those two
either — but the *feed* split this table describes is derived from the golden
and the plan, not from a simulation, so none of these numbers moved.)

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
unrepresentable. **Task 3 added `SUBMITTED`, Task 7 added `RETRIED`, W3c added
`SKIPPED`**, so the enum has nine and **4 of the 13 remain unrepresentable**:
`PAUSED`, `RESTARTED`, `WARNING`, `CANCELLED`. Each is recorded here rather than
worked around in the normaliser, because a normaliser that quietly maps an
unrepresentable state onto a representable one is a normaliser that hides the gap
it was built to expose.

**The test that derives all of this could not see the enum until W3c.**
`the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` carried its
own literal list of the eight state names, so it measured a list in the test
file against the golden and never once read `RunState` — adding `Skipped` left
it green. It now derives the vocabulary from `RunState::ALL`, which is the
single list, and `run_state_all_holds_every_variant_in_order` is what keeps that
list and the enum in step.

**What each addition bought, and what the four still cost.** Of the 266 `runs`
entries, **35** use only the six members the plan drafted, **229** use only the
seven, **233** use only the eight and **237** use only the nine — so `SUBMITTED`
alone is what moved the reachable set by 194 entries, `RETRIED` moves it by 4 and
`SKIPPED` by 4,
and it was the single cheapest thing any task could do to this oracle. It is
also not optional for Task 3's own slice: `SUBMITTED` is in 580 of the golden's
840 run histories and on every task the worker ran, so a sequential flow of
`Return` tasks cannot match without it.

**`SKIPPED`'s 4 buy no bank either, and for a different reason.** `RETRIED`'s
four are blocked by a replacement execution nothing produces; `SKIPPED`'s four
are blocked by a *second* construct each: `valids/task-runif` declares a
`Subflow`, `…-executionupdating` an `ExecutionUpdatableTask`, `…-taskswithstate`
a `finally:`, and `…-workingdirectory` a `WorkingDirectory` planned opaque.
Being inside the vocabulary is necessary and not sufficient, and this is the
second measured example of that.

**And `RETRIED`'s 4 buy no bank at all**, which is worth saying beside the
number: the four entries it covers are exactly the `retry-new-execution-*`
flows, and every one of them carries a **replacement execution** in `spawned`
that egret cannot create. The state was still worth adding — without it those
four are `FAILED` where Kestra recorded `RETRIED`, which is a wrong answer rather
than a missing one — but it moved them from *wrong on several fields* to *right
on everything except the field nothing produces*. See the sub-execution hole.

The other four, with the number of `runs` entries whose trace uses each and the
task that owns it:

| state | `runs` entries | owner |
|---|---|---|
| `PAUSED` | 18 | `Pause`, which no W3a task claims — see the hole list |
| `RESTARTED` | 12 | execution histories only; a restart is W3b's, not `advance()`'s |
| `WARNING` | 10 | `AllowFailure` and `warningOnRetry` — **not Task 7's**, and this row said it was. `warningOnRetry` turns a retried task's eventual SUCCESS into WARNING, so `valids/retry-success` and `valids/task-allow-failure-runnable` need it; `AllowFailure` is planned opaque and belongs to W8. Unclaimed |
| `CANCELLED` | 3 | flow concurrency, which no W3a task claims |

`SKIPPED` was a fifth row, 4 entries, credited first to `disabled: true` (wrong)
and then to `runIf:` (right). **W3c implements `runIf:`**, so the row is gone
from here as well; see *What W3c had to change, and what it did NOT bank*.

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

> **Superseded by Task 8, and kept because it is the record of the debt.** The
> 22 became 19 (Task 4 banked two `Dag` refusals, Task 5 one `If`) and then
> **5**. The 6 became **23**. The "two are right by accident" paragraph below is
> still true and has gained a measurement: Task 8 rehearsed W1's fix on all
> three flows and it costs no over-acceptance. See *What Task 8 had to change*.

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

**Task 8 measured what the fix costs, and it is nothing.** Since `build_plan`
runs `validate`, deleting the offending token from a flow is a complete
rehearsal of W1's fix on that flow. `valids/switch-impossible` then **plans**;
`invalids/switch-invalid` is refused by `switch_no_branches` and
`invalids/invalid-parallel` by `parallel_tasks_required` — each the reason
Kestra itself gives. So the two accidental refusals survive the fix on their
merits. The rehearsal is in
`a_null_task_list_is_an_over_refusal_that_reaches_the_http_api`, which used to
assert `is_ok()` for those two and now asserts the code.

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
(555 after Task 6, 656 after Task 7, **647** after the final review; the zero
has never moved). A pair is
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
The population is **647** after the final review (656 after Task 7, 555 after
Task 6), and the assertion of zero `after` is unchanged. Task 7 adds 101 pairs
and every one of them is made by an `errors:` run — a run that exists is in a
pair with every other run of its execution — so that growth is a new
*population*, not a new relation. The final review takes 9 back off, for the
mirror-image reason: implementing `disabled:` deletes runs egret used to create
and Kestra never did, and a run that no longer exists is in no pair at all.

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
  **15 across 4 entries** after the final review and is the number the ratchet
  prints. It was 22 across 6 at Task 4, 25 across 7 at Tasks 5 and 6, and 30
  across 9 at Task 7; five of those nine entries were `disabled:` flows, and
  implementing `disabled:` deleted both the runs and the pairs they made. The 2
  beyond the golden's own 13 are on `valids/exit-killed`, whose egret run set
  *differs* from Kestra's (`Exit` is an `ExecutionUpdatableTask`, unclaimed), so
  its pair indices do not denote the same pairs and such an entry can never bank
  anyway (`runs` is compared first and verbatim). The test asserts that
  separation, so an entry losing pairs while its runs DO match shows up as a
  changed map. **A shrinking projection is the oracle seeing more, not less.**

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

**Task 8 re-measured it and took it, and the number moved for a reason rather
than by drift: it is 7 of 19, not 8 of 20.** `invalids/if-without-condition` was
the eighth and Task 5 banked it in the builder, so the population shrank by one
at both ends. The other 12 that `validate` would not refuse are the ones Task 8
had to write new rules for — or name with an owner, which is what happened to
five of them. What made the decision safe rather than a judgement call is a
measurement this section did not have: over the golden's **266 `runs` entries**,
`validate` reports **zero** errors, so the gate refuses nothing this Kestra
accepted. M8's danger was a rule that refused for a reason unrelated to the
flow; this one refuses exactly what Kestra's own validator refuses, and the
ratchet's regression arm is what says so.

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
`defaults: []`, and `defaults: null` was refused by W1's parser, so the two
spellings this crate could tell apart were the two that occur.

**The null-list fix does not widen this gap, and the corpus says which side it
lands on.** `defaults: null` now parses to the same empty `Vec` an absent
`defaults:` gives — and on the artifact those two spellings behave *the same*:
a null field and an absent one are both `null` after Jackson, so both end the
`Switch` `FAILED` when no case matches. `defaults: []` is the odd one out, and it
is the one nothing writes: measured over the whole corpus, `defaults: null`
occurs in exactly two flows (`valids/switch-impossible`,
`invalids/switch-invalid`) and `defaults: []` in none.

`valids/switch-impossible` is the flow that reaches the question — its input is
`"impossible"` and its only case is `"NO"`, so no case matches and the empty
defaults decide the outcome — and it is banked: egret reproduces Kestra's
`FAILED` on it exactly. So the spelling the fix newly accepts is one egret now
gets *right*, and what stays unrepresentable is `defaults: []` alone. Closing
that needs an `Option<Vec<Task>>` and a `drive_switch` arm for the third state;
nothing measures it, so it stays a paragraph rather than a test. Owner: W1, with
W3b's `drive_switch`.

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
`Loop`, with 81 tasks inside a loop body between them, and not one of those
tasks has a run in its parent** — on either side.
`no_run_is_ever_created_for_a_task_inside_a_loop` derives that from egret's
traces *and* from Kestra's own golden, because if Kestra ever did put one there,
`advance()` refusing to create one would be the defect rather than the rule.

`Plan::is_loop_body` is the predicate, and "inside" is the **whole subtree**, not
the direct children. Nothing forces that from the type — a narrow reading
compiles and passes everything else, because `advance()` never calls the method
at all — so the two readings are measured against each other: over all 345
corpus flows the wide one names **88** tasks and the narrow one **63**. (82,
89 and 64 until the final review implemented `disabled:`, which deletes
`valids/disable-flowable`'s `1-each-2` — a direct child of a `Loop` — from the
plan. The gap of 25 between the two readings is unchanged: the task left both at
once.)

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
| of the 41, entries that differ on **nothing but `spawned`** — 33 at Task 6, 38 at Task 7, **39** at the final review | **39** |
| sub-executions whose `(parent_run, value)` IS compared | **52** |
| sub-executions whose **trace** is compared | **0** |

*(**Every row of this table is W3a's and the last three are false now.** This is
the fourth moment the "differ on nothing but `spawned`" quantity was written
down, and the whole quantity is retired: the field is filled, so the question is
which entries **match** (16 of the 41) rather than which are held only by an
empty field. 35 entries produce a sub-execution trace and 126 ordered pairs
inside one. The table is kept because it is what the final review measured; see
*The coverage statement*.)*

`the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` asserts
the first three, requires all 41 to still be baselined, and names the ones that
differ on more than `spawned` with the reason for each. There were 8 at Task 6:
four `retry-new-execution-*` replacement executions and one
`valids/retry-flowable-parallel`, which Task 7 closed; one `Loop` whose first
iteration failed and whose flow-level `errors:` then ran, which Task 7 closed
too; `valids/disable-flowable`, which the final review closed by implementing
`disabled:`; and the two `values: "{{ outputs.write.uri }}"` flows the harness
cannot render, which are **the only two left** and are the harness rather than
`advance()`.

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
believed. Both entries are baselined, and after the final review they are the
**only two** that differ on more than `spawned` — it was 8 at Task 6 and 3 at
Task 7.

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

- `worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:61`
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
| M90 | `plan.rs` | `out[ix].errors = flatten_list(out, &seq.errors, depth + 1)?;` | `let _ = &seq.errors;` | **6** (incl. ratchet, 3 regressions) — re-run at round 2, see below |
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

**M90 was re-run at round 2, and it is the row that shows the widened guard can
now fail.** Round 2 rewrote
`every_corpus_errors_list_is_on_a_flowable_egret_plans` to read the raw YAML and
then ask the plan, which gave it a dependency on `build_plan` it did not have
before. Exactly one row can move as a result — M90 is the only mutation that
touches how `errors:` reaches the plan (M91 and M92 are retry-only, M93 is
`children_of`, and nothing else is in `plan.rs`) — and it does, from 5 victims
to 6, the sixth being that guard. Round 2's other change,
`no_captured_execution_has_a_duplicate_taskrun`'s non-vacuity assertion, reads
`traces.json` and no crate code under mutation, so it moves nothing.

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
| `disabled:` | ~~`SKIPPED`, 4 entries~~ — **the state was misattributed and the construct is now implemented**; see the two rows below | a task-level flag | **W3a's final review took it.** `flatten_list` drops a disabled task from the plan, which is Kestra's own rule (`Execution.removeDisabled`, called on `tasks`, `errors` and `finally` inside `findTaskDependingFlowState`): no taskrun at all, and a flowable whose children are all disabled still succeeds. 4 of its 6 baselined entries banked |
| `runIf:` | ~~**6 runs across 4 entries** — the golden's whole `SKIPPED` population — and 7 baselined entries~~ — **implemented by W3c** | a task-level expression, rendered per run | **W3c took it**, and both halves moved in one commit: W1's `Task` carries `run_if` and `RunState` has `Skipped`. **None of the 7 banks and none of the 6 skipped runs is reproduced under this oracle** — not because the rule is wrong but because the harness cannot render any of the four flows' conditions; see *What W3c had to change, and what it did NOT bank* |
| `ExecutionUpdatableTask` | **23** over-submitted runs at Task 5 and at Task 6 (19 at Task 3, across 16 baselined entries — the entry count has not been re-derived since) | `Labels`, `SetVariables`, `UnsetVariables`, `Exit` | **unclaimed** |
| `Subflow` | 22 baselined `runs` entries, **25** over-submitted runs after Task 6 (22 after Task 4, 18 before it) | `io.kestra.plugin.core.flow.Subflow` | **unclaimed — the word does not appear in the plan** |
| `LoopUntil`, and its alias `WaitFor` | **3** + **6** over-submitted runs | `io.kestra.plugin.core.flow.{LoopUntil,WaitFor}` | **W8** (spec §2.1) — the plan gained the row after Task 6 named it |
| `WARNING` | 10 golden `runs` entries, including `valids/retry-success` and `valids/task-allow-failure-runnable` | `AllowFailure` and `retry.warningOnRetry` | **unclaimed — and this table's state list said Task 7 owned it.** Task 7 reads `warningOnRetry` into `RetryPolicy` and does nothing with it, because `RunState` cannot hold `WARNING`; `AllowFailure` is planned opaque |
| a `Loop`'s own `errors:` | 4 baselined entries — `valids/all-flowable`, `valids/finally-loop`, `valids/finally-loop-parallel` and `invalids/loop-switch-failed`, which is a `runs` entry despite its directory | `Loop.errors`, resolved by Kestra inside the **sub-execution** (`Loop.java:272`) | **unclaimed — new at Task 7 fix round 1, and found by a guard that could not fail.** `models::Loop` has no `errors` field, so `plan.rs` has no `Loop` arm. It is filed beside `Subflow` and the `CREATE_NEW_EXECUTION` replay rather than as a missing `match` arm: a `Loop`'s `errors:` runs in an execution W3a does not create |
| `finally:` | the whole `finally-*` family — 11 baselined entries — plus the only two corpus flows that put an `errors:` on a `Dag` or a `Parallel` | `AbstractBranch._finally` and `Flow._finally` | **unclaimed — new at Task 7.** W1's model carries `Flow::finally_` and Task 7 deliberately did NOT add the task-level field beside `errors`, because no W3a task implements it and a field nothing reads is a dead field |
| the replay a `CREATE_NEW_EXECUTION` retry starts | 4 baselined entries (`retry-new-execution-*`), each right on every field but `spawned` | `AbstractRetry.Behavior.CREATE_NEW_EXECUTION` → `ExecutionService.replay` | **W6/W3b, with `Subflow` and the `Loop` iteration** — it is a third kind of sub-execution and `ExecutionState` has no field for any of them |
| `afterExecution:` | **10** baselined entries | `Flow.afterExecution` | **unclaimed — new at Task 8.** W1's `Flow::after_execution` parses and `spec::validate` walks it; `build_plan` flattens `tasks` and `errors` and nothing else, so those tasks are in no plan. It is `finally:`'s sibling and unclaimed for the same reason |
| `sla:` | **5** baselined entries | `Flow.sla`, `SLA.Type.{MAX_DURATION,EXECUTION_ASSERTION}` | **W8** (programme spec). W1 ports it as a data shape and says so; nothing evaluates one, and the row is here because a reader of the residual baseline had no way to learn that from this file |

**Two rows are Task 8's, and checking the REMAINDER rather than trusting the
table is what found them** — the same method Task 3 used for `Subflow` and Task
6 for `LoopUntil`. `the_residual_baseline_is_attributed_to_a_construct_or_named`
derives the whole table from the baselined flows' own YAML and then asserts, by
name, the **13** entries that declare none of these constructs. `afterExecution:`
and `sla:` were both in that remainder, and neither appeared anywhere in this
file or in the plan. The test is what stops the next one arriving silently: a
new baselined entry belonging to no owner fails it.

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

## What Task 8 had to change, and what it banked

Task 8 owns **refusal** — egret answering where Kestra refuses, which is the one
divergence class the programme forbids shipping. It banks **14** of the 163
entries Task 7 left, taking `baseline.txt` to **149**.

| | golden entries | mismatching | baselined | broken | ordered pairs removed |
|---|---|---|---|---|---|
| **control** — clean tree at BASE `936d54b0`, `git status --porcelain` empty | 294 | **163** | 163 | 0 | 30 |
| **after** | 294 | **149** | 149 | 0 | 30 |

**14 banked, 0 regressions, 0 broken.** Every one was in `baseline.txt` at the
control run, which for a ratchet is what "shown failing before the code that
earns it" means.

| banked | what earns it |
|---|---|
| `invalids/duplicate` | `duplicate_task_id` — a rule `validate` already had |
| `invalids/duplicate-parallel` | the same, nested six levels deep |
| `invalids/duplicate-inputs` | `duplicate_input_id` — also already there |
| `invalids/empty` | `empty_tasks` — also already there |
| `valids/invalid-draft-flow` | the same rule, on the one `refused-by-both` entry |
| `invalids/recursive-flow` | `self_subflow` — also already there |
| `invalids/workingdirectory-no-tasks` | `working_directory_tasks_required` — also already there |
| `invalids/duplicate-key` | **new**: the same key twice in one YAML mapping |
| `invalids/workingdirectory-invalid` | **new**: a `Pause` as a child of a `WorkingDirectory` |
| `invalids/scheduled-missing-inputs-defaults` | **new**: a `Schedule` and an input with no default |
| `invalids/scheduled-missing-inputs-defaults-2` | the same, with no defaulted input to distract |
| `invalids/inputs-validation` | **new**: `itemType` on an `ARRAY`/`MULTISELECT` |
| `invalids/inputs-key-with-subtraction-symbol-validation` | **new**: `{{inputs.server-fqn}}` |
| `invalids/outputs-key-with-subtraction-symbol-validation` | the same, on `outputs.` |

**Seven of the fourteen needed no new rule at all** — `validate` had been
refusing them at `POST /flows` since W1 and `build_plan` never asked.

### The rejections, classified by CAUSE

`corpus.json` records 51 rejections. **28 of them are evidence that egret must
refuse a flow**, and they are the golden's 28 `refused` entries:

| class | count | Task 8 evidence? |
|---|---|---|
| `invalid-flow` | 27 | **yes** |
| `refused-by-both` | 1 | **yes** — `valids/invalid-draft-flow` |
| `task-type-not-in-image` | 22 | no — the pinned image lacks a test-only plugin class |
| `checkout-drift` | 1 | no — the checkout has a feature the artifact does not |
| `unclassified-rejection` | 0 | — |

The plan's own Task 8 step says "45 rejections … the other 18". **That is stale
and this file's `rejected is four different things wearing one status code`
table is right**: the counts are 51 and 23, re-derived here from `corpus.json`
rather than copied. The `invalids/` directory is 28 flows and only **27** of
them are refusals — `invalids/loop-switch-failed` publishes, runs, and reaches
`FAILED`; Kestra's own `RunContextTest.failedTasksVariable` executes it. It
carries `accepted_though_invalid` and is a `runs` entry.

### The seam: `build_plan` called `parse` and not `validate`

**The largest single cause of the over-acceptance was not a missing rule. It was
a missing call.** W1's `spec::validate` already carried `duplicate_task_id`,
`duplicate_input_id`, `empty_tasks`, `self_subflow` and
`working_directory_tasks_required`; `build_plan` never ran it, so egret had two
different answers to "is this flow acceptable" — `POST /flows` refused seven
flows that `advance()` would happily plan.

Task 4 measured this at **8 of the then-20** remaining refusals and named them
id for id. **Re-measured at Task 7's HEAD it is 7 of 19**, and the difference is
not drift: `invalids/if-without-condition` was the eighth, and Task 5 banked it
in the builder.

Running `validate` there costs nothing on the accepting side, measured rather
than argued: over the golden's **266 `runs` entries** — every flow this pinned
Kestra published *and ran* — `validate` reports **zero** errors.

### The gate order is measured, and the message assertion is what measured it

The first version ran `validate` **before** the builder's own structural checks.
The ratchet was happy — same 14 banks, no regressions — and
`egret_refuses_every_flow_kestra_refuses` failed on three entries at once:

```text
invalids/if-without-condition: refused for a DIFFERENT reason than the group it is in.
invalids/dag-cyclicdependency: refused for a DIFFERENT reason than the group it is in.
invalids/dag-notexist-task:    refused for a DIFFERENT reason than the group it is in.
```

`validate` carries `if_condition_required`, `dag_cycle` and
`dag_dependency_not_found` too, so running it first made Task 4's and Task 5's
own arms unreachable from `build_plan` — three refusals silently becoming three
other refusals, with every one of them still `Err`. **A test that asserted only
"it errored" would have passed.** That is the whole argument for the per-cause
message, and it paid for itself on the first run. The order is now parser →
builder → `validate`.

### The 23 refusals, by cause

`egret_refuses_every_flow_kestra_refuses` carries them as a table; each group is
an implementation unit, and the assertion is on a fragment of egret's own
message so a refusal that becomes a different refusal is visible.

| cause | entries | where the rule lives | new at Task 8 |
|---|---|---|---|
| the YAML is not a flow | 4 | `spec::parse` | |
| the same key twice in one mapping | 1 | `spec::parse` | **yes** |
| an explicit `null` where W1 has a bare `Vec<Task>` — right by accident | 2 | `spec::parse` | |
| a flowable with nothing to decide on | 1 | `plan.rs` | |
| a `Dag` whose `dependsOn` graph cannot be ordered | 1 | `plan.rs` | |
| a `Dag` node depending on a non-node | 1 | `plan.rs` | |
| an id declared twice | 2 | `validate` (`duplicate_task_id`) | call only |
| an input id declared twice | 1 | `validate` (`duplicate_input_id`) | call only |
| a flow with no tasks | 2 | `validate` (`empty_tasks`) | call only |
| a flow that calls itself | 1 | `validate` (`self_subflow`) | call only |
| a `WorkingDirectory` with no children | 1 | `validate` | call only |
| a `WorkingDirectory` child that is not runnable | 1 | `validate` | **yes** |
| a `Schedule` that cannot supply a required input | 2 | `validate` | **yes** |
| an `itemType` an ARRAY/MULTISELECT cannot hold | 1 | `validate` | **yes** |
| a dotted reference to a dashed input id | 1 | `validate` | **yes** |
| the same, on an `outputs.` reference | 1 | `validate` | **yes** |
| **total** | **23** | | |

### The 5 that are still over-accepted, and why each is

`STILL_OVER_ACCEPTED` asserts they are **accepted**, not merely absent from the
table above — so the day one starts being refused the test fails and forces the
record to be corrected, which is the `banked` arm's discipline in the other
direction.

**Three of them are one cause: egret has no plugin schema.**

- `invalids/invalid-task` — `Invalid type: io.kestra.plugin.core.debug.MissingTwo`.
  egret's `spec::registry` is not Kestra's classpath. `unknown_task_type` is a
  **warning**, and promoting it to an error would refuse every plugin egret has
  not registered. That is a product decision, not a Task 8 rule.
- `invalids/invalid-property` — `Unrecognized field "invalid"`. W1 models a
  task's plugin properties as an opaque `Leaf { spec }` on purpose.
- `invalids/dag-invalid-subtask` — `tasks[task1].tasks[task1].task.message: must
  not be null`, a `@NotNull` on `Log.message`. Bean validation of a plugin's own
  property; same gap.

The other two are each their own:

- `invalids/inputs-bad-validator-syntax` — `validator: \`, refused as
  `invalid pattern [\]`. Kestra compiles it with `java.util.regex.Pattern`.
  Compiling it with the `regex` crate instead would refuse valid Java patterns
  (lookaround, backreferences), which trades an over-acceptance for an
  **over-refusal**. It needs a Java-Pattern-compatible syntax check. Owner: W1.
- `invalids/inputs-with-multiple-constraint-violations` — publish returned
  **200**. Kestra refuses it when the execution's input *values* are read.
  `build_plan(&str)` takes no input values, so **no change to it can reach that
  seam**. Owner: W3b/W1 input coercion.

### The price of running `validate`: two over-refusals the ratchet cannot see

> **Superseded in scope at the final review, not in content.** Task 8 pinned
> these two *by name*, which guards these two and nothing else. The sweep below
> — *Over-refusal, swept rather than pinned* — makes the population
> `corpus.json`'s own `publish_status == 200` and finds **three**: these two
> plus `valids/switch-impossible`, the over-refusal already on `main`.
>
> **Two, since the null-list fix.** `valids/switch-impossible` is no longer
> refused; see *The null list Kestra accepts and egret refused*, at the end of
> this file.

Measured over all **345** corpus flows, `validate` reports errors on **18**.
Sixteen are `refused` golden entries. The other two are flows this Kestra
published with **200**:

| flow | codes | why it is an over-refusal |
|---|---|---|
| `valids/task-flow-dynamic` | `subflow_namespace_invalid`, `subflow_flow_id_invalid` | it writes `namespace: "{{inputs.namespace}}"`; the grammar is checked against the **unrendered** source, so every dynamic subflow reference is refused |
| `valids/trigger-flow-listener-namespace-condition` | `dependency_flow_id_required`, `dependency_namespace_required` | measured at the checkout `96893b589`, `Flow.java:364-380`'s `Dependency` puts **no `@NotNull`** on `namespace`, `flowId`, `states` or `labels`; a namespace-only dependency is a documented Kestra pattern |

**How they were found is worth as much as the finding.** The ratchet was green
through the change that introduced them: neither flow has a golden entry, so
neither is comparable. What surfaced them is a *count over the whole corpus* —
`every_corpus_retry_is_written_in_literals` runs over every corpus flow
`build_plan` can plan, and its total dropped by one. Chasing which flow had left
the population meant sweeping `validate` over all 345, and the sweep found three
things rather than one: the expected departure
(`invalids/duplicate-parallel`, now correctly refused) and these two.

**Both are W1's rules and both predate this task** — publish has returned 400
for these two flows since W1 shipped. `build_plan` running `validate` does not
create the defect; it stops the executor from disagreeing with the publish path
about it. Both flows are `not-standalone`, so **neither has a golden entry and
the ratchet cannot see either one** — which is exactly why they are a committed
test rather than a sentence here.

**Neither is a rule to loosen casually, and that is the finding.** Both exist
because `publish::subflow_ref_name` encodes the pair as `"{namespace}/{flowId}"`
and a `/` in either half decodes back into a different pair. Skipping the check
when the value holds `{{` would not fix the flow; it would move the collision to
render time, where nothing looks for it. The fix is to the **encoding**, and it
is W1's.

### W1's null-list fix costs no over-acceptance, and that is now measured

> **The fix has landed.** Task 8's prediction below was measured against the
> rehearsal — the token deleted from each flow — and the real fix agrees with it
> on all three rows. See *The null list Kestra accepts and egret refused*, at the
> end of this file, for what the prediction did **not** cover.

`build_plan` refused `valids/switch-impossible`, a flow this Kestra published and
**ran**, because W1's bare `Vec<Task>` rejects an explicit `null` Jackson
accepts. That over-refusal was not Task 8's to fix.

What *is* new: since `build_plan` now runs `validate`, deleting the offending
token from a flow is a complete rehearsal of W1's fix on that flow, and
`a_null_task_list_is_an_over_refusal_that_reaches_the_http_api` performs it:

| flow | with the `null` | with the `null` deleted |
|---|---|---|
| `valids/switch-impossible` | refused (over-refusal) | **plans** |
| `invalids/switch-invalid` | refused for the `null` | refused for `switch_no_branches` |
| `invalids/invalid-parallel` | refused for the `null` | refused for `invalid_namespace` / `parallel_tasks_required` |

So the two refusals that are right *by accident* survive the fix on their merits,
and the fix costs no over-acceptance. Before Task 8 those last two rehearsals
both came out `Ok`, which is why that test used to assert `is_ok()` for them.

### Over-refusal, swept rather than pinned

Task 8's concern 2 asked the final review to decide whether to pin the two
over-refusals above as exceptions or fix the encoding. The answer is neither on
its own: **pin them, and sweep for a third.** Pinning two flows by name guards
those two flows. Nothing in this package looked for a third, and nothing could:
both live in the 26 `not-standalone` flows, the population the coverage
statement says is never compared.

**The gap was demonstrated before it was closed, and re-measured on this tree
afterwards.** An invented `build_plan` rule refusing three more flows this
Kestra published with 200 — `valids/kv`, `valids/pause_no_tasks`,
`valids/trigger-flow-listener`, all `not-standalone` — leaves the crate
**green at 579 passed** with this one test skipped. (The review reported 536 on
its own tree; the total is the tree's, the result is the same.) Over-refusal is
the mirror of the one divergence class the programme forbids shipping, and it
could be introduced without a single other test moving. With the test in place:

```text
OVER-REFUSAL: 3 flow(s) this Kestra published with 200 are refused by
`build_plan` and are not in PUBLISHED_BUT_REFUSED …
  valids/kv: invented_rule: refused
  valids/pause_no_tasks: invented_rule: refused
  valids/trigger-flow-listener: invented_rule: refused
```

`every_flow_kestra_published_plans_except_two_named_over_refusals` closes it
(named `..._three_...` until the null-list fix took the table from three rows to
two — the mutation records above quote the older name). The population is
`corpus.json`'s own `publish_status`, **295** of the 345 flows — deliberately
wider than the golden's 266 `runs` entries, because it includes every
`not-standalone` flow, which is exactly where the two known over-refusals hide.
Running it over the whole population found **exactly three**, and finds **two**
since the null-list fix:

| flow | egret's code | whose rule |
|---|---|---|
| ~~`valids/switch-impossible`~~ | ~~`cannot parse flow`~~ | **fixed.** W1's bare `Vec<Task>` and an explicit `null`. The only one of the three with a golden entry, and the only one the ratchet could see — which is why it is the only one that got fixed |
| `valids/task-flow-dynamic` | `subflow_namespace_invalid` / `subflow_flow_id_invalid` | W1's, and the fix is to `publish::subflow_ref_name`'s encoding |
| `valids/trigger-flow-listener-namespace-condition` | `dependency_namespace_required` / `dependency_flow_id_required` | W1's, same encoding |

**It is written inverted, the way `STILL_OVER_ACCEPTED` is**, and that is the
whole difference between an exception table and a suppression list: each row is
asserted to be *still refused, for its own code*, so fixing one fails the test
by name and forces this table to be corrected. A suppression list would let a
fixed row rot in place.

One flow in the population published with 200 and was refused later — the
`invalids/inputs-with-multiple-constraint-violations` entry, refused when the
execution's *input values* were read. It must still **plan**: `build_plan(&str)`
never sees an input value, so refusing it here would be refusing it for a reason
egret cannot have. That is the same seam its `STILL_OVER_ACCEPTED` row records,
seen from the other side.

The sweep costs about a third of a second over 295 flows.

### A wrong reason found beside a dropped field

W1's `models::trigger::Schedule` dropped `inputs` and recorded the reason as
"the deprecated `inputs` map". Measured at the pinned checkout `96893b589`,
`Schedule.java:223` is a plain `private Map<String, Object> inputs;` with **no
`@Deprecated` anywhere in the file**, and `FlowValidator.java:284-312` reads it
to decide whether a scheduled flow can start. Had the rule been written against
the dropped field, it would have refused every scheduled flow that supplies its
own inputs — an over-refusal manufactured out of an incorrect note. The field is
restored with the correct reason; only its **key set** is read here, and
coercing the values against each input's declared type is W6's, at the moment
the schedule fires.

### Two upstream defects copied deliberately

The Global Constraints say upstream defects are copied, and the dashed-reference
rule is where that bites twice:

- Kestra builds the inputs pattern as `"\\{\\{\\s*inputs." + id + "\\s*\\}\\}"`
  with the `.` **not escaped**, so `inputs_server-fqn` matches too. Copied.
- Both patterns require the reference to be the **whole** expression, so
  `{{ inputs.server-fqn | upper }}` slips through upstream. Copied, and
  `bracket_notation_and_an_undashed_id_are_both_silent` asserts it, because
  inventing the wider rule would refuse a flow this Kestra accepts.

One difference runs the other way and is checked rather than argued: Kestra
reflects over `object.getClass().getDeclaredFields()` — the concrete class's own
fields, so an inherited one is never scanned — where this scans the task's whole
serialised form, a superset. A superset can only refuse **more**, so it is
measured: over the 294 golden entries the two rules fire on exactly the two
flows Kestra refuses for them, and the ratchet's regression arm covers the other
266.

### One fail-open rule, on purpose

`is_runnable_child` answers Kestra's `task instanceof RunnableTask<?>` from a
model with no classpath: a typed `TaskKind` variant is not runnable, and a
`Leaf` whose canonical type has a `KESTRA_FLOW_CONTROL` row is not either (which
is the arm `invalids/workingdirectory-invalid` needs — its offending child is a
`Pause`, unported and therefore a `Leaf`).

`KESTRA_FLOW_CONTROL`'s **other** use is fail-CLOSED: the `not_yet_enforced`
warning fires for anything spelled like `io.kestra.plugin.core.flow.*`. This one
is not, and the asymmetry is the point. A spurious warning costs a reader one
line; a spurious **refusal** rejects a flow. The same spelling test would refuse
`io.kestra.plugin.core.flow.Sleep` and `…PurgeFlows` — measured at the checkout,
the only two classes in that package that implement `RunnableTask`
— and both are legitimate children of a `WorkingDirectory`. The residual is
stated instead: a control-flow class Kestra adds tomorrow, inside a
`WorkingDirectory`, is accepted here until it gains a row.

### Task 8's mutations

Twelve. Every row is a literal `-` → `+`, applied to a file anchored to
`git show HEAD:<path>` (the driver refuses to snapshot a file that already
differs from HEAD), **built first** — `cargo build -p egret-workflow --tests`,
0 errors asserted — and only then run against the **whole crate**,
`cargo test -p egret-workflow --no-fail-fast`, never a name-filtered subset.
Restore is `shutil.copyfile`, verified against the anchor after every mutation
and for all three files at the end. Victim counts are from **one replay of the
final tree** at `e0a22aa7`.

**The first replay was discarded and re-run whole**, because its driver was
invoked through `| tail -80` and the first seven rows scrolled out of reach —
the same family of mistake this file already records for a `head -20`, made
again with the other end of the pipe. A table that cannot be read is not a
record either.

| # | file | edit (`-` → `+`) | victims |
|---|---|---|---|
| M99 | `plan.rs` | `- if let Some(first) = report.errors.first() {` → `+ … .first().filter(\|_\| false) {` | **6** (ratchet, **13 regressions**) — the whole `validate` gate. 13 rather than 14 because `invalids/duplicate-key` is refused at *parse* |
| M100 | `parse.rs` | `- serde_norway::from_str::<DuplicateKeyProbe>(source).map_err(located)?;` → `+ let _ = DuplicateKeyProbe;` | 4 (ratchet, 1 regression — the fourteenth) |
| M101 | `validate.rs` | `- if !is_runnable_child(child) {` → `+ if false && !is_runnable_child(child) {` | 4 (ratchet, 1 regression) |
| M102 | `validate.rs` | `is_runnable_child`'s whole `Leaf` arm → `true` | 3 (ratchet, 1 regression). Kills only the **unported-flowable** test, which is the arm `invalids/workingdirectory-invalid` needs |
| M103 | `validate.rs` | the schedule rule's guard → `if true \|\| …` | 3 (ratchet, 2 regressions) |
| M104 | `validate.rs` | `- .filter(\|id\| !supplied.is_some_and(…))` → `+ … \|\| true)` | **9** (ratchet, 1 regression) — ignoring the schedule's own `inputs:` over-refuses a flow this Kestra ran |
| M105 | `validate.rs` | the `itemType` guard → `if false && matches!(…)` | 3 (ratchet, 1 regression) |
| M106 | `validate.rs` | `- if !matches!(input.r#type, Array \| Multiselect) { continue; }` → `+ if false { continue; }` | **1** — and that one victim is the point: the gate is what stops this crate's flat `Input` inventing a refusal Java could never make, and nothing else in the crate reaches it |
| M107 | `validate.rs` | the dashed-reference guard → `if true \|\| …` | 4 (ratchet, 2 regressions) |
| M108 | `validate.rs` | `- .filter(\|i\| i.id.contains('-'))` deleted | **19** (ratchet, **6 regressions**, 1 bogus bank) — a pattern for *every* input refuses half the `If`/`Switch` corpus |
| M109 | `validate.rs` | the two dashed codes swapped in the loop's table | 2 — `egret_refuses_every_flow_kestra_refuses` dies on the CODE, which is what a per-cause message buys over `is_err` |
| M110 | `plan.rs` | the `validate` block moved **before** the two `flatten_list` calls | **7** (0 regressions) — the six builder refusal tests plus the gate test. See below |

**M110 is the row that justifies the gate order, and it is a warning about
running `validate` first.** Its seven victims are
`a_dag_dependency_on_a_task_that_is_not_a_node_is_refused_by_name`,
`a_cyclic_dag_is_refused_by_name`, `a_dag_node_with_no_task_is_refused`,
`a_loop_with_no_values_is_refused_by_name`,
`a_switch_with_no_value_is_refused_by_name`,
`an_if_with_no_condition_is_refused_by_name` and
`egret_refuses_every_flow_kestra_refuses` — **and zero ratchet regressions**.
So `validate`-first banks the same 14, refuses the same flows, and silently
unhooks six of Task 4's and Task 5's own guards. The ratchet cannot see it; six
unit tests and one message assertion can.

**No mutation killed nothing**, which is worth stating because six of Task 7's
did and that is what its replay was for.

## What the final review had to change, and what it banked

Eight tasks, then a whole-branch review of all 32 commits. Its findings are here
because four of them are corrections to *this file*, and a correction that lives
only in a review report is a correction nobody reads.

**The ratchet: 149 → 145.** Four banked, zero regressions, zero broken.

| banked | what earns it |
|---|---|
| `valids/disable-simple` | a disabled task in the middle of the flow's root list |
| `valids/disable-error` | a disabled task in the middle of the flow's `errors:` list |
| `valids/sequential-with-disabled` | a `Sequential` whose only child is disabled — it still runs, and still succeeds |
| `valids/if-with-only-disabled-tasks` | the same, on the branch an `If` chose |

### `disabled:` was an over-acceptance, and over-acceptance is the class that must not ship

`disabled:` had been unclaimed since Task 3, and the plan required the final
review to assign it or defer it with a reason. Deferring was not open: on all
six flows that use the flag, egret **created and ran a task the author had
disabled**, and on four of them that flipped the execution's terminal state —
`valids/disable-simple` is `["CREATED","RUNNING","SUCCESS"]` in the golden and
`FAILED` here. `baseline.txt` filed those as "does not reproduce yet", which is
the wrong category: they were egret answering where Kestra refused to.

The flag was parsed by W1 and dropped at the plan boundary. `flatten_one` never
read `task.disabled`, and `grep '\.disabled\b'` outside `src/models/` was empty.

**Kestra's rule, read from the goldens and then confirmed in the source.** A
disabled task gets **no taskrun at all** — it is absent from `taskRunList`, not
present in some skipped state — and its parent flowable still runs and can
succeed with every child disabled. `Execution.removeDisabled` is called on
`tasks`, `errors` **and** `finally` at the top of
`Execution.findTaskDependingFlowState` (`Execution.java:585-587`), which every
flowable's `resolveNexts`/`resolveState` goes through, and again on the flow's
own root list in `FlowableUtils.resolveSequentialNexts`. The all-disabled case
has its own short-circuit in `resolveSequentialState`, and a second copy of it
in `If.java:192-198`.

So it is a *list* rule, and it is implemented as one: `flatten_list` drops a
disabled task, and every list-shaped child position in the builder goes through
`flatten_list`.

**The two of the six that did not bank, and why.**

- `valids/disable-flowable` carries three sub-executions. It now differs on
  `spawned` and on nothing else — it is the 39th entry in that population, and
  the whole of what stood between it and a bank was this flag.
- `valids/parallel-disabled-tasks` reproduces every run, parent, history and
  attempt, and differs on **one ordered pair**: `(task3, task2)` is `before` in
  the golden and `overlapping` here. That is the clockless-machine hole
  `valids/dag` and `valids/if` are already in, and it is where this entry is
  filed now. **The review that commissioned this fix expected five of the six to
  bank; the measurement says four.** The fifth is this one, and the ratchet's
  own failure output is what says so.

**The one position the rule is not applied to, deliberately.** A `Dag`'s nodes
are flattened by `flatten_dag`, and a disabled `Dag` node is left planned.
Kestra removes it from `currentTasks` like any other child but leaves it in
`taskDependencies` (`FlowableUtils.resolveDagNexts`), so every node that
`dependsOn` it waits for a run that is never created and the `Dag` never
terminates. Copying that is reproducing a hang; dropping the node *and* its
incoming edges is inventing a semantics Kestra does not have; dropping the node
alone makes `flatten_dag` refuse the flow for a dangling `dependsOn`, which is
an over-refusal. No corpus flow reaches the question, and
`no_corpus_dag_node_is_disabled` re-derives that from all 345 flows on every
run, so the position is a named gap rather than a guess.

### `SKIPPED` is `runIf:`'s, and `runIf:` had no owner

*(`runIf:` has an owner since W3c, which implements it. This section is the
attribution that made that possible and it stands as written; the sentence
"unclaimed by two packages at once" is what stopped being true.)*

This file credited the golden's `SKIPPED` state to `disabled: true` in **three**
places, and the plan's own table said it too. It is false. Walking every run of
`golden.json` finds `SKIPPED` on **6 runs across 4 entries** —
`valids/task-runif`, `valids/task-runif-executionupdating`,
`valids/task-runif-taskswithstate`, `valids/task-runif-workingdirectory` — and
**not one of those four YAML files contains `disabled:` at all**. Every one is
`runIf:`.

Two things follow, and the second is the one worth having.

1. `runIf:` had **no row and no owner anywhere** — not here, not in the plan.
   Its 7 baselined entries were absorbed by other rows: `valids/task-runif` was
   counted under `Subflow`, which it happens to declare, so fixing `Subflow`
   would not have banked it. It now has its own row, and its owner is
   *unclaimed by two packages at once*: W1's `Task` carries no field for it, and
   `RunState` has no `SKIPPED` to hold a skipped run. Either half alone blocks
   it.
2. **Nothing derived the attribution.** The counts beside it were derived from
   the baselined flows' own YAML on every run; the words naming the construct
   were not — and an attribution nothing derives is the same shape as a count
   written beside a derivation, which is the defect this package has now
   produced four times.
   `the_skipped_state_comes_from_runif_and_never_from_disabled` closes it: it
   names the six runs, requires each of their flows to declare `runIf:` and not
   `disabled:`, and — the half that is the control — requires that **no** flow
   declaring `disabled:` produces a `SKIPPED` anywhere. Without that half the
   test would pass on a golden where both constructs skipped things.

Had the old sentence been true, deleting a disabled task from the plan would
have been the wrong shape of fix, so this is not a documentation-only finding.

### The W1↔W3a dispatch seam nobody owned

`flatten_one` matched `(&task.kind, task.r#type.as_str())` against the FQCN
literals, while W1's `canonical_flow_type` reaches the same `ModelKind` from
**four** spellings — the FQCN, the `core.` short form, and either in any case.
Measured on a two-level chain at the review's HEAD:

```text
io.kestra.plugin.core.flow.Sequential -> 2 tasks, Sequential { tasks: [1] }
core.sequential                       -> 1 task,  Runnable { type_id: "core.sequential" }
core.Sequential                       -> 1 task,  Runnable { .. }
io.kestra.plugin.core.flow.sequential -> 1 task,  Runnable { .. }
```

An aliased flowable was planned opaque, taking its whole subtree with it — so a
60-level `core.sequential` chain planned as **one** task and `MAX_TASK_DEPTH`
never fired. The builder's own comment said "`models::TaskKind::Sequential` is
reached by that type string and by no other", which was false from the moment W1
gained the short form.

Latent rather than live: no corpus flow writes an alias, which is why 294 golden
entries and every unit test were green through it. The builder now dispatches
through `canonical_flow_type` — the *same* reduction W1 dispatches through,
rather than a second opinion about which strings name a flowable — and
`every_spelling_of_a_flowable_reaches_the_same_plan` plans a chain past the
bound in each of the four spellings and requires the refusal from every one. A
guard that fires on only one spelling of the input it guards is not a bound.

### The depth bound was measured against the wrong walk

`MEASURED_MAX_CORPUS_DEPTH = 6` was derived through `Task::children()`, which is
`spec::validate`'s walk and excludes every task-level `errors:` list. That was
right when it was written and stopped being right at Task 7, which made
`flatten_one` descend into `errors` at `depth + 1`. From Task 7 the derivation
was measuring a walk that is not the one the constant guards.

Re-measured over the builder's own output: the deepest **plan** in the corpus is
`valids/full` at **6**, the same number, so there is no live consequence and the
8× headroom is intact. `the_depth_bound_is_derived_from_the_deepest_corpus_flow`
now derives both readings and requires them to agree.

### The final review's mutations

Seven rows. Every one is a literal `-` → `+` applied to a file anchored to
`git show HEAD:<path>` (the driver refuses to snapshot a file that already
differs from HEAD), **built first** — `cargo build -p egret-workflow --tests`,
0 errors asserted — and only then run against the **whole crate**. Restore is
byte-compared against the anchor after every row and again for all touched files
at the end.

| # | file | `-` → `+` | victims |
|---|---|---|---|
| M111 | `plan.rs` | `.filter(\|t\| !t.disabled)` deleted | **6**, ratchet **4 regressions** — the four banked entries, and the ratchet prints 149 mismatching and 30 dropped pairs again |
| M112 | `plan.rs` | `canonical_flow_type(&task.r#type)` → the same, `.filter(\|_\| task.r#type.starts_with("io.kestra.plugin.core.flow."))` | **1**, and that one is the point: `every_spelling_of_a_flowable_reaches_the_same_plan`, on `core.sequential`. Nothing else in the crate or the corpus can tell |
| M113 | `plan.rs` | a `report.warnings.first()` refusal inserted before the `errors` one | **68**, including `every_flow_kestra_published_plans_except_three_named_over_refusals`: *"OVER-REFUSAL: 281 flow(s) this Kestra published with 200 are refused by `build_plan` and are not in PUBLISHED_BUT_REFUSED"*. This is the asymmetry `build_plan`'s own doc comment forbids, and it is now a test failure rather than a paragraph |
| M114 | `plan.rs` | `children_of`'s `errors` half deleted | **0** — see below |
| M114b | `plan.rs` | `MEASURED_MAX_CORPUS_DEPTH = 6` → `5` | **1**, the depth test |
| M116 | `tests/executor.rs` | `corpus_source` appends `\n# disabled: true\n` | **1**: *"valids/task-runif: a flow declaring `disabled:` produced a SKIPPED run"* — the control half of the attribution |
| M117 | `tests/executor.rs` | `corpus_source` rewrites `runIf:` to `runIff:` | **2 when it was written**; **3 replayed at W3c**. The second of the original two was "the residual-baseline table's new `runIf:` row", and **W3c deleted that row** — this figure went stale because of a later commit rather than because it was wrong. The third victim is W3c's own `the_caller_can_render_every_expression_except_the_ones_named` |
| M115 | `tests/executor.rs` | `simulate_with`'s `spawned: Vec::new()` → `vec![Trace::default()]` | **4**, including `only_the_top_level_order_pairs_are_ever_asserted`'s control: *"egret produces no `spawned` at all, which is WHY the 227 nested pairs are unreachable"* |

**M114 killed nothing, and it is recorded rather than dropped.** Deleting the
`errors` half of `Plan::children_of` — the half Task 7 added, and the half the
depth bound's second reading exists for — moves no assertion in this crate. That
is not a hole in the guards: it is the corpus. `is_loop_body` is the only
consumer, and `an_errors_list_inside_a_loop_body_is_a_shape_the_corpus_does_not_reach`
already asserts, by name, that no corpus flow puts an `errors:` inside a `Loop`
body; the deepest plan is `valids/full`, whose deepest path is not an `errors:`
either. So the code path is provably unreached by the 345 flows, and the crate
already carries the test that says so. Anyone widening `children_of`'s consumers
should expect this row to start killing something.

**The new plan-side depth assertion is killable, and M113 is what showed it.**
M114b fires the *model-side* assertion first (they are ordered), so its message
is the old one. Under M113 the population of plannable flows collapses and the
new assertion fires on its own: *"the BUILDER's own walk is what
`MAX_TASK_DEPTH` bounds … The two agree at 2 … the deepest PLAN is
valids/flow-concurrency-parallel-subflow-kill"*.

`no_corpus_dag_node_is_disabled` carries its positive control **inside the
test** rather than in this table: it runs the walker over a synthetic `Dag` with
one disabled node and one enabled one, in both spellings, and requires it to
report exactly the disabled one. A control that runs on every CI run is worth
more than a mutation run once.

### Seven documentation drifts, and the shape they share

Six of the seven were written in the same commit as the derivation that
contradicts them.

| where | said | is |
|---|---|---|
| `baseline.txt` header | the narrative stops at Task 7, "banked 16, leaving 163" | the file holds 145. Task 8 deleted 14 lines and added no paragraph — the file-no-task-owns symptom the plan's *Final review* section predicted |
| `plan.rs` | "Five things `validate` does not cover", then lists **seven** | six of the seven ARE in `validate`; only `MAX_TASK_DEPTH` is the builder's alone. The paragraph argued against the gate order settled three paragraphs above it |
| `plan.rs` | `POST /flows` refuses on `report.errors` | `create_flow`/`update_flow` go through `parse_draft`, which only parses. The gate is `publish::publish` (`publish.rs:118-127`) |
| `plan.rs` | a `Loop`'s `errors:` makes "three corpus flows' lists" invisible | **four**, which the test written in the same commit already asserted |
| `models/task.rs` | "`build_plan` never calls `validate`" | false since `e0a22aa7`; the argument built on it is void, and the conclusion survives on its other half |
| `plan.rs` | a task-level `errors:` subtree is invisible "to the walk and to this builder alike" | stale since Task 7 — see *the depth bound* above |
| this file | "51 flows are never compared at all", then lists 26 + 23 | **49**. The 51 only closes if the 2 unstable traces are counted, and this 51 and the denominator table's "51 rejected" are different sets coinciding in size |


## What W3c had to change, and what it did NOT bank

W3c is a follow-on package with one subject: **`runIf:`**, the over-acceptance
W3a's final review named and left unowned. It changes no number in the ratchet,
and the interesting half of this section is why a correct fix cannot.

| | golden entries | mismatching | baselined | broken | ordered pairs removed |
|---|---|---|---|---|---|
| **control** — clean tree at BASE `05083cbc`, `git status --porcelain` empty | 294 | **144** | 144 | 0 | 15 |
| **after** | 294 | **144** | 144 | 0 | 15 |

**0 banked, 0 regressions, 0 broken.**

### The defect, and the surface it belongs to

A task whose `runIf:` renders falsy is **created** by Kestra and then not
executed: `CREATED SKIPPED`, no `SUBMITTED`, no `RUNNING`, and one attempt
reading `CREATED SKIPPED`. egret ran it. That is the same class as `disabled:`
— egret producing work Kestra did not — and it is the class this programme
forbids shipping.

**The surface was derived rather than taken from the brief, and it is one
property.** The question is "which task-level properties make Kestra create a
run and then not execute it", and it was answered from two directions:

- **From the state.** `State.Type.SKIPPED` is written at **four** sites in the
  whole checkout's main sources — `ExecutorService.java:1302` and `:1382`,
  `ExecutionEventMessageHandler.java:272`, `WorkerTaskProcessor.java:141` —
  and every one of the four is guarded by
  `TruthUtils.isTruthy(runContext.render(task.getRunIf()))`. Nothing else in the
  artifact can put a run into `SKIPPED`.
- **From the field list.** `Task.java` declares **16** task-level properties
  (`id`, `type`, `version`, `description`, `retry`, `timeout`, `disabled`,
  `workerSelector`, `policyRefs`, `logLevel`, `allowFailure`, `logToFile`,
  `runIf`, `allowWarning`, `taskCache`, `assets`). Exactly two stop a task
  running: `disabled:`, which removes it from the list before a run exists
  (W3a's final review), and `runIf:`. Two more are near neighbours and neither
  is this shape: `taskCache` restores outputs *instead of* running the body but
  the run still goes `SUBMITTED RUNNING SUCCESS`, and both it and `timeout:` are
  enforced in the **worker** (`WorkerTaskProcessor.java:225`,
  `WorkerTaskCallable.java:61`) — the same place `timeout:` already sits in the
  unclaimed list. `allowFailure`/`allowWarning` change a terminal state, not
  whether the body runs.

So the surface is `runIf:` alone, and the two independent derivations agree.
Its corpus population is asserted rather than described: **9** of the 345 flows
declare one (`no_corpus_parallel_or_dag_child_declares_a_run_if`) and they carry
**17** tasks between them — `grep -rEc '^[[:space:]]*runIf:'`; the 20 raw `runIf`
hits include three comments in `task-runif-taskswithstate`. 17 is what the 11
planned expressions below reconcile with: 17 − 1 `not-standalone` − 3
`WorkingDirectory` children − 2 inside a `finally:` = 11.

### Where the rule lives, and the two rules it forced open

`flowable::open` is every created run's single door — `sequential::walk` and
`parallel::drive_concurrent`'s rule 3 both go through it, and a `Loop` goes
through it before `loop_::open` resolves its `values:`. The check is at the top
of it, which is what makes a skipped `Loop` spawn nothing and a skipped
`Sequential` create no children. `Command::SkipRun` writes the state and the
attempt together, because Kestra's four sites do
(`withState(SKIPPED).addAttempt(...state(new State().withState(SKIPPED)))`, and
a `new State()` starts at `CREATED`) and a `SetRunState` cannot: `apply` appends
to the *last* attempt and a skipped run has none.

**`SKIPPED` is the first terminal state this executor holds that is not a
failure, and two rules were written on the assumption that there was no such
thing.**

- `parallel::drive_concurrent`'s rule 1 was `terminal && != SUCCESS`, so a falsy
  `runIf:` on one branch would have `fail_the_rest` of its siblings — egret
  failing runs Kestra succeeds. `State.java:293` puts `SKIPPED` in
  `isTerminatedNoFail()` beside `SUCCESS`, and `Execution.guessFinalState` has
  no arm for it at all.
- `dag::drive`'s predicate was `== SUCCESS`, with a doc comment arguing that
  Kestra's own "all dependencies are **terminated**" could not be told apart
  because every terminated-but-not-successful state had already ended the
  branch. **That argument broke exactly where it said it would**, and its own
  comment is what said so: with rule 1 excluding `SKIPPED`, a skipped dependency
  is terminated, alive and not `SUCCESS`, and a `SUCCESS`-only reading would
  hang the `Dag` for ever on a flow Kestra completes.

**Neither of those two is measured on the artifact**, and that is stated rather
than implied: `no_corpus_parallel_or_dag_child_declares_a_run_if` derives, from
all 345 flows and with a positive control in all four spellings, that no corpus
`Parallel` or `Dag` child carries a `runIf:`. They are read from Kestra's Java
and held by two committed tests.

### Why nothing banks, measured rather than argued

All **7** baselined entries that declare a `runIf:` stay baselined, and — the
sharper statement — **egret reproduces none of the golden's 6 `SKIPPED` runs
under this oracle.** The cause is the render tier, and it is per-entry:

| entry | why the skipped run is not reached |
|---|---|
| `valids/task-runif` | its second task's `runIf:` is `{{ outputs.output['values']['taskrun_data'] == 1 }}`. `render_context` carries `inputs` and `vars` and no `outputs`, so that render FAILS, the run FAILS, and the two skipped runs after it are never created. Kestra's own `willfailedtheflow` proves a failed `runIf:` render is a `FAILED` run, so the *rule* is right and the *context* is short |
| `valids/task-runif-executionupdating` | the same, one task later: `setVariables` fails on `{{ outputs.… }}` |
| `valids/task-runif-taskswithstate` | both its `runIf:` tasks are in `finally:`, which `build_plan` does not flatten — they are in no plan |
| `valids/task-runif-workingdirectory` | both are children of a `WorkingDirectory`, planned opaque |

**So the ratchet is not the instrument for this fix, and saying so is the
point.** What holds the rule is a set of committed tests over hand-built plans
— the skip, the truthiness rows including the two `trim()` rows where a naive
transliteration disagrees, the unrendered case, a skipped flowable's children, a
skipped `Loop`'s spawns, the `Parallel` sibling, the `Dag` dependent, and the
`disabled:`/`runIf:` pair asserted side by side — plus the two corpus-derived
guards above. `the_caller_can_render_every_expression_except_the_ones_named`
gained an arm that derives the whole `runIf:` population (11 planned, 7 that
this context cannot render, 2 literal `"false"`, 2 truthy) so the boundary is a
named list rather than a rate.

### What DID move, and it moved down — on FOUR entries, not two

Two entries produce *fewer* runs than before, and the order-pair population goes
**647 → 642**. `valids/task-runif` goes 3 runs/3 pairs → 2/1 and
`valids/task-runif-executionupdating` 4/6 → 3/3: each now stops at a `runIf:`
this harness cannot render, where before it ran straight past the field.
Measured by re-running the whole arm with `PlanTask::run_if` forced to `None` —
which reproduces the pre-W3c tree exactly, since nothing else W3c changed can
fire without a `runIf:`.

**This paragraph said "only those two entries move" and was understating it by
half, because the instrument could not see the other two.**
`valids/restart-with-finally` and `valids/restart-with-after-execution` each hold
a `fail_randomly` whose `runIf:` is `{{taskrun.attemptsCount == 0}}`. That run
was `["CREATED","SUBMITTED","RUNNING","FAILED"]` with one attempt — **byte-identical
to the golden's** — and is now `["CREATED","FAILED"]` with none. Their run count
and pair count are unchanged, which is exactly why a measurement over pairs is
blind to them: a trace can move without any run appearing or disappearing.
`the_caller_can_render_every_expression_except_the_ones_named` now reads the
golden's own terminal for each of the seven unrenderable `runIf:` expressions
and asserts all seven, so the cost is derived per task rather than counted.
**Six of the seven are the harness's cost; one — `willfailedtheflow` — is a
render Kestra failed too, so reproducing its `FAILED` is right.** All four
entries were baselined before and are baselined after.

### W3c's mutations

Fourteen rows, then a fifteenth pass after the fix below. Every one is a literal
`-` → `+` applied to a file anchored to `git show HEAD:<path>` — the driver
refuses to snapshot a file that already differs from HEAD and refuses a `-` text
that does not occur exactly once — **built first** (`cargo build -p
egret-workflow --tests`, 0 errors asserted) and only then run against
`--lib --test executor --test oracle`. Restore is byte-compared against the
anchor after every row and again for all touched files at the end.

| # | file | `-` → `+` | victims |
|---|---|---|---|
| M1 | `flowable/mod.rs` | `RunIfVerdict::Skip => return Step::Commands(vec![Command::SkipRun{..}])` → `=> {}` | **7** — *"left: `["CREATED","SUBMITTED","RUNNING","SUCCESS"]` right: `["CREATED","SKIPPED"]`"*. The whole rule |
| M2 | `flowable/mod.rs` | `NotRenderedYet => return Step::Waiting` → skip instead | **6**, and one is the ratchet: *"REGRESSION: 1 entr(y/ies) mismatch that were NOT in `oracle-exec/baseline.txt` … `valids/condition_with_input`"*. **This is the one thing the corpus can judge about `runIf:`** — a task Kestra *ran* being skipped |
| M3 | `flowable/parallel.rs` | rule 1's `!matches!(s, Success \| Skipped)` → `s != Success` | **2** — *"the sibling must run to completion, not be failed by rule 1"*, and the `Dag` test with it |
| M4 | `flowable/dag.rs` | `Some(Success) \| Some(Skipped)` → `Some(Success)` | **1** — *"left: `["d","up"]` right: `["d","up","down"]`"*: the `Dag` hangs |
| M5 | `state.rs` | the `attempts.push` in the `SkipRun` arm deleted | **2** — *"left: 0 right: 1"* on the attempt count. The skip's attempt is Kestra's, not decoration |
| M6 | `state.rs` | `\| RunState::Skipped` deleted from `is_terminal` | **5** — *"assertion failed: `RunState::Skipped.is_terminal()`"* |
| M7 | `executor/plan.rs` | `run_if: task.run_if.clone()` → `None` | **11** — the plan-side seam |
| M8 | `models/task.rs` | `optional(&mut map, "runIf")?` → `None` | **13** — the model-side seam, one wider than M7 because the field then survives only inside a leaf's `spec` |
| M9 | `flowable/mod.rs` | `branch::is_truthy(rendered)` → `rendered != "false"` | **1** — *"rendered `\"0\"`: expected skipped=true"*. The naive predicate a reader would write, and the row that catches it |
| M10 | `tests/executor.rs` | tier 2a's filter → `false` (the harness never renders a `runIf:`) | **2**, both via a PANIC arm: *"STUCK after 4 step(s) … run 1 (`fail`) is `Some(Created)`"* — an unanswered `runIf:` is a wait, so deleting the answer deadlocks 5 entries rather than changing 5 traces |
| M11 | `tests/executor.rs` | `Command::SkipRun { run, .. } => ended[*run] = Some(step)` → `=> {}` | **0** at first. See below |
| M12 | `state.rs` | `RunState::Skipped` deleted from `RunState::ALL` | **1** — the vocabulary arm, which could not fail at all before W3c |
| M13 | `models/task.rs` | the `runIf` emission in `Serialize` deleted | **1** — *"left: None right: `Some(\"false\")`"* |
| M14 | `flowable/mod.rs` | `rendered_run_if` → `rendered` in `run_if_verdict` (read the wrong slot) | **10** — the two-slot decision, asserted rather than argued |

### W3c fix round 2's mutations, and the replay that had to be thrown away

A review found three things the round-1 tree did not hold. All three are now
guards, and all three carry a mutation.

| # | file | `-` → `+` | victims |
|---|---|---|---|
| R2-M15 | `state.rs` | rule 2's `if *s == RunState::Created { … }` deleted | **1** — *"a run that has re-entered CREATED has not been rendered for this attempt … left `Some("true")` right `None`"*. The `runIf:` over-acceptance across a retry |
| R2-M16 | `state.rs` | rule 2's `r.rendered = None;` deleted, keeping `r.rendered_run_if = None;` | **0**. See below |
| R2-M17 | `flowable/mod.rs` | `SkipRun { run, at: now }` → `at: Tick(0)` | **2** — the skip's instant, end to end from `open` to the stored [`Attempt`] |
| R2-M18 | `flowable/mod.rs` | the control: `StartRun { run, at: now }` → `at: Tick(0)` | **3** — the pattern that already existed one line away and had not been applied to the new command |
| M117 | `tests/executor.rs` | replayed | **3**, up from the 2 recorded at the final review |

**R2-M16 killed nothing, and it is provably unreachable rather than merely
untested.** Rule 2 clears both render slots when a run re-enters `CREATED`;
only the `runIf:` one can be reached today, and the chain is structural rather
than corpus-dependent: `retry::scan` gates on
`matches!(kind, TaskKind::Runnable { .. })`, so only a `Runnable` is ever given
a `not_before`; `reopen_due` reopens exactly those; and a `Runnable` never has a
`rendered`, which is an `If`/`Switch`/`Loop` slot. `only_a_runnable_can_re_enter_created`
derives that with a positive control (the child *is* reopened, twice), so the
half is kept with the reason committed rather than deleted — and whoever lets a
flowable be retried should expect this row to start killing something.

**Three things round 1 got wrong, recorded because the corrections are the
point.** `runIf:` was decided once per run and never re-asked across a retry —
a narrower instance of the very class W3c closes, and W3c's own, since
`rendered_run_if` is the first render slot that can sit on a retried run.
`SkipRun`'s `at: now` was unheld while `command.rs` claimed the instant could
only be the `now` of the decision. And *What DID move* said two entries; it is
four.

**The replay itself was killed mid-mutation and was discarded whole.** A
2-minute tool timeout took the driver out between "write the mutation" and
"restore", and the next run's poison check — *"REFUSED — `state.rs` already
differs from HEAD"* — is what caught it. The tree was restored from HEAD, every
touched file byte-compared against `git show HEAD:<path>`, and the whole replay
re-run from a verified-clean tree. It was not topped up. That is the second time
this package's own poison check has caught a killed replay, and both times the
discipline alone would not have.

**M11 killed nothing, and the reason is the finding rather than an excuse.**
`simulate_with`'s `SkipRun` arm records a skipped run's right endpoint for the
happens-before relation, and **no golden entry ever reaches a `SkipRun`** — the
four `task-runif*` flows all stop earlier on a condition this harness cannot
render. So the arm is correct, load-bearing the moment anything skips, and
unreachable from the corpus: a guard nobody could see fail, which is the pattern
this package has paid for eight times. It was not left that way.
`a_skipped_run_still_gets_a_happens_before_endpoint` drives `simulate_with` with
a `skipping_machine` of its own and requires the pair, and re-running M11 against
it kills **1**. The comment beside the arm claimed the five ordered pairs W3c
removed as its evidence; that was wrong — those five are two entries producing
fewer runs — and the mutation is what said so.

### A guard that could not fail, and it was this file's own numbers

`the_state_vocabulary_this_enum_holds_is_derived_from_the_golden` is the test
that derives the state table above, and it carried **its own literal list** of
the eight state names. So it compared a list in `tests/executor.rs` against the
golden and never once read `RunState`: adding `Skipped` to the enum left it
green, and the `SKIPPED` row in the missing table would have stayed until
somebody noticed by eye. It now derives the vocabulary from `RunState::ALL`, and
`run_state_all_holds_every_variant_in_order` keeps that list and the enum in
step. This is the eighth guard-that-could-not-fail in this package and the
second that was hiding something.

`RunState::ALL` is not a proof of completeness and does not claim to be:
`RunState::slot` is exhaustive so a new variant forces an arm, and the test
requires the arm's number to be the variant's real position — but a variant
given a slot past the end of `ALL` is caught by nothing here. That limit is on
`ALL`'s own doc rather than argued away.

## W3b Task 1 — the handler surface, and the ceiling it puts on W3b

> **`handler-surface.txt` is Task 1's measurement and it is STALE by
> construction — found by W3b Task 7's sweep, reproduced without any of that
> task's edits.** The file is a pure function of `baseline.txt`, which W3b Task
> 5 moved by 32 entries, so `python3 handler_surface.py --check` exits **1** with
> 124 diff lines and `--self-test` exits **1** with `12/13 guards fired for the
> right reason` — its *"a blocker rule that fires on an entry egret already
> reproduces"* case now trips the trigger-reconciliation guard first
> (`expected 'the blocker rules are wrong', got: … 15 entries the naive pass
> drops declare no 'triggers:'`). **Nothing in the test suite runs either**, which
> is why six tasks passed a green gate over it.
>
> It is deliberately **not** regenerated: the artifact's content is the evidence
> for the ceiling of **172** and for the **11** entries the feed was keeping
> alive, and re-running it would replace that record with today's baseline —
> "0 at risk, ceiling 171" — which is a sentence about a decision that has
> already been taken. The ceiling and the 11 are what Task 5 was held to, and
> `oracle-exec/README.md`'s coverage statement carries what happened to them.
> Whoever needs a live handler surface should re-run the generator **and** repair
> the self-test case, in a commit that says the ceiling is being restated.

**W3b** is the package that makes `advance()` run for real — a claim loop, a
reaper, four handlers, a `SpawnExecution` executor — and then re-points this
oracle at that driver, so what the ratchet judges is the system that ships. Its
first task produces the number the rest of it may not report above:
`handler-surface.txt`, generated by `handler_surface.py` from `corpus/`,
`golden.json`, `baseline.txt` and `task-kinds.json` and by nothing else.

```
python3 handler_surface.py               # rewrite handler-surface.txt
python3 handler_surface.py --check       # regenerate and diff; exit 1 on drift
python3 handler_surface.py --self-test   # 13 guards, each with its message
python3 handler_surface.py --refresh-kinds   # re-derive task-kinds.json
```

It needs no Kestra, no docker and no third-party module — the rule `harvest.py`
is written to. `task-kinds.json` is the only input from the sibling checkout and
is **committed**, for the same reason `test-source-classes.json` is.

### The population is 201, and 194 was a different set

The plan's Step 1 says *194 of the 266 use only `Log`, `Return`, `Fail` and
`Sleep` as non-flowable tasks* and asks for it to be re-derived. It is **201**,
and the derivation is in section 2 of `handler-surface.txt`. The definition it
uses is stated there because the answer depends entirely on it: an entry is in
the population when every task type it declares whose **Kestra class** is not a
`FlowableTask` is one of the four.

194 is reproducible, and reproducing it is how the difference became a
derivation rather than a disagreement. It comes from two errors in the same
pass, and the generator reproduces both over the same corpus to show it:

- **It swept `triggers:` in.** A textual pass over `type:` lines cannot tell a
  trigger from a task. 27 comparable entries declare a trigger and nothing else
  outside the four, and every one of them is a flow four handlers can run —
  derived, and `handler_surface.py` refuses to write the file if one of the 27
  does not declare a `triggers:`.
- **It classified by MODULE.** Everything under `core.flow.*` counted as
  flowable. That is the same mistake in the other direction as `core.flow.Sleep`,
  which is a `RunnableTask` (`Sleep.java:47`) despite its package — and measured,
  the one type it actually admits is `Subflow`, an `ExecutableTask`, on 20
  entries.

`194 − 20 + 27 = 201`, asserted beside the parts. **The brief's figure is not a
conservative version of this one; it is a different set** — it admits 20 entries
the population excludes and excludes 27 it admits. Whoever quotes 194 next is
quoting a number that counts triggers as tasks.

`task-kinds.json` is what makes the classification evidence rather than a
naming heuristic: every task type any corpus flow declares, resolved
**transitively** to the Kestra interface its class implements —
`Loop extends AbstractBranch implements FlowableTask`,
`WorkingDirectory extends Sequential`, `WaitFor` is an alias of `LoopUntil`. 38
of the 40 resolve; the two that do not are `core.debug.MissingTwo` and
`MissingThree`, which the checkout declares nowhere, and that is the whole point
of `invalids/invalid-task`.

**The spec's §3 table is a top five, and the column has ten more rows.** Its
five `RunnableTask` counts reproduce here exactly — `Return` 115, `Log` 82,
`Fail` 55, `OutputValues` 23, `Sleep` 20, which is what makes this generator's
YAML reader evidence rather than a second opinion — but section 1 of
`handler-surface.txt` carries all 25 types the comparable corpus declares, of
which **nine are `RunnableTask`s** — the spec's five plus `PurgeLogs` (5
entries), `Assert` (2), `storage.Write` (2) and `kv.Set` (1) — one is an
`ExecutableTask` and four are `ExecutionUpdatableTask`s. A real driver meets
every one of them; a table of the top five hides them.

### 45 of the 201 need a task to have DONE something, and they are not one class

| | entries |
|---|---|
| read `outputs.`, `taskrun.` or `tasksWithState()` | 45 |
| read none of them | 156 |
| **TOTAL — the population** | **201** |

By marker: `taskrun.` **30**, `outputs.` **14**, `tasksWithState()` **1**.
The rows *can* overlap and measured none does, so they sum to 45 exactly. Lumping the three overstates the problem, and the
split is the useful part: **`taskrun.` reads the current run's own metadata** —
its id, `startDate`, `value`, `attemptsCount` — which a driver knows without any
handler producing anything. Only `outputs.` and `tasksWithState()` read
*another* task's result, and that is **15 entries**. Those 15 are the ones where
an exact handler output is load-bearing, and they are the population spec §4.4
is about: W3a's harness render context reaches `inputs` and `vars` and stops,
which is why W3c's `runIf:` fix banks nothing and why `loop-outputs-auto` cannot
resolve its `values:`.

Whole-line comments are cut before scanning. One corpus comment writes
`tasksWithState()` in prose (`valids/task-runif-taskswithstate.yml:4`), and
counting it would make a flow depend on an output because someone explained the
flow.

### Three of the four handlers produce nothing, and `Fail` does not always fail

Section 4 of `handler-surface.txt` carries the file and line for every claim.
The two that a handler written from the type name gets wrong:

- **`Return`'s output field is `value`** (`Return.java:119`), not `return` and
  not `format`, and `format` is rendered with `.orElse(null)` (`:99`) — so a
  `Return` with no `format` **succeeds** with `value: null`.
- **A `Fail` with a falsy `condition:` returns `null` and SUCCEEDS**
  (`Fail.java:137`). `condition` is optional (`:123`); truthiness is
  `TruthUtils`, whose false set is `"false"`, `"0"`, `"-0"`, `""` after a trim
  (`TruthUtils.java:6`, `:12`). A handler written as *`Fail` always throws* is
  wrong on every flow that uses the condition form. The two throwing paths also
  carry different messages — the literal `Fail on a condition` with a condition
  (`:135`), the rendered `errorMessage` without one (`:140`) — and **nothing in
  `golden.json` records a failure message**, so the ratchet cannot tell them
  apart. That is Task 3's own tests' job, not the oracle's.

`Log` (`Log.java:68`, returning `null` at `:109`) and `Sleep` (`Sleep.java:47`,
`:62`) produce nothing at all, and `VoidOutput` is an empty class
(`VoidOutput.java:3`) that neither of them even instantiates — so `outputs.<id>`
is **absent**, not an empty map.

**One claim in the brief this task could not confirm.** *`Sleep`'s duration must
be positive — `PT0S` is refused by the artifact* is not visible in `Sleep.java`:
`duration` carries `@NotNull` (`:52`) and no positivity constraint, and
`TimeUnit.MILLISECONDS.sleep(0)` does not throw. No corpus flow writes `PT0S`,
so nothing here can settle it either way. Task 3 should measure it against the
running artifact before implementing a refusal egret's users would be held to.

### The ceiling is 172, and the ratchet can go DOWN

**Of the 294 comparable entries, at most 172 can agree with the golden once the
oracle is re-pointed at the real driver.** Today it matches 150. That is not
`150 + 22`: it is 139 kept, 11 lost and 33 gained, and the middle term is this
task's finding.

| | entries |
|---|---|
| reproduced today, and by nothing the driver removes | 139 |
| reproduced today **only** because the feed supplies a terminal for a task type W3b has no handler for | 11 |
| not reproduced today, and nothing found blocks it | 33 |
| not reproduced today, and blocked | 111 |
| **TOTAL** | **294** |

`139 + 33 = 172` is the ceiling **if Task 5 removes the feed**, which is its
stated intent. `139 + 11 + 33 = 183` is the ceiling if the driver keeps feeding
a terminal for task types it has no handler for. The two are the same quantity
under two different answers to one question, and the composition is printed in
`handler-surface.txt` so a reader cannot take one for the other. **172 is the
bound W3b should be held to.** If Task 5 keeps the feed it must say so, and
report 183 as a decision rather than as progress.

**The 11 are the finding.** *What the oracle feeds in* records that W3a's
harness supplies the terminal of every `Runnable` run from the golden — 575 of
the golden's 840 run terminals. Eleven entries that the ratchet counts as
matching today contain a run of a task type W3b writes no handler for
(`OutputValues` ×6, `PurgeLogs` ×5, `Assert` ×1, overlapping on
`valids/assert`), and the feed is the whole of why they agree. A driver that
runs handlers instead of reading terminals cannot complete those runs at all.
They are named individually in `handler-surface.txt` so Task 5 cannot absorb
them into a net figure.

The two rules behind the table are deliberately different, and the difference is
what makes the number a bound rather than a guess:

- For an entry egret **already reproduces**, a construct it declares is proven
  inert by the match itself — `valids/sla-max-duration-ok` declares an `sla:`
  nothing evaluates and reproduces anyway. Only the feed can be taken away from
  it.
- For an entry egret **does not** reproduce, a declared construct no W3b task
  owns is the leading explanation, and a ceiling may not assume it away.

The second rule is deliberately strict, and the **direction** of its error is
the point: a construct declared and never exercised blocks an entry that does
not match today, so 172 can be too low and cannot be too high. A ceiling that is
too low costs a later task one edit to `handler_surface.py`, with the entry
named and the reason given. A ceiling that is too high costs nothing, which is
why it would not be a ceiling.

That asymmetry is guarded rather than described: `handler_surface.py` refuses to
write the file if any entry that matches today acquires a blocker that is *not*
a missing handler, because a construct or a state blocking an entry that already
agrees means the rules are wrong and not that the entry is at risk.

**What blocks the other 111, by cause.** The rows overlap and the causes are
counted over distinct entries: **102** a construct no W3b task owns, **29** a
state `RunState` cannot hold, **21** a missing handler, **5** an over-acceptance
that is W1's validation and not a handler at all. The largest single rows are
`Subflow` (22), `PAUSED` (18), `OutputValues` (17), `finally:` (16), `Pause`
(12) and `RESTARTED` (12).

**Two of the plan's four named causes are invisible to this derivation**, and
saying so is the point: an unreachable ordered pair and an output the handler
cannot produce are in neither a flow's YAML nor the golden's states. W3a
measured both in
`tests/executor.rs::the_residual_baseline_is_attributed_to_a_construct_or_named`,
so `handler-surface.txt` takes its thirteen named entries and **derives where
each one lands here**. Five land in the bankable bucket carrying a W3a cause
this file cannot re-derive:

| entry | what W3a says holds it |
|---|---|
| `valids/dag` | one `order` pair a clockless machine cannot reach |
| `valids/if` | three `order` pairs, same cause |
| `valids/parallel-disabled-tasks` | one `order` pair, same cause |
| `valids/full` | flow-level `outputs:` and `variables:` |
| `valids/retry-failed-flow-duration` | `maxDuration` with no elapsed time |

So the **working expectation is 172 − 5 = 167**, and a task that reaches above
it should say which of the five it moved. Two deserve a sentence:

- The three `order`-pair entries are the ones **spec §4.3 says the real driver
  makes worse**, not better: a message-driven runner applies a batch partially
  and produces the `CREATED` sibling W3a's Task 4 deleted a guard for, naming
  its trigger as *a caller that applies a batch partially — W4's worker*. That
  caller is now W3b's.
- `valids/retry-failed-flow-duration` is the one entry here whose blocker
  durability **removes**: `maxDuration` in an environment with no elapsed time,
  and a real driver has elapsed time.

### An attribution this derivation found wrong

`the_residual_baseline_is_attributed_to_a_construct_or_named` attributes
`valids/full` to *flow-level `outputs:` and `variables:`, same*. **It has
neither.** Its top-level keys are `id`, `labels`, `namespace`, `tasks` —
derived and printed in `handler-surface.txt`, so the sentence cannot drift back.
`variables:` would not hold it in any case: `render_context` supplies `vars`
from the flow's own `variables:`. Whatever holds
`valids/full`, it is not the two things that table names, and W3b must not plan
around them.

That is the third attribution in this file found wrong by deriving where an
entry lands instead of describing it — after `SKIPPED` credited to `disabled:`
and `WARNING` credited to Task 7.

### Task 1's mutations

Anchored to `git show HEAD:crates/egret-workflow/oracle-exec/handler_surface.py`;
each is a single literal edit, replayed against the final tree.

| # | mutation | what dies, and its message |
|---|---|---|
| M1 | `--self-test`, 13 cases over the committed inputs | all 13 fire, each asserting on its own substring so a case cannot pass because a different guard fired — e.g. `1 runs Kestra recorded name a task the YAML subset parser did not find, e.g. [('invalids/loop-switch-failed', 'foreach')]` and `valids/assert matches today and is blocked by [('construct', '…OutputValues'), …]` |
| M2 | `derive`: `!= len(runs):` → `!= len(runs) + 1:` | `the population split does not account for every comparable \`runs\` entry: 201 + 65 != 266` |
| M3 | `derive`: `!= len(population):` → `!= len(population) + 1:` | `the outputs split does not account for every member of the population: 45 + 156 != 201` |
| M4 | `derive`: `!= len(golden) - len(baseline):` → `… + 1:` | `safe + at-risk must be the entries egret reproduces today: 139 + 11 != 150` |
| M5 | `derive`: `!= len(baseline):` → `!= len(baseline) + 1:` | `bankable + blocked must be the baseline: 33 + 111 != 144` |
| M6 | `derive`: the four-bucket total `!= len(golden):` → `… + 1:` | `the ceiling's four buckets do not account for every comparable entry: 139 + 11 + 33 + 111 != 294` |
| M7 | `handler-surface.txt`: one line appended | `--check` prints the diff and exits **1**: `handler-surface.txt is stale: 5 diff lines. Re-run handler_surface.py`. On the clean tree it exits **0** — the check discriminates |

M2–M6 are the arithmetic guards, and they are the reason every count in
`handler-surface.txt` is asserted beside its parts rather than written next to
them. None of them can be reached by mutating the committed *data*, which is why
they are source mutations and the other thirteen are not.

## W3b Task 4 — `SpawnExecution` has an executor, and the two counts have not moved

**Read this before quoting the 39 or the 227 from anywhere else in this file.**
The hole is not closed. It has an executor for the first time, and the oracle
still cannot see it.

### The two counts, before and after

Both were re-derived before anything was changed and again after, and both are
unchanged:

| quantity | before | after | derived by |
|---|---|---|---|
| entries carrying a sub-execution | 41 | 41 | `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` |
| sub-executions they carry | 142 | 142 | the same test |
| entries differing on **nothing but** `spawned` | **39** | **39** | the same test, printed by name |
| kept ordered pairs at the top level | 877 | 877 | `only_the_top_level_order_pairs_are_ever_asserted` |
| kept ordered pairs **inside** `spawned` | **227** | **227** | the same test |
| ratchet: golden / mismatching / baselined / broken | 294 / 144 / 144 / 0 | 294 / 144 / 144 / 0 | `executor_matches_golden` |

**The published figures were right.** The golden-side halves were also
re-derived independently of this crate, in python straight from `golden.json`:
41 entries, 142 sub-executions, and `877 + 227 = 1104` with the nested total
attributed per entry (81 `loop-nested`, 45 `finally-loop`, 45
`finally-loop-parallel`, 10 `working-directory-taskrun-nested`, 6 `loop-switch`,
… over 18 entries, summing to 227). The 39 is the one figure a python pass
cannot reach — it needs egret's own `evaluate()` — so it was checked by counting
the **names** the test prints rather than by trusting the number it asserts.

### Why nothing moved, and why that is the honest answer

`tests/executor.rs` still drives the **harness**, not the worker.
`simulate_recording_spawns` collects `Command::SpawnExecution` beside the trace
and leaves `Trace::spawned` empty on purpose, so the comparison still zips an
empty left side. Re-pointing the oracle at the real driver is W3b **Task 5**,
and until it lands:

- the 39 entries still differ on `spawned` and are still baselined;
- the 227 nested pairs are still unreachable;
- and the ratchet still matches 150.

There is a second reason, and it is worth stating because it also bounds Task 5:
**nothing renders.** `advance()` is pure and the worker fills neither render
slot, so a `Loop` waits at `CREATED` until a caller writes `TaskRun::rendered`
and calls `worker::resume`. Today the only caller that does is
`tests/worker.rs`, which calls the real `render_str` with the real (empty)
`RenderContext` — the identity on a literal `values:`, an error on anything
else. Task 5 owns the render surface; this task owns what happens after it.

### What did move

`Command::SpawnExecution` went from *refused* to *executed*. Before this task
`worker::claim::drive` returned `WorkerError::NotYetExecutable` for it, so any
`Loop` aborted the tick; now `src/worker/spawn.rs` creates the sub-execution,
runs it, and reports its terminal back to the loop that spawned it.
Migration 0135 is the four columns that make a sub-execution addressable —
`parent_execution_id`, `parent_run_ix`, `loop_index`, `loop_value` — with a
CHECK that they are all set or all null and a UNIQUE index on the triple.

Thirteen tests are the instrument, because the ratchet is not one yet: five unit
tests in `spawn.rs` (the index derivation, its guard, the continuation's
branches, the KILLED reading, and a round-trip of the synthesised sub-flow over
**all 52** `Loop`s the corpus declares) and eight integration tests in
`tests/worker.rs` driving real Postgres through `start_execution`, `resume` and
`tick`.

### Three facts measured on the ARTIFACT, not on the checkout

W3b Task 3 established that the image and the `data-portal/kestra` checkout are
different builds, so every claim below is re-derived from `traces.json` — the
artifact's own output, harvested from the pinned digest
`sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`.

1. **`loopRun.index` is 0-based and contiguous.** Over the 670 loop
   sub-executions in `traces.json`, the index sets per spawning taskrun are
   `{0}` ×45, `{0,1}` ×40, `{0,1,2}` ×175 and `{0,1,2,3}` ×5 — 265 groups, none
   starting at 1, none with a gap. (The other 40 descendants carry `kind: null`
   and no index; they are not loop iterations.)

2. **The creation instant carries no information at all.** All **670** loop
   sub-executions have the `startDate` of their *parent execution*, and all
   **220** multi-sibling groups share one instant between them. `normalise.py`
   records that ordering by `startDate` made nine flows fail the five-round
   stability check; this is the stronger form of the same fact — the instant
   does not order these badly, it does not order them at all, and what actually
   decided the order in those nine rounds was the ksuid tiebreak. Ordering
   `spawned` by `loopRun.index` is therefore not a preference.

   `a_loop_over_two_values_spawns_two_sub_executions_ordered_by_index` asserts
   the same shape on egret's side: both iterations are written in one
   transaction, so Postgres's `now()` is identical for both and `created_at`
   cannot order them either.

3. **`concurrencyLimit` is a live cap, not a first-batch size.** `advance()`
   emits the first batch and cannot emit a second — it is pure over
   `(plan, state, now)` and `ExecutionState` records nothing about a
   sub-execution — so everything after the first batch is the spawner's. Three
   corpus entries pin the three branches, and all three are counts of what
   Kestra actually ran:

   | entry | limit | values | `transmitFailed` | sub-executions Kestra ran | loop run |
   |---|---|---|---|---|---|
   | `valids/loop-parallel-less` | 2 | 4 | default | **4** | SUCCESS |
   | `valids/loop-failed` | default (1) | 3 | default (true) | **1** | FAILED |
   | `valids/loop-failed-no-transmit` | default (1) | 3 | `false` | **3**, all FAILED | **SUCCESS** |

   The third row is the one a reader gets wrong from the name: a loop whose
   every iteration failed **succeeds**, unless `transmitFailed` short-circuited
   it first.

### Where this is knowingly narrower than Kestra, and what it will cost Task 5

A `Loop`'s own `errors:` and `finally:` run **in the sub-execution**.
`valids/all-flowable` is the measurement: its sub-execution runs `1-each-1`
(FAILED) and then `1-each-seq-error`, which is the `Loop` task's `errors:`
list, while the *flow-level* `errors:` (`error-1st`, `error-2nd`) run in the
parent. **W1's `models::Loop` carries neither field** — it holds `tasks`,
`values`, `concurrencyLimit` and `transmitFailed` — so both are gone before the
spawner can see them, and the synthesised sub-flow cannot put them back. This is
the same `Loop`-has-no-`errors` gap the unclaimed table already records for
`advance()`, showing up a second time in a second place. Widening the model is
W1's; the entries it costs are Task 5's to attribute rather than to discover.

### Mutation

Baseline for every replay: `cargo test -p egret-workflow --no-fail-fast` —
**648 passed, 0 failed**. Each mutation is one literal `-` → `+` applied by a
script that **refuses unless the anchor occurs exactly once**, built first and
asserted to compile (0 errors), run against the whole crate, then restored with
the restore verified by md5 (`spawn.rs` `4313416512089e9857aa6b534bb32ed8`,
`claim.rs` `6ad8a5365f969162140faef5e65c0aff`) — `cp` is aliased to `cp -i` in
this shell and returns 0 having written nothing, which is how W3b Task 2 lost a
snapshot.

**M0 is the failing-first evidence.** The tests were written beside the
implementation, so the evidence is a stub pass replayed against the final tree:
`spawn_all` returns `Ok(vec![])` and `on_execution_terminal` returns `Ok(())`.
**8 of the 8 integration tests fail**; the five unit tests pass, because they
drive the pure decisions the stub did not touch, and that is said here rather
than counted.

| # | mutation (literal) | fires in | message |
|---|---|---|---|
| M0 | `spawn_all` → `Ok(Vec::new())`, `on_execution_terminal` → `Ok(())` | 8 | ``two values, two sub-executions, indexed 0 and 1 in `values:` order — left: [], right: [(0, "a"), (1, "b")]``; and ``the body ran once per iteration, in its own execution — left: 0, right: 2`` |
| M1 | `if transmit_failed && terminated_in_error(child) {` → `if terminated_in_error(child) {` | 2 | `with the flag off a failure starts the next iteration like any other — left: Terminate(Failed), right: Start(1)`; and `left: [a FAILED], right: [a, b, c all FAILED]` |
| M2 | `} else if started < total {` → `} else if started + 1 < total {` | 3 | `and the other two are started one at a time as each terminates — left: [a, b], right: [a, b, c]` |
| M3 | `let index = base + k;` → `let index = k;` | 2 | ``an iteration started after two others is index 2, not index 0 -- `loopRun.index` is the flow's numbering, not the batch's`` |
| M4 | `Some(expected) if expected != value =>` → `… if false && expected != value =>` | 1 | `the batch must be a PREFIX of the values not yet started — left: Ok([0]), right: Err("spawn 0 carries value \"b\", but iteration 0 of its values is \"a\"…")` |
| M5 | the "loop run already terminal" early return deleted | 1 | `three iterations reported a failure to one loop run; it must carry ONE FAILED, not one per sibling — left: [CREATED, RUNNING, FAILED, FAILED, FAILED], right: [CREATED, RUNNING, FAILED]` |
| M6 | `matches!(state, RunState::Failed \| RunState::Killed)` → `matches!(state, RunState::Killed)` | 4 | ``one iteration failed and `transmitFailed` defaults to true, so the other two values are never started — left: [a, b, c all FAILED], right: [a FAILED]`` |
| M7 | `settle`: `if let Some(terminal) = is.filter(\|s\| s.is_terminal())` → `if let Some(terminal) = is` | 7 | `resume: Internal(error returned from database: duplicate key value violates unique constraint "workflow_executions_loop_run_idx")` |
| M8 | `sub_flow_source`: `flow.tasks = body;` → `flow.tasks.extend(body);` | 9 | ``holds a source that no longer plans: duplicate_task_id: task id 'item' is declared more than once: tasks.loop.item, tasks.item`` |

**M3 and M4 fire only in the unit tests, and that is a finding rather than an
omission.** `advance()` emits exactly one batch per loop run, so `base` is
always 0 where `spawn_all` reads it and the batch is always the values' own
prefix by construction. Both are guards on a path only a *second* emitter could
take. They are kept because the alternative — indexing by emission order — would
be silently wrong the day one appears, and they are tested through pure
functions precisely so that "unreachable today" does not mean "unchecked".

**M7 is the one worth reading twice.** Reporting every state change to the
parent rather than only a terminal one makes the loop start its next iteration
while the previous one is still `RUNNING`, and the failure that surfaces is
migration 0135's `UNIQUE (parent_execution_id, parent_run_ix, loop_index)` —
the collision `normalise.py` has to *detect and bail on* is, on this side,
unrepresentable.


## W3b Task 5 — the harness is gone, and the ratchet judges the real driver

This is the W3b package's deliverable. `tests/executor.rs` no longer contains a
state machine driver at all: `oracle-exec/driver.rs` starts a real execution
with `worker::start_execution`, drives it with `worker::tick`, and reads the
trace back out of Postgres. Every entry the ratchet banks is one the **shipping
system** reproduces.

### The before and after, and the move is two events rather than one

| | before | after |
|---|---|---|
| ratchet: golden / mismatching / baselined / broken | 294 / 144 / 144 / 0 | 294 / 144 / 144 / 0 |
| entries egret reproduces | 150 | 150 |
| banked by this task | — | **16** |
| lost by this task | — | **16** |
| entries where egret produces a `spawned` at all | **0** | **35** |
| of the 41 entries carrying a sub-execution, how many MATCH | 0 | **16** |
| ordered pairs egret produces inside `spawned` | **0** | **126** |
| golden run terminals FED from `golden.json` | **630** (575 runnable + 55 loop) | **0** |

The headline number did not move and the two halves under it are what this task
did. **Reporting `150 → 150` alone would be the worst possible summary of it**,
which is why `oracle-exec/baseline.txt`'s header names all 32 entries with a
cause each.

### The three holes W3a published, before and after

1. **39 entries differed on nothing but `spawned`.** egret's side of the field
   was `Vec::new()` unconditionally. It is populated now: 35 entries produce a
   sub-execution trace and 16 of the 41 match outright
   (`the_sub_executions_are_compared_and_this_is_what_they_cost`). Six still
   differ only inside `spawned` — `invalids/loop-switch-failed`,
   `valids/retry-flowable-parallel` and the four `retry-new-execution-*`.
2. **227 of the 1104 kept ordered pairs were never asserted**, because
   `Trace::restrict_order_to` descends through a `zip` with an empty left side.
   egret now produces **126** pairs inside `spawned`
   (`the_ordered_pairs_inside_spawned_are_reachable_now`). The golden-side split
   is unchanged and still asserted at 877 + 227 = 1104.
3. **The oracle was circular for one field** — a `Runnable` run's last state and
   each attempt's last, plus a `Loop` run's terminal. **It is not fed at all any
   more.** `driver::run` takes a flow source and the posted inputs; nothing on
   egret's side reads `golden.json`. Demonstrated rather than asserted: rewriting
   `valids/minimal`'s run terminal AND attempt terminal in `golden.json` from
   `SUCCESS` to `KILLED` makes the ratchet report it as a REGRESSION
   (145 mismatching, `valids/minimal` named), where the same corruption used to
   leave the entry matching.
   `a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to` is the
   committed version, and it carries the **negative control W3a's first attempt
   lacked**: an entry for which the same corruption changes nothing, derived as
   "a golden whose first run's history holds a state `RunState` cannot hold"
   rather than named — W3a's chosen `valids/minimal2` matches now, because W3c
   added `SUBMITTED` to the enum.

### What the driver still supplies, in full

Three things, all properties of the environment rather than of the execution's
semantics, and none read from the golden:

1. **The inputs** `harvest.py` posted, out of `corpus.json`. Unchanged from the
   harness.
2. **The passage of time.** `driver::pass_time` moves `executions.created_at`
   back by exactly the shortfall to the earliest `not_before` still ahead, then
   calls `worker::resume`. That is the harness's tier 4 against a real column.
3. **`core.flow.Sleep` does not really wait.** The registry uses
   `Sleep::capped(ZERO)`, which validates the duration exactly as the product
   handler does — the `@DurationMin(millis = 1)` refusal included — and then
   returns. Four corpus flows sleep for 30 s or more and one for `PT10M`.
   Nothing compared can see it: `normalise.py` drops every clock, and egret's
   happens-before endpoints are the logical `executions.seq`.

### A stall is a wrong answer now, not a broken instrument

The harness completed every `Runnable` from the golden, so an execution that
could not finish meant the state machine was spinning and was reported as
`Broken`. With real handlers a flow whose task type has no handler **stalls** —
`Registry::run` refuses an unknown type rather than inventing a state — and
stalling is what the shipping worker does. So the driver returns the partial
trace, and only exceeding the step bound or panicking is `Broken`. **84 of the
294 entries end unfinished**, counted and named by
`every_mismatching_entry_is_attributed_to_the_field_it_differs_on` so the
population cannot grow in silence.

### The render surface, which is the largest thing this task added

`worker/render.rs` is the tier `advance()` cannot have and Task 2 shipped
without: until it, a flow with a `runIf:`, an `If`, a `Switch` or a `Loop` got
an empty command vector **for ever**. It lives inside `claim::settle`, between
two rounds of `advance()`, so the shipping worker resolves them and not only the
oracle. What it reaches: `inputs` (coerced by the input's declared `type:`, and
that now applies to POSTED values and not only to defaults), `vars`, `outputs`
(a task's own and its ancestors'), `task`, `taskrun`, `execution`, `flow`,
`labels`, `item` (with `key`, `parent` and `parents`) and the `tasks` variable
`tasksWithState()` reads. What it still refuses, and therefore FAILS a run on:
`trigger`, `kv`, `secret`, `envs`, `globals` and the flow-level `outputs:`
block.

Two of those names are measurements rather than choices:

- **`outputs.<id>` is `{}` for a task that produced nothing, never an absent
  key.** The W3b Task 3 brief said the opposite and called it load-bearing for
  this module; Task 3 measured it on the artifact and it is the other way round.
- **`taskrun.attemptsCount` is the number of attempts already FINISHED**, which
  is one fewer than egret holds while the current one is open. Read off the
  golden: `valids/restart-child` renders `{{ 1 / taskrun.attemptsCount }}` and
  Kestra FAILED it, and `valids/restart_last_failed` renders
  `{{ taskrun.attemptsCount == 1 ? 'ok' : ko }}` and Kestra FAILED that too —
  `ko` is undefined, so the ternary took the else branch, so the count was not 1.

### The guard W3a deleted did NOT come back, and the reason is the transaction

W3a deleted Kestra's "first created, leave" (`resolveParallelNexts` returns
nothing while any existing child run is still `CREATED`) because the mutation
bypassing it killed nothing, and named the trigger: *"a caller that applies a
command batch partially — W4's worker."* It measured the decision point at 0
under its own harness and at `valids/dag` 1 / `valids/if` 3 under a driver that
applies one worker result **between** `advance()` calls.

`worker::CREATE_BESIDE_A_CREATED_SIBLING` counts that decision point over the
whole corpus and it is **0**. The reason is a real difference between the driver
W3a predicted and the one W3b built: `settle` applies every command of one
`advance()` round before asking again, and the whole cascade is one transaction,
so a worker result is never delivered *between* two rounds of one batch. W3b's
driver is transaction-driven, not message-driven, and the guard's state needs
the second. A zero is only a measurement if the instrument can move, so
`worker::claim`'s `the_create_beside_a_created_sibling_counter_can_fire` drives
the decision point directly and asserts both that the counter increments and
that a child of a *different* parent does not move it.

**The three entries W3a named are still baselined, and what holds them is
something else**: `valids/dag`, `valids/if`, `valids/parallel-disabled-tasks`
and — the finding — `valids/full` are the four entries whose runs, attempts and
execution history are byte-identical to Kestra's and which differ on the
happens-before relation ALONE. The cause is this driver's delivery discipline:
it finishes ONE task run per tick and creates that run's successors in the same
transaction, where Kestra's worker pool finished two siblings before either
successor existed. W3b Task 1's Concern 3 said `valids/full`'s recorded
attribution was wrong and its real cause unknown; this is the cause, and it is
the same one.

### The happens-before endpoints are a logical clock now, and they had to be

Migration 0136 adds `executions.seq`, `task_runs.created_seq` and
`task_runs.ended_seq`. A `timestamptz` cannot carry the relation: every row a
`settle` writes shares one `now()`, because `now()` in Postgres is TRANSACTION
time. That is the same shape W3b Task 4 measured on Kestra's own side — all 670
loop sub-executions carry their parent execution's `startDate` and all 220
multi-sibling groups share one instant, so the instant does not order siblings
badly, it does not order them at all.

## W3b Task 6 — `egret.sql`, and the coverage this ratchet does NOT give it

**Read this before reading a green ratchet as coverage of the workflow SQL
task.** It is the one handler egret ships that the oracle cannot say anything
about, and the whole of Task 6's contribution to this file is making that
unmissable rather than inferable.

### The measurement

| population | flows naming an `egret.*` task type | flows naming `core.log.Log` (the control) |
|---|---|---|
| the whole corpus — 345 flows | **0** | 108 |
| the golden — 294 entries, i.e. what this oracle compares | **0** | 97 |

Derived on every run by
`no_corpus_entry_can_exercise_the_egret_sql_handler`, which walks every corpus
source and asserts both triples whole. Kestra has no `egret.*` type, so this is
not a gap somebody could close by harvesting more of Kestra's test suite: **no
Kestra corpus can ever exercise `worker::handlers::sql`.**

**The `Log` column is the point of the table, not decoration.** A zero is only a
measurement if the instrument can move, and a scanner that parsed nothing would
report zero `egret.*` flows and look exactly like the truth. It cannot also
report 108 flows naming a type it failed to parse.

That control also caught its own first version. A scanner requiring `type:` to
begin its line gave **106**, missing `invalids/invalid` and
`invalids/system-labels`, which write `- type: io.kestra.plugin.core.log.Log` as
a list item. Two entries, in the direction that would have made a broken scanner
look healthy.

### What proves what, kept apart

| claim | instrument |
|---|---|
| the state machine reproduces Kestra on 150 of 294 comparable entries | this ratchet |
| `Log`, `Return`, `Fail`, `Sleep` produce what Kestra's classes produce | this ratchet, plus each handler's own tests |
| two workers never claim one run; a crash applies nothing; the reaper takes back exactly the expired | `tests/worker.rs` — the ratchet cannot see any of it |
| **`egret.sql` binds rather than interpolates, scopes to the workspace, and writes the output Postgres holds** | **`tests/sql_starrocks_live.rs` against a real StarRocks, and `worker::handlers::sql`'s own unit tests. Nothing else.** |

`egret.sql` is in **neither** registry, and that is structural rather than an
oversight — it answers the open question Task 5 handed on ("a fifth handler needs
adding to both or to neither"). `Handlers::with_core_tasks()` takes no arguments
and `driver::registry()` must never reach an engine, while `Sql` is constructed
from an `Arc<dyn QueryExecPort>`. A caller wires it in one line; the oracle never
does, because a handler no corpus flow can call has nothing to contribute to a
comparison.

### What the live suite asserts, and why each assertion could not pass against a fake

`deploy/seed-starrocks-workflow.sh` writes five rows into
`egret_workflow_live.shipments`: `('EU', 40), ('EU', 51), ('US', 6), ('US', 7)`
and a fifth whose `region` is the literal text `EU' OR '1'='1`.

- **A bound value is data, not SQL.** The statement is
  `WHERE region = {{ inputs.r }}` and the value is that injection text. Bound, it
  matches the one row holding it — **1** of 5. Interpolated,
  `WHERE region = 'EU' OR '1'='1'` matches **all 5**. Both answers are non-zero,
  so the assertion cannot pass by the statement having been broken, which is the
  failure an "expect zero rows" version would have had.
- **The numbers are the engine's.** `EU => 91` is a SUM of two rows; `91` appears
  nowhere in the flow source, in the test, or in any fixture in this crate, and
  the `Spy` port the unit tests drive could not invent it. It is deliberately not
  the `42` `deploy/seed-starrocks-dataapi.sh` writes.
- **`fetch: NONE` produced `{}` AND the statement still ran.** The second half is
  what makes the first mean anything: the same `NONE` over a statement naming a
  column that does not exist fails, and the engine's own words reach the author.
  A handler that skipped execution for `NONE` would answer `{}` there too.
- **The port's workspace scoping, exercised rather than trusted.** The same
  handler and statement against a `database` registered in a *second* workspace
  is refused, with the first workspace's success as the control — without which a
  handler that refused everything would pass.
- **The output is read back out of Postgres**, driven through
  `worker::start_execution` + `worker::tick` rather than by calling the handler.
  W3b already paid for this hole once: Task 3's guard on `Return`'s output died
  only to in-process tests, because `task_runs.outputs` did not exist yet.

The suite self-skips without `EGRET_TEST_STARROCKS_URL`, following the
convention this repo already has (`EGRET_TEST_TRINO_URL`, `EGRET_TEST_S3_ENDPOINT`),
because CI has no StarRocks. **A skipped run therefore proves nothing**, and that
is the honest cost of the trade rather than a footnote.

### Three things `egret.sql` does not have, stated here rather than found later

1. **No cancellation.** `TaskHandler::cancel` **refuses** on this handler rather
   than answering `Ok(())`: it holds a statement running on a remote engine, and
   `QueryExecPort` has no cancellation method. Nothing calls `cancel` at all yet
   (W4 owns the kill path), so adding one to the port would be adding a method
   ahead of its caller twice over. The refusal is what makes W4 fail loudly at
   the one place that has to change.
2. **No `run_as`.** The statement runs as `TaskInput::principal`, which
   `worker::claim` reads from `workflow.flows.created_by` through
   `executions.flow_id`. That is the rule `egret-automation` already established
   for an unattended run — as the owner, and *fail* rather than fall back to the
   service account — but the column is the flow's *creator*, is
   `ON DELETE SET NULL`, and is absent entirely for an execution with no
   `flow_id`. The fix is a `run_as` on `workflow.executions`, and W6 is the
   package that has to decide what a *scheduled* run acts as.
3. **No identifier binding.** `ORDER BY ?` is a syntax error on both engines, and
   this handler has no `| ident` escape hatch, because a flow declares no
   `allowed` list for one to emit the author's own string from. A dynamic column
   list belongs in `{% if %}` branches over literal SQL.

### The `TRUNCATE` ruling: the premise was false, and the invariant is now structural

The plan ruled that `egret_side()` "truncates `workflow.executions` in the shared
`egret_test` database" and had to be scoped or given a private database.
**Measured, that premise is false.** `common::db()` goes through
`egret_test_db::url()`, which does not return `DATABASE_URL`'s database at all —
it clones a migrated template into `egret_t_<pid as 32 hex digits>`, one per test
*process*, and drops it when the process exits. A probe printed:

```text
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
current_database()=egret_t_000000000000000000000000000185c6
```

So the per-test database the ruling asked for was already taken, and has been
since before Task 5; the destructive statement never reaches `egret_test`, and a
concurrent run loses nothing. **Task 5's Concern 4 named a real hazard about the
wrong database.**

What Task 6 changed is that this is now enforced rather than incidental.
`is_process_clone` is checked immediately before the `TRUNCATE`, so the statement
cannot run against anything but this process's own clone even if a future edit
points `common::db()` somewhere else — which is the change that would otherwise
reintroduce exactly the destruction the ruling was written about. The predicate
is anchored on both ends (`egret_t_` plus **exactly** 32 hex digits) because
`LIKE 'egret_t_%'` is what once dropped `egret_test` in this repo: `_` is a
single-character wildcard in `LIKE`, so the obvious pattern matches the very
database it was written to protect. `the_truncate_is_confined_to_this_process_own_clone`
drives it in both directions, `egret_test` first, because on the live path the
predicate can only ever answer `true`.

### The ratchet did not move, and that is not evidence of anything

150 of 294, unchanged from Task 5, and it could not have moved: this task added a
handler zero corpus entries reach. It is recorded because a number that stays
still for a good reason and a number that stays still because nothing ran look
identical in a table.

## What this oracle does not prove

> **This is W3a's hole list, kept as W3a's record. Its successor is *What this
> ratchet does not prove* inside *The coverage statement*, and where the two
> disagree that one is right.** Three bullets below are false since W3b Task 5
> re-pointed the ratchet at the real driver and are annotated in place: the
> ordered-pair bullet (**354** of the 1104 can fail, not 1104 and not 877), the
> sub-execution bullet (`Command::SpawnExecution` has an executor and this
> oracle drives it), and the render-context bullet (the render tier lives in
> `worker::render.rs` now, and the driver is not the thing that renders). The
> holes that are *not* in the coverage statement are here and only here — the
> `else`-branch population, the two render-tier shapes, `PlanTask::timeout`, the
> ION-file `values:` shape — and every one of them is still open.

The corpus is Kestra's own test suite, which is a sample of an infinite space.
The ratchet prevents regression; it does not bound behaviour on a flow nobody
wrote. Beyond that, the specific holes, each with its count:

- **26 `not-standalone` flows are never compared at all.** They have a
  corpus entry and no golden entry. Kestra itself cannot run them without what
  the Java test brings — a resume, a trigger, an uploadable file, a mocked
  worker, or an input value that is computed rather than literal.

  **Task 8 found two defects in there, and the hole is what hid them.** Running
  `validate` behind `build_plan` refuses `valids/task-flow-dynamic` and
  `valids/trigger-flow-listener-namespace-condition`, both of which this Kestra
  published with 200. Because neither has a golden entry, **the ratchet reported
  nothing** — it was green through the change that introduced them. What led to
  them was a corpus-wide sweep of `validate` over all 345 flows, and the reason
  that sweep was run at all is unrelated: `every_corpus_retry_is_written_in_literals`
  counts over every corpus flow `build_plan` can plan, its total moved by one,
  and finding out which flow had left the population meant looking at all of
  them. "Never compared at all" is not a theoretical limit, and the instrument
  that caught it was a *count over the whole corpus*, not the golden.
- **23 rejections are not evidence of anything about the flow**, and are
  excluded for that reason. Nothing in this package tests what egret does with
  them — Task 8 included, which is the point: making egret refuse them would be
  a defect manufactured out of a naming convention, and `corpus.json`'s
  `rejected_class` rather than the `invalids/` directory is what keeps them out.
- **2 flows did not reproduce across five rounds** and are excluded:
  `retry-failed-task-duration` (three attempts in some rounds, four in others —
  a *duration*-bounded retry counts whatever fits) and `waitfor-max-duration`.
  Both are bounded by wall-clock duration, so this is the flow being
  nondeterministic and not the harness.
- **13 ordered pairs were dropped**, all of them because the relation flipped
  between rounds; none because a run had no start or no end. The kept 1104 —
  743 `before`, 361 `overlapping` — are the whole of what this oracle asserts
  about serialisation.
  *(**Wrong, for the second time — see* The coverage statement*.** The first
  correction took 227 off this claim because they were unreachable; W3b Task 7
  measured that only **354** of the 1104 are pairs a change to which the ratchet
  reports at all. The other 750 sit on baselined entries, which mismatch whatever
  the pair says — flipping all 750 leaves the ratchet green and unmoved. That was
  as true when this sentence was written as it is now.)*
  **Since Task 4 the ratchet compares only those**: egret
  produces a relation for every pair, the golden does not, and comparing an
  assertion against an absence was costing `valids/parallel-nested` a bank on 3
  pairs the golden itself declines to state. Because egret's relation domain is
  measured at `{before, overlapping}` (**647** pairs after the final review,
  656 after Task 7, 555 after Task 6, 557 after Task 5, zero `after`), a dropped
  pair is one Kestra was seen producing *both* values for, so dropping it is
  exactly accepting either. Derived and guarded by three tests — see *What Task
  4 had to change, and what it banked*. Driven through the real `advance()` the
  projection removes **15 pairs across 4 entries** after the final review (30
  across 9 at Task 7, 25 across 7 at Tasks 5 and 6, 22 across 6 at Task 4).
  Task 7's 101 new pairs are all made by `errors:` runs — a run that exists is
  in a pair with every other run of its execution. The final review's
  `disabled:` takes 9 back off and removes five entries from the projection,
  every one of them a `disabled:` flow whose extra pairs existed only because
  egret was creating runs Kestra never created. The two that remain beyond the
  golden's own 13 are `valids/exit-killed`'s, whose run set differs for a reason
  that is `Exit`'s and unclaimed.
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
  `worker/src/main/java/io/kestra/worker/processors/internals/WorkerTaskCallable.java:61`,
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
  the run** (Task 5).
  *(**Superseded at W3b Task 5, and the "caller" is no longer the oracle.** The
  render tier lives in `worker::render` inside `claim::settle`, so the shipping
  worker resolves a `runIf:`, an `If`, a `Switch` and a `Loop` rather than only
  this harness — and it reaches `inputs` (coerced by the declared `type:`, for
  POSTED values as well as defaults), `vars`, `outputs`, `task`, `taskrun`,
  `execution`, `flow`, `labels`, `item` and the `tasks` variable
  `tasksWithState()` reads. What it still refuses, and therefore FAILS a run on:
  `trigger`, `kv`, `secret`, `envs`, `globals` and the flow-level `outputs:`
  block. Every count below is W3a's, and the three tests it names went with the
  harness — see* The coverage statement, Pointers that no longer resolve.*)*
  `advance()` cannot render, so the oracle does; it supplies
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
  (`WorkerTaskCallable.java:61` wraps `task.run(runContext)` in
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
  distribution — **from the raw YAML, because its round-0 version could not
  fail**. It matched on W1's parsed kind, so a type whose model has no `errors`
  field contributed zero by construction and the map could only ever hold the
  five types that already had one. It reported "9 tasks on three types" and was
  hiding a fourth. Measured properly over all 345 flows: 17 write a flow-level
  `errors:` and **18 tasks** write one, on **seven** types — `Sequential` 7,
  `AllowFailure` 4, `Loop` 4, `Dag` 1, `Parallel` 1, `Pause` 1,
  `WorkingDirectory` 1. `If` and `Switch` carry the field because Java does and
  no corpus flow writes one there.

  Of those seven, three are planned opaque (`AllowFailure`, `Pause`,
  `WorkingDirectory`) so an unread `errors:` costs nothing, and **`Loop` is a
  gap**: `advance()` drives it, `models::Loop` has no `errors` field, and four
  flows' lists are invisible. See the unclaimed table.

  (The `valids/`-only reading of that distribution gives `Loop` 3 and
  `Sequential` 6, and dropping `invalids/` is what costs the difference:
  `invalids/loop-switch-failed` is a `runs` entry Task 1 measured is not an
  invalid flow at all, and `invalids/duplicate` is a flow egret over-accepts.
  `every_corpus_flow()` is the population everywhere in this crate.)
- **Sub-executions are ASKED FOR and still not produced, and Task 6 did not
  close this.** `Command::SpawnExecution` is emitted; nothing executes one until
  W3b/W6, and `ExecutionState` holds no field for a sub-execution.
  *(**W3b Task 4 changed half of that sentence and not the other half.** The
  command now HAS an executor — `worker::spawn` — but this oracle still drives
  the harness, so egret's side of `spawned` is still empty and every number in
  this bullet is unchanged. See* W3b Task 4 — `SpawnExecution` has an executor,
  and the two counts have not moved.*)*
  *(**W3b Task 5 changed the other half, and the whole bullet is now history.**
  This oracle drives `worker::start_execution`/`worker::tick`; egret's `spawned`
  is populated for **35** entries, **16** of the 41 MATCH, **6** differ only
  inside the field and 19 on more, and egret produces **126** ordered pairs
  inside `spawned`. Task 4 left this sentence annotated rather than rewritten
  precisely so W3b Task 7's stale-twin sweep would meet it; it did. See* The
  coverage statement.*)* So egret's
  side of `spawned` is still **always empty** and every one of the **41** golden
  entries that carries a sub-execution mismatches on that field alone. **39 of
  the 41 now differ on nothing else** — they reproduce every run, parent,
  history, attempt and kept ordered pair Kestra recorded, and are baselined
  purely for a field egret does not fill. It was 33 at Task 6 and 38 at Task 7;
  the five Task 7 adds are the four `retry-new-execution-*` entries and
  `valids/retry-flowable-parallel`, and the final review adds the 39th,
  `valids/disable-flowable`. A `CREATE_NEW_EXECUTION` retry ends its
  execution `RETRIED` and Kestra then **replays the whole flow as a different
  execution**, which is a sub-execution of a second kind: it carries
  `parent_run: null` rather than a loop run, and nothing in `Command` can ask
  for one. The remaining two — three until the final review implemented
  `disabled:` and closed `valids/disable-flowable` — are
  `valids/loop-outputs-auto` and `valids/loop-values-from-uri` (the ION-file
  `values:` shape), and both are the harness rather than `advance()`.

  Those entries look compared and are not.
  `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` is the
  marking the plan asked for: it derives all three numbers, requires all 41 to
  be baselined, and names the 2 that differ on more with a reason each. What
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

## The coverage statement

This is the last section on purpose. Everything above is what this oracle
proves; this is the boundary of it. **W3b Task 7 re-derived every count here on
this tree** rather than editing the previous statement, because W3b Task 5
replaced the test harness behind the ratchet with the real driver — a claim loop
over Postgres, five handlers and a spawner — and the thing being measured
changed underneath every sentence. Several numbers moved in directions the old
wording did not allow for, and one of them (the ordered pairs) had already been
found over-claiming by 227 once.

**Where this section and anything above it disagree, this section is right**,
and the disagreements are listed by name in *The stale twins this statement
retires* at the end. Every **count** below is printed by a named test in
`crates/egret-workflow/tests/executor.rs` on every run, so none of them is a
number written down beside a derivation. Three things here are not counts and
say so where they appear: the **630** fed terminals, which is history and whose
code was deleted with the harness; the provenance, which is a fact about a
docker tag; and the two **experiments** — flipping ordered pairs and corrupting a
terminal — which are recorded with their method and their control so they can be
re-run rather than believed.

### Provenance, stated as narrowly as it is true

The golden was harvested against the image tagged **`kestra/kestra:v2.0.0-rc13`
as that tag stood** — not against a version and not against a digest.
W3b Task 3 measured that the tag is **mutable over a nightly build**: the image's
own labels read `version=nightly`,
`revision=169ca7f25a1b3aeeaaab1206a89b2446a1335a46`, created 2026-09-01, and
`docker-compose.yml` carried `pull_policy: always` for the whole of W3a, W3c and
W3b Tasks 1–3, so any `up` could have replaced the authority behind the corpus
without a word.

`docker-compose.yml` pins
`kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`
with `pull_policy: missing` — but **only since `e72c83cc`, which is after this
golden was harvested**. So that digest is what a *re-harvest* would use; it is
not established to be what *this* harvest used, and the honest statement is the
one this paragraph makes rather than a version number. The class citations
scattered through this file come from the sibling checkout `96893b589a`
(2026-08-17), which does **not** contain the image's revision — which is exactly
how a class-level citation and an artifact measurement came to disagree about
`Sleep`'s `@DurationMin`.

### The denominators

| | count |
|---|---|
| flows in Kestra's `valids/` + `invalids/` at checkout `96893b589` | **345** |
| … `runs` — published and reached a terminal state standalone | 268 |
| … `rejected` — Kestra refused it | 51 |
| … `not-standalone` — Kestra itself could not run it here | 26 |
| golden entries, i.e. flows this oracle compares at all | **294** |
| … `runs` verdict (268 minus the 2 whose trace did not reproduce) | 266 |
| … `refused` verdict (the 28 rejections that are evidence) | 28 |

`345 = 268 + 51 + 26` and `294 = (268 − 2) + 28`, asserted by
`every_corpus_flow_is_compared_or_excluded_by_name`, which also prints the
exclusions so a flow can never be *silently* skipped.

**51 flows are never compared at all**, and the three reasons are different:

```text
  26   not-standalone
  23   rejections that are not evidence about the flow
   2   traces that did not reproduce across five harvests
  ---
  51   TOTAL
```

- **26 `not-standalone`.** They have a corpus entry and no golden entry. Kestra
  itself cannot run them without what the Java test brings — a resume, a
  trigger, an uploadable file, a mocked worker, or an input value that is
  computed rather than literal. Whatever egret does with them is unmeasured, and
  **two of them are the over-refusals W3a Task 8 found**: precisely because they
  have no golden entry, the ratchet could not have told anyone.
- **23 rejections that are not evidence** — 22 `task-type-not-in-image` and 1
  `checkout-drift`: the pinned image or the sibling checkout differing from each
  other, never Kestra judging a flow. Demanding a refusal for those would
  manufacture a defect out of a packaging accident, and the class is *equally*
  not evidence that the flow is valid.
- **2 flows did not reproduce across five harvests** and are excluded by name:
  `valids/retry-failed-task-duration` and `valids/waitfor-max-duration`. Both are
  bounded by wall-clock duration, so this is the flow being nondeterministic, not
  the harness.

*(The previous statement wrote **49** here and then listed 26 + 23. The two
unstable traces are a third reason and were left out of the total. It survived
review because the table above happens to carry an unrelated **51** — every flow
Kestra rejected, 28 of which this oracle very much does compare — so the wrong
number looked like a number that was already on the page. A coincidence of size
between two different sets is how an arithmetic slip becomes invisible.)*

### Matching, baselined, and what each half is made of

| | count |
|---|---|
| golden entries | **294** |
| matching | **150** |
| baselined — egret does not reproduce it yet | **144** |

```text
  matching  150 = 127 of the 266 `runs` + 23 of the 28 `refused`
  baselined 144 = 139 of the 266 `runs` +  5 of the 28 `refused`
  ------------------------------------------------------------------
            294 = 266 + 28
```

144 is simultaneously the mismatching count, the size of `baseline.txt` and the
baselined count: **0 stale lines, 0 regressions, 0 unbanked progress, 0 broken**.
That identity is the ratchet's own output, not a hand check.

The five `refused` entries on the baselined side are **over-acceptances** —
flows Kestra refused and egret plans — and they are the whole of egret's measured
disagreement about whether a flow is acceptable at all. Each is named with an
owner in `egret_refuses_every_flow_kestra_refuses`'s `STILL_OVER_ACCEPTED`; the
other 23 refusals are reproduced across 17 causes. Everything else in the
baseline is a flow egret plans and does not yet *reproduce*, which is a smaller
claim than "egret is wrong about it".

**150 did not move at W3b Task 5, and reading it as "nothing happened" would be
the worst possible summary.** Under it, **16 entries banked and 16 were lost**,
every one named with a cause in `oracle-exec/baseline.txt`'s header. The 16 lost
are entries W3a's harness kept alive by *feeding* them a terminal state; the 16
banked are sub-executions the driver actually runs. A number that stays still
because two events cancelled looks identical, in a table, to a number that stayed
still because nothing ran.

### The ordered pairs: **kept is not asserted**

```text
  kept by the harvest (identical in all five rounds)      1104
    at the top level                                       877
    inside `spawned`                                       227

  a change to which the ratchet REPORTS                    354
    at the top level                                       255
    inside `spawned`                                        99
  a change to which it does NOT                            750
    at the top level                                       622
    inside `spawned`                                       128
  ----------------------------------------------------------
  TOTAL                                                   1104
```

Derived by `kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line`
from `golden.json` and `baseline.txt`, with the total asserted beside the parts,
and cross-derived from the other direction by
`the_ordered_pairs_inside_spawned_are_reachable_now`.

**The line is the baseline, not the nesting**, and that is the whole correction.
W3a published *"877 asserted, 227 never"*, and the 227 were right for a
structural reason: `Trace::restrict_order_to` descends through
`self.spawned.iter_mut().zip(&golden.spawned)`, and egret's `spawned` was
`Vec::new()` unconditionally, so that `zip` never iterated. The driver fills
`spawned` — 35 entries produce one — so the reason is gone. **The replacement is
not 1104.** A pair on a *baselined* entry is compared and cannot be falsified:
the entry mismatches whatever that pair says, so changing it changes nothing the
ratchet reports. That was true of the 622 top-level pairs on baselined entries
under W3a too, and the old sentence counted them as asserted.

Measured rather than reasoned, by flipping `before` ⇄ `overlapping` in
`golden.json` and reading the ratchet, restoring the file from a snapshot and
verifying its md5 (`5773748d22d521e6a65afc5e2e2663d1`) after every run:

| flipped | pairs | entries | the ratchet |
|---|---|---|---|
| every **nested** pair — W3a's own experiment | 227 | 18 | **FAILS**: 150 mismatching, **6 REGRESSIONs** |
| every pair on a **baselined** entry | 750 | 91 | green: 144 mismatching, 0 banked |
| every pair on a **matching** entry | 354 | 40 | **FAILS**: 184 mismatching, **40 REGRESSIONs** |

The first row is the one to read against W3a's, which recorded that flipping all
227 left this crate **green**. The second is the honest half: three quarters of
the kept pairs still cannot fail, and the way to make one of them fail is to bank
its entry.

Two smaller facts about the population, unchanged and still derived: **13 pairs
were dropped** by the harvest (`valids/dag` 1, `valids/if` 9,
`valids/parallel-nested` 3), all of them because the relation flipped between
rounds; and the restriction removes **87** pairs across 7 entries from egret's
side for having no counterpart in the golden — 15 under W3a's harness, over
top-level pairs only. egret's relation domain is measured at
`{before, overlapping}` over all **778** pairs it produces, sub-executions
included, so dropping a pair is exactly accepting either value Kestra was seen
to produce.

### The terminal states this oracle feeds: **630 → 0**

W3a's harness was **circular for one field**: `golden_outcome` handed egret's
side the golden's own terminal for every `Runnable` run and every `Loop` run —
**630** of them, 575 + 55 — so for that field the oracle compared the golden with
itself.

**It feeds none.** `oracle-exec/driver.rs` takes a flow source and the posted
inputs, starts a real execution with `worker::start_execution`, drives it with
`worker::tick` and reads the trace back out of Postgres; nothing on egret's side
reads `golden.json`. The golden still supplies the **population** — which flows
are driven, by id — and `corpus.json` supplies the input values `harvest.py`
posted, and the driver still supplies the passage of time and a `Sleep` that
validates its duration without waiting. **No state comes from any of them.**

*Demonstrated, and the demonstration has to be on an entry that matches.*
Rewriting one run's last state and its last attempt state from `SUCCESS` to
`KILLED` in `golden.json`:

| corrupted | the ratchet |
|---|---|
| `valids/sequential`, a top-level `Runnable` run — an entry that **matches** | **FAILS**: 145 mismatching, `valids/sequential` named a REGRESSION |
| `valids/loop-serial`, a run **inside a sub-execution** — also matching | **FAILS**: 145 mismatching, `valids/loop-serial` named |
| `valids/purge_logs_no_arguments`, whose task type has **no handler** | green, 144 mismatching — **the control** |

The third row is the point. **W3a's first attempt at exactly this demonstration
was vacuous**: it corrupted `valids/minimal2`, whose golden carried a `SUBMITTED`
that `RunState` could not represent, so that entry could not match under *any*
value and the experiment could not have reported anything. An entry that already
mismatches gives the same green answer whether the field is fed or not.
`a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to` is the
committed version, and it *derives* its control ("a golden whose first run's
history holds a state `RunState` cannot hold") instead of naming one — W3c added
`SUBMITTED` to the enum, so the entry W3a picked by name matches today and would
have made the control vacuous in its turn.

### The sub-executions

```text
  41   golden entries carry a sub-execution
 142   sub-executions they carry between them

  16   of the 41 now MATCH
   6   agree on everything the parent execution holds and differ INSIDE `spawned`
  19   differ on something else as well
  ---
  41   TOTAL
```

Asserted, total beside the parts, by
`the_sub_executions_are_compared_and_this_is_what_they_cost`. Beside it:
**35 entries produce a `spawned` at all**, where every entry produced an empty
one for the whole of W3a and W3b Tasks 1–4, and egret produces **126** ordered
pairs inside `spawned` where it produced 0.

The six that differ only inside a sub-execution are
`invalids/loop-switch-failed`, `valids/retry-flowable-parallel` and the four
`valids/retry-new-execution-*`. **This is a different sentence from the one W3a's
`39 of the 41 differ on nothing but spawned` made**: that field was empty, and
these six are filled and wrong somewhere inside.

### The residual baseline, by construct and owner

144 entries. `the_residual_baseline_is_attributed_to_a_construct_or_named`
derives this from the baselined flows' own YAML on every run and fails if a row
moves; **the rows overlap and this is not a partition**, but the remainder is
exhaustive and named.

| baselined entries declaring it | construct | owner |
|---|---|---|
| 21 | `Loop` — a sub-execution egret **runs and gets wrong** | see below |
| 22 | `Subflow` | **unclaimed** |
| 16 | `finally:` | **unclaimed** |
| 16 | `ExecutionUpdatableTask` (`Labels`, `SetVariables`, `UnsetVariables`, `Exit`) | **unclaimed** |
| 12 | `AllowFailure` / `allowFailure:` → `WARNING` | **unclaimed** |
| 12 | `Pause` → `PAUSED` | W8 |
| 10 | `afterExecution:` | **unclaimed** |
| 9 | `LoopUntil` / `WaitFor` | W8 |
| 7 | flow `concurrency:` → `CANCELLED` | W8 |
| 5 | `sla:` | W8 |
| 4 | `CREATE_NEW_EXECUTION`'s replay | W6 |
| 4 | `WorkingDirectory` (planned opaque) | **unclaimed** |
| **29** | **none of the above** | named individually in `UNATTRIBUTED` |

**The `Loop` row was 37 and is 21, and the label moved with the number.** It used
to mean "a sub-execution egret asks for and cannot run"; W3b executes them, so
what is left is a sub-execution that runs and diverges. No single package owns
the residue and four separate causes are named for parts of it — W1's
`models::Loop` carrying neither `errors:` nor `finally:` (W3b Task 4), W2's
missing `is json` test (W3b Task 5), `advance()`'s unimplemented third `values:`
shape (an ION file URI), and the loop `outputs:` family. Nobody has been asked to
account for the row entry by entry, and this table should not be read as though
somebody had.

**`disabled: true` and `runIf:` are not rows here and both are implemented.** A
row is an attribution to the thing that *holds* an entry, not to a construct the
flow happens to declare — the same rule that adds a row when a cause is found,
run backwards.

**The named remainder went from 13 to 29**, and 16 of the 18 new names are W3b
Task 5's: eleven entries whose task type has no handler at all (`OutputValues`
×6, `PurgeLogs` ×5, `Assert` ×1 — measured in advance by W3b Task 1) and five
more a fed terminal had been hiding, four of which are W2 divergences that only a
real handler rendering a real property can reach (`errorLogs()`, `render()`,
`??` over an undefined *root*, and integer division by zero rendering
`Infinity`), the fifth being a sibling still in flight when its `Parallel` failed
— nothing calls `TaskHandler::cancel`, and W4 owns the kill path.

`valids/full`'s entry in that list is a correction: it was attributed to
flow-level `outputs:` and `variables:`, and the flow has **neither** (W3b Task 1
Concern 3). It differs on the happens-before relation alone, the same cause as
`valids/dag`, `valids/if` and `valids/parallel-disabled-tasks`.

### What this ratchet does **not** prove

Each of these has a section of its own above with the derivation; they are
gathered here so the boundary can be read in one place.

**1. The corpus is Kestra's own test suite, which is a sample of an infinite
space.** 345 flows is what one project chose to write tests for. Nothing here
says anything about the flow a user writes tomorrow. The ratchet is a regression
guard, not a bound on behaviour: it fails when a *comparable* entry moves, and a
flow nobody wrote is comparable to nothing.

**2. Three quarters of the kept ordered pairs cannot fail.** 750 of 1104, shown
above by flipping every one of them and watching the ratchet stay green. A pair
becomes assertable when its entry banks, not before.

**3. `egret.sql` reaches zero corpus entries and zero golden entries**, and a
green ratchet says **nothing whatever** about it.

| population | flows naming an `egret.*` task type | flows naming `core.log.Log` (the control) |
|---|---|---|
| the whole corpus — 345 flows | **0** | 108 |
| the golden — 294 entries | **0** | 97 |

Kestra has no `egret.*` type, so this is not a gap more harvesting could close:
**no Kestra corpus can ever exercise `worker::handlers::sql`.** The `Log` column
is why the zero means anything — a scanner that parsed nothing would report zero
`egret.*` flows and look exactly like the truth, and it cannot also report 108
flows naming a type it failed to parse. *The control caught its own first
version*: a scanner requiring `type:` to begin its line gave **106**, missing the
two flows that write `- type: …` as a list item.

What the handler rests on instead is `tests/sql_starrocks_live.rs` against a real
StarRocks and `worker::handlers::sql`'s own unit tests — **and that suite
self-skips without `EGRET_TEST_STARROCKS_URL`**, so a run without an engine
leaves the handler entirely unmeasured while reporting green. 7 of the 26 tests
W3b Task 6 added do not run by default. See *W3b Task 6 — `egret.sql`, and the
coverage this ratchet does NOT give it*.

**4. Durability is proved by W3b Task 2's properties, not by this corpus.**
`tests/worker.rs` owns all four: two concurrent `tick`s never claim the same run;
a process killed between the handler and the commit leaves nothing half-applied
(and the run comes back only *through* `reap`, which is what makes the lease a
live path rather than an unreachable one); `reap` reclaims exactly what
`working_timeout_s` says, asserted in **both** directions; and **a run whose
result committed is never claimed again**. The fourth is invisible to any corpus
by construction: a re-run that reaches the same terminal state produces an
**identical trace**, so the only place a duplicate execution is visible is the
handler's call log. A green ratchet is compatible with running every task twice.

**5. One run per tick is a ruling, not an accident.** Four entries —
`valids/dag`, `valids/if`, `valids/parallel-disabled-tasks`, `valids/full` —
reproduce every run, attempt and execution state Kestra recorded and differ on
the **happens-before relation alone**. This driver finishes one task run per tick
and creates that run's successors in the same transaction, where Kestra's worker
pool finished two siblings before either successor existed. Closing it means
applying several results before calling `advance()`, which changes the
transaction boundary W3b Task 2 built the durability properties on. **Four
ordered-pair entries is the wrong price for the durability guarantee**; W4
introduces a real worker pool and owns the decision. This is also the single
largest lever left on the ratchet.

**6. 84 of the 294 executions end unfinished**, counted and named by
`every_mismatching_entry_is_attributed_to_the_field_it_differs_on`. A task type
with no handler makes `Registry::run` refuse, the tick abort for that run and the
execution stall — which is what the shipping worker does, so the driver returns
the partial trace and only the step bound or a panic is `Broken`. Nothing here
exercises what `reap` would then do with the leased run in production.

**7. Nothing keyed on elapsed time is reachable**, because `driver::pass_time`
moves the clock only to the next `not_before` something is already waiting for.
The cost is one named entry: `valids/retry-failed-flow-duration` is bounded by
`maxDuration: PT7S` with no `maxAttempts`, Kestra ran three attempts and egret
runs four, because an attempt that costs no elapsed time leaves room for another
inside the same window.

**8. No banked entry takes an `else` branch**, so the ratchet cannot see a
predicate that is wrong in the truthy direction; three mutants prove it rather
than argue it, and all three leave `executor_matches_golden` passing.

**9. The ratchet itself is not deterministic, and a green run is a sample.**
Found by W3b Task 7 and **reproduced on the package's base commit**, so it is not
this task's and not a regression: with the machine under load,
`executor_matches_golden` fails roughly one run in eight, and it moves in
**both** directions.

| observed | entry | what happened |
|---|---|---|
| 145 mismatching, 1 REGRESSION | `valids/parallel-nested` | five ordered pairs against run 9 (`1-3-2-2_end`) come out `overlapping` where the golden — and every other run — says `before` |
| 143 mismatching, 1 PROGRESS NOT BANKED | `valids/parallel-fail-with-flowable` | a baselined entry **matched**: the `Sleep` that is normally still in flight when its sibling fails the `Parallel` had already finished |

Both are timing, and the mechanism is on egret's side: `worker::claim` orders by
`created_at` **with no tiebreak**, and sibling runs of one flowable are inserted
in a single transaction and therefore share it, so which of several runnable
siblings a tick takes first is arbitrary. W3b Task 2 recorded that
(*"benign — `SKIP LOCKED` already makes it arbitrary"*) and it is not benign
here: the round in which each run terminates is what the logical clock records,
so an arbitrary claim order is an arbitrary happens-before relation.

**Nothing about this was visible for six tasks**, because the gate was run once
per task on an idle machine and passed. It is stated here rather than fixed,
because the fix is a tiebreak in the claim's `ORDER BY` — production code, on the
hot path of every tick, which would move the corpus and therefore the 16 banked
and 16 lost this package accounts for. It belongs to whoever owns the next change
to the worker, with a test that fails without the tiebreak. Until then: **a green
ratchet is one draw, and two of the 294 entries are known to be a coin toss.**

**10. `advance()` is unchanged by the whole of W3b** — `git diff --stat --
crates/egret-workflow/src/executor/` is empty against the package's base, checked
with `if [ -z "$(…)" ]` rather than a bare `&& echo CLEAN`. So every W3a
statement about the state machine still stands as W3a made it; what changed is
the driver in front of it.

### The stale twins this statement retires

W3a's README ended up carrying **two true-at-different-moments figures for one
quantity, with only surrounding prose to say which** — `38 of the 41` in the
header's Task 7 note and `39` in the body's table, both correct on the day they
were written. W3b Task 7 swept for that shape. What it found, and what a reader
must take from here instead:

| where | what it says | why it is not current |
|---|---|---|
| header, Task 6 note | *"41 entries, 142 sub-executions, **33** of the 41 differing on nothing else"* | the field was empty then. **16 match, 6 differ only inside it, 19 on more** |
| header, Task 7 note | *"the sub-execution hole is **38** of 41, not 33"* | same quantity, a third moment |
| *What Task 6 had to change*, the 41 table | *"33 at Task 6, 38 at Task 7, **39** at the final review"* | same quantity, a fourth moment |
| *What this oracle does not prove*, ordered-pair bullet | *"the kept 1104 … are the whole of what this oracle asserts about serialisation"* | **354** are; already corrected once, by 227, and still too wide by 750 |
| same bullet | *"only **877** … are ever asserted"* | 877 is what the comparison *reads*, not what it can report a change to |
| same section, sub-execution bullet | *"`Command::SpawnExecution` is emitted; **nothing executes one until W3b/W6**"* | `worker::spawn` executes it and this oracle drives it. W3b Task 4 annotated both occurrences and deliberately left them for this sweep |
| header, W3b Task 1 note | *"11 of the 150 … agree only because the harness feeds a terminal"* | the feed is gone; all 11 were lost, with five more the feed was hiding |
| `runnable.txt`, W3a Task 6 block | *"**0** sub-executions whose TRACE is compared"* | 35 entries produce one; 126 nested pairs |

#### Pointers that no longer resolve — thirteen, and this is the worse shape

A stale *number* can be checked by reading the sentence beside it. **A pointer
that resolves to nothing cannot be checked at all**, and this file gives
thirteen. Every sentence of the form *"asserted by X"* below whose X is in the
left column is asserting nothing a reader can go and read.

| cited here | what it is now |
|---|---|
| `only_the_top_level_order_pairs_are_ever_asserted` | renamed at W3b Task 5 → `the_ordered_pairs_inside_spawned_are_reachable_now` |
| `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` | renamed at W3b Task 5 → `the_sub_executions_are_compared_and_this_is_what_they_cost` |
| `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` | → `every_sub_execution_egret_runs_is_one_kestra_ran`. A prefix was all one batch could offer; the driver runs them all, so it is a whole-set comparison |
| `a_null_task_list_is_an_over_refusal_that_reaches_the_http_api` | renamed on `main` by `05083cbc`, **before this branch** → `a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own` |
| `every_flow_kestra_published_plans_except_three_named_over_refusals` | same commit → `…_except_two_named_over_refusals` |
| `a_skipped_run_still_gets_a_happens_before_endpoint` | **deleted** with W3a's harness at W3b Task 5 |
| `an_order_pair_is_before_only_if_the_second_run_was_created_after_the_first_ended` | deleted with the harness |
| `restricting_the_order_cannot_hide_a_disagreement_on_a_kept_pair` | deleted with the harness |
| `the_caller_renders_and_a_failed_render_fails_the_run` | deleted with the harness's render tier; rendering lives in `worker::render` and has its own tests |
| `the_caller_can_render_every_expression_except_the_ones_named` | the same; the most-cited of the thirteen |
| `the_caller_can_render_every_branch_expression_except_the_two_kestra_could_not` | the same, in `runnable.txt` |
| `the_feed_absorbs_a_measured_share_of_the_golden_run_terminals` | deleted; the feed is **0** |
| `the_loop_completion_tier_is_load_bearing_for_exactly_these_entries` | deleted; that tier is `worker::spawn` |

**The citations themselves are left where they are, and that is a decision, not
an oversight.** Nine of the thirteen are cited inside a *W3a task's own section*,
describing what that task asserted on the day it ran; renaming them there would
rewrite a record rather than correct one, and this file has already paid twice
for having two versions of one sentence. The table above is the single lookup.
What is **not** left to prose is the table's own accuracy:
`the_readme_pointers_that_no_longer_resolve_are_recorded_and_their_successors_do`
fails if a retired name comes back as a live test, if a successor named here does
not exist, or if a row survives the last citation it explains — and it carries a
positive control, because every one of its assertions is a *"does not contain"*
and an empty source string satisfies all of them.

Two more, of a different shape, found by the same sweep and **fixed** rather than
annotated:

- The ratchet's own failure message named
  `the_order_pairs_the_golden_dropped_are_the_only_ones_not_compared` as the test
  deriving its population. **No test by that name exists** — it is
  `the_golden_has_no_relation_for_exactly_these_pairs`. That one is fixed in
  place because it is printed by a *current* test into a *current* failure
  message; it is not a record of anything.
- `python3 harvest.py --rewrite-runnable` — the command this file documents —
  **deleted the whole hand-written half of `runnable.txt`**, 319 lines, silently
  and with exit 0. Measured, not suspected: a copy of the script and the
  committed inputs run in a scratch directory took the file from 1074 lines to
  755. That is the file this README tells a reader to believe *over itself*.
  `write_runnable` now emits a sentinel and carries everything after it across a
  rewrite, reads the result back and fails if the tail shrank.

Left in place deliberately, and named so it is not rediscovered: the per-task
notes in the header and in `runnable.txt` are **records of what a task measured
on the day it ran**, and rewriting a record makes two chances to disagree instead
of one. They are corrected *here*, by name, in the table above.

A limit stated plainly is worth more than a number rounded up.

## The null list Kestra accepts and egret refused

W3a found this, measured it, and deliberately did not fix it: the fix is W1's
model, and W3a's own tests are what pinned the refusal. This section is the fix,
written after it landed.

### The defect

`models/task.rs` declared its task lists as bare `Vec<Task>` under
`#[serde(default)]`. That covers an **absent** key and nothing else: an explicit
`tasks: null` is `invalid type: null, expected a sequence`, a hard parse error.
Jackson does not error on a `null` for a `List` — it leaves the field null — so
the same YAML that Kestra reads, egret refused.

It is not an oracle curiosity. `spec::parse` is called from `routes.rs:302`
(`draft_dto`), `routes.rs:335` (`parse_draft`, used by `create_flow` and
`update_flow`) and `routes.rs:640` (`validate_flow`), so `POST /flows`,
`PUT /flows/{id}` and `POST /flows/{id}/validate` answered **400 InvalidInput**
for `valids/switch-impossible` — a flow this pinned Kestra
(`kestra/kestra:v2.0.0-rc13`) publishes with 200 and **runs**. Over-refusal is
the mirror of the divergence class this programme forbids shipping.

### Kestra's own 422 is the evidence for what the fix should do

The strongest evidence is not in Kestra's source but in this corpus's
`reason` field for `invalids/invalid-parallel`, which lists:

```text
tasks[para2].tasks: The 'tasks' property cannot be empty
tasks[para4].tasks: The 'tasks' property cannot be empty
tasks[para3].tasks: The 'tasks' property cannot be empty
```

`para2` has no `tasks` key, `para3` has `tasks: []`, and `para4` has
`tasks: null`. Kestra **parsed all three** — Jackson did not error on the null —
and then refused all three at bean validation, for being *empty*. Two stages,
and the null belongs to the second. That is exactly the split
`bean-validation-is-not-parsing` names, the principle `models/task.rs` already
applied to `Parallel::concurrent`; it was simply never applied to a list.

### The fix, and why it is wider than the three flows that showed it

`wire::null_is_absent` — `Option::<T>::deserialize(de)?.unwrap_or_default()` —
under `#[serde(default, deserialize_with = …)]`, on **all 17** list fields in
`models/task.rs`: every `tasks`, `errors`, `then`, `else`, `cases`, `defaults`
and `dependsOn` on `Sequential`, `Parallel`, `If`, `Switch`, `Dag`, `DagNode`,
`Loop`, `LoopUntil`, `AllowFailure` and `WorkingDirectory`.

The measured defect touched three of them. Fixing three and leaving fourteen
would have made `tasks: null` parse while `errors: null` did not, on the same
struct — a line no reader could derive and no rule explains. The
`egret_refuses_every_flow_kestra_refuses` sweep is what makes the wider fix
checkable rather than merely tidy: it asserts every flow Kestra refuses is still
refused, **for its own code**, so an over-acceptance introduced by the widening
would fail by name. None was.

### What broke, and how each break was recorded

**Eight** guards moved. Seven fired on the model change itself; the eighth,
`the_residual_baseline_is_attributed_to_a_construct_or_named`, fired on the
banking that followed it. Every one of them was a place W3a recorded the defect,
and each now records its absence:

| guard | what it said | how it is recorded now |
|---|---|---|
| `a_null_task_list_is_an_over_refusal_that_reaches_the_http_api` | "if this now plans, W1's model has been fixed — delete this test" | **inverted, not deleted.** It rehearsed the fix by deleting the token from each of three flows; it now asserts the same three outcomes with the token in place, and adds the equality `null` ≡ absent. Renamed `a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own` |
| `every_flow_kestra_published_plans_except_three_named_over_refusals` | panicked naming `valids/switch-impossible` and its cause | the row is deleted, the table is **two**, and the test is renamed `…_two_…`. Its inverted design is what forced the deletion rather than allowing a quiet edit |
| `egret_refuses_every_flow_kestra_refuses` | the two accidental refusals were one `RefusalGroup` whose cause was "an explicit `null`" — not a cause at all | that group is gone; `invalids/switch-invalid` and `invalids/invalid-parallel` are two groups, each asserting the code Kestra's own 422 names (`switch_no_branches`, `invalid_namespace`). 16 causes → 17 |
| `executor_matches_golden` | `PROGRESS NOT BANKED: valids/switch-impossible` | banked. `baseline.txt` 145 → **144** |
| `the_residual_baseline_is_attributed_to_a_construct_or_named` | the entry was in `UNATTRIBUTED` as "an OVER-REFUSAL" | removed. It was in that list precisely because no missing construct explained it |
| `the_feed_absorbs_a_measured_share_of_the_golden_run_terminals` | `(575, 55, 108, 102, 79)` | `(575, 55, 109, 101, 79)`, and the README table with it |
| `planning_the_loop_removed_forty_spurious_submitted_emissions` | `(424, 73, 343, 114)` | `(424, 74, 342, 114)` |
| `the_caller_can_render_every_expression_except_the_ones_named` | 26 reachable branch expressions | **27**. The `Switch` value `{{inputs.string}}` was always renderable; the parse refusal is what kept it out of the population |

The last three are the same single run and the same single expression, counted
by three different derivations. An over-refusal does not only return a 400: it
**shrinks every population an oracle measures**, silently, in the direction that
looks like less work to do.

### The ratchet

**145 → 144.** One banked, zero regressions, zero broken, zero over-acceptances.
`valids/switch-impossible` reproduces Kestra's trace exactly — one `impossible`
run, `FAILED` — the moment it is allowed to parse. Nothing in the executor
changed.

W3a predicted the fix would cost no over-acceptance, having rehearsed it by
deleting the token from each flow. Measured against the real fix on all 17
fields, the prediction holds.

### Mutation

Reverting **one attribute** — `Switch::defaults` back to a bare
`#[serde(default)]` — fails seven tests, the same seven the model change moved:

```text
a_null_task_list_parses_and_the_two_accidental_refusals_stand_on_their_own:
  a flow this Kestra published with 200 and RAN must plan: PlanError { message:
  "cannot parse flow at line 9 column 3: tasks: invalid type: null, expected a
  sequence at line 9 column 3" }

every_flow_kestra_published_plans_except_two_named_over_refusals:
  OVER-REFUSAL: 1 flow(s) this Kestra published with 200 are refused by
  `build_plan` and are not in PUBLISHED_BUT_REFUSED …
    valids/switch-impossible: cannot parse flow at line 9 column 3 …

executor_matches_golden:
  valids/switch-impossible: egret REFUSED a flow Kestra ran …
```

The second is the one worth reading twice. The exception table shrank from three
rows to two, and the sweep is what makes that shrinkage *mandatory*: with the row
deleted and the rule restored, the flow is not "a known exception" — it is an
unlisted over-refusal, reported by name with its message.
