# Engine Lifecycle Hardening — Part B (Provisioning) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the provisioning defects of SP1 §4 (P1–P8, C1, D9): random stored internal secrets with rotation, no unbound Trino while Ranger is on, full create compensation, update rollback, Trino rematerialization, apply-before-delete catalog removal, a constrained lifecycle status, a liveness-checked StarRocks bootstrap, redaction for every catalog kind, and connector-first catalog kinds.

**Architecture:** All engine-side changes stay inside `crates/egret-engine-mgmt` (handlers in `engines.rs`, rows in `store.rs`), with two small seam edits: `PolicyPort::delete_service` (`egret-policy-api` + `egret-policy`) and `StarRocksProvisioner::update` (`egret-provisioner`). Every Trino apply goes through one pipeline — `build_provision_req_with` → `apply_built` — which mints internal secrets as *pending* and records them on the engine row only after Kubernetes accepted them, so the monitor always reads the password the pods actually carry. Three migrations (0144–0146) drop the catalog `kind` CHECK, constrain `engines.status`, and add the two secret columns. One new route (`POST /api/control-plane/engines/{id}/rotate-secrets`) with a button in EngineDetail → Settings → Security & Exposure.

**Tech Stack:** Rust (axum 0.8, sqlx/Postgres, utoipa, tokio, bcrypt 0.19, rand 0.8, base64 0.22, regex 1), React 18 + TanStack Query 5 + vitest, orval via `npm run gen:api`.

**Spec:** `docs/en/specs/2026-09-15-engine-lifecycle-hardening-design.md` §4 items 1–10 and the Part-B bullets of §5.

## Global Constraints

- Branch: `fix/engine-provisioning-hardening`, cut from `origin/main` **after Part A has merged**. Never commit on `main`; check `git branch --show-current` before every commit.
- Internal secrets: "32 random bytes (base64)" — 32 bytes from `rand::thread_rng()` (rand 0.8), encoded `base64::engine::general_purpose::URL_SAFE_NO_PAD` (the encoding `ensure_delegator_secret` already uses; no `:`, so it is safe inside a Basic-auth pair).
- "the derived value is never used again once a secret row exists" — `egret-internal-{id}` / `egret-monitor-{id}` may be rendered or presented **only** while `engines.internal_secret_id` / `engines.monitor_secret_id` are NULL.
- Rotate route: `POST /api/control-plane/engines/{id}/rotate-secrets`, permission `engine:update`, "pods will restart" confirmation in the UI.
- Status: `status ∈ {provisioning, running, failed, deleting}` with a CHECK; the migration first maps any other stored value to `provisioning`. Part A already writes `deleting`.
- Redaction: for every kind, `credential`, `clientRowId` and any property whose key matches `(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)`.
- Re-create with the same name: "if the existing profile has no bound engine, reuse it (and its tag service); if it is bound, fail the create with 409. While `config.ranger.enabled`, a Trino engine is never provisioned unbound."
- Update failure: "on Kubernetes failure restore the snapshot and return 502 … A missing `delegationMode` keeps the stored value."
- Catalog kind: "Migration drops the `kind` CHECK on `engine_mgmt.engine_catalogs`; validation stays in `add_catalog`."
- DB tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` (user/password/host from `.env`, database name swapped). **Never** the dev database `egret`. DB-gated tests self-skip only under `EGRET_ALLOW_DB_SKIP`; still read the `finished in` time of every DB run — a DB test that finishes in well under 0.3 s did no database work.
- After adding a migration, if a test run dies with `VersionMissing` or a checksum error from `egret_test_template`, run `psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS egret_test_template'` once and re-run. Do not drop it pre-emptively (other branches share it). The migrations CLI reads `EGRET_DATABASE__URL`, not `DATABASE_URL`.
- Never run two workspace test suites at once. Per-task runs use `-p egret-engine-mgmt` (or the named crate); `cargo test --workspace` runs exactly once, in Task 16.
- OpenAPI: regenerate with `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json` (without `rtk proxy` the RTK hook filters stdout and corrupts the file); the drift gate is `cargo test -p egret-server --lib committed_openapi_json_is_current`. Frontend client: `cd frontend && npm run gen:api` only — never bare `orval` — and keep only the generated files the change needs.
- Frontend gates (in `frontend/`): `npm run lint`, `npm run typecheck`, `npm test`, `npm run build`. No prettier.
- Rust gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
- Commits: Conventional Commits; body ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Author identity is the repo default — never override it.
- RTK rewrites shell commands. Any command whose raw stdout is piped or redirected (`cargo test … | tail`, `cargo run … > openapi.json`, `grep … | wc`) runs as `rtk proxy <cmd>`; `grep` through plain `rtk` also swallows piped stdin.

## Spec statements verified against the code, and the decisions they forced

Each §2.2 claim was re-read in source on `docs/control-plane-dg-review` (same code as `main` for these files). Line numbers below are pre-Part-A.

| Claim | Verdict | Consequence in this plan |
|---|---|---|
| P1 derivable secrets (`engines.rs:659`, `:1457`) | **True.** Both are `format!` of the engine UUID; `monitor.rs:149` re-derives the monitor password. | Tasks 6–9. |
| P2 "leaves applied Kubernetes objects" | **Mostly false.** `TrinoProvisioner::provision` and `StarRocksProvisioner::provision` already call `self.delete` when `apply_all` fails. The real leak on StarRocks is that `delete` returns early on the CR's 404 (fixed by Part A's 404 tolerance). | Task 12 still calls `prov.delete` once (harmless, 404-tolerant after Part A) so a failure *after* a successful apply is also covered. |
| P2 "TLS/delegator secret rows" leak | **True.** `build_provision_req` mints `engine-tls-{id}` / `egret-delegator-{id}`; StarRocks create mints `egret-root-{id}`; none are deleted on rollback. | Task 12. |
| P2 "policies seeded for this engine" leak | **Partly false.** Trino create seeds nothing before its rollback points (binding happens after provisioning today). StarRocks create seeds admin all-access into a **pre-existing, possibly shared** profile. Deleting those policies would strip them from every other engine bound to that profile. | **Decision:** compensation deletes policies only by deleting a profile/tag service that *this request created* (cascade `authz_policies.service_id ON DELETE CASCADE`). Policies in a pre-existing profile are never touched. |
| P3 Trino update persists then 400s | **True** (`engines.rs:3649-3664`); the "kubernetes not configured" check also runs after the persist, and a `build_provision_req` error after the persist is not restored either. | Task 10 checks Kubernetes first, restores on build *and* apply failure. |
| P3 "mirrors `update_catalog`" returning 502 | **Wrong detail.** `update_catalog` restores but returns **400**. | **Decision:** engine update and rotate return 502 via a new `EngineHttpError::ClusterApplyFailed(String)`; the three catalog routes keep 400. |
| P3 missing `delegationMode` reads back as `impersonate` | **True** when the body carries a `trinoConfig` without the key (`store.rs:123-127`). | Task 10. |
| (not in spec) StarRocks update on apply failure | **Worse than P3:** `StarRocksProvisioner::update` calls `self.provision`, whose failure path **deletes the whole cluster** (`starrocks.rs:841-842`). Restoring the spec cannot undo that. | **Added** Task 5: `update` calls `apply_all` directly. Task 10 gives StarRocks update the same snapshot/restore/502. |
| P4 Trino never rematerializes | **True.** | Task 10 runs `rematerialize_for_engine` unconditionally for Trino, for the same retry-healing reason the StarRocks path documents (`engines.rs:3764-3772`). |
| P5 `remove_catalog` order | **True** (`engines.rs:4606-4625`); with no provisioner it deletes and returns 200. | Task 11; no provisioner → 400 before any write (matches `add_catalog`/`update_catalog`). |
| P6 status never written | **True.** `set_engine_status` has no callers; every write is the column default. The only non-`provisioning` stored value anywhere is a test seed of `'running'` (`egret-query/tests/registry_workspace_routes_api.rs:648`). The UI **already** shows live-else-stored (`get_engine` falls back to the row; `ControlPlaneEngines` passes `fallback={e.status}`), so no frontend change is needed for item 7. | Task 4. |
| §4.7 "a failed provision that is not rolled back sets `failed`" | **No such path exists** — every provision failure rolls back. | **Decision:** `failed` is written when create compensation cannot delete the engine row (Task 12). |
| P7 bootstrap does not re-check the engine | **True** (`engines.rs:3340-3508`). | Task 14. |
| P8 redaction only for `egret-warehouse` | **True.** But the edit form PUTs back what GET returned (`CatalogForm.tsx:108,249`), so plain masking would overwrite the real password with the mask on the next save. | **Decision:** GET replaces sensitive values with `********`; `update_catalog` restores stored values for keys that still carry `********`. `secretProperties` is left alone — its values are secret *ids*, and its key contains "secret". |
| C1 kind CHECK | **True** (`0057:10`; no later migration touches it; `create_engine_catalog` maps the violation to `Db` → 500). | Task 2. |
| D9 unbound on `AlreadyExists` | **True** (`engines.rs:2239-2249`). | Task 13 decides the profile before minting anything and binds before the first provision. |
| §4.1 secrets created "on the next apply (update, catalog change, restart)" | **Restart is not an apply.** `restart_engine` only patches a timestamp; it renders no env Secret or `password.db`. Minting on restart would switch the monitor to a password no pod has. | **Decision:** secrets are minted only where a spec is rendered and applied (create, update, add/update/remove catalog, rotate). |
| §4.1 rotate "regenerates both" for any engine | StarRocks has neither secret (its analogues are the root and delegator passwords). | **Decision:** rotate on a non-Trino engine → 400. |
| §2.1 D9 lists `tls_cert_secret_ref` among the secrets to delete | **Wrong.** It references an **admin-provided** `kubernetes.io/tls` secret that may be a shared company certificate (`0089`). | **Decision:** never deleted; `engine_owned_secret_ids` (Task 12) excludes it, and Task 15 applies the same list to Part A's delete. |
| §5 "rotate changes … the password.db hash" | **Unfalsifiable as written:** `bcrypt::hash` salts randomly, so `password.db` changes on *every* apply. | Tests verify the new password with `bcrypt::verify` and that the old one no longer verifies. |

Also found while reading, fixed in passing: Trino create runs `validate_spec_resource_groups(&spec)?` **after** minting the catalog client (`engines.rs:2099`), so an invalid resource-group spec leaks a client. Task 12 validates before minting.

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `migrations/0144_engine_catalogs_drop_kind_check.sql` (create) | Drop `engine_catalogs_kind_check`. | 2 |
| `migrations/0145_engine_status_check.sql` (create) | Normalise then CHECK `engines.status`. | 4 |
| `migrations/0146_engine_internal_secrets.sql` (create) | `internal_secret_id`, `monitor_secret_id` columns. | 6 |
| `crates/egret-engine-mgmt/src/store.rs` | `Engine` fields; `engine_status` consts; `transition_engine_status`, `set_internal_secret_ids`, `count_engines_bound_to_authz_service`. | 4, 6, 13 |
| `crates/egret-engine-mgmt/src/lib.rs` | Re-export `engine_status`. | 4 |
| `crates/egret-engine-mgmt/src/http_error.rs` | `ClusterApplyFailed(String)` → 502. | 8 |
| `crates/egret-engine-mgmt/src/engines.rs` | All handler changes; new helpers (redaction, internal secrets, apply pipeline, compensation, profile planning, bootstrap liveness). | 3–4, 7–8, 10–15 |
| `crates/egret-engine-mgmt/src/monitor.rs` | Monitor reads the stored password. | 7 |
| `crates/egret-engine-mgmt/src/assignment.rs`, `tests/store.rs` | `Engine` literal fixtures gain the two fields. | 6 |
| `crates/egret-engine-mgmt/Cargo.toml` | `regex.workspace = true`. | 3 |
| `crates/egret-engine-mgmt/tests/routes.rs` | Recording/failing `FakeEnginePort`, Ranger fixture, secret helpers, route tests. | 1–4, 7–8, 10–13, 15 |
| `crates/egret-engine-mgmt/tests/store.rs` | Status CHECK/transition test. | 4 |
| `crates/egret-provisioner/src/starrocks.rs`, `src/kube_client.rs` | `update` never deletes; `FakeKube` apply-failure injection. | 5 |
| `crates/egret-policy-api/src/port.rs`, `crates/egret-policy/src/port_impl.rs` | `PolicyPort::delete_service`. | 12 |
| `openapi.json` | Regenerated. | 8, 10, 11 |
| `frontend/src/api/generated/control-plane-engines/control-plane-engines.ts` | Regenerated client. | 8 |
| `frontend/src/api/controlPlane.ts` | `rotateEngineSecrets`. | 8 |
| `frontend/src/routes/control-plane/RotateSecretsSection.tsx` (create) | Rotate button + confirmation. | 9 |
| `frontend/src/routes/control-plane/EngineDetail.tsx` | Thread `engineId` into the Security & Exposure section. | 9 |
| `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx` | Rotate test. | 9 |

---

### Task 1: Route-test harness — recording, failing provisioner and Ranger fixture

**Files:**
- Modify: `crates/egret-engine-mgmt/tests/routes.rs:23-30` (Fixture), `:56-92` (build_app), `:98-111` (attach_real_pdp), `:122-156` (setup_full), `:376-423` (FakeEnginePort), after `:1430` (helpers)

**Interfaces:**
- Produces (test-only, used by Tasks 2–4, 7, 8, 10–13, 15):
  - `struct FakeEnginePort { restarted, provisioned: Arc<Mutex<Vec<EngineSpec>>>, updated: Arc<Mutex<Vec<EngineSpec>>>, deleted: Arc<Mutex<Vec<(String, String, String)>>>, fail_provision: Arc<AtomicBool>, fail_update: Arc<AtomicBool> }` (`Clone + Default`; all fields shared through `Arc`, so a clone kept by the test sees what the app did).
  - `async fn setup_with_ranger(engine_provisioner, kube, ranger_enabled: bool) -> Option<Fixture>`; `setup_full(p, k)` = `setup_with_ranger(p, k, false)`.
  - `fn secret_store(f: &Fixture) -> egret_secrets::WorkspaceSecretStore` (same test crypto key as `build_app`).
  - `async fn engine_secret_ids(f: &Fixture, engine_id: Uuid) -> (Option<Uuid>, Option<Uuid>)` — added in Task 6 (needs the columns), declared here for reference.
  - `async fn create_engine_via_api(f: &Fixture, token: &str, name: &str, wh_id: Uuid, trino_config: serde_json::Value) -> (StatusCode, serde_json::Value)`.

Before starting, confirm the preconditions:

- [ ] **Step 0: Branch off main with Part A merged**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin
git switch -c fix/engine-provisioning-hardening origin/main
git branch --show-current            # expect: fix/engine-provisioning-hardening
rtk proxy grep -rn "engine_has_dependents" crates/egret-engine-mgmt/src | head -3   # expect ≥1 hit (Part A is in)
ls migrations | tail -1              # expect: 0143_workflow_resume_terminal_warning.sql
```

If the last migration is newer than `0143`, renumber this plan's `0144/0145/0146` to the next three free numbers everywhere they appear (file names and commit commands). If Part A's marker is missing, stop: Part B must not start before Part A merges.

- [ ] **Step 1: Extend `FakeEnginePort`**

Replace the struct and its impl (`routes.rs:376-423`) with:

```rust
/// Kube-free `EngineProvisioningPort` fake. `status` reports a single running,
/// ready pod named "coordinator-0" (so the pod-ownership check in
/// `pod_logs`/`pod_exec` passes for that name). Every other call is RECORDED in
/// shared `Arc` state, so a test that keeps a clone of the fake can inspect
/// exactly what the app asked the cluster to do. `fail_provision`/`fail_update`
/// make the matching call record its spec and then fail, standing in for a
/// Kubernetes API error. The kube-shaped ops (exec/pod-logs/coordinator-request)
/// are NOT on this trait — `egret_provisioner::kube_client::FakeKube` covers
/// those, passed separately as the fixture's `kube` field.
#[derive(Clone, Default)]
struct FakeEnginePort {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    provisioned: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    updated: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    deleted: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    fail_provision: Arc<std::sync::atomic::AtomicBool>,
    fail_update: Arc<std::sync::atomic::AtomicBool>,
}

impl FakeEnginePort {
    fn last_provisioned(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.provisioned
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("provision was called")
    }
    fn last_updated(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.updated
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("update was called")
    }
    fn set_fail_update(&self, fail: bool) {
        self.fail_update
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    fn set_fail_provision(&self, fail: bool) {
        self.fail_provision
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl EngineProvisioningPort for FakeEnginePort {
    async fn provision(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.provisioned.lock().unwrap().push(req.clone());
        if self.fail_provision.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected provision failure".into());
        }
        Ok(())
    }
    async fn status(
        &self,
        _kind: &str,
        _ns: &str,
        _name: &str,
    ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
        Ok(egret_engine_mgmt_api::EngineStatus {
            phase: "running".into(),
            message: None,
            pods: vec![egret_engine_mgmt_api::PodStatus {
                name: "coordinator-0".into(),
                phase: "Running".into(),
                ready: true,
            }],
        })
    }
    async fn update(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.updated.lock().unwrap().push(req.clone());
        if self.fail_update.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected update failure".into());
        }
        Ok(())
    }
    async fn delete(&self, kind: &str, ns: &str, name: &str) -> Result<(), String> {
        self.deleted
            .lock()
            .unwrap()
            .push((kind.to_string(), ns.to_string(), name.to_string()));
        Ok(())
    }
    async fn restart(&self, _kind: &str, ns: &str, name: &str) -> Result<(), String> {
        self.restarted
            .lock()
            .unwrap()
            .push((ns.to_string(), name.to_string()));
        Ok(())
    }
}
```

Part A already changed `FakeEnginePort` ("delete recording/injection", its Task 4). Keep Part A's fields and its `delete` behaviour (failure injection included); add only the fields above that are missing. If Part A records deletes under another name or shape, keep Part A's and use it wherever this plan reads `port.deleted` (Task 12 Step 9) instead of adding a second record.

- [ ] **Step 2: Thread `ranger_enabled` through the fixture**

In `struct Fixture` add `ranger_enabled: bool,`. Change `build_app`'s signature and body:

```rust
fn build_app(
    db: &Db,
    sessions: &SessionService,
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    ranger_enabled: bool,
) -> axum::Router {
```

and inside the `EngineMgmtState { … }` literal replace `ranger_enabled: false,` with `ranger_enabled,`. In `attach_real_pdp` pass `f.ranger_enabled` as the new last argument. Replace `setup_full` with:

```rust
async fn setup_full(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
) -> Option<Fixture> {
    setup_with_ranger(engine_provisioner, kube, false).await
}

/// `setup_full` with `config.ranger.enabled` under the test's control: the Trino
/// create path auto-creates (or reuses) a Ranger profile only when it is on.
async fn setup_with_ranger(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    ranger_enabled: bool,
) -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    // Existing route tests authenticate a platform_admin and assert non-authz
    // behavior, so an allow-all PDP suffices. The authz test rebuilds with a real
    // Cedar PDP via `attach_real_pdp`.
    let app = build_app(
        &db,
        &sessions,
        engine_provisioner.clone(),
        kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
        ranger_enabled,
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        engine_provisioner,
        kube,
        ranger_enabled,
    })
}
```

- [ ] **Step 3: Add the shared helpers**

Directly after `seed_secret` (ends at `routes.rs:1430`) add:

```rust
/// The app's named-secret store, built with the SAME test crypto key as
/// `build_app`, so a test can read back what the handlers sealed.
fn secret_store(f: &Fixture) -> egret_secrets::WorkspaceSecretStore {
    egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    )
}

/// `POST /engines` for a Trino engine with the wizard defaults and the given
/// `trinoConfig`; returns the status and body.
async fn create_engine_via_api(
    f: &Fixture,
    token: &str,
    name: &str,
    wh_id: Uuid,
    trino_config: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": trino_config,
            })),
        ))
        .await
        .unwrap();
    into_json(resp).await
}
```

- [ ] **Step 4: Prove the harness still drives every existing test**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes 2>&1 | tail -5
```

Expected: `test result: ok.` with the same test count as before the change and `finished in` well above 1 s (these tests hit the database).

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # fix/engine-provisioning-hardening
git add crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
test(engine-mgmt): record provisioner calls and inject apply failures

FakeEnginePort now records every provision/update/delete spec and can fail
provision or update on demand; the fixture can enable Ranger. Part B's route
tests need both to observe what reached the cluster and what was rolled back.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Drop the legacy catalog `kind` CHECK (C1)

