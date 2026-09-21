# SP3b — StarRocks on the catalog registry

**Status:** designed; extends `2026-09-19-catalog-registry-design.md` (SP3a merged in #587)
**Date:** 2026-09-21

SP3a put every catalog in `registry.catalogs` and taught Trino to read it. StarRocks was
deliberately refused (`catalog_apply::STARROCKS_NOT_YET`). SP3b removes that refusal.

## 1. Why StarRocks is not Trino

| | Trino | StarRocks |
|---|---|---|
| A catalog is | a `.properties` file in a ConfigMap | a row in FE metadata, created by DDL |
| Applying | write the k8s object; the operator rolls the pods | `CREATE` / `ALTER` / `DROP CATALOG` on a live FE |
| Needs the cluster up | no | **yes** |
| Idempotent | yes — the whole spec is re-rendered every apply | **no** — one-shot statements |
| Secrets | `${ENV:…}`, resolved into a k8s Secret | **inline in the DDL text** |
| Removing a stale catalog | the next apply prunes it | nothing prunes it; it must be dropped |

Four consequences drive this design: an apply can be refused because the cluster is not ready;
a half-applied change is normal, not exceptional; egret must compensate its own writes; and the
FE holds credentials egret cannot indirect away.

## 2. Decisions

**D1 — Attach records, then converges.** An attach writes the attachment row and attempts the
DDL. On failure it stores `apply_status = 'failed'` with the error, and returns 201 with that
status rather than 502. Convergence comes from the generalised bootstrap (D2) and the existing
Retry route. This differs from Trino's apply-then-write, and deliberately: a StarRocks FE is
unreachable for five to ten minutes after `create_engine`, which is exactly when the create-engine
wizard attaches catalogs. Detach still applies first: a `DROP CATALOG` that fails keeps the row,
because a row egret has forgotten is worse than one it retries.

**D2 — The bootstrap converges every attachment, not the primary.** `spawn_starrocks_catalog_wire`
loops over the engine's attachments in name order, writing `apply_status` per attachment instead of
only logging. It keeps its six `engine_still_current` checkpoints. It becomes reachable from the
Retry route, so an admin has a way to converge an engine that today can only be fixed by
re-creating it.

**D3 — Create is preceded by a read.** Before `CREATE EXTERNAL CATALOG`, egret reads
`SHOW CATALOGS` and, when the name exists, `SHOW CREATE CATALOG`. Absent → create. Present and
matching what the registry would render → treat as applied. Present and **different** → refuse,
with an error naming the catalog and the differing properties, and leave the FE untouched. The
current DDL is `CREATE EXTERNAL CATALOG IF NOT EXISTS`, which silently adopts whatever is there —
including the `egret` catalog today's Warehouse UI tells operators to create by hand, and any
catalog left by a pre-SP3a engine. Re-applying an edited catalog uses `ALTER CATALOG … SET`, never
a second `CREATE`, because `IF NOT EXISTS` makes a credential rotation a silent no-op.

**D4 — StarRocks 4.x is the floor.** Egret provisions `starrocks/fe-ubuntu:4.1-latest`. `can_read`
gates on that: iceberg, hive, deltalake, hudi, elasticsearch, tpch/tpcds (rendered `benchmark`),
and jdbc for postgresql, mysql, mariadb, oracle, sqlserver and clickhouse. The image is
overridable, so an engine pinned to an older FE can still refuse a kind egret allows; that refusal
surfaces as the DDL error on the attachment, not as a validation error. Today `can_read` returns
`true` for every typed kind on both engines, which is simply wrong, and SP3b is where it stops
being wrong.

**D5 — JDBC drivers are pinned in egret config, overridable per catalog.** StarRocks makes the FE
download the driver named by `driver_url`. Egret ships a `driver_url` and `driver_class` per
dialect, pinned to an exact version, in configuration rather than in code, so an air-gapped
platform can repoint them at an internal mirror. `advanced.starrocks.driver_url` overrides for one
catalog. A dialect with no pinned URL and no override is refused at validation time.

**D6 — Secrets are resolved at DDL-build time and never logged.** There is no `${ENV:…}` for
StarRocks; the plaintext goes into the statement. Therefore: the DDL text is never logged, not even
at debug, and errors returned from the FE are scrubbed before they reach an `apply_error` column or
an API response. The catalog DDL also starts using the engine's pinned FE certificate, which the
delegation path already passes and the catalog path does not — today an OAuth2 credential crosses
the wire in cleartext on a TLS-capable engine. Anyone with FE admin rights can still read the
credential back with `SHOW CREATE CATALOG`; that is StarRocks' model, and SP4 is where per-user
authorization stops mattering less than it should.

**D7 — Egret compensates its own half-writes.** If the DDL succeeds and the attachment row cannot
be written, egret drops the catalog again. SP3a's comment says the next apply prunes it; that is
true for Trino and false for StarRocks. Detach treats "unknown catalog" as success, so a row can
always be cleaned up, and keeps refusing to detach the engine's primary catalog.

**D8 — The DDL runner becomes injectable.** `run_starrocks_ddl` is reached through a trait that
tests can fake. Today the bootstrap past FE-ready cannot be tested at all — it dials a real MySQL
port — so five of its six guard checkpoints are pinned by a test that greps the function body.
SP3b replaces that with real tests.

## 3. What SP3b does not do

The Catalogs UI, the Warehouses redirect and the removal of `engine_catalogs`,
`engines.warehouse_id` and `warehouse:*` are SP3c. Per-catalog Ranger policy generation is SP4;
SP3b only preserves the invariant that a catalog has one name across engines. StarRocks has no tag
service and never will until upstream starrocks#67458 lands, so nothing here changes tag handling.

## 4. Testing

- Rendering per kind and dialect against hand-written expected DDL, including the `benchmark` and
  `deltalake` spellings and every pinned driver.
- A faked DDL runner drives: attach against a ready FE, attach against an unreachable FE (row
  written, `failed` status, then converged by Retry), drift refusal, credential rotation via
  `ALTER`, detach with an unknown catalog, and a compensating drop.
- The bootstrap's checkpoints become behavioural tests instead of a source-text grep.
- Live on kind: provision a StarRocks engine, attach an iceberg and a postgres catalog, query both,
  rotate the password, re-apply, detach. Reported as manual.
