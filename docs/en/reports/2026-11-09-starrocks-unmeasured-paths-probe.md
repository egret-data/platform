# StarRocks unmeasured paths: enforcement probe, measured (2026-11-09)

This report measures whether egret's access model is enforced on four StarRocks paths that
the existing fork patches (`0001`–`0009` + AuditLoader) and the earlier differential tests
never exercised: non-Iceberg catalogs (JDBC, Hive, Paimon), stream and routine load, and the
CN/BE/FE ports reached from inside the engine's network namespace. The fifth path, Java and
Python UDFs, was **dropped** before measurement (see Boundaries).

Each path is probed on two images — the **official** `starrocks/allin1-ubuntu:4.1.7` and the
**egret fork** `ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4` — and the egret image
is probed a second time with the candidate mitigation `enable_http_auth = true`
(`BE_HTTP_AUTH=1`). Every check prints one verdict from a fixed enum
(`ENFORCED`, `LEAK`, `BLIND`, `N/A`, `SKIP`, `ERROR`; see `tests/e2e/probe_lib.py` in the fork).
`BLIND` (a check that cannot see) and `ERROR` (a check that broke) fail the run; all three runs
below finished with **zero `BLIND` and zero `ERROR`**.

A `LEAK` is a demonstrated disclosure or bypass, not a suspicion: each leak check has a
**sight control** that proves the probe could have seen enforcement if it were there (an
authorised user reads the same object, or a refusal is distinguished from a login failure).

## Raw files

The raw outputs are kept outside the repository, in the measuring session's scratchpad,
`/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/pkgc/`
(named `RAW/` below). Each run re-runs on any `egs-probe-*` stack with the fork's
`tests/e2e/probe.sh`.

| File | Command |
|---|---|
| `RAW/probe-official.txt` | `tests/e2e/probe.sh starrocks/allin1-ubuntu:4.1.7` |
| `RAW/probe-egret.txt` | `tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4` |
| `RAW/probe-egret-mitigated.txt` | `BE_HTTP_AUTH=1 tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4` |
| `RAW/kind-inns.txt`, `kind-inns-default-control.txt`, `kind-inns-auth.txt`, `kind-cn-probe-break.txt` | probe 4 on a real kind engine (see "Probe 4 on kind") |

The probe files that carry a confirmed leak (`probe_catalogs.py`, `probe_loads.py`,
`inns_checks.py`) are held **local only** on the fork branch `test/probe-unmeasured-paths` and
are **not pushed** until the findings here are read and the follow-ups below are decided
(spec open question 1, ruling b).

## Pins

| Thing | Pin |
|---|---|
| Official image | `starrocks/allin1-ubuntu:4.1.7` @ `sha256:7f6041f6d537162a2627668565630e3204756f5e85b60944de3acc2d3abd96f8` |
| Egret image | `ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4` @ `sha256:bcb14c9013857bb6ca8c687142fda16be80444f6cfa04f9ddca4a93a4e450376` |
| Fork patches on egret.4 | `0001`–`0009` + AuditLoader (`AuditLoader.zip`) |
| StarRocks version | `4.1.7-b81ff9c` |
| Services | Hive Metastore (catalog), PostgreSQL (JDBC catalog), MinIO/RustFS (Paimon), Kafka (routine load), the fake Ranger (`policies.py --probe`), the egress canary (`canary.py`) |

The kind run (probe 4 on a real engine) used the fork FE
`ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4` and the official CN
`starrocks/cn-ubuntu:4.1.7` — the provisioner's shipped default for a shared-data engine.

## Verdict counts

| Run | ENFORCED | LEAK | BLIND | N/A | ERROR |
|---|---|---|---|---|---|
| official `4.1.7` | 47 | 34 | 0 | 3 | 0 |
| egret `4.1.7-egret.4` | 68 | 14 | 0 | 2 | 0 |
| egret + `enable_http_auth` | 81 | 8 | 0 | 2 | 0 |