**Files:**
- Create: `migrations/0144_engine_catalogs_drop_kind_check.sql`
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (append)

**Interfaces:**
- Consumes: `create_engine_via_api` (Task 1).
- Produces: nothing new in code; `engine_mgmt.engine_catalogs.kind` accepts any text.

- [ ] **Step 1: Write the failing test**

Append to `routes.rs`:

```rust
/// C1: `add_catalog` is connector-first — any non-blank kind is a Trino
/// connector name. The 0057 CHECK (`egret-warehouse|postgresql|tpch|tpcds|generic`)
/// turned a `mysql` catalog into a 500.
#[tokio::test]
async fn add_catalog_accepts_a_connector_kind_outside_the_legacy_enum() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_c1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "shop",
                "kind": "mysql",
                "config": { "properties": {
                    "connection-url": "jdbc:mysql://db:3306",
                    "connection-user": "reader"
                } }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add mysql catalog; body={j}");
    assert_eq!(j["kind"], "mysql", "stored kind; body={j}");
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes add_catalog_accepts_a_connector_kind_outside_the_legacy_enum 2>&1 | tail -15
```

Expected: FAIL — `add mysql catalog` got `500 Internal Server Error`.

- [ ] **Step 3: Write the migration**

`migrations/0144_engine_catalogs_drop_kind_check.sql`:

```sql
-- 0144_engine_catalogs_drop_kind_check.sql
-- Catalogs are connector-first (v2): the stored `kind` IS the Trino connector
-- name (mysql, iceberg, clickhouse, ...), validated in `add_catalog`. The 0057
-- column CHECK still limited it to the v1 enum, so every newer connector
-- failed at INSERT with a 500. Postgres named the inline column constraint
-- `<table>_<column>_check`; 0062's SET SCHEMA kept the name.
ALTER TABLE engine_mgmt.engine_catalogs
    DROP CONSTRAINT IF EXISTS engine_catalogs_kind_check;
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes add_catalog_accepts_a_connector_kind_outside_the_legacy_enum 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed`, `finished in` > 0.3 s. (`IF EXISTS` would silently no-op on a wrong name — this test is what proves the name was right.) On `VersionMissing`, apply the template-drop rule from Global Constraints once.

- [ ] **Step 5: Commit**

```bash
git branch --show-current
git add migrations/0144_engine_catalogs_drop_kind_check.sql crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): accept connector-first catalog kinds

Drop the 0057 kind CHECK: add_catalog already validates kinds, and the stale
enum turned every mysql/iceberg/clickhouse catalog into a 500.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Redact sensitive catalog properties for every kind (P8)

**Files:**
- Modify: `crates/egret-engine-mgmt/Cargo.toml` (add `regex.workspace = true` after `base64.workspace = true`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs:295-320` (`EngineCatalogDetailDto` doc, `redact_catalog_config`), `:4401` (`get_catalog` call), `:4679-4685` (`update_catalog` persist)
- Test: `engines.rs` `mod tests` (replace `redact_strips_warehouse_credential` at `:5402-5416`), `tests/routes.rs` (append)

**Interfaces:**
- Produces:
  - `const REDACTED_VALUE: &str = "********";`
  - `fn is_sensitive_config_key(key: &str) -> bool`
  - `fn redact_catalog_config(config: serde_json::Value) -> serde_json::Value` (the `kind` parameter is removed)
  - `fn restore_redacted_properties(config: &mut serde_json::Value, stored: &serde_json::Value)`

- [ ] **Step 1: Write the failing unit tests**

Replace the test `redact_strips_warehouse_credential` in `engines.rs` with:

```rust
    #[test]
    fn redact_strips_warehouse_credential() {
        let cfg = serde_json::json!({
            "warehouse": "wh1", "s3_endpoint": "http://minio:9000",
            "credential": "id:secret", "clientRowId": "abc"
        });
        let out = redact_catalog_config(cfg);
        assert!(out.get("credential").is_none());
        assert!(out.get("clientRowId").is_none());
        assert_eq!(out["warehouse"], "wh1");
    }

    #[test]
    fn redact_masks_inline_secrets_for_every_kind() {
        let cfg = serde_json::json!({
            "host": "db", "user": "u", "password": "legacy-top-level",
            "properties": {
                "connection-url": "jdbc:postgresql://db:5432/x",
                "connection-user": "u",
                "connection-password": "hunter2",
                "iceberg.rest-catalog.oauth2.token": "t0k",
                "s3.aws-access-key": "AKIA",
                "s3.aws-secret-key": "shh",
                "bigquery.credentials-key": "json",
                "gcs.json-private.key": "pem",
                "hive.s3.accesskey": "AKIA2",
                "tokenless-setting": "kept? no — contains token"
            },
            "secretProperties": { "connection-password": "0b3f5c0e-1111-4222-8333-944455556666" }
        });
        let out = redact_catalog_config(cfg);
        assert_eq!(out["password"], REDACTED_VALUE);
        assert_eq!(out["host"], "db");
        assert_eq!(out["user"], "u");
        let p = &out["properties"];
        assert_eq!(p["connection-url"], "jdbc:postgresql://db:5432/x");
        assert_eq!(p["connection-user"], "u");
        for k in [
            "connection-password",
            "iceberg.rest-catalog.oauth2.token",
            "s3.aws-access-key",
            "s3.aws-secret-key",
            "bigquery.credentials-key",
            "gcs.json-private.key",
            "hive.s3.accesskey",
            "tokenless-setting",
        ] {
            assert_eq!(p[k], REDACTED_VALUE, "{k} must be masked");
        }
        // secretProperties maps a property to a secret ID, not to secret
        // material; masking it would unbind the secret on the next save.
        assert_eq!(
            out["secretProperties"]["connection-password"],
            "0b3f5c0e-1111-4222-8333-944455556666"
        );
    }

    #[test]
    fn sensitive_key_matcher_follows_the_spec_pattern() {
        for k in ["PASSWORD", "x.Secret", "oauth-token", "credentials", "private_key",
                  "privatekey", "private.key", "private-key", "access_key", "accesskey"] {
            assert!(is_sensitive_config_key(k), "{k} should match");
        }
        for k in ["connection-url", "user", "host", "private--key", "keyaccess", "properties"] {
            assert!(!is_sensitive_config_key(k), "{k} should not match");
        }
    }

    #[test]
    fn restore_redacted_properties_puts_back_only_masked_sensitive_values() {
        let stored = serde_json::json!({
            "password": "old-top",
            "properties": { "connection-password": "hunter2", "connection-user": "u" }
        });
        let mut incoming = serde_json::json!({
            "password": REDACTED_VALUE,
            "properties": {
                "connection-password": REDACTED_VALUE,
                "connection-user": "u2",
                "api-token": REDACTED_VALUE
            }
        });
        restore_redacted_properties(&mut incoming, &stored);
        assert_eq!(incoming["password"], "old-top");
        assert_eq!(incoming["properties"]["connection-password"], "hunter2");
        assert_eq!(incoming["properties"]["connection-user"], "u2");
        // Nothing stored to restore from: the mask stays as typed.
        assert_eq!(incoming["properties"]["api-token"], REDACTED_VALUE);
        // A NEW value replaces the stored one.
        let mut changed = serde_json::json!({ "properties": { "connection-password": "new" } });
        restore_redacted_properties(&mut changed, &stored);
        assert_eq!(changed["properties"]["connection-password"], "new");
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib engines::tests::redact 2>&1 | tail -15
```

Expected: compile error — `redact_catalog_config` takes 2 arguments; `REDACTED_VALUE`, `is_sensitive_config_key`, `restore_redacted_properties` not found.

- [ ] **Step 3: Implement**

Add `regex.workspace = true` to `crates/egret-engine-mgmt/Cargo.toml` `[dependencies]` directly under `base64.workspace = true`.

Replace `engines.rs:309-320` (the doc comment and `redact_catalog_config`) with:

```rust
/// What `GET /engines/{id}/catalogs/{cid}` returns in place of a sensitive
/// value. The edit form PUTs the config back verbatim, so `update_catalog`
/// treats this exact string as "unchanged" (see [`restore_redacted_properties`]).
const REDACTED_VALUE: &str = "********";

/// Whether a catalog config key names secret material (spec SP1 §4.9).
fn is_sensitive_config_key(key: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)",
            )
            .expect("static sensitive-key pattern compiles")
        })
        .is_match(key)
}

/// Strip or mask secret material before a catalog config leaves the server.
/// For every kind: the minted client's `credential` and `clientRowId` are
/// removed, and every top-level or `properties` value whose key matches
/// [`is_sensitive_config_key`] becomes [`REDACTED_VALUE`]. `secretProperties`
/// is left intact: its values are secret IDs, and its own key contains
/// "secret".
fn redact_catalog_config(mut config: serde_json::Value) -> serde_json::Value {
    let Some(obj) = config.as_object_mut() else {
        return config;
    };
    obj.remove("credential");
    obj.remove("clientRowId");
    for (k, v) in obj.iter_mut() {
        if k != "properties" && k != "secretProperties" && is_sensitive_config_key(k) {
            *v = serde_json::Value::String(REDACTED_VALUE.into());
        }
    }
    if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for (k, v) in props.iter_mut() {
            if is_sensitive_config_key(k) {
                *v = serde_json::Value::String(REDACTED_VALUE.into());
            }
        }
    }
    config
}

/// Undo [`redact_catalog_config`] on a PUT body: a sensitive key whose value is
/// still exactly [`REDACTED_VALUE`] gets the stored value back, so re-saving
/// the edit form never overwrites a password with the mask.
fn restore_redacted_properties(config: &mut serde_json::Value, stored: &serde_json::Value) {
    fn restore_level(
        new: &mut serde_json::Map<String, serde_json::Value>,
        old: Option<&serde_json::Map<String, serde_json::Value>>,
    ) {
        for (k, v) in new.iter_mut() {
            if v.as_str() == Some(REDACTED_VALUE) && is_sensitive_config_key(k) {
                if let Some(prev) = old.and_then(|o| o.get(k)) {
                    *v = prev.clone();
                }
            }
        }
    }
    let Some(new_obj) = config.as_object_mut() else {
        return;
    };
    restore_level(new_obj, stored.as_object());
    if let Some(new_props) = new_obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
        restore_level(new_props, stored.get("properties").and_then(|p| p.as_object()));
    }
}
```

Update the doc on `EngineCatalogDetailDto` (`engines.rs:295-296`) to:

```rust
/// `GET /engines/{id}/catalogs/{cid}` payload: full stored config + secretRef,
/// with secret material removed or masked (see [`redact_catalog_config`]).
```

In `get_catalog` replace `config: redact_catalog_config(&cat.kind, cat.config.clone()),` with `config: redact_catalog_config(cat.config.clone()),`.

In `update_catalog` replace

```rust
    let updated = st
        .engine_mgmt
        .update_engine_catalog(cid, &b.config, b.secret_ref)
        .await?
        .ok_or(CpError::NotFound)?;
```

with

```rust
    // The form echoes what GET returned; keep stored values the admin left masked.
    let mut config = b.config.clone();
    restore_redacted_properties(&mut config, &prev.config);
    let updated = st
        .engine_mgmt
        .update_engine_catalog(cid, &config, b.secret_ref)
        .await?
        .ok_or(CpError::NotFound)?;
```

- [ ] **Step 4: Run the unit tests to verify they pass**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib -- engines::tests::redact engines::tests::sensitive_key engines::tests::restore_redacted 2>&1 | tail -5
```

Expected: `4 passed` (no DB; sub-second is correct here).

- [ ] **Step 5: Write the route test**

Append to `routes.rs`:

```rust
/// P8: an inline password on a non-warehouse catalog is masked on read, and
/// re-saving the form (which PUTs the masked config back) keeps the real value
/// in the row AND in what reaches the cluster.
#[tokio::test]
async fn catalog_detail_masks_inline_secrets_and_a_resave_keeps_them() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p8_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "pg", "kind": "postgresql",
                "config": { "properties": {
                    "connection-url": "jdbc:postgresql://db:5432/x",
                    "connection-user": "u",
                    "connection-password": "hunter2"
                } }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add pg catalog; body={j}");
    let cat_id = j["id"].as_str().expect("catalog id").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}/catalogs/{cat_id}"), &token, None))
        .await
        .unwrap();
    let (status, detail) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "catalog detail; body={detail}");
    assert_eq!(detail["config"]["properties"]["connection-password"], "********");
    assert_eq!(detail["config"]["properties"]["connection-user"], "u");
    assert!(!detail.to_string().contains("hunter2"), "no plaintext in GET; body={detail}");

    // Re-save exactly what the form received.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            Some(serde_json::json!({ "config": detail["config"].clone() })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "re-save catalog; body={j}");

    let stored = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .get_engine_catalog(Uuid::parse_str(&cat_id).unwrap())
        .await
        .expect("get catalog")
        .expect("catalog row");
    assert_eq!(stored.config["properties"]["connection-password"], "hunter2");
    let rendered = port
        .last_updated()
        .catalogs
        .into_iter()
        .find(|c| c.name == "pg")
        .expect("pg catalog rendered");
    assert_eq!(rendered.properties["connection-password"], "hunter2");
}
```

- [ ] **Step 6: Run it**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes catalog_detail_masks_inline_secrets_and_a_resave_keeps_them 2>&1 | tail -5
```

Expected: `1 passed`, `finished in` > 0.3 s. To confirm the test can fail, temporarily comment out the `restore_redacted_properties(&mut config, &prev.config);` line: the assertion on `stored.config` must fail with `"********"`. Restore the line.

- [ ] **Step 7: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/Cargo.toml Cargo.lock crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): redact catalog secrets for every connector kind

GET catalog detail removed credentials only for egret-warehouse; inline
passwords/tokens/keys of other kinds reached engine:read callers. Mask every
key matching the SP1 pattern, and restore masked values on PUT so re-saving
the edit form keeps the real secret.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Constrain and record `engines.status` (P6)

**Files:**
- Create: `migrations/0145_engine_status_check.sql`
- Modify: `crates/egret-engine-mgmt/src/store.rs` (after `set_engine_status`, `:240-247`; new `engine_status` module above `pub struct Engine`), `crates/egret-engine-mgmt/src/lib.rs:21`, `crates/egret-engine-mgmt/src/engines.rs:3523-3570` (`get_engine`)
- Test: `crates/egret-engine-mgmt/tests/store.rs` (append), `tests/routes.rs` (append)

**Interfaces:**
- Produces:
  - `pub mod engine_status { pub const PROVISIONING: &str; pub const RUNNING: &str; pub const FAILED: &str; pub const DELETING: &str; }` in `store.rs`, re-exported as `egret_engine_mgmt::engine_status`.
  - `EngineMgmtStore::transition_engine_status(&self, id: Uuid, from: &str, to: &str) -> Result<bool, EngineMgmtError>` — `true` iff a row moved.
- Consumes: existing `EngineMgmtStore::set_engine_status(&self, id: Uuid, status: &str)`.

- [ ] **Step 1: Write the failing store test**

Append to `crates/egret-engine-mgmt/tests/store.rs`:

```rust
#[tokio::test]
async fn engine_status_is_constrained_and_transitions_only_from_the_expected_value() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (wh_id, _) = seed_warehouse(&catalog, "status").await;
    let e = engine_mgmt
        .create_engine(
            &format!("status-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            wh_id,
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(e.status, egret_engine_mgmt::engine_status::PROVISIONING);

    let err = sqlx::query("UPDATE engine_mgmt.engines SET status = 'ready' WHERE id = $1")
        .bind(e.id)
        .execute(catalog.pool())
        .await
        .expect_err("'ready' is not a lifecycle status");
    assert_eq!(
        err.as_database_error().and_then(|d| d.constraint()),
        Some("engines_status_check")
    );

    assert!(engine_mgmt
        .transition_engine_status(e.id, "provisioning", "running")
        .await
        .unwrap());
    assert!(
        !engine_mgmt
            .transition_engine_status(e.id, "provisioning", "running")
            .await
            .unwrap(),
        "the row is no longer provisioning"
    );
    engine_mgmt.set_engine_status(e.id, "deleting").await.unwrap();
    assert!(!engine_mgmt
        .transition_engine_status(e.id, "running", "running")
        .await
        .unwrap());
    assert_eq!(
        engine_mgmt.get_engine(e.id).await.unwrap().unwrap().status,
        "deleting"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test store engine_status_is_constrained 2>&1 | tail -10
```

Expected: compile error — `engine_status` / `transition_engine_status` not found.

- [ ] **Step 3: Migration, store and re-export**

`migrations/0145_engine_status_check.sql`:

```sql
-- 0145_engine_status_check.sql
-- engines.status is the recorded lifecycle phase. Nothing but the column
-- default ever wrote it, so map any stray value to 'provisioning' first, then
-- pin the set: provisioning (created, pods not all ready yet), running (all
-- pods reported ready), failed (a create that could not be rolled back),
-- deleting (delete in progress; retryable).
UPDATE engine_mgmt.engines
   SET status = 'provisioning'
 WHERE status NOT IN ('provisioning', 'running', 'failed', 'deleting');

ALTER TABLE engine_mgmt.engines
    ADD CONSTRAINT engines_status_check
    CHECK (status IN ('provisioning', 'running', 'failed', 'deleting'));
```

In `store.rs`, directly above `/// A provisioned query engine` add:

```rust
/// The values `engine_mgmt.engines.status` may hold (`engines_status_check`).
pub mod engine_status {
    /// Created; the cluster has not yet reported every pod ready.
    pub const PROVISIONING: &str = "provisioning";
    /// Live status reported every pod ready at least once.
    pub const RUNNING: &str = "running";
    /// A create failed and its rollback could not remove the row.
    pub const FAILED: &str = "failed";
    /// A delete is in progress (written by the delete handler).
    pub const DELETING: &str = "deleting";
}
```

After `set_engine_status` add:

```rust
    /// Move `status` from `from` to `to` only if it is still `from`; returns
    /// whether a row moved. Conditional so a reader recording `running` can
    /// never overwrite a concurrent `deleting`.
    pub async fn transition_engine_status(
        &self,
        id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<bool, EngineMgmtError> {
        let res = sqlx::query(
            "UPDATE engine_mgmt.engines SET status = $3 WHERE id = $1 AND status = $2",
        )
        .bind(id)
        .bind(from)
        .bind(to)
        .execute(self.pool())
        .await?;
        Ok(res.rows_affected() == 1)
    }
```

In `lib.rs` replace `pub use store::{Engine, EngineCatalog, EngineMgmtStore};` with `pub use store::{engine_status, Engine, EngineCatalog, EngineMgmtStore};`.

Part A added a `ENGINE_STATUS_DELETING` constant to `store.rs` (its plan, "File Structure"). Keep one definition — the module above:

```bash
rtk proxy grep -rn "ENGINE_STATUS_DELETING" crates
```

Delete the `pub const ENGINE_STATUS_DELETING …` line in `store.rs`, rewrite each usage the grep lists to `crate::engine_status::DELETING` (inside `egret-engine-mgmt/src`) or `egret_engine_mgmt::engine_status::DELETING` (in `tests/`), remove any now-unused `use` of the old constant, and run `rtk proxy cargo check -p egret-engine-mgmt --all-targets` until it is clean. If `lib.rs` re-exported the old constant, drop that name from the re-export.

