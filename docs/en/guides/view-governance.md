# View governance: what clearance protects through views, and what it does not

A classification's clearance withholds a classified column from a direct read on both engines. Without more, a view reads straight through it. egret closes that path differently on each engine, and this guide says what is covered, what is not, and what to do when egret refuses something.

Design: `docs/en/specs/2026-10-13-view-governance-design.md`. Engine versions and pinning: [engine-versions.md](engine-versions.md).

## What is protected

### StarRocks

- **A direct read of a classified column returns `NULL`, whoever denies it, provided the query reads some other column the reader may select.** Each classified column carries a clearance mask: `MASK_NULL` for an uncleared user, no transform for a cleared one. StarRocks 4.1.6 checks no access on a column whose deciding mask is `MASK_NULL`, from whichever policy, so the read returns `NULL` for that column instead of `ERROR 5204`, and a hand-written `select` deny on that column is not applied either. The query as a whole is still refused unless the reader may select another column it reads; when it selects only such columns, StarRocks checks the table's first unmasked column (or `___count___` for `count(*)`). So a reader with no grant is refused, through that other column. Explain shows the column as "not access-checked on StarRocks", with the mask and the policy that decided it, and no `select` verdict. A column masked any other way (`MASK_HASH` was measured) is access-checked as usual.
- **When the clearance mask is skipped, the deny decides.** egret skips a column's clearance mask for an engine whose plugin egret does not model, bound plugins that fold differently, or a mask on the column that names a group other than `public` or a role. The recompile reports that skip, and the column's clearance deny then refuses an uncleared read.
- **Not measured: a table on which every column is `MASK_NULL` for the reader** (a fully classified table, read by an uncleared reader with no grant). Which column StarRocks checks then is unknown. If it checks none, such a reader learns the row count. Treat a fully classified table as exposing its row count until this is measured.
- **Views return `NULL`, or are refused.** A base-column mask applies through plain and nested views, so an uncleared reader gets `NULL` there. Through an INVOKER view, StarRocks 4.1.6 refuses the uncleared reader (`ERROR 5204 … SELECT privilege(s) on COLUMN …`). Both are fail-closed.
- **New materialized views are denied.** A materialized view stores real values, and a mask does not follow stored data. So on each StarRocks profile with a classified column, `create materialized view` is denied on `default_catalog`, every database. StarRocks creates a materialized view only in `default_catalog` and checks the privilege on the target database, so the deny is engine-wide.

### Trino

- **Views in egret's Iceberg catalogs carry their bases' classifications.** egret runs the catalog itself, so a `CREATE VIEW` or `CREATE OR REPLACE VIEW` reaches egret before it is committed. egret asks the engine which base tables the view reads (`EXPLAIN (TYPE IO)`), tags every column of the view with the classifications of those tables, and holds the commit until every Trino plugin holds the new tags. The view does not exist until then, so nobody reads it untagged.
- **The tags follow the bases.** Classifying, reclassifying or declassifying a base column re-tags every view whose recorded lineage reads that table, in the same tag version.
- The lineage is table-level: every column of the view gets every classification of every table it reads. This over-tags by design.

## What is not protected

- **Data a cleared user copies out.** A cleared user can `CREATE TABLE AS SELECT`, `INSERT … SELECT` or export classified values. Clearance does not follow the data.
- **Views of other connectors.** A view in a memory, Hive or any catalog that is not in egret's registry never reaches egret, so it is not tagged.
- **Native StarRocks tables.** The registry cannot hold `default_catalog`, so a native table cannot carry a classification. StarRocks protection applies to classified columns of external catalogs egret registers.
- **Engines older than StarRocks 4.1.6.** There, a reader with no grant on a plain view reads it anyway (an engine defect). The mask still nulls the classified column, but the view grant itself is unchecked. Pin the engine to 4.1.6 as [engine-versions.md](engine-versions.md) describes.
- **Materialized views that already exist.** The deny stops new ones only. An existing materialized view may hold real classified values, and egret does not drop it. The classification page lists the ones it finds ("Existing materialized views"); drop or rebuild them yourself.
- **Materialized views on Trino** could not be created on an Iceberg REST catalog when measured, so they are out of scope.

