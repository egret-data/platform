# Governance sub-project 2b — the Trino OPA target — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a Trino engine bound to a generated Trino profile can be switched to Trino's own OPA access-control plugin. egret answers the OPA REST protocol itself from the neutral IR, with masks, row filters and refusals in force. The switch is backed by:
- a decision-level differential against real Ranger;
- an end-to-end differential of Trino-on-OPA against Trino-on-Ranger;
- measured latency.

**Architecture:**
- **Evaluator.** A new pure crate `egret-perm-eval` holds `Snapshot` (one profile's lowered statements plus its resolved column classifications). It answers `decide`, `filter`, `row_filters` and `column_masks`. Every operation name the Trino 483 OPA plugin sends is classified in one table, `trino_ops.rs`.
- **Snapshot build.** `egret-perm-compile` builds snapshots with the same loaders and lowerings the Ranger compile uses (`lower_ep_via`, split out of `compile_desired`). A `SnapshotCache` holds one `Arc<Snapshot>` per OPA profile, keyed by `(policy_version, tag_version)`. A background refresher rebuilds a snapshot when its key moves. The decision path reads only the cache.
- **Endpoint.** `opa_surface()` serves four `POST /opa/v1/data/egret/trino/*` routes. It authenticates the engine id and secret carried in `input.context.properties` with the engine's Ranger credential (the same constant-time check), through a short-TTL credential cache.
- **Switch.** Migration `0213` adds `policy.authz_services.mechanism`. `PATCH /api/control-plane/policies/services/{service}` with `{"mechanism": …}` refuses with a named condition unless:
  - the profile is Trino;
  - the profile is generated;
  - every bound engine's image ships `plugin/opa`;
  - the profile holds no `all - …` default row;
  - the profile and its tag service hold no hand-written (`managed_by = 'operator'`) row.

  On success it recompiles and re-applies every bound engine.
- **Engine configuration.** `opa_plugin_files` sits next to `ranger_plugin_files`. Both provisioners render the OPA `access-control.properties` and the context file. Externally run engines get `GET /engines/{id}/opa-plugin-config`.
- **Explain.** `/perm/explain*` answers an OPA profile through `egret-perm-eval`.
- **Ranger stays the oracle (S8).** An OPA profile still compiles its Ranger artifact.

**Tech Stack:** Rust workspace (axum 0.8, sqlx 0.9 runtime queries, tokio), the Java Ranger harness under `tests/ranger` (Maven, Ranger plugins-common), Docker Compose for the live and end-to-end checks, React + orval + vitest for the UI, criterion for the microbenchmarks.

**Spec:** `docs/en/specs/2026-11-04-gov-sp2-trino-opa.md`, section "2b — the OPA target", with "Errors", "Verification" and "Out of scope". Decisions S1–S8 bind. Divergences from the spec are listed under **Spec deviations** below. Each one follows the code or the measured jar.

**Split point.** The plan has 25 tasks. **Package 2b-1** is Tasks 1–20: a usable OPA engine plus the Layer 1 differential and its CI job, ending in a live check. **Package 2b-2** is Tasks 21–25: Layer 2, latency, the UI and the docs page. Each package is its own PR. 2b-2 branches from 2b-1 once 2b-1 merges.

## Global Constraints

- **Worktree and branch.** Work in your own worktree, branched from the branch the controller names. Never use bare `git stash`. Use `git stash push -u -m <tag>` only when you must, and restore with `git stash apply <sha>`.
- **Tools.** `rg` is not installed; use `grep -rn`.
- **Builds.** Use your own `CARGO_TARGET_DIR`, for example `$PWD/target-sp2b`. Never share one across checkouts. Disk is tight: delete your target dir when your task is done.
- **DB tests.**
  - Use `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` with `EGRET_ALLOW_DB_SKIP` unset (`env -u EGRET_ALLOW_DB_SKIP …`).
  - Run one test suite at a time.
- **Every commit:**
  - `cargo fmt --all -- --check` passes;
  - `cargo clippy --workspace --all-targets -- -D warnings` passes;
  - the message is a Conventional Commit;
  - the message ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Golden files.** Every file under `crates/egret-perm-compile/tests/golden/*.json` stays byte-identical in every task. This plan adds no golden file.
- **S8, the Ranger artifact is the oracle.** The Ranger policy download of every profile, OPA or not, stays byte-identical to what it is before this plan. Nothing in this plan changes `ranger::emit`, `ranger::completion` or the infra renderer.
- **`egret-perm-ir` stays pure.** `crates/egret-perm-ir/tests/boundary.rs` stays green unchanged. That means no sqlx, tokio, `egret-policy`, `egret-policy-api` or `egret-ranger-eval`. `Mechanism` is the only public type that may say `Ranger`; that exemption already exists.
- **`egret-perm-eval` is pure.** It has:
  - no sqlx, tokio, network or filesystem in `src/`;
  - dependencies `egret-perm-ir` and `thiserror` only, plus `criterion` as a dev-dependency from Task 22;
  - a test, `crates/egret-perm-eval/tests/boundary.rs`, that enforces this.
- **The decision path never queries the database.** The only exception is a credential-cache miss (Task 9), which calls the existing `DbVerifier`.
- **Migration number `0213`.** Only Task 5 adds a migration. Evidence, gathered 2026-11-04:
  - `origin/main` ends at `0212_drop_orphan_group_grants.sql`;
  - the open PRs are #835 `docs/gov-sp2-spec` and #836 `feat/gov-sp2a-infra-ir`, and both end at `0212`;
  - no remote branch carries any `021[1-9]`.

  Re-check before committing Task 5:

  ```bash
  git fetch origin
  git ls-tree --name-only origin/main migrations/ | tail -1
  for b in $(gh pr list --state open --json headRefName -q '.[].headRefName'); do git ls-tree --name-only "origin/$b" migrations/ | tail -1; done
  ```

  If any of these shows `0213` or higher, renumber above it and update this plan's text in the same commit.
- **API changes.**
  - Regenerate `openapi.json` with `cargo run -p egret-bin -- openapi > openapi.json`.
  - Then regenerate the client with `cd frontend && npm run gen:api`. Never run bare `orval`.
  - Commit both. `crates/egret-server/src/lib.rs`'s `committed_openapi_json_is_current` and the API digest test must pass.
- **Live and end-to-end stacks.**
  - They run only under the compose project `egret-opa-e2e` (Task 20).
  - Never touch `egret-demo`, `egret-dev-*`, `egretdocs-*` or `egret-s4-*`.
  - Never run `just up/down/reset-testdata`.
  - No StarRocks is involved. If one ever is, use only `ghcr.io/egret-data/egret-starrocks-{fe,allin1}` pinned images.
- **The secret is never logged.** No log line, error text or test assertion message contains the value of `egret.secret` or a Ranger credential.
- **Fail closed.** Every refusal path answers a denying result. For the two value routes, an internal error answers HTTP 500, which the plugin also turns into a failure.

---

## Design

Line numbers are against branch `feat/gov-sp2a-infra-ir` at `4b94ac67`.

### D1. What the Trino 483 plugins send, measured

Extracted on 2026-11-04 from `trinodb/trino:483@sha256:db58cc93…0534`. The jars are `io.trino_trino-opa-483.jar` (sha256 `2e4e7510…f216`) and `io.trino_trino-ranger-483.jar` (sha256 `26dd71b8…ee70`). Task 1 re-runs the extraction and pins the output as fixtures.

The OPA plugin sends **63 operation names**. They are the string constants of `OpaAccessControl`, `OpaBatchAccessControl` and `OpaHighLevelClient`; `UNKNOWN`, a version default in `OpaAccessControlFactory`, is not among them.

```text
AccessCatalog AddColumn AlterColumn CreateCatalog CreateFunction CreateMaterializedView CreateSchema
CreateTable CreateView CreateViewWithExecuteFunction CreateViewWithSelectFromColumns DeleteFromTable
DropCatalog DropColumn DropFunction DropMaterializedView DropSchema DropTable DropView ExecuteFunction
ExecuteProcedure ExecuteQuery ExecuteTableProcedure FilterCatalogs FilterColumns FilterFunctions
FilterSchemas FilterTables FilterViewQueryOwnedBy GetColumnMask GetRowFilters ImpersonateUser
InsertIntoTable KillQueryOwnedBy ReadSystemInformation RefreshMaterializedView RenameColumn
RenameMaterializedView RenameSchema RenameTable RenameView SelectFromColumns SetCatalogSessionProperty
SetColumnComment SetMaterializedViewProperties SetSchemaAuthorization SetSystemSessionProperty
SetTableAuthorization SetTableComment SetTableProperties SetViewAuthorization SetViewComment ShowColumns
ShowCreateFunction ShowCreateSchema ShowCreateTable ShowFunctions ShowSchemas ShowTables TruncateTable
UpdateTableColumns ViewQueryOwnedBy WriteSystemInformation
```

- **GRANT, REVOKE, DENY and every role operation are never sent.**
  - The plugin routes them through `enforcePermissionManagementOperation`.
  - With `opa.allow-permission-management-operations=false`, that method throws the denial locally.
  - With `true`, it returns without asking OPA at all.
  - So `false` is the only safe value, and egret pins it.
- **Request schema** (field names from `io.trino.plugin.opa.schema`):
  - `input.context`: `identity {user, groups}`, `softwareStack {trinoVersion}`, `properties {…}`, `queryId`.
  - `input.action`: `operation`, plus `resource` | `filterResources[]` | `targetResource`, plus `grantee`.
  - A resource carries one of:
    - `user {user}`;
    - `systemSessionProperty {name}`;
    - `catalogSessionProperty {catalogName, propertyName}`;
    - `function {catalogName, schemaName, functionName}`;
    - `catalog {name}`;
    - `schema {catalogName, schemaName, properties}`;
    - `table {catalogName, schemaName, tableName, columns?, properties}`;
    - `column {catalogName, schemaName, tableName, columnName, columnType}`.
  - The procedure and function names both travel in `function.functionName`.
- **Response** `{"decision_id"?: string, "result": …}`:
  - `allow`: a boolean.
  - Batch: a list of allowed indices.
  - Row filters: a list of `{expression, identity?}`.
  - Batch column masks: a list of `{index, viewExpression {expression, identity?}}`.
  - The plugin builds the Trino `ViewExpression` with the table's catalog and schema, as the Ranger plugin does.
- **What the Trino Ranger plugin checks for the same calls** (`RangerSystemAccessControl` 483):

| Calls | Ranger access type on the named resource |
|---|---|
| `SelectFromColumns`, `CreateViewWithSelectFromColumns`, `GetRowFilters`, `GetColumnMask` | `select` |
| `InsertIntoTable`, `UpdateTableColumns` | `insert` |
| `DeleteFromTable`, `TruncateTable` | `delete` |
| `AccessCatalog`, `ShowSchemas`, `ShowTables`, `ShowColumns`, `ShowFunctions`, all `Filter*` except `FilterViewQueryOwnedBy` | `_any` (any access type on the resource or below it) |
| `ImpersonateUser`, `KillQueryOwnedBy`, `ViewQueryOwnedBy`, `FilterViewQueryOwnedBy` | `impersonate` on `trinouser` |
| `ExecuteQuery` | `execute` on `queryid` |
| `ExecuteProcedure`, `ExecuteFunction`, `CreateViewWithExecuteFunction` | `execute` on `procedure` or `schemafunction` |
| `SetSystemSessionProperty`, `SetCatalogSessionProperty` | `alter` on `systemproperty` or `sessionproperty` |
| `Create*` | `create` |
| `Drop*` | `drop` |
| `Show*Create*` | `show` |
| `ReadSystemInformation`, `WriteSystemInformation` | `read_sysinfo`, `write_sysinfo` |
| every other `Rename*`/`Set*`/`*Column`/`ExecuteTableProcedure`/`RefreshMaterializedView` | `alter` |

- **Mask rendering by the Ranger plugin:**
  - `MASK_NULL` becomes `NULL`.
  - `CUSTOM` becomes `maskedValue`, with `NULL` when it is absent.
  - Every other type becomes the service-def's `transformer`, with `{col}` replaced by the column name and `{type}` by the column type's display name.
  - `MASK_NONE` gives no mask.
  - The transformers are in `crates/egret-policy/src/service-defs/trino.json` → `dataMaskDef.maskTypes[].transformer`.

### D2. Evaluator semantics

- **Audiences are already expanded by the lowering** (`MemberName::User`, `MemberName::Everyone`). A `MemberName::Unexpanded` member is a snapshot build error. S7: `identity.groups` is ignored, and the snapshot carries no group directory (deviation 3).
- **Path matching.** Each `Level::Named(v)` is a Ranger wildcard pattern (`*`, `?`), compared case-insensitively, as the Trino service-def's `matcherOptions` say (`wildCard: true, ignoreCase: true`). For an access statement, `Level::Unnamed` matches every value, because the Ranger target renders it as `*` (`ranger/grants.rs::scope_to_resources`, `PolicyTarget::Access`).
- **Deny wins** (umbrella D6), for each privilege and each column:
  - a `Deny` or `Withhold` that matches the user, the object and the privilege refuses it, whatever allows it;
  - a `Withhold` audience is `include` minus `except`.
- **Classifications.**
  - A column carries its own classifications plus those of its table.
  - A table-level request carries only the table's.
  - This is how the Ranger tag enricher feeds requests (`ANCESTOR` for the table tag on a column request).
- **Privileges.**
  - `Read` answers `select`.
  - `Write` answers `insert` and `delete`.
  - Everything else (create, drop, alter, show, sysinfo) is held only by `Target::Infra(InfraObject::WholeEp)`, the `egret-admin` all-access. Grants never compile those access types, and the `all - …` defaults are refused at the switch.
- **Reach** is the `_any` check:
  - A user reaches an object when some allowing statement for that user matches the object or any object below it.
  - The allowing statement is an `Allow`, a `WholeEp`, or an `EngineObject` whose leading values name the object.
  - A deny on the object or an ancestor removes a privilege from that reach. The object is reached while at least one privilege remains.
- **Infrastructure and engine capabilities:**
  - `WholeEp` allows every check.
  - `AnyUser`/`SessionUser` allow impersonation: `SessionUser` only when the target user equals the session user.
  - `AnyQuery` allows `ExecuteQuery`.
  - `SystemProperty(p)` allows `SetSystemSessionProperty` of `p`.
  - `EngineObject { capability, values }` allows the operation named by the capability id (`trino.impersonate_user`, `trino.set_system_session_property`, `trino.set_catalog_session_property`, `trino.execute_procedure`, `trino.execute_function`) when its values equal the request's, compared case-insensitively.
- **Row filters** of every matching `RowFilter` statement combine with `AND`, in statement order: `(a) AND (b)`.
- **Masks.**
  - At most one mask applies per column per user.
  - `WithholdMask` counts as a `MASK_NULL` mask for its audience.
  - Two masks on one column for one user is an `EvalError::TwoMasks`. The routes answer it with HTTP 500, so the plugin fails the query.
- **Metadata tables** (`t$files`, …), mirroring `ranger/completion/metadata_tables.rs`:
  - A table name containing `$` is matched as written, so a grant on `*` reaches it and a grant on `t` does not.
  - In addition, a `Read` on `t$…` is denied to any user for whom a mask, a row filter, a deny or a withhold reaches any column of `t`.

### D3. Snapshot, cache and refresher

- **Building.** `egret_perm_compile::opa::build_snapshot_via(conn, service_id)`:
  1. `lower_ep_via` (the four families);
  2. `ir_load::load_infra_inputs` + `lower_infra`;
  3. `load_classified_columns_via`;
  4. `Snapshot::build`.
- **The cache:**
  - `SnapshotCache` maps `service_id → CacheEntry { key: (policy_version, tag_version), snapshot: Option<Arc<Snapshot>>, error: Option<String> }`.
  - `get(service_id)` answers `Lookup::{NotOpa, NotBuilt { error }, Ready(Arc<Snapshot>)}`.
- **The refresher**, `spawn_snapshot_refresher(cache, pool, period = 1 s, shutdown)`, runs `refresh_once` each period:
  - it reads `SELECT id, policy_version, tag_version FROM policy.authz_services WHERE mechanism = 'opa' ORDER BY id`;
  - it drops entries no longer listed;
  - it rebuilds each entry whose key moved, in a `REPEATABLE READ, READ ONLY` transaction;
  - when a build fails, it keeps the previous snapshot, stores the error, logs at `warn`, and calls `compile_runs::record_compile_failure(pool, id, "OPA snapshot: {e}")`.

### D4. Spec deviations

1. **`opa_surface()` lives in `egret-perm-compile`, not `egret-server`.**
   - `egret-server` mounts it next to `ranger_surface()`, which itself lives in `egret-policy` (`crates/egret-policy/src/ranger/routes.rs:71`).
   - The snapshot build needs `ir_load`, which is `pub(crate)` in `egret-perm-compile`, and `egret-policy` cannot depend on `egret-perm-compile` (the dependency points the other way).
   - The evaluator stays its own pure crate, so S2's later sidecar is unaffected.
2. **The PATCH route takes the profile name.** It is `PATCH /api/control-plane/policies/services/{service}`, like every other profile route (`admin/mod.rs:512`), not `{id}`.
3. **The snapshot holds no users → groups directory.** The lowering has already expanded every audience to users (`MemberName::User`/`Everyone`). Matching on groups would only add a way to disagree with the Ranger artifact.
4. **The operation table has more classes than the spec's four.** Its classes are `Data(Read|Write)`, `Reach`, `AdminOnly`, `EngineCapability(id)`, `Impersonation`, `ExecuteQuery`, `SystemSessionProperty` and `ValueQuestion`.
   - "Always denied" has no row: per D1, GRANT/REVOKE/role operations never reach egret.
   - "Always allowed" has no row: `FilterViewQueryOwnedBy` on one's own queries is answered by the `egret-user-impersonate-self` infra statement, which is how Ranger answers it.
   - An unknown name is still denied as "operation not modelled".
5. **Two more switch refusals.** Besides the decided `all - …` refusal (D-all), the switch is refused while the profile or its tag service holds a `managed_by = 'operator'` row. The reason is the same: it is stored Ranger JSON the evaluator would silently ignore.
6. **"Unknown engine" and "wrong secret" log one reason**, "engine credential refused". The `EngineCredentialVerifier` contract (`egret-engine-mgmt-api/src/credential.rs:47`) forbids callers to tell them apart.
7. **A credential cache, the one database read.** The decision path consults a credential cache (positive entries 30 s, negative 5 s). A miss calls `DbVerifier`, the one database read on the request path. Without the cache, every OPA request would read the engine row and the secret store.
8. **Internal evaluator errors answer HTTP 500** on `row_filters` and `batch_column_masks`. On those two routes `[]` means "no filter, no mask", which would fail open. Refusals still answer 200 with a denying result, as the spec says.
9. **The matrix rows for `opa(trino)` copy the Ranger Trino rows' answers.** Their evidence names the Layer 1 tests (Task 18), and a unit test keeps the two in step. This keeps the lowered statement set, and so the Ranger artifact of an OPA profile, identical to a Ranger profile's.
10. **Layer 1 builds store states with the golden fixture builders.** It writes them into Postgres through `crates/egret-perm-compile/tests/golden/fixtures.rs`. The existing 2000-case Ranger corpus draws raw Ranger JSON and has no egret store states to reuse.
    - What Layer 1 reuses is the Java runner (`tests/ranger/src/test/java/EgretCaseRunner.java`) and its Rust driver, `ask_ranger_json`.
    - It runs 40 states × 50 questions = 2000 cases.
11. **`explained.toml` is new.** The Ranger differential keeps its accepted differences as Rust consts (`ID_DIVERGENCES`, `DECLINED_CEILINGS`). The OPA differential uses the file the spec names.
12. **The migration number is `0213`**, chosen by the evidence in Global Constraints.

---

## File Structure

| Path | Task | Responsibility |
|---|---|---|
| `scripts/opa/extract-trino-plugin-facts.sh` | 1 | Pulls both 483 plugin jars from the pinned image and writes the fixtures |
| `crates/egret-perm-eval/{Cargo.toml,src/lib.rs,tests/boundary.rs}` | 1 | The pure evaluator crate |
| `crates/egret-perm-eval/tests/fixtures/trino-483-opa-operations.txt` | 1 | The 63 operation names |
| `crates/egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt` | 1 | Ranger plugin method → access types |
| `crates/egret-perm-eval/tests/fixtures/trino-483-plugins.sha256` | 1 | Jar digests |
| `crates/egret-perm-eval/src/trino_ops.rs` | 2 | The operation table |
| `crates/egret-perm-eval/src/{pattern.rs,snapshot.rs,decide.rs}` | 3 | Matching, the snapshot, `decide`/`filter` |
| `crates/egret-perm-eval/src/values.rs` | 4 | `row_filters`, `column_masks`, mask rendering |
| `migrations/0213_profile_mechanism.sql` | 5 | The `mechanism` column |
| `crates/egret-perm-ir/src/{ep.rs,matrix.rs}` | 5 | `Mechanism::Opa`, `opa(trino)` rows |
| `crates/egret-perm-compile/src/opa/{mod.rs,build.rs}` | 6 | `lower_ep_via`, `build_snapshot_via`, classified columns |
| `crates/egret-perm-compile/src/opa/cache.rs` | 7 | `SnapshotCache`, refresher |
| `crates/egret-perm-compile/src/opa/wire.rs` | 8 | OPA input parsing and result building |
| `crates/egret-perm-compile/src/opa/surface.rs` | 9 | `opa_surface`, `OpaState`, credential cache |
| `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs` | 10, 15 | Mounting and wiring |
| `crates/egret-policy/src/admin/mechanism.rs` | 11, 12 | Switch refusals and the PATCH route |
| `crates/egret-engine-mgmt-api/src/opa_config.rs` | 13 | `opa_plugin_files` |
| `crates/egret-provisioner/src/{trino.rs,docker/plan.rs}` | 14 | Rendering per mechanism |
| `crates/egret-engine-mgmt/src/{engines.rs,external.rs,store.rs}` | 15 | Apply reads the mechanism; re-apply; external route |
| `crates/egret-perm-compile/src/{routes.rs,explain_effect.rs}` | 16 | Explain through the IR |
| `crates/egret-ranger-eval/tests/support/ranger_jvm.rs` | 17 | `ask_ranger_json`, moved |
| `crates/egret-perm-compile/tests/opa_support/to_ranger.rs` | 17 | Operation → Ranger request translation |
| `crates/egret-perm-compile/tests/opa_differential.rs` | 18 | Layer 1 |
| `crates/egret-perm-compile/tests/opa_differential/explained.toml` | 18 | Accepted differences |
| `.github/workflows/ci.yml` | 19, 21 | `opa-differential`, `opa-e2e` jobs |
| `deploy/opa-e2e/**`, `scripts/opa-e2e/*.sh` | 20, 21, 22 | Own compose project, live check, Layer 2, latency |
| `crates/egret-perm-eval/benches/eval.rs` | 22 | criterion |
| `docs/en/reports/<measurement date>-gov-sp2b-opa-latency.md` | 22 | Dated report, named by the day it is measured, named by the day it is measured |
| `frontend/src/routes/governance/engines/tabs/MechanismSection.tsx` | 23 | Ranger \| OPA selector |
| `frontend/src/components/policy/TrinoConfigDialog.tsx` | 24 | OPA variant |
| `frontend/src/routes/control-plane/ExternalEngineDetail.tsx` | 24 | OPA plugin config for external engines |

---

# Package 2b-1

## Phase 1 — the evaluator

### Task 1: Pin the 483 plugin facts and create `egret-perm-eval`

**Files:**
- Create: `scripts/opa/extract-trino-plugin-facts.sh`
- Create: `crates/egret-perm-eval/Cargo.toml`, `crates/egret-perm-eval/src/lib.rs`, `crates/egret-perm-eval/tests/boundary.rs`
- Create (generated by the script, then committed): `crates/egret-perm-eval/tests/fixtures/trino-483-opa-operations.txt`, `trino-483-ranger-checks.txt`, `trino-483-plugins.sha256`
- Modify: root `Cargo.toml`. Add `egret-perm-eval = { path = "crates/egret-perm-eval" }` under `[workspace.dependencies]`; `members = ["crates/*"]` already picks the crate up.

**Interfaces:**
- Produces:
  - the crate `egret_perm_eval`;
  - the three fixture files;
  - the script, which needs `docker`, `unzip` and `javap` (JDK 17+).

- [ ] **Step 1: Write the script.**

```bash
#!/usr/bin/env bash
# Extract what Trino 483's OPA and Ranger plugins send and check, from the
# pinned image egret runs. Output files are committed as test fixtures of
# egret-perm-eval; re-run when KNOWN_ENGINE_IMAGES gains a Trino image.
set -euo pipefail
IMAGE="trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
OUT="${1:?usage: extract-trino-plugin-facts.sh <out-dir>}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cid="$(docker create "$IMAGE")"
docker cp "$cid:/usr/lib/trino/plugin/opa/io.trino_trino-opa-483.jar" "$WORK/opa.jar"
docker cp "$cid:/usr/lib/trino/plugin/ranger/io.trino_trino-ranger-483.jar" "$WORK/ranger.jar"
docker rm "$cid" >/dev/null
mkdir -p "$WORK/opa" "$WORK/ranger"
(cd "$WORK/opa" && unzip -q ../opa.jar 'io/trino/plugin/opa/*')
(cd "$WORK/ranger" && unzip -q ../ranger.jar 'io/trino/plugin/ranger/*')
(cd "$WORK" && shasum -a 256 opa.jar ranger.jar) > "$OUT/trino-483-plugins.sha256"

# Operation names: the string constants of the three classes that build requests.
for c in OpaAccessControl OpaBatchAccessControl OpaHighLevelClient; do
  javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/$c.class"
done | grep -oE '// String [A-Z][A-Za-z]+$' | awk '{print $NF}' | sort -u \
  > "$OUT/trino-483-opa-operations.txt"

# Ranger plugin: each public check/filter/get method and the access types it names.
javap -c -p -constants "$WORK/ranger/io/trino/plugin/ranger/RangerSystemAccessControl.class" \
| awk '/^  public [^(]* (checkCan|filter|get)[A-Za-z]+\(/ {
         if (m != "") print m ":" acc
         match($0, /(checkCan|filter|get)[A-Za-z]+/); m = substr($0, RSTART, RLENGTH); acc = "" }
       /RangerTrinoAccessType\.[A-Z_]+/ {
         match($0, /RangerTrinoAccessType\.[A-Z_]+/); a = substr($0, RSTART + 22, RLENGTH - 22)
         if (index(acc, " " a) == 0) acc = acc " " a }
       END { print m ":" acc }' | sort -u > "$OUT/trino-483-ranger-checks.txt"
```

  Run it as `bash scripts/opa/extract-trino-plugin-facts.sh crates/egret-perm-eval/tests/fixtures`.
  - Expected: `trino-483-opa-operations.txt` has exactly 63 lines, the list in D1 of this plan.
  - The ranger file has a line `checkCanSelectFromColumns: SELECT`.
  - If the count differs, stop and report the diff; do not edit the fixture by hand.

- [ ] **Step 2: Write the crate manifest.**

```toml
[package]
name = "egret-perm-eval"
version = "0.1.0"
edition.workspace = true
license.workspace = true

# Governance SP2b: the OPA decision evaluator over the neutral IR. Pure: no
# sqlx, tokio, network or filesystem in src/ (tests/boundary.rs), like
# egret-ranger-eval, so it can move into a sidecar later (spec S2).
[dependencies]
egret-perm-ir.workspace = true
thiserror.workspace = true
```

  `src/lib.rs` starts as only the module doc, saying what the crate is and that it is pure.

- [ ] **Step 3: Write the boundary test** `tests/boundary.rs`:

```rust
//! egret-perm-eval is pure and target-neutral.
#[test]
fn the_evaluator_depends_on_no_database_no_runtime_and_no_target() {
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest.split("[dependencies]").nth(1).unwrap();
    let deps = deps.split("[dev-dependencies]").next().unwrap();
    for forbidden in ["sqlx", "tokio", "egret-policy", "egret-ranger-eval", "egret-perm-compile", "reqwest", "axum"] {
        assert!(!deps.contains(forbidden), "egret-perm-eval must not depend on {forbidden}");
    }
}

#[test]
fn src_does_no_io() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        for word in ["std::fs", "std::net", "std::process", "std::env"] {
            assert!(!text.contains(word), "{p:?} uses {word}");
        }
    }
}
```

- [ ] **Step 4: Run.**

```bash
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-eval
```

  Expected: 2 tests PASS.

- [ ] **Step 5: Commit** `feat(perm-eval): crate skeleton and the Trino 483 plugin facts as fixtures`.

---

### Task 2: The operation table

**Files:**
- Create: `crates/egret-perm-eval/src/trino_ops.rs`
- Modify: `crates/egret-perm-eval/src/lib.rs` (`pub mod trino_ops;`)
- Test: inline `#[cfg(test)]` in `trino_ops.rs`

**Interfaces:**
- Consumes: the fixture `tests/fixtures/trino-483-opa-operations.txt` (Task 1).
- Produces:

```rust
pub enum DataPrivilege { Read, Write }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Read (select) of each listed column, or Write (insert/delete) of the table.
    Data(DataPrivilege),
    /// Ranger `_any`: some privilege on the object or below it.
    Reach,
    /// Only the all-access infrastructure statement (`InfraObject::WholeEp`) allows it.
    AdminOnly,
    /// An engine capability from the catalogue, by id (`egret-perm-compile/src/engine_capabilities.rs`).
    EngineCapability(&'static str),
    /// Impersonate, view, kill or filter queries of the resource's user.
    Impersonation,
    ExecuteQuery,
    SystemSessionProperty,
    /// Answered on a value route: `GetRowFilters`, `GetColumnMask`.
    ValueQuestion,
}
pub struct OpRow { pub name: &'static str, pub class: OpClass, pub why: &'static str }
pub const TRINO_483_OPERATIONS: &[OpRow];
pub fn classify(name: &str) -> Option<OpClass>;
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/trino-483-opa-operations.txt");

    #[test]
    fn the_table_covers_every_operation_the_483_plugin_sends() {
        let sent: std::collections::BTreeSet<&str> = FIXTURE.lines().filter(|l| !l.is_empty()).collect();
        let modelled: std::collections::BTreeSet<&str> = TRINO_483_OPERATIONS.iter().map(|r| r.name).collect();
        let missing: Vec<_> = sent.difference(&modelled).collect();
        let extra: Vec<_> = modelled.difference(&sent).collect();
        assert!(missing.is_empty(), "sent by the 483 plugin, not modelled: {missing:?}");
        assert!(extra.is_empty(), "modelled, never sent by the 483 plugin: {extra:?}");
        assert_eq!(sent.len(), 63);
    }

    #[test]
    fn each_name_appears_once_and_says_why() {
        let mut seen = std::collections::BTreeSet::new();
        for r in TRINO_483_OPERATIONS {
            assert!(seen.insert(r.name), "{} twice", r.name);
            assert!(!r.why.trim().is_empty(), "{} has no reason", r.name);
        }
    }

    #[test]
    fn an_unknown_operation_is_not_modelled() {
        assert_eq!(classify("GrantTablePrivilege"), None);
        assert_eq!(classify("selectfromcolumns"), None, "names are case-sensitive");
    }

    #[test]
    fn spot_checks_against_the_ranger_plugin() {
        assert_eq!(classify("SelectFromColumns"), Some(OpClass::Data(DataPrivilege::Read)));
        assert_eq!(classify("UpdateTableColumns"), Some(OpClass::Data(DataPrivilege::Write)));
        assert_eq!(classify("TruncateTable"), Some(OpClass::Data(DataPrivilege::Write)));
        assert_eq!(classify("FilterColumns"), Some(OpClass::Reach));
        assert_eq!(classify("CreateTable"), Some(OpClass::AdminOnly));
        assert_eq!(classify("KillQueryOwnedBy"), Some(OpClass::Impersonation));
        assert_eq!(classify("ExecuteProcedure"), Some(OpClass::EngineCapability("trino.execute_procedure")));
        assert_eq!(classify("GetColumnMask"), Some(OpClass::ValueQuestion));
    }
}
```

  `DataPrivilege` derives `Debug, Clone, Copy, PartialEq, Eq`.

- [ ] **Step 2: Run** `cargo test -p egret-perm-eval trino_ops`. Expected: FAIL (items undefined).

- [ ] **Step 3: Implement the table, alphabetically, with exactly these classes.** `why` cites the Ranger plugin's check (`trino-483-ranger-checks.txt`).
  - `Data(Read)`:
    - `SelectFromColumns`, `CreateViewWithSelectFromColumns`. Why: "Ranger: SELECT on each column".
  - `Data(Write)`:
    - `InsertIntoTable`, `UpdateTableColumns`. Why: "Ranger: INSERT on the table".
    - `DeleteFromTable`, `TruncateTable`. Why: "Ranger: DELETE on the table".
  - `Reach`. Why: "Ranger: _ANY, resource or descendants".
    - `AccessCatalog`, `ShowSchemas`, `ShowTables`, `ShowColumns`, `ShowFunctions`;
    - `FilterCatalogs`, `FilterSchemas`, `FilterTables`, `FilterColumns`.
  - `EngineCapability`. Why: "Ranger: EXECUTE on schemafunction/procedure" or "ALTER on sessionproperty".
    - `ExecuteFunction`, `CreateViewWithExecuteFunction`, `FilterFunctions` → `"trino.execute_function"`;
    - `ExecuteProcedure` → `"trino.execute_procedure"`;
    - `SetCatalogSessionProperty` → `"trino.set_catalog_session_property"`.
  - `Impersonation`. Why: "Ranger: IMPERSONATE on trinouser".
    - `ImpersonateUser`, `KillQueryOwnedBy`, `ViewQueryOwnedBy`, `FilterViewQueryOwnedBy`.
  - `ExecuteQuery`. Why: "Ranger: EXECUTE on queryid".
  - `SystemSessionProperty`: `SetSystemSessionProperty`. Why: "Ranger: ALTER on systemproperty".
  - `ValueQuestion`: `GetRowFilters`, `GetColumnMask`.
  - `AdminOnly`, the remaining 35 names. Why names the Ranger access type: CREATE / DROP / ALTER / SHOW / READ_SYSINFO / WRITE_SYSINFO, "which no grant compiles; only the egret-admin all-access holds it".

    ```text
    AddColumn AlterColumn CreateCatalog CreateFunction CreateMaterializedView CreateSchema CreateTable
    CreateView DropCatalog DropColumn DropFunction DropMaterializedView DropSchema DropTable DropView
    ExecuteTableProcedure ReadSystemInformation RefreshMaterializedView RenameColumn
    RenameMaterializedView RenameSchema RenameTable RenameView SetColumnComment
    SetMaterializedViewProperties SetSchemaAuthorization SetTableAuthorization SetTableComment
    SetTableProperties SetViewAuthorization SetViewComment ShowCreateFunction ShowCreateSchema
    ShowCreateTable WriteSystemInformation
    ```

  `classify` is a linear search over the table.

- [ ] **Step 4: Run** `cargo test -p egret-perm-eval`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(perm-eval): the Trino 483 operation table, pinned to the jar`.

---

### Task 3: Snapshot, matching, `decide` and `filter`

**Files:**
- Create: `crates/egret-perm-eval/src/pattern.rs`, `src/snapshot.rs`, `src/decide.rs`
- Modify: `src/lib.rs`. It exports:
  - from `snapshot`: `Snapshot`, `SnapshotInput`, `ClassifiedColumn` and `SnapshotError`;
  - from `decide`: `Resource`, `Decision`, `ObjectPath` and `EvalError`.
- Test: `crates/egret-perm-eval/tests/decide.rs`

**Interfaces:**
- Consumes:
  - `egret_perm_ir::statement::{Statement, Source, Target, Effect, Privilege, Path, Level, InfraObject, InfraKind}`;
  - `egret_perm_ir::audience::{Audience, Member, MemberName, Via}`;
  - Task 2's `classify`, `OpClass` and `DataPrivilege`.
- Produces:

```rust
// snapshot.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedColumn {
    pub catalog: String, pub schema: String, pub table: String,
    /// None: the classification is on the whole table.
    pub column: Option<String>,
    /// `tags.name`, the name `Target::Classification` carries.
    pub classification: String,
}
pub struct SnapshotInput {
    pub service_id: i64,
    /// (policy_version, tag_version) the inputs were read at.
    pub key: (i64, i64),
    pub statements: Vec<egret_perm_ir::statement::Statement>,
    pub classified: Vec<ClassifiedColumn>,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("statement {index} names an unexpanded principal {principal}")]
    UnexpandedMember { index: usize, principal: String },
}
pub struct Snapshot { /* private: statements, indexes */ }
impl Snapshot {
    pub fn build(input: SnapshotInput) -> Result<Snapshot, SnapshotError>;
    pub fn service_id(&self) -> i64;
    pub fn key(&self) -> (i64, i64);
    pub fn len(&self) -> usize;
}

