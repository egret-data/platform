# Ranger auto-admin policies on provision (Sub-project A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a Trino/StarRocks engine is provisioned with Ranger + per-user delegation (impersonate or JWT), egret auto-creates a Ranger all-access policy granting each platform-admin's engine username, so admins query immediately with no manual policy seeding.

**Architecture:** Generalize the existing Trino default-policy seeder in `egret-policy` to grant *users* (not just a group) and to work for any Ranger service-def (Trino or StarRocks); expose it via `PolicyPort::seed_admin_all_access`; add `RoleRepo::platform_admin_user_ids()` in `egret-auth`; thread `UserRepo`+`RoleRepo` into `EngineMgmtState` and call the seeder from the provision path alongside the delegator seeding.

**Tech Stack:** Rust (axum, sqlx, async-trait, serde_json), Playwright e2e, kind.

**Design doc:** `docs/en/specs/2026-07-20-ranger-auto-admin-policies-design.md`.

**Branch:** `feat/ranger-auto-admin-policies` (already created, off main; the design doc is already committed here).

**Conventions:** Rust cmds from repo root `egret/`. Commit per task. Before merge: `cargo fmt --all --check`, `cargo clippy --workspace`, `cargo test --workspace` (needs `DATABASE_URL=…/egret_test`), FE `npm run lint/typecheck/test/build`. Live e2e on the Ranger-on kind deploy.

---

## Task 1: Generalize the policy seeder + add upsert + admin all-access (egret-policy)

**Files:**
- Modify: `crates/egret-policy/src/default_policies.rs`
- Modify: `crates/egret-policy/src/store.rs` (add `upsert_policy`)
- Test: inline `#[cfg(test)]` in `default_policies.rs` (add cases)

Reuse the existing resource-hierarchy walk (`parse_resources`, `is_hierarchy_leaf`, `hierarchy_chain`, `all_access_types`) — do NOT change them.

- [ ] **Step 1: Write the failing unit tests.** Add to the `tests` module in `default_policies.rs`:

```rust
#[test]
fn build_grants_users_with_service_type() {
    let def = trino_service_def().unwrap();
    let policies = build_default_policies(&def, "eng1", "trino", "egret-admin",
        &["alice".to_string()], &[]);
    // one policy per hierarchy leaf (same 13 as the group default)
    assert_eq!(policies.len(), 13);
    let (name, body) = policies.iter().find(|(n, _)| n == "egret-admin - catalog, schema, table").unwrap();
    assert_eq!(name, "egret-admin - catalog, schema, table");
    assert_eq!(body["serviceType"], "trino");
    assert_eq!(body["policyItems"][0]["users"], json!(["alice"]));
    assert_eq!(body["policyItems"][0]["groups"], json!([]));
    // select must be among the granted accesses for the table hierarchy
    let accesses = body["policyItems"][0]["accesses"].as_array().unwrap();
    assert!(accesses.iter().any(|a| a["type"] == "select"));
}

#[test]
fn build_works_for_starrocks_service_type() {
    let def = crate::service_def::starrocks_service_def().unwrap();
    let policies = build_default_policies(&def, "sr1", "starrocks", "egret-admin",
        &["bob".to_string()], &[]);
    assert!(!policies.is_empty());
    let (_, body) = &policies[0];
    assert_eq!(body["serviceType"], "starrocks");
    assert_eq!(body["policyItems"][0]["users"], json!(["bob"]));
}
```

- [ ] **Step 2: Run tests — expect compile failure** (`build_default_policies` undefined).

Run: `cargo test -p egret-policy default_policies -- --nocapture` → FAIL (unresolved `build_default_policies`).

- [ ] **Step 3: Extract `build_default_policies`.** In `default_policies.rs`, add a free function that produces the `(name, body)` pairs, parameterized by service-def, service type, name prefix, and grantee users/groups. This is the existing loop body from `seed_default_trino_policies`, generalized:

