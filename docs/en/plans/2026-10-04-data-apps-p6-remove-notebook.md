# Data Apps P6 — Remove the Notebook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the Jupyter notebook stack — `egret-notebook`, `egret-kernel`, `kernel-image/`, the
notebook frontend, the notebook tables, and every `notebook:*` permission/sharing/workflow
reference — now that Data Apps (marimo) is its replacement and P5 (run mode) is merged. This is the
last phase of the whole Data Apps programme.

**Architecture:** Subtractive. Each part removes one layer, left compiling/building/testing green,
so the removal is reviewable and bisectable the same way the additive P1–P5 parts were. Order
matters: cross-cutting references first (so nothing else still calls into the crates being
deleted), then the crates themselves, then infra (images, Helm, CI), then the database last (the
only irreversible step).

**Tech Stack:** Rust (axum, sqlx), React/TypeScript, Helm, GitHub Actions, PostgreSQL.

## Global Constraints

- Rust: `cargo fmt`, clippy clean on touched crates (main still carries pre-existing clippy debt
  from a toolchain change — `egret-workspace`/`egret-bi`/`main.rs:276` — do not add to it, do not
  fix it in this plan).
- A deleted file is `git rm`, not emptied — do not leave a near-empty module as a placeholder.
- Commits are Conventional Commits, no `Closes #` in commit bodies (PR descriptions only).
- A dropped permission, sharing kind, or workflow task type is removed from EVERY place it is
  listed, not just its primary definition — each task below says where to check.
- **D25 (decided with the user, 2026-10-04): `egret.python`'s `notebook` reference is deleted
  outright, not re-pointed to an app version.** It has no runtime handler today (confirmed: zero
  real flows use it, only test fixtures; `handlers::script::Script` already asserts
  `reg.get("egret.python").is_none()`). Re-pointing it to run a published Data App would be new
  functionality (a workflow triggering an app), not a notebook-removal task, and is out of scope
  here. If that capability is wanted later, it is a fresh spec decision, not a P6 carry-over.
- `cargo test --workspace` is not run locally in this environment (disk headroom); rely on
  per-crate runs plus CI, the same discipline P4/P5 used.
- Every test removed because its subject no longer exists is removed with a comment-free diff that
  is obviously a deletion in review — not silently marked `#[ignore]`.

## File structure

Deleted entirely:
- `crates/egret-kernel/` (the whole crate — its own doc comment says what it is: "per-(user×workspace)
  Jupyter Kernel Gateway lifecycle"; nothing in it is used outside the Jupyter notebook path. D22,
  already decided in P5, is why Data Apps never widened this crate instead of building its own
  `AppPodLauncher` — confirms there is no "non-Jupyter half" to keep).
