# StarRocks ↔ Ranger Enforcement — Full Native Parity — Design

**Date:** 2026-07-16 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-ranger-enforcement` (off main)
**Program:** StarRocks feature-parity-with-Trino, **slice 1 of 3** (the other two — control-plane provisioning, delegation/security parity — are separate later specs). This spec covers **StarRocks Ranger policy enforcement** only.

## Goal

Bring StarRocks up to **everything StarRocks + Apache Ranger natively support** for data governance, at parity with egret's Trino Ranger enforcement:

- **Resource-based access policies** over StarRocks' full resource hierarchy (catalog → database → table → column, plus view / materialized_view / function / global_function / resource / resource_group / storage_volume / user / system).
- **Column-masking policies** (StarRocks' six native mask types).
- **Row-level-filter policies**.

Policies are **authored in egret**, **served by egret's Ranger-Admin-compatible `/service` REST**, and **enforced by StarRocks FE's built-in Ranger integration**. The outcome is proven live: a masked-vs-clear column, a row-filtered result set, and a denied `SELECT` — each divergent between a cleared and an uncleared principal on a real StarRocks query.

## Non-goals (explicit scope boundaries)

- **Tag-based policies are OUT.** StarRocks' Ranger plugin supports **resource-based policies only** — tag-based access/masking/row-filter is an open upstream feature request ([StarRocks #67458](https://github.com/StarRocks/starrocks/issues/67458)), not implemented. StarRocks' FE will **never poll the tag service** (`/service/tags/download/...`), so egret's P3.2 classification-tag → mask model does **not** flow to StarRocks natively. This is a documented, permanent-until-upstream gap, not a bug. (Tagging a column masks it on Trino only; StarRocks masking is authored per-column/resource.)
- **Engine provisioning is OUT** (separate slice). egret does not provision StarRocks, so the Ranger plugin configuration is delivered as **deploy-time config** on the dev StarRocks, not templated by an orchestrator. When the provisioning slice lands, that slice will move this config into the StarRocks provisioner (analogous to what P3.2 did for Trino).
- **Ranger autocomplete is OUT.** The service-def ships `implClass: ""`; egret does not implement Ranger's resource-lookup endpoint. Authoring uses egret's own catalog/identity autocomplete, not the Ranger plugin's server-side autocomplete.
- **Query path is unchanged.** Per-user JWT delegation already ships (`StarRocksConnector` presents the OIDC engine JWT via the `mysql_clear_password` / cleartext plugin, login user = `preferred_username`). StarRocks FE authorizes that principal; no connector change is needed.

## Background — current state (verified against the codebase)

- **egret's Ranger serving layer is already engine-agnostic.** `crates/egret-policy/src/ranger/routes.rs` mounts `ranger_router()` at `/service` with `GET /plugins/policies/download/{service}` (`download_policies`) and `GET /tags/download/{service}` (`download_tags`). Both take an arbitrary `{service}` name and serve the stored service-def + enabled policies (or tags) **verbatim**. No engine-specific branching — a StarRocks service "drops in" on the serving side with no route change.
- **The policy store is multi-service.** `PolicyStore` models the full Ranger surface (`list_service_defs / get_service_def / list_services / create_service / create_policy(service_id, …) / update_policy / enabled_policies_for_service`), and every policy write bumps the service's `policy_version` so a polling engine re-downloads automatically. Service-defs stored as jsonb; policies stored verbatim.
- **Service-defs are embedded + seeded at startup.** `crates/egret-policy/src/service_def.rs` embeds `service-defs/trino.json` (id 203) and `service-defs/tag.json` (id 204), with `seed_trino` / `seed_tag`, both called from `crates/egret-bin/src/main.rs:975` gated on `ranger.enabled`. **No StarRocks def exists yet.**
- **Authoring is partly Trino-hardcoded — this is the real work.** `crates/egret-policy/src/admin/ranger_json.rs` (the editor→Ranger-JSON serializer) stamps `serviceType: "trino"` (lines 129/199/254) and its access/mask surfaces assume the Trino service-def. The earlier P4 design *intended* a fully def-driven editor "so StarRocks drops in with only a new service-def," but the serializer + FE mask-type list are still Trino-specific. Making authoring **service-type-aware** is a genuine scope item, not a no-op.
- **Auth on `/service` is anonymous when no token is set.** `ranger/auth.rs::require_bearer` is anonymous with an empty `RangerSettings.auth_token` (the default) — deliberately, for stock Ranger plugins that authenticate out-of-band. Same posture used for the Trino P3.2 smoke. Read-only surface.
- **The dev StarRocks exists but has no Ranger config.** `deploy/docker-compose.dev.yml` runs `starrocks/allin1-ubuntu:latest`; `deploy/starrocks/` holds the JWT security-integration artifact (`init/security-integration.sql`) but no Ranger plugin config. StarRocks' Ranger support is **built into the FE** (native `com.starrocks.privilege.ranger`), enabled by config only — **no jar install**, mirroring Trino's "plugin already in the image."

## Authoritative StarRocks Ranger model (from the canonical service-def)

Source: `conf/ranger/ranger-servicedef-starrocks.json` (StarRocks main). Verbatim structure to embed (adapted only for a stable id + `implClass`):

- **name:** `starrocks`
- **resources (name | parent | level):** `catalog|—|10`, `database|catalog|20`, `table|database|30`, `column|table|40`, `view|database|30`, `materialized_view|database|30`, `function|database|30`, `global_function|—|10`, `resource|—|10`, `resource_group|—|10`, `storage_volume|—|10`, `user|—|10`, `system|—|10`.
- **accessTypes:** `grant, node, operate, delete, drop, insert, select, alter, export, update, usage, plugin, file, blacklist, repository, refresh, impersonate, create database, create table, create view, create function, create global function, create materialized view, create resource, create resource group, create external catalog, create storage volume`.
- **dataMaskDef.maskTypes:** `MASK, MASK_HASH, MASK_NULL, MASK_NONE, MASK_DATE_SHOW_YEAR, CUSTOM` (StarRocks — distinct from Trino's `MASK_SHOW_LAST_4` set).
- **rowFilterDef:** present.
- **policyConditions:** `ip-range` (IP-range evaluator). **contextEnrichers:** none.
- **configs (for reference; egret stamps the service instance's config, not the def):** `username` (default `root`), `password`, `jdbc.driverClassName` (default `com.mysql.cj.jdbc.Driver`), `jdbc.url` (default `jdbc:mysql://127.0.0.1:9030`).

