# Ranger auto-admin policies on provision (Security e2e — Sub-project A) — Design

> Part of the "full security flow" effort. Sub-project **A** of three:
> **A** = Ranger auto-permissions for platform admins on provision (this doc) →
> **B** = per-user permission enforcement across SQL Lab → dataset → chart →
> dashboard → notebook (later) → **C** = notebook write isolation to a
> per-workspace namespace (later).

## 1. Goal

When an engine (Trino **or** StarRocks) is provisioned with **Ranger enabled**
and **per-user delegation** (impersonate or JWT), egret automatically creates a
Ranger **all-access** policy that grants each **platform-admin user** full data
access, keyed by that user's **engine username**. The effect mirrors how Apache
Ranger seeds default admin policies on service setup: a freshly provisioned,
Ranger-governed engine is immediately queryable by admins with **no manual
policy creation**.

This is the enabling foundation for sub-projects B and C, whose tests require a
user who already holds real data permissions.

## 2. Current state (what exists today)

- **Provision auto-seeds only plumbing policies.** In
  `crates/egret-engine-mgmt/src/engines.rs` (~lines 622–628), when
  `config.auth.is_some() && engine.is_impersonate_delegation()`, egret injects
  the delegator user and calls `seed_delegator_policies(st, service_name)`
  (engines.rs ~1700–1731), which creates exactly two Ranger policies:
  `egret-delegator-impersonate` (service user may impersonate `trinouser:*`) and
  `egret-query-timeout` (`{USER}` may alter `query_max_run_time`). **Neither
  grants any real table/select access.**
- **A user-facing all-access seeder exists but is manual and group-based.**
  `PolicyStore::seed_default_trino_policies(service_id, service_name,
  admin_principal)` (`crates/egret-policy/src/default_policies.rs:94`) builds one
  Ranger policy per resource-hierarchy leaf of the **Trino** service-def, granting
  `policyItems:[{ "groups":[admin_principal], "users":[], "accesses": <all> }]`.
  Its **only** caller is the manual REST endpoint `POST
  /policies/services/{service}/seed-defaults`
  (`crates/egret-policy/src/admin/mod.rs:655–664`), gated on `policy:seed`, which
  hardcodes the principal to the literal group `"admin"`. It is **not** called on
  provision.
- **No StarRocks equivalent.** There is a StarRocks Ranger service-def
  (`crates/egret-policy/src/service_def.rs`, `starrocks_service_def()`,
  resource-only — no tag policies) but **no** `seed_default_starrocks_policies`.
- **Per-user username seam.** `ensure_username(user_id)` (on the user repo) maps a
  principal to its engine username; it is the same seam used by SQL Lab
  (`crates/egret-query/src/execution/routes.rs:129,149`), BI
  (`crates/egret-bi/src/routes.rs:1821`), and notebook
  (`crates/egret-notebook/src/runtime.rs:34`).
- **Platform-admin identity.** `PrincipalContext::is_platform_admin()`
  (`crates/egret-core/src/context.rs:31`) is true when the principal's platform
  roles contain `"platform_admin"`. There is **no** existing "enumerate all users
  holding `platform_admin`" query — this must be added.

## 3. Requirements (decisions from brainstorming)

| Decision | Choice |
| --- | --- |
| Grantee | The **specific admin user's** engine username (not a group) |
| Which user(s) | **All users holding the `platform_admin` role** |
| Scope | **Full all-access** over every resource hierarchy (Ranger default-admin style) |
| Engines | **Trino and StarRocks** |
| Trigger | **Automatic on provision** when Ranger is enabled + delegation is impersonate or JWT |

## 4. Architecture

Four components. The policy-building stays in `egret-policy` (no identity
dependency); the caller (provision path) resolves *who* the admins are and passes
their usernames down.

### 4.1 Component 1 — generalize the all-access seeder (users + both engines)