```rust
/// Build one default all-access policy per resource hierarchy of `def`.
/// `service_type` sets the policy `serviceType` (e.g. "trino"/"starrocks");
/// `name_prefix` prefixes each policy name; grantees go in `users`/`groups`.
fn build_default_policies(
    def: &Value,
    service_name: &str,
    service_type: &str,
    name_prefix: &str,
    users: &[String],
    groups: &[String],
) -> Vec<(String, Value)> {
    let resources = parse_resources(def);
    let fallback_accesses = all_access_types(def);
    let mut out = Vec::new();
    for res in &resources {
        if !is_hierarchy_leaf(res, &resources) {
            continue;
        }
        let chain = hierarchy_chain(res, &resources);
        let accesses: &[String] = if res.access_type_restrictions.is_empty() {
            &fallback_accesses
        } else {
            &res.access_type_restrictions
        };
        let mut resources_map = Map::new();
        for r in &chain {
            resources_map.insert(
                r.name.clone(),
                json!({ "values": ["*"], "isExcludes": false, "isRecursive": false }),
            );
        }
        let names: Vec<&str> = chain.iter().map(|r| r.name.as_str()).collect();
        let policy_name = format!("{} - {}", name_prefix, names.join(", "));
        let body = json!({
            "name": policy_name,
            "service": service_name,
            "serviceType": service_type,
            "policyType": 0,
            "isEnabled": true,
            "resources": Value::Object(resources_map),
            "policyItems": [{
                "groups": groups,
                "users": users,
                "roles": [],
                "accesses": accesses.iter().map(|a| json!({ "type": a, "isAllowed": true })).collect::<Vec<_>>(),
                "delegateAdmin": true,
            }],
            "denyPolicyItems": [],
            "allowExceptions": [],
            "denyExceptions": [],
            "isDenyAllElse": false,
        });
        out.push((policy_name, body));
    }
    out
}
```

- [ ] **Step 4: Rewrite `seed_default_trino_policies` to delegate** (behavior unchanged — same names `all - …`, group grantee, empty-guard preserved):

```rust
pub async fn seed_default_trino_policies(
    &self,
    service_id: i64,
    service_name: &str,
    admin_principal: &str,
) -> Result<usize, PolicyStoreError> {
    if !self.list_policies(service_id).await?.is_empty() {
        return Ok(0);
    }
    let def = trino_service_def()?;
    let policies = build_default_policies(
        &def, service_name, "trino", "all", &[], &[admin_principal.to_string()],
    );
    let mut created = 0usize;
    for (name, body) in &policies {
        self.create_policy(service_id, name, body).await?;
        created += 1;
    }
    Ok(created)
}
```

- [ ] **Step 5: Add `seed_admin_all_access`** (no empty-guard; per-name upsert; service-def chosen by `service_type`):

```rust
/// Idempotently grant each user in `users` all-access on every resource
/// hierarchy of the given service type ("trino" or "starrocks"). Upserts by
/// (service, policy name) so re-provision / rolling updates refresh, never
/// duplicate. Returns the number of policies written.
pub async fn seed_admin_all_access(
    &self,
    service_id: i64,
    service_name: &str,
    service_type: &str,
    users: &[String],
) -> Result<usize, PolicyStoreError> {
    if users.is_empty() {
        return Ok(0);
    }
    let def = match service_type {
        "starrocks" => crate::service_def::starrocks_service_def()?,
        _ => trino_service_def()?,
    };
    let policies = build_default_policies(
        &def, service_name, service_type, "egret-admin", users, &[],
    );
    let mut written = 0usize;
    for (name, body) in &policies {
        self.upsert_policy(service_id, name, body).await?;
        written += 1;
    }
    Ok(written)
}
```

- [ ] **Step 6: Add `PolicyStore::upsert_policy`** in `store.rs` (model on `create_policy` at store.rs:252, but `ON CONFLICT (service_id, name)` — the unique constraint per `migrations/0007_authz.sql:26`):

```rust
/// Create the policy, or update its JSON if one already exists with the same
/// (service_id, name). Bumps the service policy version. Returns the row id.
pub async fn upsert_policy(
    &self,
    service_id: i64,
    name: &str,
    policy: &Value,
) -> Result<i64, PolicyStoreError> {
    let mut tx = self.db.pool.begin().await?;
    let id: i64 = sqlx::query_scalar::<_, i64>(
        r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb)
           VALUES ($1,$2,$3)
           ON CONFLICT (service_id, name)
           DO UPDATE SET policy_jsonb = EXCLUDED.policy_jsonb,
                         version = policy.authz_policies.version + 1,
                         updated_at = now()
           RETURNING id"#,
    )
    .bind(service_id)
    .bind(name)
    .bind(policy)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
    )
    .bind(service_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}
```

Confirm the real column names (`version`, `policy_jsonb`) against `create_policy` + the migration; adjust the `ON CONFLICT` update columns to match exactly.

- [ ] **Step 7: Run tests + clippy.**

Run: `cargo test -p egret-policy default_policies` → PASS (the two new build tests + the existing 3).
Run: `cargo clippy -p egret-policy` → clean.

