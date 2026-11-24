# AllowFailure block retry — implementation plan (package 9b)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Lift `unsupported_allow_failure_retry`; let a block `retry:` on an `AllowFailure` run as on any other flowable (failing child inherits it, block maps the post-retry terminal via `allow_failure::block_state`), reconciling the corpus census + golden in the same commit.

**Architecture:** Retry inheritance (`retry.rs` `inherited_retry`), `is_retried_kind` (flowables never retried as a unit), and `allow_failure::block_state` (WARNING mapping) already exist. 9b removes the validate refusal and verifies/produces the golden-matching run, then reconciles the whole-suite census coupled to that refusal.

## Global Constraints

- Spec: `docs/en/specs/2026-11-25-workflow-allowfailure-retry-design.md` (VI mirror). D1–D5 bind.
- **No migration, no new API, no model field.** Chat VI; code/docs EN.
- Conventional Commits; `Closes #N` in PR body only; commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage by explicit path; never `git add -A`; never stage `target*/`. Never commit secrets/scratch files.
- `cargo fmt --all --check` before push. ONE heavy Rust build at a time; run builds FOREGROUND.
- **Scoped test runs MUST include `--test routes --test execution_timestamps --test executor`** (the census + two cross-cutting guards live there).
- **The oracle is the vendored golden** (`oracle-exec/golden.json`), never edited to match egret. **Lift the refusal LAST, in the same commit the behavior is correct** (§3 of the spec), reconciling the full blast radius.

## File Structure

- `src/spec/validate.rs` — remove the `unsupported_allow_failure_retry` arm (~2307) + its unit test (~6193-6235) + module-header note (~58). (Lift commit only.)
- `src/executor/plan.rs`, `src/executor/allow_failure.rs` — doc comments; verify planner keeps `TaskKind::AllowFailure`.
- `src/executor/mod.rs` — self-probe (~4346) + `PublishedButRefused` entry (~4614). (Lift commit only.)
- `src/executor/retry.rs` / `src/executor/allow_failure.rs` — likely NO behavior change; confirm the inherited-retry path reaches an AllowFailure child and the block maps the post-retry terminal.
- `oracle-exec/driver.rs` — LIVE/refused counts. (Lift commit only.)
- `tests/executor.rs` — census tuple `(301, 279, 22) → (301, 280, 21)` + `PublishedButRefused`/over-refusal lists. (Lift commit only.)
- `tests/routes.rs` — flip `a_draft_with_an_allow_failure_saves_...` to assert clean validation + keep a still-refused control. (Lift commit only.)
- `tests/` — a behavioral fixture test (child always fails → block WARNING) + warningOnRetry.

---

### Task 0: Worktree baseline + establish the oracle from the vendored trace

