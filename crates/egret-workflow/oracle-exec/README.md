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
>   different moment. It is now **22 of the 41 MATCH, 4 differ only *inside*
>   `spawned`, 15 differ on more** — a different sentence, because the field is
>   filled rather than empty. *(This bullet read 16/6/19 until W6 Task 8: the
>   note that exists to stop a reader picking up a stale figure was itself two
>   packages stale, in the file's own reading instructions. All three numbers
>   are constants the suite asserts.)*;
> - **`Command::SpawnExecution` has an executor** (`worker::spawn`) and this
>   oracle drives it. Two places still say "nothing executes one until W3b/W6";
> - **nothing is fed from `golden.json` any more** — **426** run terminals were
>   (386 top-level `Runnable` + 40 top-level `Loop`; the 630 this note used to
>   give counts 204 that were inside sub-executions egret never created and were
>   therefore never fed), and the circular field is gone;
> - **"the kept 1104 are the whole of what this oracle asserts about
>   serialisation"** is wrong for the second time. **399** are; the other **705**
>   sit on baselined entries and cannot fail, which was as true under W3a as it
>   is now — and the same is true of every other golden field on a baselined
>   entry, not only of the pairs. *(357/747 until W6 Task 8, 360/744 until W8;
>   the split moves with `baseline.txt`, so every bank moves it — which is why
>   this bullet has now been stale twice and why W8 Task 7 swept all five copies
>   of it rather than the one it was handed.)*;
> - **`egret.sql` ships with no oracle coverage at all** — 0 of 345 corpus flows
>   and 0 of 294 golden entries — and its own suite **fails** without an engine
>   now; it used to self-skip and report `7 passed; 0 failed; 0 ignored` in
>   0.00s, indistinguishable from the run with an engine except by the clock.
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
*States and constructs no W3a task claims*, which named five things — `Pause`,
flow concurrency, `disabled:`, `ExecutionUpdatableTask` and **`Subflow`** — as
stranding baselined entries with no owner. Three of the five have since been
corrected out of that reading: `disabled:` and `Subflow` are implemented, and
**flow concurrency stranded nothing** — its state was misattributed and W8 Task
1 deleted the row. See *What Task 3 had to change, and what
it banked*.

Every number **in this harvest section** is a number `harvest.py` prints. If this
file and `runnable.txt` ever disagree, `runnable.txt` is right and this file is
stale — it has been wrong that way once already.

*(Scoped by the whole-branch review's Important 3 sweep. It used to say "every
number **below**", written when this section was the end of the file; six
sections have been appended under it since, and almost none of their numbers
comes from `harvest.py`. Same defect as the provenance rule in the coverage
statement, found by asking formulation 3 of the RULES rather than of the
figures: a rule that says "below" ages every time somebody appends.)*

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

> **W8 Task 3 fix round 2 RE-MEASURED this and the paragraph above is FALSE.**
> The claim was built by varying the input VALUE of one fixed expression and
> never the expression FORM, so it generalised over exactly the class it had
> not sampled. Five runs per form against the same pinned digest:
>
> | `behavior:` | value | result |
> |---|---|---|
> | `FAIL` (literal) | — | `FAILED`, 1 task run, 5/5 |
> | `"{{ 'FAIL' }}"` | — | `FAILED`, 1 task run, 5/5 |
> | `"{{inputs.behavior}}"`, `FAIL` posted | `FAIL` | **`FAILED`, 1 task run, 5/5** |
> | `RESUME` (literal) | — | `SUCCESS`, 2 task runs |
> | `"{{ 'RESUME' }}"` | — | `SUCCESS`, 2 task runs |
> | `"{{ vars.b }}"` | `RESUME` | `SUCCESS`, 2 task runs |
> | `"{{inputs.behavior}}"`, `RESUME` posted | `RESUME` | `SUCCESS`, 2 task runs |
>
> **The artifact honours the VALUE, in every form.** There is no
> dynamic-property drift here at all, and `valids/pause-behavior`'s golden —
> `FAILED`, one task run — is simply `behavior: FAIL` working, because `FAIL`
> is what `corpus.json` posts for it.
>
> **`probe.json`'s row for this flow is falsified with it.** It records that all
> four documented values produce the identical `FAILED`/1 execution and calls
> the row "proven NOT falsifiable"; measured, only `FAIL` does. That row's
> `differs: false` is therefore an artefact of the same unsampled axis. It is
> left in place as data — rewriting a harvested artefact from a later probe
> would destroy the record — and contradicted here, where the claim was made.
>
> **One anomaly, reported rather than smoothed.** In one window the exact
> corpus flow with `FAIL` posted returned `SUCCESS`/2 seven consecutive times,
> then `FAILED`/1 five consecutive times in another. The VALUE-honouring
> conclusion above is drawn from the 5/5 windows and from every `RESUME` form;
> the non-determinism itself is unexplained and is **not** the behaviour egret
> reproduces.

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
proven discriminating 18   proven non-discriminating 0   unprobed 19
```

*(This block is a verbatim copy of `runnable.txt`'s own generated line. It read
`17 … 1` until W8 Task 3 fix round 3 corrected `probe.json`; the generator emits
`18 … 0` now, and `--rewrite-runnable` reproduces it.)*

- **18** rows are proven falsifiable by measurement.
- **0** are proven *not* falsifiable. **This bullet read `1` and named
  `valids/pause-behavior` until W8 Task 3 fix round 3, and every sentence under
  it was false.** It is corrected here AND in `probe.json`, because
  `runnable.txt` is a pure function of `corpus.json` + `probe.json` through
  `--rewrite-runnable`: prose beside a falsehood does not stop a regeneration
  re-emitting it, and this one was re-emitted at `runnable.txt:722-723` for
  three packages.

  The claim was that all four documented behaviours — `FAIL`, `RESUME`, `WARN`,
  `CANCEL` — produce the identical `FAILED`/1 execution, diagnosed as a Pebble
  expression failing to render into `Property<Behavior>` at resume.
  **Re-measured on the same pinned digest with a FRESH, never-reused flow id for
  every single execution, 3 runs each**, the expression form gives four
  *different* executions:

  | posted | state | task runs |
  |---|---|---|
  | `FAIL` | `FAILED` | 1 |
  | `RESUME` | `SUCCESS` | 2 |
  | `WARN` | `WARNING` | 2 |
  | `CANCEL` | `CANCELLED` | 1 |

  That is exactly what `PauseTest.java:374-380` asserts, so there is **no drift
  at this seam at all** and `EXECUTION_DRIFT` is now empty.

  **The contaminant was FLOW-ID REUSE, and the old control could not exclude
  it.** Publishing a different source under the same `(namespace, flowId)`
  leaves `execution.inputs` still reading the posted value — so *"the execution
  document records `inputs: {"behavior": "FAIL"}`, the input landed"* establishes
  that the INPUT arrived and says nothing about **which source ran**. Variant
  probing does precisely that reuse.

  **The harvest is structurally immune, which is why the golden is
  reproducible.** All **275** corpus flows that execute have a unique
  `(namespace, flow_id)` — the three duplicated ids are all 422-rejected and
  never run — and `stability.json`'s 5 rounds name **2** unstable traces,
  `valids/retry-failed-task-duration` and `valids/waitfor-max-duration`, neither
  a `Pause`. `valids/pause-behavior`'s golden is `behavior: FAIL` working.

  **One thing the grid could not see, recorded rather than generalised over.**
  Every cell above renders to a *valid* enum constant. Outside the enum the
  artifact does not terminate: `NOPE`, lower-case `fail` and an undefined
  `{{inputs.nosuch}}` each leave the execution **`RUNNING` past 120 s**. No
  corpus flow writes one, so nothing here can score it — it is named so that
  "the artifact honours the value" is never read wider than the enum.
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

A sub-execution is fetchable by id but does **not** appear in the search
listing. A harvester that enumerates executions silently loses every loop
iteration and every subflow.

**Two sentences here were false and are corrected by W6** — the design spec's §7
items 1 and 2, both of which this section is the origin of.

*"A sub-execution … carries `parentId`"* is true of a **loop iteration** and of a
`CREATE_NEW_EXECUTION` **replay**, and false of a **subflow child**, whose
`parentId` is NULL and whose link is `trigger.variables.executionId`. Measured on
the pinned artifact and recorded in full in *W6 Task 2 — three kinds of child
execution*: `valids/subflow-parent`, a flow whose only task is a `Subflow` Kestra
ran to SUCCESS, has `"descendants": []` in all five harvest rounds. The child
exists; nothing keyed on `parentId` can see it. **That is a hole in this
oracle's population, not merely in this sentence**, and it is why W6 links a
subflow child by a column of its own.

`descendants()` **was** documented — in its own docstring in `harvest.py`, the
only place it is documented — as unioning **two independent discoveries**. It
has **one**, and since W6 Task 8 the docstring and the call site say so too, so a
reader who never opens this file is no longer told there are two:

1. ~~the parent's log lines, which carry `in sub-execution [[link execution="…"]]`~~
   — **dead**: the regex matches neither the raw JSON body (escaped quotes) nor
   the decoded message, whose markup carries `flowId=` and `namespace=` before
   the closing `]]`. W6 Task 4 saw the live shape again while probing a `Loop`:
   `in sub-execution [[link execution="5Z4dkYmmBkChEpPYNR1HLM" flowId="replay-in-loop" namespace="egret.probe4"]]`.
   W6 Task 8 re-measured it by **running the regex** over both shapes with a
   **positive control on the shape it was written for** — 0 matches, 0 matches,
   1 match — because an arm that matched nothing on every input would look
   identical to a dead one;
2. a recursive walk of Kestra's Postgres over the generated `parent_id` column,
   which is the storage layer rather than the API — **the only arm that has ever
   found anything**, and the one all **142** recovered sub-executions came from.

The SQL goes over psql's **stdin**, not `-c`, and the id is a psql variable
(`:'root'`): psql does not interpolate variables into a `-c` string, so the
parameterisation would silently not happen. A non-zero psql exit is fatal — and
with the log-scrape arm dead, a swallowed error would degrade sub-execution
recovery to **nothing at all**, silently, and the harvest would still write a
plausible `corpus.json`.

**The dead arm is left in the code, with an owner, and that is a decision.** It
is dead against *this artifact's log markup* — a measurement about the artifact,
not about the code — and deleting it changes what a re-harvest requests, which
is a change W6 cannot run end to end because W6 deliberately does not
re-harvest. It belongs to the package that makes triggers measurable, which has
to open `descendants()` anyway to union `trigger_execution_id`; see the W6
section of *The coverage statement*. **And the dead arm is not the hole**:
`parent_id` is NULL for a subflow child and for anything a trigger creates, so
no arm keyed on it could see those however well its regex matched.

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
575 *fed* and **15** of the 55 *spawner* runs live inside a sub-execution egret
never creates, and for them nothing is fed and nothing is compared. This table
is a classification of Kestra's runs by egret's plan, not a claim about
coverage. That was always true of it; Task 6 is the first task large enough to
make it matter, and the pair `(189, 15)` is asserted so the descriptions stay
true of every run they cover. *(The sentence introducing that pair read `189 of
the 565 fed`; 565 is what Task 6 left the bucket at and the table 23 lines above
says **575**, which later tasks moved it to. Corrected — and this is exactly the
shape *The stale twins* is about, one paragraph away from the number that
contradicts it.)*

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
of both, and that union is **13**, not 12: `RESTARTED` appears **12** times in
`golden.json` execution histories and **never** in a run history. *(This said 60
until the whole-branch review. 60 is `traces.json`'s count — the raw five-round
harvest, where every entry appears five times — and `golden.json` is what this
section is about. A number from the right file's neighbour reads exactly like a
number from the right file.)* Going the other way,
`SKIPPED` and `SUBMITTED` appear only in run histories.

`executor::RunState` had six members when this was written, so 7 of the 13 were
unrepresentable. **Task 3 added `SUBMITTED`, Task 7 added `RETRIED`, W3c added
`SKIPPED`, W8 Task 2 added `PAUSED` and `RESTARTED`, W8 Task 5 added `WARNING`
and W8 Task 6 added `CANCELLED`**, so the enum has **thirteen and the two
vocabularies are the same set** — `the_state_vocabulary_this_enum_holds_is_derived_from_the_golden`
asserts that equality, beside the now-empty *still missing* map, because an
empty map is also what a broken walk returns. Each was recorded here rather than
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

**What each addition bought, and what the two still cost.** Of the 266 `runs`
entries, **35** use only the six members the plan drafted, **229** use only the
seven, **233** use only the eight, **237** use only the nine and **253** use
only the eleven — so `SUBMITTED`
alone is what moved the reachable set by 194 entries, `RETRIED` moves it by 4,
`SKIPPED` by 4 and W8 Task 2's pair by 16,
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

**W8 Task 2's 16 buy no bank either, and that is the third measured instance of
"inside the vocabulary is necessary and not sufficient".** `PAUSED` (18 entries)
and `RESTARTED` (12) were added together because `Pause` and `LoopUntil` need
the **same** variant and splitting them across packages would block each on the
other's state. Nothing in the crate emits either one: `build_plan` still plans
`Pause`, `LoopUntil` and `WaitFor` opaque, so the ratchet is **unchanged at
127** across the commit that added them. What the two rows record, measured over
`golden.json` rather than inferred:

- `PAUSED` is in **18** of the 408 execution histories and **20** of the 840 run
  histories, and **0** of the 878 attempt histories (all three derived by
  `tests/executor.rs::the_golden_populations_are_derived_recursed_and_unrecursed`,
  which also pins the un-recursed 266/570/604 — **878 was published as 604 for
  one round**, that being the same walk with recursion off) — a paused run's attempt
  reads `["CREATED","RUNNING","SUCCESS"]` while the run itself reads
  `["CREATED","RUNNING","PAUSED","SUCCESS"]`
  (`valids/flow-concurrency-cancel-pause`). The first producer of the variant
  has to emit something other than a plain `SetRunState` or it will push a
  state Kestra never records onto the open attempt.
- `RESTARTED` is in **12** execution histories and **0** run and **0** attempt
  histories. All 12 are `Pause`-and-resume, shape
  `CREATED RUNNING PAUSED RESTARTED RUNNING …`; the other **6** `PAUSED`
  executions carry no `RESTARTED` at all and are exactly `LoopUntil`/`WaitFor`'s,
  which re-enters `RUNNING` directly. **That 12 + 6 is where the 18-entry
  population's split comes from**, derived here rather than taken from the
  design doc.
- Neither is terminal on the artifact. `State.Type.isTerminated()` decompiled
  from `/app/kestra` in the pinned digest lists `FAILED, WARNING, SUCCESS,
  KILLED, CANCELLED, RETRIED, SKIPPED, RESUBMITTED` and neither of these; and
  neither ends any of the 408 execution histories. `RESTARTED` is in
  `isCreated()` beside `CREATED`, and `ExecutorService.handleRestart` maps it
  straight back to `RUNNING`.

The other two, with the number of `runs` entries whose trace uses each and the
task that owns it:

| state | `runs` entries | owner |
|---|---|---|
| `WARNING` | 10 | **`RunState::Warning` exists since W8 Task 5**; the rest of this cell is the attribution that preceded it and is what the split still rests on. **W8's, 8 of the 10** — `AllowFailure` ×3 and the `allowFailure:` flag ×5, derived by entry name in the residual table's `allowFailure` paragraph. The other 2 are unclaimed: `valids/retry-success` (`warningOnRetry`) and `valids/exit` (an `Exit`). This row said **Task 7's** and then said **unclaimed**; neither was right for the 8. It also kept a tail contradicting its own correction, removed in fix round 1: that tail said `valids/task-allow-failure-runnable` needs `warningOnRetry` — **its YAML contains no such key** (it declares `allowFailure: true` and a 3-attempt `retry: {type: constant, interval: PT0.100S, maxAttempts: 3}`) — and it ended with the word "Unclaimed", the owner the cell had just overturned. **Two** of the ten declare `warningOnRetry`: `valids/retry-success` and `valids/allow-failure-with-retry`. (Fix round 1's replacement said "`allowFailure:` and nothing else" and "`retry-success`'s alone", losing the `retry:` and the second declarer — a transcription that lost a member, inside the commit whose headline is a transcription that lost a member. Corrected in fix round 2) |
| `CANCELLED` | 3 | **not flow concurrency, which this row said for four packages.** Two are an `sla:` with `behavior: CANCEL` (`valids/flow-concurrency-sla-assert-cancel`, `valids/sla-execution-condition`) and one is an `Exit` declaring `state: CANCELLED` (`valids/exit-cancelled`). Not one is a limit's — `the_cancelled_state_never_comes_from_a_concurrency_limit` derives it, and its control is that the two corpus flows writing `concurrency: {behavior: CANCEL}` produce no `CANCELLED` at all. W8 via `sla:`; `Exit` is the `ExecutionUpdatableTask` row's. **W8 Task 6 added `RunState::Cancelled` — the last of the golden's thirteen states — and reproduced TWO of the three**; `valids/exit-cancelled` stays baselined on the `Exit`, which is a different construct and still unclaimed |

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

*(Annotation, 2026-10-12, no-silent-stall Task 1: true when written, and no
longer. Since the ruling of 2026-10-11 and Task 1, `validate` reports errors on
52 of the 266 `runs` entries — gapped `Pause`s, `AllowFailure`,
`WorkingDirectory` and unregistered types, each a task egret could not run and
whose execution stalled. All are deliberate and named in
`executor::PUBLISHED_BUT_REFUSED`; see the section "no-silent-stall Task 1" at
the end of this file.)*

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
which entries **match** (**22** of the 41) rather than which are held only by an
empty field. **39** entries produce a sub-execution trace and 126 ordered pairs
inside one. *(Read 16 and 35 until W6 Task 8 — a stale figure inside the note
that retires the table above it, which is the shape W4's sweep named and this
one met again.)* The table is kept because it is what the final review measured; see
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
| ~~flow concurrency~~ | ~~`CANCELLED`, 3 entries~~ — **the state was misattributed and the row is DELETED, not lowered**; see the two paragraphs below this table | flow-level `concurrency:` | **nobody, and deliberately.** It reaches **0** baselined entries and the ratchet cannot score it. W8 Task 1 removed the row under the same rule that removed `disabled:` and `runIf:` |
| `disabled:` | ~~`SKIPPED`, 4 entries~~ — **the state was misattributed and the construct is now implemented**; see the two rows below | a task-level flag | **W3a's final review took it.** `flatten_list` drops a disabled task from the plan, which is Kestra's own rule (`Execution.removeDisabled`, called on `tasks`, `errors` and `finally` inside `findTaskDependingFlowState`): no taskrun at all, and a flowable whose children are all disabled still succeeds. 4 of its 6 baselined entries banked |
| `runIf:` | ~~**6 runs across 4 entries** — the golden's whole `SKIPPED` population — and 7 baselined entries~~ — **implemented by W3c** | a task-level expression, rendered per run | **W3c took it**, and both halves moved in one commit: W1's `Task` carries `run_if` and `RunState` has `Skipped`. **None of the 7 banks and none of the 6 skipped runs is reproduced under this oracle** — not because the rule is wrong but because the harness cannot render any of the four flows' conditions; see *What W3c had to change, and what it did NOT bank* |
| `ExecutionUpdatableTask` | **23** over-submitted runs at Task 5 and at Task 6 (19 at Task 3, across 16 baselined entries — the entry count has not been re-derived since) | `Labels`, `SetVariables`, `UnsetVariables`, `Exit` | **unclaimed** |
| `Subflow` | 22 baselined `runs` entries, **25** over-submitted runs after Task 6 (22 after Task 4, 18 before it) | `io.kestra.plugin.core.flow.Subflow` | **unclaimed — the word does not appear in the plan** |
| ~~`LoopUntil`, and its alias `WaitFor`~~ — **taken by W8 Task 4** | ~~**3** + **6** over-submitted runs~~ — **0**, and the two spellings are one `models::TaskKind` now | `io.kestra.plugin.core.flow.{LoopUntil,WaitFor}` | **W8 Task 4**, one entry banked (`valids/waitfor-nested`). The 9 over-submitted runs are gone: a `plan::TaskKind::LoopUntil` gets no `type_id`, so no worker claims it — and the SEPARATE fact Task 4 closed is that `WaitFor` never reached this tier at all. `models::kind_for` had no row for the alias, so it parsed to `Leaf` and its `tasks:` sat inside an opaque `spec` where `children()` could not see them; **seven of the ten corpus flows of this construct use that spelling.** Both now map to `TaskKind::LoopUntil`, which also closes the one publish egret accepted that the artifact refuses (a `WaitFor` child of a `WorkingDirectory`) |
| ~~`WARNING`~~ — **taken by W8 Task 5** | 10 golden `runs` entries | `AllowFailure`, the `allowFailure:` flag, `retry.warningOnRetry` and an `Exit` | **`RunState::Warning` exists since W8 Task 5 and SEVEN of the row's twelve baselined entries banked with it.** What remains is the `AllowFailure` FLOWABLE (4 entries, still planned opaque) and `valids/waitfor-child-task-warning` (an `OutputValues` child with no handler — measured, not argued). The cell below is the pre-Task-5 attribution and is kept because its derivation is still what the split rests on. It was **W8 for 8 of the 10, unclaimed for 2** — corrected at W8 Task 1, and this cell had said Task 7's and then unclaimed. The 8 are the allow\* axis's (`AllowFailure` ×3, `allowFailure:` ×5) and the programme spec §9 has always given `allowFailure` to W8; the 2 that stay unclaimed are `valids/retry-success` (`warningOnRetry` alone — Task 7 reads it into `RetryPolicy` and does nothing with it) and `valids/exit` (an `Exit`, this table's `ExecutionUpdatableTask` row). See the residual table's `allowFailure` paragraph for the derivation by entry name |
| a `Loop`'s own `errors:` | 4 baselined entries — `valids/all-flowable`, `valids/finally-loop`, `valids/finally-loop-parallel` and `invalids/loop-switch-failed`, which is a `runs` entry despite its directory | `Loop.errors`, resolved by Kestra inside the **sub-execution** (`Loop.java:272`) | **unclaimed — new at Task 7 fix round 1, and found by a guard that could not fail.** `models::Loop` has no `errors` field, so `plan.rs` has no `Loop` arm. It is filed beside `Subflow` and the `CREATE_NEW_EXECUTION` replay rather than as a missing `match` arm: a `Loop`'s `errors:` runs in an execution W3a does not create |
| `finally:` | the whole `finally-*` family — 11 baselined entries — plus the only two corpus flows that put an `errors:` on a `Dag` or a `Parallel` | `AbstractBranch._finally` and `Flow._finally` | **unclaimed — new at Task 7.** W1's model carries `Flow::finally_` and Task 7 deliberately did NOT add the task-level field beside `errors`, because no W3a task implements it and a field nothing reads is a dead field |
| ~~the replay a `CREATE_NEW_EXECUTION` retry starts~~ — **taken by W6 Task 4, all 4 banked** | ~~4 baselined entries (`retry-new-execution-*`), each right on every field but `spawned`~~ | `AbstractRetry.Behavior.CREATE_NEW_EXECUTION` → `ExecutionService.replay` | **W6 Task 4.** It is the third kind of sub-execution and `ExecutionState` had no field for any of them; it has one now — `metadata`, Kestra's own `ExecutionMetadata`, stored by migration 0139. `worker::replay::spawn_due` is the other half of the artifact's execution-delay loop |
| `afterExecution:` | **10** baselined entries | `Flow.afterExecution` | **unclaimed — new at Task 8.** W1's `Flow::after_execution` parses and `spec::validate` walks it; `build_plan` flattens `tasks` and `errors` and nothing else, so those tasks are in no plan. It is `finally:`'s sibling and unclaimed for the same reason |
| `{{ task.type }}` on a flowable | **0 baselined entries, and that is the finding** — a divergence this corpus cannot show | `worker::render`'s `task` context: `TaskKind::Runnable { type_id } => Some(..), _ => None`, where Kestra's `task.type` is always the FQCN. The divergence set is exactly `{Sequential, Parallel, If, Switch, Loop, Dag, Subflow}` — `Subflow` is IN it (`Executable` since W6) and the four opaque flowables are OUT (planned `Runnable`, so populated) | **unclaimed, and deliberately not fixed at W8 Task 1.** Only `valids/logs` and `valids/disable-simple` write `{{task.type}}` and both write it on a `core.log.Log` leaf, so no golden entry and no assertion can fail on it. The element of the class this corpus does not contain is **a `{{ task.type }}` on a flowable**. Found by the predicate sweep; recorded here and beside `render.rs`'s match rather than only in a task report |
| ~~`sla:`~~ — **half taken by W8 Task 6** | ~~**5** baselined entries~~ — **3**, and all three are `MAX_DURATION`s | `Flow.sla`, `SLA.Type.{MAX_DURATION,EXECUTION_ASSERTION}` | **W8 Task 6 took `EXECUTION_ASSERTION` and refused `MAX_DURATION` by name.** The cell read *"W1 ports it as a data shape and says so; nothing evaluates one"*, and that is now half false: `src/sla.rs` evaluates an `ExecutionChangedSLA` and `worker::claim::settle` acts on it, which banks `valids/flow-concurrency-sla-assert-cancel` and `valids/sla-execution-condition`. The other subtype is an `ExecutionMonitoringSLA` — a **separate scheduled loop** in the artifact, not the executor's message path — and **the three that remain cannot be banked by any implementation whatsoever**; see the fifth blindness mechanism. The row is LOWERED rather than deleted, because the construct really does still hold them |

**The `sla:` row is the first this table has lowered for a reason the reader
must not read as progress-pending.** Every other lowering marks work somebody
can still do. Here the remaining three, plus the two in `UNATTRIBUTED` held by
an `sla:` in their CHILD, are **unbankable by any implementation GIVEN THIS
HARNESS** — and the qualifier is load-bearing, because six lines below this one
names the repair. W8 Task 6 wrote *"permanently unbankable"* and then described
how to fix it, which is a contradiction a reader is entitled to stop at. The
five are not permanently unbankable; they are unbankable while
`driver::registry()` caps the `Sleep`, and the uncapping is on the record below
as a scoped, costed follow-on. What is permanent is
`valids/loop-until-restart` — a
`MAX_DURATION` needs an interval in which the execution is non-terminal and
older than the bound, and `driver.rs`'s `Sleep::capped(Duration::ZERO)` removes
it. A future package that implements `MAX_DURATION` correctly will move none of
these five, and a package that budgets against them is budgeting against
nothing. What a future reader would have to change to make them scorable is
named in that mechanism's row: **uncap the driver's `Sleep`** — which costs
`PT10M` on one entry alone — and give egret something the driver's
`tick`/`pass_time` loop has no analogue of, a monitor keyed on wall-clock time.

**The `flow concurrency:` row is the fourth this table has deleted rather than
lowered, and the third deleted because a STATE was credited to the wrong
construct.** `disabled:` lost `SKIPPED` to `runIf:`; `Subflow` lost its 22 when
it started executing; and this row's label — `concurrency:` → `CANCELLED` — is
false in the same shape as the first of those. `CANCELLED` is in exactly three
golden entries and **two are an `sla:`'s and one is an `Exit`'s**. Not one is a
limit's. A row like that is not merely too big: it makes the construct look
reachable *because the state really is there*, for another reason, and a
programme budgeting entries against it budgets against nothing.

**No limit ever fired, and it takes three instruments to say so — one per
`behavior`.** The thirteen declarers are **10 `QUEUE`, 2 `CANCEL`, 1 `FAIL`**,
and each behaviour leaves a different trace, so no single count covers all
thirteen:

- **`QUEUE` (10).** **`QUEUED` occurs zero times in `golden.json`** — over every
  execution history, run history, attempt history and sub-execution. It is in
  `harvest.py`'s `STATE_TYPES`, the vocabulary every captured state is validated
  against, and `normalise.py`'s rule 4 keeps history sequences **verbatim**, so
  a queued run would have reached the golden had one existed. `QUEUED` is the
  only state a `QUEUE` produces, so zero is not "the oracle could not see it" —
  it is *it did not happen*.
- **`CANCEL` (2).** Covered by the `CANCELLED` derivation above:
  `valids/flow-concurrency-cancel` and `…-cancel-pause` are the only flows
  writing `behavior: CANCEL`, and neither golden holds a `CANCELLED`.
- **`FAIL` (1).** Covered by neither of those, and **a count of `FAILED` could
  not cover it** — 609 of them are in the golden for every other reason. What
  covers it is that `valids/flow-concurrency-fail` is one of the **seven
  declarers that already MATCH**: egret implements no concurrency whatsoever,
  so an entry egret reproduces exactly is an entry whose limit did not fire.
  The ratchet asserts that match, and the `(13, 7)` assertion below names the
  population.

**Fix round 2 wrote this out.** The first version said the `QUEUED` zero proved
"no concurrency limit ever fired at all". The conclusion is true and all three
legs above hold, but `QUEUED` alone can only see `QUEUE` behaviour — a claim
true for a reason wider than its stated evidence, which is the same defect as a
figure written beside a derivation.

**And the zero has a mechanism rather than being an absence** — the count says
*that* no limit fired, these say *why*, which a count cannot. Three legs, each
measured:

1. **The harness never creates the contention.** `harvest.py`'s trace loop is
   strictly serial — `for i, rec in enumerate(wanted, 1): execute_once(...)`,
   each execution awaited to termination before the next is posted — so a
   `limit: 1` is never exceeded from outside.
2. **A flow can still contend with itself, and exactly one does.**
   `valids/flow-concurrency-parallel-subflow-kill` starts two `Subflow`s of
   `…-kill-child` from one `Parallel`, and `-child` is the flow carrying the
   limit. **W6 Task 3 banked all three of that family**, and the parent's golden
   says why a `QUEUE` that really fires is still invisible: its three `order`
   pairs are all `overlapping` and its `spawned` is empty, so *when* the second
   child execution starts is in no compared field. (Leg 1 alone would not have
   settled this, and naming that is the point of stating both.)
3. **The control.** ~~Seven~~ **TEN** of the thirteen corpus declarers already
   MATCH with nothing implemented — 7 when this was written, 9 after W8 Task 3
   banked the two `-pause` ones, 10 after W8 Task 6 — and the only two writing
   `behavior: CANCEL` — the sole configuration that could cancel — are among
   them: `valids/flow-concurrency-cancel`'s whole execution history is
   `CREATED RUNNING SUCCESS`.

   **The tenth turns this leg from an absence into a measurement, which is
   worth more than the count.** `valids/flow-concurrency-sla-assert-cancel`
   declares `concurrency: {behavior: QUEUE, limit: 1}` **and** ends `CANCELLED`
   — the exact shape a reader would credit to the limit. W8 Task 6 banked it by
   implementing `sla:` and nothing whatsoever about `concurrency:`, so that
   entry's `CANCELLED` is now reproduced **from the other construct, by name**.
   Legs 1 and 2 say no limit fired; this one shows where the state came from
   instead.

`tests/executor.rs`'s `the_cancelled_state_never_comes_from_a_concurrency_limit`
derives all of it on every run, so the sentence cannot drift back. Of the row's
six baselined entries, five are counted unmoved by rows that stayed (`Pause`
twice, `afterExecution:` twice, `sla:` once) and the sixth,
`valids/flow-concurrency-sla-fail-parent`, is now named in the residual
derivation's `UNATTRIBUTED` list beside `valids/sla-parent-flow` — same cause,
an `sla:` in its **child**, which a per-flow needle cannot see.

**What this does NOT say**, named rather than generalised over: nothing here is
a claim that Kestra's `concurrency:` does not cancel, or that the construct is
unimportant or untestable. It is the narrow claim the ratchet needs — *under
this oracle* the state is never a limit's — and the consequence is only that
flow `concurrency:` must be specified as an un-ratcheted surface with its own
tests, the position `egret.sql` and the four trigger words already occupy.

**Two rows are Task 8's, and checking the REMAINDER rather than trusting the
table is what found them** — the same method Task 3 used for `Subflow` and Task
6 for `LoopUntil`. `the_residual_baseline_is_attributed_to_a_construct_or_named`
derives the whole table from the baselined flows' own YAML and then asserts, by
name, the entries that declare none of these constructs — **13 when this
paragraph was written, 36 today** (the arc 13 → 29 → 35 → 36 is narrated under
the residual-baseline table below, and only the derivation is authoritative; W8
Task 1's sweep found this number stale, as it found the same count stale in
`tests/executor.rs`'s own comment). `afterExecution:`
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

*(Annotation, 2026-10-12, no-silent-stall Task 1: true when written, and no
longer. Since the ruling of 2026-10-11 and Task 1, `validate` reports errors on
52 of the 266 `runs` entries — gapped `Pause`s, `AllowFailure`,
`WorkingDirectory` and unregistered types, each a task egret could not run and
whose execution stalled. All are deliberate and named in
`executor::PUBLISHED_BUT_REFUSED`; see the section "no-silent-stall Task 1" at
the end of this file.)*

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
  egret's `spec::registry` is not Kestra's classpath. `unknown_task_type` was a
  **warning**, and promoting it to an error would refuse every plugin egret has
  not registered. That is a product decision, not a Task 8 rule. **It was
  taken at no-silent-stall Task 1 (2026-10-12)**: an unregistered type stalled
  every run that reached it (`NoHandler`), so `unknown_task_type` is an error,
  this entry is refused for Kestra's own reason and banked, and it moved from
  `STILL_OVER_ACCEPTED` to a refusal group. The 44 Kestra-published flows the
  same error refuses are named in `executor::PUBLISHED_BUT_REFUSED`.
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
line; a spurious **refusal** rejects a flow. *(Since no-silent-stall Task 1
that other finding is the error `unsupported_flow_task`, not a warning: an
unregistered type spelled like control flow is one no handler runs, so a run
reaching it stalled. It is fail-closed still, and it judges a different
question — whether egret can run the task at all — from this one.)* The same spelling test would refuse
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
  `WorkerTaskCallable.java:61`). *(The clause that stood here — "the same place
  `timeout:` already sits in the unclaimed list" — is retired twice over: no row
  of the unclaimed table was ever `timeout:`, and W4 Task 2 implemented it. See
  item 13.)* `allowFailure`/`allowWarning` change a terminal state, not
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

> **`a_skipped_run_still_gets_a_happens_before_endpoint` NO LONGER EXISTS, so
> the guard above is left that way after all — found by W10 Task 7's population
> 5, 2026-09-15.** It was deleted with the four-tier harness at W3b Task 5, and
> `tests/executor.rs`'s own deleted-test registry lists it with an **empty**
> successor and the note *"There is no successor: the tier each pinned does not
> exist."* `simulate_with` and `skipping_machine` went with it. So the paragraph
> that says a guard-nobody-could-see-fail was closed is now the only thing
> standing where the guard was, which is strictly worse than the state it
> reports: a reader checking this hole finds a citation instead of a test.
> **Recorded rather than fixed** — writing a successor means choosing which tier
> it pins, and that is a W3a decision this task has no standing to take. The
> shape is the one this file has already named twice: *a sentence citing a test
> is a claim like any other, and it ages like one.*

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

> **This artifact and its generator's self-test had both been failing, unrun,
> since W3b Task 5 — found by W3b Task 7's sweep and closed in its fix round.**
> `handler-surface.txt` is a pure function of `baseline.txt`, which Task 5 moved
> by 32 entries, so `python3 handler_surface.py --check` exited **1** with 124
> diff lines; and `--self-test` exited **1** with `12/13 guards fired for the
> right reason`. Nothing in the test suite ran either command, which is how six
> green gates passed over a generator whose own self-test could not pass.
>
> Both are closed, and **both halves of the fix are here rather than one**:
>
> - the artifact is **regenerated**, so `--check` exits 0 and the numbers below
>   are today's;
> - the rotted self-test case is **repaired**. *"A blocker rule that fires on an
>   entry egret already reproduces"* provoked its guard by calling
>   `OutputValues` a `FlowableTask`, which only produced the contradiction while
>   those entries still matched — and dropping the feed stopped them matching, so
>   the mutation went somewhere else and the case failed with the *trigger*
>   reconciliation guard's message. Measured afterwards: **no entry that matches
>   today runs a task type outside `W3B_HANDLERS` and `W3A_FLOWABLES` at all**,
>   so no `kinds` mutation can reach that guard and only the `state` arm can. The
>   case now gives a matching entry a run state `RunState` cannot hold, and its
>   victim is *derived* — the first matching `runs` entry — because naming one is
>   how the old case rotted;
> - both commands are **wired into the suite**
>   (`the_handler_surface_generator_checks_itself_and_its_artifact`), so their
>   exit codes are somebody's problem. A missing `python3` **fails rather than
>   skips**: everything the generator reads is committed.
>
> **The ceiling moved, 172 → 171, and every entry that moved is attributed.**
> The four buckets go `139 safe / 11 at risk / 33 bankable / 111 blocked` →
> `150 / 0 / 21 / 123`:
>
> | movement | entries | cause |
> |---|---|---|
> | at risk **11 → 0** | the 11 Task 1 named | the feed is gone (Task 5), so all 11 stopped matching; every one is now **blocked**, on the missing handler Task 1 predicted |
> | bankable **−16** | the 16 `loop-*` / `switch-in-concurrent-loop` / `flowable-with-parent-fail` entries | banked by the spawner; they moved from *bankable* to *safe* |
> | bankable **+4** | `errors`, `parallel-fail-with-flowable`, `restart-child`, `variables` | lost when the feed went, and **nothing in this file's rules blocks them** — they are W2 and W4 divergences a declaration-driven rule cannot see |
> | blocked **+12** | the 11 above plus `webhook-plaintext` | |
> | **ceiling −1** | `valids/webhook-plaintext` | the only entry that left the ceiling entirely: it matched, it is baselined now, and it is blocked rather than bankable |
>
> `150 + 21 = 171` and `21 + 123 = 144`, and the generator asserts both. Task 1's
> **172** and its eleven named entries are not lost by regenerating: they are in
> this file's coverage statement, in Task 1's and Task 5's reports, and in this
> artifact's own git history — **not** in the coverage statement, which carries
> no `172`, no `171` and no `ceiling` at all. That pointer was wrong when it was
> written, and it is corrected here rather than repaired by adding a number the
> statement has no use for. What a regeneration must never do is move the
> ceiling silently, which is why this paragraph exists.

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
  that never fires (`PT10S` over a `PT0.5S` sleep) and reproduces anyway. Only
  the feed can be taken away from it. (This example read *"an `sla:` nothing
  evaluates"* until W8 Task 6, which evaluates one of `SLA.Type`'s two subtypes.
  The reason the entry is inert was never that nothing evaluates an `sla:`; it
  is that **this** `sla:` is not violated, and that distinction is exactly the
  one the rule above is about.)
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
   (`the_ordered_pairs_inside_spawned_are_reachable_now`). The golden-side
   POPULATION is unchanged at 877 + 227 = 1104 — but "asserted" is the word the
   coverage statement retired: only **357** of those 1104 are pairs a change to
   which the ratchet can report, because the other 747 live on baselined
   entries. See *The coverage statement*.
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
it applies ONE task run's result per transaction and creates that run's
successors in the same one, where Kestra's worker pool finished two siblings
before either successor existed. That discipline is now a ruling with both sides
measured — see *One result per transaction is a ruling* immediately below. W3b
Task 1's Concern 3 said `valids/full`'s recorded attribution was wrong and its
real cause unknown; this is the cause, and it is the same one.

> **W4 Task 4 checked whether widening the tick is what brings the guard back.
> It is not, and that was measured rather than reasoned about.** The overlap is
> real — three of the four entries above are the three W3a named — so the
> hypothesis was worth testing: the trigger W3a wrote down is *"a caller that
> applies a batch partially"*, and W4's worker is the caller it meant. Under a
> prototype that applies a whole claim batch's results in one transaction before
> calling `advance()`, at `BATCH = 32`,
> `worker::CREATE_BESIDE_A_CREATED_SIBLING` is still **0** and
> `the_guard_w3a_deleted_did_not_come_back_and_the_reason_is_the_transaction`
> still passes — while the same run banks three of the four entries. The two
> things are independent, and the reason is the one this section already gives:
> the guard's state needs a worker result delivered *between two rounds of one
> `advance()` cascade*, and `claim::drive` runs that cascade to a fixed point
> inside one transaction under **either** discipline. Batching results changes
> what `advance()` is handed at the START of a cascade, not what it sees in the
> middle of one. The shared entries are a coincidence of which corpus flows have
> observable sibling order, not a shared mechanism.

### One result per transaction is a ruling — W4 Task 4

W3b handed W4 the question in *What this ratchet does not prove*, item 5. **The
answer is that the discipline stays**, and the ruling was written down with its
justification before the ratchet was run under the alternative.

> **Every number in this section names a run that is not reproducible from this
> checkout**, because the widened tick is a prototype and the shipped tree does
> not contain it. The raw output of all four runs — the widened ratchet, the
> whole crate under the mutation, the five reap configurations on both
> disciplines, and the fence-hole experiment — is **`w4t4-widened-tick.txt`**,
> beside this file, together with the literal mutation diff and the probe's
> source. A count written beside a derivation has to be derived; a count written
> beside a ruling has to be checkable.

#### "One run per tick" is the wrong name for it

`worker::tick` claims `n` runs and finishes `n`. What is one-at-a-time is the
*transaction*: `apply_outcome` takes one handler result, drives `advance()` to a
fixed point and commits. The two halves are separable and were measured
separately, which is the first finding — the constant everybody points at is not
the lever.

| configuration | `executor_matches_golden` |
|---|---|
| shipping — `driver::BATCH = 1`, one result per transaction | 143 mismatching |
| `BATCH = 32`, one result per transaction | 143 mismatching, `executor.rs` 34/34 |
| `BATCH = 1`, a whole batch per transaction | 143 mismatching — **forced by construction**: at `BATCH = 1` a group is one run, so the widened apply *is* `apply_outcome`. A consistency check, not an independent measurement |
| `BATCH = 32`, a whole batch per transaction | **140** — banks `valids/dag`, `valids/full`, `valids/parallel-disabled-tasks`; no regression |

`BATCH` buys nothing on its own because `worker::claim` orders by
`created_at, execution_id, run_ix` (W3b Task 7's fix), which makes a batch of `n`
indistinguishable from `n` successive claims of 1, and because the
happens-before endpoints are `executions.seq`, bumped once per `settle`. The
entries are bought by applying several results **before** `advance()` is called
at all.

`valids/if` is the one of the four that does not bank even then; it still differs
on `order` alone, and nothing here explains why. Its `Parallel` is the corpus's
only `concurrent: 4` (`every_corpus_concurrent_is_a_literal_integer` derives the
whole population as `valids/full=3`, `valids/if=4`), so a cap that throttles the
batch is the obvious suspect — **stated as a suspect, not measured.**

#### The step it costs, on one flow, both ways

Two `Sequential` branches of one `Parallel` — `valids/parallel-disabled-tasks`'s
shape. One tick finishes `task1` and `task3`.

| | `ended(task1)` | `created(task2)` | `ended(task3)` |
|---|---|---|---|
| shipping | 8 | 9 | 11 |
| a batch per transaction | 8 | 9 | **8** |

The widened row is Kestra's shape exactly: both siblings on one step, both
successors on the next. `tests/worker.rs`'s
`one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs`
asserts the shipping row, and **it is the only test in the crate that can tell
the two disciplines apart** — measured: under the widened prototype at
`BATCH = 1`, it is the only failure in the crate and every one of the other
**707** tests passes, `executor.rs` included (589 lib + 34 executor + 10 oracle
+ 13 publish + 10 repo + 17 routes + 8 sql_starrocks_live + 27 worker = 708,
less this one). Before this task the ruling had no instrument at all.

#### Why it was refused, and it is not the number

Four costs, all measured on the prototype rather than argued, in the order of
how much weight the ruling puts on them. **Every run behind them is in
`w4t4-widened-tick.txt`**, because the widened code is not in this checkout and
the numbers are otherwise unreproducible from it.

1. **THE ONE THE RULING RESTS ON: two guards would stop being guards, and
   `persist`'s four-mutation table would start certifying a shape the code no
   longer has.** That table proves the `outputs` fence and the state fence are
   ALTERNATIVES, each sufficient alone — measured against a transaction that
   releases exactly ONE lease. Widened, it releases `n`.

   The experiment (RUN 4) puts a defect in the surviving alternative that can
   only bite at `n > 1`: delete the `outputs` fence outright, which the table
   says is survivable, **and** make `persist` fence only the group's FIRST lease
   — byte-equivalent at `n == 1`, and at `n > 1` a reaped worker can commit a
   terminal over its replacement, which is W3b's Critical exactly. Result:

   | suite | under the widened tick with both fence defects |
   |---|---|
   | lib | 589 passed, 0 failed |
   | `executor.rs` | 34 passed, 0 failed |
   | `oracle` / `publish` / `repo` / `routes` | 10 / 13 / 10 / 17, all passing |
   | `sql_starrocks_live` | 8 passed, 0.74s |
   | `worker.rs` | 26 passed, **1 failed** — the ordering guard, firing for the ordering |

   So `a_worker_whose_lease_was_reaped_cannot_commit` and
   `a_tick_killed_between_the_handler_and_the_commit_applies_nothing` pass with
   both fences effectively gone for every non-first member of a group, because
   both drive `apply_outcome` with a single `Claimed`. **A programme whose first
   law is *a guard nobody has seen fail is a guard nobody has seen* cannot buy an
   ordering nicety with two guards that stop being guards.**
2. **`working_timeout_s` stops meaning what it says.** A run's lease is held for
   `input_for` plus its own handler today; batched, for the sum over the batch.
   The reaper still reclaims exactly what the timeout says — durability property
   3 survives — but the quantity it bounds changes from one handler to a whole
   batch, and the operator setting it cannot know the batch size.
3. **It is the coupling `tick`'s own doc refused once already** — *"One run's
   failure does not strand the rest of the batch … the first draft did exactly
   that with `?`"* — this time inside the transaction, where an early return
   cannot be softened into "carry on with the next one".
4. **A lost lease costs the whole group rather than the run it named — the
   weakest of the four, and the first version of this section got it wrong.** A
   batch is one transaction, so one `WorkerError::LeaseLost` aborts every result
   in it. That was first measured in a single configuration — a registry reaping
   from the SECOND handler — and reported as a live-lock belonging to the
   widening. **It does not belong to the widening.** Five configurations, one
   scratch database each (RUN 3), six rounds, handler calls / SUCCESS runs:

   | reap | narrow (shipping) | widened |
   |---|---|---|
   | none — the control | 2 / 3 | 2 / 3 |
   | 2nd handler, every call | 7 / 1 | 12 / 0 |
   | 2nd handler, once | 3 / 3 | 4 / 3 |
   | **1st handler, every call** | **12 / 0** | **12 / 0** |
   | 1st handler, once | 4 / 3 | 4 / 3 |

   The shipping tick live-locks too, with the identical
   `a, b, a, b, a, b, …` call log, when the reap fires from the first handler.
   An adversary that reaps on every call starves both disciplines. What widening
   actually costs is the **asymmetry**: this tick survives one of the two
   placements and the widened one survives neither. Under a transient reap — the
   realistic case, since `reap` fires on a lease that has actually expired — both
   recover fully and the widening costs exactly ONE extra handler re-run.

   The row that makes the other four readable is the control: two handler calls,
   three SUCCESS runs. Without it every row is compatible with a fixture that
   never ran.

Against that, the four entries differ on the happens-before relation alone —
every run, attempt and execution state already matches. Buying them by changing
what a lease means is the trade W3b refused when reversing the claim's tiebreak
also banked `valids/dag`, and the reason is the same: **choosing the shape of the
runner by what it banks is how an oracle gets tuned to its own answer.** The
ruling and its costs were written down before the 140 was known; the number is
reported as measured and did not change them. What the number *is* changed by:
`valids/if`'s cause is unknown, so **140 is a lower bound on what the widening
banks** and the benefit column is biased downward, not upward.

**A fifth thing the widening changes, derived from the code shape rather than
measured, and it is a BEHAVIOUR change rather than a durability one.** `tick`
today calls `input_for` and the handler and the commit per run, in that order,
so the second claimed run's render surface is loaded *after* the first has
committed and a sibling that writes `{{ outputs.<earlier sibling> }}` resolves
it. The widened tick runs every `handle_one` before any commit, so no member of
a batch can see another member's outputs at all. Neither behaviour is Kestra's
guarantee — a `Parallel` sibling reading a sibling's outputs is a race there too
— but the widening silently *narrows* what a handler can see, and no test in
this crate or entry in this corpus would notice.

#### What would reopen it, named rather than left implicit

A design in which a batch's results are applied together **without sharing a
failure** — a `SAVEPOINT` per run inside the group transaction, so a lost lease
rolls back one run's writes and not the group's. That removes cost 4 entirely
and shrinks cost 3. Costs 1 and 2 remain, and **cost 1 is the binding one**: such
a design has to bring `n`-lease versions of both fence tests and re-derive
`persist`'s mutation table against the `n`-row shape, and the savepoint boundary
needs a mutation table of its own. The three entries stay available at that
price; this ruling does not spend them.

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

**The suite used to self-skip without `EGRET_TEST_STARROCKS_URL`, and this
section's account of what that cost was wrong.** It said *"7 of the 26 tests W3b
Task 6 added do not run by default"*, which is not the observable: measured both
ways in one command, a run without the engine printed `7 passed; 0 failed;
0 ignored — 0.00s` and a run with it printed `7 passed; 0 failed; 0 ignored —
0.88s`. Identical counts, nothing `ignored`; **all 26 report `ok` either way**,
and only the duration distinguishes an empty green from a real one.

It **fails** now, and a skip has to be typed into the invocation
(`EGRET_ALLOW_ENGINE_SKIP=1`), which is the rule `egret-test-db` already applies
to `DATABASE_URL` and the rule W3b Task 7 applied to `python3` in
`tests/executor.rs` and not here. `just check`, `just test-no-db` and CI carry
the opt-out explicitly, so a green with no StarRocks says so in the command that
produced it. `the_engine_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed`
drives the gate in both directions and is the one assertion in that file that
runs without an engine — a gate that panicked unconditionally would pass a
one-direction test and break every deliberate skip.

### Three things `egret.sql` does not have, stated here rather than found later

1. **No cancellation.** `TaskHandler::cancel` **refuses** on this handler rather
   than answering `Ok(())`: it holds a statement running on a remote engine, and
   `QueryExecPort` has no cancellation method. Nothing calls `cancel` at all yet
   (W4 owns the kill path), so adding one to the port would be adding a method
   ahead of its caller twice over. The refusal is what makes W4 fail loudly at
   the one place that has to change.

   > **W4 Task 3 correction.** The kill path exists now
   > (`worker::cancel`), it fires when a task-level `timeout:` expires, and this
   > refusal is reached — asserted by
   > `handlers::sql::tests::the_kill_path_reaches_this_refusal_through_the_registry`.
   > It still refuses, and the reason moved from "nothing calls it" to where the
   > cancellation channel actually stops: `Connector::execute` takes a
   > `CancellationToken` and Trino honours it, but `port_impl::run_bounded`
   > owns that token privately and `QueryExecPort` exposes no cancel verb across
   > its 17 implementation sites (16 of them test fakes). The refusal now names
   > the per-engine cost — Trino
   > bounds the abandoned statement with the `query_max_run_time` the task
   > already sent; the StarRocks connector does not read `Limits::timeout` at
   > all, so nothing bounds it.
   >
   > **Fix round 1 — this is a DIVERGENCE, not a gap the artifact shares.** The
   > first reading stopped at `WorkerJobLifecycle.kill()`, whose default body is
   > empty. One layer down,
   > `io.kestra.plugin.jdbc.AbstractJdbcQuery.kill()` calls
   > `Statement.cancel()` then `close()` on a handle it kept, and both
   > `plugin-jdbc-trino`'s and `plugin-jdbc-mysql`'s `Query` extend it
   > (disassembled from `/app/plugins/*.jar` in the pinned image). **Kestra's
   > own SQL task stops the remote statement; `egret.sql` does not** — a
   > `timeout: PT10S` on a one-hour StarRocks query runs the full hour here.
   > *(Corrected 2026-10-17: on StarRocks 4.1.6 the statement is cancelled when egret drops the connection at the budget — measured; see docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md §1.2.)*
   > What keeps the refusal is cost and scope, not the artifact: the port verb
   > would buy a true cancel only on Trino, which `query_max_run_time` already
   > bounds, because on StarRocks the token is honoured only cooperatively
   > between drained rows and a real cancel needs `KILL QUERY` on a second
   > connection. `handlers::sql`'s `cancel` doc carries the full disassembly and
   > the three things an implementer needs.
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
> ordered-pair bullet (**357** of the 1104 can fail, not 1104 and not 877), the
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
  measured that only **357** of the 1104 are pairs a change to which the ratchet
  reports at all. The other 747 sit on baselined entries, which mismatch whatever
  the pair says — flipping all 747 leaves the ratchet green and unmoved. That was
  as true when this sentence was written as it is now. The annotation itself said
  354/750, which is what those two were before the review's fix round banked
  `valids/replay-loop`; a correction is a sentence like any other and ages like
  one.)*
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
  exception. W4 owns it — **and W4 Task 2 has now taken it**: `worker::timeout`
  renders the budget per run and enforces it around `Registry::run` in
  `claim::run_one`, which is where the artifact enforces it. What has NOT
  changed is this bullet's own subject: `PlanTask::timeout` is still `None` on
  every task and `advance()` still never reads one, so the field is still dead
  and still will be. See the coverage statement's item 13. The plan's Task 7
  step said "`timeout` is a deadline compared against the `now` parameter"; it
  is not, and no line of Task 7 implements one.

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
  is populated for **39** entries, **22** of the 41 MATCH, **4** differ only
  inside the field and 15 on more *(35/16/6/19 until W6 Task 8)*, and egret
  produces **126** ordered pairs
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
changed underneath every sentence. **The whole-branch review's fix round
re-derived them again**, because it banked one more entry (`valids/replay-loop`,
by giving a sub-execution its parent's `inputs`) and every split below is a
function of `baseline.txt`. The two flip experiments and the two corruption
experiments were **re-run**, not adjusted. Several numbers moved in directions the old
wording did not allow for, and one of them (the ordered pairs) had already been
found over-claiming by 227 once.

**W4 Task 6 re-derived them a third time**, on this tree, and the ratchet had
not moved at any point in the package: **143 before Task 1 and 143 at this
commit**. Nothing here changed because of what W4 shipped — three holes were
closed and none of them is visible to this corpus, for three *different* reasons
— so the re-derivation's whole yield is a sweep. It ran **five times**, and each
pass found what the one before it missed: eight figures in this file and five in
`runnable.txt`; then a fourteenth **inside the note that retires the other
thirteen**; then five more in `src/`, `tests/` and `oracle-exec/driver.rs`; then
two — a fifteenth and its twin — **in this section's own preamble and in
`runnable.txt` nine lines from a figure the same sweep had already superseded**.
Four of the five passes were prompted by review rather than by the sweep
noticing its own gap, and each of the five scoped itself by *surface* when the
property is about *mechanism*. **The rule that would have caught all seventeen
is in *The rule this sweep ends with* at the end, and it is the third
formulation, not the first.** They
are in *The stale twins W4 retires* at the end, with what each is now. The one
number that moved in this package's own arithmetic used to be the count of
things this statement does not prove — thirteen when it was first written,
seventeen after W4, eighteen after W6 Task 8 — and **that total is no longer
stated, because maintaining it is what made this paragraph disagree with itself
twice.** The list below is numbered; the reader who wants the size of it can
count the headings. What a total cannot say is which item to read first, so that
is named instead: **item 15**, which was *"nothing in this workspace runs this
worker"* and is **CLOSED since W10 Task 2** with a bound that replaces it,
**item 18**, the four trigger constructs, and **item 19**, what the execution
API W10 shipped still cannot do. *(The sentence this paragraph carried for four
packages — *"it was thirteen and it is seventeen, and the seventeenth is not the
last one a reader should look at; item 15 is, which is why it is also the sixth
fact of the W4 section above: nothing in this workspace runs this worker"* — was
**true when written and false from W10 Task 2 onward**, in the one place this
statement tells a reader to start. Kept here as the record, because the
proposition it states is the one W10 Task 7 swept the repository for.)*

**W6 Task 8 re-derived them a fourth time, and this is the first package whose
ratchet MOVED under a re-derivation**: 143 before Task 1, **127** at this
commit, sixteen entries banked — twelve by `Subflow` executing and four by the
`CREATE_NEW_EXECUTION` replay. So the sweep is not the whole yield this time;
half of it is the W6 section below, which exists because **sixteen banked
entries can make four triggers look measured when none of them is.** The sweep
found stale copies at **sixteen** sites — thirteen corrected, three named — and
every single one is a figure the suite asserts under a name the stale sentence
did not use, including **two of the five bullets in this file's own opening
reading instructions**, four doc comments in `src/` and `tests/`, and a pair
that moved in opposite directions by the same amount so that *the total held*. Both
of those falsify rules this statement has relied on, and they are written out at
the end of the W6 section rather than here.

**W8 Task 7 re-derived them a fifth time, and the ratchet moved again**: 127
before W8 Task 1, **109** at this commit, eighteen banked across four
constructs. Two things make this pass different from the four before it. First,
the sweep's yield was not stale prose beside a correct figure — it was **one
family of six numbers copied to five places and maintained in one of them, with
two of the copies disagreeing with each other**; the W8 section carries it as a
table with the sites named. Second, W8 Task 7's own change is a **product defect
the ratchet scores in neither direction** — `claim::due_retries` would never
wake a `PAUSED` run — measured at **109 with the fix and 109 without it**, which
is item 15's shape one level in and is why the rule this statement ends with is
extended there: *a figure is safe only if a running program compares it — and so
is a **predicate***. *(Item 15 is **closed since W10 Task 2**; this names the
shape it recorded, not its verdict, and the shape outlived it — W10's own change
is the third measurement of the same kind, 109 with the wiring and 109 without
it.)*

**W10 Task 7 did NOT re-derive them a sixth time, and that is this statement's
own rule being applied rather than skipped.** Every count in the first row of
the provenance table is compared by a named test on every run; the ratchet did
not move (**109 before W10 Task 1, 109 at this commit, and 109 with every line
of W10's wiring deleted** — the third number is what makes the first two mean
anything, and it was measured rather than argued; *W10* below has the method);
and the gate is green. Re-typing those figures here would be re-running what
`cargo test` already ran, and would add a copy for the next package to find
stale. **What this pass swept instead is the half the suite does not compare** —
and the thing it found is not a figure at all but a **proposition**: *nothing
wires the worker into the binary*, true for three packages and false since W10
Task 2, in the sentence this statement tells a reader to start at. So this pass
has no banked entries and no ratchet movement to attribute; its yield is the
sweep, five populations wide, listed in *What W10 Task 7 swept, and what each
population yielded* below with every denominator computed over `git ls-files` so
it can be re-run. **It is five populations rather than a sixth surface because
all five passes before it scoped themselves by surface**, which is the
structural finding at the end of this file and not a preference here.

**Where this section and anything above it disagree, this section is right**,
and the disagreements are listed by name in *The stale twins this statement
retires* at the end, for the ones W6 moved in *The figures W6 retires* in the W6
section, for W8's in *The figures W8 Task 7 retires* in the W8 section, and for
W10's in *The figures W10 retires* in the W10 section.

**The provenance rule below used to be stated for the whole statement and was
itself stale — the whole-branch review's Important 3.** It read: *"Every **count**
below is printed by a named test in `crates/egret-workflow/tests/executor.rs` on
every run, so none of them is a number written down beside a derivation"*, with
three named exceptions. That was true when W3b wrote it and **the W4 section
appended underneath it falsified it**, because W4 added figures that no test
compares. A stale *rule*, which is why five sweeps for stale *figures* went past
it: they were looking for numbers that had drifted, and this is a sentence that
stopped being true without any number changing.

So the rule is restated per family rather than for everything:

| family | compared by a running program? |
|---|---|
| the ratchet/corpus **counts** (143, 345, 294, 266, …) | **yes** — a named test in `tests/executor.rs` on every run |
| the **426** fed terminals | no — history, and its code was deleted with the harness |
| the **provenance** | no — a fact about a docker tag |
| the two **experiments** (flipped pairs, corrupted terminal) | no — recorded with method and control so they can be re-run |
| W4's instrumented **`wake` / `guard`** figures | **no** — measured once, under instrumentation that is not in the tree |
| W4's prototype **143 → 140** | **no**, and `claim.rs` says so itself: not reproducible from this checkout |
| W4 Task 5's **29-row mutation table** | **no** — derived by a harness in a scratch directory, not by anything `cargo test` runs |
| W6's **sixteen banked entries**, the trigger census, and the `142 = 134 + 8` control | **yes** — `the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none`, on every run |
| W6's **artifact measurements** (the cron dialect, DST and late-tick behaviour, the 12/47/**100** nested loops, the 498/499 publish boundary, the `dependsOn` double-fire) | **no** — taken by running Kestra's own code against the pinned digest. Reproduced once each by a second party, which is the strongest thing said about them |
| W6's **mutation tables** (Task 2's 7 then 11, Task 3's 12, 31/30/1 schedule, 45/45 flow, the webhook's 60/60 against a 0/30 control on the unmutated binary, and Task 8's 5 at 3 runs each plus 1 at 5) | **no** — run by hand, each with its run count and its control beside it, because *a one-run verdict is a coin flip reported as a measurement* |
| W6's **claim classification** (149 claims → MEASURED 91 / DERIVED 33 / CHOSEN 25) | **no** — a hand audit, after a keyword classifier scoring 70/3/76 was built and thrown away |
| W8's **eighteen banked entries**, the **six projections** of a baselined entry, and the **`(8, 17, 25)`** retry split | **yes** — `the_residual_baseline_is_attributed_to_a_construct_or_named`, `the_baselining_argument_covers_every_field_and_not_only_the_ordered_pairs`, `kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line` and `the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them`, on every run. **The prose copies of the six are NOT**, and five of them were stale — see the W8 section |
| W8 Task 7's **instrumented `wake` over the oracle** (1 fire in one of five runs, 0 in four) | **no** — measured under an `eprintln!` that is not in the tree, over five runs, with `tests/worker.rs` as the positive control on the same build and the ratchet read at **109** in every one |
| W10's **publish census** — `LIVE` **248** of 345, `UNKNOWN_TYPE_REFUSED` **68**, `VALIDATE_REFUSED` **20**, the `SUBFLOW_TARGETS_*` split **18/1**, the `SUBFLOW_LOOKUPS_ON_{PUBLISHED,REFUSED}` pair **47/2** | **yes** — every one is a `const` in `oracle-exec/driver.rs` that `driver::census`, `driver::target_census` and `driver::lookup_census` compare on every run of `executor_matches_golden`. This is the family that has now moved three times (16 → 21 → 248), and the **prose copies of it are not compared** — see *The figures W10 retires* |
| W10's **production call sites** (5 across 3 files) and the **trigger isolation** re-measurement | **no** — a `git ls-files` grep with a positive control, re-runnable and written out beside each figure in the W10 section. Nothing in `cargo test` counts either |
| W10's **"109 with the wiring deleted"** | **no** — one mutation run (`runtime.rs` deleted, the module declaration and re-export removed, the `egret-bin` call site commented out; `git diff --stat` showed `338` deletions before the verdict was read), against an unmutated control in the same session. Both read `109 mismatching, 109 baselined, 0 broken` |

A figure is safe only if a running program compares it. **Every row the second
column marks `no`** is a figure a reader must treat as a measurement someone
took, not as a fact the suite maintains — which is the honest statement, and the
one that survives the next section being appended.

*(That sentence read **"everything in rows 2–7"** until W10 Task 7. It was
written when the table ended at row 7, and W6, W8 and W10 have each appended
under it since — so a rule whose whole subject is figures that go stale when
somebody appends **went stale when somebody appended**, in exactly the way
*"every number below"* did in this file's opening reading instructions. The fix
is the same one: name the property, never the row numbers. No count replaces the
range, on purpose.)*

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
| matching | **183** |
| baselined — egret does not reproduce it yet | **111** |

```text
  matching  183 = 160 of the 266 `runs` + 23 of the 28 `refused`
  baselined 111 = 106 of the 266 `runs` +  5 of the 28 `refused`
  ------------------------------------------------------------------
            294 = 266 + 28
```

111 is simultaneously the mismatching count, the size of `baseline.txt` and the
baselined count: **0 stale lines, 0 regressions, 0 unbanked progress, 0 broken**.
That identity is the ratchet's own output, not a hand check.

**176 → 183 at W8 Task 5**, one construct with seven instances: the
`allowFailure:`/`allowWarning:` FLAG, which is not the `AllowFailure` flowable.
The `refused` half is unchanged again: no corpus flow is refused for writing
either flag, and `models::Task` reading them changes no publish verdict.

**175 → 176 at W8 Task 4**, one entry: `valids/waitfor-nested`. The `refused`
half is unchanged for the same reason Task 3's was — the only `LoopUntil`-family
flow Kestra refused is none at all, and `invalids/workingdirectory-invalid` is
still refused by egret, now for BOTH spellings of the construct rather than one.

**167 → 175 at W8 Task 3**, one construct with eight instances: `Pause`. See
*What W8 Task 3 had to change, and what it banked* at the end of this file; the
`refused` half is unchanged, because the only `Pause` flow Kestra refused is
`invalids/workingdirectory-invalid` and egret still refuses it.

**163 → 167 at W6 Task 4**, one cause with four instances: the
`CREATE_NEW_EXECUTION` replay. `valids/retry-new-execution-{flow,task}-{attempts,duration}`
were each right on every field but `spawned`, and each now reproduces the chain
the artifact ran — three executions, the first two `RETRIED` with a replay
hanging off `parentId`, the third `FAILED` with none. What was missing was not
the link column (migration 0138 gave it that at Task 2) but the two facts that
END a chain: `execution.getMetadata()`'s `attemptNumber` and
`originalCreatedDate`, which `advance()` read as the constants `(1, Tick(0))`.
Those are true of the first execution of a chain and false of every replay, so
nothing could ever refuse the fourth. Migration 0139 stores them,
`ExecutionState::metadata` carries them, `retry::scan` emits the `ExecutionDelay`
the artifact builds for **both** retry behaviours, and `worker::replay` creates
the execution.

**151 → 163 at W6 Task 3**, and unlike the two movements below it this one is a
single cause with twelve instances: `Subflow` executes. Every one of the twelve
declares a `Subflow`, every one of them stalled at that run before
(`build_plan` planned it an opaque `Runnable`, which gave it a
`task_runs.type_id`, which made it claimable, and `claim::leaf` then answered
`None` because `models::TaskKind` for the same task is not a `Leaf` —
`WorkerError::NotALeaf` aborted the tick), and each is attributed individually in
`baseline.txt`'s header. **The 28 `refused` entries did not move at all**, which
is the load-bearing half of the split above: nothing in this task changed what
egret refuses.

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

**150 → 151 at the review's fix round**, and that one is a single event:
`valids/replay-loop`. `worker::spawn`'s INSERT did not name `executions.inputs`,
so a sub-execution got migration 0136's `DEFAULT '{}'` and a loop body reading
`{{ inputs.… }}` rendered against nothing. Task 4 wrote the INSERT and Task 5
added the column — a seam, and the shape this file keeps finding. It also moves
`valids/finally-loop` and `valids/finally-loop-parallel` out of *differs on more*
into *differs only inside `spawned`*; those two are the entries whose loop body
branches on `{{ inputs.failed == false }}` with `true` posted, i.e. the ones that
name the defect. They stay baselined for `finally:`.

### The ordered pairs: **kept is not asserted**

```text
  kept by the harvest (identical in all five rounds)      1104
    at the top level                                       877
    inside `spawned`                                       227

  a change to which the ratchet REPORTS                    399
    at the top level                                       297
    inside `spawned`                                       102
  a change to which it does NOT                            705
    at the top level                                       580
    inside `spawned`                                       125
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
`spawned` — 39 entries produce one since W6 Task 4's four replays — so the
reason is gone. **The replacement is
not 1104.** A pair on a *baselined* entry is compared and cannot be falsified:
the entry mismatches whatever that pair says, so changing it changes nothing the
ratchet reports. That was true of the 622 top-level pairs on baselined entries
under W3a too, and the old sentence counted them as asserted.

Measured rather than reasoned, by flipping `before` ⇄ `overlapping` in
`golden.json` and reading the ratchet, restoring the file from a snapshot and
verifying its md5 (`5773748d22d521e6a65afc5e2e2663d1`) after every run:

| flipped | pairs | entries | the ratchet |
|---|---|---|---|
| every **nested** pair — W3a's own experiment | 227 | 18 | **FAILS**: 150 mismatching, **7 REGRESSIONs** |
| every pair on a **baselined** entry | 747 | 90 | green: 143 mismatching, 0 banked |
| every pair on a **matching** entry | 357 | 41 | **FAILS**: 184 mismatching, **41 REGRESSIONs** |

*(**These three rows are W4-era and are NOT re-run at W6 Task 3 or Task 4.** The
populations they flip moved with the baseline — 747 → 744 and 357 → 360 at Task
3, again at Task 4, and again across W8's eighteen banks to **705 and 399** —
and the `143 mismatching` in the middle row would read `109` — so the numbers in the
right-hand column are a record of an experiment on a different tree, not a
current measurement. They are left as they were run rather than adjusted,
because an adjusted experimental result is a number no program ever produced;
the two populations they rest on ARE re-derived above and asserted by
`kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line`. The
CONCLUSION is unaffected: two thirds of the kept pairs still cannot fail.)*

The first row is the one to read against W3a's, which recorded that flipping all
227 left this crate **green**. The second is the honest half: two thirds of the
kept pairs still cannot fail, and the way to make one of them fail is to bank its
entry.

**And the un-falsifiability is not a fact about ordered pairs.** It is a fact
about a baselined ENTRY, and it therefore holds of every golden field on it:
`executor_matches_golden` reports a non-baselined mismatch or a baselined match
and nothing else, so a change to any field of a baselined entry leaves the
ratchet saying exactly what it said before. The pairs are simply the family
somebody measured. The same split over the other families, derived by
`the_baselining_argument_covers_every_field_and_not_only_the_ordered_pairs`:

| family | on a baselined entry, i.e. un-falsifiable | total |
|---|---|---|
| run terminals | **396** | 840 |
| run-history state tokens | **1528** | 3355 |
| execution-history state tokens | **522** | 1462 |
| executions (266 entries + their 142 sub-executions) | **147** | 408 |
| sub-executions | **43** | 142 |
| kept ordered pairs | **705** | 1104 |

*(All six fall together, because they are six projections of one set — what
lives on a baselined entry. They fell at W6 Task 3 (445 / 1759 / 709 / 177 / 51
/ 744, the twelve `Subflow` entries carrying 16 run terminals, 48 run-history
tokens, 42 execution-history tokens, 14 executions, 2 sub-executions and 3
ordered pairs between them), again at W6 Task 4 over four entries, and again
across the whole of W8. **The W8 deltas are read off the `const`s' own arcs
rather than subtracted here**: run terminals `433 → 419 → 412 → 396`
(**37** over Tasks 3–5, and Task 6 moved it by 0 because both of its entries
have `"runs": []`); run-history tokens `1703 → 1647 → 1610 → 1528` (**175**);
execution-history tokens `665 → 617 → 562 → 526 → 522` (**143**); executions
`165 → 157 → 156 → 149 → 147` (**18**); ordered pairs `744 → 705` (**39**); and
sub-executions `43 → 43`, **0**, because **not one of the eighteen carries a
sub-execution**.)*

**This table was two packages stale, it is the fourth copy of these six numbers
in this file, and it DISAGREED with the third.** Item 2 carried
`744 / 433 / 1703 / 665 / 165 / 43` and this table carried
`744 / 445 / 1759 / 709 / 177 / 51` — the same six projections of the same set,
differing in five places, because item 2 was re-derived at W6 Task 8 and this
one was left at W6 Task 3. **Two copies of one figure that disagree with each
other and with the program are the strongest available argument for the rule**:
the six live as `const`s in `tests/executor.rs` whose doc comments carry the
whole W8 arc and were updated by every task that moved them, and as prose here,
in item 2, in the block above and in this file's opening blockquote —
**maintained in exactly one of the five places, and inconsistent among the other
four.** The rule that finds this is *sweep the POPULATION a figure belongs to* —
every site quoting any of the six — rather than the sentence that prompted the
sweep; see the W8 section's own retirement table.

The run-terminal row is the same 840 the fed table classifies, so the two
sections are talking about one set of runs. Nothing here is a *new* weakness —
it is the same one sentence, applied to the fields nobody had applied it to.

Two smaller facts about the population: **13 pairs were dropped** by the harvest
(`valids/dag` 1, `valids/if` 9, `valids/parallel-nested` 3), all of them because
the relation flipped between rounds; and the restriction removes **85** pairs
across **9** entries from egret's side for having no counterpart in the golden —
15 under W3a's harness, over top-level pairs only. egret's relation domain is
measured at `{before, overlapping}` over all **791** pairs it produces,
sub-executions included, so dropping a pair is exactly accepting either value
Kestra was seen to produce.

*(**80 across 6 over 771 through W5; 85 across 9 over 791 at W6 Task 3.** The
three entries added are `valids/restart-parent`, `valids/restart-parent-loop`
and `valids/subflow-secret-output-parent` — `Subflow` parents that used to stall
at their `Subflow` run and now run past it into a successor Kestra never
reached, which is exactly a pair with no counterpart. All three are registered in
`STILL_OVER_ACCEPTED_AT_RUNTIME` against the child-side over-acceptance that
causes it. The 771 → 791 is the twelve banked entries' own runs gaining
endpoints.)*

*(**This paragraph said "unchanged and still derived: … 87 pairs across 7
entries … over all 778 pairs" and both figures were a version behind**, which
W4 Task 6 found by reading `RESTRICTION_REMOVES` and
`EGRET_PAIRS_BEFORE_RESTRICTION` — the constants the sentence's own citation
asserts — rather than the sentence. The review's fix round banked
`valids/replay-loop` and took one entry and seven pairs out of the projection
with it; the tables above were re-derived for that bank and this sentence was
not, because the word "unchanged" is not checked by anything. It is the
statement's own instance of the shape it retires below: a figure true at a
different moment, sitting one paragraph from the figures that moved.)*

### The terminal states this oracle feeds: **426 → 0**

W3a's harness was **circular for one field**: `golden_outcome` handed egret's
side the golden's own terminal for every `Runnable` run and every `Loop` run.

**The numerator is 426 and it used to be written 630, which overstates it by
204 — by this file's own table.** 630 is `575 fed + 55 spawner`, the whole of
those two buckets; but the table those buckets come from says in the same row
that **189** of the 575 and **15** of the 55 live inside a sub-execution egret
never created, so nothing was supplied to them and nothing was compared about
them either. A terminal that was never fed cannot be part of the count of fed
terminals:

```text
  fed bucket        575  −  189 inside a sub-execution  =  386
  spawner bucket     55  −   15 inside a sub-execution  =   40
  --------------------------------------------------------------
  terminals actually fed                                   426
```

The removal is real and is not what is being corrected: `golden_outcome` is
deleted, and the only golden→egret flow left at HEAD is
`Trace::restrict_order_to`. What 630 did was make the circularity look 48%
larger than it was. This is history and not a count — the code that produced it
is gone — which is why it is derived here rather than printed by a test.

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
| `valids/sequential`, a top-level `Runnable` run — an entry that **matches** | **FAILS**: 144 mismatching, `valids/sequential` named a REGRESSION |
| `valids/loop-serial`, a run **inside a sub-execution** — also matching | **FAILS**: 144 mismatching, `valids/loop-serial` named |
| `valids/purge_logs_no_arguments`, whose task type has **no handler** | green, 143 mismatching — **the control** |

The third row is the point. **W3a's first attempt at exactly this demonstration
was vacuous**: it corrupted `valids/minimal2`, whose golden carried a `SUBMITTED`
that `RunState` could not represent, so that entry could not match under *any*
value and the experiment could not have reported anything. An entry that already
mismatches gives the same green answer whether the field is fed or not.
`a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to` is the
committed version, and **it now carries the nested half too** — the second row
above was prose here and nothing in the test file, so the claim that a wrong
value *inside* a sub-execution is caught rested on nobody's assertion. "The
field is filled" and "a wrong value in it is caught" are exactly the two things
this package keeps finding apart. It *derives* its control ("a golden whose first run's
history holds a state `RunState` cannot hold") instead of naming one — W3c added
`SUBMITTED` to the enum, so the entry W3a picked by name matches today and would
have made the control vacuous in its turn.

### The sub-executions

```text
  41   golden entries carry a sub-execution
 142   sub-executions they carry between them

  22   of the 41 now MATCH
   4   agree on everything the parent execution holds and differ INSIDE `spawned`
  15   differ on something else as well
  ---
  41   TOTAL
```

*(**17/8/16 through W5; 18/8/15 at W6 Task 3; 22/4/15 since W6 Task 4** — the
four `valids/retry-new-execution-*` entries moved from the middle row to the
top one when `worker::replay` created the replacement execution they were
missing. **This block read 18/8/15 until W6 Task 8's sweep**, and the reason it
survived Task 4 is worth naming: 18 + 8 + 15 and 22 + 4 + 15 both total 41, so
the total-beside-the-parts discipline this file relies on **cannot see this
class of error at all** — the same coincidence-of-size that once hid a 49 where
the page carried an unrelated 51. Both moved numbers are named constants the
suite asserts (`SPAWNED_ENTRIES_THAT_MATCH` = 22,
`SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` = 4).)*

*(W6 Task 3's mover was
`valids/loop-with-subflow` — the only one of the twelve `Subflow` banks that
carries a sub-execution at all. Its `Loop` body IS a `Subflow`, so the body ran
and the `Subflow` inside it stalled: the whole entry hung on the one construct
that task shipped. `SPAWNED_ENTRIES_THAT_MATCH_BY_NAME` asserts the names — all
**twenty-two** of them.)*

Asserted, total beside the parts, by
`the_sub_executions_are_compared_and_this_is_what_they_cost`, which also asserts
the twenty-two **by name**, because `baseline.txt`'s header attributes them one
by one and got two of those attributions wrong. Beside it: **39 entries produce
a `spawned` at all** (35 until W6 Task 4's four replays;
`ENTRIES_WHERE_EGRET_PRODUCES_A_SPAWNED` is the constant, and the ordered-pairs
section above already said 39 while this sentence still said 35), where every entry produced an empty one for the whole of W3a
and W3b Tasks 1–4, and egret produces **126** ordered pairs inside `spawned`
where it produced 0. (The pair count does not move at W6 Task 4: a replay's
sub-execution holds a single run, and one run is no ordered pair.)

The **four** that differ only inside a sub-execution are
`invalids/loop-switch-failed`, `valids/finally-loop`,
`valids/finally-loop-parallel` and `valids/retry-flowable-parallel`. **This is a
different sentence from the one W3a's `39 of the 41 differ on nothing but
spawned` made**: that field was empty, and these four are filled and wrong
somewhere inside.

*(It was **eight** through W6 Task 3, and the four that left are the four
`valids/retry-new-execution-*` entries Task 4 banked. They were in this bucket
for the strongest version of "differs inside `spawned`": everything the parent
execution held agreed and the child was ABSENT, because nothing created a
replay. All four remaining are `Loop`s; `retry-flowable-parallel` is the one
whose name still reads like a replay's and is not — its `retry:` is a bare
`type: constant`, the default `RETRY_FAILED_TASK` behaviour, and exactly four of
the 345 corpus flows write `CREATE_NEW_EXECUTION`.)*

**`baseline.txt` said "Fifteen are the sub-execution hole closing" above sixteen
names, and W3b Task 5's report said "fifteen … and one needed only the render
surface".** Neither is right, and the fact that settles it is derived: all
sixteen of Task 5's banked entries carry a `spawned`, and this test names them.
None banked on the render surface alone. The same paragraph named **four**
spawner+render entries where Task 5's own table named **five**; the table is
right — `switch-in-concurrent-loop` renders a `Switch` inside a sub-execution
exactly as `loop-switch` does. Both are corrected in `baseline.txt`.

### The residual baseline, by construct and owner

**108 entries** — it read `111` until W8 Task 7, two banks behind, and `109`
until W9a Task 11 banked `valids/loop-values-from-uri` (a `Loop` over the ION
file a `core.storage.Write` wrote; the `Loop` row below fell 19 → 18 with it).
`the_residual_baseline_is_attributed_to_a_construct_or_named` derives the rows
from the baselined flows' own YAML on every run and fails if one moves; **the
rows overlap and this is not a partition**, but the remainder is exhaustive and
named. **The total is the one thing that test does not assert** — it `println!`s
`baseline.len()` — so this heading is prose sitting beside a derivation, which
is exactly the class W8 Task 7's sweep went looking for and the reason it was
found.

| baselined entries declaring it | construct | owner |
|---|---|---|
| 18 | `Loop` — a sub-execution egret **runs and gets wrong** | see below |
| 16 | `finally:` | **unclaimed** |
| 1 | `ExecutionUpdatableTask` (`Exit` left at kill-and-exit Task 3, 16 → 12; `Labels`, `SetVariables`, `UnsetVariables` at group C2 Task 5, 12 → 1) | group C2 Task 5 banked eleven; the one left, `valids/task-runif-taskswithstate`, is held by its flow-level `finally:` and not by its `SetVariables` |
| 5 | `AllowFailure` / `allowFailure:` → `WARNING` | **W8 Task 5 banked seven of its twelve**; the five that remain are held by the FLOWABLE and not by the flag — see the paragraph below |
| 3 | `Pause` → `PAUSED` | W8 — and **not one of the three is held by the wait**; see below |
| 10 | `afterExecution:` | **unclaimed** |
| 3 | `LoopUntil` / `WaitFor` | W8 — **and not one of the three is held by the loop** (eight until group C1 Task 2); see below. **8 → 3 at group C1 Task 2**: five of the six `OutputValues` ones banked |
| 3 | `sla:` | **W8 Task 6 banked two of the five**; the three that remain are `MAX_DURATION`s and **no implementation can bank them under this harness** — see the fifth blindness mechanism below |
| 4 | `WorkingDirectory` (planned opaque) | **unclaimed** |
| **29** | **none of the above** | named individually in `UNATTRIBUTED` (36 → 30 at group C1 Task 2: six entries held by `OutputValues` alone banked; 30 → 29 at group C1 Task 3: `valids/assert` banked) |

**The `LoopUntil` / `WaitFor` row is 9 → 8 at W8 Task 4, and the eight that
remain are held by three things, none of which is the construct.** SIX have an
`io.kestra.plugin.core.output.OutputValues` child and ~~`driver.rs`'s registry
ships four handlers, none of them that one — so their bodies cannot run at all,
which is the strict/upper-bound separator for this row rather than a failure of
it.~~ **(Group C1 Task 2, 2026-10-18: no longer true.** `driver.rs`'s registry
ships `OutputValues` now, and five of the six banked. The row is **3**:
`waitfor-child-task-warning` (one more `WaitFor` iteration than the golden
under `maxDuration: PT3S`, poller latency), `loopuntil-failed-flowable` and
`loop-until-restart`, each described below, and still none of them is held by
the loop.) (One of the six, `valids/waitfor-child-task-warning`, was held twice: its
other child is an `allowFailure: true` `Fail` whose run is `WARNING`, which was
the `AllowFailure` row's. **W8 Task 5 closed that half and the entry is still
baselined**, which is the measurement that turns *held twice* from a claim into
a fact: the `WARNING` child is byte-identical to the golden now and
`childTaskRunExecuted` lets it through, so the loop reaches the `OutputValues`
child for the first time and stalls there. The entry is reproduced strictly
further than it was.) `valids/loopuntil-failed-flowable` differs on ONE
field — its condition is `{{ outputs.loop.iterationCount < "5" }}`, Pebble
widens both operands of a relational operator to `Number` and throws on a
`String`, minijinja orders across kinds and answers `True`, so the loop succeeds
where the artifact failed the task; it is registered in
`STILL_OVER_ACCEPTED_AT_RUNTIME` with the four other renderer divergences.
`valids/loop-until-restart` differs on ONE field too — four iterations against a
golden of three, under `maxDuration: PT3S` with `interval: PT1S`, because the
golden's third iteration is past that bound only by the **artifact's
delay-poller latency**: its two `PT1S` pauses took **1.17 s and 1.92 s** on the
captured round, and `driver.rs::pass_time` moves the clock to the earliest
deadline and never further. The corroboration that this is a property of the
CONSTRUCT rather than of that entry: `valids/waitfor-max-duration`, the same
shape at `PT5S`/`PT0.2S`, is one of only **two** corpus flows whose normalised
trace Kestra ITSELF could not reproduce across five rounds, so it has no golden
entry and is excluded by name in `stability.json`. ~~**This is a sixth way the
oracle is blind, and it belongs beside the other five: a golden figure that is a
function of the artifact's SCHEDULER rather than of the flow.**~~ **W8 Task 6
fix round 1 DEMOTED that, and fix round 2 is what brought the correction here.**
It is not a sixth mechanism and not a category of its own: its cause is
mechanism 5, and what felt new is a value on a second axis — it is **effect 2**,
*the ratchet speaks and is wrong*, in the product table under *EFFECT is a
second axis*, roughly 500 lines below. **The blindness count stays at five.**
*(An ORDINAL survives the renaming of the concept it names: nothing that
searches for "mechanism 6" or for the concept's own words finds "a sixth way" or
"beside the other five", and both go false the moment the count moves. Sweep
ordinals and counts as their own population.)* Both entries now
carry a blocker row in `handler_surface.py` — `clock` and `render` — so the
ceiling does not name an entry nothing can bank.

**The `CREATE_NEW_EXECUTION` replay row was 4 and is GONE, which is the fourth
time this table has lost a row rather than lowered one.** W6 Task 4 banked all
four; `worker::replay` creates the replacement execution and the needle
`"CREATE_NEW_EXECUTION"` now matches no baselined entry at all. A draft of the
removal kept the row at 0 "because `valids/retry-flowable-parallel` is a fifth
flow declaring it" — **it declares no such thing**, its `retry:` is a bare
`type: constant` on a `Loop` and its behaviour is the default
`RETRY_FAILED_TASK`. Exactly four of the 345 corpus flows write the word.

**The `Subflow` row was 22 and is GONE, which is the third time this table has
lost a row rather than lowered one.** `Subflow` executes since W6 Task 3, so it
holds nothing — the same rule `disabled: true` and `runIf:` were removed under.
Twelve of the 22 banked in that commit; **three** of the remaining ten are
absorbed by rows that stayed (`Loop` once, `AllowFailure` twice) and the other
**seven** are named one by one in `UNATTRIBUTED`, with the cause
that actually holds each: two on `state::apply`'s attempt rule
(`valids/retry-subflow`, `valids/subflow-parent-retry`, where Kestra accumulates
a sub-execution's retries INSIDE the parent's attempt), two on a child-side
over-acceptance the parent now transmits (`valids/restart-parent`,
`valids/sla-parent-flow` — the second of those is W8 Task 6's, measured: its
child really is spawned, really does run its `Sleep`, and really does end
SUCCESS, which is the capped `Sleep` one level down and not an unresolvable
target), one on the unevaluated flow-level `outputs:`
(`valids/subflow-secret-output-parent`), and one on `OutputValues` having no
handler (`valids/task-runif` — whose `Subflow` is the corpus's only unresolvable
target and is SKIPPED by a falsy `runIf:`, so `Subflow` was never what held it),
and — the seventh, added by W8 Task 1 when the `flow concurrency:` row that had
been absorbing it was deleted — a second child-side over-acceptance,
`valids/flow-concurrency-sla-fail-parent`, whose child's `sla: MAX_DURATION`
egret does not evaluate.
**Lowering the row to 10 instead would have kept six entries attributed to a
construct that works**, which is the failure mode the rule exists to prevent.

**The `flow concurrency:` row was 7, then 6, and is GONE — the fifth time this
table has lost a row rather than lowered one, and the third time because a STATE
was credited to the wrong construct.** W6 Task 3 took it 7 → 6 by banking
`valids/flow-concurrency-parallel-subflow-kill-child`, already on the grounds
that its `concurrency:` was never a blocker for its own trace. W8 Task 1 found
the label itself false: `CANCELLED` is in three golden entries and **two are an
`sla:`'s and one is an `Exit`'s**, never a limit's, so the row was making the
construct look reachable *because the state really is there, for another
reason* — `disabled:`/`SKIPPED` exactly. Five of its six entries are counted
unmoved by rows that stayed (`Pause` ×2, `afterExecution:` ×2, `sla:` ×1) and
the sixth, `valids/flow-concurrency-sla-fail-parent`, is named in
`UNATTRIBUTED` with the cause that holds it: an `sla:` in its **child**.
The construct reaches **0** here for a reason with a mechanism — a serial
harvester, one self-contending flow already banked, and seven of thirteen
declarers already matching — written out under *States and constructs no W3a
task claims* above and derived by
`the_cancelled_state_never_comes_from_a_concurrency_limit`.

**The `Pause` row was 12, then 4, and is 3** — and it was the first row this
table LOWERED where the remainder is held by something other than the construct
the row names. W8 Task 3 banked eight and W8 Task 5 banked the ninth,
`valids/pause-timeout-allow-failure`, which is the entry this row was recorded
as over-claiming. `advance()` drives the wait, its bound and its resume, and the
three that remain each declare a *property* of `Pause` that
`executor::plan::pause_gap` refuses to plan, so the whole task stays an opaque
`TaskKind::Runnable` — `AllowFailure`'s and `WorkingDirectory`'s rule, applied
per task rather than per type:

| entry | what holds it |
|---|---|
| `valids/pause-behavior` | `behavior:`. **Not a defect and not about expressions** — fix round 2 measured the artifact honouring the value in every form (see *Drift at the execution seam*), so its `FAILED`/1 golden is `behavior: FAIL` working, `FAIL` being what `corpus.json` posts. This cell ended *"it banks when `RunState` gains `CANCELLED` too"*, and **W8 Task 6 measured that false**: the vocabulary is complete, and the entry did not move (that task's ratchet went 111 → 109 and both ids were `sla:`'s). What holds it is `pause_gap`, a PLAN decision — a fifth outcome the artifact produces for an out-of-enum value is a non-terminal HANG no variant can represent, and a DYNAMIC value cannot be classified at plan time at all, which is where `pause_gap` decides |
| `valids/pause_on_pause` | `onPause:`. Its golden records the sub-task as a run with `parent: null` — a **sibling** of the pause, `overlapping` it |
| `valids/pause-errors-finally-after-execution` | `errors:` on the `Pause`, plus `finally:` and a flow-level `afterExecution:` — two rows above that are **unclaimed**. Its pause run re-enters `RUNNING` (`CREATED RUNNING PAUSED RUNNING FAILED`) to drive the handler |
| ~~`valids/pause-timeout-allow-failure`~~ — **banked, W8 Task 5** | the only one of the four that WAS planned `TaskKind::Pause`, and it was held one layer out: `allowFailure: true` turns the timeout's `FAILED` into `WARNING`. That was the `AllowFailure` row's cause, counted here only because this table attributes by declaration. `ExecutorService.java:937` is the line — `.state(duration != null ? behavior.mapToState() : State.Type.fail(pauseTask))` — so the flags reach a `timeout:` bound and NOT a `pauseDuration:` one, which is why `valids/pause-delay` is untouched by them |

**The row is not deleted**, unlike the five below, and the difference is worth
stating: a deleted row means the construct holds nothing. `Pause` holds all
three of the entries left, in the narrow sense that a `Pause` property is what
stops each. **The one it over-claimed is gone**: this paragraph used to argue
that `pause-timeout-allow-failure` was counted here only because the table
attributes by declaration, and W8 Task 5 supplied the variant that banked it
rather than leaving the argument standing.

**The `Loop` row was 37, then 21, then 20, and is 19; the label moved with the
number.** The 20 → 19 is `valids/loop-with-subflow` at W6 Task 3.
It used to mean "a sub-execution egret asks for and cannot run"; W3b executes
them, so what is left is a sub-execution that runs and diverges. The 21 → 20 is
`valids/replay-loop`, which the sub-execution `inputs` fix banked. No single package owns
the residue and four separate causes are named for parts of it — W1's
`models::Loop` carrying neither `errors:` nor `finally:` (W3b Task 4), W2's
missing `is json` test (W3b Task 5), `advance()`'s unimplemented third `values:`
shape (an ION file URI), and the loop `outputs:` family. Nobody has been asked to
account for the row entry by entry, and this table should not be read as though
somebody had.

**`allowFailure` had two owners in two documents, and W8 is the right one.**
The programme spec §9 gives `allowFailure` to W8; this table gave its 12
baselined entries to **unclaimed**, through the `WARNING` row of *States and
constructs no W3a task claims*. One of the two had to be wrong, and it was this
one. W8 Task 1 corrected it here rather than lowering §9, and **W8 Task 5
settled it by measurement**: seven of the twelve bank.

**The estimate that came with the correction was "6 are bankable" and "six of
the twelve run to completion today and differ from the golden on one symbol".
Both halves are corrected here.** Measured by unbaselining all twelve and
reading the ratchet's own diff: **eight** run to completion, not six, and
**seven** bank. The twelfth of the eight,
`valids/waitfor-child-task-warning`, is the one this table already called *held
twice* — and W8 Task 5 turned that from a claim into a measurement. With the
flag implemented its `allow_failure` child is
`["CREATED","SUBMITTED","RUNNING","WARNING"]` over an attempt of
`["CREATED","RUNNING","FAILED"]`, byte-identical to the golden, and
`childTaskRunExecuted` lets it through — so the loop reaches its SECOND child
for the first time, an `io.kestra.plugin.core.output.OutputValues` with no
handler, and the execution stalls at `["CREATED","RUNNING"]`. It is reproduced
strictly further than before and still mismatches.

**The five that remain are the `AllowFailure` FLOWABLE's, and it is a different
construct from the flag.** `io.kestra.plugin.core.flow.AllowFailure` is a class
that `extends Sequential`; `build_plan` still plans it opaque, so its children
get no runs at all and all four of its declarers — `valids/allow-failure`,
`valids/allow-failure-with-retry`, `valids/finally-allowfailure` and
`valids/retry-with-flowable-errors` — stall in the flowable rather than differ
on a terminal. Two of the four are held by `finally:` as well, and
`valids/retry-with-flowable-errors` by `io.kestra.plugin.core.kv.Set`. The
fifth is `valids/waitfor-child-task-warning`, above. **This row's label is now
wrong for every entry left under it**, and it is kept rather than renamed
because the needle that derives it matches the flowable's FQCN and the flag's
literal alike; whoever implements the flowable should split the row rather than
lower it.

The `WARNING` population splits, and the split is derived rather than asserted:
**10 golden entries hold `WARNING` and 8 of them are the allow\* axis's** —
`valids/allow-failure`, `…-with-retry` and `valids/finally-allowfailure` on the
`AllowFailure` flowable; `valids/pause-timeout-allow-failure`,
`valids/task-allow-failure-executable-flow`, `…-flowable`, `…-runnable` and
`valids/waitfor-child-task-warning` on the `allowFailure:` flag. **The other two
are not W8's and stay unclaimed**: `valids/retry-success` (`warningOnRetry`
alone) and `valids/exit` (an `Exit`, the `ExecutionUpdatableTask` row's). So
`RunState::Warning` arriving banks neither of those two.

**The sentence that used to end this paragraph was wrong twice and W8 Task 5's
Step 1 is what falsified it.** It read *"and two of the eight need no new
variant at all — `allowWarning: true` maps `WARNING → SUCCESS`, so their golden
terminal is `SUCCESS`"*. (1) **None of the eight** needs no new variant: the
eight are by construction the golden entries that HOLD a `WARNING` symbol, and
an entry holding one cannot be reproduced by an enum that lacks it. (2) The
entries that need no new variant are **three**, they are
`valids/task-allow-warning-{runnable,flowable,executable-flow}`, and none of
them is in the eight — their goldens hold no `WARNING` at all. (3) The reason is
not that "the existing enum expresses the mapping". Measured on the pinned
artifact, `io.kestra.core.models.flows.State$Type.fail(Task)` is a
CONSTRUCTION out of two booleans with **no source state**: `isAllowFailure()`
false → `FAILED`, true + `isAllowWarning()` → `SUCCESS`, true alone →
`WARNING`, and `isAllowWarning()` is not read at all when `isAllowFailure()` is
false. There is no `WARNING` to map *from*. All three flows write BOTH flags,
the incoming state is `FAILED`, and `SUCCESS` is reached in one step with no
`WARNING` written to any history — which is exactly why
`valids/task-allow-warning-executable-flow`'s golden run reads
`["CREATED","RUNNING","FAILED","SUCCESS"]` with nothing between the two
terminals. See `src/executor/allow_failure.rs`'s module doc for the bytecode
and for the second spelling, which is a real conversion and is the only one
that can move a stored `WARNING`.

**`disabled: true` and `runIf:` are not rows here and both are implemented.** A
row is an attribution to the thing that *holds* an entry, not to a construct the
flow happens to declare — the same rule that adds a row when a cause is found,
run backwards.

**The named remainder went from 13 to 29, to 35 at W6 Task 3, and to 36 at W8
Task 1** (that last one is `valids/flow-concurrency-sla-fail-parent`, moved out
of the deleted `flow concurrency:` row; the six before it were moved out of the
retired `Subflow` row rather than newly discovered), and
16 of the 18 names the 13 → 29 added are W3b
Task 5's: eleven entries whose task type has no handler at all (`OutputValues`
×6, `PurgeLogs` ×5, `Assert` ×1 — measured in advance by W3b Task 1) and five
more a fed terminal had been hiding, four of which are W2 divergences that only a
real handler rendering a real property can reach (`errorLogs()`, `render()`,
`??` over an undefined *root*, and integer division by zero rendering
`Infinity`), the fifth being a sibling still in flight when its `Parallel` failed
— W4 Task 3 wired `TaskHandler::cancel`, and deliberately not to that seam: the
kill path fires on a task's own `timeout:` expiring, while a run a **flowable**
abandoned is `advance()`'s decision and `advance()` has no worker to tell.

`valids/full`'s entry in that list is a correction: it was attributed to
flow-level `outputs:` and `variables:`, and the flow has **neither** (W3b Task 1
Concern 3). It differs on the happens-before relation alone, the same cause as
`valids/dag`, `valids/if` and `valids/parallel-disabled-tasks`.

### W4 — the ratchet did not move once in five tasks, and that is five facts

`143` before W4 Task 1 and `143` at this commit, and every task in between
measured it and reported no movement. **A number that never moves is the hardest
thing to read honestly**, because "nothing happened" and "everything that
happened was invisible here" print identically. This section is what separates
them, and it is a W4 section rather than a W4 paragraph because the five tasks
produced **six different facts** and one number:

1. the **retry** hole is hidden by the *harness* — this oracle does the
   product's job for it;
2. the **timeout** hole is hidden by the *corpus* — the predicate fires four
   times and no budget is short enough to expire;
3. the **kill** hole is hidden by being *downstream of the timeout*, so its
   predicate is never evaluated at all;
4. three entries were **bankable and declined**, which is not invisibility;
5. the **runner** reaches zero entries and rests entirely on its own tests;
6. and **nothing in this workspace runs this worker at all** — `worker::tick`
   has no production caller, so `143` is a statement about a state machine and
   a driver and not about a deployment. That is item 15 below, and it bounds
   every one of the five above. *(**CLOSED at W10 Task 2**, 2026-09-15:
   `runtime::spawn` is the claim loop, `egret-bin` starts it under `[workflow]
   worker_enabled`, and `worker::tick` has a production caller. The fact is kept
   as W4's record because it was true for the whole of W4, W6 and W8; what
   replaces it as a bound is in item 15 and in the W10 section, and it is
   **narrower but not empty** — `143` was never about a deployment, and `109`
   still is not, for a reason that survives the wiring.)*

Only the first three are "the ratchet cannot see this". The fourth is a
decision, the fifth is a population that does not exist, and **the sixth is the
one to read first**: it was the fifteenth of seventeen items in a list six
hundred lines below this section when W4 Task 6 first wrote it, and a reader who
reads exactly the section this document points them at would have finished it
believing `143` bounds a shipping worker.

| | ratchet | what the task shipped |
|---|---|---|
| package base `ad93ef2d` (W3b, on `main`) | 143 mismatching, 143 baselined, 0 broken | — |
| Task 1 | 143 | `claim::due_retries` + `claim::wake`, called by `tick` ahead of the claim |
| Task 2 | 143 | `worker::timeout` — a per-task budget rendered from the frozen source and enforced around `Registry::run` |
| Task 3 | 143 | `TaskHandler::cancel` wired to the budget's expiry |
| Task 4 | 143 | nothing: a **ruling** that one handler result stays one transaction |
| Task 5 | 143 | `worker::commands`, `egret.container`, `egret.script.python` |

Measured before and after on each task, and Task 2's "before" was taken on
`d7233cf0` — the commit after Task 1 — with the branch stashed and
`egret_test_template` dropped between the two runs.

**`src/executor/` is byte-identical to the package base**, which is the Global
Constraint W4 inherited and the reason every number above is about a worker
rather than about a state machine: all **11** files under
`crates/egret-workflow/src/executor/` have the same blob hash at `HEAD` as at
`ad93ef2d`, verified per file rather than by a `git diff` whose empty output
also means "nothing was checked" (`git diff --stat ad93ef2d HEAD --
crates/egret-workflow/src/executor/` is empty too, and the file count is 11 on
both sides, so nothing was added or deleted either). W3b's own diff against
*its* base was 16 lines in 2 files — see item 12 — so this is a stronger
statement than the one it inherits, not the same one.

**`baseline.txt` is NOT byte-unchanged since `ad93ef2d`, and a review said it
was.** The correction is recorded here because the claim was true in substance
and false as stated, which is what this whole sweep has been about. Verified:

```text
blob ad93ef2d 4de4d0560c49edd45e07331b7f8e253260a80dee
blob HEAD     8a40d34f8cc12738a11415a0a4b2dee99a214d8c   ← different
git diff --stat ad93ef2d HEAD -- oracle-exec/baseline.txt
  1 file changed, 6 insertions(+), 2 deletions(-)        ← b1a47222, W4 Task 3
changed lines that are NOT comments                       0
grep -vc '^\s*#\|^\s*$'  at ad93ef2d 143 · at HEAD 143  ← the ratchet is unaffected
```

`b1a47222` rewrote a comment from 2 lines to 6. **Every** changed line begins
with `#`, so the 143 baselined entries are identical on both sides and every
figure resting on them stands. The right claim is *"143 is unchanged and every
edit to this file has been to comments"*, which a `git diff` can be asked for —
not *"the file is byte-unchanged"*, which is simply false and which nobody
checked because the number it protects was right.

#### The three holes W4 closed, and the three different mechanisms that hide them

Each row's population is derived on every run by the named test; the *reason*
column is the part that matters, because all three rows end in the same verdict
and only one of them ends there for the reason a reader would guess.

| hole | the population, derived | is the predicate reached? | why nothing can move |
|---|---|---|---|
| **`retry:`** — the waker (Task 1) | **25** golden entries declare a `retry:` = **8 baselined + 17 matching** since W8 Task 5 (10 + 15 after W6 Task 4, 14 + 11 before it); all **8** remaining baselined ones are `RETRY_FAILED_TASK`, because the **4** `CREATE_NEW_EXECUTION` entries banked at W6 Task 4 and `valids/task-allow-{warning,failure}-runnable` banked at W8 Task 5 — the first `retry:` entries a bank has moved to the matching side since. Derived by `the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them`, which asserts `(8, 17, 25)` and names all eight. *(This cell read "of the 14, 4 are `CREATE_NEW_EXECUTION`, which writes no `not_before` at all". That is now false in both halves: that arm emits the `ExecutionDelay` the artifact builds for both behaviours, and `due_retries` has a `RETRIED` arm to read it. It then read `10 baselined + 15 matching` for two more tasks.)* | **Yes, constantly.** `tick` calls `wake` unconditionally, ahead of the claim, so `due_retries`' `SELECT` runs on every tick of all 294 entries — and returns **0 rows on the retry population** every time | **The oracle contains a second waker, and it wins.** `driver::pass_time` is the only thing here that makes a deadline elapse, and it does not merely move the clock: it rewinds `executions.created_at` by the shortfall **and calls `worker::resume` itself**, in the same function. So a `RETRYING` run is due only *inside* the call that has already resumed it, and no tick ever sees one. **`driver.rs`'s own module doc said this all along** — *"it changes one timestamp and calls the same `resume` any other caller would"* — and the package spent five tasks writing that the rewind alone was the mechanism. Instrumented `wake`: **0** fires, with `tests/worker.rs` as the positive control on the same build — a W4-era reading over a predicate whose `RETRYING`/`RETRIED` arms are byte-identical today, which is what carries it forward. **The flat "0 fires over the whole oracle" this cell used to end with is false since W8 Task 7** — the `'PAUSED'` arm it added gives the oracle a second door to the same `resume`, and the instrument reads 1 fire in one of five runs and 0 in the other four, ratchet 109 in all five. See the W8 section |
| **`timeout:`** (Task 2) | **5** golden entries declare one. **4** are baselined; the **1** matching is `invalids/duplicate-parallel`, a flow Kestra *refused* — so it has no runs at all, and the reportable half of this population is empty for a reason nothing can change. *(W4's spec §1.1 says **4** for this row and is right over **its** population, the baseline; this table counts the golden. Both now name the population they count over, because a reader holding the two documents open sees a contradiction where there is a scope difference — and this file's whole subject is figures that are true under a scope nobody wrote down.)* | **Yes, exactly four times.** Instrumented `worker::timeout::guard` over all 294 entries: the budget was applied `1s`, `1s`, `250ms`, `3600s` — precisely the four declarations — and **expired 0 times** | **No corpus budget is short enough to fire.** Three of the four sit on a `Pause`, a flowable whose `timeout:` is `advance()`'s and which Kestra's own worker ignores (measured on the artifact: a `Sequential` carrying `timeout: PT1S` around a four-second `Sleep` ran to SUCCESS in 4.2 s); the fourth is an **hour** on a `core.log.Log` |
| **kill** (Task 3) | **5** baselined ids contain `kill`, and **0** of them declare a `timeout:` anywhere in any of `Flow`'s four task lists | **No, zero times.** `Registry::cancel` hangs off the budget expiring, and the row above is why that never happens | **The five named ids are not this path's population at all**, and their own YAML says what they are: `exit-killed` is `Exit` carrying `state: KILLED`, an author-declared terminal rather than a kill anybody performs; `pause-kill-delay` is a bare `Pause`; `flow-concurrency-queue-killed-after-execution` and `flow-concurrency-parallel-subflow-kill-child` declare `concurrency: {behavior: QUEUE, limit: 1}`; and `flow-concurrency-parallel-subflow-kill` declares two `Subflow`s and no `concurrency:` of its own — it is named for what happens to its child. Four of the five are W8 constructs and the fifth is an output. The one runtime kill this corpus *does* hold — `parallel-fail-with-flowable`'s `Sleep`, abandoned by a failed `Parallel` — is `advance()`'s decision, and `advance()` has no worker to tell |

**Two of these three would be lost by merging them**, which is why the middle
column exists. Task 1's predicate is evaluated thousands of times and selects
nothing, because the instrument that makes this oracle fast is doing the
product's job for it. Task 2's is evaluated exactly as many times as there are
declarations, and no declaration is small enough to fire. Task 3's is not
evaluated at all, because it is downstream of Task 2's. *Same verdict, three
mechanisms* — and only the first is a statement about the harness, only the
second is a statement about the corpus, and only the third is a statement about
this branch's own wiring.

**What the ratchet is *not* silent about is retrying itself.** Eleven *matching*
entries declare a `retry:`, all eleven with the behaviour that writes a
`not_before`, and a change on a matching entry is one this ratchet reports —
so `ScheduleRetry`, the `RETRYING` state and `reopen_due` putting a run back are
covered by 11 entries the baselining argument does not disarm. (Whether each of
the eleven *reaches* a failure and schedules a delay is not derived here; the
declaration is.) The hole is the **waker**, not the retry.
`the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them` prints
all of it with every total beside its parts — `294 = 25 declaring + 264 not +
5 this crate refuses at parse`, `25 = 14 + 11`, `14 = 4 + 10` — and it
exists because the sentence it replaces — *"14 baselined entries declare a
`retry:` … which is derived rather than assumed"* — was written into this
statement by W4 Task 1 **with nothing deriving it**. The 14 was right. Nothing
in this crate counted it until now, and the words "derived rather than assumed"
are exactly the words that stop a reader checking.

#### Three entries were bankable, and were refused — which is not the same as invisible

Every hole above is something this ratchet *cannot* see. **W4 Task 4's three
entries are something it can see and was told not to take**, and a coverage
statement that does not separate those two is worth less than no statement.

`valids/dag`, `valids/full` and `valids/parallel-disabled-tasks` bank under a
worker that applies a whole claimed batch's results before calling `advance()`:
**143 → 140, no regression in either direction**, measured on the prototype and
recorded raw in `w4t4-widened-tick.txt`. The ruling refused it, and **not for
its yield**. The binding cost is that `persist`'s four-mutation table proves the
`outputs` fence and the state fence are *alternatives*, each sufficient alone,
against a transaction that releases exactly **one** lease; widened, it releases
`n`, and RUN 4 puts a defect in the surviving alternative that only bites at
`n > 1` and watches the whole crate stay green — `a_worker_whose_lease_was_reaped_cannot_commit`
and `a_tick_killed_between_the_handler_and_the_commit_applies_nothing` included,
because both drive `apply_outcome` with a single `Claimed`. So the trade is
three entries against two durability guards that would stop being guards. See
*One result per transaction is a ruling — W4 Task 4* for all four costs and the
step it buys. **The way back is named rather than left open**: a savepoint per
run inside one transaction would give the interleaving without giving up the
one-lease-per-commit shape the fence table is measured against. Nobody has built
it; this file's job is to make sure the 143 is not read as though nothing were
available.

#### Nothing runs this worker, and that bounds all five facts above

> **RETIRED — false since W10 Task 2, annotated at W10 Task 7, 2026-09-15.**
> Every sentence in this subsection was
> true when W4 Task 6 wrote it and is false now: `worker::tick` has a production
> caller (`crate::runtime::spawn`), `worker::reap` has one in the same file,
> `worker::start_execution` has three in `routes.rs` and `webhook_routes.rs`,
> and `egret-bin` starts the claim loop. **The subsection is kept rather than
> rewritten**, because it is W4's record of what W4 measured and because the
> proposition in it is the one this file then repeated across its own sections
> and into `runnable.txt` — which is the finding W10 Task 7 exists for. The current derivation, the
> current call-site count and the bound that *replaces* this one are in item 15
> and in the W10 section.

`worker::tick` has **no production caller**. A grep for `tick(`, `reap(` and
`start_execution(` over every `crates/*/src` finds no call site; the four files
that call them are `oracle-exec/driver.rs`, `tests/worker.rs`,
`tests/executor.rs` and `tests/sql_starrocks_live.rs` — and `tests/executor.rs`
does it through `#[path = "../oracle-exec/driver.rs"] mod driver;`, so the call
is in the compiled binary rather than in that file's own text. `egret-bin`
builds a `WorkflowState` — the flow repo and its refs, so flows can be authored,
published and read over HTTP — and no claim loop.

So the reaper, the lease fence, the waker and the budget have never met an
operator's `working_timeout_s`. **The three holes above are real defects that
were really closed**, and every durability property in item 4 is really proved —
of code that no deployment executes. Item 15 has the full derivation, and
`worker::claim`'s doc for `wake` now carries it too, because that doc said
`tick` calling the waker *"is what gives it a production caller"* and that is
the sentence a reader would quote as evidence the waker is wired. It is wired to
`tick`; nothing is wired to `tick`.

This is not W4's to fix and it is not a defect in anything W4 shipped. It is the
scope of the number.

#### The runner reaches zero entries, and its own tests are the whole instrument

W4 Task 5's handlers are further outside this oracle than `egret.sql` is, and
the count that says so is in item 3b. What stands in for the ratchet there is a
mutation table of **29 rows — 29 compiled, 29 killed, 0 survivors, 70 killers,
27 of the 29 killable with no daemon** — derived from a single run of the
harness against `136309cd` and, after a defect in the *renderer* let a
non-compiling row ship as `compiles ✓`, computed from the **rendered text**
rather than from the arithmetic behind it. The two rows that need a daemon are
the two questions no pure function can answer: *was a process spawned*, and
*does the daemon agree which container this worker owns*.

### W6 — the ratchet moved sixteen, and the four triggers are not among them

`143` before W6 Task 1 and **`127`** at this commit: the first movement since
W3b, and the first time in this programme that a coverage statement has had to
separate *what a bank proves* from *what shipped beside it*. **That makes the
risk this section exists for sharper rather than smaller.** W4's problem was
that a number which never moves reads as "nothing happened"; W6's is the
opposite — **sixteen banked entries can make four triggers look measured when
none of them is.** And it really is half by volume, not a figure of speech:
**12,274 of the 23,267 lines this package had added under `crates/` at
`b4e1c368`** are `src/trigger/`, `src/webhook_routes.rs` and the two webhook
route suites — **53%** of the package, judged by nothing in this file.
*(`git diff --numstat cc4ad991 b4e1c368 -- crates/`, added lines. **The
denominator is dated on purpose**: written as `cc4ad991..HEAD` it counted the
file it lives in, so every later edit to this README falsified it — the
whole-branch review caught it one commit after it was written, at 23,273. A
figure that counts its own file is a shape "hunt the number whose checker is
elsewhere" does not cover, and it is the only one this sweep created rather than
found. **The numerator was claimed exact and not moving, and that was the same trap
one half further in** — `82fd8ba9` edits `webhook_routes.rs`, which is one of
the four numerator paths, so the numerator moves whenever this package touches
its own uncovered half. Both halves dated together, **at `82fd8ba9`**:
numerator **12,279** = `src/trigger/*` + `webhook_routes.rs` **1,215** +
`tests/webhook_route.rs` + `egret-server/tests/workflow_webhook_mount.rs`;
denominator **23,740**; **52%**. Dating the denominator and leaving the
numerator undated in the same sentence is what let it through: **a sentence
escapes the self-reference trap only if EVERY figure in it is dated, and the
ratio is the only part worth quoting undated, because both halves move
together.**

**And the third instance is this paragraph.** The correction above shipped
labelled *"measured at `a20f5e72`"*, and `12,279 / 23,740` are `82fd8ba9`'s
figures — at `a20f5e72` they are `12,274 / 23,273`. Re-measured at five commits
to find that: `b4e1c368` 12,274/23,267 · `a20f5e72` 12,274/23,273 · `ef943cf2`
12,274/23,585 · `82fd8ba9` 12,279/23,740 · `a8bb1da6` 12,279/**23,754** — and
the last of those moved *because that commit edits this file*, which is the rule
demonstrating itself for the third time in one paragraph. **A date is a figure
too, and it needs the same treatment as the number it labels.** The ratio has
been 52–53% at every one of the five, which is why it is the only part worth
quoting. A line count is a crude measure of anything and is used here only
to say that the uncovered half is not a corner.)*

Every figure in this section is either derived by a named test on every run or
labelled as a measurement someone took. The derivations are
`the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none` and
`the_seeded_correlation_id_divergence_has_no_corpus_population` in
`tests/executor.rs`; both were **validated by injecting a known defect before
their zeros were believed** (five defects, each killed on 3 of 3 runs, against a
3-of-3 control on the unmutated binary — and the first form of one of them
**did not compile**, which looks exactly like a mutation no test killed until
you check).

| | ratchet | what the task shipped |
|---|---|---|
| package base `cc4ad991` (W4's tip, on `main`) | 143 mismatching, 143 baselined, 0 broken | — |
| Task 1 | 143 | the driver publishes the corpus into `workflow.flows` |
| Task 2 | 143 | migration `0138` — three kinds of child execution, four CHECKs |
| Task 3 | **131** | `Subflow` executes; `src/executor/` opens for the first time since W3a |
| Task 4 | **127** | the `CREATE_NEW_EXECUTION` replay; migration `0139`'s metadata |
| Task 5 | 127 | the schedule trigger, its cron engine and backfill |
| Task 6 | 127 | the flow trigger |
| Task 7 | 127 | the webhook trigger, its router and the lever that keeps it off |

#### The sixteen, and the mechanism of each

**16 = 12 + 4**, and both halves are derived from the corpus rather than listed:
the twelve are *golden entries declaring a `Subflow`, verdict `runs`, matching*,
and the four are *golden entries whose source writes `CREATE_NEW_EXECUTION`*.

| | count | the mechanism that banked it |
|---|---|---|
| `Subflow` | **12** | `Subflow` was planned an opaque `TaskKind::Runnable`, which set `task_runs.type_id`, which made it claimable — and `claim::leaf` answered `None`, because `models::TaskKind` for the same task is not a `Leaf`. `WorkerError::NotALeaf` aborted the tick and every one of them stalled at that run. It is a `TaskKind::Executable` now: nothing claims it, and `worker::subflow` spawns the child and terminates the parent's run. The child is linked by `trigger_execution_id`, **not** `parent_execution_id`, because `driver.rs` builds `spawned` from the latter and all 41 `spawned`-carrying entries have zero `Subflow` children |
| `CREATE_NEW_EXECUTION` replay | **4** | Migration `0138` gave the replay a representable row at Task 2 and it banked nothing; what was missing was the two facts that **END** a chain — `execution.getMetadata()`'s `attemptNumber` and `originalCreatedDate`, which `advance()` read as the constants `(1, Tick(0))`. True of a chain's first execution and false of every replay, so nothing could refuse the fourth. `0139` stores them, `retry::scan` emits the `ExecutionDelay` the artifact builds for both retry behaviours, and `worker::replay` creates the execution |

**The attribution of the twelve is a running program and not a list** — W6 Task
3 mapped `Subflow` back to `TaskKind::Runnable`, one line in `plan.rs`, and the
ratchet returned to **143 reporting exactly those twelve as regressions**. That
is the strongest form of attribution this programme has produced, and it is a
measurement that task took rather than anything this suite re-runs.

**The trap in the derivation, asserted rather than avoided.** *Matching ∧
declares a `Subflow`* is **thirteen**, not twelve: `invalids/recursive-flow` is a
flow Kestra **refused**, egret refuses it too, and it has matched since long
before this package. The test asserts that thirteenth **separately, by name**,
so the twelve cannot quietly absorb it. `23 = 12 + 1 + 10`: twenty-three golden
entries declare a `Subflow`, and ten are still baselined — the design spec's
"22 declarations" was the **baselined** population before the package
(`22 = 12 + 10`), a different set from this one.

#### The constructs the ratchet cannot speak for, and the blindness is FIVE mechanisms

W4 established that *"the oracle cannot see it"* is at least three distinct
facts. W6/W7 made it four over the triggers. **W8 Task 3 adds a fifth that is
not a trigger at all**, which is why the heading no longer counts constructs:
the mechanisms are the subject and the constructs are the examples. **They must
not be merged** — merging them would lose the only thing a reader can act on,
which is what a future package would have to change to make each one visible.

**Count the MECHANISMS, not the rows.** W8 Task 3 fix round 3 adds a sixth row,
*an out-of-enum `behavior:`*, and it is still five mechanisms: that row shares
`backfill`'s — **no population** — and is listed for what it would otherwise
license somebody to over-claim, not because it is a new way of being blind.
**W8 Task 6 adds a seventh row, `sla: MAX_DURATION`, and it is still five
mechanisms**: it shares the fifth — *the harness compresses time* — with the
unbounded `Pause`. But it shares the mechanism and **not the effect**, and that
is what the row is for; see the paragraph under the table.

| construct | corpus | golden entries | what the ratchet holds | **the mechanism of the blindness** |
|---|---|---|---|---|
| **webhook** | 15 flows, 15 declarations | 14 — 11 matching, 3 baselined | nothing | **The harness never sends the request.** `harvest.py` contains the string `executions/webhook` **exactly once**, and that once is the *reason* `valids/webhook-inputs` is excluded from the corpus ("Executed only through POST /executions/webhook/…"). The trigger is not invisible to a discovery walk; it is **never invoked**, and the one corpus flow that can only be started that way has **no golden entry at all** |
| **schedule / cron** | 7 flows, 8 declarations | 6 — all 6 matching | nothing | **The execution exists and hangs off nothing.** `docker compose` runs `command: server standalone`, so the scheduler is live and fires it — the design spec's §1.1 measured **13** executions on the artifact of which **10** came from something other than the harness's POST, six of them from a `Schedule` nobody asked for. The execution it creates carries `parentId` NULL **and no trigger link either** (measured there, not here). `descendants()` walks the generated `parent_id` column transitively **from the one root the harness posted**, so a schedule-fired execution is not a descendant of anything and cannot be attributed to an entry even in principle. The `trigger`-link union the flow-trigger row needs **would not find this one** |
| **flow trigger** | 17 flows, 21 declarations | 12 — all 12 matching | nothing | **The link exists and is not walked.** A `Flow`-fired execution records its cause in `trigger.variables.executionId` — the same place a subflow child's link lives, and the field egret's own `trigger_execution_id` column stands in for — and **no arm of `descendants()` reads it**: same verdict as the row above, opposite cause. This is the one of the four a `descendants()` union would close, and two things about that cost are worth writing down: it is a **JSON path** rather than a column swap unless Kestra generates a column for it (`parent_id` is generated and this file has never named a second — **unmeasured**), and it would still need `normalise.py`, which builds `by_parent` from `parentId` and `bail`s on a descendant that does not hang off the root, so a *discovered* trigger-fired execution could not be normalised into a trace |
| **backfill** | **0 live declarations**; 2 commented-out lines, in `valids/full` and `valids/labels-deserialization` | 0 | nothing | **No population.** Not a blind instrument — nothing to be blind to. The two commented lines are the scanner's own positive control: it finds what *is* there, so the zero is a measurement rather than a reader that sees nothing |
| **an unbounded `Pause`** (W8 Task 3) | 7 flows: `pause-test`, `pause_on_resume`, `pause_on_resume_optional`, `loop-pause`, `resume-validate`, `pause_no_tasks`, `follow-dependencies-partial-parent` | **0** | nothing | **The ARTIFACT never finished, so the harvest excluded the flows.** Not a link nobody walks and not an empty population: each of the seven has an `execution_id` in `corpus.json` and a disposition saying it "reached PAUSED and stayed there for the full 90s", because a `Pause` with neither `pauseDuration:` nor `timeout:` ends only on an explicit resume and a standalone harvest issues none. Kestra's own tests drive these with `runOneUntilPaused(...)` then `executionService.resume(...)`. So the golden has no entry to compare and **an egret that resumed such a pause by itself would leave every figure in `baseline.txt` unchanged** — the one over-acceptance in this package that is invisible in both directions. What a future package would change: teach the harvest to issue a resume, which means `harvest.py` gaining a second verb and `normalise.py` a way to record when it was issued |
| **`sla: MAX_DURATION`** (W8 Task 6) | 4 flows declare one (`sla-max-duration-fail`, `sla-subflow`, `flow-concurrency-sla-fail-child`, `sla-max-duration-ok`); **2 more entries are held by one in a different flow** (`sla-parent-flow`, `flow-concurrency-sla-fail-parent`, each calling a `Subflow` whose target declares it) | **6 — 1 matching, 5 baselined** | **the five baselined ones, for as long as the driver caps its `Sleep`.** The ratchet compares them, they mismatch, and no implementation can change that **without changing the harness** — which is effect 3, not effect 2, and is why the word is not "permanently" | **The harness compresses time, and the WINDOW DOES NOT EXIST.** `MaxDurationSLA.evaluate` is `Duration.between(execution.startDate, Instant.now()) > duration`, so it needs an interval in which the execution is non-terminal and older than the bound. `driver::registry()` is built with `Sleep::capped(Duration::ZERO)`: `Sleep::run` is synchronous inside one `worker::tick` and writes **no** `not_before_ms`, so the only task these flows have reaches its terminal inside the tick that started it. **The clock does move in this harness** — `pass_time` rewinds `executions.created_at` by `shortfall + 1` and calls `worker::resume`, landing 1 ms past every deadline — but its query selects on `r.not_before_ms IS NOT NULL`, so with no deadline registered it returns `Ok(false)` the first time it is asked and the loop breaks. Measured by `the_max_duration_window_does_not_exist_in_this_harness` **over the four DECLARERS** — the two `*-parent` entries are not in it and their sentence is different, see below: **0 deadlines** across all four, against a control of **28 of 269 driven flows** that do register one; elapsed at the last run write 14.4 / 22.3 / 22.0 / 14.1 ms against bounds of 500 / 1000 / 3000 / 10000 ms; and all six `sleep` runs read `["CREATED","SUBMITTED","RUNNING","SUCCESS"]`, so no run of a DECLARER is ever in flight across a step. **The two parents have a different and stronger reason, and the generalisation to all five was W8 Task 6's error**: their own `Subflow` run necessarily DOES span steps, and what stops the clock there is that `pass_time`'s `WITH RECURSIVE` joins `parent_execution_id` while a subflow child is linked by `trigger_execution_id` — **the clock-advance walk cannot reach the child at all**, which would hold even if the child registered a deadline. `the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded` runs that exact CTE and asserts it returns only the parent. **What a future package would change**: uncap the driver's `Sleep` — `PT10M` on `flow-concurrency-sla-fail-child` alone, `PT30S` three more times — AND give egret a monitor keyed on wall-clock time, which `advance()` (pure over `(plan, state, now)`, no I/O and no clock) cannot host and the driver's `tick`/`pass_time` loop has no analogue of |
| **an out-of-enum `behavior:`** (W8 Task 3 fix round 3) | **0 corpus flows**; measured on probes only | 0 | nothing | **No population, and the behaviour is a HANG rather than a state.** A `Pause` whose `behavior:` renders to something outside `RESUME`/`WARN`/`FAIL`/`CANCEL` -- `NOPE`, lower-case `fail`, an undefined `{{inputs.nosuch}}` -- leaves the execution `RUNNING` past 120 s, non-terminal, with the pause run still `RUNNING`. Every one of the four enum values terminates, and terminates differently (`FAILED`/1, `SUCCESS`/2, `WARNING`/2, `CANCELLED`/1). The corpus's single `Pause` `behavior:` posts `FAIL`, so nothing here can score this edge; it is listed because the sentence *"the artifact honours the value"* is true **inside the enum only**, and W8 Task 3 fix round 2 wrote it without that qualifier from a grid whose every cell was a valid constant |

#### EFFECT is a second axis, orthogonal to mechanism — and it demotes the sixth

**W8 Task 6 found this and stated it too narrowly; fix round 1 records the
product rather than the sentence.** The task wrote *"the fifth mechanism has
three effects"*. It has, but **effect is not a property of mechanism 5** — it is
an independent axis, and writing it as a property of one mechanism hides that
the same three values classify every row in the table above.

The axis is the product of two questions:

* **does a golden entry exist?** — which decides whether the ratchet can speak
  at all, and therefore whether an over-acceptance is invisible in **both**
  directions or only in one;
* **where does the irreproducibility live?** — in the **golden** (the captured
  value is not a function of the flow), in the **harness** (the golden is fine
  and the harness will not produce the input), or in **egret** (both sides are
  fine and egret's own answer is wrong — the ordinary baselined entry).

| | irreproducibility in the GOLDEN | in the HARNESS | in EGRET |
|---|---|---|---|
| **no golden entry** | — *(empty: with no entry there is nothing to be a function of)* | **effect 1 — the ratchet cannot speak.** An unbounded `Pause`; the webhook that is never POSTed; `backfill`'s empty population. Over-acceptance invisible in **both** directions. **Act on**: make the harness produce the input | — *(empty: nothing to compare egret against)* |
| **a golden entry exists** | **effect 2 — the ratchet speaks and is wrong.** `valids/loop-until-restart`. **Act on**: nothing; the golden is the thing that cannot be reproduced | **effect 3 — it speaks, is right about Kestra, and egret's side is deleted by the harness.** `sla: MAX_DURATION`. Over-acceptance visible in **one** direction. **Act on**: repair the harness — uncap `driver.rs`'s `Sleep`, then build the monitor | the ordinary baselined entry: implement it |

**Applied outward, the table reclassifies two things the file already had.**

* **Mechanism 4 — *the harness never performs the out-of-band action*** (the
  webhook) lands squarely on **effect 1**: no entry, invisible both ways, and
  the repair is *make the harness do it*. It is the same cell as the unbounded
  `Pause`, reached by a different mechanism. That is the point of a product:
  mechanism says *why the input is missing*, effect says *what the ratchet can
  therefore do*, and they vary independently.
* **The "sixth mechanism" is not a mechanism at all — it IS effect 2**, and is
  demoted here. It was introduced as *the golden encodes the artifact's own
  scheduler*, which is a **cause** already covered (mechanism 5, the harness and
  the artifact disagreeing about time); what made it feel new was that the
  ratchet spoke and was wrong, and that is a value on this axis rather than a
  sixth way of being blind. The blindness count stays at **five**.

**The rule this yields, and it is the durable part**: a refusal must name the
mechanism **and** the effect. *"The oracle cannot see it"* is five mechanisms;
*"the harness compresses time"* is one mechanism whose entries land in three
different cells; and the only sentence a later reader can act on is the one that
says which cell.

#### A LATENCY-DEPENDENT GOLDEN — not a sixth mechanism, and since W8 Task 6
#### fix round 1 not a category either: it is **effect 2**

**W8 Task 4 fix round 1 rules on this, and the ruling is sharper than the one
the task first wrote.** Every mechanism in the table above is a reason the
ratchet **cannot speak** for a construct: no population, no request, no link, no
golden entry. `valids/loop-until-restart` is the opposite — **the ratchet speaks
and is wrong.** The golden entry exists, is compared, and **its value is a
function of the artifact's SCHEDULER rather than of the flow**, so a correct
implementation is baselined against it for ever.

Its **cause** is the fifth mechanism's — the harness compresses time — and its
**effect is the opposite of the unbounded `Pause`'s**: there, a flow the
artifact never finished produced no golden entry at all and an over-acceptance
was invisible in both directions; here a golden entry exists and **permanently
cannot be reproduced by any implementation**. (W8 Task 6 adds a THIRD effect to
the same mechanism and the table above separates all three; this one is the
only one where *there is nothing to fix* is the right answer.) **Fix round 1
demotes this section's own claim to a cell**: *"a different category"* was
right that it is not a mechanism and wrong that it is a category of its own. It
is the (golden entry exists × irreproducibility in the GOLDEN) cell of the
product above, and the reason it is the only member is that this corpus has one
latency-dependent golden, not that the cell is special. Calling it a sixth blindness would file an
unbankable entry under "the ratchet cannot see it" and lose the only thing a
reader can act on, which is that **there is nothing to fix**. Its `LoopUntil` is bounded by
`maxDuration: PT3S` with `interval: PT1S`, Kestra's `reachedMaximums` compares
the loop taskrun's creation plus the bound against `Instant.now()`, and on the
captured round its two `PT1S` pauses took **1.17 s and 1.92 s** — so its third
iteration ended at T+3.33 s and was past the bound. `driver.rs::pass_time` moves
the clock to the earliest deadline and never further, so egret's third iteration
ends near T+2.05 s and a fourth runs. **The corroboration that this is the
construct and not the entry**: `valids/waitfor-max-duration`, the same shape at
`PT5S`/`PT0.2S`, is one of only **two** corpus flows whose normalised trace
**Kestra itself** could not reproduce across five rounds, so it has no golden
entry at all. The finer the interval, the more of the iteration count is
latency. **The wording for W8 Task 7's coverage statement is therefore
*a latency-dependent golden*, listed apart from the five blindness mechanisms
and not among them** — the class is "entries no implementation can bank", whose
other members are named in `baseline.txt` rather than here. It is written down
at all because a finding that lives only in a task report is a finding that has
to be rediscovered. `handler_surface.py` carries it as a
`clock` blocker so the ceiling does not name the entry as bankable.

**Both sides are blind, and the second half is egret's.** Even if Kestra's
trigger-fired executions reached the golden, nothing on egret's side of this
oracle evaluates a trigger: measured with a repo-wide grep, `trigger::tick`,
`trigger::run_until_idle`, `TriggerState`, `ScheduleTrigger` and
`WindowFreeFlowTrigger` have **zero** uses outside `crates/egret-workflow/src/trigger/`
— no production caller, no driver caller, no `tests/executor.rs` caller. The one
exception is the webhook, whose only consumer is `webhook_routes.rs`, mounted by
`egret-server` **only** when `[workflow] webhooks_enabled` is true, and its
default is `false` (asserted in `egret-config`'s own tests, and `figment` makes a
malformed value a hard startup error rather than a silent `false`). This is item
15's shape one level in: the ratchet is a statement about a state machine and a
driver, and for these four constructs it is not even that.

*(**Re-measured at `054ebdf6`, 2026-09-15, and it still holds**, which is worth
saying because the sentence one level up — *nothing runs this worker* — stopped
holding in the same package. Over every tracked `*.rs` outside
`crates/egret-workflow/src/trigger/`: `trigger::tick` **0**, `run_until_idle`
**0**, `TriggerState` **0**, `WindowFreeFlowTrigger` **0**, and `ScheduleTrigger`
**4** — all four in doc comments and a code comment (`models/trigger.rs:103`,
`:147`, `spec/validate.rs:634`, `:1720`), none of them a use. Positive control on
the same command: `RunState` **828**. **The trigger loop is W12**, so this is a
scheduled gap rather than an accident; W10 shipped the worker loop and
deliberately no trigger ticker, and its own spec says so. `item 15's shape` above
names the shape that item recorded, not the item's current verdict — item 15 is
closed.)*

**The control that makes these zeros mean something** is the **142**
sub-executions the golden does hold — `142 = 134` carrying a loop index `+ 8`
carrying none (the replays) — across 41 entries. This oracle records a
sub-execution whenever Kestra links it by `parent_id`. So "not one trigger-fired
execution anywhere in the golden" is a statement about **the link**, not about an
instrument that records nothing; and the test asserts `(0, 0)` sub-executions
under **every one of the 31** trigger-declaring golden entries beside that 142.

**The inversion this section exists to prevent, stated as a number: 28 of the
167 matching entries declare one of the three triggers.** Their matching is not
evidence the trigger works — it is evidence the trigger was **not evaluated on
either side**, since Kestra ran them from the harness's POST and egret ran them
from the driver's. A reader who counts matching entries by construct would
conclude the flow trigger is the best-covered thing in the package: **12 of 12
entries matching, zero baselined.**

**And the sharpest single row is `valids/task-flow`.** It is one of the sixteen,
it **declares a `Schedule` trigger**, and it banked because `Subflow` executes.
The design spec's §1 recorded the cron row as *1 baselined declaration*; that
row is **0** now, and not one thing egret does with a schedule caused it. An
attribution that counted declarations rather than causes would have credited the
scheduler with a bank. The test asserts both that `valids/task-flow` is the
overlap and that it is the **only** one.

#### What each trigger's own tests establish — and the three refusals

These are the instruments that stand in for the ratchet, and this is the
position `egret.sql` (item 3) and `egret.container` (item 3b) already occupy.
**94** tests live under `trigger::` — `14` in `trigger/mod.rs` (the clock, the
state, `tick`, `run_until_idle`, backfill), `22` schedule, `23` flow, `35`
webhook — plus `15` in `webhook_routes` and `31` in `tests/webhook_route.rs`.
*(Derived from `cargo test --lib -- --list`, not from a `test result:` line: the
filter `trigger::` is a **substring** match and reports 112, because it also
selects `models::trigger::`. A count whose instrument answers a wider question
than the one asked is this file's oldest recurring defect.)*

**The schedule trigger** rests on four measurements taken by **running Kestra's
own code off the pinned artifact** (a probe class compiled into
`io.kestra.plugin.core.trigger` with a fixed `SchedulerClock`): the cron dialect
built field by field over cron-utils 9.2.1 rather than a preset; `convertDateTime`'s
`withZoneSameInstant` behaviour across a DST gap and a fall-back hour; missed
ticks replaying one per pass without `lateMaximumDelay` and being dropped with
it; and the backfill window advancing and clearing as it passes `end`. **31
mutations, 30 killed, 1 survived and is named** (`M29`, a progress guard, left
unclaimed rather than described as covered). Its own review then hand-audited
all **149** claims in the module and its report into **MEASURED 91 / DERIVED 33 /
CHOSEN 25**, asserted to partition, after **throwing away** a keyword classifier
that scored 70/3/76 — *"reporting it would have been the same sin in a new
costume."*

**Refusal 1 — `lateMaximumDelay` is REFUSED, not interpreted.** It parses,
resolves and is captured verbatim, and `ScheduleTrigger::resolve` refuses the
schedule rather than scheduling as though the key were absent (which is what
dropping it did: over-acceptance in the direction this oracle cannot catch).
Reproducing `java.time.Duration.parse` is a measurement nobody has taken, and
*"trading a known hole for an unknown one"* is the thing the refusal exists to
avoid.

**The flow trigger** is `WindowFreeFlowTrigger`, and the name is the second
refusal. **Refusal 2 — `dependsOn` is refused, and that is 20 of the 21 corpus
declarations**, which is the only figure in this half a test compares **against
the corpus** (the 94 tests above are counted by `--list`, and everything else
here is a measurement someone took on the artifact; `oracle-exec/corpus` appears
in exactly one place under `src/trigger/`, and this is it): `the_corpus_flow_triggers_split_twenty_to_one` resolves exactly one
declaration (`flow-trigger-when-condition-flow-listen/on_completion`) and
refuses twenty, and it is written as the **acceptance criterion that fails the
day the window lands**. The hazard behind the refusal was seen **live on the
artifact** by that task's reviewer: one `dependsOn` listener fired **twice off a
single source execution** (`PAUSED`, then `SUCCESS`) — exactly what Kestra's
`fireOnce` suppresses, and wholly invisible to a ratchet with zero reachable
entries. **45 mutations, 45 killed**, 13 probes, and **four permanent controls**,
because *"a sweep with no controls reports the same thing broken as passing."*
The task's own lesson is the one this section is written under: **a two-sided
mutation pair on an OPERATOR says nothing about where the OPERAND came from** —
`max_depth` survived being replaced by the literal `100` across all 17 tests,
and swept properly there were **nine** fields of that shape.

> *W13b Task 5 (2026-09-26): the window landed, and this acceptance criterion
> did **not** fail — which is the correct outcome, and the sentence above
> predicted the wrong one.* W13b built the `dependsOn` arm **beside**
> `WindowFreeFlowTrigger`, not inside it (`trigger::depends_on`,
> `trigger::window`, and `flow_trigger::process::listen_depends_on`, which the
> processor takes before `resolve` is ever called), so `resolve` still refuses
> the twenty and `the_corpus_flow_triggers_split_twenty_to_one` still passes.
> The count that moved lives in its own test now,
> `trigger::depends_on::tests::the_corpus_depends_on_declarations_resolve_eighteen_of_twenty`:
> of the twenty, **18 pass the run-time gate** and would open a window, **1 is
> refused at save** (`trigger-flow-listener-namespace-condition`, W1's
> namespace-only over-refusal) and **1 does not parse** (the `fireOnce` flow).
> See *W13b* in the coverage statement.

**The webhook trigger** is the one whose verdict is not "approved". It is
**APPROVED and EXPOSURE NOT APPROVED**, and **the approval rests on the lever's
default being OFF**: before `[workflow] webhooks_enabled` existed the router was
mounted unconditionally, so *"do not expose"* had no lever at all. Two Criticals
were closed **structurally** rather than by a check — a flow declaring
`disabled: true` fired where the artifact answers 409, because the handler read
the DB column and never `parsed.disabled`; and `inputs: {x: "{{ trigger }}"}`
passed a substring scan and fed the execution the key-comparison context's
private object, which is now an `InputContext` that **binds no `trigger` at
all**, so a misclassification is a worse error message and can never be a leak
(proved against 55 spellings with a positive control, then 14 more by a
reviewer, including the root reached through a map **key**). **Refusal 3 — no
`wait:`, no `STORE`, no multipart**: the surface is knowingly narrower than the
artifact's and says so at the call site. Its anti-DoS control was itself a DoS
(`SWEEP_AT` was a trigger, not a cap: 65,536 attacker-chosen keys gave 1.93 ms
of mutex-held CPU per admission) and is now a `Mutex<Box<[AttemptWindow]>>` of
exactly 4096 — flat 0.56→0.78 µs per admission and constant 98,304 bytes over
250k keys, against a positive control reimplementing the old algorithm that goes
to 4,672 µs.

**What none of the three establishes, and no test in them can:
over-acceptance.** A trigger that fires where Kestra would not is the next
instance of this programme's recurring defect, and **the oracle cannot catch it**
— it is a Global Constraint of this package rather than an observation about it.
Every refusal above is an attempt to make that class smaller by declining
surface, which is a different thing from measuring it.

#### Instruments and defects handed on, each with an owner

Recorded rather than fixed, because each belongs to a package that is not this
one:

| | owner |
|---|---|
| **`egret-test-db`'s opt-out prints `N passed`** where this programme's rule is `0 passed; N ignored` — the rule item 3 states and `EGRET_ALLOW_ENGINE_SKIP` obeys | whoever owns `egret-test-db`'s `skip_verdict`; it is the crate the rule was taken FROM, which is why nobody checked it |
| **`egret-query/tests/connector_trino_params.rs` self-skips** (`12 passed; 0 ignored` in 0.01 s) and is **actively corrupting gate logs** — its `eprintln!` mangled a harness line and ate a test name out of W6 Task 6's name-set diff | `egret-query` |
| **`FlowRepo::find_by_name` is still unscoped**, and **two** modules now route around it | the flow-repo owner; a `workspace_id` parameter is the fix, and the routing-around is the evidence it is needed |
| **the dead log-scrape arm of `descendants()`** — left in place deliberately, with the measurement at the site | the package that makes triggers measurable: it has to open `descendants()` anyway to union `trigger_execution_id` |
| **`plan::MAX_TASK_DEPTH = 48` sits above a cliff bracketed at 42-survives / 47-aborts**, because `spawn_one` settles a child **inside its caller's transaction**, so depth in the data is depth on the stack. Kestra has **no** semantic bound to port: 12, 47 and **100** nested one-value `Loop`s all run to SUCCESS on the artifact, and its only refusal is the YAML reader's Jackson limit, bisected to 498-publishes / 499-`422` | whoever stops `settle` driving a child's first settle inside its caller's transaction. Until then, moving that one W3a constant **below** the cliff converts a process abort into a plan-time refusal |
| **the SEEDED `correlationId` divergence** — Kestra copies the cause's `system.correlationId` **label** verbatim and egret derives the chain root's **id**, so a root that declares one diverges | the wiring seam, `trigger::flow::FiredExecution::correlation_id`. `the_seeded_correlation_id_divergence_has_no_corpus_population` is why it is recorded and not banked: 22 corpus flows declare labels, **3** declare a `system.*` one, **none** declares `system.correlationId`, and the only **2** flows whose text names `correlationId` read it. **This row said more than that until the whole-branch review**, and the part it covered up was a defect rather than a divergence: the walk that produces the id stopped at a subflow link and answered the child's own id. Fixed, measured on the artifact, and instrumented — see below |

#### What the whole-branch review found, and why nothing in this section could

This section was written, reviewed and committed before the whole-branch review
ran, and the review found a **Critical in the one place this section had just
declared measured**. It belongs here rather than in a task report, because the
mechanism is the coverage statement's own subject.

**`Surface::load`'s `root` walked `parent_execution_id` alone.** It feeds
`labels.system.correlationId`. Migration `0138` — the migration this package
added — gives a subflow child `trigger_execution_id` with
`parent_execution_id` **NULL**, so the walk stopped at the first subflow link
and answered the child's **own** id. **That is `subflow::chain_holds`'s W6 Task
3 Critical, one module over, never swept for**: a guard whose premise is *"the
chain is walkable by one column"* against a migration that makes a child carry
exactly one of two. Task 3 fixed it where it was found.

**Three artefacts said the opposite and two of them called it measured** —
`render.rs`'s own comment (*"agrees with the artifact on every machine-generated
chain"*: a subflow chain is machine-generated), this file's W6 Task 4 section
(*"`root` deliberately keeps walking the whole chain, and W6 Task 4 measured
that this is right"*: Task 4 measured the **replay** link, and "the whole chain"
was true of the parent chain and read as true of the causal chain), and the
owner row above, which registered the divergence as reachable only by a
**seeded** label and named an **unwired** seam as its owner while the wired path
shipped it.

**The corpus reaches it.** `valids/subflow-grand-child` logs
`{{labels.system.correlationId}}` and is the target of `valids/subflow-child`,
which is **one of the twelve entries this section credits**; the same shape holds
for `valids/follow-dependencies-partial-{parent,child}`. The ratchet cannot see
it: the golden records **states**, and a `Log` renders either value without
failing. So this is the third kind of blindness in this very section — *the
predicate is evaluated and its output is not compared* — reached not by a
trigger but by a banked entry.

**Measured on the artifact rather than argued**, once the review had found it —
the corpus's own chain, renamed into a probe namespace on the pinned digest, read
out of Kestra's Postgres:

```text
flow              id                      parentId  trigger.…executionId     system.correlationId
corr-parent       2cvzjgi7mY6lDfaQxiNa0x  null      (started by hand)        2cvzjgi7mY6lDfaQxiNa0x
corr-child        8yWnkcC1oAvHsunjmTG1Q   null      2cvzjgi7mY6lDfaQxiNa0x   2cvzjgi7mY6lDfaQxiNa0x
corr-grand-child  2Cr8T9lsAM5icJ7KT5NEna  null      8yWnkcC1oAvHsunjmTG1Q    2cvzjgi7mY6lDfaQxiNa0x
```

The label is the **root's** id two links down, `parentId` is NULL at every hop,
and the cause is `trigger.variables.executionId`. Fixed: `Link::cause()` is
`parent.or(trigger)`, unambiguous because `one_link_kind` makes them exclusive.
The ratchet was re-read after the fix — **127, 0 broken** — which is what the
paragraph above predicts and not what it assumes.

**And the test that was supposed to cover this could not**, which is the part
worth carrying forward. `the_seeded_correlation_id_divergence_has_no_corpus_population`
(then named without `seeded`) asserts that both label-reading entries are
matching — and the oracle drives both **only as root executions**, where every
implementation of the walk returns the same value. *A fixture answers the
question you built it for and stays silent about the one you then generalise
to*, for the eighth time in this package, in the test written to state the
boundary. The walk's instrument is now
`a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item`, which drives
**two** subflow links (one cannot tell "the root" from "the immediate cause"),
keeps a loop child as the other-link control, and keeps a standalone execution
as the control that says the label renders at all.

**The widened walk needed a guard, and the guard has been seen to fail.**
`loop -> subflow child` is writable now, and without gating the `levels`/`item`
push on the same `inheriting` flag that child resolves `{{ item.value }}` off
its **caller's** iteration: measured, `LOOPVAL`, SUCCESS, killed 3 of 3 runs by
the mutation that removes the gate. It closes the asymmetry W6 Task 2's
re-review recorded as N-2 — `item` crossing a non-loop link while `outputs`
stopped.

#### The figures W6 retires above this section, and what each is now

The sweep ran under the third formulation — *a figure is safe only if a running
program compares it; every other figure, in any file, at any distance from any
derivation, is stale until re-derived* — and the yield says something about the
rule. **Every figure it found was already compared by a running program, under a
name the stale sentence did not use.** These are not numbers nobody checked;
they are numbers checked under their own name and copied out under another.

**Thirteen rows below: ten corrected in place and three named rather than
corrected.** One of the ten rows is four separate doc comments, so it is
**thirteen corrected sites and sixteen in all** — four of them in `src/` and
`tests/` rather than anything in this file, and two in this file's own opening
reading instructions.

| where | said | is |
|---|---|---|
| **this file's opening blockquote**, the sub-execution bullet | `16 of the 41 MATCH, 6 inside, 19 on more` | **22 / 4 / 15** — the note that exists to stop a reader picking up a stale figure, two packages stale itself, in the file's own reading instructions |
| **this file's opening blockquote**, the ordered-pair bullet | `357 are; the other 747` | **360 / 744** |
| *The coverage statement* → *The sub-executions* | `18 / 8 / 15` | **22 / 4 / 15**, and `18 + 8 + 15` and `22 + 4 + 15` **both total 41** — so the total-beside-the-parts discipline this file relies on cannot see this class at all |
| same section | `the eighteen by name`, `35 entries produce a spawned` | **twenty-two**, **39** — and the ordered-pairs section two screens up already said 39 |
| item 2 | `747 of 1104`, `461 / 1807 / 751 / 191 / 53` | **744 of 1104**, **433 / 1703 / 665 / 165 / 43** — six figures, all of them constants the suite asserts, and the *ratio* the item is about did not move, which is why nothing prompted a re-read |
| item 6 | `84 … 63 since W6 Task 3` | **59 since W6 Task 4**, and the four that left **never stalled**: a classifier artefact, not four executions that started finishing |
| *The sub-executions: what is compared* — the retirement note on W3a's table | `16 of the 41`, `35 entries` | **22**, **39**: a stale figure inside the note that retires the table above it |
| *What this oracle does not prove* (W3b's), the sub-execution bullet | `35 / 16 / 6 / 19` | **39 / 22 / 4 / 15** |
| `oracle-exec/baseline.txt:204` | `It is 16 of the 41 MATCHING` | **22**. **CORRECTED at W8 Task 3 fix round 2**, in place and with the number. The reason it was left — *"this package may not edit `baseline.txt`, whose md5 (`a3d887ae72595e61c8bc16e8530f9951`) is the reference every published W1–W6 number is stated against"* — **lapsed when W8 Task 3 moved that md5 itself**, which it did twice for banks before this round moved it again. A stale reason outlives a stale figure and is worth more attention: the figure was one number, the reason silently protected every other number in the file |
| `src/trigger/mod.rs`, the scheduler's own module doc | `seven corpus flows declare a cron and **one of those is baselined**` | **none of the six comparable entries is baselined**. Inherited from the design spec's §1 table, which was measured *before* the package; the one entry it counted is `valids/task-flow`, which W6 Task 3 banked because `Subflow` executes. **The scheduler's own doc invited a reader to read a `Subflow` bank as a schedule bank** |
| `src/worker/subflow.rs`, `tests/executor.rs` (twice) and `tests/worker.rs` | `the ratchet is unmoved at 131` | **127**. The value the day W6 Task 3 measured it, four lines of the same package before Task 4 banked four more. The claim is *unmoved*; the number beside it was a moment, and all four now say so |
| `oracle-exec/runnable.txt`, W4's *measured now* table | `matching 151 / baselined 143`, `357 / 747`, `17 / 8 / 16` | **167 / 127**, **360 / 744**, **22 / 4 / 15**. W6 appends its own block there rather than rewriting W4's, which is that file's convention |
| the flip and corruption **experiments** in this section | `green at 143`, `144 mismatching` | left as they were run. They are experimental records on a different tree, and an adjusted experimental result is a number no program ever produced. Both mechanisms are still checked on every run by `a_corrupted_golden_terminal_now_breaks_the_entry_it_used_to_be_fed_to` and the pairs test; only the printed counts are historical |

**Two rules were falsified as well as figures**, which is what W4 warned the
sweep would find:

1. *"A figure a test compares cannot go stale"* — false in the only way that
   matters. Eleven of the rows above are figures the suite asserts. What went
   stale is the **copy**, and a copy has no name for the constant it came from.
   The shape to look for is not an unchecked number; it is **a number whose
   checker is elsewhere**.
2. *"Assert the total beside the parts and an error shows up"* — the
   `22 + 4 + 15` row is a counterexample this file can now point at. Two figures
   moved in opposite directions by the same amount and the total held.
3. **"Where this section and anything above it disagree, this section is
   right"** — this statement's own governance rule, and the sweep falsified it
   *inside* the section. *The sub-executions* said **35** entries produce a
   `spawned` while *The ordered pairs*, two screens up **in the same section**,
   said **39**; item 2's five family figures were a package behind while the
   tables they are drawn from were current. The authority is not this section.
   **It is the named constants in `tests/executor.rs`** — `SPAWNED_ENTRIES_THAT_MATCH`,
   `ENTRIES_WHERE_EGRET_PRODUCES_A_SPAWNED`, `TERMINALS_ON_BASELINED_ENTRIES`
   and the rest — which is where the next sweep should start.

**And the method that worked, stated so the next sweep does not have to find it
again: sweep from the CONSTANTS outward, not from the prose inward.** Every
earlier formulation scoped the search by *place* (this file, this section, the
files this task touched) and every one of them missed copies living somewhere
nobody had a reason to open. Enumerating the constants the suite asserts and
then grepping each one's **concept** — "of the 41", "entries produce a
`spawned`", "end unfinished", "kept ordered pairs", and the bare ratchet values
`143`/`131` — over `src/`, `tests/`, `oracle-exec/` and `docs/` in every file
type found every one of the sixteen, including the four `131`s in doc comments
and the module doc that had inherited a figure from the design spec's
pre-package table. A phrase grep would have found what its author already knew
was there; the constants are a list nobody has to remember.

#### What this section does not do

It does not attribute the trigger half to anything. **If every line of
`src/trigger/` were wrong, this file would print `127` and every test in this
suite would pass** — W6 Task 6 said so of its own package while shipping it, and
it is the honest sentence to end on. Making triggers measurable is a package of
its own: `descendants()` unioning `trigger_execution_id`, `normalise.py`
building its tree on that link, a golden schema that can hold a sub-execution
with no loop index, a **re-harvest**, and a **rewritten `baseline.txt`** — the
reference point every published W1–W6 number is stated against. That cost
belongs beside the decision, which is why it is written here rather than
implied.

### W8 — the ratchet moved eighteen, and the last fix is one it cannot score

`127` before W8 Task 1 and **`109`** at this commit. **Eighteen banked**, which
is exactly what the design spec predicted — **and not by the route it
predicted**, which is the part a reader can act on and the total is not. The
per-task movement first, then the eighteen, then the four places the route
differed, then what the ratchet still cannot speak for.

| | ratchet | what the task shipped |
|---|---|---|
| package base `d76de96e` (W6's tip — the commit W8's spec `3a38b0e6` sits on) | 127 mismatching, 127 baselined, 0 broken | — |
| Task 1 | 127 | three corrections before anything was counted; the `concurrency:` row DELETED |
| Task 2 | 127 | `RunState::Paused` and `RunState::Restarted`, non-terminal, with the partition guarded |
| Task 3 | **119** | `Pause` — the wait, its bound, and the resume |
| Task 3 fix round 1 | 118 | a ninth bank, `valids/pause-behavior` |
| Task 3 fix round 2 | **119** | **the ninth bank WITHDRAWN** — it survived only on an over-accepting predicate |
| Task 4 | **118** | `LoopUntil` / `WaitFor`; `Command::PauseRun` carries its `Resume` |
| Task 5 | **111** | `allowFailure:` / `allowWarning:`, and `RunState::Warning` |
| Task 6 | **109** | `sla:` `EXECUTION_ASSERTION`; `MAX_DURATION` refused by name and recorded unbankable |
| Task 7 | **109** | `claim::due_retries` wakes a `PAUSED` run — **and the ratchet is 109 with the fix and 109 without it, measured both ways** |

#### The eighteen, by construct

**Each is named rather than counted**, because a package that reports a total
can be right about the number and wrong about every attribution in it — which is
what happened twice inside this package and is recorded under *the route* below.

| construct | count | the entries |
|---|---|---|
| **`Pause`** (Task 3) | **8** | `valids/pause-delay`, `valids/pause-timeout`, `valids/pause-kill-delay`, `valids/pause-duration-from-input`, `valids/trigger-flow-with-pause`, `valids/flow-trigger-paused-flow`, `valids/flow-concurrency-cancel-pause`, `valids/flow-concurrency-queue-pause` |
| **`LoopUntil` / `WaitFor`** (Task 4) | **1** | `valids/waitfor-nested` |
| **`allowFailure:` / `allowWarning:`** (Task 5) | **7** | `valids/task-allow-warning-{runnable,flowable,executable-flow}` (Step 2, no new state); `valids/task-allow-failure-{runnable,flowable,executable-flow}` and `valids/pause-timeout-allow-failure` (Step 3, with `RunState::Warning`) |
| **`sla:` `EXECUTION_ASSERTION`** (Task 6) | **2** | `valids/flow-concurrency-sla-assert-cancel`, `valids/sla-execution-condition` |

**`valids/pause-timeout-allow-failure` is a `Pause` entry that Task 5 banked,
not Task 3**, and the row it sits in is the one that banked it. Its `timeout:`
fires and its bound's terminal is `WARNING` rather than `FAILED`, which is
`allow_failure::fail`'s answer and not `pause::deadline`'s — so counting it
under `Pause` would credit the wait with a bank the flag made. Migration `0143`
exists for that one entry's `resume_with = 'TERMINAL:WARNING'`.

**The attribution standard is W6 Task 3's and it was met for all four
constructs, which is checked here rather than asserted**: a mutation that
reverts the construct returns the ratchet to its old value **naming exactly the
banked ids**.

| construct | the mutation | what the ratchet named |
|---|---|---|
| `Pause` | M1, one line | 127, **exactly those eight** as regressions |
| `LoopUntil` / `WaitFor` | M2 — `descend`'s arm back to `return Step::Waiting`, one line, token count `1 → 0` printed before the verdict | 119, `REGRESSION: 1` — `valids/waitfor-nested`, and nothing else |
| `allowFailure:` / `allowWarning:` | **M12** (`PlanTask` carries `false,false`) and **M13** (`models::Task` parses `false`) — the provenance chain end to end, author YAML → `models::Task` → `PlanTask` → each of the four sites | **all seven**, on each of the two independently |
| `sla:` | `and(None)`, 1 grep hit, **3 runs, dead 3/3** | `REGRESSION: 2` — exactly the two |

Those are measurements those tasks took, each with its run count and its control
beside it; they are **not** something `cargo test` re-runs. Two of W8 Task 5's
mutations **survived** at a green and unmoved ratchet and were fixed rather than
explained away — see its section.

#### The route, not the total — four places the package differed from its own spec

The spec predicted eighteen and eighteen is what happened, and **not one of the
four constructs landed on its predicted number.** Two errors cancelled.

1. **Task 3 was low by one.** The spec gave `Pause` seven; it banked eight, and
   the eighth is **`valids/pause-timeout`** — `timeout: PT1S`, no
   `pauseDuration:`. The estimate treated a `timeout:` on a flowable as W4's,
   which is what `FlowService.java:574` says for every non-`RunnableTask`
   **except `Pause`** — an exception this file and `PlanTask::timeout` have both
   recorded since W3a Task 7. **Reading the exception as the rule cost one
   entry**, and it is the one bank in the package that required
   `PlanTask::timeout` to stop being `None` for the first time in the crate's
   life. (The spec's `Pause` row also counts `valids/pause-timeout-allow-failure`,
   which Task 3 did **not** bank — Task 5 did — so the predicted seven and the
   measured eight are not nested and *"the eighth"* means *the one the estimate's
   reason excluded*, not a set difference.)
2. **Task 4 was high by two, and the cause is a spelling.** The spec said
   *"three bankable"* out of nine `LoopUntil`/`WaitFor` entries. One banked.
   **The *"three"* was a spelling split, not a separator**: six of the nine are
   held by `driver.rs`'s registry shipping no
   `io.kestra.plugin.core.output.OutputValues` handler, and those six are
   **exactly the six whose flows spell the construct `WaitFor`** — so a split
   that looked like a behavioural classification was a partition on the task
   type's alias, and the three it named bankable were the three spelled
   `LoopUntil`. Of those three, one banked, one is `valids/loopuntil-failed-flowable`
   (a renderer divergence, registered in `STILL_OVER_ACCEPTED_AT_RUNTIME`) and
   one is `valids/loop-until-restart`, which **no implementation can bank** —
   see below.
3. **Task 5 was low by one.** The spec gave `allowFailure:` six; it banked
   seven, the seventh being the `Pause` entry named above.
4. **A ninth `Pause` bank was banked and then WITHDRAWN.** Task 3 fix round 1
   banked `valids/pause-behavior` and fix round 2 took it back out: the entry
   matched only because the predicate under it accepted more than Kestra does,
   and **an entry that banks for an unattributed reason is a finding, not a
   bank.** The ratchet went 119 → 118 → 119 across the two rounds. This is the
   only place in the programme where `baseline.txt` has moved **up** on purpose,
   and it is recorded here rather than only in the task's section because a
   reader reconciling `127 − 18 = 109` will otherwise find a nineteenth removal
   in the git history with no counterpart.

#### Entries no implementation can bank — listed apart from the five blindnesses

**This is not a sixth way the oracle is blind and must not be filed as one.**
The five mechanisms in *The constructs the ratchet cannot speak for* are reasons
the ratchet **cannot speak**. This class is the opposite: the ratchet speaks,
compares the entry, and **is wrong** — it is **effect 2** of the product under
*EFFECT is a second axis*, the (a golden entry exists × the irreproducibility
lives in the GOLDEN) cell. **The blindness count stays at FIVE.**

| entry | why no implementation can bank it | what to do |
|---|---|---|
| `valids/loop-until-restart` | **A latency-dependent golden.** `maxDuration: PT3S` with `interval: PT1S`; Kestra's `reachedMaximums` compares the loop taskrun's creation plus the bound against `Instant.now()`, and on the captured round its two `PT1S` pauses took **1.17 s and 1.92 s**, so the third iteration ended at T+3.33 s and was past the bound. The golden holds three iterations. `driver.rs::pass_time` moves the clock to the earliest deadline and never further, so any correct implementation ends its third near T+2.05 s and runs a fourth. **The captured value is a function of the artifact's scheduler rather than of the flow.** Corroborated as a property of the CONSTRUCT rather than of the entry: `valids/waitfor-max-duration`, the same shape at `PT5S`/`PT0.2S`, is one of only **two** corpus flows whose normalised trace **Kestra itself** could not reproduce across five rounds, so it has no golden entry at all | **Nothing.** That is the whole actionable content, and filing this as a blindness is what loses it. `handler_surface.py` carries it as a `clock` blocker so the ceiling does not name it bankable |

**The three `MAX_DURATION` entries are adjacent and are NOT in this class**, and
the distinction is the reason the class exists. They share the *mechanism* — the
harness compresses time — and differ on the *effect*: they are **effect 3**, so
a repair exists and is nameable (uncap `driver.rs`'s `Sleep`, then build a
monitor keyed on wall-clock time that `advance()` cannot host). *"No
implementation can bank them"* is true of them only with **"under this
harness"** attached, and W8 Task 6's refusal is written with that clause and
with a running program under it — **0 registrars across the four DECLARERS**,
against a control of **28 of 269** driven flows that do register a deadline.
**That measurement covers four entries and not six**, and generalising it to all
of them was W8 Task 6's own error: the two `*-parent` entries declare no `sla:`,
their `Subflow` run necessarily DOES span steps, and what holds them is a
**different and stronger** fact — `pass_time`'s `WITH RECURSIVE` joins
`parent_execution_id` while a subflow child is linked by
`trigger_execution_id`, so **the clock-advance walk cannot reach the child at
all**, which would hold even if the child registered a deadline.
`the_two_sla_parents_are_held_by_a_child_that_ran_and_succeeded` runs that exact
CTE. **Scope a refusal to what was measured, and name the route for the rest.**

#### Two corrections this package made that read as lost scope, and are not

* **The `concurrency:` row is DELETED, not lowered.** W8 Task 1 removed it from
  *States and constructs no W3a task claims*. It is a **correction, not a
  surrender**: the row attached **3 entries** to flow concurrency through the
  `CANCELLED` state, and that attribution was wrong for four packages. The
  golden's three `CANCELLED`s are **two `sla:`s** (`valids/flow-concurrency-sla-assert-cancel`,
  `valids/sla-execution-condition` — both banked by W8 Task 6) and **one
  `Exit`** declaring `state: CANCELLED` (`valids/exit-cancelled`, the
  `ExecutionUpdatableTask` row's, still unclaimed). Not one is a limit's.
  `the_cancelled_state_never_comes_from_a_concurrency_limit` derives it, and its
  control is that the two corpus flows writing `concurrency: {behavior: CANCEL}`
  produce **no `CANCELLED` at all**. So flow concurrency stranded nothing: it
  reaches **0** baselined entries and the ratchet cannot score it either way.
* **`allowFailure:`'s double assignment was settled, and the residual table was
  the loser.** The construct was **assigned twice in two documents**: to W8 in
  the programme spec's §9, and to *"unclaimed"* in this file's residual table.
  One of those had to be wrong and the package had to say which rather than
  inherit both. It is the residual table, **corrected at W8 Task 1 rather than
  §9 being lowered** — and the ruling is derived rather than administrative: all
  twelve entries reach the construct and **seven bank inside W8 alone**, which
  is the cheapest set in the package, and a row whose entries a package can bank
  is not unclaimed. **§9 needed no change.**

  *(A separate fact, often quoted with it and not the same one: one entry,
  `valids/waitfor-child-task-warning`, was **held twice** — by the `AllowFailure`
  row and by the `LoopUntil`/`WaitFor` row. W8 Task 5 closed the `allowFailure`
  half, the `WARNING` child is byte-identical to the golden now, **and the entry
  is still baselined** — which is what turned *held twice* from a claim into a
  measurement: it stalls on the `OutputValues` child instead, reproduced
  strictly further than it was, and is the `LoopUntil` row's alone.)*

#### W8 Task 7 — a defect in the product that the ratchet scores in neither direction

`claim::due_retries`, the SQL half of the waker, was gated to
`state = 'RETRYING' OR (state = 'RETRIED' AND …)` from W4 Task 1. A bounded
`Pause` parks its run `'PAUSED'` with a `not_before_ms`, so **in the product a
bounded wait never ended**, however correct `advance()` was — and W8 gave that
state **two** producers, `Pause` at Task 3 and `LoopUntil`/`WaitFor` at Task 4,
which is what turned Task 2's prediction into a live defect. Task 7 added the
`'PAUSED'` arm.

**Two, not three**, and the third was checked rather than counted.
`RunState::Restarted` is the other non-terminal state W8 Task 2 added and is
often named beside these, but **nothing can put a RUN in it carrying a live
deadline**: `pause_mark` pushes it onto the **execution**'s history and nothing
pushes it onto a run's — 0 of the golden's 840 run histories against 12
execution histories that do — and `Command::RestartLoop`, the one command that
ends a `LoopUntil`'s wait, clears `not_before` and `resume_with` in the same arm
and pushes `RUNNING`. A state with no producer of due rows is not a gap in the
gate, and `the_waker_names_exactly_the_states_something_will_move` asserts it
rather than leaving it to be re-argued.

**Neither the defect nor the fix moves anything here, and that was measured in
both directions rather than argued.** `driver.rs`'s `pass_time` selects on
`not_before_ms` with **no state term anywhere in its SQL** and then calls
`worker::resume` itself, so the oracle reaches the same runs through a different
door: the ratchet reads **109 with the arm and 109 with it removed**. This is
item 15's shape one level in — a property proved about a path no deployment
takes — and it is **item 11**'s subject, where the measurement and the two tests
that do score it are written out.

*(Item 15 is **closed since W10 Task 2**, and this sentence names the shape that
item recorded rather than its verdict. **The shape outlived the item**, which is
the part worth keeping: a deployment takes this path now — `runtime::spawn` →
`tick` → `wake` → `resume`, and `tick` calls the waker ahead of the claim — and
the ratchet still reads 109 either way, because the oracle's second door was
never the reason it could not see the arm. Wiring the worker changed which of
the two sentences here is true and changed neither number.)*

**One figure did move, and it is the one this file had recorded as a flat
zero.** *Instrumented `wake`: 0 fires over the whole oracle* is now false as
stated: widening the gate gave the oracle a second door to the same `resume`,
and the instrument reads **1 fire in one of five runs and 0 in the other four**
— latency-dependent, because it fires only when real wall-clock time crosses a
pause's bound before `pass_time` rewinds `created_at`. The ratchet is **109 in
all five**, because both doors call the same `worker::resume` and which one
opens changes no trace. A positive control on the same build (`tests/worker.rs`,
1 fire) is what makes the zeros mean anything.

**Stated as narrowly as it was measured.** The instrument printed the execution
id and nothing else, so **the single fire was not attributed to an entry** — and
it is not attributed here. What is known is the *change*: the only thing W8 Task
7 altered is the `'PAUSED'` arm, and `RETRYING`/`RETRIED` selection is
byte-identical, so **the retry mechanism the W4 row describes is untouched** —
`pass_time` still consumes a due retry inside the call that made it due, and
`the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them` is the
standing derivation over that population. *"0 fires on the retry population"* is
an inference from an unchanged predicate, not a reading of this instrument, and
the difference is the kind this file exists to keep.

#### The figures W8 Task 7 retires, and the population each was hiding in

The rule this statement ends with is *a figure is safe only if a running program
compares it*, and **W8 Task 7 extends it: so is a PREDICATE.** `due_retries`'
state gate was a predicate nothing compared, and no figure sweep could have
found it — W6's Critical was the same class. Swept by concept — *what selects a
run to act on* — **eleven selectors were examined and one was wrong**:
`claim::claim`, `claim::due_retries`, `reaper::reap`, `replay::due_run`,
`spawn::terminated_count`, `worker::subflow`'s parent report,
`driver.rs::pass_time`, `executor::retry::reopen_due`,
`flowable::resume_paused`, `sequential::walk` and `parallel::drive_concurrent`.
Two of the ten correct ones are correct **structurally** rather than by
vigilance, and they are the pattern: `terminated_count` derives its state list
from `RunState::ALL` filtered by `is_terminal()`, and `resume_paused` reads the
RUN's own `resume_with` so there is no list of kinds to be missing from.
`the_waker_names_exactly_the_states_something_will_move` puts `due_retries` on
the same footing — it answers for **every** member of `RunState::ALL` and
asserts its own key set equals `ALL`'s, so a fourteenth variant fails the test
rather than joining the unnamed majority by omission.

The figure sweep beside it found that **one family of six numbers is copied to
five places in this file and its suite, maintained in exactly one of them, and
inconsistent among the other four.** The family is the six projections of *what
lives on a baselined entry* — run terminals, run-history tokens,
execution-history tokens, executions, sub-executions and kept ordered pairs. The
maintained copy is five `const`s plus one assertion tuple in
`tests/executor.rs`, whose doc comments carry the whole W8 arc
(`165 → 157 → 156 → 149 → 147` and its four siblings) and were updated by every
task that moved them.

| site | figures examined | corrected | what it read |
|---|---|---|---|
| this file's **opening blockquote** | 2 | **2** | `360` asserted / `744` not → **399 / 705** |
| *The ordered pairs*' **ASCII block** | 10 | **4** | `360 / 258` and `744 / 619` → **399 / 297** and **705 / 580**; the totals and the two nested counts were right |
| *The ordered pairs*' **six-family table** | 6 | **6** | `744 / 445 / 1759 / 709 / 177 / 51` → **705 / 396 / 1528 / 522 / 147 / 43** — W6 **Task 3**-era, two packages behind |
| **item 2** of *What this ratchet does not prove* | 6 | **5** | `744 / 433 / 1703 / 665 / 165 / 43` → **705 / 396 / 1528 / 522 / 147 / 43** — W6 **Task 8**-era; `43` is the one that did not move, because **not one of the eighteen carries a sub-execution** |
| `kept_ordered_pairs_…`'s own assertion **message** | 1 | **1** | *"Flipping all 744"*, three lines under a tuple asserting `705` |
| **total** | **25** | **18** | |

**Two of those copies disagreed with each other**, which is the strongest
argument this file has produced for the rule: item 2 and the six-family table
are the same six projections of the same set and they differed in five places,
because one was re-derived at W6 Task 8 and the other at W6 Task 3, and nothing
compares a copy to a copy.

**And item 2 is where this happened last time too.** Its own parenthesis records
W6 Task 8 re-deriving all six because they were *"a package behind"*, with the
diagnosis: *a figure checked under its own name and copied out under a different
one.* The parenthesis was right and **did not prevent the recurrence** — because
**a warning written beside a figure does not check the figure**, and because the
sweep it came from swept the sentence it was handed rather than the population
the figure belongs to. The rule that finds all five sites in one query is *take
the maintained copy's NAME and find every site quoting its VALUE*.

One more, in a different population:

| site | figures examined | corrected | what it read |
|---|---|---|---|
| *The residual baseline*'s heading | 1 | **1** | `111 entries` → **109 entries**. `the_residual_baseline_is_attributed_to_a_construct_or_named` asserts every row and **`println!`s the total**, so nothing compared it |

And one **claim** rather than a figure, in the W4 three-holes table's `retry:`
row: `10 baselined + 15 matching` → **8 + 17** (`(8, 17, 25)`, asserted), and
*"Instrumented `wake`: **0** fires over the whole oracle"* → **0 on the retry
population**, with the whole-oracle figure now latency-dependent. **3 examined,
3 corrected** in that row.

Two more populations, reported with their denominators because **a denominator
equal to its numerator is a sign the sweep only found what it went looking
for**:

* **the phrase, verbatim, across the whole repository** — `due_retries` appears
  in **ten** committed files (`.superpowers/` is the package's working record and
  is not swept). **10 examined / 6 corrected**: `src/worker/claim.rs`,
  `src/worker/mod.rs`, `src/executor/retry.rs`, `tests/worker.rs`,
  `oracle-exec/runnable.txt` and **this file**. Two are **examined and still
  true**: `src/worker/subflow.rs`'s sentence (*"a global scan, so a shipping
  worker wakes such a retry like any other"*) is truer now, and
  `tests/executor.rs`'s is about the retry population, which did not move. Two
  are **examined and deliberately NOT corrected** —
  `docs/en/plans/…-w8-advanced-semantics.md` and `docs/en/specs/…-design.md`.
  They are the dated plan and design record; the plan's line *is* this task, and
  the spec's §Risks item 2 is a prediction this package **discharged** rather
  than a claim that went stale. Editing them would falsify the record of what
  was known when. **That is a verdict, not a miss**, and it is written here so
  the next sweep does not re-find them as a defect.
* **data that feeds a generator** — `oracle-exec/runnable.txt`'s retry block is
  **below `HANDWRITTEN_SENTINEL`**, which `harvest.py --rewrite-runnable`
  preserves, so the correction cannot be undone by a regeneration. **1 examined,
  1 corrected.** It had read `14 baselined + 11 matching` with a parenthesis
  (*"`CREATE_NEW_EXECUTION` writes no `not_before` at all"*) that W6 Task 4 had
  falsified in both halves — **README corrected its own copy at W6 Task 4 and
  this one survived two packages**, which is the branch's *corrected where
  found, missed elsewhere* one more time, in the file this README tells a reader
  to believe over itself.

And **four predicates stated as prose**, corrected in `src/` and `tests/`:
`retry.rs`'s *"that one is a live defect now"* (fixed); `worker/mod.rs`'s `wake`
naming *"a due retry"* (it names a due `PAUSED` run too); `tests/worker.rs`'s
*"no merged code has ever written a `PAUSED` row at all"* (W8 Task 3 falsified
it in the commit **before** the one that wrote it); and `tests/worker.rs`'s
*"`CANCELLED` is the state `RunState` still does not hold"* (W8 Task 6 added it
two tasks later — the row is still refused, for the narrower reason that
`pause::deadline` produces `SUCCESS`, `FAILED` and `WARNING` and nothing else).
**4 examined, 4 corrected**, and all four are the same shape as the figures: a
sentence that went false without any number changing.

### W10 — the worker runs in the product, and the oracle cannot speak for that at all

**`109` before W10 Task 1, `109` at this commit, and `109` with every line of
W10's wiring deleted.** The third number is the only reason the first two mean
anything. Every package before this one could at least say *the ratchet did not
move and here is what could have moved it*; W10 cannot, because **the channel
between what it shipped and what this ratchet reads is empty by construction**,
and a zero out of an instrument with no channel is not a measurement of anything
until the instrument is shown to be capable of a different answer. That is what
the third clause does.

This section therefore reports the **route** rather than the total, in the
discipline W6 and W8 used, and then says what the green does not cover.

| | ratchet | what the task shipped |
|---|---|---|
| package base `92cfae9b` (W8's tip, on `main`) | 109 mismatching, 109 baselined, 0 broken | — |
| Task 1 | 109 | the W10/W11/W12 spec rows; **the trigger loop named as W12**, so the gap in the W6 section above is scheduled rather than forgotten |
| Task 2 | 109 | `runtime::spawn` — two config-gated tickers — and `egret-bin` starting it. **The first time egret runs a workflow in production** |
| Task 3 | 109 | `ExecutionRepo`, the read layer nothing had over `workflow.executions` |
| Task 4 | 109 | the two read endpoints |
| Task 5 | 109 | the three write endpoints — start, resume, replay — and the pause egret cannot end |
| Task 6 | 109 | the first end-to-end test, and the finding under it: **no Kestra FQCN was publishable at all** |
| Task 7 | 109 | this section |

#### The zero channel, and it was measured in both directions

**The mechanism first, because it is nameable and short.** `driver::run` is
`executor::build_plan` + `worker::start_execution` + a loop of
`worker::tick(pool, BATCH, reg)` and `driver::pass_time`, and it **never calls
`publish`, `validate` or `registry::known_type`**. `tick` is handed
`driver::registry()` — a `Handlers` the driver builds itself, four handlers, one
of them a `Sleep::capped(Duration::ZERO)` that does not exist in any deployment
— so the oracle exercises the **body** of the loop W10 Task 2 wired up and is
blind to the wiring by construction. It is blind to the publish path for a
second, independent reason: `publish` writes `flow_versions`, and the runtime's
subflow consumer resolves against the **draft** row, `workflow.flows.source`
(`worker::subflow`'s own doc records that decision and the three reasons it
rests on). Nothing W10 changed can reach `golden.json`.

**Neither half of that is an argument here, because both were run.**

- *The publish half*, measured by this branch's reviewer at `054ebdf6`:
  `baseline.txt` emptied and `executor_matches_golden` run twice — at `HEAD`, and
  with Task 6's five changed sources reverted to `53a8970b` — and the **1029
  lines of per-entry detail are byte-identical**. Falsifiable in both
  directions rather than assumed to be: the comparison's two failure modes were
  both provoked and both printed — `REGRESSION:
  valids/working-directory-taskrun-nested` for a mismatch outside the baseline,
  `PROGRESS NOT BANKED` for a baselined id that now matches — so an
  instrument that could only ever print "identical" is excluded. **227 corpus
  flows becoming publishable moved
  zero entries**, which is the expected answer for a change to publish's table
  and would not have been for a change to the executor.
- *The wiring half*, measured by Task 7 on this tree, 2026-09-15:
  `crates/egret-workflow/src/runtime.rs` deleted, `pub mod runtime;` and
  `pub use runtime::RuntimeConfig;` removed from `lib.rs`, and the
  `egret_workflow::runtime::spawn(…)` call commented out of
  `crates/egret-bin/src/main.rs`. Evidence printed **before** the verdict was
  read — `git diff --stat` reporting `338` deletions in `runtime.rs`, `0`
  occurrences of the call left in `main.rs`, and `ls` reporting the file gone —
  because *a mutation that silently fails to land prints a result line identical
  to the control's*. Both runs print
  `294 golden entries, 109 mismatching, 109 baselined, 0 broken`. Unmutated
  control in the same session, same build.

That second run also says why the deletion cannot matter: the `executor` test
binary never links `egret-bin` at all, and neither `oracle-exec/driver.rs` nor
`tests/executor.rs` names the module — `runtime::spawn`, `RuntimeConfig`,
`crate::runtime` and `egret_workflow::runtime` are each **0** occurrences in
both, with `worker::` at **26** and **37** as the positive control on the same
command. *(The bare word `runtime` is not the test: it occurs in both, in
`build_runtime` and in `runtime_over_acceptance_is_registered_and_owned`, which
have nothing to do with this module — a substring is not a reference, and
counting one as the other is how a zero gets claimed for the wrong reason.)*
**A structural zero and a measured zero, and the structural one is the stronger
of the two** — it holds for every configuration, where the run holds for one.

#### Item 15 is closed, and the bound that replaces it is narrower, not gone

Item 15 read *"nothing in this workspace runs this worker, and that bounds every
claim above"*. **W10 Task 2 made it false.** Derived on this tree rather than
quoted — `git ls-files 'crates/*/src/*.rs' 'crates/*/src/**/*.rs'` grepped for
`worker::tick(`,
`worker::reap(`, `worker::start_execution(` and
`worker::start_execution_with_trigger(`, with doc and comment lines dropped:

| production call site | what it is |
|---|---|
| `crates/egret-workflow/src/runtime.rs:304` | `worker::tick` — the claim ticker |
| `crates/egret-workflow/src/runtime.rs:328` | `worker::reap` — the lease reaper |
| `crates/egret-workflow/src/routes.rs:1436` | `worker::start_execution` — the management API's start |
| `crates/egret-workflow/src/routes.rs:1633` | `worker::start_execution` — replay's replacement execution |
| `crates/egret-workflow/src/webhook_routes.rs:651` | `worker::start_execution_with_trigger` — W6 Task 7's webhook |

**Five sites in three files, and `runtime::spawn` itself is called from
`crates/egret-bin/src/main.rs:1733`.** `worker::resume` gained one too
(`routes.rs:1523`), and a second indirectly: `tick` calls `wake` ahead of the
claim, so the chain `runtime` → `tick` → `wake` → `resume` runs in the product.

**The fifth row is why the old grep was true and misleading at the same time.**
W6 Task 7's webhook has been able to *create* an execution since W6 — behind
`[workflow] webhooks_enabled`, which defaults to `false`, so no deployment had
one unless it typed the flag — and the
grep item 15 published searched for `start_execution(`, which does not match
`start_execution_with_trigger(`. The proposition survived not because nothing
started executions but because **nothing advanced them**: a webhook wrote a row
and no claim loop ever picked it up. That is the sentence `runtime.rs`'s own
module doc opens with, and it is the honest form of what item 15 was reaching
for.

**What replaces it as a bound**, because *"the worker runs now"* is not a licence
to read `109` as *"the worker is fine in production"*:

1. **The default is `false`.** `egret_config::WorkflowConfig::worker_enabled`
   defaults `false` through a named function rather than `bool::default()`, and
   `runtime::spawn` returns before spawning anything when it is unset. A
   deployment that has not typed `EGRET_WORKFLOW__WORKER_ENABLED=true` behaves
   exactly as every deployment did before W10.
2. **The deployment's registry is not this oracle's registry, and neither is a
   superset of the other.** `egret-bin` passes
   `worker::Handlers::with_core_tasks()` — `core.log.Log`, `core.debug.Return`,
   `core.execution.Fail`, `core.flow.Sleep`, registered through
   `add_publishable` — and the oracle passes `driver::registry()`, the same four
   registered through `add`, with `Sleep` capped at zero. **Four handlers is the
   whole of what any egret deployment runs today.**
3. **`109` is still not a statement about a deployment.** It is a statement about
   `advance()` and a driver, for the reason the measurement above gives: the
   ratchet is 109 with the wiring and 109 without it. The old bound said *no
   deployment runs this code*; the new one says *a deployment runs this code and
   this number still does not watch it*. The second is narrower and no weaker.

#### What the runtime runs, and what a green `109` still does not cover

**These are findings, not omissions, and each is recorded where a future task
will trip over it.**

- **`egret.container` and `egret.script.python` are registered nowhere.** Both
  handlers exist, both have `TASK_TYPES` rows, and neither constructor is called
  by `with_core_tasks` or by `driver::registry()`. **`egret.sql` is in neither
  registry either**, and that one is structural: `handlers::sql::Sql` needs an
  `Arc<dyn QueryExecPort>`, `with_core_tasks()` takes no arguments, and the
  oracle's registry must never reach an engine. So items 3 and 3b below —
  *"reaches zero corpus entries"* — now have a second, independent reason
  standing beside the corpus one: **zero corpus entries and zero registrations.**
- **Kill is a finding rather than a missing endpoint.**
  `POST …/executions/{id}/kill` is not shipped and cannot be written without a
  mechanism underneath it: `RunState::Killed` has **zero** producing sites
  (`Command::SetRunState { state: RunState::Killed }` occurs nowhere; the variant
  appears only in aggregation and terminality predicates); a killer holds **no
  lease**, so `persist` cannot clear `claimed_at` and `apply_outcome`'s fence
  `WHERE id = $2 AND claimed_at = $3` still matches — the worker would write a
  terminal onto a killed run *afterwards*; and *kill-requested* is
  **unrepresentable**, because `executor::RunState` has no `Killing` variant
  (**0** occurrences in `executor/state.rs`, against a control of **9** in
  `models/state.rs`, which is the wire model the harvester reads and not the
  state machine). `routes.rs` carries the full bill. **A kill the worker can
  overwrite is worse than no kill**, which is why nothing was shipped.
- **An unbounded `Pause` cannot be ended by the resume route, and no number of
  calls will change that.** `flowable::resume_paused` is the only thing that
  moves a run out of `PAUSED`, it is pure over `(state, run, now)`, and its
  `Resume::Never` arm returns `Step::Waiting` **before it reads `now`**. The
  route is the right shape — gate, 404, 409 — and the missing piece is a
  `Command` beneath it. `an_unbounded_pause_is_not_ended_by_the_resume_route`
  pins it, so the arrival of that command makes a **test** fail rather than a
  paragraph go stale.
- **The converse of the publish guard is unguarded on purpose, and its symptom is
  a HANG.** `Handlers::add_publishable` **panics** if a handler's `type_id()` has
  no `TASK_TYPES` row, so a handler publish would refuse cannot reach
  `with_core_tasks` and `egret-bin` will not start — loud, at startup. The other
  direction — a `TASK_TYPES` row with no handler, which is `core.http`,
  `core.slack` and every `egret.*` a deployment declines to wire — is **not**
  guarded, and it is worse than its mirror. Guarding it symmetrically would
  refuse to start every deployment that does not wire `egret.sql` — i.e. every
  deployment — so the asymmetry is deliberate and the cost is written down here
  rather than discovered. **Item 19 carries the mechanism**, and it is narrower
  than this bullet used to state: `reap` clears only `claimed_at` and leaves the
  run `RUNNING`, which is what `claim` re-selects — and **it is one of two
  causes, not the only one.**
- **Logs are W11 and the trigger loop is W12**, and both deferrals carry a
  measurement rather than a preference. Logs: **`LogSink` has three
  implementations under `crates/egret-workflow/src/` and none of them persists a
  line** — `TracingSink`, a test `Recorder` and `NameHintSink` (a fourth lives in
  `tests/container_docker_live.rs`, which is why the count is scoped to `src/`;
  unscoped it is false) — and the container handler records
  `stdOutLineCount`/`stdErrLineCount`, i.e. counts and not content. A Logs panel
  therefore needs a table, a persisting sink on every handler path, a retention
  policy and a paginated read API, which is why W10's spec named it as the place
  to stop. *(2026-09-27, W11 Task 6: the logs half of this bullet is the record
  of the deferral, not the present. W11 built all four pieces -- migration
  0159, `logs::PgLogSink` behind every handler's `RunLog`, the hourly pruner
  and `GET …/executions/{id}/logs`; see the W11 section below.)* Triggers: re-derived in the W6 section at this commit — zero uses of
  `trigger::tick`, `run_until_idle`, `TriggerState` and `WindowFreeFlowTrigger`
  outside `crates/egret-workflow/src/trigger/`, four doc-comment mentions of
  `ScheduleTrigger`, against a control of 828 for `RunState`.

#### What W10 Task 7 swept, and what each population yielded

Five populations, because all five passes before this one scoped themselves by
**surface** when the property is about **mechanism**, and because a list of
sites handed to a sweep is the list of sites somebody already knew. Each
denominator is computed over `git ls-files` so it can be re-run; **a denominator
equal to its numerator would mean the sweep only found what it went looking
for**, and none of these is.

**Every figure in the table is taken at `054ebdf6`**, i.e. over the population
as it stood *before* this task edited anything — a sweep reported over its own
output is a sweep that cannot show a numerator.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the proposition **and its gloss**, verbatim, across the whole repository — every tracked file, **flattened to one line before matching**, for `no production caller`, `nothing (in this workspace) runs this worker`, `no claim loop`, `no deployment runs/executes/takes`, `a deployment runs none`, `wires the worker`, `a path no deployment`, `item 15's shape` | **3696 files**, 17 matching, **38 occurrences** | **15** |
| 2 | **ordinals and counts** in the four regions population 1 landed in — this statement's preamble, the W4 section's six facts, the W4 subsection, and item 15 | **95 tokens** | **9** |
| 3 | **data that feeds a generator** — the seven committed data files in `oracle-exec/` plus `harvest.py`, `normalise.py` and `handler_surface.py`, which emit prose into `runnable.txt` | **10 inputs** | **0** |
| 4 | **the other language** — every file under `docs/vi/`, flattened, for `vòng claim`, `caller production`, `không deployment`, `deployment nào chạy`, `không có gì gọi`, `mục 15`, `16 trên 345` | **279 files**, 6 matching, **17 occurrences** | **0** |
| 5 | **references to something deleted** — every backticked identifier of 15 characters or more in this file, checked for existence anywhere in the tree | **194 identifiers**, 6 absent | **1** |

**Population 1's handed list was eight sites; the sweep found fifteen false
occurrences, in three files rather than one**, and the gap is not
carelessness on whoever compiled the list — it is the instrument.
**The most consequential instance is invisible to a line-oriented
grep**, because it wraps a line break: this statement's own preamble reads
*"…the sixth fact of the W4 section above: nothing in this workspace ⏎ runs this
worker"*, and `grep` sees two lines that each say nothing. That is the sentence
this file points a reader at **first**. The sweep above flattens whitespace
before matching for exactly that reason.

**And the rule that would have put it on the list was already written in this
file.** *The rule this sweep ends with* has carried *"a phrase that WRAPS A LINE
BREAK … returns nothing quietly, which is indistinguishable from being clean"*
since W8 Task 6 fix round 2, with the remedy beside it — *search for the
shortest distinctive fragment, or use a matcher that spans lines*. **No earlier
pass missed this sentence** — it was true until W10 Task 2, so there was nothing
to find — which is what makes the miss precise rather than general: the first
enumeration taken after it went false was built line-wise and came out eight
where the answer is fifteen. Meanwhile the *same rule's* ordinals half **was**
reached for, twice, inside this package: W10 Task 5 corrected `worker::subflow`'s
*"two files"* to three with the note *"the ordinal moved with the count"*, and
Task 6 removed an ordinal from `claim.rs` rather than maintain it. So the
finding is not that the hazard is unknown. It is that **a rule in a document is
applied where whoever is sweeping remembers it** — the same failure mode as a
warning written beside a figure not checking the figure (item 2's parenthesis,
which did not prevent its own recurrence). The only durable form is the
mechanical one: **flatten before you match, always, so remembering is not part
of it.**

The fifteen are **eleven in this file** — the preamble (the wrapped one), the W4
section's sixth fact and the pointer under it, the four sentences of the W4
subsection *Nothing runs this worker*, and the four inside item 15 — **two in
`runnable.txt`**, which this file's own reading instructions say outranks it for
anything in the harvest section, and **two in
`crates/egret-workflow/tests/worker.rs`**, which is the one site outside those
two files and is dealt with next. **Two further occurrences in this file were
examined and left**, and they are the reason the numerator is not just "every
hit": `no production caller` at the W6 trigger paragraph is about
`trigger::tick` and is **still true** (re-measured at this commit, in that
section), and `no production caller` in item 11 is W3b history about `resume`,
correctly tensed. **Four more are re-pointed rather than corrected** — the
*"item 15's shape one level in"* pointers in this preamble, the W6 section and
the W8 section, which name a shape the item recorded and not the item's verdict.

**The one site outside this file and `runnable.txt` was reachable only by
widening the pattern from the proposition to its GLOSS, and the two share no
words.** `tests/worker.rs`'s doc for
`a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken`
read *"That is W4 item 15's shape — a property proved about a path
**no deployment takes**"*, present tense,
about `claim::due_retries`. W10 Task 2 corrected exactly that gloss in
`due_retries`' **own** doc — *"'a path no deployment takes' is no longer true of
this query"* — and left this copy standing, which is this branch's *corrected
where found, missed elsewhere* one more time, in the same package that wrote the
correction. Population 1's first pattern, built from the proposition, returns
**nothing** for it: it contains neither *worker*, nor *runs*, nor *caller*.
**That is the same lesson as the line break, one level down** — the first
instrument you reach for is built from the sentence you already know.

**Everything else outside this file is already right, and that is reportable
rather than empty**: Tasks 1–6 corrected `egret-config/src/lib.rs:437`,
`egret-workflow/src/lib.rs:21`, `worker::claim`'s `wake` doc **and** its
`due_retries` gloss, `runtime.rs`'s module doc, and the programme spec's W3b row
**in both languages**, each as a dated correction rather than a deletion. The
remaining repository-wide matches are `egret-dataapi`'s, about a different
subject entirely, the W8 plan's and spec's *"a path no deployment takes"* — W8-era
documents describing the state as it was, left as records — and the W10 plan's
and spec's quotations of the sentence being corrected, which are correctly
tensed (*"Until W10 ships…"*, *"Chừng nào W10 chưa ship…"*). Together they are
the control that says the matcher was capable of a hit somewhere other than
here.

**Population 2's nine**: five are the item-list total — `thirteen`,
`seventeen`, `seventeenth`, `eighteen`, `eighteenth` — retired as a family by
**removing the total rather than raising it to nineteen**; three are the
caller-file count (*"the four files that call them"*, *"Four files reach them,
all four on the test side"*); and one is *"exercised by exactly two things"*.
**Four of the nine are corrected by a dated retirement banner over the block
that contains them rather than by editing the numeral**, because those blocks
are W4's own record and renumbering a record makes it a worse record. *An
ordinal shares no letters with what it names*, which is why this is its own
population and not a by-product of population 1.

**Population 3's zero has a positive control**: the same scan over `probe.json`
for `why` returns **37** and over `harvest.py` for `runnable` returns **40**, so
"the generator's inputs carry none of this" is a statement about the inputs and
not about a scan that was reading nothing. `runnable.txt`'s block **is**
corrected, but it sits below `HANDWRITTEN_SENTINEL`, in the half
`--rewrite-runnable` preserves, so it belongs to population 1.

**Population 4's zero has three positive controls**, and they matter because the
Vietnamese is a *translation* and shares no letters with an English grep — so a
pattern built from the English would return zero for a file full of the thing.
Seventeen occurrences across six files, all already right: the VI programme spec
carries this proposition at §9's W3b row — *"vòng claim và reaper ship ra dưới
dạng hàm KHÔNG có caller production nào"* — written by Task 1 as a dated
correction naming W10 as the row that closes it; it carries the publish census
at **248 trên 345** and the retired *"16 trên 345"* argument **inside its own
retirement**, both written by Task 6; and the VI W10 spec is correctly tensed
(*"Chừng nào W10 chưa ship…"* — *"until W10 ships"*). The sweep found them; it
did not need to change any of them. **This sweep was re-run flattened after the
first pass used `grep`**, and the flattening is what surfaced *"mã không ⏎
deployment nào chạy"* — the same wrap, in the other language.

**Population 5's one correction is the shape this population exists for.** The
W3c section says a guard *"was not left that way"* and names
`a_skipped_run_still_gets_a_happens_before_endpoint` as the test that closed it.
**No test by that name exists**, and it is not a typo:
`tests/executor.rs`'s own deleted-test registry lists it with an empty successor
and the note *"Deleted with the four-tier harness at W3b Task 5. There is no
successor: the tier each pinned does not exist."* So the paragraph asserting a
hole was closed is the only thing standing where the guard used to be, and the
hole is open. Annotated in place, not fixed — writing a successor means choosing
which tier it pins, and that is a W3a decision this task has no standing to
take. The other five absences are records that already say so: two are this
file's own findings about names no test ever had, `dynamic_behavior` names a
rule W8 Task 3 fix round 2 explicitly withdrew, `simulate_recording_spawns` is
W3b Task 4's record of a harness Task 5 removed, and `egret_test_template` lives
in `egret-test-db`.

#### The figures W10 retires

**The rule applied here is Task 6's**: *prefer removing a numeral to maintaining
one.* Rewriting `driver.rs`'s *"the reason `LIVE` is 21 and not 295"* as *"the
reason `LIVE` is not 345"* — no numeral left to go stale — immediately exposed
**two more wrong counts** that a sweep for the numerals already known about
would not have reached: *"an allow-list of the **eight** `egret.*` types"*, which
had been **seven** since W6, and a *"this table is **fourteen** rows"* written
in the same round's own fix round 1, which is **thirteen** (7 + 2 + 4). The
family below is why the rule is worth stating: it has now moved **three times**
(16 → 21 → 248) and every move left prose copies behind.

| where | what it says | why it is not current |
|---|---|---|
| *W6 Task 1*, the census table and the paragraphs around it | `live version` **16**, `draft only — unported task type` **301**, `draft only — validate` **19**, *"301 of the 320 refusals are one rule"*, *"the 16 that do publish are not a random 16"*, *"16 flows and 18 `RefKind::Subflow` lookups"*, *"the other 301 drafts"* | **248 / 68 / 20**, 47 lookups on published and 2 on refused. Superseded **twice** — by W8 Task 3 (16 → 21) and by W10 Task 6 fix round 1 (21 → 248) — and the section carried no marker for either. **Banner added at the head of that section rather than renumbering a task's own record**; every current value is a `const` in `driver.rs` that `driver::census` compares on every run |
| *W6 Task 1*, consequence 1 | *"A resolver that went through `flow_versions` would resolve **2 of 20**"* | **18 of 20.** This one is an **argument**, not a record, and W10 Task 6 retired it in `worker::subflow`'s doc and in the programme spec in both languages while this copy went on making it. Struck in place |
| *W8 Task 3*, *Five corpus flows PUBLISH now* | `LIVE` 16 → 21, `UNKNOWN_TYPE_REFUSED` 301 → 296, the lookup pair 18/4 → 19/3 | correct as W8's record of W8's move; **pointer added** to where the family stands now, because a reader landing on the most recent-looking of the three copies gets the middle one |
| this statement's preamble | *"it was thirteen and it is seventeen … item 15 … nothing in this workspace runs this worker"* | the proposition is false since W10 Task 2, **in the sentence that tells a reader where to start**. The total is now **not stated at all** rather than corrected to nineteen, for the rule above |
| *W3c*, the M11 paragraph | *"`a_skipped_run_still_gets_a_happens_before_endpoint` drives `simulate_with` with a `skipping_machine` of its own"* | **no test by that name exists**; it was deleted at W3b Task 5 with no successor. Annotated in place — the guard it claims to have closed is open |
| `tests/worker.rs`, `a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken` | *"W4 item 15's shape — a property proved about a path **no deployment takes**"* | false since W10 Task 2. **W10 Task 2 corrected this exact gloss in `claim::due_retries`' own doc and left this copy standing** — *corrected where found, missed elsewhere*, inside the package that wrote the correction. Reachable only by sweeping the **gloss** as well as the proposition, and the two share no words: this sentence contains neither *worker*, nor *runs*, nor *caller*. Corrected in place |
| item 3 / item 3b | *"reaches zero corpus entries"* | still true, and now **not the only reason**: `egret.sql`, `egret.container` and `egret.script.python` are registered in **neither** registry. Added rather than replaced |
| the **provenance rule** under its own table | *"everything in **rows 2–7** is a figure a reader must treat as a measurement someone took"* | the table ended at row 7 when that was written and three packages have appended under it since — **found while appending to it**, which is the only reason it was found at all. Replaced by *"every row the second column marks `no`"*, and **no count put in its place**. This is the third time this file has caught a rule that ages by position rather than by content (*"every number below"*, *"rows 2–7"*, and the ordinals), and it is the same defect each time: a pointer that the next append silently invalidates |

**And one figure this section states with its date and its provenance, because
that is what made its predecessor wrong.** The bare-`#[test]` shortfall over
`tests/executor.rs` + `oracle-exec/driver.rs` is **19** at `054ebdf6` on
2026-09-15 — 39 bare `#[test]`, 18 `#[tokio::test]`, 1 `#[sqlx::test]`, 58 in
total — derived here and not quoted. It was `16` when W8 Task 2 measured it and
`19` since `bf0782ea`; it is a count of what a wrong instrument misses, so it
moves whenever anybody adds an async test, and **a figure like that is wrong the
moment it is carried undated.**

#### Fix round 1 — a file that retracts a claim and then goes on making it

**Two stale claims were named to this round and the sweep found a third**, in a
surface neither of the two lives in and in **both languages**. All three are one
shape: *a correction landed where it was found and nowhere else.*

**The population, and why it is not "the two I was handed".** The same three
mechanisms that made W10 Task 7's own sweep 8→15 apply again, so the scan is
defined the same way: every tracked file, **flattened before matching**, for the
claim **and its gloss**, and separately over `docs/vi/` where the text shares no
words with the English at all.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the retired `flow_versions`-resolver argument — `2 of (the) 20`, `would resolve 2`, `bank nothing`, `17 are draft`, plus the glosses `version-based resolver`, `resolver reading flow_versions`, `two of the twenty` | **3696 files**, 11 occurrences | **3** |
| 2 | the route claim — `no product route`, `version snapshot`, `starts one at all`, `starts an execution from a` | **3696 files**, 9 occurrences | **2** |
| 3 | **the false cross-reference the Critical named** — `same two doors`, `both doors`, `closes in Rust`, `startable_flow`, and the Vietnamese `đóng đúng hai cánh cửa`, `đóng bằng Rust` | **3696 files**, 22 occurrences | **2** |
| 4 | the generator's inputs (7 data files + 3 Python sources) | **10 inputs** | **0** |
| 5 | `docs/vi/`, flattened, for all three claims | **279 files** | **1** (inside population 3) |

**Population 1 — 3 corrected, and the site is this file's own voice.** The
argument *"a `flow_versions` resolver would resolve 2 of 20 … this package would
bank nothing"* was retired at W10 Task 6 fix round 1 in `worker::subflow`'s doc,
in the programme spec **in both languages**, in `tests/worker.rs`'s `file_flow`
fixture, in `driver.rs`'s `SUBFLOW_TARGETS_LIVE`, and in this file's *W6 Task 1*
section. It then stood **unannotated 340 lines below this file's own
retraction**, in *Resolution: the draft row*, stated flat. **A file that
retracts a claim and then repeats it further down has not retracted it** — and
the reason the earlier sweep's population 1 did not catch it is that the sweep
was defined over *the proposition about the worker*, not over this one. Two
neighbouring sentences in the corrected paragraph went with it.

**Population 2 — 2 corrected, and only half the sentence was false.** *"Nothing
in `crates/*/src` starts an execution from a version snapshot — there is no
product route that starts one at all yet."* The second clause is false: there
are **three** start routes plus replay, and W10 Task 5 added the third. The
first clause is **still true**, and is now the stronger statement because there
are routes for it to be true of — `flow_versions` is read in `crates/*/src`
only by `repo.rs` and surfaced by the read-only `routes::list_flow_versions`,
and every start path passes either `flows.source` or `executions.source`.
**Verified against the shipped routes rather than against the sentence**, which
is what the brief asked and is the only reason the half that survived was not
struck with the half that did not.

**Population 3 — the sweep's own yield, and it is the Critical's false
mechanism.** `24855ba7` corrected *"`resolve` closes the same two doors
`routes::startable_flow` closes"* in `routes.rs` and `subflow.rs` — and left it
standing in `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §6.1,
where it reads as a cross-reference rather than an assertion. `startable_flow`
closes **three** doors (trash, `state` column, YAML `disabled:`); `resolve`
closes **two** and cannot close the third, because `disabled:` is not a column.
Both language copies corrected, each pointing at `spawn_one` as where the third
door actually lives. **The Vietnamese shares no words with the English** — it
reads *"nó đóng đúng hai cánh cửa bằng SQL"* — so an English-pattern sweep
returns zero for a file that carries the claim, which is population 5's whole
reason for existing.

**No denominator equals its numerator.** Populations 1–3 each left occurrences
uncorrected **on purpose**, and naming them is what makes the numerators mean
something: the retraction rows in this file's *figures W10 retires* table and in
`driver.rs`/`subflow.rs`/`tests/worker.rs` are correct **as retractions** and
quote the retired text deliberately; `repo.rs:33`'s *"version snapshots and
notes"* and the EN spec's *"frozen into the version snapshot"* are a different
subject; `tests/executor.rs:5191`'s *"bank nothing at all"* is about W8 Task 2's
sixteen. Those are the controls that say the scan was capable of a hit it should
not act on.

#### What this round changed in item 19, and why the mechanism was too wide

Item 19 said a handler-less publishable type hangs because *"the run keeps the
lease"* and the reaper clears it. **That is one cause of two, and the mechanism
was imprecise.** `reap` clears **only** `claimed_at`; the state stays `RUNNING`
and the attempt stays open, and `claim`'s own predicate
`state = 'RUNNING' AND claimed_at IS NULL` is what re-selects the row. Nothing
counts the cycles — `attempts.push` is in one file and neither site is on this
path — so one `warn` per cycle is the entire signal, and **9 of 13 publishable
types** are in this condition on the shipped registry. *(W5a Task 6,
2026-09-28: **7 of 15** now; see item 19 (a). W9a Tasks 11-12 added nine
core storage rows, all with handlers: **7 of 24** since; Data Apps P6a (#629)
removed `egret.python`: **6 of 23** since.)*

**The second cause was not recorded anywhere in this statement**: a handler that
**outruns** `working_timeout`. `Sleep::new()` is uncapped and is one of the four
registered handlers, so `duration: PT2H` is reachable on a flow anyone may
publish — reaped at 1 h, re-claimed, a *fresh* two-hour sleep started, and the
first attempt meeting the fence with `LeaseLost`, for ever, each cycle costing a
new handler. **The distinction is load-bearing rather than taxonomic**: a
give-up-after-N rule closes the first and quietly converts the second into a
**wrongly failed** run, so a statement that carried only the first would have
recommended a fix that breaks the second.

Item 19 also gains the Critical: **"off" means two things**, `resolve` read only
one of them, and a flow whose YAML said `disabled: true` answered 409 to the API
and **ran anyway as a `Subflow` target**. What belongs here is not the fix but
the instrument: **neither of `resolve`'s two stated doors had a test** — both
deleted, `--test worker` stayed at 59/0, against a workspace control that failed
58/1 — and the ratchet reads **109** through every version of it.


#### What this package could not settle

**There is no ratchet movement to attribute and no mutation table to lean on**,
which is the condition this package's Global Constraints named at the start: with
nothing to lean on, every claim has to be pinned by a test that fails when the
claim is false. The tests that do that for W10 are in `tests/runtime.rs`,
`tests/executions_routes.rs`, `tests/execution_repo.rs` and
`tests/end_to_end.rs`, and they are the package's evidence — not this file's
`109`, which would read exactly the same if all four were wrong.

**Three things this pass left open rather than closed, named so the next reader
does not mistake them for finished:**

1. **The W6 Task 1 census is annotated, not renumbered.** Its table still reads
   16 / 301 / 19 under a banner saying all three are three generations stale.
   That is a judgement — a task's own record is worth more unaltered than
   rewritten — and it is the *weaker* option: a reader who skips the banner gets
   three wrong numbers. The structural fix is the one `driver.rs` already has,
   which is for prose to stop carrying the family at all.
2. **The guard `a_skipped_run_still_gets_a_happens_before_endpoint` was meant to
   close is still open.** Population 5 found the citation dangling; writing a
   successor means deciding which tier it pins, which is a W3a decision this
   task has no standing to take. Recorded in place, unfixed.
3. **Every sweep here is a search, and a search is not a proof of absence.**
   Populations 3 and 4 report zero, each beside a positive control that says the
   instrument could have found something — which is the most a search can offer.
   The only population with a mechanical guarantee is the call-site derivation,
   because the compiler agrees with it.

### W12 — the trigger loop fires in production now, and the oracle still cannot see a single tick

**`109` before W12 Task 1, `109` at this commit, and `109` with `fire_due`
deleted.** The same three-number method W10 used, and for a related but not
identical reason: W10's channel was empty because nothing called the worker
loop; W12's is empty for **two** independent reasons stacked on top of each
other, and merging them would lose the one a future package would have to fix.

1. **The wiring gap, exactly W10's shape.** `driver::run` never calls
   `scheduler::sync` or `scheduler::fire_due` — `crate::scheduler::` and
   `crate::runtime::` are each **0** occurrences in `oracle-exec/driver.rs` and
   `tests/executor.rs`, against `worker::` at **26** and **37** on the same
   command (the identical positive control W10 used, unmoved, because neither
   file has changed since). The oracle's driver builds executions through
   `worker::start_execution_with_trigger` and ticks them through
   `worker::tick`; nothing in it claims a `workflow.triggers` row.
2. **The data-shape gap underneath it, which predates W12 and is not W12's to
   fix.** Even a driver that called `fire_due` would create executions the
   golden comparison cannot place: migration 0156 adds `scheduled_date` and a
   `CHECK` tying it to `execution_trigger->>'type'`, but it does **not** give a
   `Schedule`-fired execution a `parent_id` or any column `descendants()`
   walks — because Kestra doesn't either. This is the W6 section's *"the
   execution exists and hangs off nothing"* row, re-measured rather than
   assumed: `parent_id` stays generated and NULL for every row
   `start_execution_in` writes with `flow_id` set and no caller-supplied
   parent, `scheduled_date`-bearing or not. **W12 built the loop; it did not
   -- and could not, without changing what a `descendants()` union walks --
   change what the ratchet can see fire.**

Both are structural, not measured-once-and-argued: (1) is a `git ls-files` grep
result that a future call site would flip, (2) is a schema fact `0156` itself
states in its own `CHECK`. Neither can be shown false by running more tests
against this corpus, which is the same limit the W6 section already recorded
for `Schedule` — W12 inherits the blindness rather than introducing a new one.

| | ratchet | what the task shipped |
|---|---|---|
| package base `6fb7520e` (origin/main, W10's tip) | 109 mismatching, 109 baselined, 0 broken | — |
| Task 1 | 109 | `oracle-exec/w12-schedule-trigger.json` — the pinned artifact's own `Schedule.Output`, measured rather than guessed: variables are exactly `{date, next, previous}`, RFC 3339 with the trigger's own zone offset (not `Z`), six fractional digits |
| Task 2 | 109 | migration 0156 — `workflow.triggers`, one row per `Schedule`; `scheduler::sync`, called from `FlowRepo`'s three writers that can change what fires (`create`, `update`, `soft_delete`) — `set_live` is exempt, since it changes neither `source` nor `deleted_at` and `state` is read at fire time |
| Task 3 | 109 | `start_execution_in` — `start_execution_with_trigger`'s body, taking the caller's own transaction rather than opening one, so a later caller inside a transaction of its own can create the execution in it |
| Task 4 | 109 | `scheduler::fire_due` — claim, tick, start, advance, one trigger per transaction; the unique-index backstop and the retry/consume/take-off-queue classification |
| Task 5 | 109 | `runtime.rs`'s third ticker, behind `scheduler_enabled` (default `false`); the chart's lever |
| Task 6 | 109 | this section, and the sweep below |

#### `fire_due` deleted, measured rather than assumed

Mutation, this task, on the working tree (reverted immediately after, never
committed): `fire_due` given an unconditional early `return
Ok(PassReport::default())`, so the claim, the tick, the start and the advance
-- everything below the new line -- is unreachable, kept in the file behind
`#[allow(dead_code, unreachable_code, ...)]` only so the crate still compiles
(deleting the functions outright would also have deleted their own unit tests
in the same file). `sync` untouched -- it is the write path `scheduler_sync.rs`
and `runtime.rs`'s `a_save_that_meets_a_claimed_trigger…` still exercise, and
gutting it would answer a different question. Evidence the mutation landed,
printed before the verdict was read: `git diff --stat` reported
`crates/egret-workflow/src/scheduler/fire.rs | 9 +++++++++` (9 insertions, 0
deletions -- the early return and its attributes; nothing deleted, because an
early return, not a deletion, is what makes the body unreachable) and the
marker comment it carries, `W12 Task 6 mutation`, occurred **1** time in the
file. `executor_matches_golden` then read **294 golden entries, 109
mismatching, 109 baselined, 0 broken** -- byte-identical to the control, 1 run
against 1 (the earlier, already-reported `executor_matches_golden ... ok` run
on the unmutated tree, same build, same session -- this test carries no
randomness anywhere in its own corpus walk, so a second control run would
print the same thing rather than add information). Reverted with `git checkout
-- crates/egret-workflow/src/scheduler/fire.rs`; `git diff` against `HEAD` was
confirmed empty and the marker comment's count confirmed **0** before
continuing.

This is the weaker half of the two gaps above, and it is reported knowing
that: it re-confirms (1), the wiring gap, and says nothing about (2), because
deleting `fire_due`'s body cannot un-delete a `parent_id` the migration never
added. A mutation that could move the ratchet by restoring a link column does
not exist to run.

#### The safety argument is not this file's; it is one call away

`scheduler::mod`'s own doc (*Safety with N replicas, per statement*) carries
the concurrency argument -- the claim's `FOR UPDATE OF t SKIP LOCKED`, the
unique-index backstop on the execution insert, the final `UPDATE` under the
claim's lock, and `sync`'s upsert serialised by the row lock -- with its own
mutations (two, `tests/scheduler_fire.rs`'s `two_scheduler_tickers_never_fire_one_tick_twice`
killing both) and its own test names. It is not duplicated here because a
copy would be the fifth thing in this file's own history to go stale by
position (the *"rows 2-7"* pointer W10 Task 7 found ageing under exactly that
failure mode) -- one link, kept current by pointing at the module rather than
quoting it.

#### The sweep: "a cron cannot fire in production" is retired, and where it actually lived

The phrase was handed as five literal patterns, plus two ordinals and a
generated-data pass -- W10 Task 7's discipline, unchanged: every tracked file
flattened to one line before matching, so a phrase wrapping a line break is not
invisible, and a denominator computed over `git ls-files` so it is re-runnable.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the phrase, English, whole repo -- `cannot fire`, `NOT BUILT: W12`, `Not built: W12`, `constructed only in tests`, `no caller outside` | **3811 files**, 18 matching `cannot fire` (15 files), 1 each for the other four (3 files total, 2 of them the same site) | **2** |
| 2 | ordinals/counts in the regions population 1/1b landed in -- `one caller`, `exactly one`, VI `đúng một … caller` | **4 files** (the two EN spec passages population 1 corrected, plus their VI twins), 1 occurrence each once markdown emphasis is stripped (`**exactly one** caller` defeats a literal `one caller` grep -- the same line-break-shaped miss W10 Task 7 named, one level down, in `**`) | **4** |
| 3 | data that feeds a generator -- the 13 files under `oracle-exec/`: 9 data files (`baseline.txt`, `corpus.json`, `golden.json`, `handler-surface.txt`, `harvest-run.txt`, `probe.json`, `runnable.txt`, `stability.json`, `w12-schedule-trigger.json`) plus 4 generators (`driver.rs`, `harvest.py`, `normalise.py`, `handler_surface.py`) | **13 inputs**, 0 matching any of the five phrase patterns (positive controls: `why` 37/5/0/…, `runnable` 0/7/2/41/…, `schedule` 1/13/3/…, each nonzero somewhere in the set) | **0** |
| 4 | the other language -- every file under `docs/vi/`, flattened, for `không thể nổ`, `không có caller nào ngoài`, `đúng một … caller`, `CHƯA DỰNG: W12` | **279 files**, 2 matching (the VI programme spec's W12 row, the VI W10 spec) | **2** |

**Population 1's own denominator is data, not a coincidence**: three of its
five patterns (`NOT BUILT: W12`, `Not built: W12`, `constructed only in
tests`) return **zero** real sites -- the only place they occur at all is
`docs/en/plans/2026-09-24-workflow-w12-trigger-loop.md`'s own Task 6 step
text, which names them as search targets rather than asserting any of them.
The actual sites use different words -- `designed in`, `cannot fire`,
`exactly one caller` -- which is itself the finding: a list of patterns
compiled before a package ships describes what its author expected the stale
text to say, not what it turned out to say. `cannot fire`'s 18 occurrences
across 15 files are mostly a different subject entirely and are the negative
population this sweep is supposed to leave standing: `egret-dataapi`'s two
(`ratelimit.rs`, `lifecycle.rs`, about an `expect`/`saturating_sub` that
cannot panic), `oracle-exec/driver.rs`'s one (a `#[sqlx::test]` ordering
comment), `runners/env.rs`'s *"no caller outside `runners`"* (module privacy,
not this proposition), five frontend files (a disabled-button/cannot-submit
sense), and the dashboard-parity plan's two (an `undefined`-guarded branch).
Corrected: the programme spec's W12 row (§9) and the W10 spec's §1, each
carrying the measurement dated and annotated in place -- *corrected where
found* is not enough on its own, which is why population 4 re-runs the same
question in translation rather than trusting that a fix in English reaches
its Vietnamese twin.

**Population 2's grep-defeating markdown is the same shape as W10 Task 7's
line-break miss, one level in.** `` `TriggerState::initialise` has **exactly
one** caller `` renders, flattened, as `exactly one** caller` -- the `**`
between the ordinal and its noun breaks a literal `one caller` match even
though a reader sees one contiguous claim. `exactly one` alone is a hopeless
grep repo-wide (509 occurrences, most of them unrelated arithmetic in
unrelated crates), so this population is scoped the way W10 Task 7 scoped its
own population 2 -- **to the regions population 1 already found** -- rather
than swept blind. The two English hits are the same sentence in two documents
-- the programme spec's W12 row and the W10 spec's §1 -- corrected in the same
edit as population 1's; the two Vietnamese hits are that same claim in
translation, not a copy of the English words, corrected alongside population
1b's pass rather than swept separately.

**Population 3's zero is a scan that could have found something and did
not**, not a scan that found nothing to look at: `why`, `runnable` and
`schedule` are all present somewhere in the thirteen inputs (`probe.json`'s
`why` alone is 37), so an instrument capable of matching text in these files
was run and came back clean. None of the corpus/golden/harvest data carries
prose at all -- the proposition could only leak in through a Python
generator's docstrings or `harvest.py`'s prose, and it does not.

**Population 4 is why this sweep does not stop at population 1.** The two
Vietnamese sites are translations, not copies -- `không thể nổ trong
production` shares no characters with `cannot fire in production` beyond the
Latin alphabet itself -- so an English-only sweep would have reported this
proposition retired while its Vietnamese twin still asserted it. Both
corrected in the same pass as their English originals, each dated
2026-09-25 rather than silently rewritten.

**No denominator equals its numerator.** Fifteen files matched `cannot fire`
and thirteen of them were left standing, on purpose, as the control that says
the pattern was capable of a hit this proposition does not own.

**One more site, reachable only by widening the pattern to the proposition's
GLOSS rather than its words -- the same shape W10 Task 7 found in
`tests/worker.rs`.** `crates/egret-workflow/src/lib.rs`'s own module doc read
*"W6 built the predicate as a pure function and no table, so nothing here
fires in production"*, present tense, in the one file every reader of this
crate opens first. It shares no word with any of the five patterns -- no
*cannot*, no *fire in production* as a contiguous phrase -- and would not have
been found by population 1 at all. Corrected in place, dated in this file's
own commit rather than the spec's, because a module doc is closer to the code
it describes than a spec is: *"so nothing here fired in production until
**W12** built `workflow.triggers` and the loop that claims it
(`scheduler::fire_due`, spec §9) -- behind `scheduler_enabled`, default
`false`"*.

### W13a — the flow trigger fires in production now, and the oracle still cannot see it

**109 before W13a Task 1, 109 at this commit, and 109 with `process_events`
deleted.** The same three-number method W10 and W12 used, for the reason
W12's own section already names as the general shape: `oracle-exec/driver.rs`
never calls `flow_trigger::process_events` or `trigger::flow::evaluate` --
`flow_trigger::` and `process_events(` are each **0** occurrences in
`oracle-exec/driver.rs` and `tests/executor.rs`, against `worker::` at **26**
and **37** on the same command, the identical positive control W10 and W12
used, unmoved because neither file has changed since. The oracle's driver
builds and ticks executions through `worker::start_execution_with_trigger`
and `worker::tick`; nothing in it claims a row of
`workflow.flow_trigger_events`.

This package inherits W12's second reason too, and it predates W13a rather
than being introduced by it: `trigger::flow`'s own module doc measured, before
Task 1 wrote a line of production code, that **zero baselined golden entries
are reachable through a flow trigger at all** -- a `Flow`-fired execution
carries `trigger_execution_id` and `parentId` NULL, `harvest.py`'s settle walks
`parent_id`, and `baseline.txt` reads **127 before this module and 127
after**. Wiring the seam in production cannot change what `harvest.py` walks,
so this reason is unaffected by anything Task 1-4 shipped and is restated
here rather than re-derived.

| | ratchet | what the task shipped |
|---|---|---|
| package base `006563ee` (origin/main, W12's tip) | 109 mismatching, 109 baselined, 0 broken | -- |
| Task 1 | 109 | Kestra measurement on the pinned artifact (`oracle-exec/w13-flow-trigger.json`) -- `execution_trigger`'s nine `variables`, the depth ping-pong to 100, and the born-FAILED `inputs` fact, corrected in fix round 1 after an initial reading had it as a refusal; the baseline gate re-measured (3835 unique test names) |
| Task 2 | 109 | migration 0157, `workflow.flow_listeners`; `scheduler::sync` extended to keep it in step with `flow.source`, called from the same three `FlowRepo` writers W12's row names (`create`, `update`, `soft_delete`) |
| Task 3 | 109 | `NewExecution`'s link and `execution_depth`, the source's render context, and the `settle` outbox hook -- one conditional `INSERT`, the only statement this PR adds to `settle`'s hot path (spec §4.3); a mid-task ruling corrected the hook to write one event per state a settle APPENDS, not only the last |
| Task 4 | 109 | `flow_trigger::process_events` and its tests -- a per-listener savepoint, `MAX_ATTEMPTS` retry-then-give-up, the six off switches reused from `scheduler/fire.rs`; fix round 1 corrected a controller ruling that had a render failure in `inputs` REFUSE the fire, to match Kestra's own measured born-FAILED execution |
| Task 5 | 109 | this section, and the sweep below |

#### `process_events` deleted, measured rather than assumed

Mutation, this task, on the working tree (reverted immediately after, never
committed): `process_events` given an unconditional early `return
Ok(EventReport::default())`, marked with the comment
`TASK-5-RATCHET-MUTATION`, so the claim loop, every listener check and the
fire itself are unreachable, kept in the file behind
`#[allow(unreachable_code)]` only so the crate still compiles. Evidence the
mutation landed, printed before the verdict was read: `git diff --stat`
reported `crates/egret-workflow/src/flow_trigger/process.rs | 7 +++++++` (7
insertions, 0 deletions -- the early return and its attributes; nothing
deleted, for the same reason W12 Task 6 gave), and the marker comment
occurred **1** time in the file. `executor_matches_golden` then read **294
golden entries, 109 mismatching, 109 baselined, 0 broken** -- byte-identical
to the unmutated control run immediately before it in the same session, same
build (this test carries no randomness in its own corpus walk, so a second
control run would print the same thing rather than add information).
Reverted with `git checkout -- crates/egret-workflow/src/flow_trigger/process.rs`;
the marker comment's count confirmed **0** and `git diff` against `HEAD`
confirmed empty before continuing.

#### The sweep: "nothing calls `trigger::flow::evaluate`" is retired, and where it actually lived

W10 Task 7's and W12 Task 6's discipline, unchanged: every tracked file
flattened to one line before matching, so a phrase wrapping a line break is
not invisible, and every denominator computed over `git ls-files` so it is
re-runnable. **Every figure below is taken at `1ec0f79f`**, Task 4's tip and
this task's own base, over the population as it stood *before* this task
edited anything.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the proposition and its glosses -- `wiring seam` (owed or missing), `no production constructor`, `has no such guarantee` -- across the whole repository, flattened | **3822 files**, 3 matching, **6 occurrences** | **4** |
| 1b | the born-FAILED citation family -- `git grep -n listener_run1_FAILED_bad_inputs 1ec0f79f -- . ':!crates/egret-workflow/oracle-exec/'` | **9 raw hits, 6 files** (2 code key literals + **7 prose citations**) | **1** |
| 2 | ordinals and counts in the five regions populations 1 and 1b corrected | **5 regions** | **0** |
| 3 | data that feeds a generator -- the 14 files under `oracle-exec/`: 10 data files (`baseline.txt`, `corpus.json`, `golden.json`, `handler-surface.txt`, `harvest-run.txt`, `probe.json`, `runnable.txt`, `stability.json`, `w12-schedule-trigger.json`, `w13-flow-trigger.json`) plus 4 generators (`driver.rs`, `harvest.py`, `normalise.py`, `handler_surface.py`) | **14 inputs** | **0** |
| 4 | the other language -- every file under `docs/vi/`, flattened | **287 files**, 1 matching | **0** |

**Population 1's four corrections are all in `trigger/flow.rs`, and all four
are the same shape: a paragraph that said a bridge did not exist yet, written
before Task 1-4 built it.**

- The module doc (*"The fact the whole package's link design rests on"*) and
  [`FiredExecution::correlation_id`]'s own doc both said egret's render walk
  "will render the child's own id" until "the wiring seam" wired it. **The
  seam now exists**: `crate::flow_trigger::process::listen` sets
  `trigger_execution_id` on the fired child, and
  `worker::render::Surface::load`'s causal walk -- corrected at **W6 Task 8**,
  *before* W13a started, to follow whichever link a row carries rather than
  `parent_execution_id` alone -- reaches through it exactly as it already did
  for a `Subflow` child. A flow-trigger child now renders
  `labels.system.correlationId` as the chain's root id, not its own. Both
  doc comments are corrected in place, dated, rather than deleted, because
  each is the record of a decision that was real when it was made.
- The `PAUSED` bridge (`evaluate`'s own doc, next to `StateNotListed`) said
  `SourceExecution` had **no production constructor** and nothing converted
  an executor `RunState::Paused` into a `models::State::Paused` for the
  predicate to see. **It does now**: `settle`'s outbox hook does not gate on
  terminality -- `PAUSED` is in a Flow trigger's default `states` and is
  written to the outbox like any other appended state (`worker/claim.rs`'s own
  comment says so) -- and `flow_trigger::process::process_one` parses the
  event's stored state back into `models::State` before calling `evaluate`,
  pinned to `RunState::wire()`'s spelling by
  `run_state_wire_is_the_model_states_serde_spelling`. Corrected in place.
- [`Refusal::DifferentWorkspace`]'s doc said *"egret's caller has no such
  guarantee yet"* that a listener and its source share a workspace, unlike
  Kestra's `withFlowTriggersOnly`. **The caller now has it**:
  `flow_trigger::process`'s listener query reads `WHERE l.workspace_id = $1`
  bound to the event's own `workspace_id`, so a cross-workspace pair is
  unrepresentable by the time `evaluate` sees it too -- the refusal is kept as
  the second line of defence rather than the only one, the same shape
  migration 0138's composite foreign keys already are. Corrected in place.

**Two occurrences of `wiring seam` were examined and left, because they are a
different, still-open gap and conflating them would have retired the wrong
one.** This file's own *Instruments and defects handed on* table and
`tests/executor.rs`'s `the_seeded_correlation_id_divergence_has_no_corpus_population`
both name "the wiring seam" as the owner of the **SEEDED** `system.correlationId`
divergence -- a root execution that declares an *explicit*
`system.correlationId` label, which Kestra copies down its whole chain
verbatim and egret's walk cannot, because egret has no column to carry a
label rather than a derived id. That is spec §8's *"the `system.correlationId`
label"*, listed there as deliberately not in W13 at all, and it is untouched
by anything Task 1-4 built: the seam this task closes lets the walk *reach*
the source through a flow-trigger link, not carry a seeded string instead of
the id it derives. Both sites are correctly tensed and are left as the
control that the sweep does not retire everything it matches.

**Population 1b is a different shape of defect: not a stale tense, a
citation that pointed at the wrong mechanism.** `flow_trigger::process`'s
`FORBIDDEN_ROOTS` doc cited `listener_run1_FAILED_bad_inputs` -- the measured
Kestra throw, *"Unable to find `trigger`"* -- as what its own guard,
`render_str_guarded`, "refuses to reach". **The fixture is `inputs`, not
`when`, and `inputs` is not this guard's surface at all**: it renders through
`render_value`, a different function, a few lines below, and a failure there
is **not** a refusal -- it is the born-FAILED execution
(`inputs_that_fail_to_render_create_a_failed_execution`). The citation was
accurate about the underlying Kestra fact and wrong about which egret
mechanism it illustrated, which is why it reads, uncorrected, as evidence
that naming `trigger` in a `when` refuses the same way.

**The command, so the count does not have to be trusted:**
`git grep -n listener_run1_FAILED_bad_inputs 1ec0f79f -- . ':!crates/egret-workflow/oracle-exec/'`
(the exclusion drops only the generated `oracle-exec/` inputs population 3
already covers). **Raw output: 9 hits, in 6 files.** The rule that splits
them: a hit inside a Rust string literal used as a data key -- indexing a
parsed JSON fixture, or naming one in a test-data tuple -- is not a
*citation* of the measurement, it is the SAME string doing the job a key
does; a hit inside a doc comment or prose sentence citing the measurement as
evidence IS a citation. That is **2 key literals + 7 prose citations**, and
every site, named rather than counted:

| site | line | kind |
|---|---:|---|
| `crates/egret-workflow/src/flow_trigger/process.rs` | 45 | prose -- the `FORBIDDEN_ROOTS` doc, the one corrected |
| `crates/egret-workflow/src/flow_trigger/process.rs` | 544 | prose -- the `inputs` comment |
| `crates/egret-workflow/src/flow_trigger/process.rs` | 776 | **key literal** -- `("listener_run1_FAILED_bad_inputs", "producer_run1")`, a test-data tuple |
| `crates/egret-workflow/src/worker/claim.rs` | 1508 | prose -- the `born_failed` field doc |
| `crates/egret-workflow/tests/flow_trigger_fire.rs` | 315 | prose -- the test's doc comment |
| `crates/egret-workflow/tests/flow_trigger_fire.rs` | 335 | **key literal** -- `o["executions"]["listener_run1_FAILED_bad_inputs"]`, indexing the fixture |
| `docs/en/plans/2026-09-25-workflow-w13a-flow-triggers-window-free.md` | 216 | prose -- the plan's own fix-round-1 amendment note |
| `docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md` | 150 | prose -- the design spec's §4.4 walkthrough |
| `docs/vi/specs/2026-09-25-workflow-w13-flow-triggers-design.md` | 160 | prose -- its VI twin |

Six files, not five: the plan doc is the sixth, and no earlier pass of this
sweep had named it -- the same *"a list of patterns compiled before a
package ships describes what its author expected"* shape W12 Task 6 named,
here applied to a list of FILES rather than a list of patterns. **Of the 7
prose citations, 1 is corrected** (`process.rs:45`) **and 6 were already
correctly framed**: `process.rs:544` and `claim.rs:1508` say *"NOT a
refusal"* in as many words; the design spec (`:150`) says *"does NOT refuse
the fire"*; its VI twin (`:160`) says *"KHÔNG làm lần bắn bị từ chối"* --
four of the six stating it in as many words. The other two convey the same
fact without that phrase: `flow_trigger_fire.rs:315`'s doc says Kestra
*"STILL creates the listener's execution"*, and the plan's own note
(`:216`) says the line "first said the fire is refused, which misread the
measurement" and then states the corrected fact without repeating the word
*"refusal"*. **Six correctly-framed sites, four of six in as many words**,
which is the positive control unchanged in substance from the earlier round:
this package usually gets the distinction right, and the one site that did
not is the one nobody had reread since Task 1's first, wrong draft of it.
Corrected in place, the guard kept as `when`-only and the citation kept
because it remains the only measured *"Unable to find `trigger`"* this
package has -- reworded to say what it is evidence of rather than removed.

**Population 2's zero is not vacuous**: the five corrected regions above
carry other numerals untouched by this correction -- *"five id columns"* and
*"Migration 0138"* in the `DifferentWorkspace` doc (a different,
still-accurate historical fact about the STS cross-workspace incident), and
*"W6 Task 8"* named twice, in the module doc and in
[`FiredExecution::correlation_id`]'s doc, both correctly -- so the scan reads
content rather than skipping past digits, and none of them described the
wiring seam's own status. (The SEEDED-divergence table row's *"22 corpus
flows... 3... 2 flows"* is a different, LEFT-STANDING region -- population
1's control, not population 2's population -- and is untouched for the
reason given above, not because this scan missed it.)

**Population 3's zero has positive controls the same way W10 Task 7's and
W12 Task 6's did**: `why`, `runnable`, `schedule` and `trigger` are all
present somewhere across the 14 inputs (`probe.json`'s `why` alone is **37**),
so an instrument capable of matching text in these files was run and came
back clean for the three wiring-seam patterns. `w13-flow-trigger.json` itself
carries **2** occurrences of `listener_run1_FAILED_bad_inputs` -- the
artifact's own measurement narrative, the source the population 1b citations
quote from, not the mischaracterised copy -- left standing on purpose.

**Population 4's zero is a found-and-already-right, not a not-found.** The
one Vietnamese hit, the W13 design spec's own §4.4 walkthrough, reads *"Render
lỗi KHÔNG làm lần bắn bị từ chối"* -- "a render failure does NOT refuse the
fire" -- which already states the corrected fact population 1b's English
citation had gotten backwards in one place. Nothing to change; recorded
because a sweep that only reports corrections cannot be told apart from one
that skipped this file.

#### What this package could not settle

**W13b has not started.** No `window` model, no `TimeWindow` port, no
migration 0158, no `dependsOn` processing -- the W13 spec's §7.2 table is the
plan for it, not a record of it, and the programme spec's own W13a/W13b
correction says so in both languages. Twenty of the corpus's twenty-one flow
trigger declarations carry a `dependsOn` and are refused at
`WindowFreeFlowTrigger::resolve`, unmoved by anything in this package.

*Retired by W13b Task 5 (2026-09-26), kept as the record of when it was
true:* W13b has shipped the `window` model, the `TimeWindow` port, migration
0158 and the `dependsOn` processing, and §7.2's table is a record now -- every
row's mutation was run and killed (see *W13b* below). `resolve` still refuses
the twenty, by design: the processor routes a `dependsOn` listener to its own
arm before `resolve` is called.

**The SEEDED `system.correlationId` divergence stays open, by the same
decision that scoped it out of W13 entirely** (spec §8). Closing it would
need a column egret does not have, to carry a copied label rather than a
derived id, and the corpus cannot even exercise it: zero of the 22
label-declaring corpus flows declare `system.correlationId` itself.

**Render-context correctness for a `PAUSED`-then-resumed source is a
mechanism this file can point at, not a golden entry that can see it.** The
bridge population 1 closes lets a `PAUSED` source fire a listener with the
right `models::State`; whether the render CONTENT a resumed source's later
settle produces still matches what the paused moment saw is the whole-branch
review's question (globals.md item 4), not this ratchet's.

### W13b — `dependsOn` fires inside a window now, and the oracle still cannot see it

**109 before W13b Task 1, 109 at this commit, and 109 with the `dependsOn`
branch deleted.** The same three-number method W10, W12 and W13a used, and
for the same structural reason: `oracle-exec/driver.rs` and
`tests/executor.rs` name none of W13b's code. `flow_trigger::`,
`process_events(`, `sweep_expired_windows`, `depends_on::` and
`listen_depends_on` are each **0** occurrences in both files, against
`worker::` at **26** and **38** occurrences on the same command (the positive
control). The 38 is on 37 lines of `tests/executor.rs`, at `741169f9` and at
this commit alike -- W13a's "37" is that line count, not a change this branch
made; this branch's diff to the file adds or removes no `worker::`. W13a's second reason is unchanged too: a
`Flow`-fired execution carries `trigger_execution_id` and no `parent_id`, so
`harvest.py` cannot walk to it and zero baselined golden entries are
reachable through any flow trigger, windowed or not.

| | ratchet | what the task shipped |
|---|---|---|
| package base `741169f9` (origin/main, W13a's merge) | 109 mismatching, 109 baselined, 0 broken (`executor_matches_golden` ok in Task 1's baseline gate) | -- |
| Task 1 | 109 | the window measured on the pinned artifact (`oracle-exec/w13b-windows.json`): bounds of the four `TimeWindow` kinds and midnight (in UTC and two zones without DST, Asia/Kolkata and Asia/Ho_Chi_Minh; **DST was not measured** -- the container was UTC and no case crossed a transition -- so DST is derived from `java.time`, see `trigger::window`'s module doc), expiry, the mode sequences, the condition keys; user decisions D1-D3 (replace an expired window, only a matching event opens one, the ticker sweeps; `timezone` supported; the measured `DURATION_WINDOW` rule) |
| Task 2 | 109 | `models::Window` (seven fields, `deny_unknown_fields`, so `fireOnce` is refused at parse as the artifact refuses it), ISO durations capped at 366 days, and the save-time refusals; three census constants moved for the one flow that no longer parses |
| Task 3 | not measured (no execution path) | the pure `trigger::window` and `trigger::depends_on`: bounds, `merge`, `satisfied`, the murmur3 condition keys reproducing the artifact's three, and one refusal list shared by save and run time |
| Task 4 | 109 | migration 0158 (`workflow.flow_trigger_windows`), the `dependsOn` arm of the processor (`FOR UPDATE` on the window row, fire then delete under every mode), the ticker's sweep; fix round 1 made a lost `40001` race free of `MAX_ATTEMPTS` |
| Task 5 | 109 | this section, the corpus count and its test, the four §7.2 mutations Task 4 had not run, and the sweep below |

#### The `dependsOn` branch deleted, measured rather than assumed

Mutation, on the working tree at `1e9d68b1` (reverted immediately after, never
committed): in `flow_trigger::process::listen`, the `return
listen_depends_on(..)` of the `dependsOn` branch replaced by `return
Ok(Fate::Refused(..))`, with the marker `TASK-5-RATCHET-MUTATION` -- so no
`dependsOn` listener reaches a window or a fire. Evidence it landed, printed
before the verdict: `git diff --stat` reported `process.rs | 3 ++-` (2
insertions, 1 deletion; 16 changed-line tokens). `executor_matches_golden`
then read **294 golden entries, 109 mismatching, 109 baselined, 0 broken** --
identical to the unmutated control run minutes earlier on the same tree
(`1e9d68b1`, same line, byte for byte). Restored with `git checkout`, and
`git status` / `git diff --stat` were empty before anything else ran.

#### The corpus: eighteen of twenty resolve, and none names a window

`trigger::depends_on::tests::the_corpus_depends_on_declarations_resolve_eighteen_of_twenty`
puts every corpus `dependsOn` declaration through the gate the processor
applies (`depends_on_refusals`, `TimeWindow::from_model`, `condition_keys`)
and through `spec::validate`, and asserts the two agree declaration by
declaration. Seventeen corpus flows declare a flow trigger, with 21
declarations; one is window-free
(`flow-trigger-when-condition-flow-listen/on_completion`), and of the **20**
with `dependsOn`:

- **18 resolve** and would open the default one-day `DURATION_WINDOW`:
  `flow-trigger-any-mode-flow-listen/flow` (ANY),
  `flow-trigger-at-least-mode-flow-listen/flow` (AT_LEAST, `minSatisfied: 2`
  of 3), `flow-trigger-depends-on-flow-listen/flow`,
  `flow-trigger-invalid-inputs-flow-listen/flow`,
  `flow-trigger-mixed-conditions-flow-listen/{on_completion,on_failure}`,
  `flow-trigger-multiple-conditions-flow-listen/{on_completion,on_failure}`,
  `flow-trigger-multiple-depends-on-flow-listen/{flow1,flow2}`,
  `flow-trigger-paused-listen/flow`,
  `flow-trigger-stable-condition-id-flow-listen/flow`,
  `trigger-flow-listener-invalid/listen-flow-invalid`,
  `trigger-flow-listener-no-inputs/{listen-flow,listen-flow-failed}`,
  `trigger-flow-listener-with-concurrency-limit/flow`,
  `trigger-flow-listener-with-pause/listen-flow` and
  `trigger-flow-listener/listen-flow`.
- **1 is refused at save**:
  `trigger-flow-listener-namespace-condition/listen-flow`, with
  `dependency_flow_id_required` twice and `dependency_namespace_required`
  once -- a dependency carrying only `namespace:` and another carrying only
  `states:`. This is **W1's over-refusal, not W13b's**: Kestra's `Dependency`
  puts no `@NotNull` on either field, the artifact published the flow with
  200, and it is already a row of `executor::PUBLISHED_BUT_REFUSED`. W13b
  moved the two checks into `depends_on_refusals` unchanged, so a draft of it
  is refused at run time as well.
- **1 does not parse**: `flow-trigger-fire-once-true-flow-listen`, whose
  `window.fireOnce` the pinned artifact refuses with 422 too (Task 2).

**Zero of the twenty declare a `window:`** -- beside the 18 that resolve, a
number that could have disagreed. So the four window kinds, `timezone` and
the midnight grid are judged by Task 1's measurements and this crate's own
tests, never by the corpus, and the corpus exercises exactly one window
shape: the default. **DST is judged by neither a measurement nor the
corpus**: it is derived from `java.time` (a fixed-duration add from local
midnight; `trigger::window`, *DST -- chosen from the Java, never measured*)
and pinned only by this crate's own tests. Spec §5.1 states its visible
effect.

#### §7.2, row by row

Every row's mutation has now been run. Ten of the eleven rows name one;
Tasks 2-4 ran six (Task 3's mode swap for ALL/ANY only), Task 4's review found
four not run, and this task ran those four plus AT_LEAST's comparison, each
with `git diff
--stat` and a changed-line token count printed before the first run, an
unmutated control (`flow_trigger_windows` 14/14 twice, the ticker sweep test
1/1, the golden reading above; and 14/14 again after the last restore), and
the tree verified clean after each restore. Scripts and logs are the task's
own; nothing was committed.

| property | mutation | run by | result |
|---|---|---|---|
| `ALL` over two dependencies fires once and resets | **do not merge results**: `let merged = met.clone()` at the call site (1 line, 11 tokens) | Task 5 | **killed 2/2**, `all_fires_once_both_are_met_and_resets:316`, p2's pass reported `accumulated: 1` where `fired: 1` was expected; over the whole binary it killed 6 of 14 tests. The same mutation inside the pure `merge` (`BTreeMap::new()` for the stored results) was killed by the same assertion and by two lib tests (`merge_writes_only_true_and_never_unsets`, `the_measured_mode_sequences_replay`) |
| an expired window is replaced and the event counts | ignore the event / keep the expired results | Task 4 | killed (`an_expired_window_does_not_count`) |
| a non-matching event opens and purges nothing | open the window before matching | Task 4 | killed (`an_unrelated_execution_does_not_open_a_window`) |
| the scheduler sweep deletes expired windows | **drop the sweep**, two ways: the sweep's `DELETE` matches nothing (`WHERE false AND ...`, 14 tokens), and the ticker never calls it (16 tokens) | Task 5 | **killed**: the first 2/2 by `the_sweep_deletes_only_expired_windows:1037` (swept 0, expected 1) and 1/1 by the ticker test; the second 2/2 by `the_scheduler_ticker_sweeps_expired_windows` ("timed out after 10s waiting for the expired window to be swept", both windows still present) |
| deleting a listener deletes its window row | **drop the cascade**, two ways in migration 0158: the whole foreign key removed (13 tokens), and `ON DELETE CASCADE` alone removed (13 tokens) | Task 5 | **killed**: 2/2 by `deleting_a_listener_cascades_its_window:1076` ("cascaded", the window row still present with its result), and 1/1 by the same test's delete failing `23503` on `flow_trigger_windows_flow_id_trigger_id_fkey`. The test template was dropped before and after each, because sqlx refuses an edited migration's checksum |
| `ANY`; `AT_LEAST` with `minSatisfied` | swap the mode comparison | Task 3 (ALL/ANY swapped, pure) and Task 5 (AT_LEAST's `>=` made `>`, 18 tokens) | killed: Task 3's by `modes_all_any_and_at_least`; this task's 2/2 by `at_least_two_of_three:410` ("two of three", 0 fires against 1) and 1/1 by two lib tests, `modes_all_any_and_at_least` and `the_measured_mode_sequences_replay` (`q2_atleast2_nowindow`) |
| two processors satisfying `ALL` fire exactly once | drop the window's `FOR UPDATE` | Task 4 | killed 3/3 |
| an expired window's results do not count | drop the `end` check | Task 4 | killed (the "drop the expiry replacement" mutant is this check) |
| an off-hours event is ignored for `DAILY_TIME_WINDOW` | **drop the in-window check**: `if false && !TimeWindow::is_valid(..)` (14 tokens) | Task 5 | **killed 2/2**, `an_event_outside_a_daily_window_is_ignored:820` ("outside the window": the daily listener fired, 1 against 0) |
| the four kinds' bounds match Task 1 | -- | Task 3 | pure tests fed from `w13b-windows.json` |
| a conflicting `window:` is refused at save | drop validation | Task 2 | 17/17 killed |

#### The sweep: "`dependsOn` is refused" is retired where it described W13b's absence

Same discipline as W13a: every tracked file flattened to one line (comment
markers stripped) before matching, denominators over `git ls-tree` at
**`4549db67`**, Task 4's tip and this task's base, before this task edited
anything, and `/usr/bin/grep` with file arguments.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the brief's three phrases and their glosses -- `dependsOn` is/are refused, refuse(s) `dependsOn`, refused for `dependsOn`, `UnsupportedDependsOn`, `window` is dropped / drop the window, `DEPENDS_ON_IS_W13B`, "the day the window lands", "`MultipleConditionWindow` this package does not model", "`dependsOn` is not supported", "W13b has not started / is still pending" -- across the whole repository, flattened | **3828 files**, 10 matching, **34 occurrences** (33 by the pattern set, plus one "W13b ... has not started" the pattern's 60-character gap missed and reading the matched paragraph found) | **14** (and 20 left: 14 + 20 = 34) |
| 2 | ordinals and counts inside the regions population 1 corrected (20 of 21, twenty, nineteen-at-`resolve` plus one-at-parse) | **10 regions** (README ×3, `flow.rs` ×3, the design spec EN/VI §1-§2, the programme spec EN/VI W13 row and correction) | **0** -- each re-derived from `the_corpus_depends_on_declarations_resolve_eighteen_of_twenty`: 20 = 18 + 1 + 1, and `resolve` still refuses 19 with the twentieth refused at parse |
| 3 | data that feeds a generator -- the 20 files under `oracle-exec/` other than this README and the 345 corpus YAMLs | **20 files**, 7 matching, **517 lines**: 505 in `w13b-windows.json` (Task 1's own measurement fixture, whose keys and values are window data) and 12 in the other six (`baseline.txt` 2, `corpus.json` 1, `driver.rs` 4, `harvest-run.txt` 1, `harvest.py` 1, `runnable.txt` 3). Population 1's phrase set matches **0** of them | **0** -- every hit outside the fixture is the `fireOnce` 422, a task-level `dependsOn`, or the max-duration window. *(Fix round 1: this row first said "13 lines", a figure no command reproduces; the command is below.)* |
| 4 | the other language -- every file under `docs/vi/`, flattened, for `Từ chối \`dependsOn\``, `` `window` bị bỏ``, "W13b vẫn còn treo", "CHƯA bắt đầu" and (từ chối / bị bỏ) near `dependsOn` | **287 files**, 2 matching, **4 occurrences**, 1 of them also population 1's (the design spec VI's `UnsupportedDependsOn`) | **4**, of which **3 are not counted in population 1** |

**Corrected occurrences, without double counting: 14 (population 1) + 3
(population 4 only) = 17**, plus one edit site no pattern matched (item 18
below), so **18 edited sites**. "Corrected" means rewritten or retired in
place by a dated note; the stale text is kept as the record. *(Fix round 1:
the per-file figures below first read README 3 + `flow.rs` 5 + programme
spec EN 2 / VI 2 + design spec EN 3 / VI 2 = 17 under a population-1 total of
14. The VI programme-spec occurrences, and one of the two VI design-spec
ones, are population 4's, not population 1's; the lists below assign every
occurrence to exactly one population.)*

**Population 1's fourteen corrections, by site** (3 + 5 + 2 + 3 + 1 = 14).
Each was stale in the same way W13a's were: a sentence written before W13b,
describing its absence.

- `oracle-exec/README.md` (3): the W6 section's *Refusal 2* paragraph -- "is
  refused" and "the acceptance criterion that fails the day the window
  lands" -- gets a dated note: the window landed and the test correctly did
  **not** fail, because the arm was built beside `WindowFreeFlowTrigger`;
  W13a's *What this package could not settle* ("W13b has not started") is
  retired in place. (Item 18 of *What this ratchet does not prove* also gets
  a dated parenthesis; it is not a pattern hit, found by reading around one,
  and is not in the 14.)
- `src/trigger/flow.rs` (5): the module doc's "fails the day the window
  lands"; item 1 of *Over-acceptance is the hazard*; the
  `UnsupportedDependsOn` variant's "a `MultipleConditionWindow` this package
  does not model"; its `Display` text, which said "`dependsOn` is not
  supported" and now says the trigger "is not the window-free arm" (no
  production path reaches it: `listen` branches first and `sync`'s arm is
  `unreachable!`); the twenty-to-one test's doc, pointing at the new count.
- `docs/en/specs/2026-09-01-workflow-programme-design.md` (2): W13a's
  correction said "W13b is still pending" and "has not started"; a new dated
  W13b correction retires both without deleting them.
- `docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md` (3): §1's
  table is dated `006563ee` and correct as that snapshot, but its `resolve`
  row ("Refuses `dependsOn` with `UnsupportedDependsOn`", 2 occurrences) and
  "`window` is dropped" (1) read as current; a dated note under the table
  says what is true since W13b. Its §7.2 "drop the window" is left (below).
- `docs/vi/specs/2026-09-25-workflow-w13-flow-triggers-design.md` (1): the
  `UnsupportedDependsOn` in its §1 `resolve` row, retired by the VI twin of
  the same note.

**Population 4's three further corrections** (4 occurrences − 1 shared = 3):
`docs/vi/specs/2026-09-01-workflow-programme-design.md` (2: "W13b vẫn còn
treo" and "CHƯA bắt đầu", retired by the VI twin of the programme-spec
correction) and `docs/vi/specs/2026-09-25-workflow-w13-flow-triggers-design.md`
(1: "`window` bị bỏ", retired by the same §1 note as population 1's VI site).

**The commands, so every figure is re-runnable.** Flattening, per file at
`4549db67` (`git archive 4549db67 | tar -x`): replace every
`\n\s*(//[!/]?|#|\*|>|--)?\s*` with one space, then collapse whitespace
(Python `re.sub`); each file is written under its path with `/` spelled
`%2F`, and counted with `/usr/bin/grep -o -i -E -- "$P" * | wc -l` (8
dotfiles checked separately, 0 hits). Population 1's `$P`:

```text
dependsOn`? (is|are|was|were) refused|refus(e|es|ed|ing) (a |the |every |any )?`?dependsOn|refused for `?dependsOn|UnsupportedDependsOn|`?window`? (is|was|gets|are) dropped|drop(s|ped)? the window|DEPENDS_ON_IS_W13B|the day the window lands|MultipleConditionWindow` this package does not model|dependsOn` is not supported|W13b (has not started|is still pending)|W13b —[^|]{0,60}has not started
```

Population 4's, over `docs%2Fvi%2F*` only:

```text
Từ chối `dependsOn`|`window` bị bỏ|W13b vẫn còn treo|CHƯA bắt đầu|(từ chối|bị bỏ)[^.]{0,40}dependsOn
```

Population 3, on the raw (unflattened) files, which are unchanged since
`4549db67`:

```bash
git ls-tree -r --name-only 4549db67 -- crates/egret-workflow/oracle-exec \
  | /usr/bin/grep -v -e oracle-exec/corpus/ -e oracle-exec/README.md > files.txt   # 20
/usr/bin/grep -c -i -E 'dependsOn|UnsupportedDependsOn|window' $(cat files.txt)   # 7 files > 0, sum 517
```

**Twenty occurrences examined and left, each for a reason:** seven
`UnsupportedDependsOn` identifiers in `trigger/flow.rs` (the variant, its
`Display` arm, `resolve`'s return, item 1's doc link, two test assertions, the
corpus test's match) and one in `scheduler/sync.rs` (an `unreachable!` arm) --
the variant is still what the window-free arm returns, and renaming a public
enum is not a doc sweep; `flow.rs`'s *Resolution* test comment, the
twenty-to-one test's assertion message, and `tests/flow_trigger_sync.rs`'s
"a row even though `resolve()` refuses `dependsOn`", all about `resolve`,
which still refuses; `flow.rs` item 4, already dated by W13b Task 2;
`flow.rs`'s, `window.rs`'s and `tests/flow_trigger_sync.rs`'s "a `window` with
no `dependsOn` is refused" (3), a different proposition that stays true; the
W13a plan (1) and this package's own plan (3, the brief quoting the phrases);
the design spec's §7.2 "drop the window's `FOR UPDATE`" (1), a mutation's
name. (7 + 1 + 3 + 1 + 3 + 1 + 3 + 1 = 20.) **Those left are the control that the sweep
does not retire everything it matches.**

### W11 — every line a task prints is stored now, and the oracle cannot see a single one

**109 before W11 and 109 at its tip, both measured.** Both figures are the
`executor_matches_golden` line printed with `--nocapture`: **294 golden
entries, 109 mismatching, 109 baselined, 0 broken**. The base was measured at
the package base `6749a37b`, in a temporary detached worktree with its own
target directory (the shared `egret_test_template`, already migrated to 0159 by
this branch, refused the base with `VersionMissing(159)`, so it was dropped
first and re-migrated by the next branch run), and the tip at `e0f86320` (the branch rebased onto
`cd48121e`). `origin/main` moved between the two (#599, #600), but
`git diff --stat 6749a37b cd48121e -- crates/egret-workflow migrations` is
empty, so the move changed nothing the ratchet reads.

The ratchet cannot move for W11, and that is structural rather than
observed:

- **The golden holds no log line.** `golden.json` has 13 distinct keys at any
  depth, and none contains `log` or `message`. The oracle compares traces
  (states, terminals, ordered pairs), not what a task printed.
- **The driver names none of W11's code.** `logs::`, `RunLog`, `PgLogSink`,
  `LogRepo` and `task_logs` each occur **0** times in `oracle-exec/driver.rs`,
  against `worker::` at **26** (the positive control, the same command W13b
  used). In `tests/executor.rs` the only W11 name is one
  `log: egret_workflow::logs::RunLog::discard()`, a field W11 added to
  `TaskInput` and set in the kill-path test's hand-built input; `worker::`
  occurs **38** times there. The driver builds its registry with
  `Handlers::new()` and never calls `with_log_sink`, so its runs write to
  `TracingSink` exactly as before.

| | ratchet | what the task shipped |
|---|---|---|
| package base `6749a37b` (origin/main, W13b's merge) | 109 mismatching, 109 baselined, 0 broken of 294 (measured, above) | -- |
| Task 1 | 109 (docs and one JSON fixture only) | the log behaviour of the pinned artifact (`oracle-exec/w11-logs.json`): stdout INFO and **stderr ERROR** (not WARN, whatever `warningOnStdErr` says), marker lines not logged, one WARN `Invalid outputs '…'` for a broken marker, the debug bracket, no framing lines, long lines **split at 15,360 characters** (decision D6); the failed-attempt line |
| Task 2 | 109 | migration 0159 (`workflow.task_logs`), `LogRepo`: a batch insert that drops, and counts, a line whose execution was deleted instead of poisoning the batch; the cursor read behind the 2 s settle lag; the batch prune |
| Task 3 | 109 (`--test executor`, 58 passed) | async `LogSink`, `LogLine`, the per-attempt `RunLog` (split, 10,000-line / 10 MiB cap, one WARN summary outside it); `Log`, `Fail`, container and python moved onto it; levels per Task 1 |
| Task 4 | 109 | `PgLogSink` and its writer (channel 10,000, `send().await`, batches of 500 or 1 s, 30 s retry, 10 s shutdown drain), the hourly pruner, the three config keys, the wiring in `egret-bin` |
| Task 5 | 109 | `GET /api/ws/{workspace}/workflows/executions/{id}/logs`, utoipa, `gen:api`; fix round 1: the flush barrier in `RunLog::finish` and `complete` |
| Task 6 | **109** (measured, above) | this section, the sweep below, the full local gate |

#### What W11's tests cover, and what they do not

**Covered** (against a real Postgres wherever a line is stored or read; the
split and the cap are pure `RunLog` tests): a flow run through the real runtime (`spawn_with_logs`, the
production registry, nothing driving the tick) stores its `Log` and `Fail`
lines, and the API returns them in order at the measured levels
(`end_to_end_a_flow_logs_through_the_real_runtime`); a full channel makes the
writing handler wait and loses nothing; a failing batch is retried, then
reported, and never ends the writer; shutdown drains the channel, and a line an
in-flight run writes after cancel is stored; the cap stores up to its limit
and writes exactly one WARN summary; a 40,000-character line splits into
15,360 / 15,360 / 9,280 characters in order; out-of-order commits are not
skipped by a polling reader; the prune deletes only lines older than the
window, and `0` starts no pruner; another workspace's execution is a 404;
`finish()` runs on error, timeout and cancel, and does not return before the
run's lines are inserted; `complete` stays false until the settle lag has
passed. Container and python stdout/stderr reaching `RunLog` at INFO/ERROR is
tested in `tests/container_docker_live.rs` against a real Docker daemon.
Mutations: Task 2 4/4, Task 3 13/13, Task 4 6/6 (five plus the shutdown-order
mutant its fix round added), Task 5 6/6 (two plus four in the fix round), each
killed by a named assertion and recorded in that task's report. None survived.

**Not covered, or not true, and stated here so nobody infers it:**

1. **No production run stores a stdout or stderr line today.** `egret-bin`
   hands `Handlers::with_core_tasks()` to the runtime, and that registry holds
   `Log`, `Return`, `Fail` and `Sleep` only: no container and no python
   handler (user decision D5, 2026-09-27). Container and python output is
   stored and tested **only through test registries**. In production the stored
   lines come from `Log`, `Fail` and the handlers' own failure and timeout
   messages. Wiring container, python and sql into production is its own
   package, because how the server may run containers is a security decision.
   *(W5a Task 6, 2026-09-28: that package is W5a, and for containers this
   item is the record of the gap, not the present. With
   `workflow.container_runner` set to `docker` or `kubernetes`, production
   registers Kestra's four script types and the two aliases, and their
   stdout and stderr are stored line by line at INFO and ERROR -- on
   Kubernetes too, where stderr is carried through a file because
   `pods/log` merges the streams; measured live, see the W5a section. The
   default is still `none`, which leaves the registry exactly as described
   above, and `egret.sql` and `egret.python` are still not in production.)*
2. **There is no SSE and no `follow`.** A client polls the cursor. That was
   decided with the user (D1); egret has no SSE endpoint anywhere.
3. **A line becomes readable about 2 s after it is written.** A page stops
   before the first row after the cursor whose `inserted_at` is less than 2 s
   old (a watermark, not a per-row filter), so that an id committed after a
   higher one (two writers: two replicas) is not skipped by a reader whose
   cursor has already passed it. That holds while each write transaction is
   shorter than **half** the lag (1 s); `inserted_at` is the transaction's
   start.
4. **The flush barrier is bounded at 5 s, and it does not wait at all while
   the writer is failing.** `RunLog::finish` waits for the writer to store
   every line the attempt handed it (`WriterConfig::flush_within`), then the
   outcome commits. If the writer does not acknowledge within 5 s, or is gone,
   `finish` logs a `warn` and returns, and the timeout sets the writer's
   breaker; while the breaker is set (an insert failed, until one succeeds),
   the barrier returns at once and a `write` on a full channel drops its line
   with a count instead of waiting. So a log write never changes a run's
   outcome, and can delay it by at most one barrier bound per failing spell.
   A write can still wait while the writer is healthy but slow (D3), and that
   wait counts against the task's `timeout:`.
5. **What `complete` guarantees, and what it does not** (W11 design spec §11).
   `complete` is true when the execution is terminal **and** its `updated_at`
   is more than 2 s old. Because the barrier inserts a run's lines before its
   outcome commits, a client that stops on `complete && items == []` has read
   every line that was stored. It does **not** mean that every line the run
   printed was stored: `complete` can be true with lines missing when the
   process died with lines still in the channel, when a batch was dropped
   (retries exhausted, or its one attempt failed while the writer was
   failing; its count is in `tracing::error!`), or when the barrier did not
   wait (timed out, or skipped by the breaker; item 4). A run that executes
   twice (reaped while its handler still ran) writes both runs' lines under
   one attempt, and its stale handler can write after the terminal commit. And `complete`
   assumes nothing writes after the terminal state; that holds while egret
   has no kill route, and a future kill must re-check it.
6. **Paths with no test:** a barrier that times out because the channel is
   full is reasoned about in `logs::writer` and not exercised; the 10 s
   shutdown drain does not bound an insert already in flight. (The
   lost-acknowledgement and drain-time acknowledgement paths are tested since
   the final-review fix wave.)
7. **The barrier's cost is not measured under load.** Every `finish()`
   forces its own INSERT, which defeats the 1 s batching for short runs and
   adds one insert per run to the sequential claim loop. Not measured. During
   a log-write fault the breaker stops the barrier from waiting (item 4);
   `tests/task_logs.rs`'s `a_failing_writer_does_not_slow_finish` measures
   five runs' `finish` under failing inserts, not throughput under load.

#### The sweep: "nothing persists a log line" is retired where it described W11's absence

Same discipline as W13b: every tracked file flattened to one line (comment
markers stripped), denominators over `git ls-tree` at **`e0f86320`**, this
task's base after the rebase, before this task edited anything, and
`/usr/bin/grep` with file arguments.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the brief's phrases and their glosses, EN, whole repository | **3912 files** (3904 plus 8 dotfiles, which had 0 hits), 13 matching, **33 occurrences** | **14** |
| 2 | the other language: every file under `docs/vi/`, flattened | **289 files**, 4 matching, **11 occurrences** | **9** |
| 3 | stale level claims: stderr at WARN, whole repository, case-sensitive | **3912 files**, 3 matching, **5 occurrences** | **0** |
| 4 | `handlers/mod.rs`'s *"egret has no durable workflow-log store yet"* | 1 file | **0** here: Task 3 (`8bb88306`) already rewrote the trait's doc, and the phrase's one remaining hit is the W11 plan quoting it (counted in population 1) |

**Population 1's fourteen corrections, by site** (7 + 3 + 4 = 14):

- `docs/en/specs/2026-09-01-workflow-programme-design.md` (7): §6.1's
  *"NOT BUILT: W11"* rewritten to what W11 built (the old text is quoted in
  the correction); W7's row, *"logs panel needs W11"*, gets a dated
  parenthesis (the API exists, the panel is W7's); W11's row, *"no `task_logs`
  table"*, *"none of the three persists a line"*, *"counts, not content"* and
  *"W7 ships without a logs panel"*, retired by a new dated W11 correction
  under the table; §11 open question 1's *"nothing persists a log line"* gets
  its answer.
- `docs/en/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md`
  (3): §5's measurement (*"persists a line"*, *"counts, not content"*,
  *"ships without a logs panel"*), dated in place by a note at the end of §5.
- this README (4): the W10 section's bullet (*"Logs are W11"*, *"persists a
  line"*, *"counts and not content"*) and W3b's later *"Logs are W11 and the
  trigger loop is W12"*, each by a dated parenthesis.

**Population 2's nine corrections**: the VI programme spec (6, the twins of
six of population 1's seven; the seventh's twin, *"không migration nào tạo bảng
`task_logs`"*, matched no pattern and is retired by the same VI correction,
which names the row's whole measurement) and the VI W10 spec (3, *"không bản nào lưu một dòng nào"*,
*"không phải nội dung"*, *"không có panel log"*), by the VI twins of the same
notes. *"không bản nào lưu"* was not in the first VI pattern; it was found by
reading the W10 VI section a hit pointed at, and the pattern below includes
it.

**Nineteen EN occurrences examined and left** (population 1): seven *"nothing
persists"* about other things (migrations 0136 and 0137, `worker/mod.rs`, the
W3b plan and this README's note under item 13 of *What this ratchet does
**not** prove*, all about `Outcome::outputs` or
`Outcome::message`; `EngineCreate.tsx` and its test, about the engine form);
four in the W11 design spec EN and two in its VI twin (§1's table, headed
*"state on origin/main (`6749a37b`)"*, and §8's task list quoting the
phrases); three in the W11 plan (the brief quoting them); three in the W10
plan (its scope decision, a record of the instruction given).
(7 + 4 + 2 + 3 + 3 = 19; 14 + 19 = 33.) **Two VI occurrences left**
(population 2): the W11 VI design spec's §1 table, dated like its EN twin, and
`2026-08-16-data-api-p5-design.md`'s unrelated *"không phải nội dung"*.
**All five of population 3 left**: three say stderr is ERROR *not* WARN (the
fixture and the W11 spec EN/VI §5), and two are the W11 spec's §1 table row,
dated. `worker/commands.rs` itself has **0** hits: Task 3 rewrote the comment
to *"stderr is **ERROR**"*.

**The commands.** Flattening, per file at `e0f86320`: `git show
e0f86320:<path>`, replace every `\n\s*(//[!/]?|#|\*|>|--)?\s*` with one space,
collapse spaces and tabs, write under the path with `/` spelled `%2F`; count
with `/usr/bin/grep -o -i -E -- "$P" * | wc -l`. Population 1's `$P`:

```text
persists a (log )?line|none of the three persists|nothing persists|ships without a logs panel|logs panel needs W11|NOT BUILT: W11|no durable workflow-log store|durable (workflow[- ])?log store|no `?task_logs`? table|task_logs` (does not exist|is not built)|W11 (has not started|is (still )?pending|is not built)|until W11|counts,? (and )?not content|The text goes to `tracing` and nowhere else|logs are W11|logs are deferred|defers? `?(workflow\.)?task_logs`? to W11|logs (are|remain) (in )?W11
```

Population 2's, over `docs%2Fvi%2F*` only:

```text
CHƯA DỰNG: W11|ghi xuống lấy một dòng|không (cái|bản) nào (ghi|lưu)|không có panel log|Panel log cần W11|không phải nội dung|không có gì ghi xuống|kho log bền|W11 (chưa|CHƯA)|log (là|thuộc) W11|Log là W11|hoãn[^.]{0,40}task_logs
```

Population 3's, case-sensitive (`-o -E`, no `-i`, so `warningOnStdErr` does
not match):

```text
stderr[^.;|]{0,30}\bWARN\b|stdout INFO, stderr WARN
```

### W5a — script tasks run in production now, and the oracle cannot see a single container

**109 before W5a and 109 at its tip.** The tip figure is the
`executor_matches_golden` line printed with `--nocapture` in Task 6's gate:
**294 golden entries, 109 mismatching, 109 baselined, 0 broken** on
`4b09eefa` (the branch rebased onto `ce21e167`) plus this task's
comment and documentation edits. The base figure is derived from a pass rather than printed: the
test was `ok` at the package base `742a2d7a` (`baseline-gate.txt`, W5a's
Task 1 gate), and it fails on any stale or banked entry, so `ok` means the
mismatching set equals `baseline.txt`'s **109** entries; `baseline.txt`,
`golden.json` and `corpus/` are byte-identical between the base and the tip
(`git diff --quiet 8f35f85a HEAD -- …` exits 0). The branch was rebased
from `8f35f85a` onto `ce21e167` (#603, SP4c) in Task 6, which touches no
workflow crate; the branch's own diff is identical before and after, modulo
hunk offsets.

The ratchet cannot move for W5a, structurally:

- **The corpus declares no script task.** Of **71** distinct `type:` values
  in `oracle-exec/corpus/`, the pattern
  `script|container|shell|command|docker|python|runner` matches **6**, and
  all six are Kestra's own test tasks under `io.kestra.core.runners.*`
  (`TaskThatFail`, `WorkerTaskResultTooLarge`, ...), not script tasks. So the
  count that could disagree is 6, and the count of script tasks is **0**.
- **The driver names none of W5a's code.** `scripts::`, `gate::`, `k8s::`,
  `KubernetesRunner`, `DockerRunner`, `renew_lease`, `handlers_for` and
  `script::` each occur **0** times in `oracle-exec/driver.rs` and in
  `tests/executor.rs`, against `worker::` at **26** and **38** (the positive
  control W11 and W13b used).
- **The golden holds nothing W5a writes.** Its 13 keys (`attempts`,
  `execution`, `id`, `order`, `parent`, `parent_run`, `rejected_class`,
  `runs`, `spawned`, `state`, `task`, `value`, `verdict`) include no lease,
  no pod and no log line. Lease renewal is on the driver's path
  (`worker::tick` → the claim loop) and changes only `claimed_at`, which no
  trace carries.

| | ratchet | what the task shipped |
|---|---|---|
| package base `8f35f85a` (+ spec `3ea4b84d`, plan `742a2d7a`) | 109 (derived from the pass, above) | -- |
| Task 1 | 109 | Kestra's script tasks measured on the pinned artifact (`oracle-exec/w5a-scripts.json`): default images `ubuntu` / `python:3.13-slim`, accepted and refused properties per type, `outputFiles` shape, inline `inputFiles`, `warningOnStdErr` (**no effect**: stderr is ERROR and the task SUCCESS, contradicting the spec's first draft), `failFast`, a tagless image pulling `:latest` |
| Task 2 | 109 | the four types and two aliases, `taskRunner:` (Docker, Kubernetes; `Process` refused), save-time refusals by name, `default_enabled` in the feature catalog and `workflow_containers` (off by default), `plugin_configurations`, the one gate at publish |
| Task 3 | 109 | lease renewal every `working_timeout / 3` under a fence; `container_default_timeout_s` (3600) |
| Task 4 | 109 | `KubernetesRunner`: pure pod-spec builder, exec+tar files in and out, stderr through a file, log following, re-attach by (run, attempt), delete, the orphan sweep; the helper image |
| Task 5 | 109 | config, `runtime::handlers_for`, the Docker runner as `taskRunner` Docker with re-attach, the run-time gate, self-fence, the Helm chart (task namespace, Role, RoleBinding, NetworkPolicy); fix rounds: Kestra `outputFiles` semantics, bounded listings and globs, 169.254.169.254 denied |
| Task 6 | **109** (measured, above) | the end-to-end on a Calico kind cluster, this section, the sweep below, the full local gate |

#### What is covered live, what by unit test, and what is not measured

**Live on a real cluster, through egret's API** (Task 6, a temporary kind
cluster `egret-w5a` with kindnet disabled and **Calico v3.28.2** enforcing
NetworkPolicy; the chart installed with `workflow.containerRunner=kubernetes`;
the commands and their outputs are kept in the package's working notes,
and every figure below is copied from them):

1. **A `shell.Commands` flow publishes and runs in a pod.** Two runs of one
   flow (`ubuntu:latest`, `taskRunner` Kubernetes): SUCCESS; the logs route
   returns `hello-stdout` at **INFO** and `hello-stderr` at **ERROR**,
   `complete: true`; the execution carries `outputFiles` `{"out.txt":
   "payload-from-pod\n"}` and `vars` `{"answer": 42}`; the pod watch shows
   each run's pod `ADDED` and `DELETED` (the clean run: created 11:59:14,
   deleted 11:59:17), and `egret-tasks` holds **0** pods afterwards.
2. **Killing egret mid-run re-attaches.** A 120 s flow was started, its pod
   running, and the egret pod force-deleted (`--grace-period=0 --force`) at
   second 19. The replacement pod's reaper reclaimed the lease
   (`working_timeout_s` set to 30 for the test), the next claim logged
   `re-attaching to the task's existing pod`, and the run ended SUCCESS with
   **one attempt**. The pod watch holds **one UID** for that pod name, one
   `ADDED` and one `DELETED`; its `done.txt` names that pod; the logs hold
   all **40** tick lines, **one** of them twice (`tick 5`, the repeat spec
   §11 allows) and **none** missing.
3. **NetworkPolicy, measured on Calico with a control.** From a task pod
   (`alpine:3.20`, busybox `nc -w 4`, exit 0 = connected): with the chart's
   policy, egret's Postgres (Service IP and pod IP) and the Kubernetes API
   server (the `kubernetes` Service and its endpoint `172.23.0.3:6443`) are
   **BLOCKED**, while egret's own API (same namespace, the positive control
   for "in-cluster is not simply dead") and `1.1.1.1:443` are **REACHABLE**.
   With the policy deleted and the same flow re-run, all four blocked
   targets turn **REACHABLE**, so the blocks are the policy's.
   **`169.254.169.254` is UNMEASURED**: it answered BLOCKED with the policy
   and without it, because nothing listens there on kind; the rule is present
   in the rendered policy (`except: 169.254.169.254/32`) and that is all this
   run shows.
4. **A disabled workspace creates nothing.** In a workspace with
   `workflow_containers` off (the catalog default, visible on a fresh
   workspace), publish answered **400** `the 'workflow_containers' feature is
   disabled for this workspace; ...`. Starting the unpublished draft anyway
   (the W13b path) ran the run-time gate: the task run FAILED with the same
   reason as its one log line, and `egret-tasks` held **0** pods and **0**
   events, against **5** `Scheduled` pods and 40 events after the other runs
   (the control for that zero).
5. **Found live, not by any test: the chart's RBAC cannot exec.** Every
   `files-out` exec (stderr sentinel, output download) failed with
   `403 Forbidden`; the API server's reason was *"cannot **get** resource
   `pods/exec`"*. kube-rs opens exec as a WebSocket, which is an HTTP GET,
   and on Kubernetes **v1.33.1** that is authorised as verb `get`; the Role
   grants `pods/exec` **`create`** only, exactly as the spec says. The runner
   kept the pod and re-attached every `working_timeout` without ever
   finishing; adding `get` to the Role **in the temporary cluster only**
   made the stuck run finish on its next claim, and every result above was
   measured with that patch. *(Fix round 1, user decision 2026-09-28: the
   chart's Role grants `pods/exec` `get` and `create`, and spec §5 says so.
   Tasks 4 and 5's live tests ran the runner with an administrator's
   kubeconfig, so RBAC was never exercised by them; `k8s_runner_live` now
   runs every runner and sweep call as a ServiceAccount bound to a Role
   parsed from the chart's `rbac-tasks.yaml`, and dropping `get` from the
   chart fails it with the same 403.)*
6. **A re-attached run under-counts its lines.** After a re-attach,
   `stdErrLineCount`/`stdOutLineCount` count only lines the last holder read
   (the first hello run: 0 stderr / 1 stdout against 1 / 2 on its clean
   re-run; the killed run: 35 stdout lines counted of the 40 it printed): the lines already stored are dropped as replayed and
   not counted. The stored log lines are complete; the counts are not.

**Live against a real daemon or cluster, driven by tests** (gated by env var,
as `container_docker_live` is gated on Docker): `tests/k8s_runner_live.rs`
(`EGRET_TEST_KIND=1`) -- a command runs in a pod and the pod is deleted;
input and output files round-trip; a non-zero exit fails and keeps outputs;
cancel deletes the pod; a second `run` for the same attempt re-attaches rather
than re-creating; a line written just before the reclaim is not lost; the
orphan sweep deletes pods of terminal runs only (and the Docker sweep,
containers); and, from the final review, a Role without `pods/exec` `get`
and an unpullable helper image each FAIL the task by name in bounded time,
and a task writing past its ephemeral-storage limit is evicted and FAILED
with the kubelet's reason. `tests/container_docker_live.rs` covers the Docker runner,
re-attach included. These ran on the shared `egret` kind cluster (kindnet),
in their own namespace.

**Against a real Postgres, no cluster:** `tests/scripts_gate.rs` (the
refusals by name at publish, the gate at run with no runner call on refusal,
a disabled feature and a rendered image outside the allowlist each FAILED
with nothing created); `tests/runtime.rs` and `tests/worker.rs` (a run longer
than `working_timeout` is not reaped while its worker lives; reaped after it
stops renewing; a lost fence records no outcome; a script task without
`timeout:` gets the default). **Pure unit tests:** the pod spec (every
`restricted` field, no service-account token, resources capped by
`max_resources` and an absent limit set to the max), image normalisation and
glob semantics against the fixture, the output-file budgets.

**Mutations:** every task's report records its mutants with the killing
assertion (Task 2 5/5, Task 3 4/4, Task 4 9/9 plus its fix rounds, Task 5
6/6 plus its fix rounds). None survived. One was **not run** and is
therefore unproved: Task 5 fix round 2's quadratic-parse mutant (restoring
the O(n²) `docker diff` parse), skipped because at 200,000 paths it runs for
minutes; its timing test is the instrument, unmutated.

**Not covered, or not true, and stated so nobody infers it:**

1. **The default is still `none`.** A deployment that sets nothing has
   exactly W11's registry; no script task runs, and publish refuses every
   script task.
2. **Seven of fifteen publishable types still hang** (item 19 (a)):
   `egret.sql`, `egret.python`, `egret.dataapi.call`,
   `egret.dataset.refresh`, `egret.report.send`, `core.http`, `core.slack`.
   A script task reached without publish (a draft run) under `none` hangs the
   same way.
3. **Spec §11's limits hold as written:** a pod that outlives egret entirely
   (egret uninstalled) is not cleaned up by egret; re-attach may repeat a few
   log lines around the reclaim instant (the dedupe is by exact
   `(timestamp, line)` and text); `pods/exec` in the task namespace lets
   egret exec into task pods, and the namespace holds nothing else.
4. **NetworkPolicy is IPv4 only.** On a dual-stack cluster IPv6 egress is
   denied, not open (fail-closed). The in-cluster Postgres rule is by pod
   selector (`app: postgres` in the release namespace); a metadata Postgres
   elsewhere must be named in `deniedNamespaces` or `deniedCidrs`, and the
   pod and service CIDRs must be right: wrong ones silently reopen Postgres
   on a CNI that matches `ipBlock` against pod IPs. The e2e measured the
   chart's defaults on one CNI (Calico); Cilium and cloud CNIs are unmeasured.
5. **stderr is ERROR only for a shell interpreter on Kubernetes.** A
   non-shell `interpreter` keeps `pods/log`'s merged stream (stderr at INFO)
   and writes one WARN saying so.
6. **Deferred minors** carried in the tasks' reviews: the helper image is
   pinned by tag, not digest; a symlinked parent directory in `outputFiles`
   is followed inside `files-out`; an unschedulable pod gives no reason;
   the self-fence tests use real 60/200 ms timers. *(The re-attached run
   whose feature was turned off, FAILED while its container ran on, is
   fixed by the final review: the re-claim adopts the container and the run
   finishes, spec §4.)*
7. **The ratchet cannot judge any of this in either direction**, for the
   three reasons above.
8. **Line counts after a re-attach are short** (spec §11, added in fix
   round 1): `stdOutLineCount`/`stdErrLineCount` count only what the last
   holder read (live item 6 above). The stored lines are complete.
9. *(Retired by the final review's I4, kept as the record:)* **a missing
   `pods/exec` `get` degraded silently**. It now FAILS the task naming the
   missing verb, and the first refusal is an ERROR line with the RBAC hint;
   a helper image that cannot be pulled fails the task naming
   `workflow.kubernetes.helper_image`. Both measured live by
   `k8s_runner_live` (a Role without the verb; an unpullable helper). What
   it said: the chart
   grants it now, but an RBAC regression, or a cluster that authorises
   WebSocket exec differently, brings back F1's behaviour without failing
   anything: a run with no input or output files waits about 15 s for the
   stderr sentinel on every run and then succeeds; a run with input or
   output files never finishes (every exec is refused, the pod is kept, the
   run re-attaches for ever). The only signal is a server-side WARN per
   attempt. Measured live on kind 1.33 and by `k8s_runner_live`'s mutation,
   where exactly the file-transfer cases fail and the other seven pass.
10. *(Retired by fix round 1, kept as the record:)* the chart's Role granted
   `pods/exec` `create` only, and every exec was a 403 on Kubernetes 1.33
   (live item 5). It grants `get` too now, and `k8s_runner_live` runs as
   that Role, so the class is covered by a test rather than by this line.
11. **The outcome commits before the pod is deleted** (final review I3).
   A commit that fails after the task finished leaves the pod, and the next
   claim re-attaches and records the same outcome (`tests/scripts_gate.rs`,
   a commit made to fail by a deferred trigger). Still at-least-once: a
   finished pod deleted by someone else before the re-claim is re-created,
   and the task runs again (spec §11).
12. **Node disk is bounded on Kubernetes only** (final review I5):
   `ephemeral-storage` on `main` and a `sizeLimit` on every `emptyDir`
   (default 10 GiB); a task writing past it is evicted and FAILED with the
   kubelet's message (measured live by `k8s_runner_live`). The Docker runner
   has no portable disk limit and has none (spec §11).
13. **The e2e above ran a hand-patched Role** (live item 5) against a
   backend built at `a30f3683`; the committed Role and the final review's
   fixes were exercised by `k8s_runner_live`, not by a second e2e install.

**Decisions the user made (2026-09-28)**, recorded here because the tests
enforce them and a reader could mistake them for accidents: W5 split into
W5a (containers) and W5b (plugin host); follow Kestra's shape (script tasks,
`taskRunner:`, FQCNs, Kubernetes under the EE FQCN); `workflow_containers`
off by default plus one global image allowlist in `plugin_configurations`,
where no entry allows no image; a Kubernetes pod per task in production,
Docker for development; lease renewal and a 1 h default timeout; standard
pod hardening, egress open except egret's Postgres and the Kubernetes API;
in Task 5's review: `outputFiles` follow Kestra (a missing file is ignored,
non-recursive globs) with the 1 MiB caps kept, `169.254.169.254/32` denied by
default, and the task namespace may not be the release namespace.

#### The sweep: "no Kubernetes runner" and "nine of thirteen hang" are retired where they described W5a's absence

Denominators over the tree at **`a30f3683`** (Task 5's tip, before this
task edited anything; `git archive a30f3683`, **3950** files), counted with
`/usr/bin/grep` with file arguments.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | the programme spec EN: lines matching `W5\b\|kestra-plugin-host\|kestra\.<FQCN>\|egret\.container` | **9 lines**, 12 occurrences | **4** lines, plus one new dated correction under §9's table |
| 2 | its VI twin, same pattern | **9 lines**, 12 occurrences | **4** lines, plus the VI correction |
| 3 | "nine of thirteen hang", EN and VI, whole tree: `nine of (the )?thirteen\|9 of 13\|thirteen publishable\|chín trên (mười ba\|13)\|9 trên 13` (`-i`) | **8 files, 10 occurrences** | **5**, plus **1** restored in `egret-bin` |
| 4 | "no Kubernetes runner exists": `pod later\|one implementation today\|no kubernetes runner\|kubernetes runner is a second\|only implementation` (`-i`), whole tree | **8 occurrences** in 5 files | **3** |

**Population 1's four**: §6.4's `egret.container` row (dated: an alias of
`shell.Commands` now, run by `taskRunner:`/`container_runner`, not the
kernel launcher) and `kestra.<FQCN>` row (*W5b; not built*); §9's W5 row
(split into W5a and W5b, user decision); §11 question 3 (answered for
containers, still open for plugin jars). The new correction under the table
retires the W11 correction's *"production's registry has no container or
python handler"* for containers. **Left (5)**: §3's spike heading and its
`W5 host` sentence (the spike's record, and true of W5b's host), §3.1's two
lines saying the plugin host is driven through the `egret.container`
contract (still W5b's plan), and W4's row (a record). Population 2: the
same four and five, line for line.

**Population 3**: corrected `worker/claim.rs`'s doc (*"13 rows ... 9 of
13"* → 15 rows, **7 of 15**, with the seven named and the old figure
quoted), this README's item 19 (a) and the W10 fix-round note above it (dated
parentheses), and the W11 design spec's §11 limit, EN and VI (dated). The
`egret-bin` comment the brief names had **0** hits at `a30f3683`: Task 5
had already rewritten it and dropped the count; this task restores a
count, correctly, because the hang it describes is still true for seven
types. **Left (5)**: `oracle/README.md` and this README's *"Nine of the
thirteen are ..."* (other subjects: threads and pointers); the W5a plan
quoting the brief; the W5a design spec's §1 table, EN and VI, headed as the
state on `8f35f85a`.

**Population 4**: `worker/commands.rs`'s module table (*"a container today,
a pod later"*) and `TaskRunner`'s doc (*"One implementation today"*, and the
second *"Kubernetes runner is a second"* with it) name both runners now.
**Left (5)**: the module doc's *"a Kubernetes runner is a second
implementation rather than a second copy of every handler"* (a design
statement, true), three unrelated *"only implementation"* in
`egret-perm-compile` and the SP4b plan, and the W5a plan quoting the brief.
`handlers/container.rs` had **0** hits: Task 5 had rewritten it.

### W5b — Kestra plugins run in the host image, and the ratchet cannot see one

**109 before W5b and 109 at its tip.** The tip figure is the
`executor_matches_golden` line printed with `--nocapture` in Task 7's gate:
**294 golden entries, 109 mismatching, 109 baselined, 0 broken** at the
Task 7 commit. The base figure is derived from a pass, as W5a's was: the test
was `ok` in W5b's Task 1 gate (`baseline-gate.txt`, at `f4a84116`, whose
workflow code is the package base `6acb1d68`'s), and it fails on any stale or
banked entry, so `ok` means the mismatching set equals `baseline.txt`'s
**109** entries. `baseline.txt`, `golden.json` and `corpus/` are
byte-identical between the base and the tip (`git diff --quiet 6acb1d68 HEAD
-- …` exits 0).

What the ratchet can see of W5b, and what it cannot:

- **It cannot see a plugin task.** Of **68** distinct `type:` values in
  `oracle-exec/corpus/` (`/usr/bin/grep -rhoE '^ *type: *[A-Za-z0-9_.]+'`),
  **0** start with `kestra.` or name a JDBC class. No corpus flow reaches the
  `kestra` handler, the host, the catalog or the gate.
- **The driver names none of W5b's code.** `plugins::`, `kestra::`,
  `secrets::`, `plugin_host` and `render_with_secrets` each occur **0** times
  in `oracle-exec/driver.rs` and in `tests/executor.rs`, against `worker::` at
  **26** and **37** (the same positive control W5a used).
- **It can see two things W5b changed, and both still match.** (1)
  `pluginDefaults:` is now refused at publish on every flow: **0** corpus
  files and **0** lines of `corpus.json` and `golden.json` hold the key, so the
  refusal meets nothing. (2) `secret()`: **2** corpus flows call it
  (`valids/secrets` on `Return` and `Log`, `valids/webhook-secret-key` in a
  webhook trigger's `key`), none on a `kestra.*` task, so W5b's refusal still
  applies to both, with its new reason; neither is in `baseline.txt`, and the
  pass means both still match their golden.
- **The golden holds nothing W5b writes.** No trace carries a log line, a
  pod, a secret or a plugin output.

| | ratchet | what the task shipped |
|---|---|---|
| package base `6acb1d68` (+ spec and plan `377f3e1c..343eb72e`) | 109 (derived from the pass, above) | -- |
| Task 1 | 109 | the three plugins measured on the pinned artifact (`oracle-exec/w5b-plugins.json`): outputs per `fetchType`, log levels, masking, the properties needing storage, `pluginDefaults` refused at save, `kill()` per engine |
| Task 2 | 109 | `plugin-host/`: `ShimRunContext`, `EgretPluginHost` (`run`, `describe`), exit codes 0/2/3/4, `plugins.lock`, the Dockerfile, `catalog.json` and the lockfile-hash test |
| Task 3 | 109 | `kestra.<FQCN>` types from the catalog, the refusals by name at publish, `pluginDefaults` refused at publish (ignored at run, user ruling 2A), `workflow_kestra_plugins`, `allowed-types`, the gate at publish |
| Task 4 | 109 | `secret()` on `kestra.*` tasks from the execution's workspace, render once (user ruling 1A), masking in `RunLog` |
| Task 5 | 109 | both runners speak the host protocol and cancel with a grace; `files-out` trap for plugin pods only |
| Task 6 | 109 | the `kestra` handler, `workflow.plugin_host` config, the chart value, the live tests; fix round: no connection pool in the host, `kill_grace_s` ≥ 3 |
| Task 7 | **109** (measured, above) | the end to end on a temporary kind cluster, this section, the specs' corrections, the full local gate |

#### What is covered live, and by which gate

**Live on a real cluster, through egret's API** (Task 7, a temporary kind
cluster `egret-w5b-e2e`, kind v0.29 / Kubernetes v1.33.1, kindnet; the chart
installed from this branch with `workflow.containerRunner=kubernetes`,
`workflow.workerEnabled=true` and `workflow.pluginHost.image=egret-plugin-host:dev`;
a Postgres 16 in its own namespace `e2e-data`; the transcripts are
`task-7-e2e-*.txt` in the package's working notes, and every figure below is
copied from them):

1. **A `kestra.…postgresql.Query` flow publishes and runs in pods.** Two
   tasks, both with `password: "{{ secret('pg', subkey='password') }}"` from a
   basic-auth workspace secret created through the API: `hold`
   (`SELECT pg_sleep(20)`) and `rows` (three rows from `generate_series`, with
   the secret in a SQL comment). SUCCESS in 32.1 s; `rows`' outputs are
   `{"rows": [{"n": 1, "label": "row-1"}, …, {"n": 3, "label": "row-3"}],
   "size": 3}`; the logs route returns the plugin's own `DEBUG Starting query:
   … /* ****** */` line, `complete: true`. One pod per task, each `Killing`
   right after its task, and `egret-tasks` held **0** pods afterwards.
2. **The pod spec holds no secret.** `kubectl get pods -n egret-tasks -o yaml`
   taken **while** `hold`'s `pg_sleep(20)` was active on Postgres (1 active
   backend at the moment of the dump): 8430 bytes, 1 pod, **0** occurrences
   of the password, against **2** of `egret-plugin-host:dev` (the control).
   The pod's env is `WORKING_DIR` and `EGRET_KILL_WAIT_MS=13000` only.
3. **The database holds no secret.** A `pg_dump --data-only` of egret's
   metadata database after every scenario: schema `workflow` (13 flows, 7
   versions, 6 executions, 8 task runs, 13 log lines) **0** matches; every
   schema **0** matches; egret's server log **0**. Controls in the same dumps:
   the flow source's `secret('pg'` 22, the masked `/* ****** */` 2, the
   outputs' `row-3` 2, and the connection's plaintext username 1.
4. **A timeout cancels the query on Postgres.** `SELECT pg_sleep(600)` with
   `timeout: PT30S`: the query reached Postgres at 11 s, the task FAILED
   `Timeout after 00:00:30.000`, and Postgres logged a second connection from
   the pod's IP followed by `ERROR: canceling statement due to user request`
   on the query's backend in the same millisecond, then its disconnection;
   `pg_stat_activity` held **0** `pg_sleep(600)` backends at every sample
   from then to 65 s. The pod went to `Failed` with a deletion grace of 15 s,
   then was removed.
   **With `timeout: PT10S`, as the plan wrote it, the check is vacuous**:
   in two runs the pod's `main` started at about 6 s and the plugin never
   connected (Postgres' connection log holds no connection from the pod), so
   the "0 active" those runs show proves nothing. That is the
   timeout-shorter-than-boot limit (W5b spec §12), measured on kind.
5. **A disabled feature refuses at publish and at run, and creates no pod.**
   With `workflow_kestra_plugins` off, publishing a new flow and re-publishing
   an old one both answered **400** `the 'workflow_kestra_plugins' feature is
   disabled for this workspace; …`; starting the version published while it
   was on ran the run-time gate: FAILED in 1.0 s with the same reason as its
   one ERROR log line, and **0** `egret-tasks` events and **0** pods from that
   moment on, against 12 events in the minute before (the control).
6. **`pluginDefaults:` is refused**: a list on a flow with a plugin task,
   `[]` on a flow with only a `Log`, and a list on a flow with only a `Log`
   each show `removed_plugin_defaults` in `validate` and answer **400** at
   publish; the same `Log` flow without the key validates clean and
   publishes (the control).

**Found live, not by any test:** nothing new in the product. The chart's
bundled MinIO image (`quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z…`)
answered **401** from quay.io, so the chart's post-install hooks never ran and
`egret init` was applied by hand from `helm template --show-only
templates/init-job.yaml`; that is the chart's MinIO pin, not W5b's.

**Live, driven by tests, each gated by an env var** (a run without the gate
reports IGNORED or fails, never passes empty):

| test | gate | what it runs |
|---|---|---|
| `tests/plugin_host_live.rs` `plugin_tasks_run_end_to_end_and_no_secret_is_stored` | `EGRET_PLUGIN_HOST_LIVE=1` (`#[ignore]`, `--ignored`) | Docker runner, `egret-plugin-host:dev`: Postgres, Trino 483 and StarRocks 4.1.4 `Query`s; masking; the `workflow.*` scan |
| `tests/plugin_host_live.rs` `a_timed_out_query_is_cancelled_on_postgres` | `EGRET_PLUGIN_HOST_LIVE=1` | `pg_sleep(600)`, `timeout: PT30S`: query sent, then cancelled (`canceling statement due to user request`) |
| `tests/container_docker_live.rs` `docker_cancel_with_grace_lets_a_sigterm_trap_run`, `docker_cancel_returns_only_after_the_container_stopped` | Docker daemon present; `EGRET_ALLOW_DOCKER_SKIP=1` opts out | the Docker runner's graceful cancel |
| `tests/k8s_runner_live.rs` `k8s_cancel_with_grace_waits_for_termination_then_release_deletes` | `EGRET_TEST_KIND=1`, context `kind-egret` | the Kubernetes runner's graceful cancel, as the chart's Role |
| `plugin-host` `EgretPluginHostTest` (19 tests) | none: `mvn -f plugin-host/pom.xml verify`, Java 25 | the shim's W9 traps, exit codes, the kill switch |

**Not measured, and stated so nobody infers it:**

1. **`kill()` alone on Trino.** The W5b spec promised a live test; it was not
   built (the plan's live tests time out on Postgres only). Task 1 measured
   Trino's query ending through Kestra's kill, which also interrupts the
   thread.
2. **StarRocks is not cancelled** (spec §7.1, §12), by design of the measured
   Connector/J lock; not re-measured here.
3. **The upstream `kill()` window** (spec §12): a SIGTERM between the plugin
   opening its connection and publishing its statement closes the socket
   with no cancel. Known, not observed.
4. **NetworkPolicy was not enforced in this run** (kindnet); the task pods
   reached Postgres in `e2e-data` through rule 1, which is what W5a measured
   on Calico.
5. **The ratchet cannot judge any of this in either direction**, for the
   reasons above.

### W9a — internal storage, and the one entry it banked

**109 before W9a and 108 after, both measured.** Before: the
`executor_matches_golden` line printed with `--nocapture` at the package base
(W9a Task 1, `baseline-ratchet.txt`, code = `5e43eb58`): **294 golden
entries, 109 mismatching, 109 baselined, 0 broken**. After: the same line in
Task 14's full gate, on the branch rebased onto `origin/main`:
**294 golden entries, 108 mismatching, 108 baselined, 0 broken** (`executor_matches_golden -- --nocapture`, HEAD `58d92e7e`). `golden.json` and `corpus/` are byte-identical to
`origin/main`'s (`git diff --quiet origin/main HEAD -- …golden.json …corpus`
exits 0); `baseline.txt` lost exactly one line, `valids/loop-values-from-uri`.

| | ratchet (measured) | what the task shipped |
|---|---|---|
| package base `5e43eb58` | 109 (Task 1) | -- |
| Task 1 | 109 | the artifact measured (`oracle-exec/w9a-storage.json`): URIs, the file functions, FILE inputs, STORE files (binary Ion), the plugin bridge's reach, the core storage tasks, `Loop` over Ion |
| Tasks 2-5 | 109 (each task's report) | streaming in `egret-storage`; URIs, the index (migration 0166) and `resolve_readable_many`; the render snapshot and the file functions; staging `inputFiles` URIs into a pod |
| Task 6 | 109 | `outputFiles`/`outputDirectory` collected into URIs (**breaking**, below) |
| Task 7 | 109 | FILE inputs: the multipart start, JSON URIs, replay and loop copies |
| Task 8 | 109 | the execution file API (download, metas, preview of CSV/JSONL/Ion) |
| Task 9 | 109 | the plugin storage bridge (`EgretStorage`), the four storage-needing plugin classes in the catalog |
| Task 10 | 109 | `egret.sql` `fetch: STORE` |
| Task 11 | **108** | `Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`; `Loop` over a file (migration 0167): **`valids/loop-values-from-uri` banked** |
| Task 12 | 108 | `Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems` |
| Task 13 | 108 | the retention pruner and the orphan sweep (migration 0168) |
| Task 14 | **108** (measured, above) | the end to end on a temporary kind cluster, this section, the specs' corrections, the full local gate |

**The corpus's flows that touch W9a, and which moved** (`/usr/bin/grep -rl`
over `oracle-exec/corpus/`; each classified against `harvest-run.txt`,
`golden.json` and `baseline.txt`):

| population | flow | in the golden | baselined before → after | why |
|---|---|---|---|---|
| FILE input (5) | `valids/inputs`, `valids/inputs-small-files`, `valids/restart_with_inputs`, `valids/pause_on_resume` | no | -- | not-standalone at harvest: Kestra's own test supplies the file from the JVM classpath (or stays PAUSED), so there is no golden trace to compare |
| | `invalids/inputs-validation` | yes (refused) | no → no | Kestra refuses it at publish for a `multiselect` input, not for its FILE input; it matched before W9a and still does |
| `outputFiles` (1) | `valids/working-directory-outputs` | no | -- | rejected at harvest: `WorkingDirectoryTest$LocalFiles` is a test-only task type |
| STORE (2) | `valids/loop-outputs-store` | yes | yes → yes | a `Loop`'s own `outputs:` in STORE mode, which W9a does not produce (the user's ruling of 2026-10-03; W9a spec §15) |
| | `valids/webhook-store-body` | yes (FAILED) | no → no | a webhook trigger's STORE body; W9a does not implement it, and egret already matched Kestra's FAILED |
| `core.storage` (2) | `valids/loop-values-from-uri` | yes | **yes → no** | `Write` an Ion file, `Loop` over its URI: banked at Task 11, measured |
| | `valids/loop-outputs-auto` | yes | yes → yes | `fetchType: AUTO` over a file writes the `Loop`'s outputs as STORE files: same ruling as `loop-outputs-store` |

What the ratchet can see of W9a: the driver registers the nine core storage
tasks over the test `MemStore` (`oracle-exec/driver.rs:131-159`; `storage::`
occurs 5 times there, against `worker::` 30). It cannot see a pod, S3, the
plugin bridge, the file API or the pruner: no corpus flow declares a script
task, a plugin task or a FILE input that runs standalone.

#### The breaking change, and its release note

**`outputs.<task>.outputFiles` changed from contents to URIs.** W5a stored each
declared output file's contents (UTF-8, at most 1 MiB) in the task's outputs.
W9a stores `{relative path → kestra:// URI}`, as Kestra does. A flow that read
`{{ outputs.t.outputFiles['out.txt'] }}` as text reads
`{{ read(outputs.t.outputFiles['out.txt']) }}` now (up to
`workflow.storage.max_read_bytes`, 10 MiB), or passes the URI to a later
task's `inputFiles`, which egret stages. It reaches only script tasks, which
run behind the workspace feature `workflow_containers`, **off by default**.
The other changes a deployment sees:

- `workflow.container_runner` `docker` or `kubernetes` needs a `[storage]`
  section: the server refuses to start with one and not the other.
- The task-pod helper image needs `cat` (it already needed `sh`, `tar`,
  `touch`, `wc`, `sleep`, `tail`); Alpine's base image has it and the
  Dockerfile checks.
- W5a: the 1 MiB-per-file and UTF-8 output rules are gone (`max_file_bytes`
  1 GiB, `max_task_bytes` 5 GiB instead); a symlink, hard link or directory in
  the collected outputs fails the task by name; `outputDirectory: true` works;
  staged files carry the upload's time (W5a's were dated 1970) and uid 0 in
  the archive.
- W5b: `storage()` works instead of exiting 4; the catalog holds nine classes
  (`postgresql.Batch`, `CopyIn`, `CopyOut` and `mysql.Batch` join), and
  `fetchType: STORE` publishes; namespace files exit 4 naming W9b and
  `namespaceKv()` W9c.

- **Retention (new).** `workflow.storage_retention_days` (env
  `EGRET_WORKFLOW__STORAGE_RETENTION_DAYS`) defaults to **30**; `0` keeps files
  for ever. An hourly pruner deletes the files of an execution that ended more
  than that many days ago (a loop family together, counted from its loop root's
  end; never one that has not ended, nor a source a running dependent can
  still read). Afterwards the file API answers **404**, and `read()`,
  `fileSize`, `fileExists` and `isFileEmpty`, staging and the plugin bridge
  fail naming the URI and the window ("files are kept 30 days after their
  execution ends"). It runs only where the worker is on and `[storage]` is
  configured.
- **File API (new)**, under `/api/ws/{workspace}/workflows/executions/{id}/`:
  `file?path=` (download), `file/metas?path=` and
  `file/preview?path=&maxRows=` (CSV, JSONL or Ion; at most 5000 rows). All
  three need **`workflow:read`**, the same permission as reading the execution;
  no new permission was added. `path` must be a file of the execution's loop
  family, else 404.
- **Config keys (new)**, besides retention: `[workflow.storage]` `prefix`
  (default `workflow/`; non-empty, ends with `/`), `max_file_bytes` (1 GiB),
  `max_task_bytes` (5 GiB) and `max_read_bytes` (10 MiB, also the cap of a
  `Loop` over a file); each must be at least 1, and the server refuses to start
  otherwise. A multipart start carries at most `max_file_bytes` + 1 MiB.
- **Manual replay.** A replay copies the source's `FILE` inputs by URI; once
  one of those files was pruned the replay is refused with **400** naming the
  input, instead of starting an execution that cannot read it.
- **A permanent store error is retried, not failed** (W9a final review I2).
  `AccessDenied`, `NoSuchBucket` or a refused key is handled as an outage: the
  run is re-claimed every `working_timeout`, no attempt is spent, and it never
  fails on its own. It can only be egret's own `[storage]` credentials or
  bucket; the server log names the error, and fixing the configuration lets
  the waiting runs finish. Follow-up: classify permanent S3 errors.
- **Memory per reader** (spec §15). An Ion value read by an item task, a
  `Loop` or the preview can cost about 8 × `max_read_bytes`, per concurrent
  reader, and each concurrent `read()` up to `max_read_bytes` more; raising
  `max_read_bytes` raises both. A `Loop` over a file has no record-count cap
  (as Kestra): a 10 MiB file of two-byte records spawns ~5 million
  iterations.
- **Plugin pod memory.** The JVM runs with `-Xmx384m` (`pluginHost.jvmMaxHeap`)
  under the default task-pod memory limit of **512Mi**. A limit of 128Mi is
  OOM-killed (measured: `usage 131072kB, limit 131072kB`); keep the limit well
  above the heap if you lower `default_resources`.
- **Database: recovery for a dev/kind DB (the one place the recipe lives).**
  Migrations 0166-0168 were written as 0161/0169/0170 and renumbered. Two
  cases are affected, both on databases that are not production (the branch
  has not merged):
  1. **It applied the OLD numbers** (161 "workflow execution files", 169
     "workflow loop file values", 170 "workflow execution files pruned").
  2. **It applied the NEW 166-168 before the Task 14 fix-round-1 commit
     (40455dc4)**, which added comments to the three headers: sqlx hashes the
     whole file, comments included, so these answer `VersionMismatch`.

  Either reset it (`just reset-testdata`), or undo by hand and let the server
  re-apply:

  ```sql
  DROP TABLE IF EXISTS workflow.loop_file_values;
  ALTER TABLE workflow.executions
    DROP COLUMN IF EXISTS files_pruned_at,   -- takes workflow_executions_unswept_idx with it
    DROP COLUMN IF EXISTS files_swept_at;
  DROP TABLE IF EXISTS workflow.execution_files;
  ALTER TABLE workflow.executions
    DROP COLUMN IF EXISTS replayed_from_execution_id;  -- takes its FK, CHECKs and index
  -- case 1:
  DELETE FROM public._sqlx_migrations WHERE (version, description) IN
    ((161, 'workflow execution files'), (169, 'workflow loop file values'),
     (170, 'workflow execution files pruned'));
  -- case 2:
  DELETE FROM public._sqlx_migrations WHERE (version, description) IN
    ((166, 'workflow execution files'), (167, 'workflow loop file values'),
     (168, 'workflow execution files pruned'));
  ```

  Match on description as well as version (main's own 0161-0165 are Data Apps).
  Also drop `egret_test_template` on any Postgres a checkout runs tests
  against, so test templates rebuild. **No migration file is touched after
  merge**: the three files change their checksum only once more, here, before
  it. The local branch feat/sp4d-debt-and-guardrails also holds 0166-0168;
  whichever merges second renumbers.

The programme spec's W9a correction (§9) carries the same note.

#### What is covered live, and by which gate

**Live on a real cluster, through egret's API** (Task 14, a temporary kind
cluster `egret-w9a-e2e`, kind v0.29 / Kubernetes v1.33.1, kindnet; created
with its own kubeconfig so the shared `kind-egret` context was never
switched; the chart installed from this branch with
`workflow.containerRunner=kubernetes`, `workflow.workerEnabled=true`,
`workflow.pluginHost.image=egret-plugin-host:dev`, the backend image built
from the rebased branch; a Postgres 16 in `e2e-data`; transcripts
`w9a-t14-e2e-*.txt` in the package's working notes, every figure below copied
from them):

1. **A FILE input through a pod and back.** A multipart start uploaded a
   31-byte `in.csv` (`kestra:///e2e/w9a/files/executions/{id}/inputs/f/in.csv`);
   a `shell.Commands` task staged it as `inputFiles: {in.csv: "{{ inputs.f }}"}`,
   upper-cased it and declared `outputFiles: ["*.csv"]` (which collected the
   staged `in.csv` too, as a glob over the working directory does); a `Log`
   rendered `READ=ID,NAME\n1,ALPHA\n2,BETA\n3,GAMMA\n` through `read()`; and
   **a later task staged the earlier task's `outputFiles` URI through the real
   snapshot path** (Task 5's open check): `LATER 31 653fb20d…caff5`, the
   expected file's sha256. The file API downloaded the same 31 bytes (`cmp`
   equal, `attachment` disposition), the input file's bytes equal the upload,
   `metas` answered `{"size":31}`, and `preview` with `maxRows=2` answered
   `["ID,NAME","1,ALPHA"]`, `truncated: true`. Objects lie under
   `workflow/{workspace id}/e2e/w9a/files/executions/{id}/…` in the bucket.
2. **A plugin STORE file through `Batch`, `CopyOut` and `CopyIn`.**
   `postgresql.Query` `fetchType: STORE` (500 rows, `size: 500`, a `.ion` URI)
   → `postgresql.Batch` from that URI (`rowCount: 500`) → `CopyOut` (CSV)
   → `CopyIn` from the CopyOut URI (`rowCount: 500`). In Postgres: `src`,
   `dst` and `dst2` each 500 rows with `sum(n)` 125250, and `src EXCEPT dst2`
   empty. **The plan's `Query STORE → CopyIn` is not possible as written**:
   the STORE file is binary Ion and `CopyIn` reads CSV/text (Task 9's live
   test measured the same), so the chain goes through `Batch` and `CopyOut`.
3. **`core.storage.Write` → `Loop` over its URI → `Size`.** Three iterations,
   each its own execution with `loop_value` `{"n":1,…}`, `{"n":2,…}`,
   `{"n":3,…}` and the `Return` outputs `0 - …`, `1 - …`, `2 - …`; the loop
   run's `rendered` is the URI and `3`, not the records; `Size` 55, the
   content's byte count.
4. **The pod holds no storage credential.** `kubectl get pods -n egret-tasks
   -o yaml` taken while scenario 1's task slept (pod `Running`, 2/2): 8091
   bytes, **0** occurrences of the secret key `egretsecret`, of `minio`, of
   `9000`, of `STORAGE`, `ACCESS_KEY`, `SECRET_KEY`, `s3://` and the bucket
   name `warehouse`; no value equal to the access key `egret` (the pod's only
   env is `WORKING_DIR=/egret/working-dir`). Controls in the same dump:
   `alpine:3.20` **2**, `egret-task-helper:dev` **4**.
5. **Another workspace cannot read the URI.** Workspace `w9a2`, with the same
   namespace and flow id, ran `{{ read('<w9a1's out.csv URI>') }}`: FAILED,
   *"… belongs to an execution this flow may not read …"*, naming the URI.
   The file API from `w9a2` answered **404** for `w9a1`'s execution
   (`not found: execution`) and **404** for that URI under its own execution
   (`no such file of execution …`); the same request in `w9a1`: **200**.
6. **Retention.** With `EGRET_WORKFLOW__STORAGE_RETENTION_DAYS=1` and
   scenario 1's execution's `updated_at` set back two days, the restarted
   server's first pass logged `workflow storage pruned executions=1 orphans=0
   failed=0`: its 3 index rows went to 0, `files_pruned_at` was set, its 3
   objects left the bucket (6 → 3; the other two executions' 3 stayed), the
   file API answered **404** *"… was deleted: files are kept 1 days after their
   execution ends"* (an unpruned file: **200**), and a manual replay of it was
   refused **400** naming its pruned FILE input.

**Found live, not by any test:** nothing that fails. Two wordings: the
readable-set refusal lists *"its own execution's files, its loop family's …,
its trigger execution's and, for a replay, its source's"* and omits the
trigger's loop family the 2026-10-01 ruling added; the pruned message says
*"1 days"*. Both were fixed in Task 14 fix round 1 (the message now says *"1 day"* and names the trigger's loop family). The chart's MinIO images (`quay.io/minio/…`) answered **401** again,
as in W5b; the e2e loaded a local `minio/minio` (RELEASE.2025-09-07, the same
release, `mc` RELEASE.2025-08-13 inside) as `deps.minio.image` and `mcImage`.

**Found by a live suite on the same cluster, and fixed in the test:**
`k8s_runner_live`'s `a_plugin_stores_and_reads_storage_in_a_pod` failed in
the suite (21/22) and in 2 of 3 runs alone: the plugin host's JVM, run with
`-Xmx384m` (sized below egret's default 512Mi), was OOM-killed by its memory
cgroup under the suite's own 128Mi pod limit (`memory: usage 131072kB, limit
131072kB`, java anon-rss 129 MB). The plugin cases now use egret's default
limit: 5 of 5 alone, then the whole suite **22 passed, 0 failed**. The chart's
defaults never had the mismatch, and every plugin pod of the e2e succeeded.

**Live, driven by tests, each gated** (without the gate a case reports
IGNORED or fails, except where the table says it prints a skip):

| test | gate | what it runs |
|---|---|---|
| `egret-storage` `tests/s3_stream.rs` (11) | `EGRET_TEST_S3_ENDPOINT` + `_KEY`/`_SECRET`/`_BUCKET` (prints a loud skip line without them) | single PUT and multipart, `get_stream`, `head`, `delete_prefix`, abort, against MinIO |
| `tests/storage_render_s3_live.rs` | `EGRET_TEST_S3_*` | `read()` on a real `S3Store` under concurrent renders (the multi-thread rule); FILE upload |
| `tests/storage_files_api_s3_live.rs` | `EGRET_TEST_S3_*` | a 64 MiB download streamed, a sliver pulled when the first chunk arrives |
| `tests/storage_prune_s3_live.rs` | `EGRET_TEST_S3_*` | `delete_prefix` of an ended execution, the sweep reading S3's `last_modified`, under a throwaway prefix |
| `tests/container_docker_live.rs` | Docker daemon; `EGRET_ALLOW_DOCKER_SKIP=1` opts out | staging and collecting through `docker cp`, mtimes, Python zipping a staged file |
| `tests/k8s_runner_live.rs` | `EGRET_TEST_KIND=1` (`EGRET_TEST_KIND_CONTEXT` for a temporary cluster) | streamed staging in and out of a pod, `tar -x` dying in the pod, as the chart's Role |
| `tests/plugin_host_live.rs` `plugins_read_and_write_storage_through_egret` | `EGRET_PLUGIN_HOST_LIVE=1` + `EGRET_TEST_S3_*` | Query STORE → Batch → CopyOut → CopyIn → `read()`, on Docker |
| `tests/sql_starrocks_live.rs` | `EGRET_TEST_STARROCKS_URL` (`EGRET_ALLOW_ENGINE_SKIP=1` opts out) | `egret.sql` STORE against StarRocks, 50k rows |
| `plugin-host` `EgretStorageTest` (21) | none: `mvn -f plugin-host/pom.xml verify`, Java 25 | the bridge's reads, writes, caps, refusals |

**Not measured, and stated so nobody infers it:**

1. **NetworkPolicy was not enforced** (kindnet), as in W5b: the task pods'
   reach is judged by the pod spec here, not by a CNI.
2. **No S3 outage was induced live**; the infrastructure-error split (a store
   outage re-claims the run, spends no attempt) rests on unit and MemStore
   tests.
3. **`egret.sql` STORE on Trino**, and `mysql.Query` `inputFile` (StarRocks
   refuses `LOAD DATA`, Task 1).
4. **A large file through a pod on kind**: the e2e moved bytes and kilobytes;
   the 12 MiB and 64 MiB cases are the gated tests above.
5. **The ratchet cannot judge any of the e2e**, for the reasons above.

#### The sweep: messages and docs that described W9's absence

Counted with `/usr/bin/grep` with file arguments at the rebased tip before
this task edited anything.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | `\bW9\b` in `crates/egret-workflow/src`, `plugin-host/src` and both programme specs | **20** lines (src 8: `runners/functions.rs` 6, `worker/commands.rs` 2; `plugin-host/src` 0; spec EN 6, VI 6) | **2** messages in `commands.rs` (an `outputFiles` directory and the 1,000-name cap blamed "no internal storage … W9"); per spec **4** lines (§3 finding 4, the W5 row, the W9 row split, §11 question 1) plus the W5b correction's *"waits for W9"* annotated and a new W9a correction; **left**: `functions.rs`' 6 (they assign `http` to W9 deliberately, and name W9b/W9c for `fileURI`/`kv`), the W4/W5b-era history lines |
| 2 | *"1 MiB"* or *contents* about `outputFiles`, in workflow docs and doc comments | **49** `1 MiB` lines (`crates/egret-workflow/src`, `plugin-host/src`, the workflow specs EN+VI, this README) and **13** `outputFiles` lines naming contents; **15** of them state what `outputFiles` holds | **4**: the W5a spec's divergence row and properties line, EN and VI (dated notes). **Left 11**: the W5a spec's §1 table, EN and VI (headed as the state on `8f35f85a`); the W9a spec's header, §1 table (the state at the base), §6.2 and §15, EN and VI where they match (they describe the change itself); `handlers/container.rs:53` (already says URIs); the plan's two Task 14 lines |
| 3 | *"no internal storage"* | **21** (code 14, `plugin-host` 5, the plan 2) | **1** (`commands.rs`, counted in population 1 too); the other 20 are conditional (*"egret has no internal storage configured (the `[storage]` section …)"*, the plan's history) and true |

The W5b spec's storage refusals (§1 table, §3 shim, exit code 4, publish
refusals, the catalog note, the test table, §14, §15), EN and VI, each gain a
dated *Superseded 2026-10-04, W9a* note rather than a rewrite.

### W9b — namespace files

**108 before W9b and 108 after, both measured; no entry moved, as expected.**
Before: the `executor_matches_golden` line printed with `--nocapture` at the
package base (W9b Task 1, `w9b-baseline-ratchet.txt`, code = `e0543603`, on the
old base `e539d8b3`): **294 golden entries, 108 mismatching, 108 baselined, 0
broken**. After: the same line on the branch rebased onto `origin/main`
`edccf6c4` (W9b Task 11, HEAD `c61868b9`): **294 golden entries, 108 mismatching, 108 baselined, 0 broken** (`executor_matches_golden -- --nocapture`, 1 passed).
`golden.json`, `baseline.txt` and `corpus/` are byte-identical to
`origin/main`'s (`git diff --quiet edccf6c4 HEAD -- …golden.json …baseline.txt
…corpus` exits 0). W9b's spec (§7, §13) predicted no change, and a change would
have been a finding to explain, not a bank.

| | ratchet (measured) | what the task shipped |
|---|---|---|
| package base `e0543603` | 108 (Task 1) | -- |
| Task 1 | 108 | the artifact measured (`oracle-exec/w9b-namespace-files.json`): layout and revisions, soft delete, globs, the functions, `ifExists` across namespaces, the four tasks, the API, `PurgeFiles`, the path rules; the `_files` count |
| Tasks 2-4 | `executor_matches_golden` passed in each gate (a moved entry fails it; the line was not quoted) | the store, URIs and path rules (migration 0174, written as 0169); revisions, conflicts, move and soft delete under advisory locks; the five expression functions over namespace files |
| Task 5 | 108 | `namespaceFiles` staged on the four script types; `inputFiles` namespace-file URIs; `WorkingDirectory.namespaceFiles` validated at publish only |
| Task 6 | 108 | `UploadFiles`, `DownloadFiles` |
| Task 7 | 108 | `DeleteFiles`, `PurgeFiles` |
| Tasks 8-9 | `executor_matches_golden` passed in each gate | the namespace file API (staging, the hourly staging sweep), the export zip |
| Task 10 | 108 | the plugin bridge (`EgretNamespace`, `task.json`'s `namespaceFiles`) |
| Task 11 | **108** (above) | the end to end on a temporary kind cluster, this section, the specs' corrections, the full local gate |

**The two `WorkingDirectory` corpus flows, before and after: unchanged, and
outside the ratchet both times.** `valids/working-directory-namespace-files`
and `valids/working-directory-namespace-files-with-namespaces` are the only
corpus flows that declare `namespaceFiles`. Task 1 measured
(`w9b-baseline-corpus-flows.txt`, HEAD `e0543603`) and Task 11 re-measured
(HEAD `c61868b9`, the same `/usr/bin/grep -n 'namespace-files'` over
`baseline.txt`, `golden.json`, `runnable.txt` and `harvest-run.txt`) the same
four lines and nothing else: `runnable.txt:223`/`:225` `[task-type-not-in-image]`
and `harvest-run.txt:347`/`:348` *"rejected publish -> 422: Invalid type:
io.kestra.core.tasks.test.Read"*. Neither is in `baseline.txt` (whose 3
`working-directory` lines are other flows) or among the 294 golden entries. The
artifact refused both at publish, so there is no golden trace to compare; the
spec's first wording, *"stay baselined"*, was corrected on 2026-10-05. Egret
now publishes their `namespaceFiles` block (validated, kept in the model) and
still does not run `WorkingDirectory`, so no ratchet change can come from them.

**The `_files`-namespace count (Task 1), the population the new publish refusal
reaches.** 0 of 345 corpus flows have a namespace with a `_files` segment
(control: 342 have an `io` segment). Of published flows, **0 of 5**: a
read-only `SELECT` over every database of the dev Postgres that has
`workflow.flows` (38 of 44, plus `:5439`) found 5 rows in all, every one in
namespace `t` (`w9b-files-segment-namespaces.txt`). That zero rests on very few
rows: no database with real flows existed to count (the dev `egret` database
was at migration 127 and has no `workflow.flows`). A deployment that has a
flow in such a namespace sees the publish refusal on its next save, and the
flow's existing executions are untouched.

What the ratchet can see of W9b: in principle the four `core.namespace`
tasks, which the driver registers over the test `MemStore` with the
namespace-file database options of the pool it runs (`oracle-exec/driver.rs:129-169`,
since W9b Tasks 6 and 7); in practice nothing. **0** corpus flows name a
`core.namespace` task, `nsfile` or `fileURI` (control: 108 name
`core.log.Log`), `golden.json` holds none of the three, and the only
`namespaceFiles` declarations are the two `WorkingDirectory` flows above. No
corpus flow runs a script or plugin task standalone either. Every W9b
behaviour rests on the tests below.

#### What is covered live, and by which gate

**Live on a real cluster, through egret's API** (Task 11, a temporary kind
cluster `egret-w9b-e2e`, kind v0.29 / Kubernetes v1.33.1, kindnet, its own
kubeconfig so the shared `kind-egret` context was never touched; the chart
from this branch with `workflow.containerRunner=kubernetes`,
`workflow.workerEnabled=true`, `workflow.pluginHost.image=egret-plugin-host:dev`
and the backend image built at `c61868b9`; the chart's object store is now
RustFS, with **distinct** root credentials so a zero below means something; a
Postgres 16 in `e2e-data`; transcripts `w9b-e2e-0-setup.txt` … `w9b-e2e-7.txt`
and `w9b-e2e-server-log.txt` in the package's working notes, every figure below
copied from them). **7 of 7 passed; no product defect was found.**

1. **Upload twice, revisions, two downloads** (`w9b-e2e-1.txt`). Two uploads of
   `/cfg/a.txt` and one of `/data.csv`, each 200; `revisions` answered
   `[{revision:1,size:10},{revision:2,size:26}]`; `?revision=1` and the latest
   download each `cmp`-equal to their upload, differing at byte 9; `stats`,
   `directory` and `search` (`q=A.TXT` → `["/cfg/a.txt"]`) answered; the
   bucket keys follow §3's layout (`…/e2e/w9b/_files/cfg/a.txt` and `….v2`).
2. **Functions and script staging** (`w9b-e2e-2.txt`). A `Log` printed
   `READ=[config v2 (second upload)\n] URI=[kestra:///e2e/w9b/_files/cfg/a.txt.v2]`.
   A script task in a pod, `namespaceFiles` over `e2e.w9b` and `e2e.other`
   with `folderPerNamespace: true`, printed both namespaces' files under their
   folders, `./e2e.w9b/cfg/a.txt=OVERWRITTEN-BY-INPUTFILES` (an `inputFiles`
   entry overwrote the namespace file), and the `inputFiles` values
   `nsfile://e2e.other/x.txt` and `{{ fileURI('data.csv') }}` staged.
3. **`UploadFiles` → `DownloadFiles` → `DeleteFiles` → `PurgeFiles`**
   (`w9b-e2e-3.txt`). Three upload runs gave revisions 1-3 of `/up/w.txt` and
   `/up/gone.txt`; `DownloadFiles` `/up/**` gave execution URIs whose `read()`
   was revision 3; after `DeleteFiles` `/up/gone.txt` its revision 3 was
   flagged deleted, a latest read 404 and `?revision=2` 200 (R1);
   `PurgeFiles` `keepAmount: 1`, `filePattern: "up/*"` answered `size: 5`:
   `w.txt` kept `[3]`, `gone.txt` was purged entirely (§2), `/cfg/a.txt` kept
   `[1,2]`, and the bucket held only `up/w.txt.v3` under `/up/`. A first purge
   with `"/up/*"` purged nothing (`size: 0`): the pattern is matched against
   the path without its leading `/`, as Kestra's is (spec §8.4, measured by
   Task 1) — the specified behaviour, and a trap the spec's §15 now records.
4. **`CopyIn` from `{{ fileURI('data.csv') }}`** (`w9b-e2e-4.txt`), run by the
   plugin host in a pod: `rowCount: 3`, and Postgres held the three rows.
5. **The pod holds no storage credential** (`w9b-e2e-5.txt`). `kubectl get
   pod -o yaml` of three task pods while their `main` container ran — a script
   pod with `namespaceFiles` and both kinds of namespace `inputFiles` (held by
   `sleep 20`), a plugin pod (`pg_sleep(20)`), and a plugin pod **using the
   nsfile bridge** (`CopyIn`, held by a `LOCK TABLE`) — counted **0** of the
   access key, the secret key, the endpoint, `rustfs`, `:9000`, `warehouse`,
   `s3://`, `AWS_`, `STORAGE`, `ACCESS_KEY` and `SECRET_KEY` in each, against
   controls `alpine:3.20` 2, `egret-plugin-host:dev` 2 and 2,
   `egret-task-helper:dev` 4, 4, 4. The bridge pod's `task.json` lists the file
   as `{"uri":"kestra:///e2e/w9b/_files/data.csv", …, "staged":".egret/storage/0"}`
   and holds no key, endpoint or bucket. Both storage keys occur 0 times in the
   server log.
6. **Isolation** (`w9b-e2e-6.txt`). From a second workspace, reading the first
   workspace's file by every form — `read` of `nsfile://`, of the
   `kestra:///…/_files/…` URI and its `.v2`, with `namespace=`, `fileURI`, a
   script `inputFiles`, a plugin `CopyIn from:`, `namespaceFiles.namespaces`,
   `DownloadFiles` — failed naming the path or staged nothing (`COUNT=0`,
   `files: {}`); the API answered 404 on every route that takes a path, while
   `search` (`200 []`) and `export` (an empty 22-byte zip) were `cmp`-identical
   to a namespace that never held anything. A flow writing to another
   namespace of its own workspace (`UploadFiles`, `DeleteFiles`, `PurgeFiles`)
   failed naming both namespaces and the rule; its control succeeded; nothing
   moved in the bucket.
7. **Export** (`w9b-e2e-7.txt`). `200 application/zip`,
   `filename="e2e.w9b_files.zip"`, `Content-Length: 414` and 414 bytes
   received; `unzip -t` "No errors detected"; three stored entries `cmp`-equal
   to the uploads; Python's `testzip` and Java's `ZipInputStream` (the reader
   Kestra's import uses) read them all.

**Live, driven by tests, each gated** (without its gate a case reports IGNORED
or fails, never passes; all were run during the package with the gates set):

| test | gate | what it runs |
|---|---|---|
| `tests/nsfiles_s3_live.rs` (2) | `EGRET_TEST_S3_ENDPOINT` + `_KEY`/`_SECRET`/`_BUCKET`, `DATABASE_URL` | a namespace file larger than one multipart part round-trips; a move and a purge of names holding spaces and non-ASCII, under a fenced throwaway prefix |
| `tests/nsfiles_api_s3_live.rs` (2) | `EGRET_TEST_S3_*`, `DATABASE_URL` | a 64 MiB upload through staging and a server-side copy, streamed back down; the staging sweep deleting an old orphan, fenced |
| `tests/container_docker_live.rs` `namespace_files_are_staged_into_the_container_as_the_artifact_does` | Docker daemon; `EGRET_ALLOW_DOCKER_SKIP=1` opts out | two namespaces, `folderPerNamespace`, each `ifExists`, and an `inputFiles` entry overwriting a namespace file under every `ifExists` |
| `tests/k8s_runner_live.rs` `namespace_files_are_staged_into_the_pod_as_the_artifact_does` | `EGRET_TEST_KIND=1` (`EGRET_TEST_KIND_CONTEXT` for a temporary cluster) | the same in a pod, as the chart's Role |
| `tests/plugin_host_live.rs` `plugins_read_namespace_files_through_egret` | `EGRET_PLUGIN_HOST_LIVE=1`, Docker, `egret-plugin-host:dev`, `EGRET_TEST_S3_*` | a plugin reading namespace files through the bridge, on Docker |
| `tests/nsfiles_render_outage.rs` | `DATABASE_URL` (its own binary: it installs namespace-file connect options that reach a closed port) | a database outage during a namespace lookup at render is egret's, not the author's |
| `plugin-host` `EgretNamespaceTest` (11) | none: `mvn -f plugin-host/pom.xml verify`, Java 25 | the bridge's `Namespace` over staged files, the latest view, the refusals |

**Not measured live, and stated so nobody infers it:**

1. **NetworkPolicy was not enforced** (kindnet), as in W5b and W9a.
2. **The frontend** was not built; the e2e drove the API only.
3. **A large file through a pod on kind**: the e2e moved bytes; the 64 MiB and
   multi-part cases are the gated S3 tests above.
4. **No store or database outage was induced live**; the infrastructure-error
   split rests on `nsfiles_render_outage` and the MemStore tests.
5. **`WorkingDirectory`** does not run on egret, so its `namespaceFiles` was
   only published, never staged. *(Since no-silent-stall Task 1 a
   `WorkingDirectory` does not publish either: it is refused as
   `unsupported_working_directory`, and its `namespaceFiles` is still
   validated, so a draft shows both findings.)*
6. **The ratchet cannot judge any of this**, for the reason above.

#### The release note

The programme spec's W9b correction (§9) carries it; in short: the config key
`workflow.storage.max_namespace_files` (10 000); migration **0174** (written as
0169, renumbered at the rebase onto `edccf6c4`, before merge: a dev database
that applied the branch's 0169 must reset with `just reset-testdata`, or delete
`(169, 'workflow namespace files')` from `_sqlx_migrations`, drop
`workflow.namespace_files` and `workflow.namespace_file_staging`, and drop
`egret_test_template`); the staging table and its hourly sweep; publish refuses
a namespace with a `_files` segment and a flow id `_files`; `namespaceFiles`
publishes on script tasks; `WorkingDirectory.namespaceFiles` is validated at
publish; nine API routes plus the export; the plugin host no longer exits 4 on
`storage().namespace(..)`; `workflow.storage.prefix` over 200 bytes is refused
at boot.

#### The sweep: messages and docs that described W9b's absence

Counted with `/usr/bin/grep` with file arguments at `c61868b9`, before this
task edited anything.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | `-w W9b` in `crates/egret-workflow/src`, `plugin-host/src` and both programme specs | **163** lines (src 139, `plugin-host/src` 12, spec EN 6, VI 6) | **0** in code: every code line is a provenance tag or describes W9b as present (Tasks 2-10 updated each message they flipped). Per spec **5** of 6 lines, each with a dated note rather than a rewrite: §3 finding 4, the W9 row, the W5b correction's *"namespace files for W9b"*, the W9a release note's *"namespace files exit 4 naming W9b"* and the W9a correction's *"What is still not true: namespace files (W9b)"*; plus a new W9b correction. **Left**: the W9a correction's account of the split, which stays true |
| 2 | `namespaceFiles` in `docs/`, `crates/egret-workflow/src`, `plugin-host/src` and this README, saying it is refused | **170** lines | **2**: the W5a spec's *"Refused at save … `namespaceFiles`"*, EN and VI (dated note). **Left**: the W9b spec's §1 table (the state at the base) and §7 (the change itself), the plan's task text, the code's doc comments (already dated by Task 5) |
| 3 | `NO_NAMESPACE_FILES` | **8** lines (`ShimRunContext.java` 1, W9b spec EN 2, VI 2, plan 3) | **0**: the code line is the replacement's history comment; the spec lines are §1 (state at the base) and §10 (the change) |
| 4 | (beside the brief's three) `-w W9b` in the W5b and W9a specs | **7** EN (W5b 3, W9a 4), **7** VI | **3** per language: the W5b spec's shim line and exit code 4, and the W9a spec's *"`fileURI` stays refused, now naming W9b"* (dated notes). **Left**: scope and out-of-scope lines that assign the work to W9b, which stay true |


### W9c — the KV store

**108 before W9c and 108 after, both measured; no entry moved, as expected.**
Before: the `executor_matches_golden` line printed with `--nocapture` at the
package base (W9c Task 1, `w9c-baseline-ratchet.txt`, code = `35fbaeee`, on
`origin/main` `11782600`): **294 golden entries, 108 mismatching, 108
baselined, 0 broken**. After: the same line at W9c Task 8 (HEAD `1e9832a4`;
the rebase was a no-op, because `origin/main` was still `11782600`):
**294 golden entries, 108 mismatching, 108 baselined, 0 broken**
(`executor_matches_golden -- --nocapture`, 1 passed). `golden.json`,
`baseline.txt` and `corpus/` are byte-identical to `origin/main`'s
(`git diff --quiet 11782600 HEAD -- …golden.json …baseline.txt …corpus` exits
0). W9c's spec (§12, §13) predicted no change, and a change would have been a
finding to explain, not a bank.

| | ratchet (measured) | what the task shipped |
|---|---|---|
| package base `35fbaeee` | 108 (Task 1) | -- |
| Task 1 | 108 | the artifact measured (`oracle-exec/w9c-kv.json`); the `_kv` count |
| Tasks 2-3 | `executor_matches_golden` passed in each gate | migration 0175 (`workflow.kv_entries`), the key rule, `Resource::Kv`, reads, revisions and the inheritance walk; writes over W9b's staging, soft delete, `Put`'s locked merge, the purge by row id, the hourly expiry sweep |
| Task 4 | 108 (`7c1a9baa`) | `kv()`; `secret_in_kv_value` at publish; exact comparisons (the comparator rewrite pass) |
| Task 5 | 108 | the six `io.kestra.plugin.core.kv` tasks |
| Task 6 | `executor_matches_golden` passed in each gate | the seven API routes |
| Task 7 | `executor_matches_golden` passed in its gate | the plugin host's `namespaceKv()` message |
| Task 8 | **108** (above; Task 8a measured the same at `1e9832a4`) | the end to end on a temporary kind cluster, this section, the specs' corrections, the full local gate |

**The fixture: 444 cells, 266 measured, 11 code, 167 chosen.**
`oracle-exec/w9c-kv.json`, re-counted at Task 8 by the package's checker
(`cells 444 (measured 266, code 11, chosen 167); chosen_cells 167; errors 0`).
Task 1 committed 431 cells (274 / 12 / 145); the user's rulings and the fix
rounds of Tasks 1, 3 and 5 moved cells from measured to chosen (each with
Kestra's measured answer under `kestra` and the reason under `why`) and added
cells. A **measured** cell is Kestra's answer and egret's; a **code** cell is
computed by running the artifact's own class in jshell (`Put.mergeValues`'s
edge cases); a **chosen** cell is egret's deliberate divergence, every one
listed in `egret_expected_basis.chosen_cells`.

**The two corpus flows, before and after.**

- `valids/retry-with-flowable-errors` (`baseline.txt:614`). Before: a draft
  refused at publish (an unknown `kv.Set`) and a baselined golden entry.
  After: it **publishes** (the census moved `LIVE` 247 → 248 and
  `UNKNOWN_TYPE_REFUSED` 66 → 65 at Task 5, `oracle-exec/driver.rs`), and its
  golden entry **stays baselined**, because it also needs the `AllowFailure`
  flowable, which egret still plans opaque. W9c alone cannot bank it.
- `valids/kv`. No golden entry before or after: the artifact's harvest did not
  run it standalone (`harvest-run.txt:150`, *not-standalone*). It publishes
  before and after (`tests/publish.rs`,
  `valids_kv_and_retry_with_flowable_errors_publish`).

**The `_kv`-namespace count (Task 1), the population the new publish refusal
reaches.** 0 of 345 corpus flows have a namespace with a `_kv` segment, and 0
published flows on the dev Postgres (one read-only `SELECT`; that database
held no flows, so its zero cannot disagree; `kv_segment_namespaces` in the
fixture). A deployment with such a flow sees the refusal on its next save.

What the ratchet can see of W9c: in principle the six `core.kv` tasks and
`kv()`; in practice one flow, `retry-with-flowable-errors`, which stays
baselined for `AllowFailure`. Every W9c behaviour rests on the tests below and
the end to end.

#### What is covered live, and by which gate

**Live on a real cluster, through egret's API** (W9c Task 8, a temporary kind
cluster `egret-w9c-e2e`, kind v0.29.0, its own kubeconfig so the shared
`kind-egret` context was never touched; the chart from this branch with
`workflow.workerEnabled=true`, `containerRunner` left at `none`, RustFS with
**distinct** root credentials, and the backend image built at `1e9832a4`;
transcripts `w9c-e2e-0-setup.txt` … `w9c-e2e-6.txt` and
`w9c-e2e-server-log.txt` in the package's working notes, every figure below
copied from them). **6 of 6 passed; no product defect was found.**

1. **The API's `PUT` bodies, `GET`, revisions** (`w9c-e2e-1.txt`). `{a:1}` →
   JSON, `1` → NUMBER, `hello world` → STRING (the whole text, the raw-text
   ruling), `1.50` → NUMBER `1.5`, an offset timestamp → DATETIME in UTC,
   `2025-01-02` → DATE, `not ion {a:` → the raw text. A blob, an s-expression,
   a blob nested in a struct and a whitespace-only body each answered 400, and
   a `GET` of each refused key 404, so nothing was written. `description` and
   `ttl: PT1H` came back. A second `PUT` of `cfg` gave revisions
   `[{revision:1,size:5},{revision:2,size:13}]`, each readable. Exact decimals:
   `123456789012345678901234567890.5` and a 33-digit integer kept every digit,
   `0.0000001` came back plain. The bucket keys follow §3's layout
   (`…/e2e/w9c/_kv/cfg.ion` and `cfg.ion.v2`).
2. **`kv()`, inheritance, `errorOnMissing`, expired = missing**
   (`w9c-e2e-2.txt`). From `e2e.w9c.child.leaf`: `kv('n')` → `1`,
   `kv('cfg').a` → `2`, `kv('dec') == 1.5` → `true`; a leaf `Set` shadowed the
   parent's `n`; an explicit namespace did not walk; `Get` with the flow's
   namespace walked, and with another did not. `kv('nope')` failed naming the
   key; `Get` with `errorOnMissing: true` failed with Kestra's message.
   Keys written with `ttl: PT3S` (by the API and by `Set`) were missing to
   every reader after 6 s (`kv()`, `Get`, the API's 404, `revisions`
   `expired: true`), the walk skipped an expired level to the parent's value,
   and the `workflow.kv_entries` rows were the same before and after every
   read (reads never write).
3. **`Set`, `Get`, `GetKeys`, `Put`, `Delete`, `PurgeKV`, with the API's
   revisions after each** (`w9c-e2e-3.txt`). A YAML map `Set` was stored
   typed (R-a); `overwrite: false` failed with Kestra's message; `Put` deep
   merged (a list appended, a dotted key nested, the description carried
   over); a conflicting `q.r`/`q` patch was refused naming both and wrote
   nothing; `Delete` answered `true` then `false` and a soft-deleted revision
   stayed readable by number (R1); `PurgeKV` by `keepAmount: 1` (`size: 2`),
   by `before` (`size: 1`, Kestra's superseded-at rule) and the default
   expired purge (`size: 1`) each left exactly the expected revisions and
   objects; a version purge with neither property was refused.
4. **`secret()` in a `Set` or `Put` value** (`w9c-e2e-4.txt`): refused at
   publish with `secret_in_kv_value`, at `tasks.leak.value` and at a nested
   `Put` leaf; the control published and ran.
5. **Isolation** (`w9c-e2e-5.txt`). A second workspace reading the first's
   namespace saw nothing (`kv()` null or "does not exist", `Get` `{}`,
   `GetKeys` `[]`); its purge, `Delete` and bulk delete touched nothing; its
   API answered **byte for byte** as for a namespace that never held anything
   (listings `200 []`, key, revision and `revisions` reads 404). A flow
   writing its parent, sibling or child namespace (`Set`, `Put`, `Delete`,
   `PurgeKV`) failed naming both namespaces and the rule; the control
   succeeded; the other rows (md5) and objects (sha) were identical before and
   after.
6. **The hourly expiry sweep, observed live** (`w9c-e2e-6.txt`). There is no
   config knob (`KvSweepConfig::default()`, `interval_at`: the first pass an
   hour after start), so the run waited for the first pass on the unmodified
   pod: the server logged `expired KV keys swept removed=4`, and the four
   expired keys' rows and objects were gone across both workspaces, while the
   controls (no ttl, and a `PT1H` ttl not yet expired) kept theirs.

**Live, driven by tests, each gated** (without its gate a case reports IGNORED
or fails, never passes; all were run during the package with the gates set):

| test | gate | what it runs |
|---|---|---|
| `tests/kv_s3_live.rs` (1 case, `harness = false`) | `EGRET_TEST_S3_ENDPOINT` + `_KEY`/`_SECRET`/`_BUCKET`, `DATABASE_URL` | a value of exactly `max_read_bytes` (10 MiB) written through staging and read back, one byte over refused, `Put`'s merge, a delete, a purge and the sweep deleting real objects, under a fenced throwaway prefix |
| `tests/kv_store.rs`, `kv_sweep.rs`, `kv_tasks.rs`, `kv_render.rs`, `kv_api.rs`, `kv_api_small_cap.rs`, `kv_api_no_store.rs` | `DATABASE_URL` (Postgres; the store is `MemStore`, or a probe over it) | the store, the concurrency cases (`overwrite: false`, two `Put`s, revision numbering), the sweep (two in parallel, a key rewritten between listing and lock), the six tasks and their re-claims, `kv()` in `runIf`/`If`/`Switch` and its outage, every API status (409, 413, 408, 503 included) |
| `plugin-host` `EgretPluginHostTest` | none: `mvn -f plugin-host/pom.xml verify`, Java 25, after `plugin-host/fetch-plugins.sh plugin-host/target/plugins` | the `namespaceKv()` message, exactly |
| `worker::handlers::kestra` unit tests | none | the exit-4 prefix, and that no `task.json` key names KV |

**Not measured live, and stated so nobody infers it:**

1. **The plugin host under a container runner.** `containerRunner` was
   `none`, so no plugin pod ran: neither the no-regression check of a
   `kestra.…postgresql.Query` nor `namespaceKv()` (no catalog plugin calls
   it). The new message is pinned by Task 7's tests only.
2. **409, 413, 408 and 503 were not driven live**: the lock wait, the caps,
   the body timeouts and the no-store answer rest on `kv_api*.rs`.
3. **The sweep's concurrency**: two replicas at once, a busy key skipped, a
   key rewritten between listing and lock, a failed object delete. The
   in-process tests in `kv_sweep.rs` and `kv_s3_live.rs` cover them.
4. **The frontend** was not built; the e2e drove the API only.
5. **The ratchet cannot judge any of this**, for the reason above.

#### The release note

The programme spec's W9c correction (§9) carries it; in short: migration
**0175** (`workflow.kv_entries`); `workflow.storage.max_read_bytes` must be at
least 4096, checked at boot; the hourly expiry sweep on every process with a
store; the bulk delete's 1,000-key cap and 1 MiB body; the 1,000-digit number
limit and the dense-value limit; `secret()` refused in a `Set`/`Put` value; a
`_kv` namespace segment refused at publish; exact decimals and comparisons
(the comparator rewrite pass, `kestra_` names reserved, chained comparisons
evaluating every operand); `Put`'s limits; seven API routes, no new
permission; the plugin host's new `namespaceKv()` message; the known limits of
the W9c spec's §15.

#### The sweep: messages and docs that described W9c's absence

Counted with `/usr/bin/grep` with file arguments at `1e9832a4`, before this
task edited anything.

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | `\bW9c\b` in `crates/egret-workflow/src`, `plugin-host/src` and both programme specs | **120** lines (src 98, `plugin-host/src` 4, spec EN 9, VI 9) | **0** in code: every code line is a provenance tag or describes W9c as present; no message says KV "needs W9c" or "does not provide yet" (`functions.rs`'s *"`kv` still names W9c"* is a dated W9b-era history line, followed in the same doc by the 2026-10-07 line that took `kv` out). Per spec **8** of 9 lines, each with a dated note rather than a rewrite: §3 finding 4 (two lines, one note), the W9 row, the W5b correction's *"only KV still waits, for W9c"*, the W9a release note's *"`namespaceKv()` W9c"* and its *"What is still not true … the KV store (W9c)"*, the W9b release note's *"`namespaceKv()` still exits 4 naming W9c"* and its *"What is still not true … the KV store (W9c)"*; plus a new W9c correction. **Left**: the W9a correction's account of the split, which stays true |
| 2 | lines naming `kv` beside `REFUSED`, `not_yet`, *"stays refused"* or *"refuses `kv`"*, in `crates/egret-workflow/src`, `docs/`, `plugin-host/src` and this README; plus the `REFUSED` table's own doc (`functions.rs:262-282`) | **15** lines, and the doc | **2**: the W9a spec's *"`kv` names W9c"*, EN and VI (dated note). **Left**: the W9c spec's §1 table (the state at the base) and §6/§12 (the change itself), the plans' task text, `functions.rs`'s `kv_without_a_scope_refuses_by_name` doc (a render with no scope still refuses `kv()` by name, which is true), and the `REFUSED` doc, whose last dated line already says `kv` is implemented. The expression oracle's `oracle/README.md` already carries a W9c Task 4 paragraph |
| 3 | `namespaceKv` in `docs/`, `crates/egret-workflow/src`, `plugin-host/src` and this README | **40** lines (W9c spec 7+7, W9c plan 8, W9a plan 1, programme spec 2+2, W5b spec 1+1, `kestra.rs` 4, `plugin-host/src` 6, this README 1) | **2**: the W5b spec's shim line, EN and VI (dated note); the programme spec's 4 are row 1's. **Left**: the code and Java lines (Task 7 rewrote each), the W9c spec and plan (the change), the W9a plan's task text, and this README's W9a release note (that section's record) |
| 4 | `the namespace KV store` (the old refusal's wording) | **4** lines (W9c spec §1 EN and VI, the W9a plan, the W9c plan) | **0**: each quotes the old refusal as the state at the base or as task text |
| 5 | (beside the brief's four) `\bW9c\b` in the W5b, W9a and W9b specs | **7** EN (W5b 3, W9a 3, W9b 1), **7** VI | **2** per language: the W5b spec's exit code 4, and the W9a spec's `kv` line (counted in row 2 too). **Left**: scope and out-of-scope lines that assign the KV store to W9c, which stay true |

### What this ratchet does **not** prove

Each of these has a section of its own above with the derivation; they are
gathered here so the boundary can be read in one place.

**1. The corpus is Kestra's own test suite, which is a sample of an infinite
space.** 345 flows is what one project chose to write tests for. Nothing here
says anything about the flow a user writes tomorrow. The ratchet is a regression
guard, not a bound on behaviour: it fails when a *comparable* entry moves, and a
flow nobody wrote is comparable to nothing.

**2. Two thirds of the kept ordered pairs cannot fail — and the pairs are not
special.** **705** of 1104 today, derived by
`kept_ordered_pairs_are_not_all_asserted_and_the_baseline_is_the_line`; the
flip experiment that demonstrated it was run at **747**, on the W4 tree, and is
labelled above rather than adjusted. A pair becomes assertable when its entry
banks, not before. **The same holds of every other golden field on a baselined
entry**, because `executor_matches_golden` reports only a non-baselined mismatch
or a baselined match: **396** of 840 run terminals, **1528** of 3355 run-history
tokens, **522** of 1462 execution-history tokens, **147** of 408 executions and
**43** of 142 sub-executions are un-falsifiable for exactly the same reason. The
statement used to carry this sentence about the pairs alone, with no
counterpart, which reads as though the pairs were the weak family. They are the
measured one.

*(**All six figures in this item were a package behind and W6 Task 8's sweep
re-derived them**: 747/461/1807/751/191/53 were the W4-era values, at
`baseline.txt` = 143. Each is a constant the suite asserts on every run —
`TERMINALS_ON_BASELINED_ENTRIES` and its four siblings, and the pairs table
above — so every one of them was **compared by a running program under a name
this paragraph did not use**. That is the shape: not a figure nobody checked, a
figure checked under its own name and copied out under a different one. The
ratio the item is about did not move — 744/1104 was still two thirds — which is
exactly why nothing prompted a re-read.)*

*(**And it happened again in W8, in the same item, for the same reason.** Every
figure above was a package behind a second time when W8 Task 7 swept: 744 / 433
/ 1703 / 665 / 165 were the W6-era values, at `baseline.txt` = 127, and the
eighteen entries W8 banked moved five of the six. **43** is the one that did not
— none of the eighteen carries a sub-execution — which is why the honest count
is five of six. Each of the five is a `const` in `tests/executor.rs` whose own
doc comment carries the whole W8 arc and was updated by each task that moved it;
this prose copy, six hundred lines away, was updated by none of them. **The
parenthesis above did not prevent the recurrence, and that is the finding**: a
warning written beside a figure does not check the figure. The rule that would
have caught it is *sweep the population a figure belongs to, not the sentence
that quotes it* — here, every name in the provenance table's first row.)*

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
StarRocks and `worker::handlers::sql`'s own unit tests.

**Since W10 there is a second, independent reason, and it is worth stating
because the first one could have been read as an accident of harvesting:
`egret.sql` is registered in NEITHER registry.** `worker::Handlers::with_core_tasks()`
— what `egret-bin` hands `runtime::spawn`, and therefore the whole of what a
deployment runs — is four handlers, and `driver::registry()` is the same four.
`handlers::sql::Sql` is constructed from an `Arc<dyn QueryExecPort>` and neither
constructor has one: `with_core_tasks()` takes no arguments, and the oracle's
registry must never reach an engine. A deployment wires it in one line
(`add_publishable`, not `add` — the type has a `TASK_TYPES` row and the
invariant applies to it). So the zero above is *zero corpus entries*, and this is
*zero registrations*, and a green ratchet is silent about both.

**That suite used to self-skip, and this statement was wrong about what that
looked like.** It said *"7 of the 26 tests W3b Task 6 added do not run by
default"*, and the whole-branch review measured the observable in one command:

```text
without EGRET_TEST_STARROCKS_URL:  7 passed; 0 failed; 0 ignored — 0.00s
with    EGRET_TEST_STARROCKS_URL:  7 passed; 0 failed; 0 ignored — 0.88s
```

Identical counts, nothing `ignored`, only the duration differing — so **26 of 26
reported `ok` either way**, and the only instrument this handler has reported
success having asserted nothing. It **fails** now, and a skip has to be typed
into the invocation as `EGRET_ALLOW_ENGINE_SKIP=1`, which is the rule
`egret-test-db`'s `skip_verdict` already applies to `DATABASE_URL` and the rule
W3b Task 7 applied to `python3` in `tests/executor.rs` — *"there is no
environment in which skipping is the right answer"* — and did not apply here, to
the suite this section rests on. `just check`, `just test-no-db` and CI type the
opt-out, so a green with no engine says so in the command that produced it. The
gate itself is driven in both directions by
`the_engine_gate_fails_rather_than_skipping_and_the_opt_out_must_be_typed`, which
is the one assertion in that file that runs without an engine. See *W3b Task 6 —
`egret.sql`, and the coverage this ratchet does NOT give it*.

**3b. `egret.container` and `egret.script.python` reach zero corpus entries too,
and for a reason the harvester cannot fix — and since W10 they are registered
nowhere either, which is a second reason standing beside the first.** Both
handlers exist and both have `spec::registry::TASK_TYPES` rows, so publish
accepts a flow declaring them; **nothing constructs either one.** They are not
in `with_core_tasks()` — the registry `egret-bin` hands `runtime::spawn`, and
therefore the whole of what any egret deployment runs — and not in
`driver::registry()`. A flow that names one publishes and then **hangs**, for
the mechanism item 19 derives. W4 Task 5 measured the corpus half rather than
trusting the brief:

| population | flows naming a script/container/shell/command type | flows naming `core.log.Log` (the control) |
|---|---|---|
| the whole corpus — 345 flows | **0** | 108 |

**Derived by `no_corpus_entry_can_exercise_the_egret_sql_handler`**, the same
test item 3 rests on, which asserts `(345, 0, 0, 108)` — every corpus flow, the
`egret.*` flows, the script/container/shell/command flows of any vendor, and the
control — with the total beside the parts. W4 Task 6 moved it there; it was a
shell pipeline quoted in this section, which is the one thing this statement
says it never does:

```bash
# W4 Task 5's original derivation, kept as the record of how the zero was found
cd crates/egret-workflow/oracle-exec
find corpus -name '*.yaml' -o -name '*.yml' | wc -l                     # 345
grep -rhoE 'type:\s*\S+' corpus | sed 's/.*type:[[:space:]]*//' | sort -u \
  | grep -icE 'script|container|shell|command|docker|bash|python|node'   # 0
```

**The pipeline and the test count different things and agree**, which is worth
saying rather than leaving for someone to notice: `sort -u` makes the pipeline's
zero a count of distinct **types**, and the test's is a count of **flows**. Both
are zero, and only one of them is asserted on every run. The test's filter also
carries a positive control of its own — a filter that answers `false` to
everything produces this table exactly.

The control is there for item 3's reason: a scanner that parsed nothing would
report zero and look exactly like the truth, and it cannot also report 108 flows
naming a type it failed to parse. Kestra's own script and container tasks live
in plugin jars (`/app/plugins/io_kestra_plugin__plugin-script-*.jar`,
`…plugin-docker…`) that this corpus never reaches, so **the ratchet sitting at
143 after Task 5 is not evidence about either handler.**

What they rest on instead is `tests/container_docker_live.rs` against a real
Docker daemon and the unit tests in `worker::commands`,
`worker::handlers::container` and `worker::handlers::python`. Fix round 1 moved
the split between the two: `ingest_line` and `ingest_output_file` are pure
functions now, so the marker cap, the output-file caps, the missing file, the
directory and the log level are all judged **without a daemon**, and what stays
in the live suite is what genuinely needs one — *was a process started*, and
*does the daemon agree which container this worker owns*.

**That suite does not use libtest**, and that is the other half of item 3's
rule. libtest has no stable way to report a test IGNORED at run time, so every
self-skipping suite in this workspace counts a test that did not run as
**passed** — which is exactly what made the `egret.sql` defect above readable
only from the clock. Measured, on one binary:

```text
daemon + images:                      15 passed;  0 failed;  0 ignored — 15.21s
no daemon, no opt-out:                 1 passed; 14 failed;  0 ignored —  0.00s, exit 101
no daemon, EGRET_ALLOW_DOCKER_SKIP=1:  1 passed;  0 failed; 14 ignored —  0.00s
daemon up, images not pulled, opt-out: 1 passed;  0 failed; 14 ignored —  0.00s
```

*(All four rows moved by one, because the suite gained
`a_symlinked_output_file_cannot_read_the_worker_hosts_files` in Task 5's fix
round 2 and this table was written before it. The first three were **re-measured
on this tree** — the "no daemon" half by pointing `DOCKER_HOST` at a socket that
does not exist, which is the axis the gate reads.*

*The fourth was **derived from `main`**, and W4 Task 6's first attempt at this
paragraph got it wrong twice over: it left the row at 13 and wrote that the row
had not been re-measured because emptying this machine's image cache costs more
than the row is worth. **The figure is not about the image cache.** `main`'s
`Ok(false)` arm reports `selected.len()` ignored; `selected` is
`daemon_cases()` — **14** cases — filtered by the CLI filter; and the image axis
decides only **which verdict** is produced, never how many cases are in it. So
the row is 14 by reading thirty lines of `container_docker_live.rs`, and the
"not re-measured" caveat was a category error sitting **two lines under a note
saying every row moved by one**. It was the fourteenth stale figure, inside the
correction that retires the other thirteen. A caveat is not a substitute for a
derivation, and "I did not measure it" is only honest when measuring is what the
figure needed.)*

The image axis is under the same opt-out as the daemon axis (fix round 1's
Important 3: it was not, so a machine with Docker running and nothing pulled
hard-failed through `just check`), and the one test that passes in every row is
the gate's own, which drives a pure verdict function and needs neither.

Neither handler is in `Handlers::with_core_tasks()` **or** in the ratchet's own
`oracle-exec/driver.rs` registry, for `egret.sql`'s structural reason plus one
of their own: a registry that runs arbitrary container images by default is a
capability decision and not a handler's. A caller wires them in one line.

**Both are in `spec::registry::TASK_TYPES`, and the first version of this
section was wrong to imply otherwise.** `egret.container` has been there since
W1; `egret.script.python` was added in fix round 1, because the `egret.python`
row it first tried to claim is the **notebook-kernel** task (programme spec
§6.4) and its `notebook` reference is required.
`worker::handlers::container`'s `the_publish_table_and_these_handlers_agree`
pins all of that, so this paragraph cannot decay into the last one.

**4. Durability is proved by `tests/worker.rs`'s properties, not by this
corpus.** There are **five**, and the fifth is the whole-branch review's: *a
worker whose lease was reaped must not be able to commit.* The first four could
not see it — the reap property reaps a run whose handler never ran, and nothing
called `apply_outcome` twice with one `Claimed` — and the hole it names needed no
configuration at all: `reap` clears `claimed_at` and leaves `state = 'RUNNING'`,
which is the claim predicate, so a run is re-claimable while its first handler is
still in flight, `DEFAULT_WORKING_TIMEOUT` is an hour, `Handlers::with_core_tasks`
registers `Sleep`, and `duration: PT2H` opens the window, with nothing able to
tell the first handler to stop — W4 Task 3's kill path fires only on a
task-level budget, inside the worker that owns the run, and `reap` is an
`UPDATE` run by another process holding no handle to anyone's future. Measured
on the shipped code, two terminals landed on one
attempt, the second **wiping `ended_seq` from `Some(4)` to NULL** — the column
migration 0136 exists to hold and the happens-before endpoint this oracle reads —
and the execution reached `FAILED` while a successor was `RUNNING` and claimable,
which is the dual of the plan's Global Constraint. `Claimed` now carries
`claimed_at` as a fencing token, both of `apply_outcome`'s writes carry
`AND claimed_at = $n`, and **0 rows affected is a `WorkerError::LeaseLost`
rather than a silent success** — the two are the same value to `execute()`, which
is how a fence that is written but never read passes everything.

The other four: two concurrent `tick`s never claim the same run;
a process killed between the handler and the commit leaves nothing half-applied
(and the run comes back only *through* `reap`, which is what makes the lease a
live path rather than an unreachable one); `reap` reclaims exactly what
`working_timeout_s` says, asserted in **both** directions; and **a run whose
result committed is never claimed again**. The fourth is invisible to any corpus
by construction: a re-run that reaches the same terminal state produces an
**identical trace**, so the only place a duplicate execution is visible is the
handler's call log. A green ratchet is compatible with running every task twice.

**5. One result per TRANSACTION is a ruling, and W4 Task 4 made it.** Four
entries — `valids/dag`, `valids/if`, `valids/parallel-disabled-tasks`,
`valids/full` — reproduce every run, attempt and execution state Kestra recorded
and differ on the **happens-before relation alone**. `worker::apply_outcome`
applies one handler result, drives `advance()` to a fixed point and commits,
where Kestra's worker pool finished two siblings before either successor
existed. See *One result per transaction is a ruling* below for both sides of
the measurement, the three entries the alternative banks, and the four costs it
was refused for — the binding one being that widening would leave two lease-fence
guards green over a path they do not touch, measured in
`w4t4-widened-tick.txt` RUN 4. It is still the single largest lever left on the
ratchet, and it is now a decided one rather than an open one, with a named way
back in.

**6. 84 of the 294 executions end unfinished — 63 since W6 Task 3, and 59 since
W6 Task 4**, counted and
named by `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`. A
task type with no handler — or a task the executor plans **opaque**, so the claim
takes it and `models::TaskKind` has no leaf for it — makes `Registry::run` or
`Claimed::input` refuse, the tick abort for that run and the execution stall,
which is what the shipping worker does.

> **The 21 that left are the whole of `Subflow`, and they are the second half of
> that sentence rather than the first.** Through W5 a `Subflow` was planned an
> opaque `Runnable`, which set `task_runs.type_id`, which made it claimable —
> and `claim::leaf` then answered `None`, because `models::TaskKind` for the
> same task is not a `Leaf`. Every entry holding an executed top-level `Subflow`
> stalled there. A `TaskKind::Executable` gets no `type_id`, so nothing claims
> it, and `worker::subflow` terminates it: 12 of the 21 are the entries W6 Task
> 3 banked, and the other nine still mismatch on something else (five registered
> in `STILL_OVER_ACCEPTED_AT_RUNTIME`, two on `state::apply`'s attempt rule, two
> on the missing `WARNING`). **The remaining 63 became 59 at W6 Task 4, and
> those four never stalled at all**: the `retry-new-execution-*` entries declare
> only `core.execution.Fail`, which HAS a handler and ran it, and they were
> counted because `is_stalled` omitted `RETRIED` from its terminal list while
> `RunState::Retried`'s own doc, Kestra's `State.isTerminated()`,
> `worker::claim::settle` and `worker::spawn::terminated_count` all called it
> terminal. **A classifier artefact leaving the population, not four executions
> that started finishing** — the count would have fallen to 59 with `RETRIED`
> added and no replay implemented at all. The 59 are what the sentence above
> always meant: a task type with no handler. So the driver returns the partial
> trace,
and `Broken` is now reserved for the instrument failing: the step bound, a panic,
or `WorkerError::Corrupt`, which `driver.rs` used to discard along with every
other worker error under `Err(_) => continue` and can now be reported. Nothing
here exercises what `reap` would then do with the leased run in production.

**The count used to exclude the case it exists to bound.** The collection ran
*after* `if actual == expected { continue; }`, so an execution that stalled and
nevertheless MATCHED — egret doing less than Kestra and agreeing anyway — was
excluded by construction. It is collected before the `continue` now and the
intersection is asserted empty.

**It is empty, and an assertion over an empty set cannot tell a working
classifier from a broken one** — measured: moving the three lines back below the
`continue` kills nothing at all. So the classifier is a function, `is_stalled`,
driven on both answers by `a_stalled_execution_that_also_matched_would_be_counted`,
and making it answer `false` unconditionally kills that test **and** the
corpus-wide count. The emptiness is a measurement; the instrument that measures
it has its own positive control.

**7. Nothing keyed on elapsed time is reachable**, because `driver::pass_time`
moves the clock only to the next `not_before` something is already waiting for.
The cost is one named entry: `valids/retry-failed-flow-duration` is bounded by
`maxDuration: PT7S` with no `maxAttempts`, Kestra ran three attempts and egret
runs four, because an attempt that costs no elapsed time leaves room for another
inside the same window.

**8. No banked entry takes an `else` branch**, so the ratchet cannot see a
predicate that is wrong in the truthy direction; three mutants prove it rather
than argue it, and all three leave `executor_matches_golden` passing.

**9. The ratchet's answer used to depend on scheduling. It does not now, and
the fix is one `ORDER BY`.** W3b Task 7 found it and W3b Task 7's fix round
closed it; it is kept here because the *reason* it existed is a property of this
oracle that a reader has to know.

`worker::claim` ordered by `created_at` alone. One `settle` writes every run of
one round in one transaction and `now()` in Postgres is TRANSACTION time, so
sibling runs share that instant to the microsecond and the sort had nothing left
to compare — measured, six siblings whose physical order had been perturbed by an
ordinary `UPDATE` came back `[0, 2, 4, 3, 1, 5]`. W3b Task 2 recorded the missing
tiebreak as *"benign — `SKIP LOCKED` already makes it arbitrary"*, and it was:
which of two ready runs goes first is not a correctness property. **W3b Task 5
made it one**, because the round a run terminates in is its `ended_seq`, which is
an endpoint of the happens-before relation this oracle compares.

Observed before the fix: `executor_matches_golden` failing about one run in eight
under load, **in both directions** — `valids/parallel-nested` regressing on five
ordered pairs, and `valids/parallel-fail-with-flowable` matching its way out of
the baseline — and reproduced on the package's base commit, so it was neither new
nor anybody's edit.

The claim now orders by `created_at, execution_id, run_ix`, on **both** the
subquery that decides *which* rows are taken (the whole answer at `LIMIT 1`,
which is what `driver::BATCH` is) and the outer select that decides the order
they come back in. `(execution_id, run_ix)` is `workflow.task_runs`'s own unique
key, so there is no tie left for a scan to decide, and `run_ix` is `RunIx` — the
executor's creation order, which is the order Kestra's `taskRunList` carries.

**Proved by mechanism, not by counting green runs**, and that is deliberate:
a paired measurement under load produced **0 failures in 20 control runs (the
tiebreak reverted) and 0 in 40 fixed runs**, which is not evidence of anything —
a control that does not reproduce the fault cannot say the fix removed it. What
*is* evidence is that the causal channel is exact and deterministic:

| experiment | result |
|---|---|
| six siblings, physical order perturbed, tiebreak absent | claim returns `[0, 2, 4, 3, 1, 5]` — an arbitrary permutation |
| the same, tiebreak present | `[0, 1, 2, 3, 4, 5]`, and `[0,1,2,3,4,5]` for the un-perturbed control too |
| **tiebreak REVERSED** (`run_ix DESC`), nothing else changed | the ratchet moves: **143 mismatching, `valids/parallel-nested` a REGRESSION, `valids/dag` and `valids/parallel-fail-with-flowable` banking** |

The third row is the one that closes it. It names *the same two entries that were
observed flaking*, from a deterministic one-token change — so the claim's order is
demonstrably what decided those entries' verdicts, and that order is now a total
one over a unique key.

**The corpus did not move.** Every derived figure this statement carried was
identical either side of the tiebreak **on the day the experiment was run** —
294/144/144/0, 150 matching, 16/6/19, 35 entries with a `spawned`, 565 + 126
pairs, 87 removed, the field breakdown, the residual baseline. The order the
tiebreak pins is the one an idle machine already produced; what changed is that
it is now guaranteed rather than observed.

*(Read that list as a record, not as this statement's figures: five of the eight
have moved since, with `valids/replay-loop`'s bank at the review's fix round —
143/151, 17/8/16, 80 removed. **The tiebreak experiment has not been re-run**,
and W4 Task 6 is not claiming it would give the same answer; what it re-derived
is the figures, not the experiment. A sentence of the form "every figure here is
identical under X" ages with the figures and not with X, which is why it is now
dated instead of standing.)*

**And the tiebreak is a choice, which the third row makes visible.** Under
`run_ix DESC` this oracle banks `valids/dag` — the entry this file has called
instrument-bound since W3a Task 4, on the ordered pair `(task3, task4)` "no state
machine without a clock can bank". It is bankable, by finishing sibling runs in
the other order; it just costs `valids/parallel-nested`, which is a bank today.
So a claim order is a decision about *which of Kestra's serialisations to
reproduce*, not a neutral tidy-up, and `run_ix` ascending is chosen because it is
the executor's own creation order rather than because it banks more.

**10. Over-acceptance has a runtime register now, and it has eleven rows.**
`STILL_OVER_ACCEPTED` is **publish-only** — flows Kestra refused and egret plans
— and W3b creates the first *runtime* instances, egret running something Kestra
did not. Every one of them was named somewhere in this repository and none was
counted as the class the plan's Global Constraint forbids shipping.
`runtime_over_acceptance_is_registered_and_owned` derives them from the two
traces, on three shapes — egret produced more runs, egret produced more
sub-executions, or a run Kestra ended `KILLED`/`FAILED` that egret ended
`SUCCESS`/`WARNING` — and requires each to be in the register with an owner:

| owner | entries | what egret does that Kestra did not |
|---|---|---|
| **W2** (the renderer answers where Java throws) | `all-flowable`, `disable-flowable`, `restart-child`, `restart-loop` | `{{ taskrun.value }}` is undefined in a `Loop` body and a `Switch` case in this artifact and Kestra FAILED both; `{{ 1 / taskrun.attemptsCount }}` renders `Infinity` where Java throws. So the run succeeds and everything after it runs — `all-flowable` 12 runs and 3 sub-executions against Kestra's 3 and 1 |
| **W4** (the kill path) | `parallel-fail-with-flowable` | a `Sleep` still in flight when its sibling failed the `Parallel`. W4 Task 3 wired `TaskHandler::cancel` to the budget's expiry — the artifact's only `kill(null)` site — and this entry is not that: `advance()` abandoned the run, and `advance()` has no worker to tell. The row stands and the owner is still W4 |
| **W8** (`sla:`) | ~~`flow-concurrency-sla-assert-cancel`~~, `flow-concurrency-sla-fail-child`, ~~`sla-execution-condition`~~, `sla-max-duration-fail`, `sla-subflow` | **Five until W8 Task 6, three now.** The two struck out are the `EXECUTION_ASSERTION`s: egret cancels the execution before any run exists, so it no longer runs anything Kestra did not, and both bank. The three left are `MAX_DURATION`s, where Kestra KILLED the `Sleep` and egret completes it — and that is an over-acceptance of the **harness** rather than of egret, because `driver.rs` caps the `Sleep` to `ZERO` and no window exists to kill it in. They stay registered rather than excused |
| **unclaimed** (a `Loop`'s own `outputs:`) | `loop-outputs-failed-render` | Kestra FAILED the loop at iteration 1 when its `outputs:` could not render; `advance()` has no loop `outputs:`, so egret runs all three |

A **stalled** execution is deliberately not in this class: egret doing less than
Kestra is under-acceptance, which the ratchet already reports as a mismatch and
item 6 already counts.

**11. A retry with no live sibling never resumed, and `claim.rs` used to say the
opposite — CLOSED by W4 Task 1, and this oracle could see neither the hole nor
the fix.** As W3b left it: the module doc justified omitting a `not_before` term
with *"the delay is `advance()`'s to enforce and it already does"*; nothing in
this worker made the delay elapse; `retry::reopen_due` is `not_before`'s only
reader and it runs inside a `settle` whose `now` is fixed; `reap` excludes
`RETRYING` as the claim does; and `resume`, the one entry point that re-reads
the clock, had no production caller — its only caller was this oracle's
`driver::pass_time`, which *rewinds* `executions.created_at` because there is no
other way to make time pass.

W4 Task 1 added `claim::due_retries` and `claim::wake`, and `tick` calls the
waker ahead of the claim. **The second half of W3b's own correction turned out
to be false as well**: it said a `not_before` term *"would be a guard nobody
could ever see fail"*, and the term exists now, in a statement of its own, with
a test that has seen it fail in both directions
(`tests/worker.rs::a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`).
What is true is narrower: the term cannot live on the *claim*, because a
`RETRYING` run is not runnable until `advance()` puts it back to `CREATED`.

**The ratchet did not move: 143 mismatching before and after, 0 banked, 0
regressions, over three runs each side.** 14 baselined entries declare a
`retry:` with a non-zero `interval:`, 10 of them with the only behaviour that
writes a `not_before` at all, and none of them moved. `wake` was instrumented to
print every execution it selected and run over the whole 294-entry oracle: it
fired **zero** times, with `tests/worker.rs` as the positive control for the
instrument on the same build. The mutation that deletes the `RETRYING` term is
invisible here for the same reason — under it the whole `tests/executor.rs`
binary stays green and only `tests/worker.rs` fails. **This is a hole the ratchet
speaks for in neither direction.**

**Two things W4 Task 6 changed about this paragraph, and it kept the 14.**

*First, it is derived now.* This paragraph said the 14 was *"derived rather than
assumed"* and **nothing derived it** — no test in this crate counted a `retry:`
declaration until
`the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them`, which
now prints the whole split with every total beside its parts. The number was
right; the citation was the assumption. It also widens the population, because
the coverage question is not a question about `baseline.txt`: **25** golden
entries declare a `retry:`, **11 of them matching**, and a change on a matching
entry is one this ratchet reports. So retrying itself — `ScheduleRetry`, the
`RETRYING` state, `reopen_due` — is not the hole. **The waker is.**

*Second, the mechanism was stated backwards.* This paragraph said `pass_time`
*"rewinds `executions.created_at` before any deadline can elapse in wall-clock
time, so `due_retries`' predicate is never satisfied"*. Rewinding `created_at`
is what **makes** a deadline elapse, so as written the sentence gives the reason
the predicate would be satisfied. The true mechanism is one line further on:
`driver::pass_time` rewinds the column **and then calls `worker::resume`
itself**, in the same function (`driver.rs`, the `UPDATE` and the `resume`
immediately under it). So the oracle contains a second waker, and it consumes
every due retry inside the call that made it due — `tick`'s own `wake` runs on
every step and never sees a `RETRYING` run past its deadline.

**And `driver.rs` had already documented that**, in the module doc every task in
this package read before touching the oracle: *"it changes one timestamp and
**calls the same `resume` any other caller would**"* (`driver.rs`, the numbered
list, item 2). So the sentence this statement carried — that the oracle's
intervention is the rewind — did not merely lack a derivation; it contradicted
the driver's own doc, in the same directory, for a whole package. That is the
strongest available argument for why this section exists at all: **the fact was
not missing, it was written down in the file the claim is about, and five tasks
wrote the opposite anyway.** A claim about a mechanism has to be read off the
mechanism, and the cheapest place to read it off is the doc of the function
being described. `due_retries`'
`SELECT` is therefore *evaluated constantly and selects nothing*, which is a
different fact from a predicate that is never reached, and item 13 is a third
thing again. The zero was measured correctly either way; only the explanation
was wrong, and an explanation is what a reader of a coverage statement carries
away.

**Third, W8 Task 7 found that the predicate itself was wrong, and this
paragraph's own figures had gone stale — both invisible here for the same
reason.**

*The predicate.* `due_retries`' gate read `state = 'RETRYING' OR (state =
'RETRIED' AND …)`, so it **would never wake a `PAUSED` run**. W8 gave that state
two producers — `Pause` at Task 3 and `LoopUntil`/`WaitFor` at Task 4;
`RunState::Restarted` is **not** a third, and the W8 section says why — and
eight banked golden entries depend on a bounded wait ending, so this was a live defect in the product and not a prediction. Every one
of those eight ends its wait through `driver::pass_time`, which selects on
`not_before_ms` with **no state term anywhere in its SQL** and then calls
`worker::resume` itself, so the driver reaches the same runs through a different
door: **the ratchet is 109 with the `'PAUSED'` arm and 109 with it removed**,
measured in both directions rather than argued. That is this item's own shape one
package on, and it is why the two instruments that DO score it are integration
tests: `a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken`
drives three real pauses with the second elapsing in real time, and
`the_waker_names_exactly_the_states_something_will_move` asks the predicate about
**every** member of `RunState::ALL` and asserts its own table's key set equals
`ALL`'s — so a fourteenth variant fails rather than joining the unnamed majority
by omission, which is the class no figure sweep can find.

*The figures.* This paragraph's `14` / `10` / `11 matching` were right when W4
Task 6 derived them and are three packages stale: the derivation asserts
**`(8, 17, 25)`** — W6 Task 4 banked the four `CREATE_NEW_EXECUTION` entries and
W8 Task 5 banked `valids/task-allow-{warning,failure}-runnable`. **The paragraph
that says "it is derived now" is the one that went stale**, which is the sharpest
available statement of the rule: a citation to a running program is not a copy of
its output, and only the output is maintained. And the *"fired **zero** times"*
measurement is now latency-dependent rather than flat — **1 fire in one of five
runs and 0 in the other four**, on the pause population only; `wake` still fires
**0** times on the retry population, which is what the test's name asserts. The
ratchet is 109 in all five, because both doors call the same `worker::resume`
and which one opens changes no trace.

**12. `advance()` is unchanged by the whole of W3b** — and the sentence that
used to be here overstated it, so it is restated as measured. `git diff --stat
-- crates/egret-workflow/src/executor/` against the package's base
(`aa036cf1`) is **not** empty: it is **16 lines in 2 files**, all of them W3b
Task 3's `pub(super) fn parse_duration` becoming `pub fn` plus the doc comment
on its re-export, which that task wrote out in full at the re-export because the
Global Constraint asks for a sentence saying why. There is no semantic change
and `advance()` itself is byte-identical, which is the claim that matters and
the one the whole-branch review checked by blob hash at all 17 commits. The fix
round adds nothing to that directory — `git diff --stat 91205c08 --
crates/egret-workflow/src/executor/` is empty, checked with `if [ -z "$(…)" ]`
rather than a bare `&& echo CLEAN`. So every W3a statement about the state
machine still stands as W3a made it; what changed is the driver in front of it.

**W4 is stronger and the difference is worth stating rather than folding in:
all 11 files under `src/executor/` are blob-identical at `HEAD` to `ad93ef2d`,
W4's own base**, so this package's 16-line caveat does not have a W4 twin. Read
per file, because a `git diff` that prints nothing looks the same whether it
compared eleven files or none, and the file count is checked on both sides so a
deletion cannot hide inside an empty diff.

> **W6 Task 3 ENDS THAT, and this item is now a diff with a reason per file
> rather than a machine check.** The paragraph above about what that costs
> (*Pointers that no longer resolve*) is still exactly right, so it is not
> softened here: item 12 was a one-command check and is a judgement call from
> this commit on. What replaces it is the per-file diff below, each row saying
> what the change is FOR, and a single measured claim it all rests on.
>
> **The claim:** `flowable::start` emits `SUBMITTED` for every
> `TaskKind::Runnable`, and **not one of the 24 executed `Subflow` runs in
> `golden.json` carries it** — derived over the golden on every run by
> `no_subflow_run_in_the_golden_passes_through_submitted`, and from egret's side
> by `only_a_runnable_passes_through_submitted`. So `Subflow` planned an opaque
> `Runnable` — a runnable whose handler spawns and waits, the cheaper answer the
> plan asked to be tried first — mismatches EVERY entry the package exists to
> bank, whatever else its handler gets right. It banks nothing. That is the
> finding; the directory opening is its consequence.
>
> `git diff --stat 3539fea2 HEAD -- crates/egret-workflow/src/executor/`:
>
> | file | what the change is for |
> |---|---|
> | `plan.rs` | the new `TaskKind::Executable` variant, and the `(ModelKind::Subflow, "subflow")` arm that builds one. Kestra's own `ExecutableTask`, and its doc carries the measurement above |
> | `flowable/mod.rs` | `start` does not emit `SUBMITTED` for it and emits a `Command::SpawnSubflow` beside the `StartRun`; `descend` returns `Waiting` for it, as it does for a `Loop` |
> | `command.rs` | the `Command::SpawnSubflow` variant — **and the stale pointer this table's own successor list has been holding since W3b** |
> | `state.rs` | `apply`'s arm for it (a no-op, like `SpawnExecution`), and a second measured counter-example to rule 1 recorded beside the first |
> | `retry.rs` | `scan` spells Kestra's `RunnableTask \|\| Subflow` instead of half of it — the doc there predicted this exact edit |
> | `mod.rs` | `#[cfg(test)]` only: the two fixpoint helpers and `only_a_runnable_passes_through_submitted`, which gains an `Executable` arm and `valids/subflow-parent` |
> | `flowable/loop_.rs` | **nothing new** — the two stale pointers, fixed |
>
> Nothing here changes `advance()`'s contract for a kind that already existed:
> a `Runnable`, a `Sequential`, a `Parallel`, a `Dag`, an `If`, a `Switch` and a
> `Loop` all take exactly the commands they took before, which is what
> `only_a_runnable_passes_through_submitted`'s unchanged `flowables >= 8 &&
> runnables >= 21` and the whole unchanged W1–W5 half of the ratchet assert.
> `advance()` is still pure over `(plan, state, now)`.

**13. A task-level `timeout:` never reached a handler — CLOSED by W4 Task 2, and
the ratchet is blind to it for a different reason than item 11.** W3b measured
that `models::Task`'s deserializer takes `timeout` out of a leaf's `spec` before
a handler sees it, so `egret.sql` ran under a 300-second constant unrelated to
what the author wrote and every other handler ran unbounded;
`executor::plan::PlanTask::timeout` is `None` on every task and `advance()`
never reads one, so nothing anywhere enforced it. W4 Task 2 adds
`worker::timeout` — the budget is rendered per tick from the execution's frozen
`source`, carried on `TaskInput::timeout`, and enforced around `Registry::run`
in `claim::run_one`, which is where the artifact enforces it
(`AbstractWorkerCallable.callWithTimeout`). A timed-out run is **FAILED, not
KILLED**, carrying Kestra's own `Timeout after HH:mm:ss.SSS`; both facts were
taken from the pinned image twice, by `javap` and by running flows through a
standalone server.

**The ratchet did not move: 143 mismatching before and after, 0 banked, 0
regressions, over three runs each side** — the "before" measured on `d7233cf0`
itself, with the branch stashed and `egret_test_template` dropped between the
two.

**The "four baselined entries declare a `timeout:`" figure counts declarations,
not reachable entries, and the reachable population is EMPTY.**
`tests/executor.rs::the_baselined_timeout_declarations_are_derived_and_none_can_move_the_ratchet`
derives it on every run and asserts the total beside the parts: **143 baselined
= 4 declaring a `timeout:` + 139 not**; the four are `valids/full`,
`valids/pause-errors-finally-after-execution`, `valids/pause-timeout` and
`valids/pause-timeout-allow-failure`; **three of them declare it on a `Pause`**,
which is a flowable whose `timeout:` is `advance()`'s and which Kestra's own
worker ignores (measured: a `Sequential` carrying `timeout: PT1S` around a
four-second `Sleep` ran to SUCCESS in 4.2 s); and the **one** declaration on a
type this worker has a handler for is `valids/full`'s `t3` — a `core.log.Log`
asking for `PT60M`, a budget it cannot outrun.

The same answer arrives from the other side. `worker::timeout::guard` was
instrumented to print every budget it applied and every one that expired, and
the whole 294-entry oracle was run on that build: the budget was applied
**4** times — `1s`, `1s`, `250ms`, `3600s`, i.e. exactly those four
declarations — and expired **0** times, with
`tests/worker.rs::a_task_level_timeout_reaches_the_handler_and_the_worker_enforces_it`
as the positive control for the instrument on the same build (it printed
`BUDGET-APPLIED 1s`, `BUDGET-EXPIRED 1s`, `BUDGET-APPLIED 30s`). The
instrumentation was removed and the file re-hashed against its pre-instrumentation
copy at the destination, not the source.

Two things that run is also evidence for. First, **a `Pause` IS claimed by this
worker and DOES get a worker budget applied** — `build_plan` plans `Pause`,
`WorkingDirectory`, `WaitFor`/`LoopUntil` and `AllowFailure` as opaque
`TaskKind::Runnable`s, which Kestra's worker never sees. (This sentence named
`Subflow` as a fifth; W6 Task 3 made it a `TaskKind::Executable`, whose
`type_id` is never written, so it is not claimable at all. Corrected by W8 Task
1's sweep of every list enumerating the opaque types — **eight of them: seven
prose lists, all seven wrong, plus the derivation they are checked against**.
The sweep's first census said six, because it certified `worker::timeout`'s
list correct from a transcription that had dropped `Subflow`; that list names
`Subflow` too, and was corrected in fix round 1. The transcription lost the
member because `grep` prints lines and the list **wrapped** — `Subflow` ended
one line and every needle sat on the next.) Nothing reaches the
hazard today (no handler is registered for `Pause`, so `Registry::run` refuses
before any budget can expire), and it is written down in `worker::timeout`'s
module doc rather than special-cased, because a special case there would be a
guard nobody could ever see fail. Second, **this hole is one the ratchet speaks
for in neither direction**, like item 11 but for a different reason — and the
difference is the whole reason both items are here. Item 11's predicate runs on
every tick and *selects nothing*, because the driver's own `pass_time` resumes
each retry inside the call that makes it due. This one's predicate is *evaluated
four times*, once per declaration in the corpus, and no corpus budget is short
enough to fire. Item 14's is *not evaluated at all*, because it is downstream of
this one. Three holes, three mechanisms; *W4 — the ratchet did not move once in
five tasks* above puts them in one table for exactly that reason.

A side effect worth recording because it changes what this repository can
assert: `worker::Outcome::message` is **persisted** now
(`workflow.task_runs.message`, migration 0137), written by `apply_outcome` in
the same fenced statement as `outputs`. W3b's own doc said *"Nothing persists it
yet"*. Nothing in `golden.json` records a failure message, so the ratchet still
cannot compare one — but a test can now read a run's reason back, which is what
made the timeout's boundary assertable at all.

**14. The kill path is wired and this oracle has never reached it — CLOSED by
W4 Task 3, and it is the third mechanism, not a repeat of the other two.**
`TaskHandler::cancel` is called by `cancel::guarded` when a task-level budget
expires, which is the artifact's only `kill(null)` site. No budget in this
corpus expires (item 13), so `Registry::cancel` is reached **zero** times across
all 294 entries, and the predicate is not merely unsatisfied — it is never
evaluated.

**The five ids the spec's number came from are not this path's population**, and
`the_kill_path_cannot_move_the_ratchet_and_the_named_kill_entries_are_not_its_population`
derives that on every run rather than asserting it: the five baselined ids whose
name contains `kill` are `valids/exit-killed`,
`valids/flow-concurrency-parallel-subflow-kill`,
`…-kill-child`, `valids/flow-concurrency-queue-killed-after-execution` and
`valids/pause-kill-delay`, and **not one of them declares a `timeout:`** anywhere
in any of `Flow`'s four task lists — the only thing that reaches this path. What
they are instead, read off their YAML: one `Exit` with `state: KILLED` (an
author-declared terminal), one bare `Pause`, two flows declaring
`concurrency: {behavior: QUEUE, limit: 1}`, and one parent that declares two
`Subflow`s and is named for the concurrency limit on its *child*. Four W8
constructs and one output. That test's second leg is the one that
could actually fail in future: every handler in the driver's own registry must
answer `Ok(())` to a cancel, read from the registry rather than listed, so a
fifth handler that refuses one would be caught here instead of turning a
budget's expiry into a refusal nothing has seen.

The corpus's one *runtime* kill stays where item 10 put it:
`parallel-fail-with-flowable`'s `Sleep`, still in flight when its sibling failed
the `Parallel`. That is `advance()` abandoning a run, and `advance()` has no
worker to tell — a different seam from the budget, deliberately not wired to it.

**15. Nothing in this workspace runs this worker, and that bounds every claim
above — CLOSED by W10 Task 2, and the bound that replaces it is narrower rather
than absent.** *(Also stated as the sixth fact in W4's own section, and
deliberately in both places: it was only here when this statement was first
written, fifteenth of seventeen, six hundred lines under the section a reader is
pointed at.)*

**What is true now, measured on this tree at `054ebdf6` (2026-09-15).**
`egret_workflow::runtime::spawn` is the claim loop and the lease reaper, two
`tokio::time::interval` tickers, and `crates/egret-bin/src/main.rs:1733` starts
it. `worker::tick`, `worker::reap`, `worker::start_execution` and
`worker::resume` have **five production call sites across three files**, listed
in the W10 section. **This was the most consequential sentence in this file and
it stayed false for the seventeen commits between `6eb501a5` (W10 Task 2) and
`054ebdf6` (the commit this sweep ran on)**, which is why the W10 section reports
the sweep as five populations rather than as a fix.

**Three things still bound `109`, and none of them is "no deployment runs it":**

1. **`[workflow] worker_enabled` defaults to `false`**, through a named function
   rather than `bool::default()`, and `spawn` returns before spawning anything
   when it is unset. A deployment that has not typed
   `EGRET_WORKFLOW__WORKER_ENABLED=true` behaves exactly as every deployment did
   before W10.
2. **The shipped registry is four handlers** —
   `worker::Handlers::with_core_tasks()`, i.e. `core.log.Log`,
   `core.debug.Return`, `core.execution.Fail` and `core.flow.Sleep` — and it is
   **not** this oracle's registry, which builds the same four itself with
   `Sleep::capped(Duration::ZERO)`. Neither is a superset of the other.
3. **The ratchet is 109 with the wiring and 109 without it**, measured both ways
   (W10 section, *The zero channel*). So `109` is still a statement about
   `advance()` and a driver rather than about a deployment — for a reason that
   **survived** the wiring rather than one the wiring removed, which is the
   difference between this bound and the one it replaces.

**The derivation as it stood is kept below**, unedited, because it is the thing
seven other sites in this file, plus two in `runnable.txt`, were copies of, and
because the
grep in it is the one that was true-and-misleading at the same time: it searched
for `start_execution(`, which does not match
`start_execution_with_trigger(` — the webhook W6 Task 7 shipped. Nothing was
*starting* executions in production, and then the webhook was, and still nothing
*advanced* one. **The load-bearing half of the proposition was always the tick,
not the start.**

---

*As W4 Task 6 left it:*

`worker::tick` has **no production caller**: a grep for `tick(`, `reap(` and
`start_execution(` over every `crates/*/src` finds no call site at all. Four
files reach them, all four on the test side — `oracle-exec/driver.rs`,
`tests/worker.rs`, `tests/executor.rs` and `tests/sql_starrocks_live.rs` — and
**the scope of that sentence is the compiled binary rather than the file**:
`tests/executor.rs` contains no such call in its own text and pulls the driver
in with `#[path = "../oracle-exec/driver.rs"] mod driver;`. In a document whose
subject is figures that are true under an unstated scope, that distinction is
not a footnote. `worker::reap` and `worker::start_execution` are in the same
position. `egret-bin` builds a `WorkflowState` — the flow repo and its refs, so
that flows can be authored, published and read over HTTP — and no claim loop.
So the runner this package built is exercised by exactly two things, this oracle
and this crate's tests, and a deployment runs none of it. **That is not a defect
and it is not this package's to fix**, but it is the reason a reader must not
read `143` as *"the worker is fine in production"*: there is no production of it
yet, and the reaper, the lease fence and the waker have never met an operator's
`working_timeout_s`.

**16. Three divergences the ratchet cannot hold, each measured on the artifact
rather than reasoned about.**

*`egret.sql` does not stop the statement it timed out.* `worker::timeout::guard`
drops the handler's future and reports FAILED with Kestra's own
`Timeout after HH:mm:ss.SSS`; on StarRocks the query goes on running
engine-side, because `execution::connector::starrocks`'s `execute` leaves
`limits.timeout` **unread** — its own doc says so — and the cancellation token
`run_bounded` builds is private to that function. Kestra's SQL task **does**
stop it: `AbstractJdbcQuery` overrides `kill()`, which calls `Statement.cancel()`
then `Statement.close()`. So this is a *measured divergence from the artifact*,
not parity, and it is a **runtime over-acceptance** of the shape item 10
registers — egret leaving work running that Kestra killed — which item 10's
register cannot hold, because that register is derived from the two traces and
no corpus entry names an `egret.*` type at all. It is recorded here instead,
with the fix and its price in `worker::handlers::sql`'s doc: a cancel verb on
`QueryExecPort` (17 implementation sites, 16 of them test fakes), a
`KILL QUERY` path on a second connection in the StarRocks connector, and a live
assertion. A `query_timeout` session variable would bound it more cheaply, and
that connector is **shared with the Data API serving path**, so it is a contract
change with an owner who is not this package.

*A declared output file can still leave the working directory, inside the
container.* `egret.container` refuses a symlink at any component of a declared
path, `lstat` rather than `stat`, so no byte of the **worker host** crosses into
a task's outputs. What remains is a declared name like `etcdir/passwd` where
`etcdir` is a link the *container* made: `docker cp` resolves it container-side
and egret returns the container's own `/etc/passwd`. Kestra returns **no entry
at all** for that leg (its `MatcherFileVisitor` walk simply does not match the
name) and an **empty file** for the leaf leg (a tar symlink entry carries no
content for `Files.copy` to materialise). The two legs differ, an earlier draft
of this gave one answer for both, and neither leg reaches the host — which is
why the residual is accepted rather than fixed.

*A replay of a `Subflow` child never tells the parent; Kestra's does, and at
one ordering that changes the parent flow's own verdict.* W6 Task 4, fix rounds
2-4. Measured on the pinned digest in **four separate compose projects** — fix
rounds 2 and 3 and re-reviews 2 and 3, each on its own project, each torn down
`-v` — with `tests/worker.rs`'s own fixture shape and the successor as the only
variable.

`Execution.childExecution()` copies `this.trigger` verbatim
(`Execution.java:405-431`, the field at `:422`), so the replacement is still
addressed to the parent's `sub` taskrun and Kestra reports its terminal there.
egret's replacement carries no trigger link at all, so **no stamp is ever
delivered, at any ordering** — and that is pinned twice rather than once.
`claim::settle`'s terminal block (`claim.rs:950-960`) fires **both** dispatch
arms on every terminal execution and both return `None` for a replacement:
`subflow::on_child_terminal`'s `trigger_of` zips two NULL trigger columns, and
`spawn::on_execution_terminal`'s `parent_of` zips a set `parent_execution_id`
with a NULL `parent_run_ix`. The first is pinned by the
`(trigger, trigger_run) == (None, None)` assertion in the test named below; the
second by a CHECK — `workflow_executions_loop_run_whole_or_absent` makes a
non-NULL `parent_run_ix` without a `loop_index` unrepresentable — which is the
stronger kind of pin. **That is why the second ordering needs no second
fixture**: egret's answer is invariant by construction, not by timing.

**The size of the divergence depends on one thing: whether the parent is still
alive when the stamp lands.** Rounds 1-2 measured only the first ordering and
generalised from it; the third round changed the one thing the fixture held
fixed — the successor — and measured again.

| successor | parent EXECUTION | `sub` run | runs, by round |
|---|---|---|---|
| `Log` (finishes before the replacement does) | Kestra `SUCCESS` = egret `SUCCESS`, **16 of 16** | Kestra `FAILED`, egret `SUCCESS` | **16** = 2 (r2) + 1 (rr2) + 2 (r3) + 11 (rr3) |
| `Sleep PT8S` (outlives the replay) | **Kestra `FAILED` in 21 of 22**, egret `SUCCESS` in all 22 | Kestra `FAILED`, egret `SUCCESS` | **22** = 2 (rr2) + 2 (r3) + 18 (rr3) |

Eight of re-review 3's eighteen ordering-B runs were fired **concurrently** and
all eight came out `FAILED`, so this is not a narrow timing window. The
replacement's `trigger` was byte-identical to the child's in every run in which
both were dumped.

**Magnitudes are quoted as ranges observed with their run counts, because two
figures written here as properties were falsified by the very next round's
runs.** Nothing below should be cited as a single characteristic value.

* the stamp follows the replacement's own terminal by **8.5-33.3 ms**, over the
  **16** stamps recorded to a decimal (4 in fix round 3, 12 in re-review 3);
  three further stamps recorded in fix round 2 and re-reviews 1-2 (~31, 30.8,
  28.9 ms) fall inside that range. This entry said *"13-33 ms **in every run**"*:
  8.5 ms falsifies the low end, and *"in every run"* was a claim over runs
  nobody had made yet;
* at ordering A it lands **407-1627.6 ms after** the parent's own terminal, over
  **8** observations. This entry quoted 407-1134 ms and called the spread *"about
  three-fold"*; 1627.6 ms exceeds it, and the spread is four-fold over what has
  now been measured;
* at ordering B it lands **17.4-45.9 ms before** it, over **8** observations,
  with the parent still `RUNNING`;
* at ordering B Kestra's `after` then completed `SUCCESS` **6.1-7.8 s after its
  own execution had already been declared `FAILED`**, over **7** observations.

**So this can be a divergence in the FLOW'S OWN VERDICT** — the same class, and
at that ordering the same size, as the Critical this task opened with. The
`sub` run diverges always; the execution diverges whenever a successor outlives
the replay. Earlier drafts of this entry said *"every EXECUTION-level answer
agrees"*; that was true of one fixture and is corrected here.

**One rule, and its consequence depends on the parent's liveness** — report the
replacement's terminal through the copied trigger. **It is not established that
the rule always fires, and an earlier draft of this entry said it was.** That
draft read *"it is **one deterministic rule**, not a race"*, which is stronger
than 22 ordering-B runs support: **1 of the 22 delivered no stamp at all** on a
live parent — the `sub` run stopped at
`["CREATED","RUNNING","RETRIED","SUCCESS"]`, the parent ended `SUCCESS`, and the
replacement had failed 8.4 s before the successor finished — and
`Execution.hasTaskRunJoinable` (`Execution.java:844`) is a live path that
**discards** a subflow result whose taskrun carries fewer histories than the
execution row now holds. That run was not reproduced in 17 further attempts and
is recorded as a **calibrated counterexample, not a falsification**: the honest
statement is that the ordering is robust over 38 runs and the delivery is not
known to be unconditional. **Faithfulness to behaviour** is still the right
frame for what the fidelity rule covers; what it does not do is excuse egret
here.

**What following the rule would cost. The grounds have moved twice, and this is
the current state of them — materially weaker than fix rounds 2-3 registered.**

1. **NOT a relaxed CHECK. Cost 1 does not exist**, and it was the strongest of
   the three grounds the ruling stood on. `worker::replay`'s INSERT
   (`replay.rs:234-260`) sets `parent_execution_id` to the **child** and leaves
   both trigger columns NULL; the child's own row carries
   `(trigger_execution_id, trigger_run_ix)` = the parent execution and its `sub`
   run. So **one hop up `parent_execution_id` from the replacement lands on the
   row holding exactly the address Kestra's copied trigger holds** — the same
   pair by construction, because `childExecution()` copies `this.trigger`
   verbatim rather than deriving a new one. **Deriving the address instead of
   storing it is what makes `workflow_executions_one_link_kind` irrelevant**,
   and `spawned` never moves: `driver::read_trace_inner` reads
   `parent_execution_id`, which such a walk does not touch, so all 41 golden
   `spawned` entries and all sixteen banks are unaffected — which is the
   entirety of what this cost was protecting. The mechanism is already in this
   module (`subflow::chain_holds`, `subflow.rs:959-986`, walks
   `COALESCE(trigger_execution_id, parent_execution_id)` recursively and
   terminates because a row can only name an older row), and so is the call site
   (`claim::settle` already calls `on_child_terminal` on **every** terminal
   execution; today it returns at the first line). Two things such a walk must
   get right, both expressible: it must **stop at a loop link** — one predicate,
   `loop_index IS NULL`, in the same recursive CTE — or a loop iteration's
   terminal would be delivered to an outer `Subflow` run, which is exactly the
   distinction `ExecutableUtils.isSubflow` makes by trigger *type*
   (`ExecutableUtils.java:313`, read at the pinned checkout); and
   `claim.rs:953-957`'s dispatch comment stops being true **as written** — there
   is still no double report, because `parent_of` zips a NULL `parent_run_ix`,
   but the reason moves from the CHECK to the zip.
2. **A weakened guard — and it is ASSERTED, never measured.**
   `subflow::on_child_terminal` returns early on a parent run that is already
   terminal (`subflow.rs:603-609`), and the run **is** terminal in *both*
   orderings before the replacement even exists: the test below asserts
   `["CREATED","RUNNING","RETRIED","SUCCESS"]` on it at that point. Kestra has no
   such guard — `Execution.hasTaskRunJoinable` blocks terminal→non-terminal
   (`:838`) and an outdated-history join (`:844`), and **permits
   terminal→terminal when the history grows** — so following the rule means
   replacing egret's "already terminal ⇒ never again" with Kestra's
   monotonic-history rule. **Nothing in this tree observes this guard.**
   Mutating it so it can never fire (`.is_some_and(|s| s.is_terminal())` →
   `.is_some_and(|_s| false)`) left **53** worker tests, **45** executor tests
   and the ratchet's `294 / 127 / 127 / 0` with a field breakdown byte-identical
   to the clean run. And the two corpus flows its own doc names are **both
   baselined** — `valids/retry-subflow` (`baseline.txt:386`) and
   `valids/subflow-parent-retry` (`:397`) — so the ratchet is blind to them by
   construction and could not have reported a change there either.

   **Its stated grounds are also wrong in one half and redundant in the other.**
   This entry and the test doc both said *"load-bearing for `transmitFailed` and
   `wait: false`"*. The guard's own doc (`subflow.rs:571-602`) names something
   else — a `Subflow` task's own `retry:` starting a SECOND child under one
   `(execution, run_ix)`, which is why 0138's reverse index is deliberately not
   unique — and `wait: false`. `transmitFailed` is not one of its grounds
   anywhere in `src/`. And the `wait: false` half is **redundant except on an
   error path**: `if !sub.wait { return Ok(()) }` sits *after* this guard
   (`subflow.rs:627`), so removing the terminal guard changes a `wait: false`
   run's outcome only where `spec::parse` or `subflow_of` fails on the parent's
   own source — turning a silent `Ok(())` into an error. What is actually
   exposed is the `retry:` half alone.

   **Measuring it is not possible from outside `src/`, which is why it is still
   asserted here.** The guard's firing is not observable through any public
   surface: `on_child_terminal` is `pub(super)`, it returns `Ok(())` on both
   paths, and a caller cannot distinguish "the guard fired" from "the child was
   not a subflow child". Two instruments would settle it, and both change
   `src/`: a temporary counter on the guard, run over the whole oracle and the
   worker suite, which answers *how often it fires and on what*; or a test that
   constructs the exposed shape — two children under one `(execution, run_ix)`
   with the first child's terminal delivered **after** the second attempt has
   moved the run — paired with the mutation above, which answers *whether it is
   load-bearing*. The second is the stronger instrument and the harder one: it
   is not established that egret can produce that delivery order at all, since
   `on_child_terminal` runs synchronously inside the child's own settle, and
   establishing that either way is itself unmeasured work.

**A third cost was claimed in round 2 and is WITHDRAWN in round 3:** *"egret
would need a path that writes to a terminal execution."* It does not exist at
the ordering that matters — the parent is live, the write is ordinary, and
W3b's durability property is not in play. (Kestra performs post-terminal run
writes in *both* orderings anyway: `sub` in the first, `after` in the second.)

**Zero corpus population, derived rather than remembered:** over `corpus/`,
**345 flows, 4 writing `CREATE_NEW_EXECUTION` and none of those declaring a
`Loop`; 25 declaring a `Subflow` across 20 literal `flowId` targets (a 21st
string is an unrendered `{{ }}` template, counted by `driver::target_census`);
intersection with the four = zero.** The ratchet is silent either way, which is
why this is registered rather than baselined.

**Parked, not hidden.** The third derived shape in `tests/executor.rs`'s
`STILL_OVER_ACCEPTED_AT_RUNTIME` — a run Kestra stopped that egret completed —
is exactly this divergence's shape. The moment any corpus flow reaches it, that
test fails as an *unregistered* runtime over-acceptance rather than passing
quietly. That is what makes shipping it defensible.

**Ruled by the controller (fix rounds 2-3), recorded as a decision rather than
an omission — and the footing under that ruling is now weaker than it was when
the ruling was given.** The ruling stood on two grounds: 0138's CHECK and the
empty corpus population. **The CHECK is avoidable** by a walk this module
already contains (cost 1 above), so what is actually left is **zero corpus
population plus one guard whose load-bearingness nobody has measured, whose
stated justification is half redundant and half not what its own doc says**.
Stated plainly so no reader has to re-derive it: this divergence — a divergence
in a flow's own verdict, the class the plan's Global Constraints single out as
*must not ship* — ships on an empty population and an unmeasured guard. It is
asserted as a divergence, never as parity, by
`a_subflow_child_is_replayed_and_egret_cannot_report_it_to_the_parent`, whose
messages carry Kestra's value beside egret's. A register that overstates its own
justification is the same failure mode as one that understates the divergence,
and this entry has now done both.

**Two assertions in this task claimed parity here and were wrong**, one per
round, in nearly the same words (*"which is what Kestra does too — measured"*).
The lesson is worth more than either correction: **a measurement's framing can
be an artifact of the fixture's incidental choices, and the way to find that is
to change one thing the fixture holds fixed and measure again.** Both defects
were found that way, by a reader who asked what the fixture was holding fixed —
not by re-reading the claim. Fix round 4 adds the corollary the first three
rounds paid for separately: **a correction applied where the diff points is not
a correction.** Rounds 1-3 each corrected the sentence they were handed and left
the same claim standing elsewhere — at its worst in a **second register of this
same divergence**, ~1300 lines below, which contradicted this entry and ended
with *"so Task 4's Concern 3 stands"*. Before correcting a claim, find every
place that states it; that section is now a pointer to here rather than a rival
account of the same measurement.

**17. A worker that dies leaves containers and staging directories behind, and
nothing collects them.** Reproduced, not hypothesised: killing three concurrent
gates with `pkill -f 'target/debug/deps/'` left **four** `alpine:3.20` containers
`Exited (0)`, found afterwards by
`docker ps -a --filter label=egret.io/taskrun-id`, each carrying its run id,
lease and attempt on its labels. The labels are the whole of the mitigation.
There is no reaper for them, no `resume:` path, and the staging directory under
the worker's own tree survives the same way. `reap` reclaims the *lease*, which
is the row; nothing reclaims the process's leavings. An operator gets a query
and no collector.

**18. Four constructs W6 shipped reach zero entries, and a green `127` would
look identical if every line of them were wrong** — the schedule trigger with
its cron engine and backfill, the flow trigger, and the webhook trigger with its
router. This is the largest uncovered surface any package in this programme has
added, and it is **four different blindnesses**, not one: the webhook's request
is never sent, a schedule-fired execution hangs off nothing, a flow-fired one
hangs off a column no arm reads, and backfill has no corpus population at all.
The W6 section above derives all of it, names what each construct's own tests do
and do not establish, and names the three refusals those tests rest on — one of
which (`dependsOn`, 20 of 21 corpus declarations) is the only figure in that
half a running program compares. *(W13b Task 5, 2026-09-26: that refusal is
now a routing fact rather than a hole -- the twenty take the `dependsOn` arm
W13b built beside `WindowFreeFlowTrigger`, 18 of them pass its run-time gate,
and the flow-fired execution still hangs off `trigger_execution_id`, so the
zero-reachable-entries count above is unchanged.)* **The webhook's verdict is EXPOSURE NOT
APPROVED, and that approval rests on `[workflow] webhooks_enabled` defaulting to
`false`.**

**19. W10 wired the worker and shipped five execution endpoints, and this
ratchet is 109 with all of it and 109 with none of it** — so everything in this
item is judged by W10's own tests and by nothing here. Item 15 used to say the
worker reached no deployment; it reaches one now, and **that is what makes this
item necessary rather than what makes it unnecessary.** Four things a green
`109` says nothing about, each derived rather than asserted:

- **There are TWO ways a run hangs for ever, they are not the same way, and a
  green `109` is silent about both.** Recorded on `worker::claim::tick` by W10
  Task 5 fix round 2 and **deliberately not fixed**: they are liveness, not
  correctness. *Correctness is fenced* — `apply_outcome`'s `claimed_at = $3`
  fence rejects the stale worker every time, so no wrong value is ever written.
  What no fence does is make the run finish, or make anything **notice** that it
  has not.

  **(a) A publishable type with no handler** — `with_core_tasks` registers
  **4** handlers against **13** `TASK_TYPES` rows every one of which `publish`
  accepts, so **9 of 13 publishable types** produce a run that is claimed, found
  handler-less, and left `RUNNING`. The mechanism is narrower than *"the run
  keeps its lease"*: [`reap`] clears **only** `claimed_at`
  (`SET claimed_at = NULL, updated_at = now()`), the state stays `RUNNING`, the
  attempt stays open, and `claim`'s own predicate
  `state = 'RUNNING' AND claimed_at IS NULL` re-selects exactly that row. It is
  re-claimed, re-missed, re-reaped, for ever. **Nothing counts it**:
  `attempts.push` occurs in one file (`executor/state.rs`, 2 sites) and neither
  is on this path, so the run's `attempts` array is the same length on the
  ten-thousandth cycle as on the first. **One `warn` per cycle is the entire
  signal.** Guarding it symmetrically at publish is not available — that would
  refuse to start every deployment declining to wire `egret.sql`, `core.http` or
  the container tasks, i.e. every deployment. *(W5a Task 6, 2026-09-28:
  `TASK_TYPES` has **15** rows now, and **7 of 15** publishable types hang
  whatever `workflow.container_runner` is -- `egret.sql`, `egret.python`,
  `egret.dataapi.call`, `egret.dataset.refresh`, `egret.report.send`,
  `core.http`, `core.slack`. The four script rows are served when a runner is
  configured and refused at publish when it is not; a draft run of one under
  `none` still reaches this loop. See the W5a section.)*
  *(No-silent-stall, 2026-10-12: **(a) is closed.** Tasks 2–3 refuse a type
  this deployment does not serve at validate, publish and every start
  (`task_type_not_served`), so no new execution of one is created; Task 4
  makes the worker FAIL a run that reaches it anyway -- an execution started
  on an older binary, or under a configuration that served the type -- with
  the run-log line "egret cannot run task type `X` on this deployment: no
  handler is registered for it. This is not an error in the flow.", through
  the normal commit, so `retry:` and `errors:` apply and the execution
  settles. `WorkerError::NotALeaf` fails the same way. The "somewhere to
  record *tried, no handler*" below turned out to be the attempt itself: a
  `FAILED` attempt is the record. An old execution whose stored source
  validation now refuses (an unregistered type, an `AllowFailure`, a
  `WorkingDirectory`) is covered too since Task 4 fix round 1: the worker
  plans a stored execution with `executor::plan_stored`, without validation
  (user ruling: validation gates starts only), so its run reaches one of
  those two arms and fails. Before that round it stopped at
  `WorkerError::Unplannable` in `claim::load` and cycled through the reaper
  exactly as (a) did. `Unplannable` is again only a source that does not
  parse or flatten.)*

  **(b) A handler that OUTRUNS `working_timeout`** — and this is a *slow*
  handler, not a *missing* one. `DEFAULT_WORKING_TIMEOUT` is 3600 s;
  `Sleep::new()` sets `cap: None`, uncapped, and is **one of the four
  registered**, so this is reachable on the shipped registry with a flow anyone
  may publish. `duration: PT2H`: claimed, sleeping, reaped at 1 h, **re-claimed**,
  starts a *fresh* two-hour sleep; when the first handler returns it hits the
  fence with `LeaseLost`. The second is reaped at its own hour, and so on — the
  run never completes and **each cycle costs a new handler**. *(W5a Task 6,
  2026-09-28: **closed for a live worker** by W5a Task 3's lease renewal. While
  any handler runs, the worker renews `claimed_at` every
  `working_timeout / 3` under a fence (`claimed_at = $held`), so `reap` never
  reclaims a live worker's run and a `PT2H` sleep completes once
  (`a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`).
  What remains is by design: a worker that stops renewing -- dead, or cut off
  from Postgres, in which case it abandons the handler after
  `working_timeout` without a renewal -- has its run reclaimed
  (`a_run_whose_worker_stopped_renewing_is_reaped`), and a worker whose fence
  moved records no outcome (`a_worker_that_loses_its_fence_records_no_outcome`).
  The ratchet reads 109 through all of it; see the W5a section.)*

  **The two need different fixes, and that is why they are listed apart**: a
  give-up-after-N rule closes (a) and quietly converts (b) into a **wrongly
  failed** run. `Sleep::capped(cap)` already exists and is the shape of the
  answer for (b); it does nothing for (a), which needs somewhere to record
  *"tried, no handler"* — a schema question, not a patch.

  **The oracle is built around (a) rather than blind to it by accident**: it
  drives 294 entries of flows whose types `driver::registry()` does not all
  implement, and `driver::run`'s loop comments say a worker error is not a
  driver error. With `BATCH == 1` the refusal concerns only the run it names,
  the driver carries on, and the trace still compares. **The ratchet cannot
  score either cause in either direction.**
  *(No-silent-stall Task 4, 2026-10-12: a `NoHandler` or `NotALeaf` run no
  longer stalls the driver -- it is FAILED like any failed task and the trace
  carries that `FAILED`. A worker error the driver still carries on past is
  now an abort that is not one of those two, e.g. `Unplannable`. Measured:
  **no `baseline.txt` line moved.** Task 1 had already brought
  `ENTRIES_LEFT_UNFINISHED` to 0 and Task 3 refuses an unserved type at
  start, so no corpus entry reaches either arm in the driver.)*
- **Kill has no mechanism, and the endpoint is absent for that reason.**
  `RunState::Killed` has **zero** producing sites — no
  `Command::SetRunState { state: RunState::Killed }`, no `SetExecutionState`
  with it — and `advance()` is pure over `(plan, state, now)` and takes no
  operator input, so it cannot grow one without widening a contract this
  programme has held for four packages. A killer holds **no lease**, so
  `persist` cannot clear `claimed_at` and `apply_outcome`'s fence
  (`WHERE id = $2 AND claimed_at = $3`) still matches: the worker would write a
  terminal onto a killed run **afterwards**. And *kill-requested* is
  **unrepresentable** — `executor::RunState` has no `Killing` variant, **0**
  occurrences in `executor/state.rs` against a control of **9** in
  `models/state.rs`, which is the wire model the harvester reads and not the
  state machine. Item 14 says this oracle has never reached the *budget* kill
  path; this says the *operator* kill path does not exist to be reached.
- **An unbounded `Pause` cannot be ended by the resume route.**
  `flowable::resume_paused` is the only thing that moves a run out of `PAUSED`,
  it is pure over `(state, run, now)`, and its `Resume::Never` arm returns
  `Step::Waiting` **before it reads `now`** — so no value of the clock and no
  number of calls to `POST …/executions/{id}/resume` ends such a wait. Seven
  corpus flows are written that way and every one of them *"reached PAUSED and
  stayed there for the full 90s"* on the pinned artifact, because such a pause
  ends only on an explicit resume signal and egret has no command that carries
  one. `an_unbounded_pause_is_not_ended_by_the_resume_route` pins it, so the
  command's arrival breaks a test rather than this paragraph.
- **Over-acceptance on these five endpoints is invisible here by construction.**
  An endpoint that started a flow its caller may not read, or listed executions
  across workspaces, would leave this ratchet at exactly 109 — the oracle calls
  no route, holds no `WsContext` and never evaluates a Cedar decision. That is
  the `chart_id` class of defect, and the only instruments that can see it are
  `tests/executions_routes.rs` and `tests/execution_repo.rs`.

- **"Off" means two different things, and the ratchet has never seen either
  door.** A flow is switched off by the `state` column the management UI writes
  **or** by its own YAML's `disabled: true`, and the same defect — reading the
  column and never parsing the YAML — has now been found on **two** of the three
  start paths. W6's webhook was the first (closed structurally). `24855ba7` is
  the second and the worse one: `worker::subflow::resolve` tests
  `deleted_at IS NULL AND state <> 'disabled'` and **never parses the source**,
  so an operator could write `disabled: true`, watch
  `POST …/flows/{id}/executions` answer **409**, and have a sibling flow run that
  same flow as a `Subflow` target. The fix cannot live in `resolve` — `disabled:`
  is not a column, so no predicate reaches it — and lives in `spawn_one` after
  `spec::parse`, answering `resolve`'s miss **verbatim** so that both spellings
  of "off" give an operator one answer.

  **What belongs in a coverage statement is the instrument, not the fix**:
  **neither of `resolve`'s two stated doors had a test at all.** Deleting
  `state <> 'disabled'` left `--test worker` at **59 passed / 0 failed**;
  deleting `deleted_at IS NULL` as well, still **59/0** — against a workspace
  control that failed **58/1**, which is what says the instrument could have
  spoken. The ratchet is **109** through all of it: no corpus flow declares
  `disabled: true` on a `Subflow` *target*, and a cross-workspace or trashed
  target is not a shape `golden.json` holds. **Three predicates now have one
  test each.**

**Logs are W11 and the trigger loop is W12.** The trigger half of that is
re-measured in the W6 section at this commit and still reads zero uses outside
`crates/egret-workflow/src/trigger/`. *(2026-09-27: both have shipped since --
W12, W13a/b and W11 each in its own section above. The sentence is W10's.)*

### The stale twins this statement retires

W3a's README ended up carrying **two true-at-different-moments figures for one
quantity, with only surrounding prose to say which** — `38 of the 41` in the
header's Task 7 note and `39` in the body's table, both correct on the day they
were written. W3b Task 7 swept for that shape. What it found, and what a reader
must take from here instead:

| where | what it says | why it is not current |
|---|---|---|
| header, Task 6 note | *"41 entries, 142 sub-executions, **33** of the 41 differing on nothing else"* | the field was empty then. **17 match, 8 differ only inside it, 16 on more** (this row said 16/6/19, which was the split before the review's fix round — a stale twin inside the table that retires them; found by W4 Task 6 against `SPAWNED_ENTRIES_THAT_MATCH`) |
| header, Task 7 note | *"the sub-execution hole is **38** of 41, not 33"* | same quantity, a third moment |
| *What Task 6 had to change*, the 41 table | *"33 at Task 6, 38 at Task 7, **39** at the final review"* | same quantity, a fourth moment |
| *What this oracle does not prove*, ordered-pair bullet | *"the kept 1104 … are the whole of what this oracle asserts about serialisation"* | **357** are (354 when this row was written); already corrected once, by 227, and still too wide by 747 |
| same bullet | *"only **877** … are ever asserted"* | 877 is what the comparison *reads*, not what it can report a change to |
| same section, sub-execution bullet | *"`Command::SpawnExecution` is emitted; **nothing executes one until W3b/W6**"* | `worker::spawn` executes it and this oracle drives it. W3b Task 4 annotated both occurrences and deliberately left them for this sweep |
| header, W3b Task 1 note | *"11 of the 150 … agree only because the harness feeds a terminal"* | the feed is gone; all 11 were lost, with five more the feed was hiding |
| `runnable.txt`, W3a Task 6 block | *"**0** sub-executions whose TRACE is compared"* | 35 entries produce one; 126 nested pairs |

Four more, found by the whole-branch review's own sweep, and they are of the
shapes the sweep above did not look for — a number from the neighbouring file, a
pointer into a section that does not carry the thing pointed at, a paragraph
contradicted by a table 23 lines above it, and a word this statement had already
retired being used again in the same branch:

| where | what it says | why it is not current |
|---|---|---|
| *The state vocabulary, measured* | *"`RESTARTED` appears **60** times in execution histories"* | **12**, in `golden.json`. 60 is `traces.json`'s five-round count |
| *W3b Task 1*, the regeneration note | *"Task 1's **172** … are in this file's coverage statement"* | that statement carries no `172`, no `171` and no `ceiling` |
| *What the oracle feeds in*, last paragraph | *"**189** of the **565** fed"* | the table 23 lines above says **575**; 565 is where Task 6 left the bucket |
| *W3b Task 5*, hole 2 | *"still asserted at 877 + 227 = 1104"* | **written in this branch**, and by a task whose own statement had just retired "asserted" for that population: **357** are reportable |
| *What this ratchet does not prove*, item 2 and the pairs table | 87 pairs removed across 7 entries, over 778 produced | **80 across 6, over 771** — corrected in place above, because those two are the coverage statement's own and it does not get to keep a record |

And one number this statement itself published wrong: **630** fed terminals,
where 204 of them are inside sub-executions egret never created and nothing was
supplied to them. It is **426**, derived above from this file's own table
(`575 − 189 = 386`, `55 − 15 = 40`, `386 + 40 = 426`) — **and this statement's
own preamble went on saying 630 for three more commits**, four lines under the
sentence claiming this section outranks every other. The re-review of W4 Task
6's second fix round found it, together with its twin in `runnable.txt`.

**Two sites still read 630 and are correct, so a sixth pass does not need to
touch them**, and the distinction is the whole of the correction: *What the
oracle feeds in* (whose blockquote already opens *"nothing in this section is
current"*) and W3b Task 5's before/after table both count the **gross size of
the two buckets** — 575 `Runnable` + 55 `Loop` — which is 630 and always was.
**630 is wrong only as a count of terminals *fed***, which is what the preamble
called it. A quantity with two readings needs the reading named beside the
number, not the number changed everywhere it appears.

#### The stale twins W4 retires, and the class one correction did not close

W4's five tasks found a stale claims-list apiece — `NOT_YET_ENFORCED` carrying
two false rows, this file carrying four false *"nothing calls
`TaskHandler::cancel`"* sentences and a wrong implementation count,
`worker/commands.rs` carrying four doc comments naming a handler that had been
renamed, and nine lines across the specs and plan in two languages saying W4
builds `egret.python` when the programme spec's §6.4 gives that name to the
notebook `KernelLauncher`. Five separate finds, and each read as though it had
closed the class. **It had not**, and Task 6's sweep is the evidence: the
largest group below is one event's — the whole-branch review's fix round banking
`valids/replay-loop` — whose corrections landed in the tables that were
re-derived and nowhere else.

| where | what it says | why it is not current |
|---|---|---|
| this statement, *The ordered pairs*, last paragraph | *"unchanged and still derived: … the restriction removes **87** pairs across 7 entries … over all **778** pairs"* | **80 across 6, over 771.** Both are asserted by `RESTRICTION_REMOVES` and `EGRET_PAIRS_BEFORE_RESTRICTION`, i.e. by the very test the sentence cites; the word "unchanged" is what nothing checks. **Corrected in place** — this statement does not get to keep a record |
| this statement, *The stale twins*, the Task 6 note row | *"**16** match, **6** differ only inside it, **19** on more"* | **17 / 8 / 16**, from `SPAWNED_ENTRIES_THAT_MATCH` and its siblings. A stale twin inside the table that retires stale twins. **Corrected in place** |
| this statement, item 9, *The corpus did not move* | *"Every derived figure in this statement is identical either side of the tiebreak — 294/144/144/0, 150 matching, 16/6/19, … 87 removed"* | four of the eight listed figures are demonstrably not what this statement carries now — `294/143/143/0`, **151** matching, **17/8/16**, **80** removed — and the count is left at "four" rather than raised to a tidier number, because whether the field breakdown and the residual baseline also moved with `valids/replay-loop` was not re-derived here. **Dated in place**: a claim of the form *"every figure here is identical under X"* ages with the figures, and the tiebreak experiment was **not** re-run |
| this statement, item 3b, the four-row libtest table | `14 passed / 13 failed / 13 ignored` | **15 / 14 / 14** — the suite gained a test in Task 5's fix round 2. Re-measured on this tree for three of the four rows |
| this statement, item 11 | *"14 baselined entries declare a `retry:` … which is derived rather than assumed"* | the **14 is right**; nothing derived it. `the_retry_declarations_are_derived_and_the_waker_reaches_none_of_them` does now, and it also shows the population the paragraph left out: **25** entries declare one, **11 of them matching** |
| this statement, item 11 | *"`pass_time` rewinds `created_at` **before** any deadline can elapse … so the predicate is never satisfied"* | rewinding is what **makes** a deadline elapse. The mechanism is that `pass_time` calls `worker::resume` itself; corrected in place |
| *What this oracle does not prove*, the W3b annotations (2 places) | **354** of the 1104 reportable, the other **750** | **357 / 747**. A correction is a sentence like any other and ages like one |
| *W3b Task 6*, the *What proves what, kept apart* table | *"the state machine reproduces Kestra on **150** of 294 comparable entries"* | **151.** Left in place — it is a task's own record — but it is listed because that table reads as timeless where the paragraphs around it read as dated, and a claim/instrument table is exactly what a reader quotes |
| `runnable.txt`, *the denominators, re-derived* | 150 matching / 144 baselined, `127 + 23` and `139 + 5` | **151 / 143**, `128 + 23` and `138 + 5` |
| `runnable.txt`, *the ordered pairs* | 354 / 750, nested 99 / 128; the flip table's `6 REGRESSIONs`, `40 REGRESSIONs` and `green, 144 mismatching` | **357 / 747**, nested **102 / 125**; **7**, **41**, `green, 143` |
| `runnable.txt`, the W3b *measured now* table | `16 of the 41 MATCH / 6 / 19` | **17 / 8 / 16** |
| `runnable.txt`, *the terminal states this oracle FEEDS* | *"leaves the ratchet green at **144**"* | **143** |
| `runnable.txt`, *entries differing on the happens-before relation ALONE* | *"That is a RULING (see the W3b plan) … and **W4 owns it**"* | W4 **took** it — Task 4 ruled, and the ruling is *one result per transaction*. Three of the four bank under the alternative and were refused for a cost that is not the yield |

**Two more live outside this directory, and they are the same shape one step
further out.** W4's design spec carries a §1.1 headed *"The '20 baselined
entries' was a union of three declaration greps"* — and **§2.1, thirty lines
below it, still said *twelve*.** Correcting a table does not correct the prose
under it. Worse, **the Vietnamese spec had no §1.1 at all**: the correction was
written in one language and the reference copy carried the uncorrected claim with
no marker. Both are fixed at the source, and its kill row is corrected too — it
said the flow-id matches *"declare no runnable task at all"*, and a `Subflow`
**was** planned, opaquely, as a runnable. (Present tense — *"is planned"* — until
W8 Task 1 fix round 3. It was true when W4 wrote it and W6 Task 3 made it false;
a stale twin inside the section that exists to retire stale twins is the
self-referential form of the defect, and the W4 spec's kill row carried the same
clause.)

**The dispositions differ by file, and deliberately.** The rows in *this
statement* are corrected **in place**, because this section is not a record —
it is the thing every other section defers to, and a coverage statement with a
stale figure in it is worse than one that never carried the figure. The
`runnable.txt` rows are **superseded by name**, in a `W4` block that this file's
own convention put there twice already: rewriting a record makes two chances to
disagree instead of one, and those blocks are headed with the task that measured
them.

#### The rule this sweep ends with, and the two it had to discard to get there

**Read this first if you are sweeping.** Three formulations were tried, each
falsified by the pass after it, and only the third survives:

| # | formulation | what falsified it |
|---|---|---|
| 1 | *every figure a test asserts was already right, so look where no test asserts* | `tests/executor.rs` carried a **fourth** copy of the flip table in a doc comment, and the assertion **message** under `(622, 125, 747)` read *"flipping all 750 … at 144"*. A message is not asserted by anything; it is prose that happens to live inside an `assert_eq!` |
| 2 | *a value a test **compares** is safe; prose that merely sits **beside** one is not — being inside a test file buys it nothing* | closer, and still too narrow: **every example in it is adjacent to a derivation**, and the two figures the re-review then found are adjacent to nothing. The `630` at the top of this statement sits beside no test at all, and the `630` in `runnable.txt` sits in a file that runs nothing |
| **3** | **a figure is safe only if a running program compares it. Every other figure — in any file, at any distance from any derivation — is stale until re-derived.** | nothing yet. Checked backwards against every miss this package produced: the thirteen, the fourteenth inside the note retiring them, the two in the flip-table copy, the fourth pass's five, and the re-review's two. It catches all of them |

Formulations 1 and 2 are recorded rather than deleted because **the way they
failed is the finding**. Both are *locality* rules — they ask where a figure
sits. The third is a *provenance* rule: it asks whether anything recomputes the
figure when the code runs. A locality rule cannot be applied to a file nobody
has opened, which is exactly where the last two hid.

#### And the same question for a PHRASE — W8 Task 6 fix round 2

**The rule above is about figures.** A figure is safe when a running program
recomputes it. A *phrase* has no such test, and W8 Task 6 corrected the same
sentence in four files and left it standing in two more. Both misses have a
**nameable reason**, which is what makes them worth a section rather than an
apology — this is the fourteenth *corrected where found, missed elsewhere* round
on this branch and the first where inattention is not the explanation.

| the miss | why the sweep could not see it | what to do instead |
|---|---|---|
| **A phrase that WRAPS A LINE BREAK.** `baseline.txt` still read *"unbankable by any\n# implementation whatsoever"* after two passes over that very file | `grep` is **line-oriented**. The phrase exists in the file and in no single line of it, so every pattern containing both halves returns nothing — and returns nothing *quietly*, which is indistinguishable from being clean | when you qualify a phrase, search for its **shortest distinctive fragment** (`unbankable by any`), never the whole sentence; or read the file with a matcher that spans lines |
| **An ORDINAL, which names a concept and survives the concept's renaming.** `README.md` said *"This is a **sixth** way the oracle is blind, and it belongs **beside the other five**"* ~500 lines above the section demoting it | searching for *"mechanism 6"*, or for the concept's own words, finds neither *"a sixth way"* nor *"beside the other five"* nor *"the other five mechanisms"* — and **every one of them goes false the moment the count moves.** An ordinal is a name with no letters in common with what it names | sweep **ordinals and counts as their own population**, separately from the concept: *sixth, fifth, the other five, three effects, N mechanisms*. Report it as its own N / M |

**The general form**: the "sweep by concept, not by phrase" rule assumes the
concept's words appear. An ordinal is a counter-example — it refers without
naming — and a line break is a counter-example to the tooling rather than to the
rule. A qualification sweep therefore has **four** populations, not one: the
concept, the phrase verbatim (fragment-matched), the ordinals that count it, and
**data that feeds a generator** — `probe.json` is an input to
`harvest.py --rewrite-runnable`, so prose beside a false row does not stop a
regeneration re-emitting it, and a correction in `runnable.txt` is durable only
because it sits below `HANDWRITTEN_SENTINEL`, which that flag preserves.

#### And the same question for a PREDICATE — W8 Task 7

**Formulation 3 is about figures, and the thing it cannot reach is a
predicate.** *A figure is safe only if a running program compares it.* **So is a
predicate — and a predicate has no printed value for a sweep to compare.**
`claim::due_retries`' state gate was wrong for a whole package: it enumerated
`'RETRYING'` and `'RETRIED'` while `advance()` had gained two more producers of
resumable runs, and **no figure moved when it went false.** W6's Critical was
the same class and its own sweep could not have found it, *because it swept
figures and that was a predicate.*

Two things make the class findable, and they are different jobs:

* **To FIND one**: sweep by concept — *"what selects a run to act on"*, *"what
  walks the execution chain"* — and enumerate the whole population with a
  verdict each. Eleven selectors, one wrong. A phrase sweep reaches none of
  them, because a predicate defined by an **omission** shares no text with what
  it omits.
* **To keep one found**: give it the shape `is_terminal` has. A predicate over
  an enum must be **derived** from the enum (`spawn::terminated_count` filters
  `RunState::ALL` by `is_terminal()`), **carried on the data** (`resume_paused`
  reads the run's own `resume_with`, so there is no list of kinds to be missing
  from), or **answered for every member with the key set asserted**
  (`the_waker_names_exactly_the_states_something_will_move`, and
  `neither_paused_nor_restarted_is_terminal` before it). All three make the
  fourteenth variant a **failing test** instead of a silent omission. A hand
  list behind a catch-all makes it a stall.

**And the same asymmetry the figures have applies**: `retry::reopen_due` is the
pure-side twin of the same predicate and the driver *does* reach it, so
narrowing that one fails loudly. One sentence about *"the ratchet cannot see
it"* is wrong about one of the two, which is why they are recorded separately in
`retry.rs` and `claim.rs` rather than together.

#### Five passes, and every one scoped by surface when the predicate is about mechanism

This is the structural finding, and it explains the sequence better than any of
the individual misses. **Five sweeps have now run over this document, and each
one missed what the next found:**

| pass | scope it chose | what the next pass found |
|---|---|---|
| 1 (W3b Task 7) | this README | `runnable.txt`'s copies |
| 2 (W4 Task 6) | this README + `runnable.txt` | a row **inside its own correction table**, and the specs in two languages |
| 3 (the review) | the files the task edited | four surfaces it had not read: `src/`, `tests/`, `driver.rs`, the plan |
| 4 (this task, corrected predicate) | `src/`, `tests/`, `driver.rs` prose | the statement's **own preamble**, and `runnable.txt` nine lines from a figure the same sweep had superseded |
| 5 (the re-review) | those two | *(nothing — and the rule above is what a sixth should use instead of a sixth surface)* |
| 6 (W10 Task 7) | **not a surface** — five *populations* over `git ls-files`: the phrase flattened, the ordinals, the generator's inputs, the other language, the deleted names | *(nothing yet. Pass 6's subject was a **proposition** rather than a figure, which is a class none of the five above was looking for at all)* |

**Pass 6 is the first that did not choose a surface**, and the first whose
subject was a *proposition* rather than a figure — *"nothing wires the worker
into the binary"*, true for three packages and false from W10 Task 2. Its five
populations are in the W10 section with an N/M each, computed over `git ls-files`
so the denominators can be re-derived; an earlier pass's could not be, because
it counted over `.superpowers/`, which is gitignored and growing.

**And pass 6 nearly missed its own subject, to a hazard this very section
already names.** The statement's opening sentence — the one it tells a reader to
start at — asserts the proposition across a **line break**. It was *true* until
W10 Task 2, so passes 1–5 missed nothing by it; what the line break did was keep
it out of the enumeration **this** task was handed, which was built with a
line-oriented grep and named eight sites where there are fifteen. The rule two
sections up — *a wrapped phrase returns nothing quietly* — has been written here
since W8 Task 6 fix round 2, and was still not reached for on the first occasion
it mattered. **A rule in a document is not an instrument**: it is applied where
whoever is sweeping happens to remember it. The mechanical form is the only
durable one — *flatten whitespace before matching, every time* — and pass 6's
population 1 is defined that way rather than as a reminder.

**Every one of them, the reviews included, scoped by surface — and the predicate
is about mechanism.** "Sweep this file" is a question about where somebody
happened to type a number; "is anything recomputing this figure?" is a question
about the number. The first has to be repeated once per file and is never
finished; the second is finished when every figure either has a derivation or is
marked as history. That is why five passes each found the sixth's work, and it
is the argument for the third formulation over any list of places to look.

#### What the last two passes found

Run under formulation 2, over every doc comment and string literal in `src/`,
`tests/` and `oracle-exec/driver.rs`, a **fourth** pass found five, in surfaces
the first three never read; the **fifth** pass — the re-review — then found two
more that formulation 2 could not have caught, because neither sits beside a
derivation:

| where | what it said | what it is |
|---|---|---|
| `tests/executor.rs`, module header | *"**630** of the golden's 840 run terminals were fed"* | **426.** This statement derived 426 and published 630 as a number *it had itself got wrong* — and the retired figure went on standing in the module doc of the file that holds the derivation |
| `tests/executor.rs`, `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED`'s doc | *"these **six** differ inside it"* | **eight** — and the constant `= 8` is the *next line*. The value was compared on every run and was right; the word beside it was not compared by anything |
| `src/worker/claim.rs`, `CREATE_BESIDE_A_CREATED_SIBLING`'s doc | cites `the_guard_w3a_deleted_is_reachable_again_and_what_it_costs` | **no test has ever had that name.** A dangling pointer from product source into this test file, i.e. the "worse shape" in a surface the pointer table did not cover |
| `oracle-exec/driver.rs` and `src/worker/claim.rs`, the `BATCH` tables | *"`executor.rs` at 34/34"* | 35 since this task added a test. Annotated rather than restated: the row's claim is the **143**, and the binary size was never the measurement |
| `tests/worker.rs`, module header | *"the other **707** tests pass"* | one larger now, for the same reason. The claim is *"the only test that can tell the two disciplines apart"*, which no total carries — so the total is **removed** rather than corrected |
| **this statement's own preamble**, four lines under *"where this section and anything above it disagree, this section is right"* | *"the **630** fed terminals, which is history"* | **426**, which this statement's own section derives and which *predates this task entirely*. The fifteenth stale figure, in the paragraph that establishes the statement's authority, on the quantity the same commit was re-litigating in `tests/executor.rs` |
| `runnable.txt`, *the terminal states this oracle FEEDS* | *"630 under W3a's harness — 575 `Runnable` + 55 `Loop`"* | **426.** The sweep opened that section, superseded the `144` **nine lines below this line**, and left it |

Two of the fourth pass's five are this task's own arithmetic ageing by one
within the same commit, which is the cheapest possible demonstration of the
rule: **a figure nobody re-derives goes stale, and a figure that is a by-product
of a claim goes stale fastest.** The `707` row is the only one where the right
answer was to delete the number rather than update it.

**The last two are the ones that killed formulation 2**, and both are `426`,
derived from this file's own table: `575 − 189 = 386`, `55 − 15 = 40`,
`386 + 40 = 426`. Neither sits beside a test. One sits in the paragraph claiming
this section outranks every other, and the other sits nine lines from a figure
this same sweep superseded by name. **Proximity was never the property**; being
recomputed by a running program is.

**The prediction, recorded because it was made before the pass ran and was
half wrong:** more stale figures would turn up, most likely in `tests/worker.rs`
or `driver.rs`, and at least one dangling pointer. Both those files did carry
one — but the two sharpest were in `tests/executor.rs`, the file this task had
already edited twice, and one of them sits one line above the constant that
contradicts it.

**The largest group is one event; the rest is one habit.** Nothing in the list
is a task reporting a number it did not measure — every figure was true when it
was written. What failed is that a bank re-derives the tables it appears in and
nothing else: the neighbouring file, the annotation in an older section, the
paragraph that says *"unchanged"*, the row inside the correction table, the
other language's copy of the correction. **A correction is not a class-closing
event.**

And the one part that did not drift is the part this file already knew how to
do. **Every figure any of the five passes had to fix is one no running program
recomputes, and every figure a test compares was already right** — 143, 151,
17/8/16, 80, 771, 565, 126, 35, 84, 461/1807/751/191/53, the residual baseline's
thirteen rows. **23 corrections — `13 + 1 + 2 + 5 + 2`**: the thirteen this
task's own table retires, the fourteenth inside the note retiring them, the two
in `tests/executor.rs`'s copy of the flip table (a doc comment and an assertion
*message*), the fourth pass's five, and the re-review's two. **Not one of the 23
landed on a value inside an `assert_eq!`.** That is the case for formulation 3, made by the
exceptions rather than by restating it: `RESTRICTION_REMOVES` was correct in the
source and wrong in the prose beside it, in the same commit, for the same
quantity — and `426` was correct in this section and wrong in its own preamble,
across all three of this task's commits.

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
| `every_flow_kestra_published_plans_except_three_named_over_refusals` | same commit → `…_except_two_named_over_refusals`, then W7b Task 7b → `every_flow_kestra_published_plans_except_the_named_refusals` |
| `every_flow_kestra_published_plans_except_two_named_over_refusals` | renamed at W7b Task 7b → `every_flow_kestra_published_plans_except_the_named_refusals`: since W7b Task 4b its table also holds three deliberate `Pause` refusals |
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

**That sentence's scope was this file, and W4 Task 6's fourth sweep pass found
the same names cited in the crate's own source, where the record argument does
not apply.** Four sites, and the disposition splits on a constraint rather than
on a principle:

| site | cites | disposition |
|---|---|---|
| `src/worker/claim.rs`, `CREATE_BESIDE_A_CREATED_SIBLING`'s doc | `the_guard_w3a_deleted_is_reachable_again_and_what_it_costs` — **a name no test has ever had**, and so not even in the thirteen | **fixed**: it is `the_guard_w3a_deleted_did_not_come_back_and_the_reason_is_the_transaction` |
| `tests/executor.rs`, the field-family list | `the_feed_absorbs_a_measured_share_of_the_golden_run_terminals` | **fixed** — marked as deleted at the point of use |
| `src/executor/command.rs`, `SpawnExecution`'s doc | `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran` | **fixed at W6 Task 3**, the commit that opened the directory. It names `every_sub_execution_egret_runs_is_one_kestra_ran` now — a prefix was all one batch could offer; the driver runs them all, so it is a whole-set comparison — and the *"Nothing executes this yet"* sentence beside it, false since W3b Task 4, went with it |
| `src/executor/flowable/loop_.rs`, module doc | `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank` | **fixed at W6 Task 3.** It names `the_sub_executions_are_compared_and_this_is_what_they_cost`, which derives the same 41 entries / 142 sub-executions the doc quotes |
| `src/executor/flowable/loop_.rs`, the `values:` note | `the_caller_can_render_every_expression_except_the_ones_named` | **fixed at W6 Task 3**, and it is the one with **no successor**: it was deleted with W3a's harness and its render tier. The note now says so at the point of use — nothing asserts those two entries' second cause today, and both remain baselined |

The last three rows are `src/executor/`, which stayed byte-unchanged from W3a
until **W6 Task 3** and no longer does. Their successors are named above so that
the record is a lookup rather than a dead end — the `fixed` rows tell a reader
what happened, and a `recorded only` row that did not name its successor would
be a second dangling pointer standing in for the first. *(All three read `fixed`
now; the paragraph is kept as written because the argument below it is why they
waited, and that argument is still the right one.)*

**The reason for not touching them is stronger than "a constraint says so", and
it is worth stating properly: `src/executor/` byte-unchanged is the calibration
of this instrument.** Everything in this file rests on `advance()` being
demonstrably the same function eight packages of oracle work judged; item 12
exists for no other purpose than to report a `git diff --stat` over that
directory. One harmless doc-comment edit turns a **one-command machine check**
into a judgement call — *"the diff is not empty, but only comments changed"* —
and every future package inherits that judgement call rather than the check. Two
dangling pointers cost a reader one lookup; a diff that is no longer empty costs
every later reader the guarantee.

**Named so the defect has an owner rather than a shrug:** they should be fixed
by **the first package that legitimately opens `src/executor/`**, in the same
commit that opens it, because that commit already pays the whole cost. By this
file's own residual-baseline table that is **W8** — `Pause` (12 entries),
`LoopUntil`/`WaitFor` (9), `AllowFailure`/`allowFailure:` (12) and `sla:` (5)
are all its, and the first three are flowables that must be ported into
`advance()`. (`sla:` is the one of the four that did **not** need
`src/executor/` opened for it: W8 Task 6 put the evaluation in
`worker::claim::settle`, where the artifact puts it, precisely so `advance()`'s
`(plan, state, now)` contract did not have to widen to reach a renderer.) (This list said flow `concurrency:` (7); that row was 6 by W6 and
is deleted at W8 Task 1, its state having been misattributed, and `allowFailure`
takes its place in the enumeration because §9 owns it.) Unless **W6**
gets there first through `CREATE_NEW_EXECUTION`'s replay (4), which lives in
`executor/retry.rs`. Whoever it is: the two lines are in `command.rs`'s
`SpawnExecution` doc and `loop_.rs`'s module doc and `values:` note, and this
table is the successor list.

> **DONE — W6 Task 3, `9fb5b0fa`, and not through the replay.** The package that
> opened the directory is W6 and the reason is `Subflow`, not
> `CREATE_NEW_EXECUTION`: `flowable::start` emits `SUBMITTED` for every
> `TaskKind::Runnable`, and **not one of the 24 executed `Subflow` runs in
> `golden.json` carries that state** (derived on every run by
> `no_subflow_run_in_the_golden_passes_through_submitted`). So a `Subflow` left
> planned an opaque `Runnable` — the cheaper answer, a runnable whose handler
> spawns and waits — mismatches every entry the package exists to bank, whatever
> else its handler gets right. It is a `TaskKind::Executable` now.
>
> **All three pointers were fixed in that commit**, which is the condition under
> which they were left, and the three rows above say `fixed` rather than
> `recorded only`. The third had no successor and now says so at the point of
> use rather than only here.
>
> The paragraph above this one is still exactly right about what it cost: item
> 12 is no longer a one-command machine check for this branch, and every later
> package inherits the judgement call. What replaces it is stated in item 12
> itself — the diff is reported per file with what each file's change is for.
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

---

# W6 Task 1 — the corpus in `workflow.flows`, and the publish path that refuses it

> **Every publish figure in this section has been superseded TWICE and the
> section carried no marker for either — annotated at W10 Task 7, 2026-09-15.**
> `LIVE` went 16 → **21** (W8 Task 3, when `Pause` stopped reaching the `Leaf`
> arm) → **248** (W10 Task 6 fix round 1, when `spec::registry::TASK_TYPES`
> gained the four Kestra FQCNs the worker implements);
> `UNKNOWN_TYPE_REFUSED` went 301 → 296 → **68**; `VALIDATE_REFUSED` 19 → **20**;
> the `SUBFLOW_TARGETS_{LIVE,DRAFT_ONLY}` split 2/17 → **18/1**; and the
> `SUBFLOW_LOOKUPS_ON_{PUBLISHED,REFUSED}` pair 18/4 → 19/3 → **47/2**.
> *W13b Task 2 (2026-09-26) moved three again, all for one flow:* `LIVE`
> 248 → **247**, `PARSE_REFUSED` 5 → **6**, and `SUBFLOW_LOOKUPS_ON_PUBLISHED`
> 47 → **45** — `valids/flow-trigger-fire-once-true-flow-listen` no longer
> parses, because `models::Window` refuses `fireOnce` as the pinned artifact
> does (422), and its two `dependsOn` lookups went with it.
> *Three later moves, each recorded in `driver.rs`'s `LIVE` doc (annotated at
> W9c Task 8, 2026-10-08):* `LIVE` 247 → **245** at W5b's final fix round
> (`valids/secrets` and `valids/webhook-secret-key` refused at publish), 245 →
> **247** at W9a Task 11 (the two `core.storage.Write` flows), and 247 →
> **248** at W9c Task 5 (`valids/retry-with-flowable-errors`, once
> `TASK_TYPES` held the six `core.kv` FQCNs); `UNKNOWN_TYPE_REFUSED` went
> 68 → 66 → **65** with the last two.
> **The current value of every one of them is a `const` in
> `oracle-exec/driver.rs` that `driver::census`, `driver::target_census` and
> `driver::lookup_census` compare on every run**, which is where a reader should
> take them from; the numbers below are W6 Task 1's record of what W6 Task 1
> measured and are left as such, because renumbering a task's own record makes
> it a worse record and no truer. **The finding this section reports is the one
> that decayed, not the counts**: *"the product's publish path cannot publish
> this corpus"* was an accurate reading of a defect, and W10 Task 6 found the
> defect — `TASK_TYPES` did not implement programme spec §8 rule 4, so egret
> refused every pasted Kestra flow. One argument below is corrected in place
> rather than dated, because it is an argument and not a measurement: see
> consequence 1.

Until this task `driver.rs` called `start_execution(pool, ws, None, source,
inputs)` and **nothing ever wrote a row to `workflow.flows`**. A `core.subflow`
task names its target by `(namespace, flowId)` and the only lookup that pair can
go through is `FlowRepo::find_by_name`, which reads that table — so egret could
not execute a `Subflow` at all, and nothing else in W6 was measurable until it
could. 25 corpus flows declare one.

`driver::publish_corpus` now files the whole corpus through the product's own
path — `spec::parse` + `serde_json::to_value` + `FlowRepo::create` (that is
`routes::create_flow` minus the HTTP), then `publish::publish` through the real
`WorkflowRefs` (that is `routes::publish_flow` minus the HTTP). Nothing here
INSERTs a row itself.

## The ratchet did not move, and the mechanism is nameable

**143 mismatching before, 143 after; 0 broken; `tests/executor.rs` 35/35 both
times.** The ratchet fails in *both* directions — a mismatch outside
`baseline.txt` is a regression, a baselined id that now matches is unbanked
progress — so "143 → 143" is set equality and not two counts that happen to
agree.

Zero moves is the *expected* answer here rather than a disappointment, and the
reason is structural rather than statistical: **nothing on the execution path
reads `workflow.flows`.** The single reference is `worker::claim`'s
`LEFT JOIN workflow.flows f ON f.id = e.flow_id`, which reads `flows.created_by`
as the run's `principal_id`; the driver still passes `None` for `flow_id`, so
that column is NULL for every execution the ratchet judges and the join
contributes nothing. `handlers::sql` is `principal_id`'s only reader, and
`no_corpus_entry_can_exercise_the_egret_sql_handler` already measures that no
corpus flow reaches it.

That `None` is deliberate and is called out in `publish_corpus`'s own doc:
handing the flow id to `start_execution` would be a *second* widening with a
second population to attribute, bought for a field nothing in this corpus can
see. W6's `Subflow` links its child by its own column rather than by
`parent_execution_id`, and takes the target's id from this table directly.

## The census, and every number in it is asserted

`driver::census` compares each figure against a constant in the same file and
prints the whole table, because a figure nothing compares is stale the moment
the code moves.

| | corpus flows | |
|---|---:|---|
| **live version** | **16** | `publish` ran to `set_live` |
| draft only — unported task type | 301 | `registry::known_type` refused a `Leaf` |
| draft only — egret's own `validate` | 19 | a real disagreement about the flow |
| unparseable — no row at all | 5 | `spec::parse` refused; `namespace`/`flow_id` are `NOT NULL` |
| name-contested — no row at all | 4 | an earlier file already holds the `(namespace, flowId)` |
| | **345** | |

**336 of 345 corpus flows have a draft row, and that is what a `Subflow`
resolves against.**

## The finding: the product's publish path cannot publish this corpus

**`publish` reaches a live version for 16 of 345, and 301 of the 320 refusals
are one rule.** Publish step 4 asks `spec::registry::known_type` for every
`TaskKind::Leaf`, and that table is an allow-list of the eight `egret.*` types
plus `core.http`/`core.slack`/`core.log`. The corpus is Kestra's: every leaf in
it is an `io.kestra.plugin.core.*` or `io.kestra.core.*` FQCN. So the refusals
are, in order of population, `debug.Return` ×114, `log.Log` ×65,
`execution.Fail` ×32, `flow.Sleep` ×19, `flow.Pause` ×18, and twenty more
classes.

**This is not a defect and no fix is proposed here.** Refusing an unported
plugin at publish is exactly what W1 built `TASK_TYPES` for, and it is the same
answer Kestra gives for a class its own image does not carry — 50 of these 345
were refused by Kestra with a 422 `Invalid type:` for precisely that reason. It
is a statement about the *instrument*: the oracle drives flows the product would
not publish, and that asymmetry now has a number on it.

Two consequences the rest of W6 inherits:

1. **A `Subflow` must resolve against the DRAFT row, not a live version — and
   the population that decides this is the TARGETS, not the declarers.**
   Corrected in fix round 1; the first version of this line said "seven of the
   nine remaining `Subflow` declarers", which was wrong in both the number and
   the population. All nine of those declarers have a draft row, so nothing
   about *them* is missed; what a resolver has to reach is the set of names
   `Subflow` tasks point AT, and a declarer that publishes says nothing about
   whether its target did. Measured by `driver::target_census`, asserted
   against `SUBFLOW_TARGETS*` on every run:

   | of the 21 distinct target strings a corpus `core.subflow` names | |
   |---|---:|
   | **literal `(namespace, flowId)` names** | **20** |
   | — held by a corpus flow with a **live version** | **2** |
   | — held by a corpus flow with a draft row only | 17 |
   | — held by no corpus flow at all (`dummy_namespace/dummy_flow`) | 1 |
   | not a name: `{{inputs.namespace}}/{{inputs.flowId}}` | 1 |

   ~~**A resolver that went through `flow_versions` would resolve 2 of 20**
   (`io.kestra.tests/subflow-child` and
   `io.kestra.tests/flow-concurrency-parallel-subflow-kill-child`)~~ — **RETIRED
   at W10 Task 6 fix round 1: it is 18 of 20**, and this sentence is corrected
   rather than dated because it is an *argument* for a design decision and not a
   record of a measurement. The decision — a `Subflow` resolves against the
   draft row — stands on the reason that always carried it: a parent and its
   child must not disagree about which version of the world they are in.
   `worker::subflow`'s own doc and the programme spec in both languages retired
   this argument when the figure moved; **this copy went on making it**, which
   is the third site of the same retraction and the reason population 1 of W10
   Task 7's sweep is defined over the whole repository. It is still true that it
   was not "miss seven of nine". `FlowRepo::find_by_name` reads `workflow.flows`
   and does not look at `live_version_id`, so the lookup the product already uses
   is the one that reaches 19 of the 20. The 21st string is counted apart rather than
   dropped so the denominator is visible: it is `valids/task-flow-dynamic`'s
   dynamic target, read unrendered because that is where `publish` reads it too,
   and it is the recorded `PUBLISHED_BUT_REFUSED` row rather than a name that
   happens to be missing.
2. **The 16 that do publish are not a random 16.** They are exactly the corpus
   flows whose every task is control flow or a `core.subflow` — a `Subflow` is a
   typed `TaskKind`, dispatched in `kind_for` before any `Leaf` arm exists to
   consult the registry, so it never meets the allow-list. All sixteen are
   `Subflow` declarers, and each of them resolved its `(namespace, flowId)`
   through `find_by_name` and froze a `flow_ref`: `flow-concurrency-subflow`,
   `flow-concurrency-parallel-subflow-kill`(`-child`),
   `flow-concurrency-sla-fail-parent`, `loop-with-subflow`, `retry-subflow`,
   `sla-parent-flow`, `subflow-child`, `subflow-inherited-labels-parent`,
   `subflow-nullable-input-parent`, `subflow-parent`, `subflow-parent-no-wait`,
   `subflow-parent-of-failed`, `subflow-parent-retry`, `task-flow`,
   `task-flow-inherited-labels`. That is the two-pass order below working, not
   an assumption about it.

## The 19 `validate` refusals found no new over-refusal

Sixteen are `invalids/` flows Kestra itself refused with a 422. Three are
`valids/`, and all three are already accounted for:

- `valids/invalid-draft-flow` — Kestra refused it too (`422 tasks: must not be
  empty`), and egret's message is the same rule.
- `valids/task-flow-dynamic` and
  `valids/trigger-flow-listener-namespace-condition` — Kestra published both
  with 200. Both are already the *whole* of
  `executor::mod`'s `PUBLISHED_BUT_REFUSED` table, both are W1-owned, and
  `every_flow_kestra_published_plans_except_two_named_over_refusals` already
  sweeps for a third. Neither has a golden entry (`disposition:
  not-standalone`), so neither can move the ratchet.

So: **no over-refusal that was not already recorded.** The `task-flow-dynamic`
row is the one W6 should read twice — egret cannot express a *dynamic* subflow
target at all, and the fix its row names is to `publish::subflow_ref_name`'s
encoding, not to the check.

### How much of a sweep this actually is, bounded

Fix round 1 corrects an overclaim here. The first version called this "the sweep
this task was positioned to make", on the premise that publishing was the first
thing to run `spec::validate` over the corpus. **That premise was stale:**
`build_plan` has called `validate` since W1's fix — `src/executor/plan.rs:642`,
"W1's `validate`, as the third gate" — so every flow Kestra published was
already swept for a validate-level over-refusal by
`every_flow_kestra_published_plans_except_two_named_over_refusals`, and the 19
refusals above **reproduce** that sweep rather than extend it. (The stale
premise came from the task brief, not from this file; recording the correction
here because a decayed fact repeated is the class this programme has now found
in six consecutive packages.)

What publishing genuinely exercises for the first time is **steps 4–7**:
`registry::known_type`, `RefKind` resolution through `WorkflowRefs`,
`insert_version` and `set_live`. And that ran on **16 flows and 18
`RefKind::Subflow` lookups** — counted by `driver::CountingRefs` and asserted
as `SUBFLOW_LOOKUPS_ON_PUBLISHED`, not estimated — because step 4 refuses the
other 301 drafts before their references are read. Four further lookups happen
inside publishes that then fail (`SUBFLOW_LOOKUPS_ON_REFUSED`), on a `Subflow`
that `resolve_all` reached before it met the unported `Leaf` that refused the
flow; they are counted apart, because a lookup on a flow nobody can publish is
not evidence the resolution shipped. **No other `RefKind` is exercised at all**,
and no `core.trigger.flow` `dependsOn` either — none of the 16 declares one.

**Those 16 are self-selected by step 4's own refusal** — they are exactly the
flows with no unported leaf type, i.e. the ones whose every task is control flow
or a `core.subflow`. So a green census here is evidence that `RefKind::Subflow`
resolution works *for those 16*; it is **not** corpus-wide evidence, and nothing
about the other 329 flows' references has been resolved even once. Task 3 should
read it that way.

## Two passes, and the order is `harvest.py`'s rather than the alphabet's

Every draft is created before anything is published. `harvest.py`'s own capture
loop records why, in the comment above its publish loop: ten corpus flows name a
target that sorts later than they do, and it measured the cost on
`valids/loop-with-subflow` — harvested flow-by-flow it failed with one loop
iteration, harvested after everything was published it succeeds with two,
"which is the whole of the 141-vs-142 gap".

The iteration order is `corpus.json`'s **array** order, not the sorted key order
`corpus_json()` hands back, and that is load-bearing. Three `(namespace,
flowId)` pairs are claimed by more than one corpus file —
`io.kestra.tests/switch-impossible` by 2, `/duplicate` by 3, `/empty` by 4 — and
`UNIQUE (workspace_id, namespace, flow_id)` (migration 0132) lets only the first
of each win. Sorted, `invalids/switch-invalid` takes
`io.kestra.tests/switch-impossible` from `valids/switch-impossible`: the invalid
file *parses* perfectly well, so `FlowRepo::create` accepts it and egret's
refusal only arrives at publish, by which time the row is in the table. The
loser is the one member of the three groups that Kestra published and executed
and that has a golden entry.

`driver::name_contest_won_by_the_right_file` is the guard, and it asks the
question the way a `Subflow` will: it calls `FlowRepo::find_by_name` on each
contested pair and asserts the row that comes back is the file
`NAME_CONTESTED` names.

## `RefKind::DataApiEndpoint` and `RefKind::Notebook` are asserted unreachable

`publish_corpus` builds the real `WorkflowRefs` with two `unreachable!()`
doubles for the other seams' ports — the same shape, and the same reason, as
`refs.rs`'s own `UnreachableDataApi`/`UnreachableNotebooks`. Both kinds are
declared only by an `egret.*` task type, `publish` reads a `RefField` only after
`known_type` has matched, and no corpus flow names an `egret.*` type at all
(`no_corpus_entry_can_exercise_the_egret_sql_handler` measures that). A double
answering `Ok(None)` would turn a corpus that had grown one into a silent
`NotFound`, indistinguishable from the 320 refusals beside it.

## Mutations

Three, each applied by a script that asserts its anchor is **unique in the whole
file** and that the file's hash actually changed, each run against the committed
code, each restored with `git checkout --` and confirmed by
`if [ -z "$(git status --porcelain)" ]`.

| mutation | what died, and with what |
|---|---|
| iterate `super::corpus()`'s array order sorted by id | `name_contest_won_by_the_right_file` — *`io.kestra.tests/switch-impossible` must resolve to `valids/switch-impossible`; it resolves to `invalids/switch-invalid`* |
| drop pass 2 — never call `publish::publish` | `census` — `live versions: []`, left `0`, right `16` |
| publish each flow inside pass 1, at the moment its draft is created, instead of in a second pass | `census` — left `11`, right `16` |

The third is the one worth reading, and it is the only one whose number could
not have been predicted from the code. **Five flows lose their target**, because
the target had not been created yet when they were published:
`loop-with-subflow` — `harvest.py`'s own named example, and the flow its
141-vs-142 measurement was taken on — plus `sla-parent-flow`, `subflow-child`,
`subflow-parent-retry` and `flow-concurrency-parallel-subflow-kill-child`. That
is the same defect `harvest.py` hit on Kestra's side, reproduced on egret's, and
it is why the two passes are two.

### Fix round 1's mutations

Three more, same harness — unique anchor asserted against the whole file,
SHA-256 asserted changed, run against committed code, restored with
`git checkout --` and confirmed by `if [ -z "$(git status --porcelain)" ]`.

| mutation | what died |
|---|---|
| drop `Task::error_children()` from `subflow_targets`' walk | **nothing — 35/35 still green.** Recorded rather than reverted; see below |
| weaken the `{{` split so a template falls through into `absent` | `target_census` — *targets no corpus flow holds*, left `2`, right `1` |
| attribute every `Subflow` lookup to the published bucket | `lookup_census` — left `22`, right `18` |

The second was first written as a *deletion* of the `if` block and **failed to
compile** (`type annotations needed for Vec<_>` — `unrendered` was then never
pushed to). A mutation that does not build looks exactly like one where no test
died, so it was rewritten as a predicate no real target can match.

**The first is a finding rather than a dead guard.** `subflow_targets` walks
`Task::children()` *and* `Task::error_children()`; neither `spec::validate`
(`validate.rs:1283`) nor `publish::resolve_task` recurses through
`error_children()` at all — both walk `children()` only. So **a `Subflow`
nested inside a flowable's `errors:` would never have its reference resolved at
publish**: the same blind spot W3b Task 5 found in `worker::claim`'s
`leaf_spec` one seam over, and the same shape as the whole-branch-review
finding that `publish` never walked `flow.triggers`. It is latent — no corpus
flow puts a `core.subflow` inside an `errors:`, which is exactly why the
mutation survives — and it belongs to whoever owns `Subflow` resolution. The
wider arm is kept because this census asks a different question from `publish`
(*what would a resolver have to reach*), and its redundancy is written down
here rather than left for a future reader to mistake for strength.

---

# W6 Task 2 — three kinds of child execution, and the CHECK the banks depend on

Migration `0135` gave `workflow.executions` exactly **one** way to be a child:
`(parent_execution_id, parent_run_ix, loop_index, loop_value)`, all four set or
all four null. That CHECK was right about the thing it was written for — a child
that knows its parent but not its index is not orderable, and the ordering is
the whole of what `spawned` is compared on — and it forbade one more shape by
accident: **a child with a parent and no loop index at all.**

Kestra makes three different things here, and the distinction is measured on the
pinned artifact rather than read off a class:

| kind | `parentId` | `loopRun` | the link Kestra can be found by |
|---|---|---|---|
| loop iteration | set | set | `parentId` |
| `CREATE_NEW_EXECUTION` replay | set | **absent** | `parentId` |
| subflow child | **NULL** | absent | `trigger_execution_id` |

**The replay row.** Over the four `valids/retry-new-execution-*` flows, all
**eight** descendants in `traces.json` carry a `parentId`, a NULL `loopIndex`, a
NULL `loopValue` and a NULL `spawnedBy` (which is `harvest.py`'s name for
`loopRun.taskRunId`). Their golden entries say the same thing from the other
side: every `spawned` element is `{"parent_run": null, "value": null, …}`.

**The subflow row is measured by a visible absence.** `harvest.py` discovers
descendants by following `parentId`; `valids/subflow-parent` is a flow whose only
task is a `Subflow` that Kestra ran to SUCCESS, and its `descendants` is `[]` in
all five rounds. The child exists. Nothing keyed on `parentId` can see it.

## What migration `0138` adds, and what it refuses

Two columns — `trigger_execution_id` and `trigger_run_ix` — and four
constraints. The one that matters to this directory is
**`workflow_executions_one_link_kind`**: a row may carry `parent_execution_id`
or `trigger_execution_id`, never both. `read_trace_inner` builds `spawned` with
`WHERE parent_execution_id = $1`, and **all 41 golden `spawned`-carrying entries
have zero `Subflow` children** — a subflow child linked the other way would
appear in egret's trace and turn W6's intended banks into `spawned` mismatches.
`driver::w6_subflow_link::a_subflow_child_is_invisible_to_the_query_that_builds_spawned`
drives the real `read_trace` over one subflow child and one loop child of the
same parent and asserts exactly that split; the loop child is there because an
empty `spawned` is what the test wants to see, so "no children found" cannot be
the only thing asserted.

The other three: `workflow_executions_loop_run_whole_or_absent` (the loop triple
is whole or absent, and needs a parent when present — 0135's real target, kept),
`workflow_executions_trigger_run_needs_its_execution`, and
`workflow_executions_no_self_link`, which closes a hole 0135 also had: an INSERT
that supplies its own `id` for `parent_execution_id` satisfies the foreign key,
because the row exists by the time the constraint is checked.

`workflow_executions_loop_run_idx` is recreated under the same name with
`NULLS NOT DISTINCT`, so a parent may have **one** replay child and never two —
two would tie under `ORDER BY parent_run_ix, loop_index` and be ordered by
nothing, which is the collision `normalise.py` has to BAIL on. One per parent is
the artifact's own behaviour: of the 710 descendants in `traces.json`, 40 carry
a NULL `loopIndex` and **no parent has more than one of them**, because Kestra
chains a `CREATE_NEW_EXECUTION` retry (root → replay → replay) instead of fanning
it out. The same sweep found **no parent that mixes** a NULL-index child with
loop children, so nothing yet has to decide where a replay sorts among
iterations.

The reverse index, `workflow_executions_trigger_idx`, is deliberately **not**
unique. A `Subflow` task may carry its own `retry:` and each attempt starts a new
child under the same `(execution, run_ix)`: `valids/subflow-parent-retry`
declares two such tasks at `maxAttempts: 3`, and `valids/retry-subflow` puts a
flow-level `retry:` over one. A unique index there would have made the second
attempt unrepresentable.

## What this task deliberately did NOT change, and what it costs W6 Task 4

`read_trace_inner`'s `spawned` query is **unchanged**, including its decode
type. It reads `Vec<(Uuid, i32, String)>` — not `Option`s — which was exactly
right while a child with a parent and no loop index was unrepresentable. It no
longer is: that child is the replay, and the four `retry-new-execution` entries
want `{"parent_run": null, "value": null}` in `spawned`, which this decode cannot
produce and would fail on. **Nothing writes such a row yet, so nothing is broken
today.** Widening it belongs to the commit that starts writing one, so that the
entries it moves are attributable to it. The comment above the query says the
same thing at the site.

*(**W6 Task 4 is that commit.** The decode is `(Uuid, Option<i32>,
Option<String>)` now, and mutation M14 reverting it kills nine tests including
all four entry tests — a `ColumnDecode` on a NULL, which the driver reports as
`Broken` rather than as a mismatch.)*

**Ratchet: 143 → 143, zero moves.** Nothing in this task writes an execution
row, so nothing the ratchet compares could move; a step that changed it would
have been a step nobody asked for.

# W6 Task 3 — `Subflow` executes, and `src/executor/` opens

**Ratchet: 143 → 131.** Twelve entries banked, zero regressions, zero broken,
zero stale lines. Every one declares a `Subflow`; the attributions are in
*Matching, baselined, and what each half is made of* above and one by one in
`baseline.txt`'s header.

## The finding, and why the cheap answer banks nothing

`src/executor/` had been byte-unchanged since W3a across four packages, and
*Pointers that no longer resolve* named the condition for opening it. **W6 is
the package**, and the reason is `Subflow` rather than the
`CREATE_NEW_EXECUTION` replay that section predicted.

`flowable::start` emits **`SUBMITTED`** for every `TaskKind::Runnable`, and it
is the one state in Kestra's record that separates a task the worker ran from
one the executor ran. **Not one of the 24 executed `Subflow` runs in
`golden.json` carries it** — all 24 read `CREATED RUNNING …`, and the one
skipped run reads `CREATED SKIPPED`. So the cheaper answer the plan asked to be
tried first — *a `Subflow` left an opaque `Runnable` whose handler spawns and
waits* — mismatches **every** entry the package exists to bank, whatever else
its handler gets right.

Derived on every run, from both sides, and from a third that is stronger than
either:

- `no_subflow_run_in_the_golden_passes_through_submitted` walks the golden
  (24 executed + 1 skipped, over 22 entries) and asserts the population beside
  the claim, so a walk that found no `Subflow` run cannot pass vacuously;
- `only_a_runnable_passes_through_submitted` asserts egret's side on
  `valids/subflow-parent`;
- `planning_the_loop_removed_forty_spurious_submitted_emissions` reproduces all
  four of Task 5's committed `fed`/`flowable`/`unplanned`/`over_fed` figures
  **only after adding back the 22 runs and the one TYPE W6 removed**, with
  `!over_fed.contains_key("…flow.Subflow")` asserted so the `+ 1` is checked.
  **22 of W5's 114 over-submitted runs were `Subflow`s.**

And two mutations make it a fact about the ratchet rather than about a unit
test: emitting `SUBMITTED` for an `Executable` fails
`only_a_runnable_passes_through_submitted` with
`["CREATED","SUBMITTED","RUNNING","SUCCESS"]`, and mapping `Subflow` back to
`TaskKind::Runnable` — one line in `plan.rs` — returns the ratchet to 143 with
exactly the twelve banked entries reported as regressions.

## The child, and the link

Measured on the pinned digest by running flows against it, not read off the
sibling checkout:

| measurement | flow |
|---|---|
| the child's `parentId` is **null**, its `trigger` is `{"type": "io.kestra.plugin.core.flow.Subflow", "variables": {"executionId", "taskRunId", "taskId", "namespace", "flowId", "flowRevision"}}`, and `parent_id` finds nothing | `io.kestra.tests/subflow-parent` |
| `wait: false` does not wait, and the parent records `{"state":"CREATED","executionId":…}` — no `outputs` key | `egret.probe/probe-nowait-parent` |
| `wait: true` gives `{executionId, state, outputs}` — a successor rendered `eid=… state=SUCCESS o1=hello` | `egret.probe/probe-outputs-parent` |
| `transmitFailed` is applied ON TOP of the child's own state: run and attempt each `CREATED RUNNING FAILED SUCCESS` | `egret.probe/probe-notransmit-parent` |
| an unresolvable target publishes with **200** and fails the RUN, with no child | `egret.probe/probe-unresolvable` |
| a `Subflow` inside a flowable's `errors:` **runs** | `egret.probe/probe-errors-subflow` |

So the link is `trigger_execution_id` / `trigger_run_ix` (migration 0138) and
never `parent_execution_id` — which is what keeps the child invisible to the
query that builds `spawned`, where all 41 golden entries have zero `Subflow`
children. `a_subflow_child_is_linked_by_the_trigger_columns_and_never_by_parent_execution_id`
asserts that query returns empty **first**, before it looks at the trigger
columns at all, so a child linked the wrong way fails the load-bearing assertion
rather than a lookup that quietly returns nothing.

**Both wrong links were mutated.** Setting `parent_execution_id` while leaving
`trigger_run_ix` is refused by the database, by name —
`workflow_executions_trigger_run_needs_its_execution`, Task 2's guard firing
live. Setting `parent_execution_id` alone is **representable** (it is the replay
shape) and is caught by the test:
`the child is 78ff3430-…, and the `spawned` query must not see it`.

## Resolution: the draft row, and the workspace

Against `workflow.flows.source`, not `flow_versions`. ~~W6 Task 1 measured that
only **2 of the 20** literal `Subflow` targets reach a live version, so a
version-based resolver would resolve 2 of 20 and this package would bank
nothing.~~ It is also what `publish`'s own `FlowRepo::find_by_name` reads, and
~~**nothing in `crates/*/src` starts an execution from a version snapshot** —
there is no product route that starts one at all yet.~~

> **Both halves above are RETIRED — corrected at W10 Task 7 fix round 1,
> 2026-09-15, and the decision they argued for is unchanged.**
>
> **The figure**: it is **18 of 20**, not 2 of 20, since W10 Task 6 fix round 1
> put the four core Kestra FQCNs into `spec::registry::TASK_TYPES`
> (`driver::SUBFLOW_TARGETS_LIVE`, asserted every run). A `flow_versions`
> resolver would now reach almost all of them, so *"this package would bank
> nothing"* is no longer a reason for anything. **This is the FOURTH site of one
> retraction**: `worker::subflow`'s doc, the programme spec in both languages
> and this file's own *W6 Task 1* section all retired it, and this paragraph —
> 340 lines under this file's own retraction at *the 21 distinct target strings*
> — went on making it in its own voice. *A file that retracts a claim and then
> repeats it further down has not retracted it.*
>
> **The route count**: *"there is no product route that starts one at all yet"*
> is false — there are **three**, and W10 Task 5 added the third. What is
> **still true**, and is now the stronger statement because there are routes to
> be true of, is that **none of them reads a version snapshot**. Derived at
> `24855ba7` rather than quoted, and `flow_versions` is read in
> `crates/*/src` only by `repo.rs` (insert/list/get) and surfaced by the
> read-only `routes::list_flow_versions`:
>
> | site | what it starts from |
> |---|---|
> | `worker::subflow::resolve` (W6 Task 3) | `SELECT id, source FROM workflow.flows …` — the **draft** |
> | `webhook_routes::handle` (W6 Task 7) | the same table, `SELECT id, source, state` — the **draft** |
> | `routes::start_flow_execution` (W10 Task 5) | `FlowRepo::get` → `flows.source` — the **draft** |
> | `routes::replay_execution` (W10 Task 5) | `executions.source`, the copy **frozen at the original start** — not a version either |
>
> `a_start_runs_the_draft_even_when_a_published_version_exists`
> (`tests/executions_routes.rs`) is what keeps this from going stale: it
> publishes a version, edits the draft underneath it, and asserts the start runs
> the draft. **Before it, no fixture in the tree had a published version at
> all**, so nothing in this repository could tell a draft read from a
> `flow_versions` read — which is why the sentence could be false for a package
> without any test noticing.
>
> The **binding** reason is untouched and was never either of these: a parent
> and its child must not disagree about which version of the world they are in.

**Scoped to `workspace_id`, where `find_by_name` cannot be.** That method's doc
records why (`WorkflowRefPort::workspace_of` carries no caller-workspace
parameter); this caller has it on `ExecutionCtx`. Deleting the term makes
`a_subflow_target_in_another_workspace_does_not_resolve` report
`(1, SUCCESS, None)` where it wants `(0, FAILED, "unable to find flow …")` —
another tenant's flow source read and executed.

## The unresolvable target, and the one shape that diverges

`the_only_unresolvable_subflow_target_in_the_corpus_is_skipped` derives over all
345 corpus flows that `dummy_namespace/dummy_flow` is the only literal target no
corpus flow holds, and that the run naming it is `["CREATED","SKIPPED"]` — so
**the ratchet cannot see this decision at all**, and it was measured on the
artifact instead. egret agrees on all four observable parts: the flow is not
refused, the run fails, the successor never runs, no child is created.

One shape differs and is pinned in `tests/worker.rs` rather than described: the
artifact's run reads `["CREATED","FAILED"]` with an attempt of `["FAILED"]` —
no `RUNNING`, because it never gets as far as building the `SubflowExecution` —
where egret reads `["CREATED","RUNNING","FAILED"]`. `Command::SpawnSubflow`
cannot be emitted without a `StartRun` beside it (`apply` records nothing for
it, so the round would repeat for ever), and no golden entry reaches the
difference.

## Fix round 1 — the guard that had been seen to fail

**The cycle guard above shipped with a hole, and the review reproduced the same
SIGABRT it was written for.** It walked `trigger_execution_id` only. A `Loop`
sub-execution is linked by `parent_execution_id` and its `trigger_execution_id`
is **NULL** — not by accident but by `workflow_executions_one_link_kind`, the
constraint W6 Task 2 added to make carrying both unrepresentable. So the walk
**truncated at every loop boundary**, and a cycle whose every hop crossed one
was invisible to it:

```text
t/cyl-a:  loop(values: ["1"]) -> Subflow t/cyl-b
t/cyl-b:  loop(values: ["1"]) -> Subflow t/cyl-a

fatal runtime error: stack overflow, aborting   (SIGABRT)
```

Reproduced on this branch before the fix, with the guard shipped and enabled.
The control is what made the diagnosis certain: with only **one** side looping
the guard caught it, so the link kind is the whole difference.

**The premise was wrong, not the arithmetic.** It assumed a causal chain is
walkable by one link kind; 0138 guarantees the opposite — **a child is linked by
exactly one of two columns, and which one depends on why it exists.** The walk
now reads `COALESCE(trigger_execution_id, parent_execution_id)`, "whichever link
this row has", and the `COALESCE` is unambiguous *because* of that same CHECK.
A subflow child, a `Loop` iteration and a `CREATE_NEW_EXECUTION` replay are all
one nested `settle` inside the caller's transaction, so all three are one link
of the thing the guard is about.

`tests/worker.rs`'s `a_cycle_whose_every_hop_crosses_a_loop_boundary_stops_too`
is that reproduction kept as a test, and it asserts the chain **by name and in
order** — `cyl-a (loop) → cyl-a (loop) → cyl-b (subflow) → cyl-b (loop)`, four
executions, one refusal — because a bare count would not say the walk crossed
the loop links at all.

### And a backstop, which the earlier argument said could not exist

The first version shipped **no** depth bound, on the argument that *"a bound
large enough to be worth having is not one this recursion can survive"* —
measured, 16 links overflowed. **That argument was made against a guard that
turned out not to cover the reachable case**, and it generalised a measurement
about a bound used as THE guard to one used as a backstop. As a backstop the
number only has to sit between what is real (the corpus's deepest chain is **2
links**) and what kills a stack (**16**). `MAX_SPAWN_CHAIN = 8` is inside both,
and `a_chain_of_subflows_deeper_than_the_bound_is_refused_rather_than_overflowing`
is its positive control — twelve distinct chained flows, so only the bound can
stop them.

### The backstop then failed its own test, and that is the second finding

The bound was first written as a second column in the guard's **own** recursive
CTE. **Mutation M9 — reverting the walk to one link kind — proved that
worthless**: it shared the walk, so breaking the walk broke both, and the
reproduction **aborted the test binary with SIGABRT** rather than failing an
assertion. *A backstop derived from the thing it is backstopping is not a
backstop.*

So the bound moved onto `claim::ExecutionCtx::depth`, a counter carried **down
the call stack** — `0` at every entry point that begins a fresh stack
(`start_execution`, `load_with`), and `caller + 1` at the only two places that
nest a settle inside another (`spawn::spawn_one`, `subflow::spawn_one`). It
counts the frames, which is what a stack cares about, and it is derived from
nothing that can be wrong. Only `subflow::spawn_one` refuses on it: a `Loop`
cannot recurse without bound, so bounding the loop spawner would be a behaviour
change to a construct that cannot reach it.

Re-running M9 after that move is the measurement that matters — an **assertion**
naming the runaway chain, with the process intact:

```text
left:  [… "cyl-b (loop)", "cyl-a (subflow)", "cyl-a (loop)", …]  10 executions
right: ["cyl-a (loop)", "cyl-a (loop)", "cyl-b (subflow)", "cyl-b (loop)"]  4
```

That is what lets the SIGABRT reproduction live in this suite at all: a
regression of the cycle guard now fails an assertion instead of aborting the
test binary and taking every other test in it along.

**What neither closes, in the words the class deserves: `spawn_one` settles the
child inside its caller's transaction, so depth in the data is depth on the
stack.** Every bound here is a number chosen against a stack size this code does
not control. The real fix is to stop settling the child there — a change to how
`tick` finds work — and nothing in this package does it.

> **Fix round 2 correction.** The paragraph above this one — *"it counts the
> frames … derived from nothing that can be wrong … only `subflow::spawn_one`
> refuses on it: a `Loop` cannot recurse without bound"* — is wrong in both
> halves and is corrected below. **The sentence you are reading now is the one
> that survived**, and it was true before that round and after it.

### Fix round 2 — the counter counted one descent, and a `Loop` needs no `Subflow`

**A. The counter was zeroed on every re-entrant load.** `claim::load_with` set
`depth: 0` for every caller, under a comment asserting that a caller that loads
an execution *"is starting a fresh call stack … a terminal callback or
`subflow::complete`, and none of them is inside another `settle`"*. **Both named
callers are inside another `settle`**: `claim::settle` step 4 calls
`spawn::on_execution_terminal` and `subflow::on_child_terminal`, and step 3
reaches `subflow::complete` through `subflow::spawn_all`. So a *completing*
chain reset the counter once per link while the stack climbed —
`0,1,2,0,1,2,…` — and **nine flows filed in one workspace, plain `wait: true`,
aborted the worker with `fatal runtime error: stack overflow`** (eight on
`wait: false`). `MAX_SPAWN_CHAIN` never fired on the reachable shape.

The fix is `claim::Stack`, a two-armed enum every `load`/`load_with` now takes:
`Stack::Fresh` for the three entry points that begin a stack (`apply_outcome`,
`resume`, `input_for`) and `Stack::Nested(caller_depth)` for the three
re-entrant sites. **There is no default**, which is the point: the false
sentence was a `0` that could be written by omission.

`tests/worker.rs`'s
`a_chain_of_subflows_that_completes_is_bounded_by_the_same_counter` is the
control and it exercises the **ascent** — the sibling test cannot, because *its
chain never completes*: it runs straight down through nested `spawn_one`s,
where the `caller + 1` arithmetic was always right.

**B. Nested `Loop`s reach the same recursion with no `Subflow` in the flow, and
this is what the ARTIFACT does about it.** Measured on the pinned digest,
namespace `egret.probe2`, one flow per depth, one-value `Loop`s around a
`Return`:

| nested `Loop`s | Kestra (pinned artifact) | egret |
|---|---|---|
| 2 / 12 | published, executed, `CREATED RUNNING SUCCESS` | runs; depth 12 is 50 % past `MAX_SPAWN_CHAIN` and unrefused |
| 47 | published, executed, **SUCCESS** | **SIGABRT — stack overflow**, one flow file, no `Subflow` |
| 100 | published, executed, **SUCCESS** in 18 s | not reached |
| 498 | published (`200`) | — |
| 499 | **`422`** — `Illegal genericflow source: YAML parsing error: Document nesting depth (1001) exceeds the maximum allowed (1000, from StreamReadConstraints.getMaxNestingDepth())` | — |

**Kestra imposes no semantic bound on `Loop` nesting.** Its only refusal is
Jackson's document-nesting limit inside the YAML reader — two levels per
`Loop`, so 498 publish and 499 does not — and it is a *parse* error, not a rule
about flowables. The 498/499 boundary was bisected; the flow source is written
flow-style so its size is linear in the depth (an indented source hits the
request-size limit at ~352 and returns `413`, which is not a semantic answer).

So *"fix it with the artifact's behaviour"* is not available: the artifact's
behaviour is **accept and run it**, and egret cannot, because `spawn_one`
settles the child inside its caller's transaction. **It is therefore recorded
with an owner rather than patched with a new number.** It predates W6 —
`spawn::on_execution_terminal`'s recursion is W3b (`ad93ef2d`),
`plan::MAX_TASK_DEPTH` is W3a (`6f046e57`), and neither is in W6 Task 3's diff —
and it is raised only because fix round 1 asserted it could not happen. The
owner is **whoever changes `claim::settle` to stop driving a child's first
settle inside its caller's transaction**; until then `MAX_TASK_DEPTH = 48` is
the number on the wrong side of the cliff and it is W3a's constant.

**What the two `Subflow` guards do bound is `Subflow` recursion, and that is
all the text on this branch now claims.**

### A false claim about a defence that does not exist, in two files

`worker::subflow::resolve`'s doc said *"Migration 0138's composite foreign key
would refuse the resulting child row"* for a cross-workspace resolve. **It would
not.** The child carries the PARENT's `workspace_id` and the OTHER workspace's
`flow_id`, so `workflow_executions_trigger_same_workspace` is satisfied, and the
only constraint on `flow_id` is `executions_flow_id_fkey FOREIGN KEY (flow_id)
REFERENCES workflow.flows(id)` — **one column, no workspace half**. The branch's
own mutation proves it: dropping the `workspace_id = $1` predicate makes
`a_subflow_target_in_another_workspace_does_not_resolve` report
`(1, Some("SUCCESS"), None)` — the other tenant's flow read AND executed,
refused by nothing.

**The same sentence was in `tests/worker.rs` as well**, on the very test that is
the defence, and the sweep found it because it was looked for rather than
because it was handed over. Both now say what is true: `workspace_id = $1` is
the only thing standing between an author-typed `(namespace, flowId)` and
another tenant's flow source, and **nothing in the schema is a second line of
defence.**

**Ratchet unmoved: 131 before fix round 1 and 131 after**, `baseline.txt`
byte-unchanged, so the twelve are the same twelve.

## What this task deliberately did NOT change

- **`read_trace_inner`'s decode**, still `(Uuid, i32, String)` — Task 4's, and
  untouched here for the reason Task 2 gave. A `Subflow` child is invisible to
  that query by construction, so nothing this task writes can reach it.
  *(Widened by W6 Task 4, in the commit that starts writing the row it could not
  decode.)*
- **The publish-time half of the `errors:` gap.** `publish::resolve_task` still
  walks `children()` only, so a `Subflow` inside a flowable's `errors:` never
  has its `flow_ref` filled. The execution-time walk in `worker::subflow` does
  go through `error_children()` — and unlike the census's wider walk, **it is
  not redundant**: mutation M2 kills
  `a_subflow_inside_an_errors_list_is_reached_and_spawned` with
  `task "handler" is planned Executable but is not a Subflow of the execution's own source`.
  The gap is written at the point of use rather than only in a report.
- ~~**`allowFailure` / `allowWarning`**~~ — **closed by W8 Task 5.** This read
  *"which `models::Task` does not carry and `RunState` has no `Warning` for.
  `guess_state` implements only the `transmitFailed` half of
  `ExecutableUtils.guessState` and says so; the two
  `task-allow-*-executable-flow` entries are what that costs and both stay
  baselined."* All three halves changed in one commit and both entries bank.

# W6 Task 4 — the `CREATE_NEW_EXECUTION` replay, and the two facts that end a chain

**Ratchet: 131 → 127.** Four entries banked — the whole of the row the residual
table assigned to this task — zero regressions, zero broken, zero stale lines,
and nothing else moved in either direction:

```text
  valids/retry-new-execution-flow-attempts   flow-level retry, maxAttempts: 3
  valids/retry-new-execution-flow-duration   flow-level retry, maxDuration: PT6S / interval: PT2S
  valids/retry-new-execution-task-attempts   the same two policies on the TASK, which is a
  valids/retry-new-execution-task-duration   different Kestra method reading the same metadata
```

Each was right on every field but `spawned`, and each now reproduces the chain
the artifact ran: **three executions**, the first two `RETRIED` with a replay
hanging off `parentId`, the third `FAILED` with none.

## The finding: a caller could not have done it, and the reason is what `advance()` READS

The task's step 1 asked whether the new execution could be spawned by a caller
without changing what `advance()` outputs. It cannot, and the interesting half
of the answer is one level further in: **it could not be done without changing
what `advance()` READS.**

`retry::policy_for` already treated `CREATE_NEW_EXECUTION` as a policy kind, and
`retry::scan` already ended the execution `RETRIED`. What it could not do is
STOP. `next_retry_at`'s arm for that behaviour read two constants:

```rust
// `execution.getMetadata().getAttemptNumber()`, and `getOriginalCreatedDate()`.
// This execution IS the first attempt, so its metadata attempt number is 1 --
// and `Tick`'s origin is the execution's creation, so the origin is `Tick(0)`.
RetryBehavior::CreateNewExecution => (1, Tick(0)),
```

Every word of that comment is true of the **first** execution of a chain and
false of every replay. With `attemptNumber` pinned at 1, `1 >= maxAttempts` is
false at every depth and `-flow-attempts` / `-task-attempts` retry for ever; with
the origin pinned at each execution's own creation, no elapsed time ever
accumulates and `-flow-duration` / `-task-duration` do the same. A caller cannot
repair that after the fact, because **the golden's third execution ends
`["CREATED","RUNNING","FAILED"]` with no `RETRIED`** — and whether `RETRIED` is
emitted is `advance()`'s output.

So the input widens by exactly Kestra's own `ExecutionMetadata` —
`attemptNumber` and `originalCreatedDate`, the two fields both `TaskRun`'s and
`ExecutionService`'s `nextRetryDate(retry, execution)` read — carried on
`ExecutionState::metadata` and stored by migration **0139**. `advance()` is
still pure over `(plan, state, now)`: no I/O, no clock, and one more field on
the snapshot it is handed.

The output changes too, by one existing command. Both arms of
`ExecutorService.java:603-608` build the **same** `ExecutionDelay`,
`.taskRunId(taskRun.getId()).date(nextRetryDate)`, and differ only in its
`delayType` — `RESTART_FAILED_TASK` against `RESTART_FAILED_FLOW`. egret's
`CREATE_NEW_EXECUTION` arm emitted none, under a test asserting `now == Tick(0)`
and the words *"no `ScheduleRetry`: the delay belongs to the replay, not to this
run"*. The artifact says the opposite, and the delay is the whole of what
`maxDuration` is measured against; without it the replay would start the instant
the execution failed.

## `origin_offset_ms` can be derived, and is stored for four other reasons

Kestra stores an absolute `originalCreatedDate`. 0139 stores the same fact
relatively — how many milliseconds of the ORIGINAL execution's clock had already
elapsed when this one was created.

**This section, and 0139's header, said it "cannot be derived". That was false**
and the fix round measured it: the column is exactly the recursive per-hop sum of
`child.created_at − parent.created_at`, to the millisecond
(`offset(second) = 1355 = hop(1355)`, `offset(third) = 2712 = 1355 + 1357`),
exact because a same-transaction `now()` is. What was actually disproved is only
the **two-endpoint** subtraction `own.created_at - original.created_at` computed
on read, and the stronger sentence was generalised from it.

The two-endpoint version fails because of `driver::pass_time`. It makes time pass
by moving an execution's `created_at` **backwards** by the shortfall to its next
deadline; that is its whole intervention. A chain of replays therefore
accumulates simulated time *inside each link*, and a subtraction taken between
the last link and the first cannot see any of it — it would read ~2 s of a chain
that has simulated ~4 s, and the third execution would schedule a fourth. The
per-hop sum can see it; the difference of the endpoints cannot.

The column earns its place on four grounds instead: Kestra stores the same fact,
so storing it is fidelity; `claim::load_with` is on every settle's hot path and
O(1) beats a recursive CTE; it is immune to *any* later rewrite of an ancestor's
`created_at`, which the derivation is only accidentally safe from; and it can be,
and is, CHECK-constrained, which an expression cannot be.

The clock is deliberately **not** shifted by the offset in `claim::load_with`:
every other `Tick` in an execution — `TaskRun::not_before`, `Attempt::started_at`
— is measured from that execution's own `created_at`, and so is `pass_time`'s
shortfall. A shifted `now` would be read unshifted there and the driver would
travel a chain's whole accumulated time on every hop.

## Three refusals in `worker::replay::spawn_due`, each measured

1. **Not before the deadline.** See above.
2. **Not inside a `Loop` sub-execution.** Measured on the pinned digest
   (`egret.probe4/replay-in-loop2`, a one-value `Loop` whose body's only task is
   a `Fail` with `behavior: CREATE_NEW_EXECUTION, maxAttempts: 2`): the
   iteration ends **FAILED**, one attempt, no `RETRIED`, **no child**, and the
   server log says why — `Can't find taskrun with taskrunId
   '2xzDjVNVp77Y88mGqnc45j' on execution '67EsI18j9aAz2j6a8Ewc2a'`. The id is the
   **`Loop` run's**, which lives in the parent execution:
   `markWithTaskRunAs(…, RETRIED, markParents = true)` walks the failing run's
   parent chain and in a loop sub-execution that chain leaves the execution. So
   Kestra cannot replay a loop iteration, and creating one here would be
   over-acceptance the oracle cannot see — no corpus flow puts such a retry
   inside a `Loop`, measured over all 345.
   **The refusal is not the whole of what the artifact does there, and the
   other half was a silent inversion.** With the iteration `RETRIED` and no
   child, `spawn::decide` fell through to `terminated == total` and the loop run
   ended **SUCCESS** — so the whole flow reported SUCCESS where Kestra reports
   FAILED on both the loop run and the root (`rev4.probe/replay-in-loop`,
   measured on both sides in the fix round). Kestra's throw aborts the retry
   marking and fails the ITERATION execution, after which everything is the
   ordinary `transmitFailed` path with a FAILED child; egret reproduces the
   parent's half of that with `spawn::as_the_loop_reads_it`, one substitution at
   the point the parent reads the child's terminal. It is deliberately not
   `terminated_in_error(RETRIED)`, which reaches `terminateLoop`'s
   `state == FAILED ? stateFailure : SUCCESS` ternary and ends the loop SUCCESS
   again. The iteration's OWN terminal is still `RETRIED` where Kestra's is
   `FAILED`: that half needs a field `advance()` does not have, and it is
   asserted as it stands so it cannot drift unrecorded. **The ratchet does not
   move either way** — 127 before and after — because no corpus flow reaches it,
   which is exactly why it had to be fixed rather than baselined.
3. **Not from a nested `settle`.** `interval: PT0S` over a flow that reaches
   `FAILED` inside one settle — an unresolvable `Subflow` target is the
   reachable example — would otherwise recurse `settle → spawn → settle` inside
   a single transaction, which is the recursion `subflow::MAX_SPAWN_CHAIN` was
   measured to abort the process under. The answer is structural rather than
   numeric and it is the artifact's own: its replay comes off the
   **execution-delay loop**, a different loop from the executor cycle
   (`DefaultExecutor.java:508`), one execution per delivery.

## A replay of a `Subflow` child: what fix round 2 measured, and why the account lives in item 16

**This section used to be a second register of that divergence and it is now a
pointer**, because two registers of one measurement drift apart and these two
did: by fix round 3 this one still read *"four answers agree"*, *"the parent's
own state never moves"* and *"every execution-level answer agrees with egret …
**so Task 4's Concern 3 stands**"*, all of which item 16 had by then corrected,
~1300 lines above. **The whole account — the two orderings, the run counts, the
ranges, the costs, the empty corpus population and the controller's ruling —
is in the coverage statement's item 16, and nowhere else.**

What belongs here is the raw trace this section was written from, kept as a
record of **one** fixture on the pinned digest (fix round 2, own compose project
`t4r2kestra`, namespace `t4r2.probe`, egret's own fixture shape, two runs, torn
down `-v`), written because fix round 1 had asserted the opposite of it with the
words *"which is what Kestra does too — measured"*:

```text
parent      SUCCESS   ["CREATED","RUNNING","SUCCESS"]
  run sub   FAILED    ["CREATED","RUNNING","RETRIED","SUCCESS","FAILED"]
  run after SUCCESS
child       RETRIED   trigger.taskRunId = the parent's `sub` run
replacement FAILED    parentId = the child, trigger = BYTE-IDENTICAL to the child's
```

**That trace is one ordering of two, and reading it as the shape is exactly the
mistake this section made.** Its successor is a `Log` that finishes before the
replacement does, so the stamp lands on a parent that has already ended and the
parent EXECUTION agrees with egret. Change the successor to a `Sleep PT8S` — a
legal publishable flow — and Kestra's parent EXECUTION ends **`FAILED`** where
egret answers `SUCCESS`: a divergence in the flow's own verdict, measured 22
times across three compose projects. The sentences this section carried about
sizes, costs and what "every answer" does are superseded by item 16; the
divergence is asserted as one, never as parity, by
`a_subflow_child_is_replayed_and_egret_cannot_report_it_to_the_parent`.

## Two questions W6 Task 3 left open, both measured on the artifact

Task 3 added `Surface::load`'s sticky `inheriting` flag and deliberately left
two neighbours for this task to **measure rather than choose**.

**`root` / `correlationId` walks the whole chain, and that is right.**

> **CORRECTION, W6 Task 8's whole-branch review.** *"The whole chain"* was true
> of the **parent** chain and read as true of the **causal** chain, and the walk
> followed `parent_execution_id` alone — so it stopped dead at a subflow child,
> whose `parent_execution_id` is NULL by `0138`'s `one_link_kind`, and answered
> the child's **own** id. The measurement below is sound and settles the
> **replay** link only; the subflow link was never measured until the review
> forced it. Both are now measured and one walk serves both — see the W6
> section of *The coverage statement*, and `Link::cause` in
> `src/worker/render.rs`. This is the third artefact of this package to state
> the un-measured half as measured, and the only reason it was catchable is that
> the two link kinds were named apart in `0138`.

`egret.probe4/replay-correlation` — a flow-level `CREATE_NEW_EXECUTION` retry
over a `Log` of `{{ labels.system.correlationId }}` and a `Fail`:

```text
root   cid=3nY5p1H9MmhpWnq4hZsVOD eid=3nY5p1H9MmhpWnq4hZsVOD
replay cid=3nY5p1H9MmhpWnq4hZsVOD eid=5cNdx41IqqhyGSExZsuGLu
```

The replay's `correlationId` is the ORIGINAL's while its `execution.id` is its
own. The mechanism agrees: `ExecutionService.replay` copies the parent's labels
(adding `system.replay`) and `Execution.java:213` fills a `correlationId` in only
when one is ABSENT. A sticky `root` would have diverged. No golden entry pairs a
replay with a `correlationId` read, so the ratchet is silent and
`a_replay_inherits_the_correlation_id_of_the_execution_it_replaces` is the
instrument.

**The `levels` / `item` walk crossing a non-loop link is unreachable.** A level
is pushed only for a row that carries its own `(loop_index, loop_value,
parent_run_ix)`, so a replay contributes none; the only row that could cross a
replay link is a replay whose PARENT is a loop iteration — refusal 2 above, the
one the artifact refuses too. So the state the walk is unguarded against is one
no writer can produce, and it is left as it is rather than given a second flag
nothing could make fire.

> **CORRECTION, W6 Task 8.** That held only while the walk followed one column.
> Widening `root` to the causal chain makes `loop -> subflow child`
> **reachable**, and the push is now gated on the same `inheriting` flag. It is
> a no-op on every row that existed before — and it is not a flag nothing can
> make fire: **measured**, with the gate removed the child of a `Subflow` inside
> a one-value `Loop` renders `{{ item.value }}` as `LOOPVAL`, its CALLER's
> iteration value, and SUCCEEDS. That is `a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item`'s
> fourth part, killed 3 of 3 runs by mutation `M2`. It also closes the
> asymmetry W6 Task 2's re-review recorded as N-2.

## The classifier, corrected: `ENTRIES_LEFT_UNFINISHED` 63 → 59

`is_stalled` omitted `RETRIED` from its terminal list, while
`RunState::Retried`'s own doc, Kestra's `State.isTerminated()`,
`claim::settle`'s terminal dispatch and `spawn::terminated_count` all called it
terminal. The count's assertion says every member is *a task type this worker
has no handler for*; the four `retry-new-execution-*` entries declare only
`core.execution.Fail`, which **has** one and ran it.

**The four leave as a classifier artefact, not as four executions that started
finishing**: the count would have fallen to 59 with `RETRIED` added and no
replay implemented at all. The bank is a separate measurement in the ratchet.

`the_terminal_list_covers_every_execution_terminal_the_golden_holds` derives the
population instead of remembering it — and corrected the first answer written
for it. `CANCELLED` **does** end three golden executions (`valids/exit-cancelled`,
`valids/flow-concurrency-sla-assert-cancel`, `valids/sla-execution-condition`).
It stayed out of the list while `is_stalled` classified egret's traces and
`RunState` had no `Cancelled` — and the test asserted that rather than assuming
it, with the message *"if `RunState` ever gains the variant, this test fails and
`TERMINAL` has to grow with it"*. **W8 Task 6 gained the variant, that
assertion is what failed, and `TERMINAL` grew.** The guard is now the other
direction: every state a golden execution ends in is one `RunState` can hold and
one the classifier knows.

## Mutations

Fifteen, each built first, each with an anchor verified unique against the whole
file, each asserted to have changed the file on disk, and each restored with
`git checkout -- .` plus a `git status --porcelain` emptiness check.

| # | mutation | died in | killed by |
|---|---|---|---|
| M1 | `state.metadata.attempt` → `1` | `--lib` | `the_execution_metadata_is_what_ends_a_create_new_execution_chain` |
| M2 | `state.metadata.origin` → `Tick(0)` | `--lib` | the same |
| M3 | drop the `ScheduleRetry` from the `CreateNewExecution` arm | `--lib` | `create_new_execution_ends_the_execution_retried` |
| M4 | `spawn_due` ignores the deadline | worker | 5 tests |
| M5 | drop the `ctx.depth > 0` refusal | worker | `a_replay_is_not_started_from_a_nested_settle_and_the_next_resume_starts_it` |
| M6 | drop the `Loop` refusal | worker | `a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level` |
| M7 | drop the "a replay already exists" answer | worker | `a_second_resume_does_not_start_a_second_replay` |
| M8 | `due_retries` drops its `NOT EXISTS` term | worker | 3 tests |
| M9 | `due_retries` drops `e.parent_run_ix IS NULL` | worker | `a_replay_is_refused_inside_a_loop_…` |
| M10 | `due_retries` drops the whole `RETRIED` arm | worker | 5 tests |
| M11 | the offset stops accumulating | worker | `a_max_duration_chain_…`, `a_replay_chain_carries_…` |
| M12 | `attempt + 1` → `attempt.max(2)` | worker | `a_replay_chain_carries_its_attempt_number_and_its_origin_offset` |
| M13 | `TERMINAL` without `RETRIED` | executor | 3 tests |
| M14 | `read_trace_inner`'s decode back to `(Uuid, i32, String)` | executor | 9 tests, including all four entry tests |
| M15 | drop migration 0139's metadata CHECK | worker | `the_replay_metadata_is_unrepresentable_on_anything_but_a_replay` |

**M12 SURVIVED the first round and the test grew a link because of it.**
`attempt.max(2)` writes the right number for the first replay and the wrong one
for every later replay, and every fast test ended its chain AT the first replay.
Its only killer was the ratchet — minutes later, as the driver's step bound,
which is what an unbounded chain looks like from there. `a_replay_chain_carries_…`
now runs three links and asserts the increment twice.

**M14 did not compile on its first form** (the tuple alone), which is the shape
this programme records as indistinguishable from a mutation nothing killed. It
was rewritten to revert the two call sites with it, compiled, and then died in
nine tests.

**M1, M2 and M16 make the chain unbounded**, so the executor suite takes the
driver to `STEP_BOUND` on four entries and runs for over ten minutes. They are
reported as killed by the fast suites, which is where they died; the ratchet's
opinion of them was not waited for. `M16` (`Tick(-offset)` → `Tick(offset)`) is
killed by `a_max_duration_chain_stops_when_the_original_clock_runs_out`, which
exists because the `maxAttempts` chain cannot see the origin at all.

## What this task deliberately did NOT change

- **`ORDER BY parent_run_ix, loop_index`** in the `spawned` query. Postgres sorts
  NULLs last, so a replay sorts after every loop child of the same parent — and
  nothing can observe it. Over the 710 descendants in `traces.json`, 40 carry a
  NULL `loopIndex`, no parent carries two, and **no parent mixes** one with a
  loop child; on egret's side the same holds by construction, because refusal 2
  above means a loop parent never has a replay child at all.
- **The `RETRIED`-vs-`FAILED` divergence inside a loop sub-execution.** egret's
  `advance()` still marks such a run `RETRIED` where the artifact throws and
  ends the iteration `FAILED`, because `advance()` cannot see which link kind
  carried the execution it is driving. It predates this task, no golden entry
  reaches it, and what this task refuses is the part that would have been
  over-acceptance: the child execution.
- **The `system.replay` label.** The artifact adds one; egret's executions carry
  no labels of their own beyond the flow's, so there is nothing to add it to.

---

## What W8 Task 3 had to change, and what it banked

**Ratchet: 127 → 119.** Eight entries banked, `baseline.txt` md5
`a3d887ae72595e61c8bc16e8530f9951` → `2b0506e3d706a4a9c772103f3206e9d0`, and
nothing moved in the other direction: 0 regressions, 0 stale lines, 0 broken.

| banked | what it is |
|---|---|
| `valids/pause-delay` | the base case — `pauseDuration: PT1S`, then a `Return` |
| `valids/pause-timeout` | `timeout: PT1S`, which ends the pause **FAILED** and the successor never exists |
| `valids/pause-kill-delay` | `pauseDuration: PT1M`; named for a kill it does not declare |
| `valids/pause-duration-from-input` | `pauseDuration: "{{inputs.delay}}"` — the render slot |
| `valids/trigger-flow-with-pause` | a pause between two `Log`s |
| `valids/flow-trigger-paused-flow` | a pause and a `Return`, in a namespace a flow trigger watches |
| `valids/flow-concurrency-cancel-pause` | `concurrency: {behavior: CANCEL, limit: 1}` over a flow that is only the pause |
| `valids/flow-concurrency-queue-pause` | the same for `QUEUE` |

**The plan's estimate for this row was seven and the measurement is eight**, and
the extra one is `valids/pause-timeout`. The estimate treated a `timeout:` on a
flowable as W4's — which is what `FlowService.java:574` says for every
non-`RunnableTask` **except `Pause`**, the exception this file has recorded
since W3a Task 7. Reading the exception as the rule cost one entry.

### What `advance()` gained, and what it did not

`build_plan` gives a `Pause` a **`TaskKind::Pause`** instead of an opaque
`TaskKind::Runnable`, and `models::kind_for` a **`TaskKind::Pause(Pause)`**
instead of a `Leaf` — the variant `spec::validate`'s `KESTRA_FLOW_CONTROL` table
has anticipated by name since W1 (*"say, a future `TaskKind::Pause`"*). Three
things follow, and each is load-bearing on its own:

1. **No `SUBMITTED`.** `flowable::start` emits it for a `Runnable` and for
   nothing else, and **no `Pause` run in the golden carries it** — 12 entries,
   12 runs, derived on every run by
   `no_pause_run_in_the_golden_passes_through_submitted`. A `Pause` left opaque
   mismatches all eight of the above on that token alone.
2. **No `type_id`.** `claim::persist` writes one only for a `Runnable`, and the
   claim's own predicate is `WHERE type_id IS NOT NULL`, so the run is not
   claimable at all. Before this, a `Pause` WAS claimed, `Registry::run` refused
   it with `NoHandler`, and the tick aborted — which is why all twelve entries
   stalled with no trace rather than with a wrong one.
3. **`timeout:` is `advance()`'s.** `PlanTask::timeout` was `None` for every
   task in the crate and is populated for a `Pause`. That is the artifact's own
   division of labour, not a convenience.

`advance()`'s **contract is unchanged**: still pure over `(plan, state, now)`,
still no I/O and no clock. The `pauseDuration:` expression uses
`TaskRun::rendered`, the slot an `If`, a `Switch` and a `Loop` already use — a
fourth reader of a field that exists, at a run state (`RUNNING`) already in the
table. That was the open question in this task's brief and the answer is that
the slot costs nothing.

### `PAUSED` on the run and not on the attempt

`Command::PauseRun` exists for one measured fact: `PAUSED` is in **20 of the
golden's 840 run histories and 0 of its 878 attempt histories**. `apply`'s rule
1 pushes any non-terminal `SetRunState` onto the open attempt, so a plain
`SetRunState { Paused }` would write a state Kestra never records. W8 Task 2
measured this and left the guard out because nothing emitted the variant; this
task is the producer, and it took the answer that keeps `apply` decision-free —
a command that writes the run's history and deliberately not the attempt's,
rather than a `!= Paused` buried inside `apply`.

The resumed shape is therefore run `CREATED RUNNING PAUSED <terminal>` over
attempt `CREATED RUNNING <terminal>`, one attempt, and the execution
`CREATED RUNNING PAUSED RESTARTED RUNNING <terminal>` — the shape all twelve of
the golden's `RESTARTED` executions have.

### W8 Task 2 predicted two callers. One arrived and one did not

- **`advance()`'s `not_running_yet`** was `history.last() == Created`, narrower
  than Kestra's `isCreated()` (`CREATED || RESTARTED`), with a note saying
  whoever emitted `RESTARTED` must widen it. **True, and done.** Without the
  second term a resumed execution stops at `… PAUSED RESTARTED` and never
  re-enters `RUNNING`.
- **`retry::reopen_due`** carried a note saying the same function must learn to
  reopen a `PAUSED` run with a due `not_before`. **False, and the arm is
  byte-unchanged.** The two shapes part at `RunState::Created`: reopening a
  retry starts a NEW attempt, and a pause that ends must take its terminal on
  the attempt that is still open. Reopening a pause there would give it a second
  attempt and mismatch all twelve resumed entries. `sequential::walk` asks
  `flowable::resume_paused` at the `PAUSED` run instead, which is also where the
  decision belongs. *(This sentence used to add "the terminal depends on which
  bound elapsed, and only the plan knows that". W8 Task 4 fix round 2 falsified
  it: `resume_paused(state, run, now)` takes **no plan**, the bound is resolved
  once when the pause is entered, and the terminal it chose rides on
  `Command::PauseRun`.)*

**`worker::claim::due_retries` is still SQL-gated to `'RETRYING' OR 'RETRIED'`
and still cannot wake a `PAUSED` run.** W8 Task 2 wrote that down as a
prediction; it is a live product defect now, because eight golden entries depend
on a pause ending and every one of them ends through `driver::pass_time` — which
selects on `not_before_ms` with **no state term** — rather than through that
query. It is Task 7's, per the package's own assignment, and the ratchet cannot
see it.

### The four `Pause` entries left, and the rule that keeps them honest

None is held by the wait. Each declares a property `executor::plan::pause_gap`
refuses to plan, so the whole task stays an opaque `TaskKind::Runnable` — the
rule `build_plan` already applies to `AllowFailure` and `WorkingDirectory`,
applied per **task** rather than per **type**. The residual-baseline table above
names all four with what holds each.

`pause_gap` is asked in two places and written once, and that is deliberate:
`spec::validate` gives a `not_yet_enforced` warning for exactly the pauses
`build_plan` leaves opaque. Two copies of the predicate would be a publish that
says a `behavior:` is enforced over an executor that ignores it.

**Correction, 2026-10-11 (W7b Task 4b, the user's ruling): the warning is an
ERROR now.** `spec::validate` reports `unsupported_pause_property` for exactly
those pauses, so `build_plan`'s `validate_for_run` gate refuses the flow and
the three entries are refused at plan rather than planned opaque. The warning
let such a flow publish and start, and in production the opaque run stalled
its execution in `RUNNING` for ever (`NotALeaf`, reaped, reclaimed). They stay
baselined either way; `pause_gap` is still the one predicate.

**`valids/pause-behavior` is the one this task declines to bank on purpose.**
Its golden is the artifact's own defect — see *Drift at the execution seam*:
rendering a Pebble expression into `Property<Behavior>` fails at resume and
FAILS the task for every value, including the ones that would have succeeded.
Reproducing that would be a rule about Java reflection wearing a pause rule's
name. This paragraph added *"and the two terminals a literal `behavior:` can
produce, `WARNING` and `CANCELLED`, are not in `RunState` at all"* — **both are
in it since W8 Task 6 and the entry still does not bank**, which is the measured
correction: what holds it is `pause_gap`, not the vocabulary. Leaving the task
opaque is the answer this file's own rule gives: a visibly wrong trace that
stays baselined beats a plausible wrong one.

### Five corpus flows PUBLISH now, and the ratchet cannot show it either

`flow-concurrency-cancel-pause`, `flow-concurrency-queue-pause`,
`follow-dependencies-partial-parent`, `loop-pause` and `pause_on_pause` were
refused at publish step 4 for an unported task type, because a `Pause` reached
`models::TaskKind::Leaf` and `registry::known_type` has no row for it. A typed
`Pause` is dispatched before any `Leaf` arm exists to consult the registry —
exactly as a `Subflow` is — so all five publish. **The artifact published all
five with a 200**, so this is an over-refusal closing, and it is a divergence
the ratchet structurally cannot see: a flow nobody can publish never reaches
`golden.json`. `driver::publish_corpus`'s census carries the figures —
`LIVE` 16 → 21, `UNKNOWN_TYPE_REFUSED` 301 → 296, and the
`SUBFLOW_LOOKUPS_ON_PUBLISHED`/`_ON_REFUSED` pair 18/4 → 19/3, which must be
read as one measurement because the total is 22 either way.

*(**Superseded at W10 Task 6 fix round 1, 2026-09-15**, and this note is a
pointer rather than a correction: the move above is W8's record of W8's move and
is right about it. The family has moved a third time — `LIVE` **248**,
`UNKNOWN_TYPE_REFUSED` **68**, the lookup pair **47/2**, total **49** — because
`spec::registry::TASK_TYPES` had no row for any Kestra FQCN the worker
implements, so publish refused every pasted Kestra flow. **Three copies of one
family in three sections, each correct on its day**, is exactly the shape W8
Task 7's own sweep reported and the reason a reader must take these figures from
`driver.rs`'s constants, which a running program compares, and not from any
prose here. See *The figures W10 retires*.)*

### A figure this task invalidated without re-measuring it

W4 instrumented `worker::timeout::guard` over all 294 entries and measured the
budget applied **4** times, **three of them `Pause`es**. A `Pause` is not
claimable now, so those three cannot happen on this build and the count is
`valids/full`'s hour-long `core.log.Log` alone. **Nothing re-ran the
instrument**, so that is corrected as a claim rather than restated as a
measurement, in `worker::timeout`'s module doc and beside
`the_baselined_timeout_declarations_are_derived_and_none_can_move_the_ratchet` —
which derives DECLARATIONS (now 3 of 119, two `Pause`es and one `Log`) and never
measured applications at all.

### What W8 Task 3 fix round 2 changed, and the bank it took back

**Ratchet: 118 → 119.** The ninth bank of fix round 1, `valids/pause-behavior`,
is **withdrawn**, and the entry is baselined again. This is the only direction
this file has ever moved a line back, and the reason is worth the space.

**The rule it rested on was measured wrong.** Fix round 1 planned a `Pause`
whose `behavior:` is an EXPRESSION and made it end `FAILED` whichever bound
elapsed, on the strength of the *Drift at the execution seam* section above.
That section's measurement varied the input VALUE of one fixed expression and
never the expression FORM — so it was silent about exactly the class the rule
then generalised over. Re-measured against the same pinned digest, five runs per
form: `FAIL` as a literal, as `{{ 'FAIL' }}`, and as `{{inputs.behavior}}` with
`FAIL` posted each give `FAILED`/1, 5 of 5; the same three with `RESUME`, and
`{{ vars.b }}`, give `SUCCESS`/2. **The artifact honours the value in every
form.** The full table and the one unexplained anomaly are recorded in that
section, where the false claim was made.

So `valids/pause-behavior`'s golden is `behavior: FAIL` working — `FAIL` being
what `corpus.json` posts for it — and egret reproduced it only because its rule
happened to answer `FAILED` for the corpus's single instance. **A bank that
survives because the predicate over-accepts is not a bank**, and the predicate
over-accepted every other expression in the class: `behavior: "{{ 'RESUME' }}"`
succeeds on the artifact and egret guaranteed it would fail. That is a shipped
defect the oracle could not see, because the corpus holds one `Pause`
`behavior:` and it is the one value where the two answers agree.

**`behavior:` is a whole gap again**, for the reason it had before the round
went looking for a narrower one: it is four-valued, and `WARN`/`CANCEL` mean
`WARNING`/`CANCELLED`. This paragraph ended *"the entry becomes bankable when
`RunState` gains `CANCELLED` too"*, and **W8 Task 6 supplied `CANCELLED` and
measured the prediction FALSE** — the ratchet moved 111 → 109 and both ids were
`sla:`'s. `pause_gap` refuses the whole task whenever a `behavior:` is written,
whatever value it holds; a dynamic one cannot even be *classified* at plan time,
where `pause_gap` decides; and the fifth outcome, the non-terminal hang on an
out-of-enum value, is not a state any variant could hold. **The whole
vocabulary arriving moved nothing here**, which is what the half arriving
already said and nobody read that way.

**Kept from fix round 1, all of it re-verified:** the two unreachable
`spec::validate` arms stay deleted, `the_two_control_flow_tables_name_the_same_types`
stays, `parallel`'s rule 3 stays exhaustive, and the `LIVE`/`baseline.txt:204`
record corrections stay. Only the ninth bank and the `dynamic_behavior` rule it
needed are withdrawn.

## Final round

**The whole-branch review returned SHIP with no shipped defect, and four items
that are closed here.** Two of them are the same class — *a claim that a guard
exists, where the guard does not* — and the ratchet is blind to all four, which
is why each is closed by a mutation with a run count and a control rather than
by a paragraph.

| # | what | the mutation | verdict |
|---|---|---|---|
| 1 | `worker::spawn::terminated_in_error` had **no guard** and its doc cited one | **MX1** — drop `RunState::Cancelled`, leaving `Failed \| Killed` | **survived 3/3 before**, 1036 passed, byte-identical to the control; **dead 3/3 after** |
| 2 | the `'PAUSED'` arm had **no execution-terminality term**, and the reason given for not needing one was false | **MX3** — `NOT (e.state = ANY($2))` → `(e.state = ANY($2) OR TRUE)` | **dead 3/3**, and the other two pause tests pass, so the mutation is narrow |
| 3 | a citation naming a test that was never written — **the second in one package** | — | swept mechanically, below |
| 4 | the two `EXECUTION_ASSERTION` banks were **not discriminating** | **MX4** — `is_truthy(&rendered)` → `is_truthy(&rendered) && false`, i.e. violate every assertion unconditionally | **dead 3/3**, **and the ratchet reads 109, green, on the same build** |

### 1 — a wrong assertion, which is the class this package had already paid for

`terminated_in_error` spells `State.Type.isTerminatedInError()` as Kestra's
literal list. Its doc said `state::tests::no_fail_is_a_subset_of_terminal`
*"asserts the two spellings agree, which is what makes keeping both safe."*
**That test never calls the function.** It compares
`is_terminal() && !is_terminated_no_fail()` against a **literal set written in
its own body**, so it pins one spelling to a *third* copy of the list and says
nothing about the one it names — and its own message read *"this is the
assertion that would notice."*

Measured rather than argued: **MX1 survived 3 of 3 runs**, 1036 passed, a result
line byte-identical to the control. `worker::spawn::tests::the_two_spellings_of_terminated_in_error_agree`
calls the function for **every** member of `RunState::ALL` and compares it to
the derived predicate, with a positive control naming the three that must be
true — because thirteen equalities are all satisfied by an implementation
answering `false` to both sides. MX1 now dies 3/3, killed by exactly that test,
reporting `CANCELLED: terminated_in_error=false, is_terminal() && !is_terminated_no_fail()=true`.

**The killing assertion postdates the defect and cannot be evidence the defect
was ever caught.** It was written *because* MX1 showed nothing caught it. The
honest claim is the narrow one: MX1 survived 3/3 before it existed and dies 3/3
after.

**Not shipped today**, and the bound is worth stating rather than leaving to
inference: W8 Task 6's `sub_flow_source` clears `sla:` from a synthesised loop
body, so no iteration of this corpus can end `CANCELLED` and `transmitFailed`
has never been asked about one. The scenario is one package away — give a `Loop`
child a `CANCELLED` terminal and the unguarded version reports **SUCCESS** for a
cancelled iteration, silently.

### 2 — the `'PAUSED'` arm needed a term, and my reason for thinking otherwise was false

The arm shipped with this justification: *`flowable::resume_paused` moves a due
run on the very settle this waker's `resume` produces, so the stale deadline
lands in a state the predicate does not name.* **That is false whenever
`advance()` returns on its first line**, which it does when the **execution** is
terminal:

```rust
if state.history.last().is_some_and(|s| s.is_terminal()) { return Vec::new(); }
```

`RunState::Paused` is **not** terminal, so `claim::sla_terminal` will `CANCEL`
an execution that is still holding a parked bounded `Pause`. That run keeps
`state = 'PAUSED'` and its `not_before_ms`; every tick names it; every tick
relocks the execution and settles it into writing nothing but a new
`executions.seq`. **It is the endless case the `'RETRIED'` arm needed two extra
terms to avoid, reached by a third road**, and the review found it by reading
the guard rather than the prose.

`NOT (e.state = ANY($2))` closes it, and the terminal list is **derived from
`RunState::ALL` by `is_terminal()`** rather than written into the SQL —
`spawn::terminated_count`'s shape, and the only one a fourteenth variant joins
without anybody remembering to. `a_paused_run_in_a_terminal_execution_is_never_named`
drives the same row twice, differing **only** in `executions.state`: `CANCELLED`
must not be named, `RUNNING` must be, and the second is the control without
which the first is satisfied by a broken fixture.

**No producer exists in merged code** — no corpus flow declares both an `sla:`
and a `Pause` — so this is latent, and the row is written by hand for the reason
migration 0142 exists: *the predicate must be safe against a row the schema
admits, not only against rows this executor writes today.*

**Two neighbours, deliberately untouched, and each for its own reason.** The
`'RETRYING'` arm has the **same shape** — an `sla:` `CANCEL` can terminate an
execution holding a `RETRYING` run, `reopen_due` is skipped by the same early
return, the loop is identical — and it is left alone because no corpus flow
declares both `sla:` and a `retry:`, because the arm predates this task by four
packages, and because widening it is a behaviour change to a path W4 measured
and this round did not. The `'RETRIED'` arm is a **third** case and must not be
given this term without measurement: `replay::spawn_due` runs in the settle
*outside* `advance()`, so a replay owed by a cancelled execution may well still
be owed, and nothing here has established which. **Recorded as findings, not
fixed quietly.**

### 3 — a citation is a claim, and this package made the same false one twice

`claim.rs` cited `an_unbounded_pause_is_never_named_however_long_its_execution_lives`
as *"the assertion at this end."* The identifier occurred **once in the whole
tree — in the citation itself**, against **7** occurrences of the positive
control beside it. W8 Task 4 had already done this once
(`every_paused_run_names_its_resumer`, cited by `resume_paused`'s own doc before
it was written). Twice is a class, so the round swept it mechanically.

**The sweep**: every snake_case identifier of three-or-more words appearing in a
doc or line comment anywhere under `crates/egret-workflow`, matched against
every test name defined **anywhere in the repository** (a doc may legitimately
cite a sibling crate's test). Comment runs are **joined before matching**,
because a phrase that wraps a line break is invisible to a line-oriented grep —
the failure this file already records once — and a citation truncated by the
author's own ellipsis is resolved by prefix.

| | count |
|---|---|
| test-shaped identifiers cited in comments | **245** |
| resolving (exactly, or as a truncation of a real name) | **228** |
| flagged | **17** |
| of those, **defined** but missed by the extractor's attribute walk | 4 |
| of those, **deliberate records** of a name that does not exist — *"a test that no longer exists"*, *"the deleted …"*, *"it was …"*, a clippy lint name, one artifact of the line-joiner | 9 |
| **genuinely dangling** | **4** |

The four, and what each now says instead of claiming a guard it does not have:

| citation | site | now |
|---|---|---|
| `an_unbounded_pause_is_never_named_however_long_its_execution_lives` | `worker/claim.rs` | cites the assertion that exists, and records the miss |
| `a_bound_render_never_writes_a_value_into_the_text` | `runners/env.rs` | *the guard is still owed*, with what it would have to assert |
| `a_random_retry_delay_is_inside_kestras_own_bounds` | `executor/plan.rs` | names the test that **does** pin the bounds, `the_exponential_delay_doubles_and_the_random_one_stays_inside_its_bounds` — the old sentence promised a later task an assertion under a name it would not have found |
| `the_literal_in_the_jq_source_is_the_bound` | `runners/jqsandbox.rs`, twice | **nothing keeps `BOUNDED_DEFS`' literal and `MAX_STEPS` from drifting**, which is the hazard the citation claimed to have closed |

**A dangling citation is worse than silence for the same reason a wrong
assertion is**: it converts an unguarded thing into a defended one, and the next
reader spends their budget elsewhere.

### 4 — a bank that was attribution, and is evidence now

Both `EXECUTION_ASSERTION` entries end `["CREATED","CANCELLED"]` with **0 runs**
and they are the corpus's only two declarers, so **"violate every
`EXECUTION_ASSERTION` unconditionally" banks both byte for byte** — and W8 Task
6's attribution mutation cannot separate the two implementations either, because
reverting the feature makes both entries mismatch under either reading.

`corpus.json` records the way out: `valids/sla-execution-condition` declares
`inputs.string` as a `SELECT` over `PASS`/`CANCEL`/`LABEL`, the harvest drove
one of the three, and **two `inputs_alternatives` sit unused** with Kestra's own
test method beside each. `PASS` — the `SELECT`'s default — violates **neither**
assertion and must not cancel; `LABEL` violates the `NONE`-behaviour one, which
changes no state, and so separates *violated* from *cancelled* as `PASS` alone
cannot.

`the_execution_assertion_banks_discriminate_on_the_inputs_the_harvest_did_not_drive`
drives all three through `driver::run`. **Under MX4 it fails, naming `PASS` and
`LABEL` with their traces — and the ratchet on the same build reads 109,
green.** That is the measurement the item is worth: an over-acceptance this
corpus cannot score in either direction, now scored by a running program.

**It is a test and not a bank, and cannot become one.** `harvest.py` posts one
input set per flow, so the artifact's answers for `PASS` and `LABEL` were never
captured; **`baseline.txt` cannot move by one entry here**. The authority for the
two expectations is Kestra's own test *names*, recorded by the harvest in
`inputs_from` — weaker than a captured trace, and said so rather than dressed
up. What a future harvest would change: post the alternatives too, which is the
same *"teach the harness a second verb"* the unbounded-`Pause` row already owes.

### The round banked nothing, beside a number that can disagree

`baseline.txt` is byte-identical: md5 **`c702545ebe209286a02fca5b4b5eea73`**
before and after, id list **`3c290eeab650320a1a9d7b5fb5d92d75`**, ratchet
**109** — and 109 under MX1, MX3 and MX4 as well. **Three of the four items are
invisible to it by construction and the fourth is an over-acceptance it cannot
see**, which is the whole reason this section exists rather than a line in the
per-task table.

The gate after the round: **259 result lines / 3512 passed / 0 failed / 5
ignored**, exit 0, against `35d20de6`'s 259 / 3507 / 0 / 5, reconciled by a
test-NAME-SET diff of **5 added and 0 removed**. `sql_starrocks_live` reports
**8 passed in 0.75 s**, its duration rather than its count.

## Final round, part 2 — closing the family, and a figure I got wrong closing it

**A recorded divergence is not a closed one.** Part 1 put the
execution-terminality term on the `'PAUSED'` arm alone and wrote the identical
`'RETRYING'` case down as a finding. That is *corrected where found, missed
elsewhere* in its **structural** form rather than its documentary one — one
member of a family fixed, its sibling left standing — and this file has recorded
that failure mode fourteen times in its documentary form without once catching
it in this one.

### The term is hoisted, and the exemption is one arm

```sql
AND (r.state = ANY($2) OR NOT (e.state = ANY($2)))
```

One term at the top of the `WHERE`, closing `'RETRYING'`, `'PAUSED'` and both
`'RETRIED'` replay terms together. `$2` stays **derived from `RunState::ALL` by
`is_terminal()`**, so a fourteenth terminal variant joins *both halves* without
anybody remembering to: the guard is unrepresentable-to-forget for whatever arm
comes next, which is the property the per-arm shape did not have.

**It could not be hoisted bare, and the arm that refuses it is named with a
measurement.** `r.state = ANY($2)` exempts an arm whose RUN state is itself
terminal — today exactly `'RETRIED'` — and the exemption is **load-bearing**: a
`CREATE_NEW_EXECUTION` retry ends **this execution** `RETRIED`, so that arm's
entire population lives in a terminal execution by construction. The golden says
so without interpretation — every `valids/retry-new-execution-*` link reads
execution `["CREATED","RUNNING","FAILED","RETRIED"]` beside run
`["CREATED","SUBMITTED","RUNNING","FAILED","RETRIED"]`.

**And the exemption is a sound proxy rather than a coincidence that fits three
arms.** The guard is about consumers *inside* `advance()`, which returns
`Vec::new()` on its first line for a terminal execution: `'RETRYING'` is
consumed by `retry::reopen_due` and `'PAUSED'` by `flowable::resume_paused`,
both inside it; `'RETRIED'` is consumed by `worker::replay::spawn_due`, which
runs in the settle **outside** it. Keying on *"the run's own state is terminal"*
**implies** that — `advance()` moves no terminal run, so an arm whose run state
is terminal must have its consumer elsewhere — so the proxy cannot drift from
the rule it stands for.

### The mutations

| mutation | what it does | runs | verdict |
|---|---|---|---|
| **MX5** | the hoisted term made vacuous (`… OR TRUE`) | 3 | **dead 3/3** — kills `a_terminal_execution_hides_every_arm_except_the_replay` on **both** the `RETRYING`/`CANCELLED` and `PAUSED`/`CANCELLED` rows, and `a_paused_run_in_a_terminal_execution_is_never_named`. Ratchet: **109, green** |
| **MX6** | the naive hoist — the same term without `r.state = ANY($2)` | 3 | **dead 3/3** in the worker suite. Ratchet: **109, green, 3 of 3** |

Control on the unmutated tree: the five relevant tests pass, 5 passed / 0
failed. Each mutation printed out-of-band evidence it landed — the `git diff`
line and a token count separating the SQL from the doc that quotes it — **before
any verdict was read**.

**MX5's killing tests were written for MX5 and are not evidence anything caught
the defect before**, exactly as item 1's were. **MX6's are the opposite and that
is the point**: it kills `a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists`
and `a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level`,
**both W6 Task 4's, both written long before this term existed** — assertions
that *predate what they prove*, and therefore evidence the exemption was not
shaped to fit them.

### The figure I got wrong, and it was this file's own subject

**I wrote — in the doc, in the test, and in the test's own failure message —
that MX6 takes the ratchet from 109 to 113, naming those four entries. It does
not. The ratchet reads 109 under MX6, three runs of three.**

It was a plausible inference published in the voice of a measurement, and the
reason it is false is the thing this whole task is about: the oracle reaches a
due replay through `driver::pass_time` → `resume` → `settle` →
`replay::spawn_due`, and **never through `due_retries`**. A change to the waker
cannot move the ratchet for the replay entries any more than it can for the
pause entries — **the same blindness, one arm further along.** *Item 11's shape
caught me writing about item 11.*

The correction is in all three places, and the honest evidence for the exemption
is MX6's two predating victims rather than a ratchet figure that never existed.
The 3×2's message also said *"the three rows"* flip under MX5; it is **two** —
the `RETRIED` row is exempt under both forms.

### `RETRIED`'s third case is settled: REFUSED, with the measurement behind it

Part 1 left it *unmeasured*, which is the condition under which a defect
survives longest. Measured now, and the disposition is a **refusal**: the
`'RETRIED'` arm must **not** take the guard, because its population is terminal
executions by construction (the golden, above) and because removing its
exemption kills two tests that predate the term (MX6, above). The per-arm shape
is correct **for that one arm**, and the record says *that* rather than *"not
fixed"*.

### Recorded for its owner: `jqsandbox.rs`'s `MAX_STEPS` is unguarded

Found by part 1's citation sweep, outside this task's subject and left for its
owner rather than fixed here. **Nothing compares `BOUNDED_DEFS`' jq-source
literal to `MAX_STEPS`** — the drift the citation
`the_literal_in_the_jq_source_is_the_bound` claimed to have closed, for a test
that does not exist. It is written at **both** sites in
`crates/egret-workflow/src/runners/jqsandbox.rs` — the comment above the
`MAX_STEPS` constant and `bounded_defs`' own doc — so whoever next edits either
one reads it there rather than here.

### This round banks nothing, beside a number that can disagree

`baseline.txt` byte-identical: md5 **`c702545ebe209286a02fca5b4b5eea73`** before
and after, id list **`3c290eeab650320a1a9d7b5fb5d92d75`**, ratchet **109** — and
**109 under MX5 and 109 under MX6 as well**, which is the number that could have
disagreed and did not. Both halves of the hoisted term are invisible to the
oracle; the instruments are `tests/worker.rs`'s, and that is the finding, not an
aside.

The gate after part 2: **259 result lines / 3513 passed / 0 failed / 5
ignored**, exit 0, against `35d20de6`'s 259 / 3507 / 0 / 5, reconciled by a
test-NAME-SET diff of **6 added and 0 removed** over the whole task.
`sql_starrocks_live` reports **8 passed in 0.77 s**.

## no-silent-stall Task 1 — static refusals, and the stalled population is now empty

*(2026-10-12; spec `docs/en/specs/2026-10-12-workflow-no-silent-stall-design.md`
§3.)* Every task `build_plan` plans as an opaque runnable that no handler can
run is a validation **error** now: `unsupported_allow_failure`,
`unsupported_working_directory`, `unsupported_loop_until_property`,
`unsupported_flow_task` (a type spelled like Kestra control flow that egret
does not implement, which was a task-level `not_yet_enforced` warning), and
`unknown_task_type` (a warning until now). `validate_for_run` is
`build_plan`'s third gate, so publish and every start path refuse them. The
flow-level `not_yet_enforced` warnings are unchanged.

**The ratchet: 108 → 107, by one bank and nothing else.**
`invalids/invalid-task` is refused now for Kestra's own reason (an unknown
type) and left `baseline.txt`; no line moved from match to mismatch. The 49
golden `runs` entries Task 1 refuses at start were all baselined already, so
each stays a mismatch for a new reason: refused where Kestra ran it.
`tests/executor.rs`'s `RUNS_REFUSED` names them; `executor`'s
`PUBLISHED_BUT_REFUSED` names all 52 Kestra-published flows with a cause each
(three have no golden entry).

**`ENTRIES_LEFT_UNFINISHED`: 42 → 0.** Every execution the driver used to
leave mid-flight stalled on a task no handler serves — an `OutputValues`,
`Labels`, `SetVariables`, `UnsetVariables`, `Exit`, `Assert` or `PurgeLogs`
leaf, or an opaque `AllowFailure`/`WorkingDirectory` — and each of those is a
static refusal now. On this corpus no execution stalls any more; it is
refused before it starts.

The census of `workflow.flows` moved too: `LIVE` 245 → 237 (the eight
`AllowFailure`/`WorkingDirectory` flows), `VALIDATE_REFUSED` 25 → 33.
`UNKNOWN_TYPE_REFUSED` stays 65, because `driver::publish_class` files the
validator's `unknown_task_type`/`unsupported_flow_task` message under the same
class as the reference resolution's `unknown type '…'`.

The other derived figures that moved, each because a refused entry no longer
produces a trace: the field split (`MISMATCH_ON_EXECUTION` 68 → 26, `_RUNS`
90 → 41, `_ORDER` 53 → 19, `_SPAWNED` 13 → 11; the total of 103 is
unchanged), the spawned-key comparison (41 → 38 entries), the ordered pairs
(836 → 829 before the restriction), the `Loop` populations, and
`handler-surface.txt` (matching 186 → 187, blocked 81 → 80).
`planning_the_loop_removed_forty_spurious_submitted_emissions` reconstructs
an earlier builder, so it reads the 49 from `executor::plan_stored` (named `plan_unvalidated`, test-only, until no-silent-stall Task 4 fix round 1) (the
flattening without the gate, `test-util` only) and its figures do not move.

**The guard that keeps it this way** is
`executor::tests::every_task_planned_opaque_is_refused_at_its_path`: over the
whole corpus, every task the flattening plans as a `Runnable` while its model
kind is not a `Leaf` has a `validate_for_run` error at its path or below. It
judges 6 `AllowFailure`, 13 `WorkingDirectory` and 3 gapped `Pause` tasks,
plus synthetic gapped `LoopUntil`/`WaitFor` under a flowable's `errors:`.
Deleting the `unsupported_working_directory` push fails it naming
`valids/task-runif-workingdirectory`, `-working-directory-invalid-runif`,
`-working-directory-taskrun` and `-taskrun-nested`.

## WorkingDirectory Task 8: six measured flows, the driver's script runtime, and a re-harvest

**What changed in the oracle.** The six `working-directory-real-*` flows are live
corpus entries (`corpus/valids/`), and `driver.rs` now registers the Docker script
runtime and publishes through `publish_gated` with the oracle workspace's
`workflow_containers` ON. Before this, a script task had no handler in the driver and
FAILED by name (no-silent-stall), so every child of those flows was a fact about the
driver. The census moved 345 → 351 corpus flows, 244 → 250 live, `VALIDATE_REFUSED`
stays 26. `harvest.py` accepts a digest pin (`@sha256:`) for `image_tag()`.

**What the re-harvest found.** `traces.json` holds five rounds of the 274 `runs`
flows. Normalised, `golden.json` has 301 entries (273 `runs`, 28 `refused`): the
seven new ones are the six real flows and `retry-failed-task-duration`, which was
unstable at 58d21e5b's harvest and is stable in this one. `working-directory-real-runif-false`
was excluded as unstable until `normalise.py` numbered runs by their CREATED instant
(see below), which is the only normaliser change.

**The mismatches the ratchet reported, and what each was.**

- `working-directory-real-allowfail`: egret's block read FAILED. The cause was the
  driver, not the block: with no script runtime the sibling FAILED, and so did the
  block. With the runtime the block is WARNING, the child `fails` is WARNING with
  attempt FAILED, as Kestra records it. No WorkingDirectory block-state mapping was
  wrong.
- SUBMITTED on a WorkingDirectory child: a real egret difference. Kestra's raw
  captured trace has no `SUBMITTED` on any child of a block (`working-directory-real-child-fails`
  `fails` is `CREATED RUNNING FAILED` in every round). `flowable::start` now skips
  `SUBMITTED` for a `Runnable` whose parent is a `WorkingDirectory`. Pinned by
  `a_working_directory_child_is_not_submitted_and_a_plain_runnable_is`.
- `working-directory-real-runif-false` instability: Kestra creates the `SKIPPED` child
  and its next sibling within about 200 µs, and the order of their `taskRunList`
  entries flipped in rounds 4 and 5, while their `CREATED` instants stayed in the same
  order in all five. Numbering runs by `CREATED` (ties by list position) makes all five
  rounds one trace. For every other corpus flow the two orders agree, and the golden
  entries not listed above are byte-identical after the change.
- `retry-failed-task-duration` (4 attempts, Kestra 3): **pre-existing, now baselined.**
  At 58d21e5b, with this same golden, egret also makes 4 attempts. Egret's retry clock
  has no scheduler latency: a retry is due `now + interval`, and the driver's attempts
  take no logical time, so the fourth attempt fits in `maxDuration`. Kestra's three
  attempts take about 0.6 s of scheduling each. This is the class `retry-failed-flow-duration`
  is already baselined for. The entry is baselined by the user's decision (option a) and
  listed in `UNATTRIBUTED` in `tests/executor.rs` with this cause.
- The four WorkingDirectory valids, each against its golden entry:
  - `task-runif-workingdirectory`: still mismatches. The block FAILs because its child
    `log_orders` FAILs at creation (no RUNNING, no attempt) where Kestra runs it. Not
    isolated further; the `runIf:` render of a MULTISELECT input inside a block is the
    suspect.
  - `working-directory-invalid-runif`: still mismatches. Egret creates a FAILED run for
    `s2`, whose `runIf: {{ outputs.failed }}` cannot render (the render context has no
    `outputs`, as `valids/task-runif` says). Kestra creates no run for `s2` and fails
    the block with no attempt. It is in `STILL_OVER_ACCEPTED_AT_RUNTIME`.
  - `working-directory-taskrun`: MATCHES. Banked.
  - `working-directory-taskrun-nested`: still mismatches. Its `log-taskrun` child is the
    one the block stops at; the `{{ workerTaskrun }}` it renders is the render-context
    gap to check first. Not isolated.

**Census.** Every number that counts the corpus or the golden moved, each with its
derivation in the assertion's comment in `tests/executor.rs`, `src/executor/mod.rs`
and `handler_surface.py`. The deltas are explained by the seven new golden entries, the
four entries whose unstable pairs the re-harvest changed (`dag`, `finally-parallel`,
`if`, `parallel-nested`: 13 dropped pairs become 18), and the one banked baseline line.
`handler_surface.py --self-test` is 13/13. `handler-surface.txt` is regenerated and
`--check` is clean.

## kill-and-exit Task 3: `Exit`, and the four `valids/exit*` entries bank

**What changed.** `io.kestra.plugin.core.execution.Exit` has a `spec::registry`
row with no worker handler. `build_plan` plans it `TaskKind::Exit` and
`advance()` decides it (`executor::exit`). Every deployment serves it
(`spec::registry::EXECUTOR_LEVEL`, read by `runtime::Served::serves`). The four
`PUBLISHED_BUT_REFUSED` rows and the four `RUNS_REFUSED` entries are gone, and the
census moved 250 → 254 live and `UNKNOWN_TYPE_REFUSED` 65 → 61. The ratchet went
from 104 mismatching / 104 baselined to 100 / 100. `baseline.txt` lost exactly
the four `valids/exit*` lines, and no other entry moved in either direction.

**The golden decided the `KILLED` shape, and it is not the spec's first
reading.** `valids/exit-killed` reads `CREATED RUNNING KILLED` for the
execution, the `If` and the `Exit` run, with no `KILLING` anywhere. So a
`KILLED` exit does not hand the execution to `kill::advance_killing`, which
would write `KILLING` into two of those histories. It does what Kestra does
(`ExecutorService.handleExecutionUpdatingTasks`, then
`ExecutionService.killParentTaskruns`): the run and its ancestors become
`KILLED` at once, and so does the execution when nothing else is live. When a
sibling is live, the execution becomes `KILLING`. A running sibling `Runnable`
becomes `KILLING` (its worker stops it), and any other live leaf becomes `KILLED`.
These are the kill route's two leaf steps, done here because no route ran.

**Pinned figures re-derived, not reset.** The pair totals: 9 top-level pairs
on the four entries (`EGRET_TOP_LEVEL_PAIRS` 718 → 727,
`EGRET_PAIRS_BEFORE_RESTRICTION` 935 → 944, asserted 394 → 403). The baselined
populations: 10 run terminals, 4 executions, 31 run-history tokens and 12
execution-history tokens. Task 5's reconstruction derives the four `Exit` runs
(`is_an_exit`) and adds them back, as it does for `Subflow`. The handler-surface
ceiling went 228 → 232: `Exit` is in `W3A_FLOWABLES`, which is the set the
executor drives.

## Group C1 Task 2: `OutputValues`, and eleven entries bank

**What changed.** `io.kestra.plugin.core.output.OutputValues` has a
`spec::registry` row, a worker handler (`worker::handlers::output_values`,
which renders `values:` through `models::property::render_map`, Kestra's
`Property.asMap`), a `runtime::served_for` key and a place in `driver::registry`.
The census moved 254 → 274 live, `UNKNOWN_TYPE_REFUSED` 61 → 40 and
`VALIDATE_REFUSED` 26 → 27: 21 flows left the unported-type bucket, 20 publish,
and `valids/task-runif` now reaches its `Subflow` to `dummy_namespace/dummy_flow`
and is refused there (`SUBFLOW_LOOKUPS_ON_REFUSED` 2 → 3). Twenty-one
`PUBLISHED_BUT_REFUSED` rows went, eighteen `RUNS_REFUSED` entries (41 → 23),
and the ratchet went from 100 mismatching / 100 baselined to 89 / 89.

**Banked (11):** `flow-with-array-outputs`, `output-values`, `replay`,
`task-runif`, `waitfor`, `waitfor-max-iterations`, `waitfor-multiple-tasks`,
`waitfor-multiple-tasks-failed`, `waitfor-no-success`, `webhook-outputs`,
`webhook-wait`.

**Still baselined, with the cause measured by unbaselining them (7):** the
three `after-execution*`, the two `flow-concurrency-queue-*after-execution` and
`sleep-long` are held by `afterExecution:`, which egret does not run (the
`afterExecution:` run is the one missing from egret's trace);
`waitfor-child-task-warning` runs one more `WaitFor` iteration than the golden
under `maxDuration: PT3S`, the poller-latency shape `loop-until-restart` is
held by. `baseline.txt` says so on the line above each.

**Pinned figures re-derived, not reset**: `EGRET_TOP_LEVEL_PAIRS` 727 → 753 and
`EGRET_PAIRS_BEFORE_RESTRICTION` 944 → 970 (the twenty newly-running entries'
26 pairs); asserted pairs 506 → 528, unasserted 610 → 588; the baselined run
terminals 356 → 331, executions 135 → 124, run-history tokens 1388 → 1281 and
execution-history tokens 488 → 437; the field breakdown `execution` 29 → 30,
`runs` 45 → 52, `order` 20 → 26, total 96 → 85. The `Subflow` banks are fifteen
(`task-runif` joins), and the baselined webhook entries one. The
handler-surface ceiling went 232 → 243 and its population 204 → 221.

## Group C1 Task 3: `Assert`, and one entry banks

**What changed.** `io.kestra.plugin.core.execution.Assert` has a
`spec::registry` row, a worker handler (`worker::handlers::assert`: each
`conditions:` entry rendered with `render_str` and read with
`executor::is_truthy`; a render error counts as a failure; one INFO line
`<s> succeeded, <f> failed` in place of Kestra's two counters), a
`runtime::served_for` key and a place in `driver::registry`. The census moved
274 → 276 live and `UNKNOWN_TYPE_REFUSED` 40 → 38: `valids/assert` and
`valids/if-in-parallel`, the two flows whose only unported type was `Assert`,
publish. Two `PUBLISHED_BUT_REFUSED` rows and two `RUNS_REFUSED` entries
(23 → 21) went, and the ratchet went from 89 mismatching / 89 baselined to
88 / 88.

**Banked (1):** `assert` (its third condition is falsy, so `fail` FAILS, as in
Kestra).

**Still baselined, with the cause measured (1):** `if-in-parallel` runs, and
its `assert` FAILS where Kestra's SUCCEEDED: all three conditions read
`outputs.for_each.outputs[i].outputs.*`, the `Loop`'s own `outputs:`, which
egret does not produce, and each fails to render (`undefined value`; measured
by logging the handler's failures during the ratchet run). The same unclaimed
cause holds `loop-outputs*`.

The handler-surface ceiling went 243 → 245 (population 221 → 223): `Assert` is
now in `W3B_HANDLERS`. The derived figures that moved, each with a dated note
where it is pinned: `EGRET_TOP_LEVEL_PAIRS` 753 → 755, `EGRET_NESTED_PAIRS`
125 → 130, `EGRET_PAIRS_BEFORE_RESTRICTION` 970 → 977, spawned entries
40 → 41, top-level spawn keys identical 33 → 34, the field breakdown
(execution 30 → 31, runs 52 → 53, total 85 → 84), loop bodies (38, 82) →
(39, 85), the default-limit loop population 23 → 24, and the baselined
terminals, executions and history tokens by `assert`'s share.

## Group C1 Task 4: `PurgeLogs`, four entries bank and one is held by the scope rule

**What changed.** `io.kestra.plugin.core.log.PurgeLogs` has a
`spec::registry` row, a worker handler (`worker::handlers::purge_logs`, over
`LogRepo::purge`), a `runtime::served_for` key and a place in
`driver::registry` (its own `purge_logs::register`, beside `kv::register`, as
in `runtime::handlers_for`). Every delete is limited to the execution's
workspace; with no `namespace` the scope is the flow's own namespace and its
dot-children; a given `namespace` must pass
`nsfiles::access::authorize(…, Resource::Logs, Access::Purge)` or the run
FAILS and nothing is deleted (spec §2.3, the user's ruling 2026-10-18). The
census moved 276 → 281 live and `UNKNOWN_TYPE_REFUSED` 38 → 33: the five
`valids/purge_logs_*`. Five `PUBLISHED_BUT_REFUSED` rows and five
`RUNS_REFUSED` entries (21 → 16) went, and the ratchet went from 88
mismatching / 88 baselined to 84 / 84.

**Banked (4):** `purge_logs_execution_only`, `purge_logs_no_arguments`,
`purge_logs_trigger_only` and `purge_logs_with_batch_size` (one run each,
SUCCESS, as in Kestra).

**Held permanently (1):** `purge_logs_full_arguments` runs in
`io.kestra.tests` with `namespace: purge.namespace`, and egret FAILS it where
Kestra SUCCEEDED, by the decided rule. Measured during the ratchet run (a
temporary probe, reverted): `a flow of namespace 'io.kestra.tests' may not
purge logs of namespace 'purge.namespace': …`. Its `baseline.txt` line says it
is permanent, not a gap.

The handler-surface ceiling went 245 → 250 (population 223 → 228): `PurgeLogs`
is now in `W3B_HANDLERS`. The derived figures that moved, each with a dated
note where it is pinned: the field breakdown (execution 31 → 32, runs 53 → 54,
total 84 → 80), the bank split (301, 213, 88) → (301, 217, 84), the timeout
split (2, 86, 88) → (2, 82, 84), baselined terminals 329 → 325, executions
123 → 119, run-history tokens 1273 → 1257 and execution-history tokens
434 → 422. No ordered-pair, spawn or loop-body figure moved: each of the five
flows is one top-level task.

## Group C1 in total: sixteen entries bank, three gaps stay open and one entry is held by decision

Group C1 (`OutputValues`, `Assert`, `PurgeLogs`) took the ratchet from 100
mismatching / 100 baselined to 84 / 84 with 0 broken, in three steps
(100 → 89 → 88 → 84; the printout of `executor_matches_golden` in each task's
report). Sixteen entries banked: eleven by `OutputValues`, one by `Assert`, four
by `PurgeLogs`. The census LIVE count went 254 → 281 and the handler-surface
ceiling 232 → 250 (`python3 handler_surface.py`, population 228, run at the end
of the group). The three per-task sections above carry the derivation of every
other figure that moved.

What stays baselined among the entries C1 touched, and why:

| Entries | Cause | Kind |
|---|---|---|
| `after-execution`, `after-execution-error`, `after-execution-finally`, `flow-concurrency-queue-after-execution`, `flow-concurrency-queue-killed-after-execution`, `sleep-long` | `afterExecution:` is not run by egret | gap |
| `waitfor-child-task-warning` | one more `WaitFor` iteration than the golden under `maxDuration: PT3S` (poller latency, as `loop-until-restart`) | gap |
| `if-in-parallel` | its `assert` reads the `Loop`'s own `outputs:`, which egret does not produce (as `loop-outputs*`) | gap |
| `purge_logs_full_arguments` | purges `purge.namespace` from `io.kestra.tests`; egret FAILS it by the user's namespace-scope ruling of 2026-10-18 | permanent, by decision |

Three of those rows are gaps (`afterExecution:`, the `WaitFor` poller
latency, the `Loop`'s own `outputs:`) and one is permanent by decision.

The `valids/task-runif` line from the C1 Task 2 section is a bank, not a
baseline entry: the executor oracle banks it (it runs the flow through
`build_plan` and the worker, and does not go through publish), but publish
refuses it, at its `Subflow` to a flow no corpus entry defines
(`VALIDATE_REFUSED` 26 → 27, `SUBFLOW_LOOKUPS_ON_REFUSED` 2 → 3). See *Known
over-refusals at publish* below.

**The `PurgeLogs` banks prove the state sequence, not the delete.**
`driver.rs`'s registry installs no log sink, so no oracle run stores a log
line, and the oracle database is this process's own clone of the migrated
template, with `workflow.executions` `TRUNCATE`d (`CASCADE`) before the run
(`tests/executor.rs`, *The `TRUNCATE`*). An oracle run of `PurgeLogs`
therefore deletes 0 rows: the four banked entries prove `CREATED → RUNNING →
SUCCESS`, not which rows were deleted or how many. Deletion and `count` are
proved by `tests/purge_logs.rs`.

### Known over-refusals at publish

No tracked list exists for this layer. `PUBLISHED_BUT_REFUSED`
(`src/executor/mod.rs`) tracks the flows Kestra published with 200 that
`build_plan` refuses, and its sweep asserts each row is still refused by
`build_plan`; a flow that plans but that publish's reference resolution
refuses cannot be a row there. The census (`driver.rs`, `VALIDATE_REFUSED`)
counts publish's refusals and prints each by name, but names none of them as
an over-refusal. So the entry is recorded here:

| flow | where egret refuses it | why it is an over-refusal |
|---|---|---|
| `valids/task-runif` | publish, resolving the `Subflow` `notexecutedflowable` to `dummy_namespace/dummy_flow`, which does not exist (since group C1 Task 2, when its `OutputValues` stopped refusing it first) | that task has `runIf: "false"`, so it never runs; this Kestra published the flow with 200 (`corpus.json`) and ran it, and the executor oracle banks egret's run of it |
| `valids/trigger-flow-listener-no-inputs` | publish, resolving the `Flow` trigger `listen-flow-failed`'s `dependsOn` to `io.kestra.tests.trigger/other-flow`, which no corpus flow defines (census line `not found: referenced object`) | this Kestra published it with 200 (`corpus.json`); a dependency on a flow that does not exist yet is legal there. Not C1's: found while writing this list, it predates the group |

Both are the same shape: publish's reference resolution (`RefKind::Subflow`)
refuses a reference to a flow that does not exist, and this Kestra does not.
The census prints both lines by name in every run; neither is asserted as an
over-refusal anywhere.

## Group C2 Task 5: `SetVariables`, `UnsetVariables` and `Labels`, eleven entries bank

**What changed.** The three `ExecutionUpdatableTask`s have `spec::registry`
rows (FQCN only) and are in `spec::registry::EXECUTOR_LEVEL`, as `Exit` is: no
worker handler, no `driver::registry` entry and no `runtime::served_for` key,
because `runtime::Served::serves` answers true for every executor-level type
(`spec::unrunnable`'s `the_executor_level_types_publish_on_a_deployment_with_no_handlers`
pins it on an empty `Served`). `build_plan` plans each `TaskKind::Update`
(group C2 Task 3) and the worker's render pass computes it (Task 4). The census
moved 281 → 293 live and `UNKNOWN_TYPE_REFUSED` 33 → 21 (`VALIDATE_REFUSED`
stays 27). The last twelve `unknown_task_type` rows of `PUBLISHED_BUT_REFUSED`
went, and `RUNS_REFUSED` went 16 → 4. The ratchet went from 84 mismatching / 84
baselined to 73 / 73, 0 broken.

**Banked (11):** `labels-update-nested-value-list`,
`labels-update-nested-value-map`, `labels-update-task`,
`labels-update-task-deduplicate`, `labels-update-task-empty`,
`npe-labels-update-task`, `primitive-labels-flow`, `set-variables`,
`set-variables-duplicate`, `task-runif-executionupdating` and
`unset-variables`. Both failure shapes match the golden's (`CREATED FAILED`,
attempts `[["FAILED"]]`).

**Still baselined (1):** `task-runif-taskswithstate`. Measured with a
temporary probe (reverted): egret's `hello` and `will_fail` runs and the
execution `CREATED RUNNING FAILED` match the golden exactly; the two runs
missing are `write_execution_context` (the `SetVariables`) and
`skipped_when_failures_exist`, both in the flow-level `finally:`, which egret
does not run. Not a C2 defect.

The handler-surface ceiling went 250 → 261 (population 228; `W3A_EXECUTOR_LEVEL`
holds the three). The derived figures that moved, each with a dated note where
it is pinned: the bank split (301, 217, 84) → (301, 228, 73), the field
breakdown (runs 54 → 55, order 26 → 27, total 80 → 69), the ordered pairs
(`EGRET_TOP_LEVEL_PAIRS` 755 → 777, `EGRET_PAIRS_BEFORE_RESTRICTION`
977 → 999, asserted 529 → 550, baselined 587 → 566), the timeout split
(2, 82, 84) → (2, 71, 73), baselined terminals 325 → 303, executions
119 → 108, run-history tokens 1257 → 1192 and execution-history tokens
422 → 389. The residual `ExecutionUpdatableTask` row went 12 → 1.
