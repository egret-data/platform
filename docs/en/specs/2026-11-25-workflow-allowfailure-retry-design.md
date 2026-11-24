# AllowFailure block retry — design (package 9b)

**Roadmap:** parity roadmap package #9 was split into **9a** (full Pause flowable, merged #911) and **9b** (this: `retry:` on an `AllowFailure` block). 9b completes package #9.

**Depends on:** the `AllowFailure` flowable (#680), retry inheritance (`executor/retry.rs`), and the corpus/golden harness (`oracle-exec/`).

## 1. Goal

Lift the `unsupported_allow_failure_retry` refusal so that a `retry:` declared on
an `AllowFailure` block is accepted at publish/start and runs exactly as Kestra's
does — and, in the same commit, reconcile the corpus census so the one corpus
flow this refusal has been blocking (`valids/retry-with-flowable-errors`) goes
from *published-but-refused* to *live and matching the golden*.

## 2. What Kestra does — the measurement is already vendored

The measurement is **not** a live-image step. The pinned Kestra trace for the
exact interaction is already vendored in `oracle-exec/golden.json` (and
`corpus.json`/`traces.json`) as `valids/retry-with-flowable-errors`:

```yaml
# oracle-exec/corpus/valids/retry-with-flowable-errors.yaml (abridged)
tasks:
  - id: set_kv            # KV Set retry_counter_1 = 1
  - id: retry_block
    type: io.kestra.plugin.core.flow.AllowFailure
    tasks:
      - id: run_script    # Log; message renders `ko` (fails) while counter < 2, else "It works"
    errors:
      - id: incr_counter  # KV Set retry_counter_1 += 1
    retry:
      type: constant
      behavior: RETRY_FAILED_TASK
      maxAttempts: 3
      interval: PT0.5S
      warningOnRetry: true
```

Golden trace (the oracle every 9b test and the driver must reproduce):

| run | parent | state | attempts |
|---|---|---|---|
| `set_kv` | — | CREATED→…→SUCCESS | 1 |
| `retry_block` (AllowFailure) | — | CREATED→RUNNING→**SUCCESS** | **1** (block never retried as a unit) |
| `run_script` (child) | `retry_block` | …FAILED→RETRYING→…→**SUCCESS** | **2** (attempt 1 FAILED, attempt 2 SUCCESS) |

Execution: `CREATED → RUNNING → RETRYING → RUNNING → SUCCESS`.

This confirms the three load-bearing facts, all already present in the engine for
every *other* flowable:

- **A flowable is never retried as a unit** (`retry.rs` `is_retried_kind` lists
  only `Runnable`/`Executable`): the block stays a single run; its **child**
  runs the attempts.
- **Retry is inherited** (`retry.rs` `inherited_retry`, lines ~140-153): a child
  with no `retry:` of its own uses its nearest ancestor flowable's `retry:`.
- **`AllowFailure` maps the terminal** (`executor/allow_failure.rs`
  `block_state`): a failed child → block `WARNING`; here the child ultimately
  **succeeds** (attempt 2) so the block is `SUCCESS`.

`AllowFailure` is the one flowable whose block `retry:` egret has been *refusing*
rather than running — not because the behavior differs, but because the
interaction was unmeasured when #680 shipped. The measurement now exists, so the
refusal is removed.

> **Note on the `incr_counter` errors task.** The golden `runs` list does **not**
> contain a separate `incr_counter` run, yet `run_script` succeeds on attempt 2.
> Task 0 must read `oracle-exec/traces.json` for this flow and record in §6 the
> exact mechanism by which attempt 2 succeeds (whether the errors handler run is
> folded/elided in the golden shape, or the KV increment happens on a path the
> golden does not surface as its own run). The driver must reproduce the golden
> **shape as vendored** — do not invent an `incr_counter` run the golden omits,
> and never edit the golden to match egret.

## 3. The corpus-ratchet coupling — lift LAST, in ONE commit (the 9a lesson)

egret's whole-suite census (`tests/executor.rs` census tuple `(301, 279, 22)` =
total, matching, baselined; and the `PublishedButRefused` table in
`src/executor/mod.rs`) **couples plan + validate + driver**. `valids/retry-with-flowable-errors`
is currently a `PublishedButRefused` entry keyed on
`unsupported_allow_failure_retry`. The moment the validate refusal is lifted,
that flow publishes and the driver runs it — so if the driver does not yet
reproduce the golden, the suite goes red mid-feature.

**RULE (carried verbatim from 9a):** lift the refusal **last, in the same commit
the behavior is correct**. Until that commit, develop and test the executor
behavior through direct fixtures / `plan_stored` with the refusal still active,
so the corpus census stays unchanged and green.

