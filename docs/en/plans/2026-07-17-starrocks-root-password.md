# StarRocks Root Password Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Every egret-provisioned StarRocks FE gets a minted `root` password, set in-cluster via the operator's initPassword mechanism (Secret + ConfigMap + Job), stored as an egret secret, and used by egret's own FE-bootstrap connections. Secures the passwordless FE root.

**Architecture:** Replicate the kube-starrocks Helm `initPassword` (3 resources applied directly by egret's provisioner — no `KubeClient` change, `apply(&Value)` derives the GVK). Mint + store the password like the delegator secret. Thread it into the bootstrap connection.

**Tech Stack:** Rust (`egret-orchestrator`, `egret-engine-mgmt`, `egret-engine-mgmt-api`), `batch/v1` Job, `mysql_async`.

**Spec:** `docs/en/specs/2026-07-17-starrocks-root-password-design.md`. **Branch:** `feat/starrocks-root-password` (off `main`).

**Reference anchors:** the operator's `fe_initpwd.sh` at `starrocks-kubernetes-operator/helm-charts/charts/kube-starrocks/charts/starrocks/templates/init-pwd/{job,configmap}.yaml` (copy the script verbatim). `crates/egret-orchestrator/src/starrocks.rs`: `starrocks_fe_configmap`@88 (ConfigMap shape), `apply_all`@589 (provisioner apply seam), `delete` (cleanup). `crates/egret-engine-mgmt/src/engines.rs`: `ensure_delegator_secret`@1388 (`(Uuid,String)` mint/reuse keyed off a `*_secret_id` column), `build_starrocks_provision_req`@~1900 (resolves secrets → `StarRocksConfig`), `spawn_starrocks_catalog_wire` (the FE root connection), `create_starrocks_engine`. `StarRocksConfig` in `egret-engine-mgmt-api/src/lib.rs`.

---

## Task 1: initpwd resource builders (Secret + ConfigMap + Job)

**Files:** `crates/egret-orchestrator/src/starrocks.rs`.

- [ ] **Step 1: Failing tests** (`mod tests`): `initpwd_secret_shape` (`starrocks_root_password_secret(spec)` → `{name}-initpwd-secret` Opaque, `stringData.password` = the spec's root password; `None` when the spec has no root password); `initpwd_configmap_carries_script` (`starrocks_initpwd_configmap(spec)` → `{name}-initpwd-shell` with `data["fe_initpwd.sh"]` containing `SET PASSWORD = PASSWORD('$INIT_PWD')` + the `1045`/`1064`-as-success idempotency branch); `initpwd_job_shape` (`starrocks_initpwd_job(spec)` → `batch/v1` Job `{name}-initpwd`: FE image, `command:[/bin/bash]`, `args:[/opt/starrocks/fe_initpwd.sh, {name}-fe-service.{ns}, "9030"]`, `env INIT_PWD` from `secretKeyRef {name}-initpwd-secret/password`, the script volume-mounted from the ConfigMap, `restartPolicy: OnFailure`, `backoffLimit` >= 5). All three return `None`/absent when `sr.root_password` is unset. Run → FAIL.

- [ ] **Step 2: Implement the three builders.** Copy `fe_initpwd.sh` verbatim from the operator template (idempotent on 1045/1064). The Job mounts the ConfigMap script at `/opt/starrocks/fe_initpwd.sh` (a `configMap` volume + `volumeMount`), `INIT_PWD` from the secret, targets the FE **service** DNS (`{name}-fe-service.{ns}`) so it works once any FE pod is up; the script's `--connect-timeout` + `backoffLimit`/`OnFailure` retry until the FE accepts connections. Gate each on `spec.starrocks.as_ref().and_then(|s| s.root_password.as_ref())`.

- [ ] **Step 3: Run → PASS.** fmt + clippy clean; `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 4: Commit.** `feat(orchestrator): StarRocks initpwd Secret/ConfigMap/Job builders`.

---

## Task 2: Provisioner applies + cleans up the initpwd resources

**Files:** `crates/egret-orchestrator/src/starrocks.rs` (`apply_all`@589, `delete`).

- [ ] **Step 1: Failing test** (FakeKube): `provisioner_applies_initpwd_when_root_password_set` — provision a spec with a root password → the initpwd Secret + ConfigMap + Job are among the applied objects; a spec WITHOUT a root password → none applied (legacy path). Run → FAIL.

- [ ] **Step 2: Apply in `apply_all`** (@589, after the FE/CN ConfigMaps, before/after the CR — order-independent, but apply the Secret + ConfigMap before the Job): when `starrocks_root_password_secret(spec)` is `Some`, `self.kube.apply(&secret)?; apply(&configmap)?; apply(&job)?`. Both `provision` and `update` route through `apply_all` (verify), so update re-applies the Job (idempotent — the script no-ops if the password is already set).

- [ ] **Step 3: Cleanup in `delete`** — best-effort delete `{name}-initpwd-secret` / `-initpwd-shell` / `{name}-initpwd` (Job), mirroring the existing ranger/ssl/cn orphan cleanup.

- [ ] **Step 4: Run → PASS.** fmt + clippy; `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 5: Commit.** `feat(orchestrator): apply + clean up StarRocks initpwd resources`.

---

## Task 3: Contract + mint the root password secret (secure-by-default)

**Files:** `crates/egret-engine-mgmt-api/src/lib.rs` (`StarRocksConfig`), `crates/egret-engine-mgmt/src/engines.rs`, a new migration for `root_secret_id`.

- [ ] **Step 1: Failing tests.** `build_starrocks_provision_req_threads_root_password` (a stored engine with a `root_secret_id` → the assembled `StarRocksConfig.root_password` is the resolved plaintext) — DB-gated; `ensure_root_password_secret_is_idempotent` (second call returns the same secret id/plaintext). Run → FAIL.

- [ ] **Step 2: Contract.** Add `root_password: Option<String>` to `StarRocksConfig` (redact in the manual `Debug` like `storage_secret_key`). Update every `StarRocksConfig { … }` literal (grep) with `root_password: None` / `..Default::default()`.

- [ ] **Step 3: Migration + column.** Add `root_secret_id UUID NULL` to the engines table (new migration file, mirroring `delegator_secret_id`); touch `metastore/src/lib.rs` to force the embedded migrator to re-embed (see the sqlx-migration-staleness note). Add `root_secret_id` to `store::Engine` + its row mapping.

- [ ] **Step 4: `ensure_root_password_secret(st, engine) -> Result<(Uuid, String), CpError>`** mirroring `ensure_delegator_secret`@1388 (reuse the existing global-secret mint/store; a fresh strong random password on first call, keyed off `engine.root_secret_id`).

- [ ] **Step 5: Wire create + provision-req.** In `create_starrocks_engine`, always (secure-by-default) `ensure_root_password_secret` after the engine row exists + persist `root_secret_id`. In `build_starrocks_provision_req`, resolve the root password from the stored secret into `StarRocksConfig.root_password` (so update re-applies the same one).

- [ ] **Step 6: Run → PASS.** fmt + clippy; `DATABASE_URL=…egret_test cargo test -p egret-engine-mgmt -p egret-engine-mgmt-api` (2 pre-existing assignment failures excepted).

- [ ] **Step 7: Commit.** `feat(engine): mint + store StarRocks root password (secure-by-default)`.

---

## Task 4: egret's FE-bootstrap connection uses the root password

**Files:** `crates/egret-engine-mgmt/src/engines.rs` (`spawn_starrocks_catalog_wire` + its `StarRocksDelegationBootstrap`/connection).

- [ ] **Step 1: Failing/adjust test.** The wire connection is dev-DNS-blocked, so a live test isn't possible; add/adjust a unit test on the connection-opts builder (grep how the bootstrap builds its `mysql_async` opts) asserting the root password is set on the connection when the engine has one, absent otherwise. Run → FAIL.

- [ ] **Step 2: Thread the password.** Where `spawn_starrocks_catalog_wire` (or `build_starrocks_provision_req`'s bootstrap) builds the FE `mysql_async` connection as `root`, set `OptsBuilder.pass(Some(root_password))` when the engine has a root secret; keep `pass(None)` (passwordless) for legacy engines (no `root_secret_id`). Pass the resolved root password through the `StarRocksDelegationBootstrap` (add a `root_password: Option<String>` field) from `build_starrocks_provision_req`.

- [ ] **Step 3: Run → PASS.** fmt + clippy; `cargo test -p egret-engine-mgmt`.

- [ ] **Step 4: Commit.** `feat(engine): use the minted root password for FE bootstrap connections`.

---

## Task 5: Full CI + live smoke (verifiable in dev — Job is in-cluster)

- [ ] **Step 1: Full CI.** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` (vendored `mysql_async` excepted); `DATABASE_URL=…egret_test cargo test --workspace` (2 pre-existing assignment failures unrelated).

- [ ] **Step 2: Rebuild egret + provision a StarRocks** (3.4). Wait for FE Ready + the `{name}-initpwd` Job to Complete (`kubectl -n egret-engines get job {name}-initpwd`).

- [ ] **Step 3: Prove the password is set** — `kubectl exec {name}-fe-0 -c fe -- mysql -h127.0.0.1 -P9030 -uroot -e "SELECT 1"` (no password) is REJECTED; `mysql -uroot -p<minted> -e "SELECT 1"` succeeds. Retrieve `<minted>` from the egret secret store / the `{name}-initpwd-secret`.

- [ ] **Step 4: Idempotency** — trigger an update (e.g. rescale) → the Job re-applies → it exits 0 (script treats already-set as success), no provisioning failure.

- [ ] **Step 5: Report** exactly what ran; note the passwordless→rejected / minted→accepted proof.

---

## Self-Review

- **Spec coverage:** builders → T1; apply/cleanup → T2; contract+mint+migration → T3; bootstrap connection → T4; live → T5. Covered.
- **Type consistency:** `starrocks_root_password_secret`/`_initpwd_configmap`/`_initpwd_job` (T1) consumed by `apply_all` (T2); `StarRocksConfig.root_password` (T3) fed by `ensure_root_password_secret` + `root_secret_id` + `build_starrocks_provision_req` (T3) and read by T1's builders + T4's connection. Mirrors the delegator-secret path throughout.
- **No placeholders:** the operator's verbatim `fe_initpwd.sh`, the 3 resources, `KubeClient::apply`, the `ensure_delegator_secret` mint pattern, the migration, and the bootstrap-connection thread are named.
- **In-cluster ⇒ dev-verifiable** (T5 proves it in dev, unlike the delegation bootstrap). Secure-by-default; legacy-safe (no `root_secret_id` ⇒ passwordless as before).
