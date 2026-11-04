# Governance sub-project 2b — the Trino OPA target — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a Trino engine bound to a generated Trino profile can be switched to Trino's own OPA access-control plugin. egret answers the OPA REST protocol itself from the neutral IR, with masks, row filters and refusals in force. The switch is backed by:
- a decision-level differential against real Ranger;
- an end-to-end differential of Trino-on-OPA against Trino-on-Ranger;
- measured latency.

**Architecture:**
- **Evaluator.** A new pure crate `egret-perm-eval` holds `Snapshot` (one profile's lowered statements plus its resolved column classifications). It answers `decide`, `filter`, `row_filters` and `column_masks`. Every operation name the Trino 483 OPA plugin sends is classified in one table, `trino_ops.rs`.
- **Snapshot build.** `egret-perm-compile` builds snapshots with the same loaders and lowerings the Ranger compile uses (`lower_ep_via`, split out of `compile_desired`, run with `MissingUsername::Omit` so it never writes). A `SnapshotCache` holds one `Arc<Snapshot>` per profile that is on OPA or has an engine still applied with OPA, keyed by `(policy_version, tag_version)`. A background refresher:
  - rebuilds a snapshot when its key moves;
  - re-checks the switch's stored-state refusals on every build;
  - splices fresh infrastructure into the previous snapshot when only the data lowering fails, so the SP2a kill switch still works.

  The decision path reads only the cache.
- **Endpoint.** `opa_surface()` serves five `POST /opa/v1/data/egret/trino/*` routes. It authenticates the engine id and secret carried in `input.context.properties` with the engine's Ranger credential (the same constant-time check).
  - A 5-second verification cache sits in front of the check.
  - The engine → profile binding comes from a map the refresher reloads every second.
  - `allow` and `batch` refuse with HTTP 200 `false`/`[]`. The value routes refuse with HTTP 403, so a refusal there never reads as "no mask, no filter".
- **Switch.** Migration `0213` adds `policy.authz_services.mechanism` and `engine_mgmt.engines.applied_mechanism`. `PATCH /api/control-plane/policies/services/{service}` with `{"mechanism": …}` refuses with a named condition unless:
  - OPA is enabled on the installation (`[governance.opa] enabled`, default `false` until 2b-2's last task);
  - the profile is Trino;
  - the profile is generated;
  - every bound **provisioned** engine's image ships `plugin/opa` (external engines are listed in the response, because their operator installs the plugin);
  - the profile holds no `all - …` default row;
  - the profile and its tag service hold no hand-written (`managed_by = 'operator'`) row;
  - the profile serves no stored differing mask or row-filter pair.

  On success it recompiles and re-applies every bound provisioned engine.
- **Engine configuration.** `opa_plugin_files` sits next to `ranger_plugin_files`. Both provisioners render the OPA `access-control.properties` and the context file. Externally run engines get `GET /engines/{id}/opa-plugin-config`.
- **Explain.** `/perm/explain*` answers an OPA profile through `egret-perm-eval`.
- **Ranger stays the oracle (S8).** An OPA profile still compiles its Ranger artifact.

**Tech Stack:** Rust workspace (axum 0.8, sqlx 0.9 runtime queries, tokio), the Java Ranger harness under `tests/ranger` (Maven, Ranger plugins-common), Docker Compose for the live and end-to-end checks, React + orval + vitest for the UI, criterion for the microbenchmarks.

**Spec:** `docs/en/specs/2026-11-04-gov-sp2-trino-opa.md`, section "2b — the OPA target", with "Errors", "Verification" and "Out of scope". Decisions S1–S8 bind. Divergences from the spec are listed under **Spec deviations** below. Each one follows the code or the measured jar.

**Split point.** The plan has 26 tasks.
- **Package 2b-1** is Tasks 1–20: the OPA engine behind a default-off switch, plus the Layer 1 differential and its CI job, ending in a live check.
- **Package 2b-2** is Tasks 21–26: Layer 2, latency, the UI, the docs page, and finally turning the switch on by default.

Each package is its own PR. 2b-2 branches from 2b-1 once 2b-1 merges.

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
- **OPA is off by default.** `egret_config::Config::governance.opa.enabled` (`[governance.opa] enabled`, env `EGRET_GOVERNANCE__OPA__ENABLED`) defaults to `false` until Task 26. While it is false:
  - the OPA surface is not mounted;
  - the refresher does not run;
  - `PATCH …/services/{service}` with `mechanism: "opa"` answers 409 `"OPA is not enabled on this installation"`.

  Switching back to Ranger always works.
- **Value routes fail closed with 403.** On `/row_filters`, `/batch_column_masks` and `/column_mask`, only an evaluation against a `Ready` snapshot answers 200. Every refusal, malformed request, wrong resource shape or not-built state answers HTTP 403 `{"error": reason}`. `/allow` and `/batch` answer refusals with 200 `false`/`[]`. An internal evaluator error answers 500 on every route.
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
- **Fail closed.** Every refusal path answers a denying result: `false`/`[]` on `allow`/`batch`, 403 on the value routes. The plugin turns any non-2xx into a failed query.

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
  - Single column mask (`opa.policy.column-masking-uri`, `OpaColumnMaskQueryResult`): `{expression, identity?}` or `null`.
  - The plugin builds the Trino `ViewExpression` with the table's catalog and schema, as the Ranger plugin does.
- **egret configures both masking URIs**, batch and single. Whichever SPI mask method Trino 483's engine calls, the plugin then has a URI for it; an unset URI would mean "no mask", which fails open.
- **`SELECT count(*)` and other column-less reads** reach both plugins as `SelectFromColumns` with an empty column set. The Ranger plugin's `RangerTrinoResource.forColumns` turns an empty set into one table-level resource (`forTable`), so Ranger asks `select` on `{catalog, schema, table}`. Verified with `javap` on `io.trino_trino-ranger-483.jar`.
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
  - `CUSTOM` becomes `maskedValue`, with `NULL` when it is absent. `{col}` and `{type}` are substituted in it too: the plugin replaces them in whichever expression it chose, and `maskedValue` holds the literal `{col}` (`egret-ranger-eval/tests/mask_filter.rs:552-570`). The lowering requires `{col}` in every CUSTOM expression (`InvalidCustomMask`).
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
- **An empty column list is one table-level Read check.** The column level is absent: it matches statements whose column is `Unnamed` or a run of `*` (`*`, `**`, …; `level_is_a_wildcard_run`). A column-only deny does not apply, mirroring Ranger's descendant match. This is how `count(*)` is checked.
- **Reach is the `_any` check, in its conservative form** (decided 2026-11-04). A user reaches an object iff both hold:
  - some applying allow matches the object or a descendant. An allow is an `Allow` path, `WholeEp`, or an `EngineObject` whose capability's first resource key is `catalog` (`trino.set_catalog_session_property`, `trino.execute_procedure`, `trino.execute_function`) and whose first value is the object's catalog;
  - **no** applying `Deny` or `Withhold` carrying **any** privilege matches the object or an ancestor. That covers a match by path, and a match by a classification on the object (for a column, also on its table).

  A deny that matches only a descendant is ignored. A reach difference where OPA allows and Ranger denies is never accepted as explained; the evaluator is fixed instead (Task 18).
- **Infrastructure and engine capabilities:**
  - `WholeEp` allows every check.
  - `AnyUser`/`SessionUser` allow impersonation: `SessionUser` only when the target user equals the session user.
  - `AnyQuery` allows `ExecuteQuery`.
  - `SystemProperty(p)` allows `SetSystemSessionProperty` of `p`.
  - `EngineObject { capability, values }` allows the operation named by the capability id (`trino.impersonate_user`, `trino.set_system_session_property`, `trino.set_catalog_session_property`, `trino.execute_procedure`, `trino.execute_function`) when its values equal the request's, compared case-insensitively.
- **Row filters.**
  - A `RowFilter` path level that is `Unnamed` matches every value. The Ranger renderer writes `*` for it (`ranger/grants.rs:290-307`, legacy rows stored before `validate_rule_shape`).
  - Identical expressions are merged first: they are the agreeing ties `ambiguous_arbitration` admits (`crates/egret-policy/src/ambiguous_arbitration.rs:18-32`).
  - The remaining expressions combine with `AND`, in statement order: `(a) AND (b)`. After the switch's `StoredOverlaps` refusal and the compile's `refuse_overlaps`, more than one distinct filter is reachable only from a state that is already refused.
- **Masks.**
  - A `Mask` whose column level is `Unnamed` matches every column of the tables it names. The Ranger target omits that level (`ranger/grants.rs:264-270`) and its behaviour is unmeasured, so Layer 1 decides; any difference goes into `explained.toml`.
  - Applying masks with identical `(mask_type, expression)` are merged first (agreeing ties).
  - After merging, at most one mask applies per column per user.
  - `WithholdMask` counts as a `MASK_NULL` mask for its audience.
  - Two distinct masks on one column for one user is an `EvalError::TwoMasks`. The routes answer it with HTTP 500, the last guard, and the plugin fails the query.
- **Metadata tables** (`t$files`, …), mirroring `ranger/completion/metadata_tables.rs:25-35` (M17):
  - A table name containing `$` is matched as written, so a grant on `*` reaches it and a grant on `t` does not.
  - In addition, a `Read` on `t$…` is denied to any user restricted on `t` by any of:
    - a `Deny` or `Withhold` carrying Read;
    - a non-`MASK_NONE` mask;
    - a non-empty row filter.

    Exempt users and `MASK_NONE` items do not count.

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
  - **Which profiles.** Those with `mechanism = 'opa'`, and those with a bound engine whose `applied_mechanism = 'opa'` (open question (a): an engine not yet re-applied after a switch back to Ranger keeps being answered).
  - **The engine → profile map.** `SELECT id, authz_service_id FROM engine_mgmt.engines WHERE authz_service_id = ANY($1)` for those profiles, loaded in the same pass. The surface reads the binding from this map, never from a cached credential.
  - **Stale entries.** It drops entries no longer listed.
  - **Rebuilds.** It rebuilds each entry whose key moved, in a `REPEATABLE READ, READ ONLY` transaction, lowering with `MissingUsername::Omit`.
  - **The stored-state refusals on every build:** `all - …` rows, `operator` rows (profile and tag service), and stored differing overlaps. A hit makes the entry `NotBuilt { error: <the refusal message> }`, which denies everything, and records a compile failure naming it.
  - **A data-lowering failure.** It splices freshly lowered infrastructure into the previous snapshot (`Snapshot::with_infra`), stores the error, logs at `warn`, and calls `compile_runs::record_compile_failure(pool, id, "OPA snapshot: {e}")`.
  - **An infrastructure-lowering failure** makes the entry `NotBuilt`.

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
5. **More switch refusals, also re-checked by the refresher.** Besides the decided `all - …` refusal (D-all), the switch is refused while:
   - the profile or its tag service holds a `managed_by = 'operator'` row;
   - the profile serves a stored differing mask or row-filter pair (`StoredOverlaps`, through a new `ProfileRecompiler::stored_overlaps_in_tx` over `overlap::stored_overlaps_via`).

   Both are stored state the evaluator cannot express. The refresher re-checks all three on every build, and `create_policy` refuses a hand-written policy in the tag service of an OPA profile.
6. **"Unknown engine" and "wrong secret" log one reason**, "engine credential refused". The `EngineCredentialVerifier` contract (`egret-engine-mgmt-api/src/credential.rs:47`) forbids callers to tell them apart.
7. **A 5-second verification cache, the one database read.** The cache holds only "this (engine, secret hash) verified". The binding comes from the refresher's per-second map. A miss calls `DbVerifier`, the one database read on the request path.
8. **The value routes refuse with 403, not 200 `[]`** (controller ruling C1). On those routes `[]`/`null` means "no filter, no mask", so a 200 refusal would fail open whenever `/allow` had already said yes. Internal evaluator errors answer 500.
9. **The matrix rows for `opa(trino)` copy the Ranger Trino rows' answers.** Their evidence names the Layer 1 tests (Task 18), and a unit test keeps the two in step. This keeps the lowered statement set, and so the Ranger artifact of an OPA profile, identical to a Ranger profile's.
10. **Layer 1 builds store states with the golden fixture builders.** It writes them into Postgres through `crates/egret-perm-compile/tests/golden/fixtures.rs`. The existing 2000-case Ranger corpus draws raw Ranger JSON and has no egret store states to reuse.
    - What Layer 1 reuses is the Java runner (`tests/ranger/src/test/java/EgretCaseRunner.java`) and its Rust driver, `ask_ranger_json`.
    - It runs 40 states × 50 questions = 2000 cases.
11. **`explained.toml` is new.** The Ranger differential keeps its accepted differences as Rust consts (`ID_DIVERGENCES`, `DECLINED_CEILINGS`). The OPA differential uses the file the spec names.
12. **The migration number is `0213`**, chosen by the evidence in Global Constraints.
13. **The image refusal covers provisioned engines only** (ruling C6). An external engine has no stored image, and its operator installs the plugin config. The switch's 200 body lists them as `external_engines_unchanged`, and the dialog and the docs say the operator must run Trino ≥ 483 with `plugin/opa`.
14. **A default-off configuration gate** (ruling I8): `[governance.opa] enabled = false` until Task 26.
15. **`engine_mgmt.engines.applied_mechanism`** (open question (a)) records what each provisioned engine was last applied with. Snapshots are kept for a profile while any of its engines still runs OPA, and a re-sent PATCH with the profile's current mechanism re-runs the reconfigure.
16. **A fifth route, `/column_mask`**, with `opa.policy.column-masking-uri`, so the single-column mask path cannot fail open (I4).

### D5. Review findings not applied

None. Every Critical and Important claim of the 2026-11-04 plan review was checked against the code before its fix was written in:

| Finding | Evidence |
|---|---|
| C1 | protocol per D1 |
| C2 | `RangerTrinoResource.forColumns` → `forTable` on an empty set, by `javap` |
| C3 | `migrations/0081_catalog_tag_targets.sql:14`; `store.rs:1462`, `:1497` |
| C4 | spec 2a convergence |
| C6 | `engine_delegation.rs:86-88`; `engine_images.rs:108-130` |
| C7 | `ranger/grants.rs:290-307` |
| C8 | `store.rs:565` |
| I1 | `ambiguous_arbitration.rs:18-32` |
| I2 | `golden/fixtures.rs:1004-1006`; `golden_compile.rs:65,142` |
| I3 | `credential.rs:12-18` |
| I5 | `matrix.rs:358-368` |
| I9 | `members.rs:484-491`; `ir_load.rs:397` |

- **C5** rests on a reading of Ranger's item matcher that the reviewer marked as a reading. It is applied anyway, because it is the conservative direction (ruling (b)).
- **C8 nuance.** A tag service *is* stamped `generated` once a compile writes into it (`trigger.rs:923-938`). It stays `manual` only while nothing compiles into it. The fix still holds for that case and for shared tag services.

### D6. Deferred minors

Every plan-text minor (M1–M14, M4b) is applied in its task. None is deferred.

---

## File Structure

| Path | Task | Responsibility |
|---|---|---|
| `scripts/opa/extract-trino-plugin-facts.sh` | 1 | Pulls both 483 plugin jars from the pinned image and writes the fixtures |
| `crates/egret-perm-eval/{Cargo.toml,src/lib.rs,tests/boundary.rs}` | 1 | The pure evaluator crate |
| `crates/egret-perm-eval/tests/fixtures/trino-483-opa-operations.txt` | 1 | The 63 operation names |
| `crates/egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt` | 1 | Ranger plugin method → access types |
| `crates/egret-perm-eval/tests/fixtures/trino-483-plugins.sha256` | 1 | Jar digests |
| `crates/egret-perm-eval/tests/fixtures/trino-483-opa-request-shapes.txt` | 1 | Bytecode of the batch filter and mask request builders |
| `crates/egret-perm-eval/src/trino_ops.rs` | 2 | The operation table |
| `crates/egret-perm-eval/src/{pattern.rs,snapshot.rs,decide.rs}` | 3 | Matching, the snapshot, `decide`/`filter` |
| `crates/egret-perm-eval/src/values.rs` | 4 | `row_filters`, `column_masks`, mask rendering |
| `migrations/0213_profile_mechanism.sql` | 5 | `policy.authz_services.mechanism`, `engine_mgmt.engines.applied_mechanism` |
| `crates/egret-perm-ir/src/{ep.rs,matrix.rs}` | 5 | `Mechanism::Opa`, `opa(trino)` rows |
| `crates/egret-perm-compile/src/opa/{mod.rs,build.rs}` | 6 | `lower_ep_via`, `build_snapshot_via`, classified columns |
| `crates/egret-perm-compile/src/opa/cache.rs` | 7 | `SnapshotCache`, refresher, engine binding map |
| `crates/egret-policy/src/opa_guard.rs` | 7 | Stored-state checks shared by the refresher and the switch |
| `crates/egret-perm-compile/tests/opa_support/fixtures_host.rs` | 6 | What `golden/fixtures.rs` needs from its includer |
| `crates/egret-config/src/lib.rs` | 5, 26 | `[governance.opa] enabled` |
| `crates/egret-perm-compile/src/opa/wire.rs` | 8 | OPA input parsing and result building |
| `crates/egret-perm-compile/src/opa/surface.rs` | 9 | `opa_surface`, `OpaState`, verification cache |
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
- Create (generated by the script, then committed): `crates/egret-perm-eval/tests/fixtures/trino-483-opa-operations.txt`, `trino-483-ranger-checks.txt`, `trino-483-opa-request-shapes.txt`, `trino-483-plugins.sha256`
- Modify: root `Cargo.toml`. Add `egret-perm-eval = { path = "crates/egret-perm-eval" }` under `[workspace.dependencies]`; `members = ["crates/*"]` already picks the crate up.

**Interfaces:**
- Produces:
  - the crate `egret_perm_eval`;
  - the four fixture files;
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

# Request shapes of the batch filters and of both mask paths, as committed evidence
# (open question (c)): the bytecode that builds FilterColumns and GetColumnMask requests.
{
  echo "## OpaBatchAccessControl: filterColumns and the batch request builders"
  javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/OpaBatchAccessControl.class" \
    | awk '/filterColumns|lambda\$filterColumns\$0|lambda\$batchRequestBuilder\$[01]/,/^$/'
  echo "## OpaHighLevelClient and OpaAccessControl: the column-mask requests"
  for c in OpaHighLevelClient OpaAccessControl; do
    javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/$c.class" \
      | awk '/getColumnMask|getColumnMasks|GetColumnMask/,/^$/'
  done
} > "$OUT/trino-483-opa-request-shapes.txt"
```

  Read `trino-483-opa-request-shapes.txt` and write down in the task report:
  - whether one `FilterColumns` request carries exactly one table, with the result indices referring to that table's `columns`;
  - whether a batch `GetColumnMask` request carries one `column` resource per column in `filterResources`;
  - which of `getColumnMask`/`getColumnMasks` the plugin implements.

  Task 8 relies on these answers; if they differ from its assumptions, Task 8's tests change to match the fixture.

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
    /// A copy whose infrastructure statements (`Source::Infra`) are replaced by `infra`,
    /// every other statement kept. The refresher uses it when only the data lowering failed (C4).
    pub fn with_infra(&self, infra: Vec<egret_perm_ir::statement::Statement>) -> Result<Snapshot, SnapshotError>;
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
- **Data.** For `Data(Read)` on a `Table`, check each listed column; every column must be allowed. **An empty `columns` list is one table-level Read check**, not a vacuous allow: the column level is absent, so it matches statements whose column is `Unnamed` or a run of `*` (`*`, `**`, …; `level_is_a_wildcard_run`), and a column-only deny does not apply. This mirrors the Ranger plugin, whose `RangerTrinoResource.forColumns` asks `select` on `{catalog, schema, table}` for an empty set; `SELECT count(*)` arrives this way. For `Data(Write)`, check the table, with the column level absent in the same way. A privilege on an object is allowed when:
  - an applying `Effect::Allow { privileges }` with that privilege has a `Target::Path` matching all four levels; or an applying `Target::Infra(InfraObject::WholeEp)` statement exists;
  - and no applying `Effect::Deny`/`Effect::Withhold` with that privilege matches. A withhold matches through `Target::Path` (StarRocks shape) or `Target::Classification { name }`. For a column request, the classification is the column's or its table's; for a table request, only the table's.
- **Reach** (operations `Reach`), on the request's object:
  - Catalog: `Catalog(c)`.
  - Schema: `Schema`.
  - Table: a `Table` with no columns, or a `Table` in `filter` for `FilterTables`.
  - `FilterColumns`: each column (Task 8 turns that request into one `Table` per column).

  A user reaches the object iff both hold:
  - **some applying allow matches the object or a descendant.** The allow is an `Allow` path whose levels are compatible with the object's named levels (unnamed or deeper levels are free), or WholeEp, or an `EngineObject` whose capability's first resource key is `catalog` and whose first value equals the object's catalog. Those capabilities are `trino.set_catalog_session_property`, `trino.execute_procedure` and `trino.execute_function` (`engine_capabilities.rs:92-145`); never `trino.impersonate_user`, whose first value is a username;
  - **no applying `Deny` or `Withhold` carrying any privilege matches the object or an ancestor.** It may match by path, or by a classification carried by the object (a column: its own or its table's; a table: its own).

  A deny that matches only a descendant is ignored. This is the conservative rule (ruling (b)): a reach where OPA allows and Ranger denies is never accepted as an explained difference.
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
  2. is then denied for `Read` if the user is restricted on any column of the base table (the name before the first `$`), by path or by classification. Restricted means one of:
     - an applying `Deny`/`Withhold` carrying Read;
     - a `Mask` whose type is not `MASK_NONE`;
     - a `WithholdMask`;
     - a non-empty `RowFilter`.

     Exempt users and `MASK_NONE` do not count, as in `ranger/completion/metadata_tables.rs:25-35`.
- **The index.** `Snapshot::build` sorts statement indices into:
  - `by_catalog: BTreeMap<String /* lowercase literal */, Vec<usize>>`;
  - `wild_catalog: Vec<usize>` (catalog level contains `*`/`?`);
  - `by_classification: BTreeMap<String, Vec<usize>>`;
  - `infra: Vec<usize>`;
  - `engine: Vec<usize>`;
  - `classes: BTreeMap<(String, String, String), (BTreeSet<String> /* table */, BTreeMap<String, BTreeSet<String>> /* column */)>`, all lowercased.

  A question scans only `by_catalog[c] ∪ wild_catalog` plus the classification lists of the classifications its object carries.
- **`filter`** returns the indices `i` for which `decide(user, op, &resources[i])` allows, ascending.
- **`decide_privilege`** applies the Data rule to an `ObjectPath`. An absent level matches only statements whose level is `Unnamed` or a run of `*`.

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
        infra(InfraKind::DelegatorImpersonate, who(&["egret_delegator"], &[]), InfraObject::AnyUser, "impersonate"),
        infra(InfraKind::SelfImpersonate, who(&["*"], &[]), InfraObject::SessionUser, "impersonate"),
        infra(InfraKind::ExecuteQuery, who(&["*"], &[]), InfraObject::AnyQuery, "execute"),
        infra(InfraKind::QueryTimeout, who(&["*"], &[]), InfraObject::SystemProperty("query_max_run_time".into()), "alter"),
    ], vec![]);
    assert!(allowed(&s.decide("egret_delegator", "ImpersonateUser", &Resource::User("ann".into()))));
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

#[test]
fn count_star_needs_a_table_level_read() {
    // SELECT count(*) arrives as SelectFromColumns with no columns (D1, Ranger's forColumns → forTable).
    let none = snap(vec![], vec![]);
    assert!(!allowed(&none.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))), "no grant, no count(*)");
    let catalog = snap(vec![st(1, who(&["ann"], &[]), path("lake", None, None, None),
                               Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(allowed(&catalog.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))));
    let column_only = snap(vec![st(1, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), Some("id")),
                                   Effect::Allow { privileges: vec![Privilege::Read] })], vec![]);
    assert!(!allowed(&column_only.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))),
            "a column grant does not answer a table-level select");
}

#[test]
fn a_withheld_column_is_not_reached() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read] }),
        Statement { source: Source::Withhold { classification_id: Uuid::nil(), classification_name: "pii".into() },
                    audience: who(&["*"], &[]), target: Target::Classification { name: "pii".into() },
                    effect: Effect::Withhold { privileges: vec![Privilege::Read] }, derivation: Derivation::AsDeclared },
    ], vec![ClassifiedColumn { catalog: "lake".into(), schema: "s".into(), table: "t".into(), column: Some("ssn".into()), classification: "pii".into() }]);
    let one = |c: &str| cols("lake", "s", "t", &[c]);
    assert_eq!(s.filter("ann", "FilterColumns", &[one("id"), one("ssn")]), vec![0], "the withheld column is hidden");
    assert!(allowed(&s.decide("ann", "ShowColumns", &cols("lake", "s", "t", &[]))), "a deny on a descendant is ignored");
}

#[test]
fn a_deny_carrying_any_privilege_on_an_ancestor_hides_the_object() {
    let s = snap(vec![
        st(1, who(&["ann"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read, Privilege::Write] }),
        st(2, who(&["ann"], &[]), path("lake", Some("s"), None, None), Effect::Deny { privileges: vec![Privilege::Write] }),
    ], vec![]);
    assert!(!allowed(&s.decide("ann", "ShowTables", &Resource::Schema { catalog: "lake".into(), schema: "s".into() })),
            "Ranger's _any: a deny item with any access type on the object refuses it");
    assert!(allowed(&s.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))), "the deny is on a descendant of the catalog");
}

#[test]
fn an_impersonation_grant_is_not_catalog_reach() {
    let s = snap(vec![Statement {
        source: Source::EnginePrivilege { engine_grant_id: Uuid::nil() }, audience: who(&["ann"], &[]),
        target: Target::EngineObject { capability: "trino.impersonate_user".into(), values: vec!["lake".into()] },
        effect: Effect::EnginePrivilege { capability: "trino.impersonate_user".into() }, derivation: Derivation::AsDeclared }], vec![]);
    assert!(!allowed(&s.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))),
            "a username that happens to equal a catalog name is not a catalog");
}

#[test]
fn with_infra_replaces_only_the_infrastructure() {
    let s = snap(vec![
        st(1, who(&["ann"], &[]), path("lake", None, None, None), Effect::Allow { privileges: vec![Privilege::Read] }),
        infra(InfraKind::AdminAllAccess, who(&["root"], &[]), InfraObject::WholeEp, "all"),
    ], vec![]);
    let t = s.with_infra(vec![]).unwrap();
    assert!(!allowed(&t.decide("root", "CreateTable", &cols("lake", "s", "t", &[]))), "the disabled admin lost all-access");
    assert!(allowed(&t.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["a"]))), "data statements kept");
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
  - (g) Treat an empty column list as allowed. `count_star_needs_a_table_level_read` must fail.
  - (h) Drop classification withholds from reach. `a_withheld_column_is_not_reached` must fail.
  - (i) Count a reach deny only when it denies every privilege the allow grants. `a_deny_carrying_any_privilege_on_an_ancestor_hides_the_object` must fail.
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
    /// The AND of every distinct applying row filter on the table, in statement order; empty when none.
    pub fn row_filters(&self, user: &str, catalog: &str, schema: &str, table: &str) -> Result<Vec<String>, EvalError>;
    /// Column name → SQL expression, only for masked columns. `columns` are (name, Trino type display name).
    pub fn column_masks(&self, user: &str, catalog: &str, schema: &str, table: &str, columns: &[(String, String)])
        -> Result<std::collections::BTreeMap<String, String>, EvalError>;
    /// The applying mask before rendering, for explain and the differential: (mask_type, custom expression).
    pub fn column_mask_choice(&self, user: &str, catalog: &str, schema: &str, table: &str, column: &str)
        -> Result<Option<(String, Option<String>)>, EvalError>;
}
/// Ranger Trino plugin rendering: MASK_NULL→"NULL"; CUSTOM→expr or "NULL"; MASK_NONE→None;
/// others→the transformer; then {col}→column and {type}→column_type in the chosen expression,
/// CUSTOM's included (egret-ranger-eval/tests/mask_filter.rs:552-570).
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
  - **Which row filters apply.** A `RowFilter { expression }` statement applies when its audience applies, and either:
    - its `Target::Path` matches this catalog, schema and table (wildcards allowed; an `Unnamed` level matches every value, because the Ranger renderer writes `*` for it in `crates/egret-perm-compile/src/ranger/grants.rs:290-307`); or
    - its `Target::Classification` is carried by the table.
  - **Merging filters.**
    1. Identical expressions are merged first. They are the agreeing ties `crates/egret-policy/src/ambiguous_arbitration.rs:18-32` admits.
    2. The rest become one string, `(e1) AND (e2)`, returned as a one-element vector.
  - **Which masks apply.** A `Mask` matches a column:
    - by path: catalog, schema and table match, and the column level matches or is `Unnamed` (an `Unnamed` column covers every column of the table; the Ranger target omits that level, so Layer 1 decides whether Ranger agrees);
    - or by a classification on the column or its table.
  - A `WithholdMask` counts as `MASK_NULL`.
  - `MASK_NONE` is an applying mask that renders to nothing.
  - **Merging masks.** Applying masks with identical `(mask_type, expression)` are merged into one (agreeing ties).
  - After merging, more than one applying mask → `Err(EvalError::TwoMasks { user, column })`.

- [ ] **Step 1: Write the failing tests** `tests/values.rs`, reusing Task 3's helper set (copy the helpers block into this file, and add `use egret_perm_eval::EvalError;`):

```rust
#[test]
fn a_legacy_row_filter_with_an_unnamed_table_filters_every_table_of_the_schema() {
    let s = snap(vec![st(1, who(&["*"], &[]), path("lake", Some("s"), None, None),
                         Effect::RowFilter { expression: "region = 'us'".into() })], vec![]);
    assert_eq!(s.row_filters("ann", "lake", "s", "any_table").unwrap(), vec!["(region = 'us')".to_string()]);
    assert!(s.row_filters("ann", "lake", "other", "t").unwrap().is_empty());
}

#[test]
fn an_agreeing_tie_is_one_mask_and_one_filter() {
    let s = snap(vec![
        st(1, who(&["*"], &[]), path("lake", Some("s"), Some("t"), Some("c")), Effect::Mask { mask_type: "MASK".into(), expression: None }),
        st(2, who(&["*"], &[]), path("lake", Some("s"), Some("t"), Some("c")), Effect::Mask { mask_type: "MASK".into(), expression: None }),
        st(3, who(&["*"], &[]), path("lake", Some("s"), Some("t"), None), Effect::RowFilter { expression: "x = 1".into() }),
        st(4, who(&["ann"], &[]), path("lake", Some("s"), Some("t"), None), Effect::RowFilter { expression: "x = 1".into() }),
    ], vec![]);
    assert_eq!(s.column_masks("ann", "lake", "s", "t", &[("c".into(), "varchar".into())]).unwrap().len(), 1);
    assert_eq!(s.row_filters("ann", "lake", "s", "t").unwrap(), vec!["(x = 1)".to_string()]);
}

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
  - (e) Do not merge identical masks. `an_agreeing_tie_is_one_mask_and_one_filter` must fail.
  - (f) Treat an `Unnamed` row-filter table as matching nothing. The legacy row-filter test must fail.
- [ ] **Step 6: Commit** `feat(perm-eval): row filters and column masks`.

---

## Phase 2 — the mechanism and the snapshot

### Task 5: Migration 0213, `Mechanism::Opa`, the OPA config gate, and the profile's mechanism everywhere it is read

**Files:**
- Create: `migrations/0213_profile_mechanism.sql`
- Modify: `crates/egret-config/src/lib.rs`. `Config` (:539) gains `#[serde(default)] pub governance: GovernanceConfig`, with:
  - `#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)] pub struct GovernanceConfig { #[serde(default)] pub opa: OpaSettings }`;
  - `#[derive(…same…)] pub struct OpaSettings { #[serde(default)] pub enabled: bool }`;
  - a doc comment saying the key is `[governance.opa] enabled`, env `EGRET_GOVERNANCE__OPA__ENABLED`, default `false` until SP2b-2 (Task 26). Model it on `WorkflowConfig::webhooks_enabled` (:768-790).
- Modify: `crates/egret-perm-compile/tests/golden_compile.rs:65` (`capture`). When `std::env::var("EGRET_GOLDEN_MECHANISM").as_deref() == Ok("opa")`, run `UPDATE policy.authz_services SET mechanism = 'opa' WHERE id = $1 AND type = 'trino'` on `tx` for each profile before its recompile.
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
  - the column `engine_mgmt.engines.applied_mechanism text NULL` (`'ranger'`/`'opa'`, the mechanism the engine was last applied with; NULL for external engines and for engines not applied since this migration);
  - `egret_config::Config::governance.opa.enabled: bool`;
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

-- What each provisioned engine was last applied with. A switch is not atomic
-- across engines (each coordinator restarts on its own apply), so egret keeps
-- answering OPA for an engine still running the OPA plugin after its profile
-- went back to Ranger, until that engine is re-applied. NULL: external, or not
-- applied since this migration (treated as 'ranger').
ALTER TABLE engine_mgmt.engines
    ADD COLUMN applied_mechanism text NULL
    CHECK (applied_mechanism IN ('ranger', 'opa'));
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

  In `crates/egret-config/src/lib.rs`'s test module, add `governance_opa_is_off_by_default`. It parses a TOML with no `[governance]` section and asserts `!config.governance.opa.enabled`.

  Then add these two tests inline in `crates/egret-perm-ir/src/matrix.rs`. The first pins every row (I5); the second spot-checks answers:

```rust
#[test]
fn every_trino_row_has_an_identical_opa_trino_row() {
    let trino: Vec<_> = ENTRIES.iter().filter(|e| e.kind == TRINO).collect();
    let opa: Vec<_> = ENTRIES.iter().filter(|e| e.kind == OPA_TRINO).collect();
    assert_eq!(trino.len(), opa.len());
    for t in &trino {
        assert!(opa.iter().any(|o| o.capability == t.capability && o.versions == t.versions && o.answer == t.answer),
                "no identical OPA_TRINO row for {}", t.capability);
    }
}
```


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
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-config governance
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test golden_compile
env -u EGRET_ALLOW_DB_SKIP EGRET_GOLDEN_MECHANISM=opa CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test golden_compile
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib committed_openapi_json_is_current
git status --short crates/egret-perm-compile/tests/golden
```

  Expected: PASS, and the last command prints nothing. The second golden run is S8 pinned: every Trino fixture, compiled on an OPA profile, equals its blessed Ranger artifact byte for byte.
- [ ] **Step 6b: Mutation check.** Delete one `OPA_TRINO` row (for example `PoliciesReachMetadataTables`). `every_trino_row_has_an_identical_opa_trino_row` must fail, and the `EGRET_GOLDEN_MECHANISM=opa` golden run must fail on a fixture with a deny. Restore.

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
    pub whole_tables: Vec<egret_perm::TableCount>,
    pub view_guard: Vec<egret_perm_ir::lower::withhold::PendingGuard>,
}
pub(crate) async fn lower_ep_via(conn: &mut sqlx::PgConnection, service_id: i64,
    missing: crate::members::MissingUsername) -> Result<Option<LoweredEp>, CompileError>;
```

  - **The split.** `compile_desired` calls `lower_ep_via(conn, service_id, MissingUsername::Mint)`, then `crate::ranger::emit` once per family in the same order. It then runs the rest unchanged: the StarRocks clearance fold, `completion::complete`, `refuse_overlaps` and the counting. `emit` takes no connection, so emitting after all the reads changes nothing. The golden suite proves it.
  - **Threading `missing` (I9).** Today the minting reads hard-code `MissingUsername::Mint`:
    - `members.rs:877`, inside `expand_enumerated_principals`;
    - `ir_load.rs:397`, inside `load_withhold_inputs`;
    - the admin read inside `load_infra_inputs` (`ir_load.rs:540-560`).

    Give `expand_enumerated_principals`, `load_ep`, `load_withhold_inputs` and `load_infra_inputs` a `missing: MissingUsername` parameter (`members.rs:484`). Every existing caller passes `Mint`, so the compile is unchanged; the snapshot build and explain pass `Omit`, which leaves an unnamed holder out and never writes.
- Create: `crates/egret-perm-compile/src/opa/mod.rs` (`pub mod build; pub mod cache; pub mod wire; pub mod surface;`; declare only `build` in this task) and `src/opa/build.rs`.
- Modify: `crates/egret-perm-compile/src/lib.rs` (`pub mod opa;`). Modify `Cargo.toml`: add `egret-perm-eval.workspace = true`.
- Create: `crates/egret-perm-compile/tests/opa_support/fixtures_host.rs`. This is what `golden/fixtures.rs` needs from its includer as `crate::…`:
  - `pub struct Names` and its `impl` (copied from `tests/golden_compile.rs:13-26`);
  - `#[allow(dead_code)] pub async fn capture(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, profiles: &[(i64, Option<i64>)]) -> serde_json::Value { unreachable!("golden scenarios are not run from here") }`;
  - `#[allow(dead_code)] pub fn compare_or_bless(_: &str, _: serde_json::Value, _: &Names) { unreachable!() }`, with the signatures of `tests/golden_compile.rs:65` and `:142`.

  A test binary includes it at its crate root, with `include!("opa_support/fixtures_host.rs");` followed by `#[path = "golden/fixtures.rs"] mod fixtures;`.
- Test: `crates/egret-perm-compile/tests/opa_snapshot.rs` (new, DB-gated)

**Interfaces:**
- Consumes:
  - `egret_perm_eval::{Snapshot, SnapshotInput, ClassifiedColumn}`;
  - `crate::ir_load::{load_ep, load_infra_inputs}`;
  - `egret_perm_ir::lower::infra::lower_infra`;
  - `egret_policy::{catalogs_for_service_via, tag_assignments_for_catalogs_via}` (`crates/egret-policy/src/store.rs:1497`, `:1462`).
- Produces:

```rust
// crates/egret-perm-compile/src/opa/build.rs
pub async fn load_classified_columns_via(conn: &mut sqlx::PgConnection, service_id: i64)
    -> Result<Vec<egret_perm_eval::ClassifiedColumn>, CompileError>;
pub struct SnapshotParts {
    pub key: (i64, i64),
    /// grants, engine privileges, rules, withholds — everything but infrastructure.
    pub data: Vec<egret_perm_ir::statement::Statement>,
    pub infra: Vec<egret_perm_ir::statement::Statement>,
    pub classified: Vec<egret_perm_eval::ClassifiedColumn>,
}
/// The infrastructure statements alone (`load_infra_inputs` + `lower_infra`), for the refresher's
/// splice when the data lowering fails (C4). None: not trino/starrocks.
pub async fn lower_infra_via(conn: &mut sqlx::PgConnection, service_id: i64,
    missing: crate::members::MissingUsername) -> Result<Option<Vec<egret_perm_ir::statement::Statement>>, CompileError>;
/// None: the service is not trino/starrocks. Reads at the caller's transaction.
pub async fn snapshot_parts_via(conn: &mut sqlx::PgConnection, service_id: i64,
    missing: crate::members::MissingUsername) -> Result<Option<SnapshotParts>, CompileError>;
/// `snapshot_parts_via` then `Snapshot::build` over data ++ infra.
pub async fn build_snapshot_via(conn: &mut sqlx::PgConnection, service_id: i64,
    missing: crate::members::MissingUsername) -> Result<Option<egret_perm_eval::Snapshot>, CompileError>;
```

  - **`load_classified_columns_via`** writes **no new SQL** (C3). It calls `egret_policy::catalogs_for_service_via(conn, service_id)`, which `store.rs:1488-1496` names "the ONE spelling of the engine-reachability leg". It then calls `egret_policy::tag_assignments_for_catalogs_via(conn, &catalog_ids)`, the rows the tag download serves. Each row becomes `ClassifiedColumn { catalog, schema: schema_name, table: table_name, column, classification: tag_name }`, where:
    - **`column_name == ""` maps to `column: None`.** Table-level assignments are stored with the `''` sentinel, never NULL (`migrations/0081_catalog_tag_targets.sql:14`, `column_name text NOT NULL DEFAULT ''`);
    - any other value maps to `Some(column_name)`.
  - **`snapshot_parts_via`:**
    1. `lower_ep_via(conn, id, missing)`, collecting every family's `statements` into `data`;
    2. `lower_infra_via(conn, id, missing)` into `infra`;
    3. `load_classified_columns_via`;
    4. `SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1` for `key`.
  - **`build_snapshot_via`** maps a `SnapshotError` to `CompileError::Db(format!("OPA snapshot: {e}"))`.

- [ ] **Step 1: Write the failing test** `tests/opa_snapshot.rs`. At its crate root: `include!("opa_support/fixtures_host.rs");` and `#[path = "golden/fixtures.rs"] mod fixtures;`. Each test opens a transaction, builds a state with the fixture builders, and calls `egret_perm_compile::opa::build::build_snapshot_via(&mut tx, profile, MissingUsername::Omit)`. `MissingUsername` is `pub(crate)`, so add `pub fn build_snapshot_for_test(conn, id)` to `harness_support.rs` (feature `harness`); it passes `Omit`. The tests call that.

```rust
#[tokio::test]
async fn the_snapshot_answers_what_the_store_says() {
    // Arrange (fixtures.rs builders): a trino profile with a tag service (profile_with_config),
    // a warehouse + catalog "lake" attached to an engine bound to it (engine_bound_spec with
    // {"trinoConfig":{"delegationMode":"open"}}), users ann and bob, grant Allow Read on
    // scope(lake) to ann, a path mask MASK_HASH on lake.s.t.email for public except bob,
    // tag "pii" on lake.s.t.ssn with gate(roles=["cleared_role"]) held by bob.
    // Act: build_snapshot_for_test.
    // Assert: decide("ann","SelectFromColumns", Table{lake,s,t,[id]}) is Allow;
    //   decide("ann", …, [ssn]) is Deny{explicit:true} (withheld);
    //   decide("bob", …, [id]) is Deny (no grant);
    //   column_masks("ann", lake,s,t,[(email,varchar)]) has email;
    //   column_masks("bob", …) is empty; snapshot.key() equals the row's (policy_version, tag_version).
}

#[tokio::test]
async fn a_table_level_classification_reaches_every_column_of_the_table() {
    // Arrange: as above, plus tag_table(pii, wh, "s", "u") (stored with column_name = '').
    // Assert: decide("ann","SelectFromColumns", Table{lake,s,u,["untagged"]}) is Deny{explicit:true};
    //   load_classified_columns_via returns the row with column == None.
}

#[tokio::test]
async fn a_qualifying_engine_puts_the_infra_statements_in_the_snapshot() {
    // Arrange: same profile, engine spec {"trinoConfig":{"coordinator":{},"delegationMode":"impersonate","auth":{"password":{}}}}.
    // Assert: decide("ann","ExecuteQuery",None) Allow; decide("ann","KillQueryOwnedBy",User("ann")) Allow;
    //   decide("ann","KillQueryOwnedBy",User("bob")) Deny;
    //   decide(egret_core::engine_principals::DELEGATOR_PRINCIPAL,"ImpersonateUser",User("ann")) Allow.
}

#[tokio::test]
async fn the_snapshot_build_never_mints_a_username() {
    // Arrange: a role granted Read on lake whose one holder has users.username NULL.
    // Act: build_snapshot_for_test, then roll back nothing: count users with NULL username before/after.
    // Assert: the build succeeds, the holder is absent from the audience (decide for nobody else changes),
    //   and the NULL-username count is unchanged (Omit wrote nothing).
}
```

  The split itself is proved by `golden_compile` staying byte-identical (Step 4).

  Fill each body completely, as described in its comments. Use the builder signatures in `tests/golden/fixtures.rs`: `profile`, `profile_with_config`, `warehouse`, `catalog`, `catalog_attached`, `engine_bound_spec`, `user` (pass `None` as `username` for the unnamed holder), `grant`, `rule`, `tag`, `tag_column`, `tag_table`, `gate`, `platform_role` and `role_held`.

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

### Task 7: `SnapshotCache`, the refresher, and the stored-state checks

**Files:**
- Create: `crates/egret-perm-compile/src/opa/cache.rs`, and add `pub mod cache;` to `opa/mod.rs`
- Create: `crates/egret-policy/src/opa_guard.rs`, and declare it from `lib.rs` with `pub mod opa_guard;`. It holds the stored-state checks that both the refresher (here) and the switch (Task 11) call, so there is one copy.
- Test: `crates/egret-perm-compile/tests/opa_cache.rs` (new, DB-gated). Its crate root has `include!("opa_support/fixtures_host.rs"); #[path = "golden/fixtures.rs"] mod fixtures;` (Task 6).

**Interfaces:**
- Consumes:
  - Task 6's `snapshot_parts_via` and `lower_infra_via`;
  - Task 3's `Snapshot::build` and `Snapshot::with_infra`;
  - `crate::compile_runs::record_compile_failure(pool: &PgPool, service_id: i64, error: &str)`;
  - `crate::overlap::stored_overlaps_via(conn) -> Result<Vec<StoredOverlap>, CompileError>` (`overlap.rs:478`; `StoredOverlap { service_id, service_name, kind, coordinate, rules, .. }` at `:432`).
- Produces:

```rust
// crates/egret-policy/src/opa_guard.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredStateRefusal {
    AllDefaults { count: i64 },
    HandWritten { service: String, count: i64 },
}
impl StoredStateRefusal { pub fn message(&self) -> String; }
/// The `all - …` rows of the profile, then the `managed_by = 'operator'` rows of the profile and of
/// the tag service its `config_jsonb->>'tagService'` names. Empty: nothing stored the evaluator ignores.
pub async fn stored_state_refusals_via(conn: &mut sqlx::PgConnection, service_id: i64)
    -> Result<Vec<StoredStateRefusal>, crate::PolicyStoreError>;

// crates/egret-perm-compile/src/opa/cache.rs
pub enum Lookup { NotOpa, NotBuilt { error: Option<String> }, Ready(std::sync::Arc<egret_perm_eval::Snapshot>) }
#[derive(Default)]
pub struct SnapshotCache { /* std::sync::RwLock<State { entries: HashMap<i64, CacheEntry>, bindings: HashMap<uuid::Uuid, i64> }> */ }
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport { pub rebuilt: Vec<i64>, pub spliced: Vec<i64>, pub failed: Vec<i64>, pub dropped: Vec<i64> }
impl SnapshotCache {
    pub fn get(&self, service_id: i64) -> Lookup;
    /// The profile an engine is bound to, as of the last refresh; None: unknown or bound to no served profile.
    pub fn binding(&self, engine_id: uuid::Uuid) -> Option<i64>;
    pub async fn refresh_once(&self, pool: &sqlx::PgPool) -> Result<RefreshReport, sqlx::Error>;
}
pub fn spawn_snapshot_refresher(cache: std::sync::Arc<SnapshotCache>, pool: sqlx::PgPool,
    period: std::time::Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static)
    -> tokio::task::JoinHandle<()>;
```

- **`StoredStateRefusal::message`, exactly:**
  - `AllDefaults`: ``"the profile holds the `all - …` default policies ({count}); delete them first"``.
  - `HandWritten`: `"{service} holds {count} hand-written policies, which OPA would ignore; delete them first"`.
- **The `stored_state_refusals_via` queries:**
  - `SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name LIKE 'all - %'`. This is migration 0180's shape; no `_` or `%` occurs in `'all - '`.
  - `SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND managed_by = 'operator'`, for the profile and for its tag service.
- **`refresh_once`:**
  1. **Which profiles.** It reads `SELECT s.id, s.policy_version, s.tag_version FROM policy.authz_services s WHERE s.mechanism = 'opa' OR EXISTS (SELECT 1 FROM engine_mgmt.engines e WHERE e.authz_service_id = s.id AND e.applied_mechanism = 'opa') ORDER BY s.id`. An engine not yet re-applied after a switch back to Ranger still runs the OPA plugin and must keep being answered (open question (a)).
  2. **Bindings.** It reads `SELECT id, authz_service_id FROM engine_mgmt.engines WHERE authz_service_id = ANY($1)` over those ids, and replaces `bindings` with the result (I3: the surface never trusts a cached binding).
  3. **Stale entries.** It removes cache entries whose id is not listed (`dropped`).
  4. **Rebuilds.** For each listed id whose cached key differs, or which has no entry, it begins a transaction, issues `SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY`, and then:
     - (a) **The stored-state checks (C8, ruling on point 3).** It runs `egret_policy::opa_guard::stored_state_refusals_via(&mut tx, id)` and `crate::overlap::stored_overlaps_via(&mut tx)`, the latter filtered to `service_id == id`. Any hit sets the entry to `NotBuilt { error: Some(<first refusal message, or "the profile serves N mask/row-filter pairs whose winner the artifact does not express (see GET /perm/overlaps)">) }` and drops any previous snapshot. It calls `record_compile_failure(pool, id, &format!("OPA snapshot: {msg}"))` and counts the id in `failed`.
     - (b) `snapshot_parts_via(&mut tx, id, MissingUsername::Omit)`. On `Ok(Some(parts))`, it builds `Snapshot::build` over `data ++ infra` and stores it with its key (`rebuilt`).
     - (c) **On a data-lowering `Err(e)` (C4):** it runs `lower_infra_via(&mut tx, id, Omit)` alone.
       - With `Ok(Some(infra))` and a previous snapshot, it stores `previous.with_infra(infra)` (`spliced`). So an admin disabled while the data lowering fails loses `WholeEp` within one refresh. It also stores `e.to_string()`, logs `tracing::warn!(service_id, error = %e, "OPA snapshot build failed; serving the previous data with fresh infrastructure")`, and calls `record_compile_failure`.
       - With no previous snapshot, or an infrastructure-lowering error, the entry becomes `NotBuilt { error }` (`failed`).
     - (d) In every outcome it records the key read, so the same failure is not retried every second.
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
async fn switching_back_to_ranger_drops_the_entry_once_no_engine_runs_opa() {
    // Bind an engine; set its applied_mechanism='opa'. UPDATE mechanism='ranger'; refresh_once →
    //   the entry stays Ready (an engine still runs OPA). Then UPDATE applied_mechanism='ranger';
    //   refresh_once → dropped == [id]; get(id) is NotOpa.
}

#[tokio::test]
async fn bindings_follow_the_engine_rows_every_refresh() {
    // An engine bound to the OPA profile → binding(engine) == Some(id). UPDATE its authz_service_id to
    //   NULL; refresh_once → binding(engine) == None.
}

#[tokio::test]
async fn a_failed_build_still_drops_a_disabled_admins_all_access() {
    // Arrange: one platform admin (fixtures `platform_role` + `role_held`, or the admin_policy_sync
    //   helpers), a qualifying impersonate engine; build once → admin may CreateTable.
    // Then break the data lowering: with the fixtures' `grant` builder (which inserts by SQL and
    //   bypasses the authoring checks) add an Allow Read grant whose principal is `user_ref("bad@ref")`;
    //   `egret_perm_ir::lower::grants::validate_user_ref` refuses '@' with `CompileError::UnusableUserRef`.
    //   Disable the admin exactly as crates/egret-perm-compile/tests/admin_policy_sync.rs::
    //   disabling_an_admin_does_not_depend_on_a_profile_compiling does (the kill switch converges infra
    //   without a compile); then bump policy_version by hand if the kill switch did not.
    // Act: refresh_once.
    // Assert: report.spliced == [id]; the admin may no longer CreateTable; a data grant decided before
    //   still decides the same; perm.compile_runs.error LIKE 'OPA snapshot:%';
    //   a second refresh_once reports nothing (the failing key is remembered).
}

#[tokio::test]
async fn a_hand_written_tag_policy_added_after_the_switch_makes_the_profile_deny() {
    // Arrange: OPA profile Ready. Insert a policy with managed_by='operator' into its tag service; bump
    //   the tag service's policy_version (migration 0182's trigger bumps the profile).
    // Act: refresh_once. Assert: report.failed == [id]; get(id) is NotBuilt with an error containing
    //   "hand-written"; the compile_runs row names it.
}

#[tokio::test]
async fn an_all_defaults_row_or_a_stored_overlap_makes_the_profile_deny() {
    // Two cases, each from Ready: (1) insert an `all - catalog` row by SQL; (2) create a stored differing
    //   mask pair the way crates/egret-perm-compile/tests/compile_runs.rs::overlapping_model does
    //   (two path masks, then move one rule's column by SQL), and bump the version.
    // Assert for each: get(id) is NotBuilt with the matching message.
}
```

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --features harness --test opa_cache`. Expected: FAIL.
- [ ] **Step 3: Implement** `opa_guard.rs` and `cache.rs`.
- [ ] **Step 4: Run** the same command, then `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --lib`. Expected: PASS.
- [ ] **Step 5: Mutation checks**, each failing the named test and then restored:
  - (a) Rebuild unconditionally (ignore the key). This fails the first test on "second report.rebuilt is empty".
  - (b) Keep the previous snapshot whole on a data failure. This fails `a_failed_build_still_drops_a_disabled_admins_all_access`.
  - (c) Skip the stored-state checks. This fails both deny tests.
  - (d) Load bindings once at the first refresh. This fails `bindings_follow_the_engine_rows_every_refresh`.
- [ ] **Step 6: Commit** `feat(perm-compile): cache OPA snapshots per profile version, re-checking stored state`.

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
/// Err when `filterResources` holds anything but exactly one table: the index semantics would be ambiguous.
pub fn expand_filter_columns(req: &OpaRequest) -> Result<Vec<Resource>, String>;
pub fn allow_result(allowed: bool) -> serde_json::Value;                // {"result": bool}
pub fn batch_result(indices: &[usize]) -> serde_json::Value;            // {"result": [..]}
pub fn row_filters_result(filters: &[String]) -> serde_json::Value;     // {"result": [{"expression": ..}]}
pub fn column_masks_result(masks: &[(usize, String)]) -> serde_json::Value; // {"result": [{"index": i, "viewExpression": {"expression": e}}]}
pub fn column_mask_result(mask: Option<&str>) -> serde_json::Value;     // {"result": {"expression": e}} or {"result": null}
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

- [ ] **Step 0: Read the committed request shapes.** Open `crates/egret-perm-eval/tests/fixtures/trino-483-opa-request-shapes.txt` (Task 1) and Task 1's report. The tests below assume:
  - `FilterColumns` puts exactly one `table` resource with `columns` into `filterResources`, and the result indices refer to `columns`;
  - a batch `GetColumnMask` puts one `column` resource per column into `filterResources`;
  - a single `GetColumnMask` puts one `column` resource into `resource`.

  If the fixture says otherwise, change `expand_filter_columns`, the parsing and the tests to match it, and say so in the task report.

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
        let x = expand_filter_columns(&r).unwrap();
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
        assert_eq!(column_mask_result(Some("NULL")), json!({"result": {"expression": "NULL"}}));
        assert_eq!(column_mask_result(None), json!({"result": null}));
    }

    #[test]
    fn filter_columns_over_two_tables_is_refused_not_guessed() {
        let t = |n: &str| json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": n, "columns": ["a"]}});
        let r = parse_request(&input(json!({"operation": "FilterColumns", "filterResources": [t("t1"), t("t2")]}))).unwrap();
        assert!(expand_filter_columns(&r).is_err());
    }
}
```

- [ ] **Step 2: Run** `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-perm-compile --lib opa::wire`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(perm-compile): parse OPA plugin requests and build its results`.