## Architecture — the three seams

egret already owns store + serving; the connector already presents identity. The gaps are: (1) the def, (2) service-type-aware authoring, (3) config delivery to StarRocks.

### 1. Service-def + seed (`egret-policy`) — small, well-understood

- Embed `crates/egret-policy/src/service-defs/starrocks.json`: the canonical StarRocks def with a **stable numeric id 205** (Trino 203, tag 204) and `implClass: ""` (no autocomplete). Resources / accessTypes / dataMaskDef / rowFilterDef / `ip-range` condition verbatim.
- Add to `service_def.rs`: `STARROCKS_SERVICE_DEF_JSON`, `STARROCKS_DEF_NAME = "starrocks"`, `starrocks_service_def()`, `seed_starrocks(store)`.
- Wire `seed_starrocks` into `crates/egret-bin/src/main.rs:975` alongside `seed_trino` / `seed_tag`, gated on `ranger.enabled`.
- Unit test mirroring `embedded_trino_def_parses_with_id_and_resources`: asserts `name == "starrocks"`, `id == 205`, resources include catalog/database/table/column, accessTypes include `select`, `dataMaskDef` object with the StarRocks mask types, `rowFilterDef` object.

### 2. Service-type-aware authoring (`egret-policy/admin/ranger_json.rs` + FE editor) — medium

The single behavioral change: policy authoring must emit the **selected profile's** service type and honor **its** def, instead of the hardcoded Trino values.

- **Backend (`admin/ranger_json.rs`):** thread the profile's service type (resolved from `authz_services.type` for the target service) through `editor_to_ranger_json` / the access/mask/rowfilter serializers so the emitted body carries `serviceType: "starrocks"` for a StarRocks profile. The Trino path is unchanged (still emits `"trino"`). Add unit tests asserting a StarRocks-profile policy serializes `serviceType: "starrocks"` with StarRocks resource keys (`database` not `schema`) and a StarRocks mask type (`MASK_HASH`).
- **Frontend (Policies editor):** drive the **mask-type options** and **resource selector hierarchy** from the selected profile's service-def (already downloadable via the defs endpoint) rather than Trino constants. A StarRocks profile then offers `MASK/MASK_HASH/MASK_NULL/MASK_NONE/MASK_DATE_SHOW_YEAR/CUSTOM` and the `catalog→database→table→column` hierarchy.
- **Profile creation:** allow creating a **`starrocks`-typed policy profile** (a `create_service(name, "starrocks", cfg)` service instance) from the Policies UI — the manual analogue of Trino's provisioning auto-bind. The profile's config carries the StarRocks JDBC coordinates (host:9030) for operator reference; enforcement itself needs only name + type.

**Boundary note:** the serializer stays a thin, resource-shape transform. Resource *validation* (does this catalog/db/table exist) remains egret's catalog concern, not the serializer's — the serializer only reshapes editor JSON → Ranger JSON for the chosen service type.

### 3. Deploy config + live smoke (`deploy/starrocks`) — medium, live-only

Because there is no StarRocks provisioner, the plugin config is delivered as dev-stack config (the provisioning slice will later relocate it into the orchestrator):

