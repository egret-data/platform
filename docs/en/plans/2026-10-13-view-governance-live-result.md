# View governance: live result

> **Measured:** 2026-10-14 (UTC), branch `feat/view-governance`, base `19d7ac2db`, worktree `egret-viewgov`.
> Canonical (English). Section 1 is Task 5 (measure before building Part B). Section 2 is added by Task 11.

## 1. Measurements before building Part B (Task 5)

**Result: BLOCKED on one STOP gate (Q3).** EXPLAIN needs `select` on every column the view's SQL reads, so the
`egret-lineage` principal needs `select` on all governed data. The two ways to give it `select` without data, a
`1 = 0` row filter and a `MASK_NULL` mask, both corrupt the lineage. Q1, Q2 and Q4 passed their gates. Q1 chose
the axum layer, and Q4's `pluginId` maps to an engine through the service name. Each answer has measured
consequences for Tasks 6 to 8, and these are listed per question.

### 1.0 Stack and versions proven by output

| piece | what ran |
|---|---|
| egret | `cargo build -p egret-bin --bin egret` from this worktree, with two scratch request loggers (see "Instrument" below), copied to the scratchpad and run as `egret serve` on **:8096**. DB **`egret_viewgov_live`** was created in `egret-dev-postgres-1`, then `egret migrate` and `egret init` ran against it. Env: `EGRET_RANGER__ENABLED=true`, `EGRET_CATALOG__ENABLED=true`, `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8096`, storage `http://localhost:19000` bucket `warehouse`, and `EGRET_OIDC__ENABLED=false`. Log: `kubernetes unavailable; engine provisioning disabled` (expected), `egret listening addr=0.0.0.0:8096`. |
| object store | `vg-rustfs`: `rustfs/rustfs:1.0.1` on network `vg-net`, host port 19000. Bucket `warehouse`, created with `amazon/aws-cli:2.37.9`. |
| catalog | Through the API: storage connection `vg-rustfs`, then warehouse `vgwh` (`lakekeeperWarehouseId 3b131b18-c778-11f1-a80e-f3fe7a2ed2cd`), then role `vg-engine` (`catalog` / `write`) and engine client `egret_e327d0ff270cac83`. |
| Trino | `vg-trino`: `trinodb/trino@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` (the `egret_perm::engine_images` pin) on host port 18096. `select version()` returned `"483"`. Catalog `ice` is Iceberg REST on egret: `iceberg.rest-catalog.uri=http://host.docker.internal:8096/catalog`, `warehouse=vgwh`, `security=OAUTH2` with the engine client, and `s3.endpoint=http://vg-rustfs:9000`. The Ranger files are verbatim from `GET /engines/{id}/ranger-plugin-config`, except DEV-1. |
| StarRocks | `vg-sr`: `starrocks/allin1-ubuntu@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d`. `select current_version()` returned `4.1.6-6862092`. The Ranger files are as rendered: `access_control = ranger` was appended to `fe/conf/fe.conf`, the two XMLs were copied into `fe/conf`, and the container was restarted. |
| registration | Profiles `vg-tag` (trino), `vg-trino` (trino, `tagService=vg-tag`) and `vg-sr` (starrocks). External engines `vg-trino` (2.8.0) and `vg-sr` (2.9.0) both answered `rangerPluginVersionModelled: true`. |
| policies | All went through `POST /api/control-plane/policies`. The baseline `trinouser:*` `impersonate` and `queryid:*` `execute` are for group `public`. `vgadmin` has `all` at catalog, schema and column level. The `egret-lineage` policies are listed in 1.3. |

**Instrument (scratch, not committed; both files were restored from byte copies and `git status` is clean).**
- In `egret-catalog-engine/src/mount.rs::guard_then_inject_metadata`, behind `EGRET_VG_REQLOG`, a logger writes
  every catalog request: `VGCAT-REQ <method> <path> query=… body=…` and `VGCAT-RESP … -> <status>`.
- In the same function, a hold (`EGRET_VG_HOLD_MS`) and a veto (`EGRET_VG_VETO`) apply to `POST …/views*`.
- In `egret-server/src/lib.rs`, a layer on the `/service` (Ranger) router logs
  `VGRANGER <method> <uri> -> <status> headers=…`.

**Deviations.**
- **DEV-1 (config):** poll interval 5000 ms instead of the rendered 30000 ms, on both engines.
- **DEV-2 (config):** Trino's `ice` catalog uses static S3 keys with `vended-credentials-enabled=false`. This is
  not the renderer's vended credentials. Storage auth is outside every question here.

### 1.1 Q1: which hook sees a view change before it is committed?

**Answer: the axum layer around the mounted router (`mount.rs`). lakekeeper's contract verification cannot be the
hook.**

**lakekeeper `contract_verification`** was read at the pinned rev `791650571`
(`~/.cargo/git/checkouts/lakekeeper-90676c980e2118c5/7916505/crates/lakekeeper/src/service/contract_verification.rs`
and its callers).
- **It does not run for view create.** `check_view_updates` has exactly one caller,
  `server/views/commit.rs:216`. `server/views/create.rs` calls no verifier: it writes the row in a transaction,
  writes the metadata file, then `t.commit()`.
- **For a replace or commit, it runs before the commit and can veto.** It runs inside `try_commit_view`, after
  `C::Transaction::begin_write` and `load_view`, and before `build_new_metadata`, `C::commit_view` and the
  metadata write. `Violation { error_model }` becomes `Err` through `into_result()?`.
- Holding there means holding an open Postgres write transaction, inside a retry loop
  (`MAX_RETRIES_ON_CONCURRENT_UPDATE`), for up to 60 s.
- **What it sees:** the `ViewUpdate` list (an `add-view-version` carries the SQL representation,
  `default-namespace` and `default-catalog` when present) and the previous `ViewMetadata`. It does not see the
  warehouse or the caller.
- **egret wires none:** `mount.rs` builds `contract_verifiers: ContractVerifiers::default()`.
- The other lakekeeper extension points do not qualify either. The `Authorizer` sees the action but not the SQL.
  The `EventDispatcher` `emit_view_committed_async` runs after the commit and cannot veto.

**Requests Trino 483 sends** (egret request log, warehouse prefix shortened to `{wh}`):

| statement | request that carries the change | what it carries |
|---|---|---|
| `CREATE VIEW ice.s.v1 AS SELECT id, name FROM ice.s.pii` | `POST /{wh}/namespaces/s/views -> 200` | `"view-version":{…,"default-namespace":["s"],"representations":[{"type":"sql","sql":"SELECT\n  id\n, name\nFROM\n  ice.s.pii\n","dialect":"trino"}]},"schema":{…},"properties":{"trino.run-as-owner":"vgadmin"}` |
| `CREATE OR REPLACE VIEW ice.s.v1 AS SELECT id, name, ssn FROM ice.s.pii` | `POST /{wh}/namespaces/s/views/v1 -> 200` (view commit) | `"requirements":[{"type":"assert-view-uuid",…}],"updates":[{"action":"set-properties","updates":{"trino.run-as-owner":"vgadmin"}},{"action":"add-schema",…},{"action":"add-view-version","view-version":{…,"default-namespace":["s"],"representations":[{…"sql":"SELECT\n  id\n, name\n, ssn\nFROM\n  ice.s.pii\n"…}]}},{"action":"set-current-view-version","view-version-id":-1}]` |
| `ALTER VIEW ice.s.v1 AS SELECT …` | none | **Not in Trino 483's grammar:** `line 1:21: mismatched input 'as'. Expecting: '.', 'REFRESH', 'RENAME', 'SET'` |
| `ALTER VIEW … SET AUTHORIZATION vgadmin` | none | `setViewPrincipal is not supported for Iceberg REST catalog` |
| `COMMENT ON VIEW ice.s.v1 IS 'hello'` | `POST /{wh}/namespaces/s/views/v1 -> 200` | `"updates":[{"action":"set-properties","updates":{"comment":"hello"}}]` (no SQL) |
| `COMMENT ON COLUMN ice.s.v1.id IS 'idc'` | `POST /{wh}/namespaces/s/views/v1 -> 200` | `add-schema` (with `"doc":"idc"`) plus an `add-view-version` repeating the **same SQL** |
| `ALTER VIEW ice.s.vtmp RENAME TO ice.s.vtmp2` | `POST /{wh}/views/rename -> 204` | `{"source":{"namespace":["s"],"name":"vtmp"},"destination":{"namespace":["s"],"name":"vtmp2"}}` |
| `DROP VIEW ice.s.vtmp2` | `DELETE /{wh}/namespaces/s/views/vtmp2 -> 204` | no body |