---

### Task 9: `opa_surface`, authentication and the verification cache

**Files:**
- Create: `crates/egret-perm-compile/src/opa/surface.rs`, and add `pub mod surface;` to `opa/mod.rs`. Re-export `opa_surface`, `OpaState` and `OPA_SURFACE_PREFIX` from `lib.rs`.
- Modify: `crates/egret-perm-compile/Cargo.toml`. Add `egret-engine-mgmt-api.workspace = true` if it is not already a dependency (`grep -n engine-mgmt-api crates/egret-perm-compile/Cargo.toml`).
- Test: `crates/egret-perm-compile/tests/opa_surface.rs` (new; no DB, stub verifier and a hand-built cache)

**Interfaces:**
- Consumes:
  - Task 7's `SnapshotCache`, `Lookup` and `SnapshotCache::binding`;
  - Task 8's wire functions;
  - `egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity}`.
- Produces:

```rust
pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";
/// The largest request body accepted (M13): a FilterTables/FilterColumns batch for 10k objects.
/// (Task 10 fix round 1: 4 MiB, about 28-35k tables at 120-150 B each, with a
/// pre-auth parse bound `OPA_MAX_CONCURRENT_PARSES`; was 16 MiB.)
/// Task 20 must measure a live `SHOW TABLES` on a 10k-table schema.
pub const OPA_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone)]
pub struct OpaState {
    pub cache: std::sync::Arc<crate::opa::cache::SnapshotCache>,
    pub verifier: std::sync::Arc<dyn egret_engine_mgmt_api::credential::EngineCredentialVerifier>,
    pub credentials: std::sync::Arc<VerifiedCache>,
}
/// Remembers only "this (engine, sha256(secret)) verified" (I3). Never the binding, never the secret.
pub struct VerifiedCache { /* Mutex<HashMap<(Uuid, [u8; 32]), Instant>> */ }
impl VerifiedCache {
    pub fn new(ttl: Duration) -> Self;   // production: 5 s
    /// true: verified now or within the TTL. Err only for an infrastructure failure of the verifier.
    pub async fn verified(&self, v: &dyn EngineCredentialVerifier, engine: Uuid, secret: &str) -> Result<bool, String>;
}
pub fn opa_surface(state: OpaState) -> axum::Router;   // routes under OPA_SURFACE_PREFIX
impl crate::opa::cache::SnapshotCache {
    /// Test seams.
    #[doc(hidden)] pub fn insert_for_test(&self, service_id: i64, snapshot: egret_perm_eval::Snapshot);
    #[doc(hidden)] pub fn bind_for_test(&self, engine: uuid::Uuid, service_id: Option<i64>);
}
```

