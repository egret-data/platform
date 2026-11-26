# Workflow Subflow parity (package #12) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use `- [ ]`.
> DRAFT in scratchpad — formalize to `docs/{en,vi}/plans/2026-11-26-workflow-subflow.md` on the #12 worktree (off main) before executing.

**Goal:** Honour the five Subflow keys egret ignores — `labels`, `inheritLabels`, `revision`, `scheduleDate`, `restartBehavior` — plus fix "LoopUntil Subflow child matched by position only".

**Architecture:** egret already runs `TaskKind::Subflow`. Each key is honoured at the Subflow spawn/restart seam (`worker/handlers/subflow.rs`, the child `NewExecution`, `restart::open`). labels/inheritLabels thread into the child labels; revision reuses the 7b flow-version machinery; scheduleDate reuses `executions.scheduled_date` + the scheduler due-gate; restartBehavior reuses the 7a/7b restart seam. Behaviour in Tasks 1–4; one reconcile commit (Task 5) moves the census.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first:** Task 0 pins semantics on local Kestra `96893b589` before any key is honoured.
- **D1** all five keys, one package. **D2** scheduleDate reuses `executions.scheduled_date` + due-gate. **D3** revision reuses 7b flow-version (default latest). **D4** restartBehavior: RETRY_FAILED default (7a) + NEW_EXECUTION via 7a/7b restart seam + the LoopUntil-position fix. **D5** migration-free expected (Task 0 confirms). **D6** census moves once (Task 5); Tasks 1–4 do not touch keys.rs/measure.rs/property-treatment.json.
- **LIFT-REFUSAL DISCIPLINE:** these are `ignored`→honoured census moves (no publish refusal to lift — the keys already parse), but still grep warning-assertion tests before Task 5. Behaviour tasks keep the census still.
- **Scoped runs** include the Subflow/executor/restart/routes/execution_timestamps suites (+ whatever Task 0 names); tick/worker-driving tests hold the file-local serialising mutex.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` in PR body only. `cargo clippy --all-targets -- -D warnings` (CI runs it; `cargo check` alone misses large_enum_variant/clippy lints) and `cargo fmt --all --check` before push; foreground builds.

---

### Task 0: Measure — Kestra Subflow semantics + egret seams (load-bearing, report only)

**Files:** none.

- [ ] **Step 1 — labels/inheritLabels.** Kestra's exact child-label composition: own `labels` (rendered), `inheritLabels` merging the parent execution's labels, system labels, and the ORDER; the render context for the label values.
- [ ] **Step 2 — revision.** How 7b's flow-version machinery resolves a flow at revision N (the function + table); the error for a missing revision; confirm egret stores revision history usable from the Subflow spawn path. Default = latest (current).
- [ ] **Step 3 — scheduleDate.** The exact egret seam that gates a child execution's start on `scheduled_date` (scheduler/worker); how a `wait:true` parent behaves across the delay; Kestra's handling of a past or invalid `scheduleDate`.
- [ ] **Step 4 — restartBehavior + LoopUntil-position.** Kestra's NEW_EXECUTION semantics (fate of the previously-started child, parent↔child linkage, how outputs resolve) vs RETRY_FAILED; the `restart::open` Subflow seam to extend; the exact locus of the "LoopUntil Subflow child matched by position only" bug (7a note) and how a child should be matched to its source by identity.
- [ ] **Step 5 — census + migration + lift map.** Post-#11 census baseline; the five keys' `measure.rs` read-set + `keys.rs` lift sites; a lift-refusal grep (warning-assertion tests/fixtures); migration verdict (expect free; next number if needed). Record a BASE commit.

---

### Task 1: `labels` + `inheritLabels` on the child execution

**Files:** Modify `crates/egret-workflow/src/worker/handlers/subflow.rs` (child `NewExecution` build); reuse `src/labels.rs`. Test: `crates/egret-workflow/tests/subflow*.rs` (the file Task 0 names).

**Interfaces:** Consumes Task 0 §1. Produces the child-label composition the restart path (Task 4) also honours.

- [ ] **Step 1 — Failing test.** A parent flow whose Subflow task declares `labels: {team: x}` and `inheritLabels: true`, parent execution carrying label `env: prod` → the child execution carries BOTH (order per Task 0 §1). A second case `inheritLabels: false` → child carries only its own labels.
- [ ] **Step 2 — Run → fail.**
- [ ] **Step 3 — Implement.** Render the Subflow `labels` against the task context; when `inheritLabels` renders true, compose with the parent execution's labels via `visible_labels` ordering (Task 0 §1); thread into the child `NewExecution.labels`. A label that fails to render is omitted (Kestra, as webhook 11c) unless Task 0 says otherwise.
- [ ] **Step 4 — Run → pass.**
- [ ] **Step 5 — Scoped + fmt. Step 6 — Commit** `feat(workflow): Subflow labels + inheritLabels on the child execution`.

---

### Task 2: `revision` — spawn a specific subflow revision

**Files:** Modify `worker/handlers/subflow.rs` (flow resolution) reusing the 7b version lookup. Test: subflow tests.

**Interfaces:** Consumes Task 0 §2 (the version-resolve function). Produces nothing downstream.

- [ ] **Step 1 — Failing test.** A subflow with two revisions; the parent's Subflow task sets `revision: 1` → the child runs revision 1's source; with no `revision` → runs the latest. A non-existent revision → the error Task 0 §2 pins.
- [ ] **Step 2 — Run → fail.**
- [ ] **Step 3 — Implement.** When `revision` is set, resolve the subflow source at that revision via the 7b machinery (Task 0 §2) instead of the latest; default unchanged.
- [ ] **Step 4 — Run → pass. Step 5 — Scoped + fmt. Step 6 — Commit** `feat(workflow): Subflow revision (spawn a specific subflow revision)`.

---

### Task 3: `scheduleDate` — delay the child start

**Files:** Modify `worker/handlers/subflow.rs` (set `scheduled_date` on the child) + confirm the start due-gate (Task 0 §3). Test: subflow tests.

**Interfaces:** Consumes Task 0 §3 (the start gate seam). Produces nothing downstream.

- [ ] **Step 1 — Failing test.** A Subflow with `scheduleDate` a short future datetime → the child execution is created but does NOT start until the date; after it passes, the child runs and the `wait:true` parent completes. A past `scheduleDate` → starts immediately (Task 0 §3).
- [ ] **Step 2 — Run → fail.**
- [ ] **Step 3 — Implement.** Render `scheduleDate`; set the child `NewExecution.scheduled_date`; rely on the existing start due-gate (Task 0 §3) so the child only runs when due. Ensure the parent's wait spans the delay.
- [ ] **Step 4 — Run → pass. Step 5 — Scoped + fmt. Step 6 — Commit** `feat(workflow): Subflow scheduleDate (delay the child start)`.

---

### Task 4: `restartBehavior` NEW_EXECUTION + LoopUntil child-by-identity

**Files:** Modify `src/restart*.rs` (the `restart::open` Subflow seam, 7a/7b) + `worker/handlers/subflow.rs`. Test: restart/executor/subflow tests.

**Interfaces:** Consumes Task 0 §4. The heaviest task — restart machinery.

- [ ] **Step 1 — Failing tests.** (a) A parent with a Subflow `restartBehavior: NEW_EXECUTION` that failed → restarting the parent spawns a FRESH child execution (not an in-place restart of the old one); the old child's fate + linkage per Task 0 §4. (b) Default `RETRY_FAILED` → unchanged (restart in place, 7a). (c) A LoopUntil whose body is a Subflow: a restarted/looped child is matched to its source by identity, not list position (the row #12 bug) — construct the case Task 0 §4 identifies.
- [ ] **Step 2 — Run → fail.**
- [ ] **Step 3 — Implement.** Thread `restartBehavior` into `restart::open`'s Subflow seam: NEW_EXECUTION spawns a fresh child (born via the normal Subflow spawn, linked to the parent task run) instead of reopening the existing child; RETRY_FAILED keeps 7a. Fix the LoopUntil child matching to key on child identity (Task 0 §4).
- [ ] **Step 4 — Run → pass all. Step 5 — Scoped + fmt (restart + executor + subflow + routes + execution_timestamps). Step 6 — Commit** `feat(workflow): Subflow restartBehavior NEW_EXECUTION + match looped child by identity`.

---

### Task 5: Reconcile — census, warnings, docs

**Files:** Modify `spec/schema/measure.rs` (Subflow read-set +5 keys), `spec/keys.rs` (lift_of for the 5 keys), `spec/schema/property-treatment.json` (regen), `spec/schema/README.md`. Create: egret-data/docs additions (separate PR, controller-handled).

**Interfaces:** Consumes Task 0 §5 lift map.

- [ ] **Step 1 — Read-set.** Add `labels`, `inheritLabels`, `revision`, `scheduleDate`, `restartBehavior` to the Subflow read-set in `measure.rs`.
- [ ] **Step 2 — Lift warnings.** Drop the Package(12) `lift_of` for these five Subflow keys. Update warning-assertion tests (grep first).
- [ ] **Step 3 — Regen census.** `EGRET_WRITE_PROPERTY_TREATMENT=1`, then re-run without. Five Subflow cells ignored→honoured; record the new aggregate (Task 0 pins expected numbers); update README Subflow row.
- [ ] **Step 4 — Run** `cargo test -p egret-workflow spec::keys` + census golden + routes/execution_timestamps green; `cargo fmt --all --check`.
- [ ] **Step 5 — Docs (egret-data/docs, separate PR).** Document the five Subflow keys + any Task-0 divergences (NEW_EXECUTION, scheduleDate). Bilingual.
- [ ] **Step 6 — Commit** `feat(workflow): lift Subflow key warnings + reconcile census (package #12)`.

---

## Self-Review notes

- **Spec coverage:** labels/inheritLabels → T1; revision → T2; scheduleDate → T3; restartBehavior + LoopUntil-position → T4; census/warnings/docs → T5; measurement → T0. D1–D6 are Global Constraints.
- **Risk:** T4 is the heaviest (restart machinery + the loop-position bug). If it over-runs, (a)/(b)/(c) are separable but kept one task for one reviewer gate; escalate if restart semantics fight the plan.
- **Census discipline:** Tasks 1–4 are behaviour only; Task 5 is the single census move.
- **Migration:** expected free (reuse scheduled_date/labels/version/restart); if Task 0 finds NEW_EXECUTION needs stored state, insert a migration task before Task 4.