The fork closes **20 of the 34** official leaks outright (the catalog policy-rewrite family,
all of C3–C13 on every connector). `enable_http_auth` closes **6 more** (the anonymous BE-HTTP
family). **Eight leaks survive both**; they are the subject of the follow-ups.

## Hypotheses

The eight hypotheses from the 4.1.7 source (spec §"Hypotheses from the 4.1.7 source"), each
confirmed or left unmeasured, with the checks that decide it.

| Id | Verdict | Evidence |
|---|---|---|
| H1 | **not measured** | `CREATE FUNCTION` refused while `enable_udf` off. The UDF probe was dropped; source-consistent only (`CreateFunctionAnalyzer.analyze`). |
| H2 | **not measured at runtime** (source-confirmed) | Java UDF runs with no SecurityManager unless `java.security.policy` is set. Confirmed by reading `UDFClassLoader.java`; not run (UDF probe dropped). |
| H3 | **not measured** (source-confirmed) | FE fetches the `file` URL at `CREATE FUNCTION` before authorisation. Confirmed by reading `CreateFunctionAnalyzer.setupCredential`; recorded as a follow-up. |
| H4 | **confirmed** | BE HTTP serves `/varz`, `/greplog`, `/api/update_config`, `/api/_stop_be`, `/api/_load_error_log` with no login while `enable_http_auth` is off: N2, N3, N4, N5, N11, S8 all LEAK. With auth on they flip to ENFORCED, **except** `/api/_load_error_log`, which needs only a login — S8‑eve still LEAK. |
| H5 | **confirmed** | `enable_token_check` is mutable: an unauthenticated `update_config` (N4) turns it off, then `/api/_download_load` serves tablet bytes with no token (N5‑after LEAK; the leak is a sha256 match against the real segment file). Both flip under `enable_http_auth`. |
| H6 | **confirmed, both halves** | FE thrift `getDbNames` trusts the request-supplied `current_user_ident` (N8 LEAK: `root` sees 4 system dbs, `eve` sees `[]` — the identity is believed). `forward` refuses a non-FE caller (N9 ENFORCED: "request from invalid host"). |
| H7 | **confirmed** | Paimon system tables (`t$files`, `t$snapshots`, `t$partitions`) resolve in the Paimon catalog, not as StarRocks `MetadataTable`s, so `0001`'s check never reaches them: C14‑paimon LEAK on **both** images. A `table = *` grant reads them under no mask or row filter. The `table = orders`-only variant is DENIED on both images — the engine authorises system tables under their own name, and the `*` grant carries no filter/mask. |
| H8 | **confirmed (control)** | The BE accepts a stream load with no login of its own and asks the FE at transaction begin: eve's direct-to-BE load (S2) is refused at begin, loader's (S5) succeeds. The enforcement path works. |

## Checks

One row per check. **Class** is the finding's relation to egret's model: *enforced* (egret
enforces it, or fixes it versus official), *leak* (a disclosure egret should close), *out of
model* (egret never promised to enforce it; an operator control answers it). **Follow-up** names
the row's entry in the follow-ups table, if any.

Catalog checks C1–C13 and C15–C16 ran on **all three** connectors (JDBC, Hive, Paimon) with
identical verdicts; one row stands for the three. C14 differs by connector and is split out.

### Catalogs (probe 2)

