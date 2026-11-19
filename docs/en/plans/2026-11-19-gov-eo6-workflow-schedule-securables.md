# EO6 Workflow & Schedule Securables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make workflows and schedules (alerts + reports + dataapi_alerts) first-class egret-object securables with per-object sharing, exactly as dashboards/apps are — share one object with a user/group, enforced per-object by the PDP, surfaced in the Access tab.

**Architecture:** Two new `ObjectKind`s (`Workflow`; `Schedule` with kind string `"alert"`). `classify` promotes the existing `workflow:*`/`alert:*` keys to object grants. `shares.rs` maps the kinds to their schema-qualified tables; `has_object_grant_in_workspace`/`access_views.rs`/`effective.rs` admit and roll up object guests. The flow and schedule per-id handlers convert to `require_object`. Lists stay workspace-gated (matching dashboards/apps). Zero migrations (`object_type` is free text; keys already exist).

**Tech Stack:** Rust (egret-perm-ir, egret-bi, egret-workspace, egret-auth, egret-workflow, egret-automation), axum, sqlx 0.9; React/TS frontend; orval client.

## Global Constraints

- **Prefix every cargo command with `CARGO_TARGET_DIR=target-eo6`.** `target-eo6/` is untracked and must never be staged; stage by explicit path.
- **Test DB:** container `egret-eo6-pg` (postgres:16, port **5469**, db `egret_test`), `DATABASE_URL=postgres://egret:egret@localhost:5469/egret_test`. The MIGRATOR auto-applies migrations to scratch DBs.
- **Zero migrations.** `access_grants.object_type` has no CHECK; `workflow:*`/`alert:*` keys exist and are seeded. Do NOT add a migration unless a task proves one is unavoidable (then it is 0242, renumber-at-merge).
- **Kind strings are load-bearing.** `ObjectKind::Workflow.as_str() == "workflow"`; `ObjectKind::Schedule.as_str() == "alert"`. These are what `access_grants.object_type`, the `/objects/{kind}` routes, and `gate_kind` compare against. Do not invent `"schedule"`.
- **`dataapi_alert` gate is security-critical.** A schedule per-id handler that today calls `require_dataapi_endpoint_read` for `dataapi_alert` rows MUST keep doing so; an `alert` object grant must not bypass it.
- **Lists stay workspace-gated.** Do NOT add directly-shared objects to list endpoints — dashboards/apps don't, and it's out of scope.
- **`object_table` must be schema-qualified.** `workflow.flows`/`automation.schedules` are not in `analytics.`; callers that format `analytics.{table}` must drop the literal prefix once the function returns a qualified name. This affects `shares.rs` (`object_name`, `object_ref`) and `effective.rs` (its private `object_table` + rollup SQL).
- **`catalog_drift` test** (`egret-controlplane/tests/catalog_drift.rs`) scans the enforcing source files per key; new `:read`/`:manage` object-grant enforcement must live in files it already lists (`shares.rs`, workflow/automation routes). Run it.
- **Run `cd frontend && npm run typecheck`** after any OpenAPI change — the drift gates do not run `tsc`.
- Conventional Commits; `Closes #N` only in the PR body. Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Separate egret-data/docs PR owed at ship.

---

