# StarRocks unmeasured paths: a probe (design)

**Status:** draft for the user's review, 2026-11-09. The scope (four paths, fork-e2e style, a
measurement package) was decided by the user; the open questions are at the end.

**Package:** C of the StarRocks follow-ups. B (engine posture) comes first, then A (the audit
platform side and the Query log tab), then C. A and B have their own specs.

## Goal

Measure, on the same harness that proved patches `0001` to `0009`, whether StarRocks 4.1.7 enforces
egret's security model on four paths no one has measured:

1. Java and Python UDFs;
2. non-Iceberg catalogs: Hive, JDBC and Paimon;
3. stream load and routine load;
4. the CN/BE ports, from inside the engine namespace.

This is a **measurement package**. It delivers:
- probe checks committed to the fork (`egret-data/egret-starrocks`, `tests/e2e/`);
- a findings report in this repository,
  `docs/en/reports/2026-11-09-starrocks-unmeasured-paths-probe.md`;
- for each confirmed leak, a follow-up item (a new fork patch from `0010` on, or an egret change).
  Each follow-up gets its own spec later. **This package writes no patch and no product code.**

Each finding ends in one of three classes:
- **enforced**: the path refuses what egret's policies refuse;
- **leak**: the path gives a principal what egret's policies refuse. It gets a candidate fork patch
  `00NN` or an egret-side mitigation;
- **out of egret's model**: the behaviour is by design or belongs to the operator. It is written
  down as a known issue in `egret-data/docs`.

## What is already closed (controls only)

Patches `0001` to `0009` close these holes, and this package does not probe them again:
- Iceberg metadata tables;
- prepared statements and the binary protocol;
- Iceberg hidden columns under a policy;
- the HTTP SQL API and Arrow Flight SQL;
- `SHOW CREATE CATALOG`;
- `FILES()`;
- materialized views over masked or filtered tables;
- `EXPLAIN COSTS`;
- `BROKER LOAD`, `EXPORT`, `INTO OUTFILE` and `ENGINE = file` with the cluster's identity.

Some of these patches are generic (for example `0004`, `0005`, `0007` and `0008` do not look at the
catalog type). The probe reruns their read paths on the new catalogs as **controls**, to show that
the patch reaches those connectors too. It does not test the patches again on Iceberg.

## Where egret stands today (checked against `main` 26016f9b1)

- **Engine images.** egret provisions the FE from the fork image
  `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4`, and the CN from the official
  `starrocks/cn-ubuntu` image. No fork patch touches BE or CN code (fork `README.md`).
- **FE config.** `starrocks_fe_configmap` (`crates/egret-provisioner/src/starrocks.rs`) writes the
  FE ports 8030, 9020, 9030 and 9010. It runs the FE in shared-data mode, with the storage
  volume's keys in `fe.conf`, and turns on `access_control = ranger` when the engine is bound.
  - It does not set `enable_udf`. That key is not in `SR_PROTECTED_FE_KEYS`
    (`crates/egret-engine-mgmt-api/src/lib.rs`), so an admin's `feConfig` can turn it on.
- **CN config.** `starrocks_cn_configmap` writes only the admin's `cn_config`. The CN therefore
  runs with the 4.1.7 defaults:
  - ports: `be_port` 9060, `be_http_port` 8040, `heartbeat_service_port` 9050, `brpc_port` 8060
    and `starlet_port` 9070 (`be/src/common/config.h`);
  - **`enable_http_auth = false`**.
- **NetworkPolicy.** `egret-engine-isolation` (`deploy/helm/egret/templates/networkpolicy-engines.yaml`)
  admits, on **every** port:
  - every pod of the engine namespace;
  - egret's pods;
  - the `extraFrom` peers.

  It admits the operator only on 8030 and 9030, and the ingress controller only on the engine
  HTTP ports. Egress is open.
- **Catalogs.** egret creates every external catalog itself, as `root`, with its secrets inline in
  the statement (`crates/egret-catalog-registry/src/render/starrocks.rs`). The types are
  `iceberg`, `hive`, `deltalake`, `hudi` and `jdbc`, plus `paimon` through the advanced
  passthrough.