// decide.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    None,
    Catalog(String),
    Schema { catalog: String, schema: String },
    Table { catalog: String, schema: String, table: String, columns: Vec<String> },
    Column { catalog: String, schema: String, table: String, column: String, column_type: String },
    User(String),
    SystemProperty(String),
    CatalogSessionProperty { catalog: String, property: String },
    Function { catalog: String, schema: String, function: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// `by` names the deciding statement's source, e.g. "grant 6f1c…", "infra ExecuteQuery".
    Allow { by: String },
    /// `explicit`: a Deny/Withhold refused it (vs nothing allowed it).
    Deny { reason: String, explicit: bool },
}
/// A data object for `explain`: absent levels match only statements covering the whole level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPath { pub catalog: String, pub schema: Option<String>, pub table: Option<String>, pub column: Option<String> }
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("two masks apply to column {column} for user {user}")]
    TwoMasks { user: String, column: String },
}
impl Snapshot {
    pub fn decide(&self, user: &str, operation: &str, resource: &Resource) -> Decision;
    pub fn filter(&self, user: &str, operation: &str, resources: &[Resource]) -> Vec<usize>;
    pub fn decide_privilege(&self, user: &str, privilege: DataPrivilege, object: &ObjectPath) -> Decision;
}
```

**Semantics to implement (from this plan's D2; repeated here so the task stands alone):**
- `pattern::matches(pattern, value)` implements Ranger wildcards: `*` is any run, `?` is one character, and the comparison is case-insensitive (lowercase both sides). `Level::Unnamed` on an access statement matches anything.
- **Audience.** A statement applies to `user` when:
  - `include` holds `MemberName::User(user)` or `MemberName::Everyone`;
  - and `except` holds neither.
- **Data.** For `Data(Read)` on a `Table`, check each listed column; every column must be allowed. For `Data(Write)`, check the table, with column `*`. A privilege on an object is allowed when:
  - an applying `Effect::Allow { privileges }` with that privilege has a `Target::Path` matching all four levels; or an applying `Target::Infra(InfraObject::WholeEp)` statement exists;
  - and no applying `Effect::Deny`/`Effect::Withhold` with that privilege matches. A withhold matches through `Target::Path` (StarRocks shape) or `Target::Classification { name }`. For a column request, the classification is the column's or its table's; for a table request, only the table's.
- **Reach** (operations `Reach`), on the request's object:
  - Catalog: `Catalog(c)`.
  - Schema: `Schema`.
  - Table: a `Table` with no columns, or a `Table` in `filter` for `FilterTables`.
  - `FilterColumns`: each column (Task 8 turns that request into one `Table` per column).

  An applying allow (Allow path, WholeEp, or EngineObject whose first value equals the catalog) whose levels are compatible with the object's named levels makes it reachable. Unnamed or deeper levels are free. A deny counts only if its path covers the object (every level the object names matches, and every deeper deny level is `*`/Unnamed) and it denies every privilege the allow grants.
- **`AdminOnly`.** Allowed only by an applying WholeEp. The deny reason is `"{op} needs the all-access engine administration grant"`.
- **`EngineCapability(id)`.** Allowed by:
  - an applying `Target::EngineObject { capability == id, values }` whose values equal the request's values, compared case-insensitively. Values are `[catalog, schema, function]` for `Function`, and `[catalog, property]` for `CatalogSessionProperty`;
  - or WholeEp.
- **`Impersonation`** of `User(target)`. Allowed by:
  - an applying `InfraObject::AnyUser`;
  - `InfraObject::SessionUser` when `target` equals `user`, compared case-insensitively;
  - `EngineObject { "trino.impersonate_user", [target] }`;
  - or WholeEp.
- **`ExecuteQuery`.** Allowed by `InfraObject::AnyQuery` or WholeEp.
- **`SystemSessionProperty`** of `p`. Allowed by:
  - `InfraObject::SystemProperty(p)`;
  - `EngineObject { "trino.set_system_session_property", [p] }`;
  - or WholeEp.
- **`ValueQuestion` on `decide`** is a deny: "asked on a value route".
- **Unknown operation** → `Deny { reason: format!("operation not modelled: {op}"), explicit: false }`.
- **A resource of the wrong shape** for its class → `Deny { reason: "malformed resource for {op}", explicit: false }`.
- **Metadata tables.** A table name containing `$`:
  1. is checked as written;
  2. is then denied for `Read` if any Mask, RowFilter, Deny, Withhold or WithholdMask statement applying to the user reaches any column of the base table (the name before the first `$`), by path or by classification.
- **The index.** `Snapshot::build` sorts statement indices into:
  - `by_catalog: BTreeMap<String /* lowercase literal */, Vec<usize>>`;
  - `wild_catalog: Vec<usize>` (catalog level contains `*`/`?`);
  - `by_classification: BTreeMap<String, Vec<usize>>`;
  - `infra: Vec<usize>`;
  - `engine: Vec<usize>`;
  - `classes: BTreeMap<(String, String, String), (BTreeSet<String> /* table */, BTreeMap<String, BTreeSet<String>> /* column */)>`, all lowercased.

  A question scans only `by_catalog[c] ∪ wild_catalog` plus the classification lists of the classifications its object carries.
- **`filter`** returns the indices `i` for which `decide(user, op, &resources[i])` allows, ascending.
- **`decide_privilege`** applies the Data rule to an `ObjectPath`. An absent level matches only statements whose level is `Unnamed` or exactly `*`.

- [ ] **Step 1: Write the failing tests** `tests/decide.rs`. Build statements directly; this is the helper set:

```rust
use egret_perm_eval::{ClassifiedColumn, Decision, Resource, Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::statement::*;
use uuid::Uuid;

fn who(users: &[&str], except: &[&str]) -> Audience {
    let m = |n: &str| Member { name: if n == "*" { MemberName::Everyone } else { MemberName::User(n.into()) }, via: Via::Direct };
    Audience { include: users.iter().map(|u| m(u)).collect(), except: except.iter().map(|u| m(u)).collect() }
}
fn path(c: &str, s: Option<&str>, t: Option<&str>, col: Option<&str>) -> Target {
    let l = |v: Option<&str>| v.map_or(Level::Unnamed, |v| Level::Named(v.into()));
    Target::Path(Path { catalog_id: None, catalog: c.into(), schema: l(s), table: l(t), column: l(col) })
}
fn st(source_n: u128, audience: Audience, target: Target, effect: Effect) -> Statement {
    Statement { source: Source::Grant { grant_id: Uuid::from_u128(source_n) }, audience, target, effect, derivation: Derivation::AsDeclared }
}
fn infra(kind: InfraKind, audience: Audience, object: InfraObject, cap: &str) -> Statement {
    Statement { source: Source::Infra { kind }, audience, target: Target::Infra(object),
                effect: Effect::EnginePrivilege { capability: cap.into() }, derivation: Derivation::AsDeclared }
}
fn snap(statements: Vec<Statement>, classified: Vec<ClassifiedColumn>) -> Snapshot {
    Snapshot::build(SnapshotInput { service_id: 1, key: (1, 1), statements, classified }).unwrap()
}
fn cols(c: &str, s: &str, t: &str, cs: &[&str]) -> Resource {
    Resource::Table { catalog: c.into(), schema: s.into(), table: t.into(), columns: cs.iter().map(|x| x.to_string()).collect() }
}
fn allowed(d: &Decision) -> bool { matches!(d, Decision::Allow { .. }) }
```

  Tests, each asserting exactly what its name says:

```rust
#[test]
fn a_read_grant_on_a_catalog_allows_select_of_any_column() {
    let s = snap(vec![st(1, who(&["ann"], &[]), path("lake", None, None, None),
                         Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["a", "b"]))));
    assert!(!allowed(&s.decide("bob", "SelectFromColumns", &cols("lake", "s", "t", &["a"]))));
    assert!(!allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "t", &[]))));
}