### Task 1: Vocabulary — ObjectKind + classify promotions

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs` (`ObjectKind` enum ~:15, `ALL` ~:27, `as_str` ~:38, `classify` ~:131, tests ~:183/:223)

**Interfaces:**
- Produces: `ObjectKind::Workflow` (`as_str`="workflow"), `ObjectKind::Schedule` (`as_str`="alert"); `classify("workflow:read"|"workflow:manage")` → `ObjectGrant{Workflow, Read|Manage}`; `classify("alert:read"|"alert:manage")` → `ObjectGrant{Schedule, Read|Manage}`; `workflow:use_secrets` stays `TypedEpPrivilege`.

- [ ] **Step 1: Write failing tests.** Extend the round-trip test and the classify table (~:223) with rows: `("workflow:read", Workflow, Read)`, `("workflow:manage", Workflow, Manage)`, `("alert:read", Schedule, Read)`, `("alert:manage", Schedule, Manage)`; and assert `classify("workflow:use_secrets") == Some(TypedEpPrivilege)`. Assert `ObjectKind::ALL.len() == 9` and both new variants round-trip via `from_str_opt`.
- [ ] **Step 2: Run to verify fail** — `CARGO_TARGET_DIR=target-eo6 cargo test -p egret-perm-ir egret_object`. Expected FAIL (no variants).
- [ ] **Step 3: Add variants + arms.** Add `Workflow` and `Schedule` to `ObjectKind`; bump `ALL: [ObjectKind; 7]` → `; 9]` and add both; add `as_str` arms (`Workflow => "workflow"`, `Schedule => "alert"`); in `classify`, add the four object-grant arms. Update the `:163-166` comment that lists `workflow`/`alert` as typed-EP (they are now object grants, except `workflow:use_secrets`).
- [ ] **Step 4: Run to verify pass** — `CARGO_TARGET_DIR=target-eo6 cargo test -p egret-perm-ir egret_object`. Also check `crates/egret-auth/tests/pdp_regression.rs` and `crates/egret-controlplane/tests/catalog_drift.rs` still compile/refer correctly (don't fix drift yet — Task 7).
- [ ] **Step 5: Commit** — `git add crates/egret-perm-ir/src/egret_object.rs` then `git commit -m "feat(perm-ir): workflow & schedule object securable kinds + classify"`.

---

### Task 2: Shares data layer — object_table (schema-qualified), actions, gate_kind

**Files:**
- Modify: `crates/egret-bi/src/shares.rs` (`object_table` ~:23, `manage_action` ~:45, `read_action` ~:58, `gate_kind` ~:81, `object_name` ~:117, `object_ref` ~:144, doc strings ~:312/:432/:696, unit test ~:760)

**Interfaces:**
- Consumes: Task 1 kinds.
- Produces: `object_table("workflow") == Some("workflow.flows")`, `object_table("alert") == Some("automation.schedules")` (schema-qualified); `read_action`/`manage_action` arms for both; `gate_kind` maps folder `entity_type` `"flow"`→`workflow` and `"alert"`→`alert`; `object_name`/`object_ref` use the qualified name (no `analytics.` literal).

- [ ] **Step 1: Write failing tests.** In the `shares.rs` test module (~:717): assert `object_table("workflow") == Some("workflow.flows")`, `object_table("alert") == Some("automation.schedules")`; `manage_action("workflow") == Some("workflow:manage")`, `read_action("workflow") == Some("workflow:read")`, `manage_action("alert") == Some("alert:manage")`, `read_action("alert") == Some("alert:read")`. Update the existing test (~:760) that pins `"alert"` as rejected — it is now accepted.
- [ ] **Step 2: Run to verify fail** — `CARGO_TARGET_DIR=target-eo6 cargo test -p egret-bi --lib shares`. Expected FAIL.
- [ ] **Step 3: Implement.** Add arms to `object_table` returning schema-qualified names; add `read_action`/`manage_action` arms; add a `gate_kind` mapping so folder `entity_type` `"flow"`→`"workflow"` and `"alert"`→`"alert"` resolve to the right `manage_action` (today both return `None`). **Fix the `analytics.` prefix**: in `object_name` (~:117) and `object_ref` (~:144) the SQL is `... FROM analytics.{table}`; change to `... FROM {table}` now that `object_table` returns the schema. Verify `workflow.flows` and `automation.schedules` both have `name`, `folder_id`, `workspace_id` columns (they do). Update the utoipa doc strings (~:312/:432) and `BiSharesApi` (~:696) kind lists.
- [ ] **Step 4: Run to verify pass** — `CARGO_TARGET_DIR=target-eo6 cargo test -p egret-bi --lib shares`.
- [ ] **Step 5: Commit** — `git add crates/egret-bi/src/shares.rs` then `git commit -m "feat(bi): share workflow & alert objects (schema-qualified object_table)"`.

---

### Task 3: Admission + shared-object rollups

**Files:**
- Modify: `crates/egret-workspace/src/rbac.rs` (`has_object_grant_in_workspace` ~:52)
- Modify: `crates/egret-workspace/src/access_views.rs` (the `ws_objects` rollup CTE ~:205)
- Modify: `crates/egret-auth/src/effective.rs` (private `object_table` ~:282 + rollups ~:301/:420/:425, schema-qualified)
- Test: `crates/egret-bi/tests/share_deny.rs` (~:392), `crates/egret-workspace/tests/shares_rollup_db.rs` (~:351)

**Interfaces:**
- Consumes: the new kind strings.
- Produces: a non-member holder of a `workflow`/`alert` object grant is admitted into `WsContext`; the shared-objects rollups include `workflow`/`alert` rows.

- [ ] **Step 1: Write failing test.** In `shares_rollup_db.rs`, add: grant a user an object grant on a workflow and on a schedule they otherwise can't see; assert both appear in the shared-objects rollup with kind `"workflow"`/`"alert"`. In `share_deny.rs`, add a case: a non-member with a workflow object grant is admitted (not 403 at `WsContext`) for that object.
- [ ] **Step 2: Run to verify fail** — `DATABASE_URL=… CARGO_TARGET_DIR=target-eo6 cargo test -p egret-workspace --test shares_rollup_db` (and `-p egret-bi --test share_deny`). Expected FAIL.
- [ ] **Step 3: Implement.** Add to `has_object_grant_in_workspace`'s OR-chain: `OR (ag.object_type='workflow' AND ag.scope_id IN (SELECT id FROM workflow.flows WHERE workspace_id=$2))` and the `automation.schedules` equivalent for `'alert'`. Add `UNION ALL SELECT 'workflow', id, name FROM workflow.flows WHERE workspace_id=$1` and `SELECT 'alert', id, name FROM automation.schedules WHERE workspace_id=$1` to the `access_views.rs` `ws_objects` CTE. In `effective.rs`, add arms to its private `object_table` (schema-qualified) and the `ObjectGrant`/`who_can_access` rollups (fix the `analytics.` literal as in Task 2).
- [ ] **Step 4: Run to verify pass** — the two test files above, plus `-p egret-workspace --lib` and `-p egret-auth --lib`.
- [ ] **Step 5: Commit** — stage the three src files + two test files; `git commit -m "feat(workspace): admit & roll up workflow/alert object guests"`.

---

### Task 4: Workflow per-id handlers → require_object

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (flow-scoped per-id handlers), `triggers/routes.rs`, `backfill_routes.rs`; use `repo.rs` (`FlowRow.folder_id`)
- Test: `crates/egret-workflow/tests/` (add a share-access test) or `crates/egret-bi/tests/shares.rs` pattern

**Interfaces:**
- Consumes: `shares::resolve`/`ObjectRef`/`require_object` (the dashboard/app precedent), Task 1–3.
- Produces: each flow per-id endpoint authorizes via `require_object(kind="workflow")` so an object-grant holder is allowed for that flow.

- [ ] **Step 1: Write failing test.** A DB test: user with only a `workflow:read` object grant on flow X (no workspace role) can `GET` flow X; cannot GET flow Y; with `workflow:manage` can update X. Model it on `crates/egret-bi/tests/shares.rs` / `crates/egret-apps/tests/http.rs`.
- [ ] **Step 2: Run to verify fail** — `DATABASE_URL=… CARGO_TARGET_DIR=target-eo6 cargo test -p egret-workflow <name>`. Expected FAIL (handlers are workspace-gated).
- [ ] **Step 3: Implement.** For each flow-scoped per-id handler (`get_flow` ~:740, `update_flow` ~:804, `delete_flow` ~:876, `restore_flow`, `purge_flow`, `validate_flow`, `publish_flow` ~:1132, `list_flow_versions`, `get_flow_version`, `get_flow_graph`, `start_flow_execution` ~:3143, `validate_execution_inputs`, and the trigger/backfill per-flow routes): resolve the flow id → `ObjectRef{kind:"workflow", id, folder_id: row.folder_id, workspace_id}` (use the repo), then replace the workspace `require(WF_READ|WF_MANAGE)` with `require_object(&*st.pdp,&ctx, WF_READ|WF_MANAGE, obj)`. Follow `crates/egret-apps/src/routes.rs` (`app_object_ref`/`object_ref_of`/`caps`) as the exact precedent; add a `flow_object_ref` helper. For **execution** routes keyed by execution id, map execution→flow (the execution row carries the flow) and check against the flow's ObjectRef. Replace `caller_may_manage` (~:1892) per-object (compare `apps::caps`). Keep `list_flows`/`create_flow`/namespaces/`*_unsaved_*`/storage/nsfiles workspace-gated.
- [ ] **Step 4: Run to verify pass** — the new test + `-p egret-workflow` suite.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): per-object PDP enforcement on flow handlers"`.