- `crates/egret-notebook/` (the whole crate).
- `kernel-image/` (the notebook kernel's Docker image sources).
- `.github/workflows/kernel-image.yml` (CI for the image above).
- `crates/egret-bin/src/kernel_launcher_k8s.rs` (implements `egret_kernel::KernelLauncher`; distinct
  from `apps_pod_k8s.rs`, which stays — that one serves Data Apps pods and has nothing to do with
  this).
- Frontend: `frontend/src/routes/explore/{Notebooks,NotebookPage,NotebookSettings}.tsx` + their
  `__tests__`, `frontend/src/components/notebook/`, `frontend/src/api/{notebooks,notebookSettings}.ts`
  + `frontend/src/api/__tests__/notebooks.test.tsx`, `frontend/src/api/generated/notebooks/`, every
  `frontend/src/api/generated/model/{notebook*,createNotebook,updateNotebook}.ts` (the orval output
  for the routes this plan removes — regenerated away in Task 6c-3, not hand-deleted up front).
- `deploy/helm/egret/templates/{namespace-kernels,rbac-kernels,networkpolicy-kernels}.yaml`.

Modified:
- `crates/egret-workflow/src/spec/registry.rs` (drop the `egret.python` `TaskTypeSpec` row),
  `crates/egret-workflow/src/refs.rs` (drop the `RefKind::Notebook` variant and its arm).
- `crates/egret-analytics-api/src/{port.rs,lib.rs}` (drop `NotebookDocsPort` — after the above, it
  has no consumer left: `egret-kernel` is deleted, `egret-workflow`'s `RefKind::Notebook` is gone).
- `crates/egret-core/src/rbac.rs` (drop the `notebook:read`/`notebook:write` `perm!()` rows).
- `crates/egret-bi/src/{shares.rs,routes.rs}` (drop the `"notebook"`/`"notebooks"` arms from the
  generic sharing-kind dispatch — the mirror image of adding `"app"` there in P2).
- `Cargo.toml` (workspace members), `crates/egret-bin/{Cargo.toml,src/main.rs}`,
  `crates/egret-server/{Cargo.toml,src/lib.rs}` (remove the dependency and all wiring).
- `frontend/src/App.tsx` (remove the 3 notebook routes + imports),
  `frontend/src/hooks/useWorkspaceFeatures.ts` (remove `"notebooks"` from `ALL_FEATURE_KEYS`; its
  own comment already says "until the notebook is removed" — this is that commit).
- `justfile` (`egret-kernel:dev` build/load references), `deploy/helm/egret/values.yaml` (drop the
  `kernel:` section), `deploy/helm/egret/templates/{backend-deployment,ingress}.yaml` (drop the
  kernel-conditional blocks — confirm exact blocks when touched; both files mix kernel and
  non-kernel content so neither is deleted outright).
- `openapi.json` (regenerated, not hand-edited).
- New migration `migrations/0166_remove_notebook.sql` (next free number — confirm still free when
  this task runs; another branch may have taken it).

---

# Part P6a — Decouple notebook from every cross-cutting system

Branch `feat/data-apps-p6a-decouple` off `main`. Nothing user-visible changes; the notebook UI and
API keep working exactly as before. This part only removes OTHER systems' knowledge of notebooks,
so Part P6c can delete the crates without anything else referring to them.

### Task 6a-1: Workflow — delete `egret.python` and `RefKind::Notebook`

**Files:**
- Modify: `crates/egret-workflow/src/spec/registry.rs` (the `TaskTypeSpec { r#type: "egret.python", ... }` row)
- Modify: `crates/egret-workflow/src/refs.rs` (the `RefKind::Notebook` variant, its resolver arm at
  the call into `NotebookDocsPort`, and its doc table row)
- Modify: `crates/egret-workflow/src/models/flow.rs` (the `type: egret.python` doc comment/example
  at line 468 — replace with a task type that still exists, e.g. `egret.sql`, so the comment stays
  truthful)
- Modify: `crates/egret-workflow/src/spec/validate.rs` (the fixture YAML at line ~2629 using
  `type: egret.python\n  notebook: nb` — replace with a `core.*`/`egret.sql` task so the fixture
  still parses after the type is gone)
- Modify: `crates/egret-workflow/src/worker/handlers/script.rs` (the assertion
  `assert!(reg.get("egret.python").is_none())` at line 1170 — this assertion's PREMISE no longer
  exists once the type is removed from `TASK_TYPES`; decide whether it still means anything once
  `egret.python` is absent from the registry entirely, and either delete it or restate what it now
  proves)
- Modify: `crates/egret-workflow/src/worker/claim.rs` (the doc-comment list at line ~799 naming
  `egret.python` among task types — drop it from the list)

- [ ] **Step 1:** Run `cargo test -p egret-workflow` first, before touching anything, to get a
  baseline (note the pass count).
- [ ] **Step 2: Remove the `egret.python` `TaskTypeSpec` row** from `TASK_TYPES` in `registry.rs`.
  Update its neighboring comment (the one explaining why `egret.script.python` took a different
  name because `egret.python` already existed) — that reasoning is now historical, not current;
  say so rather than deleting the paragraph outright, since it is also the record of a real past
  incident (a shipped type-name collision) worth keeping for the next person who wonders why the
  names differ.
- [ ] **Step 3: Remove the `RefKind::Notebook` variant** from `refs.rs`'s `RefKind` enum, its match
  arm (the one calling `NotebookDocsPort::get_summary`), and its row in the doc table at the top of
  the file. Fix the `for kind in [RefKind::DataApiEndpoint, RefKind::Notebook]` test loop at line
  ~245 — remove `RefKind::Notebook` from it (or whatever the compiler says needs fixing: this is a
  closed enum, so the compiler finds every remaining match site).
- [ ] **Step 4:** `cargo check -p egret-workflow` — let the compiler enumerate every remaining
  reference (non-exhaustive match arms, the `models/flow.rs` doc example, the `validate.rs`
  fixture). Fix each one forward (swap the example/fixture task type, do not comment it out).
- [ ] **Step 5:** `cargo test -p egret-workflow` — compare against the Step 1 baseline; every test
  that referenced `egret.python`/`RefKind::Notebook` should now either be deleted (if it tested
  exactly this, e.g. `script.rs`'s assertion) or pass unchanged (if it only incidentally touched the
  registry, e.g. anything iterating all task types generically).
- [ ] **Step 6:** Commit `feat(workflow): remove the egret.python task type and RefKind::Notebook (D25)`.

### Task 6a-2: Delete `NotebookDocsPort`

**Files:**
- Modify: `crates/egret-analytics-api/src/port.rs` (remove the trait)
- Modify: `crates/egret-analytics-api/src/lib.rs` (remove its re-export)

- [ ] **Step 1:** After Task 6a-1, confirm by `grep -rn "NotebookDocsPort" crates/` that the only
  remaining references are its definition (`egret-analytics-api`) and its implementation
  (`egret-notebook/src/docs_port.rs`, deleted whole-crate in Part P6c — leave it for now, this task
  only removes the trait side). If anything else still references it, stop and investigate before
  deleting — the file-structure section's claim that nothing else consumes it should be re-verified
  here, not assumed.
- [ ] **Step 2:** Remove the trait and its re-export.
- [ ] **Step 3:** `cargo check -p egret-analytics-api` — clean (nothing in this crate implements or
  calls the trait itself, so removing it should not cascade).
- [ ] **Step 4:** Commit `feat(analytics-api): remove NotebookDocsPort — no consumer after D25`.

### Task 6a-3: RBAC — remove the two notebook permissions

**Files:**
- Modify: `crates/egret-core/src/rbac.rs` (the `notebook:read`/`notebook:write` `perm!()` rows)
- Test: `crates/egret-auth/tests/pdp_regression.rs`, `crates/egret-controlplane/tests/catalog_drift.rs`
  (both reference `notebook:` strings per a repo-wide grep; each needs the specific line found and
  either updated to a permission that still exists or removed if it specifically asserted something
  about notebook permissions)

- [ ] **Step 1:** `grep -n "notebook:" crates/egret-auth/tests/pdp_regression.rs
  crates/egret-controlplane/tests/catalog_drift.rs` to see exactly what each asserts before
  touching `rbac.rs` — a regression test naming a permission by string will not fail to compile when
  the permission disappears (it is just a string), so these must be found by reading, not by letting
  the compiler find them.
- [ ] **Step 2:** Remove the two `perm!("notebook:read", ...)` / `perm!("notebook:write", ...)` rows
  from the `PERMISSIONS`/catalog list in `rbac.rs`.
- [ ] **Step 3:** Fix the two test files found in Step 1 forward (swap the asserted permission for
  one that still exists if the test's point survives that swap, e.g. `app:read`/`app:write`; delete
  the specific assertion if the test existed ONLY to cover notebook permissions and has no
  equivalent left to assert).
- [ ] **Step 4:** `cargo test -p egret-core -p egret-auth -p egret-controlplane` — green.
- [ ] **Step 5:** Commit `feat(rbac): remove notebook:read and notebook:write`.

### Task 6a-4: Sharing — remove the `"notebook"`/`"notebooks"` kind

**Files:**
- Modify: `crates/egret-bi/src/shares.rs` (the `"notebook" => Some("notebook:write")` arm, the
  `("notebook", "notebook:write")` entry in the kind table, and the explanatory comments naming it)
- Modify: `crates/egret-bi/src/routes.rs` (the `"notebooks" => Some(("notebook", "notebook:read",
  "notebook:write"))` arm)
- Test: whatever test exercises the generic sharing-kind table (the one P2's memory notes as
  `catalog_drift.rs` scanning `shares.rs` — check if that scan asserts every DB-known kind has a
  dispatch entry, which would make removing `"notebook"` from the Rust table fail until the DB side
  is removed too in Part P6e; if so, this task's removal must either wait for P6e or the drift
  check must be told `"notebook"` is intentionally retired before its DB row is gone)

- [ ] **Step 1:** Read `catalog_drift.rs`'s sharing-kind scan (the one mentioned in prior P2 work) to
  learn whether it compares Rust's dispatch table against a DB-side enum/table of kinds. If it does,
  this task's order relative to Part P6e's migration needs to be decided here, concretely, before
  writing the removal — do not guess; read the check.
- [ ] **Step 2:** Remove the `"notebook"` arm from `shares.rs`'s kind-to-permission dispatch and its
  table entry; remove `"notebooks"` from `routes.rs`'s kind-string dispatch. Update the two comments
  that name `"notebook"` as an example kind (one explains the generic action-permission mapping
  rule, one explains the generic `<kind>:read` rule) to use a kind that still exists, e.g. `"app"`.
- [ ] **Step 3:** `cargo test -p egret-bi` — green, or fixed forward per Step 1's finding.
- [ ] **Step 4:** Commit `feat(bi): remove the notebook sharing kind`.

### Task 6a-5: PR P6a

- [ ] `cargo fmt --all`, clippy on touched crates, full suites for
  `egret-workflow`/`egret-analytics-api`/`egret-core`/`egret-auth`/`egret-controlplane`/`egret-bi`.
  Open the PR against `main`; stop for the user's merge approval.

---

# Part P6b — Frontend: remove the notebook UI

Branch `feat/data-apps-p6b-frontend-ui` off `main` after P6a merges. The backend notebook API still
exists and still serves requests (deleted in Part P6c) — this part only removes the UI that calls
it, so the generated API client is deliberately left alone here and cleaned up in Part P6c once the
routes it was generated from are actually gone.

### Task 6b-1: Remove the notebook routes, pages, and components

**Files:**
- Delete: `frontend/src/routes/explore/{Notebooks,NotebookPage,NotebookSettings}.tsx` and their
  `frontend/src/routes/explore/__tests__/{Notebooks,NotebookPage,NotebookSettings}.test.tsx`
- Delete: `frontend/src/components/notebook/` (whole directory, including `NotebookHeader.tsx`, its
  test, and `monaco/notebookCompletion.ts`)
- Modify: `frontend/src/App.tsx` (remove the 3 `<Route path="notebook...">` lines and their 3
  imports)
- Modify: `frontend/src/hooks/useWorkspaceFeatures.ts` (remove `"notebooks"` from
  `ALL_FEATURE_KEYS`; replace the comment above it — it currently says the routes are "still gated
  … until the notebook is removed", which is no longer forward-looking once this commit lands)

- [ ] **Step 1:** `grep -rln "notebook" frontend/src --include=*.tsx --include=*.ts | grep -v
  __tests__ | grep -v generated` to get the full, current file list — the file structure section
  above is from this session's survey; re-run it here in case anything changed since.
- [ ] **Step 2:** Delete the files/directory listed above; remove the 3 routes + imports from
  `App.tsx`; update `useWorkspaceFeatures.ts`.
- [ ] **Step 3:** `grep -rn "notebooks\|Notebook" frontend/src/components/layout/` (the sidebar/nav)
  to confirm there is nothing left to remove there — per this session's survey, the nav link was
  already removed earlier ("Apps replaced it in the menu"); confirm rather than assume.
- [ ] **Step 4:** `npx tsc --noEmit` — the only remaining errors should be in files that import the
  now-deleted hand-written API wrappers (`@/api/notebooks`, `@/api/notebookSettings`) or the
  generated notebook client; if `tsc` reports errors OUTSIDE those two files, something in Step 1's
  list was missed.
- [ ] **Step 5:** Commit `feat(frontend): remove the notebook routes, pages, and components`
  (expect this commit to leave `tsc` red on the API-wrapper files on purpose — Task 6c-3 closes that
  gap once the backend routes are gone and the client is regenerated; say so in the commit body so a
  reviewer does not mistake it for an oversight).

### Task 6b-2: PR P6b

- [ ] Since `tsc`/`npx vitest run` cannot be fully green yet (the API wrapper files still import a
  client this part does not touch), this PR is reviewed on its diff and on `eslint`/`vitest` for the
  files it DOES touch, not on a clean full-repo `tsc`. Say this explicitly in the PR description.
  Open against `main`; stop for the user's merge approval.

---

# Part P6c — Delete the crates and their wiring

Branch `feat/data-apps-p6c-delete-crates` off `main` after P6b merges. This is the part where the
repository actually stops building a Jupyter kernel gateway. Everything should compile and test
green at the end — this is the part that finally closes the `tsc` gap Task 6b-1 left open.

### Task 6c-1: Delete `crates/egret-kernel` and its wiring

**Files:**
- Delete: `crates/egret-kernel/` (whole crate)
- Delete: `crates/egret-bin/src/kernel_launcher_k8s.rs`
- Modify: `Cargo.toml` (workspace `members`), `crates/egret-bin/Cargo.toml`,
  `crates/egret-server/Cargo.toml` (remove the `egret-kernel` dependency from each)
- Modify: `crates/egret-bin/src/main.rs` (remove: the `mod kernel_launcher_k8s;` declaration; the
  `impl egret_kernel::engine_conn::EngineConnResolver for EngineConnResolverImpl` block at line
  ~215; the `kernel_registry`/`kernel_launcher`/`kernel_manager`/`spawn_reaper` block around line
  ~1764–1799; the `KernelState` construction and its `notebook_kernels`/`proxy`/`active` fields
  around line ~1971–1991 — confirm each line range against the current file before editing; main.rs
  moves between parts)
- Modify: `crates/egret-server/src/lib.rs` (remove: the `pub kernel: Option<egret_kernel::KernelState>`
  field on `AppState`; the `.nest("/api", egret_kernel::kernel_router())` and
  `.nest("/api", egret_kernel::kernel_callback_router())` calls; the `doc.merge(egret_kernel::openapi())`
  call)

- [ ] **Step 1:** `cargo test -p egret-kernel` one last time before deleting, to have a final green
  baseline on record (not required to pass — just a sanity check that nothing was already broken
  before this removal, so any new failure elsewhere is attributable to this change).
- [ ] **Step 2:** `git rm -r crates/egret-kernel crates/egret-bin/src/kernel_launcher_k8s.rs`.
- [ ] **Step 3:** Remove the workspace member + the two `Cargo.toml` dependency lines.
- [ ] **Step 4:** `cargo check --workspace` — let the compiler enumerate every remaining reference in
  `main.rs` and `egret-server/src/lib.rs`; remove each one listed above. Do this as ONE commit (the
  trait impl, the launcher construction, and the router mounts are one seam — a partial removal
  leaves the binary in a state that compiles but is semantically confused about whether kernels
  exist).
- [ ] **Step 5:** `cargo check --workspace` again — clean.
- [ ] **Step 6:** Commit `feat(apps): delete egret-kernel — the Jupyter notebook runtime`.

### Task 6c-2: Delete `crates/egret-notebook`

**Files:**
- Delete: `crates/egret-notebook/` (whole crate)
- Modify: `Cargo.toml` (workspace members), and whichever of `egret-bin`/`egret-server`'s
  `Cargo.toml`s still list it (confirm with `grep -rln "egret-notebook" crates/*/Cargo.toml` — by
  this point in the plan it may already be gone if Task 6c-1's `cargo check` pulled it out as part
  of chasing `egret_kernel`'s own dependents; do not assume, check)

- [ ] **Step 1:** `grep -rn "egret_notebook" crates/ --include=*.rs` — after Task 6c-1, this should
  already be empty (the only consumer was `egret-kernel`'s `NotebookKernelStore`/`docs_port`
  wiring, which went with it). If anything remains, read it before deleting the crate out from
  under it.
- [ ] **Step 2:** `git rm -r crates/egret-notebook`; remove its workspace member + any remaining
  dependency line.
- [ ] **Step 3:** `cargo check --workspace` — clean.
- [ ] **Step 4:** Commit `feat(apps): delete egret-notebook — the notebook storage/runtime crate`.

### Task 6c-3: Regenerate the OpenAPI doc and the frontend client; close the Task 6b-1 gap

**Files:**
- Modify: `openapi.json` (regenerated)
- Delete: `frontend/src/api/{notebooks,notebookSettings}.ts`,
  `frontend/src/api/__tests__/notebooks.test.tsx`, `frontend/src/api/generated/notebooks/`, every
  `frontend/src/api/generated/model/{notebook*,createNotebook,updateNotebook}.ts`
- Modify: anything `npm run gen:api` changes elsewhere (expected to be nothing beyond the notebook
  files, since no other backend route changed)

- [ ] **Step 1:** With the backend no longer serving notebook routes, regenerate
  `openapi.json` the way this repo normally does (the same command P1–P5 used whenever a route
  changed — confirm the exact command in `justfile`/CI rather than guessing; it is not hand-edited).
- [ ] **Step 2:** `cd frontend && npm run gen:api` (never bare `orval` — it silently drops
  `encodeURIComponent` per this project's own prior incident). Check `git status`: if orval leaves
  stale generated notebook files behind instead of removing them (regeneration tools do not always
  delete files for endpoints that disappeared), delete them explicitly — do not leave a generated
  file for a route that no longer exists.
- [ ] **Step 3:** Delete the two hand-written wrapper files and the orphaned test.
- [ ] **Step 4:** `npx tsc --noEmit` — clean (this closes the gap Task 6b-1 left open on purpose).
- [ ] **Step 5:** `npx eslint .` — no new errors.
- [ ] **Step 6:** Commit `chore(frontend): regenerate the API client without the notebook routes`.

### Task 6c-4: PR P6c

- [ ] `cargo fmt --all`, clippy on touched crates, `cargo test -p egret-bin -p egret-server` (plus
  any crate whose `Cargo.toml` changed), `npx tsc --noEmit`, `npx vitest run`, `npx eslint .`. Open
  the PR against `main`; stop for the user's merge approval.

---

# Part P6d — Infra: kernel-image, CI, Helm, justfile

Branch `feat/data-apps-p6d-infra` off `main` after P6c merges.

### Task 6d-1: Delete the kernel image and its CI workflow

**Files:**
- Delete: `kernel-image/` (whole directory), `.github/workflows/kernel-image.yml`
- Modify: `justfile` (every recipe that builds/loads `egret-kernel:dev` — confirm the exact recipes
  with `grep -n "egret-kernel\|kernel-image" justfile` when this task runs; at least
  `deploy-kind-oidc`'s `kind load docker-image` line names it alongside the three images this plan
  keeps)

- [ ] **Step 1:** `git rm -r kernel-image .github/workflows/kernel-image.yml`.
- [ ] **Step 2:** Edit `justfile`'s recipes to drop `egret-kernel:dev` from every build/load list,
  without touching the other three images' lines.
- [ ] **Step 3:** `just --list` (or equivalent dry check) to confirm the file still parses as a
  justfile after the edits.
- [ ] **Step 4:** Commit `chore: delete the notebook kernel image and its CI workflow`.

### Task 6d-2: Remove the kernel Helm surface

**Files:**
- Delete: `deploy/helm/egret/templates/{namespace-kernels,rbac-kernels,networkpolicy-kernels}.yaml`
- Modify: `deploy/helm/egret/values.yaml` (drop the `kernel:` section — confirm its exact key range
  with `grep -n "^kernel:" -A30 values.yaml` when this task runs), `deploy/helm/egret/templates/
  {backend-deployment,ingress}.yaml` (drop only the kernel-conditional blocks within each — both
  files carry apps/kernel/other content side by side, so this is a targeted edit, not a file
  deletion; find each block with `grep -n "kernel" <file>` and read enough surrounding context to
  know where the `{{- if }}`/`{{- end }}` pair starts and ends before removing it)

- [ ] **Step 1:** `helm template deploy/helm/egret` with `kernel.launcher=kubernetes` set, BEFORE
  this task's edits, and save the output — this is the "before" to diff against, so the removal's
  blast radius is visible rather than assumed.
- [ ] **Step 2:** Delete the three dedicated template files; remove the `values.yaml` `kernel:`
  section; remove the kernel-conditional blocks from `backend-deployment.yaml`/`ingress.yaml`.
- [ ] **Step 3:** `helm template deploy/helm/egret` again (now `kernel.*` values do not exist, so
  this run uses whatever defaults remain) — it must render without error. Diff against Step 1's
  saved output and confirm every difference is an expected kernel-related removal, nothing else.
- [ ] **Step 4:** Extend `crates/egret-server/tests/deployment_probes.rs` with one assertion that no
  rendered template under `deploy/helm/egret/templates/` contains the string `kernel` (a `grep -rl`
  check over the raw template text, same style the file already uses for the apps Ingress/NetworkPolicy
  checks) — this is the test that would have caught a half-finished removal.
- [ ] **Step 5:** Commit `chore(deploy): remove the notebook kernel's Helm surface`.

### Task 6d-3: PR P6d

- [ ] `cargo test -p egret-server --test deployment_probes`, `helm template` sanity check from Task
  6d-2. Open the PR against `main`; stop for the user's merge approval.

---

# Part P6e — Database: drop the notebook tables and grants

Branch `feat/data-apps-p6e-migration` off `main` after P6d merges. **This is the only irreversible
part of P6** — once this migration runs against a real database, notebook content is gone. Flag
this plainly in the PR description; do not bundle it quietly with anything else.

### Task 6e-1: Write the removal migration

**Files:**
- Create: `migrations/0166_remove_notebook.sql` (confirm 0166 is still the next free number —
  `ls migrations/ | tail -3` — before writing; another branch may have taken it since this plan was
  written)

- [ ] **Step 1:** Read every notebook migration in order (`0037, 0038, 0039, 0042, 0046, 0047, 0049,
  0090, 0091`, plus whatever else `grep -rl notebook migrations/*.sql` finds once Part P6a–d are
  merged) to build the exact list of tables/columns/role_permissions rows that exist today. Do not
  reconstruct this from memory of what the migration NAMES suggest — read the current schema state,
  since later migrations may have altered what an early one created.
- [ ] **Step 2:** Write `0166_remove_notebook.sql` in dependency order: `DELETE FROM
  role_permissions WHERE permission IN ('notebook:read','notebook:write')` (or whatever the actual
  column/table is called — confirmed in Step 1) before dropping the permission catalog row if one
  exists in the DB (Task 6a-3 removed it from the Rust static catalog; check whether `rbac.rs`'s
  `perm!()` list is ALSO mirrored into a DB table that needs its own row deleted, or whether the
  catalog is Rust-only and the DB only holds `role_permissions` grants referencing it by string);
  then `DROP TABLE` for every notebook-specific table, child tables before parents (e.g. whatever
  0042/0090/0091 added before the `notebooks`/`notebook_folders` tables 0037/0038 created).
- [ ] **Step 3:** Run the migration against a disposable test database (`just reset-testdata` or
  equivalent) and confirm `cargo test --workspace` is not the vehicle for checking this (the
  compiled Rust code already has no notebook table references after Part P6a–c) — the check here is
  that the migration itself applies cleanly, forward, on a database that has all 165 prior
  migrations already applied.
- [ ] **Step 4:** Commit `feat(db): drop the notebook tables and their permission grants`.

### Task 6e-2: PR P6e

- [ ] State plainly in the PR description: this is the irreversible step; anyone running it against
  a database with real notebook content loses that content permanently; recommend an export/backup
  step for any deployment that has real data, as a note to the operator, not as code this plan
  builds (no such deployment exists yet for Data Apps' predecessor at the time of writing — confirm
  that is still true before merging, since it changes nothing about the diff but changes who needs
  to be told before it is applied for real). Open the PR against `main`; stop for the user's merge
  approval, and do not apply this migration against anything but a disposable/test database without
  being asked.

---

## Self-review against the spec

| Requirement (spec, programme summary) | Task |
|---|---|
| `egret-notebook` removed | 6c-2 |
| Jupyter half of `egret-kernel` removed | 6c-1 (the whole crate — confirmed no non-Jupyter half exists) |
| `kernel-image/` removed | 6d-1 |
| The notebook frontend removed | 6b-1, 6c-3 (generated client) |
| The notebook tables removed | 6e-1 |
| `notebook:*` removed | 6a-3 (permissions), 6a-4 (sharing kind) |
| `egret.python` re-pointed or marked unsupported — decided in P6's plan | 6a-1 (D25: deleted outright, user's explicit choice over the two options the spec left open) |

Gaps and honest limits, stated rather than hidden:
- **The exact `role_permissions`/permission-catalog DB shape for Task 6e-1 is not confirmed by this
  plan** — `rbac.rs`'s `perm!()` macro's relationship to any DB-side permission table was not traced
  to its full depth while writing this (a `grep` found the Rust catalog and the seeding migrations,
  not a full schema read). Task 6e-1 Step 1 is written to require reading the real schema before
  writing SQL, not to assume a shape.
- **`catalog_drift.rs`'s relationship to the sharing-kind table (Task 6a-4) is not confirmed** — if
  it enforces that every DB-known share kind has a Rust dispatch entry (and not just the reverse),
  removing the Rust side in Part P6a before the DB side is gone in Part P6e could make that check
  fail from P6a through P6d. Task 6a-4 Step 1 is written to read that check first and decide the
  real order before touching anything, rather than this plan guessing wrong up front.
- **Helm `values.yaml`'s exact `kernel:` key range and the exact kernel-conditional blocks inside
  `backend-deployment.yaml`/`ingress.yaml` are not line-numbered here** — confirmed only that they
  exist (`grep -n "^kernel:"` / `grep -rln kernel deploy/helm/egret/templates/`), not their exact
  boundaries, which is exactly what Task 6d-2 Steps 1–3 require finding and diff-checking rather
  than assuming.
- **No live kind verification task is included for P6.** Unlike P4e/P5e, P6 is pure removal with no
  new runtime behavior to prove live — the existing P4e/P5e live specs (which never touched
  notebooks) are the regression signal that nothing else broke. If any of them starts failing after
  P6c merges, that is the finding, not something this plan pre-builds a new live test to catch.