| Check | Official | Egret | Mitigated | Class | Follow-up |
|---|---|---|---|---|---|
| C1 alice text (sight control) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C2 bob text | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C3 bob prepared (`COM_STMT_EXECUTE`) | LEAK | ENFORCED | ENFORCED | enforced (fork `0002`) | — |
| C4 svc `EXECUTE AS bob` + prepared | LEAK | ENFORCED | ENFORCED | enforced (delegation) | — |
| C5 bob HTTP SQL API | LEAK | ENFORCED | ENFORCED | enforced (fork `0004`) | — |
| C6 bob Arrow Flight SQL | LEAK | ENFORCED | ENFORCED | enforced (fork `0004`) | — |
| C7 bob `SELECT *` (hidden columns) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C8 eve `SELECT` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C9 eve `SHOW CREATE CATALOG` secret | LEAK | ENFORCED | ENFORCED | enforced (fork `0005`) | — |
| C10 bob `SHOW CREATE CATALOG` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C11 eve `SHOW DATABASES/TABLES`, `information_schema` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C12 bob `EXPLAIN COSTS` | LEAK | ENFORCED | ENFORCED | enforced (fork `0008`) | — |
| C13 mallory `CREATE MATERIALIZED VIEW` | LEAK | ENFORCED | ENFORCED | enforced | — |
| C14‑jdbc / C14‑hive partition listing | N/A | N/A | N/A | n/a (no listing path on 4.1.7) | — |
| **C14‑paimon** `$files/$snapshots/$partitions` | **LEAK** | **LEAK** | **LEAK** | leak | FU‑1 |
| C15 audit table + `fe.audit.log` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| C16 eve `CREATE EXTERNAL CATALOG` (canary) | ENFORCED | ENFORCED | ENFORCED | enforced | — |

C14‑paimon is the one catalog leak the fork does not already close. A user with a column mask
and a row filter on the base table reads the partition values (including the hidden `eu`
partition), the data-file paths and the per-partition record counts from the Paimon system
tables.

### Stream and routine load (probe 3)

| Check | Official | Egret | Mitigated | Class | Follow-up |
|---|---|---|---|---|---|
| S1 eve stream load via FE | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| S2 eve stream load direct to BE | ENFORCED | ENFORCED | ENFORCED | enforced (H8) | — |
| S3 bob (SELECT only) via FE | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| S4 loader via FE | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| S5 loader direct to BE | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| S6 nat SQL `INSERT` + stream load | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| S7 eve `/api/transaction/begin` on BE | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| **S8‑eve** fetch load `ErrorURL` (logged in) | LEAK | LEAK | **LEAK** | leak | FU‑4 |
| S8‑anon fetch load `ErrorURL` (anonymous) | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |
| **S9** eve `SHOW LOAD` / `information_schema.loads` | LEAK | LEAK | LEAK | leak | FU‑5 |
| S10 loader overwrites a filter-hidden row | ENFORCED | ENFORCED | ENFORCED | out of model | FU‑10 |
| **S11** stream/routine loads in the audit table | N/A | LEAK | LEAK | out of model (audit gap) | FU‑9 |
| R1 eve `CREATE ROUTINE LOAD` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| R2 bob `CREATE ROUTINE LOAD` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| R3 loader `r_plain` (PLAINTEXT) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| R4 loader `r_sasl` (SASL PLAIN) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| R5 eve/bob `SHOW ROUTINE LOAD` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| R6 `kafkasecret` in any visible output | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| **R7** routine load keeps loading after revoke | LEAK | LEAK | LEAK | leak | FU‑2 |
| R8 eve `PAUSE`/`STOP` loader's job | ENFORCED | ENFORCED | ENFORCED | enforced | — |

S8‑anon is the only load check `enable_http_auth` closes; the logged-in fetch (S8‑eve) and the
load-listing visibility (S9) survive. S11 is an audit-coverage gap, not a data leak — it belongs
to package A. On the official image S11 is N/A because that image ships no AuditLoader.

### CN/BE/FE ports from inside the namespace (probe 4)

| Check | Official | Egret | Mitigated | Class | Follow-up |
|---|---|---|---|---|---|
| N1 `GET :8040/api/health` (reach control) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| **N2** `GET :8040/varz` | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |
| **N3** `GET :8040/greplog` (S4 label) | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |
| **N4** `POST :8040/api/update_config` (token check) | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |
| **N5** `GET :8040/api/_download_load` (no token) | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |
| **N6** `:8060/flags`, `/vars` (brpc built-ins) | LEAK | LEAK | **LEAK** | out of model | FU‑6 |
| N7 TCP connect 9060/9050/8060/9070/9020 | ENFORCED | ENFORCED | ENFORCED | enforced (reach map) | — |
| **N8** FE thrift `getDbNames` (identity trust, H6) | LEAK | LEAK | **LEAK** | out of model | FU‑7 |
| N9 FE thrift `forward` (host auth, H6) | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| N10‑show `GET :8030/api/show_proc` | ENFORCED | ENFORCED | ENFORCED | enforced | — |
| **N10‑metrics** `GET :8030/metrics` (unauth) | LEAK | LEAK | **LEAK** | out of model | FU‑8 |
| **N11** `GET :8040/api/_stop_be` | LEAK | LEAK | ENFORCED | enforced under auth | FU‑3 |