---

### Task 5: Schedule per-id handlers → require_object (dataapi_alert gate preserved)

**Files:**
- Modify: `crates/egret-automation/src/routes.rs` (per-id handlers), `repo.rs` (`ScheduleRepo`, `folder_id`)
- Test: `crates/egret-automation/tests/` (share-access + dataapi_alert-gate)

**Interfaces:**
- Produces: each schedule per-id endpoint authorizes via `require_object(kind="alert")`; `dataapi_alert` rows keep `require_dataapi_endpoint_read`.

- [ ] **Step 1: Write failing tests.** (a) user with only an `alert:read` object grant on schedule X can `GET` X, not Y; `alert:manage` lets them update X. (b) a `dataapi_alert` row with an `alert` object grant is STILL hidden if the user fails the Data API endpoint read gate.
- [ ] **Step 2: Run to verify fail** — `DATABASE_URL=… CARGO_TARGET_DIR=target-eo6 cargo test -p egret-automation <name>`. Expected FAIL.
- [ ] **Step 3: Implement.** For `detail` ~:596, `update` ~:634, `remove` ~:752, `runs` ~:778, `run_now` ~:824, `set_active` ~:892, `trash` ~:947, `restore` ~:969, `set_folder` ~:1011, `bookmark_add`: resolve id → `ObjectRef{kind:"alert", id, folder_id: row.folder_id, workspace_id}` (via `load_detail`/`ScheduleRepo`) and use `require_object(&*st.pdp,&ctx, READ|MANAGE, obj)`. Keep the existing `require_dataapi_endpoint_read` (~:481) for `dataapi_alert` rows IN ADDITION to the object check. Keep `list`/`create`/`smtp_*`/`slack_*` workspace-gated.
- [ ] **Step 4: Run to verify pass** — the new tests + `-p egret-automation` suite.
- [ ] **Step 5: Commit** — `git commit -m "feat(automation): per-object PDP enforcement on schedule handlers"`.

