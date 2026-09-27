# StarRocks FE metadata must survive a pod replacement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** stop a StarRocks FE from losing everything it knows whenever its pod is replaced.

**The defect, verified at source and live before this plan was written.** egret emits a `storageVolumes` entry only in the TLS branch (`crates/egret-provisioner/src/starrocks.rs:660`), so nothing covers the FE metadata path. The live CR's only entry is the TLS keystore, and the running pod carries `fe-meta {"emptyDir": {}}` and `fe-log {"emptyDir": {}}` — operator defaults. So **any** pod replacement — a Ranger rebind, an image change, a restart, a node drain — wipes the FE's metadata. PR #597's live gate hit exactly this: the FE came back with `first start fe with meta not exist`, `SHOW CATALOGS` down to `default_catalog`, `root` with no password, and the `analyst` user gone.

**Architecture:** the CRD's `storageVolumes` items accept either a `hostPath` or a PVC (`storageClassName` + `storageSize`). The FE meta path is `/opt/starrocks/fe/meta`.

**Tech stack:** Rust workspace (sqlx 0.9 — `query()` takes `&'static str`, never `format!` SQL), StarRocks operator CRD, kind cluster `kind-egret` (default StorageClass `standard`, `rancher.io/local-path`).

## Global Constraints

- **Build isolation is mandatory.** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-sp4a/target-check`. Never `.../egret/target`: another session builds the MAIN checkout there, so cargo links the wrong artefacts and reports `E0432 no X in the root` for symbols that plainly exist — a failure that looks exactly like a broken branch and is not one.
- **Never launch a background task and wait on it.** Three agents on this programme stalled doing exactly that and delivered nothing. Foreground only.
- sqlx 0.9: `query()` takes `&'static str`. No `format!`-built SQL.
- OpenAPI must satisfy `committed_openapi_json_is_current`; any frontend client change must be exactly `npm run gen:api` output, never bare `orval`, committed exactly as produced.
- `cargo build --workspace` and `cargo fmt --all -- --check` clean.
- Tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only — **never** the dev database `egret`. DB-gated tests skip silently without it, so always `env -u EGRET_ALLOW_DB_SKIP`. Run `egret-engine-mgmt` with `RUST_TEST_THREADS=4`: at default parallelism the suite exhausts Postgres connections and fails ~8 tests with `PoolTimedOut`, which is not behaviour.
- Standing project constraints: engine deletion never deletes Ranger profiles, it only unbinds. Do not modify `demo-trino`'s data. Never `git stash`.
- Every new test is verified by mutation: break the fix, watch that specific test die, restore with `cat backup > file` (`cp` is aliased to `cp -i` and silently refuses to overwrite), confirm `git status --porcelain` is empty. **Check *why* each mutation died** — a mutation that dies for the wrong reason proves nothing, and that happened twice on this programme.

**USER DECISION (2026-09-26):** every engine gets the PVC, old and new alike. Existing engines therefore lose FE metadata **once**, on their first roll after the upgrade, and after that the problem is gone. That one-time loss takes catalogs **and** StarRocks-side users and roles; `POST /catalogs/{id}/reapply` restores only the catalogs. It must be documented where an operator will actually see it.

---

### Task 1: Persist the FE metadata volume

**Files:** Modify `crates/egret-provisioner/src/starrocks.rs`.

- [ ] Emit a `storageVolumes` entry for `/opt/starrocks/fe/meta` backed by a **PVC** (`storageClassName` + `storageSize`), not a `hostPath`.
  **Why PVC and not hostPath, given the TLS keystore uses hostPath:** the keystore is rebuilt from a Secret on every boot, so losing it costs nothing; metadata is the opposite. A `hostPath` survives a pod restart on the same node and is silently destroyed the moment the pod is rescheduled elsewhere — which converts today's loud, reproducible failure into a rare one that looks like corruption. Do not follow the keystore's precedent here.
- [ ] Make the class and the size configurable through the existing StarRocks config surface, with defaults that work on a cluster that has a default StorageClass. Do not invent a new top-level config surface if one already exists — find it first.
- [ ] **Enumerate every `emptyDir` the operator defaults for this cluster** (`fe-meta`, `fe-log`, and whatever BE/CN carry) and state, per volume, whether it needs persistence and why. Do not silently fix one and leave its siblings — that is the defect class this programme has shipped twelve times. `fe-log` and a CN cache are plausibly fine on an `emptyDir`; say so explicitly rather than by omission.
- [ ] Tests: assert the rendered FE spec carries a meta volume at the correct path with a PVC (not a hostPath), and that the TLS keystore entry still renders alongside it rather than being replaced. The second half is the assertion that catches a careless overwrite of the array.

### Task 2: Prove it survives a pod replacement, live

The only test that matters. Unit tests can only show what egret *renders*.

**Files:** Create `docs/en/plans/2026-09-26-fe-meta-live-result.md`.

- [ ] Deploy to kind. **All six helm values are required** — `oidc.enabled`, `ingress.enabled`, `kernel.launcher=kubernetes`, `backend.replicas=2`, `sharedStore.enabled`, **and `ranger.enabled=true`**. The chart defaults `ranger.enabled` to `false`; dropping it makes every engine deny every query, which looks exactly like "the policies work" while measuring nothing. That has already happened once here.
- [ ] Verify by the pod, never by helm's report: the serving pods' `imageID` matches the image just loaded, and `EGRET_RANGER__ENABLED=true` read from inside a running pod.
- [ ] Create a StarRocks engine, give it a catalog and a non-root user, then **delete the FE pod** and confirm after it returns: the catalog is still listed, the user still exists, and the log does **not** say `first start fe with meta not exist`. Record the verbatim output of each check.
- [ ] **Change nothing else in that step.** PR #597's C1 hid precisely because the apply under test also changed a mount list, so the pod rolled for an unrelated reason and the mechanism under test was never exercised. Delete the pod and change nothing, or the measurement proves nothing.
- [ ] Confirm the PVC is **retained** across the replacement (the same volume is rebound, not a fresh one) — a new empty PVC each time would pass a naive "the pod came back" check while losing the data anyway.
- [ ] Record the one-time migration loss as observed, not as predicted: take an existing `emptyDir` engine through the upgrade and record what it actually loses.
- [ ] Anything that fails is recorded as FAILED with verbatim output. Partial honest results are the success condition; a run reporting cases it did not observe cannot be falsified.

### Task 3: Say it where an operator will see it

- [ ] Document the one-time metadata loss in the place an operator reads before upgrading, not only in a plan file. Name what is lost (catalogs, StarRocks users and roles), what recovers it (`POST /catalogs/{id}/reapply` for catalogs only), and what does not.
- [ ] Note that `fe-log` stays on an `emptyDir` by design, so logs do not survive a pod replacement — if that is the decision Task 1 reached.