- **Functions.** egret's StarRocks capability list (`crates/egret-perm-compile/src/engine_capabilities.rs`)
  offers `starrocks.use_global_function` (USAGE on a global function). It offers no capability to
  create a function.
- **Delegation.** egret queries as a service user that runs `EXECUTE AS <end user>`. End users may
  also log in to StarRocks directly.

## Hypotheses from the 4.1.7 source

These come from reading the fork's checked-out 4.1.7 source (`work/starrocks` after
`scripts/apply.sh`). **They are hypotheses, not findings.** The probe exists to measure them.

| Id | Hypothesis | Source |
|---|---|---|
| H1 | `CREATE FUNCTION` is refused for everyone while `enable_udf` is off, and `enable_udf` is off by default. | `CreateFunctionAnalyzer.analyze`; `Config.enable_udf = false` |
| H2 | A Java UDF on the BE/CN runs with no SecurityManager. `UDFClassLoader` installs one only when the JVM has `java.security.policy`, and the official `be.conf` `JAVA_OPTS` does not set it. So UDF code reads the node's files and opens connections as the node. | `java-extensions/udf-extensions/.../UDFClassLoader.java`; `conf/be.conf` |
| H3 | At `CREATE FUNCTION`, the FE fetches the `file` URL itself (`computeMd5`). With storage properties that carry no keys, it uses the default credential chain: the `0009` pattern, on a statement `0009` does not cover. | `CreateFunctionAnalyzer.setupCredential`, `computeMd5` |
| H4 | BE/CN HTTP endpoints ask for no login while `enable_http_auth` is false (the default). This includes `/varz`, `/greplog`, `/api/update_config`, `/api/_stop_be` and `/api/_load_error_log`. With auth on, `/api/_load_error_log` needs only a login (privilege NONE). | `be/src/service/service_be/http_service.cpp`, `http_auth_response.cpp`, `download_action.h` |
| H5 | `enable_token_check` is mutable, so an unauthenticated `update_config` can turn it off. `/api/_download_load` then serves tablet files with no token. | `config.h` (`CONF_mBool(enable_token_check, "true")`), `download_action.cpp` |
| H6 | The FE's thrift service (9020) takes the caller's identity from the request (`current_user_ident`) for metadata calls such as `getDbNames`. `forward` refuses a caller that is not a known FE. | `FrontendServiceImpl.getDbNames`, `forward` |
| H7 | Paimon system tables (`t$files`, `t$snapshots`, ...) are resolved by the Paimon catalog, not as StarRocks `MetadataTable`s. So `0001`'s check does not reach them, and a `table = *` grant reads them under a mask and a row filter. | `connector/paimon/PaimonMetadata.java`; patch `0001` body |
| H8 | The BE accepts a stream load with no login of its own (`need_auth() == false`). It then asks the FE for the user's password and privileges when the transaction begins. | `be/src/http/action/stream_load.h`, `transaction_stream_load.h` |

H6's `forward` half and H8 are expected to come out **enforced**. They are controls that show the
probe sees a refusal when there is one.

## Attacker models