#[test]
fn deny_wins_over_allow_on_one_column() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read] }),
        st(2, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), Some("ssn")), Effect::Deny { privileges: vec![Privilege::Read] }),
    ], vec![]);
    let d = s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["id", "ssn"]));
    assert!(matches!(d, Decision::Deny { explicit: true, .. }), "{d:?}");
    assert!(allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["id"]))));
}

#[test]
fn an_except_member_is_not_in_the_audience() {
    let s = snap(vec![st(1, who(&["*"], &["bob"]), path("lake", None, None, None),
                         Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(!allowed(&s.decide("bob", "SelectFromColumns", &cols("lake", "s", "t", &["a"]))));
}

#[test]
fn wildcards_match_case_insensitively() {
    let s = snap(vec![st(1, who(&["ann"], &[]), path("lake", Some("sa*"), Some("T?"), None),
                         Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(allowed(&s.decide("ann", "SelectFromColumns", &cols("LAKE", "sales", "t1", &["x"]))));
    assert!(!allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "hr", "t1", &["x"]))));
}

#[test]
fn a_classification_withhold_refuses_the_tagged_column_and_the_whole_tagged_table() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read, Privilege::Write] }),
        Statement { source: Source::Withhold { classification_id: Uuid::nil(), classification_name: "pii".into() },
                    audience: who(&["*"], &["cleared"]), target: Target::Classification { name: "pii".into() },
                    effect: Effect::Withhold { privileges: vec![Privilege::Read, Privilege::Write] }, derivation: Derivation::AsDeclared },
    ], vec![
        ClassifiedColumn { catalog: "lake".into(), schema: "s".into(), table: "t".into(), column: Some("ssn".into()), classification: "pii".into() },
        ClassifiedColumn { catalog: "lake".into(), schema: "s".into(), table: "u".into(), column: None, classification: "pii".into() },
    ]);
    assert!(!allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["ssn"]))));
    assert!(allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["id"]))));
    assert!(allowed(&s.decide("cleared", "SelectFromColumns", &cols("lake", "s", "t", &["ssn"]))));
    assert!(!allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "u", &["any"]))), "table tag rolls down");
    assert!(!allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "u", &[]))), "table tag on a table request");
    assert!(allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "t", &[]))), "a column tag is not on the table request");
}

#[test]
fn reach_sees_a_grant_below_the_object() {
    let s = snap(vec![st(1, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), None),
                         Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(allowed(&s.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))));
    assert!(allowed(&s.decide("ann", "ShowTables", &Resource::Schema { catalog: "lake".into(), schema: "s".into() })));
    assert!(!allowed(&s.decide("ann", "ShowTables", &Resource::Schema { catalog: "lake".into(), schema: "other".into() })));
    assert_eq!(s.filter("ann", "FilterCatalogs", &[Resource::Catalog("x".into()), Resource::Catalog("lake".into())]), vec![1]);
}

#[test]
fn admin_only_needs_the_whole_ep_statement() {
    let s = snap(vec![infra(InfraKind::AdminAllAccess, who(&["root"], &[]), InfraObject::WholeEp, "all")], vec![]);
    assert!(allowed(&s.decide("root", "CreateTable", &cols("lake", "s", "t", &[]))));
    let d = s.decide("ann", "CreateTable", &cols("lake", "s", "t", &[]));
    assert!(matches!(&d, Decision::Deny { reason, .. } if reason.contains("all-access")), "{d:?}");
}

#[test]
fn impersonation_rules() {
    let s = snap(vec![
        infra(InfraKind::DelegatorImpersonate, who(&["egret-delegator"], &[]), InfraObject::AnyUser, "impersonate"),
        infra(InfraKind::SelfImpersonate, who(&["*"], &[]), InfraObject::SessionUser, "impersonate"),
        infra(InfraKind::ExecuteQuery, who(&["*"], &[]), InfraObject::AnyQuery, "execute"),
        infra(InfraKind::QueryTimeout, who(&["*"], &[]), InfraObject::SystemProperty("query_max_run_time".into()), "alter"),
    ], vec![]);
    assert!(allowed(&s.decide("egret-delegator", "ImpersonateUser", &Resource::User("ann".into()))));
    assert!(allowed(&s.decide("ann", "KillQueryOwnedBy", &Resource::User("ann".into()))));
    assert!(!allowed(&s.decide("ann", "KillQueryOwnedBy", &Resource::User("bob".into()))));
    assert!(allowed(&s.decide("ann", "ExecuteQuery", &Resource::None)));
    assert!(allowed(&s.decide("ann", "SetSystemSessionProperty", &Resource::SystemProperty("query_max_run_time".into()))));
    assert!(!allowed(&s.decide("ann", "SetSystemSessionProperty", &Resource::SystemProperty("query_max_memory".into()))));
    assert_eq!(s.filter("ann", "FilterViewQueryOwnedBy", &[Resource::User("bob".into()), Resource::User("ann".into())]), vec![1]);
}

#[test]
fn an_engine_capability_allows_exactly_its_values() {
    let s = snap(vec![Statement {
        source: Source::EnginePrivilege { engine_grant_id: Uuid::nil() }, audience: who(&["ann"], &[]),
        target: Target::EngineObject { capability: "trino.execute_procedure".into(), values: vec!["lake".into(), "system".into(), "rollback".into()] },
        effect: Effect::EnginePrivilege { capability: "trino.execute_procedure".into() }, derivation: Derivation::AsDeclared }], vec![]);
    let f = |p: &str| Resource::Function { catalog: "lake".into(), schema: "system".into(), function: p.into() };
    assert!(allowed(&s.decide("ann", "ExecuteProcedure", &f("rollback"))));
    assert!(!allowed(&s.decide("ann", "ExecuteProcedure", &f("expire"))));
    assert!(!allowed(&s.decide("ann", "ExecuteFunction", &f("rollback"))), "a procedure grant is not a function grant");
}

#[test]
fn unknown_and_malformed_are_denied_with_a_reason() {
    let s = snap(vec![], vec![]);
    assert_eq!(s.decide("ann", "GrantTablePrivilege", &Resource::None),
               Decision::Deny { reason: "operation not modelled: GrantTablePrivilege".into(), explicit: false });
    assert!(matches!(s.decide("ann", "SelectFromColumns", &Resource::Catalog("x".into())),
                     Decision::Deny { ref reason, .. } if reason.starts_with("malformed resource")));
}

#[test]
fn a_metadata_table_is_refused_to_someone_a_mask_restricts() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read] }),
        st(2, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), Some("email")), Effect::Mask { mask_type: "MASK_HASH".into(), expression: None }),
    ], vec![]);
    assert!(!allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t$files", &["lower_bounds"]))));
    assert!(allowed(&s.decide("bob", "SelectFromColumns", &cols("lake", "s", "t$files", &["lower_bounds"]))));
}

#[test]
fn an_unexpanded_member_fails_the_build() {
    let p = egret_perm::Principal { kind: egret_perm::PrincipalKind::Role, r#ref: "r".into() };
    let mut a = who(&[], &[]);
    a.include.push(Member { name: MemberName::Unexpanded(p), via: Via::Direct });
    let e = Snapshot::build(SnapshotInput { service_id: 1, key: (1, 1), classified: vec![],
        statements: vec![st(1, a, path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read] })] });
    assert!(e.is_err());
}
```

  Add `egret-perm.workspace = true` and `uuid.workspace = true` under `[dev-dependencies]` for these tests.

- [ ] **Step 2: Run** `cargo test -p egret-perm-eval --test decide`. Expected: FAIL (items undefined).
- [ ] **Step 3: Implement** `pattern.rs`, `snapshot.rs` and `decide.rs` exactly to the semantics above.
- [ ] **Step 4: Run** `cargo test -p egret-perm-eval`. Expected: PASS.
- [ ] **Step 5: Mutation checks.** For each, make the change, run `cargo test -p egret-perm-eval`, confirm a named test fails, and restore. Record each pair in the task report.
  - (a) Let an allow win over a deny.
  - (b) Ignore `audience.except`.
  - (c) Do not roll table classifications down to columns.
  - (d) Let `SessionUser` match any target.
  - (e) Allow `AdminOnly` with an `Allow { Read }` path statement.
  - (f) Skip the metadata-table rule.
- [ ] **Step 6: Commit** `feat(perm-eval): snapshot, matching, decide and filter`.

---

### Task 4: Row filters and column masks

**Files:**
- Create: `crates/egret-perm-eval/src/values.rs`
- Modify: `src/lib.rs` (`pub mod values;`, re-export `render_mask`)
- Test: `crates/egret-perm-eval/tests/values.rs`

**Interfaces:**
- Consumes: Task 3's `Snapshot` and `EvalError`, plus the statement and audience types of `egret_perm_ir`.
- Produces:

```rust
impl Snapshot {
    /// The AND of every applying row filter on the table, in statement order; empty when none.
    pub fn row_filters(&self, user: &str, catalog: &str, schema: &str, table: &str) -> Result<Vec<String>, EvalError>;
    /// Column name → SQL expression, only for masked columns. `columns` are (name, Trino type display name).
    pub fn column_masks(&self, user: &str, catalog: &str, schema: &str, table: &str, columns: &[(String, String)])
        -> Result<std::collections::BTreeMap<String, String>, EvalError>;
    /// The applying mask before rendering, for explain and the differential: (mask_type, custom expression).
    pub fn column_mask_choice(&self, user: &str, catalog: &str, schema: &str, table: &str, column: &str)
        -> Result<Option<(String, Option<String>)>, EvalError>;
}
/// Ranger Trino plugin rendering: MASK_NULL→"NULL"; CUSTOM→expr or "NULL"; MASK_NONE→None;
/// others→the transformer with {col}→column and {type}→column_type.
pub fn render_mask(mask_type: &str, custom: Option<&str>, column: &str, column_type: &str) -> Option<String>;
pub const TRINO_MASK_TRANSFORMERS: &[(&str, &str)];
```

- **The transformer table**, verbatim from `crates/egret-policy/src/service-defs/trino.json`:

| `mask_type` | transformer |
|---|---|
| `MASK` | `cast(regexp_replace(regexp_replace(regexp_replace({col},'([A-Z])', 'X'),'([a-z])','x'),'([0-9])','0') as {type})` |
| `MASK_SHOW_LAST_4` | `cast(regexp_replace({col}, '(.*)(.{4}$)', x -> regexp_replace(x[1], '.', 'X') \|\| x[2]) as {type})` |
| `MASK_SHOW_FIRST_4` | `cast(regexp_replace({col}, '(^.{4})(.*)', x -> x[1] \|\| regexp_replace(x[2], '.', 'X')) as {type})` |
| `MASK_HASH` | `cast(to_hex(sha256(to_utf8({col}))) as {type})` |
| `MASK_DATE_SHOW_YEAR` | `date_trunc('year', {col})` |

  In the Rust source the `||` is written unescaped.
- **Semantics:**
  - A `RowFilter { expression }` statement applies when its audience applies and its `Target::Path` names exactly this catalog, schema and table (wildcards allowed), or its `Target::Classification` is carried by the table.
  - Several filters become one string, `(e1) AND (e2)`, returned as a one-element vector.
  - A `Mask` matches a column by path (all four levels) or by a classification on the column or its table.
  - A `WithholdMask` counts as `MASK_NULL`.
  - `MASK_NONE` is an applying mask that renders to nothing. It still counts toward the two-mask check.
  - More than one applying mask → `Err(EvalError::TwoMasks { user, column })`.

- [ ] **Step 1: Write the failing tests** `tests/values.rs`, reusing Task 3's helper set (copy the helpers block into this file):

```rust
#[test]
fn row_filters_combine_with_and_in_statement_order() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", Some("s"), Some("t"), None), Effect::RowFilter { expression: "region = 'us'".into() }),
        st(2, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), None), Effect::RowFilter { expression: "tier < 3".into() }),
    ], vec![]);
    assert_eq!(s.row_filters("ann", "lake", "s", "t").unwrap(), vec!["(region = 'us') AND (tier < 3)".to_string()]);
    assert_eq!(s.row_filters("bob", "lake", "s", "t").unwrap(), vec!["(region = 'us')".to_string()]);
    assert!(s.row_filters("bob", "lake", "s", "other").unwrap().is_empty());
}

#[test]
fn a_hash_mask_renders_the_service_def_transformer() {
    let s = snap(vec![st(1, who(&["*"], &["boss"]), path("lake", Some("s"), Some("t"), Some("email")),
                         Effect::Mask { mask_type: "MASK_HASH".into(), expression: None })], vec![]);
    let m = s.column_masks("ann", "lake", "s", "t", &[("email".into(), "varchar".into()), ("id".into(), "bigint".into())]).unwrap();
    assert_eq!(m.get("email").map(String::as_str), Some("cast(to_hex(sha256(to_utf8(email))) as varchar)"));
    assert!(!m.contains_key("id"));
    assert!(s.column_masks("boss", "lake", "s", "t", &[("email".into(), "varchar".into())]).unwrap().is_empty());
}

#[test]
fn null_custom_and_none_render_like_the_ranger_plugin() {
    assert_eq!(egret_perm_eval::render_mask("MASK_NULL", None, "c", "varchar").as_deref(), Some("NULL"));
    assert_eq!(egret_perm_eval::render_mask("CUSTOM", Some("substr(c, 1, 2)"), "c", "varchar").as_deref(), Some("substr(c, 1, 2)"));
    assert_eq!(egret_perm_eval::render_mask("CUSTOM", None, "c", "varchar").as_deref(), Some("NULL"));
    assert_eq!(egret_perm_eval::render_mask("MASK_NONE", None, "c", "varchar"), None);
}

