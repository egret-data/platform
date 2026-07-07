# Data Governance — Policies P4 (Ranger-parity core) — Design

**Date:** 2026-07-06 · **Status:** Design — approved direction · **Branch:** `feat/data-governance-policies` (continues P1–P3)
**Program:** Sub-epic 1 of Data Governance, phase 4. Closes the highest-value gaps vs Apache Ranger's policy admin (from the `data-portal/ranger` research) for a Trino-first governance UI, without importing Ranger's heavyweight generality.

## Goal
Bring egret's Policies editor to functional parity with Ranger for the Trino table-access use case by adding: a real **resource selector** (multi-value + include/exclude + recursive + catalog-metadata autocomplete), **Allow/Deny/Exceptions** structure with a **deny-all-else** toggle, **description + labels**, an **inline enable/disable toggle** in the list, and a **fixed policy summary** that reflects every principal bucket.

## Background — Ranger research findings (condensed)
From reading `ranger/agents-common/.../RangerPolicy.java`, `security-admin/.../ServiceREST.java`, `ranger-servicedef-trino.json`, and the React UI (`security-admin/.../react-webapp/src/views/PolicyListing/`):
- Ranger `RangerPolicy` carries far more than egret models: `allowExceptions`/`denyExceptions`, `isDenyAllElse`, per-access `isAllowed`, `roles`, `delegateAdmin`, policy/item `conditions`, `validitySchedules`, `description`, `policyLabels`, `policyPriority`, security `zoneName`, `additionalResources`, `resourceSignature`, `options`.
- `RangerPolicyResource` supports `values: List` (multi-value), `isExcludes`, `isRecursive` — egret's model already has these fields (P2's `ResourceLevel`) but the UI only edits ONE value per level and never exposes the toggles.
- Ranger's Trino service-def defines **zero** `policyConditions` — so conditions are Ranger generality that does not pay off for Trino.
- Ranger's editor UX worth borrowing: **deny-all-else toggle that collapses the Deny side**, **Allow/Deny + Exceptions grouping**, **per-resource-level include/exclude + recursive toggles**, **resource value autocomplete (creatable + live lookup)**, and **inline principal columns that union all buckets**. Ranger's list view is **missing** an inline enable toggle (forces opening the editor) — egret should do better.

