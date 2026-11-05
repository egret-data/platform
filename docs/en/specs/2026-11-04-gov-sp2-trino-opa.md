# Governance sub-project 2 — Trino on OPA

Umbrella: [governance layer concepts](2026-11-02-governance-layer-concepts.md) (sub-project 0).
Builds on sub-project 1 (neutral IR + capability matrix, #833).

## Goal and acceptance

A Trino engine runs on OPA behind a per-profile switch. Acceptance, from the umbrella spec:

1. a Trino engine bound to an OPA profile serves real queries, with masks, row filters and refusals in force;
2. differential parity with Ranger on the corpus, every difference explained;
3. latency measured.

## Decisions

| # | Decision | Why |
|---|---|---|
| S1 | egret itself answers the OPA REST protocol (`POST …` `{"input": …}` → `{"result": …}`). There is no OPA binary and no Rego. | One evaluator serves Trino and `explain`; no second policy language to keep in step with the IR. |
| S2 | The decision endpoint runs inside the egret server process, mounted by `egret-server` next to `ranger_surface()`. Its router, `opa_surface()`, lives in `egret-perm-compile`, which holds the loaders the snapshot build needs (amended 2026-11-04 at plan review). The evaluator is its own pure crate, so it can move into a sidecar later. | Simplest delivery. Whether a sidecar is needed is decided by the latency and restart numbers this sub-project measures, not in advance. |
| S3 | Infrastructure policies move into the IR in this sub-project, for both Ranger and OPA. | Trino asks OPA about impersonation, query execution, kill and session properties on every query; they cannot stay stored Ranger JSON. One source keeps both targets equal. |
| S4 | One spec, two plans, two PRs in order: **2a** infrastructure in the IR (Ranger output byte-identical), then **2b** the OPA target. | 2a is internal and proven by the golden fixtures; it lands without waiting on 2b's risks. |
| S5 | Trino authenticates to egret with the engine secret carried in `opa.context-file`, which the plugin copies into `input.context.properties` of every request. | Measured on `trino-opa-483.jar`: the plugin has no setting for an `Authorization` header (its HTTP client offers only TLS key and trust stores). The context file gives the same trust as today's Ranger Basic auth without a TLS listener on egret. |
| S6 | Only a **generated** Trino profile may switch to OPA. | Hand-written policies are Ranger JSON and mean nothing to OPA (umbrella D8). |
| S7 | egret resolves a user's groups from its own directory and ignores `identity.groups` sent by Trino. | Trino does not know egret's groups; the IR already expands them (umbrella D3). |
| S8 | An OPA profile still compiles its Ranger artifact. | Switching back is immediate, and the live differential always has an oracle. |

### Amendments to the umbrella spec

Made in the same PR as this spec:

- Architecture: the `opa(trino)` delivery is "decision API served by egret", not "OPA bundle (egret as bundle server)". Delivery item 3 changes accordingly.
- Verification item 3: the differential for OPA is egret's evaluator against real Ranger (decision level) and Trino-on-OPA against Trino-on-Ranger (end to end), not "an OPA harness (real OPA)".
- Architecture item 2: infrastructure policies are generated from the IR. They still materialise as `managed_by='infra'` rows, because those rows are the compiler's output (as generated policies are), not hand-authored input.

## Facts this design rests on

Measured on `trinodb/trino:483`:

- `plugin/opa` ships in the image (`io.trino_trino-opa-483.jar`), next to `plugin/ranger`.
- Plugin settings: `opa.policy.uri`, `opa.policy.batched-uri`, `opa.policy.row-filters-uri`, `opa.policy.column-masking-uri`, `opa.policy.batch-column-masking-uri`, `opa.allow-permission-management-operations`, `opa.context-file`, `opa.log-requests`, `opa.log-responses`, and the HTTP client's `opa.http-client.*` settings.
- Request input: `context { identity { user, groups }, softwareStack { trinoVersion }, properties { … }, queryId }` and `action { operation, resource | filterResources[] | targetResource, grantee }`.
- Results:
  - `allow`: a boolean.
  - Batched filters: a list of the allowed indices.
  - Row filters: a list of `{expression, identity?}`.
  - Batch column masks: a list of `{index, viewExpression {expression, identity?}}`.
- `OpaAccessControl` has 77 distinct `check*`, `filter*` and `get*` methods; the operation names they send are the list the operation table must cover.
- The plugin fails closed on any error: transport errors, non-2xx responses, and undecodable bodies.

From the code at `origin/main` 06331547:

- Six infrastructure policy families, named in `egret-policy-api/src/infra_names.rs`:
  - `egret-admin - …` and `all - …`, one per service-def hierarchy, built in `egret-policy/src/default_policies.rs`;
  - `egret-delegator-impersonate`, `egret-user-impersonate-self`, `egret-user-execute-query` and `egret-query-timeout`, built in `egret-engine-mgmt/src/delegator_policy.rs`.
- Pure functions build the infrastructure policies. Humans cannot edit, disable or delete them (`tests/infra_guard.rs`).
- They are written when a profile is created, when an engine is created, and at boot (`seed_user_baselines_for_existing_engines`). The boot write bumps `policy_version` by 3 on every start, even when nothing changed.
- `egret_perm_ir::ep::Mechanism` has the single variant `Ranger`, hard-coded in `ir_load.rs`.

## 2a — infrastructure in the IR

> **As built (2026-11-04).** This section describes 2a as implemented by the SP2a plan
> (`docs/en/plans/2026-11-04-gov-sp2a-infra-in-ir.md`). It replaces the first draft, which
> put the `all - …` defaults in the IR, used `Target::WholeEp`/`Effect::AllAccess`/`EngineValue`,
> wrote through `apply_diff_in_tx`, and promised a byte-identical demo stack.

### Which rows the IR owns

- The IR owns exactly two name shapes:
  - the `egret-admin - …` rows, one per service-def hierarchy, Trino or StarRocks;
  - four fixed Trino names: `egret-delegator-impersonate`, `egret-user-impersonate-self`, `egret-user-execute-query` and `egret-query-timeout`.
- **The `all - …` defaults stay outside the IR.** Their route (`POST …/seed-defaults`), `seed_default_trino_policies` and `upsert_infra_policy` are unchanged. Convergence never reads, writes or deletes an `all - …` row.

### When each family is wanted

- A **qualifying engine** of a profile is bound to it, is not `origin = 'external'`, and delegates (`egret_core::engine_delegation::supports_delegation`). For Trino, that means `trinoConfig.delegationMode` is `impersonate` (the default when absent) or `trinoConfig.auth.jwt` is an object. For StarRocks, it means `starrocksConfig.delegation.mode` is `impersonate` or `jwt`.
- A **delegator-impersonating engine** is a qualifying Trino engine in `impersonate` mode whose `trinoConfig.auth` is an object.

| Family | Wanted when |
|---|---|
| `egret-admin - …` | ≥ 1 qualifying engine **and** ≥ 1 engine admin |
| `egret-user-impersonate-self`, `egret-user-execute-query`, `egret-query-timeout` | Trino profile with ≥ 1 qualifying engine |
| `egret-delegator-impersonate` | Trino profile with ≥ 1 delegator-impersonating engine |

### IR

- `Source::Infra { kind: InfraKind }`, where `InfraKind` is one of `AdminAllAccess`, `DelegatorImpersonate`, `SelfImpersonate`, `ExecuteQuery` or `QueryTimeout`.
- `Target::Infra(InfraObject)`, where `InfraObject` is one of:
  - `WholeEp`: every object the EP has (all-access);
  - `AnyUser` and `SessionUser`: the object of impersonation;
  - `AnyQuery`;
  - `SystemProperty(name)`.
- `SessionUser` is neutral. The Ranger target renders it as `{USER}`, and an evaluator compares it with the session user.
- The effect is `Effect::EnginePrivilege { capability }`, with `capability` one of `all`, `impersonate`, `execute` or `alter`. The audience is the admins, the delegator principal, or every user (`MemberName::Everyone`, which the Ranger target renders as `users: ["{USER}"]`, `groups: []`). It is not the Ranger `public` group.
- `egret_perm_ir::lower::infra::lower_infra(&InfraInputs) -> Vec<Statement>` is pure. `InfraInputs` holds:
  - the engine type;
  - the sorted, deduped engine admins;
  - `delegating`: whether ≥ 1 qualifying engine is bound;
  - `delegator_impersonates`.

### Ranger target

- `egret-perm-compile/src/ranger/infra.rs::render` renders each statement into `(name, policy JSON)` pairs. All-access expands over the service-def's hierarchies.
- The JSON body builders live in one place, `egret-policy-api/src/infra_bodies.rs`, plus `default_policies::admin_all_access_bodies`. They moved from `delegator_policy.rs` and `write_admin_all_access_in_tx`. They live in `egret-policy-api`, not in `ranger/`, because moving them there would create a crate cycle.

### Write path: convergence

- `egret_perm_compile::infra::converge_infra_in_tx(tx, service_id)` loads the inputs, lowers, renders, and reconciles by name:
  - it updates a row in place when the content changed, so the policy id is kept;
  - it inserts a missing row, in render order;
  - it deletes every IR-owned row that is not wanted.
- It bumps `policy_version` once, and only if something changed. Converging twice is a no-op. This removes the +3 bump on every boot.
- Convergence is its **own path**, separate from `apply_diff_in_tx`. That function's compiler-prefix reconciliation and `origin = 'generated'` stamp are unchanged. Keeping them separate means the kill switch does not depend on a full compile: disabling a platform admin removes them from `egret-admin` even when the profile's compile fails.
- Convergence runs under the locks its callers already take (profile row lock, then mint lock), with no new lock site. It runs:
  - at the recompile chokepoint, before the compile, for every profile, hand-written ones included;
  - on the kill switch (`set_user_status_and_recompile_in_tx`);
  - in the boot pass and `egret init` (`converge_infra_policies`);
  - at Trino and StarRocks engine apply, through `ProfileCompilerPort::converge_infra`;
  - after an engine delete.
- These old writers are deleted:
  - `seed_admin_all_access` and `write_admin_all_access_in_tx`;
  - `seed_delegator_policies`, `seed_user_baseline`, `seed_engine_user_baseline`;
  - the boot seeder `seed_user_baselines_for_existing_engines`;
  - `sync_admin_policy_in_tx`.

### Accepted differences (D5)

Convergence deletes rows that the old writers left behind:

- (a) `version`, `policyVersion` and `policyUpdateTime` move when a row changes;
- (b) a stale `egret-delegator-impersonate` row after an engine switches from impersonate to jwt;
- (c) every IR-owned row on a profile that no qualifying engine is bound to, for example after unbinding or deleting its last engine;
- (d) `egret-admin - …` rows for hierarchies the service-def no longer builds;
- (e) `egret-admin - …` rows when there are no engine admins.

### Proof

- **Byte-identical means:** every existing golden file under `crates/egret-perm-compile/tests/golden/` is unchanged. A demo-stack download before and after the upgrade is identical, ignoring D5 (a)–(e).
- New goldens:
  - `G22_infra_fresh_trino_and_starrocks` covers a fresh Trino profile and a fresh StarRocks profile, which get 13 + 4 rows and 13 rows;
  - `G23_infra_stale_rows_removed` covers a jwt engine losing the delegator row, and an unbound profile losing its baseline rows.
- Integration tests (`tests/infra_converge.rs`, `tests/admin_policy_sync.rs`) cover:
  - convergence idempotence;
  - the kill switch;
  - unbind and delete;
  - the chokepoint order: convergence runs before the compile reads the profile.
- Mutation checks show each of those tests fails when its rule is broken.
- The live check restarts egret twice. After the second restart the download is unchanged and `policy_version` has not moved.

## 2b — the OPA target

### Evaluator: crate `egret-perm-eval`

The crate is pure: no sqlx, tokio, network or filesystem, like `egret-ranger-eval`.

- **`Snapshot`** is one EP's lowered statement set: grants, rules, withholds, engine privileges and infrastructure. It also holds the resolved directory (users → groups) and the resolved column classifications. Statements are indexed by catalog, then schema, then table, so a question scans only the statements that can match it.
- **Questions:**
  - `decide(principal, op, resource) -> Decision { Allow | Deny { reason } }`;
  - `filter(principal, op, resources) -> Vec<usize>`;
  - `row_filters(principal, table) -> Vec<String>`;
  - `column_masks(principal, table, columns) -> BTreeMap<String, String>`.
- **Semantics:**
  - Deny wins (umbrella D6).
  - Row filters combine with AND, which is Trino's own semantics.
  - A column has at most one mask. Two masks on one column for one user are already refused at write time (`refuse_overlaps`); the evaluator treats one as an internal error and denies.
- **Operation table** (`trino_ops.rs`) lists every operation name the 483 plugin sends. Each one is exactly one of:
  - a **data** question (READ, WRITE or CREATE on a securable);
  - an **infrastructure** question (impersonate, execute, kill, session property);
  - **always allowed**, with the reason written beside the row, for example `FilterViewQueryOwnedBy` on one's own queries;
  - **always denied, with a reason**, for example GRANT, REVOKE and the role operations, since `opa.allow-permission-management-operations=false`.

  An operation missing from the table is denied with the reason "operation not modelled" (umbrella D7). A test asserts that the table covers every operation name in the 483 plugin.
- `explain` gains an IR path: for an OPA profile, `/perm/explain*` answers through `egret-perm-eval` and returns the evaluator's reason. A Ranger profile keeps `egret-ranger-eval`, which also stays the test oracle.

### Endpoint: `opa_surface()`, in `egret-perm-compile`, mounted by `egret-server`

| Route | Plugin setting |
|---|---|
| `POST /opa/v1/data/egret/trino/allow` | `opa.policy.uri` |
| `POST /opa/v1/data/egret/trino/batch` | `opa.policy.batched-uri` |
| `POST /opa/v1/data/egret/trino/row_filters` | `opa.policy.row-filters-uri` |
| `POST /opa/v1/data/egret/trino/batch_column_masks` | `opa.policy.batch-column-masking-uri` |
| `POST /opa/v1/data/egret/trino/column_mask` | `opa.policy.column-masking-uri` |

- **Configuration gate.** The surface, its snapshot refresher, and a switch to OPA exist only with `[governance.opa] enabled = true` (env `EGRET_GOVERNANCE__OPA__ENABLED`). It defaults to `false` until 2b-2 lands, whose last task makes `true` the default. Switching back to Ranger is always allowed. *(Amended 2026-11-04 at plan review.)*

- **Authentication.** `input.context.properties` carries `egret.engine` (the engine id) and `egret.secret`. egret checks the secret with the same constant-time engine-secret check the Ranger surface uses. It then reads which profile the engine is bound to, from a binding map the snapshot refresher reloads every second, never from a cached credential.
- **Refusal.** An unknown engine, a wrong secret, a malformed request, an engine bound to no OPA profile, or a profile with no usable snapshot is refused, and a log line names the reason. *(Amended 2026-11-04 at plan review.)*
  - On `allow` and `batch`, a refusal is HTTP 200 with a denying result (`false`, `[]`).
  - On the three value routes (`row_filters`, `batch_column_masks`, `column_mask`), a refusal is **HTTP 403**. There, `[]` or `null` means "no filter, no mask", so a 200 refusal would fail open. Only an evaluation against a ready snapshot answers 200.
  - An internal evaluator error answers 500 on any route. The plugin turns every non-2xx into a failed query.
- **Snapshot cache.** egret keeps one `Arc<Snapshot>` per service, keyed by `(policy_version, tag_version)`. When the version moves, one task rebuilds the snapshot while in-flight requests keep the old one.
  - The decision path never touches the database, except on a miss of a 5-second credential-verification cache.
  - Every rebuild re-checks the stored-state refusals listed under the switch below. A hit makes the profile deny everything, with the reason reported as a compile failure.
  - If only the data lowering fails, fresh infrastructure statements are spliced into the previous snapshot, so the 2a kill switch still holds.
  - A profile keeps being served while any of its engines was last applied with OPA. *(Amended 2026-11-04 at plan review.)*
- **Logging.** Every denial is logged at `info` with the engine, user, operation, resource and reason. Allows are counted in metrics.

### Per-profile switch

- **Migration `0213`.** `policy.authz_services` gains `mechanism TEXT NOT NULL DEFAULT 'ranger' CHECK (mechanism IN ('ranger','opa'))`, plus a CHECK that `opa` requires `type='trino'`. `engine_mgmt.engines` gains `applied_mechanism`, the mechanism each provisioned engine was last applied with.
- **IR.** `Mechanism::Opa` is added, and `ir_load.rs` reads the mechanism from the profile.
- **Matrix.** `opa(trino)` rows are filled from the differential's measurements. An absent row still reads `Missing(NotMeasured)`.
- **`PATCH /policies/services/{service}` with `{ "mechanism": "opa" | "ranger" }`.** `{service}` is the profile name, like every other profile route.
  - egret refuses the switch, with the failing condition named, unless all of these hold *(amended 2026-11-04 at plan review)*:
    - OPA is enabled on the installation (the configuration gate);
    - the profile is Trino;
    - the profile is generated (S6);
    - every bound **egret-provisioned** engine runs an image in `KNOWN_ENGINE_IMAGES` that ships `plugin/opa` (today: Trino 483). External engines are not checked, because egret cannot see their image. The response lists them, and the dialog and the docs say their operator must run Trino ≥ 483 with `plugin/opa`;
    - the profile holds no `all - …` default policy (decision D-all, below);
    - neither the profile nor its tag service holds a hand-written (`managed_by = 'operator'`) policy;
    - the profile serves no stored differing mask or row-filter pair (`GET /perm/overlaps`).

    The last three are stored state the evaluator cannot express. While a profile is on OPA, a hand-written policy in its tag service is refused at write time.
  - On success egret recompiles, bumps the version, and reconfigures every bound provisioned engine. If an engine cannot be re-applied, the answer is 502, naming the engines applied and the engines that failed; the switch stays committed. Re-sending the same request re-runs the reconfigure.

### Engine configuration

- **`opa_plugin_files(engine, egret_url, secret)`** sits next to `ranger_plugin_files`. It writes two files:
  - `access-control.properties`:

    ```properties
    access-control.name=opa
    opa.policy.uri=<egret>/opa/v1/data/egret/trino/allow
    opa.policy.batched-uri=<egret>/opa/v1/data/egret/trino/batch
    opa.policy.row-filters-uri=<egret>/opa/v1/data/egret/trino/row_filters
    opa.policy.batch-column-masking-uri=<egret>/opa/v1/data/egret/trino/batch_column_masks
    opa.policy.column-masking-uri=<egret>/opa/v1/data/egret/trino/column_mask
    opa.allow-permission-management-operations=false
    opa.log-requests=false
    opa.log-responses=false
    opa.context-file=/etc/trino/egret-opa-context.properties
    ```

  - `egret-opa-context.properties`, with `egret.engine` and `egret.secret`.
- **Kubernetes provisioner** (`egret-provisioner/src/trino.rs`): the ConfigMap carries the mechanism's `access-control.properties`. The context file goes in the Secret that holds the Ranger XML today. A change of mechanism rolls the coordinator out, because Trino reads access control only at start.
- **Docker provisioner** (`docker/plan.rs`): the same files, and the coordinator restarts.
- **External engines**: `GET /engines/{id}/opa-plugin-config` sits beside `/ranger-plugin-config`. `TrinoConfigDialog.tsx` shows the OPA variant and warns not to enable `opa.log-requests`, which would log the secret.
- **The rollout fails queries running on the coordinator.** The switch dialog says so before the admin confirms.

### Features that go through infrastructure

Each of these is answered by an infrastructure statement from 2a, and each is covered by the differential:

- delegation (Jwt, Impersonate, Open): `ImpersonateUser`;
- the monitor's kill: `KillQueryOwnedBy` and `ExecuteQuery`;
- `query_max_run_time`: `SetSystemSessionProperty`.

The `all - …` defaults are not in the IR (2a), so the OPA evaluator does not see them. **Decided (D-all, 2026-11-04): the switch is refused while the profile holds any of them, with the condition named.** They are not lowered. This is reachable: only the manual seed-defaults route writes them, and it refuses a generated profile.

### UI and docs

- The profile page gets a "Ranger | OPA" mechanism selector, which shows the refusal reason when the switch is not allowed.
- egret-data/docs (Platform) gets a "Trino on OPA" page: the switch, what the rollout does, external-engine setup, the secret warning, and the known differences from `explained.toml`.

## Errors

- **egret does not answer** (down, restarting, overloaded): Trino denies every check, so queries fail closed (umbrella D7). The length of this window is measured (see Latency) and documented.
- **Snapshot build fails** (for example, a lowering error): egret keeps serving the previous snapshot and reports the failure on the profile, as a compile failure is reported today. If there is no previous snapshot, it denies with the reason.
- **Unknown operation or malformed input**: deny, with the reason logged.

## Verification

### Layer 1 — decision-level differential (CI job `opa-differential`)

- **Corpus.** A seeded generator produces store states (grants, rules, withholds, classifications, engine grants, infrastructure). For each state it also produces questions in Trino operation terms: `(user, operation, resource)`. There are 2000 cases, the size of the existing Ranger corpus.
- **Ranger path.** Compile to Ranger JSON. Translate each question into the Ranger access request the Trino 483 `ranger` plugin would make. Ask real Ranger through `EgretCaseRunner`.
- **OPA path.** Compile to a `Snapshot` and ask `egret-perm-eval`.
- **Compared:** allow or deny, the row filters, and the masks.
- **`tests/opa_differential/explained.toml`** names every accepted difference: an id, the mechanism behind it, which side is right, and why. A difference with no entry fails CI. So does an entry that no case produces any more, which keeps the list from rotting.
- **Known risk.** The operation-to-Ranger-request translation is itself an oracle and could be wrong in the same way as the evaluator (a correlated oracle). Layer 2 exists because of this.

### Layer 2 — end-to-end differential

- **Setup.** Two Trino 483 coordinators run on the same data. Both are bound to the same profile, one through Ranger and one through OPA; S8 makes this possible.
- **Scenarios.** The sub-project 1 golden scenarios, run as SQL × user.
- **Compared:**
  - `SELECT` results: a row-set hash, plus per-column values so that masks show;
  - the error class, refused or not;
  - `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES` and `SHOW COLUMNS`;
  - `information_schema`;
  - Iceberg `$files`;
  - impersonation, kill, and `query_max_run_time`.
- **Where it runs.** A script on its own compose project, never the shared demo stack. It runs in CI on PRs that touch `crates/egret-perm-*`. It uses the same `explained.toml`.

### Latency (measured; no threshold in this sub-project)

- **Microbenchmarks** (criterion): `decide`, `filter` and `column_masks` on snapshots of 1k and 10k statements.
- **End to end, Ranger against OPA on the same stack:** planning time and p50/p99 on metadata-heavy queries.
  - The queries: `SHOW TABLES` over 1000 tables, `SELECT *` on a 500-column table, and `information_schema.columns` (Trino #25628).
  - The number of OPA requests per query, with and without the batched URI.
- **egret restart:** how many seconds Trino queries fail.
- **Report:** `docs/en/reports/` (dated). The numbers decide whether a sidecar package (S2) is needed.

### Other tests

- The operation table covers every operation name the 483 plugin sends, extracted from the jar.
- Mutation tests on the evaluator's deny-wins, row-filter AND, mask selection and infrastructure rules.
- Endpoint authentication is tested in both directions, allow and refuse, on every route: right secret, wrong secret, unknown engine, Ranger profile.
- Switch refusals are tested, one per condition.
- Live check: on its own stack, an unprivileged user queries Trino on OPA and sees the mask, the row filter and the refusal.

## Out of scope

- OPA for StarRocks (upstream StarRocks #77386).
- An OPA sidecar or bundle server, unless the latency numbers call for one in a later package.
- Replacing profiles with EPs, removing hand-written policies, and groups-only clearance (sub-project 3).
- Service accounts (sub-project 4).
- `iceberg-rest` (sub-project 5).