#[test]
fn the_transformers_equal_the_trino_service_def() {
    let def: serde_json::Value = serde_json::from_str(include_str!(
        "../../egret-policy/src/service-defs/trino.json")).unwrap();
    for m in def["dataMaskDef"]["maskTypes"].as_array().unwrap() {
        let name = m["name"].as_str().unwrap();
        let want = m.get("transformer").and_then(|t| t.as_str());
        let got = egret_perm_eval::TRINO_MASK_TRANSFORMERS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t);
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn a_classification_mask_and_a_withhold_mask_reach_the_tagged_column() {
    let pii = ClassifiedColumn { catalog: "lake".into(), schema: "s".into(), table: "t".into(), column: Some("ssn".into()), classification: "pii".into() };
    let s = snap(vec![Statement { source: Source::Rule { rule_id: Uuid::nil(), rule_name: "r".into() },
        audience: who(&["*"], &[]), target: Target::Classification { name: "pii".into() },
        effect: Effect::Mask { mask_type: "MASK_SHOW_LAST_4".into(), expression: None }, derivation: Derivation::AsDeclared }], vec![pii]);
    assert!(s.column_masks("ann", "lake", "s", "t", &[("ssn".into(), "varchar".into())]).unwrap()["ssn"].starts_with("cast(regexp_replace(ssn,"));
}

#[test]
fn two_masks_on_one_column_is_an_error_not_a_choice() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", Some("s"), Some("t"), Some("c")), Effect::Mask { mask_type: "MASK_NULL".into(), expression: None }),
        st(2, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), Some("c")), Effect::Mask { mask_type: "MASK_HASH".into(), expression: None }),
    ], vec![]);
    assert!(matches!(s.column_masks("ann", "lake", "s", "t", &[("c".into(), "varchar".into())]), Err(EvalError::TwoMasks { .. })));
    assert!(s.column_masks("bob", "lake", "s", "t", &[("c".into(), "varchar".into())]).is_ok());
}
```

  Add `serde_json.workspace = true` under `[dev-dependencies]`.

- [ ] **Step 2: Run** `cargo test -p egret-perm-eval --test values`. Expected: FAIL.
- [ ] **Step 3: Implement** `values.rs`.
- [ ] **Step 4: Run** `cargo test -p egret-perm-eval`. Expected: PASS.
- [ ] **Step 5: Mutation checks**, recorded in the report:
  - (a) Join filters with `OR`.
  - (b) Return the first mask instead of erroring on two.
  - (c) Drop the `{type}` replacement.
  - (d) Ignore `WithholdMask`.
- [ ] **Step 6: Commit** `feat(perm-eval): row filters and column masks`.

---

## Phase 2 — the mechanism and the snapshot

### Task 5: Migration 0213, `Mechanism::Opa`, and the profile's mechanism everywhere it is read

**Files:**
- Create: `migrations/0213_profile_mechanism.sql`
- Modify: `crates/egret-perm-ir/src/ep.rs` (`Mechanism::Opa`) and `src/matrix.rs` (the `OPA_TRINO` rows)
- Modify: `crates/egret-perm-compile/src/ir_load.rs:160-168`. `load_ep` reads `svc.mechanism`.
- Modify: `crates/egret-policy/src/store.rs`:
  - `ServiceRow` gains `pub mechanism: String`;
  - every `SELECT` that builds a `ServiceRow` lists `mechanism` (store.rs:915, 958, 973, 998, 1373; find all with `grep -n "FROM policy.authz_services" crates/egret-policy/src/store.rs`).
- Modify: `crates/egret-policy-api/src/dto.rs:12`. `PolicyService` gains `pub mechanism: String`, and the `From<ServiceRow>` impl fills it.
- Modify: `crates/egret-policy/src/admin/mod.rs:437-473` (`ProfileDto` gains `pub mechanism: String`) and `list_profiles` (:837-857).
- Modify: `openapi.json` and `frontend/src/api/generated/**` (regenerated).
- Test: `crates/egret-policy/tests/store_services.rs` (append), `crates/egret-perm-ir/src/matrix.rs` (inline)

**Interfaces:**
- Produces:
  - the column `policy.authz_services.mechanism text NOT NULL DEFAULT 'ranger'`;
  - `egret_perm_ir::ep::Mechanism::{Ranger, Opa}`;
  - `ServiceRow::mechanism`, `PolicyService::mechanism`, `ProfileDto::mechanism`, each `"ranger"` or `"opa"`;
  - `EpInput.kind.mechanism`, which equals the profile's mechanism.

- [ ] **Step 1: Write the migration.**

```sql
-- 0213_profile_mechanism.sql
--
-- Governance SP2b: how a profile's engines are asked. 'ranger' (every profile
-- today) or 'opa' (egret answers Trino's OPA plugin from the IR; spec
-- docs/en/specs/2026-11-04-gov-sp2-trino-opa.md). OPA exists for Trino only
-- (StarRocks #77386 is unmerged), so the type condition is in the schema, not
-- in a predicate a writer has to remember. Every row keeps 'ranger'.
ALTER TABLE policy.authz_services
    ADD COLUMN mechanism text NOT NULL DEFAULT 'ranger';

ALTER TABLE policy.authz_services
    ADD CONSTRAINT authz_services_mechanism_is_known
    CHECK (mechanism IN ('ranger', 'opa'));

ALTER TABLE policy.authz_services
    ADD CONSTRAINT authz_services_opa_is_trino_only
    CHECK (mechanism = 'ranger' OR type = 'trino');
```

- [ ] **Step 2: Write the failing tests.** In `crates/egret-policy/tests/store_services.rs`, using that file's existing DB helper:

```rust
#[tokio::test]
async fn mechanism_defaults_to_ranger_and_opa_is_trino_only() {
    // Arrange: one trino and one starrocks profile via create_service.
    // Assert: both rows read mechanism == "ranger" through get_service.
    // Act/Assert: `UPDATE policy.authz_services SET mechanism='opa' WHERE id=<trino>` succeeds,
    //   and get_service now reads "opa"; the same UPDATE on the starrocks id fails with a
    //   check violation naming `authz_services_opa_is_trino_only`; mechanism='kafka' fails
    //   with `authz_services_mechanism_is_known`.
}
```

  Fill the body with real arrange code, the act calls, and assertions with messages, as described in the comments.

  Then add this test inline in `crates/egret-perm-ir/src/matrix.rs`:

```rust
#[test]
fn opa_trino_rows_answer_like_the_ranger_trino_rows() {
    let ranger = EpKind { engine: EngineType::Trino, mechanism: Mechanism::Ranger };
    let opa = EpKind { engine: EngineType::Trino, mechanism: Mechanism::Opa };
    let caps = [
        Capability::ClassificationSelector(RuleEffectKind::Mask),
        Capability::ClassificationSelector(RuleEffectKind::RowFilter),
        Capability::ClassificationRowFilterOnColumnAttachment,
        Capability::WithholdByClassification, Capability::GroupPrincipal, Capability::RolePrincipal,
        Capability::WithholdExemptionByGroup, Capability::WithholdPrivilege(Privilege::Read),
        Capability::WithholdPrivilege(Privilege::Write), Capability::PoliciesReachMetadataTables,
        Capability::MaskType("MASK_HASH".into()), Capability::MaskType("CUSTOM".into()),
    ];
    for channel in [true, false] {
        for c in &caps {
            assert_eq!(support_for_kind(opa, Some("2.8.0"), channel, c),
                       support_for_kind(ranger, Some("2.8.0"), channel, c), "{c:?} channel={channel}");
        }
    }
}
```

- [ ] **Step 3: Run.**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test store_services mechanism
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-ir opa_trino_rows
```

  Expected: FAIL. The first fails because the field is missing; the second because `Mechanism::Opa` does not exist.

- [ ] **Step 4: Implement.**
  - Add `Opa` to `Mechanism` and fix every exhaustive match `cargo check --workspace --all-targets` reports.
  - In `matrix.rs`, add `const OPA_TRINO: EpKind = EpKind { engine: EngineType::Trino, mechanism: Mechanism::Opa };`. Then add one row per existing `TRINO` row, with the same `versions`, `capability`, `key` and `answer`, and `evidence: "SP2b: the OPA evaluator answers from the same lowered statements; measured by crates/egret-perm-compile/tests/opa_differential.rs"`.
  - `load_ep`: `mechanism: match svc.mechanism.as_str() { "opa" => Mechanism::Opa, _ => Mechanism::Ranger }`.
  - Leave the `Mechanism::Ranger` literals in `ranger/rules.rs:460`, `ranger/completion.rs:587`, `rules.rs:156` and `harness_support.rs:75` unchanged. They name the Ranger target.

- [ ] **Step 5: Regenerate the API.**

```bash
CARGO_TARGET_DIR=$PWD/target-sp2b cargo run -p egret-bin -- openapi > openapi.json
(cd frontend && npm run gen:api && npx tsc -b)
```

- [ ] **Step 6: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test store_services
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-ir
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test golden_compile
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib committed_openapi_json_is_current
git status --short crates/egret-perm-compile/tests/golden
```

  Expected: PASS, and the last command prints nothing.

- [ ] **Step 7: Commit** `feat(policy): a profile's mechanism (migration 0213) and Mechanism::Opa`.

---

### Task 6: `lower_ep_via` and `build_snapshot_via`

**Files:**
- Modify: `crates/egret-perm-compile/src/trigger.rs:375-519` (`compile_desired`). Split the lowering out:

```rust
pub(crate) struct LoweredEp {
    pub loaded: crate::ir_load::Loaded,
    /// Each family's lowering, in the order compile_desired emits them:
    /// grants, engine privileges, rules, withholds.
    pub families: Vec<egret_perm_ir::statement::Lowered>,
    pub whole_tables: <type of WithholdLowered::whole_tables>,
    pub view_guard: Vec<egret_perm_ir::lower::withhold::PendingGuard>,
}
pub(crate) async fn lower_ep_via(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<Option<LoweredEp>, CompileError>;
```

  `compile_desired` calls `lower_ep_via`, then `crate::ranger::emit` once per family in the same order, and then runs the rest unchanged: the StarRocks clearance fold, `completion::complete`, `refuse_overlaps` and the counting. `emit` takes no connection, so emitting after all the reads changes nothing. The golden suite proves it.
- Create: `crates/egret-perm-compile/src/opa/mod.rs` (`pub mod build; pub mod cache; pub mod wire; pub mod surface;`; declare only `build` in this task) and `src/opa/build.rs`.
- Modify: `crates/egret-perm-compile/src/lib.rs` (`pub mod opa;`). Modify `Cargo.toml`: add `egret-perm-eval.workspace = true`.
- Test: `crates/egret-perm-compile/tests/opa_snapshot.rs` (new, DB-gated)

**Interfaces:**
- Consumes:
  - `egret_perm_eval::{Snapshot, SnapshotInput, ClassifiedColumn}`;
  - `crate::ir_load::{load_ep, load_infra_inputs}`;
  - `egret_perm_ir::lower::infra::lower_infra`.
- Produces:

```rust
// crates/egret-perm-compile/src/opa/build.rs
pub async fn load_classified_columns_via(conn: &mut sqlx::PgConnection, service_id: i64)
    -> Result<Vec<egret_perm_eval::ClassifiedColumn>, CompileError>;
/// None: the service is not trino/starrocks. Reads at the caller's transaction.
pub async fn build_snapshot_via(conn: &mut sqlx::PgConnection, service_id: i64)
    -> Result<Option<egret_perm_eval::Snapshot>, CompileError>;
```

  - **`load_classified_columns_via`.** The query is the same join as `egret_policy::tag_assignments_for_service_via` (`crates/egret-policy/src/store.rs:2042`), with no tag-key filter, selecting `tags.name`:

    ```sql
    SELECT DISTINCT c.name AS catalog, a.schema_name, a.table_name, a.column_name, t.name AS classification
      FROM catalog_tag_assignments a
      JOIN tags t ON t.id = a.tag_id
      JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
      JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
      JOIN engine_mgmt.engines e ON e.id = ea.engine_id
     WHERE e.authz_service_id = $1
     ORDER BY 1, 2, 3, 4, 5
    ```

    `column_name` is `NULL` for a table-level assignment. Check the column's nullability in `migrations/0177_catalog_tag_levels_are_present_and_literal.sql`, and map it to `ClassifiedColumn::column`.
  - **`build_snapshot_via`** runs:
    1. `lower_ep_via`;
    2. it collects every family's `statements`;
    3. `load_infra_inputs` → `lower_infra`, appended;
    4. `load_classified_columns_via`;
    5. it reads `SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1` for `key`;
    6. `Snapshot::build`. A `SnapshotError` maps to `CompileError::Db(format!("OPA snapshot: {e}"))`.

- [ ] **Step 1: Write the failing test** `tests/opa_snapshot.rs`. Build the store with the golden fixture builders: `#[path = "golden/fixtures.rs"] mod fixtures;`, plus the `pub struct Names` and `impl Names { id, text }` the fixtures import as `crate::Names`, copied from `tests/golden_compile.rs:13-26`. Each test opens a transaction, builds a state, and calls `egret_perm_compile::opa::build::build_snapshot_via(&mut tx, profile)`.

```rust
#[tokio::test]
async fn the_snapshot_answers_what_the_store_says() {
    // Arrange (fixtures.rs builders): a trino profile with a tag service (profile_with_config),
    // a warehouse + catalog "lake" attached to an engine bound to it (engine_bound_spec with
    // {"trinoConfig":{"delegationMode":"open"}}), users ann and bob, grant Allow Read on
    // scope(lake) to ann, a path mask MASK_HASH on lake.s.t.email for public except bob,
    // tag "pii" on lake.s.t.ssn with gate(roles=["cleared_role"]) held by bob.
    // Act: build_snapshot_via.
    // Assert: decide("ann","SelectFromColumns", Table{lake,s,t,[id]}) is Allow;
    //   decide("ann", …, [ssn]) is Deny{explicit:true} (withheld);
    //   decide("bob", …, [id]) is Deny (no grant);
    //   column_masks("ann", lake,s,t,[(email,varchar)]) has email;
    //   column_masks("bob", …) is empty; snapshot.key() equals the row's (policy_version, tag_version).
}

#[tokio::test]
async fn a_qualifying_engine_puts_the_infra_statements_in_the_snapshot() {
    // Arrange: same profile, engine spec {"trinoConfig":{"coordinator":{},"delegationMode":"impersonate","auth":{"password":{}}}}.
    // Assert: decide("ann","ExecuteQuery",None) Allow; decide("ann","KillQueryOwnedBy",User("ann")) Allow;
    //   decide("ann","KillQueryOwnedBy",User("bob")) Deny;
    //   decide(egret_core::engine_principals::DELEGATOR_PRINCIPAL,"ImpersonateUser",User("ann")) Allow.
}
```

  The split itself is proved by `golden_compile` staying byte-identical (Step 4); no third test.

  Fill each body completely, as described in its comments. Use the builder signatures in `tests/golden/fixtures.rs`: `profile`, `profile_with_config`, `warehouse`, `catalog`, `catalog_attached`, `engine_bound_spec`, `user`, `grant`, `rule`, `tag`, `tag_column`, `gate`, `platform_role` and `role_held`.

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test opa_snapshot`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the split and `opa/build.rs`.
- [ ] **Step 4: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test opa_snapshot
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test golden_compile
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --test recompile
git status --short crates/egret-perm-compile/tests/golden
```

  Expected: PASS, and no golden change.

- [ ] **Step 5: Commit** `feat(perm-compile): build an OPA snapshot from the same lowering the Ranger compile uses`.

---

### Task 7: `SnapshotCache` and the refresher

**Files:**
- Create: `crates/egret-perm-compile/src/opa/cache.rs`, and add `pub mod cache;` to `opa/mod.rs`
- Test: `crates/egret-perm-compile/tests/opa_cache.rs` (new, DB-gated)

**Interfaces:**
- Consumes: Task 6's `build_snapshot_via`, and `crate::compile_runs::record_compile_failure(pool: &PgPool, service_id: i64, error: &str)`.
- Produces:

```rust
pub enum Lookup { NotOpa, NotBuilt { error: Option<String> }, Ready(std::sync::Arc<egret_perm_eval::Snapshot>) }
#[derive(Default)]
pub struct SnapshotCache { /* std::sync::RwLock<HashMap<i64, CacheEntry>> */ }
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport { pub rebuilt: Vec<i64>, pub failed: Vec<i64>, pub dropped: Vec<i64> }
impl SnapshotCache {
    pub fn get(&self, service_id: i64) -> Lookup;
    pub async fn refresh_once(&self, pool: &sqlx::PgPool) -> Result<RefreshReport, sqlx::Error>;
}
pub fn spawn_snapshot_refresher(cache: std::sync::Arc<SnapshotCache>, pool: sqlx::PgPool,
    period: std::time::Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static)
    -> tokio::task::JoinHandle<()>;
```

- **`refresh_once`:**
  - It reads `SELECT id, policy_version, tag_version FROM policy.authz_services WHERE mechanism = 'opa' ORDER BY id`.
  - It removes cache entries whose id is not listed (`dropped`).
  - For each listed id whose cached key differs, or which has no entry:
    1. it begins a transaction and issues `SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY`;
    2. it calls `build_snapshot_via`;
    3. on `Ok(Some(s))`, it stores `Arc::new(s)` with `s.key()` and clears the error (`rebuilt`);
    4. on `Err(e)`, it keeps the previous snapshot, stores `e.to_string()`, logs `tracing::warn!(service_id, error = %e, "OPA snapshot build failed; serving the previous snapshot")`, calls `record_compile_failure(pool, id, &format!("OPA snapshot: {e}"))`, and records the key read, so the same failure is not rebuilt every second (`failed`).
- **`get`:**
  - absent → `NotOpa`;
  - an entry with no snapshot → `NotBuilt { error }`;
  - else `Ready`.
  - It holds the read lock for the clone of the `Arc` only.
- **The refresher** uses `tokio::time::interval(period)` with `MissedTickBehavior::Delay` and `select!` on shutdown, like `spawn_ranger_retire_sweeper` (`crates/egret-engine-mgmt/src/ranger_credential.rs:705`).

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn a_ranger_profile_is_not_cached_and_an_opa_profile_is_built_once_per_version() {
    // Arrange: a trino profile (fixtures) with one grant; UPDATE mechanism='opa'.
    // Act: refresh_once twice. Assert: first report.rebuilt == [id]; second report.rebuilt is empty;
    //   get(id) is Ready with key == row's versions; get(other ranger profile) is NotOpa.
    // Act: add a grant and recompile_service (policy_version moves); refresh_once.
    // Assert: rebuilt == [id] and the new snapshot allows the new grant's user.
}

#[tokio::test]
async fn switching_back_to_ranger_drops_the_entry() {
    // UPDATE mechanism='ranger'; refresh_once → dropped == [id]; get(id) is NotOpa.
}

#[tokio::test]
async fn a_failed_build_keeps_serving_the_previous_snapshot_and_reports_the_failure() {
    // Arrange: build once (Ready). Then write, with the fixtures' `grant` builder (which inserts
    //   by SQL and so bypasses the authoring checks), an Allow Read grant whose principal is
    //   `user_ref("bad@ref")`. `egret_perm_ir::lower::grants::validate_user_ref` refuses a ref
    //   containing '@' with `CompileError::UnusableUserRef`, which aborts the lowering.
    //   Then `UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1`.
    // Act: refresh_once.
    // Assert: report.failed == [id]; get(id) is still Ready with the OLD key;
    //   the perm.compile_runs row for id has outcome='error' and error LIKE 'OPA snapshot:%';
    //   a second refresh_once reports failed == [] (the failing key is remembered).
}
```

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test opa_cache`. Expected: FAIL.
- [ ] **Step 3: Implement** `cache.rs`.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Mutation check.** Rebuild unconditionally (ignore the key). The first test must fail on "second report.rebuilt is empty". Restore it.
- [ ] **Step 6: Commit** `feat(perm-compile): cache OPA snapshots per profile version`.

---

## Phase 3 — the endpoint

### Task 8: The OPA wire format

**Files:**
- Create: `crates/egret-perm-compile/src/opa/wire.rs`, and add `pub mod wire;` to `opa/mod.rs`
- Test: inline `#[cfg(test)]` in `wire.rs`