- [ ] **Step 8: Commit.**

```bash
git add crates/egret-policy/src/default_policies.rs crates/egret-policy/src/store.rs
git commit -m "feat(policy): generalize default-policy seeder to users + any service-def; add upsert_policy + seed_admin_all_access"
```

---

## Task 2: Expose `seed_admin_all_access` on `PolicyPort` (egret-policy-api + port_impl)

**Files:**
- Modify: `crates/egret-policy-api/src/port.rs` (trait method)
- Modify: `crates/egret-policy/src/port_impl.rs` (impl)
- Modify: any other `impl PolicyPort` (test doubles) — find first

- [ ] **Step 1: Find all `PolicyPort` implementations** so none break when a required method is added.

Run: `grep -rn "impl PolicyPort\|impl .*PolicyPort for" crates` — note each (the real `port_impl.rs`, plus any test stubs/mocks).

- [ ] **Step 2: Add the trait method** to `PolicyPort` (port.rs, after `create_policy` ~line 58):

```rust
    /// Seed idempotent admin all-access policies for `users` on the given
    /// service type ("trino"/"starrocks"). Returns the count written.
    async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyError>;
```

- [ ] **Step 3: Implement in `port_impl.rs`** (delegate to the concrete store, mirroring the other delegating methods):

```rust
    async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyError> {
        Ok(self.seed_admin_all_access(service_id, service_name, service_type, users).await?)
    }
```

- [ ] **Step 4: Update any other `PolicyPort` impls** found in Step 1 (test doubles) to add the method (a trivial `Ok(0)` for stubs is fine).

- [ ] **Step 5: Build.**

Run: `cargo build -p egret-policy-api -p egret-policy` → compiles.

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-policy-api/src/port.rs crates/egret-policy/src/port_impl.rs
git commit -m "feat(policy-api): expose seed_admin_all_access on PolicyPort"
```

---

## Task 3: `RoleRepo::platform_admin_user_ids()` (egret-auth)

**Files:**
- Modify: `crates/egret-auth/src/roles.rs` (new method, model on `count_platform_admins` at roles.rs:143)
- Test: inline `#[sqlx::test]` in `roles.rs`

- [ ] **Step 1: Write the failing test** (model on existing `roles.rs` sqlx tests; seed a platform_admin user grant, assert it is returned):

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn lists_platform_admin_user_ids(pool: sqlx::PgPool) {
    let repo = RoleRepo::new(/* db handle per existing test setup */);
    // seed: a user + a platform-scoped platform_admin grant in access_grants
    // (reuse the crate's existing test seeding helpers; see count_platform_admins tests)
    let ids = repo.platform_admin_user_ids().await.unwrap();
    assert!(ids.iter().any(|id| *id == seeded_user_id));
}
```

Match the exact `RoleRepo` construction + seeding helpers already used by the `count_platform_admins` tests in this file.

- [ ] **Step 2: Run — expect FAIL** (method undefined).

Run: `DATABASE_URL=postgres://…/egret_test cargo test -p egret-auth platform_admin_user_ids` → FAIL.

- [ ] **Step 3: Implement** (model on `count_platform_admins`, roles.rs:143 — same join, select distinct user principal ids):

```rust
/// User ids directly granted the platform_admin role at platform scope.
/// (Direct user grants only — group-granted admins are not expanded here;
/// see the note in the design doc §6. The seeded platform admin is a direct
/// grant, which is what auto-seeding needs.)
pub async fn platform_admin_user_ids(&self) -> Result<Vec<UserId>> {
    let rows = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"SELECT DISTINCT pra.principal_id
             FROM access_grants pra
             JOIN roles r ON r.id = pra.role_id
            WHERE r.name = 'platform_admin'
              AND pra.scope_type = 'platform'
              AND pra.principal_type = 'user'"#,
    )
    .fetch_all(self.pool())
    .await?;
    Ok(rows.into_iter().map(UserId).collect())
}
```

Adjust `self.pool()` / the `UserId` newtype wrap to match this crate's exact repo accessors + `egret_core::ids::UserId` shape.

- [ ] **Step 4: Run — PASS.** `DATABASE_URL=… cargo test -p egret-auth platform_admin_user_ids` → PASS. `cargo clippy -p egret-auth` → clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-auth/src/roles.rs
git commit -m "feat(auth): RoleRepo::platform_admin_user_ids() (direct platform grants)"
```

---

## Task 4: Thread identity into `EngineMgmtState` + wire admin seed into provision (egret-engine-mgmt + egret-bin)

This is the integration task — adding state fields breaks every `EngineMgmtState` construction site, so it must compile atomically across the workspace.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/state.rs` (add `users`, `roles` fields + constructor params)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (helper + provision wiring)
- Modify: `crates/egret-bin/src/main.rs` (pass repos when constructing `EngineMgmtState`)
- Modify: any `EngineMgmtState` construction in `egret-engine-mgmt` tests

