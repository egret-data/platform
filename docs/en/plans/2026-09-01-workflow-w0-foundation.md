# Workflow W0 — Foundation & Renames Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `egret-workflow` crate pair with a Kestra-mirroring module tree and its first faithfully ported type, register the `workflows` entitlement, delete the two dead nav placeholders, and rename `egret-orchestrator` so it stops colliding with the workflow orchestrator.

**Architecture:** W0 ships no routes and no tables. It creates the seams the rest of the programme (W1–W12; the table ran to W9 when this plan was written, and W10, W11 and W12 were added on 2026-09-14) lands in: a workspace entitlement key that fails closed before any route exists, two crates whose module names mirror Kestra's Java packages, and a rename done as its own mechanical change so no later diff carries it.

**Tech Stack:** Rust 1.85 / edition 2021, axum + sqlx (not used in W0), React 18 + react-router 7 + Vitest, `just` recipes.

Spec: `docs/en/specs/2026-09-01-workflow-programme-design.md` (§7 fidelity rules, §9 packages).

## Global Constraints

- **Kestra source reference:** `data-portal/kestra` at commit `96893b589` (2.0.0-SNAPSHOT). Read ported code against this checkout only.
- **Kestra artifact reference:** `2.0.0-rc13` on Maven Central. Not used in W0.
- **Every ported file names its source on line one**, e.g. `// kestra: core/src/main/java/io/kestra/core/models/flows/State.java`. (§7 rule 5)
- **Rust module names mirror Java package names:** `models/`, `runners/`, `executor/`, `worker/`, `scheduler/`, `plugins/`, `storages/`, `queues/`, `repositories/`. (§7 rule 2)
- **Type names are kept verbatim from Kestra.** (§7 rule 3)
- Workspace crates are globbed by `members = ["crates/*"]` in the root `Cargo.toml`, but each crate must **also** get a `workspace.dependencies` path entry to be referenced by name.
- Feature entitlements are **absent-row-means-enabled** (`crates/egret-workspace/src/features.rs` module docs). Adding a key therefore needs **no migration**.
- Commit messages follow Conventional Commits. Do **not** merge; open the PR and stop (`egret-no-auto-merge-await-approval`).
- **Verification command for Rust work — corrected during execution of Task 1.** `just test-no-db` is
  NOT usable as a regression gate on this branch: `crates/egret-audit/tests/audit_store.rs:6` does
  `egret_test_db::url().await.expect("DATABASE_URL")`, and under `EGRET_ALLOW_DB_SKIP=1` that `url()`
  returns `None` **by design** (`egret-test-db/src/lib.rs:471`), so `.expect()` panics instead of
  skipping. `cargo test --workspace` stops at the first failing binary, so the recipe validates only
  3 of ~245 binaries and then exits 101. This is pre-existing and out of scope to fix here — the
  mechanism was verified directly, not assumed.

  Use instead, and compare the two logs rather than reading one:

  ```bash
  cd /Users/1pro-l4r8/Documents/github/data-portal/egret
  env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 cargo test --workspace --no-fail-fast \
    > /tmp/w0-<task>-after.log 2>&1; echo "EXIT=$?"
  grep -c '^test result:' /tmp/w0-<task>-after.log                        # binaries reached
  grep -E '^test result:' /tmp/w0-<task>-after.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'
  grep -E '^test .* FAILED$' /tmp/w0-<task>-after.log | sort -u > /tmp/w0-<task>-after.failed
  ```

  `sort -u` is load-bearing: the same test name appears in several binaries, so an un-deduplicated
  list compares 41 lines against a 26-line baseline and reports a spurious mismatch. Both sides must
  be deduplicated or neither.

  The gate is that `/tmp/w0-<task>-after.failed` is **identical** to the same file captured before the
  change (`diff` them). A count alone is not enough — the same number of failures with different names
  is a regression plus a coincidence. Baseline on this branch: ~245 binaries, 2425 passed / 41 failed.

  Additionally run the touched crate's own suite, which must be fully green:
  `cargo test -p <crate> --no-fail-fast`.
