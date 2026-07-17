# StarRocks Root Password (secure the passwordless FE root) — Design

**Date:** 2026-07-17 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-root-password` (off `main`)
**Program:** StarRocks-parity/hardening. The full live smoke found the StarRocks FE `root` account is **passwordless on :9030** — egret never sets a root password (only the `egret_delegator` service user gets one). Fine for a kind dev cluster, a real hole in prod. This slice sets a minted root password via the operator's in-cluster init mechanism.

> Vietnamese reference: `docs/vi/specs/2026-07-17-starrocks-root-password-design.md`.

## Problem

A freshly-provisioned StarRocks FE has `root@%` with **no password** (`SHOW ALL AUTHENTICATION` → `root | No | MYSQL_NATIVE_PASSWORD | NULL`). Anyone who can reach the FE query port (`:9030`) logs in as root — full cluster control. egret's provisioning never closes this (the code comment at `engines.rs:2621` explicitly connects "as root with no password (dev-cluster default)").

## Goal

Every egret-provisioned StarRocks FE gets a **minted root password**, set **in-cluster** (so it works in dev *and* prod, unlike the delegation bootstrap which is blocked by the egret-host→FE-DNS gap), stored as an egret secret. egret's own FE-bootstrap connections use the stored password.

Proven live (in-cluster, verifiable in dev): after provision, `mysql -uroot` (no password) is REJECTED and `mysql -uroot -p<minted>` succeeds.

## Non-goals

- **No UI** — this is automatic hardening (secure-by-default), not an admin toggle. (A future "rotate root password" action is out of scope.)
- **No change to the query/delegation data path** — delegated queries use the JWT / `egret_delegator` impersonation, not root; securing root doesn't touch them.
- **No retroactive fix for existing engines** — engines provisioned before this slice have no root secret and stay passwordless until re-provisioned; the change is additive (absence of a root secret = legacy passwordless behavior).

## Background — the operator's in-cluster init mechanism (verified)

The kube-starrocks Helm chart implements `initPassword` as three resources (NOT a `StarRocksCluster` CR field), which egret replicates by applying them directly:
- **Secret** `{name}-initpwd-secret` — `data.password = <b64 minted>`.
- **ConfigMap** `{name}-initpwd-shell` — `fe_initpwd.sh`, a bash script that runs `SET PASSWORD = PASSWORD('$INIT_PWD')` via `mysql --connect-timeout 30 -h $1 -P $2 -u root` and treats error **1045** (auth failed = already changed) / **1064** as success (idempotent — safe to re-run).
- **Job** `{name}-initpwd` — runs the FE image + `fe_initpwd.sh <fe-host> 9030`, `INIT_PWD` env from the Secret. Runs **in-cluster**, so the FE service DNS resolves (the delegation-bootstrap DNS blocker does NOT apply).

egret's `KubeClient::apply(&Value)` derives the `ApiResource` from `apiVersion`+`kind`, so it applies a `batch/v1` Job / `v1` Secret / `v1` ConfigMap with no client change.

## Architecture — components

### A. Provisioning: mint + apply the init-password resources (`egret-orchestrator/src/starrocks.rs`)
Add builders, gated on a `root_password` on the StarRocks spec (see §C):
1. `starrocks_root_password_secret(spec)` → `{name}-initpwd-secret` Opaque Secret, `stringData.password = <minted>`.
2. `starrocks_initpwd_configmap(spec)` → `{name}-initpwd-shell` ConfigMap with `fe_initpwd.sh` (the operator's script verbatim, idempotent).
3. `starrocks_initpwd_job(spec)` → `{name}-initpwd` `batch/v1` Job: FE image, `command: [/bin/bash]`, `args: [/opt/starrocks/fe_initpwd.sh, {name}-fe-service.{ns}, "9030"]`, `env INIT_PWD` from `secretKeyRef {name}-initpwd-secret/password`, the script mounted from the ConfigMap, `restartPolicy: OnFailure`, `backoffLimit` a few. (Target the FE **service** DNS; the Job retries via the script's `--connect-timeout` + Job backoff until the FE is up.)
4. In `StarRocksProvisioner::provision` (and `update`, idempotently) apply the Secret + ConfigMap + Job when the spec carries a root password. Clean them up in `delete` (best-effort, like the ranger/ssl/cn orphan cleanup).

### B. egret's FE-bootstrap connection uses the stored root password (`egret-engine-mgmt/src/engines.rs`)
egret's post-Ready `spawn_starrocks_catalog_wire` (external-catalog + delegation bootstrap) connects to the FE **as root**. Once root has a password, that connection must present it. Thread the minted root password into the bootstrap connection (`mysql_async` `OptsBuilder.pass(...)`) when the engine has a root secret; keep the passwordless path for legacy engines. (This path is dev-DNS-blocked regardless, but must be correct for prod.)

### C. Contract + minting (`egret-engine-mgmt-api` + `egret-engine-mgmt/src/engines.rs`)
- Add `root_password: Option<String>` to the CR-building `StarRocksConfig` (the minted plaintext, redacted in `Debug` like `storage_secret_key`).
- In `create_starrocks_engine`: mint a root password + store it as an egret secret (reuse the secret-store pattern that mints `ensure_delegator_secret`); record its id on the engine (a `root_secret_id`, mirroring `delegator_secret_id`). Secure-by-default: always mint for a new StarRocks engine. Pass the plaintext into `StarRocksConfig.root_password` for the provisioner.
- `build_starrocks_provision_req` resolves the root password from the stored secret (like the delegator secret) so update re-applies the same one.

## Data flow

Create StarRocks → mint root password → store egret secret (`root_secret_id`) → provisioner applies the CR + FE ConfigMap + **initpwd Secret/ConfigMap/Job** → the Job (in-cluster) sets `root`'s password once the FE is up (idempotent) → egret's later root connections use the stored password. Query/delegation paths unchanged.

## Testing strategy

- **Unit (CI):** `starrocks_initpwd_job`/`_configmap`/`_secret` shapes (Job runs `fe_initpwd.sh` against `{name}-fe-service` w/ `INIT_PWD` from the secret; ConfigMap carries the idempotent script; secret carries the password); the provisioner applies all three when a root password is set and none when absent (legacy); `build_starrocks_provision_req` threads `root_password` from the stored secret; the bootstrap connection uses the password when present.
- **Live (controller-driven, VERIFIABLE IN DEV — the Job is in-cluster):** provision a StarRocks → the `{name}-initpwd` Job completes → `mysql -uroot` (no password) is REJECTED, `mysql -uroot -p<minted>` succeeds. Confirm idempotency (the Job re-run on update exits 0).

## Live-only risks (validated at smoke)

1. **Job timing** — the Job may start before the FE query port is up; the script's `--connect-timeout` + Job `backoffLimit`/`OnFailure` must retry until Ready (mirror the operator's behavior).
2. **`SET PASSWORD` vs `ALTER USER`** — the operator uses `SET PASSWORD = PASSWORD(...)`; confirm it applies on the 3.4 image (it does per the operator's own script; use it verbatim).
3. **Re-provision idempotency** — the script treats 1045 (already-set) as success, so an update that re-applies the Job doesn't fail; confirm.

## Work breakdown (for the plan)

1. **initpwd builders** — `starrocks_root_password_secret` / `_initpwd_configmap` / `_initpwd_job` + tests.
2. **Provisioner apply/cleanup** — apply the three on provision/update, clean up on delete; tests.
3. **Contract + minting** — `StarRocksConfig.root_password` + mint/store the secret at create + `root_secret_id` + `build_starrocks_provision_req` resolution; tests.
4. **Bootstrap connection** — thread the root password into `spawn_starrocks_catalog_wire`'s FE connection; tests.
5. **Live smoke** — Job sets the password in-cluster; passwordless rejected, minted accepted; idempotent.

## Self-review

- **Scope focused:** secure the passwordless root via the operator's own in-cluster Job mechanism + store/use the secret; no UI, no data-path change, additive for legacy engines. ✔
- **No placeholders:** the exact three resources, the idempotent `fe_initpwd.sh`, `KubeClient::apply`, the mint/secret pattern (`ensure_delegator_secret`), and the bootstrap-connection thread are named. ✔
- **In-cluster ⇒ dev-verifiable:** unlike the delegation bootstrap, the Job runs in-cluster, so the live smoke actually proves it in dev. ✔
- **Ambiguity:** secure-by-default (always mint); `SET PASSWORD` via the operator's idempotent script; target the FE service DNS. ✔