- [ ] **Step 1: Find every `EngineMgmtState` construction site.**

Run: `grep -rn "EngineMgmtState" crates --include=*.rs | grep -iE "new\(|EngineMgmtState \{|::new"` — enumerate the constructor + all call sites (egret-bin + tests).

- [ ] **Step 2: Add fields to `EngineMgmtState`** (state.rs). `UserRepo` and `RoleRepo` are both from `egret_auth` (already a dependency). Add:

```rust
use egret_auth::users::UserRepo;
use egret_auth::roles::RoleRepo;
// … in the struct:
    pub users: UserRepo,
    pub roles: RoleRepo,
```

Add the two params to the state's constructor (`new(...)`) and set the fields. Match the crate's exact `UserRepo`/`RoleRepo` type paths.

- [ ] **Step 3: Add the `admin_usernames` helper** in `engines.rs` (near `seed_delegator_policies`):

```rust
/// Engine usernames for every platform admin (deduped). Missing/unresolvable
/// usernames are skipped; an empty list is valid (nothing to seed).
async fn admin_usernames(st: &CpState) -> Result<Vec<String>, CpError> {
    let ids = st.roles.platform_admin_user_ids().await
        .map_err(|e| CpError::BadRequest(format!("list platform admins: {e}")))?;
    let mut names = Vec::new();
    for id in ids {
        if let Ok(u) = st.users.ensure_username(id).await {
            if !names.contains(&u) {
                names.push(u);
            }
        }
    }
    Ok(names)
}
```

Match the exact `CpError` variant used elsewhere in the file and the `platform_admin_user_ids` / `ensure_username` error handling.

- [ ] **Step 4: Wire the admin seed into the provision path** (engines.rs, right after the impersonate delegator block at ~line 628). Gate on a Ranger service present AND delegation supported (impersonate OR jwt), so both modes seed:

```rust
    // Ranger auto-admin (sub-project A): grant each platform admin all-access on
    // the bound Ranger service, so a governed engine is immediately queryable by
    // admins with no manual policy seeding. Fires for impersonate AND jwt (both
    // reach the engine with a per-user identity). Idempotent (upsert by name).
    if let Some(service_name) = ranger_service_name.as_deref() {
        if engine.supports_delegation() {
            if let Some(svc) = st.authz.get_service_opt(service_name).await
                .map_err(|e| CpError::BadRequest(format!("resolve ranger service: {e}")))?
            {
                let service_type = if engine.is_starrocks() { "starrocks" } else { "trino" };
                let admins = admin_usernames(st).await?;
                st.authz
                    .seed_admin_all_access(svc.id, service_name, service_type, &admins)
                    .await
                    .map_err(|e| CpError::BadRequest(format!("seed admin policy: {e}")))?;
            }
        }
    }
```

Confirm `engine.is_starrocks()` exists (store.rs referenced it in Q1); if the method name differs, use the correct kind check.

- [ ] **Step 5: Update `egret-bin` construction** (main.rs). Where `EngineMgmtState::new(...)` is built, pass the already-constructed `UserRepo` + `RoleRepo` (the same ones wired for auth/session). If they aren't in scope there, construct them from the shared DB handle exactly as the auth wiring does.

- [ ] **Step 6: Update test construction sites** found in Step 1 to pass `UserRepo`/`RoleRepo` built from the test pool.

- [ ] **Step 7: Build + test the workspace.**

Run: `cargo build --workspace` → compiles.
Run: `DATABASE_URL=postgres://…/egret_test cargo test -p egret-engine-mgmt` → existing tests pass (delegation-mode tests at engines.rs:4993 etc. still green).
Run: `cargo clippy --workspace` → clean.

- [ ] **Step 8: Commit.**

```bash
git add crates/egret-engine-mgmt/src/state.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-bin/src/main.rs
git commit -m "feat(engine-mgmt): auto-seed Ranger admin all-access on provision (impersonate+jwt)"
```

---

## Task 5: e2e — auto-admin policy proves query without manual seeding