| Model | Who in the harness | Real-world counterpart |
|---|---|---|
| M1: an ordinary SQL user with no grant | `eve` | any user with a direct StarRocks login |
| M2: a user with a masked and filtered read grant | `bob`; `mallory` (who may also create tables and materialized views in `demo`) | a governed analyst |
| M3: a user with one narrow elevated grant | `udfdev` (CREATE FUNCTION on `demo`, and bob's policies); `loader` (INSERT on one table only) | an engineer granted one capability |
| M4: the delegated path | `svc` with `EXECUTE AS <user>` | egret's own queries |
| M5: a pod in the engine namespace, with no credentials | a container on the harness network, with no StarRocks login | a compromised engine pod, or code that runs inside one (a UDF, a JDBC driver, a Trino worker) |

Item 1 links to item 4: a UDF runs inside the CN, so whoever may create one is an M5 attacker if
H2 holds.

## How a check reports (the verdict model)

The fork's `checks.py` is a gate: every check must pass on the egret image. Probe checks cannot be
a gate yet, because on most of these paths neither image is patched. They live in separate files,
run by a separate script, and print a **verdict** instead of PASS/FAIL:

| Verdict | Meaning |
|---|---|
| `ENFORCED` | The request was refused or the result was what the policies allow, **and** the check's sight control succeeded. |
| `LEAK` | The result carries what the policies refuse. |
| `BLIND` | The sight control failed, so a refusal proves nothing. The run fails. |
| `N/A` | The path does not exist on this image, for a measured reason that is printed (for example `python3 not found`). It is never counted as enforced. |
| `SKIP` | Opted out by `PROBE_SKIP`, with its name printed. |
| `ERROR` | The check broke (harness fault). The run fails. |

**Sight control.** Every check has one. It is the same request made by a principal entitled to
the result, or a direct read by the harness, and it must produce the very content that the
check's leak predicate looks for. Examples:
- root's stream load succeeds;
- alice reads the Paimon `t$files` rows;
- the harness `cat`s the sentinel file.

Without it a refusal could be a broken request, which is why `BLIND` fails the run. Where a check
has a mitigation variant (below), the variant must flip it from `LEAK` to `ENFORCED`. That is the
second proof that the check can produce both verdicts.

**Exit code.** `probe.sh` exits 0 when no check is `BLIND` or `ERROR`. A `LEAK` does not fail the
run, because it is the measurement.

**Official against egret.** Each check runs on both images and gets a pair of verdicts:
- (LEAK, LEAK): both leak, a candidate follow-up;
- (ENFORCED, ENFORCED): both enforce;
- (LEAK, ENFORCED): an existing patch already covers this path. The report names which one;
- (ENFORCED, LEAK): a patch broke something. That is a bug in the fork, reported at once.

**Promotion.** When a follow-up patch lands, its probe checks move into `checks.py`, tagged with
the patch number, in the existing "official fails / egret passes" form.

## Harness changes in the fork

Everything stays in the shape of `tests/e2e/run.sh`:
- one allin1 container with `access_control = ranger`;
- RustFS and the iceberg-rest fixture on a private Docker network;
- the fake Ranger on the host, serving `policies.py`.

The changes:
- **`tests/e2e/stack.sh` (new).** The start-up steps of `run.sh` become shell functions in it:
  network, RustFS, iceberg-rest, fake Ranger, the StarRocks container, the wait loops, the seed
  SQL and the AuditLoader. `run.sh` sources it, and its behaviour and output are unchanged. The
  guard: the same check lines, and the same N/N on the egret image, before and after.
- **`tests/e2e/sqlpaths.py` (new).** The path helpers of `checks.py` move here, and `checks.py`
  imports them. The helpers are `connect`, `norm`, `run`, `text`, `sql_prepare`, `binary`, `http`
  and `flight`, now taking their ports as parameters.
- **`tests/e2e/probe.sh <allin1-image>` (new).** It starts the stack under its own names and
  ports:
  - container prefix `egs-probe`, network `egs-probe-net`;
  - host ports 192xx, Flight ports 19508 and 19519.

  It then starts the optional services, seeds the probe objects, runs the probe files and prints
  a verdict summary. It and `run.sh` never run at the same time, because both use the fake Ranger
  port 18777 that `ranger-starrocks-security.xml` names. `KEEP=1` works as in `run.sh`.
- **`tests/e2e/probe_lib.py` (new).** It holds:
  - the `Verdict` enum and the sight-control logic;
  - the `Probe` enum, `udf`, `python-udf`, `hive`, `jdbc`, `paimon`, `stream-load`,
    `routine-load` and `in-namespace`;
  - the parsing of `PROBE_SKIP`.
- **Typed opt-out.** `PROBE_SKIP` is a comma list of `Probe` names. An unknown name stops the
  run with exit 2. Each skipped probe prints a `SKIP` line per check, and the summary counts skips
  apart from the other verdicts. The report must list every skip and its reason. Nothing else
  skips: a missing service is an `ERROR`, not a skip.
- **`tests/e2e/canary.py` (new).** A host HTTP server on port 18778. It:
  - serves files from a work directory under `/files/` (the UDF jar, the Postgres JDBC driver);
  - logs every other request's path and peer address to a file the checks read.

  It measures egress: from a UDF, from the FE fetching a `file` URL, and from a load source.
- **`tests/e2e/fakeranger.py`.** It re-reads the policy file on every request, so a probe can
  publish policy version 2 (a revoke) during a run. `run.sh`'s behaviour is unchanged, because
  its file never changes.
- **`tests/e2e/policies.py`.** It gains two flags; with neither, it writes exactly what it writes
  today:
  - `--probe` adds the probe users' policies: `udfdev`, `loader`, the catalog policies, and the
    function grants;
  - `--version 2 --revoke loader` writes version 2, without `loader`'s INSERT grants.
- **Optional services.** Each starts only when its probe is not skipped. The image tags are
  pinned in Task 1 after `docker manifest inspect` succeeds, and recorded in `probe.sh`:
  - Postgres, for `jdbc`;
  - a Hive Metastore, for `hive`. A derived image under `tests/e2e/images/hive-metastore/` is
    added only if the metastore needs the S3A jars to create an `s3://` table location;
  - Kafka in KRaft mode, for `routine-load`, with one PLAINTEXT and one SASL_PLAINTEXT (PLAIN)
    listener;
  - Paimon needs no service: it uses a filesystem warehouse on RustFS.
- **`tests/e2e/inns_checks.py` (new).** The item-4 checks. They run in a throwaway
  `python:3.12-slim` container on the probe network, use only the standard library, and get no
  credentials. The FE thrift calls are hand-encoded with TBinaryProtocol (the FE's `ThriftServer`
  uses `TBinaryProtocol.Factory`), with field ids taken from `gensrc/thrift/FrontendService.thrift`
  and `Types.thrift`.

**Mitigation variants.** Two environment switches make `probe.sh` start the stack with an
egret-side mitigation, so the report can say whether that mitigation works:
- `BE_HTTP_AUTH=1` appends `enable_http_auth = true` to `be.conf`;
- `UDF_JAVA_POLICY=1` adds `-Djava.security.policy=<a policy that grants nothing>` to `be.conf`
  `JAVA_OPTS`.

## Probe 1: Java and Python UDFs

**Setup.**
- The main checks run with `enable_udf` off, as on an egret engine. The UDF phase then appends
  `enable_udf = true` to `fe.conf` and restarts the StarRocks container. `probe.sh` waits again
  for the BE and for policy version 1.
- The harness writes a sentinel file, `/opt/egret-probe/sentinel.txt` (`egret-probe-sentinel-<RUN_ID>`),
  into the container.
- The jar `probe-udf.jar` is built from `tests/e2e/udf/` in a pinned JDK 17 container and served
  by the canary. Its scalar classes are:
  - `Echo`: returns its argument;
  - `ReadFile`: returns the first 200 bytes of a path, or `ERR:<exception class>`;
  - `Fetch`: makes an HTTP GET and returns the status, or `ERR:<exception class>`.

**Checks.**

| Id | Request | Egret-desired | Sight control |
|---|---|---|---|
| U0 | root `CREATE FUNCTION`, with `enable_udf` off (main phase) | refused, "UDF is not enabled" (H1) | U1's sight in the UDF phase |
| U1 | eve `CREATE FUNCTION demo.p_echo` | DENIED | udfdev creates it |
| U2 | eve `CREATE GLOBAL FUNCTION` | DENIED | root creates one |
| U3 | udfdev `CREATE FUNCTION` in `ice.sales` (no grant there) | DENIED | U1 sight |
| U4 | eve and bob call `demo.p_echo` (no USAGE) | DENIED | udfdev's call returns its argument |
| U5 | udfdev `p_read('/opt/egret-probe/sentinel.txt')` | `ERR:` with a security exception (H2) | `p_echo` runs; `docker exec cat` shows the sentinel |
| U6 | udfdev `p_fetch('http://host.docker.internal:18778/udf-egress-<RUN_ID>')` | no canary hit | the canary is reachable from the container (`docker exec` curl) |
| U7 | udfdev `p_fetch('http://127.0.0.1:8040/varz')`: the UDF reaching its own node's admin port | refused | N2 |
| U8 | udfdev `CREATE FUNCTION`, `file` = `s3://warehouse/probe/none.jar`, no keys (H3) | DENIED, as `0009` does | the same with own keys is not refused |
| U9 | udfdev `CREATE FUNCTION`, `file` = a canary URL (the FE fetches) | recorded: a canary hit is a finding of class "out of model" or "leak" | canary log |
| U10 | svc `EXECUTE AS udfdev`, U1 again | same as udfdev | U1 |

**Python UDFs.**
- **P0 measures** whether the image can run them at all:
  - `command -v python3` in the image;
  - `python3 -c 'import pyarrow'`;
  - the BE's `python_envs`, which defaults to empty.
- **P1** is U1 with an inline `LANGUAGE`/`type` Python function, per
  `CreateFunctionAnalyzer.analyzePython`: inline content, `symbol`, `input` = scalar.
- **P2 to P4** repeat U4 to U6 with Python. When P0 shows that Python cannot run, they are `N/A`
  with the measured reason. Creation (P1) is still measured.

**Classification guide.**
- U1 to U4 are expected to be enforced.
- If U5, U6 or U7 leak, then whoever may create a UDF has the node's identity. That is a leak
  whenever `enable_udf` can be on. The candidate follow-ups:
  - egret: add `enable_udf` to `SR_PROTECTED_FE_KEYS`, or refuse it in `feConfig`;
  - the `UDF_JAVA_POLICY` mitigation, if the variant run shows it closes U5 to U7.
- U8 leaking is a fork-patch candidate that extends `0009`.

## Probe 2: Hive, JDBC and Paimon catalogs

**Setup.** For each catalog `C` in `hive`, `jdbc` and `paimon`, the harness creates the catalog
**as root, after the AuditLoader is loading**, with a known secret in its properties:
- `jdbc`: the Postgres password `pgsecret`;
- `hive` and `paimon`: the RustFS keys.

It then seeds `C.sales.orders` with the same four rows as `ice.sales.orders`. The seeding differs
per catalog:
- **`jdbc`:** `psql` into Postgres.
- **`hive`:** StarRocks `CREATE TABLE` and `INSERT` through the catalog, as root. The table is
  partitioned by `region`.
- **`paimon`:** StarRocks 4.1.7 has no Paimon `createTable` (`PaimonMetadata`), so the seed is
  written outside StarRocks. Task 4 tries, in this order:
  1. `pypaimon` in the harness venv, writing the S3 filesystem warehouse;
  2. if that fails, recorded as a chosen boundary, with the reason (open question 4).

`policies.py --probe` gives `C.sales.orders` the policies of `ice.sales.orders`:
- alice reads all;
- bob and mallory get `MASK_NULL` on `email` and the row filter `region = 'us'`;
- eve gets nothing;
- mallory may also create materialized views in `demo`.

**Checks, per catalog** (`ORDERS_ALL` and `ORDERS_BOB` as in `checks.py`):

| Id | Request | Egret-desired | Covers |
|---|---|---|---|
| C1 | alice text | `ORDERS_ALL` | the sight control for C2 to C7 |
| C2 | bob text | `ORDERS_BOB` | the policy rewrite on this connector |
| C3 | bob `COM_STMT_EXECUTE` | `ORDERS_BOB` | `0002` on this connector |
| C4 | svc `EXECUTE AS bob` + `COM_STMT_EXECUTE` | `ORDERS_BOB` | the delegated path |
| C5 | bob HTTP SQL API | `ORDERS_BOB` | `0004` |
| C6 | bob Arrow Flight SQL | `ORDERS_BOB` | `0004` |
| C7 | bob `SELECT *` | exactly three columns, `ORDERS_BOB` | hidden columns (`0003`'s class) |
| C8 | eve `SELECT` | DENIED | grants |
| C9 | eve `SHOW CREATE CATALOG C` | DENIED | `0005` |
| C10 | bob `SHOW CREATE CATALOG C` | output without the full secret | the redaction of credentials |
| C11 | eve `SHOW DATABASES FROM C`, `SHOW TABLES FROM C.sales`, and `information_schema.tables` for `C` | nothing about `sales` or `orders` | metadata exposure; sight: alice sees them |
| C12 | bob `EXPLAIN COSTS` | DENIED | `0008` |
| C13 | mallory `CREATE MATERIALIZED VIEW` over `C.sales.orders` | DENIED | `0007` |
| C14 | connector metadata: Paimon `orders$files`, `$snapshots` and `$partitions` as bob (H7); Hive partition listing as bob, through whichever of `SHOW PARTITIONS` or `information_schema.partitions_meta` 4.1.7 accepts for the catalog | DENIED, or no `eu` partition or bound | `0001`'s class; sight: alice reads them |
| C15 | the audit table and `fe.audit.log` after the root `CREATE EXTERNAL CATALOG` | no row carries the secret | catalog secrets in the audit trail (package A reads that table) |
| C16 | eve `CREATE EXTERNAL CATALOG` (a JDBC catalog with a canary `driver_url`) | DENIED, and no canary hit | who may make the FE load a driver |

**Classification guide.**
- C2 to C13 leaking on a connector where Iceberg is enforced is a fork-patch candidate: the
  generic patch misses that connector.
- C14 leaking on Paimon is a `0001`-style patch candidate.
- C15 leaking is a candidate for both package A and the fork's AuditLoader patches.

## Probe 3: stream load and routine load

**Setup.**
- Tables:
  - `demo.s_target`, a duplicate-key table that is the target of the stream loads;
  - `demo.r_target`, the target of the routine loads;
  - `demo.pk_target`, a primary-key table for S10.
- Users:
  - `loader` has INSERT on the three tables and a row filter `id = 1` on `pk_target`;
  - `nat` gets a **native** `GRANT INSERT ON demo.s_target` from root, and no Ranger policy.

**Stream load checks.** Each one sends the request through the FE (8030) and also directly to the
BE (8040, published):
- The FE answers 307 with the BE's registered address. When that address does not resolve from
  the host (allin1 may register `127.0.0.1`), the client rewrites it to the published BE port,
  and the run records the rewrite.
- A refusal is `Access denied` in the load's JSON, or HTTP 401 or 403.

| Id | Request | Egret-desired | Sight control |
|---|---|---|---|
| S1 | eve stream load via the FE | DENIED | S4 |
| S2 | eve stream load directly to the BE (H8) | DENIED | S5 |
| S3 | bob (SELECT only) via the FE | DENIED | S4 |
| S4 | loader via the FE | rows loaded | the row count before and after |
| S5 | loader directly to the BE | rows loaded | the row count |
| S6 | nat SQL `INSERT`, then nat stream load to the BE | both DENIED: Ranger decides, not the native grant | if `GRANT` itself is refused under `access_control = ranger`, recorded as `N/A` with the error |
| S7 | eve `/api/transaction/begin` on the BE | DENIED | loader's begin succeeds |
| S8 | loader loads a malformed row that holds the marker `egret-errlog-<RUN_ID>`; eve then fetches the returned `ErrorURL` from the BE (H4) | refused | root fetches it and sees the marker |
| S9 | eve `SHOW LOAD` and `information_schema.loads` for loader's label | invisible | root sees the label |
| S10 | loader overwrites `id = 2` of `demo.pk_target`, a row her filter hides, by SQL `INSERT` and by stream load | the two paths agree | recorded: if both allow it, "out of egret's model" (row filters are read-side, as in Trino) |
| S11 | the audit table after S4 and S5 | one row per load, with user `loader` | recorded for package A: a missing row is an audit gap |

**Routine load checks.** A Kafka topic `probe-r` is fed by `kafka-console-producer.sh` through
`docker exec`.

| Id | Request | Egret-desired | Sight control |
|---|---|---|---|
| R1 | eve `CREATE ROUTINE LOAD` on `demo.r_target` | DENIED | R3 |
| R2 | bob, likewise | DENIED | R3 |
| R3 | loader creates job `r_plain` (PLAINTEXT); 3 messages are produced | 3 rows within 60 s | itself |
| R4 | loader creates job `r_sasl` (SASL PLAIN, password `kafkasecret`); 3 messages | 3 rows | itself |
| R5 | eve and bob `SHOW ROUTINE LOAD`, `SHOW ALL ROUTINE LOAD` and `information_schema.routine_load_jobs` | loader's jobs invisible | root sees them |
| R6 | any output visible to eve or bob, the audit table and `fe.audit.log` | no `kafkasecret` | root's `SHOW ROUTINE LOAD` (shows whether StarRocks redacts it at all) |
| R7 | revoke: the fake Ranger serves version 2 without loader's INSERT; `probe.sh` waits for `lastKnownVersion=2` in `ranger.log`; 3 more messages | no new rows; the job pauses or stops | R3's rows arrived before the revoke |
| R8 | eve `PAUSE` and `STOP ROUTINE LOAD` on loader's job | DENIED | loader may pause her own |

**Classification guide.**
- S2, S6 or S7 leaking means a write path that bypasses Ranger: a fork-patch candidate.
- S8 and S9 are exposures of other users' data: fork-patch candidates.
- R7 leaking is the `0007` pattern on loads: a fork-patch candidate.
- S10 is expected to be out of the model.

## Probe 4: CN/BE ports from inside the engine namespace

**Position.** A container on the probe network, with no StarRocks credentials. This is the M5
attacker: the NetworkPolicy admits every pod of the engine namespace on every port.
- **allin1 harness:** the BE in allin1 runs the same binary and HTTP service as a CN
  (`starrocks_main.cpp` `--cn`; `http_service.cpp`), but in shared-nothing mode.
- **Kind check:** shared-data specifics (`LakeService` on brpc, no local tablets) are measured
  only by the optional kind check (Task 7).

| Id | Request (no credentials) | Egret-desired | Sight control |
|---|---|---|---|
| N1 | `GET :8040/api/health` | 200 | the reachability control for N2 to N11 |
| N2 | `GET :8040/varz` | 401 (H4) | `BE_HTTP_AUTH=1` flips it |
| N3 | `GET :8040/greplog?pattern=<S4 label>` | 401 | `docker exec grep` finds the label in `be.INFO` |
| N4 | `POST :8040/api/update_config?enable_token_check=false` | 401 (H5) | the `varz` value before and after |
| N5 | `GET :8040/api/_download_load?file=<a segment file of demo.customers>`, without a token, before and after N4 | refused both times | the harness finds the path as root (`SHOW TABLET` + `find`) and compares sha256 |
| N6 | `GET :8060/flags` and `/vars` (brpc built-in services) | not served | recorded |
| N7 | TCP connect to 9060, 9050, 8060, 9070 and 9020 | recorded: reachability only | none |
| N8 | FE thrift 9020 `getDbNames` with `current_user_ident = root`, then with `eve` (H6) | no `demo` for either | the `eve` call returns no `demo`, so a `demo` for root shows the identity is trusted |
| N9 | FE thrift 9020 `forward` (a `TMasterOpRequest` as root, `SELECT 1`) | rejected, "request from invalid host" | expected enforced, a control |
| N10 | `GET :8030/api/show_proc?path=/backends`, and `GET :8030/metrics` | 401; metrics recorded (whether they name tables or users) | none |
| N11 | `GET :8040/api/_stop_be`, **last, and only in the throwaway allin1 container** | 401 | the BE's heartbeat stops in `SHOW BACKENDS` |

N4 is restored (`enable_token_check=true`) right after N5.

**Mitigation variant `BE_HTTP_AUTH=1`.**
- Without credentials, N2 to N5 and N11 must flip to `ENFORCED`.
- With eve's credentials, the OPERATE endpoints must stay refused.
- S8 is measured again: with auth on, the error log needs only a login.

**Classification guide.**
- N2 to N5 and N11 leaking with the default config, and enforced with `BE_HTTP_AUTH=1`: an egret
  change is the candidate. The provisioner writes `enable_http_auth = true` into `cn.conf`.
  Task 7 also measures whether the operator's probes and FE-to-CN calls still work.
- N8 leaking is not reachable by a config switch. The candidates are a fork patch (check the
  caller is a known FE or BE, as `forward` does) and a NetworkPolicy that admits 9020 only from
  FE and CN pods.
- N6 and N7 are recorded. Forging brpc or BE thrift calls (`exec_plan_fragment`, `LakeService`)
  is out of this package: it needs StarRocks' generated stubs, and its outcome is decided by the
  same network boundary as N8.

## Where results go

- **Raw output:** both runs' full output, the official and the egret image, and both variants, go
  into the measuring session's scratchpad. The report names the files and the commands, as the
  SP2b latency report does.
- **Report:** `docs/en/reports/2026-11-09-starrocks-unmeasured-paths-probe.md`. It holds:
  - one table row per check, with the official verdict, the egret verdict, the class and the
    follow-up id;
  - every `SKIP`, `N/A` and chosen boundary, each with its reason;
  - the measured image digests and service tags.
- **Fork README:** a "Probe" section with the date, the images and the verdict counts, in the
  style of the existing result paragraphs.

## From finding to follow-up

Each `LEAK` gets a follow-up id (`F1`, `F2`, ...) in the report, with:
- the class;
- the candidate fix: fork patch `00NN` from `0010` on, an egret change, or a known issue;
- the checks that will become its gate.

Each follow-up gets its own spec before any code. The order of the follow-ups is the user's
decision once the report is in. "Out of model" findings go to `egret-data/docs` Known issues,
under the next free KI numbers, in a docs PR of this package.

## Out of scope

- Writing patches, or egret code, for any finding.
- Re-probing `0001` to `0009` on Iceberg.
- Delta Lake, Hudi, Elasticsearch and other catalog types. Each can be added later as one more
  `Probe` value with the same checks.
- Forged brpc and BE-thrift calls (see probe 4).
- Pulsar routine load, Spark load, and `INSERT` from table functions other than `FILES()`.
- Egress control from the engine: the NetworkPolicy restricts only ingress; that is a separate
  decision.
- External StarRocks engines: their operator owns their config.

## Open questions

1. **Disclosure: commit leak checks to the public fork now?** The fork is public. A check that
   shows an unpatched leak is a public disclosure, and `0001` was reported privately first.
   - (a) Push every probe check as written.
   - (b) Push the harness and the checks that come out enforced or out of model. Keep the checks
     of confirmed leaks on a local branch until each is reported to the StarRocks security team
     and its patch is ready.
   - (c) Keep the whole probe local.

   **Recommendation: (b).** The same applies to the platform report if this repository is public:
   the leak rows would then wait for the same moment.
2. **Kafka, Hive Metastore and Postgres in the fork CI, or local only?** They add start-up time
   and image pulls to every fork build.
   - (a) Add them all to CI.
   - (b) Local only: `probe.sh` is not in CI. Only the services that a promoted check needs join
     CI, with its patch.

   **Recommendation: (b).**
3. **Python UDFs if the image has no Python.**
   - (a) Measure creation and record execution as `N/A`, with P0's measured reason.
   - (b) Build a probe-only image with Python and `pyarrow`, and set `python_envs`.

   **Recommendation: (a).** egret's CN is the official image, so (a) measures what egret runs.
   Revisit only if an admin path to `python_envs` appears.
4. **Paimon seed if `pypaimon` cannot write the S3 warehouse.**
   - (a) Add a Spark or Flink container just to write it.
   - (b) Record Paimon's write-dependent checks as a chosen boundary, keep C9, C11, C15 and C16,
     which need no rows, and open a follow-up.

   **Recommendation: (b)** unless the user wants Paimon covered fully now. C14 (H7) is the
   Paimon check that matters, and it needs at least one snapshot.
5. **The kind check of probe 4 (Task 7).**
   - (a) Run it: a provisioned shared-data engine, a pod in `egret-engines`, and a pod in another
     namespace as the NetworkPolicy control.
   - (b) Rely on allin1.

   **Recommendation: (a).** The CN's shared-data services and the real NetworkPolicy exist only
   there. Skip N11 there: it would stop an engine.
6. **Mitigation variants in this package.**
   - (a) Run `BE_HTTP_AUTH=1` and `UDF_JAVA_POLICY=1` now.
   - (b) Leave them to the follow-up specs.

   **Recommendation: (a).** Each is one config line, and it is what proves the checks can flip.

## Rulings (2026-11-12)

The user answered the open questions:

1. **(b)**: the harness and the enforced or out-of-model checks may be pushed; the checks of
   confirmed leaks stay on a local branch until each is reported and its patch is ready.
2. **(b)**: local only; `probe.sh` is not in the fork CI.
3. **(a)**: measure creation, and record execution as `N/A` with P0's measured reason.
4. **(b)**: if `pypaimon` cannot write the S3 warehouse, the write-dependent Paimon checks are a
   chosen boundary with a follow-up; no Spark or Flink.
5. **(a)**: run probe 4 on kind (Task 7), without N11.
6. **(a)**: run both mitigation variants in this package.