## Scope decisions (locked with the user)
- **IN (P4 core):** resource selector v2 (multi-value + exclude/recursive + catalog-metadata autocomplete); `allowExceptions`/`denyExceptions` + `isDenyAllElse`; `description` + `policyLabels`; inline enable/disable toggle in the list; fix `summarize_policy` to union all principal/permission buckets.
- **OUT (deferred to P5 / skipped):** **roles** (deferred — groups already cover most of egret's need); **per-access `isAllowed`** (dropped as redundant with the Deny bucket — YAGNI); `validitySchedules`, `delegateAdmin`, policy/item `conditions`, security `zones`, `policyPriority`, import/export, grant/revoke, version history (Ranger-bloat for Trino-only egret).
- **Autocomplete source:** catalog names from the engine-catalog config of engines bound to the profile (real metadata, DB); schema/table/column from distinct values already used in this service's policies (DB). Live warehouse-browser enrichment for schema/table/column is a documented **P5 stretch**. The frontend combobox **always** allows free-typing + wildcard `*`.

## Architecture
All three policy kinds already flow through `editor_to_ranger_json` (P3). P4 extends the DTOs + serializers, adds a resource-lookup and an enable-toggle endpoint, fixes the summary, and restructures the Access editor. Masking/Row-filter editors gain only `description`/`labels`. No new crates.

## 1. Backend — model + serializer (`crates/egret-controlplane/src/policies/ranger_json.rs`)
Extend `AccessPolicyInput`:
- `allow_exceptions: Vec<AccessItemInput>` (serde `allowExceptions`, default `[]`)
- `deny_exceptions: Vec<AccessItemInput>` (serde `denyExceptions`, default `[]`)
- `is_deny_all_else: bool` (serde `isDenyAllElse`, default `false`)
- `description: String` (default `""`), `labels: Vec<String>` (serde `policyLabels`? keep `labels` on the wire; map to Ranger `policyLabels`) (default `[]`)

`access_to_ranger_json` now emits `allowExceptions`, `denyExceptions` (via the existing `item_obj`), `isDenyAllElse`, `description`, `policyLabels`. When `is_deny_all_else` is true, `denyPolicyItems` + `denyExceptions` are forced empty (Ranger semantics: deny-all-else means "everything not allowed is denied").
`ResourceLevel` already carries `values`/`is_excludes`/`is_recursive` — no model change; the serializer already passes them through `resource_obj`. `MaskingPolicyInput`/`RowFilterPolicyInput` gain `description` + `labels` (emitted the same way).

## 2. Backend — summary fix (`ranger_json.rs` / `mod.rs`)
`summarize_policy` currently reads only the one `item_key(policy_type)` array, so a deny-only or exceptions-only access policy shows blank principals/permissions (confirmed live). Fix: for **access** policies, union principals + permissions across `policyItems`, `denyPolicyItems`, `allowExceptions`, `denyExceptions`; for masking/rowfilter keep the single array. Add `description: String` and `labels: Vec<String>` to `PolicySummary`. (Mirrors Ranger's `showGroupsOrUsersOrRolesForPolicy` union.)

## 3. Backend — endpoints (`crates/egret-controlplane/src/policies/mod.rs`)
- `GET /control-plane/policies/resource-lookup?service={svc}&level={catalog|schema|table|column}&catalog=&schema=&table=&q=` → `Json<Vec<String>>`. Resolution: **catalog** = distinct catalog names from `list_engine_catalogs` of engines whose `authz_service_id` = the service's id, unioned with distinct `catalog` resource values from the service's existing policies; **schema/table/column** = distinct values at that level from the service's existing policies, filtered so the parent levels match the passed `catalog`/`schema`/`table`. Case-insensitive `q` filter, capped (e.g. 50), sorted. Admin-gated.
- `PUT /control-plane/policies/{id}/enabled` (body `{ "enabled": bool }`) → toggles a policy's enabled flag without rewriting its body. Backed by a new `AuthzStore::set_policy_enabled(id, enabled)` that flips `enabled`, bumps the policy `version` + the service `policy_version`. Admin-gated. (Powers the list-row toggle.)

## 4. Frontend — resource selector v2 (`frontend/src/components/policy/ResourceLevelEditor.tsx`, new)
A reusable per-level control used by all three editors:
- A **Combobox** (shadcn `Popover` + `Command`-less list, or `Popover` + `Input` + filtered list — match the `PrincipalPicker` pattern) that is creatable (free-type + wildcard `*`) and shows suggestions from `resourceLookup(service, level, {catalog,schema,table}, q)`. Multi-value: selected values render as removable `Badge`s (like `PrincipalPicker`).
- Two shadcn `Switch`es to the right: **Exclude** (binds `isExcludes`) and **Recursive** (binds `isRecursive`), each with a tooltip. (Show Recursive always for now; Trino treats it as a near-no-op but the model carries it.)
- Cascading: the `catalog` value scopes the `schema` lookup, etc. (pass the parent values to `resourceLookup`).

## 5. Frontend — Access editor restructure (`AccessPolicyDialog.tsx`)
Widen the Dialog (or switch to a `Sheet`); sections top-to-bottom:
1. **Header:** Policy Name* (`Input`) + Enabled `Switch` + a `Popover` "…" holding **Description** (`Textarea`) + **Labels** (creatable tag multi-select).
2. **Resources:** four `ResourceLevelEditor`s (catalog/schema/table/column) in a card.
3. **Deny-all-else `Switch`** ("Deny everything else not explicitly allowed"). When on, the Deny tab is hidden/disabled.
4. **Allow | Deny `Tabs`.** Each tab body = a repeatable list of permission-item rows (principals via `PrincipalPicker` + access-type checkboxes from the service-def) **plus** a collapsed `Accordion` "Exceptions" holding its own repeatable item rows (`allowExceptions` under Allow, `denyExceptions` under Deny).
5. **Footer:** Save / Cancel (+ Delete via `AlertDialog`).
`toInput` reverse-map extends to read `allowExceptions`/`denyExceptions`/`isDenyAllElse`/`description`/`policyLabels` and multi-value resources.

## 6. Frontend — Masking/Row-filter + list
- `MaskingPolicyDialog`/`RowFilterPolicyDialog`: add Description + Labels in the same header "…" popover; swap their resource `Input`s for `ResourceLevelEditor` (masking = 4 levels, rowfilter = 3).
- `Policies.tsx` list: add an **inline enable `Switch`** per row (calls `setPolicyEnabled(id, next)` → invalidate); add a **Labels** column (badges) and show **Description** as a row tooltip/subtext; the summary now shows deny/exception principals correctly (from §2).
- `api/policies.ts`: extend the input types (allowExceptions/denyExceptions/isDenyAllElse/description/labels; resources stay `ResourceLevel` with values[]/isExcludes/isRecursive); add `resourceLookup()`, `setPolicyEnabled()`, `updatePolicyEnabled` types.

## 7. Testing
- **Backend unit:** `access_to_ranger_json` round-trips allowExceptions/denyExceptions/isDenyAllElse/description/labels + multi-value/exclude/recursive resources through `summarize_policy`; deny-all-else empties deny buckets; `summarize_policy` unions all four buckets (a deny-only policy now reports its principals).
- **Backend route (DB):** create a policy with exceptions + deny-all-else → list shows unioned principals; `resource-lookup` returns catalog names from a bound engine's catalogs + existing-policy values; `PUT /policies/{id}/enabled` flips the flag and the list reflects it; admin-gating on both new routes.
- **Frontend (Vitest):** `ResourceLevelEditor` adds/removes multi-values + toggles exclude/recursive; Access editor emits allowExceptions + isDenyAllElse; list enable `Switch` calls `setPolicyEnabled`.
- **Full local CI + manual smoke** (per `egret-run-full-ci-local-before-merge`, `egret-frontend-viz-verify`): drive the new editor against the running stack — create an Allow policy with an Exclude on a column + an exception; toggle deny-all-else; toggle enable in the list; confirm a deny-only policy shows its principals.

## 8. Phasing (within P4)
- **P4a:** model + serializer (§1) + summary fix (§2) + backend unit tests.
- **P4b:** resource-lookup + enable-toggle endpoints (§3) + route tests.
- **P4c:** `ResourceLevelEditor` + api extensions (§4) + Vitest.
- **P4d:** Access editor restructure (§5) + Masking/Row-filter + list (§6) + Vitest.
- **P4e:** full CI + smoke.

## 9. Risks
- **Editor complexity:** the Access dialog grows significantly; keep it a Dialog/Sheet with clear sections — don't replicate Ranger's 2000-line page. Exceptions default collapsed.
- **Resource-lookup source:** catalog metadata depends on engines being bound; degrade gracefully to existing-policy values + free-text so the picker is never empty/blocking.
- **`isDenyAllElse` semantics:** must force-empty deny buckets on serialize AND the editor must hide the Deny tab, or the two disagree.
- **Summary union** must not double-count principals (dedupe) and must still classify policy_type correctly.
- **Wire-format:** editor inputs stay camelCase (`isExcludes`/`isRecursive`/`isDenyAllElse`/`allowExceptions`/`policyLabels`); the P1 list summary stays snake_case (`policy_type`/`resource_path`/`is_enabled`, + new `description`/`labels`).

## 10. Out of scope (P5+)
Roles (principal registry + picker), per-access `isAllowed`, validity schedules, delegate-admin, policy/item conditions, security zones, policy priority/override, import/export, grant/revoke, version history, live warehouse-browser resource enrichment for schema/table/column.