**Interfaces:**
- Consumes: `egret_perm_eval::{Resource, Decision, Snapshot, EvalError}`.
- Produces:

```rust
pub const ENGINE_PROPERTY: &str = "egret.engine";
pub const SECRET_PROPERTY: &str = "egret.secret";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaRequest {
    pub user: String,
    pub engine: Option<String>,
    pub secret: Option<String>,
    pub operation: String,
    pub resource: Resource,
    pub filter_resources: Vec<Resource>,
}
pub fn parse_request(body: &serde_json::Value) -> Result<OpaRequest, String>;
/// FilterColumns arrives as ONE table resource with `columns`; expand to one Table per column.
pub fn expand_filter_columns(req: &OpaRequest) -> Vec<Resource>;
pub fn allow_result(allowed: bool) -> serde_json::Value;                // {"result": bool}
pub fn batch_result(indices: &[usize]) -> serde_json::Value;            // {"result": [..]}
pub fn row_filters_result(filters: &[String]) -> serde_json::Value;     // {"result": [{"expression": ..}]}
pub fn column_masks_result(masks: &[(usize, String)]) -> serde_json::Value; // {"result": [{"index": i, "viewExpression": {"expression": e}}]}
```

- **Parsing rules** (from D1):
  - `user` = `input.context.identity.user` (required). `identity.groups` is ignored (S7).
  - `engine` and `secret` = `input.context.properties["egret.engine"]` and `["egret.secret"]`.
  - `operation` = `input.action.operation` (required).
  - `resource` = `input.action.resource`, or `Resource::None` when absent.
  - `filter_resources` = `input.action.filterResources`, or empty.
  - A resource object maps by the key present:
    - `user.user` → `User`;
    - `systemSessionProperty.name` → `SystemProperty`;
    - `catalogSessionProperty{catalogName,propertyName}` → `CatalogSessionProperty`;
    - `function{catalogName,schemaName,functionName}` → `Function`;
    - `catalog.name` → `Catalog`;
    - `schema{catalogName,schemaName}` → `Schema`;
    - `table{catalogName,schemaName,tableName,columns?}` → `Table`;
    - `column{catalogName,schemaName,tableName,columnName,columnType}` → `Column`.
  - Anything else is `Err("unrecognised resource: <keys>")`.
  - `Err` never contains the secret.

- [ ] **Step 0: Confirm the FilterColumns and GetColumnMask shapes from the jar.**

```bash
W=$(mktemp -d) && cid=$(docker create trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534) \
 && docker cp "$cid:/usr/lib/trino/plugin/opa/io.trino_trino-opa-483.jar" "$W/opa.jar" && docker rm "$cid" \
 && (cd "$W" && unzip -q opa.jar 'io/trino/plugin/opa/*') \
 && javap -c -p -constants "$W/io/trino/plugin/opa/OpaBatchAccessControl.class" | grep -n "lambda\$filterColumns\$0\|lambda\$batchRequestBuilder\$1" -A25 \
 && javap -c -p -constants "$W/io/trino/plugin/opa/OpaHighLevelClient.class" | grep -n "GetColumnMask" -B5 -A25; rm -rf "$W"
```

  Write down what you find in the task report:
  - whether `FilterColumns` puts one `table` resource with `columns` into `filterResources`, with the result indices referring to `columns`;
  - whether `GetColumnMask` on the batch URI puts one `column` resource per masked column in `filterResources`.

  The tests below assume both. If the bytecode says otherwise, change `expand_filter_columns` and the tests to match it, and note it in the report.

- [ ] **Step 1: Write the failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(action: serde_json::Value) -> serde_json::Value {
        json!({"input": {"context": {"identity": {"user": "ann", "groups": ["g"]},
            "softwareStack": {"trinoVersion": "483"},
            "properties": {"egret.engine": "e1", "egret.secret": "s3cr3t"}}, "action": action}})
    }

    #[test]
    fn a_select_parses_to_a_table_with_columns() {
        let r = parse_request(&input(json!({"operation": "SelectFromColumns",
            "resource": {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["a", "b"]}}}))).unwrap();
        assert_eq!(r.user, "ann");
        assert_eq!(r.engine.as_deref(), Some("e1"));
        assert_eq!(r.resource, Resource::Table { catalog: "lake".into(), schema: "s".into(), table: "t".into(), columns: vec!["a".into(), "b".into()] });
    }

    #[test]
    fn filter_columns_expands_to_one_resource_per_column() {
        let r = parse_request(&input(json!({"operation": "FilterColumns", "filterResources": [
            {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["a", "b"]}}]}))).unwrap();
        let x = expand_filter_columns(&r);
        assert_eq!(x.len(), 2);
        assert_eq!(x[1], Resource::Table { catalog: "lake".into(), schema: "s".into(), table: "t".into(), columns: vec!["b".into()] });
    }

    #[test]
    fn a_column_mask_request_parses_each_column_with_its_type() {
        let r = parse_request(&input(json!({"operation": "GetColumnMask", "filterResources": [
            {"column": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columnName": "email", "columnType": "varchar"}}]}))).unwrap();
        assert!(matches!(&r.filter_resources[0], Resource::Column { column_type, .. } if column_type == "varchar"));
    }

    #[test]
    fn a_malformed_body_is_an_error_that_does_not_echo_the_secret() {
        let e = parse_request(&input(json!({"operation": "SelectFromColumns", "resource": {"bogus": {}}}))).unwrap_err();
        assert!(!e.contains("s3cr3t"), "{e}");
        assert!(parse_request(&json!({"input": {}})).is_err());
    }

    #[test]
    fn results_have_the_plugins_shapes() {
        assert_eq!(allow_result(false), json!({"result": false}));
        assert_eq!(batch_result(&[0, 2]), json!({"result": [0, 2]}));
        assert_eq!(row_filters_result(&["(a) AND (b)".into()]), json!({"result": [{"expression": "(a) AND (b)"}]}));
        assert_eq!(column_masks_result(&[(1, "NULL".into())]), json!({"result": [{"index": 1, "viewExpression": {"expression": "NULL"}}]}));
    }
}
```

- [ ] **Step 2: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --lib opa::wire`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(perm-compile): parse OPA plugin requests and build its results`.

---

### Task 9: `opa_surface`, authentication and the credential cache

**Files:**
- Create: `crates/egret-perm-compile/src/opa/surface.rs`, and add `pub mod surface;` to `opa/mod.rs`. Re-export `opa_surface`, `OpaState` and `OPA_SURFACE_PREFIX` from `lib.rs`.
- Modify: `crates/egret-perm-compile/Cargo.toml`. Add `egret-engine-mgmt-api.workspace = true` if it is not already a dependency (`grep -n engine-mgmt-api crates/egret-perm-compile/Cargo.toml`).
- Test: `crates/egret-perm-compile/tests/opa_surface.rs` (new; no DB, stub verifier and a hand-built cache)

**Interfaces:**
- Consumes:
  - Task 7's `SnapshotCache` and `Lookup`;
  - Task 8's wire functions;
  - `egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity}`.
- Produces:

```rust
pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";
#[derive(Clone)]
pub struct OpaState {
    pub cache: std::sync::Arc<crate::opa::cache::SnapshotCache>,
    pub verifier: std::sync::Arc<dyn egret_engine_mgmt_api::credential::EngineCredentialVerifier>,
    pub credentials: std::sync::Arc<CredentialCache>,
}
pub struct CredentialCache { /* Mutex<HashMap<(Uuid, [u8; 32]), (Option<EngineIdentity>, Instant)>> */ }
impl CredentialCache {
    pub fn new(positive_ttl: Duration, negative_ttl: Duration) -> Self;
    pub async fn verify(&self, v: &dyn EngineCredentialVerifier, engine: Uuid, secret: &str) -> Result<Option<EngineIdentity>, String>;
}
pub fn opa_surface(state: OpaState) -> axum::Router;   // routes under OPA_SURFACE_PREFIX
impl crate::opa::cache::SnapshotCache {
    /// Test seam: insert a ready snapshot for a service.
    #[doc(hidden)] pub fn insert_for_test(&self, service_id: i64, snapshot: egret_perm_eval::Snapshot);
}
```

- **Routes** (`axum::routing::post`, JSON body `serde_json::Value`):
  - `/allow` (`opa.policy.uri`);
  - `/batch` (`opa.policy.batched-uri`);
  - `/row_filters` (`opa.policy.row-filters-uri`);
  - `/batch_column_masks` (`opa.policy.batch-column-masking-uri`).
- **Each handler:**
  1. `parse_request`. On error, the refusal "malformed input: {e}".
  2. `engine` must parse as a `Uuid`, and `secret` must be present. Otherwise, the refusal "no engine credential in opa.context-file".
  3. `credentials.verify(...)`:
     - `Ok(None)` → the refusal "engine credential refused";
     - `Err(e)` → log `error!` and HTTP 500;
     - an identity with `authz_service_id: None` → the refusal "engine is bound to no profile".
  4. `cache.get(service_id)`:
     - `NotOpa` → the refusal "profile is not on OPA";
     - `NotBuilt { error }` → the refusal "no snapshot yet: {error or 'building'}";
     - `Ready(s)` → answer.
  5. Answers:
     - `/allow`: `s.decide(user, op, &resource)`.
     - `/batch`: `s.filter(user, op, resources)`, where `resources` is `expand_filter_columns(&req)` for `FilterColumns` and `req.filter_resources` otherwise.
     - `/row_filters`: `resource` must be a `Table`; `s.row_filters`.
     - `/batch_column_masks`: every `filter_resources[i]` is a `Column`, grouped by table and passed to `s.column_masks`; returns `(i, expr)` for the masked ones.
- **A refusal** is HTTP 200 with `allow_result(false)` on `/allow` and `{"result": []}` on the other three. It also logs `tracing::info!(target: "egret::opa", engine = %engine_or_dash, user = %user_or_dash, operation = %op_or_dash, resource = ?resource_or_none, reason = %reason, "opa: denied")`.
- **Every `Decision::Deny`** logs the same line with its reason. Every allow increments `metrics::counter!("egret_opa_allow_total", "route" => route)`, and every deny increments `egret_opa_deny_total`. Add `metrics.workspace = true` to `Cargo.toml` if absent.
- **`EvalError::TwoMasks`** → HTTP 500 with body `{"error": "two masks on one column"}`, logged at `error`.
- **`CredentialCache`** keys on `(engine, sha256(secret))`. The secret itself is never stored. Positive entries live `positive_ttl` (production: 30 s) and negative entries `negative_ttl` (5 s). `sha2` is already a dependency.

- [ ] **Step 1: Write the failing tests** `tests/opa_surface.rs`, driving the router with `tower::ServiceExt::oneshot`. Add `tower` as a dev-dependency if absent; `crates/egret-server/Cargo.toml` uses `tower.workspace = true`. Fixtures:
  - a `Stub` verifier that accepts `(GOOD, "right")` with `authz_service_id: Some(1)`, accepts `(UNBOUND, "right")` with `None`, and answers `Ok(None)` otherwise, counting calls in an `AtomicUsize`;
  - a cache where service 1 holds a snapshot (built with `Snapshot::build` from one `Allow Read` statement for `ann` on `lake`, plus one `Mask MASK_NULL` on `lake.s.t.c` for everyone, plus one `RowFilter "x = 1"` on `lake.s.t`);
  - service 2 is not inserted (a Ranger profile).

  Then, **for each of the four routes**, one test per case:

```rust
async fn post(app: &axum::Router, route: &str, engine: &str, secret: &str, action: serde_json::Value) -> (u16, serde_json::Value);

#[tokio::test] async fn right_secret_answers_from_the_snapshot_on_every_route() { /* allow→true for ann select lake.s.t[c];
    batch FilterCatalogs [x, lake]→[1]; row_filters lake.s.t→[{"expression":"(x = 1)"}];
    batch_column_masks [column c]→[{"index":0,"viewExpression":{"expression":"NULL"}}] */ }
#[tokio::test] async fn wrong_secret_is_a_200_denial_on_every_route() { /* allow→false; others→[] */ }
#[tokio::test] async fn unknown_engine_is_a_200_denial_on_every_route() { /* random uuid */ }
#[tokio::test] async fn a_ranger_profile_is_a_200_denial_on_every_route() { /* stub binds GOOD2 to service 2 */ }
#[tokio::test] async fn an_unbound_engine_is_a_200_denial() {}
#[tokio::test] async fn a_missing_context_property_is_a_200_denial() {}
#[tokio::test] async fn two_masks_answer_500_so_the_plugin_fails_closed() {}
#[tokio::test] async fn the_credential_cache_asks_the_verifier_once_within_the_ttl() {
    /* two allow calls with the right secret → stub call count == 1; a wrong secret twice → 1 more call */ }
#[tokio::test] async fn no_log_or_body_carries_the_secret() {
    /* capture with tracing_subscriber's test writer (dev-dep `tracing-subscriber` with "fmt" if absent),
       send a wrong secret "s3cr3t-value"; assert the captured output and every body lack it */ }
```

  Fill each body completely. Assert the status and the exact JSON.

- [ ] **Step 2: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --test opa_surface`. Expected: FAIL.
- [ ] **Step 3: Implement** `surface.rs`.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Mutation checks**, recorded in the report:
  - (a) Accept any secret when the engine is known.
  - (b) Answer `NotOpa` from the snapshot of the engine's profile anyway.
  - (c) Return `[]` on `TwoMasks`.
- [ ] **Step 6: Commit** `feat(perm-compile): the OPA decision surface`.

---

### Task 10: Mount the surface and run the refresher

**Files:**
- Modify: `crates/egret-server/src/lib.rs`:
  - `AppState` (:24) gains `pub opa: Option<egret_perm_compile::OpaState>`, with `None` in `AppState::without_db`;
  - after the Ranger merge at :321, add `if let Some(opa) = state.opa.clone() { router = router.merge(egret_perm_compile::opa_surface(opa)); }`.
- Modify: `crates/egret-bin/src/main.rs`.
  - Where `RangerState` is built (:1982), when the Ranger surface is enabled, build:
    - `OpaState { cache, verifier, credentials: Arc::new(CredentialCache::new(Duration::from_secs(30), Duration::from_secs(5))) }`;
    - `verifier` is a clone of the same `Arc<DbVerifier>`, bound to a variable so both states share it;
    - `cache` is `Arc::new(SnapshotCache::default())`.
  - Spawn `spawn_snapshot_refresher(cache, db.pool.clone(), Duration::from_secs(1), <the shutdown future the other sweepers use>)` where `spawn_ranger_retire_sweeper` is spawned.
- Test: `crates/egret-server/tests/opa_surface_mounted.rs` (new)

**Interfaces:**
- Consumes: `egret_perm_compile::{opa_surface, OpaState, OPA_SURFACE_PREFIX}`, `opa::cache::{SnapshotCache, spawn_snapshot_refresher}`, `opa::surface::CredentialCache`.

- [ ] **Step 1: Write the failing test.** Build `AppState::without_db()` with `opa: Some(...)`, using a stub verifier that rejects everything. Then `POST /opa/v1/data/egret/trino/allow` with a minimal valid body answers 200 `{"result": false}`, and `GET` on the same path answers 405. Also assert `/opa/v1/data/egret/trino/batch` exists (200, `{"result": []}`).
- [ ] **Step 2: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --test opa_surface_mounted`. Expected: FAIL.
- [ ] **Step 3: Implement** the mount and the wiring.
- [ ] **Step 4: Run.**

```bash
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --test opa_surface_mounted
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib
```

  Expected: PASS. The OPA routes carry no utoipa annotations: like the Ranger surface, they serve a third-party protocol. So `openapi.json` is unchanged; check with `git diff --stat openapi.json`.
- [ ] **Step 5: Commit** `feat(server): mount the OPA surface and refresh OPA snapshots`.

---

## Phase 4 — the switch

### Task 11: The switch's refusals

**Files:**
- Modify: `crates/egret-perm/src/engine_images.rs:16-23`.
  - `KnownEngineImage` gains `pub opa_plugin: bool`. It is `true` for the `trinodb/trino` `483` row and `false` for the five StarRocks rows.
  - Add `pub fn ships_opa_plugin(engine_type: &str, image_ref: &str) -> bool`. It uses the file's private `parse` and is true only for a known image with `opa_plugin`, matched by repository and tag (a digest, when present, must equal the row's).
- Modify: `crates/egret-policy/src/admin/reader.rs:12`. `EngineBinding` gains `pub image: Option<String>`. `crates/egret-bin/src/main.rs:440` fills it with `egret_perm::stored_engine_image(&e.r#type, &e.spec)`. Every test impl of `EngineBindingReader` sets `image: None`; `grep -rn "EngineBinding {" crates` lists them.
- Create: `crates/egret-policy/src/admin/mechanism.rs`, declared from `admin/mod.rs`.
- Test: `crates/egret-policy/tests/mechanism_switch.rs` (new, DB-gated)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MechanismName { Ranger, Opa }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchRefusal {
    NotTrino { r#type: String },
    NotGenerated,
    EngineImage { engine: String, image: Option<String> },
    AllDefaults { count: i64 },
    HandWritten { service: String, count: i64 },
}
impl SwitchRefusal { pub fn message(&self) -> String; }
/// Every condition, in this order; empty means the switch to OPA may proceed.
pub async fn opa_switch_refusals(conn: &mut sqlx::PgConnection, svc: &crate::ServiceRow,
    bindings: &[crate::EngineBinding]) -> Result<Vec<SwitchRefusal>, crate::PolicyStoreError>;
```

- **The messages, exactly:**

| Refusal | Message |
|---|---|
| `NotTrino` | `"only a Trino profile can use OPA; this profile is of type {type}"` |
| `NotGenerated` | `"only a generated profile can use OPA: hand-written policies are Ranger JSON that OPA cannot evaluate"` |
| `EngineImage` | `"engine {engine} runs {image or 'an unknown image'}, which does not ship the Trino OPA plugin; supported: trinodb/trino:483"` |
| `AllDefaults` | `"the profile holds the `all - …` default policies ({count}); delete them first"` |
| `HandWritten` | `"{service} holds {count} hand-written policies, which OPA would ignore; delete them first"` |

- **The queries:**
  - `AllDefaults`: `SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name LIKE 'all - %'`. This is the same shape migration 0180 matches; `_` and `%` do not appear in `'all - '`.
  - `HandWritten`: `SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND managed_by = 'operator'`, run for the profile and for its tag service (`config_jsonb->>'tagService'` resolved by name).
- **Switching to `Ranger`** has no refusal.

- [ ] **Step 1: Write the failing tests.** One test per condition, each asserting `opa_switch_refusals` returns exactly that one refusal and its message, plus one that returns empty:

```rust
#[tokio::test] async fn a_starrocks_profile_is_refused_as_not_trino() {}
#[tokio::test] async fn a_manual_profile_is_refused_as_not_generated() {}
#[tokio::test] async fn an_engine_on_an_image_without_the_opa_plugin_is_refused_by_name() {
    /* binding image Some("trinodb/trino:470") and one with None → two EngineImage refusals, in binding order */ }
#[tokio::test] async fn all_defaults_are_refused_with_their_count() {
    /* seed_default_trino_policies on a manual trino profile (13 rows), then UPDATE origin='generated' */ }
#[tokio::test] async fn a_hand_written_row_in_the_tag_service_is_refused() {
    /* insert a policy with managed_by='operator' into the tag service */ }
#[tokio::test] async fn a_generated_trino_profile_on_483_switches() { /* empty */ }
```

  Also add an inline unit test in `engine_images.rs` for `ships_opa_plugin`:
  - `true` for `trinodb/trino:483` and `trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534`;
  - `false` for `trinodb/trino:470`, `trinodb/trino:483@sha256:0000`, and every StarRocks image.

- [ ] **Step 2: Run.**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test mechanism_switch
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm ships_opa_plugin
```

  Expected: FAIL.

- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same commands, then `cargo check --workspace --all-targets`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(policy): the conditions under which a profile may switch to OPA`.

---

### Task 12: `PATCH /policies/services/{service}` with `mechanism`

**Files:**
- Modify: `crates/egret-policy/src/admin/mechanism.rs`. Add the handler `set_mechanism`, the body `SetMechanismBody { mechanism: MechanismName }` and the response `ProfileMechanism { name: String, mechanism: MechanismName, engines_reconfigured: Vec<String> }`.
- Modify: `crates/egret-policy/src/admin/mod.rs`:
  - route: `.route("/policies/services/{service}", axum::routing::delete(delete_profile).patch(mechanism::set_mechanism))` at :512;
  - add `set_mechanism` to `PolicyAdminApi` `paths(...)` (:2290);
  - add `SetMechanismBody`, `ProfileMechanism` and `MechanismName` to `components(schemas(...))` (:2307);
  - add `pub reconfigurer: Arc<dyn ProfileEngineReconfigurer>` to `PolicyAdminState` (:80).
- Modify: `crates/egret-audit` action constants. Add `action::PROFILE_MECHANISM = "profile.mechanism"` next to `PROFILE_TAG_SERVICE`; find it with `grep -rn "PROFILE_TAG_SERVICE" crates/egret-audit/src`.
- Modify: every `PolicyAdminState { … }` construction. Tests get a `NoReconfigure` stub; `grep -rn "recompiler:" crates --include='*.rs'` lists them. `crates/egret-bin/src/main.rs` gets the real impl in Task 15; until then it wires `NoReconfigure`, which logs `warn!` and returns `Ok(vec![])`.
- Test: `crates/egret-policy/tests/mechanism_switch.rs` (append)

**Interfaces:**
- Consumes:
  - Task 11's `opa_switch_refusals`;
  - `ProfileRecompiler::recompile_in_tx(&self, tx, &[i64])` (`admin/mod.rs:115`);
  - `crate::store::lock_service_rows_via(conn, &[i64])` (store.rs:2306).
- Produces:

```rust
#[async_trait::async_trait]
pub trait ProfileEngineReconfigurer: Send + Sync {
    /// Re-apply every egret-provisioned engine bound to `service_id`; returns the engine names applied.
    async fn reconfigure_bound_engines(&self, service_id: i64) -> Result<Vec<String>, String>;
}
```

  The route is `PATCH /api/control-plane/policies/services/{service}`, with body `{"mechanism": "opa" | "ranger"}`, permission `policy:update`, and tag `policy-admin`.

- **The handler:**
  1. `require(&*st.pdp, &p, "policy:update")`.
  2. `get_service(&service)`, which answers 404 when there is none.
  3. `st.engines.list_engine_bindings()`, filtered to `authz_service_id == Some(svc.id)`.
  4. Begin a transaction and `lock_service_rows_via(&mut tx, &[svc.id])`.
  5. Re-read the row under the lock. If `mechanism` already equals the requested one, commit and return 200 with `engines_reconfigured: []`. Nothing is written.
  6. For `opa`, run `opa_switch_refusals`. If any refusal holds, answer **409** with body `{"error": <first message>, "code": "mechanism_refused", "refusals": [<every message>]}`, using `PolicyHttpError::Overlap`'s structured-body pattern or a new `ConflictBody { body }` variant mirroring `BadRequestBody`.
  7. `UPDATE policy.authz_services SET mechanism = $2, policy_version = policy_version + 1, updated_at = now() WHERE id = $1`.
  8. `st.recompiler.recompile_in_tx(&mut tx, &[svc.id])`.
  9. Record the governance event `PROFILE_MECHANISM` with summary `{"before": old, "after": new}`.
  10. Commit.
  11. After the commit, call `st.reconfigurer.reconfigure_bound_engines(svc.id)`. An `Err(e)` answers **502** with `{"error": "the profile is now on {new}, but its engines were not reconfigured: {e}; retry with PUT /engines/{id} or restart them"}`. The switch stays committed: the Ranger artifact still compiles (S8), so the engines keep enforcing it until they restart.
- **The utoipa description** of the 200 says: "The coordinator of every bound engine restarts to load the new access control; queries running on it fail."

- [ ] **Step 1: Write the failing route tests.** Drive `policy_admin_router()` as `crates/egret-policy/tests/admin_routes.rs` does; reuse its auth helpers.

```rust
#[tokio::test] async fn switching_a_ready_profile_to_opa_writes_recompiles_and_reconfigures() {
    /* 200; body.mechanism=="opa"; row reads opa; policy_version moved; recompiler stub saw [id];
       reconfigurer stub saw id once; governance event row exists with action 'profile.mechanism' */ }
#[tokio::test] async fn each_refusal_is_a_409_that_names_its_condition_and_writes_nothing() {
    /* for each Task 11 case: 409, body.refusals contains the message, row still 'ranger',
       policy_version unchanged, reconfigurer never called */ }
#[tokio::test] async fn switching_back_to_ranger_is_never_refused() {}
#[tokio::test] async fn the_same_mechanism_again_is_a_no_op() { /* version unchanged, reconfigurer not called */ }
#[tokio::test] async fn a_failed_reconfigure_is_a_502_and_the_switch_stays() {}
#[tokio::test] async fn without_policy_update_it_is_403() {}
```

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test mechanism_switch`. Expected: FAIL.
- [ ] **Step 3: Implement**, then regenerate the API (`cargo run -p egret-bin -- openapi > openapi.json`; `cd frontend && npm run gen:api`).
- [ ] **Step 4: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test mechanism_switch
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test admin_routes
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib committed_openapi_json_is_current
```

  Expected: PASS.
- [ ] **Step 5: Commit** `feat(policy): switch a profile between Ranger and OPA`.

---

## Phase 5 — engine configuration

### Task 13: `opa_plugin_files` and the mechanism on the Ranger binding

**Files:**
- Create: `crates/egret-engine-mgmt-api/src/opa_config.rs`, and add `pub mod opa_config;` to `lib.rs`.
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs:407`. `RangerBinding` gains `pub mechanism: AccessMechanism`, defined in `opa_config.rs`:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub enum AccessMechanism { #[default] Ranger, Opa }`;
  - every `RangerBinding { … }` literal sets `mechanism: AccessMechanism::Ranger` (`grep -rn "RangerBinding {" crates`).
- Modify: `crates/egret-policy/src/admin/mod.rs:1137-1164` (`trino_config`). For a profile whose `mechanism == "opa"`, the response files are `opa_plugin_files(PLACEHOLDER_ENGINE_ID, &st.engine_egret_url, PASSWORD_PLACEHOLDER)`, and `warnings` adds `OPA_LOG_REQUESTS_WARNING`. `TrinoConfigResponse` gains `pub mechanism: String`.
- Test: inline in `opa_config.rs`; `crates/egret-policy/tests/admin_routes.rs` (append one test)

**Interfaces:**
- Produces:

```rust
pub const OPA_CONTEXT_FILE: &str = "egret-opa-context.properties";
pub const OPA_CONTEXT_PATH: &str = "/etc/trino/egret-opa-context.properties";
pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";   // equal to egret_perm_compile's; pinned by a test in Task 15
pub const PLACEHOLDER_ENGINE_ID: &str = "REPLACE_WITH_THE_ENGINE_ID";
pub const OPA_LOG_REQUESTS_WARNING: &str = "Do not enable opa.log-requests: every request carries egret.secret in input.context.properties, and the coordinator log would hold it.";
pub fn opa_plugin_files(engine_id: &str, egret_url: &str, secret: &str) -> crate::ranger_config::RangerPluginFiles;
```

  `opa_plugin_files` returns, in order, `("access-control.properties", …)` and `(OPA_CONTEXT_FILE, …)`. The base URL is `{egret_url without trailing /}{OPA_SURFACE_PREFIX}`. The first file is exactly:

```properties
access-control.name=opa
opa.policy.uri=<base>/allow
opa.policy.batched-uri=<base>/batch
opa.policy.row-filters-uri=<base>/row_filters
opa.policy.batch-column-masking-uri=<base>/batch_column_masks
opa.allow-permission-management-operations=false
opa.log-requests=false
opa.log-responses=false
opa.context-file=/etc/trino/egret-opa-context.properties
```

  The second is `egret.engine=<engine_id>\negret.secret=<secret>\n`. A secret containing `\n`, `\r` or `\\` is refused by the caller; `RenderableCredential` already guarantees a URL-safe base64 value.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn opa_files_point_every_uri_at_egret_and_pin_the_safe_settings() {
    let f = opa_plugin_files("e-1", "http://egret:8080/", "pw");
    let ac = f.get("access-control.properties").unwrap();
    assert!(ac.starts_with("access-control.name=opa\n"));
    assert!(ac.contains("opa.policy.uri=http://egret:8080/opa/v1/data/egret/trino/allow\n"));
    assert!(ac.contains("opa.policy.batch-column-masking-uri=http://egret:8080/opa/v1/data/egret/trino/batch_column_masks\n"));
    assert!(ac.contains("opa.allow-permission-management-operations=false\n"));
    assert!(ac.contains("opa.log-requests=false\n"));
    assert!(ac.contains(&format!("opa.context-file={OPA_CONTEXT_PATH}\n")));
    assert_eq!(f.get(OPA_CONTEXT_FILE).unwrap(), "egret.engine=e-1\negret.secret=pw\n");
    assert!(!ac.contains("pw"), "the secret lives only in the context file");
}
```

  In `admin_routes.rs`: `trino_config_of_an_opa_profile_shows_the_opa_files_and_the_log_warning`. Set `mechanism = 'opa'` by SQL on a generated Trino profile, then `GET …/trino-config`. Assert that `mechanism == "opa"`, that the file names are `access-control.properties` and `egret-opa-context.properties`, that the password is the placeholder, and that `warnings` contains `OPA_LOG_REQUESTS_WARNING`.

- [ ] **Step 2: Run.**

```bash
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-engine-mgmt-api opa_config
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test admin_routes trino_config
```

  Expected: FAIL.
- [ ] **Step 3: Implement.** Then regenerate the API (`openapi.json`, `npm run gen:api`).
- [ ] **Step 4: Run** the same commands plus `cargo check --workspace --all-targets`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(engine-mgmt-api): render the Trino OPA plugin files`.

---

### Task 14: Both provisioners render the mechanism

**Files:**
- Modify: `crates/egret-provisioner/src/trino.rs`:
  - `trino_config_map` (:300). When `spec.ranger` is `Some(r)` and `r.mechanism == AccessMechanism::Opa`, `access-control.properties` is the first file of `opa_plugin_files(<engine id>, &r.egret_url, PASSWORD_PLACEHOLDER)`. Pass the placeholder because the ConfigMap must never carry the secret. Otherwise it is unchanged.
  - `trino_ranger_secret` (:481). For OPA, `stringData` is `{ OPA_CONTEXT_FILE: <the context file rendered with the real credential> }`, instead of the XML. Same Secret name and labels.
  - `trino_deployment` (:674). For OPA, the `ranger-secret` volume mounts `{"mountPath": OPA_CONTEXT_PATH, "subPath": OPA_CONTEXT_FILE}` instead of the XML mount (:749-751).
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs:407`. `RangerBinding` gains `pub engine_id: String`, which is the `egret.engine` value (the engine's uuid). Set it in `crates/egret-engine-mgmt/src/engines.rs::ranger_binding` (:1008) from `engine.id.to_string()`, and in every other literal.
- Modify: `crates/egret-provisioner/src/docker/plan.rs`. `file_mode` (:224) returns `0o600` for a path ending in `/egret-opa-context.properties` (add `OPA_CONTEXT_SUFFIX` next to `RANGER_SECURITY_XML_SUFFIX` at :236). `resolve_mounts` already maps `ranger-secret` sub-paths to the Secret's `stringData`, so the new mount needs no other change.
- Test: inline tests in `trino.rs` and `docker/plan.rs`

**Interfaces:**
- Consumes: Task 13's `opa_plugin_files`, `AccessMechanism`, `OPA_CONTEXT_FILE` and `OPA_CONTEXT_PATH`.
- Produces: an `EngineSpec` whose `ranger.mechanism == Opa` renders an OPA coordinator. `config_checksum` (:530) already hashes the ConfigMap and the Secret, so a mechanism change rolls the pods. The Docker update recreates every container (`docker/provisioner.rs:231`).

- [ ] **Step 1: Write the failing tests** in `trino.rs`'s test module. Base them on the existing spec builder those tests use (`grep -n "fn spec_with_ranger\|fn test_spec" crates/egret-provisioner/src/trino.rs`):

```rust
#[test]
fn an_opa_binding_renders_opa_access_control_and_a_context_secret() {
    let mut spec = /* existing helper with ranger: Some(binding) */;
    spec.ranger.as_mut().unwrap().mechanism = AccessMechanism::Opa;
    let cm = trino_config_map(&spec);
    let ac = cm["data"]["access-control.properties"].as_str().unwrap();
    assert!(ac.starts_with("access-control.name=opa\n"));
    assert!(!ac.contains(spec.ranger.as_ref().unwrap().credential.as_ref().unwrap().password()));
    let sec = trino_ranger_secret(&spec).unwrap();
    assert!(sec["stringData"]["egret-opa-context.properties"].as_str().unwrap().contains("egret.secret="));
    assert!(sec["stringData"].get("ranger-trino-security.xml").is_none());
    let dep = trino_deployment(&spec, Role::Coordinator);
    let mounts = dep["spec"]["template"]["spec"]["containers"][0]["volumeMounts"].as_array().unwrap();
    assert!(mounts.iter().any(|m| m["mountPath"] == "/etc/trino/egret-opa-context.properties"));
    assert!(!mounts.iter().any(|m| m["mountPath"] == "/etc/trino/ranger-trino-security.xml"));
}

#[test]
fn switching_mechanism_changes_the_pod_checksum() {
    let ranger = /* helper */;
    let mut opa = ranger.clone();
    opa.ranger.as_mut().unwrap().mechanism = AccessMechanism::Opa;
    assert_ne!(config_checksum(&ranger), config_checksum(&opa));
}
```

  In `docker/plan.rs`: `an_opa_coordinator_gets_its_context_file_mode_0600`. Call `plan_trino` with an OPA spec. The coordinator's `files` contains `/etc/trino/egret-opa-context.properties` with `mode == 0o600`, and no `ranger-trino-security.xml`.

- [ ] **Step 2: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-provisioner opa`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p egret-provisioner` (all of it). Expected: PASS. The existing Ranger snapshots are unchanged.
- [ ] **Step 5: Commit** `feat(provisioner): render Trino's OPA access control for an OPA profile`.

---

### Task 15: Apply reads the mechanism; re-apply on switch; the external route

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`.
  - Add `pub(crate) async fn resolve_profile_mechanism(st: &CpState, authz_service_id: Option<i64>) -> Result<AccessMechanism, CpError>`, reading `PolicyService::mechanism` from `st.authz.list_services()`, like `resolve_ranger_service_name` (:2838).
  - `build_provision_req_with` (:749-770) passes it into `ranger_binding`, which sets `RangerBinding::mechanism`.
- Modify: `crates/egret-engine-mgmt/src/store.rs`. Add `pub async fn list_engines_bound_to_authz_service(&self, service_id: i64) -> Result<Vec<Engine>, StoreError>`, next to `count_engines_bound_to_authz_service` (:401).
- Modify: `crates/egret-engine-mgmt/src/engines.rs`. Add `pub async fn reapply_bound_engines(st: &CpState, service_id: i64) -> Result<Vec<String>, CpError>`. For each bound engine with `origin != "external"`:
  1. `build_provision_req(st, &engine)`;
  2. `apply_built(st, prov, &engine, &built, ApplyOp::Update)`;
  3. collect its name.

  It mirrors `catalog_apply::propagate` (`catalog_apply.rs:1442`). An `ApplyBuiltError` stops the loop and returns `CpError::BadRequest(format!("re-apply engine {name}: {e}"))`.
- Modify: `crates/egret-engine-mgmt/src/external.rs`:
  - add `.route("/engines/{id}/opa-plugin-config", get(opa_plugin_config))` (:179);
  - add `opa_plugin_config` to `ExternalEngineApi` `paths(...)` (:161).
  - The handler mirrors `ranger_plugin_config` (:847): `engine:read`, 409 unless external, 409 unless bound, and 409 with `"profile {name} is on Ranger; use /ranger-plugin-config"` unless the profile is on OPA. It returns `OpaPluginConfigDto { files: Vec<RangerPluginFileDto>, warnings: Vec<String>, admin_must_supply: Vec<String> }`, from `opa_plugin_files(&engine.id.to_string(), &st.engine_egret_url, PASSWORD_PLACEHOLDER)`:
    - `warnings` = `[OPA_LOG_REQUESTS_WARNING]` + `download_url_warning(&st.engine_egret_url)`;
    - `admin_must_supply` = `["egret.secret: the engine's Ranger plugin credential (rotate it with POST /api/control-plane/engines/{id}/rotate-ranger-credential to see it once)", "a Trino 483 coordinator: plugin/opa ships in trinodb/trino:483"]`.
- Modify: `crates/egret-bin/src/main.rs`. Implement `egret_policy::ProfileEngineReconfigurer` for a `ReconfigurerImpl { cp: Arc<tokio::sync::OnceCell<egret_engine_mgmt::CpState>> }`. Its method fails with "engine management is not wired yet" when the cell is empty, and otherwise calls `egret_engine_mgmt::reapply_bound_engines`. Fill the cell right after the `CpState` is built (:3032). Replace Task 12's `NoReconfigure` in `PolicyAdminState` with it.
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (append); `crates/egret-perm-compile/src/opa/surface.rs` (one inline test pinning the prefix constant)

**Interfaces:**
- Consumes:
  - Task 12's `ProfileEngineReconfigurer`;
  - Task 13's `opa_plugin_files`, `AccessMechanism` and `OPA_LOG_REQUESTS_WARNING`;
  - Task 14's rendering.
- Produces:
  - `egret_engine_mgmt::reapply_bound_engines`, re-exported from `lib.rs`;
  - `GET /api/control-plane/engines/{id}/opa-plugin-config`.

- [ ] **Step 1: Write the failing tests** in `routes.rs`, with that file's test state and provisioner fake (`grep -n "struct FakeProvisioner\|impl EngineProvisioningPort for" crates/egret-engine-mgmt/tests/routes.rs`):

```rust
#[tokio::test] async fn an_opa_profiles_engine_is_applied_with_the_opa_binding() {
    /* bind a trino engine to a profile, UPDATE mechanism='opa', call reapply_bound_engines;
       the fake provisioner's last `update` spec has ranger.mechanism == Opa and ranger.engine_id == engine id */ }
#[tokio::test] async fn reapply_skips_external_engines_and_names_the_applied_ones() {}
#[tokio::test] async fn opa_plugin_config_for_an_external_engine_on_an_opa_profile() {
    /* 200; files[0].name == "access-control.properties"; files[1].content contains
       "egret.engine=<id>" and the PASSWORD_PLACEHOLDER; warnings[0] == OPA_LOG_REQUESTS_WARNING */ }
#[tokio::test] async fn opa_plugin_config_refuses_a_ranger_profile_with_409() {}
#[tokio::test] async fn opa_plugin_config_refuses_a_provisioned_engine_with_409() {}
```

  Inline in `surface.rs`: `assert_eq!(OPA_SURFACE_PREFIX, egret_engine_mgmt_api::opa_config::OPA_SURFACE_PREFIX);`.

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-engine-mgmt --test routes opa`. Expected: FAIL.
- [ ] **Step 3: Implement**, then regenerate the API.
- [ ] **Step 4: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-engine-mgmt --test routes
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --lib opa
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib committed_openapi_json_is_current
```

  Expected: PASS.
- [ ] **Step 5: Commit** `feat(engine-mgmt): apply the profile's mechanism and serve the OPA plugin config`.

