# Catalog Engine CE-4 — Minimal lakekeeper Authorizer + namespace scope — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/catalog-engine-ce4-authorizer` (off main; CE-1+CE-2+CE-3 merged)
**Parent:** `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`; findings `docs/en/specs/2026-06-26-catalog-engine-CE1-findings.md`; predecessor `docs/en/specs/2026-06-27-catalog-engine-CE3-token-scope-design.md`

## 0. Goal

Add a **real, minimal lakekeeper `Authorizer`** (replacing the inert `AllowAllAuthorizer`) so that catalog **data actions** (namespace / table / view) are authorized inside lakekeeper itself — defense-in-depth behind egret's `front_auth` — and introduce **per-token namespace scoping** (`namespace_scope[]`): a token can be restricted to one or more namespace subtrees (e.g. `marts.*`) and is denied everywhere else.

This is *minimal* by deliberate scope: the Authorizer enforces only the **data actions** (the catalog REST attack surface). Control-plane actions (warehouse / project / server) that the normal catalog flow legitimately touches are **allowed** — `front_auth` already gates entry (per-workspace token ownership + read/write), and the `{prefix} == warehouse-of-workspace` invariant bounds every request to one workspace's warehouse. We do **not** reimplement lakekeeper's full authorization model (users, roles, projects, server admin, OpenFGA tuples).

## 1. Decisions (locked)

- **Authorizer posture:** `EgretAuthorizer` enforces `namespace` / `table` / `view` actions (read/write permission + `namespace_scope`). Control-plane actions (`server` / `project` / `warehouse` / `user` / `role`) that the normal flow needs → **allow**. Post-action hooks (`delete_*`, role assignments) → no-op `Ok`. (User-chosen: "allow control-plane, enforce only data actions".)
- **System principal bypass:** the egret system principal (used by bootstrap + the in-process browse reads) is recognized by its `UserId` and **allowed for everything**. We do NOT rely on lakekeeper's `LAKEKEEPER__INSTANCE_ADMINS` config (an instance-admin would *bypass* the Authorizer — the opposite of what we want for token principals).
- **Scope bridge:** `front_auth` stops injecting `RequestMetadata::new_lakekeeper_internal` (which bypasses authz). Instead it injects a **token principal** — `Actor::Principal(EGRET_TOKEN_PRINCIPAL)` (a stable, **non-admin** `UserId` distinct from the system principal) plus a `limes::Authentication` whose **`claims` carry the egret scope** under one key: `claims["egret"] = { "perms": ["catalog:read", ...], "ns": ["marts", ...] }`. The Authorizer reads it back via `RequestMetadata::authentication()?.claims("egret")`.
  - Scope is carried in a **custom claim**, NOT in `Authentication::roles()` — lakekeeper parses `roles()` into typed `RoleIdent`s (`extract_and_set_token_roles`), which we must not perturb.
- **`front_auth` stays the first line:** the existing ownership check (`{prefix} == warehouse`, 401/403) and read/write `required_permission` check (CE-3) are **unchanged**. The Authorizer is the second, lakekeeper-native line — and the home of `namespace_scope` (which `front_auth` cannot enforce without re-parsing namespaces out of every route path).
- **Back-compat:** `namespace_scope` defaults to empty (NULL) = "all namespaces in the workspace". Existing tokens are unaffected.
- **No new lakekeeper user rows:** the token principal need not exist in lakekeeper's `users` table — the request path does not auto-provision users, and `create_table` does not FK `created_by → users` (ownership is the Authorizer's concern, and ours grants by claim, not by stored tuples). Recorded `created_by` for token writes is the token principal's synthetic id; that is acceptable (finer per-token audit identity is out of scope).

## 2. Background — why the bridge is clean (CE-1/CE-2 verified, re-confirmed here)

- `RequestMetadata::authentication() -> Option<&limes::Authentication>` is public (`request_metadata.rs:462`).
- `limes::Authentication` carries `claims: serde_json::Value` with accessor `claims(key: &str) -> Option<&serde_json::Value>`, and is built via `Authentication::builder()…build()` — the exact pattern already used by `bootstrap::system_request_metadata`.
- `are_allowed_namespace_actions_impl` / `…table…` / `…view…` receive the structured target (`&impl AuthZNamespaceInfo`, `NamespaceWithParent`, the resolved warehouse) **and** `&RequestMetadata` — so the Authorizer has both the scope (from the claim) and the target namespace identifier to match against. No path parsing.
- Switching `front_auth` from the internal actor to `Actor::Principal` makes lakekeeper run authz **for real** on every request — this is the point, and the main risk (see §6).

## 3. Components

### 3.1 Schema (migration `0028_catalog_token_namespace_scope.sql`)
```sql
ALTER TABLE catalog_tokens
  ADD COLUMN namespace_scope text[] NOT NULL DEFAULT '{}';
```
Empty array = unrestricted (all namespaces in the workspace). Existing tokens get `{}` = full. Touch `crates/egret-metastore/src/lib.rs` to force the embedded migrator to re-embed (see [[egret-sqlx-migration-staleness]]).