**Files:**
- Create: `frontend/tests/e2e/ranger-auto-admin-kind.spec.ts`
- Modify: `justfile` (add `e2e-kind-ranger-auto` target, or extend `e2e-kind-governance`)

Model closely on `governance-kind.spec.ts` — but the point is it seeds **no** access policies by hand.

- [ ] **Step 1: Write the spec.** Opt-in (`EGRET_E2E_KIND=1`), `test.setTimeout(900_000)`, `loginLocalAdmin`. Reuse `helpers.ts`: `csrf`, `firstWorkspace`, `provisionStorageAndWarehouse`, `provisionTrinoEngine`, `waitForEngineReady`, `enableEngineJwt`, `assignEngineToWorkspace`, `runQuery`/`runSqlInLab`, `deleteEngine`, `engineProfileName`, `createPolicy` (only for the seeded *table*, not access grants).

Test "auto-admin: provision Ranger Trino → admin SELECT succeeds from the auto-seeded policy":
1. `provisionStorageAndWarehouse` → `provisionTrinoEngine` → `waitForEngineReady` → `enableEngineJwt` → `waitForEngineReady` → `assignEngineToWorkspace`.
2. Seed a small governed Iceberg table via the engine (mirror `governance-kind`'s table-seed helper).
3. As the platform admin, `runQuery("SELECT * FROM <catalog>.<schema>.<table>")` → **assert it succeeds and returns the seeded rows** — with NO hand-written access/mask/row-filter policy (the contrast with `governance-kind`).
4. Assert the policy exists: GET the service's policies via the policy API and assert one whose `policyItems[].users` includes the admin's engine username (compute via `engineProfileName`/the same slug helper the suite uses).
5. `finally { deleteEngine(...) }`.

Add a StarRocks variant of the same flow if a StarRocks provisioning helper exists; otherwise note StarRocks e2e as a follow-up and cover StarRocks by the Task 1 unit test for now.

- [ ] **Step 2: Add the `just` recipe** (mirrors `e2e-kind-governance`, which deploys Ranger-on):

```makefile
e2e-kind-ranger-auto: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test ranger-auto-admin-kind
```

- [ ] **Step 3: Typecheck + lint the spec.**

Run (from `frontend/`): `npm run typecheck` and `npx eslint tests/e2e/ranger-auto-admin-kind.spec.ts` → clean.

- [ ] **Step 4: Commit.**

```bash
git add frontend/tests/e2e/ranger-auto-admin-kind.spec.ts justfile
git commit -m "test(security): e2e — Ranger auto-admin policy lets admin query with no manual seeding"
```

---

## Task 6: Verify + live-deploy + run the e2e

- [ ] **Step 1: Full local CI.** `cargo fmt --all --check`; `cargo clippy --workspace` (0 warnings); `DATABASE_URL=postgres://…/egret_test cargo test --workspace`; FE `npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] **Step 2: Deploy the Ranger-on kind stack** and run the e2e: `just e2e-kind-ranger-auto` (builds images, deploys `ranger.enabled=true`, waits, runs the spec).
- [ ] **Step 3: Live check** — the auto-admin test is green: a freshly provisioned Ranger Trino engine is immediately queryable by the admin with no manual policy, and the `egret-admin - …` policies exist on the service. (Cluster-capacity caveat: provisioning a Trino engine needs node headroom — see prior kind-e2e notes.)

---

## Self-Review Notes
- The generalized `build_default_policies` keeps `seed_default_trino_policies` byte-for-byte equivalent (same names `all - …`, group grantee, empty-guard) — the manual `seed-defaults` endpoint is unchanged.
- Idempotency: admin seed uses `upsert_policy` (`ON CONFLICT (service_id, name)`), so JWT-enable rolling re-provision refreshes rather than duplicates/errors — unlike the delegator seed's `policy_exists` skip (fine, since the delegator policies never change).
- Gate is `supports_delegation()` (impersonate **or** jwt), deliberately wider than the impersonate-only delegator block — jwt engines are Ranger-bound and reach the engine with the caller's JWT, so admins need the grant there too.
- StarRocks parity is "free" in Task 1 (same builder, different service-def); the e2e StarRocks arm is best-effort depending on a provisioning helper — the unit test covers the StarRocks policy shape regardless.
- Known limitation (design §6): `platform_admin_user_ids()` returns *direct* user grants only; group-granted admins aren't expanded. The seeded platform admin is a direct grant, so the e2e passes; group expansion is a documented follow-up.
- Out of scope: sub-projects B (per-user enforcement across SQL Lab→dataset→chart→dashboard→notebook) and C (notebook write isolation).
