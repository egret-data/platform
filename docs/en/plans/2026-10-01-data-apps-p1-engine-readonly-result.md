# Data Apps P1 - engine-side read-only session: measurement result

Spec section 4.3 asks whether the delegated (per-user) connection can also be put
into a read-only mode on each engine, as defence in depth behind the gateway's
fail-closed parser. Everything below was run **manually** (kubectl, docker and the
engine CLIs), on 2026-10-01. Nothing here is an automated test and no product code
was changed.

## Summary

| Engine | Available | Mechanism | Recommendation |
| --- | --- | --- | --- |
| Trino 470 | Yes, per transaction (not a session property) | `START TRANSACTION READ ONLY`, then the statement in that transaction | Follow-up (not adopted in P1): would need a transaction round-trip in the connector (see below); parser + Ranger remain the primary gate |
| StarRocks 4.1.3 | No (variable is accepted and ignored) | none | Don't. Rely on the parser plus Ranger. **Do NOT add `SET transaction_read_only` for StarRocks: `SHOW VARIABLES` reports it as true, and it enforces nothing - INSERT and DELETE still succeed.** |

## Where each measurement ran (and what could not)

- **StarRocks**: the kind cluster FE `sp3c2-sr-live-fe-0` (namespace `egret-engines`),
  `mysql` client inside the FE container, `select current_version()` returned
  `4.1.3-8a8e186`. Logged in as `root` (the FE's local client). Session variables are
  not permission-gated, so this answers the question; it does not exercise the JWT
  login path.
- **Trino**: NOT on the kind cluster. `sp3c2-trino-live-coordinator` runs
  `http-server.authentication.type=JWT` with `allow-insecure-over-http=false`;
  `kubectl exec ... trino --execute "SHOW CATALOGS"` against `localhost:8080` returned
  `403 Forbidden: Authentication over HTTP is not enabled`, and I had no
  egret-minted JWT to present over 8443. The `demo-trino-coordinator` pod was
  crash-looping (exec exited 137). So I measured the engine feature on a throwaway
  `trinodb/trino:470` container (the version the cluster runs: the kind coordinator
  CLI reports `Trino CLI 470`) with only a `memory` and a `tpch` catalog and no
  access control. The read-only-transaction check is in Trino's core transaction
  manager, so it is not connector- or auth-specific, but it was **not** run against
  the cluster's Iceberg catalogs or behind Ranger.

## Trino 470

### Session properties: none

```text
trino> SHOW SESSION          -- filtered for read|only|transaction|write|insert|allow
```

Only writer-tuning properties matched (`scale_writers`, `redistribute_writes`,
`max_writer_task_count`, ...). Nothing makes a session read-only.

```text
$ trino --session read_only=true --execute "SELECT 1"
Query ... failed: Unknown session property read_only
$ trino --execute "SET SESSION mem.read_only = true"
Query ... failed: line 1:29: Session property mem.read_only does not exist
```

The `memory` connector has no such property either. No session-level switch exists.

### `START TRANSACTION READ ONLY`: writes are rejected

CLI, one session, `--ignore-errors --file`:

| Statement inside `START TRANSACTION READ ONLY` | Engine response |
| --- | --- |
| `INSERT INTO mem.p1ro.t VALUES (2)` | `Cannot execute write in a read-only transaction` |
| `CREATE TABLE mem.p1ro.t2 (id int)` | `Cannot execute write in a read-only transaction` |
| `DROP TABLE mem.p1ro.t` | `Cannot execute write in a read-only transaction` |
| `CREATE SCHEMA mem.p1ro2` | `Cannot execute write in a read-only transaction` |
| `CREATE TABLE mem.p1ro.t3 AS SELECT 1 x` | `Cannot execute write in a read-only transaction` |
| `DELETE FROM mem.p1ro.t` | `This connector does not support modifying table rows` (this is the memory connector's own limit, not the transaction check; unproven for DELETE on a connector that supports it) |
| `SELECT * FROM mem.p1ro.t` | returns the row; `COMMIT` succeeds |

After the rejected writes, `SELECT` and `SHOW TABLES` showed the table unchanged
(no `t2`, `t3`, `p1ro2`).

### Over HTTP (the way egret talks to Trino)

The transaction must be requested explicitly and carried by the client:

```text
POST /v1/statement   X-Trino-User: u                          body: START TRANSACTION READ ONLY
  -> error "Client does not support transactions"
POST /v1/statement   X-Trino-User: u, X-Trino-Transaction-Id: NONE   body: START TRANSACTION READ ONLY
  -> response header X-Trino-Started-Transaction-Id: 708aa0d6-fdf5-48ba-80be-a4aa36af434a
POST /v1/statement   ... X-Trino-Transaction-Id: <that id>    body: INSERT INTO mem.p1ro.t VALUES (9)
  -> "Cannot execute write in a read-only transaction"
POST /v1/statement   ... X-Trino-Transaction-Id: <that id>    body: SELECT ...
  -> "Current transaction is aborted, commands ignored until end of transaction block"
POST /v1/statement   ... X-Trino-Transaction-Id: <that id>    body: ROLLBACK
```

Negative control: an `INSERT` sent **without** a transaction id is autocommit and
succeeds. Note that a first attempt of mine sent the INSERT with an empty transaction
id because the `START TRANSACTION` had failed, and the row was written; the
protection exists only when the id is actually carried.

### Usability from egret's connector path

`TrinoConnector` is stateless per request: it POSTs `/v1/statement` with
`X-Trino-Session` (`query_max_run_time`) and, for parameterised statements,
`X-Trino-Prepared-Statement` (`crates/egret-query/src/execution/connector/trino.rs:460-470`
for `execute`, `:572-582` for the describe path, `:660-661` for the other prepared
path). It never sends `X-Trino-Transaction-Id` or reads `X-Trino-Started-Transaction-Id`
(grep of `Transaction` in `trino.rs` finds nothing outside the comments above).
Adopting this means: a `START TRANSACTION READ ONLY` request with
`X-Trino-Transaction-Id: NONE`, capture the started id, send the real statement with
it, then `ROLLBACK`, all on the same per-user authenticated client (the transaction is
bound to the user and, in a multi-coordinator deployment, to the coordinator that
started it). Not measured: cost, behaviour when `ROLLBACK` is lost (idle transactions
expire server-side), and interaction with `X-Trino-Prepared-Statement`.

**Recommendation: needs follow-up.** The engine behaviour is real and cheap for the
engine, but egret would have to add transaction handling to the HTTP client, and the
cluster's Iceberg catalogs were not tested. Defer to a P3/P4 task if the extra
round-trips are acceptable; until then the parser plus Ranger are the gate.

## StarRocks 4.1.3

**Do NOT add `SET transaction_read_only` for StarRocks: `SHOW VARIABLES` reports it as true, and it enforces nothing - INSERT and DELETE still succeed.**

Scratch database `p1_ro_probe_28978` created by me, with table `t (id INT)`. Each
statement was run in one `mysql` session, `-vvv --force`:

| Statement | Engine response | Following `INSERT` |
| --- | --- | --- |
| `show variables like '%read_only%'` | `innodb_read_only true`, `transaction_read_only false` | n/a |
| `SET SESSION transaction_read_only = 1` | `Query OK`; `show variables like 'transaction_read_only'` then shows `true` | `INSERT INTO ...t VALUES (1)` -> `Query OK, 1 row affected ... 'status':'VISIBLE'` (not rejected) |
| `SET transaction_read_only = ON` | `Query OK` | `VALUES (2)` -> `1 row affected`, `VISIBLE` |
| `SET TRANSACTION READ ONLY` | `Query OK` | `VALUES (3)` -> `1 row affected`, `VISIBLE` |
| `SET SESSION tx_read_only = 1` | `Query OK` | `VALUES (4)` -> `1 row affected`, `VISIBLE` |
| `SET SESSION read_only = 1` | `ERROR 1193 ... Unknown system variable 'read_only'` | `VALUES (5)` -> `1 row affected`, `VISIBLE` |
| `SET SESSION innodb_read_only = 1` (after the above) | `Query OK` (accepted) | not retested with an INSERT |
| `SET SESSION query_timeout = 5` | `Query OK`; `show variables like 'query_timeout'` -> `5` | n/a |

`select id from ...t` then returned all five rows (1 to 5). With
`transaction_read_only = 1` still set, `DELETE FROM ...t WHERE id=5` also succeeded
(`delete_... VISIBLE`) and `count(*)` dropped to 4. So `transaction_read_only` and its
aliases exist for MySQL client compatibility and are **stored but not enforced**.
`innodb_read_only` is a global that already shows `true`; I set it at session level but did not retest with a write (unmeasured).

Usable from egret: the connector opens `mysql_async::Conn::new` per `execute` and
already issues one session statement (`EXECUTE AS` ... `WITH NO REVERT`,
`crates/egret-query/src/execution/connector/starrocks.rs:224-228`, called from
`execute` at `:251-262`), so a `SET` could be added there trivially, but it would
protect nothing. A user connection can also `SET` the variable back. `query_timeout`
is settable the same way if egret ever needs it.

**Recommendation: don't.** No enforcing mechanism exists in 4.1.3; rely on the
parser plus Ranger (StarRocks Ranger enforcement is already live).

## Cleanup (all manual)

- StarRocks: `DROP DATABASE p1_ro_probe_28978 FORCE` -> `Query OK`;
  `show databases like 'p1_ro_probe%'` -> `Empty set`. No existing table touched.
- Trino: `DROP TABLE mem.p1ro.t`, `DROP SCHEMA mem.p1ro` -> `DROP TABLE`/`DROP SCHEMA`;
  `SHOW SCHEMAS FROM mem` -> `default`, `information_schema`; `docker rm -f p1-ro-trino`,
  `docker ps -a | grep p1-ro` empty. Only the memory catalog of the throwaway
  container was written to (rows 9, 10, 10 from my own protocol probes).
- Left behind: the pulled image `trinodb/trino:470` (and the pre-existing `latest`)
  in the local Docker image cache; scratch files under the session scratchpad.
- Kind cluster: only the rejected `SHOW CATALOGS` and read-only `SHOW SESSION` attempts
  against the Trino coordinator, and the StarRocks probe above.
