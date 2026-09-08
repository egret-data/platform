# W6 — Subflow, replay, and four words the oracle cannot see

**Status:** design, awaiting review
**Depends on:** W3b (durable executor), W4 (runner)
**Followed by:** W7 (frontend), W8 (Pause / WaitFor / concurrency / sla)

## 1. The measurement, first, because it splits the package in two

The programme spec §9 gives W6 *"cron, webhook, flow trigger, `subflow`,
backfill"*. Measured over the 345-flow corpus and the 143-entry baseline
**before** any of this was written:

| construct | corpus declarations | baselined declarations | **reachable** |
|---|---|---|---|
| `Subflow` | 25 flows | 22 entries | **9 strict / 11 upper bound** |
| flow trigger | 17 flows | 0 | **0** |
| cron / schedule trigger | 7 flows | 1 | **0** |
| webhook trigger | 15 flows | 3 | **0** |
| backfill | **0 live** (2 commented-out lines) | 0 | **0** |
| `CREATE_NEW_EXECUTION` replay | 4 | 4 | **4** |

**Four of the row's five words are worth zero baselined entries, and one of
those four has no corpus population at all.** So:

- **The bankable half is `Subflow` + replay** — 13 to 15 entries, judged by the
  existing ratchet, and they share one migration.
- **The unmeasurable half is cron, webhook, flow trigger and backfill.** W6
  builds them, because a workflow feature without a schedule is not a feature —
  but **nothing in the ratchet moves whatever W6 builds there, and nothing in
  the ratchet would notice if it broke.** They are judged by their own tests,
  the position `egret.sql` (W3b) and `egret.container` (W4) already occupy.

This spec says that on page one rather than letting a green ratchet imply
otherwise, which is the discipline W3b's §3.1 established after the fact and W4
adopted before the fact.

### 1.1 Why triggers are invisible — the mechanism, not the verdict

"The oracle cannot see it" is three different facts and this is the third one.

**The harness fires triggers and cannot see a single execution they create.**
Both halves were measured on the pinned artifact. `docker compose` runs
`server standalone`, which includes the scheduler; publishing three trigger
flows and executing one produced **13 executions of which 10 came from something
other than the harness's POST** — six from a `Schedule` nobody asked for, three
from `Subflow`, one from a `Flow` trigger.

None of them can reach the golden, for three independently verified reasons:

1. `harvest.py` never calls `POST /executions/webhook/…` — the string appears
   once in the file, as the *reason* `valids/webhook-inputs` is excluded. The
   webhook trigger is never invoked.
2. `run_flow` settles `{root} ∪ descendants(root)`, and `descendants()` walks
   **`parent_id`** — which Kestra leaves **NULL** for anything a trigger creates
   (a `Flow`-fired execution carries `trigger_execution_id` instead; a
   `Schedule`-fired one carries neither).
3. `normalise.py` builds `by_parent` from `parentId` and `bail`s on a descendant
   that does not hang off the root, so even a discovered trigger-fired execution
   could not be normalised into the trace.

**The mechanism is precise and worth naming:** this is not a predicate that is
never evaluated. It is **a predicate evaluated on every one of the 268 executed
flows, at every settle poll, that structurally cannot select a trigger-fired
execution** — it keys on a column Kestra never sets for one. Beside it sits a
weaker second mechanism: the log-scrape arm's regex **cannot match on this
artifact at all**, in either the raw body or the decoded message, which is why
`descendants()` has one arm and not the two its own documentation claims.

**Making triggers measurable is a package of its own, and it is deliberately not
this one.** It needs `descendants()` to union `trigger_execution_id`,
`normalise.py` to build the tree on that link, a golden schema that can hold a
sub-execution with no loop index, a **re-harvest**, and a **rewritten
`baseline.txt`** — the reference point every published number from W1 to W4 is
stated against. That cost belongs beside the decision, not inside this package.

## 2. `Subflow` cannot reuse `SpawnExecution`, and the reason is load-bearing

W3b built a `SpawnExecution` executor. It is tempting to point `Subflow` at it.
**That would break the entries it is meant to bank**, for four structural
reasons, each measured:

1. `SpawnExecution` derives the child's source from the **parent's own YAML** and
   carries a loop `value`. A `Subflow` resolves a **different flow** by
   `(namespace, flowId)`.
2. Migration `0135`'s all-or-nothing CHECK makes a child with a parent but **no
   loop index** unrepresentable.
3. **Decisively:** `driver.rs` builds the `spawned` field from
   `parent_execution_id`. Linking a `Subflow` child that way puts it in egret's
   trace — where **all 41 golden `spawned`-carrying entries have zero `Subflow`
   children.** The intended banks would become `spawned` mismatches.
4. Kestra itself makes the distinction: it sets `parentId` for a **loop
   iteration** and for a **`CREATE_NEW_EXECUTION` replay**, and leaves it NULL
   for a **subflow child**, which is linked by `trigger_execution_id`.

**So W6 copies Kestra's distinction rather than widening
`parent_execution_id`.** The replay's four entries need the same schema change
`Subflow` needs, which is why they are in this package: one migration, not two.

## 3. The driver cannot resolve a `Subflow` target today