- **Routes** (`axum::routing::post`, JSON body `serde_json::Value`, `axum::extract::DefaultBodyLimit::max(OPA_BODY_LIMIT_BYTES)` on the router):

| Route | Plugin setting | Refusal |
|---|---|---|
| `/allow` | `opa.policy.uri` | 200 `{"result": false}` |
| `/batch` | `opa.policy.batched-uri` | 200 `{"result": []}` |
| `/row_filters` | `opa.policy.row-filters-uri` | **403** `{"error": reason}` |
| `/batch_column_masks` | `opa.policy.batch-column-masking-uri` | **403** `{"error": reason}` |
| `/column_mask` | `opa.policy.column-masking-uri` | **403** `{"error": reason}` |

- **Each handler:**
  1. `parse_request`. On error, the refusal "malformed input: {e}".
  2. `engine` must parse as a `Uuid`, and `secret` must be present. Otherwise, the refusal "no engine credential in opa.context-file".
  3. `credentials.verified(...)`:
     - `Ok(false)` → the refusal "engine credential refused";
     - `Err(e)` → log `error!` and answer HTTP 500.
  4. `cache.binding(engine)`. `None` → the refusal "engine is bound to no profile served on OPA". The binding is read from the refresher's per-second map, never from the verifier's `EngineIdentity` (I3).
  5. `cache.get(service_id)`:
     - `NotOpa` → the refusal "profile is not on OPA";
     - `NotBuilt { error }` → the refusal "no snapshot: {error or 'building'}";
     - `Ready(s)` → answer.
  6. Answers:
     - `/allow`: `s.decide(user, op, &resource)`.
     - `/batch`: `s.filter(user, op, resources)`. For `FilterColumns`, `resources` is `expand_filter_columns(&req)`, and its `Err` is the refusal; otherwise it is `req.filter_resources`.
     - `/row_filters`: `resource` must be a `Table`; anything else is the refusal "malformed resource for GetRowFilters". Answers `s.row_filters`.
     - `/batch_column_masks`: every `filter_resources[i]` must be a `Column`, or the whole request is refused. Group them by table, pass them to `s.column_masks`, and return `(i, expr)` for the masked ones.
     - `/column_mask`: `resource` must be a `Column`. Answers `column_mask_result(s.column_masks(...).get(column))`.