---

## Phase 6 — explain

### Task 16: `/perm/explain*` answers an OPA profile through the evaluator

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs`:
  - in `explain_users` (:2608), inside the per-profile loop after `engine_type_for` (:2687), branch on `svc.mechanism == "opa"`;
  - `ExplainProfile` (:694) gains `pub mechanism: WireMechanism` (always set) and `#[serde(skip_serializing_if = "Option::is_none")] pub opa: Option<ExplainOpa>`;
  - `ExplainAccess` gains `#[serde(skip_serializing_if = "Option::is_none")] pub ir_reason: Option<String>`.
- Modify: `crates/egret-perm-compile/src/explain_wire.rs`. Add `#[derive(Serialize, ToSchema)] #[serde(rename_all = "lowercase")] pub enum WireMechanism { Ranger, Opa }` and `pub struct ExplainOpa { pub column_mask: Option<String>, pub row_filter: Option<String> }`.
- Modify: `crates/egret-perm-compile/src/explain_effect.rs`. Add `pub fn opa_effect_lines(subject: &str, action: Action, target: &Scope, profile: &ExplainProfile) -> Vec<ExplainEffectLine>`.
- Test: `crates/egret-perm-compile/tests/explain_opa.rs` (new, DB-gated)

**Interfaces:**
- Consumes:
  - Task 6's `build_snapshot_via`;
  - Task 3's `decide_privilege`, `ObjectPath` and `Decision`;
  - Task 4's `row_filters` and `column_mask_choice`;
  - `render_mask`.
- **The OPA branch, per profile:**
  1. `let snap = crate::opa::build::build_snapshot_via(&mut *conn, service_id).await?` (in the explain's own read-only transaction; `None` cannot happen for a Trino profile).
  2. Build `ObjectPath { catalog: catalog_name, schema: target.schema, table: target.table, column: target.column }`.
  3. Accesses:
     - `read` → one `ExplainAccess { access_type: "select" }`;
     - `write` → two, `"insert"` and `"delete"`, each from `decide_privilege(user, Write, …)`.
  4. Map `Decision::Allow { by }` to `WireDecision::Allowed { policy_id: None, policy_name: by }`.
  5. Map `Decision::Deny { reason, explicit }` to `WireDecision::Denied { policy_id: None, policy_name: String::new(), reason: if explicit { DenyPolicyItem } else { NoPolicyMatched } }`.
  6. `ir_reason: Some(reason or by)`; `attribution: None`.
  7. For a `read` naming a table:
     - `opa.row_filter` = `row_filters(...)` joined (`None` when empty);
     - when a column is named, `opa.column_mask` = `column_mask_choice` rendered with `render_mask(mask_type, custom, column, "varchar")`. The type is unknown to explain, so the line says "masked by {mask_type}" and shows the expression with `varchar`.
  8. `mask: None`, `row_filter: None`, `tags: vec![]`, `tag_policies_in_download: 0`, `clearance_masks_missing: vec![]`, `masked_read_skips_access: false`, `other_columns_access` as the Ranger path computes it, or its empty value.
  9. `effect_lines = opa_effect_lines(...)`.
  10. `mechanism: Opa`. Ranger profiles get `mechanism: Ranger` and are otherwise unchanged.
- **`opa_effect_lines`:**
  - one `EffectAbout::Access` line per access, `Allowed` or `Refused`, with text `"{subject} {is|is not} allowed to {access_type} {target}: {ir_reason}"`;
  - for a read with a column, a `Column` line, `Masked` (with a `Code` part for the expression) or `RealValue`;
  - for a read with a table, a `Rows` line, `Filtered` (with a `Code` part) or `AllRows`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test] async fn an_opa_profile_is_explained_by_the_evaluator_with_its_reason() {
    /* fixture: generated trino profile with a grant Read on lake to ann and a withhold on pii (bob cleared);
       UPDATE mechanism='opa'; POST /perm/explain {subject ann, read, lake.s.t.ssn} →
       profiles[0].mechanism=="opa", accesses[0].decision is denied, ir_reason mentions "pii";
       the same for bob → allowed */ }
#[tokio::test] async fn an_opa_profile_reports_its_mask_and_row_filter() {}
#[tokio::test] async fn a_ranger_profile_answers_exactly_as_before_plus_mechanism_ranger() {
    /* compare the JSON of an existing explain test's response minus the new "mechanism" key */ }
```

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test explain_opa`. Expected: FAIL.
- [ ] **Step 3: Implement**, then regenerate the API.
- [ ] **Step 4: Run, one at a time:** `--test explain_opa`, `--test explain_principal` and `--test routes explain`, then `cd frontend && npx tsc -b && npm test -- --run explain`. The explain page's TS types gain the optional fields; existing UI tests stay green. Expected: PASS.
- [ ] **Step 5: Commit** `feat(perm-compile): explain an OPA profile through the IR evaluator`.

---

## Phase 7 — Layer 1, the decision-level differential

### Task 17: The JVM driver moved, and the operation → Ranger request translation

**Files:**
- Create: `crates/egret-ranger-eval/tests/support/ranger_jvm.rs`. Move `ranger_dir` (differential.rs:4291), `ask_ranger_json` (:4848) and the `static MAVEN: Mutex<()>` here verbatim, as `pub fn`s, together with the surefire-summary assertions they make.
- Modify: `crates/egret-ranger-eval/tests/differential.rs`. Add `#[path = "support/ranger_jvm.rs"] mod ranger_jvm;` and `use ranger_jvm::{ask_ranger_json, ranger_dir};`, and delete the moved bodies.
- Create: `crates/egret-perm-compile/tests/opa_support/to_ranger.rs`
- Test: inline in `to_ranger.rs`, compiled through a tiny `crates/egret-perm-compile/tests/opa_to_ranger.rs` that does `#[path = "opa_support/to_ranger.rs"] mod to_ranger;`

**Interfaces:**
- Produces:

```rust
// to_ranger.rs: the Ranger request(s) the Trino 483 ranger plugin makes for one OPA question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangerAsk {
    pub access_type: String,                     // "select", "_any", "impersonate", …
    pub resource: Vec<(String, String)>,          // ordered (key, value), Trino service-def keys
    pub matching_scope: Option<&'static str>,     // Some("SELF_OR_DESCENDANTS") for _any
}
/// All asks must allow for the question to allow (SelectFromColumns: one per column).
pub fn ranger_asks(operation: &str, resource: &egret_perm_eval::Resource) -> Option<Vec<RangerAsk>>;
```

- **The translation**, derived from `crates/egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt` (Task 1):

| Operation class | Ranger ask(s) |
|---|---|
| `SelectFromColumns`, `CreateViewWithSelectFromColumns` | one ask per column: `select` on `catalog,schema,table,column` |
| `InsertIntoTable`, `UpdateTableColumns` | `insert` on `catalog,schema,table` |
| `DeleteFromTable`, `TruncateTable` | `delete` on `catalog,schema,table` |
| Reach ops and `Filter*` items | `_any` on the object's levels, `SELF_OR_DESCENDANTS` |
| Impersonation ops | `impersonate` on `trinouser` |
| `ExecuteQuery` | `execute` on `queryid=*` |
| `SetSystemSessionProperty` | `alter` on `systemproperty` |
| `SetCatalogSessionProperty` | `alter` on `catalog,sessionproperty` |
| `ExecuteProcedure` | `execute` on `catalog,schema,procedure` |
| `ExecuteFunction`, `CreateViewWithExecuteFunction` | `execute` on `catalog,schema,schemafunction` |
| AdminOnly ops | the access type from the fixture line, on the levels the op names |
| `GetRowFilters`, `GetColumnMask` | `select`, asked as policy type 2 (row filter) and 1 (mask) |

  An operation absent from the fixture answers `None`.

- [ ] **Step 1: Move the JVM driver.** Run `EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture`. Expected: PASS, with the same summary lines as before the move. This needs JDK 17 and Maven; with no `EGRET_TEST_RANGER_E2E` it self-skips, and that skip is not evidence.
- [ ] **Step 2: Write the failing tests** in `to_ranger.rs`:

```rust
#[test]
fn every_operation_in_the_fixture_has_a_translation_whose_access_type_the_fixture_names() {
    let checks = include_str!("../../../egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt");
    for row in egret_perm_eval::trino_ops::TRINO_483_OPERATIONS {
        let r = sample_resource(row.class);            // a resource of the right shape per class
        let asks = ranger_asks(row.name, &r).unwrap_or_else(|| panic!("{} untranslated", row.name));
        let method = method_for(row.name);             // "SelectFromColumns" → "checkCanSelectFromColumns", "Filter*" → "filter*", "Get*" → "get*", else checkCan + name
        let line = checks.lines().find(|l| l.starts_with(&format!("{method}:"))).unwrap_or_else(|| panic!("{method} not in fixture"));
        for a in &asks {
            assert!(line.to_ascii_lowercase().contains(&a.access_type.to_ascii_lowercase()), "{} asks {} but the plugin's {line}", row.name, a.access_type);
        }
    }
}

#[test]
fn a_select_of_two_columns_is_two_asks() {
    let r = egret_perm_eval::Resource::Table { catalog: "lake".into(), schema: "s".into(), table: "t".into(), columns: vec!["a".into(), "b".into()] };
    let asks = ranger_asks("SelectFromColumns", &r).unwrap();
    assert_eq!(asks.len(), 2);
    assert_eq!(asks[1].resource, vec![("catalog".into(), "lake".into()), ("schema".into(), "s".into()), ("table".into(), "t".into()), ("column".into(), "b".into())]);
}
```

  `sample_resource` and `method_for` live in `to_ranger.rs`. `method_for` maps a name to its method:
  - the four query ops: `KillQueryOwnedBy` → `checkCanKillQueryOwnedBy`, `ViewQueryOwnedBy` → `checkCanViewQueryOwnedBy`, `FilterViewQueryOwnedBy` → `filterViewQueryOwnedBy`, `ExecuteQuery` → `checkCanExecuteQuery`;
  - `ExecuteFunction` → `canExecuteFunction` and `CreateViewWithExecuteFunction` → `canCreateViewWithExecuteFunction`. Those two are `public boolean`, not in the fixture's `checkCan|filter|get` set, so assert their access type `EXECUTE` by a direct check instead;
  - `AccessCatalog` → `canAccessCatalog`, by the same direct check (`_ANY`).

- [ ] **Step 3: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --test opa_to_ranger`. Expected: FAIL.
- [ ] **Step 4: Implement** `to_ranger.rs`.
- [ ] **Step 5: Run.** Expected: PASS.
- [ ] **Step 6: Commit** `test(perm-compile): translate OPA questions into the Ranger plugin's requests`.

---

### Task 18: Layer 1, the differential, with `explained.toml`

**Files:**
- Create: `crates/egret-perm-compile/tests/opa_differential.rs` and `crates/egret-perm-compile/tests/opa_differential/explained.toml`
- Modify: `crates/egret-perm-compile/Cargo.toml`. Add `[[test]] name = "opa_differential" required-features = ["harness"]`, and the dev-dependency `toml.workspace = true` (add `toml` to the workspace if absent, pinned to the version already in `Cargo.lock`: `grep -A1 'name = "toml"' Cargo.lock`).
- Modify: `crates/egret-perm-compile/src/harness_support.rs`. Add `pub async fn served_artifact_json_via(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<serde_json::Value, CompileError>`, returning the envelope `routes::served_artifact_via` builds for explain (the plugin download, `tagPolicies` included).
- Modify: `crates/egret-perm-ir/src/matrix.rs`. The `OPA_TRINO` rows' `evidence` names the Layer 1 test that measured each one.

**Interfaces:**
- Consumes:
  - `#[path = "golden/fixtures.rs"] mod fixtures;` (with `crate::Names` copied from `tests/golden_compile.rs:13-26`);
  - `#[path = "../../egret-ranger-eval/tests/support/ranger_jvm.rs"] mod ranger_jvm;`;
  - `#[path = "opa_support/to_ranger.rs"] mod to_ranger;`;
  - `egret_perm_compile::opa::build::build_snapshot_via`;
  - `harness_support::served_artifact_json_via`.
- **The corpus.**
  - `const SEED: u64 = 0x5b2b_0000_0001` (override with `EGRET_OPA_DIFF_SEED`, hex); `const STATES: usize = 40`; `const QUESTIONS: usize = 50`; that is 2000 cases.
  - The RNG is SplitMix64, as in `crates/egret-ranger-eval/tests/differential.rs:211-240`; copy the struct.
- **Each state** is one generated Trino profile with a tag service, one catalog `lake` with schemas `{s1, s2}` and tables `{t1, t2, t3$files}`, columns `{id, email, ssn, region}`, and users `{u0..u4}`. It draws:
  - 0–4 grants: Allow or Deny; Read, Write or both; scope at catalog, schema, table or column; a principal set of users, `public` or a group;
  - 0–3 rules: a path mask, a classification mask (types from `MASK, MASK_HASH, MASK_NULL, MASK_NONE, CUSTOM`), or a row filter `region = '<r>'`; with exemptions;
  - 0–2 classifications on columns or tables, with gates clearing a role held by some users;
  - 0–2 engine grants (`trino.impersonate_user`, `trino.set_system_session_property`);
  - one engine bound with a spec drawn from `open`, `impersonate+auth` and `jwt`, so the infra statements vary;
  - 0–1 platform admin.
- **No state may create two masks on one column for one user.** `refuse_overlaps` refuses that at compile; a refused state is redrawn and counted.
- **Each question** draws a user, an operation from `TRINO_483_OPERATIONS`, and a resource of that operation's shape over the state's vocabulary.
- **Ranger path:**
  1. `recompile_service`;
  2. `served_artifact_json_via`;
  3. for each question, `to_ranger::ranger_asks`, with the request's `tags` filled from the state's own classifications (column tags `SELF`, table tags `ANCESTOR` on a column ask);
  4. one `ask_ranger_json(payload, "2.8.0", "opa-l1")` call for all cases of all states.
- **OPA path:** `build_snapshot_via` → `decide`/`filter`/`row_filters`/`column_mask_choice`.
- **Compared:**
  - allow and deny (all asks must allow);
  - row filter: Ranger `filterExpr` against the evaluator's single expression with the outer parentheses of a single filter removed;
  - mask: Ranger `maskType` and `maskedValue` against `column_mask_choice`.
- **`explained.toml` format:**

```toml
# Every accepted difference between egret-perm-eval and real Ranger on the Layer 1 corpus.
# A difference with no entry fails CI; an entry no case produces fails CI too.
[[difference]]
id = "reach-deny-on-descendant"
operations = ["AccessCatalog", "ShowSchemas", "ShowTables", "FilterCatalogs", "FilterSchemas", "FilterTables"]
mechanism = "Ranger's _any with SELF_OR_DESCENDANTS counts a deny item on a descendant ..."
right = "ranger" # or "egret" or "both"
why = "..."
```

  Each difference found is classified by `(operation, ranger_outcome, opa_outcome)` and matched to an entry by `operations`, plus a predicate function named after the `id` in the test file. The file starts empty. Every entry that is added states its mechanism, from a reading of the Ranger source or the measured case, never a guess.
- **Failure messages** print the seed, the state index, the question, both outcomes and a reproducer command.

- [ ] **Step 1: Write the test.** Gate it with `if std::env::var("EGRET_TEST_RANGER_E2E").as_deref() != Ok("1") { eprintln!("skipped: EGRET_TEST_RANGER_E2E unset"); return; }`, as the Ranger differential does. Then:
  - two asserts at the end: `assert!(unexplained.is_empty(), …)` and `assert!(stale_entries.is_empty(), "explained.toml entries no case produced: {stale_entries:?}")`;
  - a coverage assert: every `OpClass` was asked at least 20 times, and at least 100 cases each were allow, deny, masked and filtered.
- [ ] **Step 2: Run** with an empty `explained.toml`:

```bash
env -u EGRET_ALLOW_DB_SKIP EGRET_TEST_RANGER_E2E=1 CARGO_TARGET_DIR=$PWD/target-sp2b \
  cargo test -p egret-perm-compile --features harness --test opa_differential -- --nocapture
```

  Expected: it runs 2000 cases. Each difference is either:
  - an evaluator bug, fixed in `egret-perm-eval` with a regression test added to `tests/decide.rs` or `tests/values.rs`; or
  - a real semantic difference, written into `explained.toml` with its mechanism.

  Do not widen a predicate to cover a case you have not explained.
- [ ] **Step 3: Mutation check.** Flip deny-wins in `egret-perm-eval` (Task 3, mutation a) and confirm `opa_differential` fails. Restore.
- [ ] **Step 4: Update** the matrix rows' `evidence` strings. Run `cargo test -p egret-perm-ir`.
- [ ] **Step 5: Commit** `test(perm-compile): the OPA decision-level differential against real Ranger`.

---

### Task 19: CI job `opa-differential`

**Files:**
- Modify: `.github/workflows/ci.yml`. Add a job after `ranger-differential` (:218-242).

- [ ] **Step 1: Add the job:**

```yaml
  opa-differential:
    needs: changes
    if: ${{ !cancelled() && (github.event_name != 'pull_request' || needs.changes.result != 'success' || needs.changes.outputs.code == 'true') }}
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_USER: egret
          POSTGRES_PASSWORD: egret
          POSTGRES_DB: egret_test
        ports:
          - 5432:5432
        options: >-
          --health-cmd "pg_isready -U egret"
          --health-interval 5s
          --health-timeout 3s
          --health-retries 10
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-java@v4
        with:
          distribution: temurin
          java-version: "17"
          cache: maven
      - uses: Swatinem/rust-cache@v2
      # The operation table must still equal what the 483 jar sends.
      - name: Operation table covers the 483 plugin
        run: cargo test -p egret-perm-eval --no-fail-fast
      # 40 store states x 50 Trino questions, egret-perm-eval against real Ranger 2.8.0.
      # No `-q`: the surefire summary is asserted by name.
      - name: OPA decision-level differential
        run: env -u EGRET_ALLOW_DB_SKIP EGRET_TEST_RANGER_E2E=1 cargo test -p egret-perm-compile --features harness --test opa_differential --no-fail-fast -- --nocapture
```

  `DATABASE_URL` comes from the workflow-level env (ci.yml:8-28).
- [ ] **Step 2: Check the YAML** with `python3 -I -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"`. Expected: no error.
- [ ] **Step 3: Commit** `ci: run the OPA decision-level differential`.

---

### Task 20: Own compose stack and the live check (the controller runs this, not a subagent)

**Files:**
- Create: `deploy/opa-e2e/docker-compose.yml`, `deploy/opa-e2e/egret.env`, `scripts/opa-e2e/up.sh`, `scripts/opa-e2e/down.sh`, `scripts/opa-e2e/live-check.sh`

**The stack:** compose project **`egret-opa-e2e`**, with `name: egret-opa-e2e` at the top of the file, so `docker compose -f deploy/opa-e2e/docker-compose.yml` can never act on another project. Services:
- `postgres` (`postgres:16`);
- `rustfs` (`rustfs/rustfs:1.0.1`);
- `egret`, built from `deploy/docker/backend.Dockerfile`, image `egret-opa-e2e-backend:dev`, `EGRET_RANGER__ENABLED=true`;
- `trino-opa` and `trino-ranger`, both `trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534`, with `/etc/trino` bind-mounted from `${WORK}/trino-opa/etc` and `${WORK}/trino-ranger/etc`.

  Host ports are `28080` (egret), `28081` (trino-opa) and `28082` (trino-ranger). The demo stack's 18080 is never used.