### 3.2 `egret-catalog` token store (`service/tokens.rs`)
- `create_token(workspace_id, label, permissions: &[String], namespace_scope: &[String]) -> (CatalogToken, raw)` — gains a `namespace_scope` param; INSERT it. **Validation:** each entry is a non-empty, dot-separated namespace identifier (e.g. `marts`, `marts.sales`); reject empties / leading-or-trailing dots with `BadRequest`. (No existence check — a scope may name a namespace not yet created.)
- `TokenAuth` gains `namespace_scope: Vec<String>`. `authenticate_token` selects it in the same lookup (`SELECT workspace_id, permissions, namespace_scope …`).
- `CatalogToken` DTO gains `namespace_scope: Vec<String>` (for `list-tokens` display).

### 3.3 CLI (`egret catalog mint-token` / `list-tokens`)
- Add `--namespaces <comma-list>` to `mint-token` (default empty = all). Parse to `Vec<String>`; pass to `create_token`. `list-tokens` shows a `namespaces` column (`*` when empty).

### 3.4 Scope bridge (`egret-catalog-engine/src/front_auth.rs`)
- Introduce a stable **token principal** id, mirroring `bootstrap::system_user_id` but with a **distinct fixed subject** so it can never equal the system principal: `EGRET_TOKEN_PRINCIPAL = UserId::try_from("oidc~<uuidv5(EGRET_TOKEN_NS, "egret-catalog-token")>")`. One shared principal for all token requests (scope rides in the claim, not the identity).
- Replace the `RequestMetadata::new_lakekeeper_internal(…)` injection (front_auth.rs:172) with a helper `token_request_metadata(auth: &TokenAuth) -> RequestMetadata` that:
  - builds `Authentication::builder().token_header(None).claims(json!({ "egret": { "perms": auth.permissions, "ns": auth.namespace_scope } })).subject(EGRET_TOKEN_PRINCIPAL.into()).name(Some("Egret Catalog Token")).email(None).principal_type(None).build()`,
  - `set_authentication(Actor::Principal(EGRET_TOKEN_PRINCIPAL.clone()), authentication)` onto a `new_lakekeeper_internal` base (same construction shape as `system_request_metadata`).
- The ownership + `required_permission` checks above the injection are unchanged.

### 3.5 `EgretAuthorizer` (`egret-catalog-engine/src/authorizer.rs`, new)
Implements `lakekeeper::service::authz::Authorizer`. Carries `server_id: ServerId` and the system principal `UserId` (to recognize the bypass). Constructed in `build_state`.