## The classification page: views that read the classification

The page for one classification shows two lists.

- **Existing materialized views (StarRocks).** Every running StarRocks engine is asked once, as root. On StarRocks 4.1.6 root is a superuser under Ranger and sees every materialized view, so no policy can hide one from this list. A view matches when its base-table map names a classified table, or, if it has no map, when its definition names the classified catalog and database ("matched by text"). A materialized view over a listed one is listed too, at any depth, since it holds the same values. An engine egret did not ask (external, or not running) is named "not checked". If an engine cannot be reached, the list says so and never shows "none".
- **Trino views (the DEFINER audit).** Every view in egret's catalogs whose recorded lineage reads a table of this classification, or a table that a mask or row-filter rule selects. For each one the page shows:
  - **Mode**: DEFINER or INVOKER, read from the view's stored `trino.run-as-owner` property (present for DEFINER, with the owner as its value). If the metadata cannot be read the mode is "unknown"; egret never guesses DEFINER.
  - **Owner** and **owner clearance**: whether the owner holds a clearing role of every classification the view's lineage reaches. "Clearance unknown" means the lineage is not computed, or a reached clearance names only groups.
  - **Lineage**: a view whose lineage is missing or failed is listed anyway, as "may read classified data". egret cannot say it reads nothing classified. Run the lineage sweep to compute it.

  Rules written by hand as raw policies in a manual profile are not `perm.rules` rows, and the audit does not see them.

  Owner clearance is read from current role membership. The engine enforces the last compiled policy, so when a role grant expires the page says "not cleared" while the engine still lets the owner through, until the next recompile.

  The Trino list has a deadline of 15 s. If egret's views cannot all be read in that time, or a listing fails, the page shows an error instead of the list, never a partial list.

### Why DEFINER views are listed: recommend SECURITY INVOKER

A Trino view is DEFINER by default: it reads its bases as its owner. With the tags in place a DEFINER view over classified data no longer leaks, but two behaviours still surprise readers:

- an uncleared owner blocks every reader of the view, cleared readers included;
- base masks and row filters apply as the owner, not as the reader.

An INVOKER view reads as its reader, so every reader gets their own masks, row filters and clearance. **Create views with `SECURITY INVOKER`.** egret does not rewrite views.

**Trino 483 defect: switch with `DROP VIEW`, then `CREATE VIEW`.** `CREATE OR REPLACE VIEW … SECURITY INVOKER` does not remove the `trino.run-as-owner` property, so a view that was ever DEFINER stays DEFINER. To switch:

```sql
DROP VIEW ice.sales.v_orders;
CREATE VIEW ice.sales.v_orders SECURITY INVOKER AS SELECT ...;
```

The page then shows the view as INVOKER.

## The `CREATE VIEW` wait

A view change on a catalog whose profile has a classified column takes a **hold**: a create, a replace or a rename, whether or not the view's own tables are classified.

- **One deadline of 90 s** (three times the 30 s plugin poll interval) from the request's arrival covers everything: the view's lock, the lineage `EXPLAIN` (at most 5 s), and the wait for the plugins. Measured against Trino 483's client: held commits of 54 to 60 s, and a refusal at 90.2 s server side (91.2 s at the client), in one request, with no client timeout and no retry.
- **A proxy in front of `/catalog` must read for longer than 90 s.** egret's Helm chart routes `/catalog` through its own Ingress with a 120 s read timeout. A proxy that cuts sooner answers the client with a gateway error while egret still commits or refuses the view. A client that disconnects (Ctrl-C, a statement timeout) has the same effect: the hold still runs to its end, and a refused or timed-out change is still rolled back.
- **A view that reads classified data waits one to two poll intervals** (30 to 60 s) for the plugins to confirm the tags. A plugin downloads the new tags on its next poll, and reports the new version only on the poll after, which is what confirms. A view that reads no classified table is not delayed.
- **At most 8 holds run at once** per egret process. A ninth one is refused at once with a retry message; it does not queue. A dbt run with more than 8 threads over classified catalogs therefore sees some refusals, which succeed on retry.
- **A busy view lock is refused at once** with "retry": another change to the same view is being held, and it may run for its whole deadline, or the lineage sweep is recomputing the view, which holds its lock for up to one EXPLAIN (5 s).
- On a catalog whose profile has **no** classified column, the commit is never held or refused. egret still records the view's lineage, best-effort, so a later classification re-tags the view. At most 8 such records run at once; past that, or while the view's lock is taken, the commit passes with no EXPLAIN and the lineage is left `failed` (or to the other writer) for the sweep.
- **The engine's own client needs `write` on the warehouse.** The catalog role of a Trino engine's client must hold a `write` grant on the warehouse (`POST /roles/{id}/grants` with `{catalog, write}`), or Trino cannot create a namespace or a view there at all (`role '…' has no grants on warehouse '…'`), and no hold starts.