Before each create, Trino reads `GET …/views/<name>` (404), `GET …/tables/<name>` (404), each referenced
relation (`GET …/views/pii` 404, then `GET …/tables/pii?snapshots=all` 200), and `GET …/namespaces/s`. These
reads are not changes.

**Hold and veto through the layer.**
- **Hold.** With `EGRET_VG_HOLD_MS=65000` (more than the spec's 60 s limit), `CREATE VIEW ice.s.v_hold` took
  65 s and succeeded. There was one request and no client retry or timeout: `VGCAT-HOLD start 65000 ms` at
  `02:52:57.609`, `VGCAT-HOLD end` at `02:54:02.610`, `VGCAT-RESP POST …/views -> 200 OK` at `02:54:02.778`.
  Trino then read the view.
- **Veto.** With `EGRET_VG_VETO=1`, the layer answers 409 with
  `{"error":{"message":"…","type":"CommitFailedException","code":409}}`. Trino prints the message verbatim, and
  in each case it sent one POST, with no retry.
  - Create: `ERROR: "egret: view refused by governance hold (scratch veto)"`, and
    `select * from ice.s.v_veto` answered `Table 'ice.s.v_veto' does not exist`.
  - Replace: `ERROR: "Commit failed: egret: view refused by governance hold (scratch veto)"`.
    `SHOW CREATE VIEW` still shows the old body.

**Chosen hook: the axum layer in `mount.rs`, on `POST /{prefix}/namespaces/{ns}/views` (create) and
`POST /{prefix}/namespaces/{ns}/views/{view}` (commit).** It sees create and replace before the commit, can hold
for 65 s, and can veto with a message Trino shows. The third case, `ALTER VIEW … AS`, does not exist in Trino 483
(parse error above), so it needs no hook.

Facts Tasks 7 and 8 need:
- **A commit can carry no SQL.** `COMMENT ON VIEW` sends only `set-properties`. A `CREATE OR REPLACE` with the
  same SQL and schema sends `"updates":[]` (1.2). The hook must then judge the view's current SQL, or skip the
  hold when the update carries no `add-view-version`.
- **The layer runs before lakekeeper's own checks.** A request that lakekeeper then refuses (for example an
  `assert-view-uuid` mismatch, or a create of a name that exists) would already have written tags. Task 7 must
  roll back on a non-2xx response from `next.run`, not only on its own refusals.
- **Rename and drop also reach the layer** (`POST /{prefix}/views/rename`, `DELETE …/views/{view}`). These are
  where B3's lineage and tag cleanup and its renaming belong.

### 1.2 Q2: is the security mode stored?

**Answer: yes, as a property: `trino.run-as-owner = <owner>` means DEFINER, and its absence means INVOKER. But
`load_view_detail` drops it. And Trino 483 never removes it, so a view that was ever DEFINER stays DEFINER.**

- **`egret_catalog_engine::load_view_detail`** (through `GET /api/control-plane/warehouses/{id}/namespaces/s/views/{v}`,
  whose `view_columns` calls it) returns columns only, for both views. The mode does not appear:
  - `v_def` (DEFINER): `[{"name":"id","type":"int","required":true},{"name":"ssn","type":"string","required":true}]`
  - `v_inv` (INVOKER): `[{"name":"id",…},{"name":"ssn",…},{"name":"amount","type":"double","required":true}]`
- **The same lakekeeper `load_view` that it wraps** (raw `GET /catalog/v1/{wh}/namespaces/s/views/{v}`) carries
  the mode:
  - `v_def`: `properties= {"trino.run-as-owner": "vgadmin"}`
  - `v_inv`: `properties= {}`
  - The create request bodies agree. DEFINER sends `"properties":{"trino.run-as-owner":"vgadmin"}` and INVOKER
    sends no `properties` key.
- **`default-catalog` is stored only when the creating session had a catalog.**
  - `v_unq` was created with session `ice.s` and unqualified SQL. It stores `"default-catalog":"ice","default-namespace":["s"]`
    and the SQL `SELECT\n  id\n, ssn\nFROM\n  pii\n`.
  - Views created with fully qualified SQL and no session catalog store only `default-namespace`.
  - `default-catalog` is the **engine's** catalog name (`ice`), not an egret identifier. When it is absent, the
    lineage EXPLAIN must use the Trino catalog that the engine binds to this warehouse.
- **Defect (Trino 483 with Iceberg REST, measured):** `CREATE OR REPLACE … SECURITY INVOKER` over a DEFINER view
  never clears the mode.
  - Same SQL: Trino sent `"updates":[]`. The properties stayed `{'trino.run-as-owner': 'vgadmin'}`, and
    `SHOW CREATE VIEW` still printed `SECURITY DEFINER`.
  - Changed SQL: Trino sent `add-schema` and `add-view-version`, with no `remove-properties`. The properties were
    still `{'trino.run-as-owner': 'vgadmin'}`, and `SHOW CREATE VIEW` still printed `SECURITY DEFINER`.
  - The other direction works. INVOKER to DEFINER sends
    `{"action":"set-properties","updates":{"trino.run-as-owner":"vgadmin"}}`.
  - So the only way to get INVOKER is `DROP VIEW` and then `CREATE`. B5's guide must say so, and B5 should read
    the property rather than trust what the author typed.

For Task 10: read `metadata.properties["trino.run-as-owner"]` through a loader that keeps the properties. This
needs a new or extended function, because `load_view_detail` returns `Vec<ColumnInfo>`. The rule is: present
means `definer` with that owner, absent means `invoker`. A view not written by Trino (no
`summary.engine-name = trino`) should show `unknown`.

### 1.3 Q3: what does EXPLAIN need? **STOP gate hit.**

Principal `egret-lineage`. The baseline `impersonate` and `execute` policies apply to it through group `public`,
because Trino 483 needs them for every user. The statement was `EXPLAIN (TYPE IO, FORMAT JSON) SELECT id, ssn FROM ice.s.pii`.
Grants were added one at a time, and each result was taken after `Switched policy engine to [N]`:

| step | grants to `egret-lineage` | result |
|---|---|---|
| 0 | none | `Access Denied: Cannot access catalog ice` |
| 1 | `catalog=ice`: `use` | `Access Denied: Cannot select from columns [id, ssn] in table or view pii` |
| 2 | plus `ice.*`: `use, show`; `ice.*.*.*`: `show, use, alter, create, read_sysinfo` (all non-data accesses) | still `Cannot select from columns [id, ssn] in table or view pii` |
| 3 | plus `ice.s.pii.id`: `select` | still `Cannot select from columns [id, ssn] …` (the whole referenced set is named). `EXPLAIN … SELECT id FROM ice.s.pii` now succeeds |
| 4 | plus `ice.s.pii.ssn`: `select` | **OK**, JSON below |
| 5 | step 2's metadata policies deleted again | still **OK**: the minimum is catalog `use` plus column `select` |

**Minimum: `use` on the catalog and `select` on every column the SQL references.** No metadata-only grant
suffices. With step 4's grants the principal reads data: `select id, ssn from ice.s.pii` as `egret-lineage`
returned `[[2, "ssn-2"], [1, "ssn-1"]]`.

