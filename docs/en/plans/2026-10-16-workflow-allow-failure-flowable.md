# AllowFailure flowable: implementation plan

**Spec:** `docs/en/specs/2026-10-16-workflow-allow-failure-flowable-design.md`. Read it first; this plan refers to its sections (§3 the Kestra behaviour, §5 the design, §6 the acceptance).

**Goal:** bank `valids/allow-failure`, `valids/allow-failure-with-retry` and `valids/finally-allowfailure` against the image, with each §5.1 row unit-tested, and no entry added to the ratchet.

**Branch:** `feat/workflow-allow-failure-flowable` from `origin/main`. A new worktree, not `egret-w7f`, which holds other work.

**Rules that apply:** the oracle is the acceptance test (spec §8). Never change `golden.json` to match egret. Run one cargo or test suite at a time. Never use the shared dev stack (`:5432`, `deploy/docker-compose.dev.yml`, shared buckets). DB-backed tests run on a private database.

## Task 1: measure the table on the image (§5.1, §3.2, §7)

Nothing is coded until this is done. The spec's reading of Java is from source; the pinned image is the reference.

- [ ] **Step 1: list the corpus flows to measure.** The three targets plus two controls: a `Sequential` with a failing child, and a failing task with a flag-only `allowFailure: true`.
- [ ] **Step 2: run each flow on the image** (`kestra/kestra@sha256:28f59bf175e1…`, a throwaway container, `server local`) through the same probe method the oracle uses. Record every state transition with its task run id. Do not reuse the checkout's jars.
- [ ] **Step 3: check the golden.** Compare the image's trace with `oracle-exec/golden.json` for the three targets. If they differ, the golden is wrong for this version and the difference is the first finding of the task, not a silent edit.
- [ ] **Step 4: measure §3.2's combination.** A block with `allowWarning: true` whose child is a `WARNING` (from a nested `allowFailure: true`), and the same block without it. Record the block's final state.
- [ ] **Step 5: write the measured table** into `docs/en/specs/…` §5.1 with a column "measured" beside the rows, and commit. If a row disagrees with §5.1, change §5.1 first, then code.

**Done when:** every row of §5.1 has a measured state, and §5.1 matches it.

## Task 2: the table as pure code, with tests (§5.1, §5.2)

- [ ] **Step 1: failing test first.** In `executor/flowable/allow_failure.rs`, a table test over the four rows of §5.1 plus the `allowWarning` combination from Task 1. Give it the children's terminal states and the flags; assert the block state. It fails because the function does not exist.
- [ ] **Step 2: implement the mapping as a pure function** `fn block_state(children: RunState, errors_ran: bool, allow_warning: bool) -> RunState`, next to `soften` in `executor/allow_failure.rs` or in the new module. It does not touch the plan or the state machine.
- [ ] **Step 3: mutation check.** Change each arm once (`FAILED` → `SUCCESS`; drop `allow_warning`; let `errors_ran` decide) and confirm a test fails for each. Restore after each.

**Done when:** the table test passes and every mutation is killed.

## Task 3: plan the construct (§5.2)

- [ ] **Step 1: failing plan test.** A flow with one `AllowFailure` holding two children plans as `TaskKind::AllowFailure { tasks }` with the two children; the `errors:` and `finally:` lists are carried.
- [ ] **Step 2: add the variant** to `TaskKind` in `executor/plan.rs`, mapped from the node in `flatten_flow` beside `Sequential`. Remove the refusal path for this node in `spec/validate.rs` (the `AllowFailure` arm of `walk_task`), keeping the nested-`errors:` walk.
- [ ] **Step 3: keep the `retry:` refusal** on a block (§5.3), with a code named in this task's commit message.
- [ ] **Step 4: update the tests that counted it opaque**: `executor/mod.rs` around 3390-3410, 3960-3970, 4140 and 4235. The corpus count (6) does not change. Each moved assertion says why in its comment.

**Done when:** `cargo test -p egret-workflow` passes the plan and validate tests, and the corpus counts are unchanged.

## Task 4: drive it (§5.1, §5.2)

- [ ] **Step 1: failing executor test** for the three targets' shapes: a child fails, its sibling is skipped, `errors:` run, `finally:` runs, the block is `WARNING`, the flow after it runs.
- [ ] **Step 2: the handler** `executor/flowable/allow_failure.rs`: call `sequential::drive` for the children, the errors and the finally, then apply Task 2's `block_state` to the terminal state. Add one dispatch arm in `flowable/mod.rs`.
- [ ] **Step 3: run the executor tests** in `crates/egret-workflow/tests/executor.rs` and the `executor::` unit tests, all green.

**Done when:** the executor test passes and the sibling is not run.

## Task 5: the oracle (§6.1, §6.5)

- [ ] **Step 1: run the oracle** with the command in `oracle-exec/README.md`, on the private database.
- [ ] **Step 2: check the three targets match the golden.** Any mismatch goes back to Task 1 Step 5 and the spec; the golden is not edited.
- [ ] **Step 3: remove the banked lines** from `oracle-exec/baseline.txt` (`valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure`), and update any count in its header comments that names them.
- [ ] **Step 4: confirm the ratchet moved down by exactly three** and added nothing.
- [ ] **Step 5: `valids/retry-with-flowable-errors`** stays baselined; its comment in `baseline.txt` names §2's retry non-goal.

**Done when:** the ratchet is three lower and the three entries match the image.

## Task 6: gates and commit (§6.6)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] `cargo test -p egret-workflow` (one suite at a time; DB-backed tests on the private database)
- [ ] The oracle run from Task 5, recorded with its counts.
- [ ] Commit by named paths, conventional message, `Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>`. Never `git add -A`.

## Task 7: docs

- [ ] Update `docs/en/specs/2026-09-01-workflow-programme-design.md` §9: the `AllowFailure` flowable row, and the list of what is still not true, with the date.
- [ ] Vietnamese reference of the spec and this plan in `docs/vi/` (convention; after the English is final).

## Out of scope

- Retry on a block (`valids/retry-with-flowable-errors`, spec §2).
- The `allowFailure:` flag (W8, `executor/allow_failure.rs`).
- The other three items of the programme's "still not true" list (WorkingDirectory, plugin namespace-file writes, KV bridge, KV masking).
