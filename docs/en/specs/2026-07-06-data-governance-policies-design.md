# Data Governance — Policies (Ranger, Trino-first) — Design

**Date:** 2026-07-06 · **Status:** Design — awaiting approval · **Branch:** `feat/data-governance-policies` (off main)
**Program:** Sub-epic 1 of the **Data Governance** menu (Policies ▶ then Data Lineage). This spec covers **Policies** only. Data Lineage (OpenLineage receiver + graph, Marquez as reference) is a separate later spec. Reference for this spec: the **Apache Ranger admin console** (repo `data-portal/ranger/`) and the native Trino Ranger plugin (`data-portal/trino/plugin/trino-ranger`, doc `security/ranger-access-control.md`).

## Goal
Build the **Policies** page under Control Plane so a platform admin can create/edit/delete Apache Ranger policies — **Access, Column-masking, and Row-filter** — for one or more query-engine **profiles**, and have Trino enforce them. Trino first; the editor is service-definition-driven so StarRocks drops in later with only a new service-def.

## Background (current state — verified)
- **`egret-authz` (`AuthzStore`)** already models the full Ranger surface and is **multi-service**: `list_service_defs / get_service_def / list_services / create_service / delete_service / create_policy(service_id, name, &json) / update_policy(id, name, &json, enabled) / delete_policy / get_policy / enabled_policies_for_service`. Every policy write **bumps `authz_services.policy_version`** (verified in `store.rs`), so a polling engine re-downloads automatically. Policies are stored verbatim as `policy_jsonb`; service-defs as `def_jsonb`.
- **`egret-ranger`** exposes the Ranger-Admin-REST-**compatible** surface, mounted at **`/service`** and gated by a **shared bearer token** (`auth.rs::require_bearer`) — this is the **machine-facing** endpoint the Trino plugin polls (`/service/plugins/policies/download/{service}`, `/service/tags/download/{service}`) plus admin CRUD. It is **not** session-auth and must not be called from the browser.
- **Trino service-def** is embedded and seeded via `egret_authz::seed_trino` / `TRINO_DEF_NAME = "trino"`; it declares `accessTypes` (select, insert, …), `dataMaskDef`, `rowFilterDef`, and resources `catalog → schema → table → column`. **No StarRocks def yet.**
- **`egret-controlplane`** is mounted under `/control-plane`, CSRF-protected, gated by **`auth.rs::require_platform_admin(&Principal)`** (`platform_admin` role). It follows a clean **module-per-domain** route pattern (`engines.rs`, `storage.rs`, `secrets.rs`, `warehouses.rs`). Engines are **Trino-only** today and carry **no link** to a Ranger service.
- **Identity**: `egret-auth` has `UserRepo.list` (paged) and a `groups` table (`groups`, `user_groups`); no principals list endpoint exists yet.
- **Frontend**: `Sidebar.tsx` already has a **Data Governance** group under `EXPLORE_GROUPS` with **Policies** (`ShieldCheck`) and **Data Lineage** (`Share2`), both routed to `<Placeholder>` in `App.tsx`. API layer is one `.ts` file per domain over `apiFetch` (`client.ts`) + react-query hooks. Control-plane routes are gated `RequireGlobalAdmin`.
- **Trino Ranger is native**: the `trino-ranger` plugin ships in the standard Trino distribution — **no custom image / jar bundling**. It is enabled with `etc/access-control.properties` (`access-control.name=ranger`, `ranger.service.name=<service>`, `ranger.plugin.config.resource=ranger-trino-security.xml`) + a `ranger-trino-security.xml` whose `ranger.plugin.trino.policy.rest.url` points at egret's `/service` base URL. The default **dev** Trino currently uses `access-control.name=file` (`deploy/trino/etc/access-control.properties`); real Ranger enforcement is exercised today only by the Java harness `tests/ranger/` (`EgretRangerEnforcementTest`, ranger-plugins-common 2.4.0).

## Decisions (locked with the user)
1. **Policies first**, Data Lineage second (separate spec).
2. **Scope = Global / Control Plane**, `platform_admin` only. The menu item **moves** from the workspace Explore sidebar to Control Plane.
3. **Full 3 policy types** — Access + Column-masking + Row-filter — with a **service-def-driven** editor.
4. **Principals** (users/groups/roles) come from **egret's own** users + groups via a picker (combobox), not free text.
5. **Multi-profile**: a *profile* = one Ranger **service** (`authz_services` row) with an **independent lifecycle**. An engine links to a profile (**many engines → one profile**, nullable). On provision the admin **creates a new profile** (default) **or links an existing** one — so several Trino clusters can share one policy set. Deleting an engine **keeps** the profile + policies (standalone, relinkable).
6. **Enforcement boundary = Layer A now + Layer B config-artifact** (see §5). No Trino image changes.