- [ ] **Step 4: Run the store test**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test store engine_status_is_constrained 2>&1 | tail -5
```

Expected: `1 passed`, `finished in` > 0.3 s.

- [ ] **Step 5: Write the failing route test**

Append to `routes.rs`:

```rust
async fn stored_status(f: &Fixture, engine_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(engine_id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine status")
}

/// P6: `GET /engines/{id}` records `running` once live status reports every pod
/// ready, and never overwrites `deleting`.
#[tokio::test]
async fn get_engine_records_running_and_leaves_deleting_alone() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p6_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    assert_eq!(stored_status(&f, &id).await, "provisioning");

    let resp = f.app.clone().oneshot(req("GET", &format!("/engines/{id}"), &token, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_status(&f, &id).await, "running", "all pods ready ⇒ running recorded");

    let name2 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name2, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create 2; body={j}");
    let id2 = j["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE id = $1")
        .bind(Uuid::parse_str(&id2).unwrap())
        .execute(&f.db.pool)
        .await
        .unwrap();
    let resp = f.app.clone().oneshot(req("GET", &format!("/engines/{id2}"), &token, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_status(&f, &id2).await, "deleting");
}
```

- [ ] **Step 6: Run it to verify it fails**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes get_engine_records_running_and_leaves_deleting_alone 2>&1 | tail -10
```

Expected: FAIL — `left: "provisioning"`, `right: "running"`.

- [ ] **Step 7: Record `running` in `get_engine`**

Part A made `get_engine` return phase `deleting` without calling the provisioner when the stored status is `deleting`; leave that early return as it is. In `get_engine` change `let engine = st` to `let mut engine = st`, and replace the `Ok(s) => EngineStatusDto { … },` arm of the `prov.status(…)` match with:

```rust
            Ok(s) => {
                // Record the first all-pods-ready observation. Conditional on the
                // row still being `provisioning`, so `deleting`/`failed` stay put.
                if s.phase == crate::engine_status::RUNNING
                    && engine.status == crate::engine_status::PROVISIONING
                {
                    match st
                        .engine_mgmt
                        .transition_engine_status(
                            engine.id,
                            crate::engine_status::PROVISIONING,
                            crate::engine_status::RUNNING,
                        )
                        .await
                    {
                        Ok(true) => engine.status = crate::engine_status::RUNNING.to_string(),
                        Ok(false) => {}
                        Err(e) => tracing::warn!(engine = %engine.name, error = %e,
                            "could not record running status"),
                    }
                }
                EngineStatusDto {
                    phase: s.phase,
                    message: s.message,
                    pods: s
                        .pods
                        .into_iter()
                        .map(|p| EnginePodDto {
                            name: p.name,
                            phase: p.phase,
                            ready: p.ready,
                        })
                        .collect(),
                }
            }
```

Also fix the now-stale comment in `list_engines` (`engines.rs:1839-1843`) to:

```rust
    // The stored `status` only advances when `get_engine` observes all pods
    // ready, so recompute the LIVE phase per engine from the provisioner (same
    // source `get_engine` uses), falling back to the stored value if the live
    // read fails or no provisioner is configured.
```

- [ ] **Step 8: Run both tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes get_engine_records_running_and_leaves_deleting_alone 2>&1 | tail -5
rtk proxy cargo test -p egret-engine-mgmt --test store 2>&1 | tail -5
```

Expected: both `ok`, DB timings > 0.3 s.

- [ ] **Step 9: Commit**

```bash
git branch --show-current
git add migrations/0145_engine_status_check.sql crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/lib.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/store.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): constrain engine status and record running

engines.status was only ever the column default, so every engine read
"provisioning" whenever live status was unavailable. Pin the four lifecycle
values with a CHECK and record running the first time get_engine sees every
pod ready, without overwriting deleting.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---
### Task 5: A failed StarRocks update must not delete the cluster

Not in the spec — found while verifying P3. `StarRocksProvisioner::update` calls `self.provision(spec)`, and `provision` deletes every object when `apply_all` fails, so one rejected CR patch tears the running cluster down.

**Files:**
- Modify: `crates/egret-provisioner/src/kube_client.rs:121-164` (`FakeKube` fields, `Default`, `apply`)
- Modify: `crates/egret-provisioner/src/starrocks.rs:841-842` (`update`)
- Test: `crates/egret-provisioner/src/starrocks.rs` `mod tests` (append)

**Interfaces:**
- Produces: `FakeKube::fail_apply_kind: Mutex<Option<String>>` — when `Some(kind)`, `apply` of an object with that `kind` returns an error without recording it.

- [ ] **Step 1: Add apply-failure injection to `FakeKube`**

In `pub struct FakeKube` add, after `patched`:

```rust
    /// When `Some(kind)`, `apply` of an object of that `kind` fails (and is not
    /// recorded) — stands in for the API server rejecting a manifest.
    pub fail_apply_kind: Mutex<Option<String>>,
```

In `impl Default for FakeKube` add `fail_apply_kind: Mutex::new(None),`. At the top of `async fn apply` (before computing `key`) add:

```rust
        if let Some(kind) = self.fail_apply_kind.lock().unwrap().as_deref() {
            if obj["kind"].as_str() == Some(kind) {
                anyhow::bail!("injected apply failure for {kind}");
            }
        }
```

(If Part A added fields to `FakeKube`, keep them; add only this one.)

- [ ] **Step 2: Write the failing test**

Append inside `mod tests` in `starrocks.rs`:

```rust
    /// An update whose CR apply fails must leave the running cluster alone:
    /// only `provision` (a brand-new engine) may clean up after a failed apply.
    #[tokio::test]
    async fn update_failure_keeps_the_running_cluster() {
        use crate::kube_client::FakeKube;

        let spec = sample_sr_spec();
        let kube = Arc::new(FakeKube::default());
        *kube.fail_apply_kind.lock().unwrap() = Some("StarRocksCluster".to_string());
        let res = StarRocksProvisioner::new(kube.clone()).update(&spec).await;
        assert!(res.is_err(), "the injected CR apply failure must surface");
        let deleted = kube.deleted.lock().unwrap();
        assert!(
            !deleted.iter().any(|d| d == "StarRocksCluster/sr1"),
            "a failed update must not delete the cluster; deleted: {deleted:?}"
        );
    }
```

- [ ] **Step 3: Run it to verify it fails**

```bash
rtk proxy cargo test -p egret-provisioner --lib starrocks::tests::update_failure_keeps_the_running_cluster 2>&1 | tail -10
```

Expected: FAIL — `a failed update must not delete the cluster; deleted: ["StarRocksCluster/sr1", …]`.

- [ ] **Step 4: Fix `update`**

In `impl EngineProvisioner for StarRocksProvisioner`, replace the first line of `update`:

```rust
        self.provision(spec).await?;
```

with:

```rust
        // Apply WITHOUT `provision`'s delete-on-failure: that cleanup is right for a
        // brand-new engine, but on an update it would tear down a running cluster
        // because one manifest was rejected. The caller restores the stored spec.
        self.apply_all(spec).await?;
```

- [ ] **Step 5: Run the provisioner tests**

```bash
rtk proxy cargo test -p egret-provisioner --lib 2>&1 | tail -5
```

Expected: `test result: ok.` (no DB; fast is correct).

- [ ] **Step 6: Commit**

```bash
git branch --show-current
git add crates/egret-provisioner/src/kube_client.rs crates/egret-provisioner/src/starrocks.rs
git commit -m "$(cat <<'EOF'
fix(provisioner): never delete a StarRocks cluster on a failed update

StarRocksProvisioner::update delegated to provision, whose failure path deletes
every object, so one rejected CR patch destroyed the running cluster. Update
now applies without that cleanup.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Internal/monitor secret columns on the engine row

**Files:**
- Create: `migrations/0146_engine_internal_secrets.sql`
- Modify: `crates/egret-engine-mgmt/src/store.rs` (`Engine` struct `:14-29`; the four column lists at `:188`, `:208`, `:217`, `:477`; new setter after `set_tls` `:293-308`; test fixtures `:493-613`)
- Modify (fixtures only): `crates/egret-engine-mgmt/src/engines.rs` (`dummy_engine`), `src/monitor.rs` (`engine_with_spec`), `src/assignment.rs` (4 fixtures), `tests/store.rs` (`coordinator_host_and_jwt_check`)
- Test: `crates/egret-engine-mgmt/tests/store.rs` (append), `tests/routes.rs` (helper)

**Interfaces:**
- Produces:
  - `Engine { …, pub internal_secret_id: Option<Uuid>, pub monitor_secret_id: Option<Uuid>, … }`
  - `EngineMgmtStore::set_internal_secret_ids(&self, engine_id: Uuid, internal_secret_id: Uuid, monitor_secret_id: Uuid) -> Result<(), EngineMgmtError>`
  - test helper `async fn engine_secret_ids(f: &Fixture, engine_id: Uuid) -> (Option<Uuid>, Option<Uuid>)` in `tests/routes.rs`

- [ ] **Step 1: Write the failing store test**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn internal_secret_ids_round_trip_on_the_engine_row() {
    let Some((catalog, engine_mgmt)) = stores().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (wh_id, _) = seed_warehouse(&catalog, "isec").await;
    let e = engine_mgmt
        .create_engine(
            &format!("isec-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            wh_id,
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    assert!(e.internal_secret_id.is_none() && e.monitor_secret_id.is_none());

    let (internal, monitor) = (Uuid::new_v4(), Uuid::new_v4());
    engine_mgmt
        .set_internal_secret_ids(e.id, internal, monitor)
        .await
        .unwrap();
    let got = engine_mgmt.get_engine(e.id).await.unwrap().unwrap();
    assert_eq!(got.internal_secret_id, Some(internal));
    assert_eq!(got.monitor_secret_id, Some(monitor));
    let listed = engine_mgmt.list_engines().await.unwrap();
    let row = listed.iter().find(|x| x.id == e.id).expect("listed");
    assert_eq!(row.monitor_secret_id, Some(monitor));
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test store internal_secret_ids_round_trip 2>&1 | tail -10
```

Expected: compile error — no field `internal_secret_id` on `Engine`.

- [ ] **Step 3: Migration**

`migrations/0146_engine_internal_secrets.sql`:

```sql
-- 0146_engine_internal_secrets.sql
-- Random per-engine secrets that replace values derived from the engine UUID
-- (which workspace members can read): Trino's internal-communication shared
-- secret and the `egret-monitor` password. Each column references the GLOBAL
-- named secret (workspace_secrets.id) that the LAST SUCCESSFUL apply rendered;
-- NULL means the running pods still carry the legacy derived values. No FK,
-- matching delegator_secret_id / tls_secret_id / root_secret_id.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN internal_secret_id uuid,
    ADD COLUMN monitor_secret_id uuid;
```

- [ ] **Step 4: `Engine` fields, column lists, setter**

In `pub struct Engine` insert after `pub tls_cert_secret_ref: Option<Uuid>,`:

```rust
    /// Named secret holding `TRINO_INTERNAL_SECRET`, as last applied. `None`:
    /// the pods still carry the legacy id-derived value.
    pub internal_secret_id: Option<Uuid>,
    /// Named secret holding the `egret-monitor` password, as last applied.
    /// `None`: the pods still carry the legacy id-derived value.
    pub monitor_secret_id: Option<Uuid>,
```

Update every column list that feeds `query_as::<_, Engine>`:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
perl -0pi -e 's/coordinator_cert, tls_cert_secret_ref, created_at/coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at/g; s/e\.coordinator_cert, e\.tls_cert_secret_ref, e\.created_at/e.coordinator_cert, e.tls_cert_secret_ref, e.internal_secret_id, e.monitor_secret_id, e.created_at/g' crates/egret-engine-mgmt/src/store.rs
rtk proxy grep -rn "query_as::<_, Engine>" crates --include='*.rs'
rtk proxy grep -rn "internal_secret_id, monitor_secret_id, created_at" crates/egret-engine-mgmt/src/store.rs | wc -l   # expect 4
```

If the first grep shows a `query_as::<_, Engine>` outside `store.rs` (for example one Part A added inside a transaction), add `internal_secret_id, monitor_secret_id` to its select list by hand.

After `set_tls` add:

```rust
    /// Record the internal/monitor secrets the last successful apply rendered.
    pub async fn set_internal_secret_ids(
        &self,
        engine_id: Uuid,
        internal_secret_id: Uuid,
        monitor_secret_id: Uuid,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query(
            "UPDATE engine_mgmt.engines SET internal_secret_id = $2, monitor_secret_id = $3 WHERE id = $1",
        )
        .bind(engine_id)
        .bind(internal_secret_id)
        .bind(monitor_secret_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
```

- [ ] **Step 5: Update every `Engine` struct literal**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
perl -0pi -e 's/(coordinator_cert: None,\n(\s*)tls_cert_secret_ref: None,\n)/$1$2internal_secret_id: None,\n$2monitor_secret_id: None,\n/g' \
  crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/engines.rs \
  crates/egret-engine-mgmt/src/monitor.rs crates/egret-engine-mgmt/src/assignment.rs \
  crates/egret-engine-mgmt/tests/store.rs
rtk proxy grep -rn "monitor_secret_id: None" crates --include='*.rs' | wc -l   # expect 12
rtk proxy cargo check -p egret-engine-mgmt --all-targets 2>&1 | tail -5
```

Expected: `12` (store.rs 5, engines.rs 1, monitor.rs 1, assignment.rs 4, tests/store.rs 1) and a clean `cargo check`. If `cargo check` reports another literal missing the fields, add the two `None` lines there.

- [ ] **Step 6: Add the route-test helper**

In `tests/routes.rs`, after `secret_store`:

```rust
/// `(internal_secret_id, monitor_secret_id)` straight off the engine row.
async fn engine_secret_ids(f: &Fixture, engine_id: Uuid) -> (Option<Uuid>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT internal_secret_id, monitor_secret_id FROM engine_mgmt.engines WHERE id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("engine secret ids")
}
```

(It is unused until Task 7; add `#[allow(dead_code)]` above it now and remove the attribute in Task 7.)

- [ ] **Step 7: Run the store tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test store 2>&1 | tail -5
rtk proxy cargo test -p egret-engine-mgmt --lib 2>&1 | tail -3
```

Expected: both `ok`; the store run takes > 0.3 s.

- [ ] **Step 8: Commit**

```bash
git branch --show-current
git add migrations/0146_engine_internal_secrets.sql crates/egret-engine-mgmt/src crates/egret-engine-mgmt/tests
git commit -m "$(cat <<'EOF'
feat(engine-mgmt): store internal and monitor secret ids per engine

Two nullable columns name the random secrets the last successful apply
rendered; NULL keeps the legacy derived values the running pods still have.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Random internal secrets through one apply pipeline (P1)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `build_provision_req` (`:543-686`), `monitor_password`/`inject_monitor_user` (`:1452-1477`), new helpers after `ensure_root_password_secret` (`:1625`), call sites in `create_engine` (`:2143-2166`, `:2267-2280`), `update_engine` (`:3658-3664`), `add_catalog` (`:4334-4355`), `remove_catalog` (`:4619-4625`), `update_catalog` (`:4694-4708`); tests `monitor_password_is_derived_from_id` (`:5232`), `inject_monitor_user_*` (`:5240-5272`), `build_provision_req_dispatches_starrocks` (`:6929`)
- Modify: `crates/egret-engine-mgmt/src/monitor.rs:22,147-158,225-233`
- Test: `engines.rs` `mod tests` (append), `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `Engine::{internal_secret_id, monitor_secret_id}`, `EngineMgmtStore::set_internal_secret_ids` (Task 6); `FakeEnginePort`, `secret_store`, `engine_secret_ids`, `create_engine_via_api` (Tasks 1, 6).
- Produces (all private to `engines.rs` unless marked):
  - `struct EngineInternalSecrets { internal_id: Uuid, internal: String, monitor_id: Uuid, monitor: String, fresh: bool }`
  - `fn random_internal_secret() -> String`
  - `async fn resolve_internal_secrets(st: &CpState, engine: &Engine, rotate: bool) -> Result<EngineInternalSecrets, CpError>`
  - `async fn commit_internal_secrets(st: &CpState, engine: &Engine, s: &EngineInternalSecrets) -> Result<(), CpError>`
  - `async fn discard_internal_secrets(st: &CpState, s: &EngineInternalSecrets)`
  - `pub(crate) fn legacy_monitor_password(id: Uuid) -> String` (replaces `pub fn monitor_password`)
  - `pub(crate) async fn monitor_password_for(st: &CpState, engine: &Engine) -> Result<String, CpError>`
  - `fn inject_monitor_user(auth: &mut Option<AuthReq>, password: &str) -> Result<(), CpError>` (was `engine_id: Uuid`)
  - `#[derive(Clone, Copy, Debug, Default)] struct BuildOpts { exclude_catalog: Option<Uuid>, rotate_internal_secrets: bool }`
  - `struct BuiltReq { req: EngineProvisionReq, internal: Option<EngineInternalSecrets> }`
  - `async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<BuiltReq, CpError>` (return type changed)
  - `async fn build_provision_req_with(st: &CpState, engine: &Engine, opts: BuildOpts) -> Result<BuiltReq, CpError>`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum ApplyOp { Provision, Update }`
  - `async fn apply_built(st: &CpState, prov: &dyn egret_engine_mgmt_api::EngineProvisioningPort, engine: &Engine, built: &BuiltReq, op: ApplyOp) -> Result<(), String>`
  - test helpers in `routes.rs`: `fn rendered_internal_secret(spec: &EngineSpec) -> String`, `fn monitor_hash(spec: &EngineSpec) -> String`, `async fn internal_secret_names(f: &Fixture, engine_id: Uuid) -> Vec<String>`

How the switch-over works, stated once so every call site agrees:

1. `build_provision_req_with` resolves internal secrets **last** (after every other fallible step). If the row names both secrets and `rotate` is false, it reuses them (`fresh: false`). Otherwise it mints two new global named secrets `engine-internal-{id}-{gen}` / `engine-monitor-{id}-{gen}` (`fresh: true`) but does **not** touch the row.
2. The monitor password's bcrypt hash goes into `config.auth.password_users` (→ `password.db` in the `{name}-config` ConfigMap); the internal secret goes into `config.env["TRINO_INTERNAL_SECRET"]` (→ the `{name}-env` Secret). Unchanged provisioner code renders both.
3. `apply_built` calls `provision`/`update`. On success it commits: writes both ids to the row and deletes the superseded rows. On failure it discards the fresh rows. The row therefore always names what the last successful apply gave the pods.
4. The monitor (`monitor.rs`) presents `monitor_password_for`: the stored secret when `monitor_secret_id` is set, else `egret-monitor-{id}` — which is exactly what an engine never applied since this change still has in `password.db`. An existing engine therefore switches on its first successful apply, and a failed apply leaves it on the legacy value.
5. Every caller checks "kubernetes not configured" **before** building, so a build never mints secrets that no apply will settle.

- [ ] **Step 1: Write the failing DB unit test**

Append inside `mod tests` in `engines.rs`:

```rust
    /// P1: internal secrets are random, stay OFF the row until committed, are
    /// reused when present, and a discarded or superseded generation is deleted.
    /// The monitor presents the legacy value exactly until a commit.
    #[tokio::test]
    async fn internal_secrets_are_pending_until_committed() {
        let Some(st) = test_state().await else {
            eprintln!("skipping internal_secrets_are_pending_until_committed: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                wh.id,
                &serde_json::json!({ "trinoConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");
        let not_found = |r: Result<String, egret_secrets::SecretsError>| {
            matches!(r, Err(egret_secrets::SecretsError::NotFound))
        };

        let first = resolve_internal_secrets(&st, &engine, false).await.expect("mint");
        assert!(first.fresh);
        assert_ne!(first.internal, first.monitor);
        {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&first.internal)
                .expect("url-safe base64");
            assert_eq!(raw.len(), 32);
        }
        let row = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert!(row.internal_secret_id.is_none() && row.monitor_secret_id.is_none());
        assert_eq!(
            monitor_password_for(&st, &row).await.unwrap(),
            legacy_monitor_password(engine.id),
            "uncommitted ⇒ the pods still have the legacy password"
        );

        commit_internal_secrets(&st, &row, &first).await.expect("commit");
        let row = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(row.internal_secret_id, Some(first.internal_id));
        assert_eq!(row.monitor_secret_id, Some(first.monitor_id));
        assert_eq!(monitor_password_for(&st, &row).await.unwrap(), first.monitor);

        let reused = resolve_internal_secrets(&st, &row, false).await.expect("reuse");
        assert!(!reused.fresh);
        assert_eq!(reused.internal, first.internal);

        let rotated = resolve_internal_secrets(&st, &row, true).await.expect("rotate");
        assert!(rotated.fresh);
        assert_ne!(rotated.internal, first.internal);
        discard_internal_secrets(&st, &rotated).await;
        assert!(not_found(st.secrets.resolve_plaintext(None, rotated.internal_id).await));
        assert!(not_found(st.secrets.resolve_plaintext(None, rotated.monitor_id).await));
        assert_eq!(
            st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap().monitor_secret_id,
            Some(first.monitor_id),
            "a discard never touches the row"
        );

        let rotated = resolve_internal_secrets(&st, &row, true).await.expect("rotate again");
        commit_internal_secrets(&st, &row, &rotated).await.expect("commit rotation");
        assert!(not_found(st.secrets.resolve_plaintext(None, first.internal_id).await));
        assert!(not_found(st.secrets.resolve_plaintext(None, first.monitor_id).await));
    }
```

Replace the two monitor tests (`monitor_password_is_derived_from_id` and `inject_monitor_user_adds_egret_monitor_when_auth_on`, `engines.rs:5231-5264`) with:

```rust
    #[test]
    fn legacy_monitor_password_is_derived_from_id() {
        assert_eq!(
            legacy_monitor_password(Uuid::nil()),
            "egret-monitor-00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn inject_monitor_user_adds_egret_monitor_when_auth_on() {
        // An engine WITH auth (one human password user).
        let mut auth = Some(AuthReq {
            password_users: vec![PasswordEntryReq {
                username: "bob".into(),
                bcrypt_hash: "$2y$hash".into(),
            }],
            jwt: None,
        });
        inject_monitor_user(&mut auth, "pw-123").unwrap();

        let users = &auth.unwrap().password_users;
        // Both the human user AND egret-monitor are present.
        assert!(users.iter().any(|u| u.username == "bob"));
        let mon = users
            .iter()
            .find(|u| u.username == "egret-monitor")
            .expect("egret-monitor injected");
        // The hash stored for Trino stays `$2y$`.
        assert!(mon.bcrypt_hash.starts_with("$2y$"));
        // `bcrypt::verify` wants the `$2b$` prefix; relabel back for the check.
        let hash = mon.bcrypt_hash.replacen("$2y$", "$2b$", 1);
        assert!(bcrypt::verify("pw-123", &hash).unwrap());
    }
```

In `inject_monitor_user_noop_when_no_auth` replace `inject_monitor_user(&mut auth, Uuid::new_v4()).unwrap();` with `inject_monitor_user(&mut auth, "pw").unwrap();`. In `build_provision_req_dispatches_starrocks` replace `let req = build_provision_req(&st, &engine)\n            .await\n            .expect("build_provision_req");` with:

```rust
        let req = build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req")
            .req;
```

- [ ] **Step 2: Run to verify it fails**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib engines::tests::internal_secrets_are_pending_until_committed 2>&1 | tail -10
```

Expected: compile errors — `resolve_internal_secrets`, `commit_internal_secrets`, `discard_internal_secrets`, `monitor_password_for`, `legacy_monitor_password` not found; `.req` on a non-struct.

- [ ] **Step 3: Replace the derived monitor password and add the secret helpers**

Replace `engines.rs:1452-1477` (`monitor_password` and `inject_monitor_user`) with:

```rust
/// System user the native Trino monitor (Phase 1.6b) authenticates as against a
/// coordinator that has auth enabled.
///
/// The legacy password, derived from the engine id. Valid ONLY for an engine
/// whose row has no `monitor_secret_id`: its pods were rendered before internal
/// secrets were stored and still carry this value in `password.db`. Everything
/// else must go through [`monitor_password_for`].
pub(crate) fn legacy_monitor_password(id: Uuid) -> String {
    format!("egret-monitor-{id}")
}

/// The `egret-monitor` password to present to this engine's coordinator: the
/// stored secret once an apply has recorded one, else the legacy value.
pub(crate) async fn monitor_password_for(st: &CpState, engine: &Engine) -> Result<String, CpError> {
    match engine.monitor_secret_id {
        Some(id) => st
            .secrets
            .resolve_plaintext(None, id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve monitor secret: {e}"))),
        None => Ok(legacy_monitor_password(engine.id)),
    }
}

/// Inject the `egret-monitor` password entry into an engine's auth block so the
/// native monitor can authenticate. No-op when `auth` is `None`: this must NOT
/// turn auth on for engines that have none — it only rides along when auth is
/// already enabled. Uses the same `$2b$`→`$2y$` relabel as the human users so
/// Trino's jBCrypt file authenticator accepts the hash.
fn inject_monitor_user(auth: &mut Option<AuthReq>, password: &str) -> Result<(), CpError> {
    let Some(auth) = auth.as_mut() else {
        return Ok(());
    };
    let bcrypt_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| CpError::BadRequest(format!("bcrypt monitor password: {e}")))?
        .replacen("$2b$", "$2y$", 1);
    auth.password_users.push(PasswordEntryReq {
        username: "egret-monitor".into(),
        bcrypt_hash,
    });
    Ok(())
}
```

After `ensure_root_password_secret` (ends `engines.rs:1625`) add:

```rust
/// A Trino engine's coordinator↔worker shared secret (`TRINO_INTERNAL_SECRET`)
/// and `egret-monitor` password, as resolved for ONE apply.
#[derive(Debug)]
struct EngineInternalSecrets {
    internal_id: Uuid,
    internal: String,
    monitor_id: Uuid,
    monitor: String,
    /// Minted for this apply and not yet on the engine row. [`apply_built`]
    /// records it after Kubernetes accepted the spec and deletes it otherwise,
    /// so the row only ever names what the running pods were given.
    fresh: bool,
}

/// 32 random bytes, URL-safe base64 without padding — no `:`, so it is also
/// safe as a Basic-auth password.
fn random_internal_secret() -> String {
    use base64::Engine as _;
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Reuse the engine's recorded internal secrets, or mint a new pending pair when
/// the row has none or `rotate` is set. Minting never writes the row.
async fn resolve_internal_secrets(
    st: &CpState,
    engine: &Engine,
    rotate: bool,
) -> Result<EngineInternalSecrets, CpError> {
    if !rotate {
        if let (Some(internal_id), Some(monitor_id)) =
            (engine.internal_secret_id, engine.monitor_secret_id)
        {
            let internal = st
                .secrets
                .resolve_plaintext(None, internal_id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve internal secret: {e}")))?;
            let monitor = st
                .secrets
                .resolve_plaintext(None, monitor_id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve monitor secret: {e}")))?;
            return Ok(EngineInternalSecrets {
                internal_id,
                internal,
                monitor_id,
                monitor,
                fresh: false,
            });
        }
    }
    // A per-generation suffix keeps a rotation from colliding with the names of
    // the secrets it replaces (those stay until the new apply commits).
    let generation = Uuid::new_v4().simple();
    let internal = random_internal_secret();
    let internal_id = st
        .secrets
        .create(
            None,
            &format!("engine-internal-{}-{generation}", engine.id),
            Some("Trino internal-communication shared secret"),
            &internal,
        )
        .await
        .map_err(|e| CpError::BadRequest(format!("store internal secret: {e}")))?;
    let monitor = random_internal_secret();
    let monitor_id = match st
        .secrets
        .create(
            None,
            &format!("engine-monitor-{}-{generation}", engine.id),
            Some("egret-monitor password for the native Trino monitor"),
            &monitor,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            if let Err(del) = st.secrets.delete(None, internal_id).await {
                tracing::warn!(engine = %engine.name, secret_id = %internal_id, error = %del,
                    "could not delete a half-minted internal secret");
            }
            return Err(CpError::BadRequest(format!("store monitor secret: {e}")));
        }
    };
    Ok(EngineInternalSecrets {
        internal_id,
        internal,
        monitor_id,
        monitor,
        fresh: true,
    })
}

/// After a successful apply: record a fresh pair on the row and delete the pair
/// it superseded. `engine` is the row as loaded BEFORE the build (its ids are
/// the superseded ones). No-op for a reused pair.
async fn commit_internal_secrets(
    st: &CpState,
    engine: &Engine,
    s: &EngineInternalSecrets,
) -> Result<(), CpError> {
    if !s.fresh {
        return Ok(());
    }
    st.engine_mgmt
        .set_internal_secret_ids(engine.id, s.internal_id, s.monitor_id)
        .await?;
    for old in [engine.internal_secret_id, engine.monitor_secret_id]
        .into_iter()
        .flatten()
    {
        if old != s.internal_id && old != s.monitor_id {
            if let Err(e) = st.secrets.delete(None, old).await {
                tracing::warn!(engine = %engine.name, secret_id = %old, error = %e,
                    "could not delete a superseded internal secret");
            }
        }
    }
    Ok(())
}

/// After a failed apply: delete a fresh pair (the pods never received it). No-op
/// for a reused pair.
async fn discard_internal_secrets(st: &CpState, s: &EngineInternalSecrets) {
    if !s.fresh {
        return;
    }
    for id in [s.internal_id, s.monitor_id] {
        if let Err(e) = st.secrets.delete(None, id).await {
            tracing::warn!(secret_id = %id, error = %e,
                "could not delete an unapplied internal secret");
        }
    }
}
```

- [ ] **Step 4: Rework `build_provision_req` into the pipeline**

Replace the signature line and the start of `build_provision_req` (`engines.rs:543-560`, through `let cats = …;`) with:

```rust
/// Knobs for one spec assembly.
#[derive(Clone, Copy, Debug, Default)]
struct BuildOpts {
    /// Render the catalogs WITHOUT this row (`remove_catalog` applies first and
    /// deletes the row only after Kubernetes dropped the catalog).
    exclude_catalog: Option<Uuid>,
    /// Mint new internal secrets even when the engine already has some.
    rotate_internal_secrets: bool,
}

/// An assembled provisioning request plus the internal secrets it rendered
/// (Trino only). Hand it to [`apply_built`], which settles those secrets.
struct BuiltReq {
    req: EngineProvisionReq,
    internal: Option<EngineInternalSecrets>,
}

/// Build the full provisioning request from the engine + its stored catalogs + config.
async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<BuiltReq, CpError> {
    build_provision_req_with(st, engine, BuildOpts::default()).await
}

/// [`build_provision_req`] with explicit [`BuildOpts`].
async fn build_provision_req_with(
    st: &CpState,
    engine: &Engine,
    opts: BuildOpts,
) -> Result<BuiltReq, CpError> {
    // StarRocks engines assemble an entirely different `EngineSpec` (FE/CN roles
    // + builtin storage volume, no Trino catalogs/auth). Dispatch to the shared
    // StarRocks assembler so ANY generic caller of `build_provision_req` is
    // correct for a StarRocks engine. (`update_starrocks_engine` calls
    // `build_starrocks_provision_req` directly instead, so it can also capture
    // the delegation bootstrap a later task will consume.)
    if engine.r#type == "starrocks" {
        let wh = st
            .catalog
            .get_warehouse(engine.warehouse_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        let (req, _bootstrap) = build_starrocks_provision_req(st, engine, &wh).await?;
        return Ok(BuiltReq {
            req,
            internal: None,
        });
    }
    let cats: Vec<EngineCatalog> = st
        .engine_mgmt
        .list_engine_catalogs(engine.id)
        .await?
        .into_iter()
        .filter(|c| Some(c.id) != opts.exclude_catalog)
        .collect();
```

Delete the monitor injection block (`engines.rs:573-581`, the comment starting `// When the engine ALREADY has auth on, inject the derived` through the closing `}` of `if !engine.is_jwt_delegation() { … }`).

Replace the internal-secret block (`engines.rs:654-661`):

```rust
    // Trino requires a coordinator<->worker shared secret whenever auth is enabled.
    // Derive a stable per-engine value (both roles read it from the env Secret).
    if config.auth.is_some() {
        config.env.insert(
            "TRINO_INTERNAL_SECRET".into(),
            format!("egret-internal-{}", engine.id),
        );
    }
```

with:

```rust
    // Internal secrets come LAST: minting writes new secret rows, so every
    // fallible step above has already passed, and a failure below cleans up
    // after itself. `apply_built` records or discards them.
    let internal =
        resolve_internal_secrets(st, engine, opts.rotate_internal_secrets).await?;
    // The native monitor signs in as `egret-monitor`. Skipped for jwt-delegation
    // engines: their coordinator is JWT-only (no PASSWORD authenticator), so a
    // basic-auth monitor user can't authenticate — the monitor degrades there.
    if !engine.is_jwt_delegation() {
        if let Err(e) = inject_monitor_user(&mut config.auth, &internal.monitor) {
            discard_internal_secrets(st, &internal).await;
            return Err(e);
        }
    }
    // Trino requires a coordinator<->worker shared secret whenever auth is
    // enabled; both roles read it from the env Secret.
    if config.auth.is_some() {
        config
            .env
            .insert("TRINO_INTERNAL_SECRET".into(), internal.internal.clone());
    }
```

Replace the tail `Ok(EngineProvisionReq { … })` with:

```rust
    Ok(BuiltReq {
        req: EngineProvisionReq {
            name: engine.name.clone(),
            namespace: engine.namespace.clone(),
            image_version: engine.spec["imageVersion"]
                .as_str()
                .unwrap_or(DEFAULT_IMAGE_VERSION)
                .into(),
            catalogs,
            config,
            ranger,
            kind: egret_engine_mgmt_api::EngineKind::Trino,
            starrocks: None,
        },
        internal: Some(internal),
    })
}

/// Which provisioning call an [`apply_built`] makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplyOp {
    Provision,
    Update,
}

/// Apply an assembled request and settle the internal secrets it rendered:
/// recorded on success, deleted on failure. `engine` must be the row the
/// request was built from. A failure to RECORD after a successful apply is
/// logged, not returned — the cluster already runs the new values, and the next
/// apply mints and records a fresh pair.
async fn apply_built(
    st: &CpState,
    prov: &dyn egret_engine_mgmt_api::EngineProvisioningPort,
    engine: &Engine,
    built: &BuiltReq,
    op: ApplyOp,
) -> Result<(), String> {
    let res = match op {
        ApplyOp::Provision => prov.provision(&built.req).await,
        ApplyOp::Update => prov.update(&built.req).await,
    };
    if let Some(secrets) = &built.internal {
        match &res {
            Ok(()) => {
                if let Err(e) = commit_internal_secrets(st, engine, secrets).await {
                    tracing::error!(engine = %engine.name, error = ?e,
                        "applied new internal secrets but could not record them; \
                         the monitor fails until the next successful apply");
                }
            }
            Err(_) => discard_internal_secrets(st, secrets).await,
        }
    }
    res
}
```

- [ ] **Step 5: Move every caller onto the pipeline**

`create_engine` — replace steps 6 and 7 (`engines.rs:2143-2166`) with:

```rust
    // 6. Kubernetes must be configured before anything is rendered (a build
    //    mints internal secrets that only an apply settles).
    let Some(prov) = st.engine_provisioner.as_ref() else {
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // 7. Build from the stored engine + catalogs, then provision; roll back the
    //    engine row (+ cascade catalog) + client on any failure.
    let built = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            let _ = st.engine_mgmt.delete_engine(engine.id).await; // CASCADE removes the catalog
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(e);
        }
    };
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Provision).await {
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }
```

and in the re-provision after auto-bind replace

```rust
                Ok(Some(bound)) => match build_provision_req(&st, &bound).await {
                    Ok(req2) => {
                        if let Err(msg) = prov.update(&req2).await {
```

with

```rust
                Ok(Some(bound)) => match build_provision_req(&st, &bound).await {
                    Ok(built2) => {
                        if let Err(msg) =
                            apply_built(&st, prov.as_ref(), &bound, &built2, ApplyOp::Update).await
                        {
```

(Tasks 12–13 replace this create body again; this step only keeps it compiling and correct.)

`update_engine` (Trino) — replace `engines.rs:3658-3664` with:

```rust
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = build_provision_req(&st, &engine).await?;
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
```

(Task 10 rewrites this handler fully.)

`add_catalog` — replace `engines.rs:4341-4355` (from `let req = match build_provision_req` through the `prov.update` block) with:

```rust
    let Some(prov) = st.engine_provisioner.as_ref() else {
        rollback(st.clone(), cat.id, minted_client).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            rollback(st.clone(), cat.id, minted_client).await;
            return Err(e);
        }
    };
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        rollback(st.clone(), cat.id, minted_client).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
```

`update_catalog` — replace `engines.rs:4694-4708` with:

```rust
    let Some(prov) = st.engine_provisioner.as_ref() else {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            restore(st.clone(), prev).await;
            return Err(e);
        }
    };
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
```

`remove_catalog` — replace `engines.rs:4619-4625` with:

```rust
    // Re-render + roll. (Task 11 reorders this to apply before deleting.)
    if let Some(prov) = st.engine_provisioner.as_ref() {
        let built = build_provision_req(&st, &engine).await?;
        if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
            return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
        }
    }
```

- [ ] **Step 6: Monitor reads the stored password**

In `monitor.rs` replace `use crate::engines::monitor_password;` with `use crate::engines::monitor_password_for;`, and replace `monitor_headers` with:

```rust
/// Monitor auth headers for a coordinator request. When the engine's stored spec
/// has an auth block (`trinoConfig.auth`), authenticate as the `egret-monitor`
/// file-password user over HTTP Basic (plus `X-Forwarded-Proto: https` so Trino
/// accepts Basic on the proxied hop), with the password the engine's pods were
/// last given ([`monitor_password_for`]); otherwise pass the trino user header
/// directly (no-auth engines).
async fn monitor_headers(st: &CpState, engine: &Engine) -> Result<Vec<(String, String)>, CpError> {
    if engine.spec["trinoConfig"]["auth"].is_object() {
        let password = monitor_password_for(st, engine).await?;
        let creds = format!("egret-monitor:{password}");
        let b64 = base64::engine::general_purpose::STANDARD.encode(creds);
        Ok(vec![
            ("Authorization".into(), format!("Basic {b64}")),
            ("X-Forwarded-Proto".into(), "https".into()),
        ])
    } else {
        Ok(vec![("X-Trino-User".into(), "egret-monitor".into())])
    }
}
```

In `coordinator_call` replace `let headers = monitor_headers(engine);` with `let headers = monitor_headers(st, engine).await?;`.

- [ ] **Step 7: Run the unit tests**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --lib 2>&1 | tail -5
```

Expected: `test result: ok.`; `internal_secrets_are_pending_until_committed` included; DB time > 0.3 s overall.

- [ ] **Step 8: Write the route tests**

In `routes.rs` remove the `#[allow(dead_code)]` above `engine_secret_ids`, then append:

```rust
/// `TRINO_INTERNAL_SECRET` exactly as the env Secret renders it.
fn rendered_internal_secret(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    egret_provisioner::trino::trino_env_secret(spec)["stringData"]["TRINO_INTERNAL_SECRET"]
        .as_str()
        .expect("TRINO_INTERNAL_SECRET rendered")
        .to_string()
}

/// The `egret-monitor` bcrypt hash exactly as `password.db` renders it,
/// relabelled `$2y$`→`$2b$` for `bcrypt::verify`.
fn monitor_hash(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    let cm = egret_provisioner::trino::trino_config_map(spec);
    let db = cm["data"]["password.db"].as_str().expect("password.db rendered");
    let line = db
        .lines()
        .find(|l| l.starts_with("egret-monitor:"))
        .expect("egret-monitor entry");
    line["egret-monitor:".len()..].replacen("$2y$", "$2b$", 1)
}

/// Names of every internal/monitor secret minted for this engine.
async fn internal_secret_names(f: &Fixture, engine_id: Uuid) -> Vec<String> {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE workspace_id IS NULL")
            .fetch_all(&f.db.pool)
            .await
            .expect("list global secret names");
    let (a, b) = (
        format!("engine-internal-{engine_id}-"),
        format!("engine-monitor-{engine_id}-"),
    );
    names
        .into_iter()
        .filter(|n| n.starts_with(&a) || n.starts_with(&b))
        .collect()
}

/// P1: a new Trino engine gets random stored secrets; the rendered env Secret
/// and password.db carry them, not the id-derived values.
#[tokio::test]
async fn create_renders_stored_random_internal_secrets() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f, &token, &name, wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id.expect("internal secret recorded"))
        .await
        .unwrap();
    let monitor = secrets
        .resolve_plaintext(None, monitor_id.expect("monitor secret recorded"))
        .await
        .unwrap();

    let spec = port.last_provisioned();
    let rendered = rendered_internal_secret(&spec);
    assert_eq!(rendered, internal);
    assert_ne!(rendered, format!("egret-internal-{id}"));
    let hash = monitor_hash(&spec);
    assert!(bcrypt::verify(&monitor, &hash).unwrap());
    assert!(!bcrypt::verify(format!("egret-monitor-{id}"), &hash).unwrap());
}

/// P1 transition: an engine rendered before this change (no secret ids) keeps
/// its legacy values through a FAILED apply and adopts stored secrets only on a
/// successful one — so the monitor never presents a password no pod has.
#[tokio::test]
async fn legacy_engine_adopts_internal_secrets_only_after_a_successful_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_legacy_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let trino_config = serde_json::json!({ "delegationMode": "impersonate", "auth": {} });
    let engine = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &name,
            "trino",
            "egret-engines",
            wh_id,
            &serde_json::json!({
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "imageVersion": "470", "trinoConfig": trino_config.clone(),
            }),
            None,
        )
        .await
        .expect("seed legacy engine row");
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": trino_config,
    });

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &format!("/engines/{}", engine.id), &token, Some(put_body.clone())))
        .await
        .unwrap();
    assert!(!resp.status().is_success(), "injected failure must surface");
    assert_eq!(engine_secret_ids(&f, engine.id).await, (None, None));
    assert!(
        internal_secret_names(&f, engine.id).await.is_empty(),
        "the failed attempt's secrets must be discarded"
    );

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req("PUT", &format!("/engines/{}", engine.id), &token, Some(put_body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (internal_id, monitor_id) = engine_secret_ids(&f, engine.id).await;
    let secrets = secret_store(&f);
    let internal = secrets.resolve_plaintext(None, internal_id.unwrap()).await.unwrap();
    let monitor = secrets.resolve_plaintext(None, monitor_id.unwrap()).await.unwrap();
    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), internal);
    assert!(bcrypt::verify(&monitor, &monitor_hash(&spec)).unwrap());
    assert_eq!(internal_secret_names(&f, engine.id).await.len(), 2);
}
```

- [ ] **Step 9: Run the route suite**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes 2>&1 | tail -5
```

Expected: `test result: ok.`, both new tests included, `finished in` well above 1 s. To see the transition test bite, temporarily change `Err(_) => discard_internal_secrets(st, secrets).await,` in `apply_built` to `Err(_) => {}`: `the failed attempt's secrets must be discarded` must fail. Restore it.

- [ ] **Step 10: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/src/monitor.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): random stored Trino internal and monitor secrets

TRINO_INTERNAL_SECRET and the egret-monitor password were derived from the
engine UUID, which workspace members can read. Every apply now goes through one
pipeline that mints 32 random bytes as pending secrets and records them on the
engine only after Kubernetes accepted them; the monitor presents the stored
password, or the legacy one while an engine has not been re-applied.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `POST /engines/{id}/rotate-secrets`

**Files:**
- Modify: `crates/egret-engine-mgmt/src/http_error.rs` (enum + `into_response`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (router `:105`; new handler after `restart_engine` `:3947`; `EngineMgmtApi` paths `:5112`)
- Modify: `openapi.json` (regenerated), `frontend/src/api/generated/control-plane-engines/control-plane-engines.ts` (regenerated), `frontend/src/api/controlPlane.ts:57-75,915-917`
- Test: `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `build_provision_req_with`, `BuildOpts`, `apply_built`, `ApplyOp` (Task 7).
- Produces:
  - `EngineHttpError::ClusterApplyFailed(String)` → HTTP 502, body `{"error": "engine cluster apply failed: <cause>"}` (used again in Task 10).
  - handler `async fn rotate_engine_secrets(State(st): State<CpState>, Auth(p): Auth, Path(id): Path<Uuid>) -> Result<StatusCode, CpError>`; route `POST /engines/{id}/rotate-secrets`; operationId `rotate_engine_secrets`.
  - generated TS `controlPlaneEnginesRotateEngineSecrets(id: string)`; hand-written `export async function rotateEngineSecrets(id: string): Promise<void>` in `frontend/src/api/controlPlane.ts`.

- [ ] **Step 1: Write the failing route test**

Append to `routes.rs`:

```rust
/// Rotate: a failed apply changes nothing and discards the attempt; a successful
/// one renders new values, records them, and deletes the old rows. Non-Trino →
/// 400, unknown → 404.
#[tokio::test]
async fn rotate_secrets_replaces_both_values_only_on_a_successful_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_rotate_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f, &token, &name, wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let secrets = secret_store(&f);
    let (old_internal_id, old_monitor_id) = engine_secret_ids(&f, id).await;
    let (old_internal_id, old_monitor_id) = (old_internal_id.unwrap(), old_monitor_id.unwrap());
    let old_internal = secrets.resolve_plaintext(None, old_internal_id).await.unwrap();
    let old_monitor = secrets.resolve_plaintext(None, old_monitor_id).await.unwrap();
    let rotate = || req("POST", &format!("/engines/{id}/rotate-secrets"), &token, None);

    port.set_fail_update(true);
    let (status, j) = into_json(f.app.clone().oneshot(rotate()).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(j["error"].as_str().unwrap().contains("injected update failure"), "body={j}");
    assert_eq!(engine_secret_ids(&f, id).await, (Some(old_internal_id), Some(old_monitor_id)));
    assert_eq!(internal_secret_names(&f, id).await.len(), 2, "attempt discarded");

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(rotate()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (new_internal_id, new_monitor_id) = engine_secret_ids(&f, id).await;
    let (new_internal_id, new_monitor_id) = (new_internal_id.unwrap(), new_monitor_id.unwrap());
    assert_ne!(new_internal_id, old_internal_id);
    assert_ne!(new_monitor_id, old_monitor_id);
    let new_internal = secrets.resolve_plaintext(None, new_internal_id).await.unwrap();
    let new_monitor = secrets.resolve_plaintext(None, new_monitor_id).await.unwrap();
    assert_ne!(new_internal, old_internal);

    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), new_internal);
    let hash = monitor_hash(&spec);
    // bcrypt salts every hash, so "the hash changed" proves nothing; verify.
    assert!(bcrypt::verify(&new_monitor, &hash).unwrap());
    assert!(!bcrypt::verify(&old_monitor, &hash).unwrap());
    assert!(matches!(
        secrets.resolve_plaintext(None, old_internal_id).await,
        Err(egret_secrets::SecretsError::NotFound)
    ));
    assert!(matches!(
        secrets.resolve_plaintext(None, old_monitor_id).await,
        Err(egret_secrets::SecretsError::NotFound)
    ));
    assert_eq!(internal_secret_names(&f, id).await.len(), 2);

    let sr = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &format!("sr-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            wh_id,
            &serde_json::json!({ "starrocksConfig": {} }),
            None,
        )
        .await
        .unwrap();
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("/engines/{}/rotate-secrets", sr.id), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("/engines/{}/rotate-secrets", Uuid::new_v4()), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes rotate_secrets_replaces_both_values 2>&1 | tail -10
```

Expected: FAIL — the first rotate returns `404` (route not mounted), not `502`.

- [ ] **Step 3: Add the 502 variant**

In `http_error.rs`, after `BadGateway`:

```rust
    /// Kubernetes rejected (or never answered) an apply of an engine's rendered
    /// spec. The stored state was restored; the cause is surfaced verbatim.
    #[error("engine cluster apply failed: {0}")]
    ClusterApplyFailed(String),
```

Part A rewrote `into_response` to compute `let msg = self.to_string();` and `match self`, with `EngineHttpError::ClusterDeleteFailed(_)` returning a 502 body that carries `"error"` and a `"code"`. Add the apply variant next to it in the same shape:

```rust
            EngineHttpError::ClusterApplyFailed(_) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": msg, "code": "engine_cluster_apply_failed" })),
            )
                .into_response(),
```

Keep both variants: Part A's names a failed cluster delete (row stays `deleting`), this one a rejected apply (stored state restored). If `into_response` does not have Part A's shape when you open it, stop and report — the plan and the code disagree.

- [ ] **Step 4: Add the handler and route**

In `router()` after `.route("/engines/{id}/restart", post(restart_engine))` add:

```rust
        .route("/engines/{id}/rotate-secrets", post(rotate_engine_secrets))
```

After `restart_engine` add:

```rust
/// `POST /engines/{id}/rotate-secrets` — mint a new Trino internal-communication
/// secret and `egret-monitor` password and re-apply the engine (pods roll). The
/// old values stay in force, and on the row, unless the apply succeeds.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/rotate-secrets",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "New internal secrets applied; the engine's pods restart"),
        (status = 400, description = "Not a Trino engine, or Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply; the previous secrets stay in force", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rotate_engine_secrets(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // Only Trino has these two secrets; StarRocks' analogues (root and
    // delegator passwords) are separate and not rotated here.
    if engine.r#type != "trino" {
        return Err(CpError::BadRequest(format!(
            "rotating internal secrets is not supported for {} engines",
            engine.r#type
        )));
    }
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = build_provision_req_with(
        &st,
        &engine,
        BuildOpts {
            rotate_internal_secrets: true,
            ..BuildOpts::default()
        },
    )
    .await?;
    apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update)
        .await
        .map_err(CpError::ClusterApplyFailed)?;
    Ok(StatusCode::NO_CONTENT)
}
```

In `#[openapi(paths(…))]` for `EngineMgmtApi`, add `rotate_engine_secrets,` after `restart_engine,`.

- [ ] **Step 5: Run the test**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes rotate_secrets_replaces_both_values 2>&1 | tail -5
```

Expected: `1 passed`, `finished in` > 0.3 s.

- [ ] **Step 6: Regenerate `openapi.json` and the frontend client**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
rtk proxy cargo test -p egret-server --lib committed_openapi_json_is_current 2>&1 | tail -3
git diff --stat openapi.json
cd frontend && npm run gen:api && git status --short src/api/generated
```

Expected: the drift gate passes; `openapi.json` gains the rotate path; `git status` lists `src/api/generated/control-plane-engines/control-plane-engines.ts`. For any **other** generated file that changed, run `git diff <file>`: if the diff is not about this route, discard it with `git checkout -- <file>` (and `rm` any untracked file it created).

- [ ] **Step 7: Hand-written client**

In `frontend/src/api/controlPlane.ts`, add `controlPlaneEnginesRotateEngineSecrets,` to the import list from `./generated/control-plane-engines/control-plane-engines` (alphabetically after `controlPlaneEnginesRestartEngine,`), and after `restartEngine` add:

```ts
/**
 * Mint a new Trino internal-communication secret and monitor password and
 * re-apply the engine. Every pod restarts.
 */
export async function rotateEngineSecrets(id: string): Promise<void> {
  await controlPlaneEnginesRotateEngineSecrets(id);
}
```

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend && npm run typecheck 2>&1 | tail -3
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git branch --show-current
git add crates/egret-engine-mgmt/src/http_error.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs openapi.json frontend/src/api/generated/control-plane-engines/control-plane-engines.ts frontend/src/api/controlPlane.ts
git commit -m "$(cat <<'EOF'
feat(engine-mgmt): rotate a Trino engine's internal secrets

POST /api/control-plane/engines/{id}/rotate-secrets (engine:update) mints new
internal and monitor secrets and re-applies the engine; a rejected apply
returns 502 and leaves the previous secrets in force.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Rotate secrets button in Security & Exposure

**Files:**
- Create: `frontend/src/routes/control-plane/RotateSecretsSection.tsx`
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx` (`EngineTabs` settings content `:470-476`; `ConfigurationTab` `:1038-1068`; `TrinoConfigEditor` props `:923-931` and security pane `:1031`; `SecuritySection` `:1075-1128`)
- Test: `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx` (inside `describe("EngineDetail — Trino Settings tab (merged Configuration + Security)")`)

**Interfaces:**
- Consumes: `rotateEngineSecrets(id: string): Promise<void>` (Task 8).
- Produces: `export function RotateSecretsSection({ engineId }: { engineId: string })`; `TrinoConfigEditor` gains optional `engineId?: string` (absent in the create wizard, which has nothing to rotate).

- [ ] **Step 1: Write the failing test**

Add inside the `describe("EngineDetail — Trino Settings tab (merged Configuration + Security)", …)` block, after the `?section=security` test:

```tsx
  test("Security & Exposure rotates secrets only after a pods-will-restart confirmation", async () => {
    vi.mocked(api.rotateEngineSecrets).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderDetail(
      `/control-plane/engines/${ENGINE.id}?tab=settings&section=security`,
    );
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(
      await screen.findByRole("button", { name: "Rotate secrets" }),
    );
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/all pods will restart/i)).toBeInTheDocument();
    expect(api.rotateEngineSecrets).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Rotate" }));
    await waitFor(() =>
      expect(api.rotateEngineSecrets).toHaveBeenCalledWith("e1"),
    );
  });
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
rtk proxy npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx -t "rotates secrets" 2>&1 | tail -15
```

Expected: FAIL — unable to find a button named "Rotate secrets".

- [ ] **Step 3: Create the component**

`frontend/src/routes/control-plane/RotateSecretsSection.tsx`:

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { rotateEngineSecrets } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { ConfigSection } from "@/components/form/ConfigSection";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

/**
 * Rotate a Trino engine's internal secrets: the shared secret its coordinator and
 * workers authenticate each other with, and the password egret's monitor signs in
 * with. Rotation mints new values and re-applies the engine, so every pod
 * restarts — hence the confirmation. Saves on its own; it is not part of the
 * Settings form's Save.
 */
export function RotateSecretsSection({ engineId }: { engineId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const mutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => rotateEngineSecrets(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      setOpen(false);
      toast.success("Secrets rotated. The engine's pods are restarting.");
    },
    onError: (e) =>
      toast.error(e.message ?? "Failed to rotate the engine's secrets."),
  });

  return (
    <ConfigSection
      title="Internal secrets"
      description="The shared secret Trino's coordinator and workers use to trust each other, and the password egret's monitor signs in with. Rotate them if they may have been exposed."
    >
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => setOpen(true)}
          disabled={mutation.isPending}
        >
          {mutation.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <KeyRound className="h-4 w-4" />
          )}
          Rotate secrets
        </Button>
      </div>

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rotate internal secrets?</AlertDialogTitle>
            <AlertDialogDescription>
              New secrets are generated and applied, so all pods will restart and
              running queries are interrupted.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                mutation.mutate(engineId);
              }}
              disabled={mutation.isPending}
            >
              {mutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Rotate
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </ConfigSection>
  );
}
```

- [ ] **Step 4: Thread `engineId` to the Security pane**

In `EngineDetail.tsx`:

1. Add `import { RotateSecretsSection } from "./RotateSecretsSection";` after `import { EngineSecurity } from "./EngineSecurity";`.
2. In `EngineTabs` replace `<ConfigurationTab cfg={cfg} imageVersion={engine.spec.imageVersion} />` with `<ConfigurationTab cfg={cfg} imageVersion={engine.spec.imageVersion} engineId={engineId} />`.
3. Replace the `ConfigurationTab` signature and its editor element:

```tsx
function ConfigurationTab({
  cfg,
  imageVersion,
  engineId,
}: {
  cfg: EngineConfigForm;
  imageVersion: string;
  engineId: string;
}) {
```

and `<TrinoConfigEditor cfg={cfg} imageVersion={imageVersion} onInvalidChange={setRawInvalid} />` → add `engineId={engineId}` (keep the existing multi-line formatting).
4. In `TrinoConfigEditor`, destructure and type the new prop:

```tsx
export function TrinoConfigEditor({
  cfg,
  imageVersion,
  onInvalidChange,
  engineId,
}: {
  cfg: EngineConfigForm;
  imageVersion: string;
  onInvalidChange?: (invalid: boolean) => void;
  /**
   * The saved engine's id. Present on the detail Settings tab (enables Rotate
   * secrets); absent in the create wizard, which has nothing to rotate yet.
   */
  engineId?: string;
}) {
```

and replace `{section === "security" && <SecuritySection cfg={cfg} />}` with `{section === "security" && <SecuritySection cfg={cfg} engineId={engineId} />}`.
5. Change `SecuritySection`'s signature to `function SecuritySection({ cfg, engineId }: { cfg: EngineConfigForm; engineId?: string }) {` and wrap its return:

```tsx
  return (
    <div className="space-y-5">
      <EngineSecurity
        passwordUsers={passwordUsers}
        setPasswordUsers={setPasswordUsers}
        jwtEnabled={jwtEnabled}
        setJwtEnabled={setJwtEnabled}
        delegationMode={delegationMode}
        setDelegationMode={setDelegationMode}
        storedDelegationMode={storedDelegationMode}
        jwksUri={jwksUri}
        setJwksUri={setJwksUri}
        requiredAudience={requiredAudience}
        setRequiredAudience={setRequiredAudience}
        ingressEnabled={ingressEnabled}
        setIngressEnabled={setIngressEnabled}
        ingressHost={ingressHost}
        setIngressHost={setIngressHost}
        ingressClassName={ingressClassName}
        setIngressClassName={setIngressClassName}
        ingressTlsSecret={ingressTlsSecret}
        setIngressTlsSecret={setIngressTlsSecret}
        ingressTlsCertSecretRef={ingressTlsCertSecretRef}
        setIngressTlsCertSecretRef={setIngressTlsCertSecretRef}
        ingressAnnotations={ingressAnnotations}
        setIngressAnnotations={setIngressAnnotations}
      />
      {engineId && <RotateSecretsSection engineId={engineId} />}
    </div>
  );
```

- [ ] **Step 5: Run the frontend gates**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
rtk proxy npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx src/routes/control-plane/__tests__/EngineCreate.test.tsx 2>&1 | tail -5
npm run lint 2>&1 | tail -3
npm run typecheck 2>&1 | tail -3
```

Expected: both test files pass (the create wizard still renders without the rotate section); lint and typecheck clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git branch --show-current
git add frontend/src/routes/control-plane/RotateSecretsSection.tsx frontend/src/routes/control-plane/EngineDetail.tsx frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): rotate engine secrets from Security & Exposure

A Rotate secrets action on a saved Trino engine's Security & Exposure pane,
behind a confirmation that all pods will restart.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---
### Task 10: Update rolls back on failure; Trino rematerializes (P3, P4)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `update_engine` (whole handler incl. its `#[utoipa::path]`), `update_starrocks_engine` (persist/build/apply section), new helpers `preserve_trino_delegation_mode` and `restore_engine_spec` (place directly above `update_engine`'s `#[utoipa::path]`); tests `update_starrocks_engine_rescales`, `update_starrocks_engine_preserves_storage`
- Modify: `openapi.json` (regenerated)
- Test: `engines.rs` `mod tests`, `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `apply_built`, `ApplyOp`, `build_provision_req` (Task 7); `EngineHttpError::ClusterApplyFailed` (Task 8); `EngineAssignmentService::rematerialize_for_engine(&self, engine_id: Uuid) -> Result<(), EngineMgmtError>`.
- Produces:
  - `fn preserve_trino_delegation_mode(spec: &mut serde_json::Value, stored: &serde_json::Value)`
  - `async fn restore_engine_spec(st: &CpState, id: Uuid, spec: &serde_json::Value, status: &str)`
  - Behaviour: both update paths return 400 "kubernetes not configured" **before** persisting; build or apply failure restores the stored spec; apply failure is 502.

- [ ] **Step 1: Write the failing unit tests**

In `engines.rs` `mod tests`, add a failing provisioner next to `NoopProvisioner`:

```rust
    /// Like `NoopProvisioner`, but `update` fails — a Kubernetes rejection.
    struct FailingUpdateProvisioner;
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for FailingUpdateProvisioner {
        async fn provision(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Ok(())
        }
        async fn status(
            &self,
            _kind: &str,
            _namespace: &str,
            _name: &str,
        ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
            Ok(egret_engine_mgmt_api::EngineStatus::default())
        }
        async fn update(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Err("injected update failure".into())
        }
        async fn delete(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        async fn restart(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }
```

and these tests:

```rust
    #[test]
    fn preserve_trino_delegation_mode_keeps_the_stored_mode_only_when_omitted() {
        let stored = serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } });
        let mut omitted = serde_json::json!({ "trinoConfig": { "worker": {} } });
        preserve_trino_delegation_mode(&mut omitted, &stored);
        assert_eq!(omitted["trinoConfig"]["delegationMode"], "jwt");
        let mut explicit = serde_json::json!({ "trinoConfig": { "delegationMode": "none" } });
        preserve_trino_delegation_mode(&mut explicit, &stored);
        assert_eq!(explicit["trinoConfig"]["delegationMode"], "none");
        let mut from_legacy = serde_json::json!({ "trinoConfig": {} });
        preserve_trino_delegation_mode(&mut from_legacy, &serde_json::json!({}));
        assert!(from_legacy["trinoConfig"].get("delegationMode").is_none());
    }

    /// P3 (StarRocks): a rejected apply restores the stored spec and is a 502.
    #[tokio::test]
    async fn update_starrocks_engine_failure_restores_spec() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_failure_restores_spec: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(FailingUpdateProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;
        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(3),
            ..Default::default()
        }));
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .expect("the apply failure must surface");
        assert!(matches!(err, CpError::ClusterApplyFailed(_)), "got {err:?}");
        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec, engine.spec, "stored spec restored");
    }

    /// Without Kubernetes an update is refused before anything is persisted.
    #[tokio::test]
    async fn update_starrocks_engine_without_kubernetes_changes_nothing() {
        let Some(st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_without_kubernetes_changes_nothing: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;
        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(3),
            ..Default::default()
        }));
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .map(|e| e.to_string());
        assert!(matches!(&err, Some(m) if m.contains("kubernetes not configured")), "got {err:?}");
        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec["starrocksConfig"]["feReplicas"], 1);
    }