- Frontend verification: `npx vitest run <path>` from `frontend/`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-provisioner/**` (renamed from `crates/egret-orchestrator/**`) | unchanged behaviour; k8s engine provisioning |
| `crates/egret-workspace/src/features.rs` | add the `workflows` key + catalog entry + gate rule |
| `frontend/src/hooks/useWorkspaceFeatures.ts` | add `"workflows"` to `ALL_FEATURE_KEYS` |
| `frontend/src/components/layout/Sidebar.tsx` | delete the `Automate` and `Data Engineering` groups |
| `frontend/src/App.tsx` | delete the two `Placeholder` routes |
| `frontend/src/components/layout/__tests__/Sidebar.test.tsx` | update the heading/item/order assertions |
| `crates/egret-workflow-api/` | leaf contract crate; stands up empty (mirrors `egret-dataapi-api` at its Task 4) |
| `crates/egret-workflow/src/lib.rs` | module tree mirroring Kestra packages |
| `crates/egret-workflow/src/models/state.rs` | `State` — the first ported type, all 17 variants + predicates |

---

### Task 1: Rename `egret-orchestrator` → `egret-provisioner`

`egret-orchestrator` today means "provision Trino/StarRocks onto k8s". Standing next to a workflow orchestrator, that name costs a reader a pause every time. Doing it first and alone means no later diff has to carry it.

**Files:**
- Move: `crates/egret-orchestrator/` → `crates/egret-provisioner/`
- Modify: `crates/egret-provisioner/Cargo.toml:2`
- Modify: `Cargo.toml:33`
- Modify: `crates/egret-bin/Cargo.toml:39`, `crates/egret-engine-mgmt/Cargo.toml:27`
- Modify: `crates/egret-bin/src/main.rs`, `crates/egret-engine-mgmt/src/state.rs`, `crates/egret-engine-mgmt/src/engines.rs`, `crates/egret-engine-mgmt/tests/routes.rs` (38 `egret_orchestrator::` code references across these four)
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs` (doc comments only)

**Interfaces:**
- Consumes: nothing.
- Produces: crate `egret-provisioner`, Rust path `egret_provisioner::`. Every public item keeps its current name: `KubeClient`, `TrinoProvisioner`, `StarRocksProvisioner`, `EngineProvisioner`, `EngineStatus`.

- [ ] **Step 1: Confirm the current state compiles, so a later failure is attributable**

Run: `just test-no-db`
Expected: PASS. Record the test count from the summary lines; Step 6 must match it.

- [ ] **Step 2: Move the directory and rewrite the package name**

```bash
git mv crates/egret-orchestrator crates/egret-provisioner
sed -i '' 's/^name = "egret-orchestrator"$/name = "egret-provisioner"/' crates/egret-provisioner/Cargo.toml
```

- [ ] **Step 3: Rewrite every reference, code and comment**

Two spellings exist and both must change: the crate name `egret-orchestrator` (TOML) and the Rust path `egret_orchestrator` (code).

```bash
grep -rl 'egret-orchestrator\|egret_orchestrator' \
  --include='*.rs' --include='*.toml' Cargo.toml crates/ \
  | xargs sed -i '' -e 's/egret-orchestrator/egret-provisioner/g' \
                    -e 's/egret_orchestrator/egret_provisioner/g'
```

- [ ] **Step 4: Verify no reference survives**

```bash
grep -rn 'egret-orchestrator\|egret_orchestrator' --include='*.rs' --include='*.toml' Cargo.toml crates/ | grep -v target
```

Expected: **no output**. Any hit is a miss, not a false positive.

Note: do **not** verify this with `rtk proxy grep` — it swallows stdin and returns empty, which reads identically to "clean" (`rtk-grep-swallows-stdin`).

- [ ] **Step 5: Rewrite the prose that the mechanical pass cannot fix**

`crates/egret-provisioner/src/lib.rs` and `crates/egret-engine-mgmt/src/engines.rs` contain sentences using the word "orchestrator" as an English noun, which `sed` did not touch. Read both and reword to "provisioner" where the sentence means this crate. Leave the word alone where it means something else.

Run: `grep -rn 'orchestrat' crates/egret-provisioner/src crates/egret-engine-mgmt/src crates/egret-engine-mgmt-api/src`
Expected: every remaining hit is deliberate; note why in the PR body.

- [ ] **Step 6: Run the full check**

Run: `just test-no-db`
Expected: PASS, with the **same test count as Step 1**. A rename that changes the count has moved or dropped a test.

- [ ] **Step 7: Formatting and lints**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. (`cargo fmt` is part of `just check` and CI — `egret-cargo-fmt-ci`.)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(provisioner): rename egret-orchestrator to egret-provisioner

The crate provisions Trino/StarRocks onto k8s. Standing next to the workflow
orchestrator landing in W1-W9, the old name is a reading hazard. Mechanical:
no public item is renamed and no behaviour changes."
```

---

### Task 2: Register the `workflows` entitlement in the backend catalog

The rule must exist **before** the routes do. `features.rs`'s own module docs make the reason explicit for `alerts` and `data_api`: an unmatched prefix rule is inert, but a route that lands before its rule **fails open**.

**Files:**
- Modify: `crates/egret-workspace/src/features.rs` — `FEATURE_KEYS` (line ~32) and `FEATURES` (line ~117)
- Test: `crates/egret-workspace/src/features.rs` (the existing `#[cfg(test)]` module, which already holds `assert_eq!(from_catalog, FEATURE_KEYS)` at ~line 513)

**Interfaces:**
- Consumes: `Feature { key, label, description, rules }` and `const fn prefix(path: &'static str) -> GateRule`, both already in this file.
- Produces: the string key `"workflows"` in `egret_workspace::features::FEATURE_KEYS`, consumed by Task 3's frontend list and by W1's routes.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/egret-workspace/src/features.rs`:

```rust
#[test]
fn workflows_is_catalogued_and_gates_its_own_prefix() {
    // The key exists and is ordered last, after data_api (W0; the nav group
    // itself lands in W7).
    assert_eq!(FEATURE_KEYS.last(), Some(&"workflows"));
    assert!(is_known("workflows"));

    // And the gate actually claims the prefix. Asserting only the key would
    // pass with `rules: &[]`, which is the fail-OPEN shape this test exists
    // to forbid.
    let f = FEATURES
        .iter()
        .find(|f| f.key == "workflows")
        .expect("workflows in FEATURES");
    assert_eq!(f.rules.len(), 1);
    assert_eq!(f.rules[0].path, "workflows");
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p egret-workspace --lib features::tests::workflows_is_catalogued_and_gates_its_own_prefix`
Expected: FAIL — `assertion failed: left == right`, because `FEATURE_KEYS.last()` is `Some("data_api")`.

If instead it fails to **compile**, stop and fix that first: a mutation or a test that does not build looks exactly like a test that fails for the right reason (`a-mutation-that-fails-to-compile`).

- [ ] **Step 3: Add the key**

In `FEATURE_KEYS` (line ~32), append after `"data_api",`:

```rust
    "workflows",
```

- [ ] **Step 4: Add the catalog entry**

In `FEATURES` (line ~117), append after the `data_api` entry, keeping the array in the same order as `FEATURE_KEYS`:

```rust
    Feature {
        key: "workflows",
        label: "Workflows",
        description: "Author, schedule, and run multi-step workflows across engines, notebooks, and containers.",
        // Declared in W0, before any route exists, for the reason `alerts` and
        // `data_api` record above: an unmatched prefix rule is inert, but a
        // route that lands before its rule fails OPEN. W1's management routes
        // land under `/api/ws/{ws}/workflows/...` and are claimed on arrival.
        rules: &[prefix("workflows")],
    },
```

- [ ] **Step 5: Run the new test and the ordering guard together**

Run: `cargo test -p egret-workspace --lib features`
Expected: PASS, including the pre-existing `assert_eq!(from_catalog, FEATURE_KEYS)` — that guard is what proves `FEATURES` and `FEATURE_KEYS` did not drift.

- [ ] **Step 6: Run the route-level test that counts the catalog**

`crates/egret-workspace/tests/features_routes.rs:198` asserts the API returns `FEATURE_KEYS.len()` entries, so it follows the catalog automatically — but run it to confirm nothing else pinned the old count.

Run: `cargo test -p egret-workspace --no-fail-fast`
Expected: PASS, fully green.

Then the workspace-wide regression gate, per Global Constraints (**not** `just test-no-db`, which
fail-fasts on a pre-existing `egret-audit` panic after 3 binaries):

```bash
env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 cargo test --workspace --no-fail-fast \
  > /tmp/w0-t2-after.log 2>&1; echo "EXIT=$?"
grep -E '^test .* FAILED$' /tmp/w0-t2-after.log | sort -u > /tmp/w0-t2-after.failed
diff /tmp/w0-t1-step6.failed /tmp/w0-t2-after.failed && echo "IDENTICAL FAILURE SET"
```

Expected: `IDENTICAL FAILURE SET`. If `/tmp/w0-t1-step6.failed` does not exist, capture a baseline the
same way from `git stash` first — do not skip the comparison.

- [ ] **Step 7: Prove the test can fail (mutation)**

Temporarily change the new entry to `rules: &[]`, re-run `cargo test -p egret-workspace --lib features`, and confirm the new test **fails**. Restore it. A guard nobody has watched fail is not a guard (`assertions-that-survive-mutation`).

- [ ] **Step 8: Commit**

```bash
git add crates/egret-workspace/src/features.rs
git commit -m "feat(workflows): add the workflows feature entitlement

Declared before any route exists: an unmatched prefix rule is inert, but a
route landing before its rule fails open. No migration -- an absent row means
enabled."
```

---

### Task 3: Delete the two nav placeholders and add the frontend feature key

`Workflows` and `Pipelines` are `<Placeholder>` routes with no backend behind them. Removing the two items empties both the `Automate` and the `Data Engineering` groups, so both headings go too. The real nav group arrives in **W7**, not here.

**Files:**
- Modify: `frontend/src/App.tsx:206-207` (delete)
- Modify: `frontend/src/components/layout/Sidebar.tsx:46-55` (delete two groups), and the `Waypoints, Workflow` icon imports on line 7
- Modify: `frontend/src/hooks/useWorkspaceFeatures.ts:16`
- Test: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`

**Interfaces:**
- Consumes: `"workflows"` from Task 2's `FEATURE_KEYS`.
- Produces: `ALL_FEATURE_KEYS` containing `"workflows"`, read by `RequireFeature` (`frontend/src/components/explore/RequireFeature.tsx:17`) when W7 adds gated routes.

- [ ] **Step 1: Update the Sidebar tests first, and watch them fail**

In `frontend/src/components/layout/__tests__/Sidebar.test.tsx`:

Replace the `renders the five zone headings` test body:

```tsx
  test("renders the three zone headings", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByText("Explore")).toBeInTheDocument();
    expect(screen.getByText("Data API")).toBeInTheDocument();
    expect(screen.getByText("Manage")).toBeInTheDocument();
    // Automate and Data Engineering each held exactly one placeholder item
    // (Workflows, Pipelines). Both items are gone in W0; the real Workflows
    // group lands in W7, so neither heading should exist in between.
    expect(screen.queryByText("Automate")).not.toBeInTheDocument();
    expect(screen.queryByText("Data Engineering")).not.toBeInTheDocument();
    expect(screen.queryByText("Data Governance")).not.toBeInTheDocument();
  });
```

In `renders representative items from each zone`, drop `"Pipelines"` and `"Workflows"` from the label array so it reads:

```tsx
    for (const label of [
      "SQL Lab",
      "Endpoints",
      "Clients",
      "Activity",
      "Workspace settings",
    ]) {
```

and add, after the existing `queryByRole` assertions in that test:

```tsx
    // W0 removed both placeholders; nothing should link to them.
    expect(screen.queryByRole("link", { name: "Workflows" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Pipelines" })).not.toBeInTheDocument();
```

Delete the `Workflows` href assertion (the `toHaveAttribute("href", "/explore/eng/workflows")` block around line 152).

Rewrite the ordering test, which named `Automate` as the group after Data API:

```tsx
  test("Data API sits right after Explore and before Manage", () => {
    renderSidebar("/explore/eng/sql-lab");
    const dataApiHeading = screen.getByRole("button", { name: /^Data API$/i });
    const exploreHeading = screen.getByText("Explore");
    const manageHeading = screen.getByRole("button", { name: /^Manage$/i });
    expect(
      exploreHeading.compareDocumentPosition(dataApiHeading) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      dataApiHeading.compareDocumentPosition(manageHeading) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });
```

Also fix the test at line 116, `Alerts & Reports is an Explore module, not an Automate item`. `groupOf` is
`screen.getByRole("button", { name: ... }).parentElement` (line 113-114), so `groupOf("Automate")` will
**throw** once the heading is gone, rather than fail an assertion. Replace line 120 only:

```tsx
    expect(groupOf("Automate")).not.toContainElement(link);
```

with:

```tsx
    // Automate no longer exists as a group at all (W0 removed its only item).
    expect(screen.queryByText("Automate")).not.toBeInTheDocument();
```

Leave lines 117-119 and 121-123 of that test unchanged, and rename it to
`"Alerts & Reports is an Explore module"`.

- [ ] **Step 2: Run the suite and confirm it fails for the right reason**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/Sidebar.test.tsx`
Expected: FAIL — `Automate` and `Data Engineering` are still rendered, and the `Workflows`/`Pipelines` links still resolve.

- [ ] **Step 3: Delete the two Sidebar groups**

In `frontend/src/components/layout/Sidebar.tsx`, delete lines 46-55 — both entries of `EXPLORE_GROUPS`, verbatim including the comment:

```tsx
  {
    heading: "Automate",
    // Alerts moved to Explore (it is an entitlement-gated module now, listed in
    // EXPLORE_MODULES); Workflows is what remains of the original pair.
    items: [{ label: "Workflows", icon: Workflow }],
  },
  {
    heading: "Data Engineering",
    items: [{ label: "Pipelines", icon: Waypoints }],
  },
```

Then remove `Waypoints, Workflow,` from the `lucide-react` import on line 7. Leave every other icon alone.

- [ ] **Step 4: Delete the two routes**

In `frontend/src/App.tsx`, delete lines 206-207:

```tsx
          <Route path="pipelines" element={<Placeholder title="Pipelines" />} />
          <Route path="workflows" element={<Placeholder title="Workflows" />} />
```

Keep the `Placeholder` import on line 56 — line 303 (`data-lineage`) still uses it.

- [ ] **Step 5: Add the frontend feature key**

In `frontend/src/hooks/useWorkspaceFeatures.ts:16`:

```ts
export const ALL_FEATURE_KEYS: string[] = [...EXPLORE_MODULES.map((m) => m.key), "data_api", "workflows"];
```

`workflows` is deliberately **not** added to `EXPLORE_MODULES`: that array drives both the nav and `useDefaultExplorePath`'s landing redirect, and a module with no route would make an empty workspace land on a 404. W7 adds it there together with its pages.

- [ ] **Step 6: Run the Sidebar suite**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/Sidebar.test.tsx`
Expected: PASS.

- [ ] **Step 7: Run the whole frontend suite**

Run: `cd frontend && npx vitest run`
Expected: PASS. Any other test naming `Automate`, `Pipelines`, or `/explore/*/workflows` surfaces here — fix it in this task rather than leaving it for W7.

- [ ] **Step 8: Typecheck and lint**

Run: `cd frontend && npx tsc --noEmit && npx eslint src`
Expected: clean. `tsc` is what catches the now-unused `Waypoints`/`Workflow` imports if Step 3 missed them.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/App.tsx frontend/src/components/layout/Sidebar.tsx \
        frontend/src/components/layout/__tests__/Sidebar.test.tsx \
        frontend/src/hooks/useWorkspaceFeatures.ts
git commit -m "feat(workflows): remove the Workflows/Pipelines placeholders

Both were <Placeholder> routes with no backend, and each was the only item in
its group, so the Automate and Data Engineering headings go with them. The real
Workflows nav group lands in W7. Adds the workflows entitlement key to
ALL_FEATURE_KEYS, but deliberately not to EXPLORE_MODULES -- that array also
drives the landing redirect, and a module with no route would 404."
```