**`up.sh`** (idempotent):
1. Build and start the stack, then wait for `GET /healthz` on 28080.
2. Log in as the bootstrap admin from `egret.env`.
3. Create warehouse `opa_e2e` and catalog `lake` via the catalog API. Copy the calls from `deploy/seed-demo-catalog.sh`.
4. Create the Trino profile `opa_e2e`.
5. Register the two Trinos as **external** engines bound to it (`POST /api/control-plane/engines/external`, `rangerPluginVersion: "2.8.0"`).
6. Rotate each engine's Ranger credential to read the password once (`POST …/rotate-ranger-credential`).
7. Seed data through `trino-ranger` as the admin: `lake.s1.t1` and `lake.s1.t2`, 1000 rows.
8. Make the profile generated by creating one grant. Then `PATCH /policies/services/opa_e2e {"mechanism":"opa"}`.
9. Fetch `GET /engines/{opa-id}/opa-plugin-config` and `GET /engines/{ranger-id}/ranger-plugin-config`. Write their files into the two `etc` dirs, replacing the placeholders with the real passwords.
10. Restart both Trinos.

**`live-check.sh`:**
1. As admin, grant `ann` read on `lake`, mask `lake.s1.t1.email` with `MASK_HASH`, add a row filter `region = 'us'` on `lake.s1.t1`, and deny `ann` on `lake.s1.t2`.
2. Wait for egret's refresher (2 s).
3. Then, through `trino-opa` as `ann` (with the `trino` CLI inside the container: `docker compose -p egret-opa-e2e exec trino-opa trino --user ann --execute …`):
   - `SELECT email FROM lake.s1.t1 LIMIT 1` returns a 64-hex string;
   - `SELECT DISTINCT region FROM lake.s1.t1` returns only `us`;
   - `SELECT * FROM lake.s1.t2` fails with `Access Denied`;
   - `grep "opa: denied" <egret logs>` shows the deny with `reason=`.
4. Then run the same three statements on `trino-ranger` and print both sides.

- [ ] **Step 1: Write the five files.** Every `docker compose` invocation in them uses `-f deploy/opa-e2e/docker-compose.yml -p egret-opa-e2e`.
- [ ] **Step 2: Rebuild check.** After `up.sh` builds the image, run `docker run --rm --entrypoint sh egret-opa-e2e-backend:dev -c 'strings /usr/local/bin/egret | grep -c opa_surface'`. Expected: ≥ 1, so a stale build cache did not ship an old binary.
- [ ] **Step 3: Run** `scripts/opa-e2e/up.sh && scripts/opa-e2e/live-check.sh`. Expected: every assertion prints PASS. Save the output in the PR body.
- [ ] **Step 4: Run** `scripts/opa-e2e/down.sh` (`docker compose … down -v`). It removes only project `egret-opa-e2e`; `docker ps --filter label=com.docker.compose.project=egret-demo` must show the same containers before and after.
- [ ] **Step 5: Commit** `test(e2e): an own compose stack and the live check for Trino on OPA`.

**This ends package 2b-1.** Open its PR with:
- `Closes` for the SP2b tracking issue, if one exists;
- the live-check output;
- the Layer 1 tally;
- every `explained.toml` entry.

The `egret-data/docs` PR for 2b-1 ships the page of Task 25 without its latency section.

---

# Package 2b-2

### Task 21: Layer 2, the end-to-end differential

**Files:**
- Create: `scripts/opa-e2e/layer2.sh`, `scripts/opa-e2e/scenarios.sql.d/*.sql`, `scripts/opa-e2e/compare.py`
- Modify: `.github/workflows/ci.yml` (job `opa-e2e`)

**Interfaces:**
- Consumes:
  - Task 20's `up.sh`/`down.sh` and project `egret-opa-e2e`;
  - `crates/egret-perm-compile/tests/opa_differential/explained.toml` (Task 18).
- **Scenarios.** One `.sql` file per sub-project 1 golden scenario G01–G23 that is Trino-shaped. Each file holds:
  - a header `-- users: ann,bob,...`;
  - the admin setup as egret API calls in `-- setup: <curl …>` lines, which `layer2.sh` executes;
  - the statements to compare, one per line.
- **The compared statements per scenario:**
  - its `SELECT`s;
  - `SHOW CATALOGS`, `SHOW SCHEMAS FROM lake`, `SHOW TABLES FROM lake.s1`, `SHOW COLUMNS FROM lake.s1.t1`;
  - `SELECT table_name, column_name FROM lake.information_schema.columns ORDER BY 1, 2`;
  - `SELECT count(*) FROM lake.s1."t1$files"`;
  - impersonation: `trino --user egret-delegator --session-user ann`;
  - kill: `CALL system.runtime.kill_query(...)` against another user's query;
  - `SET SESSION query_max_run_time = '1m'`.
- **`compare.py`** (run with `python3 -I`):
  - for each `(scenario, user, statement)`, compares the `trino-ranger` and `trino-opa` outputs: the sorted-row sha256, then the per-column values, then the error class (`Access Denied` or not);
  - writes `layer2-report.json`;
  - exits non-zero on any difference that matches no `explained.toml` entry, or on an entry tagged `layer = 2` that no case produced.

  Add an optional `layer = [1, 2]` key to the TOML format; Task 18's reader treats a missing key as `[1]`.
- **The CI job `opa-e2e`:**
  - runs on PRs whose diff touches `crates/egret-perm-*/**`, `crates/egret-perm-compile/**` or `deploy/opa-e2e/**` (a `dorny/paths-filter` output `opa`, added to the `changes` job);
  - `ubuntu-latest`, with the disk-reclaim step copied from the `rust` job;
  - runs `scripts/opa-e2e/up.sh && scripts/opa-e2e/layer2.sh`, then `down.sh` with `if: always()`;
  - uploads `layer2-report.json` as an artifact.

- [ ] **Step 1: Write the scenarios and `compare.py`.** Add a unit test file `scripts/opa-e2e/test_compare.py`, run with `python3 -I -m unittest scripts/opa-e2e/test_compare.py`. It feeds two outputs differing in one masked column and asserts the exit code is 1 and the report names the column.
- [ ] **Step 2: Run** `scripts/opa-e2e/up.sh && scripts/opa-e2e/layer2.sh; scripts/opa-e2e/down.sh`. Each difference is fixed in the evaluator (with a Layer 1 regression case) or explained. Expected: exit 0.
- [ ] **Step 3: Add the CI job** and validate the YAML as in Task 19.
- [ ] **Step 4: Commit** `test(e2e): Trino-on-OPA against Trino-on-Ranger, end to end`.

---

### Task 22: Latency, measured and reported

**Files:**
- Create: `crates/egret-perm-eval/benches/eval.rs`
- Modify: `crates/egret-perm-eval/Cargo.toml`. Add `[dev-dependencies] criterion = { workspace = true }` and `[[bench]] name = "eval" harness = false`. Add `criterion = { version = "0.5", default-features = false }` to the root `[workspace.dependencies]`; there is none today.
- Create: `scripts/opa-e2e/latency.sh`
- Create: `docs/en/reports/<YYYY-MM-DD>-gov-sp2b-opa-latency.md`, named by the day it is measured.

- **Microbenchmarks.** A generator builds snapshots of 1 000 and 10 000 statements: grants over 100 catalogs × 10 schemas × 10 tables, masks and row filters on 5 % of tables, and the infra set. Benches:
  - `decide/select_8_columns`;
  - `filter/FilterTables_1000`;
  - `column_masks/500_columns`;

  each at both sizes.
- **End to end** (`latency.sh` on the `egret-opa-e2e` stack):
  - **Setup.** Create 1 000 tables in `lake.s1` and one table with 500 columns.
  - **Planning time and p50/p99,** for each Trino over 30 runs of:
    - `SHOW TABLES FROM lake.s1`;
    - `SELECT * FROM lake.s1.wide LIMIT 1`;
    - `SELECT count(*) FROM lake.information_schema.columns` (Trino #25628).

    Take them from `EXPLAIN ANALYZE` and `system.runtime.queries`.
  - **Requests per query:** count the `egret_opa_allow_total + egret_opa_deny_total` delta from `GET /metrics` around each query. Run once with the batched URI and once with `opa.policy.batched-uri` removed from `trino-opa`'s config.
  - **Restart window:** run `SELECT 1` in a loop every 100 ms through `trino-opa`, `docker compose … restart egret`, and report the seconds between the first and last failure.
- **The report** states:
  - the machine;
  - every number, with the command that produced it;
  - the conclusion on S2: is a sidecar needed? Say yes or no, and name the measured cause.

- [ ] **Step 1: Write the benches.** Run `CARGO_TARGET_DIR=$PWD/target-sp2b cargo bench -p egret-perm-eval -- --save-baseline sp2b`.
- [ ] **Step 2: Run** `scripts/opa-e2e/up.sh && scripts/opa-e2e/latency.sh > latency.out; scripts/opa-e2e/down.sh`.
- [ ] **Step 3: Write the report** from `latency.out` and the criterion summary. No number is typed in that a command did not print.
- [ ] **Step 4: Commit** `perf(perm-eval): OPA latency measured; report`.

---

### Task 23: The "Ranger | OPA" selector

**Files:**
- Create: `frontend/src/routes/governance/engines/tabs/MechanismSection.tsx`
- Modify: `frontend/src/routes/governance/engines/tabs/AdvancedTab.tsx:185`. Render `<MechanismSection profile={profile} />` when `profile.type === "trino"`, next to `TagServiceSection`.
- Test: `frontend/src/routes/governance/engines/__tests__/MechanismSection.test.tsx`

**Interfaces:**
- Consumes:
  - the generated hook for `PATCH /policies/services/{service}`. Run `grep -n "SetMechanism" frontend/src/api/generated/policy-admin/policy-admin.ts`; the operation id from Task 12 yields `usePolicyAdminSetMechanism`;
  - `ProfileDto.mechanism` (Task 5) and `ProfileDto.origin`;
  - `refusalMessage` from `routes/governance/policyWrites`;
  - `getPolicyAdminListProfilesQueryKey`;
  - `components/ui/{radio-group,alert-dialog,alert}`;
  - the test helpers `serve`, `answers`, `ok`, `fail`, `profileDto`, `STEWARD`, `READER` from `routes/governance/engines/__tests__/engineHarness.tsx`.
- **Behaviour** (frontend rules FE-ACTION-1..4, FE-FORM-2/3):
  - A `Section` titled "Access control", with a `RadioGroup` labelled "How engines are asked". It has two items, "Ranger" and "OPA", with ids and `htmlFor`.
  - The selected value is `dto.mechanism`.
  - The radio is enabled only with `can("policy:update")`. Disabled, it shows the reason "You need policy:update to change this." (FE-ACTION-4).
  - Choosing the other value opens an `AlertDialog`:
    - title "Switch to OPA?" or "Switch to Ranger?";
    - body, verbatim: "Every engine bound to this profile restarts its coordinator to load the new access control. Queries running on it fail."
    - buttons "Cancel" and "Switch".
  - On confirm, it calls the mutation with `{ service: profile.name, data: { mechanism } }`.
    - **On success:** invalidate `getPolicyAdminListProfilesQueryKey()`, `toast.success("Switched to OPA.")` or `"Switched to Ranger."`, and show a `role="status"` line listing `engines_reconfigured`.
    - **On 409:** show every string of the body's `refusals` in an `Alert variant="destructive"`, one per line, and keep the previous selection (FE-ACTION-3).
    - **On 502:** show the server's `error`.

- [ ] **Step 1: Write the failing tests:**

```tsx
it("shows the stored mechanism and asks before switching", async () => {
  serve(answers({ "GET /policies/services": ok([profileDto("lake-tr", { mechanism: "ranger", origin: "generated" })]) }));
  /* render AdvancedTab via the harness, as AdvancedTab.test.tsx does, as STEWARD */
  await user.click(screen.getByLabelText("OPA"));
  expect(await screen.findByText("Every engine bound to this profile restarts its coordinator to load the new access control. Queries running on it fail.")).toBeVisible();
});
it("sends the switch on confirm and reports the reconfigured engines", async () => { /* PATCH body {mechanism:"opa"}; status line names "trino-a" */ });
it("shows every refusal from a 409 and keeps Ranger selected", async () => {
  /* fail(409, …) with body {error, code:"mechanism_refused", refusals:["the profile holds the `all - …` default policies (13); delete them first", "…"]} */ });
it("is read-only without policy:update and says why", async () => { /* READER */ });
```

- [ ] **Step 2: Run** `cd frontend && npx vitest run src/routes/governance/engines/__tests__/MechanismSection.test.tsx`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the gate:**

```bash
cd frontend && npm run lint && npx tsc -b && npx vitest run src/routes/governance/engines && npm run build && npm run test:bundle:governance-engines
```

  Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): choose Ranger or OPA for a Trino profile`.

---

### Task 24: The OPA variant of `TrinoConfigDialog` and of the external engine page

**Files:**
- Modify: `frontend/src/components/policy/TrinoConfigDialog.tsx`. When `config.mechanism === "opa"`:
  - the description reads "Install these two files on the Trino 483 coordinator: the OPA plugin ships in the image, and egret answers its requests.";
  - the warnings, which include the server's OPA log warning, render as they do today;
  - each file renders as today.
- Modify: `frontend/src/api/policies.ts:188-190`. `TrinoConfig` gains `mechanism`; reuse the generated type.
- Modify: `frontend/src/routes/control-plane/ExternalEngineDetail.tsx`. When the bound profile's mechanism is `opa`, query `getControlPlaneEnginesOpaPluginConfig` (the generated hook from Task 15) instead of the Ranger one, at :309, and render its files, warnings and `admin_must_supply` in the same section.
- Test: `frontend/src/components/policy/__tests__/TrinoConfigDialog.test.tsx` (append); `frontend/src/routes/control-plane/__tests__/ExternalEngineDetail.test.tsx` (append, or create following that folder's style)

- [ ] **Step 1: Write the failing tests.**
  - The dialog, with `getTrinoConfig` mocked to return `{mechanism: "opa", files: [{name: "access-control.properties", …}, {name: "egret-opa-context.properties", …}], warnings: ["Do not enable opa.log-requests: …"]}`: it shows both file names and the warning text verbatim.
  - The external page, on an OPA profile: it asks `GET /engines/{id}/opa-plugin-config`, not the Ranger route, and shows the admin-must-supply lines.
- [ ] **Step 2: Run** `cd frontend && npx vitest run src/components/policy src/routes/control-plane`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the gate** as in Task 23, plus `npx vitest run src/routes/control-plane`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(ui): the OPA plugin files for Trino profiles and external engines`.

---

### Task 25: The "Trino on OPA" docs page (in `egret-data/docs`, not this repo)

**Files (in `egret-data/docs`):**
- Create: the Platform section page `trino-on-opa`, in English and Vietnamese per the bilingual convention of that repo. Find the Platform pages' folder with `ls` there, and link the page from the Platform sidebar the same way as its siblings.
- Screenshots:
  - the Advanced tab with the Access control section;
  - the switch dialog;
  - a refused switch showing the `all - …` message;
  - the OPA TrinoConfigDialog.

  Take them from the `egret-opa-e2e` stack (Task 20), never from the shared demo stack.

**The page's content, in this order:**
1. **What it is.** egret answers Trino's built-in OPA plugin (`plugin/opa`, Trino 483) itself. There is no OPA server and no Rego. Decisions come from the same permission model as Ranger.
2. **Who can switch:**
   - a Trino profile;
   - that egret generates (it has grants or rules);
   - whose bound engines run `trinodb/trino:483`;
   - that holds no `all - …` default policies;
   - whose profile and tag service hold no hand-written policies.

   The page lists each refusal message verbatim (Task 11's table) with what to do.
3. **How to switch.** Governance → Engines → the profile's Advanced tab → Access control → OPA → Switch. API: `PATCH /api/control-plane/policies/services/{service}` with `{"mechanism":"opa"}`, which needs `policy:update`.
4. **What the switch does:**
   - it recompiles;
   - it restarts every bound egret-provisioned coordinator, and **queries running on it fail**;
   - the Ranger policies keep compiling, so switching back is immediate.
5. **External engines.** Download the two files from the engine page or `GET /api/control-plane/engines/{id}/opa-plugin-config`. Put the engine's Ranger credential in `egret.secret`; rotate it to see it once. Copy both files into `/etc/trino/`, and restart the coordinator.
6. **Warning: never enable `opa.log-requests`.** Every request carries `egret.secret`.
7. **When egret is unreachable,** Trino denies every query until egret is back. Give the measured restart window from Task 22's report. The 2b-1 docs PR says "measured in a later release" here.
8. **Known differences from Ranger:** one row per `explained.toml` entry (`id`, what differs, which side is right).
9. **Explain.** On an OPA profile, the Explain page answers from the same evaluator and shows its reason.

- [ ] **Step 1:** Write the page and the screenshots in a branch of `egret-data/docs`, and open its PR. Link it from this repo's PR body and tick the Docs box of the PR template.
- [ ] **Step 2:** Check that every message quoted on the page equals the code's text. Grep each quoted sentence in this repo, for example `grep -rn "delete them first" crates/egret-policy/src/admin/mechanism.rs`.

---

## Self-review notes (for the controller)

- **Spec coverage:**

| Spec item | Tasks |
|---|---|
| Evaluator crate and its questions | 1, 3, 4 |
| Operation table covering the jar | 1, 2 |
| Deny wins, AND, one mask | 3, 4, with mutations |
| Explain IR path | 16 |
| Endpoint routes | 9, 10 |
| Authentication via the context file | 8, 9, 13 |
| 200 refusals | 9 |
| Snapshot cache keyed by version; no DB on the decision path | 7, 9 (credential cache, deviation 7) |
| Logging and metrics | 9 |
| Migration and checks | 5 |
| `Mechanism::Opa` | 5 |
| Matrix rows | 5, 18 |
| PATCH with named refusals | 11, 12 |
| Recompile, bump and reconfigure | 12, 15 |
| `opa_plugin_files` | 13 |
| Kubernetes and Docker provisioners | 14 |
| External route | 15 |
| Dialog warning | 13, 24 |
| Rollout warning | 12, 23 |
| Infrastructure features (delegation, kill, `query_max_run_time`) | 3, 6, 18, 21 |
| D-all | 11 |
| UI | 23, 24 |
| Docs | 25 |
| Errors (egret down, failed build, unknown op) | 22, 7, 3 |
| Layer 1 | 17, 18, 19 |
| Layer 2 | 20, 21 |
| Latency | 22 |
| Switch refusal tests | 11, 12 |
| Endpoint auth both directions | 9 |
| Live check | 20 |

- **D-all check, done before writing this plan.**
  - No code path writes an `all - …` row automatically.
  - `seed_default_trino_policies` (`crates/egret-policy/src/default_policies.rs:116`) is reached only from `POST …/seed-defaults` (`admin/mod.rs:524`) and the Advanced tab's button.
  - It refuses a generated profile first (`refuse_if_generated_pooled`), and it no-ops on any profile that already has policies.
  - So a generated profile holds `all - …` rows only if they were seeded while it was manual, before its first compile. The refusal is reachable and does not block the normal path.
- **Risks to watch in review:**
  - The reach semantics (D2) are the least certain rule. Layer 1's `_any` cases decide it, and Task 18 must not paper over them.
  - Task 6's split must leave `golden_compile` byte-identical.
  - Task 12's 502 path leaves a profile on OPA while its engines still run Ranger. That is safe by S8, but the UI must say so (Task 23 shows the server's text).