```

The two existing tests that relied on "persist, then fail at the provision step" must now run with a provisioner. In `update_starrocks_engine_rescales`:
- change `let Some(st) = test_state().await else {` to `let Some(mut st) = test_state().await else {` and add `st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));` right after the `else { … };` block;
- replace the block from `// Ok(Json<EngineDto>) isn't Debug, so map to the error message first.` through its `assert!(matches!(… "kubernetes not configured" …));` with:

```rust
        if let Err(e) = update_starrocks_engine(st.clone(), engine.clone(), body).await {
            panic!("update failed: {e}");
        }
```

- update its doc comment's last two lines to: `/// The fake provisioner accepts the apply, so the spec stays persisted.`

In `update_starrocks_engine_preserves_storage` make the same `mut st` + `NoopProvisioner` change and replace `let _ = update_starrocks_engine(st.clone(), engine.clone(), body).await; // fails at prov; spec persisted` with:

```rust
        if let Err(e) = update_starrocks_engine(st.clone(), engine.clone(), body).await {
            panic!("update failed: {e}");
        }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --lib -- engines::tests::preserve_trino engines::tests::update_starrocks 2>&1 | tail -15
```

Expected: compile error — `preserve_trino_delegation_mode` not found.

- [ ] **Step 3: Implement the helpers and the Trino handler**