- **C1: only an evaluation against a `Ready` snapshot answers 200 on the three value routes.** On those routes, every refusal above answers HTTP 403 `{"error": reason}`, because `[]`/`null` there means "no filter, no mask". `/allow` and `/batch` answer refusals with 200 `false`/`[]`.
- **Logging (M9).** Every refusal and every `Decision::Deny` logs one line: `tracing::info!(target: "egret::opa", engine = %engine_or_dash, user = %user_or_dash, operation = %op_or_dash, resource = ?resource_or_none, reason = %reason, "opa: denied")`. A `/batch` request logs one summary line, with `denied = <count>` and the first denied resource, not one line per item.
- **Metrics.** Every allow increments `metrics::counter!("egret_opa_allow_total", "route" => route)`, and every deny increments `egret_opa_deny_total` (a batch counts once). Add `metrics.workspace = true` to `Cargo.toml` if absent.
- **`EvalError::TwoMasks`** → HTTP 500 with body `{"error": "two masks on one column"}`, logged at `error`. This is the last guard; Tasks 4, 7 and 11 keep such states out.
- **`VerifiedCache`** keys on `(engine, sha256(secret))`; `sha2` is already a dependency.
  - It stores only positive verifications, for `ttl`.
  - A refused secret is not cached, so every refusal asks `DbVerifier` again. That verifier does constant work and is already exposed this way on the Ranger surface.