### Refusals, and what to do about each

Every refusal is an Iceberg `409`, which Trino prints as `egret refused the view: <reason>`. The reasons:

| Message | What it means | What to do |
|---|---|---|
| `lineage could not be computed: <detail>; check that a running Trino engine egret provisions in JWT mode has the catalog attached and can plan this SQL, then retry` | egret could not learn which tables the view reads: the EXPLAIN failed or timed out (`the EXPLAIN did not answer within 5 s`), or no eligible engine could run it. Also `the engine reports no input table; egret refuses views it cannot judge`: the optimizer pruned every input (`WHERE false`, `LIMIT 0`, a constant predicate). | Check that a provisioned, running, JWT-mode Trino engine has the catalog attached and can plan the SQL. For an empty lineage, remove the constant predicate. Then retry. |
| `engines did not confirm the classification tags within 90 s: <plugin ids>; check that the engines are running and polling egret's Ranger endpoint, then retry` | A Trino plugin did not download the new tags in time. The tags and lineage are rolled back. | Check that the named engines are running and can reach egret's Ranger endpoint, then retry. |
| `no Trino engine is bound to the view's catalog, so no plugin can confirm its classification tags; attach a Trino engine to the catalog, then retry` | No plugin exists that could confirm the tags. | Attach a Trino engine to the catalog, then retry. |
| `another change to this view is being held, or the lineage sweep is recomputing it; retry when it has finished (at most 90 s)` | The view's lock is taken by another hold, or by the lineage sweep (for up to 5 s). | Retry after it finishes. |
| `egret is holding 8 classified view changes already, its maximum; retry in a minute` | The cap of 8 concurrent holds is reached. | Retry. For dbt, lower the thread count or retry failed models. |
| `egret could not write the view's classification tags within the view's deadline (90 s), because other classification changes were being written; retry` (or `… in time, because …`) | Other classification writes held the view-tag lock past the deadline. Nothing was written. | Retry. |
| `the classification tags cannot be assigned: <detail>; resolve the overlapping mask or row-filter policies, then retry` | Tagging the view would make two mask or row-filter policies decide one request differently. | Narrow, delete or align one of the overlapping rules (`GET /api/control-plane/perm/overlaps` lists stored pairs), then retry. |
| `the view's namespace has more than one level, which egret cannot govern on a catalog with classified columns; use a single-level schema` | The Ranger resource has one schema level. | Create the view in a single-level schema. |
| `<why>: egret cannot judge this view definition on a catalog with classified columns; create the view from Trino, with Trino SQL and a single-level default schema` | A create egret cannot read: no SQL representation, no schema, a multi-level default namespace, and so on. | Create the view from Trino. |
| `<why>: egret cannot judge this view version on a catalog with classified columns; change the view's definition, or DROP and CREATE it` | A replace egret cannot read, for example one that sets the current version to `-1` before adding any version. | Change the definition, or `DROP VIEW` then `CREATE VIEW`. |
| `egret could not hold the view (<what>): <error>; retry` | An internal error (database, request body). | Retry; if it persists, check egret's log. |

**Undropping a view is refused on a classified profile.** Restoring a dropped view goes through lakekeeper's management API, which the hold never sees, so the restored view would come back untagged. On a profile with a classified column the undrop answers `409`:

> egret refused to restore the view: its catalog's profile has classified columns, and a restored view would bypass the classification hold; re-create it with CREATE VIEW from Trino instead

Re-create the view with `CREATE VIEW`. A table undrop, or a view undrop on an unclassified profile, is allowed.