Directly above the `#[utoipa::path(put, path = "/api/control-plane/engines/{id}", …)]` of `update_engine` add:

```rust
/// Carry the stored Trino `delegationMode` into a new spec whose `trinoConfig`
/// omits it. An absent key reads back as `impersonate`
/// (`store::Engine::delegation_mode`), so dropping it would silently change the
/// engine's delegation mode.
fn preserve_trino_delegation_mode(spec: &mut serde_json::Value, stored: &serde_json::Value) {
    let Some(mode) = stored
        .get("trinoConfig")
        .and_then(|t| t.get("delegationMode"))
        .cloned()
    else {
        return;
    };
    if let Some(tc) = spec.get_mut("trinoConfig").and_then(|t| t.as_object_mut()) {
        tc.entry("delegationMode").or_insert(mode);
    }
}

/// Put back the spec/status an update persisted before its build or apply
/// failed. Logged, not returned: the caller is already returning that failure.
async fn restore_engine_spec(st: &CpState, id: Uuid, spec: &serde_json::Value, status: &str) {
    if let Err(e) = st.engine_mgmt.update_engine(id, spec, status).await {
        tracing::error!(engine_id = %id, error = %e,
            "could not restore the engine spec after a failed update; \
             the stored spec is ahead of the cluster");
    }
}
```

