# Notebook F1 — Access Foundation (design)

**Status:** draft · **Date:** 2026-06-30 · **Epic:** Notebooks · **Predecessor:** C1 (merged, squash `b57a418`, PR #94)

## 1. Context & scope

The notebook data-isolation model (memory `egret-notebook-access-model`) is **two-plane**:
- **Governed reads** go through Trino, authorized **per end-user by Ranger** (`X-Trino-User` impersonation). Governed lakehouse is read-only to notebooks.
- **Private read/write** lives in a per-user namespace `notebook_{user}` on the REST catalog, gated by a per-(user×workspace) **1-grant catalog token**.

F1 builds the **foundation** for both planes so D1 (kernel runtime) and E1 (execution) can consume it. F1 does **not** build the kernel or execution — it delivers (a) per-user Trino impersonation and (b) the per-user namespace + token + a runtime endpoint that hands the kernel everything it needs.

### In scope
- **F1a — Trino impersonation:** a stable per-user `username`; an opt-in `impersonate` flag per engine connection; `TrinoConnector` sends `X-Trino-User = username` (basic-auth still the service principal) when the connection opts in.
- **F1b — Per-user catalog access:** idempotent provisioning of namespace `notebook_{username}` + a 1-grant catalog token; a `GET /api/ws/{ws}/notebook-runtime` endpoint returning the kernel manifest.

### Out of scope (later)
- The kernel itself, kernel helpers, execution, output rendering (D1/E1).
- StarRocks per-user impersonation (MySQL protocol — harder; deferred).
- Trino server-side impersonation config + Ranger policies (ops/infra, not egret code — but required to actually enforce; see §3.4).
- Per-user MinIO IAM / STS vended credentials (docker/k8s launcher hardening).

## 2. Decisions (from brainstorm)
- **Impersonation scope = per-connection flag, default OFF.** No behavior change to existing SQL Lab/charts until an admin enables it on a Trino connection that is configured for impersonation. When ON, **every** query via that connection (SQL Lab + notebook) runs as the end-user — consistent governance.
- **`X-Trino-User` = `username` = email local-part**, stored as a **stable** per-user value (derived once; collision-suffixed), NOT re-derived per query.

## 3. F1a — Trino impersonation

### 3.1 Stable username
Add `username citext UNIQUE` to `users` (migration). Derivation: `slug(local-part(email))`; on collision append a short suffix (mirrors sparklabx). Backfill existing rows in the migration (or lazily on first use — see §3.5). The username is immutable once set. It is the identity Ranger policies are written against.

### 3.2 Per-connection `impersonate` flag
Add `impersonate: Option<bool>` (default false) to `ConnOptions` (`egret-database` model; stored in the `databases.options` JSONB — no schema migration needed). Surface a toggle on the engine connection form (DatabasesPage / its Add/Edit dialog), Trino-only, with a help note: "Run queries as the logged-in user (requires Trino impersonation + Ranger)."

### 3.3 Connector threading
The end-user identity must reach the connector:
- The engine job already knows the submitter `user: Option<Uuid>` (`egret-engine/src/job.rs`). Resolve that to the stable `username` (lookup) and thread it as an optional `session_user: Option<String>` through `ConnectorFactory::build` / `ConnSpec`.
- `TrinoConnector`: currently sets BOTH `X-Trino-User` and basic-auth to `self.user` (the service user). Split them:
  - basic-auth principal = `spec.options.user` (the service principal allowed to impersonate) — unchanged.
  - `X-Trino-User` = `session_user` **iff** `spec.options.impersonate == true` **and** a `session_user` is present; otherwise the service user (current behavior).
- Non-Trino connectors ignore `session_user`. StarRocks deferred.

### 3.4 Trino + Ranger config (ops — documented, not built here)
For impersonation to be enforced (not just sent), the deployment must: (1) allow the egret service principal to impersonate session users in Trino; (2) point Trino's Ranger plugin at egret-ranger's policy surface; (3) author per-user/group Ranger policies. F1 ships the egret code + a docs note; the **manual real-Trino integration test** (§7) verifies end-to-end.

### 3.5 Safety
With the flag OFF (default), behavior is byte-for-byte unchanged — no risk to existing queries. With it ON but Trino not configured for impersonation, Trino rejects the query (a loud, contained failure on that one connection), not a silent bypass. Username backfill must not break login or existing flows.

## 4. F1b — Per-user catalog access

### 4.1 Provisioning service (idempotent)
A provisioning routine, callable on every kernel start, that ensures:
1. The caller has a stable `username` (derive+store if absent — shared with F1a).
2. Namespace `notebook_{username}` exists in the workspace's catalog/warehouse (create via the existing `NamespaceService::create_namespace`; treat already-exists as success).
3. A per-(user×workspace) catalog token exists with a **single grant** `{ namespace: "notebook_{username}", table: None, permissions: [read, write] }` (via `CatalogStore::create_token`). Because tokens are stored hashed (raw not recoverable), the runtime endpoint **mints a fresh token and revokes the previous one** for this (user×workspace) on each call (`revoke_token_for_workspace` + `create_token`) — so the kernel always gets a usable raw token and at most one is active. No raw token stored at rest.

### 4.2 Runtime endpoint
`GET /api/ws/{ws}/notebook-runtime` — gated by `notebook:read` (the calling user). Runs the provisioning (idempotent), then returns the **kernel manifest**:
```json
{
  "username": "alice",
  "catalog": { "url": "<iceberg REST base>", "warehouse": "<slug/prefix>", "namespace": "notebook_alice", "token": "<raw 1-grant token>" },
  "engines":  [ { "id": "...", "name": "prod-trino", "engine": "trino", "host": "...", "impersonate": true } ],
  "catalogs": [ { "id": "...", "name": "lakehouse" } ]
}
```
- `engines` / `catalogs` come from the **Notebook settings** (RS-2: `notebook_engine_access` / `notebook_catalog_access`) — only those the workspace enabled for notebooks.
- This endpoint is the single **F1→D1 bridge**: the kernel (D1) calls it at boot to get the catalog token, its private namespace, the username for `X-Trino-User`, and the enabled engines/catalogs. The kernel helpers (`query()`, `catalog()`, preloaded handles) are built in D1/E1 on top of this manifest.

### 4.3 Where it lives
The runtime endpoint + provisioning live in `egret-notebook` (it is notebook runtime). egret-notebook gains a dependency on `egret-catalog` (token mint/revoke) and reaches namespace creation via the catalog engine's `NamespaceService` (confirm the cleanest call path in the plan — direct service call vs internal route). Reuse the existing `NotebookState` + add the catalog store/handles it needs.

## 5. Data flow
- **Governed read (SQL/notebook):** client → egret-engine job (knows user_id) → resolve username → TrinoConnector (impersonate on) → Trino with `X-Trino-User=alice` → Ranger authorizes alice → rows.
- **Private r/w (kernel):** kernel boot → `GET /notebook-runtime` → {token, namespace, …} → kernel uses the 1-grant token against the Iceberg REST catalog (scoped to `notebook_alice`); data IO via the existing remote signer.

## 6. Security / authz
- Impersonation OFF by default; ON is an explicit per-connection admin choice.
- `X-Trino-User` is the stable username; basic-auth remains the service principal (only it may impersonate, enforced Trino-side).
- The runtime token is **1-grant** (only `notebook_{username}` rw) — a compromised kernel cannot reach other namespaces or other users' notebooks. Cross-user: `notebook_{username}` is unique per user; the token names only the caller's namespace.
- Runtime endpoint gated by `notebook:read`; it provisions only the **caller's** namespace/token (never another user's). The enabled engines/catalogs are the workspace's notebook-allowed set (RS-2), already ws-scoped.
- Mint-and-revoke keeps at most one active token per (user×workspace).