**Blast radius of the lift** (every site that encodes the old refusal — grepped,
not trusted to a reviewer's enumeration, per the lift-refusal lesson). All must
change in the lift commit:

- `src/spec/validate.rs` — the refusal arm (~line 2307) and its unit test
  (`allow_failure_retry_body`, ~lines 6193-6235).
- `src/executor/mod.rs` — the self-probe (~line 4346, asserts the refusal) and
  the `PublishedButRefused { id: "valids/retry-with-flowable-errors", … }` entry
  (~line 4614) with its `cause`/`codes`.
- `src/executor/plan.rs` and `src/executor/allow_failure.rs` — doc comments
  describing the refusal (~plan.rs:607, allow_failure.rs:13) and the module
  header in validate.rs (~line 58).
- `oracle-exec/driver.rs` — the corpus driver's LIVE / refused counts for this
  flow.
- `tests/executor.rs` — the census tuple (`22 → 21` baselined, `279 → 280`
  matching, `21 + 280 = 301`) plus the `PublishedButRefused`/over-refusal lists.
- `tests/routes.rs` — `a_draft_with_an_allow_failure_saves_and_its_report_carries_the_error`
  (~line 362) asserts the refusal code; it must flip to assert the block now
  validates clean (keep a still-refused sibling, e.g. a genuinely invalid flow,
  as the control).

Task 0's grep of `unsupported_allow_failure_retry` across all of `crates/egret-workflow/{src,tests}`
AND `oracle-exec/` is the authoritative list; this section is the starting map,
not a substitute for the grep.

## 4. Decisions

- **D1 — A block `retry:` on AllowFailure is inherited by the failing child**,
  the block is never retried as a unit, and the block maps the child's final
  (post-retry) state via `allow_failure::block_state` — identical to the
  `Sequential` block retry egret already runs. Kestra parity; not a new rule.
- **D2 — The oracle is the vendored golden**, not a fresh live-image run.
  Task 0 reads `golden.json`/`traces.json` for `valids/retry-with-flowable-errors`
  and pins the executor/driver tests to that trace. A live re-measure is optional
  verification, never load-bearing.
- **D3 — No migration, no new API, no model field.** `retry:` is already a `Task`
  property; `AllowFailure` already carries `tasks:`/`errors:`/`finally:`.
- **D4 — The planner must keep an AllowFailure-with-`retry:` a flowable**
  (`TaskKind::AllowFailure`), not an opaque `Runnable`. The `retry.rs:52-54` note
  about an older planner flattening it may be stale since #680 — Task 1 verifies
  and fixes only if needed.
- **D5 — Lift the refusal last, in one commit, reconciling the whole blast radius
  of §3.** No "green on an expanded scoped bar" is trusted until the full grep is
  clean and the named binaries are on the bar.

## 5. Scope

**In:** lift the refusal; verify/produce the golden-matching run of AllowFailure
block retry; reconcile the corpus census + golden coupling; behavioral tests for
the exhausts-retry (→WARNING) and succeeds-late (→SUCCESS, the corpus case)
paths and the `warningOnRetry` interaction; docs.

**Out:** `retry:` on other flowables (already works); `CREATE_NEW_EXECUTION` vs
`RETRY_FAILED_TASK` beyond what the inherited policy already distinguishes;
anything in 9a (Pause) or later roadmap packages.

## 6. Divergences / measured facts

*(Task 0 fills this in from `traces.json` — in particular the `incr_counter`
mechanism of §2, and whether the exhausts-retry case maps to `WARNING` exactly
as the Sequential case does. Any divergence from Kestra is recorded here before
the refusal is lifted.)*

## 7. Testing

- **Lift-the-refusal:** an `AllowFailure` with a block `retry:` validates clean
  and plans as `TaskKind::AllowFailure` (not opaque).
- **Behavioral (pinned to the golden / measured terminals):** the corpus flow
  `valids/retry-with-flowable-errors` runs live and matches the golden; a
  fixture where the child **always** fails exhausts the retry and the block ends
  `WARNING`; `warningOnRetry` logs the retried attempts as warnings without
  changing the terminal.
- **Corpus green:** the full-suite census reconciles to `(301, 280, 21)`; the
  `PublishedButRefused` table no longer lists this flow.
- **Scoped runs MUST include `--test routes --test execution_timestamps`** (plus
  `--test executor` for the census) or the full `--workspace` CI is first to
  catch a stale-refusal file.

## 8. Risk

Low–medium. The executor machinery exists; the risk is the corpus coupling (§3),
which the 9a lesson addresses by lifting last in one reconciling commit, and the
`incr_counter`/attempt-2 subtlety (§2), which Task 0 resolves from the vendored
trace before any test asserts a terminal.