## Architecture — chosen approach
A new **session-authenticated, platform-admin** policy API inside `egret-controlplane` that reads/writes the **same `AuthzStore`** as the machine-facing `/service` router. Rejected: calling `/service` from the browser (bearer/machine auth, no RBAC, leaks Ranger camelCase wire format); a proxy that forwards into the ranger router (needless indirection). Because both paths share one store, a policy created in the UI is picked up by the Trino plugin through the existing `/service/plugins/policies/download/{service}` with no extra plumbing (version bump handles cache invalidation).

## 1. Backend — new module `egret-controlplane/src/policies.rs`
Mounted under `/control-plane`, CSRF + `require_platform_admin`, following the `storage.rs`/`secrets.rs` shape. Routes:

- `GET  /control-plane/policies?service={name}&type={access|masking|rowfilter}&resource={path}&q={text}` — list. Parses each `policy_jsonb` into a **typed summary DTO** (`id, name, policyType, service, resourcePath, principals[], permissions[], isEnabled, updatedAt`) rather than returning raw Ranger JSON. Filtering by service is required; type/resource/q are optional.
- `POST /control-plane/policies` — create. Accepts a **typed editor DTO** (see §3), **serializes it to valid Ranger policy JSON** (`policyType` 0/1/2; `resources{catalog,schema,table,column}`; `policyItems | dataMaskPolicyItems | rowFilterPolicyItems`), resolves the service, and stores via `AuthzStore::create_policy`.
- `GET  /control-plane/policies/{id}` — fetch one, mapped back to the editor DTO for editing.
- `PUT  /control-plane/policies/{id}` — update (same DTO→Ranger serialization) + `isEnabled` toggle.
- `DELETE /control-plane/policies/{id}` — delete.
- `GET  /control-plane/policies/services` — list profiles (services) available to manage, each with the engine(s) that link to it (0..n: id/name/status) and its type; supports create/link from the profile UI.
- `GET  /control-plane/policies/service-def/{name}` — return the service-def slices the editor renders from: `resources`, `accessTypes`, `dataMaskDef.maskTypes`, `rowFilterDef`. **This is what makes the editor def-driven and engine-agnostic.**
- `GET  /control-plane/principals?q={text}&kind={user|group}` — typeahead over egret users + groups (new thin `list`/search on `UserRepo` + `groups`), returning `{kind, name, displayName}`.

The **Ranger↔DTO (de)serialization** is the core logic and lives in a small, unit-tested submodule (e.g. `policies/ranger_json.rs`) with round-trip tests for all three policy types.

## 2. Profile ↔ engine binding (Layer A)
A *profile* (Ranger service) has its **own lifecycle**, decoupled from any engine. The engine→profile link is **many-to-one, nullable** (`ON DELETE SET NULL`): several engines may enforce the **same** policy set, and a profile survives its engine.
- **Migration**: add `authz_service_id BIGINT NULL REFERENCES authz_services(id) ON DELETE SET NULL` to the engines table (nullable — pre-existing engines and the seeded standalone `trino` service stay valid). No `UNIQUE` on the column (sharing is allowed). Remember to touch a `metastore` source file so the embedded migrator re-embeds (see the `sqlx migration staleness` note).
- **On engine provision** (in `engines.rs`): the request carries a **profile choice** —
  - `create` (default): `AuthzStore::create_service(name = engine slug, type = "trino", configs)`, seed the `trino` service-def if absent, set `engines.authz_service_id`; or
  - `link`: set `engines.authz_service_id` to an existing service id (validated: exists + type matches the engine kind). No new service created.
  Idempotent (skip if the engine is already linked).
- **Relink / detach**: an engine's governance settings can change `authz_service_id` (link a different profile, or detach = null) after provision.
- **On engine delete**: the FK's `ON DELETE SET NULL` (or explicit unlink) **keeps** the profile + all its policies; the service becomes standalone and **relinkable** by another engine. Never cascade-delete the service.
- The Policies UI **profile selector** lists services from `/control-plane/policies/services`; each option shows **which engine(s) use it** (0..n) and standalone services (used by none, e.g. the seeded `trino`) so nothing is orphaned.
- StarRocks: out of scope now; when the engines module gains StarRocks provisioning, the same choice creates/links a `starrocks`-type service against a `seed_starrocks` def — **the UI needs no change** because it is def-driven.