## 7. Testing
- **Unit/integration (`#[sqlx::test]`):** username derivation + collision suffix + stability; provisioning idempotency (namespace + token created once, re-call mints fresh + revokes old); runtime endpoint returns the manifest with only the workspace's enabled engines/catalogs; `notebook:read` gate; the token's grant is exactly `{notebook_{username}: rw}`; cross-user — user B's runtime never returns user A's namespace/token.
- **Connector test:** with `impersonate=true` + a session_user, the Trino request carries `X-Trino-User=<username>` and basic-auth = service user; with `impersonate=false`, unchanged (service user).
- **Manual real-Trino integration test** (memory `egret-manual-trino-integration-test`): enable the flag on the dev Trino connection, run a query as two users, verify Ranger enforces per-user (one allowed, one denied) — impersonation actually takes effect, not just the header. Run before merge.
- **Local CI** before merge (memory `egret-run-full-ci-local-before-merge`): full workspace fmt/clippy/test + frontend lint/typecheck/test/build. **Append-only migrations** (lesson from C1 — do NOT edit an applied migration).

## 8. Build decomposition (one spec, two groups)
- **F1a:** username column+derivation+backfill · `impersonate` flag on ConnOptions + DatabasesPage toggle · connector threading (job→username→ConnSpec→TrinoConnector) · tests.
- **F1b:** provisioning service (namespace + token mint/revoke) · `GET /notebook-runtime` manifest endpoint (reads RS-2 settings) · tests.

## 9. Open questions (resolve in planning)
1. Username backfill: in-migration vs lazy-on-first-use. *Lean: in-migration for determinism, lazy fallback for users created later (OIDC).* 
2. Exact namespace-create call path from egret-notebook (direct `NamespaceService` vs internal catalog route) — pick the one that doesn't entangle crate boundaries.
3. Whether the DatabasesPage `impersonate` toggle needs a new perm — reuse the existing database-management perm (`workspace:manage_members`, per RS-2).