N6, N8 and N10‑metrics are the three in-namespace leaks `enable_http_auth` does not cover: the
brpc port (8060) and the FE HTTP/thrift ports (8030/9020) are outside the BE-HTTP auth gate. The
compensating control for all three is the **engine NetworkPolicy**, which probe 4 on kind proves
is the real boundary (below).

## Probe 4 on kind: the NetworkPolicy is the boundary, and the `enable_http_auth` caveat

Probe 4 also ran against a **real egret-provisioned shared-data engine** on `kind-egret`
(`RAW/kind-*.txt`), in the engine namespace `egret-engines` and, as a control, from namespace
`default`.

- **The NetworkPolicy isolates the engine.** From `egret-engines`, N1 answers `200` and all five
  engine ports are open; from `default`, N1 **times out** and all five ports are closed. The
  in-namespace leaks (N2, N6, N8, N10‑metrics) are a property of the engine namespace, not of
  "any pod on the cluster". The policy works as intended.
- **The `enable_http_auth` mitigation breaks the CN health probes.** Setting
  `enable_http_auth = true` on a real CN flips the anonymous HTTP leaks to `401` (good) but also
  401s the CN container's **startup, liveness and readiness probes**, which are all an
  unauthenticated `httpGet /api/health:8040` (confirmed in the pod spec,
  `RAW/kind-cn-probe-break.txt`: `Startup probe failed: HTTP probe failed with statuscode: 401`
  ×24). The CN went `0/1 NotReady`, was dropped from its Service, and would CrashLoop after the
  5-minute startup window. Reverting restored the CN to `1/1 Ready` in ~10 s.

**Consequence for FU‑3:** the candidate mitigation "the provisioner writes
`enable_http_auth = true` into `cn.conf`" **cannot ship alone**. It must be paired with a CN
health-probe change — Basic-auth credentials on the probe, a `tcpSocket`/`exec` probe, or an
auth-exempt health path — or the CN never reaches Ready.

## Boundaries

Every `SKIP`, `N/A` and chosen boundary, with its reason.

| Boundary | Reason |
|---|---|
| **Probe 1 (Java/Python UDFs) dropped** | Chosen boundary (2026‑11‑12): the UDF probe was removed from the package. One follow-up is recorded from source reading — the FE fetches the UDF jar at `CREATE FUNCTION` **before** authorisation (`StmtExecutor` analyses before the Authorizer), so a refused caller still causes the FE to fetch the `file` URL (FU‑11). H1–H3 are therefore source-confirmed only, not measured. |
| N4, N5, N11 destructive | Run only in the throwaway allin1 container; skipped on kind with `--no-destructive` (an unauthenticated OPERATE call must never stop a shared engine). Spec open question 5, ruling a. |
| C14‑jdbc, C14‑hive = N/A | 4.1.7 exposes no partition-listing path for JDBC or Hive catalogs that alice (the sight control) can read: `SHOW PARTITIONS` errors on a non-OLAP table, `information_schema.partitions_meta` is DENIED unqualified and absent catalog-qualified. Nothing to probe, so N/A rather than a false ENFORCED. |
| S10 = ENFORCED but "out of model" | The two write paths (SQL `INSERT` and stream load) agree, as the spec requires. Row filters in StarRocks (as in Trino) are **read-side**: a user whose filter hides a row may still overwrite it by writing. This is egret's model, not a bypass — recorded, not fixed (FU‑10). |
| Paimon statistics leak unmeasured | pypaimon writes null value statistics, so C14‑paimon shows partitions, paths and counts but not column min/max. A Java/Flink-written table would likely expose `email` min/max through `$files`; measuring it needs Spark or Flink, excluded by spec open question 4 ruling b. |
| Probe services local only | The Hive/JDBC/Paimon/Kafka/Ranger/canary services are local to the measuring stack and are **not** added to the fork CI (spec open question 2 ruling b). |