- Worktree `egret-afretry` (off origin/main post-9a, branch `feat/workflow-allowfailure-retry`) and containers egret-afretry-postgres :5470 / valkey :6470 are **already created by the controller** — just verify they are up and build the crate once.
- **Authoritative grep (load-bearing):** `grep -rn "unsupported_allow_failure_retry" crates/egret-workflow/src crates/egret-workflow/tests crates/egret-workflow/oracle-exec` and write the complete hit list to the report — this is the blast radius the lift commit must cover.
- **Read the oracle:** extract the `valids/retry-with-flowable-errors` entry from `oracle-exec/golden.json` and its detailed trace from `oracle-exec/traces.json`. Record in the spec §6: the exact run shape (set_kv 1 attempt; retry_block 1 attempt → SUCCESS; run_script 2 attempts FAILED→SUCCESS; execution …RETRYING…→SUCCESS) **and the mechanism by which attempt 2 succeeds given the golden omits a separate `incr_counter` run** (does the errors handler run fold into the child's retry path? is the KV write surfaced elsewhere?). No later test may assert a terminal this step has not grounded in the vendored trace.
- Baseline: run the scoped suite (`--test executor --test routes --test execution_timestamps`) and confirm it is green BEFORE any change (census `(301, 279, 22)`, the refusal active).

### Task 1: Produce/verify the golden-matching run with the refusal STILL active

- The refusal blocks `build_plan`, so drive the behavior through `plan_stored` / a direct `TaskKind::AllowFailure { retry: Some(...), .. }` fixture (the same seam 9a used for `pause_plan_gap`). Confirm:
  1. the planner yields `TaskKind::AllowFailure` for a block with a `retry:` (fix `plan.rs` only if it flattens to opaque `Runnable` — D4);
  2. the executor, driving that stored plan, reproduces the golden shape for `retry-with-flowable-errors` (child 2 attempts FAILED→SUCCESS via `inherited_retry`; block 1 attempt → SUCCESS via `block_state`; execution RETRYING→SUCCESS).
- If the machinery already does it with no behavior change, this task is fixtures + assertions only (expected, low risk). If it does not, make the **minimal** change in `retry.rs`/`allow_failure.rs` and re-verify against the golden.
- The corpus census is UNCHANGED this task (refusal still active) → scoped suite stays green. Commit.

### Task 2: Behavioral — exhausts-retry → WARNING, and warningOnRetry

- Still with the refusal active (drive via `plan_stored`/fixtures), add executor tests for the two cases the corpus flow does NOT cover:
  1. a child that **always** fails exhausts the block's `maxAttempts` → child FAILED → block **WARNING** (the `allow_failure::block_state` mapping) → execution continues/terminal per the Sequential-parity rule;
  2. `warningOnRetry: true` logs the retried attempts as warnings without changing the terminal.
- Pin each to the measured/parity terminal, not a guess. Keep the scoped suite green. Commit.

### Task 3: LIFT the refusal + reconcile the corpus in ONE commit

- Remove the `unsupported_allow_failure_retry` arm from `validate.rs` (+ its unit test + header note); let the block validate as a normal flowable (children/errors/finally recurse as today).
- Remove the `PublishedButRefused { id: "valids/retry-with-flowable-errors", … }` entry from `mod.rs`; update the self-probe (~4346) to assert the flow now plans/validates clean (not refused).
- Update `oracle-exec/driver.rs` LIVE/refused counts and `tests/executor.rs` census tuple to `(301, 280, 21)` (`21 + 280 = 301`), with a derivation comment in the same style as the existing ones.
- Flip `tests/routes.rs::a_draft_with_an_allow_failure_saves_...` to assert the block validates clean; keep a still-refused control (a genuinely invalid flow).
- Update doc comments (`plan.rs`, `allow_failure.rs`).
- Re-run the **grep** from Task 0 across `src/`, `tests/`, `oracle-exec/` — it must return only the lines this commit intentionally rewrote (e.g. changelog-style comments), zero live refusal assertions. Add every grepped test binary to the scoped bar and run it.
- The driver now reproduces the golden for `retry-with-flowable-errors` → the full census is green. Commit (this is the single lift+reconcile commit).

### Task 4: Docs + gates + PR

- `egret-data/docs` (Platform): note that `retry:` on an `AllowFailure` block is now supported — inherited by the failing child, the block itself is not retried, and the block maps a still-failed child to `WARNING`; update `flow-control.md` (AllowFailure section) and `error-handling.md` (retry-on-flowable paragraph) to drop the "AllowFailure block retry is refused" claim.
- Gates: `cargo fmt --all --check`, clippy, the full scoped suite (`--test executor --test routes --test execution_timestamps` + every binary the Task-0 grep named).
- Open the code PR (`Closes` the 9b issue if one exists) + the docs PR. Merge on green CI (NO `--admin`). This completes roadmap package #9.

## Self-review checklist (run when formalizing — DONE)

- D1 (parity) → T1/T2; D2 (vendored oracle) → T0; D3 (no mig/API) → Global Constraints; D4 (planner flowable) → T1; D5 (lift last, reconcile) → T3.
- The corpus coupling (`valids/retry-with-flowable-errors`) is handled by the 9a lift-last rule — T1/T2 develop behind the active refusal, T3 lifts and reconciles in one commit.
- No test asserts a terminal not grounded in the vendored golden/trace (T0 is load-bearing).
- The Task-0 grep (not a reviewer's enumeration) is the authoritative blast radius for T3.