---

### Task 6: Frontend — ShareKind + ShareDialog on Workflows and Alerts pages

**Files:**
- Modify: `frontend/src/components/sharing/ShareDialog.tsx` (~:33 `ShareKind`)
- Modify: `frontend/src/routes/explore/Alerts.tsx` (add share action), the Workflows/flows list page (locate it), `frontend/src/routes/explore/settings/SharedObjects.tsx` (~:34 doc comment only)

**Interfaces:**
- Consumes: the regenerated client hooks (Task 7 runs after, but the hooks are generic over `kind`, so wiring can use the existing `useSharesGrantList(ws, kind, id)`).
- Produces: a Share action on each workflow and alert row opening `ShareDialog kind="workflow"|"alert"`.

- [ ] **Step 1: Add the kinds.** Extend `ShareKind` with `"workflow"` and `"alert"`.
- [ ] **Step 2: Wire Alerts page.** In `routes/explore/Alerts.tsx`, add a Share action per row opening `<ShareDialog ws kind="alert" id name folderId={row.folder_id ?? null} />`, following `routes/explore/Apps.tsx:568`.
- [ ] **Step 3: Wire Workflows page.** Find the flows list page (grep `routes/explore` for the workflows list; likely `Workflows.tsx` or under a workflow route) and add the same Share action with `kind="workflow"`.
- [ ] **Step 4: Typecheck** — `cd frontend && npm run typecheck`. Expected: 0 errors.
- [ ] **Step 5: Commit** — stage the touched `.tsx` files; `git commit -m "feat(frontend): share workflows and alerts via ShareDialog"`.

