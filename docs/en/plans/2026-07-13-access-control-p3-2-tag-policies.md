# Access Control P3.2 — Data-plane tag policies (Ranger) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Design context: `docs/en/plans/2026-07-13-access-control-p3-abac-design-spike.md`. Depends on P3.1 (tag catalog).

**Goal:** Tag-based **column masking / row filtering / access** on the data plane — "mask every column tagged `pii` unless role `pii_reader`" — enforced by the Trino Ranger plugin. Tag a catalog column once; the policy governs it automatically.

## ⚠️ Verification split (read first)

Everything B1→P3.1 was provable with DB + Vitest. **P3.2's enforcement (actual masking) is done by the Trino Ranger plugin at query time and CANNOT be unit/CI-verified in this environment.** Therefore:

- **Layer 1 — CI-green:** catalog table/column tag targets + serve real `download_tags`. Unit-testable (payload shape) + Vitest. Ships verified.
- **Layer 2 — code CI-green, correctness live-only:** the Ranger **tag service-def** (`tag.json`), tag-service provisioning, tag-policy editor "by tag" mode, tag-policy store/serve. CRUD + UI unit-testable; the service-def JSON correctness is live-only.
- **Layer 3 — live-only:** `tagService` config wiring so the plugin pulls + applies tag policies. Not verifiable here.

**Merge policy for THIS PR:** get the whole branch CI-green, push, open the PR — then **HOLD for the user's live Trino+Ranger "browser smoke"** (per the Data-Governance epic's practice) before merge. Do **not** admin-squash-merge Layers 2–3 unverified; they touch the live data plane. Layer 1 could be split to merge independently if desired.

## Reuse surfaces (verified on main)

- `crates/egret-policy/src/store.rs` — `PolicyStore`: `upsert_service_def(id,name,def)`, `create_service(name,type,config)`, `get_service`, `create_policy(service_id,name,body)`, `enabled_policies_for_service` (injects id/version/isEnabled), `list_policies`. Tables `policy.authz_service_defs` / `authz_services` (id,name,type,config_jsonb,policy_version) / `authz_policies`.
- `crates/egret-policy/src/ranger/api/download.rs` — `download_policies` (serves `ServicePolicies{service_def, policies}`), `download_tags` (returns `ServiceTags::empty` — the wire-in point). `ServiceTags` DTO (`egret-policy-api/src/dto.rs`): `{ op, service_name, tag_version, tags: Map<tagId→def>, service_resources: Vec<Value> }` (camelCase).
- `crates/egret-policy/src/admin/ranger_json.rs` — how a policy body is built for `serviceType:"trino"` (`policyType` 0/1/2, `resources`, item buckets, `dataMaskInfo`/`rowFilterInfo`); `summarize_policy`. Tag policies mirror this with `resources: { tag: { values:[tagName] } }` under the tag service.
- `crates/egret-policy/src/service-defs/trino.json` — the Trino service-def; the tag service-def (`tag.json`) is authored from Ranger's `ranger-servicedef-tag.json` (dataMaskDef + rowFilterDef + a `tag` resource).
- P3.1: `tags` catalog + `TagRepo` (egret-bi). This phase adds a **catalog-resource** target (columns aren't uuid-keyed).
- FE: `Policies.tsx` / `PolicyEditorPage.tsx` / `components/policy/{ResourceSelector,MaskItemTable,RowFilterItemTable,PermissionTable}` + `components/ui/tag-combobox.tsx`; catalog browse (`WarehouseDetail.tsx` / catalog table+column browse) for the tagging UI.
- Migration: latest `0080`; next **`0081`**.

## Data model — migration `0081_catalog_tag_targets.sql`

Catalog columns/tables aren't uuid-keyed, so a separate assignment table (reuses the `tags` catalog):
```sql
CREATE TABLE catalog_tag_assignments (
    tag_id       uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    warehouse_id uuid NOT NULL,
    schema_name  text NOT NULL,
    table_name   text NOT NULL,
    column_name  text NOT NULL DEFAULT '',   -- '' = table-level tag
    assigned_by  uuid REFERENCES users(id),
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tag_id, warehouse_id, schema_name, table_name, column_name)
);
CREATE INDEX catalog_tag_assignments_res_idx ON catalog_tag_assignments (warehouse_id, schema_name, table_name);
-- a monotonic tag-store version per policy profile so the plugin's lastKnownVersion works:
ALTER TABLE policy.authz_services ADD COLUMN tag_version bigint NOT NULL DEFAULT 1;
```

## Tasks

### Task 1 (Layer 1): BE — catalog tag target model + repo + endpoints

**Files:** `migrations/0081_*`; `egret-metastore/src/lib.rs`; extend `egret-bi/src/tags.rs` (or a new `catalog_tags.rs`) with `CatalogTagRepo` (assign/unassign/list by resource, list all for a warehouse); catalog-browse tag endpoints (gated `warehouse:update` or a catalog-manage perm — mirror `warehouse_admin` gating). Tests.

- [ ] TDD the repo (assign/unassign/list-for-table/list-for-warehouse, table vs column level, cascade on tag delete + on warehouse delete) + the endpoints (`GET/PUT/DELETE` catalog resource tags, gated). Confirm the exact catalog-manage perm from `warehouse_admin/mod.rs`.
- [ ] Commit `feat(access): catalog table/column tag targets (mig 0081) + endpoints`.

### Task 2 (Layer 1): BE — serve real `download_tags`

**Files:** `egret-policy/src/ranger/api/download.rs` + a builder in egret-policy (reads `catalog_tag_assignments` — via a port/dep so egret-policy can read them; if a cross-crate read is awkward, expose a read fn from egret-bi/catalog or query the table directly from egret-policy with a shared `Db`). `dto.rs` `ServiceTags` gets a real builder.

- [ ] TDD: given seeded catalog tag assignments for a service's warehouse, `download_tags` returns a `ServiceTags` with `tags` (tag-def per distinct tag) + `service_resources` (each = `{serviceResource:{resourceElements:{catalog,schema,table,column}}, tagIds:[…]}`) and a `tag_version` matching the service; `lastKnownVersion` match → 304. **Assert the JSON shape** against a known-good Ranger `ServiceTags` fixture (the correctness of the shape is the unit-test target; live application is the user's smoke).
- [ ] Commit `feat(access): serve real download_tags from catalog tag assignments`.