## Follow-ups

| Id | Class | Candidate | Gate (checks that must read ENFORCED) | Severity |
|---|---|---|---|---|
| FU‑1 | leak | **Fork patch `0010`**: apply the base table's row filter and column mask to Paimon system tables (`$files/$snapshots/$partitions`), or deny them when a filter/mask applies. | C14‑paimon on **both** images | Medium — a masked/filtered user reads hidden partition values, file paths and record counts. |
| FU‑2 | leak | **Fork patch** (0007-style): a running routine load re-checks the user's privilege when the Ranger policy version advances, and pauses/stops on revoke. | R7 | Medium — a revoked user's routine load keeps ingesting until the job is touched. |
| FU‑3 | leak → egret change | Provisioner writes `enable_http_auth = true` into `cn.conf`/`be.conf` **and** pairs it with a CN health-probe change (see the kind caveat). Closes the anonymous BE-HTTP family. | N2, N3, N4, N5, N11, S8‑anon **and** the CN reaching `1/1 Ready` with an egret query succeeding | High for the leaks; the probe-break gate is the ship blocker. |
| FU‑4 | leak | Restrict `/api/_load_error_log` to the load's owner (egret change or fork patch); under `enable_http_auth` any logged-in user still fetches another user's error rows, which can echo source data. | S8‑eve under `enable_http_auth` | Medium. |
| FU‑5 | leak | Filter `SHOW LOAD` / `information_schema.loads` by privilege so a user sees only loads they may see. | S9 | Low–Medium — load **labels/metadata** of others' INSERT jobs, not row data. |
| FU‑6 | out of model | Disable the brpc built-in `/flags`,`/vars` services, or rely on the NetworkPolicy. Config disclosure only, reachable only inside the namespace. | N6 (or accept, NetworkPolicy-bounded) | Low. |
| FU‑7 | out of model | FE thrift (9020) trusts the caller-supplied `current_user_ident` for metadata calls (upstream StarRocks, H6). Hard to patch without upstream; the NetworkPolicy is the control. | N8 (or accept, NetworkPolicy-bounded) | Medium — metadata enumeration from inside the namespace. |
| FU‑8 | out of model | FE `/metrics` (8030) served unauthenticated; may name tables/users on a seeded engine. Network-restrict or accept. | N10‑metrics (or accept, NetworkPolicy-bounded) | Low–Medium. |
| FU‑9 | out of model (audit) | Stream/routine loads produce no audit-table row. Belongs to **package A** (audit coverage), not a data leak. | S11 | Low (audit completeness). |
| FU‑10 | out of model | Row filters are read-side; a filter-hidden row can be overwritten by a write. Egret's model, as in Trino. Document only. | — | Informational. |
| FU‑11 | boundary | At `CREATE FUNCTION` the FE fetches the `file` URL before authorisation, so a refused caller still triggers an FE-side fetch. Recorded from source; the UDF probe was dropped before measuring it. | — | Low (SSRF-shaped; needs `enable_udf` on). |

## What this says about the egret fork

On the measured paths, the egret fork image enforces egret's access model everywhere the
existing patches reach: every catalog policy-rewrite path (C3–C13, all connectors) that leaks on
the official image is closed on `egret.4`. The eight surviving leaks fall into three buckets:
two want new fork patches (FU‑1 Paimon system tables, FU‑2 routine-load re-auth), two want an
egret config/provisioner change (FU‑3 BE HTTP auth with the health-probe pairing, FU‑4 error-log
owner check), and the rest are reachable only from inside the engine namespace, where probe 4 on
kind shows the **NetworkPolicy is the real boundary** — the operator's control, not a per-request
gate egret ever promised.

No product or patch code was written for this probe. The next steps are the follow-ups above,
each gated by the exact checks that must turn ENFORCED.
