# SP1 Part A — Engine delete and unassign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deleting an engine (and unassigning it from a workspace) either succeeds cleanly or refuses with a 409 that lists what blocks it — never a silent 500 after the cluster is already gone.

**Architecture:** A new `egret-engine-mgmt::dependents` module computes, in one SQL statement, the user-authored assets (Data API endpoints live and trashed, schedules, datasets) that sit on an engine's materialized `query.databases` rows. `DELETE /engines/{id}` preflights them (409), requires Kubernetes (400), marks the row `deleting`, deletes the cluster (404-tolerant; any other failure is a 502 and a retryable `deleting` row), then deletes the row in a transaction that locks the engine and its connections and re-checks dependents. Cross-seam cleanup (minted catalog clients, engine-generated secrets, the engine's own Ranger profile) runs after commit and only logs failures. Unassign and `PUT /engines/{id}/workspaces` run the same preflight restricted to the affected workspaces, and validate the whole change before writing any of it. A backstop in `EngineHttpError` logs every `sqlx::Error` and maps SQLSTATE 23503/23514 to a 409 naming the constraint. The UI shows the dependents table on 409 and a retry state for `deleting`.

**Tech Stack:** Rust (axum, sqlx 0.9 runtime queries — `query()` takes `impl SqlSafeStr`, so SQL must be `&'static str`), kube 2.0.1, utoipa 5.5, React + TanStack Query + orval-generated client, vitest.

**Spec:** `docs/en/specs/2026-09-15-engine-lifecycle-hardening-design.md` §3 and the Part A bullets of §5 (it lives on branch `docs/control-plane-dg-review`; this plan was written against that branch, whose `crates/`, `frontend/`, `migrations/` and `openapi.json` are identical to `main` at `48f91c03`).

**A warning about this document.** Every snippet was written after reading the file it targets, but a plan is still a guess about code. **Read the file before editing it; where this plan and the code disagree, the plan is wrong — stop and report.**

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Branch:** create `fix/engine-delete-preflight` off `main`. Verify with `git branch --show-current` before the first commit — a detached HEAD passes every other check.
- **No migration in Part A.** Do **not** add the `engines.status` CHECK (that is Part B §4.7); Part A only *writes* the value `'deleting'`. Do **not** edit the stale comment in `migrations/0066_query_schema.sql:13-17` — editing an applied migration changes its checksum and every DB-gated binary fails with `VersionMismatch`.
- **DB tests:** `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` (same user/password/host as `.env`, database swapped to `egret_test`). **Never** the dev database `egret`. Each test process clones `egret_test_template`; if a run fails with `VersionMissing`, drop the template (`psql postgres://egret:egret@localhost:5432/postgres -c 'DROP DATABASE egret_test_template'`) and rerun. Migrations for the *app* run via `EGRET_DATABASE__URL`, not `DATABASE_URL`.
- **A skipped DB test reports `ok`.** Without `DATABASE_URL`, `egret_test_db::url()` panics unless `EGRET_ALLOW_DB_SKIP` is set; with it set, tests print `SKIPPED`/`skip:` and assert nothing. Every "Expected: PASS" below therefore also means: the binary ran for more than a couple of seconds and printed no `skip`/`SKIPPED` line. Run with `env -u EGRET_ALLOW_DB_SKIP` to make a missing database fail loudly.
- **One test suite at a time.** Never run two `cargo test` invocations concurrently (they share the Postgres server and the template).
- **`rtk`:** shell commands are rewritten by the RTK hook. Anything whose raw stdout matters (a redirect into a file, a pipe into `grep`) must be prefixed `rtk proxy`.
- **Rust gate per task:** `cargo fmt --all -- --check` and `cargo clippy -p <crate> --all-targets -- -D warnings` for each crate touched. Whole-workspace gate only in Task 11 (`just check`).
- **Frontend gate per task** (in `frontend/`): `npm run lint`, `npm run typecheck`, `npm test`, `npm run build`. No prettier.
- **Regenerate the client with `npm run gen:api`, never bare `orval`** (bare orval drops `encodeURIComponent`). Keep only the generated files this change needs; revert unrelated churn.
- **`openapi.json` is byte-asserted** by `crates/egret-server/src/lib.rs::committed_openapi_json_is_current`. Any utoipa change needs `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json` in the same commit.
- **Error body convention:** every error body keeps the `"error"` key (`ApiError.from` reads it, `frontend/src/api/client.ts:24`). New conflict/gateway bodies add a machine-readable `"code"`.
- **Commits:** Conventional Commits, `git commit`, message ending with the line `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Never commit a scratchpad or `.superpowers/`. No PR, no merge — stop after Task 11 and report.

## Decisions this plan takes where the spec is wrong or silent

Each is load-bearing for a task below; the reviewer can reject one without the others.

1. **A purge route already exists (spec §2.1/§3.3 is wrong).** `DELETE /api/ws/{ws}/data-api/endpoints/{id}` (`crates/egret-dataapi/src/routes.rs:1112` → `EndpointStore::delete`, `endpoints.rs:1212`) hard-deletes an endpoint (trashed or not) in one transaction with its `access_grants`, audits `data_api_endpoint:delete`, and the trash view already offers **Delete permanently** (`frontend/src/routes/explore/dataapi/Endpoints.tsx:753-771`). `tests/trash.rs:852` already drives it on a trashed endpoint. **No `/permanent` route and no `:purge` audit action are added.** Task 8 adds the missing cascade proof (versions chained by `compared_against`, a `dataapi_alert` schedule, `access_log` survives). The existing route does *not* restrict itself to trashed endpoints and its confirm asks for the word `delete`, not the name — both left unchanged (out of scope, flagged).
2. **"One DB transaction" for row + secrets + Ranger profile is not achievable.** `WorkspaceSecretStore` and `PolicyPort` (a trait object; "at a real split an HTTP client fills this in") own their own pools. The transaction covers: lock engine row → lock its `query.databases` rows → re-check dependents → delete the engine row. Minted-client revoke, secret deletion and Ranger profile deletion run after commit and log failures.
3. **`PUT /engines/{id}/workspaces` is validate-then-apply, not one transaction.** `EngineAssignmentService::assign` materializes through `DatabaseRepo` on its own pool. Every refusable condition (unknown workspace → 404, engine not delegation-capable → 400, dependents on any unassigned workspace → 409) is checked before the first write; only a transport failure mid-apply can leave a partial change.
4. **Secrets deleted with the engine:** `delegator_secret_id`, `root_secret_id`, `tls_secret_id` only. `tls_cert_secret_ref` (spec D9 lists it) is an admin-provided `kubernetes.io/tls` secret, validated in `create_engine` and possibly shared — it is not engine-owned and is **not** deleted.
5. **"Auto-created Ranger profile" is not recorded anywhere**, and a profile can hold admin-written policies. **User decision (2026-09-15): engine delete never deletes a profile or tag service — it only unbinds** (the row delete does that). Part B reuses an orphan profile on re-create. See Task 5.
6. **Schedules:** only `type='alert'` rows can violate `schedule_alert_needs_query`, but every schedule with a `database_id` is counted (kind `schedule`), trashed ones included. Trashed datasets are counted too (they cascade to charts/cards/report schedules).
7. **"`From<CpError>`" does not exist separately**: `CpError` is an alias of `EngineHttpError` (`engines.rs:39`). The backstop is one `db_error()` used by `From<EngineMgmtError>`, `From<SecretsError>`, `From<egret_storage::StorageRegistryError>`.
8. **§5 "FakeKube: delete returning 404"** cannot be a route test: route tests go through `EngineProvisioningPort` (`FakeEnginePort`), and 404 handling lives in `LiveKube::delete`, which needs a cluster. Task 1 unit-tests a pure `is_not_found(&kube::Error)` and adds failure injection to `FakeKube`; Task 4 adds failure injection to `FakeEnginePort` for 502/retry.
9. **The retry badge needs the stored status**, but `GET /engines/{id}` flattens the *live* status over it. When the stored status is `deleting`, `get_engine` returns phase `deleting` without calling the provisioner (Part B §4.7 generalizes this).
10. **409 body:** `{"error", "code":"engine_has_dependents", "dependents":[{workspaceId, workspaceName, workspaceSlug, kind, count, sample}]}`. `workspaceSlug` is added (UI routes are `/explore/{slug}/…`); `error` is kept (see Global Constraints).
11. **404s:** assign (`POST /api/admin/workspaces/{id}/engines`) and `PUT` with an unknown workspace are 500 today (FK `engine_workspaces_workspace_id_fkey`); unassign from an unknown workspace is 204 today. All three become 404.

Out of scope, noticed while planning: `DatabaseRepo::delete` (`crates/egret-query/src/registry/repo.rs:400`, user-registered connections) hits the same RESTRICT/CHECK 500; the admin **WorkspaceDetail → Assigned engines → Remove** (`frontend/src/routes/control-plane/WorkspaceDetail.tsx:449`) only toasts the new 409 message.

## Live FK children (from migrations; Task 2 pins them against `pg_constraint`)

Children of `query.databases` (table created `0008`, moved to `query` in `0066`; child tables moved in `0066`/`0067`; `workspace_database_grant` dropped in `0031`):

| child table | column | FK action (`confdeltype`) | NOT NULL | source | preflight |
|---|---|---|---|---|---|
| `query.queries` | `database_id` | CASCADE (`c`) | yes | `0009:4` | cascades |
| `analytics.saved_queries` | `database_id` | SET NULL (`n`) | no | `0010:4` | sets null (no CHECK on the column) |
| `analytics.datasets` | `database_id` | CASCADE (`c`) | yes | `0012:4` | **blocks** (`dataset`) |
| `analytics.notebook_engine_access` | `database_id` | CASCADE (`c`) | yes (PK) | `0039:5` | cascades |
| `automation.schedules` | `database_id` | SET NULL (`n`) | no | `0110:24` | **blocks** (`schedule`) — `schedule_alert_needs_query` (`0110:71`) rejects the NULL |
| `dataapi.endpoints` | `database_id` | RESTRICT (`r`) | yes | `0121:30` | **blocks** (`dataapi_endpoint`, `dataapi_endpoint_trashed` by `deleted_at`, `0126`) |

Children of `engine_mgmt.engines`: `engine_mgmt.engine_catalogs.engine_id` (`0057:8`, c, NOT NULL), `engine_mgmt.engine_workspaces.engine_id` (`0084:5`, c, NOT NULL), `query.databases.engine_id` (`0084:13`, c, nullable).

Endpoint children (Task 8): `dataapi.endpoint_versions.endpoint_id` CASCADE (`0121:50`); `dataapi.endpoints.live_version_id` SET NULL (`0121:65`); `dataapi.endpoint_versions.compared_against` **no action** (`0131:65`) — safe only because the referencing versions are deleted in the same statement; `automation.schedules.dataapi_endpoint_id` CASCADE (`0128:14`); `dataapi.access_log.endpoint_id` **no FK by design** (`0123:9-26`) — rows survive; `access_grants` swept explicitly by `EndpointStore::delete`.

Preflight SQL (defined once in `dependents.rs`, Task 2):

```sql
WITH dbs AS (
  SELECT d.id, d.workspace_id FROM query.databases d
   WHERE d.engine_id = $1 AND ($2::uuid[] IS NULL OR d.workspace_id = ANY($2))
), items AS (
  SELECT dbs.workspace_id,
         CASE WHEN e.deleted_at IS NULL THEN 'dataapi_endpoint' ELSE 'dataapi_endpoint_trashed' END AS kind,
         e.name
    FROM dataapi.endpoints e JOIN dbs ON dbs.id = e.database_id
  UNION ALL
  SELECT dbs.workspace_id, 'schedule', s.name
    FROM automation.schedules s JOIN dbs ON dbs.id = s.database_id
  UNION ALL
  SELECT dbs.workspace_id, 'dataset', ds.name
    FROM analytics.datasets ds JOIN dbs ON dbs.id = ds.database_id
)
SELECT i.workspace_id, w.name AS workspace_name, w.slug AS workspace_slug, i.kind,
       count(*) AS count, (array_agg(i.name ORDER BY i.name))[1:5] AS sample
  FROM items i JOIN workspaces w ON w.id = i.workspace_id
 GROUP BY i.workspace_id, w.name, w.slug, i.kind
 ORDER BY w.name, i.workspace_id, i.kind
```

## File Structure

```text
crates/egret-provisioner/src/
  kube_client.rs        KubeClient::delete doc; LiveKube::delete 404 → Ok; is_not_found();
                        delete_objects(); FakeKube::delete_failures + fail_delete()
  trino.rs              delete() → delete_objects (propagates non-404 errors)
  starrocks.rs          delete() → delete_objects (CR failure no longer aborts the rest)
crates/egret-engine-mgmt/
  Cargo.toml            dev-dep tracing-subscriber
  src/lib.rs            pub mod dependents
  src/dependents.rs     NEW — DTOs, preflight SQL, FK classification, store methods
  src/store.rs          pool() pub(crate); ENGINE_STATUS_DELETING; missing_workspace_ids()
  src/http_error.rs     HasDependents / ConstraintViolation / ClusterDeleteFailed; db_error()
  src/engines.rs        delete_engine flow + cleanup; get_engine deleting; assignment
                        preflight; utoipa responses + schemas
  tests/dependents.rs   NEW — pg_constraint guard, preflight grouping, locked delete
  tests/error_mapping.rs NEW — one-test binary: SQLSTATE → 409 + log line
  tests/routes.rs       FakeEnginePort delete recording/injection; delete + assignment tests
crates/egret-dataapi/tests/trash.rs   purge cascade test
openapi.json                          regenerated
frontend/src/api/generated/…          regenerated (control-plane-engines + 3 models + model index)
frontend/src/routes/control-plane/
  EngineDependentsTable.tsx           NEW — table + dependentsFromError()
  EngineDetail.tsx                    delete dialog, blocked dialog, retry, Workspaces 409
  __tests__/EngineDependentsTable.test.tsx NEW
  __tests__/EngineDetail.test.tsx     delete + workspaces-409 tests