Replace `update_engine` (its `#[utoipa::path]` and body) with:

```rust
#[utoipa::path(
    put,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = CreateEngineBody,
    responses(
        (status = 200, description = "Engine spec updated and pods rolled", body = EngineDto),
        (status = 400, description = "Invalid config, Kubernetes not configured, or re-materializing workspace connections failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the update; the previous spec was restored", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateEngineBody>,
) -> Result<Json<EngineDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // StarRocks has its own spec builder + provision-req assembler (FE/CN roles,
    // builtin storage volume) — dispatch to it rather than running the Trino
    // spec/assembly below.
    if engine.r#type == "starrocks" {
        return update_starrocks_engine(st, engine, b).await;
    }
    if engine.r#type != "trino" {
        return Err(CpError::BadRequest(
            "update not supported for this engine kind".into(),
        ));
    }
    // An update is only meaningful if it can be applied: refuse BEFORE
    // persisting, so the stored spec never runs ahead of the cluster.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Rebuild the spec from the body + config. The primary Catalog Client is NOT
    // re-minted (its credential lives on the primary catalog row), so nothing is
    // revoked here. Trino has no hot-reload; the provisioner rolls the pods.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let mut spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
    });
    if let Some(tc) = &b.trino_config {
        spec["trinoConfig"] = tc.clone();
    } else if let Some(tc) = engine.spec.get("trinoConfig") {
        spec["trinoConfig"] = tc.clone();
    }
    preserve_trino_delegation_mode(&mut spec, &engine.spec);
    validate_spec_resource_groups(&spec)?;
    // Validate an optional ingress-TLS secret reference (400 on a missing /
    // non-TLS / incomplete secret) before persisting, mirroring create.
    if let Some(ing) = parse_ingress(&spec["trinoConfig"]["ingress"]) {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            resolve_tls_cert_secret(&st, ref_id).await?;
        }
    }
    // Validate an optional image-pull (dockerconfigjson) secret reference the
    // same way (400 on a missing / non-dockerconfigjson / incomplete secret)
    // before persisting, mirroring create.
    if let Some(ref_id) = spec["trinoConfig"]["imagePullSecretRef"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        resolve_dockerconfigjson_secret(&st, ref_id).await?;
    }

    // Snapshot what is stored so a failed build or apply can put it back — the
    // same contract `update_catalog` keeps for a catalog row.
    let snapshot_spec = engine.spec.clone();
    let snapshot_status = engine.status.clone();
    st.engine_mgmt
        .update_engine(id, &spec, &engine.status)
        .await?;
    let engine = match st.engine_mgmt.get_engine(id).await {
        Ok(Some(e)) => e,
        Ok(None) => return Err(CpError::NotFound),
        Err(e) => {
            restore_engine_spec(&st, id, &snapshot_spec, &snapshot_status).await;
            return Err(e.into());
        }
    };
    let built = match build_provision_req(&st, &engine).await {
        Ok(built) => built,
        Err(e) => {
            restore_engine_spec(&st, id, &snapshot_spec, &snapshot_status).await;
            return Err(e);
        }
    };
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        restore_engine_spec(&st, id, &snapshot_spec, &snapshot_status).await;
        return Err(CpError::ClusterApplyFailed(msg));
    }

    // Re-materialize every assigned workspace's connection so a delegation-mode
    // or TLS change reaches the query path — as `update_starrocks_engine` does,
    // and unconditionally for the same reason: a retried PUT must heal a
    // previous partial re-materialization, which a diff against the
    // already-persisted spec cannot see. Idempotent and cheap when unchanged.
    st.assignment
        .rematerialize_for_engine(id)
        .await
        .map_err(|e| {
            CpError::BadRequest(format!(
                "engine updated but re-materializing workspace connections failed: {e}"
            ))
        })?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(engine.into()))
}
```

- [ ] **Step 4: Same contract for StarRocks**

In `update_starrocks_engine`, replace the section from `let deleg_changed = old_delegation != sr_delegation_block(&spec);` through the closing `}` of `if let Err(msg) = prov.update(&req).await { … }` with:

```rust
    let deleg_changed = old_delegation != sr_delegation_block(&spec);
    // Refuse BEFORE persisting when nothing can apply the update.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Re-resolve the warehouse (same resolver create uses) so the FE/CN builtin
    // storage volume is fed the SAME S3 coords. It does not depend on the new
    // spec, so resolve it before persisting too.
    let wh = st
        .catalog
        .get_warehouse(engine.warehouse_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;

    // Snapshot for restore on a failed build or apply (see `update_engine`).
    let snapshot_spec = engine.spec.clone();
    let snapshot_status = engine.status.clone();
    st.engine_mgmt
        .update_engine(engine.id, &spec, &engine.status)
        .await?;
    let engine_id = engine.id;
    let engine = match st.engine_mgmt.get_engine(engine_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return Err(CpError::NotFound),
        Err(e) => {
            restore_engine_spec(&st, engine_id, &snapshot_spec, &snapshot_status).await;
            return Err(e.into());
        }
    };
    let (req, bootstrap) = match build_starrocks_provision_req(&st, &engine, &wh).await {
        Ok(v) => v,
        Err(e) => {
            restore_engine_spec(&st, engine_id, &snapshot_spec, &snapshot_status).await;
            return Err(e);
        }
    };
    if let Err(msg) = prov.update(&req).await {
        restore_engine_spec(&st, engine_id, &snapshot_spec, &snapshot_status).await;
        return Err(CpError::ClusterApplyFailed(msg));
    }
```

The code after it (the `deleg_changed` re-bootstrap and the rematerialization) is unchanged; its `prov.clone()` now clones the `Arc` directly.

- [ ] **Step 5: Run the unit tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib -- engines::tests::preserve_trino engines::tests::update_starrocks engines::tests::sr_update 2>&1 | tail -5
```

Expected: all pass; DB time > 0.3 s.

- [ ] **Step 6: Write the route tests**

Append to `routes.rs`:

```rust
async fn stored_spec(f: &Fixture, id: &str) -> serde_json::Value {
    sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine spec")
}

/// P3: a rejected apply restores the stored spec and is a 502 carrying the cause;
/// a body whose trinoConfig omits delegationMode keeps the stored mode.
#[tokio::test]
async fn trino_update_failure_restores_the_spec_and_keeps_delegation_mode() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p3_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    let before = stored_spec(&f, &id).await;
    assert_eq!(before["trinoConfig"]["delegationMode"], "jwt");

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 5,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(j["error"].as_str().unwrap().contains("injected update failure"), "body={j}");
    assert_eq!(stored_spec(&f, &id).await, before, "spec restored");

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": {
                    "coordinator": { "replicas": 1 },
                    "worker": { "replicas": 1 },
                    "auth": { "jwt": {} }
                }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update; body={j}");
    assert_eq!(
        stored_spec(&f, &id).await["trinoConfig"]["delegationMode"], "jwt",
        "omitted delegationMode must keep the stored mode"
    );
}

