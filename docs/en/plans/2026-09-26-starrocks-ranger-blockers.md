# StarRocks + Ranger blockers — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** close the three pre-existing defects that make SP4a's StarRocks mask and row-filter cases impossible to verify live.

**Why now:** SP4a (PR #596, merged `d393a87b`) shipped with `S3`/`S4` — StarRocks masking and row filtering — **never observed on a real engine**. D5's per-column tag expansion has unit tests and a verified policy layer, and nothing else. These three defects are what stand between that and a measurement. None was introduced by SP4a.

**Tech stack:** Rust workspace (axum, sqlx 0.9 — `query()` takes `&'static str`, never `format!` SQL), StarRocks operator CRD, Apache Ranger, kind cluster `kind-egret`.

## Global Constraints

- **Build isolation is mandatory.** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-sp4a/target-check`. Never `.../egret/target`: another session builds the MAIN checkout there, so cargo links the wrong artefacts and reports `E0432 no X in the root` for symbols that plainly exist — a failure that looks exactly like a broken branch and is not one.
- sqlx 0.9: `query()` takes `&'static str`. No `format!`-built SQL.
- OpenAPI must satisfy `committed_openapi_json_is_current`; any frontend client change must be exactly `npm run gen:api` output, never bare `orval`, committed exactly as produced.
- `cargo build --workspace` and `cargo fmt --all -- --check` clean.
- Tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only — **never** the dev database `egret`. DB-gated tests skip silently without it, so always `env -u EGRET_ALLOW_DB_SKIP`. One database is shared across a binary's concurrent tests, so unscoped counts are racy; scope to ids you create, but never weaken an assertion to do it. Run everything in the FOREGROUND.
- Standing project constraints: engine deletion never deletes Ranger profiles, it only unbinds. Do not modify `demo-trino`'s data. Never `git stash`.
- Every new test is verified by mutation: break the fix, watch that specific test die, restore with `cat backup > file` (`cp` is aliased to `cp -i` and silently refuses to overwrite), confirm `git status --porcelain` is empty. **Check *why* each mutation failed** — a mutation that dies for the wrong reason proves nothing, and that happened twice during SP4a.

---

### Task 1: The Ranger ConfigMap must not shadow `fe.conf`

**Verified at source before writing this plan.** `crates/egret-provisioner/src/starrocks.rs:533-538` mounts `{name}-ranger-cm` at `/opt/starrocks/fe/conf` — the **entire** config directory. The ConfigMap carries exactly two keys (`ranger-starrocks-security.xml`, `ranger-starrocks-audit.xml`, built at `:177-201`), so the mount replaces everything the image ships there, `fe.conf` included, and the FE cannot boot. Observed live during SP4a's gate as `ln: … Read-only file system`, followed by an FE that never started and lost its metadata.

**Files:** Modify `crates/egret-provisioner/src/starrocks.rs`.

- [ ] Replace the single directory mount with one `configMaps` entry per key, each using `subPath` so only that file lands in the directory:
  `{name, mountPath: "/opt/starrocks/fe/conf/ranger-starrocks-security.xml", subPath: "ranger-starrocks-security.xml"}` and the same shape for the audit XML.
  **The CRD supports this** — verified against `deploy/starrocks-operator/crd.yaml`, whose `configMaps` items declare `name`, `mountPath` and `subPath`.
- [ ] Record in a comment that a `subPath` mount does not receive ConfigMap updates, and why that is acceptable here: the plugin reads this config at boot, and a Ranger rebind rolls the FE anyway.
- [ ] Test: assert the rendered FE spec mounts each XML at its own full path with a `subPath`, and that **no mount targets the bare conf directory**. The second half is the assertion that would have caught this.

### Task 2: A StarRocks catalog on an egret warehouse must carry its S3 configuration

**Verified at source.** `crates/egret-catalog-registry/src/render/starrocks.rs:392-409`, the `IcebergMetastore::Egret` branch, never calls `out.s3(input)` — while its three siblings (`Rest` at `:433`, `Hive` at `:438`, `Glue` at `:447`) all do. The FE therefore falls back to its own `aws_s3_endpoint` and object reads fail `S3, Status Code: 400`. This is the same "one path but not its siblings" class SP4a hit nine times.

The comparison that settles the direction: Trino's own `Egret` branch (`render/trino.rs:222-226`) sets `fs.native-s3.enabled`, `s3.endpoint` (from `wh.s3_endpoint`), `s3.region` and `s3.path-style-access` **and** `vended-credentials-enabled`. So even a vending client is told where to talk.

**Files:** Modify `crates/egret-catalog-registry/src/render/starrocks.rs`.

- [ ] **First, resolve one genuine unknown by measuring it on the engine, not from documentation:** does this StarRocks version accept **vended credentials** from an Iceberg REST catalog, or does it require static `aws.s3.access_key` / `aws.s3.secret_key`? The answer decides the shape of the fix. `Out::s3()` currently *requires* a storage connection and a `secret_key_secret_id`; the `Egret` path may not have one. Record the measurement.
- [ ] Emit the S3 configuration on the `Egret` branch, using whichever source the measurement shows is correct — `wh.s3_endpoint` the way Trino does, or `input.storage` the way `s3()` does. If they can disagree, say so and pick the one the engine actually honours.
- [ ] Test: the `Egret` branch renders an endpoint, a region and path-style access; assert on the property map, not on the SQL string. A test asserting only that the SQL parses would pass against the bug being fixed.

### Task 3: `authz_service_id` on update must bind, or be refused

**Verified at source.** `update_engine` (`crates/egret-engine-mgmt/src/engines.rs:4600-4604`) takes `Json<CreateEngineBody>`, so `authz_service_id` is a **declared** field of the update body — accepted, then silently dropped. The handler immediately below already states the principle for exactly this situation, about a neighbouring field:

> "Accepting the field and dropping it would tell the caller their catalogs were attached (review F8); refusing sends them to the route that actually does it."

`catalogs` is refused. `authz_service_id` is not. **Decision: honour it**, rather than refuse it as `catalogs` is — because there is no other route to send the caller to. SP4a's live gate had to bind a StarRocks engine to a profile with a direct SQL `UPDATE`, which is the missing capability this closes.

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`.

- [ ] On update, apply `authz_service_id`: bind the engine to that profile, and unbind when it changes. Reuse the existing binding path (`set_engine_authz_service`) rather than writing a second one.
- [ ] A profile id that does not exist is a `400`/`404` naming the id, never a silent no-op — the defect being fixed is silence.
- [ ] **Recompile on bind**, in the same transaction as the write, following SP4a's established seam: affected services expanded through `services_and_tag_services_via`, locked ascending, with the `debug_assert!(service_ids.is_sorted())` gate. A newly bound profile that is not compiled is stale, and SP4a already fixed that shape at engine *create* (`680adea9`).
- [ ] Respect the standing constraint: binding and unbinding must never delete a Ranger profile.
- [ ] Tests: update with a valid `authz_service_id` binds and compiles; update with an unknown id is refused, naming it; unbinding leaves the profile row intact.

### Task 4: Measure S3 and S4 on a real StarRocks

The point of the other three tasks. Until this runs, D5's per-column tag expansion remains unverified at the read path.

**Files:** Create `docs/en/plans/2026-09-26-starrocks-live-result.md`.

- [ ] Deploy to kind. **All six helm values are required** — `oidc.enabled`, `ingress.enabled`, `kernel.launcher=kubernetes`, `backend.replicas=2`, `sharedStore.enabled`, **and `ranger.enabled=true`**. The chart defaults `ranger.enabled` to `false`; dropping it makes every engine deny every query, which looks exactly like "the policies work" while measuring nothing. This has already happened once in this programme.
- [ ] Verify by the pod, never by helm's report: the serving pods' `imageID` matches the image just loaded, and `EGRET_RANGER__ENABLED=true` read from inside a running pod. `helm upgrade --wait` has reported success here while pods stayed hours old.
- [ ] Confirm the FE actually starts and keeps its metadata after a Ranger bind — that is Task 1's real test.
- [ ] Run S1–S4: grant, deny, **tag mask**, **row filter**. Record the **literal values** returned, never "worked": a mask that returns the real value is the failure being hunted, and for the filter confirm the excluded rows are genuinely absent, not merely fewer.
- [ ] Confirm a tag row filter compiles to **one policy per table**, not per column, and that a tag mask expands **per column** — D5's central claim, at the read path this time.
- [ ] Run as a non-admin subject. Auto-bind seeds admin policies granting `*.*.*.*`, so any case run as `admin` passes vacuously.
- [ ] Reject any probe that cannot discriminate. `DESC` is not a permission probe: it returns columns for denied tables too.
- [ ] Anything that fails is recorded as FAILED with verbatim output. Partial honest results are the success condition; a run reporting cases it did not observe cannot be falsified.