Through views, the requirement follows Trino's security modes. For `v_nest_inv` (INVOKER) over `v_inv` (DEFINER,
owner `vgadmin`) over a join of `pii` and `orders`:
- EXPLAIN first needed `select` on `v_nest_inv` (`Cannot select from columns [amount, id] in table or view v_nest_inv`).
- Then it needed `select` on `v_inv` (`… in table or view v_inv`).
- It did **not** need anything on `orders`, because the DEFINER layer is checked as its owner.
- The view's own body, which is what the hook must EXPLAIN at create time (the view does not exist yet), needs
  `select` on the base columns: `Cannot select from columns [amount, cust_id] in table or view orders` until
  `orders` was granted.

So for arbitrary views, `egret-lineage` needs `select` on every column of every governed table and view. This is
the gate's "broad `select` on data".

**Both data-free alternatives break the lineage** (measured, to inform the decision, not as a workaround):
- **Row filter `1 = 0`** on `ice.*.*` for `egret-lineage`: `select` returned `[]`, and the join's
  `inputTableColumnInfos` became `[]`.
- **`MASK_NULL`** on `ice.*.*.*` for `egret-lineage`: `select` returned `[[None, None], [None, None]]`. The join
  then reported only `[{"catalog":"ice","schemaTable":{"schema":"s","table":"orders"}}]`, so `pii` was lost:
  the masked join keys are NULL and the optimizer pruned that side.

**JSON shape** (TYPE IO, FORMAT JSON). Each input is `inputTableColumnInfos[].table = {catalog, schemaTable:{schema, table}}`:
- **Join** of `ice.s.pii` and `ice.s.orders`:

  ```json
  {"inputTableColumnInfos":[
    {"table":{"catalog":"ice","schemaTable":{"schema":"s","table":"orders"}},"constraint":{"none":false,"columnConstraints":[]},"estimate":{…}},
    {"table":{"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}},"constraint":{"none":false,"columnConstraints":[]},"estimate":{…}}],
   "estimate":{…}}
  ```

- **Nested view** `SELECT id, amount FROM ice.s.v_nest_inv` (INVOKER over DEFINER over the join): inputs
  `[{"catalog":"ice","schemaTable":{"schema":"s","table":"orders"}},{"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}}]`.
  Only base tables appear, never the views. Top-level keys: `inputTableColumnInfos`, `estimate`.
- **Unqualified SQL** with session `ice`/`s`: `SELECT id, ssn FROM pii` resolves to
  `[{"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}}]`.

**Second finding, separate from the gate: EXPLAIN reports inputs after optimization, so a table pruned by a
planning-time constant disappears.**

| SQL | inputs |
|---|---|
| `SELECT id, ssn FROM ice.s.pii WHERE false` | `[]` |
| `SELECT id, ssn FROM ice.s.pii LIMIT 0` | `[]` |
| `SELECT id, ssn FROM ice.s.pii WHERE current_date > DATE '2099-01-01'` | `[]` |
| `SELECT id, ssn FROM ice.s.pii WHERE random() > 2` | `[pii]` |

`EXPLAIN (TYPE LOGICAL, FORMAT JSON)` and `(TYPE VALIDATE)` of the `current_date` case do not mention `pii`
either. A view whose filter is `current_date > DATE '<tomorrow>'` therefore gets an empty lineage today, is
committed untagged, and returns `ssn` tomorrow. B1 as written ("lineage = what Trino reports it reads") has this
hole even with the broad grant.

**Design question for the user (why this is BLOCKED):**
- Accept a lineage principal with `select` on all governed data, which is contained only by egret's
  EXPLAIN-only client.
- Or take lineage from somewhere that needs no data grant. Trino's own analysis reports referenced tables before
  optimization; for example, an event listener's `QueryCompletedEvent` `metadata.tables` with
  `directlyReferenced`. But it arrives after the commit, which conflicts with B2's hold. This option is
  unmeasured.
- Or change R3.

Separately, decide how an empty or pruned lineage is treated.

### 1.4 Q4: the `pluginId` format and how it maps to an engine

**Answer: `pluginId = <appId>@<hostname>-<serviceName>`. It maps to an engine through `<serviceName>`, which is
also the path segment. The `<hostname>` is the process's own host name, not the host egret registered.**