- `deploy/starrocks/ranger/ranger-starrocks-security.xml` — `ranger.plugin.starrocks.service.name=<profile>`, `ranger.plugin.starrocks.policy.rest.url=<egret base URL>` (the `/service` prefix is appended by StarRocks' `RangerAdminRESTClient`), `policy.source.impl=org.apache.ranger.admin.client.RangerAdminRESTClient`, `policy.pollIntervalMs`, `policy.cache.dir=/tmp/ranger/<profile>/policycache`, client timeouts.
- `deploy/starrocks/ranger/ranger-starrocks-audit.xml` — minimal, audit-to-Ranger disabled for dev.
- `fe.conf`: `access_control=ranger` (delivered via the compose volume / an init step; documented in `deploy/starrocks/README.md`).
- `docker-compose.dev.yml`: mount the two XML files + the `access_control` FE config into the `allin1-ubuntu` FE conf dir.
- `deploy/starrocks/README.md`: the manual setup + activation steps (mirrors the existing JWT-integration README).

**Live smoke (controller-driven, not a subagent):** with egret serve (`ranger.enabled`, `engine_egret_url` reachable from the container), a seeded `starrocks` def, and a StarRocks profile:
1. Create a real StarRocks-readable Iceberg/native table with a sensitive column (e.g. `phone`) and rows spanning a filterable dimension (e.g. `region`).
2. Author three resource policies against the StarRocks profile: a **column-mask** on `phone` (`MASK_HASH`, exempting role `pii_reader`), a **row-filter** (`region = 'US'` for a limited role), and an **access-deny** (no `select` for an uncleared principal).
3. Confirm StarRocks FE polls `GET /service/plugins/policies/download/<profile>` (egret logs) after `access_control=ranger` + XML land.
4. Query as two JWT principals and record both outputs: masked-vs-clear `phone`, filtered-vs-full rows, allowed-vs-denied `SELECT`. Divergence on all three = **StarRocks Ranger enforcement VERIFIED LIVE**.
5. Report exactly what ran (live StarRocks query outputs; not automated tests — egret has no StarRocks Ranger test infra in CI; this surface is LIVE-ONLY, as P3.2 was for Trino).

## Data flow

Seed `starrocks` def at startup (`ranger.enabled`) → admin creates a `starrocks` profile and authors resource access/mask/row-filter policies → each write bumps `policy_version` → StarRocks FE (`access_control=ranger`, security.xml → egret `/service`) polls and caches the ServicePolicies → user runs a query through egret's `StarRocksConnector` presenting their OIDC JWT → StarRocks FE enforces deny/mask/filter for that principal.

## Testing strategy

- **Unit (CI):** `starrocks.json` parses with id 205 + the required resources/accessTypes/mask surface; `seed_starrocks` upserts idempotently; `admin/ranger_json.rs` serializes a StarRocks-profile policy with `serviceType: "starrocks"` + StarRocks resource keys + a StarRocks mask type; the Trino serialization path is unchanged (regression-guarded).
- **DB (CI):** `download_policies` for a `starrocks` service returns the seeded def + enabled policies in the ServicePolicies envelope (mirroring the existing Trino `download_tags_db` / `store_services` tests).
- **Live-only (not CI):** the three-surface smoke above. Documented as the correctness boundary — the exact `ranger.plugin.starrocks.*` XML keys, `allin1` config-only enablement, and anonymous-download acceptance are validated at smoke, and any deviation is a config fix (XML key / fe.conf), not a re-architecture.

## Live-only risks (validated at smoke, mirroring P3.2)

1. **`allin1-ubuntu` FE honors `access_control=ranger` config-only** (no jar install). If a jar/extension is required on this image tag, the fallback is documented in the README; enforcement architecture is unaffected.
2. **Exact `ranger.plugin.starrocks.*` XML property names.** Follow the StarRocks Ranger service-type convention; a mismatch surfaces immediately in the FE log + the absence of egret download polls — a one-line XML fix.
3. **Anonymous download auth accepted** by StarRocks' `RangerAdminRESTClient` (as it is by Trino's). If the client demands a token, set `RangerSettings.auth_token` + the XML basic-auth props — additive, no re-architecture.

## Work breakdown (for the implementation plan)

1. **Service-def + seed** — `starrocks.json` (id 205, `implClass:""`), `service_def.rs` fns, startup wiring, parse test. *(egret-policy, egret-bin)*
2. **Service-type-aware serializer** — thread service type through `admin/ranger_json.rs`; StarRocks + Trino-regression unit tests. *(egret-policy)*
3. **Def-driven FE authoring** — mask-type + resource hierarchy from the selected profile's def; profile-create supports `starrocks` type. *(frontend)*
4. **Deploy config** — `ranger-starrocks-security.xml`, audit xml, `fe.conf access_control=ranger`, compose mounts, `deploy/starrocks/README.md`. *(deploy)*
5. **Full-workspace gate + LIVE SMOKE** — `cargo fmt`/`clippy`/`test --workspace` + frontend lint/typecheck/test/build green locally; then the three-surface live proof; report exactly what ran.

## Self-review checklist

- **Scope focused:** one slice (enforcement), tags + provisioning + autocomplete explicitly excluded with reasons. ✔
- **No placeholders:** service-def id (205), mask types, XML keys, and seed wire-in point (`main.rs:975`) are concrete. ✔
- **Internal consistency:** serving layer is generic (no change) ↔ authoring is Trino-hardcoded (the change) ↔ delivery is deploy-side (no provisioner) — all three stated consistently. ✔
- **Ambiguity resolved:** "full parity" is pinned to *native resource-based* surface; the tag gap is called out as permanent-until-upstream so "parity" can't be read as "tag masking too." ✔