---

### Task 7: Regen openapi/client, drift gates, catalog_drift, full suite, fmt/clippy

**Files:** `openapi.json`, `crates/egret-bi/openapi.json` (if separate), `frontend/src/api/generated/**`, any test baselines.

- [ ] **Step 1: Regenerate** — `CARGO_TARGET_DIR=target-eo6 cargo run -q --bin egret -- openapi > openapi.json`, then `cd frontend && (ls node_modules/.bin/orval||npm ci) && npm run gen:api`.
- [ ] **Step 2: Frontend typecheck** — `cd frontend && npm run typecheck`. 0 errors.
- [ ] **Step 3: Drift gates** — `CARGO_TARGET_DIR=target-eo6 cargo test -p egret-server --lib committed_openapi_json_is_current` and `… generated_client_was_emitted_from_the_committed_spec`.
- [ ] **Step 4: catalog_drift + regressions** — `DATABASE_URL=… CARGO_TARGET_DIR=target-eo6 cargo test -p egret-controlplane --test catalog_drift`; update its key→file lists if it flags a new enforcement site. Run the EO2/EO3/EO4 differentials (`cargo test -p egret-auth cedar_differential pdp_source_hot_flip cedar_grant_hot_flip`).
- [ ] **Step 5: Full suites** — `DATABASE_URL=… CARGO_TARGET_DIR=target-eo6 cargo test -p egret-perm-ir -p egret-bi -p egret-workspace -p egret-auth -p egret-workflow -p egret-automation`.
- [ ] **Step 6: fmt + clippy** — `CARGO_TARGET_DIR=target-eo6 cargo fmt --all` and `cargo clippy --workspace -- -D warnings` (at least the touched crates).
- [ ] **Step 7: Commit** — stage regen + any baseline fixes by explicit path; `git commit -m "chore(eo6): regenerate openapi/client; green the suite"`.

---

## Self-Review

- **Spec coverage:** EO6-D1 → Task 1. EO6-D2 (kind `alert` covers all schedule rows) → Tasks 1–2. EO6-D3 (no new keys/migration; promote classify) → Task 1. EO6-D4 (folder cascade both; gate_kind flow/alert) → Task 2. EO6-D5 (per-object require_object; lists workspace-gated; dataapi_alert gate) → Tasks 4–5. EO6-D6 (admission + rollups + shares API + Access tab) → Tasks 3, 6. EO6-D7 (Cedar automatic; catalog_drift) → Task 7.
- **Placeholder scan:** file:line anchors and the app/dashboard precedent named for every conversion; exact kind strings and action keys given.
- **Type consistency:** `ObjectKind::Schedule.as_str() == "alert"` consistent across Tasks 1–6; `object_table` schema-qualified consistently used by Tasks 2–3; `require_object` + `ObjectRef{kind, id, folder_id, workspace_id}` pattern consistent Tasks 4–5.
- **Risk notes:** Task 4 is the largest (≈ a dozen flow handlers + executions mapping); Task 5 must not weaken the `dataapi_alert` gate. Both end with a per-object DB test proving an object-grant holder is allowed for exactly one object.
- **Owed outside this PR:** egret-data/docs update (new securable kinds in the sharing/Access docs).