- [ ] **Step 1: Write the failing tests** `tests/opa_surface.rs`, driving the router with `tower::ServiceExt::oneshot`. Add `tower` as a dev-dependency if absent; `crates/egret-server/Cargo.toml` uses `tower.workspace = true`.

  Fixtures, defined once at the top of the file:

```rust
const GOOD: uuid::Uuid = uuid::Uuid::from_u128(1);    // bound to service 1 (on OPA, Ready)
const GOOD2: uuid::Uuid = uuid::Uuid::from_u128(2);   // bound to service 2 (not in the cache: a Ranger profile)
const UNBOUND: uuid::Uuid = uuid::Uuid::from_u128(3); // verified, bound to nothing
const RIGHT: &str = "right";
struct Stub { calls: std::sync::atomic::AtomicUsize }
#[async_trait::async_trait]
impl egret_engine_mgmt_api::credential::EngineCredentialVerifier for Stub {
    async fn verify(&self, id: uuid::Uuid, pw: &str) -> Result<Option<egret_engine_mgmt_api::credential::EngineIdentity>, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(([GOOD, GOOD2, UNBOUND].contains(&id) && pw == RIGHT)
            .then_some(egret_engine_mgmt_api::credential::EngineIdentity { engine_id: id, authz_service_id: Some(99) }))
    }
}
```

  `authz_service_id: Some(99)` is deliberately wrong. The surface must ignore it and use the cache's binding. The cache has:
  - `bind_for_test(GOOD, Some(1))`, `bind_for_test(GOOD2, Some(2))`, `bind_for_test(UNBOUND, None)`;
  - service 1 holding a snapshot, built with `Snapshot::build` from one `Allow Read` statement for `ann` on `lake`, one `Mask MASK_NULL` on `lake.s.t.c` for everyone, and one `RowFilter "x = 1"` on `lake.s.t`.

  The tests:

```rust
async fn post(app: &axum::Router, route: &str, engine: &str, secret: &str, action: serde_json::Value) -> (u16, serde_json::Value);

#[tokio::test] async fn right_secret_answers_from_the_snapshot_on_every_route() { /* allow→true for ann select lake.s.t[c];
    batch FilterCatalogs [x, lake]→[1]; row_filters lake.s.t→[{"expression":"(x = 1)"}];
    batch_column_masks [column c]→[{"index":0,"viewExpression":{"expression":"NULL"}}];
    column_mask column c→{"expression":"NULL"} */ }
#[tokio::test] async fn the_binding_comes_from_the_cache_not_the_verifier() { /* GOOD answers from service 1 although the stub said 99;
    after bind_for_test(GOOD, None) the next allow is false and the next row_filters is 403 */ }
#[tokio::test] async fn wrong_secret_is_refused_on_every_route() { /* allow 200 false; batch 200 []; the three value routes 403 */ }
#[tokio::test] async fn unknown_engine_is_refused_on_every_route() { /* random uuid; same statuses */ }
#[tokio::test] async fn a_ranger_profile_is_refused_on_every_route() { /* GOOD2; same statuses */ }
#[tokio::test] async fn an_unbound_engine_is_refused_on_every_route() { /* UNBOUND; same statuses */ }
#[tokio::test] async fn a_missing_context_property_is_refused_on_every_route() {}
#[tokio::test] async fn a_malformed_mask_request_is_not_an_empty_mask_list() {
    /* batch_column_masks with filterResources [{"table": …}] → 403, never 200 [];
       row_filters with resource {"catalog": …} → 403 */ }
#[tokio::test] async fn two_masks_answer_500_so_the_plugin_fails_closed() {}
#[tokio::test] async fn a_verified_secret_is_remembered_within_the_ttl_and_a_refused_one_is_not() {
    /* two allow calls with RIGHT → stub calls == 1; WRONG twice → 2 more calls */ }
#[tokio::test] async fn a_batch_logs_one_line_however_many_items_it_denies() {
    /* FilterCatalogs over 100 catalogs, none allowed → exactly one "opa: denied" line captured */ }
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
  - (b) Use `EngineIdentity::authz_service_id` instead of `cache.binding`.
  - (c) Return 200 `[]` on a value-route refusal.
  - (d) Return `[]` on `TwoMasks`.
- [ ] **Step 6: Commit** `feat(perm-compile): the OPA decision surface`.

---

### Task 10: Mount the surface and run the refresher

**Files:**
- Modify: `crates/egret-server/src/lib.rs`:
  - `AppState` (:24) gains `pub opa: Option<egret_perm_compile::OpaState>`, with `None` in `AppState::without_db`;
  - after the Ranger merge at :321, add `if let Some(opa) = state.opa.clone() { router = router.merge(egret_perm_compile::opa_surface(opa)); }`.
- Modify: `crates/egret-bin/src/main.rs`.
  - Where `RangerState` is built (:1982), when the Ranger surface is enabled **and `config.governance.opa.enabled`** (Task 5's gate, default `false`), build:
    - `OpaState { cache, verifier, credentials: Arc::new(VerifiedCache::new(Duration::from_secs(5))) }`;
    - `verifier` is a clone of the same `Arc<DbVerifier>`, bound to a variable so both states share it;
    - `cache` is `Arc::new(SnapshotCache::default())`.
  - Under the same condition, spawn `spawn_snapshot_refresher(cache, db.pool.clone(), Duration::from_secs(1), <the shutdown future spawn_ranger_retire_sweeper is given>)` next to `spawn_ranger_retire_sweeper`. With the gate off, `AppState::opa` is `None` and no refresher runs.
- Test: `crates/egret-server/tests/opa_surface_mounted.rs` (new)

**Interfaces:**
- Consumes: `egret_perm_compile::{opa_surface, OpaState, OPA_SURFACE_PREFIX}`, `opa::cache::{SnapshotCache, spawn_snapshot_refresher}`, `opa::surface::VerifiedCache`, and `egret_config::Config::governance.opa.enabled` (Task 5).

- [ ] **Step 1: Write the failing tests.**
  - Build `AppState::without_db()` with `opa: Some(...)`, using a stub verifier that rejects everything:
    - `POST /opa/v1/data/egret/trino/allow` with a minimal valid body answers 200 `{"result": false}`;
    - `GET` on the same path answers 405;
    - `/opa/v1/data/egret/trino/batch` answers 200 `{"result": []}`;
    - `/opa/v1/data/egret/trino/row_filters` answers 403.
  - With `opa: None` (the gate off), `POST /opa/v1/data/egret/trino/allow` answers 404.
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
- Modify: `crates/egret-policy/src/admin/mod.rs:115-124`. `ProfileRecompiler` gains a method, implemented by `egret_perm_compile::PermRecompiler` (`crates/egret-perm-compile/src/trigger.rs:2171`) over `crate::overlap::stored_overlaps_via(tx)`, filtered to `service_id`. Every test stub returns `Ok(vec![])`: `NoRecompile` in `crates/egret-policy/tests/admin_routes.rs:4229`, `policy_events.rs:52` and `generated_profile_guard.rs:1068`; `grep -rn "impl.*ProfileRecompiler for" crates` lists them. There is no default body, so a new implementor cannot forget it.

  ```rust
  async fn stored_overlaps_in_tx(&self, tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, service_id: i64)
      -> Result<Vec<String>, PolicyHttpError>;
  ```

  Each string is `"{kind} on {coordinate}: {rule names joined by ', '}"`, built from `StoredOverlap { kind, coordinate, rules }` (`overlap.rs:432`).
- Modify: `crates/egret-policy/src/store.rs:2479` (`create_policy`). Before inserting, refuse when `service_id` is the tag service of a profile on OPA (C8). The check is `SELECT count(*) FROM policy.authz_services WHERE mechanism = 'opa' AND config_jsonb->>'tagService' = (SELECT name FROM policy.authz_services WHERE id = $1)`. The refusal is `PolicyStoreError::CompilerOwned(format!("{name} is the tag service of a profile on OPA; OPA would ignore a hand-written policy here"))`, which answers 409.
- Create: `crates/egret-policy/src/admin/mechanism.rs`, declared from `admin/mod.rs`.
- Test: `crates/egret-policy/tests/mechanism_switch.rs` (new, DB-gated)

**Interfaces:**
- Consumes: `crate::opa_guard::{stored_state_refusals_via, StoredStateRefusal}` (Task 7), which holds the `all - …` and hand-written queries and their messages.
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MechanismName { Ranger, Opa }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchRefusal {
    Disabled,
    NotTrino { r#type: String },
    NotGenerated,
    EngineImage { engine: String, image: Option<String> },
    Stored(crate::opa_guard::StoredStateRefusal),
    StoredOverlaps { pairs: Vec<String> },
}
impl SwitchRefusal { pub fn message(&self) -> String; }
pub struct SwitchCheck {
    pub refusals: Vec<SwitchRefusal>,
    /// External engines bound to the profile: not refused, listed (C6); their operator installs the plugin.
    pub external_engines_unchanged: Vec<String>,
}
/// Every condition, in the enum's order; empty `refusals` means the switch to OPA may proceed.
pub async fn opa_switch_check(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, opa_enabled: bool,
    svc: &crate::ServiceRow, bindings: &[crate::EngineBinding], recompiler: &dyn crate::ProfileRecompiler)
    -> Result<SwitchCheck, PolicyHttpError>;
```

- **The messages, exactly:**