### Task 3 (Layer 2): BE — tag service-def + tag-service provisioning

**Files:** `egret-policy/src/service-defs/tag.json` (authored from Ranger `ranger-servicedef-tag.json`); seed via `upsert_service_def`; a `create_tag_service` path + `tagService` config on the trino service. Tests where possible (seed + get + shape).

- [ ] Author `tag.json` (serviceType `tag`; `resources:[{name:"tag"}]`; `dataMaskDef` + `rowFilterDef` + `accessTypes` mirroring trino's). Seed it (id after trino's). Provision a tag service instance (one shared, or per-profile); set the trino service's `config_jsonb.tagService` (+ `ranger.plugin.tag.*` download config). **Flag clearly: the service-def + tagService config correctness is live-only.**
- [ ] Commit `feat(access): Ranger tag service-def + tag-service provisioning (live-verify)`.

### Task 4 (Layer 2): BE — tag-policy CRUD (store under the tag service)

**Files:** `egret-policy/src/admin/` — a tag-policy body builder (`resources:{tag:{values:[tag]}}`, `policyType` 0/1/2, mask/row/access items) + CRUD wired to `create_policy(tagServiceId, …)`; served automatically by `download_policies(tagService)`. Tests: build + round-trip + `summarize_policy` handles the `tag` resource.

- [ ] TDD the tag-policy body builder + that `enabled_policies_for_service(tagService)` returns them; extend `summarize_policy` so a `tag` resource surfaces as `tag: <name>`.
- [ ] Commit `feat(access): tag-based policy CRUD (mask/row/access under the tag service)`.

### Task 5 (Layer 1/2): FE — catalog resource tagging UI

**Files:** extend `TagEditor` for catalog resources (`api/tags.ts` catalog hooks); wire a "Tags" affordance into the catalog browse (table + column rows in `WarehouseDetail`/browse). Vitest.

- [ ] TDD + implement tagging a table/column from the catalog browse using the tag catalog; show chips.
- [ ] Commit `feat(fe): tag catalog tables/columns from the browse`.

### Task 6 (Layer 2): FE — Policies editor "by tag" mode

**Files:** `components/policy/ResourceSelector.tsx` (+ editor) gains a **tag** resource mode (pick a tag via the tag catalog instead of catalog/schema/table); the policy is created under the tag service. `Policies.tsx` shows tag policies. Vitest.

- [ ] TDD + implement: an editor "Tag policy" path (mask/row/access) that targets a tag; list shows `tag: <name>`.
- [ ] Commit `feat(fe): tag-based policy authoring in the Policies editor`.

### Task 7: CI verification + live-smoke handoff

- [ ] Backend: `cargo fmt --all --check`; `cargo clippy -p egret-policy -p egret-bi -p egret-policy-api --all-targets`; `cargo test` the touched crates (report counts). Frontend: `npm run lint && typecheck && test && build`.
- [ ] **Write the live-smoke checklist** for the user (in the PR): provision a warehouse + Trino engine with the Ranger plugin (per the manual-Trino-integration-test recipe); tag a column `pii`; create a tag mask policy "`pii` → MASK unless `pii_reader`"; query as a non-`pii_reader` → column masked; as `pii_reader` → clear. **Do NOT auto-merge; hold for the user's confirmation.**

## Non-Goals
- StarRocks tag enforcement (Trino-first, as the DG epic was).
- Tag inheritance/propagation (table→column beyond explicit), lineage propagation.
- key=value tags for row filters beyond a single tag value (basic tag row-filter only).

## Self-Review Checklist
1. Layer 1 (catalog targets + download_tags) is fully CI-tested; Layers 2–3 clearly marked live-verify.
2. `ServiceTags` payload shape asserted against a known-good fixture.
3. tag-store `tag_version` bumps on assignment change so the plugin re-polls.
4. No change to the existing trino resource-policy path (additive); `download_policies` for the trino service unchanged.
5. Merge held for live smoke.

## Follow-ups
- P3.3 Cedar BI ABAC. StarRocks tag parity. Tag propagation. `download_tags` caching/version-bump on catalog rename.