```

---
### Task 1: Kubernetes delete is 404-tolerant and never silently fails (D5, D6)

**Files:**
- Modify: `crates/egret-provisioner/src/kube_client.rs` (trait doc ~36-42; `FakeKube` struct/Default/impl ~120-180; `LiveKube::delete` ~318-331; tests module)
- Modify: `crates/egret-provisioner/src/trino.rs:1275-1356` (`TrinoProvisioner::delete`) + tests module
- Modify: `crates/egret-provisioner/src/starrocks.rs:941-999` (`StarRocksProvisioner::delete`) + tests module

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub fn egret_provisioner::kube_client::is_not_found(err: &kube::Error) -> bool`
  - `pub async fn egret_provisioner::kube_client::delete_objects(kube: &dyn KubeClient, namespace: &str, objects: &[(&str, &str, String)]) -> anyhow::Result<()>` — attempts all, error text `failed to delete Kind/name: cause; Kind/name: cause`
  - `FakeKube::delete_failures: Mutex<BTreeMap<String, String>>` and `FakeKube::fail_delete(&self, kind: &str, name: &str, message: &str)`; a failed delete is **not** pushed to `FakeKube::deleted`.
  - Contract: `KubeClient::delete` returns `Ok` for HTTP 404; `EngineProvisioner::delete` returns `Err` if any object failed with anything else (the port maps it to `String` in `crates/egret-bin/src/main.rs:659-664`, unchanged).

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` in `kube_client.rs`:

```rust
    fn api_error(code: u16, reason: &str) -> kube::Error {
        kube::Error::Api(kube::core::ErrorResponse {
            status: "Failure".into(),
            message: format!("{reason} (test)"),
            reason: reason.into(),
            code,
        })
    }

    /// A missing object and a resource type the cluster does not serve both come
    /// back as 404; nothing else may be read as "already deleted".
    #[test]
    fn only_a_404_api_error_is_not_found() {
        assert!(is_not_found(&api_error(404, "NotFound")));
        assert!(!is_not_found(&api_error(403, "Forbidden")));
        assert!(!is_not_found(&api_error(500, "InternalError")));
    }

    #[tokio::test]
    async fn delete_objects_attempts_every_object_and_names_each_failure() {
        let k = FakeKube::default();
        k.fail_delete("Deployment", "demo-coordinator", "forbidden by RBAC");
        k.fail_delete("Service", "demo-trino", "connection refused");
        let objects = [
            ("apps/v1", "Deployment", "demo-coordinator".to_string()),
            ("apps/v1", "Deployment", "demo-worker".to_string()),
            ("v1", "Service", "demo-trino".to_string()),
            ("v1", "ConfigMap", "demo-config".to_string()),
        ];
        let err = delete_objects(&k, "egret-engines", &objects)
            .await
            .expect_err("two deletes failed");
        let msg = format!("{err:#}");
        assert!(msg.contains("Deployment/demo-coordinator: forbidden by RBAC"), "{msg}");
        assert!(msg.contains("Service/demo-trino: connection refused"), "{msg}");
        assert_eq!(
            *k.deleted.lock().unwrap(),
            vec!["Deployment/demo-worker".to_string(), "ConfigMap/demo-config".to_string()],
            "the objects after a failure are still attempted"
        );
    }
```

Append to the `mod tests` in `trino.rs` (next to `delete_removes_all_objects`):

```rust
    /// D6: every `let _ =` used to swallow a 403/timeout, so the row was deleted
    /// and the cluster orphaned. A non-404 failure must now surface — after the
    /// other objects have still been deleted.
    #[tokio::test]
    async fn delete_reports_a_failed_object_after_attempting_the_rest() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        kube.fail_delete("Deployment", "demo-coordinator", "forbidden");
        let p = TrinoProvisioner::new(kube.clone());
        let err = p
            .delete("egret-engines", "demo")
            .await
            .expect_err("a non-404 failure must surface");
        assert!(format!("{err:#}").contains("Deployment/demo-coordinator: forbidden"));
        assert_eq!(kube.deleted.lock().unwrap().len(), 11, "the other 11 objects are deleted");
    }