| Refusal | Message |
|---|---|
| `Disabled` | `"OPA is not enabled on this installation ([governance.opa] enabled)"` |
| `NotTrino` | `"only a Trino profile can use OPA; this profile is of type {type}"` |
| `NotGenerated` | `"only a generated profile can use OPA: hand-written policies are Ranger JSON that OPA cannot evaluate"` |
| `EngineImage` | `"engine {engine} runs {image or 'an unknown image'}, which does not ship the Trino OPA plugin; supported: trinodb/trino:483"` |
| `Stored(r)` | `r.message()`, Task 7's texts: ``"the profile holds the `all - …` default policies ({count}); delete them first"`` and `"{service} holds {count} hand-written policies, which OPA would ignore; delete them first"` |
| `StoredOverlaps` | `"the profile serves {n} mask/row-filter pairs whose winner the artifact does not express (see GET /perm/overlaps); resolve them first: {pairs joined by '; '}"` |

- **`EngineImage` applies only to bindings with `origin != "external"` (C6).** An external engine has no stored image (`egret_perm::stored_engine_image` reads the provisioning spec). It goes into `external_engines_unchanged`, because its operator installs the plugin config and must run Trino ≥ 483 with `plugin/opa`.
- **Switching to `Ranger`** has no refusal, and the gate does not apply to it.

- [ ] **Step 1: Write the failing tests.** One test per condition, each asserting `opa_switch_check` returns exactly that one refusal and its message, plus one that returns empty:

```rust
#[tokio::test] async fn the_gate_off_is_refused_first() { /* opa_enabled = false → [Disabled] only */ }
#[tokio::test] async fn a_starrocks_profile_is_refused_as_not_trino() {}
#[tokio::test] async fn a_manual_profile_is_refused_as_not_generated() {}
#[tokio::test] async fn a_provisioned_engine_on_an_image_without_the_opa_plugin_is_refused_by_name() {
    /* provisioned bindings with image Some("trinodb/trino:470") and None → two EngineImage refusals, in binding order */ }
#[tokio::test] async fn an_external_engine_is_listed_not_refused() {
    /* origin "external", image None → refusals empty, external_engines_unchanged == [its name] */ }
#[tokio::test] async fn all_defaults_are_refused_with_their_count() {
    /* seed_default_trino_policies on a manual trino profile (13 rows), then UPDATE origin='generated' */ }
#[tokio::test] async fn a_hand_written_row_in_the_tag_service_is_refused() {
    /* insert a policy with managed_by='operator' into the tag service */ }
#[tokio::test] async fn a_stored_overlap_is_refused_with_its_pairs() {
    /* a recompiler stub whose stored_overlaps_in_tx returns ["mask on lake.s.t.email: perm-rule-a, perm-rule-b"] */ }
#[tokio::test] async fn a_generated_trino_profile_on_483_switches() { /* empty */ }
#[tokio::test] async fn a_hand_written_policy_in_an_opa_profiles_tag_service_is_refused_at_create() {
    /* PolicyStore::create_policy on the tag service of a profile with mechanism='opa' → CompilerOwned */ }
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
- [ ] **Step 5: Mutation checks (M12).** Delete each condition from `opa_switch_check` in turn: its named test must fail. Also delete the `create_policy` guard; `a_hand_written_policy_in_an_opa_profiles_tag_service_is_refused_at_create` must fail. Restore after each, and record the pairs in the report.
- [ ] **Step 6: Commit** `feat(policy): the conditions under which a profile may switch to OPA`.

---

### Task 12: `PATCH /policies/services/{service}` with `mechanism`

**Files:**
- Modify: `crates/egret-policy/src/admin/mechanism.rs`. Add:
  - the handler `set_mechanism`;
  - the body `SetMechanismBody { mechanism: MechanismName }`;
  - the response `ProfileMechanism { name: String, mechanism: MechanismName, engines_reconfigured: Vec<String>, external_engines_unchanged: Vec<String> }`.
- Modify: `crates/egret-policy/src/admin/mod.rs`:
  - route: `.route("/policies/services/{service}", axum::routing::delete(delete_profile).patch(mechanism::set_mechanism))` at :512;
  - add `set_mechanism` to `PolicyAdminApi` `paths(...)` (:2290);
  - add `SetMechanismBody`, `ProfileMechanism` and `MechanismName` to `components(schemas(...))` (:2307);
  - `PolicyAdminState` (:80) gains `pub reconfigurer: Arc<dyn ProfileEngineReconfigurer>` and `pub opa_enabled: bool`. `egret-bin` sets `opa_enabled` from `config.governance.opa.enabled`.
  - `PolicyHttpError` gains `ConflictBody { body: serde_json::Value }` (409, served as is, mirroring `BadRequestBody`).
- Modify: `crates/egret-audit/src/governance.rs` (M1). Add `pub const PROFILE_MECHANISM: &str = "profile:mechanism";` next to `PROFILE_TAG_SERVICE` (:41). Add it to the family table (:140-160) in the `profile` family, and to `GOVERNANCE_ACTIONS` (:198-220).
- Modify: every `PolicyAdminState { … }` construction. Tests get a `NoReconfigure` stub; `grep -rn "recompiler:" crates --include='*.rs'` lists them. `crates/egret-bin/src/main.rs` gets the real impl in Task 15; until then it wires `NoReconfigure`, which logs `warn!` and returns an empty report.
- Test: `crates/egret-policy/tests/mechanism_switch.rs` (append)

**Interfaces:**
- Consumes:
  - Task 11's `opa_switch_check`;
  - `ProfileRecompiler::recompile_in_tx(&self, tx, &[i64])` (`admin/mod.rs:115`);
  - `lock_unit_rows(conn, service_id)` (`admin/mod.rs:182`), which locks the profile **and** its tag service in ascending order, the order a recompile takes (M10).
- Produces:

```rust
// crates/egret-policy-api/src/dto.rs (so egret-engine-mgmt can build one; re-exported from egret_policy)
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconfigureReport { pub applied: Vec<String>, pub failed: Vec<(String, String)> }
// crates/egret-policy/src/admin/mechanism.rs
#[async_trait::async_trait]
pub trait ProfileEngineReconfigurer: Send + Sync {
    /// Re-apply every egret-provisioned engine bound to `service_id`, each with the profile's
    /// current mechanism. Never stops at the first failure: every engine is tried.
    async fn reconfigure_bound_engines(&self, service_id: i64) -> ReconfigureReport;
}
```

  The route is `PATCH /api/control-plane/policies/services/{service}`, with body `{"mechanism": "opa" | "ranger"}`, permission `policy:update`, and tag `policy-admin`.

- **The handler:**
  1. `require(&*st.pdp, &p, "policy:update")`.
  2. `get_service(&service)`, which answers 404 when there is none.
  3. `st.engines.list_engine_bindings()`, filtered to `authz_service_id == Some(svc.id)`.
  4. Begin a transaction and `lock_unit_rows(&mut tx, svc.id)`.
  5. Re-read the row under the lock.
  6. For `opa`, run `opa_switch_check(&mut tx, st.opa_enabled, …)`. If any refusal holds, answer **409** `ConflictBody { body: {"error": <first message>, "code": "mechanism_refused", "refusals": [<every message>]} }`. Nothing is written.
  7. If the stored mechanism differs from the requested one:
     1. `UPDATE policy.authz_services SET mechanism = $2, policy_version = policy_version + 1, updated_at = now() WHERE id = $1`;
     2. `st.recompiler.recompile_in_tx(&mut tx, &[svc.id])`;
     3. record the governance event `PROFILE_MECHANISM` with summary `{"before": old, "after": new}`.

     If it is the same, nothing is written. This is the retry path of open question (a).
  8. Commit.
  9. **After the commit, always** call `st.reconfigurer.reconfigure_bound_engines(svc.id)`, also when nothing was written. That is how a re-sent PATCH re-runs a failed reconfigure.
  10. With an empty `failed`, answer 200 `ProfileMechanism { …, engines_reconfigured: report.applied, external_engines_unchanged }`.
  11. With any `failed`, answer **502**:

      ```json
      {"error": "the profile is now on {new}; engines not reconfigured: {names}. Send the same PATCH again to retry.",
       "engines_reconfigured": [..], "engines_failed": [{"engine": .., "error": ..}]}
      ```

      The switch stays committed. An engine still running OPA keeps being answered, because the refresher serves every profile with an engine whose `applied_mechanism = 'opa'` (Task 7). An engine still running Ranger keeps enforcing the Ranger artifact (S8).
- **The utoipa description** of the 200 says: "The coordinator of every bound egret-provisioned engine restarts to load the new access control; queries running on it fail. External engines are listed in `external_engines_unchanged`: their operator installs the new plugin files and must run Trino 483 or later with `plugin/opa`."

- [ ] **Step 1: Write the failing route tests.** Drive `policy_admin_router()` as `crates/egret-policy/tests/admin_routes.rs` does; reuse its auth helpers.

```rust
#[tokio::test] async fn switching_a_ready_profile_to_opa_writes_recompiles_and_reconfigures() {
    /* 200; body.mechanism=="opa"; row reads opa; policy_version moved; recompiler stub saw [id];
       reconfigurer stub saw id once; governance event row exists with action 'profile:mechanism' */ }
#[tokio::test] async fn each_refusal_is_a_409_that_names_its_condition_and_writes_nothing() {
    /* for each Task 11 case, the gate-off case included: 409, body.refusals contains the message,
       row still 'ranger', policy_version unchanged, reconfigurer never called */ }
#[tokio::test] async fn switching_back_to_ranger_is_never_refused_even_with_the_gate_off() {}
#[tokio::test] async fn the_same_mechanism_again_writes_nothing_but_reconfigures() {
    /* version unchanged, no event, reconfigurer called once */ }
#[tokio::test] async fn a_partly_failed_reconfigure_is_a_502_naming_both_lists_and_the_switch_stays() {}
#[tokio::test] async fn the_patch_locks_the_tag_service_row_too() {
    /* hold `SELECT … FOR UPDATE` on the tag service row in another connection; the PATCH waits
       (assert it has not answered after 300 ms, then release and it answers 200) */ }
#[tokio::test] async fn without_policy_update_it_is_403() {}
```

- [ ] **Step 2: Run** `env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test mechanism_switch`. Expected: FAIL.
- [ ] **Step 3: Implement**, then regenerate the API (`cargo run -p egret-bin -- openapi > openapi.json`; `cd frontend && npm run gen:api`).
- [ ] **Step 4: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test mechanism_switch
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-policy --test admin_routes
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-audit
CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib committed_openapi_json_is_current
```

  Expected: PASS.
- [ ] **Step 5: Mutation checks.**
  - (a) Return early on the same mechanism without reconfiguring. `the_same_mechanism_again_writes_nothing_but_reconfigures` must fail.
  - (b) Lock only the profile row. `the_patch_locks_the_tag_service_row_too` must fail.
  - (c) Skip the gate. The gate-off case must fail.
- [ ] **Step 6: Commit** `feat(policy): switch a profile between Ranger and OPA`.

---

## Phase 5 — engine configuration

### Task 13: `opa_plugin_files` and the mechanism on the Ranger binding

**Files:**
- Create: `crates/egret-engine-mgmt-api/src/opa_config.rs`, and add `pub mod opa_config;` to `lib.rs`.
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs:407`. `RangerBinding` gains `pub mechanism: AccessMechanism`, defined in `opa_config.rs`:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub enum AccessMechanism { #[default] Ranger, Opa }`;
  - every `RangerBinding { … }` literal sets `mechanism: AccessMechanism::Ranger` (`grep -rn "RangerBinding {" crates`).
- Modify: `crates/egret-policy/src/admin/mod.rs:1137-1164` (`trino_config`). For a profile whose `mechanism == "opa"`, the response files are `opa_plugin_files(PLACEHOLDER_ENGINE_ID, &st.engine_egret_url, PASSWORD_PLACEHOLDER)`, and `warnings` adds `OPA_LOG_REQUESTS_WARNING` and `OPA_EXTERNAL_ENGINE_NOTE`. `TrinoConfigResponse` gains `pub mechanism: String`.
- Test: inline in `opa_config.rs`; `crates/egret-policy/tests/admin_routes.rs` (append one test)

**Interfaces:**
- Produces:

```rust
pub const OPA_CONTEXT_FILE: &str = "egret-opa-context.properties";
pub const OPA_CONTEXT_PATH: &str = "/etc/trino/egret-opa-context.properties";
pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";   // equal to egret_perm_compile's; pinned by a test in Task 15
pub const PLACEHOLDER_ENGINE_ID: &str = "REPLACE_WITH_THE_ENGINE_ID";
pub const OPA_LOG_REQUESTS_WARNING: &str = "Do not enable opa.log-requests: every request carries egret.secret in input.context.properties, and the coordinator log would hold it.";
/// C6: egret cannot see an external engine's image.
pub const OPA_EXTERNAL_ENGINE_NOTE: &str = "The coordinator must run Trino 483 or later with plugin/opa; egret cannot check an external engine's image.";
pub fn opa_plugin_files(engine_id: &str, egret_url: &str, secret: &str) -> crate::ranger_config::RangerPluginFiles;
```

  `opa_plugin_files` returns, in order, `("access-control.properties", …)` and `(OPA_CONTEXT_FILE, …)`. The base URL is `{egret_url without trailing /}{OPA_SURFACE_PREFIX}`. The first file is exactly:

```properties
access-control.name=opa
opa.policy.uri=<base>/allow
opa.policy.batched-uri=<base>/batch
opa.policy.row-filters-uri=<base>/row_filters
opa.policy.batch-column-masking-uri=<base>/batch_column_masks
opa.policy.column-masking-uri=<base>/column_mask
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
    assert!(ac.contains("opa.policy.column-masking-uri=http://egret:8080/opa/v1/data/egret/trino/column_mask\n"),
            "both masking URIs, so neither mask path can fail open");
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
- `crates/egret-provisioner/src/docker/plan.rs` needs **no change**: `resolve_mounts` (:296-309) already writes every `ranger-secret` sub-path from the Secret's `stringData` at mode `0o600`, so the context file is private once Task 14's mount exists. The Docker test below pins that.
- Test: inline tests in `trino.rs` and `docker/plan.rs`

**Interfaces:**
- Consumes: Task 13's `opa_plugin_files`, `AccessMechanism`, `OPA_CONTEXT_FILE` and `OPA_CONTEXT_PATH`.
- Produces: an `EngineSpec` whose `ranger.mechanism == Opa` renders an OPA coordinator. `config_checksum` (:530) already hashes the ConfigMap and the Secret, so a mechanism change rolls the pods. The Docker update recreates every container (`docker/provisioner.rs:231`).

- [ ] **Step 1: Write the failing tests** in `trino.rs`'s test module. Base them on the existing binding helpers `bound(credential)` and `bound_as(id, credential)` (`crates/egret-provisioner/src/trino.rs:1501-1516`) and the spec builder their tests use:

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
  - Add `pub(crate) async fn resolve_profile_mechanism(st: &CpState, authz_service_id: Option<i64>) -> Result<AccessMechanism, CpError>`. It reads `PolicyService::mechanism` (Task 5) from `st.authz.list_services()`, like `resolve_ranger_service_name` (:2838); an unbound engine is `Ranger`.
  - `build_provision_req_with` (:749-770) passes it into `ranger_binding`, which sets `RangerBinding::mechanism`.
  - **Image guard (I6).** In `build_provision_req_with`, when the engine is Trino, not external, and the mechanism is `Opa`, refuse with `CpError::Conflict(format!("engine {name} runs {image}, which does not ship the Trino OPA plugin, and its profile is on OPA; use trinodb/trino:483 or switch the profile to Ranger"))` unless `egret_perm::ships_opa_plugin("trino", &image)`, where `image` is `egret_perm::stored_engine_image("trino", &engine.spec)`. Every apply passes through this function, so a bind, an image change and a create are all refused before anything is rendered. If `CpError` has no `Conflict` variant, use the variant `external.rs` uses for its 409s (`grep -n "CpError::" crates/egret-engine-mgmt/src/external.rs | head`).
  - **`applied_mechanism` (open question (a)).** After `apply_built` succeeds in each of its callers, run `UPDATE engine_mgmt.engines SET applied_mechanism = $2 WHERE id = $1`, with the mechanism of the built request. Put it inside `apply_built` (:899) itself, at the point where the provisioner accepted the request (`finish_apply`, :957), so no caller can forget it.
- Modify: `crates/egret-engine-mgmt/src/store.rs`. Add `pub async fn list_engines_bound_to_authz_service(&self, service_id: i64) -> Result<Vec<Engine>, StoreError>`, next to `count_engines_bound_to_authz_service` (:401), ordered by `name`.
- Modify: `crates/egret-engine-mgmt/src/engines.rs`. Add `pub async fn reapply_bound_engines(st: &CpState, service_id: i64) -> egret_policy_api::ReconfigureReport` (M4b).
  - `prov` is `st.engine_provisioner.as_ref()`, as `catalog_apply::apply_trino` (`catalog_apply.rs:172-200`) takes it. If it is `None`, every engine is reported failed with "no engine provisioner is configured".
  - For each bound engine, in name order:
    - skip it when `origin == "external"`, or when `status == engine_status::DELETING` (`store.rs:19`);
    - otherwise run `build_provision_req(st, &engine)` and then `apply_built(st, prov.clone(), &engine, &built, ApplyOp::Update)`;
    - on `ApplyBuiltError::CredentialChanged`, re-read the engine and retry once, the way `catalog_apply::retry_apply` (`:1388`) re-reads before applying;
    - record `applied` or `failed: (name, error)`, and **continue with the next engine**.
  - `ReconfigureReport` is `egret_policy_api::ReconfigureReport` (Task 12).
- Modify: `crates/egret-engine-mgmt/src/external.rs`:
  - add `.route("/engines/{id}/opa-plugin-config", get(opa_plugin_config))` (:179);
  - add `opa_plugin_config` to `ExternalEngineApi` `paths(...)` (:161).
  - The handler mirrors `ranger_plugin_config` (:847): `engine:read`, 409 unless external, 409 unless bound, and 409 with `"profile {name} is on Ranger; use /ranger-plugin-config"` unless the profile is on OPA. It returns `OpaPluginConfigDto { files: Vec<RangerPluginFileDto>, warnings: Vec<String>, admin_must_supply: Vec<String> }`, from `opa_plugin_files(&engine.id.to_string(), &st.engine_egret_url, PASSWORD_PLACEHOLDER)`:
    - `warnings` = `[OPA_LOG_REQUESTS_WARNING, OPA_EXTERNAL_ENGINE_NOTE]` + `download_url_warning(&st.engine_egret_url)`;
    - `admin_must_supply` = `["egret.secret: the engine's Ranger plugin credential (rotate it with POST /api/control-plane/engines/{id}/rotate-ranger-credential to see it once)", "a Trino 483 or later coordinator with plugin/opa (it ships in trinodb/trino:483)"]`.
- Modify: `crates/egret-bin/src/main.rs`. Implement `egret_policy::ProfileEngineReconfigurer` for a `ReconfigurerImpl { cp: Arc<tokio::sync::OnceCell<egret_engine_mgmt::CpState>> }`.
  - When the cell is empty, its method reports every engine as failed with "engine management is not wired yet".
  - Otherwise it calls `egret_engine_mgmt::reapply_bound_engines`.
  - Fill the cell right after the `CpState` is built (:3032), and replace Task 12's `NoReconfigure` in `PolicyAdminState` with it.
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (append); `crates/egret-perm-compile/src/opa/surface.rs` (one inline test pinning the prefix constant)

**Interfaces:**
- Consumes:
  - Task 12's `ProfileEngineReconfigurer` and `ReconfigureReport`;
  - Task 13's `opa_plugin_files`, `AccessMechanism`, `OPA_LOG_REQUESTS_WARNING` and `OPA_EXTERNAL_ENGINE_NOTE`;
  - Task 14's rendering;
  - Task 11's `egret_perm::ships_opa_plugin`;
  - Task 5's `engine_mgmt.engines.applied_mechanism`.
- Produces:
  - `egret_engine_mgmt::reapply_bound_engines`, re-exported from `lib.rs`;
  - `GET /api/control-plane/engines/{id}/opa-plugin-config`;
  - `applied_mechanism` kept current on every provisioned apply.

- [ ] **Step 1: Write the failing tests** in `routes.rs`, with that file's test state and its provisioner fake `FakeEnginePort` (`crates/egret-engine-mgmt/tests/routes.rs:820`; M4):

```rust
#[tokio::test] async fn an_opa_profiles_engine_is_applied_with_the_opa_binding_and_records_it() {
    /* bind a trino engine on trinodb/trino:483 to a profile, UPDATE mechanism='opa', call reapply_bound_engines;
       FakeEnginePort's last `update` spec has ranger.mechanism == Opa and ranger.engine_id == engine id;
       the row's applied_mechanism == 'opa'; report.applied == [name] */ }
#[tokio::test] async fn reapply_skips_external_and_deleting_engines_and_tries_every_other_one() {
    /* two provisioned engines, the fake rejects the first: report.failed names it, report.applied names the second */ }
#[tokio::test] async fn an_engine_on_an_image_without_the_opa_plugin_cannot_be_applied_on_an_opa_profile() {
    /* engine spec imageVersion "470": build_provision_req → 409 naming the engine; nothing reached the fake */ }
#[tokio::test] async fn opa_plugin_config_for_an_external_engine_on_an_opa_profile() {
    /* 200; files[0].name == "access-control.properties"; files[1].content contains
       "egret.engine=<id>" and the PASSWORD_PLACEHOLDER; warnings[0] == OPA_LOG_REQUESTS_WARNING;
       warnings[1] == OPA_EXTERNAL_ENGINE_NOTE */ }
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
- [ ] **Step 5: Mutation checks.**
  - (a) Remove the image guard. The image test must fail.
  - (b) Stop at the first failed engine. `reapply_skips_external_and_deleting_engines_and_tries_every_other_one` must fail.
  - (c) Do not write `applied_mechanism`. The first test must fail.
- [ ] **Step 6: Commit** `feat(engine-mgmt): apply the profile's mechanism and serve the OPA plugin config`.

---

## Phase 6 — explain