Decision helper, shared by the three data-action methods:
```text
fn decide(metadata, target_namespace_levels: &[String], action_is_write: bool) -> bool:
    if metadata.user_id() == Some(&system_principal):        # bootstrap / browse
        return true
    let scope = metadata.authentication()?.claims("egret")    # {perms, ns}
        else: return false                                    # no egret claim ⇒ deny (fail-closed)
    let need = if action_is_write { "catalog:write" } else { "catalog:read" }
    if !scope.perms.contains(need): return false
    if scope.ns is non-empty AND no entry in scope.ns is a prefix of target_namespace_levels:
        return false                                          # namespace_scope (subtree match)
    return true
```
- **Action read/write classification:** map each lakekeeper `Catalog{Namespace,Table,View}Action` variant to read vs write. Writes = create/drop/commit/rename/`WriteData`/set-protection/undrop/control-tasks/properties-mutation; reads = `GetMetadata`/`ReadData`/`IncludeInList`/list/`GetTasks`. Default unknown → **write** (default-deny, safer to over-require). Unit-tested per variant.
- **Namespace subtree match:** scope entry `["marts"]` matches target `["marts"]` and `["marts","sales"]`; not `["marketing"]`. (Level-wise prefix on the dot-split identifier.)
- **`are_allowed_namespace_actions_impl` / `…table…` / `…view…`:** for each `(target, action)` tuple, return `decide(...)`. Table/view targets carry their parent namespace (via `NamespaceWithParent`) — match `namespace_scope` against that parent namespace's levels.
- **All other trait methods** (`are_allowed_{server,project,warehouse,user,role,generic_table}_actions_impl`, `can_bootstrap`, `bootstrap`, `list_projects_impl`, `can_search_users_impl`, `check_assume_role_impl`, role-assignment add/remove/list): **allow** / return the permissive result (empty/`Ok`), matching `AllowAllAuthorizer` — control-plane is gated by `front_auth`, not here.
- **Post-action hooks** (`delete_user`, `delete_role`, `delete_table`, `delete_view`): no-op `Ok(())`.
- **`new_router`:** return an empty `Router` (egret does not expose lakekeeper's authz-management API). `api_doc()`: minimal/empty OpenAPI. `implementation_name()`: `"egret"`. `server_id()`: stored value.
- **Associated types:** `TableAction = CatalogTableAction`, `ViewAction = CatalogViewAction`, `NamespaceAction = CatalogNamespaceAction`, etc. — reuse lakekeeper's enums (same as AllowAll).

> Implementation note: copy `AllowAllAuthorizer`'s trait impl as the skeleton (it already returns the permissive default for every method + has the right associated types), then override only the three data-action methods with `decide(...)`. This bounds the surface and guarantees we don't miss a required method.

### 3.6 Wiring (`mount.rs`, `bootstrap.rs`, `browse.rs`, `provisioner.rs`)
- `EngineState = State<EgretAuthorizer, PostgresBackend, SecretsState>` (was `AllowAllAuthorizer`). `EngineServer = CatalogServer<PostgresBackend, EgretAuthorizer, SecretsState>`. Propagate the type alias to `bootstrap.rs` (`EngineApiServer`), `browse.rs` (`EngineApiServer`), `provisioner.rs`.
- `build_state(dsn, system_user)` constructs `EgretAuthorizer { server_id, system_principal: system_user_id(system_user)? }`. (It already reads `server_id`; it gains the system principal id — thread `catalog.system_user` in, or compute from the same config the bootstrap uses.)
- Bootstrap + browse already use `system_request_metadata` → its principal is the system principal → Authorizer bypass path. No behavior change for them (verify in tests).

## 4. Testing

- **Unit (authorizer.rs):**
  - action classification: each `Catalog{Namespace,Table,View}Action` variant → expected read/write.
  - `decide`: system principal → allow regardless; token principal with `perms=[read]` → read allow / write deny; `ns=["marts"]` → target `marts.x` allow, `marketing` deny, `marts` allow; empty `ns` → any namespace allow; missing egret claim → deny.
  - namespace subtree match helper: prefix cases above.
- **Unit (tokens.rs / front_auth.rs):** `create_token` rejects malformed namespace entries (`BadRequest`); `required_permission` table unchanged (regression).
- **Integration (DB+MinIO, extend `front_auth` test):** for workspace A, mint:
  - a `marts`-scoped read-write token: `POST .../namespaces` to create `marts` and a table under it → 200; create/commit a table under `marketing` → **denied** (lakekeeper 403 from the Authorizer); `GET` a `marts` table → 200; `GET`/`POST` under `marketing` → denied.
  - a read-only full-scope token: `GET` → 200, any write → 403 (CE-3 path, unchanged).
  - Regression: full-scope read-write token (no `--namespaces`) → create/commit/drop succeed across namespaces; cross-workspace token A on B → 403 (front_auth); garbage token → 401; browse API + bootstrap (system principal) still succeed (Authorizer bypass).
- **Defense-in-depth:** a test that calls a lakekeeper `Service` in-process with a token principal `RequestMetadata` whose claim lacks the needed scope → the action is denied even though `front_auth` was not in the path (proves the Authorizer is a real second line).

## 5. File map

- Create: `migrations/0028_catalog_token_namespace_scope.sql`; `crates/egret-catalog-engine/src/authorizer.rs`.
- Modify: `crates/egret-metastore/src/lib.rs` (touch); `crates/egret-catalog/src/service/tokens.rs` (+ test `tests/store_tokens.rs`); `crates/egret-bin/src/cli.rs` + `main.rs` (`--namespaces`, list display); `crates/egret-catalog-engine/src/front_auth.rs` (bridge + token principal; + test `tests/front_auth.rs`); `crates/egret-catalog-engine/src/{mount.rs,bootstrap.rs,browse.rs,provisioner.rs,lib.rs}` (state type + `EgretAuthorizer` construction).

## 6. Risks

- **Authz now runs for real.** Switching `front_auth` internal→principal means every lakekeeper authz check executes. If `EgretAuthorizer` denies an action the *normal* catalog flow needs (some warehouse/namespace housekeeping action), legitimate operations break. Mitigation: control-plane = allow (only the three data-action methods can deny); broad integration coverage of create/list/load/commit/drop with a full-scope token before testing the scoped-deny cases.
- **System principal must bypass.** Bootstrap + browse must keep working — they go through `system_request_metadata`; the Authorizer recognizes that `UserId`. If the recognition is wrong, startup bootstrap or the Catalog UI browse breaks loudly (covered by existing tests + new assertions).
- **Claim shape coupling.** `front_auth` writes and `EgretAuthorizer` reads the same `claims["egret"]` shape — keep them in one module-documented contract; a serde mismatch fails closed (deny), which a full-scope integration test will catch immediately.
- **Single `iceberg` version** must hold (CE-1 invariant): no new deps that pull a second `iceberg`.

## 7. Out of scope (later)

- Finer S3-verb scoping on `/aws/s3/sign` (still classified read, as CE-3).
- Per-token audit identity (distinct lakekeeper user per token / per workspace).
- Full lakekeeper authorization parity (roles, projects, server admin, OpenFGA-style tuples, authz-management API).
- CE-5 (storage profiles / secrets bridge, GCS/Azure), CE-6 (GC / task-queue workers).