**Files:** `crates/egret-policy/src/default_policies.rs`,
`crates/egret-policy/src/service_def.rs` (reuse), `crates/egret-policy/src/lib.rs`
(exports).

- Extract the resource-hierarchy walk (`parse_resources`, `is_hierarchy_leaf`,
  root-to-leaf chain, `all_access_types`) into a private core that takes a
  **service-def `Value`** and a **grantee** (`users: &[String]`, `groups:
  &[String]`) and returns the built policies. This is engine-agnostic — it works
  for any Ranger service-def.
- Rewrite `seed_default_trino_policies` to delegate to the core with the Trino
  service-def and `groups:[admin_principal]` (behavior unchanged — the manual
  endpoint keeps working).
- Add `seed_admin_all_access(service_id, service_name, service_def, users:
  &[String]) -> Result<usize>` that delegates to the core with the given
  service-def and `users:[…], groups:[]`. It builds a stable, **idempotently
  named** policy per hierarchy leaf (e.g. `egret-admin-allaccess-<resource-path>`)
  and calls `PolicyStore::create_policy` as **create-or-update by (service,
  name)** so re-provision / rolling updates neither duplicate nor error.
- Callers pass the Trino service-def or the StarRocks service-def
  (`starrocks_service_def()`), so **StarRocks parity is just a different
  service-def** — no separate seeder logic. (StarRocks is resource-only, which the
  service-def already encodes; no tag policies are emitted.)

### 4.2 Component 2 — enumerate platform-admin grantees

**Files:** the identity/RBAC store (`crates/egret-core` / `egret-identity` /
wherever platform-role grants live) + the provision handler in
`crates/egret-engine-mgmt`.

- Add a query `platform_admin_user_ids() -> Vec<UserId>` returning every user
  granted the `platform_admin` role (via the RBAC/role-assignment store).
- In the provision path, map each id through `ensure_username(user_id)` (the same
  seam the query surfaces use) to build `admin_usernames: Vec<String>`. Skip ids
  with no resolvable username. De-duplicate.

### 4.3 Component 3 — wire into the provision path

**Files:** `crates/egret-engine-mgmt/src/engines.rs`.

- Right after the existing `seed_delegator_policies(...)` call, when a Ranger
  service is present and delegation is on, also call `seed_admin_all_access(...)`
  with the engine's service-def (Trino or StarRocks, chosen by `engine.kind`) and
  `admin_usernames`.
- **Gate:** Ranger service present **and** delegation mode is impersonate or JWT
  (i.e. a per-user identity reaches the engine). When delegation is `none` (no
  per-user identity) or the deploy is non-Ranger, **skip** — there is nothing to
  grant per-user. (The current delegator gate uses `is_impersonate_delegation()`;
  confirm during planning whether JWT mode reaches this branch and widen the gate
  if needed so both impersonate and JWT auto-seed.)
- The provision handler must have access to the identity store to resolve admins
  (Component 2). If `egret-engine-mgmt` state lacks it, thread a small
  `AdminUsernameResolver` (a boxed async fn / trait) into the state so the policy
  layer stays identity-free.
- **Idempotency & rolling updates:** enabling JWT re-provisions the coordinator;
  the seeder must be safe to run again (create-or-update by name), matching how
  `seed_delegator_policies` already tolerates re-runs.

### 4.4 Component 4 — e2e test

**Files:** `frontend/tests/e2e/ranger-auto-admin-kind.spec.ts` (new), reusing
`helpers.ts`; runs on the Ranger-on deploy (`deploy-kind-governance`) under
`EGRET_E2E_KIND=1`. Add a `just` target (e.g. `e2e-kind-ranger-auto`) or fold
into `e2e-kind-governance`.

The test proves the auto-policy — the deliberate contrast with
`governance-kind.spec.ts` is that it seeds **no** access policies by hand:

1. Provision a Ranger-bound Trino engine → enable JWT → assign to the workspace
   (reusing `provisionStorageAndWarehouse` / `provisionTrinoEngine` /
   `enableEngineJwt` / `assignEngineToWorkspace`).
2. Seed one governed Iceberg table (a few rows) via the engine.
3. As the platform admin, run `SELECT * FROM <table>` in SQL Lab (`runQuery`
   API). **Assert it succeeds and returns all rows** — access came *solely* from
   the auto-created admin policy, with no manual `seed-defaults` and no
   hand-written policies.
4. Assert the admin all-access policy exists via the policy API (GET
   policies for the service → a policy whose `policyItems[].users` contains the
   admin's engine username).
5. StarRocks variant: same flow against a provisioned StarRocks engine, proving
   the service-def-driven seeder works for both.

Cleanup (delete engine) runs in `finally` with the live `request`, per the
existing kind-e2e convention.

## 5. Data flow

```
provision engine (Ranger on, delegation=impersonate|jwt)
  └─ engines.rs provision path
       ├─ ensure_delegator_secret + inject_delegator_user
       ├─ seed_delegator_policies(service)                 // existing plumbing
       ├─ admins = platform_admin_user_ids()               // Component 2
       │    .map(ensure_username).dedup()
       └─ seed_admin_all_access(service, <engine service-def>, admins)  // Component 1
            └─ per hierarchy leaf: create-or-update Ranger policy
               { users: admins, groups: [], accesses: <all> }

later — admin runs a query (SQL Lab / BI / notebook):
  OIDC principal → ensure_username → per-user JWT / X-Trino-User
    → Trino/StarRocks → Ranger plugin checks the policy → allowed
```

## 6. Edge cases & error handling

- **Multiple platform admins** → one policy set with all usernames in `users:`.
- **No platform admins resolvable** (empty list) → seed nothing (log a warning);
  provision still succeeds.
- **Delegation = none / non-Ranger deploy** → skip auto-seed entirely.
- **Re-provision / JWT-enable rolling update** → idempotent create-or-update by
  policy name; no duplicates, no failure.
- **Ranger unreachable at seed time** → surface a clear provisioning warning but
  do not fail the whole provision (match `seed_delegator_policies` behavior;
  confirm during planning). The admin can re-run provision or the manual
  `seed-defaults` as a fallback.
- **StarRocks resource-only** → the StarRocks service-def yields resource
  hierarchies only; no tag policies emitted.

## 7. Testing strategy

- **Rust unit** — the generalized core builds the expected policy JSON for a given
  service-def + `users`/`groups` (assert `policyItems`, `accesses`, resource
  chains) for both the Trino and StarRocks service-defs; idempotent naming.
- **Rust unit** — `platform_admin_user_ids()` returns exactly the users holding
  `platform_admin`.
- **e2e (kind, Ranger-on)** — Component 4 above (Trino + StarRocks), the money
  path: provision → admin query succeeds from the auto-policy.
- Run the standard local CI gates before merge (fmt / clippy / workspace test; FE
  lint / typecheck / test / build), per project convention.

## 8. Out of scope (this sub-project)

- **B** — asserting per-user enforcement across SQL Lab → dataset → chart →
  dashboard → notebook (A only proves the admin all-access path in SQL Lab).
- **C** — notebook write isolation to a per-workspace namespace.
- Non-admin user policy management UI, per-user grant workflows beyond the admin
  bootstrap, and any change to the existing manual `seed-defaults` endpoint
  (kept as-is).

## 9. Open questions (resolve during planning)

1. Does JWT-mode delegation reach the same `engines.rs` branch as impersonate, or
   is the gate `is_impersonate_delegation()` too narrow? Widen if needed.
2. Exact home + signature of `platform_admin_user_ids()` (which store owns
   role-assignments) and whether `egret-engine-mgmt` state already has identity
   access or needs a resolver threaded in.
3. Whether `PolicyStore::create_policy` already upserts by (service, name) or
   needs an update path for idempotency.