### Task 16: `/perm/explain*` answers an OPA profile through the evaluator

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (and the `PermState` struct it uses: add `pub opa_cache: Option<Arc<crate::opa::cache::SnapshotCache>>`, set in `egret-bin` next to Task 10's cache, `None` in tests):
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
  1. Get the snapshot (M11, I9):
     - `PermState` gains `pub opa_cache: Option<Arc<crate::opa::cache::SnapshotCache>>`, set by `egret-bin` to Task 10's cache when the gate is on.
     - If the cache holds `Ready(s)` with `s.key()` equal to the row's `(policy_version, tag_version)`, use it.
     - Otherwise build one with `crate::opa::build::build_snapshot_via(&mut *conn, service_id, MissingUsername::Omit)`, in the explain's own read-only transaction. `Omit` never mints, so the read-only transaction holds. `None` cannot happen for a Trino profile.
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
  - at the crate root, `include!("opa_support/fixtures_host.rs");` then `#[path = "golden/fixtures.rs"] mod fixtures;` (Task 6: `Names`, `capture` and `compare_or_bless` stubs, I2);
  - `#[path = "../../egret-ranger-eval/tests/support/ranger_jvm.rs"] mod ranger_jvm;`;
  - `#[path = "opa_support/to_ranger.rs"] mod to_ranger;`;
  - `egret_perm_compile::harness_support::build_snapshot_for_test` (Task 6);
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
- **Overlaps.**
  - `refuse_overlaps` refuses a **differing** pair at compile. A refused state is redrawn and counted.
  - It **admits agreeing ties**: two masks of one type with no expression on one column, or two identical filters (`crates/egret-policy/src/ambiguous_arbitration.rs:18-32`). The generator draws such ties on purpose, in at least 1 state in 10, and the run asserts that every tie case agrees with Ranger (I1).
- **Each question** draws a user, an operation from `TRINO_483_OPERATIONS`, and a resource of that operation's shape over the state's vocabulary.
  - At least 5 % of `SelectFromColumns` questions have an **empty column list** (C2: `count(*)`).
  - Reach questions include objects with a withheld or denied descendant or ancestor.
- **Ranger path:**
  1. `recompile_service`;
  2. `served_artifact_json_via`;
  3. for each question, `to_ranger::ranger_asks`, with the request's `tags` filled from the state's own classifications:
     - on a column ask, column tags are `SELF` and table tags `ANCESTOR`;
     - on a table ask, table tags are `SELF` and the table's column tags `DESCENDANT`;
     - on a schema or catalog `_any` ask, every tag below it is `DESCENDANT` (I7).

     **This is a correlated oracle:** both sides read the state's own classifications. So Layer 1 does not measure classification resolution, for example the `''` table-level sentinel of C3. Layer 2 measures it (Task 21), and Task 6's DB test pins the sentinel;
  4. one `ask_ranger_json(payload, "2.8.0", "opa-l1")` call for all cases of all states.
- **OPA path:** `build_snapshot_via` → `decide`/`filter`/`row_filters`/`column_mask_choice`.
- **Compared:**
  - allow and deny (all asks must allow);
  - a case where the evaluator returns `EvalError::TwoMasks` counts as "query fails" on the OPA side. It agrees only with a Ranger case the generator marked as a refused state, which cannot occur after redrawing, so any such case is an unexplained difference (I7);
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

  - **Matching.** Each difference found is classified by `(operation, ranger_outcome, opa_outcome)`. It is matched to an entry by `operations`, plus a predicate function named after the `id` in the test file.
  - **Pairing (I7).** The test also asserts that every `id` in the file has a predicate function (a `match id { … }` with no wildcard arm), and that every predicate has an `id`.
  - **Writing entries.** The file starts empty. Every entry that is added states its mechanism, from a reading of the Ranger source or the measured case, never a guess.
  - **Never explainable (ruling (b)).** A difference where **OPA allows and Ranger denies** (access, reach or filter visibility) **can never be explained**. The checker refuses any entry whose predicate matches such a case: it fails with "OPA allows what Ranger denies: fix the evaluator", whatever `explained.toml` says. Only differences where OPA is stricter, or where values differ in a way that does not widen access, may be listed.
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
- [ ] **Step 3: Mutation checks**, each restored afterwards:
  - (a) Flip deny-wins in `egret-perm-eval` (Task 3, mutation a). `opa_differential` must fail.
  - (b) Add a bogus `explained.toml` entry. The run must fail with "stale".
  - (c) Delete a needed entry, if any exist. The run must fail with "unexplained".
  - (d) Add an entry whose predicate matches an OPA-allows/Ranger-denies case (make one by applying Task 3's mutation h). The run must fail with "fix the evaluator", not pass as explained.
  - (e) Treat an empty column list as allowed (Task 3, mutation g). `opa_differential` must fail.
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
      # S8: every Trino golden fixture compiled on an OPA profile equals its blessed Ranger artifact.
      - name: The Ranger artifact does not depend on the mechanism
        run: env -u EGRET_ALLOW_DB_SKIP EGRET_GOLDEN_MECHANISM=opa cargo test -p egret-perm-compile --features harness --test golden_compile --no-fail-fast
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
- Create: `deploy/opa-e2e/docker-compose.yml`, `deploy/opa-e2e/egret.env`, `deploy/opa-e2e/trino-ranger/catalog/lake.properties.tmpl`, `scripts/opa-e2e/up.sh`, `scripts/opa-e2e/down.sh`, `scripts/opa-e2e/live-check.sh`

**The stack (C6).** Compose project **`egret-opa-e2e`**, with `name: egret-opa-e2e` at the top of the file, so `docker compose -f deploy/opa-e2e/docker-compose.yml` can never act on another project. Services:
- `postgres` (`postgres:16`);
- `rustfs` (`rustfs/rustfs:1.0.1`);
- `egret`, built from `deploy/docker/backend.Dockerfile`, image `egret-opa-e2e-backend:dev`. Its environment:
  - `EGRET_RANGER__ENABLED=true`;
  - `EGRET_GOVERNANCE__OPA__ENABLED=true` (the gate is on for this stack only);
  - `EGRET_ENGINES__PROVISIONER=docker`;
  - `EGRET_ENGINES__DOCKER__CALLBACK_URL=http://egret:8080`;
  - `EGRET_ENGINES__DOCKER__ALLOWED_IMAGES='["trinodb/trino"]'`.

  It also mounts the Docker socket through `tecnativa/docker-socket-proxy:0.3.0@sha256:9e4b9e7517a6b660f2cc903a19b257b1852d5b3344794e3ea334ff00ae677ac2`, copying that service and its `CONTAINERS/IMAGES/NETWORKS/POST` settings from `deploy/docker-compose.demo.yml:69`. Read `EnginesDockerConfig` (`crates/egret-config/src/lib.rs:597`) for the exact keys, and use those. The provisioner creates its containers on the compose network `egret-opa-e2e_default`; set the network key `EnginesDockerConfig` names.
- `trino-ranger`: `trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534`, with `/etc/trino` bind-mounted from `${WORK}/trino-ranger/etc`. This is the **external** engine.
- **`trino-opa` is not a compose service.** It is a **provisioned** engine that egret's Docker provisioner creates on the same network, which means:
  - it qualifies for infrastructure (`qualifies_for_infra` requires `origin != external`, `crates/egret-core/src/engine_delegation.rs:86-88`);
  - it carries the image `trinodb/trino:483`, which the switch checks;
  - it exercises Task 14's Docker path.

  The external `trino-ranger` on the same profile inherits the profile's infrastructure rows.

Host ports: `28080` (egret) and `28082` (trino-ranger). The provisioned `trino-opa` is reached through `docker exec`. The demo stack's 18080 is never used.

**`up.sh`** (idempotent):
1. Build and start the stack, then wait for `GET /healthz` on 28080.
2. Log in as the bootstrap admin from `egret.env`.
3. Create the warehouse `opa_e2e` and the catalog `lake` via the catalog API. Copy the calls from `deploy/seed-demo-catalog.sh`.
4. Create the Trino profile `opa_e2e`.
5. **Create `trino-opa`** through `POST /api/control-plane/engines`:
   - `type: "trino"`, image version `483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534`;
   - `trinoConfig: {"coordinator": {}, "delegationMode": "impersonate", "auth": {"password": {}}}`;
   - bound to `opa_e2e`, with `lake` attached.
6. **Register `trino-ranger`** as an external engine bound to `opa_e2e`, with `lake` in its `catalogs` (`POST /api/control-plane/engines/external`, `rangerPluginVersion: "2.8.0"`, `catalogs: [<lake id>]`). The `lake` attachment is what makes `catalogs_for_service_via` see the catalog; without it every grant is `TableNotInCatalog`.
7. Rotate `trino-ranger`'s Ranger credential to read the password once (`POST …/rotate-ranger-credential`). Write its `access-control.properties` and `ranger-trino-security.xml` from `GET /engines/{id}/ranger-plugin-config`, with the placeholder replaced by the password.
8. Write `catalog/lake.properties` for `trino-ranger` from `lake.properties.tmpl`: the Iceberg REST catalog at `http://egret:8080/catalog/v1`, warehouse `opa_e2e`, and the RustFS S3 endpoint. Read the provisioned engine's own catalog file (`docker exec <trino-opa> cat /etc/trino/catalog/lake.properties`) and copy its keys, so both engines read the same data.
9. Restart `trino-ranger`.
10. Seed `lake.s1.t1` and `lake.s1.t2` (1000 rows) through `trino-ranger` as the platform admin.
11. Make the profile generated by creating one grant.
12. `PATCH /policies/services/opa_e2e {"mechanism":"opa"}`. Expect 200, with `engines_reconfigured == ["trino-opa"]` and `external_engines_unchanged == ["trino-ranger"]`. The provisioned coordinator restarts on its own.

**`live-check.sh`:**
1. As admin:
   - grant `ann` read on `lake.s1.t1` (not on `t2`);
   - mask `lake.s1.t1.email` with `MASK_HASH`;
   - add a row filter `region = 'us'` on `lake.s1.t1`;
   - deny `ann` read on `lake.s1.t1.ssn`.
2. Wait for egret's refresher (2 s).
3. Then, through `trino-opa` as `ann` (`docker exec <trino-opa container> trino --user ann --execute …`):
   - `SELECT email FROM lake.s1.t1 LIMIT 1` returns a 64-hex string;
   - `SELECT DISTINCT region FROM lake.s1.t1` returns only `us`;
   - `SELECT * FROM lake.s1.t2` fails with `Access Denied`;
   - **`SELECT count(*) FROM lake.s1.t2` fails with `Access Denied`** (C2);
   - **`SHOW COLUMNS FROM lake.s1.t1` does not list `ssn`** (C5);
   - **`SELECT column_name FROM lake.information_schema.columns WHERE table_name = 't1'` does not list `ssn`**, and `SELECT email FROM lake.s1.t1` stays masked (open question (c));
   - `grep "opa: denied" <egret logs>` shows the denies with `reason=`.
4. Run the same statements on `trino-ranger` and print both sides.

- [ ] **Step 1: Write the six files.** Every `docker compose` invocation in them uses `-f deploy/opa-e2e/docker-compose.yml -p egret-opa-e2e`. `down.sh` also deletes the provisioned engine through `DELETE /api/control-plane/engines/{id}` before `docker compose … down -v`, so no container outlives the project.
- [ ] **Step 2: Rebuild check.** After `up.sh` builds the image, run `docker run --rm --entrypoint sh egret-opa-e2e-backend:dev -c 'strings /usr/local/bin/egret | grep -c opa_surface'`. Expected: ≥ 1, so a stale build cache did not ship an old binary.
- [ ] **Step 3: Run** `scripts/opa-e2e/up.sh && scripts/opa-e2e/live-check.sh`. Expected: every assertion prints PASS. Save the output in the PR body.
- [ ] **Step 4: Run** `scripts/opa-e2e/down.sh`. It removes only project `egret-opa-e2e` and the engine egret provisioned for it. `docker ps --filter label=com.docker.compose.project=egret-demo` must show the same containers before and after.
- [ ] **Step 5: Commit** `test(e2e): an own compose stack and the live check for Trino on OPA`.

**This ends package 2b-1.** Open its PR with:
- `Closes` for the SP2b tracking issue, if one exists;
- the live-check output;
- the Layer 1 tally;
- every `explained.toml` entry.

**The 2b-1 docs PR** in `egret-data/docs` ships a short "Trino on OPA (preview)" page, built from Task 25's items 1, 2, 5, 6, 7, 9 and 10, with:
- the API only (`PATCH …/services/{service}`, `GET /engines/{id}/opa-plugin-config`);
- the `[governance.opa] enabled` key and its default `false`;
- no UI steps, no screenshots, and no latency numbers, which arrive in 2b-2 (I8).

---

# Package 2b-2

### Task 21: Layer 2, the end-to-end differential

**Files:**
- Create: `scripts/opa-e2e/layer2.sh`, `scripts/opa-e2e/scenarios.sql.d/*.sql`, `scripts/opa-e2e/compare.py`
- Modify: `.github/workflows/ci.yml` (job `opa-e2e`)
- Modify: `crates/egret-perm-compile/tests/opa_differential.rs` (M14). Its TOML reader accepts the new optional `layer = [1, 2]` key, a missing key meaning `[1]`, and its stale-entry check counts only entries tagged with layer 1.
- Modify: `crates/egret-perm-compile/tests/opa_differential/explained.toml`, for entries Layer 2 adds.

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
  - impersonation: `trino --user egret_delegator --session-user ann` (`egret_core::engine_principals::DELEGATOR_PRINCIPAL`);
  - kill: `CALL system.runtime.kill_query(...)` against another user's query;
  - `SET SESSION query_max_run_time = '1m'`.
- **`compare.py`** (run with `python3 -I`):
  - for each `(scenario, user, statement)`, compares the `trino-ranger` and `trino-opa` outputs: the sorted-row sha256, then the per-column values, then the error class (`Access Denied` or not);
  - writes `layer2-report.json`;
  - exits non-zero on any difference that matches no `explained.toml` entry, or on an entry tagged `layer = 2` that no case produced.

  It reads the optional `layer = [1, 2]` key; a missing key means `[1]`. `compare.py` applies the same never-explainable rule as Task 18: a case where `trino-opa` returns more rows, more columns or clearer values than `trino-ranger` is never explained, and fails.
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
  - **Requests per query:** count the `egret_opa_allow_total + egret_opa_deny_total` delta from `GET /metrics` around each query. Run once with the batched URI, and once without it: `docker exec <trino-opa> sed -i '/^opa.policy.batched-uri=/d' /etc/trino/access-control.properties`, then `docker restart <trino-opa>`. The Docker provisioner writes that file into the container (`docker/plan.rs`, `files_tar`), so the edit survives a restart. Afterwards, restore the file by re-sending `PATCH …/services/opa_e2e {"mechanism":"opa"}`, which re-applies the engine.
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
    - when the profile has external engines (`dto.engines` with `origin === "external"`) and the target is OPA, a second paragraph, verbatim: "External engines are not restarted by egret. Their operator must install the new plugin files, and run Trino 483 or later with plugin/opa." (ruling C6);
    - buttons "Cancel" and "Switch".
  - On confirm, it calls the mutation with `{ service: profile.name, data: { mechanism } }`.
    - **On success:** invalidate `getPolicyAdminListProfilesQueryKey()`, `toast.success("Switched to OPA.")` or `"Switched to Ranger."`, and show a `role="status"` line listing `engines_reconfigured` and, when non-empty, `external_engines_unchanged`.
    - **On 409:** show every string of the body's `refusals` in an `Alert variant="destructive"`, one per line, and keep the previous selection (FE-ACTION-3). The gate-off refusal ("OPA is not enabled on this installation …") arrives this way too.
    - **On 502:** show the server's `error`, the `engines_failed` list, and a "Retry" button that re-sends the same PATCH.

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
it("warns about external engines before switching to OPA", async () => { /* profileDto with an external engine */ });
it("offers a retry after a 502 and re-sends the same PATCH", async () => {});
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
  - the description reads "Install these two files on a Trino 483 or later coordinator with plugin/opa: egret answers its requests, and cannot check an external engine's image.";
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
   - on an installation with `[governance.opa] enabled = true` (default `false` until this release; `EGRET_GOVERNANCE__OPA__ENABLED`);
   - whose egret-provisioned engines run `trinodb/trino:483`. External engines are not checked: their operator must run Trino 483 or later with `plugin/opa`;
   - that holds no `all - …` default policies;
   - whose profile and tag service hold no hand-written policies (after the switch, hand-written policies in the tag service are refused);
   - that serves no stored differing mask or row-filter pair (`GET /perm/overlaps`).

   The page lists each refusal message verbatim (Task 11's table) with what to do.
3. **How to switch.** Governance → Engines → the profile's Advanced tab → Access control → OPA → Switch. API: `PATCH /api/control-plane/policies/services/{service}` with `{"mechanism":"opa"}`, which needs `policy:update`.
4. **What the switch does:**
   - it recompiles;
   - it restarts every bound egret-provisioned coordinator, and **queries running on it fail**;
   - the Ranger policies keep compiling, so switching back is immediate;
   - if an engine could not be re-applied, the answer is 502 with the engines that failed; send the same request again to retry. egret keeps answering OPA for an engine still running it.
5. **External engines.** Download the two files from the engine page or `GET /api/control-plane/engines/{id}/opa-plugin-config`. Put the engine's Ranger credential in `egret.secret`; rotate it to see it once. Copy both files into `/etc/trino/`, and restart the coordinator.
6. **Warning: never enable `opa.log-requests`.** Every request carries `egret.secret`.
7. **Fail-closed behaviour.** A refused request to the row-filter or mask endpoints answers 403, so the query fails rather than returning unmasked or unfiltered data. A profile whose stored state stops being expressible (a hand-written or `all - …` policy, a stored overlap) denies every query until it is fixed; the reason is on the Governance overview as a compile failure.
8. **When egret is unreachable,** Trino denies every query until egret is back. Give the measured restart window from Task 22's report. The 2b-1 docs PR says "measured in a later release" here.
9. **Known differences from Ranger:** one row per `explained.toml` entry (`id`, what differs, which side is right).
10. **Explain.** On an OPA profile, the Explain page answers from the same evaluator and shows its reason.

- [ ] **Step 1:** Write the page and the screenshots in a branch of `egret-data/docs`, and open its PR. Link it from this repo's PR body and tick the Docs box of the PR template.
- [ ] **Step 2:** Check that every message quoted on the page equals the code's text. Grep each quoted sentence in this repo, for example `grep -rn "delete them first" crates/egret-policy/src/admin/mechanism.rs`.

---

### Task 26: Turn OPA on by default

**Files:**
- Modify: `crates/egret-config/src/lib.rs`. `OpaSettings::enabled` defaults to `true`: `#[serde(default = "default_true")]` on the field, with a `fn default_true() -> bool { true }` beside it. Rewrite its doc comment: "On by default since SP2b-2; an operator can still turn it off, and then the surface is not mounted and a switch to OPA is refused".
- Modify: the test from Task 5, renamed `governance_opa_is_on_by_default`.
- Modify: `deploy/helm` values, if the chart renders `EGRET_GOVERNANCE__OPA__ENABLED` (`grep -rn GOVERNANCE deploy/helm`). If it does not, add nothing.
- Modify (in `egret-data/docs`): the "Trino on OPA" page's prerequisites say the key defaults to `true` from this release, and the release notes mention it.

**Preconditions, checked by the controller before this task starts:**
- Tasks 21–25 are merged;
- `opa-e2e` is green on `main`;
- the latency report (Task 22) concludes on S2.

- [ ] **Step 1: Change the test first.** Run `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-config governance`. Expected: FAIL.
- [ ] **Step 2: Flip the default.** Run the same command, then `CARGO_TARGET_DIR=$PWD/target-sp2b cargo test -p egret-server --lib`. Expected: PASS.
- [ ] **Step 3: Commit** `feat(config): turn the OPA target on by default`.

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
| Refusals (200 on allow/batch, 403 on value routes, C1) | 9 |
| Snapshot cache keyed by version; no DB on the decision path | 7, 9 (verification cache and binding map, deviation 7) |
| Config gate (I8) | 5, 10, 11, 12, 26 |
| Stored-state refusals, at the switch and on every build (C8, point 3) | 7, 11 |
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
| Docs | 25, 26 |
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
  - The reach semantics (D2) are the conservative rule (ruling (b)). Layer 1 can only show OPA stricter than Ranger there, and Task 18's checker refuses to explain the other direction.
  - Task 6's split must leave `golden_compile` byte-identical.
  - Task 12's 502 path leaves engines on the old mechanism. Both directions stay served (S8 for Ranger; `applied_mechanism` for OPA), and a re-sent PATCH retries (Tasks 7, 12, 15, 23).