Quoted request lines (`VGRANGER`, from egret's request log):

```text
Trino 483 policies : GET /service/plugins/policies/download/vg-trino?supportsPolicyDeltas=false&pluginId=trino@e00d4f5be6a4-vg-trino&clusterName=&lastActivationTime=0&pluginCapabilities=7ffff&lastKnownVersion=-1 -> 200 OK headers={"accept": "application/json", "content-type": "application/json", "user-agent": "Java/25.0.3", "host": "host.docker.internal:8096", "connection": "keep-alive"}
Trino 483 tags     : GET /service/tags/download/vg-trino?pluginId=trino@e00d4f5be6a4-vg-trino&lastActivationTime=0&pluginCapabilities=7ffff&lastKnownVersion=-1&supportsTagDeltas=false -> 200 OK
StarRocks 4.1.6    : GET /service/plugins/policies/download/vg-sr?supportsPolicyDeltas=false&pluginId=starrocks%40ee17c4dc2634-vg-sr&clusterName=&lastActivationTime=0&pluginCapabilities=7ffff&lastKnownVersion=-1 -> 200 OK
```

- **Host part.** `e00d4f5be6a4` and `ee17c4dc2634` are the container IDs: `docker exec vg-sr hostname` printed
  `ee17c4dc2634`, and `docker run` printed `e00d4f5be6a4…` for Trino. The engines were registered with host
  `vg-trino` and `vg-sr`, so **the host does not match the egret engine's `host`**. On Kubernetes the host name
  is the pod name.
- **Service part.** The service part equals the profile name in the path. A profile binds to at most one engine
  ("no other engine may bind it"), so **engine = the engine bound to `<serviceName>`**, and the distinct host
  parts are that engine's plugin instances.
- **Parsing.** Strip `<appId>@` and the suffix `-<path service>`; service names contain `-`. StarRocks
  URL-encodes `@` (`%40`), which axum's `Query` decodes.
- **Tags download path.** Trino polls the tags download under its **resource** profile (`/service/tags/download/vg-trino`),
  not under the tag service `vg-tag`. B4's record is therefore keyed by the resource service.
- **StarRocks has no tag download.** It never called `/service/tags/download/vg-sr` in the whole run (count 0).
  This is consistent with its service-def having no tag enricher.
- **Activation evidence.**
  - Each poll carries `lastKnownVersion` (the version the plugin has activated) and `lastActivationTime`.
  - After `vg-sr` moved to version 2: `…lastActivationTime=1791946206852…&lastKnownVersion=1 -> 200` at
    `02:51:16.886`, the FE logged `Switched policy engine to [2]` at `02:51:16.949`, and the next poll was
    `…lastActivationTime=1791946276977…&lastKnownVersion=2 -> 304`.
  - So "the plugin holds ≥ V" is proven by the **next** poll's `lastKnownVersion`, not by the version served. A
    served download can still fail to activate.
- **Two StarRocks instances, re-measured.**
  - The FE logged `Start Ranger plugin (starrocks - vg-sr) success` a second time, from thread
    `global-state-checkpoint-worker|117` at `02:49:48`.
  - That instance logged no `Created PolicyRefresher` and **never polled**: only one `pluginId` was seen for
    `vg-sr`, at one poll per 5 s.
  - Both instances would carry the same `pluginId` (same appId, host and service), so B4 could not tell them
    apart by id anyway.
  - The checkpoint copy does not appear to serve queries. The earlier "two refresher threads" observation
    (engine-upgrade §2.2) was not reproduced in this run.
- **Trino workers.** Trino loaded `/etc/trino/access-control.properties` here without
  `access-control.config-files`. The provisioner mounts that file on every pod but sets `config-files` only on
  the coordinator, so workers probably also load Ranger and poll, each with its own pod-name `pluginId`. This was
  not measured (no Kubernetes here).
- **Spoofing.** The download is anonymous, so anyone who can reach it can send a `pluginId` and a
  `lastKnownVersion`. A forged poll could satisfy B4's confirmation.

Gate: **not hit.** `pluginId` maps to an engine through the service name.

### 1.5 Left behind

- **Stopped:** egret (`:8096`), stopped by its recorded PID. Containers `vg-trino`, `vg-sr`, `vg-rustfs` and
  network `vg-net` were removed.
- **Restored:** the two scratch-instrumented sources (`git status` clean).
  `target/debug/egret` is the scratch-instrumented build until the next `cargo build`.
- **Kept:** database `egret_viewgov_live` in `egret-dev-postgres-1`, for Task 11. It holds:
  - the profiles `vg-tag`, `vg-trino` and `vg-sr`, and the two external engines;
  - the policies above;
  - the warehouse `vgwh`, whose catalog rows point at the removed RustFS.

  Task 11 can drop and recreate it.
- **Never touched:** DB `egret`, `egret-dev-*` (beyond creating the DB), `egret-w7d-postgres`,
  `egret-s4-postgres`, and ports 5173, 5000 and 8080.

## 2. The live re-run on Trino 483 and StarRocks 4.1.6 (Task 11)

### 2.0 Predictions (committed before anything was observed)

Written from the spec, the guide, the Task 4–10 reports and the earlier measurements, before any engine of this
run was started. A wrong prediction is a finding; nothing here is edited after observing.

**Setup the predictions are about.**
- egret built from `e39476a4c` (no scratch instrumentation), DB `egret_viewgov_live` dropped and recreated.
- Warehouse and registry catalog `ice` (so the Trino catalog name, the StarRocks external catalog name and the
  registry name are one string), namespace `s`, Iceberg tables `s.pii(id, ssn, name)`, `s.orders(cust_id, amount)`
  and `s.pub(id, v)`.
- Classification `vgsecret` on `s.pii.ssn`, cleared by one role whose members are `vcadmin`, `vccleared` and
  `vcownerc`. Uncleared: `vcplain`, `vcowneru`; StarRocks also `vcnobody` (no grant at all).
- Trino engine `vgtr`: one coordinator and one worker, rendered by the provisioner for JWT delegation (TLS 8443,
  poll interval as rendered, 30 s), pod-shaped host names `vgtr-coordinator-<hash>-<id>` and
  `vgtr-worker-<hash>-<id>`. StarRocks engine `vgsr`, external catalog `ice`, views and materialized views in
  `default_catalog.vgdb`.

**StarRocks 4.1.6 (brief step 2).**

| id | cell | prediction |
|---|---|---|
| S1a | `vcplain` reads `ssn` through a plain view `vgdb.v_plain` over `ice.s.pii` | `NULL` in every row; `id`, `name` real |
| S1b | through a nested view `v_nest` (over `v_plain`) | `NULL` |
| S1c | through an expression view `v_expr` (`upper(ssn)`, `concat(ssn,'x')`, `substr(ssn,1,3)`) | `NULL` in all three columns |
| S1d | through an INVOKER view `vi_inv` | **refused**, `ERROR 5204 … SELECT privilege(s) on COLUMN ssn`: 4.1.6 re-checks an INVOKER view as the reader, so the base deny (A1) applies before the mask (engine-upgrade §2.5 B "deny only / vi_inv / vc_plain"). The guide says "`NULL` through … INVOKER" — if observed as predicted, that is a guide finding (Minor) |
| S2 | `vcplain` `select id, ssn from ice.s.pii` | `ERROR 5204 … COLUMN ssn`; `select id, name` returns rows |
| S3 | cleared `vccleared` with a rule mask (`MASK`) on `pii.ssn` that names `vccleared`; cleared `vcownerc` without one | `vccleared` gets the rule mask (`xxx-0000`-shaped) directly and through `v_plain`; `vcownerc` gets real values; `vcplain` still `NULL` |
| S4 | `create materialized view vgdb.mv_x …` as `vcplain` | `ERROR 5204 … CREATE MATERIALIZED VIEW privilege(s) on DATABASE vgdb`; `create view vgdb.v_y` and `create table vgdb.t_y` as `vcplain` succeed. `vccleared` (cleared for every classification on the profile) creates a materialized view |
| S5 | the classification page lists `vgdb.mv_pre`, created and refreshed by an admin before the classification | listed, matched by base table (`ice.s.pii` in `BASE_TABLE_REFRESH_VERSION_TIMES`); an un-refreshed MV, if any, matched by text and flagged |
| S6 | what the FE asks the plugin for `create materialized view` | one `evaluatePolicies` request `resource=[catalog=default_catalog database=vgdb] access=create materialized view`, denied by the compiled policy for `vcplain` |
| S7 | admin/root and `information_schema.materialized_views` under Ranger | root sees **only what a Ranger policy grants it**. With no policy for root it gets an **empty set, not an error**, and the page would then say "none" while an MV exists (Critical if observed). With an all-access policy for root it sees every MV |

**Trino 483 (brief step 3).**

| id | cell | prediction |
|---|---|---|
| T1 | `vcownerc` (cleared) `CREATE VIEW ice.s.vd_c AS SELECT id, ssn FROM ice.s.pii` (DEFINER), then in the same script at once `vcplain` reads it | refused on the **first** read: `Access Denied: Cannot select from columns [id, ssn] in table or view vd_c` (every view column carries the tag); also `select id from vd_c` refused (table-level over-tagging) |
| T2 | `vccleared` reads `vd_c` | rows with real `ssn` |
| T3 | duration of `CREATE VIEW` over a classified table | returns after the plugins' next tag polls: between 0 and 30 s (one interval), mean ≈ 20 s with two pollers (max of two uniform), never above 31 s. A view over `s.pub` only (unclassified, profile classified) returns in under 2 s |
| T4 | the worker container paused (it stops polling), then `CREATE VIEW` over `pii` | refused after ≈ 60 s with `egret refused the view: engines did not confirm the classification tags within 60 s: trino@vgtr-worker-…-vgtr; …`; the view does not exist afterwards; no client timeout |
| T5 | `v_late` over `s.orders` created while `orders` is unclassified, then `orders.amount` classified `vgsecret` | `v_late` returns at once (no wait); after the classification write and one plugin poll (≤ 30 s), `vcplain` is refused on `v_late`; `vccleared` reads it |
| T6 | do Trino workers poll tags, and does the hold wait for them | yes: the ledger holds a `tags` row for `trino@vgtr-worker-…-vgtr` beside the coordinator's; a hold ends only after both report `lastKnownVersion ≥ V` |
| T7 | `CREATE VIEW ice.s.v_empty AS SELECT id, ssn FROM ice.s.pii WHERE false` | refused at once (no wait): `… lineage could not be computed: … the engine reports no input table; egret refuses views it cannot judge …` |

**Explain and the page against the engine (brief step 4).** For every reader and object above, `POST /perm/explain`
and the classification page agree with the engine: 0 differences. Explain names `vgsecret` and base `ice.s.pii`
for a Trino view column denied by the propagated tag, and `MASK_NULL` for a masked StarRocks read.

**MUST items (task-11-must.md).**

| # | prediction |
|---|---|
| M1 | EXPLAIN as `egret-lineage` (through real holds) succeeds over a table with a hand-written `{public}` mask, over one with a tag mask and over one with a row filter; a join view's recorded lineage holds both `pii` and `orders` (not shortened) |
| M2 | `egret-lineage` `INSERT INTO ice.s.pub …` and `CREATE TABLE ice.s.x …` are refused (`Access Denied: Cannot insert into table …` / `Cannot create table …`); a hand-written `show` deny for `vcplain` still applies to `vcplain` |
| M3 | `egret-lineage` cannot kill `vcplain`'s query: refused, either as `Cannot kill query owned by vcplain` or earlier as `Cannot access catalog system` |
| M4 | `CREATE OR REPLACE VIEW` with changed SQL is held (the commit body ends `set-current-view-version -1`); `COMMENT ON COLUMN` sends `add-schema` (with `doc`) + `add-view-version` (same SQL) + `set-current-view-version -1` and is held too, then succeeds |
| M5 | A → B → A (`CREATE`, `OR REPLACE` B, `OR REPLACE` A) all succeed; the final `SHOW CREATE VIEW` is A |
| M6 | 32 sequential holds: 0 refusals; hold time 0–30 s, mean ≈ 20 s; the measured plugin poll period is 30 s (± 1 s). A burst of 12 concurrent holds: 4 refused at once with the "8 classified view changes" message, 8 succeed. No hold exceeds the 60 s deadline, so "a hold longer than 65 s" cannot be produced without instrumentation; the paused-worker refusal (T4) lands at ≈ 60 s and Trino prints it |
| M7 | as T7 |
| M8 | `POST /classifications/lineage-sweep` answers 200 with a report naming no failure for healthy views. A view whose best-effort lineage failed (created before the engine had a workspace connection) is `failed`; after the engine is assigned and a base column is classified, the scoped sweep makes it `computed` and tags it within seconds, with no manual sweep |
| M9 | as T6 |
| M10 | lakekeeper's stored view properties: DEFINER `{"trino.run-as-owner": "<owner>"}`, INVOKER `{}`; the page shows DEFINER with owner and INVOKER |
| M11 | as S6 |
| M12 | as S7; the lister's statement (`SELECT … FROM information_schema.materialized_views`) runs and returns its 4 columns |
| M13 | the page's Trino table is its own section and wraps long names; with the Trino half failing it shows an error line, not an empty table; the MV list names `vgdb.mv_pre`; the explain page prints "not transformed" for a `MASK_NONE` cleared user and names `default_catalog.vgdb` on the MV row. Expected cosmetic only: the MV-section indent (Task 10 N1) and a red "denied" badge for a cleared reader's MV row (Task 10 N2) |

**Eligibility (how I expect to make the Trino half run).** egret cannot provision without Kubernetes, so a
provisioned JWT Trino engine has to be represented by database rows that equal what the provisioner writes; the
container runs the provisioner's rendered configuration. If that cannot be done honestly, the Trino half is
BLOCKED, not faked.

### 2.1 Result (read this first)

**DONE_WITH_CONCERNS. 31 cells: 23 matched their prediction, 8 did not.** Nothing is BLOCKED, but the Trino half
ran on an engine that egret did not provision itself (2.2, DEV-1 and DEV-2).

**What works as designed:**
- A DEFINER view by a cleared owner is refused to an uncleared reader on the **first** read after `CREATE VIEW`
  returns.
- A view with an empty lineage is refused at once with the R6 reason.
- A view's tags follow a base classification within one poll.
- The 8-hold cap and the stale-plugin refusal behave as the guide says.
- The StarRocks views give `NULL`, the materialized-view deny holds, and the page lists every existing
  materialized view.

**What does not:**
- **Four Critical findings (2.6), all an explain-or-page versus engine difference:**
  - **C1:** Trino explain says *allowed* to every uncleared reader of a classified column, base or view; the
    engine denies.
  - **C2:** StarRocks gives an uncleared direct read `NULL`, not `ERROR 5204`; explain says *denied*.
  - **C3:** with two tags, the Trino plugin applies every tag policy to every tagged column; explain sees none
    of it.
  - **C4:** the classification page still says views return classified columns in full.
- **Holds take up to two poll intervals, not one.** The median was 60.04 s for back-to-back holds, against a 60 s
  deadline.
- **On a correctly provisioned JWT engine, every lineage EXPLAIN fails** until someone adds an allow for
  `query_max_run_time`.
- **egret's token rate limit and the Iceberg token refresh break Trino's catalog access** under concurrency and
  during the 15-minute sweep.

### 2.2 Stack, versions proven by output, and every deviation

| piece | what ran |
|---|---|
| egret | Image `egret-backend:vg-t11` built from `e39476a4c` with `deploy/docker/backend.Dockerfile` (no instrumentation). It ran as container `vg-egret-core` on network `vg-net`, DB `egret_viewgov_live` (dropped and recreated), `egret migrate` then `egret init`. Log line: `kubernetes unavailable; engine provisioning disabled`. `catalog.public_url = http://vg-egret:8096`, which is also the issuer. |
| request log | `vg-egret`: a Python reverse proxy (stdlib, `python:3.13-slim`) on `:8096` in front of `vg-egret-core`. It keeps the `Host` header and logs every `/service/*`, `/oauth/*` and view-changing catalog request with its status, duration and body. egret itself has no request log (DEV-4). |
| object store | `vg-rustfs` (`rustfs/rustfs:1.0.1`), bucket `warehouse`, storage connection and warehouse `ice` through the API. Registry catalog `ice` was created with the warehouse. |
| Trino | `vg-trino-coord` (hostname `vgtr-coordinator-6d5f7c9b8-ab1cd`, aliases `vgtr-trino` and `vgtr-trino.egret-engines.svc`) and `vg-trino-worker` (hostname `vgtr-worker-6d5f7c9b8-x9y8z`), image digest `db58cc93…` = Trino 483. Their files are **the provisioner's own rendering**: `trino_config_map` was dumped by a scratch test, then deleted. The rendering is JWT only, TLS 8443, `allow-insecure-over-http=false`, `jwt.key-file=http://vg-egret:8096/.well-known/jwks.json`, `principal-field=preferred_username`, `required-audience=engine`, the Ranger plugin on both roles, and poll 30000 ms. Catalog `ice` is in the `render_trino` Egret shape: OAuth2 catalog client and vended credentials (they worked). |
| StarRocks | `vg-sr` (`allin1-ubuntu@sha256:5cbf09c4…`, 4.1.6-6862092), alias `vgsr-fe-service.egret-engines.svc`. Ranger files as rendered, except poll 5000 ms. External catalog `ice` was **created by egret's own attach path** (`POST /engines/{id}/catalogs/{cid}` → `applied`, `CREATE EXTERNAL CATALOG ice … iceberg.catalog.uri=http://vg-egret:8096/catalog`). |
| identities | egret users `vcadmin`, `vccleared`, `vcplain`, `vcownerc` and `vcowneru`, members of workspace `default`. Platform role `vg_clear` (members `vcadmin`, `vccleared`, `vcownerc`) is the clearance of tag `vgsecret`. StarRocks has the same users plus `vcnobody`. |
| data | `ice.s.pii(id, ssn, name)`, `ice.s.orders(cust_id, amount)` and `ice.s.pub(id, v)`. `vgsecret` is on `pii.ssn`, and later also on `orders.amount`. |

**Deviations, all labelled:**

- **DEV-1 (SQL): the engine rows.**
  - `engine_mgmt.engines` rows `vgtr` and `vgsr` were inserted as `origin = 'provisioned'`, `status = 'running'`,
    namespace `egret-engines` and bound to profiles `vgtr` / `vgsr`.
  - The `vgtr` spec is what `create_engine` stores for JWT (`trinoConfig.delegationMode = jwt`,
    `auth.jwt = {}`, image `483@sha256:db58cc93…`), plus the coordinator cert.
  - The `vgsr` spec carries `starrocksConfig.feImage = starrocks/fe-ubuntu:4.1.6@sha256:d07a…`. The container runs
    the allin1 4.1.6 image, which has the same FE version and the same plugin, 2.9.0.
  - Why: `POST /engines` answers `kubernetes not configured` before it inserts anything.
  - **Everything after the rows went through egret's own code:**
    - the workspace assignment (`PUT /engines/{id}/workspaces`) materialized the delegated `query.databases` row
      `vgtr-trino.egret-engines.svc:8443` with the pinned cert;
    - the lineage EXPLAIN dialled it with egret's own token;
    - the transform was served to the pod-shaped `pluginId`s.
- **DEV-2 (SQL): the Trino attachment.** The `vgtr`→`ice` attachment row was inserted as `applied`, because the
  Trino attach answers `400 kubernetes not configured`. Inserting it bumped nothing.
- **DEV-3 (SQL): hand-written policies.** `UPDATE policy.authz_services SET origin = 'manual'` was run several
  times on `vgtr`, `vgsr` and `vgtr-tag` so hand-written policies could be written. These are the Trino baselines
  `trinouser:*` impersonate, `queryid:*` execute and `query_max_run_time` alter (`{USER}`), the test allows, the
  `{public}` `MASK_NULL` on `orders.cust_id`, the `{public}` `1 = 0` row filter on `orders`, the `vcplain` denies on
  `pub`, a tag mask on tag `vgkey`, and the StarRocks allows.
  - Every catalog attach, workspace change and classification write recompiles and stamps the profile
    `generated` again.
- **DEV-4 (harness):** the logging proxy (above).
- **DEV-5 (config):** `EGRET_RATE_LIMIT__TOKEN_MAX_ATTEMPTS = 100000` from 19:53 UTC: the last burst, T5 and the
  manual sweep. Before that the default of 60 per minute applied (finding I3).
- **DEV-6 (identity):** the test users' engine JWTs were minted by a script with **egret's own signing key**. The
  key is read from `catalog.oidc_signing_keys`, where it is stored in plain text. The claims are
  `engine_claims`'s: `iss`, `sub`, `preferred_username`, `groups: []`, `aud: engine`, a 900 s lifetime and the
  `kid`.
  - Spot check through egret's real query API as `vcplain` (`POST /api/ws/default/queries`): `vd_c` →
    `Access Denied: Cannot select from columns [id, ssn] in table or view vd_c`; `pii.name` → 3 rows. This is
    the same as the script.
- **DEV-7 (config):** StarRocks poll 5000 ms. The StarRocks users, `vgdb`, its views and `mv_pre` were created
  as root before Ranger was switched on, and the Iceberg tables were created through StarRocks.
- **DEV-8 (TLS):** a self-made coordinator cert. The first one had `CA:TRUE`; rustls refused it (the lineage failed
  with `error sending request`). It was regenerated as a leaf certificate, written to `coordinator_cert`, and the
  connection was re-materialized by unassigning and reassigning the workspace.
- **Restarts:**
  - egret twice: to put the proxy in front, and for DEV-5;
  - Trino four times: proxy routing, the new cert, and twice to recover from I3/I4.

### 2.3 StarRocks 4.1.6

**Request evidence** (the FE's `RangerPolicyEngineImpl.evaluatePolicies` debug log, excerpt kept in the
scratchpad).
- `create materialized view` by `vcplain`, `vcownerc` and `vccleared` asked
  `elements={database=vgdb; catalog=default_catalog} accessType={create materialized view}`.
- The direct read `select id, ssn from ice.s.pii` by `vcplain` asked `policyType=1` (mask) for `id`, `ssn`,
  `name`, `_file` and `_pos`, `policyType=2` (row filter) for the table, and `policyType=0` (access) **only for
  `id`**.

| id | prediction | observation | verdict |
|---|---|---|---|
| S1a | `NULL` through `v_plain` | `1 NULL alice / 2 NULL bob / 3 NULL carol` | match |
| S1b | `NULL` through `v_nest` | `NULL` ×3 | match |
| S1c | `NULL` in `u, c, s` | `NULL NULL NULL` ×3 | match |
| S1d | INVOKER refused, `5204 … COLUMN ssn` | `ERROR 5204 … SELECT privilege(s) on COLUMN ssn`; `vccleared` and `vcownerc` read it; `vcnobody` `… COLUMN id` | match (the guide says `NULL`: Minor M-1) |
| S2 | direct read `ERROR 5204` | **`NULL` ×3, no error** (no access check is asked for the masked column, above); `select id, name` gives rows; `vcnobody` gets `5204 … COLUMN id` | **mismatch → C2** |
| S3 | `vccleared` rule mask, `vcownerc` real, `vcplain` `NULL` | rule `vg-rule-mask-ssn` (`MASK`, exempt `vcownerc`): `vccleared` `000-00-0000` directly and through `v_plain`; `vcownerc` `111-22-3333`; `vcplain` `NULL`. Explain: `MASK` / `MASK_NONE` / `MASK_NULL` | match |
| S4 | `vcplain` MV refused; view and table created; `vccleared` creates an MV | `ERROR 5204 … CREATE MATERIALIZED VIEW privilege(s) on DATABASE vgdb`; `create view vgdb.v_y` OK; CTAS `vgdb.t_y` OK and readable; `vccleared` `mv_cl` OK; `vcownerc` `mv_c2` OK (`mv_c2` then gives `vcplain` real `ssn`: cell 6a, known) | match |
| S5 | page lists `mv_pre` by base table | the page lists `mv_c2`, `mv_pre` and `mv_cl`, `matchedBy: baseTable`, `default_catalog.vgdb`, engine `vgsr`, `enginesNotChecked: []` | match |
| S6 | FE asks `{default_catalog, vgdb}` + `create materialized view` | as predicted (above) | match |
| S7 | root sees only what a policy grants; with none, an empty set | **root, with no Ranger policy at all, sees all 3 MVs** (and reads `mv_pre` data); `vcadmin` and `vcplain` see all 3; `vcnobody` gets an empty set. `SHOW MATERIALIZED VIEWS FROM vgdb` works as root; the lister's statement returns its 4 columns | **mismatch** (favourable: the lister cannot be blinded; root is a superuser under Ranger on 4.1.6) |

### 2.4 Trino 483

| id | prediction | observation | verdict |
|---|---|---|---|
| T1 | first read refused, `select id` refused | `CREATE VIEW ice.s.vd_c` as `vcownerc` returned at 18:56:33 after 44.74 s. **The same script at once:** `vcplain` `SELECT *` → `Access Denied: Cannot select from columns [id, ssn] in table or view vd_c`; `SELECT id` → `… [id] …` | match |
| T2 | `vccleared` reads | `[[1,"111-22-3333"],[2,"444-55-6666"],[3,"111-22-3333"]]` | match |
| T3 | ≤ 31 s (one interval) | **44.74 s** (`POST …/views -> 200 43.957s`). Mechanism, from the ledger and proxy: the plugins downloaded v8 on their next poll (18:56:02, 200) but report `lastKnownVersion=8` only on the poll after (18:56:32), and only that confirms. So a hold takes from 1 to 2 intervals. Later holds: 33.8, 35.8, 57.2, 59.0, 59.8 s, and 40.8–60.4 s in M6. A view over `pub` only: 0.38 s | **mismatch → I1** |
| T4 | worker paused → confirmation refusal at ≈ 60 s | With the worker paused, the hold's **EXPLAIN** hangs, so the refusal arrives after 5.03 s: `egret refused the view: lineage could not be computed: the EXPLAIN did not answer within 5 s; …`, and the view does not exist. The confirmation path was then shown with a plugin id that stopped polling (one poll `trino@vgtr-worker-6d5f7c9b8-dead1-vgtr`, `lastKnownVersion=3`): `POST …/views -> 409 60.135s`, and Trino printed `egret refused the view: engines did not confirm the classification tags within 60 s: trino@vgtr-worker-6d5f7c9b8-dead1-vgtr; …`. No tag or lineage row was left, and there was no client timeout | **mismatch** (fail-closed both ways) |
| T5 | `v_late` refused ≤ 30 s after `orders.amount` is classified | `v_late` (lineage `orders`) refused **19 s** after the `PUT resource-tags`; derived rows `v_late.{amount,cust_id}` were written in the base write | match |
| T6 | workers poll tags; hold waits for all | Ledger: `trino@vgtr-worker-6d5f7c9b8-x9y8z-vgtr` `policies` and `tags` beside the coordinator's from the first poll. Every hold ended only after both reported ≥ V, and a third (dead) id blocked the hold until 60 s | match |
| T7 | `WHERE false` refused at once, R6 | 0.27 s: `egret refused the view: lineage could not be computed: the engine reports no input table; egret refuses views it cannot judge; …`; `Table 'ice.s.v_empty' does not exist` | match |

### 2.5 Explain and the page against the engine (brief step 4)

**Trino**, `POST /perm/explain` (read, column) compared with the engine's `select <col> from ice.s.<t>`:

| reader | `pii.ssn` | `pii.name` | `vd_c.id` | `vd_c.ssn` | `vi_c.ssn` | `v_pub.v` |
|---|---|---|---|---|---|---|
| `vcplain` | engine denied / explain **allowed** | rows / allowed | denied / **allowed** | denied / **allowed** | denied / **allowed** | rows / allowed |
| `vccleared` | rows (rule mask) / allowed | rows / allowed | rows / allowed | rows (real: DEFINER, owner exempt) / allowed | rows (masked: INVOKER) / allowed | rows / allowed |
| `vcowneru` | denied / **allowed** | rows / allowed | denied / **allowed** | denied / **allowed** | denied / **allowed** | rows / allowed |

That is 8 of 18 cells different. Explain lists `tags: [vgsecret]` and `tag_policies_in_download: 2` on every
differing cell, but names the grant `perm-grant-9f0cba09…` as the decision → **C1**. The explain page in the
browser prints the same ("select Allowed … Tags that could decide here vgsecret").

**StarRocks**, explain compared with the engine:

| reader | explain (access, mask) | engine | verdict |
|---|---|---|---|
| `vcplain` | **denied** (`perm-clearance-…-ice.s.pii.ssn`), `MASK_NULL` | `NULL` rows, no error | **differs → C2** |
| `vccleared` | allowed, `MASK` | `000-00-0000` | same |
| `vcownerc` | allowed, `MASK_NONE` | real | same |
| `vcnobody` | denied | `5204 … COLUMN name` | same |

**The page against the engine:**
- **Materialized views:** the page and the engine agree (2.3 S5).
- **Trino views:** `trinoViews` lists 7 views; the 50 later ones were added by M6.
  - DEFINER/owner/INVOKER equal the stored properties (M10).
  - `ownerCleared` is true for `vcadmin` and `vcownerc`, and `null` for the INVOKER view.
- **The page's static warning contradicts the engine → C4.** It reads: "A view over a classified column is not
  covered on either engine: reading the column through a view returns it in full to a user without clearance…".

### 2.6 Findings

**Critical** (any difference between explain or the page and the engine, brief step 4):

- **C1. Trino explain ignores the clearance tag deny.** For every uncleared reader of a classified column, of a
  base table or a view, explain answers `allowed` by the catalog grant. The engine answers
  `Access Denied: Cannot select from columns …` (2.5).
  - *Hypothesis, not proven:* the tag block is served with the **trino** service-def, because the tag service is a
    trino-typed profile, which is the only type the API accepts (`type must be "trino" or "starrocks"`). The
    evaluator's tag phase then matches no `tag` resource.
- **C2. StarRocks: a direct read of a classified column is not denied; the mask nulls it.** The FE asks no access
  check for a column that has a data-mask policy (2.3 request evidence), so `A1`'s deny never applies once A2's
  mask exists.
  - No value leaks (`NULL`).
  - But explain says `denied`, and the guide ("Direct reads are denied … refused by the clearance deny") and spec
    A1 are false on 4.1.6.
- **C3. Trino: with more than one tag, each tag policy applies to every tagged column.** With `vgkey` (tag mask
  `MASK_NULL` for `{public}`) on `pii.id` and `vgsecret` on `pii.ssn`:
  - cleared `vccleared` read `ssn` as `NULL`;
  - `vcplain` was **denied** `pii.id`, which carries only `vgkey`: `Cannot select from columns [name, id]`.
  - Explain showed allowed and no mask for both.
  - The plugin logs, at every policy switch,
    `RangerDefaultPolicyResourceMatcher.init() failed: policyResources elements are not part of any valid
    resourcedef hierarchy. (serviceDef=trino, policyResourceKeys=tag`, one line per tag policy.
  - The likely root cause is C1's.
  - **Effect:**
    - a user cleared for classification X but not Y is denied X-only columns;
    - every tag mask masks every tagged column;
    - a hand-written tag allow would allow every tagged column.
  - The `vgkey` tag and its mask were removed afterwards, so the other cells ran with one tag.
- **C4. The classification page says the opposite of what is enforced.** Its warning box still says views return
  the classified column "in full to a user without clearance" on both engines. Measured: Trino denies (T1) and
  StarRocks nulls (S1). The text is frontend copy, outside the Task 10 changes. It is not fixed here, because it
  is not a defect that blocks the page from working.

**Important:**

- **I1. A hold takes up to two poll intervals.**
  - Mechanism: the plugin downloads the new tags on its next poll, and only the poll after that reports
    `lastKnownVersion ≥ V`.
  - Measured:
    - single holds took 33.8–59.8 s;
    - 31 back-to-back holds succeeded with a median of **60.04 s** (min 40.8, max 60.38); the server-side times
      were 58.7–59.3 s;
    - so the margin to the 60 s deadline was about 0.7 s.
  - A hold arriving within that margin after a poll would be refused.
  - The guide ("waits up to one poll interval") and spec B2 ("Cost: … up to one poll interval") are wrong.
- **I2. The lineage EXPLAIN needs `alter` on `systemproperty` = `query_max_run_time`, and nothing grants it on a
  JWT engine.**
  - egret's client sets that property on every statement.
  - `seed_delegator_policies` (`egret-query-timeout`) runs only for **impersonate** engines.
  - The transform keeps the access but does not allow it.
  - Without a hand-written allow, every lineage failed with
    `engine vgtr: engine: Access Denied: Cannot set system session property query_max_run_time`. That
    refuses every classified view and leaves every best-effort lineage `failed`.
  - After the allow (`base-query-timeout`, `{USER}`), the served transform had no `allowExceptions` on it and
    lineage worked.
- **I3. egret's token rate limit (60 per minute) breaks Trino's catalog access.**
  - Trino's Iceberg client takes a fresh token per query session: 134 `POST /oauth/token` for one burst of 12.
  - **12 concurrent `CREATE VIEW`:** 12 of 12 failed inside Trino after about 33 s, with
    `RESTException: Unable to process (code: 429, type: slow_down)` (`Failed to create view` / `Failed to load
    metadata for namespace 's'`). egret's hold never saw them, so the 8-cap was not reached.
  - **The scheduled 15-minute sweep:** its EXPLAINs hit the same limit. 3 of 27 and then 12 of 41 timed out at
    5 s, and one concurrent user `CREATE VIEW` failed with `Failed to load view 'm6seq_18'`.
  - With DEV-5, the burst gave exactly 4 × `egret is holding 8 classified view changes already, its maximum`
    (0.1 s) and 8 successes (48 s), and the manual sweep gave 50 of 50 computed in 16.8 s.
- **I4. The Iceberg OAuth2 token refresh fails against egret.** About an hour after Trino started, Trino's
  `auth-session-refresh` sent `grant_type=…token-exchange` and egret answered `422 Failed to deserialize form
  body: missing field 'client_id'`.
  - Trino retried in a storm: 403 errors in its log, which feeds I3.
  - The renderer does not disable token refresh.
  - This exists before this branch, and it breaks the live catalog.
- **I5. Deleting a hand-written tag-service policy does not reach plugins.** `DELETE /policies/25` (in
  `vgtr-tag`) bumped `vgtr-tag` to version 4 but left `vgtr` at 20. Plugins kept getting `304` and kept the
  deleted tag mask, until an unrelated `vgtr` write bumped it. This exists before this branch.

**Minor:**

- M-1: the guide says an uncleared reader gets `NULL` through an INVOKER view. On 4.1.6 it is refused (S1d),
  which is fail-closed. *(Task 12: the guide now says plain and nested views give `NULL` and an INVOKER view is
  refused.)*
- M-2: `COMMENT ON COLUMN` and a replace that adds no tag do not wait (0.45 s). This is correct behaviour; the
  prediction was wrong.
- M-3: each Trino plugin sends **two** tag downloads per poll under one `pluginId` (138 requests for 64 polls).
  After every policy switch a fresh refresher reports `lastKnownVersion=-1` once. The ledger keeps the last
  write, which only ever delays a confirmation.
- M-4: explain's materialized-view row asks about `default_catalog.<schema of the target>` (`default_catalog.s`).
  That is where nothing is created; the materialized views here live in `vgdb`. For a cleared user it says
  "No policy matched", while the same user created an MV in `vgdb`. The page states the choice.
- M-5: the explain page's generic text ("answers about ACCESS only … masked … reported here exactly like one it
  does not") contradicts the new mask line above it.
- M-6: the classification header says "0 tagged objects" while `pii.ssn` and about 60 view columns carry the tag.
- M-7: Trino explain has no mask line. `vccleared` reads the `MASK` rule through INVOKER `vi_c`, and explain is
  silent (StarRocks only, by design).
- M-8: `POST /warehouses/{id}/roles` with an unknown `consumerType` answers 500 (a CHECK violation), not 400.
- M-9 (observation): egret's machine-IdP private key is stored in plain text in `catalog.oidc_signing_keys`.
- M-10 (not measurable here): a hand-written `show` deny for `vcplain` does not hide `pub` from
  `SHOW TABLES` (a `select` deny does deny). There was no pre-transform baseline, so this says nothing about the
  transform.

### 2.7 MUST items

| # | prediction | observation | verdict |
|---|---|---|---|
| M1 | EXPLAIN succeeds over a hand-written `{public}` mask, a tag mask and a row filter; lineage full | `v_join` (`orders` ⋈ `pii` on `pii.id = orders.cust_id`) with a hand-written `{public}` `MASK_NULL` on the join key `orders.cust_id`, a `{public}` `1 = 0` row filter on `orders`, and a tag mask `MASK_NULL` on `pii.id` (all active: `vcplain` saw `id` `NULL` and `orders` empty). Recorded lineage `{orders, pii}`, `computed`. Direct `egret-lineage` EXPLAIN listed both. Precondition: I2 | match |
| M2 | `egret-lineage` writes refused; deny for others stays | `INSERT INTO ice.s.pub` → `Cannot insert into table pub`; `CREATE TABLE` → `Cannot create table lin_x`; `CREATE VIEW` → `Cannot create view lin_v`; `DROP VIEW` → `Cannot drop view v_pub`. `vcplain`'s hand-written column deny on `pub`: `Cannot select from columns [v, id] in table or view pub`, while `vcowneru` and `egret-lineage` read it. `show` alone: M-10 | match |
| M3 | kill refused | `DELETE /v1/query/<vcplain's running query>` as `egret-lineage` → `403`, and the query kept running (40 s). As `vcowneru` → `204`, canceled (the hand-written `{public}` `trinouser:*` impersonate allows it). `CALL system.runtime.kill_query` → `Cannot access catalog system` for everybody | match |
| M4 | changed replace held (`set-current -1`); `COMMENT ON COLUMN` held | Replace held 33.8 s: `set-properties{trino.run-as-owner}`, `add-schema`(1), `add-view-version`(2, `schema-id -1`), `set-current-view-version -1`; `name` became a tagged column. `COMMENT ON COLUMN vd_c.name`: `add-schema`(2, `name` doc `person name`), `add-view-version`(3, same SQL), `set-current-view-version -1`, **0.45 s, no wait**. `COMMENT ON VIEW`: `set-properties{comment}`, 0.075 s | mismatch (M-2) |
| M5 | A→B→A succeeds | `CREATE` (35.8 s), `OR REPLACE` B (59.0 s), `OR REPLACE` A: **1.07 s**, body `set-properties` + `set-current-view-version 1`; `SHOW CREATE VIEW` = A | match |
| M6 | 0/32 refused, ≈ 20 s; poll 30 s; burst 4 refused / 8 OK | Sequential: 31 OK (median 60.04 s), 0 refused by egret, 1 failed in Trino (`Failed to load view`, during the sweep, I3). Poll period from the proxy: 30.00 s median, all four streams. Burst: 12/12 failed (I3); with DEV-5, 4 refused at once and 8 OK. **A hold past 65 s cannot occur:** the longest was 60.38 s at the client; the refusal arrives at 60.1 s | mismatch (I1, I3) |
| M7 | R6 | T7 | match |
| M8 | sweep 200, no failure; classification auto-sweeps failed views | `POST /classifications/lineage-sweep` → `{"views":50,"computed":50,"retagged":0,"failures":[]}`, 16.8 s (with DEV-5). The `v_pre_fail*` views were `failed` (created before the workspace assignment); `v_late` and `v_probe` were `failed` (I2 / DEV-8). The classification write at 18:49:44.9 flipped all five to `computed` between 18:49:45.28 and 18:49:47.07, with derived tags written. No manual sweep | match |
| M9 | workers poll; hold waits for all | T6 | match |
| M10 | DEFINER property present, INVOKER absent | `vd_c` `{comment, trino.run-as-owner: vcownerc}`, `v_aba` and `v_pre_fail` `{trino.run-as-owner: …}`, `vi_c` `{}`; the page shows DEFINER/owner and INVOKER/— | match |
| M11 | FE asks `{default_catalog, db}` | S6 | match |
| M12 | root sees only what a policy grants | S7 | mismatch |
| M13 | page and explain render; only N1/N2 cosmetic | Screenshots `t11_page_full.png`, `t11_page_2.png`, `t11_page_narrow.png` (700 px: the long names wrap, no overflow), `t11_page_trino_error.png` (response rewritten in the browser to `trinoViews: null`: the error line shows and no table), `t11_explain_vcownerc.png` ("Mask on select: not transformed (MASK_NONE, …)", MV row `default_catalog.s`). No blocking frontend defect, so no frontend commit. Cosmetic: C4 (copy), M-5, M-6. N1 (indent) was not visible; N2 was not exercised | match |

### 2.8 Left behind

- **Stopped:** containers `vg-egret`, `vg-egret-core`, `vg-trino-coord`, `vg-trino-worker`, `vg-sr`, `vg-rustfs`
  and network `vg-net` were removed; the vite dev server (PIDs 74789 and 74821, port 5196) was stopped by PID.
- **Kept:**
  - database `egret_viewgov_live` in `egret-dev-postgres-1` (throwaway);
  - image `egret-backend:vg-t11` (about 400 MB);
  - the scratchpad `t11/` (logs, evidence files and screenshots). Its proxy log holds a catalog client secret of
    that throwaway DB.
- **Untouched:** DB `egret`, every other `egret-dev-*` container, `egret-w7e-postgres`, and ports 5173, 5000 and
  8080.
- **Worktree:** clean. The scratch render test was deleted, and no product code was changed.

### 2.9 What Task 12 changed (2026-10-15)

Not re-measured live; each change is pinned by tests (Task 12 report).

- **I1:** the hold limit is 3 × the poll interval, 90 s (user ruling, spec R7). The confirmation rule is unchanged.
  Spec B2 and the guide say a hold takes one to two intervals, and why.
- **I2:** the served `egret-lineage` transform allows `alter` on `systemproperty` = `query_max_run_time` only.
  Ordinary JWT users are not covered by this; see the Task 12 report for that finding.
- **C2:** explain mirrors StarRocks: for a column a mask transforms it gives no `select` verdict and answers with
  the mask ("returns NULL"), labelled engine-specific. A column with no mask still shows the deny. Trino is
  unchanged. Spec A1 and the guide are corrected.
- **C4:** the classification page's view warning states the measured behaviour.
- **M-4 / N2:** explain no longer answers `create materialized view`; the page states the engine-wide rule.
- **M-5:** the explain page's limits text names the StarRocks mask line instead of contradicting it.
- **M-6:** the classification counts are labelled "BI objects": `assignment_count` counts BI objects only, never
  catalog columns, base or derived.
- **M-8:** an unknown `consumerType` on `POST /warehouses/{id}/roles` (and `/clients`) answers 400.
- **I3, I4, I5:** documented in the guide as known limits with workarounds; fixed in a later package.
- **C1, C3:** fixed on main in a separate PR (`fix/tag-policies-service-def`).
