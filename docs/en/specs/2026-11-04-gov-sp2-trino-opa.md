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
| S2 | The decision endpoint runs inside `egret-server`, next to `ranger_surface()`. The evaluator is its own pure crate, so it can move into a sidecar later. | Simplest delivery. Whether a sidecar is needed is decided by the latency and restart numbers this sub-project measures, not in advance. |
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

### IR

- `Source::Infra { kind: InfraKind }`, where `InfraKind` is one of `AdminAllAccess`, `DefaultAllAccess`, `DelegatorImpersonate`, `SelfImpersonate`, `ExecuteQuery` or `QueryTimeout`.
- All-access is one neutral statement: "this audience holds every privilege on the whole EP". It gets a new target, `Target::WholeEp`, and a new effect, `Effect::AllAccess`.
- The engine-level families reuse `Target::EngineObject` and `Effect::EnginePrivilege`, the same shapes engine grants use. Their capabilities are `trinouser`/`impersonate`, `queryid`/`execute`, and `systemproperty`/`alter`.
- "The session user themself" is a neutral value, `EngineValue::SessionUser`, not Ranger's `{USER}` macro. The Ranger target renders it as `{USER}`; the evaluator compares it with the session user.
- `lower_infra(&InfraInputs) -> Lowered` is pure, like the four lowerings of sub-project 1. `InfraInputs` holds:
  - the profile's type, name and admin principal;
  - for each bound engine, its delegator username and delegation mode.

### Ranger target

- `egret-perm-compile/src/ranger/infra.rs` renders the infrastructure statements. It owns the expansion of all-access over the service-def's hierarchies.
- The JSON builders move here from `default_policies.rs` and `delegator_policy.rs`, and those two files lose them. The hierarchy walk stays in `egret-ranger-eval::servicedef`, which has one walker.

### Write path

- The compiler reconciles a profile's `managed_by='infra'` rows **by name**:
  - it updates a row in place when the content changed;
  - it inserts a missing row;
  - it deletes a row that is no longer desired.
- Policy ids survive this, so the download artifact stays byte-identical.
- The infrastructure part compiles for **every** profile, hand-written ones included. The rest of a hand-written profile stays uncompiled, as today.
- Today's writers ask for a recompile instead of writing rows. The writers are profile creation, engine creation, and the boot seeder.
- A recompile whose result equals the stored rows writes nothing and does not bump `policy_version`. This removes the +3 bump on every boot.

### Proof

- The golden fixtures stay byte-identical (22 of 22).
- A new golden scenario covers create profile → create engine → restart egret → restart again. It asserts the policy ids, the content, and that `policy_version` does not move on the restarts.
- Unit and shape tests cover `lower_infra`. Mutation tests show each test fails when its lowering rule is broken.

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

### Endpoint: `opa_surface()` in `egret-server`

| Route | Plugin setting |
|---|---|
| `POST /opa/v1/data/egret/trino/allow` | `opa.policy.uri` |
| `POST /opa/v1/data/egret/trino/batch` | `opa.policy.batched-uri` |
| `POST /opa/v1/data/egret/trino/row_filters` | `opa.policy.row-filters-uri` |
| `POST /opa/v1/data/egret/trino/batch_column_masks` | `opa.policy.batch-column-masking-uri` |

- **Authentication.** `input.context.properties` carries `egret.engine` (the engine id) and `egret.secret`. egret checks the secret with the same constant-time engine-secret check the Ranger surface uses. It then reads which profile the engine is bound to.
- **Refusal.** An unknown engine, a wrong secret, or a profile that is not on OPA all get HTTP 200 with a denying result (`false`, `[]`), plus a log line that names the reason. The plugin would turn a 4xx into the same denial but without a reason.
- **Snapshot cache.** egret keeps one `Arc<Snapshot>` per service, keyed by `(policy_version, tag_version)`. When the version moves, one task rebuilds the snapshot while in-flight requests keep the old one. The decision path never touches the database.
- **Logging.** Every denial is logged at `info` with the engine, user, operation, resource and reason. Allows are counted in metrics.

### Per-profile switch

- **Migration.** `policy.authz_services` gains `mechanism TEXT NOT NULL DEFAULT 'ranger' CHECK (mechanism IN ('ranger','opa'))`, plus a CHECK that `opa` requires `type='trino'`. The migration number is chosen at plan time, above `main` and every open PR.
- **IR.** `Mechanism::Opa` is added, and `ir_load.rs` reads the mechanism from the profile.
- **Matrix.** `opa(trino)` rows are filled from the differential's measurements. An absent row still reads `Missing(NotMeasured)`.
- **`PATCH /policies/services/{id}` with `{ "mechanism": "opa" | "ranger" }`.**
  - egret refuses the switch, with the failing condition named, unless all of these hold:
    - the profile is Trino;
    - the profile is generated (S6);
    - every bound engine runs an image in `KNOWN_ENGINE_IMAGES` that ships `plugin/opa` (today: Trino 483).
  - On success egret recompiles, bumps the version, and reconfigures every bound engine.

### Engine configuration

- **`opa_plugin_files(engine, egret_url, secret)`** sits next to `ranger_plugin_files`. It writes two files:
  - `access-control.properties`:

    ```properties
    access-control.name=opa
    opa.policy.uri=<egret>/opa/v1/data/egret/trino/allow
    opa.policy.batched-uri=<egret>/opa/v1/data/egret/trino/batch
    opa.policy.row-filters-uri=<egret>/opa/v1/data/egret/trino/row_filters
    opa.policy.batch-column-masking-uri=<egret>/opa/v1/data/egret/trino/batch_column_masks
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