---

### Task 4: Stand up `egret-workflow-api` and `egret-workflow` with `models::State`

The crate pair is created together with its first genuinely ported type, so the task has a deliverable a reviewer can judge rather than two empty directories.

`State` is chosen first because §7 rule 3 makes it load-bearing and because it is where the fidelity discipline gets its first real test: the Java enum's **last** variant, `RESUBMITTED`, ends with `;` rather than `,`, so a line-oriented reading of `State.java` silently produces 16 variants instead of 17 — while `Type.isTerminated()` depends on it.

**Files:**
- Create: `crates/egret-workflow-api/Cargo.toml`, `crates/egret-workflow-api/src/lib.rs`
- Create: `crates/egret-workflow/Cargo.toml`, `crates/egret-workflow/src/lib.rs`, `crates/egret-workflow/src/models/mod.rs`, `crates/egret-workflow/src/models/state.rs`
- Modify: `Cargo.toml` (`workspace.dependencies`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: crate `egret-workflow` exposing `egret_workflow::models::State`, a `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]` C-like enum with 17 variants serialised `SCREAMING_SNAKE_CASE`, and the predicates `is_terminated`, `is_terminated_no_fail`, `is_terminated_in_error`, `is_created`, `is_running`, `only_running`, `is_failed`, `is_paused`, `is_breakpoint`, `is_retrying`, `is_success`, `is_killed`, `is_queued` — all `const fn(self) -> bool`. Crate `egret-workflow-api` exists and exposes nothing yet.

- [ ] **Step 1: Create the leaf contract crate**

`crates/egret-workflow-api/Cargo.toml`:

```toml
[package]
name = "egret-workflow-api"
version = "0.1.0"
edition.workspace = true
license.workspace = true

# Leaf contract crate for the Workflow seam (W0). Mirrors egret-dataapi-api /
# egret-query-api / egret-policy-api: no sqlx, axum, or heavy egret-* deps, so
# any cross-seam consumer can depend on the Workflow vocabulary cheaply.
#
# Deliberately EMPTY at W0. `WorkflowPort::workspace_of` belongs here, but it
# arrives with its first caller -- egret-dataapi-api records the rule this
# follows: "A trait method with no consumer is untested surface -- do not add
# one ahead of its caller."
[dependencies]
```

`crates/egret-workflow-api/src/lib.rs`:

```rust
//! `egret-workflow-api` — the Workflow seam's contract crate (W0).
//!
//! Mirrors `egret-dataapi-api` / `egret-query-api` / `egret-catalog-api`: a pure
//! leaf so any cross-seam consumer can depend on the Workflow wire vocabulary
//! cheaply.
//!
//! Empty at W0 by design. `WorkflowPort::workspace_of` — the "which workspace
//! owns this flow" question `egret-automation` will ask in order to alert on a
//! failed run — lands with that caller, not before it.
```

- [ ] **Step 2: Create the engine crate and its module tree**

`crates/egret-workflow/Cargo.toml`:

```toml
[package]
name = "egret-workflow"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true

[dev-dependencies]
# Unit-test only: proves `State` serialises exactly as Kestra spells it.
serde_json.workspace = true

# NOTE: no `egret-workflow-api` dependency yet. That crate is empty at W0, and
# an unused dep is noise a reviewer has to re-justify. It arrives in W1 with
# the first type this crate needs from it.
```

`crates/egret-workflow/src/lib.rs`:

```rust
//! `egret-workflow` — the workflow engine.
//!
//! The module tree mirrors Kestra's Java packages on purpose, so a reader can
//! move between this crate and the Kestra checkout without translating names.
//! See `docs/en/specs/2026-09-01-workflow-programme-design.md` §7.
//!
//! Source reference: `data-portal/kestra` at `96893b589` (2.0.0-SNAPSHOT).
//!
//! Modules arrive with their package:
//!
//! - `models`    — W0 (this crate's first type) and W1
//! - `runners`   — W2 (expression rendering)
//! - `executor`  — W3 (`advance()`, the pure state machine)
//! - `worker`    — W4 (claim loops, handlers, reaper)
//! - `plugins`   — W5 (handler registry)
//!
//! `models`, `runners`, and `executor` must never touch the database or the
//! network. That boundary holds all the semantic difficulty and is what makes
//! it testable without standing anything up (spec §6.5).

pub mod models;
```

`crates/egret-workflow/src/models/mod.rs`:

```rust
// kestra: core/src/main/java/io/kestra/core/models/
//! Flow, task, execution, and trigger models. No I/O.

mod state;
pub use state::State;
```

- [ ] **Step 3: Register both crates in the workspace**

In the root `Cargo.toml`, add to `[workspace.dependencies]`, after the `egret-automation` line:

```toml
egret-workflow-api = { path = "crates/egret-workflow-api" }
egret-workflow = { path = "crates/egret-workflow" }
```

`members = ["crates/*"]` already globs the directories; these entries are what let other crates name them.

- [ ] **Step 4: Write the failing test**

Create `crates/egret-workflow/src/models/state.rs` containing **only** the test module for now, so the test compiles against nothing and fails loudly:

```rust
// kestra: core/src/main/java/io/kestra/core/models/flows/State.java

#[cfg(test)]
mod tests {
    use super::State;
    use State::*;

    /// All 17, in Kestra's declaration order.
    const ALL: [State; 17] = [
        Created, Submitted, Running, Paused, Restarted, Killing, Success,
        Warning, Failed, Killed, Cancelled, Queued, Retrying, Retried,
        Skipped, Breakpoint, Resubmitted,
    ];

    /// The count is asserted on its own because the ONE variant most likely to
    /// be missed is the last: it ends with `;` in the Java enum, not `,`, so a
    /// line-oriented reading of State.java yields 16 and every predicate below
    /// still passes.
    #[test]
    fn has_all_seventeen_variants() {
        assert_eq!(ALL.len(), 17);
        assert!(ALL.contains(&Resubmitted));
    }

    #[test]
    fn predicates_match_kestra_exactly() {
        // State.Type.isTerminated()
        for s in [Failed, Warning, Success, Killed, Cancelled, Retried, Skipped, Resubmitted] {
            assert!(s.is_terminated(), "{s:?} must be terminated");
        }
        for s in [Created, Submitted, Running, Paused, Restarted, Killing, Queued, Retrying, Breakpoint] {
            assert!(!s.is_terminated(), "{s:?} must not be terminated");
        }

        // isTerminatedNoFail() -- note it EXCLUDES Failed/Killed/Cancelled.
        for s in [Warning, Success, Retried, Skipped, Resubmitted] {
            assert!(s.is_terminated_no_fail(), "{s:?}");
        }
        for s in [Failed, Killed, Cancelled] {
            assert!(!s.is_terminated_no_fail(), "{s:?}");
        }

        // isTerminatedInError()
        for s in [Failed, Killed, Cancelled] {
            assert!(s.is_terminated_in_error(), "{s:?}");
        }
        assert!(!Warning.is_terminated_in_error());

        // isCreated() covers Restarted too -- not obvious from the name.
        assert!(Created.is_created() && Restarted.is_created());
        assert!(!Submitted.is_created());

        // isRunning() covers Killing; onlyRunning() does not.
        assert!(Running.is_running() && Killing.is_running());
        assert!(Running.only_running() && !Killing.only_running());

        // isRetrying() covers the terminal Retried as well as Retrying.
        assert!(Retrying.is_retrying() && Retried.is_retrying());

        // The single-variant predicates.
        assert!(Failed.is_failed() && Paused.is_paused() && Breakpoint.is_breakpoint());
        assert!(Success.is_success() && Killed.is_killed() && Queued.is_queued());
    }

    #[test]
    fn serialises_as_kestra_spells_it() {
        assert_eq!(serde_json::to_string(&Success).unwrap(), "\"SUCCESS\"");
        assert_eq!(serde_json::to_string(&Resubmitted).unwrap(), "\"RESUBMITTED\"");
        let round: State = serde_json::from_str("\"KILLING\"").unwrap();
        assert_eq!(round, Killing);
    }
}
```

- [ ] **Step 5: Run it and confirm it fails to compile for the stated reason**

Run: `cargo test -p egret-workflow --lib`
Expected: FAIL to compile — `cannot find type State in this scope`. That is the intended failure. Do not proceed if the message is anything else.

- [ ] **Step 6: Implement `State`**

Insert above the `#[cfg(test)]` block in `crates/egret-workflow/src/models/state.rs`:

```rust
use serde::{Deserialize, Serialize};

/// A workflow execution's or task run's state.
///
/// All 17 variants are kept, including those egret's v1 never produces
/// (`Submitted`, `Paused`, `Restarted`, `Queued`, `Breakpoint`, `Resubmitted`),
/// because a missing variant is where semantics drift quietly — and because the
/// predicates below already depend on them. Spec §7 rule 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Created,
    Submitted,
    Running,
    Paused,
    Restarted,
    Killing,
    Success,
    Warning,
    Failed,
    Killed,
    Cancelled,
    Queued,
    Retrying,
    Retried,
    Skipped,
    Breakpoint,
    Resubmitted,
}

impl State {
    /// `State.Type.isTerminated()`
    pub const fn is_terminated(self) -> bool {
        matches!(
            self,
            Self::Failed
                | Self::Warning
                | Self::Success
                | Self::Killed
                | Self::Cancelled
                | Self::Retried
                | Self::Skipped
                | Self::Resubmitted
        )
    }

    /// `State.Type.isTerminatedNoFail()` — terminal *and* not an error. Not the
    /// negation of `is_terminated_in_error`: a non-terminal state is neither.
    pub const fn is_terminated_no_fail(self) -> bool {
        matches!(
            self,
            Self::Warning | Self::Success | Self::Retried | Self::Skipped | Self::Resubmitted
        )
    }

    /// `State.Type.isTerminatedInError()`
    pub const fn is_terminated_in_error(self) -> bool {
        matches!(self, Self::Failed | Self::Killed | Self::Cancelled)
    }

    /// `State.Type.isCreated()` — includes `Restarted`.
    pub const fn is_created(self) -> bool {
        matches!(self, Self::Created | Self::Restarted)
    }

    /// `State.Type.isRunning()` — includes `Killing`, which is still executing.
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Killing)
    }

    /// `State.Type.onlyRunning()` — `Running` and nothing else.
    pub const fn only_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// `State.Type.isFailed()`
    pub const fn is_failed(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// `State.Type.isPaused()`
    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }

    /// `State.Type.isBreakpoint()`
    pub const fn is_breakpoint(self) -> bool {
        matches!(self, Self::Breakpoint)
    }

    /// `State.Type.isRetrying()` — includes the terminal `Retried`.
    pub const fn is_retrying(self) -> bool {
        matches!(self, Self::Retrying | Self::Retried)
    }

    /// `State.Type.isSuccess()`
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// `State.Type.isKilled()`
    pub const fn is_killed(self) -> bool {
        matches!(self, Self::Killed)
    }

    /// `State.Type.isQueued()`
    pub const fn is_queued(self) -> bool {
        matches!(self, Self::Queued)
    }
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -p egret-workflow --lib`
Expected: PASS, 3 tests.

- [ ] **Step 8: Prove the count test can fail (mutation)**

Delete the `Resubmitted` variant and its four appearances in the predicates, then run `cargo test -p egret-workflow --lib`. Confirm `has_all_seventeen_variants` **fails**. Restore.

This is the specific mutation worth running: it reproduces exactly the mistake the source-reading pass made, and it must be caught by a test rather than by review.

- [ ] **Step 9: Cross-check the port against the source, by hand**

Open `kestra/core/src/main/java/io/kestra/core/models/flows/State.java` at the `public enum Type` block (around line 269) and read the 13 predicate bodies against the Rust. Confirm variant-for-variant. This is cheap and it is the only thing that catches a predicate that is self-consistent but wrong — the test above is written from the same reading, so it cannot catch that on its own.

- [ ] **Step 10: Full check**

```bash
cargo test -p egret-workflow --no-fail-fast
env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 cargo test --workspace --no-fail-fast \
  > /tmp/w0-t4-after.log 2>&1; echo "EXIT=$?"
grep -E '^test .* FAILED$' /tmp/w0-t4-after.log | sort -u > /tmp/w0-t4-after.failed
diff /tmp/w0-t2-after.failed /tmp/w0-t4-after.failed && echo "IDENTICAL FAILURE SET"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `egret-workflow` fully green, `IDENTICAL FAILURE SET`, fmt and clippy clean. Clippy is what
will object if `models/mod.rs` re-exports something unused. Do **not** use `just test-no-db` — see
Global Constraints.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml crates/egret-workflow-api crates/egret-workflow
git commit -m "feat(workflow): stand up egret-workflow crates with models::State

Module tree mirrors Kestra's Java packages (spec section 7 rule 2). State
carries all 17 variants and the 13 predicates ported from State.java verbatim.
RESUBMITTED is the last entry in the Java enum and ends with ';' not ',', so a
line-oriented read yields 16 -- has_all_seventeen_variants exists to catch that.

egret-workflow-api stands up empty on purpose: WorkflowPort::workspace_of
arrives with its first caller."
```

---

## Done when

- `cargo test --workspace --no-fail-fast` reaches ~245 binaries and its FAILED-test-name set is
  identical to the branch baseline; `egret-workflow` and `egret-workspace` are fully green;
  `cargo fmt --check` and `cargo clippy -D warnings` are clean.
- `cd frontend && npx vitest run && npx tsc --noEmit && npx eslint src` are clean.
- `grep -rn 'egret-orchestrator\|egret_orchestrator' --include='*.rs' --include='*.toml' Cargo.toml crates/` returns nothing.
- `egret_workspace::features::FEATURE_KEYS` ends with `"workflows"`.
- No `Workflows` or `Pipelines` link exists in the sidebar, and no route serves `/explore/:ws/workflows` or `/explore/:ws/pipelines`.

Open the PR and **stop**. Do not merge (`egret-no-auto-merge-await-approval`).

## Deliberately not in W0

No migration (entitlements are absent-row-means-enabled), no routes, no tables, no nav group, no `WorkflowPort` method. W1 brings the flow model and the `workflow.*` schema; W7 brings the nav group back with pages behind it.