```

Append to the `mod tests` in `starrocks.rs` (next to `provisioner_delete_cleans_up_initpwd`):

```rust
    /// D5/D6: a failed CR delete surfaces, and no longer aborts the cleanup of the
    /// ConfigMaps/Secrets/Job/Ingress egret owns.
    #[tokio::test]
    async fn provisioner_delete_surfaces_a_failed_cluster_delete() {
        use crate::kube_client::FakeKube;

        let kube = Arc::new(FakeKube::default());
        kube.fail_delete("StarRocksCluster", "sr1", "forbidden");
        let err = StarRocksProvisioner::new(kube.clone())
            .delete("egret-engines", "sr1")
            .await
            .expect_err("the CR delete failure must surface");
        assert!(format!("{err:#}").contains("StarRocksCluster/sr1: forbidden"));
        let deleted = kube.deleted.lock().unwrap();
        assert_eq!(deleted.len(), 10, "{deleted:?}");
        assert!(deleted.contains(&"ConfigMap/sr1-fe-cm".to_string()), "{deleted:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p egret-provisioner`
Expected: compile errors — `cannot find function is_not_found`, `delete_objects`, `no method named fail_delete`.

- [ ] **Step 3: Implement**

In `kube_client.rs`, replace the doc above `async fn delete(` in the trait with:

```rust
    /// Delete one object. An object that is already gone (HTTP 404) is `Ok`:
    /// deletes are retried after partial failures, and "already deleted" is the
    /// outcome the caller asked for. Every other failure is an `Err`.
```

Add to `FakeKube` (struct field, `Default`, and an inherent method beside `set_coordinator_response`):

```rust
    /// `"kind/name"` → error message. A `delete` of a listed object fails with
    /// that message and is NOT recorded in `deleted`. Set with [`FakeKube::fail_delete`].
    pub delete_failures: Mutex<BTreeMap<String, String>>,
```

```rust
            delete_failures: Mutex::new(BTreeMap::new()),
```

```rust
    /// Make every later `delete` of `kind/name` fail with `message`.
    pub fn fail_delete(&self, kind: &str, name: &str, message: &str) {
        self.delete_failures
            .lock()
            .unwrap()
            .insert(format!("{kind}/{name}"), message.to_string());
    }
```

Replace `FakeKube`'s `delete`:

```rust
    async fn delete(&self, _av: &str, kind: &str, _ns: &str, name: &str) -> anyhow::Result<()> {
        let key = format!("{kind}/{name}");
        if let Some(message) = self.delete_failures.lock().unwrap().get(&key) {
            anyhow::bail!("{message}");
        }
        self.deleted.lock().unwrap().push(key);
        Ok(())
    }
```

Add two free functions after `impl KubeClient for FakeKube { … }`:

```rust
/// True for the API server's "not found" answer. A missing object and a resource
/// type the cluster does not serve (e.g. KEDA's `ScaledObject` with no KEDA
/// installed) both come back as 404, and both mean there is nothing to delete.
pub fn is_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 404)
}

/// Delete every `(apiVersion, kind, name)` in `namespace`. All of them are
/// attempted even after a failure, so a retry has as little left as possible;
/// the error names every object that failed and why.
pub async fn delete_objects(
    kube: &dyn KubeClient,
    namespace: &str,
    objects: &[(&str, &str, String)],
) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for (api_version, kind, name) in objects {
        if let Err(e) = kube.delete(api_version, kind, namespace, name).await {
            failures.push(format!("{kind}/{name}: {e:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("failed to delete {}", failures.join("; "))
    }
}
```

In `LiveKube::delete`, replace the last two lines (`api.delete(name, &DeleteParams::default()).await?; Ok(())`) with:

```rust
        match api.delete(name, &DeleteParams::default()).await {
            Ok(_) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
```

Replace the whole body of `TrinoProvisioner::delete` (`trino.rs:1275-1356`) with (same 12 objects, same order as today):

```rust
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        // Every object egret applies for a Trino engine, including the ones only
        // some engines have (TLS/ingress/pull secrets, Ingress, both autoscalers):
        // an object that was never applied is a 404, which `KubeClient::delete`
        // treats as success. Anything else is an error the caller must see.
        let objects = [
            ("apps/v1", "Deployment", format!("{name}-coordinator")),
            ("apps/v1", "Deployment", format!("{name}-worker")),
            ("v1", "Service", format!("{name}-trino")),
            ("v1", "ConfigMap", format!("{name}-config")),
            ("v1", "ConfigMap", format!("{name}-catalogs")),
            ("v1", "Secret", format!("{name}-env")),
            ("v1", "Secret", format!("{name}-tls")),
            ("v1", "Secret", format!("{name}-ingress-tls")),
            ("v1", "Secret", format!("{name}-pull")),
            ("networking.k8s.io/v1", "Ingress", format!("{name}-trino")),
            ("autoscaling/v2", "HorizontalPodAutoscaler", format!("{name}-worker")),
            ("keda.sh/v1alpha1", "ScaledObject", format!("{name}-worker")),
        ];
        crate::kube_client::delete_objects(&*self.kube, namespace, &objects).await
    }
```

Replace the whole body of `StarRocksProvisioner::delete` (`starrocks.rs:941-999`) with (same 11 objects, CR first):

```rust
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        // The CR first (the operator tears down FE/CN), then the objects egret
        // applies beside it. A 404 is success; any other failure is returned,
        // after the remaining objects have still been attempted.
        let objects = [
            ("starrocks.com/v1", "StarRocksCluster", name.to_string()),
            ("v1", "ConfigMap", format!("{name}-fe-cm")),
            ("v1", "ConfigMap", format!("{name}-cn-cm")),
            ("v1", "ConfigMap", format!("{name}-ranger-cm")),
            ("v1", "ConfigMap", format!("{name}-ssl-cm")),
            ("v1", "Secret", format!("{name}-ssl-secret")),
            ("batch/v1", "Job", format!("{name}-initpwd")),
            ("v1", "ConfigMap", format!("{name}-initpwd-shell")),
            ("v1", "Secret", format!("{name}-initpwd-secret")),
            ("networking.k8s.io/v1", "Ingress", format!("{name}-fe")),
            ("v1", "Secret", format!("{name}-ingress-tls")),
        ];
        crate::kube_client::delete_objects(&*self.kube, namespace, &objects).await
    }
```

Before replacing, diff the object lists against the current bodies: they must name exactly the same objects. If the current code has an object this list lacks, add it (and adjust the counts in the Step 1 tests).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p egret-provisioner`
Expected: PASS, including the pre-existing `delete_removes_all_objects` (12), `provision_applies_ingress_and_delete_removes_it`, `provision_applies_autoscaler_and_delete_removes_both`, `provisioner_delete_cleans_up_initpwd`.

Run: `cargo fmt --all -- --check && cargo clippy -p egret-provisioner --all-targets -- -D warnings && cargo check -p egret-bin`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-provisioner/src/kube_client.rs crates/egret-provisioner/src/trino.rs crates/egret-provisioner/src/starrocks.rs
git commit -m "fix(provisioner): make engine delete 404-tolerant and surface other failures

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 2: Dependents preflight, locked row delete, and the FK-tree guard

**Files:**
- Create: `crates/egret-engine-mgmt/src/dependents.rs`
- Modify: `crates/egret-engine-mgmt/src/lib.rs` (add `pub mod dependents;` after `pub mod delegator_policy;`)
- Modify: `crates/egret-engine-mgmt/src/store.rs:173-175` (`fn pool` → `pub(crate) fn pool`)
- Create: `crates/egret-engine-mgmt/tests/dependents.rs`

**Interfaces:**
- Consumes: `EngineMgmtStore` (`store.rs`), `EngineMgmtError::Db` (`error.rs`).
- Produces (all in `egret_engine_mgmt::dependents`):
  - `pub enum EngineDependentKind { DataapiEndpoint, DataapiEndpointTrashed, Schedule, Dataset }` — `Serialize` snake_case (`"dataapi_endpoint"`, `"dataapi_endpoint_trashed"`, `"schedule"`, `"dataset"`), `ToSchema`, `Copy`, `Eq`; `pub fn as_str(self) -> &'static str`
  - `pub struct EngineDependentDto { pub workspace_id: Uuid, pub workspace_name: String, pub workspace_slug: String, pub kind: EngineDependentKind, pub count: i64, pub sample: Vec<String> }` — camelCase, `ToSchema`
  - `pub struct EngineConflictBody { pub error: String, pub code: String, pub dependents: Vec<EngineDependentDto>, pub constraint: Option<String> }` — `dependents` skipped when empty, `constraint` skipped when `None`
  - `pub enum DeleteEngineOutcome { Deleted, NotFound, Blocked(Vec<EngineDependentDto>) }`
  - `pub enum ChildOnDelete { Blocks(EngineDependentKind), Cascades, SetsNull }`, `pub struct DatabaseChildFk { table, column, on_delete: &'static str, not_null: bool, handling }`, `pub const QUERY_DATABASES_CHILD_FKS: &[DatabaseChildFk]`
  - `impl EngineMgmtStore { pub async fn engine_dependents(&self, engine_id: Uuid, workspace_ids: Option<&[Uuid]>) -> Result<Vec<EngineDependentDto>, EngineMgmtError>; pub async fn delete_engine_unless_blocked(&self, id: Uuid) -> Result<DeleteEngineOutcome, EngineMgmtError> }`

- [ ] **Step 1: Write the failing DB tests**

Create `crates/egret-engine-mgmt/tests/dependents.rs`:

```rust
//! The engine-delete preflight (`egret_engine_mgmt::dependents`) against a real
//! schema: the FK tree it classifies, what it counts, and the locked row delete.
//!
//! DB-gated: without `DATABASE_URL` this FAILS unless `EGRET_ALLOW_DB_SKIP` is
//! set, in which case every test prints SKIPPED and asserts nothing — check the
//! duration, not only the `ok`.

use egret_engine_mgmt::dependents::{
    ChildOnDelete, DeleteEngineOutcome, EngineDependentKind, QUERY_DATABASES_CHILD_FKS,
};
use egret_engine_mgmt::EngineMgmtStore;
use egret_metastore::Db;
use std::collections::BTreeSet;
use uuid::Uuid;

/// Every single-column FK pointing at `$1`, as (child table, column, confdeltype, NOT NULL).
const FK_CHILDREN_SQL: &str = "\
SELECT n.nspname || '.' || cl.relname, a.attname::text, c.confdeltype::text, a.attnotnull \
  FROM pg_constraint c \
  JOIN pg_class cl ON cl.oid = c.conrelid \
  JOIN pg_namespace n ON n.oid = cl.relnamespace \
  JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] \
 WHERE c.contype = 'f' AND c.confrelid = $1::regclass";

/// CHECK constraints on table `$1` that involve column `$2`.
const CHECKS_ON_COLUMN_SQL: &str = "\
SELECT c.conname::text FROM pg_constraint c \
 WHERE c.contype = 'c' AND c.conrelid = $1::regclass \
   AND EXISTS (SELECT 1 FROM pg_attribute a \
                WHERE a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey) AND a.attname = $2) \
 ORDER BY 1";

type FkRow = (String, String, String, bool);

async fn harness() -> Option<(Db, EngineMgmtStore)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = EngineMgmtStore::new(db.clone());
    Some((db, store))
}

macro_rules! harness_or_skip {
    () => {
        match harness().await {
            Some(h) => h,
            None => {
                eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
                return;
            }
        }
    };
}

async fn fk_children(db: &Db, parent: &str) -> BTreeSet<FkRow> {
    sqlx::query_as::<_, FkRow>(FK_CHILDREN_SQL)
        .bind(parent)
        .fetch_all(&db.pool)
        .await
        .expect("read pg_constraint")
        .into_iter()
        .collect()
}

async fn seed_workspace(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("dep-{}", Uuid::new_v4().simple()))
    .bind(name)
    .fetch_one(&db.pool)
    .await
    .expect("seed workspace")
}

async fn seed_engine(db: &Db, store: &EngineMgmtStore) -> Uuid {
    let catalog = egret_catalog_engine::CatalogStore::new(db.clone());
    let owner = seed_workspace(db, "Warehouse owner").await;
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("dep-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection");
    let wh = catalog
        .create_warehouse(&format!("dep-wh-{}", Uuid::new_v4().simple()), owner, sc)
        .await
        .expect("create warehouse");
    store
        .create_engine(
            &format!("dep-eng-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            wh.id,
            &serde_json::json!({}),
            None,
        )
        .await
        .expect("seed engine")
        .id
}

/// A materialized workspace connection for `engine_id`, as `assign` would write it.
async fn seed_connection(db: &Db, ws: Uuid, engine_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, engine_id) \
         VALUES ($1, $2, 'trino', 'coordinator', 8443, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("conn-{}", Uuid::new_v4().simple()))
    .bind(engine_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed connection")
}

async fn seed_endpoint(db: &Db, ws: Uuid, database_id: Uuid, name: &str, trashed: bool) {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints (workspace_id, name, path, method, database_id, statements, state) \
         VALUES ($1, $2, $3, 'GET', $4, \
                 jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), 'draft') \
         RETURNING id",
    )
    .bind(ws)
    .bind(name)
    .bind(format!("/{name}"))
    .bind(database_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed endpoint");
    if trashed {
        sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&db.pool)
            .await
            .expect("trash endpoint");
    }
}

async fn seed_alert(db: &Db, ws: Uuid, database_id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO automation.schedules (workspace_id, type, name, crontab, database_id, sql, validator_type) \
         VALUES ($1, 'alert', $2, '0 9 * * *', $3, 'SELECT 1', 'not_null')",
    )
    .bind(ws)
    .bind(name)
    .bind(database_id)
    .execute(&db.pool)
    .await
    .expect("seed alert");
}

async fn seed_dataset(db: &Db, ws: Uuid, database_id: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
         VALUES ($1, $2, $3, 't')",
    )
    .bind(ws)
    .bind(database_id)
    .bind(name)
    .execute(&db.pool)
    .await
    .expect("seed dataset");
}

/// The guard the spec asks for: a new child of `query.databases` — or a changed
/// action on an existing one — fails here until `QUERY_DATABASES_CHILD_FKS` (and
/// with it the preflight) classifies it. A RESTRICT child the preflight does not
/// count is exactly the silent 500 this plan exists to remove.
#[tokio::test]
async fn every_fk_child_of_query_databases_is_classified_by_the_preflight() {
    let (db, _store) = harness_or_skip!();
    let live = fk_children(&db, "query.databases").await;
    let classified: BTreeSet<FkRow> = QUERY_DATABASES_CHILD_FKS
        .iter()
        .map(|c| (c.table.to_string(), c.column.to_string(), c.on_delete.to_string(), c.not_null))
        .collect();
    assert_eq!(live.len(), 6, "positive control — the query found the known children: {live:?}");
    assert_eq!(
        live, classified,
        "the FK children of query.databases changed; classify the new/changed child in \
         dependents::QUERY_DATABASES_CHILD_FKS and, if it blocks, count it in DEPENDENTS_SQL"
    );
}

/// `SetsNull` is only safe while no CHECK needs the column. `automation.schedules`
/// is the counter-example, and is classified `Blocks` because of it.
#[tokio::test]
async fn set_null_children_have_no_check_that_needs_the_column() {
    let (db, _store) = harness_or_skip!();
    for fk in QUERY_DATABASES_CHILD_FKS {
        let checks: Vec<String> = sqlx::query_scalar(CHECKS_ON_COLUMN_SQL)
            .bind(fk.table)
            .bind(fk.column)
            .fetch_all(&db.pool)
            .await
            .expect("read checks");
        match fk.handling {
            ChildOnDelete::SetsNull => assert!(
                checks.is_empty(),
                "{}.{} is SET NULL but CHECKs {checks:?} involve it — it must block instead",
                fk.table,
                fk.column
            ),
            ChildOnDelete::Blocks(EngineDependentKind::Schedule) => assert!(
                checks.iter().any(|c| c == "schedule_alert_needs_query"),
                "positive control: the CHECK that makes schedules block: {checks:?}"
            ),
            _ => {}
        }
    }
}

/// Deleting the engine row cascades these three; nothing else may reference it.
#[tokio::test]
async fn every_fk_child_of_engines_cascades() {
    let (db, _store) = harness_or_skip!();
    let expected: BTreeSet<FkRow> = [
        ("engine_mgmt.engine_catalogs", "engine_id", "c", true),
        ("engine_mgmt.engine_workspaces", "engine_id", "c", true),
        ("query.databases", "engine_id", "c", false),
    ]
    .into_iter()
    .map(|(t, c, a, n)| (t.to_string(), c.to_string(), a.to_string(), n))
    .collect();
    assert_eq!(fk_children(&db, "engine_mgmt.engines").await, expected);
}

#[tokio::test]
async fn engine_dependents_groups_by_workspace_and_kind_and_caps_the_sample() {
    let (db, store) = harness_or_skip!();
    let engine = seed_engine(&db, &store).await;
    let other_engine = seed_engine(&db, &store).await;
    let ws_a = seed_workspace(&db, "Alpha").await;
    let ws_b = seed_workspace(&db, "Beta").await;
    let conn_a = seed_connection(&db, ws_a, engine).await;
    let conn_b = seed_connection(&db, ws_b, engine).await;
    // Same workspace, ANOTHER engine's connection: must not count.
    let foreign = seed_connection(&db, ws_a, other_engine).await;

    for n in ["g", "f", "e", "d", "c", "b", "a"] {
        seed_endpoint(&db, ws_a, conn_a, n, false).await;
    }
    seed_endpoint(&db, ws_a, conn_a, "binned", true).await;
    seed_endpoint(&db, ws_a, foreign, "not-mine", false).await;
    seed_alert(&db, ws_b, conn_b, "nightly").await;
    seed_dataset(&db, ws_b, conn_b, "orders").await;

    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let summary = |deps: Vec<egret_engine_mgmt::dependents::EngineDependentDto>| {
        deps.into_iter()
            .map(|d| (d.workspace_id, d.kind, d.count, d.sample))
            .collect::<Vec<_>>()
    };

    let all = store.engine_dependents(engine, None).await.expect("dependents");
    assert_eq!(all[0].workspace_name, "Alpha");
    assert!(all[0].workspace_slug.starts_with("dep-"), "{:?}", all[0]);
    assert_eq!(
        summary(all),
        vec![
            (ws_a, EngineDependentKind::DataapiEndpoint, 7, strings(&["a", "b", "c", "d", "e"])),
            (ws_a, EngineDependentKind::DataapiEndpointTrashed, 1, strings(&["binned"])),
            (ws_b, EngineDependentKind::Dataset, 1, strings(&["orders"])),
            (ws_b, EngineDependentKind::Schedule, 1, strings(&["nightly"])),
        ]
    );

    let only_b = store
        .engine_dependents(engine, Some(&[ws_b]))
        .await
        .expect("dependents restricted to one workspace");
    assert_eq!(
        summary(only_b),
        vec![
            (ws_b, EngineDependentKind::Dataset, 1, strings(&["orders"])),
            (ws_b, EngineDependentKind::Schedule, 1, strings(&["nightly"])),
        ]
    );
    assert!(store.engine_dependents(engine, Some(&[])).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_engine_unless_blocked_keeps_the_row_while_a_dependent_exists() {
    let (db, store) = harness_or_skip!();
    let engine = seed_engine(&db, &store).await;
    let ws = seed_workspace(&db, "Gamma").await;
    let conn = seed_connection(&db, ws, engine).await;
    seed_alert(&db, ws, conn, "blocks-the-delete").await;

    match store.delete_engine_unless_blocked(engine).await.expect("outcome") {
        DeleteEngineOutcome::Blocked(d) => {
            assert_eq!(d.len(), 1, "{d:?}");
            assert_eq!(d[0].kind, EngineDependentKind::Schedule);
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert!(store.get_engine(engine).await.unwrap().is_some(), "the row must survive");

    sqlx::query("DELETE FROM automation.schedules WHERE database_id = $1")
        .bind(conn)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        store.delete_engine_unless_blocked(engine).await.unwrap(),
        DeleteEngineOutcome::Deleted
    ));
    assert!(store.get_engine(engine).await.unwrap().is_none());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM query.databases WHERE id = $1")
        .bind(conn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the materialized connection cascades with the engine");
    assert!(matches!(
        store.delete_engine_unless_blocked(engine).await.unwrap(),
        DeleteEngineOutcome::NotFound
    ));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test dependents`
Expected: compile error — `unresolved import egret_engine_mgmt::dependents`.

- [ ] **Step 3: Implement**

In `store.rs`, change `fn pool(&self) -> &sqlx::PgPool {` to `pub(crate) fn pool(&self) -> &sqlx::PgPool {`. In `lib.rs`, add `pub mod dependents;` after `pub mod delegator_policy;`.

Create `crates/egret-engine-mgmt/src/dependents.rs`:

```rust
//! What still depends on an engine.
//!
//! An engine is materialized into each assigned workspace as a `query.databases`
//! row (`engine_id` set). Deleting the engine — or unassigning it — deletes those
//! rows, and several user-authored assets hang off them. Some cascade quietly
//! (query history, notebook engine access), one is nulled (saved queries), and
//! the rest either block the DELETE outright or would be destroyed with it:
//!
//! * `dataapi.endpoints.database_id` is `ON DELETE RESTRICT` (0121), and soft
//!   delete (0126) keeps trashed endpoints in the table, so they block too;
//! * `automation.schedules.database_id` is `SET NULL` (0110) but
//!   `schedule_alert_needs_query` rejects a NULL on an alert;
//! * `analytics.datasets` cascade (0012), and through them charts, dashboard
//!   cards and report schedules — silently, which is why they block here.
//!
//! [`QUERY_DATABASES_CHILD_FKS`] is the classification, pinned against
//! `pg_constraint` by `tests/dependents.rs`.

use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::EngineMgmtError;
use crate::store::EngineMgmtStore;

/// A kind of asset that blocks deleting or unassigning an engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineDependentKind {
    /// A Data API endpoint (draft, live or disabled) on the connection.
    DataapiEndpoint,
    /// A Data API endpoint in the trash — still a row, still blocks.
    DataapiEndpointTrashed,
    /// An alert or report schedule reading the connection.
    Schedule,
    /// A dataset on the connection (charts, cards and reports hang off it).
    Dataset,
}

impl EngineDependentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DataapiEndpoint => "dataapi_endpoint",
            Self::DataapiEndpointTrashed => "dataapi_endpoint_trashed",
            Self::Schedule => "schedule",
            Self::Dataset => "dataset",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::DataapiEndpoint,
            Self::DataapiEndpointTrashed,
            Self::Schedule,
            Self::Dataset,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// One (workspace, kind) group of blocking assets, with up to five names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineDependentDto {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// For links: the UI routes a workspace as `/explore/{slug}/…`.
    pub workspace_slug: String,
    pub kind: EngineDependentKind,
    pub count: i64,
    /// Up to five names, alphabetical.
    pub sample: Vec<String>,
}

/// The 409 body for an engine delete/unassign that was refused. `code` is
/// `engine_has_dependents` (with `dependents`) or `constraint_violation` (with
/// `constraint`).
#[derive(Debug, Serialize, ToSchema)]
pub struct EngineConflictBody {
    pub error: String,
    pub code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependents: Vec<EngineDependentDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
}

/// What [`EngineMgmtStore::delete_engine_unless_blocked`] did.
#[derive(Debug)]
pub enum DeleteEngineOutcome {
    Deleted,
    /// No such row (a concurrent delete won).
    NotFound,
    /// A dependent appeared after the preflight; nothing was deleted.
    Blocked(Vec<EngineDependentDto>),
}

/// How deleting a `query.databases` row treats one FK child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildOnDelete {
    Blocks(EngineDependentKind),
    Cascades,
    SetsNull,
}

/// One FK child of `query.databases`, as `pg_constraint` describes it.
pub struct DatabaseChildFk {
    /// Schema-qualified table.
    pub table: &'static str,
    pub column: &'static str,
    /// `pg_constraint.confdeltype`: `c` cascade, `n` set null, `r` restrict, `a` no action.
    pub on_delete: &'static str,
    pub not_null: bool,
    pub handling: ChildOnDelete,
}

pub const QUERY_DATABASES_CHILD_FKS: &[DatabaseChildFk] = &[
    DatabaseChildFk {
        table: "analytics.datasets",
        column: "database_id",
        on_delete: "c",
        not_null: true,
        handling: ChildOnDelete::Blocks(EngineDependentKind::Dataset),
    },
    DatabaseChildFk {
        table: "analytics.notebook_engine_access",
        column: "database_id",
        on_delete: "c",
        not_null: true,
        handling: ChildOnDelete::Cascades,
    },
    DatabaseChildFk {
        table: "analytics.saved_queries",
        column: "database_id",
        on_delete: "n",
        not_null: false,
        handling: ChildOnDelete::SetsNull,
    },
    DatabaseChildFk {
        table: "automation.schedules",
        column: "database_id",
        on_delete: "n",
        not_null: false,
        handling: ChildOnDelete::Blocks(EngineDependentKind::Schedule),
    },
    DatabaseChildFk {
        table: "dataapi.endpoints",
        column: "database_id",
        on_delete: "r",
        not_null: true,
        handling: ChildOnDelete::Blocks(EngineDependentKind::DataapiEndpoint),
    },
    DatabaseChildFk {
        table: "query.queries",
        column: "database_id",
        on_delete: "c",
        not_null: true,
        handling: ChildOnDelete::Cascades,
    },
];

/// Blocking dependents of engine `$1`, optionally only in workspaces `$2`.
/// Keep in step with [`QUERY_DATABASES_CHILD_FKS`]; the sample cap (5) is `[1:5]`.
const DEPENDENTS_SQL: &str = "\
WITH dbs AS ( \
  SELECT d.id, d.workspace_id FROM query.databases d \
   WHERE d.engine_id = $1 AND ($2::uuid[] IS NULL OR d.workspace_id = ANY($2)) \
), items AS ( \
  SELECT dbs.workspace_id, \
         CASE WHEN e.deleted_at IS NULL THEN 'dataapi_endpoint' ELSE 'dataapi_endpoint_trashed' END AS kind, \
         e.name \
    FROM dataapi.endpoints e JOIN dbs ON dbs.id = e.database_id \
  UNION ALL \
  SELECT dbs.workspace_id, 'schedule', s.name \
    FROM automation.schedules s JOIN dbs ON dbs.id = s.database_id \
  UNION ALL \
  SELECT dbs.workspace_id, 'dataset', ds.name \
    FROM analytics.datasets ds JOIN dbs ON dbs.id = ds.database_id \
) \
SELECT i.workspace_id, w.name AS workspace_name, w.slug AS workspace_slug, i.kind, \
       count(*) AS count, (array_agg(i.name ORDER BY i.name))[1:5] AS sample \
  FROM items i JOIN workspaces w ON w.id = i.workspace_id \
 GROUP BY i.workspace_id, w.name, w.slug, i.kind \
 ORDER BY w.name, i.workspace_id, i.kind";

#[derive(sqlx::FromRow)]
struct DependentRow {
    workspace_id: Uuid,
    workspace_name: String,
    workspace_slug: String,
    kind: String,
    count: i64,
    sample: Vec<String>,
}

async fn fetch_dependents<'e, E>(
    exec: E,
    engine_id: Uuid,
    workspace_ids: Option<Vec<Uuid>>,
) -> Result<Vec<EngineDependentDto>, EngineMgmtError>
where
    E: sqlx::PgExecutor<'e>,
{
    let rows: Vec<DependentRow> = sqlx::query_as(DEPENDENTS_SQL)
        .bind(engine_id)
        .bind(workspace_ids)
        .fetch_all(exec)
        .await?;
    rows.into_iter()
        .map(|r| {
            let kind = EngineDependentKind::parse(&r.kind).ok_or_else(|| {
                EngineMgmtError::Db(sqlx::Error::Protocol(format!(
                    "unknown dependent kind {:?}",
                    r.kind
                )))
            })?;
            Ok(EngineDependentDto {
                workspace_id: r.workspace_id,
                workspace_name: r.workspace_name,
                workspace_slug: r.workspace_slug,
                kind,
                count: r.count,
                sample: r.sample,
            })
        })
        .collect()
}

impl EngineMgmtStore {
    /// Blocking dependents of `engine_id`; `workspace_ids` restricts them to
    /// those workspaces (an unassign). Empty means nothing blocks.
    pub async fn engine_dependents(
        &self,
        engine_id: Uuid,
        workspace_ids: Option<&[Uuid]>,
    ) -> Result<Vec<EngineDependentDto>, EngineMgmtError> {
        fetch_dependents(self.pool(), engine_id, workspace_ids.map(<[Uuid]>::to_vec)).await
    }

    /// Delete the engine row unless something blocks it, re-checked under locks.
    ///
    /// The engine row is locked first (`FOR UPDATE` conflicts with the
    /// `FOR KEY SHARE` a concurrent `assign` takes when it materializes a new
    /// connection), then its connections (the same conflict with a concurrent
    /// endpoint/schedule/dataset INSERT referencing one). So the dependents read
    /// here cannot go stale before the DELETE, and a race answers 409 rather
    /// than a constraint 500.
    pub async fn delete_engine_unless_blocked(
        &self,
        id: Uuid,
    ) -> Result<DeleteEngineOutcome, EngineMgmtError> {
        let mut tx = self.pool().begin().await?;
        let locked: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM engine_mgmt.engines WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if locked.is_none() {
            tx.rollback().await?;
            return Ok(DeleteEngineOutcome::NotFound);
        }
        sqlx::query("SELECT id FROM query.databases WHERE engine_id = $1 FOR UPDATE")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
        let blocking = fetch_dependents(&mut *tx, id, None).await?;
        if !blocking.is_empty() {
            tx.rollback().await?;
            return Ok(DeleteEngineOutcome::Blocked(blocking));
        }
        sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(DeleteEngineOutcome::Deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_blocking_child_is_read_by_the_preflight_sql() {
        for fk in QUERY_DATABASES_CHILD_FKS {
            if let ChildOnDelete::Blocks(kind) = fk.handling {
                assert!(DEPENDENTS_SQL.contains(fk.table), "{} not in DEPENDENTS_SQL", fk.table);
                assert!(DEPENDENTS_SQL.contains(kind.as_str()), "{kind:?} not in DEPENDENTS_SQL");
            }
        }
        assert!(DEPENDENTS_SQL.contains("dataapi_endpoint_trashed"));
    }

    #[test]
    fn kinds_round_trip_and_serialize_snake_case() {
        for k in [
            EngineDependentKind::DataapiEndpoint,
            EngineDependentKind::DataapiEndpointTrashed,
            EngineDependentKind::Schedule,
            EngineDependentKind::Dataset,
        ] {
            assert_eq!(EngineDependentKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), serde_json::json!(k.as_str()));
        }
        assert_eq!(EngineDependentKind::parse("chart"), None);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p egret-engine-mgmt --lib dependents`
Expected: 2 passed.

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test dependents`
Expected: 5 passed, no `SKIPPED` line. **If `every_fk_child_of_query_databases_is_classified_by_the_preflight` fails with a row not in the table above, that is a finding: stop and report it — do not just add it to the constant.**

Run: `cargo fmt --all -- --check && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/dependents.rs crates/egret-engine-mgmt/src/lib.rs crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/tests/dependents.rs
git commit -m "feat(engine-mgmt): compute engine dependents and delete the row under a lock

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 3: Error backstop — log every database error, 409 for FK/CHECK, new conflict variants (D3)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/http_error.rs` (whole file)
- Modify: `crates/egret-engine-mgmt/Cargo.toml` (`[dev-dependencies]`: add `tracing-subscriber.workspace = true`)
- Create: `crates/egret-engine-mgmt/tests/error_mapping.rs`

**Interfaces:**
- Consumes: `EngineDependentDto`, `EngineConflictBody` (Task 2).
- Produces (on `egret_engine_mgmt::EngineHttpError`):
  - `HasDependents(Vec<EngineDependentDto>)` → 409 `{"error", "code":"engine_has_dependents", "dependents":[…]}`
  - `ConstraintViolation(String)` (constraint name) → 409 `{"error":"blocked by database constraint <name>", "code":"constraint_violation", "constraint":"<name>"}`
  - `ClusterDeleteFailed(String)` (cause) → 502 `{"error":"engine cluster delete failed: <cause>", "code":"engine_cluster_delete_failed"}`
  - Every `sqlx::Error` reaching `From<EngineMgmtError | SecretsError | StorageRegistryError>` is logged at `error` with `sqlstate` and `constraint` fields; SQLSTATE `23503`/`23514` with a constraint name → `ConstraintViolation`, everything else → `Internal` (body unchanged: `{"error":"internal error"}`).
  - All pre-existing variants keep their exact status and `{"error": msg}` body.

- [ ] **Step 1: Write the failing tests**

Add to `crates/egret-engine-mgmt/Cargo.toml` under `[dev-dependencies]`:

```toml
# Reads back the log line the database-error backstop writes (tests/error_mapping.rs).
tracing-subscriber.workspace = true
```

Create `crates/egret-engine-mgmt/tests/error_mapping.rs`:

```rust
//! The database-error backstop in `EngineHttpError`: a constraint violation is a
//! 409 naming the constraint, anything else stays a redacted 500, and both are
//! LOGGED with their SQLSTATE — "internal error with nothing in the logs" is the
//! defect (spec D3).
//!
//! One test in its own binary on purpose: `tracing` caches callsite interest per
//! process, and another test's subscriber evaluating the callsite first can make
//! this one's subscriber miss the event (see
//! `crates/egret-dataapi/tests/serving_guard_logs.rs`).
//!
//! DB-gated: FAILS without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP` is set.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use egret_engine_mgmt::{EngineHttpError, EngineMgmtError, EngineMgmtStore};
use egret_metastore::Db;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("utf-8 log")
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Convert with a subscriber installed; the log is written inside `From`.
fn convert(e: EngineMgmtError) -> (Response, String) {
    let cap = Capture::default();
    let sub = tracing_subscriber::fmt()
        .with_writer(cap.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let resp = tracing::subscriber::with_default(sub, || EngineHttpError::from(e).into_response());
    (resp, cap.text())
}

async fn json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn database_errors_are_logged_and_constraint_violations_are_409s() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = EngineMgmtStore::new(db.clone());

    // Seed an engine + a user so the ONLY violated FK is the workspace one.
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Err', 'active') RETURNING id",
    )
    .bind(format!("err-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(format!("err-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let wh = egret_catalog_engine::CatalogStore::new(db.clone())
        .create_warehouse(&format!("err-wh-{}", Uuid::new_v4().simple()), ws, sc)
        .await
        .unwrap();
    let engine = store
        .create_engine(
            &format!("err-eng-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            wh.id,
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap()
        .id;
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("err_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // 23503: assigning to a workspace that does not exist.
    let fk = store
        .assign_workspace(engine, Uuid::new_v4(), user)
        .await
        .expect_err("unknown workspace must violate the FK");
    let (resp, log) = convert(fk);
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = json(resp).await;
    assert_eq!(body["code"], "constraint_violation", "{body}");
    assert_eq!(body["constraint"], "engine_workspaces_workspace_id_fkey", "{body}");
    assert!(log.contains("23503") && log.contains("engine_workspaces_workspace_id_fkey"), "{log}");

    // 23514: an alert with no query.
    let check = sqlx::query(
        "INSERT INTO automation.schedules (workspace_id, type, name, crontab) \
         VALUES ($1, 'alert', 'no-query', '0 9 * * *')",
    )
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect_err("CHECK must reject an alert without a query");
    let (resp, log) = convert(EngineMgmtError::Db(check));
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(json(resp).await["constraint"], "schedule_alert_needs_query");
    assert!(log.contains("23514"), "{log}");

    // Anything else stays a redacted 500 — but is no longer silent.
    let other = sqlx::query("SELECT 1 FROM engine_mgmt.no_such_table")
        .execute(&db.pool)
        .await
        .expect_err("undefined table");
    let (resp, log) = convert(EngineMgmtError::Db(other));
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json(resp).await, serde_json::json!({ "error": "internal error" }));
    assert!(log.contains("42P01") && log.contains("no_such_table"), "{log}");
}
```

Also append a unit-test module to the end of `http_error.rs` (written now, compiled in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependents::EngineDependentKind;

    async fn body(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn dependents_are_a_409_carrying_the_list() {
        let resp = EngineHttpError::HasDependents(vec![EngineDependentDto {
            workspace_id: uuid::Uuid::from_u128(1),
            workspace_name: "Sales".into(),
            workspace_slug: "sales".into(),
            kind: EngineDependentKind::DataapiEndpointTrashed,
            count: 1,
            sample: vec!["old-orders".into()],
        }])
        .into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let j = body(resp).await;
        assert_eq!(j["code"], "engine_has_dependents");
        assert_eq!(j["dependents"][0]["workspaceSlug"], "sales");
        assert_eq!(j["dependents"][0]["kind"], "dataapi_endpoint_trashed");
        assert!(j["error"].as_str().unwrap().contains("1 workspace(s)"), "{j}");
        assert!(j.get("constraint").is_none(), "{j}");
    }

    #[tokio::test]
    async fn a_failed_cluster_delete_is_a_502_with_its_cause() {
        let resp = EngineHttpError::ClusterDeleteFailed("Deployment/x: forbidden".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body(resp).await,
            serde_json::json!({
                "error": "engine cluster delete failed: Deployment/x: forbidden",
                "code": "engine_cluster_delete_failed"
            })
        );
    }

    #[tokio::test]
    async fn existing_variants_keep_their_bodies() {
        let resp = EngineHttpError::BadRequest("kubernetes not configured".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body(resp).await, serde_json::json!({ "error": "bad request: kubernetes not configured" }));
        let resp = EngineHttpError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body(resp).await, serde_json::json!({ "error": "not found" }));
        let resp = EngineHttpError::InUse(2).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(body(resp).await, serde_json::json!({ "error": "secret is in use by 2 connection(s)" }));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test error_mapping`
Expected: FAIL — `assertion left == right` on status: `500` vs `409` (today `Db(_)` → `Internal`).

- [ ] **Step 3: Implement**

Replace `crates/egret-engine-mgmt/src/http_error.rs` above the new `mod tests` with:

```rust
//! Engine-mgmt seam HTTP error.
//!
//! The variants that predate the engine-delete preflight mirror
//! `egret-controlplane`'s mapping exactly (status + `{"error": msg}` body). The
//! three added for engine delete/unassign also carry a machine-readable `code`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_catalog_api::CatalogApiError;
use egret_secrets::SecretsError;

use crate::dependents::{EngineConflictBody, EngineDependentDto};

#[derive(Debug, thiserror::Error)]
pub enum EngineHttpError {
    /// Caller is authenticated but lacks the `platform_admin` role.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict that isn't a duplicate-name/email clash covered by
    /// `AlreadyExists` — e.g. the last-platform-admin guard or a seeded-role
    /// delete guard.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A named secret is still referenced and cannot be deleted.
    #[error("secret is in use by {0} connection(s)")]
    InUse(i64),
    /// A Trino coordinator request failed (unreachable or non-2xx / non-JSON
    /// response) — surfaced to the UI as a 502 the monitor page can show.
    #[error("engine not reachable")]
    BadGateway,
    /// Data API endpoints, schedules or datasets still use the engine's workspace
    /// connections; nothing was changed. 409 with the list.
    #[error("engine is still in use")]
    HasDependents(Vec<EngineDependentDto>),
    /// A write hit a foreign-key (23503) or CHECK (23514) constraint the
    /// preflight did not anticipate. 409 naming the constraint.
    #[error("blocked by database constraint {0}")]
    ConstraintViolation(String),
    /// The Kubernetes delete failed with something other than 404. The engine row
    /// stays `deleting` and the delete can be retried. 502 with the cause.
    #[error("engine cluster delete failed: {0}")]
    ClusterDeleteFailed(String),
    #[error("internal error")]
    Internal,
}

/// The one place a `sqlx::Error` becomes an HTTP error. It is logged here because
/// this is the last point the error still exists: `Internal` has no payload, and
/// "internal error with nothing in the logs" was the reported defect.
fn db_error(e: sqlx::Error) -> EngineHttpError {
    let (sqlstate, constraint) = match e.as_database_error() {
        Some(d) => (d.code().map(|c| c.into_owned()), d.constraint().map(str::to_owned)),
        None => (None, None),
    };
    tracing::error!(
        error = %e,
        sqlstate = sqlstate.as_deref().unwrap_or("-"),
        constraint = constraint.as_deref().unwrap_or("-"),
        "engine-mgmt database error"
    );
    match (sqlstate.as_deref(), constraint) {
        (Some("23503") | Some("23514"), Some(name)) => EngineHttpError::ConstraintViolation(name),
        _ => EngineHttpError::Internal,
    }
}

impl From<CatalogApiError> for EngineHttpError {
    fn from(e: CatalogApiError) -> Self {
        match e {
            CatalogApiError::NotFound(_) => EngineHttpError::NotFound,
            CatalogApiError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            CatalogApiError::InUse => EngineHttpError::BadRequest(
                "role is in use by one or more Catalog Clients — delete those clients first"
                    .to_string(),
            ),
            CatalogApiError::BadRequest(s) => EngineHttpError::BadRequest(s),
            CatalogApiError::Backend(_) => EngineHttpError::Internal,
        }
    }
}

impl From<egret_storage::StorageRegistryError> for EngineHttpError {
    fn from(e: egret_storage::StorageRegistryError) -> Self {
        use egret_storage::StorageRegistryError as E;
        match e {
            E::NotFound(_) => EngineHttpError::NotFound,
            E::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            E::Db(e) => db_error(e),
        }
    }
}

impl From<crate::EngineMgmtError> for EngineHttpError {
    fn from(e: crate::EngineMgmtError) -> Self {
        match e {
            crate::EngineMgmtError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            crate::EngineMgmtError::NotFound(_) => EngineHttpError::NotFound,
            crate::EngineMgmtError::NotAssignable(s) => EngineHttpError::BadRequest(s),
            crate::EngineMgmtError::Db(e) => db_error(e),
        }
    }
}

impl From<SecretsError> for EngineHttpError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotFound => EngineHttpError::NotFound,
            SecretsError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            SecretsError::InUse(n) => EngineHttpError::InUse(n),
            SecretsError::NotConfigured => {
                EngineHttpError::BadRequest("secrets master_key is not configured".to_string())
            }
            SecretsError::InvalidType(_) | SecretsError::MissingKey(_) => {
                EngineHttpError::BadRequest(e.to_string())
            }
            SecretsError::Crypto(_) => EngineHttpError::Internal,
            SecretsError::Db(e) => db_error(e),
        }
    }
}
```

Keep the existing `impl From<egret_core::Error> for EngineHttpError` block **unchanged**, then replace `impl IntoResponse for EngineHttpError` with:

```rust
fn error_json(status: StatusCode, msg: String) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

impl IntoResponse for EngineHttpError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        match self {
            EngineHttpError::HasDependents(dependents) => {
                let workspaces = dependents
                    .iter()
                    .map(|d| d.workspace_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                let body = EngineConflictBody {
                    error: format!(
                        "{msg}: {workspaces} workspace(s) have Data API endpoints, alerts or \
                         datasets on it; remove or move them first"
                    ),
                    code: "engine_has_dependents".into(),
                    dependents,
                    constraint: None,
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::ConstraintViolation(constraint) => {
                let body = EngineConflictBody {
                    error: msg,
                    code: "constraint_violation".into(),
                    dependents: Vec::new(),
                    constraint: Some(constraint),
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::ClusterDeleteFailed(_) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": msg, "code": "engine_cluster_delete_failed" })),
            )
                .into_response(),
            EngineHttpError::Forbidden => error_json(StatusCode::FORBIDDEN, msg),
            EngineHttpError::NotFound => error_json(StatusCode::NOT_FOUND, msg),
            EngineHttpError::AlreadyExists(_)
            | EngineHttpError::Conflict(_)
            | EngineHttpError::InUse(_) => error_json(StatusCode::CONFLICT, msg),
            EngineHttpError::BadRequest(_) => error_json(StatusCode::BAD_REQUEST, msg),
            EngineHttpError::BadGateway => error_json(StatusCode::BAD_GATEWAY, msg),
            EngineHttpError::Internal => error_json(StatusCode::INTERNAL_SERVER_ERROR, msg),
        }
    }
}
```

(`Forbidden`/`NotFound`/`Internal` Display is exactly the literal the old code sent — `"forbidden"`, `"not found"`, `"internal error"` — so bodies are unchanged; `existing_variants_keep_their_bodies` pins it.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p egret-engine-mgmt --lib http_error`
Expected: 3 passed.

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test error_mapping`
Expected: 1 passed, no `SKIPPED`.

Run: `cargo fmt --all -- --check && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/http_error.rs crates/egret-engine-mgmt/Cargo.toml crates/egret-engine-mgmt/tests/error_mapping.rs Cargo.lock
git commit -m "fix(engine-mgmt): log database errors and map FK/CHECK violations to 409

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 4: `DELETE /engines/{id}` — preflight, Kubernetes required, `deleting`, 502, locked delete, cleanup (D1, D2, D4, D7, D8, D9 secrets/clients)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/store.rs` (add `ENGINE_STATUS_DELETING` const after the `use` lines)
- Modify: `crates/egret-engine-mgmt/src/engines.rs:39-40` (imports), `:3523-3570` (`get_engine`), `:3866-3903` (`delete_engine`; add `cleanup_deleted_engine` after it). **Do not touch the `#[utoipa::path]` blocks here — Task 7 does.**
- Modify: `crates/egret-engine-mgmt/tests/routes.rs:376-423` (`FakeEnginePort`) + new helpers/tests at the end

**Interfaces:**
- Consumes: `EngineMgmtStore::engine_dependents`, `delete_engine_unless_blocked`, `DeleteEngineOutcome` (Task 2); `EngineHttpError::{HasDependents, ClusterDeleteFailed}` (Task 3); `set_engine_status`, `list_engine_catalogs` (existing); `client_row_id`, `is_minted_client` (`engines.rs:1901,1912`).
- Produces:
  - `pub const egret_engine_mgmt::store::ENGINE_STATUS_DELETING: &str = "deleting";`
  - `async fn cleanup_deleted_engine(st: &CpState, engine: &Engine, cats: &[EngineCatalog])` (private; Task 5 extends it)
  - Route contract, in order: 403/404 → **409** `engine_has_dependents` (nothing written) → **400** `kubernetes not configured` (nothing written) → row `status='deleting'` → provisioner delete, failure **502** `engine_cluster_delete_failed` (row stays `deleting`) → locked delete (race → 409) → cleanup (logged) → **204**.
  - `GET /engines/{id}` for a `deleting` row: `status = {phase:"deleting", message:"a previous delete did not finish; retry the delete", pods:[]}` without calling the provisioner.
  - Test fake: `FakeEnginePort { restarted, deleted: Arc<Mutex<Vec<(String, String, String)>>>, delete_error: Arc<Mutex<Option<String>>> }` recording `(kind, namespace, name)`.

- [ ] **Step 1: Make the fake observable, then write the failing route tests**

In `tests/routes.rs`, replace the `FakeEnginePort` struct and its `delete` impl:

```rust
#[derive(Clone, Default)]
struct FakeEnginePort {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// `(kind, namespace, name)` per successful `delete`.
    deleted: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    /// When `Some`, `delete` fails with this message and records nothing.
    delete_error: Arc<std::sync::Mutex<Option<String>>>,
}
```

(`restarted` is unchanged — `Vec<(String, String)>`, pushed by `restart`; keep the existing doc comment above the struct and extend it with one line about the two new fields.)

```rust
    async fn delete(&self, kind: &str, ns: &str, name: &str) -> Result<(), String> {
        if let Some(msg) = self.delete_error.lock().unwrap().clone() {
            return Err(msg);
        }
        self.deleted
            .lock()
            .unwrap()
            .push((kind.to_string(), ns.to_string(), name.to_string()));
        Ok(())
    }
```

Append to the end of `tests/routes.rs`:

```rust
// ── engine delete (SP1 Part A) ─────────────────────────────────────────────────

/// The stored lifecycle status, or `None` when the row is gone.
async fn engine_status(f: &Fixture, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(id)
        .fetch_optional(&f.db.pool)
        .await
        .expect("read engine status")
}

/// The materialized connection for (workspace, engine).
async fn materialized_db_id(f: &Fixture, ws: Uuid, engine_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM query.databases WHERE workspace_id = $1 AND engine_id = $2")
        .bind(ws)
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("materialized connection")
}

/// `PUT /engines/{id}/workspaces` → 200.
async fn put_workspaces(f: &Fixture, token: &str, engine_id: Uuid, ws: &[Uuid]) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            token,
            Some(serde_json::json!({ "workspaceIds": ws })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "put workspaces; body={j}");
}

/// Seed one blocking dependent of `kind` (a `dependents::EngineDependentKind`
/// wire name) called `name` on connection `database_id`.
async fn seed_dependent(f: &Fixture, ws: Uuid, database_id: Uuid, kind: &str, name: &str) {
    match kind {
        "dataapi_endpoint" | "dataapi_endpoint_trashed" => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO dataapi.endpoints (workspace_id, name, path, method, database_id, statements, state) \
                 VALUES ($1, $2, $3, 'GET', $4, \
                         jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), 'draft') \
                 RETURNING id",
            )
            .bind(ws)
            .bind(name)
            .bind(format!("/{name}"))
            .bind(database_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("seed endpoint");
            if kind == "dataapi_endpoint_trashed" {
                sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&f.db.pool)
                    .await
                    .expect("trash endpoint");
            }
        }
        "schedule" => {
            sqlx::query(
                "INSERT INTO automation.schedules (workspace_id, type, name, crontab, database_id, sql, validator_type) \
                 VALUES ($1, 'alert', $2, '0 9 * * *', $3, 'SELECT 1', 'not_null')",
            )
            .bind(ws)
            .bind(name)
            .bind(database_id)
            .execute(&f.db.pool)
            .await
            .expect("seed alert");
        }
        "dataset" => {
            sqlx::query(
                "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
                 VALUES ($1, $2, $3, 't')",
            )
            .bind(ws)
            .bind(database_id)
            .bind(name)
            .execute(&f.db.pool)
            .await
            .expect("seed dataset");
        }
        other => panic!("unknown dependent kind {other}"),
    }
}

/// D1/D2/D8: one blocking dependent → 409 naming it; the cluster and the row untouched.
async fn assert_engine_delete_blocked_by(kind: &str) {
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
    let token = session_for(&f, "engine_delete_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let ws = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws]).await;
    let db = materialized_db_id(&f, ws, engine_id).await;
    seed_dependent(&f, ws, db, kind, "orders-by-day").await;

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
    assert_eq!(j["code"], "engine_has_dependents", "body={j}");
    let deps = j["dependents"].as_array().expect("dependents array");
    assert_eq!(deps.len(), 1, "body={j}");
    assert_eq!(deps[0]["workspaceId"], ws.to_string(), "body={j}");
    assert_eq!(deps[0]["kind"], kind, "body={j}");
    assert_eq!(deps[0]["count"], 1, "body={j}");
    assert_eq!(deps[0]["sample"], serde_json::json!(["orders-by-day"]), "body={j}");
    assert!(port.deleted.lock().unwrap().is_empty(), "the cluster must not be touched");
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("provisioning"),
        "the row must survive with its status unchanged"
    );
    assert_eq!(materialized_count(&f, ws, engine_id).await, 1, "the connection survives");
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_trashed_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint_trashed").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_sql_alert() {
    assert_engine_delete_blocked_by("schedule").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_dataset() {
    assert_engine_delete_blocked_by("dataset").await;
}

/// D7: create refuses without Kubernetes, so delete must too — deleting only the
/// row would orphan the cluster.
#[tokio::test]
async fn delete_engine_without_kubernetes_is_400_and_keeps_the_row() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_nokube", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(j["error"].as_str().unwrap_or("").contains("kubernetes not configured"), "body={j}");
    assert_eq!(engine_status(&f, engine_id).await.as_deref(), Some("provisioning"));
}

/// D4/D5/D6: a failed cluster delete is a 502 with the cause, leaves a visible
/// `deleting` row, and the same request succeeds once the cluster answers.
#[tokio::test]
async fn delete_engine_cluster_failure_is_502_deleting_and_retryable() {
    let port = FakeEnginePort::default();
    *port.delete_error.lock().unwrap() = Some("apiserver: 500 Internal Server Error".into());
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_retry", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let uri = format!("/engines/{engine_id}");

    let resp = f.app.clone().oneshot(req("DELETE", &uri, &token, None)).await.unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert_eq!(j["code"], "engine_cluster_delete_failed", "body={j}");
    assert!(j["error"].as_str().unwrap_or("").contains("apiserver: 500"), "body={j}");
    assert_eq!(engine_status(&f, engine_id).await.as_deref(), Some("deleting"));

    let resp = f.app.clone().oneshot(req("GET", &uri, &token, None)).await.unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(j["status"]["phase"], "deleting", "stored status must not be masked; body={j}");

    *port.delete_error.lock().unwrap() = None;
    let resp = f.app.clone().oneshot(req("DELETE", &uri, &token, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "retry succeeds");
    assert_eq!(engine_status(&f, engine_id).await, None, "row gone after the retry");
}

/// Happy path: 204, cluster deleted once, engine-generated secrets gone, the
/// minted catalog client revoked, a client we did NOT mint left alone.
#[tokio::test]
async fn delete_engine_removes_cluster_row_secrets_and_minted_clients_only() {
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
    let token = session_for(&f, "engine_delete_ok", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id: Uuid = j["id"].as_str().unwrap().parse().unwrap();

    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    let delegator = secrets
        .create(None, &format!("test-delegator-{}", Uuid::new_v4().simple()), None, "pw")
        .await
        .unwrap();
    let root = secrets
        .create(None, &format!("test-root-{}", Uuid::new_v4().simple()), None, "pw")
        .await
        .unwrap();
    store.set_delegator_secret(engine_id, delegator).await.unwrap();
    store.set_root_secret(engine_id, root).await.unwrap();
    let engine = store.get_engine(engine_id).await.unwrap().unwrap();
    let mut engine_secrets = vec![delegator, root];
    engine_secrets.extend(engine.tls_secret_id);

    let primary = store
        .list_engine_catalogs(engine_id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.is_primary)
        .expect("primary catalog");
    let minted: Uuid = primary.config["clientRowId"].as_str().unwrap().parse().unwrap();

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store.get_warehouse(wh_id).await.unwrap().unwrap();
    let role = catalog_store
        .create_role(wh_id, "readonly_analyst", "external", Some("read-only role"))
        .await
        .unwrap();
    let foreign = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "not-minted-by-the-engine".into(),
            consumer_type: "external".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh_id,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    store
        .create_engine_catalog(
            engine_id,
            "external",
            "egret-warehouse",
            &serde_json::json!({ "clientRowId": foreign.id, "mintedClient": false }),
            None,
            false,
        )
        .await
        .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body={j}");

    assert_eq!(
        *port.deleted.lock().unwrap(),
        vec![("trino".to_string(), "egret-engines".to_string(), name.clone())]
    );
    assert_eq!(engine_status(&f, engine_id).await, None, "row gone");
    let secrets_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_secrets WHERE id = ANY($1)")
            .bind(engine_secrets.clone())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(secrets_left, 0, "engine-generated secrets {engine_secrets:?} must be deleted");
    let client_count = |id: Uuid| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog.catalog_clients WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(client_count(minted).await, 0, "the minted client is revoked");
    assert_eq!(client_count(foreign.id).await, 1, "a client the engine did not mint survives");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test routes delete_engine`
Expected: 7 tests, all FAIL — blocked kinds get `500` (endpoint/alert) or `204` (dataset), no-kube gets `204`, the 502 test gets `400`, the happy path fails on `secrets_left` and `client_count(foreign.id)`.

- [ ] **Step 3: Implement**

`store.rs`, after the `use` lines:

```rust
/// Stored lifecycle status written when an engine delete starts. A row still in
/// this state means the cluster delete or the row delete did not finish, and the
/// delete can be retried.
pub const ENGINE_STATUS_DELETING: &str = "deleting";
```

`engines.rs` imports (next to `use crate::{Engine, EngineCatalog};`):

```rust
use crate::dependents::DeleteEngineOutcome;
use crate::store::ENGINE_STATUS_DELETING;
```

In `get_engine`, replace `let status = match st.engine_provisioner.as_ref() { … };` with:

```rust
    // A `deleting` row is the retry state of an unfinished delete; the live
    // status of a half-deleted cluster would hide it, so it wins.
    let status = if engine.status == ENGINE_STATUS_DELETING {
        EngineStatusDto {
            phase: engine.status.clone(),
            message: Some("a previous delete did not finish; retry the delete".into()),
            pods: vec![],
        }
    } else {
        match st.engine_provisioner.as_ref() {
            // … the existing Some(prov)/None arms, unchanged …
        }
    };
```

Replace the `delete_engine` function body (keep its `#[utoipa::path]` attribute) with:

```rust
async fn delete_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:delete").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

    // 1. Preflight. Nothing is touched while a user-authored asset still sits on
    //    one of this engine's workspace connections: the row delete would fail on
    //    it (endpoints RESTRICT, alerts CHECK) or silently destroy it (datasets).
    let dependents = st.engine_mgmt.engine_dependents(id, None).await?;
    if !dependents.is_empty() {
        return Err(CpError::HasDependents(dependents));
    }

    // 2. Same rule as create: without Kubernetes the cluster cannot be removed,
    //    and deleting only the row would orphan it.
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Catalog client ids, read before the catalogs cascade away with the row.
    let cats = st.engine_mgmt.list_engine_catalogs(engine.id).await?;

    // 3. Visible, retryable state for everything below that can fail.
    st.engine_mgmt
        .set_engine_status(id, ENGINE_STATUS_DELETING)
        .await?;

    // 4. The cluster. A 404 is already success inside the provisioners.
    if let Err(msg) = prov
        .delete(&engine.r#type, &engine.namespace, &engine.name)
        .await
    {
        tracing::error!(engine = %engine.name, error = %msg, "engine cluster delete failed; row left deleting");
        return Err(CpError::ClusterDeleteFailed(msg));
    }

    // 5. The row, re-checking dependents under locks (a dependent created since
    //    step 1 answers 409; the row stays `deleting` and a retry finishes).
    match st.engine_mgmt.delete_engine_unless_blocked(id).await? {
        DeleteEngineOutcome::Deleted | DeleteEngineOutcome::NotFound => {}
        DeleteEngineOutcome::Blocked(dependents) => {
            return Err(CpError::HasDependents(dependents));
        }
    }

    // 6. Cross-seam cleanup after commit: logged, never returned.
    cleanup_deleted_engine(&st, &engine, &cats).await;
    Ok(StatusCode::NO_CONTENT)
}

/// What an engine leaves outside its own tables. Each seam owns its own pool, so
/// none of this can share the row delete's transaction; a failure here is logged
/// and leaves an orphan, never a half-deleted engine.
async fn cleanup_deleted_engine(st: &CpState, engine: &Engine, cats: &[EngineCatalog]) {
    // Only clients we minted; `mintedClient: false` belongs to someone else.
    for c in cats {
        let Some(cid) = client_row_id(&c.config) else {
            continue;
        };
        if !is_minted_client(&c.config) {
            continue;
        }
        if let Err(e) = st.catalog.revoke_client(cid).await {
            tracing::warn!(engine = %engine.name, client = %cid, error = %e, "revoke engine catalog client failed");
        }
    }
    // Secrets egret generated for this engine. `tls_cert_secret_ref` is an
    // admin-provided secret and is deliberately not here.
    for sid in [
        engine.delegator_secret_id,
        engine.root_secret_id,
        engine.tls_secret_id,
    ]
    .into_iter()
    .flatten()
    {
        match st.secrets.delete(None, sid).await {
            Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
            Err(e) => {
                tracing::warn!(engine = %engine.name, secret = %sid, error = %e, "delete engine secret failed")
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test routes delete_engine`
Expected: 7 passed, no `skip:` line.

Run the whole crate once (one suite at a time): `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt`
Expected: PASS, including the pre-existing `create_list_get_delete_engine` store test and `create_and_get_engine`.

Run: `cargo fmt --all -- --check && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "fix(engine-mgmt): preflight engine delete and delete the cluster only when the row can go

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 5: Engine delete keeps its Ranger profile — unbind only (D9, profile half)

**User decision (2026-09-15):** deleting an engine never deletes a Ranger profile or tag service. Nothing records whether a profile was auto-created, and a profile can hold policies an admin wrote. The engine row's delete already unbinds it (`engines.authz_service_id` goes with the row; other engines bound to the profile keep it). Part B (Task 13) makes a re-created engine with the same name reuse the orphan profile, which closes the allow-all hole. Admins delete orphan profiles themselves in Governance → Policies. This task only **pins** that behaviour so a later "cleanup" cannot silently delete policies.

**Files:**
- Modify: `crates/egret-engine-mgmt/tests/routes.rs` (two helpers + one test at the end)

**Interfaces:**
- Consumes: `PolicyPort::{ensure_trino_def, ensure_tag_def, create_service, get_service_opt}`; `EngineMgmtStore::set_engine_authz_service`; fixtures `setup_full`, `session_for`, `seed_provisioned_warehouse`, `seed_jwt_engine`, `req` (existing in `tests/routes.rs`).
- Produces: nothing for later tasks. Do NOT add `PolicyPort::delete_service` in Part A (Part B adds it for its own create compensation).

- [ ] **Step 1: Write the pinning test** (append to `tests/routes.rs`)

```rust
/// Seed a trino profile named `name` with its `{name}_tag` tag service, as
/// auto-bind creates them, and bind `engines` to it.
async fn seed_ranger_profile(f: &Fixture, name: &str, engines: &[Uuid]) -> i64 {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::ensure_trino_def(&policy).await.expect("trino def");
    PolicyPort::ensure_tag_def(&policy).await.expect("tag def");
    let tag = format!("{name}_tag");
    PolicyPort::create_service(&policy, &tag, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let profile = PolicyPort::create_service(
        &policy,
        name,
        "trino",
        &serde_json::json!({ "tagService": tag }),
    )
    .await
    .expect("trino profile");
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    for e in engines {
        store.set_engine_authz_service(*e, Some(profile.id)).await.unwrap();
    }
    profile.id
}

async fn service_exists(f: &Fixture, name: &str) -> bool {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::get_service_opt(&policy, name).await.unwrap().is_some()
}

/// D9, by user decision: a profile can hold admin-written policies and nothing
/// records that egret created it, so engine delete unbinds and never deletes.
#[tokio::test]
async fn delete_engine_keeps_its_ranger_profile_and_tag_service() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_keeps_ranger", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let name: String = sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let profile_id = seed_ranger_profile(&f, &name, &[engine_id]).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(service_exists(&f, &name).await, "the profile survives the engine");
    assert!(service_exists(&f, &format!("{name}_tag")).await, "and so does its tag service");
    let bound: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1",
    )
    .bind(profile_id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(bound, 0, "no engine is bound to it any more");
}
```

- [ ] **Step 2: Run it**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test routes delete_engine_keeps_its_ranger_profile -- --nocapture`
Expected: PASS after Task 4, with no `skip:` line and a non-zero duration (a skip also prints `ok`).

- [ ] **Step 3: Prove the test can fail**

Temporarily add, at the end of `cleanup_deleted_engine` in `engines.rs`, a raw `sqlx::query("DELETE FROM policy.authz_services WHERE name = $1").bind(&engine.name)` executed on the engine-mgmt pool; re-run Step 2 and confirm it FAILS on "the profile survives the engine"; then remove the line with an editor (not `git checkout`, which would also drop Task 4's uncommitted work if any) and confirm `git diff crates/egret-engine-mgmt/src/engines.rs` is empty.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-engine-mgmt/tests/routes.rs
git commit -m "test(engine-mgmt): pin that engine delete keeps its Ranger profile

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Unassign and `PUT /engines/{id}/workspaces` preflight; unknown workspace is 404 (D10)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/store.rs` (add `missing_workspace_ids` after `list_engine_workspace_ids`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — bodies of `set_engine_workspaces` (~4064), `assign_workspace_engine` (~4120), `unassign_workspace_engine` (~4146). Attributes are Task 7.
- Modify: `crates/egret-engine-mgmt/tests/routes.rs` (tests at the end; reuses Task 4 helpers)

**Interfaces:**
- Consumes: `engine_dependents(engine_id, Some(&[..]))` (Task 2), `HasDependents` (Task 3), `put_workspaces`, `materialized_db_id`, `seed_dependent` (Task 4 test helpers), `materialized_count`, `seed_workspace`, `seed_jwt_engine` (existing).
- Produces: `EngineMgmtStore::missing_workspace_ids(&self, ids: &[Uuid]) -> Result<Vec<Uuid>, EngineMgmtError>`. Route contracts:
  - `DELETE /api/admin/workspaces/{ws}/engines/{engine}`: unknown ws → 404; dependents in ws → 409 (nothing changed); else 204.
  - `POST /api/admin/workspaces/{ws}/engines`: unknown ws → 404 (was 500).
  - `PUT /api/control-plane/engines/{id}/workspaces`: unknown engine → 404; any unknown workspace among the added ones → 404; additions on a non-delegation engine → 400; dependents in any removed workspace → 409; all checked before any write.

- [ ] **Step 1: Write the failing tests** (append to `tests/routes.rs`)

```rust
// ── engine ↔ workspace preflight (SP1 Part A) ────────────────────────────────

/// Every blocking kind refuses an unassign, and nothing changes.
#[tokio::test]
async fn unassign_is_refused_while_the_workspace_uses_the_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unassign_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    for kind in ["dataapi_endpoint", "dataapi_endpoint_trashed", "schedule", "dataset"] {
        let engine_id = seed_jwt_engine(&f, wh).await;
        let ws = seed_workspace(&f).await;
        put_workspaces(&f, &token, engine_id, &[ws]).await;
        let db = materialized_db_id(&f, ws, engine_id).await;
        seed_dependent(&f, ws, db, kind, "nightly").await;

        let resp = f
            .app
            .clone()
            .oneshot(req("DELETE", &format!("/workspaces/{ws}/engines/{engine_id}"), &token, None))
            .await
            .unwrap();
        let (status, j) = into_json(resp).await;
        assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
        assert_eq!(j["code"], "engine_has_dependents", "kind={kind}; body={j}");
        assert_eq!(j["dependents"][0]["kind"], kind, "body={j}");
        assert_eq!(materialized_count(&f, ws, engine_id).await, 1, "kind={kind}: connection kept");
        let assigned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engine_workspaces WHERE engine_id = $1 AND workspace_id = $2",
        )
        .bind(engine_id)
        .bind(ws)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
        assert_eq!(assigned, 1, "kind={kind}: assignment kept");
    }
}

/// `PUT {wsB}` over `{wsA}` is one assign + one blocked unassign: nothing applies.
#[tokio::test]
async fn put_workspaces_applies_nothing_when_one_unassign_is_blocked() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "put_ws_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let ws_a = seed_workspace(&f).await;
    let ws_b = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws_a]).await;
    let db = materialized_db_id(&f, ws_a, engine_id).await;
    seed_dependent(&f, ws_a, db, "dataapi_endpoint", "orders").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws_b] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert_eq!(j["dependents"][0]["workspaceId"], ws_a.to_string(), "body={j}");
    assert_eq!(materialized_count(&f, ws_a, engine_id).await, 1, "A untouched");
    assert_eq!(materialized_count(&f, ws_b, engine_id).await, 0, "B was NOT assigned first");
}

#[tokio::test]
async fn unknown_workspaces_are_404_and_apply_nothing() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unknown_ws", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let real = seed_workspace(&f).await;
    let ghost = Uuid::new_v4();

    // PUT with one real and one unknown workspace: 404, the real one not assigned.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [real, ghost] })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "PUT unknown workspace (was 500)");
    assert_eq!(materialized_count(&f, real, engine_id).await, 0);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{ghost}/engines"),
            &token,
            Some(serde_json::json!({ "engineId": engine_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "assign to unknown workspace (was 500)");

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/workspaces/{ghost}/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unassign from unknown workspace (was 204)");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test routes -- unassign_is_refused put_workspaces_applies unknown_workspaces`
Expected: 3 FAIL — endpoint/alert unassign are `500`, dataset unassign `204`; PUT is `500` after assigning B; PUT unknown is `500`.

- [ ] **Step 3: Implement**

`store.rs`, after `list_engine_workspace_ids`:

```rust
    /// The ids in `ids` that name no workspace.
    pub async fn missing_workspace_ids(&self, ids: &[Uuid]) -> Result<Vec<Uuid>, EngineMgmtError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_scalar(
            "SELECT x FROM unnest($1::uuid[]) AS x \
             WHERE NOT EXISTS (SELECT 1 FROM workspaces w WHERE w.id = x)",
        )
        .bind(ids.to_vec())
        .fetch_all(self.pool())
        .await?)
    }
```

`engines.rs` — replace the body of `set_engine_workspaces`:

```rust
async fn set_engine_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<SetWorkspacesBody>,
) -> Result<Json<Vec<EngineWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

    let current: std::collections::HashSet<Uuid> = st
        .engine_mgmt
        .list_workspaces_for_engine(id)
        .await?
        .into_iter()
        .map(|w| w.workspace_id)
        .collect();
    let desired: std::collections::HashSet<Uuid> = b.workspace_ids.into_iter().collect();
    let to_assign: Vec<Uuid> = desired.difference(&current).copied().collect();
    let to_unassign: Vec<Uuid> = current.difference(&desired).copied().collect();

    // Validate the WHOLE change before writing any of it. The assignment service
    // writes through two seams' pools, so it cannot be one transaction; every
    // refusal it could hit is therefore checked here first.
    if !st.engine_mgmt.missing_workspace_ids(&to_assign).await?.is_empty() {
        return Err(CpError::NotFound);
    }
    if !to_assign.is_empty() && !engine.supports_delegation() {
        return Err(CpError::BadRequest(
            "engine must support delegation before it can be assigned".into(),
        ));
    }
    if !to_unassign.is_empty() {
        let dependents = st
            .engine_mgmt
            .engine_dependents(id, Some(&to_unassign))
            .await?;
        if !dependents.is_empty() {
            return Err(CpError::HasDependents(dependents));
        }
    }

    let actor = p.user_id.0;
    for ws in &to_assign {
        st.assignment.assign(id, *ws, actor).await?;
    }
    for ws in &to_unassign {
        st.assignment.unassign(id, *ws).await?;
    }

    let ws = st.engine_mgmt.list_workspaces_for_engine(id).await?;
    Ok(Json(ws.into_iter().map(Into::into).collect()))
}
```

(The 400 text equals `EngineAssignmentService::assign`'s `NotAssignable` message, which `From<EngineMgmtError>` already turns into `BadRequest` — same body as before.)

Replace the body of `assign_workspace_engine` (after `require`):

```rust
    if !st.engine_mgmt.missing_workspace_ids(&[id]).await?.is_empty() {
        return Err(CpError::NotFound);
    }
    st.assignment.assign(b.engine_id, id, p.user_id.0).await?;
    Ok(StatusCode::NO_CONTENT)
```

Replace the body of `unassign_workspace_engine` (after `require`):

```rust
    if !st.engine_mgmt.missing_workspace_ids(&[id]).await?.is_empty() {
        return Err(CpError::NotFound);
    }
    // Unassigning deletes the workspace's materialized connection, which fails on
    // (endpoints, alerts) or destroys (datasets) whatever sits on it.
    let dependents = st
        .engine_mgmt
        .engine_dependents(engine_id, Some(std::slice::from_ref(&id)))
        .await?;
    if !dependents.is_empty() {
        return Err(CpError::HasDependents(dependents));
    }
    st.assignment.unassign(engine_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt --test routes`
Expected: PASS — the 3 new tests plus the pre-existing `cp_set_engine_workspaces_assigns_and_materializes` and `workspace_admin_assign_list_unassign_engine`.

Run: `cargo fmt --all -- --check && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/tests/routes.rs
git commit -m "fix(engine-mgmt): preflight engine unassign and validate workspace changes before writing

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 7: Document the new responses; regenerate `openapi.json` and the frontend client

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `#[utoipa::path]` of `delete_engine` (~3853), `set_engine_workspaces` (~4049), `assign_workspace_engine` (~4104), `unassign_workspace_engine` (~4131); `EngineMgmtApi` `components(schemas(…))` (~5128); imports
- Modify: `openapi.json`
- Modify (generated): `frontend/src/api/generated/control-plane-engines/control-plane-engines.ts`, `frontend/src/api/generated/model/index.ts`
- Create (generated): `frontend/src/api/generated/model/engineConflictBody.ts`, `engineDependentDto.ts`, `engineDependentKind.ts`

**Interfaces:**
- Consumes: `EngineConflictBody`, `EngineDependentDto`, `EngineDependentKind` (Task 2).
- Produces: OpenAPI schemas `EngineConflictBody`, `EngineDependentDto`, `EngineDependentKind`; TS types of the same names under `@/api/generated/model/…` (used by Tasks 9–10).

- [ ] **Step 1: Confirm the drift gate is green before the change**

Run: `cargo test -p egret-server --lib committed_openapi_json_is_current`
Expected: PASS (no DB needed). If it already fails on the branch, stop and report.

- [ ] **Step 2: Edit the annotations**

Add to the imports in `engines.rs`:

```rust
use crate::dependents::{EngineConflictBody, EngineDependentDto, EngineDependentKind};
```

`delete_engine`:

```rust
#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "Cluster deleted, engine row removed (its catalogs, workspace connections, query history and notebook access cascade), engine-generated secrets, minted Catalog Clients and the engine's own Ranger profile cleaned up"),
        (status = 400, description = "Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:delete permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "Nothing deleted: Data API endpoints (including trashed), schedules or datasets still use the engine's workspace connections (`code` = `engine_has_dependents`, with `dependents`), or a database constraint refused the delete (`code` = `constraint_violation`, with `constraint`)", body = EngineConflictBody),
        (status = 500, description = "Internal error", body = ErrorBody),
        (status = 502, description = "The Kubernetes delete failed (`code` = `engine_cluster_delete_failed`); the engine stays `deleting` and the request can be retried", body = ErrorBody),
    ),
)]
```

`set_engine_workspaces` — add two rows to its `responses(...)`:

```rust
        (status = 404, description = "No such engine, or an added workspace does not exist; nothing applied", body = ErrorBody),
        (status = 409, description = "A removed workspace still has Data API endpoints, schedules or datasets on the engine's connection; nothing applied", body = EngineConflictBody),
```

(and delete its existing `(status = 404, description = "No such engine", …)` row so 404 appears once).

`assign_workspace_engine` — change the 404 row's description to `"No such engine or workspace"`.

`unassign_workspace_engine` — add:

```rust
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 409, description = "The workspace still has Data API endpoints, schedules or datasets on the engine's connection; nothing changed", body = EngineConflictBody),
```

`EngineMgmtApi` `components(schemas(…))` — add after `AssignEngineBody,`:

```rust
        EngineConflictBody,
        EngineDependentDto,
        EngineDependentKind,
```

- [ ] **Step 3: Watch the drift gate fail, then regenerate**

Run: `cargo test -p egret-server --lib committed_openapi_json_is_current`
Expected: FAIL — `openapi.json is stale`.

Run: `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json`
Then: `rtk proxy git diff --stat openapi.json` — expected: one file, a few hundred lines at most; `rtk proxy grep -c '"EngineConflictBody"' openapi.json` ≥ 1 (positive control that the redirect captured JSON, not RTK-filtered text: also `head -c 1 openapi.json` must print `{`).

Run: `cargo test -p egret-server --lib committed_openapi_json_is_current`
Expected: PASS.

- [ ] **Step 4: Regenerate the frontend client**

```bash
cd frontend
npm run gen:api
git status --short src/api/generated
```

Keep only: `src/api/generated/control-plane-engines/control-plane-engines.ts`, `src/api/generated/model/index.ts`, and the new `src/api/generated/model/engineConflictBody.ts`, `engineDependentDto.ts`, `engineDependentKind.ts`. Revert anything else with `git checkout -- <path>` (tracked) or `rm <path>` (untracked). Then confirm the path-param encoding survived: `rtk proxy grep -c "encodeURIComponent" src/api/generated/control-plane-engines/control-plane-engines.ts` must be the same as on `main` (`git show main:frontend/src/api/generated/control-plane-engines/control-plane-engines.ts | rtk proxy grep -c encodeURIComponent`).

Open `engineDependentDto.ts` and note the exact exported names (`EngineDependentDto`, and how `kind` is typed) — Tasks 9–10 import them.

Run (in `frontend/`): `npm run typecheck && npm run lint`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/engines.rs openapi.json frontend/src/api/generated
git commit -m "docs(api): document engine delete/unassign conflicts and regenerate the client

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Prove emptying the Data API trash takes the endpoint's children (spec §3.3, re-scoped)

**Files:**
- Modify: `crates/egret-dataapi/tests/trash.rs` (one test after `restore_cannot_collide_because_the_slot_was_never_released`)

**Interfaces:**
- Consumes: the existing `DELETE /api/ws/{ws}/data-api/endpoints/{id}` route and `trash.rs` fixture (`skip_without_db!`, `Fixture::{create_trashed_endpoint, send, collection}`, fields `db`, `ws_id`).
- Produces: nothing new in production code (Decision 1). The permission half of the spec's purge test already exists: `crates/egret-dataapi/tests/endpoints_routes.rs::deleting_an_endpoint_requires_manage`.

- [ ] **Step 1: Write the test**

```rust
/// Emptying the trash is `DELETE …/endpoints/{id}` — it is what an engine delete's
/// "trashed Data API endpoint" dependent tells the author to do. It must take the
/// endpoint's children with it and leave its access log.
///
/// `compared_against` (0131) references `endpoint_versions` with NO `ON DELETE`
/// action, and its comment says nothing deletes versions. A cascade deletes the
/// referencing and referenced versions in one statement, which NO ACTION allows
/// — this pins that, because RESTRICT would not.
#[tokio::test]
async fn emptying_the_trash_takes_versions_and_alerts_and_keeps_the_access_log() {
    let f = skip_without_db!();
    let id = f
        .create_trashed_endpoint("purge-me", "/purge/me", "disabled")
        .await;
    let v1: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot) \
         VALUES ($1, 1, '{}'::jsonb) RETURNING id",
    )
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .expect("v1");
    let v2: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot, breaking_changes, compared_against) \
         VALUES ($1, 2, '{}'::jsonb, '[]'::jsonb, $2) RETURNING id",
    )
    .bind(id)
    .bind(v1)
    .fetch_one(&f.db.pool)
    .await
    .expect("v2 compared against v1");
    sqlx::query("UPDATE dataapi.endpoints SET live_version_id = $2 WHERE id = $1")
        .bind(id)
        .bind(v2)
        .execute(&f.db.pool)
        .await
        .expect("point at v2");
    let alert: Uuid = sqlx::query_scalar(
        "INSERT INTO automation.schedules \
           (workspace_id, type, name, crontab, validator_type, dataapi_metric, dataapi_window_s, dataapi_endpoint_id) \
         VALUES ($1, 'dataapi_alert', 'purge-watch', '0 9 * * *', 'operator', 'call_count', 3600, $2) \
         RETURNING id",
    )
    .bind(f.ws_id)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .expect("dataapi alert");
    let request_id = format!("purge-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO dataapi.access_log (workspace_id, endpoint_id, status_code, duration_ms, request_id) \
         VALUES ($1, $2, 200, 5, $3)",
    )
    .bind(f.ws_id)
    .bind(id)
    .bind(&request_id)
    .execute(&f.db.pool)
    .await
    .expect("access log row");

    let (status, body) = f
        .send("DELETE", &format!("{}/{id}", f.collection()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let count = |sql: &'static str, bind: Uuid| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(bind)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count("SELECT count(*) FROM dataapi.endpoints WHERE id = $1", id).await, 0);
    assert_eq!(
        count("SELECT count(*) FROM dataapi.endpoint_versions WHERE endpoint_id = $1", id).await,
        0,
        "both versions, compared_against chain included"
    );
    assert_eq!(count("SELECT count(*) FROM automation.schedules WHERE id = $1", alert).await, 0);
    let log_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE request_id = $1")
            .bind(&request_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(log_rows, 1, "the access log outlives the endpoint by design (0123)");
}
```

- [ ] **Step 2: Run it**

Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-dataapi --test trash emptying_the_trash`
Expected: PASS (it pins existing behaviour). **If it fails with `23503` on `endpoint_versions_compared_against_fkey`, that is a real defect in the purge path: stop and report — do not weaken the test.** Then prove the test can fail: temporarily comment out the `DELETE` `send` call, rerun, see the endpoint-count assertion fail, restore it, and confirm with `git diff --stat` that only the new test remains.

Run: `cargo fmt --all -- --check && cargo clippy -p egret-dataapi --all-targets -- -D warnings`

- [ ] **Step 3: Commit**

```bash
git add crates/egret-dataapi/tests/trash.rs
git commit -m "test(dataapi): pin what emptying the trash deletes and keeps

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 9: Frontend — dependents table, scoped delete dialog, blocked dialog, retry state

**Files:**
- Create: `frontend/src/routes/control-plane/EngineDependentsTable.tsx`
- Create: `frontend/src/routes/control-plane/__tests__/EngineDependentsTable.test.tsx`
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx` — `phaseVariant` (~101-117), `EngineActions` (~216-319), header badge (~366)
- Modify: `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx` (new `describe` block)

**Interfaces:**
- Consumes: `EngineConflictBody`, `EngineDependentDto` from `@/api/generated/model/…` (Task 7); `ApiError` (`@/api/client`, `body` holds the parsed JSON); `deleteEngine` (`@/api/controlPlane`); the backend contract of Tasks 4 and 6.
- Produces:
  - `export function dependentsFromError(e: unknown): EngineDependentDto[] | null` — non-null only for `ApiError` with `code === "engine_has_dependents"` and an array `body.dependents`.
  - `export function EngineDependentsTable({ dependents }: { dependents: EngineDependentDto[] })` — columns Workspace / What / Count / Examples / link; kind labels `dataapi_endpoint` → "Data API endpoints", `dataapi_endpoint_trashed` → "Data API endpoints in trash", `schedule` → "Alerts & reports", `dataset` → "Datasets"; links `/explore/{slug}/data-api/endpoints`, `/explore/{slug}/alerts`, `/explore/{slug}/datasets`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/control-plane/__tests__/EngineDependentsTable.test.tsx`:

```tsx
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, test } from "vitest";

import { ApiError } from "@/api/client";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import {
  EngineDependentsTable,
  dependentsFromError,
} from "@/routes/control-plane/EngineDependentsTable";

const DEPENDENTS: EngineDependentDto[] = [
  {
    workspaceId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    workspaceName: "Engineering",
    workspaceSlug: "eng",
    kind: "dataapi_endpoint",
    count: 25,
    sample: ["a", "b", "c", "d", "e"],
  },
  {
    workspaceId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    workspaceName: "Engineering",
    workspaceSlug: "eng",
    kind: "dataapi_endpoint_trashed",
    count: 1,
    sample: ["old-orders"],
  },
  {
    workspaceId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
    workspaceName: "Data",
    workspaceSlug: "data",
    kind: "schedule",
    count: 2,
    sample: ["nightly", "weekly"],
  },
];

describe("dependentsFromError", () => {
  test("reads the list from an engine_has_dependents 409", () => {
    const e = new ApiError(409, "still in use", "engine_has_dependents", {
      error: "still in use",
      code: "engine_has_dependents",
      dependents: DEPENDENTS,
    });
    expect(dependentsFromError(e)).toEqual(DEPENDENTS);
  });

  test("is null for any other error", () => {
    expect(dependentsFromError(new ApiError(409, "c", "constraint_violation", { constraint: "x" }))).toBeNull();
    expect(dependentsFromError(new ApiError(502, "boom", "engine_cluster_delete_failed"))).toBeNull();
    expect(dependentsFromError(new Error("network"))).toBeNull();
  });
});

describe("EngineDependentsTable", () => {
  test("one row per workspace and kind, with counts, examples and a link", () => {
    render(
      <MemoryRouter>
        <EngineDependentsTable dependents={DEPENDENTS} />
      </MemoryRouter>,
    );
    const rows = within(screen.getByRole("table")).getAllByRole("row");
    expect(rows).toHaveLength(4); // header + 3

    expect(within(rows[1]).getByText("Data API endpoints")).toBeInTheDocument();
    expect(within(rows[1]).getByText("25")).toBeInTheDocument();
    // 25 > 5 names shown → the list says it is truncated.
    expect(within(rows[1]).getByText("a, b, c, d, e, …")).toBeInTheDocument();
    expect(within(rows[1]).getByRole("link", { name: "Open" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/endpoints",
    );

    expect(within(rows[2]).getByText("Data API endpoints in trash")).toBeInTheDocument();
    expect(within(rows[3]).getByText("Alerts & reports")).toBeInTheDocument();
    expect(within(rows[3]).getByText("nightly, weekly")).toBeInTheDocument();
    expect(within(rows[3]).getByRole("link", { name: "Open" })).toHaveAttribute(
      "href",
      "/explore/data/alerts",
    );
  });
});
```

Append to `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx` (add `import { ApiError } from "@/api/client";` and `import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";` to its imports):

```tsx
describe("EngineDetail — delete", () => {
  const BLOCKERS: EngineDependentDto[] = [
    {
      workspaceId: WS_A.id,
      workspaceName: WS_A.name,
      workspaceSlug: WS_A.slug,
      kind: "dataapi_endpoint",
      count: 25,
      sample: ["orders-by-day"],
    },
    {
      workspaceId: WS_A.id,
      workspaceName: WS_A.name,
      workspaceSlug: WS_A.slug,
      kind: "dataapi_endpoint_trashed",
      count: 1,
      sample: ["old-orders"],
    },
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  async function openDeleteDialog() {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("button", { name: /more engine actions/i }));
    await user.click(await screen.findByRole("menuitem", { name: /delete engine/i }));
    return { user, dialog: await screen.findByRole("alertdialog") };
  }

  test("states what is destroyed and requires the engine name", async () => {
    const { user, dialog } = await openDeleteDialog();
    expect(within(dialog).getByText(/kubernetes cluster/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/query history and notebook engine access/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/saved queries that use it lose their connection/i)).toBeInTheDocument();

    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    const input = within(dialog).getByPlaceholderText("analytics-trino");
    await user.type(input, "delete");
    expect(confirm).toBeDisabled();
    await user.clear(input);
    await user.type(input, "analytics-trino");
    expect(confirm).toBeEnabled();
  });

  test("a 409 lists the dependents and offers no delete", async () => {
    vi.mocked(api.deleteEngine).mockRejectedValue(
      new ApiError(409, "engine is still in use", "engine_has_dependents", {
        error: "engine is still in use",
        code: "engine_has_dependents",
        dependents: BLOCKERS,
      }),
    );
    const { user, dialog } = await openDeleteDialog();
    await user.type(within(dialog).getByPlaceholderText("analytics-trino"), "analytics-trino");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    const blocked = await screen.findByRole("alertdialog", { name: /still in use/i });
    expect(within(blocked).getByText("Data API endpoints")).toBeInTheDocument();
    expect(within(blocked).getByText("Data API endpoints in trash")).toBeInTheDocument();
    expect(within(blocked).getByText("25")).toBeInTheDocument();
    expect(within(blocked).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(vi.mocked(api.deleteEngine).mock.calls[0][0]).toBe("e1");
  });

  test("a deleting engine shows the retry state and Retry delete deletes again", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      status: {
        phase: "deleting",
        message: "a previous delete did not finish; retry the delete",
        pods: [],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    vi.mocked(api.deleteEngine).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderDetail();
    expect(await screen.findByText("Deleting — retry")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /retry delete/i }));
    await waitFor(() => expect(vi.mocked(api.deleteEngine).mock.calls[0]?.[0]).toBe("e1"));
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run (in `frontend/`): `npx vitest run src/routes/control-plane/__tests__/EngineDependentsTable.test.tsx src/routes/control-plane/__tests__/EngineDetail.test.tsx`
Expected: FAIL — `Failed to resolve import "@/routes/control-plane/EngineDependentsTable"`; after Step 3's first file exists, the three delete tests fail on missing text / placeholder `"analytics-trino"` / `Deleting — retry`.

- [ ] **Step 3: Implement**

Create `frontend/src/routes/control-plane/EngineDependentsTable.tsx`:

```tsx
import { Link } from "react-router-dom";

import { ApiError } from "@/api/client";
import type { EngineConflictBody } from "@/api/generated/model/engineConflictBody";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const KIND_LABEL: Record<string, string> = {
  dataapi_endpoint: "Data API endpoints",
  dataapi_endpoint_trashed: "Data API endpoints in trash",
  schedule: "Alerts & reports",
  dataset: "Datasets",
};

/** Where each kind is managed, under `/explore/{slug}/`. Trashed endpoints live
 * behind the Trash toggle of the same Endpoints page. */
const KIND_PATH: Record<string, string> = {
  dataapi_endpoint: "data-api/endpoints",
  dataapi_endpoint_trashed: "data-api/endpoints",
  schedule: "alerts",
  dataset: "datasets",
};

/** The dependents carried by an engine delete/unassign 409, or `null` for any other error. */
export function dependentsFromError(e: unknown): EngineDependentDto[] | null {
  if (!(e instanceof ApiError) || e.code !== "engine_has_dependents") return null;
  const dependents = (e.body as Partial<EngineConflictBody> | undefined)?.dependents;
  return Array.isArray(dependents) ? dependents : null;
}

/** What still uses an engine's workspace connections, one row per workspace and kind. */
export function EngineDependentsTable({ dependents }: { dependents: EngineDependentDto[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Workspace</TableHead>
          <TableHead>What</TableHead>
          <TableHead className="text-right">Count</TableHead>
          <TableHead>Examples</TableHead>
          <TableHead>
            <span className="sr-only">Link</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {dependents.map((d) => (
          <TableRow key={`${d.workspaceId}:${d.kind}`}>
            <TableCell>{d.workspaceName}</TableCell>
            <TableCell>{KIND_LABEL[d.kind] ?? d.kind}</TableCell>
            <TableCell className="text-right tabular-nums">{d.count}</TableCell>
            <TableCell className="text-muted-foreground">
              {d.sample.join(", ")}
              {d.count > d.sample.length ? ", …" : ""}
            </TableCell>
            <TableCell>
              <Link
                className="text-primary underline-offset-4 hover:underline"
                to={`/explore/${d.workspaceSlug}/${KIND_PATH[d.kind] ?? ""}`}
              >
                Open
              </Link>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
```

In `EngineDetail.tsx`:

1. Import: `import { EngineDependentsTable, dependentsFromError } from "./EngineDependentsTable";` and `import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";`.
2. `phaseVariant`: add `case "deleting":` to the `"warning"` group.
3. Header badge (~366): `<Badge variant={phaseVariant(data.status.phase)}>{data.status.phase === "deleting" ? "Deleting — retry" : data.status.phase}</Badge>`.
4. In `EngineActions`, add state and replace `deleteMutation`:

```tsx
  const [blockedBy, setBlockedBy] = useState<EngineDependentDto[] | null>(null);
  const deleting = engine.status.phase === "deleting";

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteEngine,
    onSuccess: () => navigate("/control-plane/engines"),
    onError: (e) => {
      const dependents = dependentsFromError(e);
      if (dependents) {
        setBlockedBy(dependents);
        return;
      }
      // A failed cluster delete leaves the engine `deleting`: refetch so the
      // header shows the retry state rather than the last live phase.
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      toast.error(e.message ?? "Failed to delete the engine.");
    },
  });
```

5. First child of the returned fragment (before the Restart button):

```tsx
      {deleting && (
        <Button
          variant="destructive"
          size="sm"
          onClick={() => deleteMutation.mutate(engine.id)}
          disabled={deleteMutation.isPending}
        >
          {deleteMutation.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Trash2 className="h-4 w-4" />
          )}
          Retry delete
        </Button>
      )}
```

6. Replace the `<ConfirmDeleteDialog … />` element with:

```tsx
      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete «${engine.name}»?`}
        description="This destroys the engine's Kubernetes cluster. It cannot be undone."
        confirmText={engine.name}
        onConfirm={() => deleteMutation.mutate(engine.id)}
      >
        <ul className="list-disc space-y-1 pl-5">
          <li>Query history and notebook engine access on its workspace connections are removed.</li>
          <li>Saved queries that use it lose their connection.</li>
          <li>
            Data API endpoints (including trashed ones), alerts and datasets that use it block
            the delete — you will be shown where they are.
          </li>
        </ul>
      </ConfirmDeleteDialog>

      <AlertDialog
        open={blockedBy !== null}
        onOpenChange={(open) => {
          if (!open) setBlockedBy(null);
        }}
      >
        <AlertDialogContent className="sm:max-w-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>«{engine.name}» is still in use</AlertDialogTitle>
            <AlertDialogDescription>
              Nothing was deleted. Remove or move these first, then delete the engine again.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {blockedBy && <EngineDependentsTable dependents={blockedBy} />}
          <AlertDialogFooter>
            <AlertDialogCancel>Close</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
```

`ConfirmDeleteDialog` renders `children` inside a `div` (`frontend/src/components/ui/confirm-delete-dialog.tsx:45`), and its input placeholder is `confirmText` — which is what the test types into.

- [ ] **Step 4: Run tests to verify they pass**

Run (in `frontend/`): `npx vitest run src/routes/control-plane/__tests__/EngineDependentsTable.test.tsx src/routes/control-plane/__tests__/EngineDetail.test.tsx`
Expected: PASS, including the pre-existing "Restart lives in the header (single instance) and Delete is in the ⋯ menu".

Run: `npm run lint && npm run typecheck && npm test && npm run build`
Expected: clean (lint warnings that predate the branch are not new failures; compare the warning count with `main`).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/EngineDependentsTable.tsx frontend/src/routes/control-plane/__tests__/EngineDependentsTable.test.tsx frontend/src/routes/control-plane/EngineDetail.tsx frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx
git commit -m "feat(frontend): show what blocks an engine delete and let a failed delete be retried

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task 10: Frontend — the Workspaces tab shows the dependents table on a blocked change

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx` — `WorkspacesCard` (~1401-1513)
- Modify: `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx` — one test in `describe("EngineDetail — Workspaces tab")`

**Interfaces:**
- Consumes: `EngineDependentsTable`, `dependentsFromError` (Task 9); `useControlPlaneEnginesSetEngineWorkspaces` (generated; its `mutate(variables, { onSuccess, onError })`); the PUT contract of Task 6.
- Produces: no new exports. Errors are now handled per `mutate` call (the hook-level `onError` is removed so a real 409 is not also toasted).

- [ ] **Step 1: Write the failing test** (inside `describe("EngineDetail — Workspaces tab", …)`; needs the `ApiError` import added in Task 9)

```tsx
  test("a blocked Remove shows what still uses the engine instead of a toast", async () => {
    setMutate.mockImplementationOnce(
      (_vars: unknown, opts?: { onError?: (e: unknown) => void }) =>
        opts?.onError?.(
          new ApiError(409, "engine is still in use", "engine_has_dependents", {
            error: "engine is still in use",
            code: "engine_has_dependents",
            dependents: [
              {
                workspaceId: WS_A.id,
                workspaceName: WS_A.name,
                workspaceSlug: WS_A.slug,
                kind: "dataset",
                count: 3,
                sample: ["orders", "refunds", "revenue"],
              },
            ],
          }),
        ),
    );
    const user = await openWorkspacesTab();
    await user.click(screen.getByRole("button", { name: "Remove" }));

    expect(await screen.findByText(/not removed/i)).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getByText("Datasets")).toBeInTheDocument();
    expect(within(table).getByText("orders, refunds, revenue")).toBeInTheDocument();
    // Still assigned: the list row is untouched.
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run to verify it fails**

Run (in `frontend/`): `npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx -t "blocked Remove"`
Expected: FAIL — `Unable to find an element with the text: /not removed/i` (today `removeWorkspace` passes no `onError`).

- [ ] **Step 3: Implement** — in `WorkspacesCard`, replace the `setMut`/`addWorkspace`/`removeWorkspace` block with:

```tsx
  const [blockedBy, setBlockedBy] = useState<EngineDependentDto[] | null>(null);

  const setMut = useControlPlaneEnginesSetEngineWorkspaces({
    mutation: {
      onSuccess: () => {
        setBlockedBy(null);
        queryClient.invalidateQueries({
          queryKey: getControlPlaneEnginesListEngineWorkspacesQueryKey(engineId),
        });
      },
    },
  });

  // Per call rather than on the hook, so a 409 renders the dependents table and
  // is not ALSO toasted by a hook-level handler.
  const onSetError = (e: unknown) => {
    const dependents = dependentsFromError(e);
    if (dependents) {
      setBlockedBy(dependents);
      return;
    }
    toast.error((e as { message?: string })?.message ?? "Failed to update workspaces.");
  };

  const addWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds, wsId] } },
      { onSuccess: () => toast.success("Workspace assigned"), onError: onSetError },
    );
  const removeWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds].filter((x) => x !== wsId) } },
      { onSuccess: () => toast.success("Workspace removed"), onError: onSetError },
    );
```

and insert, between the header `div` and the list `div`:

```tsx
      {blockedBy && (
        <div className="space-y-2 rounded-md border border-destructive/50 p-3">
          <p className="text-sm font-medium text-destructive">
            Not removed — the workspace still uses this engine
          </p>
          <p className="text-sm text-muted-foreground">
            Remove or move these first, then try again.
          </p>
          <EngineDependentsTable dependents={blockedBy} />
        </div>
      )}
```

- [ ] **Step 4: Run tests to verify they pass**

Run (in `frontend/`): `npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx`
Expected: PASS — the new test plus "Remove drops the workspace from the assignment set" and "Add dialog offers only active, unassigned workspaces and assigns on click" (they read `setMutate.mock.calls[0][0]`, unchanged by the second argument).

Run: `npm run lint && npm run typecheck && npm test && npm run build`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/EngineDetail.tsx frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx
git commit -m "feat(frontend): show what blocks unassigning an engine from a workspace

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Full gate and the live check on kind

**Files:** none changed (unless the gate finds something — then fix in a new commit and rerun).

**Interfaces:** consumes everything above.

- [ ] **Step 1: Rust gate — one run, nothing else running**

Check nothing else is testing: `ps -axo pid,ppid,args | awk -v me=$$ '$1!=me && $2!=me && /cargo/ && /test/' | wc -l` → `0`.

Run: `just check`
(= `./deploy/prune-target.sh`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `just test-shipping-resolution`, then `env -u EGRET_ALLOW_DB_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test --workspace`.)
Expected: exit 0. Record the `test result:` totals and compare the **set of test names** with a run on `main` (not the arithmetic): the new names are the 7+2 `delete_engine_*`, 3 assignment tests, 5 `tests/dependents.rs`, 1 `tests/error_mapping.rs`, 3+2 unit tests in `egret-engine-mgmt`, 3 in `egret-provisioner`, 1 in `egret-dataapi/tests/trash.rs`. Any failure outside these is **a hypothesis, not "pre-existing"**, until it is reproduced on `main`.

- [ ] **Step 2: Frontend gate**

Run (in `frontend/`): `npm run lint && npm run typecheck && npm test && npm run build`
Expected: clean.

- [ ] **Step 3: Live check on kind (read-only by construction)**

The spec's claim to verify: `DELETE demo-trino` answers 409 listing the endpoints and alerts, not 500. **A delete with zero dependents would destroy the cluster — so count first, and only proceed if the count is non-zero.**

```bash
just deploy-kind            # or deploy-kind-oidc if the running release has oidc.enabled (helm get values egret -n egret-system)
kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -At -c "
  SELECT e.name, count(DISTINCT ep.id) AS endpoints, count(DISTINCT s.id) AS schedules, count(DISTINCT ds.id) AS datasets
    FROM engine_mgmt.engines e
    JOIN query.databases d ON d.engine_id = e.id
    LEFT JOIN dataapi.endpoints ep ON ep.database_id = d.id
    LEFT JOIN automation.schedules s ON s.database_id = d.id
    LEFT JOIN analytics.datasets ds ON ds.database_id = d.id
   WHERE e.name = 'demo-trino' GROUP BY e.name"
```

Proceed only if that prints a row with at least one non-zero count (the spec expects 26 endpoints). Then, in the browser (Playwright or by hand; login `admin@egret.local` / `Admin12345!`): Control Plane → Engines → `demo-trino` → ⋯ → Delete engine → type `demo-trino` → Delete. Expected: the "«demo-trino» is still in use" dialog with rows whose counts match the SQL above; the engine page still loads; `kubectl get deploy -n egret-engines | grep demo-trino` still lists its Deployments; `kubectl logs -n egret-system deploy/egret --since=5m | grep -c "engine-mgmt database error"` is `0` (no backstop fired). Record the screenshot path and the counts in the report. Report this as a **manual live check**, not an automated test.

- [ ] **Step 4: Report and stop**

No PR, no merge. Report: commits (`git log --oneline main..`), the gate totals and name-set diff, the live-check counts, and every place the code disagreed with this plan.

---

## Self-review

**Spec coverage (Part A):**
- §3.1 dependents (four blocking kinds, trashed reported separately, cascades/set-null not blocking, FK-tree guard) → Task 2.
- §3.2 steps 1–8 → Task 4 (1–5, 7–8, secret rows) + Task 5 (Ranger profile) + Decision 2 (step 6 is not one transaction); D5/D6 → Task 1; backstop logging + 23503/23514 → Task 3.
- §3.3 purge → Decision 1 + Task 8 (existing route proven; no new route).
- §3.4 unassign/PUT/404 → Task 6 (+ Decision 3).
- §3.5 UI: scoped dialog + type-the-name + dependents table with links + no delete button + 502 toast + `deleting` retry → Task 9; Workspaces tab 409 → Task 10; stored-status visibility → Task 4 (`get_engine`) + Decision 9.
- §5 Part A tests: per-kind 409 with engine row and fake port untouched (Task 4), happy path with secrets/profile/minted client (Tasks 4–5), unassign per kind + mixed PUT (Task 6), FK guard (Task 2), cluster 404/500/retry (Tasks 1, 4), error mapping with SQLSTATE in the log (Task 3), purge (Task 8; permission test already exists), frontend table/retry/workspaces (Tasks 9–10), live on kind (Task 11).
- Not in Part A by instruction: the `status` CHECK, random internal secrets, create compensation, update rollback, Trino rematerialize, `remove_catalog` ordering, redaction, catalog-kind CHECK (all Part B).

**Names used across tasks:** `EngineDependentKind`, `EngineDependentDto`, `EngineConflictBody`, `DeleteEngineOutcome::{Deleted, NotFound, Blocked}`, `ChildOnDelete`, `DatabaseChildFk`, `QUERY_DATABASES_CHILD_FKS` (Task 2); `EngineHttpError::{HasDependents, ConstraintViolation, ClusterDeleteFailed}`, `db_error` (Task 3); `ENGINE_STATUS_DELETING`, `cleanup_deleted_engine` (Task 4); `seed_ranger_profile`, `service_exists` test helpers (Task 5); `missing_workspace_ids` (Task 6); `dependentsFromError`, `EngineDependentsTable` (Task 9). Wire codes: `engine_has_dependents`, `constraint_violation`, `engine_cluster_delete_failed`.