## The lineage sweep

The recorded lineage is what keeps view tags in step with their bases. A **sweep** recomputes every view's lineage and re-tags the view from it:

- **every 15 minutes**, on a schedule;
- **on demand**: `POST /api/control-plane/classifications/lineage-sweep` (permission `tag:manage`). It reports the views it could not judge. A sweep that meets a running one answers `a lineage sweep is already running; its report is not shared, retry when it ends`;
- **at once after a classification write**, scoped to the views with no computed lineage (missing or failed) in every warehouse the write's profiles reach. A view whose lineage is computed is re-tagged by the write itself.

A view whose EXPLAIN fails keeps its tags; tags are never removed on uncertainty.

A view waits for the **15-minute schedule** instead of the immediate scoped sweep in these cases:

- **Held skip.** The sweep found the view's lock taken by a hold in progress, so it reported the view and moved on.
- **EXPLAIN failure.** The view's EXPLAIN failed in that sweep.
- **A race with the classification check.** The hold read the profile as unclassified, its best-effort EXPLAIN failed, and the view committed after the classification's sweep had listed the namespace.
- **A lost notification.** The write's notification was lost while egret's listener reconnected.
- **Queuing behind a full sweep.** The scoped sweep starts only when a running full sweep ends, which takes up to 5 s per view.

15 minutes is therefore the longest a pruned branch, or a view committed with no classified column and later read over a classified one, stays untagged, **provided the next sweep can EXPLAIN the view.** A view whose EXPLAIN keeps failing stays `failed` for as long as that lasts. It keeps any tags it had, and the classification page lists it with "lineage could not be computed"; the sweep's report names the reason.

## Known limits on JWT engines (being fixed)

The live run on Trino 483 (Task 11) found two defects outside view governance that are still open. They are being fixed in a separate package. Until then:

- **egret's token rate limit breaks Trino's catalog access under load.** Trino's Iceberg client takes a fresh OAuth token per query session, and egret allows 60 token requests per minute by default. Concurrent `CREATE VIEW`s, and the 15-minute lineage sweep, then fail inside Trino with `RESTException: Unable to process (code: 429, type: slow_down)`. **Workaround:** raise `EGRET_RATE_LIMIT__TOKEN_MAX_ATTEMPTS` (the live run used 100000).
- **Trino's Iceberg token refresh fails against egret.** About an hour after Trino starts, its OAuth2 refresh (`grant_type=…token-exchange`) is answered `422 … missing field 'client_id'`, and Trino retries in a loop, which feeds the rate limit above. **Workaround:** restart the Trino engine; it starts with fresh tokens (this is how the live run recovered).

## Accepted risks

These were accepted when the design was approved. They are stated here so nobody has to rediscover them.

- **Users not cleared for every classification on a StarRocks engine cannot create any materialized view there.** The deny is engine-wide, and its only exceptions are the users cleared for every classification with a column on the profile. Such a user cannot create a materialized view in any database of that engine, even over unclassified data. To let someone build materialized views, clear them for every classification on that engine, or keep classified catalogs off the engines where materialized views are built.
- **The `egret-lineage` principal can read governed data.** The lineage EXPLAIN needs `select` on every column the view reads, so egret serves this principal `use` and `select` on every governed catalog, only to Trino engines egret provisions in JWT mode, and denies it everything else. Only egret holds its credential, and egret sends nothing but EXPLAIN as it.
- **A view is refused when the engine reports no input table.** EXPLAIN reports inputs after optimization, so `WHERE false` or `LIMIT 0` reports none. egret refuses such a view rather than commit it untagged.
- **A pruned branch is caught only at the next sweep.** A branch the optimizer pruned at commit (a predicate that becomes true later) is not in the lineage until the next sweep sees it, at most 15 minutes.
- **A forged poll can end a hold early.** The plugin download endpoint is anonymous, so anyone who can reach egret can forge a poll that confirms a tag version. That can only end a hold early: the real plugin still applies the new tags on its next poll, within one 30 s interval. A forged poller that keeps reporting a stale version can instead block holds on its service, which refuses classified views (fail-closed). Configuring the download `auth_token` closes both.
