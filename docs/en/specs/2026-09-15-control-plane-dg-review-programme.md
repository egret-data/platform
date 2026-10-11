# Control Plane & Data Governance — review findings and programme

**Status:** review complete; SP1–SP4d-3 merged; **SP4d-4 is the last package** (designed 2026-10-11);
**SP5 dropped** from this programme by the user on 2026-10-11 (see §3)
**Date:** 2026-09-15
**Specs in this programme:**
- SP1 `docs/{en,vi}/specs/2026-09-15-engine-lifecycle-hardening-design.md`
- SP2 `docs/{en,vi}/specs/2026-09-15-governance-app-split-design.md`
- SP4d-3 `docs/en/specs/2026-10-10-classify-and-migrate-policies-sp4d-3-design.md` (merged #660)
- SP4d-4 `docs/en/specs/2026-10-11-sp4d-4-closing-the-review-design.md` — the closing package

## 1. What the user asked for

1. The audit log is not a data access/deny log; it is a system-wide log → move it out of Data Governance.
2. Deleting an engine returns "internal error" → review engine provisioning thoroughly.
3. DG policies and tags are two separate policy systems, which is confusing → research and redesign policies entirely.
4. Data Governance becomes its own menu next to Control Plane; a DG user need not be a system admin.
5. Infra gains **Catalogs**: connection config per catalog type (database, iceberg, …); warehouses move in (≈ iceberg catalogs); catalogs are what Trino/StarRocks provisioning attaches.
6. On top of catalogs, an egret-native data permission model:
   1. keep Ranger policies as today, for external Trino/StarRocks;
   2. an egret model (user/group × catalog objects × select/insert/delete/…) that generates the matching Ranger profiles, so external engines can integrate; engines provisioned by egret are bound to this default profile;
   3. extend to engines without Ranger (e.g. Postgres): egret checks the user's permission, runs the query with the catalog's credentials or blocks it; row filter / masking by rewriting SQL or post-processing results.

## 2. Findings

### 2.1 Audit log (item 1)

`audit.audit_log` (migration 0076) records two kinds: **decision** — every Cedar PDP call at the API layer (`egret-auth/src/pdp/audit.rs`), and **mutation** — role/grant, tag assign, workspace member, Data API, workflow and impersonation actions. It does **not** record engine data access (Ranger audit is disabled: `xasecure.audit.is.enabled=false`), logins, or config changes to engines, warehouses, storage, secrets or policies. The user's reading is correct: it is a system log. Gated by `audit:read`, which is non-grantable (platform_admin only).

### 2.2 Navigation and permissions (item 4)

- Sidebar mode is chosen purely by URL prefix `/control-plane` (`Sidebar.tsx:126`).
- A `data_steward` platform role already exists (policy:*, tag:*, certify, read on infra/identity).
- `tag:read` is missing from both copies of `CONTROL_PLANE_PERMS` (`Header.tsx`, `guards.tsx`): a user holding only tag permissions cannot enter at all.

### 2.3 Policies and tags are three systems, not two (item 3)

| System | Store | Enforced by | UI |
|---|---|---|---|
| Ranger resource policies (access / masking / row filter) | `policy.authz_services` (profile) + `authz_policies` | Ranger plugin inside Trino/StarRocks, polling egret `/service` | Policies → profile → tabs |
| Ranger tag policies | tag service `<engine>_tag`, tags from `catalog_tag_assignments` | Trino plugin only (StarRocks plugin has no tags) | Policies → profile → Tags tab; tagging in WarehouseDetail |
| Cedar classification clearance | `classification_policies`, `tag_assignments` (BI objects) | egret API layer | Classifications → tag detail |

Systems 2 and 3 share the same `tags` table and the same Classifications page but have unrelated semantics: clearance hides whole BI objects, it cannot mask a catalog column. Defects found (to fix inside SP4, not earlier):
- **Editing a tag policy always fails**: `update_policy` (`egret-policy/src/admin/mod.rs:1040-1060`) does not branch on tag kinds, while create does.
- Tag policies store the mutable, non-unique `tags.name`; Cedar keys on `tags.key`. Renaming a tag silently breaks Ranger tag policies.
- Principals are written sometimes as email (policy picker) and sometimes as username (auto-admin policies). *Inferred, to verify:* no group provider or usersync exists, so group rules may never match inside engines.
- *Inferred, to verify:* an engine bound to an existing, differently-named profile polls a service named after the engine.

### 2.4 Engine provisioning (item 2)

Root cause of the user's 500 and fifteen further defects — see SP1 §2.

### 2.5 Catalogs (item 5)

- **No catalog registry exists.** Every catalog is an `engine_mgmt.engine_catalogs` row owned by one engine; the same Postgres connection on two engines is two rows with two sets of secrets.
- StarRocks ignores every non-primary catalog row (only the primary `egret` catalog is wired by a background task).
- Warehouses belong to one workspace (`catalog.warehouses.workspace_id NOT NULL`).
- Storage connections are the only engine-independent, global connection registry today — the pattern to follow.
- The removed Polaris model (0051, dropped in 0054) had one global connection + workspace assignments: the closest precedent.

### 2.6 Enforcement capability (item 6)

- Workspace connections (`query.databases.engine`) support only `trino` and `starrocks`; there is **no Postgres/MySQL/DuckDB query connector**.
- **No SQL parser** (no sqlparser-rs), no table-reference extraction, no rewriter; only a hand-rolled scanner for the Data API.
- **No Rust policy evaluator**; egret stores and serves Ranger JSON, engines evaluate.
- Result sets carry no column lineage (`TabularResult{columns{name,ty},rows}`).
- All server-side execution paths converge on `build_connector` (`egret-query/src/execution/connector/mod.rs:140`) through three entry styles: `run_query` (SQL Lab, BI, alerts), `QueryExecPort` (Data API, workflows — principal-aware), and direct calls (schema browse, introspection, tester). Only `QueryExecPort` carries a real principal.
- **Notebooks bypass egret**: the kernel gets a per-user engine JWT and connects directly; Iceberg reads use `AllowAllAuthorizer`.

## 3. Programme

| SP | Scope | Depends on | State |
|---|---|---|---|
| **SP1** | Engine lifecycle hardening: delete/unassign preflight, k8s ordering, error logging, cleanup, derived secrets, update/catalog ordering, status, catalog `kind` CHECK | — | designed |
| **SP2** | Governance app split: `/governance/*` next to Control Plane; audit log → Control Plane → System | — | designed |
| **SP3** | Catalog registry under Infra: typed connections (database, iceberg, …), warehouses become iceberg catalogs, engines attach registry catalogs (Trino and StarRocks) | — | to design |
| **SP4** | One egret data-permission model over registry catalogs, replacing the three systems in §2.3; compiles to Ranger profiles; egret-provisioned engines bound to the default profile; hand-written Ranger profiles kept for external engines | SP3 | to design |
| **SP4d-4** | Closing the review: refuse and report overlapping mask/row-filter rules (409 naming the other rule); measure what an absent resource level grants on the real 2.5.0/2.8.0 jars and fix the editor or the grammar accordingly; register an EXTERNAL Trino/StarRocks engine (API + UI), bind it to a profile, declare its catalogs, show its Ranger plugin config; coordinate pruning for the SP4d-3 equivalence gate | SP4d-3 | designed 2026-10-11 |
| ~~**SP5**~~ | ~~Egret-enforced engines (Postgres first): connector, SQL parser, policy evaluator, filter/mask rewrite, principal plumbing through every execution path~~ | — | **dropped 2026-10-11** (user): egret's own enforcement engine will be a separate engine, peer to Trino and StarRocks — a different story, not part of this programme |

SP1 and SP2 are independent of each other and of SP3–SP5.

**Item 6.3 of §1 ("extend to engines without Ranger") is therefore NOT delivered by this programme.** §4's
positions on SQL-rewrite enforcement are kept below as the record of what was decided, for whoever designs
that separate engine; nothing in SP1–SP4d-4 depends on them.

## 4. Positions already taken for SP3–SP5 (to confirm when designing them; the SP5 ones are historical since 2026-10-11)

- **Masking and row filters must be applied by SQL rewrite, not by post-processing results.** Post-processing masks the projection only: `WHERE email = 'x'`, `GROUP BY email`, joins and `count(DISTINCT email)` still read the real value and leak it. Ranger in Trino also rewrites (it wraps the table reference). Post-processing is acceptable only as defence in depth.
- **The egret model is the source of truth; Ranger JSON is a compiled artefact.** A generated profile is read-only in the Ranger editor; hand-written profiles remain a separate kind.
- **Egret-side enforcement covers only queries that pass through egret.** Notebooks need a proxied SQL path (or must be refused) on engines without Ranger.
- The enforcement chokepoint is a `Connector` decorator built in `build_connector`, which first requires a principal (user id, username, groups) on every entry style.
