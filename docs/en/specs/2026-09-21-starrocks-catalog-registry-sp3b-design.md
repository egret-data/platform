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
status rather than 502. **Migration 0149 adds a third status, `pending`**: `apply_status` today is
`CHECK (applied|failed)` defaulting to `applied`, so a row written before its DDL runs claims to be
applied — which `create_starrocks_engine` already does for the primary attachment, for the five to
ten minutes the FE is unreachable. Recorded-but-not-yet-converged needs a name of its own. Convergence comes from the generalised bootstrap (D2) and the existing
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
gates on that: iceberg, hive, deltalake, hudi, elasticsearch, and jdbc for postgresql, mysql,
mariadb, oracle, sqlserver and clickhouse. **`tpch` and `tpcds` are refused on StarRocks.** The
`benchmark` connector that would serve them exists only on StarRocks' `main` (added 2026-01-22,
commit `c019a148`); it is in no release branch — not 4.0, not 4.1 — and in no tag, so a `benchmark`
catalog fails on every FE egret can actually provision. The SP3 design's engine-support table said
otherwise and was wrong. When the connector ships, this is a one-line change plus a test. The image is
overridable, so an engine pinned to an older FE can still refuse a kind egret allows; that refusal
surfaces as the DDL error on the attachment, not as a validation error. Today `can_read` answers
`true` for every typed kind on both engines through a `_ => true` arm. At a 4.x floor the matrix
**denies nothing that exists today**, so this is a structural change, not a behavioural one: an
exhaustive match with no wildcard, so that adding a kind forces a decision per engine instead of
inheriting a silent yes.

**D5 — JDBC drivers are pinned in egret config, overridable per catalog.** StarRocks makes the FE
download the driver named by `driver_url`. Egret ships a `driver_url` and `driver_class` per
dialect, pinned to an exact version. The table is compiled in, because `CatalogDef::validate` is
pure and cannot read deployment configuration; `[starrocks.jdbc_drivers]` may **override** an entry,
so an air-gapped platform can repoint them at an internal mirror. `driver_url` and `driver_class`
are the only typed keys `advanced.starrocks` may override — §4.3's collision rule would otherwise
refuse the override before it could take effect — and they are consumed before the typed block.
Validation refuses an incomplete override, which is what it can see; a blanked driver is refused at
render time and surfaces as the attachment's `apply_error`.

**`mysql` renders a MariaDB URL on StarRocks.** The FE rewrites `jdbc:mysql` to `jdbc:mariadb` and
loads `org.mariadb.jdbc.Driver` (`JDBCMetadata.java:123-125,189-191`, "because of gpl protocol"), so
Trino's `sslMode=VERIFY_IDENTITY` would reach a driver that only understands
`disable|trust|verify-full`. This is the first place one registry row must render different text for
each engine.

**D6 — Secrets are resolved at DDL-build time and never logged.** There is no `${ENV:…}` for
StarRocks; the plaintext goes into the statement. Therefore: the DDL text is never logged, not even
at debug, and errors returned from the FE are scrubbed before they reach an `apply_error` column or
an API response. **Scrubbing alone is not enough.** It removes whole occurrences of a secret, in
both its raw and its escaped form, but an FE error that quotes a *truncated* statement leaks a
prefix that no substring match can catch. So the consumer caps and summarises the FE error rather
than storing it whole: `apply_error` keeps the FE's error code and a bounded, scrubbed excerpt, and
never the statement. The catalog DDL also starts using the engine's pinned FE certificate, which the
delegation path already passes and the catalog path does not — today an OAuth2 credential crosses
the wire in cleartext on a TLS-capable engine, and so does the FE **root password** on the
login probe that precedes it. Every statement goes through one target so neither can be forgotten
again. The pinned certificate only exists when delegation is configured and TLS is on, so the
cleartext path remains for engines without it; that is a limit, not a fix. Anyone with FE admin rights can still read the
credential back with `SHOW CREATE CATALOG`; that is StarRocks' model, and SP4 is where per-user
authorization stops mattering less than it should.

**D3 addendum — drift comparison is blind to secrets.** `SHOW CREATE CATALOG` masks credential
values as `xx******yy` and removes a JDBC `password` outright (`CredentialUtil.java:88-99`), and its
`checksum` is the FE's own. The comparison therefore covers plain, non-masked keys only: egret
cannot tell a rotated password from an unchanged one by reading the FE, and must not pretend to.

**Known gap, not fixed here.** Egret sends the warehouse credential as `iceberg.catalog.credential`
and `iceberg.catalog.oauth2-server-uri` — the Iceberg-standard names, which are the ones that
actually work on 3.3 (the `iceberg.catalog.oauth2.*` dot-form is silently ignored). `CredentialUtil`
masks the dot-form, so `SHOW CREATE CATALOG` may return egret's credential **in clear** to any FE
admin. SP3b keeps the names that work, records the live output verbatim in its kind check, and
leaves the fix to SP4.

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
  rotate the password, re-apply, detach. Reported as manual. (Not a `tpch` catalog: see D4.)
