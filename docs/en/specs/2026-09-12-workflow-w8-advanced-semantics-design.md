# W8 — Advanced semantics, and a row that should be deleted rather than lowered

**Status:** design, awaiting review
**Depends on:** W3b (durable executor), W6 (Subflow, replay — PR #578, stacked)
**Followed by:** W5 (plugin host), W7 (frontend), W9 (KV, storage)

## 1. The measurement, first

The programme spec §9 gives W8 *"`pause`/`resume`, `allowFailure`, `loopUntil`,
concurrency limits, `WorkingDirectory`, labels"*. Measured over the 345-flow
corpus and the **127**-entry baseline **before any of this was written**, with
"reaches" meaning *the entry's measured egret-vs-golden difference is produced by
that construct* — not *the flow declares it*:

| construct | declared (baselined) | declared (corpus) | **reaches** | **bankable by W8 alone** |
|---|---|---|---|---|
| `Pause` | 12 | 20 | **12** | **7** |
| `LoopUntil` / `WaitFor` | 9 | 10 | **9** | **3** &rarr; **1 measured** [^lu] |
| `allowFailure` / `AllowFailure` | 12 | 15 | **12** | ~~**6**~~ &rarr; **7 measured** [^af] |
| `sla:` | 5 | 6 | **7** | **2** |
| `WorkingDirectory` | 4 | 15 | **4** | **0 measured, 1 candidate** |
| flow `concurrency:` | 6 | 13 | **0** | **0** |
| flow `labels:` | 3 | 22 | **0** | **0** |

**So the package is `Pause` + `LoopUntil` + `allowFailure` + `sla`'s two, and
the honest headline is 18 strict — not the 32 the residual table's declaration
counts imply.** The separator between strict and upper bound is always the same:
a second construct in the same flow whose owner is not W8 — `afterExecution:`,
task-level `finally:`/`errors:`, `OutputValues` with no handler,
`ExecutionUpdatableTask`, a `runIf:` render W3c already named out of reach, or a
field `models::Task` does not carry.

### 1.1 `concurrency:` reaches zero, and its row should be **removed**, not lowered

The residual table says flow `concurrency:` → **`CANCELLED`**. **That is false.**
`CANCELLED` appears in **three** golden entries: **two are `sla:`'s and one is
`Exit`'s**. Not one is a concurrency limit's.

This is **the identical defect W3c fixed by moving `SKIPPED` off `disabled:`** — a
state attributed to the wrong construct, which then makes a row look reachable
because the state is really there, just not for that reason. By the table's own
rule the row does not get a smaller number; **it gets deleted.**

And the reason it can never be reached here is nameable rather than incidental:
**`harvest.py` drives flows strictly serially, so `limit: 1` is never
contended.** Seven of the thirteen declarers **already match**; the corpus's one
genuinely contended population (`flow-concurrency-parallel-subflow-kill-child`)
**is already banked by W6**, and its `QUEUE` effect lands in **no compared
field**.

### 1.2 A fifth mechanism of oracle blindness: the harness compresses time

W4 established that *"the oracle cannot see it"* is three distinct facts; W6 found
a fourth. Here is a fifth, and it is the reason **five of `sla:`'s seven entries
are unbankable by any implementation whatsoever**:

**`Sleep::capped(ZERO)` means no run is ever in flight, and `driver::pass_time`
only moves the clock to a registered `not_before_ms`.** An SLA's
`MAX_DURATION` measures a window that, in this harness, **does not exist**. Not
"is not entered" — *does not exist*. Only the two `EXECUTION_ASSERTION` entries
are bankable, because an assertion is evaluated at a terminal rather than over a
duration.

That is a property of the instrument, not of egret, and it must be in the
coverage statement beside the other four rather than discovered by the next
reader.

### 1.3 `sla:`'s cause count exceeds its declaration count

The residual table counts **5**; the cause is **7**. Two entries are held by an
`sla:` in a **different flow** — a parent whose child declares one — which a
per-flow needle **structurally cannot see**. A declaration count and a cause count
can differ in *both* directions, and this is the direction nobody had looked for.

[^af]: **7, measured at W8 Task 5**, and the twelfth entry is the finding:
    `valids/waitfor-child-task-warning` was recorded as *held twice*, and
    unbaselining it turned that into a measurement — its `WARNING` child is
    byte-identical to the golden now, `childTaskRunExecuted` lets it through,
    and the loop reaches an `io.kestra.plugin.core.output.OutputValues` child no
    handler exists for and stalls. The other four that stay are the
    `AllowFailure` FLOWABLE's, a different construct that is still planned
    opaque.

[^lu]: **Corrected by W8 Task 4, which implemented the construct and then
    measured it.** The `3` was the number of entries spelled `LoopUntil` rather
    than `WaitFor`, and that split is not the separator: the separator is the
    `OutputValues` handler, which holds exactly the six `WaitFor`-spelled ones.
    Of the other three, `valids/waitfor-nested` banks and the remaining two each
    differ from the golden on exactly ONE field, for a reason that is not the
    loop. `valids/loopuntil-failed-flowable`'s condition is
    `{{ outputs.loop.iterationCount < "5" }}`: Pebble widens both operands of a
    relational operator to `Number` and throws on a `String`, minijinja orders
    across kinds and answers `True`, so the loop succeeds where the artifact
    failed the task — a RENDERER divergence, and the corpus's only
    number-against-string comparison.
    `valids/loop-until-restart` runs four iterations against a golden of three
    under `maxDuration: PT3S` / `interval: PT1S`, because the artifact's
    delay poller made its two `PT1S` pauses take **1.17 s and 1.92 s** and
    `driver.rs::pass_time` moves the clock to the earliest deadline and never
    further. The corroboration that this is the construct rather than the entry:
    `valids/waitfor-max-duration`, the same shape at `PT5S`/`PT0.2S`, is one of
    only **two** corpus flows Kestra itself could not reproduce across five
    rounds, so it has no golden entry at all. Both now carry a blocker row in
    `oracle-exec/handler_surface.py`, so the ceiling no longer names an entry
    nothing can bank.

## 2. `Pause` and `LoopUntil` are one problem and must not be split

The 18-entry `PAUSED` population is **12 `Pause` + 6 `LoopUntil`**. They need the
same enum variant, so **splitting them across packages blocks each on the other's
state**. They ship together or not at all.

`plan.rs`'s opacity doc names `AllowFailure`, `Pause` and `WorkingDirectory` and
**omits `LoopUntil`** — which is opaque too, and stalls nine baselined entries.
That omission is a W8 correction, not a W8 discovery.

> **Superseded by W8 Tasks 3 and 4**: `Pause` left the opaque tier at Task 3 and
> `LoopUntil` left it at Task 4, so that list is now `AllowFailure` and
> `WorkingDirectory` and a `Pause` or a `LoopUntil` whose own properties this
> pure builder cannot read (`pause_gap`, `loop_until_gap`). What Task 4 found
> that this paragraph could not is one tier lower: **`WaitFor` never reached the
> opaque tier at all**, because `models::kind_for` had no row for the alias and
> it parsed to `TaskKind::Leaf` — the same plan either way, which is why every
> prose list of the opaque types stayed true while being wrong about this one.
> Seven of the ten corpus flows of the construct use that spelling.

## 3. What `advance()` needs, and what it does not

Measured, and this is the load-bearing paragraph:

- **Two new `RunState` variants** — `Paused` (non-terminal) and `Restarted` — on
  top of the current nine.
- **No new state field, and no input the oracle's driver cannot supply.** All
  twelve baselined `Pause` entries bound the pause with `pauseDuration:` or
  `timeout:`; `TaskRun::attempt_started_at()` and `TaskRun::not_before` already
  exist for exactly that measurement; and `driver::pass_time` already moves the
  clock to any `not_before_ms` **with no state term**.
- **So `advance()` stays pure over `(plan, state, now)`** — no I/O, no clock.

**What is missing is on the product side, not in the state machine:**
`claim::due_retries` is **state-gated to `RETRYING`/`RETRIED`** and would
therefore **never wake a `PAUSED` run that the oracle happily resumes.** That
asymmetry is the package's first real defect and it is invisible to the ratchet,
because the oracle's driver reaches the same runs by a different door.

Two flows need a **fourth caller-filled render slot**, because their
`pauseDuration:`/`behavior:` is an expression.

`src/executor/` was byte-unchanged from W3a until **W6 Task 3 opened it with
evidence**. W8 opens it again for the two variants, and the same standard applies:
say what the oracle could not otherwise reproduce, and treat any change to
`advance()`'s *contract* as a finding rather than a step.

## 4. `allowFailure` is the cheapest win and has two owners

**Twelve entries reach it; six are bankable; two need no new state at all** —
`allowWarning:` maps `WARNING → SUCCESS`, which the existing enum already
expresses.

> **W8 Task 5 measured all three figures and two of them were wrong.**
> **Seven** bank, not six, and the seventh is `valids/pause-timeout-allow-failure`
> — which this table counts under `Pause` because it attributes by declaration,
> so the two rows do not double-count it; the arithmetic of the package's 18 is
> unchanged. **Three** need no new state, not two, and they are
> `valids/task-allow-warning-{runnable,flowable,executable-flow}` — **none of
> which is one of the eight** golden entries the residual table derives for this
> construct, because those eight are by construction the entries that HOLD a
> `WARNING` symbol and an entry holding one cannot be reproduced without the
> variant.
>
> **And the reason given is not the mechanism.** Measured on the pinned artifact
> with `javap -p -c`, `State$Type.fail(Task)` is a CONSTRUCTION out of two
> booleans with no source state — `isAllowFailure()` false → FAILED, true +
> `isAllowWarning()` → SUCCESS, true alone → WARNING — and it does not read
> `isAllowWarning()` at all when `isAllowFailure()` is false. There is no
> `WARNING` to map *from*. The three flows above write BOTH flags, so the answer
> is SUCCESS in one step and no `WARNING` is ever written to a history. A second
> spelling, two sequential `if`s in `WorkerTaskProcessor` /
> `Execution.guessFinalState` / `ExecutableUtils.guessState`, IS a conversion
> keyed on an existing state, and it is the only one that can map a stored
> `WARNING` under `allowWarning:` alone. See
> `crates/egret-workflow/src/executor/allow_failure.rs`.

It is also **assigned twice in two documents**: W8 in the programme spec §9, and
**"unclaimed" in the residual table**. One of those is wrong and the package must
say which rather than inheriting both.

> **Settled: the residual table was the loser, and it was corrected at W8 Task 1
> rather than §9 being lowered.** W8 Task 5 re-checked both documents and the
> correction is in place: `oracle-exec/README.md`'s residual table now reads
> **W8** for this row, and the programme spec §9 is unchanged. The ruling is
> derived rather than administrative — all twelve entries reach the construct
> and seven of them bank inside W8 alone, which is the cheapest set in the
> package; a row whose entries a package can bank is not unclaimed.

## 5. What W8 unlocks for W6, measured rather than assumed

W6 left ten `Subflow` entries it could not bank. **W8 is the second blocker for
four of them — not seven — and for one via `concurrency:`, not two.**
`flow-concurrency-sla-fail-parent`'s real blocker is its **child's `sla:`**;
`sla-parent-flow`'s likewise; the other two are the `task-allow-*-executable-flow`
pair.

**Only the two `allow*` ones are bankable.** Both `sla:` parents need a
`MAX_DURATION` kill **this oracle cannot produce** (§1.2).

And the dependency runs the other way more strongly than the row implies: **W6
Task 3 is what moved all four divergences onto W8's constructs at all** — before
it they diverged at the `Subflow` — and **W6's `guess_state` is explicitly
narrowed from `{FAILED, PAUSED, KILLED, WARNING}` to `{Failed, Killed}`**, so both
`Paused` and `Warning` **widen a W6 function**.

> **Measured at W8 Task 5 fix round 1: only ONE of the two widens it.**
> `WARNING` is back in the trigger set — a child execution really can end
> `WARNING` once its own aggregate does, and the artifact transmits it (bytecode
> 44–47) and lets `allowWarning:` alone map it to `SUCCESS` (79–102). `PAUSED`
> does **not** and cannot: `on_child_terminal` runs from a child that has
> TERMINATED, and `State.Type.isTerminated()` does not list `PAUSED`, so that
> disjunct is unreachable from the caller rather than declined. Two disjuncts
> that read as one narrowing are two different facts. W6 Task 3 is also what lowered the
`concurrency:` row from 7 to 6.

## 6. What judges W8

1. **The ratchet, for 18 strict.** Report before and after with **every moved
   entry attributed** — an entry that banks for an unattributed reason is a
   coincidence, not a bank. W6's standard is the one to meet: a **one-line
   mutation** that reverts the construct and returns the ratchet to its old value
   **naming exactly the banked ids**.
2. **`pause`/`resume`'s own tests**, with no oracle coverage, stated as such — the
   blindness here is the **webhook mechanism** (the harness never performs the
   out-of-band action) reached through a different gate: **seven flows are
   excluded by a 90 s runtime timeout the harvester classifies**, and the
   exclusion message **names the two Java calls it does not make**.
3. **The three corrections in §1.1, §1.3 and §4**, because a residual table that
   attributes a state to the wrong construct is worse than one that says nothing.

## 7. Fidelity rules

Unchanged, with four that carry weight here:

- **Over-acceptance must not ship.** A `Pause` that resumes itself where Kestra
  waits for a human is the next instance, and **the oracle cannot catch it**.
- **A guard nobody has seen fail is a guard nobody has seen** — and **a two-sided
  mutation pair on an operator says nothing about where the operand came from**.
  W6 found **nine** fields of that shape in one module after being handed four,
  and seventeen unpinned claims in another.
- **A fixture answers the question you built it for and stays silent about the one
  you then generalise to.** Seven instances in one W6 task. Before any general
  sentence: **name the element of the class the fixture does not contain.**
- **A one-run mutation verdict is a coin flip reported as a measurement.** Run
  count belongs beside every verdict, and **a control on the unmutated binary is
  what catches a lying harness** — one lied twice in W6.

## 8. Risks

1. **`Paused` is non-terminal, and every existing terminal-state predicate is a
   candidate defect.** W6's `guess_state` is one that is already known to narrow
   the set; the rest have not been enumerated.
2. **`claim::due_retries` will not wake a paused run**, and the ratchet cannot see
   that because the driver resumes by another route (§3). This is the shape W4's
   item 15 named: a property proved about a path no deployment takes.
3. **Five `sla:` entries are unbankable by construction** (§1.2). A package that
   counts them will report progress it cannot make.
4. **`concurrency:`'s row will look like lost scope.** It is not: it was never
   reachable, and the number in the table was attached to the wrong state.

## 9. Deliverables

- `Paused` and `Restarted` in `RunState`, with `advance()` still pure, and the
  `src/executor/` opening justified the way W6 Task 3 justified its own.
- `Pause` and `LoopUntil` together — one state, one package.
- `allowFailure`/`allowWarning`, with the two-owner question settled.
- `sla:`'s two `EXECUTION_ASSERTION` entries, and the other five **recorded as
  unbankable with the mechanism**.
- **The `concurrency:` row deleted** from the residual table, with the
  `CANCELLED` misattribution recorded as the W3c-shaped defect it is.
- `plan.rs`'s opacity doc corrected to include `LoopUntil`.
- A coverage statement carrying the **fifth** blindness mechanism beside the other
  four.