`oracle-exec/driver.rs` calls `start_execution(..., None, source, inputs)` — it
**never publishes the corpus into `workflow.flows`**. So there is no table for a
`Subflow` to resolve `(namespace, flowId)` against, and today egret cannot
execute one at all.

**W6 must change the driver before `Subflow` is measurable**, and that change is
itself a risk: publishing 345 flows into the driver's path widens what every
other entry can reach. Every entry that moves for that reason must be
**attributed**, not absorbed — the failure W3b's §4.4 named when the render
surface widened.

## 4. Why 22 fell to 9

Three counts would each have been wrong alone — **22** (declares a `Subflow`),
**21** (has an executed `Subflow` run), **15** (no other blocker). The strict
figure is **9**, with **11** as an upper bound, and the fall comes from two
independent necessary conditions:

- **Seven entries have a second blocker owned by someone else** — W8's
  `concurrency:` ×2, the `Loop` residue ×2, `AllowFailure`/`WARNING` ×2, and
  `OutputValues` ×1. That last, `valids/task-runif`, has only a **SKIPPED**
  `Subflow` run *and* a target flow that is not in the corpus.
- **Four more are blocked because their `Subflow` target is itself baselined.**

An entry that declares a `Subflow` but mismatches on something else is **not
W6's**, and the plan must report every banked entry with the reason it moved.

## 5. What judges W6

Three instruments, and only one is the ratchet.

1. **The ratchet, for `Subflow` and replay** — 9–11 plus 4, reported before and
   after, **with every moved entry attributed**. An entry that banks for an
   unattributed reason is a coincidence, not a bank.
2. **Triggers' own tests**, with no oracle coverage, stated as such. A cron that
   fires twice, a webhook that authenticates, a flow trigger that filters — none
   of these is visible to the corpus, so their suites are the whole instrument.
   The W4 rule applies: **a suite that self-skips silently is not coverage**, and
   a missing dependency must fail rather than skip.
3. **The four false claims this measurement found in the existing record** (§7)
   must be corrected, because they are the reason the ratchet's silence about
   triggers looked like an absence of behaviour rather than a defect in the
   instrument.

## 6. Fidelity rules

Unchanged, with three that carry weight here:

- **Reproduce observable behaviour, including upstream defects.** Kestra's
  `parentId`/`trigger_execution_id` split is a distinction to copy, not to tidy.
- **Over-acceptance must not ship.** A trigger that fires where Kestra would not
  is the next instance, and the oracle cannot catch it.
- **A guard nobody has seen fail is a guard nobody has seen** — ten across W3a,
  W3c and W3b, and W4 added the inverse: a *redundant* guard and a weak one look
  identical in a mutation table.
- **A figure is safe only if a running program compares it.** W4's sweep needed
  five passes and 23 corrections to arrive at that rule; it applies to anything
  W6 writes down, including this spec.

## 7. Contradictions in the existing record, to be corrected

Each measured, not inferred:

1. `oracle-exec/README.md` claims *"A sub-execution … carries `parentId`"* —
   **false** for a `Subflow` child on the pinned artifact, where `parentId` is
   NULL and the link is `trigger_execution_id`.
2. The same section describes `descendants()` as unioning **two independent**
   discoveries. The log-scrape arm's regex matches neither the raw JSON body
   (escaped quotes) nor the decoded message (the markup carries
   `flowId=`/`namespace=` before `]]`). **It is dead code; there is one arm.**
3. `ENTRIES_LEFT_UNFINISHED = 84`'s assertion message says every one is a task
   type with no handler. The four `retry-new-execution-*` entries declare only
   `core.execution.Fail`, which **has** one, and end in `RETRIED` — which
   `is_stalled` omits from its terminal list although `RunState::Retried`'s own
   doc, and Kestra's `State.isTerminated()`, both call it terminal. **Four of
   the 84 are a classifier artefact, not a stall.**
4. The programme spec's W6 row itself. It is corrected in the same commit as
   this spec, in both languages — and the correction is the point of §1.

## 8. Risks

1. **Publishing the corpus into the driver widens the render and resolution
   surface**, and entries will move for reasons unrelated to `Subflow`. Attribute
   them; this is W3b's §4.4 failure with a new cause.
2. **Populating `trigger.*` in the render context would make
   `valids/webhook-plaintext` diverge further**, because the golden was harvested
   with **no** trigger context. Measure before populating.
3. **Triggers ship uncovered by the oracle** — a deliberate trade, and the risk
   is a green ratchet read as coverage it does not have. Three packages running
   have now taken this trade; W6 is the first where the uncovered half is the
   larger one.
4. **A scheduler is a clock**, and this programme's tests have been bitten by
   wall-clock assertions before. Trigger tests must use an injected clock.

## 9. Deliverables

- `Subflow` with its own link column, not `parent_execution_id`, and the
  `CREATE_NEW_EXECUTION` replay on the same migration.
- A driver that publishes the corpus so `(namespace, flowId)` resolves.
- Cron, webhook and flow triggers, with their own tests and an injected clock.
- Backfill, with its own tests, and a written statement that the corpus contains
  **zero** live declarations of it.
- The four corrections in §7.
- A coverage statement that separates the 13–15 entries the ratchet speaks for
  from the triggers it cannot.