/// P4: a Trino update re-materializes the connections of assigned workspaces.
#[tokio::test]
async fn trino_update_rematerializes_assigned_workspace_connections() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p4_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let ws = seed_workspace(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws] })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Make the materialized connection stale.
    sqlx::query(
        "UPDATE query.databases SET options = '{}'::jsonb WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .execute(&f.db.pool)
    .await
    .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let scheme: Option<String> = sqlx::query_scalar(
        "SELECT options->>'http_scheme' FROM query.databases WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(scheme.as_deref(), Some("https"), "connection re-materialized");
}
```

- [ ] **Step 7: Run the route tests, then the whole crate**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes trino_update_ 2>&1 | tail -5
rtk proxy cargo test -p egret-engine-mgmt 2>&1 | grep -E "test result|finished in"
```

Expected: `2 passed`; every binary `ok`, DB binaries > 0.3 s.

- [ ] **Step 8: Regenerate `openapi.json`**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
rtk proxy cargo test -p egret-server --lib committed_openapi_json_is_current 2>&1 | tail -3
```

Expected: drift gate `ok`; `git diff openapi.json` shows only the `update_engine` responses. Regenerating the frontend client is not needed (no request/response schema changed); if you run `npm run gen:api` anyway, keep only files whose diff is about this handler.

- [ ] **Step 9: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs openapi.json
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): roll back failed engine updates; rematerialize Trino

Both update paths now refuse before persisting when Kubernetes is not
configured, restore the stored spec when the build or apply fails, and return
502 with the cause. A trinoConfig without delegationMode keeps the stored mode,
and a Trino update re-materializes assigned workspace connections as the
StarRocks path already did.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Remove a catalog from the cluster before deleting its row (P5)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `remove_catalog` (handler + `#[utoipa::path]`)
- Modify: `openapi.json` (regenerated)
- Test: `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `build_provision_req_with`, `BuildOpts { exclude_catalog }`, `apply_built`, `ApplyOp` (Task 7); `client_row_id`, `is_minted_client` (existing).

- [ ] **Step 1: Write the failing test**

Append to `routes.rs`:

```rust
/// P5: removing a catalog applies the cluster WITHOUT it first; a rejected apply
/// leaves the row and its minted client in place so a retry can act.
#[tokio::test]
async fn remove_catalog_applies_before_deleting_and_revoking() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p5_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "extra", "kind": "egret-warehouse",
                "config": { "warehouseId": wh_id }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add warehouse catalog; body={j}");
    let cat_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let client_id = store
        .get_engine_catalog(cat_id)
        .await
        .unwrap()
        .unwrap()
        .config["clientRowId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("minted client id");
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let remove = || req("DELETE", &format!("/engines/{engine_id}/catalogs/{cat_id}"), &token, None);

    port.set_fail_update(true);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        port.last_updated().catalogs.iter().all(|c| c.name != "extra"),
        "the apply rendered the engine without the catalog"
    );
    assert!(store.get_engine_catalog(cat_id).await.unwrap().is_some(), "row kept");
    assert!(
        catalog_store.list_clients(wh_id).await.unwrap().iter().any(|c| c.id == client_id),
        "client not revoked"
    );

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(store.get_engine_catalog(cat_id).await.unwrap().is_none(), "row deleted");
    assert!(
        !catalog_store.list_clients(wh_id).await.unwrap().iter().any(|c| c.id == client_id),
        "minted client revoked"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes remove_catalog_applies_before_deleting_and_revoking 2>&1 | tail -10
```

Expected: FAIL — `row kept` (today the row is deleted before the apply).

- [ ] **Step 3: Reorder `remove_catalog`**

Replace the `remove_catalog` `#[utoipa::path]` responses and the handler body after the primary-catalog guard. The full handler becomes:

```rust
#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Catalog id"),
    ),
    responses(
        (status = 200, description = "Catalog removed from the cluster, then its row deleted and its minted client revoked"),
        (status = 400, description = "Cannot delete the primary catalog, Kubernetes not configured, or the engine update failed (the catalog is kept)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:delete permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:delete").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let cat = st
        .engine_mgmt
        .get_engine_catalog(cid)
        .await?
        .ok_or(CpError::NotFound)?;
    // Guard against cross-engine deletes via a mismatched path.
    if cat.engine_id != engine.id {
        return Err(CpError::NotFound);
    }
    if cat.is_primary {
        return Err(CpError::BadRequest(
            "cannot delete the primary catalog".into(),
        ));
    }
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Drop the catalog from the cluster FIRST. Only once Kubernetes accepted
    // the spec without it are the row deleted and the credential revoked, so a
    // failure leaves row, client and running catalog consistent — and a retry
    // still has a row to act on.
    let built = build_provision_req_with(
        &st,
        &engine,
        BuildOpts {
            exclude_catalog: Some(cat.id),
            ..BuildOpts::default()
        },
    )
    .await?;
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
    st.engine_mgmt.delete_engine_catalog(cat.id).await?;
    // Retire a minted Catalog Client (egret-warehouse catalogs). Gated on
    // `mintedClient` so a client we did NOT mint is never revoked out from
    // under its owner. The cluster no longer uses it, so a failed revoke is
    // logged rather than failing a removal that already happened.
    if let Some(client_id) = client_row_id(&cat.config) {
        if is_minted_client(&cat.config) {
            if let Err(e) = st.catalog.revoke_client(client_id).await {
                tracing::warn!(engine = %engine.name, %client_id, error = ?e,
                    "catalog removed but its minted client could not be revoked");
            }
        }
    }
    Ok(StatusCode::OK)
}
```

(`CatalogApiError` must be `Debug` for `error = ?e`; it is used with `?`/`From` elsewhere — if the compiler reports it is not `Debug`, use `error = %e`.)

- [ ] **Step 4: Run the test and the catalog tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes remove_catalog_applies_before_deleting_and_revoking engine_catalog_routes add_catalog_with_client_id_inherits_role_and_mints_new_client 2>&1 | tail -5
```

Expected: `3 passed`, DB time > 0.3 s.

- [ ] **Step 5: Regenerate `openapi.json`**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
rtk proxy cargo test -p egret-server --lib committed_openapi_json_is_current 2>&1 | tail -3
```

Expected: `ok`; only the `remove_catalog` response descriptions changed.

- [ ] **Step 6: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs openapi.json
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): remove a catalog from the cluster before its row

remove_catalog deleted the row and revoked the credential before updating
Kubernetes, so a failed update left the cluster using a revoked credential and
nothing to retry. Apply without the catalog first; delete and revoke only
after the apply succeeds.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Compensate a failed create completely (P2)

**Files:**
- Modify: `crates/egret-policy-api/src/port.rs` (trait), `crates/egret-policy/src/port_impl.rs` (impl)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — new `CreateUndo`, `engine_owned_secret_ids`, `compensate_failed_create` (place after `is_minted_client`, `:1914`); Trino `create_engine` steps 4–7; `create_starrocks_engine` from the client mint through the provision call
- Test: `engines.rs` `mod tests`, `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `apply_built`, `build_provision_req` (Task 7); `engine_status::FAILED` (Task 4); `Engine::{internal_secret_id, monitor_secret_id}` (Task 6).
- Produces:
  - `PolicyPort::delete_service(&self, id: i64) -> Result<(), PolicyError>`
  - `#[derive(Debug, Default)] struct CreateUndo { client_id: Option<Uuid>, engine_id: Option<Uuid>, cluster: Option<(String, String, String)>, created_profile_id: Option<i64>, created_tag_service_id: Option<i64> }`
  - `fn engine_owned_secret_ids(engine: &Engine) -> Vec<Uuid>` — delegator, TLS keystore, root, internal, monitor; **never** `tls_cert_secret_ref`. Reused by Task 15.
  - `async fn compensate_failed_create(st: &CpState, undo: CreateUndo)`

- [ ] **Step 1: Check whether Part A already added `delete_service`**

```bash
rtk proxy grep -n "fn delete_service" crates/egret-policy-api/src/port.rs crates/egret-policy/src/port_impl.rs
```

Part A does NOT add it (user decision 2026-09-15: engine delete keeps profiles), so expect no hits and do Step 2. If both files somehow already have it, skip Step 2.

- [ ] **Step 2: Add `PolicyPort::delete_service`**

In `port.rs`, after `create_service`:

```rust
    /// Delete a policy profile (service) and, by cascade, its policies. Errors
    /// `NotFound` when no such id exists. Used to undo a profile an engine create
    /// minted before failing.
    async fn delete_service(&self, id: i64) -> Result<(), PolicyError>;
```

In `port_impl.rs`, after the `create_service` impl:

```rust
    async fn delete_service(&self, id: i64) -> Result<(), PolicyError> {
        Ok(self.delete_service(id).await?)
    }
```

```bash
rtk proxy cargo check -p egret-policy -p egret-engine-mgmt 2>&1 | tail -3
```

Expected: clean (`PolicyStore` is the only `PolicyPort` implementor; the inherent `delete_service` wins method resolution inside the impl).

- [ ] **Step 3: Write the failing unit test**

In `engines.rs` `mod tests`:

```rust
    /// P2: compensation deletes the engine row, every secret egret minted for it
    /// (including pending internal secrets already recorded), and the catalog
    /// client — and never the admin-provided coordinator TLS secret.
    #[tokio::test]
    async fn compensate_failed_create_removes_everything_it_minted() {
        let Some(st) = test_state().await else {
            eprintln!("skipping compensate_failed_create_removes_everything_it_minted: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let mut tls = std::collections::BTreeMap::new();
        tls.insert("tls.crt".to_string(), "CERTPEM".to_string());
        tls.insert("tls.key".to_string(), "KEYPEM".to_string());
        let admin_tls = st
            .secrets
            .create_typed(
                None,
                &format!("company-tls-{}", Uuid::new_v4().simple()),
                None,
                egret_secrets::secret_type::TLS,
                &tls,
            )
            .await
            .expect("admin tls secret");
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                wh.id,
                &serde_json::json!({ "trinoConfig": { "delegationMode": "impersonate", "auth": {} } }),
                Some(admin_tls),
            )
            .await
            .expect("engine row");
        let (delegator_id, _) = ensure_delegator_secret(&st, &engine).await.expect("delegator");
        let engine = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        let internal = resolve_internal_secrets(&st, &engine, false).await.expect("internal");
        commit_internal_secrets(&st, &engine, &internal).await.expect("commit");
        let role_id = ensure_engine_role(&st, wh.id).await.expect("role");
        let client = st
            .catalog
            .create_client(NewClient {
                name: format!("engine-{}", engine.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await
            .expect("client");

        compensate_failed_create(
            &st,
            CreateUndo {
                client_id: Some(client.id),
                engine_id: Some(engine.id),
                ..CreateUndo::default()
            },
        )
        .await;

        assert!(st.engine_mgmt.get_engine(engine.id).await.unwrap().is_none());
        for id in [delegator_id, internal.internal_id, internal.monitor_id] {
            assert!(
                matches!(
                    st.secrets.resolve_plaintext(None, id).await,
                    Err(egret_secrets::SecretsError::NotFound)
                ),
                "secret {id} must be deleted"
            );
        }
        assert!(
            st.secrets.resolve_typed(None, admin_tls).await.is_ok(),
            "an admin-provided TLS secret is never deleted"
        );
        assert!(!st
            .catalog
            .list_clients(wh.id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client.id));
    }

    #[test]
    fn engine_owned_secret_ids_excludes_the_admin_tls_reference() {
        let mut e = dummy_engine();
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::new_v4()).collect();
        e.delegator_secret_id = Some(ids[0]);
        e.tls_secret_id = Some(ids[1]);
        e.root_secret_id = Some(ids[2]);
        e.internal_secret_id = Some(ids[3]);
        e.monitor_secret_id = Some(ids[4]);
        e.tls_cert_secret_ref = Some(ids[5]);
        assert_eq!(engine_owned_secret_ids(&e), ids[..5].to_vec());
    }
```

- [ ] **Step 4: Run to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --lib -- engines::tests::compensate_failed_create engines::tests::engine_owned_secret_ids 2>&1 | tail -10
```

Expected: compile error — `compensate_failed_create`, `CreateUndo`, `engine_owned_secret_ids` not found.

- [ ] **Step 5: Implement compensation**

After `is_minted_client` add:

```rust
/// What an engine create has minted so far; [`compensate_failed_create`]
/// undoes exactly this and nothing else.
#[derive(Debug, Default)]
struct CreateUndo {
    /// The auto-wired Catalog Client.
    client_id: Option<Uuid>,
    /// The engine row (its owned secrets are read off it at undo time).
    engine_id: Option<Uuid>,
    /// `(kind, namespace, name)` once a provision has been attempted.
    cluster: Option<(String, String, String)>,
    /// A Ranger profile THIS create made (never a pre-existing one).
    created_profile_id: Option<i64>,
    /// A tag service THIS create made (never a pre-existing one).
    created_tag_service_id: Option<i64>,
}

/// Secret rows egret minted for this engine and owns outright.
/// `tls_cert_secret_ref` is deliberately absent: it points at an admin-provided
/// `kubernetes.io/tls` secret that other engines may share.
fn engine_owned_secret_ids(engine: &Engine) -> Vec<Uuid> {
    [
        engine.delegator_secret_id,
        engine.tls_secret_id,
        engine.root_secret_id,
        engine.internal_secret_id,
        engine.monitor_secret_id,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Undo a failed engine create. Every step logs its own failure and continues.
/// Order: cluster, row (then the secrets it referenced — a row that cannot be
/// deleted keeps its secrets and is marked `failed`), profiles this create made
/// (their policies cascade), catalog client. Policies seeded into a PRE-EXISTING
/// profile are left alone: other engines may be bound to it.
async fn compensate_failed_create(st: &CpState, undo: CreateUndo) {
    if let (Some((kind, namespace, name)), Some(prov)) =
        (&undo.cluster, st.engine_provisioner.as_ref())
    {
        if let Err(e) = prov.delete(kind, namespace, name).await {
            tracing::error!(engine = %name, error = %e,
                "create compensation: cluster delete failed; objects may remain");
        }
    }
    if let Some(engine_id) = undo.engine_id {
        match st.engine_mgmt.get_engine(engine_id).await {
            Ok(Some(engine)) => {
                let owned = engine_owned_secret_ids(&engine);
                match st.engine_mgmt.delete_engine(engine_id).await {
                    Ok(()) => {
                        for secret_id in owned {
                            if let Err(e) = st.secrets.delete(None, secret_id).await {
                                tracing::error!(engine = %engine.name, %secret_id, error = %e,
                                    "create compensation: secret delete failed");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(engine = %engine.name, error = %e,
                            "create compensation: engine row delete failed; marking it failed");
                        if let Err(e) = st
                            .engine_mgmt
                            .set_engine_status(engine_id, crate::engine_status::FAILED)
                            .await
                        {
                            tracing::error!(engine = %engine.name, error = %e,
                                "create compensation: could not mark the engine failed");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!(%engine_id, error = %e,
                "create compensation: could not load the engine row"),
        }
    }
    for (what, id) in [
        ("policy profile", undo.created_profile_id),
        ("tag service", undo.created_tag_service_id),
    ] {
        if let Some(id) = id {
            if let Err(e) = st.authz.delete_service(id).await {
                tracing::error!(service_id = id, error = %e,
                    "create compensation: {what} delete failed");
            }
        }
    }
    if let Some(client_id) = undo.client_id {
        if let Err(e) = st.catalog.revoke_client(client_id).await {
            tracing::error!(%client_id, error = ?e,
                "create compensation: catalog client revoke failed");
        }
    }
}
```

(If Part A changed `EngineMgmtStore::delete_engine`'s signature — e.g. to take a transaction — call the plain-pool variant Part A kept for this purpose, or add `delete_engine` back as a thin pool wrapper.)

- [ ] **Step 6: Trino create uses it**

In `create_engine`, replace everything from the comment `// 4. Auto-wire: mint a fresh Catalog Client for the warehouse.` through the closing `}` of the `if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Provision).await { … }` block with:

```rust
    // 4. Assemble and validate the stored spec BEFORE minting anything, so an
    //    invalid resource-group config 400s with nothing to roll back.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let namespace = DEFAULT_NAMESPACE.to_string();
    let mut spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
    });
    spec["trinoConfig"] = trino_config;
    validate_spec_resource_groups(&spec)?;
    // Kubernetes must be configured before anything is minted or rendered.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // 5. Auto-wire: mint a fresh Catalog Client for the warehouse. Minted only
    //    AFTER delegation resolves (step 3), so a resolver 400 leaks no client.
    //    From here on, every failure runs `compensate_failed_create`.
    let role_id = ensure_engine_role(&st, wh.id).await?;
    let client = st
        .catalog
        .create_client(NewClient {
            name: format!("engine-{}", b.name),
            consumer_type: "engine".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh.id,
            catalog_role_id: role_id,
        })
        .await?;
    let mut undo = CreateUndo {
        client_id: Some(client.id),
        ..CreateUndo::default()
    };

    // 6. Record the intent row (status `provisioning`). The auto-wired client id
    //    lives on the PRIMARY catalog row, not in the engine spec.
    let engine = match st
        .engine_mgmt
        .create_engine(
            &b.name,
            "trino",
            &namespace,
            wh.id,
            &spec,
            b.tls_cert_secret_ref,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };
    undo.engine_id = Some(engine.id);

    // 7. Record the PRIMARY catalog row (the auto-wired egret warehouse).
    let cat_config = serde_json::json!({
        "warehouse": wh.name,
        "s3_endpoint": s3_endpoint,
        "credential": format!("{}:{}", client.client_id, client.client_secret),
        "clientRowId": client.id,
    });
    if let Err(e) = st
        .engine_mgmt
        .create_engine_catalog(
            engine.id,
            "egret",
            "egret-warehouse",
            &cat_config,
            None,
            true,
        )
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }

    // 8. Build from the stored engine + catalogs and provision.
    let built = match build_provision_req(&st, &engine).await {
        Ok(built) => built,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    };
    undo.cluster = Some((
        engine.r#type.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
    ));
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Provision).await {
        compensate_failed_create(&st, undo).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }
```

Renumber the following comment `// 8. Auto-bind the engine to a Ranger policy profile` to `// 9. Auto-bind …`. That block keeps its best-effort behaviour until Task 13 replaces it.

- [ ] **Step 7: StarRocks create uses it**

In `create_starrocks_engine`, directly after `validate_sr_autoscaling(&sr_body)?;` insert:

```rust
    // Kubernetes must be configured before anything is minted.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
```

Then replace everything from `let client = st` (the `create_client` call after `let role_id = ensure_engine_role(&st, wh.id).await?;`) through the closing `}` of `if let Err(msg) = prov.provision(&req).await { … }` with:

```rust
    let client = st
        .catalog
        .create_client(NewClient {
            name: format!("engine-{}", b.name),
            consumer_type: "engine".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh.id,
            catalog_role_id: role_id,
        })
        .await?;
    let mut undo = CreateUndo {
        client_id: Some(client.id),
        ..CreateUndo::default()
    };

    let namespace = DEFAULT_NAMESPACE.to_string();
    // Stored spec (identical builder create and update share). The delegation
    // block is persisted so `store::Engine::delegation_mode`/
    // `supports_jwt_delegation`/`supports_delegation` (read back on every
    // subsequent load, including at `EngineAssignmentService::assign` time) see
    // the SAME mode this create call configured. An omitted mode serializes to a
    // JSON `null`, which `store::Engine::delegation_mode`'s `.get("mode")` chain
    // treats exactly like a wholly-absent key — it must NOT default to
    // impersonate the way Trino's `trinoConfig.delegationMode` does.
    let mut spec = build_starrocks_spec(&sr_body, None);
    // Persist the FE HTTP ingress at TOP-LEVEL `spec["ingress"]` (kind-agnostic —
    // `build_starrocks_provision_req` reads it back into `config.ingress`, the
    // field the provisioner's `starrocks_ingress` renders from). The shared
    // create form builds `trinoConfig.ingress` even for a StarRocks engine, so the
    // wire ingress arrives on `b.trino_config`; copy it in when present.
    if let Some(ing) = b.trino_config.as_ref().and_then(|tc| tc.get("ingress")) {
        spec["ingress"] = ing.clone();
    }

    let engine = match st
        .engine_mgmt
        .create_engine(
            &b.name,
            "starrocks",
            &namespace,
            wh.id,
            &spec,
            b.tls_cert_secret_ref,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };
    undo.engine_id = Some(engine.id);

    // Step 2: record the PRIMARY catalog row — same shape/kind Trino's
    // auto-wire uses (`egret-warehouse`). No `s3_endpoint` key: the StarRocks
    // side never reads this row back into a rendered config file — it only
    // carries the credential the background wire step consumes.
    let cat_config = serde_json::json!({
        "warehouse": wh.name,
        "credential": format!("{}:{}", client.client_id, client.client_secret),
        "clientRowId": client.id,
    });
    if let Err(e) = st
        .engine_mgmt
        .create_engine_catalog(
            engine.id,
            "egret",
            "egret-warehouse",
            &cat_config,
            None,
            true,
        )
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }

    // Optional Ranger bind to an EXISTING profile (validated the same way the
    // Trino auto-bind step validates `authzServiceId`).
    if let Some(sid) = b.authz_service_id {
        match st.authz.list_services().await {
            Ok(svcs) if svcs.iter().any(|s| s.id == sid) => {
                if let Err(e) = st
                    .engine_mgmt
                    .set_engine_authz_service(engine.id, Some(sid))
                    .await
                {
                    tracing::warn!(error = %e, "set_engine_authz_service failed for starrocks create");
                }
            }
            Ok(_) => {
                tracing::warn!(
                    sid,
                    "authz_service_id not found; leaving starrocks engine unlinked"
                );
            }
            Err(e) => tracing::warn!(error = %e, "list_services failed during starrocks create"),
        }
    }
    // Secure-by-default: mint + store a strong random FE `root` password and
    // persist its `root_secret_id` UNCONDITIONALLY. Legacy engines (no
    // `root_secret_id`) stay passwordless.
    if let Err(e) = ensure_root_password_secret(&st, &engine).await {
        compensate_failed_create(&st, undo).await;
        return Err(e);
    }

    let engine = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            compensate_failed_create(&st, undo).await;
            return Err(CpError::NotFound);
        }
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };

    // Assemble the provisioning request from the STORED engine row — the SAME
    // shared helper a later update will call. For a delegation-enabled spec it
    // idempotently mints/reuses the TLS cert + delegator secret, which the
    // compensation below removes again on failure.
    let (req, delegation_bootstrap) = match build_starrocks_provision_req(&st, &engine, &wh).await {
        Ok(v) => v,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    };

    undo.cluster = Some((
        engine.r#type.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
    ));
    if let Err(msg) = prov.provision(&req).await {
        compensate_failed_create(&st, undo).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }
```

The rest (`// Step 3: best-effort, non-blocking post-provision wire.` onwards) is unchanged; `prov.clone()` there now clones the `Arc`.

- [ ] **Step 8: Run the unit tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib 2>&1 | tail -5
```

Expected: `test result: ok.` including both new tests and every `trino_create_*` / `sr_create_*` test.

- [ ] **Step 9: Write the route test**

Append to `routes.rs`:

```rust
/// P2: a create whose provision fails leaves no engine row and no catalog client,
/// and asks the cluster to delete what it may have applied — for both kinds.
#[tokio::test]
async fn failed_provision_rolls_back_row_client_and_cluster() {
    let port = FakeEnginePort::default();
    port.set_fail_provision(true);
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p2_admin", true).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let row_exists = |name: String| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f, &token, &name, wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(!row_exists(name.clone()).await, "Trino row rolled back");
    assert!(catalog_store.list_clients(wh_id).await.unwrap().is_empty(), "Trino client revoked");
    assert!(port
        .deleted
        .lock()
        .unwrap()
        .contains(&("trino".to_string(), "egret-engines".to_string(), name.clone())));

    let sr_wh = seed_provisioned_warehouse_with_secret(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": sr_name, "warehouseId": sr_wh, "kind": "starrocks",
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(!row_exists(sr_name.clone()).await, "StarRocks row rolled back");
    assert!(catalog_store.list_clients(sr_wh).await.unwrap().is_empty(), "StarRocks client revoked");
    assert!(port
        .deleted
        .lock()
        .unwrap()
        .contains(&("starrocks".to_string(), "egret-engines".to_string(), sr_name)));
}
```

- [ ] **Step 10: Run it**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes failed_provision_rolls_back_row_client_and_cluster 2>&1 | tail -5
```

Expected: `1 passed`, > 0.3 s. (The secret cleanup on this path is proven by the unit test in Step 3; a route-level secret count would race the concurrently running tests that share the per-process database.)

- [ ] **Step 11: Commit**

```bash
git branch --show-current
git add crates/egret-policy-api/src/port.rs crates/egret-policy/src/port_impl.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): undo everything a failed engine create minted

A failed create deleted the row and revoked the client but left the TLS,
delegator, root and internal secret rows and any applied cluster objects. One
compensation now removes the cluster, the row and the secrets it owns (never an
admin-provided TLS secret), profiles the create made, and the client; a row it
cannot delete is marked failed. Resource-group validation moves before the
client mint, so that 400 no longer leaks a client.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Never provision a Trino engine unbound while Ranger is on (D9)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/store.rs` (new method after `set_engine_authz_service`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — new `ProfilePlan`, `plan_trino_profile`, `bind_trino_profile` (after `compensate_failed_create`); `create_engine` steps 4–9
- Test: `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `CreateUndo`, `compensate_failed_create` (Task 12); `setup_with_ranger` (Task 1); `PolicyPort::{list_services, get_service_opt, ensure_trino_def, ensure_tag_def, create_service}`.
- Produces:
  - `EngineMgmtStore::count_engines_bound_to_authz_service(&self, service_id: i64) -> Result<i64, EngineMgmtError>` — Part A announces a method of this name for its "no other engine is bound" rule; Step 3 adds it only if it is missing
  - `#[derive(Debug, Clone, PartialEq, Eq)] enum ProfilePlan { Unbound, Existing(i64), Create }`
  - `async fn plan_trino_profile(st: &CpState, name: &str, requested: Option<i64>) -> Result<ProfilePlan, CpError>`
  - `async fn bind_trino_profile(st: &CpState, engine_id: Uuid, name: &str, plan: &ProfilePlan, undo: &mut CreateUndo) -> Result<(), CpError>`

- [ ] **Step 1: Write the failing route tests**

Append to `routes.rs`:

```rust
async fn authz_service_of(f: &Fixture, name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT authz_service_id FROM engine_mgmt.engines WHERE name = $1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("engine row")
}

/// D9 regression: re-creating an engine whose same-name profile survived (no
/// engine bound) reuses that profile, and the FIRST provision is already bound —
/// never an allow-all window.
#[tokio::test]
async fn recreate_with_same_name_binds_the_orphan_profile_before_provisioning() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_orphan_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy_api::PolicyPort::ensure_trino_def(&policy).await.unwrap();
    let orphan = egret_policy_api::PolicyPort::create_service(
        &policy, &name, "trino", &serde_json::json!({}),
    )
    .await
    .unwrap();

    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "body={j}");
    assert_eq!(authz_service_of(&f, &name).await, Some(orphan.id));
    let provisioned = port.provisioned.lock().unwrap().clone();
    assert_eq!(provisioned.len(), 1, "one provision, no unbound first pass");
    assert_eq!(
        provisioned[0].ranger.as_ref().map(|r| r.service_name.clone()),
        Some(name.clone())
    );
}

/// D9: a same-name profile already bound to another engine is a 409 before
/// anything is minted.
#[tokio::test]
async fn create_whose_profile_is_bound_elsewhere_is_409_and_mints_nothing() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_bound_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy_api::PolicyPort::ensure_trino_def(&policy).await.unwrap();
    let profile = egret_policy_api::PolicyPort::create_service(
        &policy, &name, "trino", &serde_json::json!({}),
    )
    .await
    .unwrap();
    let other = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": other, "warehouseId": wh_id, "authzServiceId": profile.id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(authz_service_of(&f, &other).await, Some(profile.id));
    let provisions_before = port.provisioned.lock().unwrap().len();

    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)",
    )
    .bind(&name)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert!(!exists);
    let clients = egret_catalog_engine::CatalogStore::new(f.db.clone())
        .list_clients(wh_id)
        .await
        .unwrap();
    assert!(!clients.iter().any(|c| c.name == format!("engine-{name}")));
    assert_eq!(port.provisioned.lock().unwrap().len(), provisions_before);
}

/// D9 + P2: a create that auto-created its profile removes it again on failure.
#[tokio::test]
async fn failed_create_removes_the_profile_it_created() {
    let port = FakeEnginePort::default();
    port.set_fail_provision(true);
    let Some(f) = setup_with_ranger(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
        true,
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_d9_undo_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(
        port.last_provisioned().ranger.is_some(),
        "the attempted provision was already bound"
    );
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    assert!(egret_policy_api::PolicyPort::get_service_opt(&policy, &name).await.unwrap().is_none());
    assert!(egret_policy_api::PolicyPort::get_service_opt(&policy, &format!("{name}_tag"))
        .await
        .unwrap()
        .is_none());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes -- recreate_with_same_name create_whose_profile_is_bound_elsewhere failed_create_removes_the_profile 2>&1 | grep -E "panicked|test result"
```

Expected: 3 failures — the orphan test sees `authz_service_id = None` (create_service `AlreadyExists` → unbound) and two provisions; the bound test gets 201; the undo test sees an unbound attempt (`ranger.is_none()`).

- [ ] **Step 3: Store count (only if Part A did not add it)**

```bash
rtk proxy grep -n "fn count_engines_bound_to_authz_service" -A 4 crates/egret-engine-mgmt/src/store.rs
```

If it exists and takes a service id `i64` and returns `Result<i64, EngineMgmtError>`, skip to Step 4. If it exists with another shape (for example `u64`, or an "other than engine X" parameter), call it accordingly in `plan_trino_profile` below and skip the rest of this step. Otherwise, after `set_engine_authz_service` in `store.rs` add:

```rust
    /// How many engines are bound to a Ranger profile.
    pub async fn count_engines_bound_to_authz_service(
        &self,
        service_id: i64,
    ) -> Result<i64, EngineMgmtError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1",
        )
        .bind(service_id)
        .fetch_one(self.pool())
        .await?)
    }
```

- [ ] **Step 4: Profile planning and binding**

After `compensate_failed_create` in `engines.rs`:

```rust
/// How a new Trino engine gets its Ranger profile — decided before anything is
/// minted, so a conflict is a clean 409.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfilePlan {
    /// No binding: Ranger is disabled and no valid profile was chosen. Trino
    /// defaults to allow-all, which is what delegated impersonation needs there.
    Unbound,
    /// Bind to a profile that already exists (chosen, or an orphan left by a
    /// deleted same-name engine). Compensation never deletes it.
    Existing(i64),
    /// Create `<name>` (trino) and `<name>_tag` (tag) during the create.
    Create,
}

/// Decide the profile for a Trino create named `name`.
async fn plan_trino_profile(
    st: &CpState,
    name: &str,
    requested: Option<i64>,
) -> Result<ProfilePlan, CpError> {
    if let Some(sid) = requested {
        let services = st
            .authz
            .list_services()
            .await
            .map_err(|e| CpError::BadRequest(format!("list policy profiles: {e}")))?;
        if services.iter().any(|s| s.id == sid) {
            return Ok(ProfilePlan::Existing(sid));
        }
        tracing::warn!(sid, "authz_service_id not found");
    }
    if !st.ranger_enabled {
        tracing::info!(
            engine = %name,
            "ranger disabled; Trino engine provisioned without Ranger access-control (allow-all)"
        );
        return Ok(ProfilePlan::Unbound);
    }
    match st
        .authz
        .get_service_opt(name)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve policy profile: {e}")))?
    {
        None => Ok(ProfilePlan::Create),
        Some(svc) if svc.r#type != "trino" => Err(CpError::Conflict(format!(
            "a policy profile named {name} already exists with type {}",
            svc.r#type
        ))),
        Some(svc) => match st.engine_mgmt.count_engines_bound_to_authz_service(svc.id).await? {
            0 => Ok(ProfilePlan::Existing(svc.id)),
            bound => Err(CpError::Conflict(format!(
                "policy profile {name} is already bound to {bound} other engine(s)"
            ))),
        },
    }
}

/// Carry out a [`ProfilePlan`] for the new engine row, recording anything it
/// creates in `undo`.
async fn bind_trino_profile(
    st: &CpState,
    engine_id: Uuid,
    name: &str,
    plan: &ProfilePlan,
    undo: &mut CreateUndo,
) -> Result<(), CpError> {
    let profile_id = match plan {
        ProfilePlan::Unbound => return Ok(()),
        ProfilePlan::Existing(id) => *id,
        ProfilePlan::Create => {
            st.authz
                .ensure_trino_def()
                .await
                .map_err(|e| CpError::BadRequest(format!("seed trino service-def: {e}")))?;
            // A TAG service alongside the trino profile (Access P3.2), linked by
            // the profile's `tagService` config, so tag-based masking/row-filter
            // policies have a service. Best-effort: the profile works untagged.
            let tag_service_name = format!("{name}_tag");
            if let Err(e) = st.authz.ensure_tag_def().await {
                tracing::warn!(error = %e, "ensure_tag_def failed during auto-bind");
            }
            let tag_wired = match st
                .authz
                .create_service(&tag_service_name, "tag", &serde_json::json!({}))
                .await
            {
                Ok(svc) => {
                    undo.created_tag_service_id = Some(svc.id);
                    true
                }
                // Left by a deleted same-name engine: reuse, never delete.
                Err(egret_policy_api::PolicyError::AlreadyExists(_)) => true,
                Err(e) => {
                    tracing::warn!(error = %e, "create tag service failed during auto-bind");
                    false
                }
            };
            let config = if tag_wired {
                serde_json::json!({ "tagService": tag_service_name })
            } else {
                serde_json::json!({})
            };
            let svc = match st.authz.create_service(name, "trino", &config).await {
                Ok(svc) => svc,
                Err(egret_policy_api::PolicyError::AlreadyExists(_)) => {
                    return Err(CpError::Conflict(format!(
                        "policy profile {name} was created concurrently; retry the create"
                    )))
                }
                Err(e) => {
                    return Err(CpError::BadRequest(format!("create policy profile: {e}")))
                }
            };
            undo.created_profile_id = Some(svc.id);
            svc.id
        }
    };
    st.engine_mgmt
        .set_engine_authz_service(engine_id, Some(profile_id))
        .await?;
    Ok(())
}
```

- [ ] **Step 5: Wire it into `create_engine`**

1. Directly after the `let Some(prov) = st.engine_provisioner.clone() else { … };` added in Task 12 step 6, insert:

```rust
    // Decide the Ranger profile BEFORE minting anything: a same-name profile
    // bound to another engine is a 409 with nothing to roll back.
    let profile_plan = plan_trino_profile(&st, &b.name, b.authz_service_id).await?;
```

2. Replace the Task 12 block from `// 8. Build from the stored engine + catalogs and provision.` through the end of the function (including the whole `// 9. Auto-bind …` section, the `if let Some(pid) = profile_id { … }` re-provision, and the final `Ok((StatusCode::CREATED, Json(engine.into())))`) with:

```rust
    // 8. Bind the Ranger profile BEFORE the first provision, so a governed
    //    engine never runs unbound (allow-all) — not even between two applies.
    if let Err(e) = bind_trino_profile(&st, engine.id, &b.name, &profile_plan, &mut undo).await {
        compensate_failed_create(&st, undo).await;
        return Err(e);
    }
    let engine = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            compensate_failed_create(&st, undo).await;
            return Err(CpError::NotFound);
        }
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };

    // 9. Build from the stored (bound) engine + catalogs and provision once.
    let built = match build_provision_req(&st, &engine).await {
        Ok(built) => built,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    };
    undo.cluster = Some((
        engine.r#type.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
    ));
    if let Err(msg) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Provision).await {
        compensate_failed_create(&st, undo).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }

    Ok((StatusCode::CREATED, Json(engine.into())))
}
```

Update the `create_engine` `#[utoipa::path]` 409 description to `"An engine with this name already exists, or its same-name policy profile is bound to another engine"`.

- [ ] **Step 6: Run the D9 tests and the crate**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes -- recreate_with_same_name create_whose_profile_is_bound_elsewhere failed_create_removes_the_profile 2>&1 | tail -5
rtk proxy cargo test -p egret-engine-mgmt 2>&1 | grep -E "test result|finished in"
```

Expected: `3 passed`; every binary `ok`, DB binaries > 0.3 s.

- [ ] **Step 7: Regenerate `openapi.json`**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
rtk proxy cargo test -p egret-server --lib committed_openapi_json_is_current 2>&1 | tail -3
```

Expected: `ok`; only the create 409 description changed.

- [ ] **Step 8: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs openapi.json
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): never provision a Trino engine unbound under Ranger

Re-creating an engine whose same-name profile survived hit AlreadyExists,
logged a warning and provisioned the engine unbound, so Trino allowed
everything while Ranger was enabled. The profile is now decided before
anything is minted: an unbound orphan is reused, a bound one is a 409, and the
engine is bound before its first and only provision.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: StarRocks bootstrap stops when its engine is gone (P7)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `spawn_starrocks_catalog_wire` (signature + five checks), its two call sites (in `create_starrocks_engine` and `update_starrocks_engine`); new `bootstrap_target_is_current`, `engine_still_current` directly above `spawn_starrocks_catalog_wire`
- Test: `engines.rs` `mod tests`

**Interfaces:**
- Consumes: `engine_status::DELETING` (Task 4).
- Produces:
  - `fn bootstrap_target_is_current(found: Option<&Engine>, engine_name: &str) -> bool`
  - `async fn engine_still_current(store: &crate::EngineMgmtStore, engine_id: Uuid, engine_name: &str) -> bool`
  - `spawn_starrocks_catalog_wire(prov, store: crate::EngineMgmtStore, engine_id: Uuid, namespace, engine_name, egret_url, warehouse_name, client_id, client_secret, root_password, delegation)`

- [ ] **Step 1: Write the failing tests**

In `engines.rs` `mod tests`:

```rust
    #[test]
    fn bootstrap_stops_when_the_engine_is_gone_renamed_or_deleting() {
        let mut e = dummy_engine();
        e.name = "sr1".into();
        e.status = crate::engine_status::RUNNING.into();
        assert!(bootstrap_target_is_current(Some(&e), "sr1"));
        assert!(!bootstrap_target_is_current(None, "sr1"), "deleted (or re-created under a new id)");
        assert!(!bootstrap_target_is_current(Some(&e), "sr2"), "not the engine it was spawned for");
        e.status = crate::engine_status::DELETING.into();
        assert!(!bootstrap_target_is_current(Some(&e), "sr1"), "delete in progress");
    }

    #[tokio::test]
    async fn engine_still_current_reads_the_row() {
        let Some(st) = test_state().await else {
            eprintln!("skipping engine_still_current_reads_the_row: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;
        assert!(engine_still_current(&st.engine_mgmt, engine.id, &engine.name).await);
        st.engine_mgmt.delete_engine(engine.id).await.unwrap();
        assert!(!engine_still_current(&st.engine_mgmt, engine.id, &engine.name).await);
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --lib -- engines::tests::bootstrap_stops engines::tests::engine_still_current 2>&1 | tail -10
```

Expected: compile error — functions not found.

- [ ] **Step 3: Implement the check**

Directly above the doc comment of `spawn_starrocks_catalog_wire` add:

```rust
/// Whether a background StarRocks bootstrap may still write to the engine it was
/// spawned for: the row must still exist under the same id and name and not be
/// on its way out. A delete, or a delete plus same-name re-create (new id), stops
/// it instead of running DDL against a cluster it no longer owns.
fn bootstrap_target_is_current(found: Option<&Engine>, engine_name: &str) -> bool {
    matches!(found, Some(e) if e.name == engine_name && e.status != crate::engine_status::DELETING)
}

/// Re-read the engine and apply [`bootstrap_target_is_current`]; a read error
/// also stops the bootstrap.
async fn engine_still_current(
    store: &crate::EngineMgmtStore,
    engine_id: Uuid,
    engine_name: &str,
) -> bool {
    let current = match store.get_engine(engine_id).await {
        Ok(found) => bootstrap_target_is_current(found.as_ref(), engine_name),
        Err(e) => {
            tracing::warn!(engine = %engine_name, error = %e,
                "starrocks bootstrap: could not re-read the engine; stopping");
            false
        }
    };
    if !current {
        tracing::info!(engine = %engine_name, %engine_id,
            "starrocks bootstrap: engine deleted, re-created or deleting; stopping");
    }
    current
}
```

Change the signature of `spawn_starrocks_catalog_wire` to:

```rust
#[allow(clippy::too_many_arguments)]
fn spawn_starrocks_catalog_wire(
    prov: Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>,
    store: crate::EngineMgmtStore,
    engine_id: Uuid,
    namespace: String,
    engine_name: String,
    egret_url: String,
    warehouse_name: String,
    client_id: String,
    client_secret: String,
    root_password: Option<String>,
    delegation: Option<StarRocksDelegationBootstrap>,
) {
```

Insert the guard `if !engine_still_current(&store, engine_id, &engine_name).await { return; }` at these five places inside the spawned task:

1. first line inside the FE-ready poll loop — after `for _ in 0..MAX_POLLS {` that precedes `match prov.status("starrocks", …)`;
2. first line inside the root-login poll loop — after `for _ in 0..MAX_POLLS {` that precedes `match run_starrocks_ddl(&fe_host, STARROCKS_FE_MYSQL_PORT, "SELECT 1", …)`;
3. directly before `let credential = format!("{client_id}:{client_secret}");`;
4. directly before `let create_integration = stmts.remove(0);`;
5. first line inside `for stmt in &stmts {`.

Update both call sites to pass the two new arguments right after `prov.clone(),`:

```rust
        st.engine_mgmt.clone(),
        engine.id,
```

(in `update_starrocks_engine` the call is indented one level deeper; the arguments are the same).

- [ ] **Step 4: Run the tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --lib 2>&1 | tail -5
```

Expected: `test result: ok.`

- [ ] **Step 5: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): stop the StarRocks bootstrap when its engine is gone

The detached catalog/delegation bootstrap polled for up to ten minutes and then
ran DDL as root without checking the engine still existed. It now re-reads the
engine by id before every poll and write, and stops if the row is gone, belongs
to another name, or is being deleted.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 15: Engine delete removes the new secret rows (extends Part A)

**Depends on Part A.** Part A's `delete_engine` deletes the engine's secret rows inside its delete transaction (spec §3.2 step 6). The two columns added in Task 6 must join that list, and the list must not include the admin-provided `tls_cert_secret_ref`.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — Part A's `delete_engine` (secret id collection)
- Test: `tests/routes.rs` (append)

**Interfaces:**
- Consumes: `engine_owned_secret_ids(engine: &Engine) -> Vec<Uuid>` (Task 12); `secret_store`, `engine_secret_ids`, `create_engine_via_api` (Tasks 1, 6).

- [ ] **Step 1: Write the failing test**

Append to `routes.rs`:

```rust
/// Deleting an engine removes every secret egret minted for it, including the
/// internal/monitor secrets.
#[tokio::test]
async fn delete_engine_removes_internal_and_monitor_secrets() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_secrets_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f, &token, &name, wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let (internal_id, monitor_id) = (internal_id.unwrap(), monitor_id.unwrap());

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let secrets = secret_store(&f);
    for sid in [internal_id, monitor_id] {
        assert!(matches!(
            secrets.resolve_plaintext(None, sid).await,
            Err(egret_secrets::SecretsError::NotFound)
        ), "secret {sid} must be deleted with the engine");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
rtk proxy cargo test -p egret-engine-mgmt --test routes delete_engine_removes_internal_and_monitor_secrets 2>&1 | tail -10
```

Expected: FAIL — `secret … must be deleted with the engine` (Part A deletes only the ids it knew about). If instead it fails on the `204` assertion, read the response body: Part A's delete may need an extra fixture (for example a configured `kube`); satisfy it in the test setup, not by weakening the assertion.

- [ ] **Step 3: Use the shared ownership list in Part A's delete**

Locate the collection:

```bash
rtk proxy grep -n "delegator_secret_id\|tls_secret_id\|root_secret_id\|tls_cert_secret_ref" crates/egret-engine-mgmt/src/engines.rs
```

Inside `delete_engine`, replace the expression that gathers the engine's secret ids (an array/`Vec` built from some of `engine.delegator_secret_id`, `engine.tls_secret_id`, `engine.root_secret_id`, and possibly `engine.tls_cert_secret_ref`) with:

```rust
engine_owned_secret_ids(&engine)
```

keeping whatever Part A does with the resulting ids (delete inside its transaction, or via `st.secrets.delete(None, id)`). If Part A's list contained `tls_cert_secret_ref`, dropping it is intentional: that id references an admin-provided, possibly shared certificate (see the verification table at the top of this plan). If Part A's delete has a test asserting the `tls_cert_secret_ref` secret is gone, change that assertion to require that it **survives**.

- [ ] **Step 4: Run the test and Part A's delete tests**

```bash
rtk proxy cargo test -p egret-engine-mgmt --test routes delete_engine 2>&1 | tail -5
```

Expected: every `delete_engine*` test passes, > 0.3 s.

- [ ] **Step 5: Commit**

```bash
git branch --show-current
git add crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "$(cat <<'EOF'
fix(engine-mgmt): delete internal and monitor secrets with the engine

Engine delete now removes every secret egret minted for the engine through the
same ownership list create compensation uses, and never the admin-provided
coordinator TLS secret.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 16: Full verification and PR

**Files:** none new.

- [ ] **Step 1: Sanity greps**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
rtk proxy grep -rn "egret-internal-" crates --include='*.rs'     # expect: only the negative assertion in routes.rs
rtk proxy grep -rn "monitor_password(" crates --include='*.rs'  # expect: no hits (only legacy_monitor_password / monitor_password_for)
rtk proxy grep -rn "prov.update(&req)\|prov.provision(&req)" crates/egret-engine-mgmt/src/engines.rs   # expect: only the StarRocks create/update paths
```

- [ ] **Step 2: Rust gates**

```bash
rtk proxy cargo fmt --all -- --check
rtk proxy cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: no diff; `Finished` with no warnings.

- [ ] **Step 3: Whole workspace test (once)**

Make sure no other workspace test run is in progress, then:

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test; unset EGRET_ALLOW_DB_SKIP
env -u EGRET_ALLOW_DB_SKIP rtk proxy cargo test --workspace 2>&1 | tee /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/c123885c-9b07-4560-a8da-198a38bd2641/scratchpad/planB-workspace-test.log | grep -E "^test result|FAILED|panicked" | sort | uniq -c | tail -20
```

Expected: every `test result: ok`; `egret-engine-mgmt`'s `routes` binary reports `finished in` of many seconds. A failure outside the files this plan touched is **not** assumed pre-existing: check it on `origin/main` before labelling it.

- [ ] **Step 4: Frontend gates**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
npm run lint 2>&1 | tail -3
npm run typecheck 2>&1 | tail -3
npm test 2>&1 | tail -5
npm run build 2>&1 | tail -3
```

Expected: all clean.

- [ ] **Step 5: OpenAPI drift**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json && git diff --exit-code openapi.json && echo "openapi current"
```

Expected: `openapi current`.

- [ ] **Step 6: Push and open the PR, then stop**

```bash
git branch --show-current            # fix/engine-provisioning-hardening
git status --short                   # expect clean
git push -u origin fix/engine-provisioning-hardening
git ls-remote origin refs/heads/fix/engine-provisioning-hardening   # expect a sha equal to `git rev-parse HEAD`
gh pr create --base main --title "fix(engine-mgmt): engine provisioning hardening (SP1 Part B)" --body "$(cat <<'EOF'
SP1 Part B — `docs/en/specs/2026-09-15-engine-lifecycle-hardening-design.md` §4.

- Random stored Trino internal-communication secret and monitor password, applied through one pipeline that records them only after Kubernetes accepted them; `POST /api/control-plane/engines/{id}/rotate-secrets` plus a Rotate secrets button (P1).
- Same-name re-create reuses an unbound orphan profile or fails 409; a Trino engine is bound before its only provision (D9).
- Failed create removes cluster objects, owned secret rows, profiles it created, row and client (P2).
- Engine update refuses before persisting without Kubernetes, restores the spec on failure (502), keeps a stored `delegationMode`; Trino rematerializes (P3, P4). A failed StarRocks update no longer deletes the cluster.
- `remove_catalog` applies before deleting/revoking (P5).
- `engines.status` CHECK; `running` recorded on first all-ready read (P6).
- StarRocks bootstrap re-checks its engine (P7).
- Catalog redaction for every kind, with masked values restored on save (P8).
- Catalog `kind` CHECK dropped (C1).

Spec deviations are listed in the plan: `docs/en/plans/2026-09-15-engine-lifecycle-part-b-provisioning.md`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Do not merge. Report the PR URL and wait for review.

---

## Self-Review

**Spec coverage (§4 / §5 Part B):**

| Spec item | Task |
|---|---|
| 4.1 columns, 32-byte secrets, create-on-provision, next-apply adoption | 6, 7 |
| 4.1 rotate route + UI + "pods will restart" | 8, 9 |
| 4.1 secrets deleted with the engine | 15 (+ 12 for failed creates) |
| 4.2 orphan reuse / 409 / never unbound under Ranger | 13 |
| 4.3 compensation (cluster, TLS/delegator/internal/monitor secrets, policies, row, client; each step logs) | 12 (policies via created-profile deletion only — see table) |
| 4.4 snapshot/restore/502; missing `delegationMode` preserved | 10 (+ 5 so the StarRocks restore is meaningful) |
| 4.5 Trino rematerialize | 10 |
| 4.6 `remove_catalog` order | 11 |
| 4.7 status set + CHECK + normalising migration; `provisioning`/`failed`/`running`; UI live-else-stored | 4 (+ 12 writes `failed`); UI already compliant, verified |
| 4.8 bootstrap re-read by id and name | 14 |
| 4.9 redaction for every kind | 3 |
| 4.10 drop `kind` CHECK | 2 |
| §5 re-create binds orphan profile | 13 |
| §5 rotate changes env secret and password.db | 8 (verified by `bcrypt::verify`, not hash inequality) |
| §5 update failure restores the spec | 10 |
| §5 `remove_catalog` failure keeps row and client | 11 |
| §5 redaction for a `postgresql` inline password | 3 |
| §5 `mysql` catalog after the migration | 2 |

**Placeholder scan:** no TBD/TODO; every code step carries the code. Six steps are conditional on Part A's final shape (Task 1 Step 1 `FakeEnginePort` fields, Task 4 Step 3 Part A's `ENGINE_STATUS_DELETING`, Task 8 Step 3 Part A's `into_response` shape, Task 12 Step 1 `delete_service`, Task 13 Step 3 Part A's bound-engine count, Task 15 Step 3 locating the id list); each names the exact grep that decides the branch and the exact code for both outcomes.

**Type consistency:** `BuildOpts`/`BuiltReq`/`ApplyOp`/`apply_built(st, prov: &dyn EngineProvisioningPort, engine, built: &BuiltReq, op)` are defined in Task 7 and used unchanged in 8, 10, 11, 12, 13. `EngineInternalSecrets` fields `internal_id/internal/monitor_id/monitor/fresh` are used identically in Tasks 7, 12. `CreateUndo` fields `client_id/engine_id/cluster/created_profile_id/created_tag_service_id` match between Tasks 12 and 13. `engine_owned_secret_ids` (Task 12) is the list Task 15 reuses. `ClusterApplyFailed(String)` (Task 8) is what Task 10 returns and its unit test matches. `engine_status::{PROVISIONING,RUNNING,FAILED,DELETING}` (Task 4) are used in 4, 12, 14. Migration numbers 0144/0145/0146 are each referenced by exactly one task.
