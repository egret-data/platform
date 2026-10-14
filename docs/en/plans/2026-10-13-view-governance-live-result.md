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