## 3. Frontend
- **Menu move** (`Sidebar.tsx`): remove **Policies** from the `EXPLORE_GROUPS` "Data Governance" group; add a **Data Governance** group to `CONTROL_PLANE_GROUPS` containing **Policies** (`ShieldCheck`). Leave **Data Lineage** where it is (Explore) as the Placeholder for the phase-2 spec.
- **Routes** (`App.tsx`, inside `ControlPlaneRoutes`, `RequireGlobalAdmin`): `policies` (list), `policies/new`, `policies/:id` (editor).
- **API layer**: new `frontend/src/api/policies.ts` + react-query hooks, mirroring `controlPlane.ts`.
- **Pages / components** (shadcn + OKLCH tokens; reference the Ranger admin console screens):
  - `PoliciesList` — top **profile selector** + **type tabs (Access / Masking / Row-filter)**; a table with columns Name · Resource · Principals · Permissions · Enabled (toggle) · Actions; empty/loading states.
  - `PolicyEditor` — one form whose fields **render from the service-def**:
    - **Resource picker**: `catalog → schema → table → column` with wildcard `*`, `isExcludes`, `isRecursive`.
    - **Access**: repeatable policy items = principal picker (user/group/role) + access-types (checkbox list from def) + Allow / Deny / Exclude sections.
    - **Masking**: per-column mask-type select from `dataMaskDef.maskTypes` (MASK, MASK_HASH, nullify, custom, …).
    - **Row-filter**: filter-expression input per principal group.
  - **Principal picker**: combobox backed by `/control-plane/principals`.

## 4. Layer B — Trino enforcement config artifact
Deliver, per profile, a **downloadable config bundle** (and documented manual install) so a Trino coordinator enforces the profile's policies — **no image change**, since the plugin is native:
- `etc/access-control.properties`: `access-control.name=ranger`, `ranger.service.name={service}`, `ranger.plugin.config.resource=ranger-trino-security.xml`.
- `ranger-trino-security.xml`: `ranger.plugin.trino.policy.rest.url = {egret base URL}` (serving `/service/...`), plus `ranger.plugin.trino.service.name`, download interval, and (if `ranger.auth_token` is set) the auth header wiring; `ranger.plugin.trino.use.rangerGroups` optional.
- Surfaced from the profile view as **"Download Trino config"** + copy-paste instructions.
- Fully automatic injection into a provisioned Trino coordinator (writing these two files at provision time) is a **small follow-up**, not part of this spec, because the plugin is native — noted so it is not silently assumed done.

## 5. Testing
- **Backend (unit)**: `ranger_json` round-trip for all three policy types (editor DTO → Ranger JSON → summary DTO); resource/exclude/wildcard mapping; principal resolution.
- **Backend (route, DB)**: create → list (filtered by service/type) → get → update → delete; `policy_version` increments on each write; cross-service isolation; `require_platform_admin` gates a non-admin; auto-bind creates + links a service on engine provision.
- **Enforcement (Java harness)**: extend `tests/ranger/EgretRangerEnforcementTest` to cover a masking and a row-filter policy authored through the new API, verifying the plugin enforces them — this is the **manual Trino integration test** to run before pushing (per the `egret-manual-trino-integration-test` note).
- **Frontend (Vitest)**: editor DTO serialization; `PoliciesList` renders rows + type tabs + empty states; profile selector switches service.
- **Before merge**: full local CI — `cargo fmt --all --check`, clippy, workspace tests, and frontend `lint/typecheck/test/build` (per `egret-run-full-ci-local-before-merge`); live smoke of create → download config → enforce.

## 6. Phasing
- **P1** — menu move; migration + auto-bind on provision; backend `services` + `service-def` + `list`/`get`; `PoliciesList` (read-only) + profile selector + type tabs. Locks look & feel + the def-driven pipeline.
- **P2** — Access policy full CRUD; principal + resource pickers; `ranger_json` create/update.
- **P3** — Masking + Row-filter editors (parity); Layer-B download bundle + Java-harness enforcement tests.

## 7. Risks
- **DTO↔Ranger JSON fidelity** for masking/row-filter is the main correctness risk — cover with round-trip unit tests before wiring the UI.
- **Migration staleness**: touch `metastore/src/lib.rs` so the new migration re-embeds, or `serve` runs a stale migrator.
- **Bind idempotency / sharing / delete**: must not double-create services on re-provision; `link` must validate service existence + type match; deleting an engine must **keep** shared/standalone profiles (`ON DELETE SET NULL`, never cascade the service).
- **Dev Trino is file-ACL by default**: enforcement verification runs through the `tests/ranger` harness, not the default docker stack — state exactly what was exercised (per the `verification-wording-precision` note).
- **Principal naming** must match Trino's group-mapping (see `egret-catalog-single-catalog-pivot` group-mapping note) or Deny/Allow won't match the runtime user.

## 8. Out of scope (later)
- **Data Lineage** (OpenLineage receiver + graph UI, Marquez reference) — separate sub-epic 2 spec.
- **StarRocks** service-def + provisioning + enforcement.
- **Tag-based policies** and **access-audit** views (Ranger audit stream / Solr).
- **Automatic** provision-time injection of the Trino Ranger config files.
- **Workspace-scoped** policy views (this epic is global/admin only).
