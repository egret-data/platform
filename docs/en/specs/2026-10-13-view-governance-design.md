# View governance under clearance: StarRocks deny + MASK_NULL, Trino lineage tags held at commit

> **Status:** design approved in conversation on 2026-10-13. Canonical (English).
> **Inputs:**
> - the view-clearance measurement `docs/en/plans/2026-10-13-view-clearance-measurement.md` (rounds 1 and 2: Trino 470, StarRocks 4.1.4 and 4.1.6);
> - its re-run on the upgraded engines, `docs/en/plans/2026-10-13-engine-upgrade-live-result.md` §2 (Trino 483, StarRocks 4.1.6). §2.6 lists what this spec may assume.
>
> **Amends:** SP4c (`2026-09-28-classification-clearance-sp4c-design.md`). SP4c ruled "state the limit, do not block view creation". This spec keeps "do not block view *creation*" on StarRocks. On Trino, it holds the *commit* of a view that reads classified data until the view's columns are tagged.

## The defect

A classification's clearance withholds a classified column from direct reads on both engines. It does not hold through views:

- **StarRocks.** A plain (`SECURITY NONE`) view is an access boundary. A reader who holds the view grant reads the classified column even when the base column has a deny. This is still true on 4.1.6 (§2.1).
  - A materialized view built by an uncleared user holds real values, because the refresh runs as the builder without a mask lookup (cell 6b).
  - A materialized view built by a cleared user is readable by anyone it is granted to (cell 6a).
- **Trino.** Views are DEFINER by default, and Ranger cannot tell `CREATE VIEW` apart from `CREATE TABLE` (cells 1.T1 to 1.T3).
  - A cleared owner hands the classified column to every reader of the view (`vd_c`).
  - An uncleared owner blocks everybody, the cleared included (`vd_u`).
  - Base masks and row filters apply as the owner (`wd_c`).
  - Only `SECURITY INVOKER` views enforce correctly, and egret cannot force that mode.

Effective access is therefore the union of the direct path and the view path. It should be the intersection.

## Decisions (user rulings, 2026-10-13)

| # | Question | Ruling |
|---|---|---|
| R1 | One spec or two | **One spec, two parts.** One plan covers both. |
| R2 | Scope of the StarRocks materialized-view deny | ~~Per database that holds a classified column.~~ **Revised 2026-10-14: the whole engine.** One deny on `default_catalog`, every database, for each StarRocks profile that holds a classified column. The exceptions are the users cleared for **every** classification with a column on that profile. (A per-database deny cannot match, see A3.) |
| R3 | How precise Trino lineage is | **Table-level, decided by the engine.** If a view reads a table with a classified column, every column of the view gets the union of that table's classification tags. egret does not parse SQL. |
| R5 | Privileges of the lineage principal (2026-10-14, after Task 5 measured that EXPLAIN needs `select`) | **It reads everything egret governs, and only on engines egret provisions.** Revised 2026-10-14 (Task 7 fix round 1): no stored policy names `egret-lineage`. egret adds its material **at download time, per plugin**, and only for a plugin of a Trino engine egret provisions in JWT delegation mode. That material is `use`+`select` on every governed catalog, an exception from every served deny and allow-write, a leading no-transform item in every served mask and row filter (hand-written included), and a deny of every other access type. Every other plugin gets the stored policies, so lineage fails closed there. Only egret holds its credential, and egret's client sends nothing but EXPLAIN as it. |
| R6 | An empty lineage (2026-10-14) | **Refuse the view, and sweep.** A view whose EXPLAIN reports no input table is refused with a reason. The B3 sweep re-runs EXPLAIN on a schedule to catch branches the optimizer pruned. |
| R4 | The window between a view existing and its tags reaching the engines | **Hold the commit until every engine has the tags.** If that cannot be done in time, refuse the view, with a reason. |
| R7 | The hold limit (2026-10-15, after Task 11 measured holds of up to two poll intervals, finding I1) | **3 × the plugin poll interval** (`RANGER_POLL_INTERVAL_MS`), 90 s today. The confirmation rule is unchanged: every plugin id seen within the last three intervals reports `lastKnownVersion ≥ V`. The EXPLAIN stays at most 5 s inside the deadline. |

## Part A — StarRocks

StarRocks has no tag service (upstream `starrocks#67458`), so a clearance compiles to resource policies (SP4c Task 2d), one set per classified column.

### A1. Keep the column deny for direct reads

There is no change here. The deny on `{catalog, database, table, column}`, with the cleared users in `denyExceptions` and the access type `select`, stays.

**Measured on 4.1.6 (Task 11, finding C2), for one reader: an uncleared reader who holds the table grant.** For that reader, with A2's `MASK_NULL` on the column, StarRocks asked Ranger for no access decision on the masked column (the FE's debug log asked `policyType=0` only for the unmasked column of the read), and the direct read returned `NULL`, not `ERROR 5204`. No value leaks either way.

**Measured on 4.1.6 (Task 13, live §3.4, cases 2a/2b/2c), and the rule that follows (final review V-1).** StarRocks keys the skip on the deciding mask's type alone. 2b: a hand-written column `select` deny plus the clearance `MASK_NULL` returned `NULL`, so the hand-written deny was not applied. 2c: `MASK_HASH` plus a deny was refused (`5204`), so a `MASK_HASH` column is access-checked. 2a: a reader with no grant was refused, but through another column (`id`, or `___count___` for `count(*)`), never through the masked one. So a column whose deciding mask is `MASK_NULL`, from whichever policy, is not access-checked on StarRocks; the query as a whole is still refused unless the reader may select another column it reads. Only the clearance's own `MASK_NULL` was measured; keying on the type rather than on the clearance policy errs toward overstating exposure. Not measured: a table on which every column is `MASK_NULL` for the reader. The deny decides:
- for a column whose clearance mask the compiler **skips** (A2 "Skip, never guess": an unmodelled plugin, bound plugins that fold differently, or a mask on the column naming a group other than `public` or a role). The recompile then reports a `SkippedItem` and compiles only the deny;
- on engines that check access on a masked column (Trino does).

`explain` mirrors the engine (Part C).

### A2. Add a MASK_NULL policy per classified column, folded with the rule masks

A base-column **mask** applies through every view kind, for every reader. It also leaks nothing through predicates, aggregates or joins (§2.6 item 4). A base deny does not apply through a plain view. So each classified column also gets one data-mask policy.

`MASK_HASH` is not used: it leaks equality, cardinality and, with a guess, content (round 2 A). The type is always `MASK_NULL`.

Ranger stops at the **first matching item** of the first applicable data-mask policy, and a data-mask policy has no exceptions. So the clearance mask decides for **everyone** who reaches it, cleared users included, and it must reproduce for a cleared user exactly what the rule masks would have given that user.

- **Priority.** The clearance mask policy has `policyPriority 1` (override). Rule masks are compiled at priority 0, so the clearance mask always comes first. A pair at different priorities is ordered, not ambiguous (`egret_policy::ambiguous_arbitration`), so SP4d-4's overlap rule does not refuse it. Rule mask policies are emitted unchanged, including those whose column resource is a wildcard.
- **Items, in this order:**
  1. For each cleared user, the outcome `egret-ranger-eval` computes from the profile's **other** data-mask policies (the desired set without the clearance masks) for that user on that column: a mask type, with its value expression for `CUSTOM`, or no mask. Users with the same outcome share one item. A user whose outcome is `MASK_NONE` or "no mask" gets `MASK_NONE`.
  2. `{public}`: `MASK_NULL`.
- **Refuse, never guess.** If the evaluator is `Undecidable` for any cleared user on that column (for example a same-priority tie between two rule masks), the compile is refused with the column and the user named. Nothing is folded by guesswork.
- **Recompile triggers.** These are SP4c's three triggers plus any rule mask write. The rule-mask trigger exists already, because the whole profile recompiles on a rule write.

**Why "users", not principals.** The cleared set is enumerated as bare usernames (SP4c D0/D0b), because a role in a policy item matches nobody. The same enumeration feeds items 1 and 2. SP4c's membership recompile trigger already covers a change to that set.

### A3. Deny `create materialized view` on the whole engine (R2, revised 2026-10-14)

**Why not per database.** Measured on StarRocks 4.1.6-6862092 with a real Iceberg REST catalog (`docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`):
- A materialized view can only be created in `default_catalog`. `CREATE MATERIALIZED VIEW ice.db1.mv …` fails with `Materialized view can only be created in default_catalog`.
- `default_catalog.db1.mv AS SELECT … FROM ice.db1.t` succeeds and copies the Iceberg rows.
- The `CREATE MATERIALIZED VIEW` privilege is checked on the **target** database `default_catalog.<db>`, never on the base table's catalog or database.

A deny on `{catalog: <external catalog>, database}` therefore never matches a real `CREATE MATERIALIZED VIEW`, and the target database is any database the builder may write to.

For each StarRocks profile that holds at least one classified column, the compiler emits one access policy:

- **resource:** `{catalog: default_catalog, database: *}`;
- **deny:** `create materialized view` for `{public}`;
- **denyExceptions:** the users cleared for **every** classification that has a column on that profile.

`create materialized view` is its own access type on the database resource. Denying it leaves `create view`, `create table`, CTAS and DML alone.

**Cost (accepted by the user).** A user who is not cleared for every classification on the engine can no longer create any materialized view on that engine, including over unclassified data. The classification page and the guide state this.

A profile with no classified column gets no policy. The policy is recompiled on the three SP4c D3 triggers:
- a classification gains or loses a column on that profile;
- a clearance changes;
- membership changes.

The compiled per-database policies named `…mv-{catalog}.{database}` from the first draft are swept by the compiler prefix.

### A4. Report the materialized views that already exist (revised 2026-10-14)

A3 only prevents new materialized views. egret lists the ones that already exist and read a classified table, and shows them on the classification page with a warning: they may hold real classified values (cells 6a and 6b), and egret does not drop them.

- **Where.** The views live in `default_catalog`. `SHOW MATERIALIZED VIEWS FROM <external catalog>.<db>` returns an empty set (measured), so it is not used.
- **How.** egret's StarRocks admin connection queries `information_schema.materialized_views` once per running engine on the profile.
  - **Match by base table.** A view matches when a key of `BASE_TABLE_REFRESH_VERSION_TIMES` (a JSON map keyed `catalog.db.table`) names a classified table of that classification, in a catalog attached to that engine.
  - **Match by text.** A view whose map is empty or missing (for example, never refreshed) is matched by text instead. `MATERIALIZED_VIEW_DEFINITION` must contain the classified `catalog` and `db` names, and the view is flagged "matched by text".
- **Engines not running.** These are not dialled. The page names them as "not checked".
- **Unreachable engine.** If a running engine cannot be reached, the page says so and never shows an empty list.

### A5. What Part A does not cover

These limits are stated in the guide and on the classification page:
- **Data a cleared user copies out** (CTAS, `INSERT … SELECT`, export) is not protected (cell 10b). Clearance does not follow the data.
- **Native StarRocks tables.** The registry cannot hold `default_catalog`, so native tables cannot carry a classification (§2.6 item 6d). Part A applies to classified columns of external catalogs that egret registers.
- **Engines older than 4.1.6.** On these, a zero-grant reader of a plain view reads it without any view grant (F-1). The mask still nulls the classified column, so A2 holds. The view grant itself is unchecked, and that is the engine's defect. The guide points to the pinning procedure in `engine-versions.md`.

## Part B — Trino

### B1. Lineage at table level, decided by the engine (R3)

For a view, the egret **lineage** is the set of base tables that Trino reports it reads.
- It comes from `EXPLAIN (TYPE IO, FORMAT JSON)` of the view's SQL.
- The SQL is run with the view's default catalog and namespace, taken from the Iceberg view metadata.
- Nested views resolve through the engine, so the reported inputs are base tables.

egret does not parse the SQL. Earlier sqlparser-based gates were bypassed four times through lexer differences, which is why.

If any lineage table has a classified column, **every** column of the view gets the union of those tables' classification tags. This over-tags: a view column that does not derive from a classified column is still denied to uncleared readers. That is the accepted cost of R3.

A tag on a view column is enforced end to end on Trino 483 (§2.6 item 2). The clearance deny, which is a tag policy and is evaluated first, applies to the reader **whatever the view's security mode**. This closes the `vd_c` leak.

**Who runs the EXPLAIN (R5, measured in live result §1 Q3).**
- A dedicated principal, `egret-lineage`.
- **Why it needs `select`.** On Trino 483, `EXPLAIN (TYPE IO)` needs `use` on the catalog and `select` on every column the SQL references, including the bases of INVOKER inner views. No metadata-only grant is enough.
- **Why it is excepted.** A row filter or a mask on the principal corrupts the lineage: a `1 = 0` filter gives `[]`, and a `MASK_NULL` mask dropped a column from a join.
- **Where its access is decided (revised 2026-10-14, Task 7 fix rounds 1 and 2).** Not in any stored policy: the compiler's output and hand-written policies say nothing about `egret-lineage`. egret transforms the policies it **serves** (`egret_policy::lineage_transform::apply_lineage_transform`). The decision is made per poller (`lineage_catalogs_for_poll_via`). The poll's `pluginId` (`<appId>@<host>-<service>`) must name the polled service, every engine bound to that profile must be a Trino engine egret provisions in JWT mode, and the host must be a coordinator or worker pod of one of them (§B4).
  - Every served mask and row-filter policy, resource or tag, compiled or hand-written, gets a leading `egret-lineage` item (`MASK_NONE` / empty filter). Ranger's first matching item wins.
  - Every served deny gets an `egret-lineage` exception for the reads it withholds (`select`, `use`). Hand-written denies are included.
  - Every served allow gets an `egret-lineage` allow exception for the non-read accesses it grants, so an inherited `{public}` write never reaches it.
  - Appended, download-only: `use`+`select` on every catalog the profile's engines attach, and an override-priority deny of every access type except what EXPLAIN needs. The only exceptions to that deny are data-chain `select`+`use`, `execute` on `queryid`, `impersonate` on `trinouser` = itself, and `alter` on `systemproperty` = `query_max_run_time` (fix round 2). The last two are narrowed by a deny with `isExcludes`. `queryid` stays `*`, because its value is the id of the query being run.
  - **Why at download time.** A hand-written mask or filter would otherwise apply to the principal and could prune one input of a join, giving a short but non-empty lineage and an under-tagged view. And an external engine's own authenticator decides who is called `egret-lineage`, so no external plugin may be served the grants.
  - The transform is deterministic, so a policy version serves the same bytes to every eligible plugin. The served version, and Task 6's ledger, are the stored `policyVersion`. Changes of binding, delegation mode or origin bump the version (§B4). A newly attached catalog that bumps nothing is missing from the allows until the next bump, so EXPLAIN there fails, which is fail-closed.
  - `explain` evaluates the same transformed download (`served_artifact_via`), decided through the same per-poller function for a representative coordinator pod of the first bound engine. The SP4d-3 migration gate evaluates the stored one, because the transform adds the same material before and after a migration.
- **Who holds the credential.** Only egret. egret's client sends nothing but `EXPLAIN` as this principal: any other statement is refused before it reaches the engine.
- **Accepted risk.** The principal can read governed data. egret is already the Ranger Admin and could grant itself the same, so the added exposure is the credential itself. The guide states this.

**Empty lineage (R6).** EXPLAIN reports inputs after optimization, so `WHERE false`, `LIMIT 0` and `WHERE current_date > DATE '2099-01-01'` all report no input.
- A view whose lineage is empty is refused, and the reason names this rule.
- A branch the optimizer prunes while others remain (for example one arm of a `UNION`) cannot be seen at commit. The B3 sweep re-runs EXPLAIN on a schedule and tags the view when that branch appears. The window between the predicate becoming true and the next sweep is stated in the guide.

**Input shape.** The JSON gives `inputTableColumnInfos[].table = {catalog, schemaTable: {schema, table}}`. It lists only base tables, for nested views and joins alike.

**Which engine.** Any running Trino engine bound to the view's catalog. If none is reachable, B2's refusal applies.

### B2. Hold the commit until the tags are on every engine (R4)

The Iceberg catalog runs inside egret (embedded lakekeeper), so a view create or replace reaches egret **before** it is committed. For a create or replace (`POST …/namespaces/{ns}/views` and the view commit endpoint):

1. Compute the lineage (B1) from the SQL in the request.
2. If no lineage table is classified, commit with no further delay. Record the lineage, which is still needed for B3.
3. Otherwise, in one transaction:
   - write the tag assignments for every column of the to-be view;
   - record the lineage;
   - bump the tag version of the affected tag service(s).
4. Wait until every engine bound to that tag service has downloaded a tag version at or above the new one (B4).
5. Then commit the view.

Before step 1: if the profile that governs the view's catalog has no classified column at all, the commit is never held or refused (user ruling, 2026-10-14, Task 8); otherwise a profile with no eligible lineage engine would refuse every view although nothing is classified. **Amended in Task 9 fix round 1:** the hold still runs the lineage EXPLAIN there, best-effort, within the same deadline and with no lock. After lakekeeper answers 2xx it records the computed lineage and the view's columns, or, on any failure, a `failed` lineage with the reason. It never refuses on this path. A later classification then re-tags the view in its own write when the lineage is computed, and its sweep (B3) finds the failed ones.

On the held path (step 3), the view does not exist until step 5, so nobody can read it untagged. Tags are resource paths, so they can be assigned before the resource exists.

Two paths commit before the view's tags are on the engines. Both close within one plugin poll after the tags are written, which is the same latency a base table's own new classification has.
- **Step 2.** A classification lands on a base between the hold's first read of the tag set (empty) and its after-commit record. The record runs under the view-tag lock and re-tags the view from the lineage it writes.
- **Step 0.** A first classification commits after the hold read the profile as unclassified. The best-effort record re-tags the view when its EXPLAIN succeeded. Otherwise the view waits for the classification's sweep (B3).

A held write waits for the view-tag lock no longer than its deadline (`lock_timeout`, at least 1 s). Past it, the write rolls back and the view is refused with a retry message (fix round 2).

**Throughput (Task 8 fix rounds).** One held request has one deadline of 3 × the poll interval (90 s, ruling R7) from its arrival, covering its lock, its EXPLAIN (at most 5 s) and its wait. At most 8 holds run at once per egret process. Any view change on a catalog with classified columns takes a hold (a create, a replace or a rename, whether or not its own tables are classified). A ninth one at once is refused with a retry message; it does not queue. So a run of many concurrent classified view changes (a dbt run with more than 8 threads, for example) sees some refusals, and they succeed on retry. Task 11 measures the refusal rate.

**Refusal.** egret refuses the create or replace, rolls back the tag assignments and the lineage, and answers with an error the engine shows the user, in each of these cases:
- the lineage cannot be computed (EXPLAIN fails, or no engine is reachable);
- an engine does not confirm the new tag version within the hold limit, which is 3 × the plugin poll interval (90 s with today's 30 s; ruling R7, 2026-10-15; it was 2 × until Task 11 measured a 0.7 s margin).

The message names the reason and what to do about it. A view that egret cannot judge is never committed untagged.

**Cost.** A `CREATE VIEW` that reads classified data waits one to two poll intervals (measured in Task 11: 33.8–60.4 s with a 30 s interval). A plugin downloads the new tags on its next poll, and reports `lastKnownVersion ≥ V` only on the poll after, which is what confirms. A view that reads no classified table is not delayed.

**Where to hook (measured, live result §1 Q1).** An axum layer in front of the mounted Iceberg router (`egret-catalog-engine/src/mount.rs`).
- lakekeeper's contract verification is not used. It never runs on view create, and on a commit it runs inside an open Postgres write transaction.
- Trino 483 sends create as `POST …/namespaces/{ns}/views`, and replace as `POST …/views/{v}` carrying `add-view-version`.
- `ALTER VIEW … AS` is not in Trino 483's grammar.
- A 65 s hold succeeded with no client retry, and a 409 is printed verbatim by Trino. The 90 s limit of R7 was not measured against Trino's client.

**Requests the layer must handle.**
- **A commit with no new view version** (`COMMENT ON VIEW`, or a replace whose updates are empty) passes through with no lineage work.
- **lakekeeper refuses the request.** The layer runs before lakekeeper's own validation, so when lakekeeper answers non-2xx after a hold, egret rolls back the tag assignments and the lineage it wrote.
- **Rename** moves the lineage and the tag assignments to the new name, within a hold.
- **Drop** deletes them after lakekeeper answers 2xx.

### B3. Keep view tags in step with base classifications

The recorded lineage (view → base tables) is what keeps view tags current when the base changes. When a column of a table is classified, reclassified or declassified, the same transaction that changes the base tag also does the following, all in one tag-version bump:
- recomputes the tag set of every view whose lineage contains that table;
- writes those view tag assignments.

Dropping a view deletes its lineage and its tag assignments. Renaming a base table is handled as drop and create: the views over it fail on the engine anyway, and their lineage is recomputed on their next replace.

When the lineage cannot be trusted, the view keeps its old tags. Tags are never removed on uncertainty. A **sweep** recomputes every view's lineage on a schedule, and on demand from the classification page. It reports views whose lineage could not be computed.

**How it is kept (Task 9).**
- **Provenance.** A view column's tag row carries `from_lineage` (migration 0188). The hold and the re-tag write `true`. Only those rows are ever removed: by a re-tag, a drop or the sweep. An admin's assignment is manual, and assigning a coordinate the lineage already derived makes it manual.
- **Columns.** The hold and the sweep record the view's columns beside its lineage (`view_lineage_state.columns`). The re-tag in the base write's transaction needs them, and no catalog metadata can be loaded there.
- **Exactly the derived set.** For a view whose lineage is computed, the derived rows become `view_tag_set(lineage)` × its columns, removals included. This also removes the tags a narrowing `CREATE OR REPLACE` left behind, because the hold only adds rows. A missing or failed lineage leaves the rows as they are.
- **One bump.** The base write, the re-tag and the bump share one transaction. The base's warehouse and the dependent views' warehouses are bumped together, so each service moves by one version. A tag that gains or loses its clearance re-tags the same way.
- **The sweep** runs every 15 minutes, and on `POST /api/control-plane/classifications/lineage-sweep` (`tag:manage`). Each view is swept under the hold's lock, in one transaction of its own, after its EXPLAIN. A view whose EXPLAIN fails keeps its tags. A computed lineage it already had is kept too, so the re-tag keeps following its bases. A view with no computed lineage is marked failed. 15 minutes is therefore the longest a pruned branch (R6), or a view committed with no classified column and later read over a classified one, stays untagged.
- **Drop.** After lakekeeper answers 2xx, the hold deletes the view's lineage and its `from_lineage` rows. An admin's tag stays.
- **One writer at a time (fix round 1).** Every writer of view tag rows or lineage takes one transaction-scoped advisory lock first: the base write, a clearance change, the hold's write, its compensation and its after-commit record, the drop and rename cleanups, the sweep and the undrop check. The hold computes its tag set inside that locked transaction, and its after-commit record re-tags from the lineage it writes. So a base classification and a held commit cannot each miss the other.
- **Compensation (fix round 1).** A refused hold removes only the rows it inserted that are still lineage-derived, never one an admin made manual meanwhile. When it restores a computed previous lineage, it re-derives that lineage's tags.
- **Clearance changes (fix round 1).** A tag that gains or loses its gate re-tags the views over its tables, with the same overlap snapshot and refusal as a base write.
- **After a classification commits (fix round 1).** The write notifies (`NOTIFY`, delivered at commit). A listener then sweeps the views with no computed lineage (missing or failed) in every warehouse the write's profiles reach. A sweep that meets another running sweep waits and runs again, so the request is never dropped. The window for those views is normally the length of that sweep. It extends to the 15-minute schedule in these cases:
  - **Held skip.** The sweep finds the view's lock taken by a hold in progress, so it reports the view and moves on.
  - **EXPLAIN failure.** The view's EXPLAIN fails in that sweep.
  - **The race with `r.classified`.** The hold read the profile as unclassified, the best-effort EXPLAIN failed, and the view is committed after the classification's sweep has listed the namespace.
  - **A lost notification.** It was lost while the listener reconnected.

  Behind a running full sweep, the scoped sweep starts only when the full one ends, which takes up to 5 s per view.
- **Best-effort records take the view's lock (fix round 2).** When another writer holds it, the commit still passes at once. No EXPLAIN runs, and the lineage is recorded `failed` once the lock is free, so the next sweep recomputes it. The sweep records under the same lock.
- **Compensation and provenance (fix round 2).** Compensation deletes a row it inserted only while the row still has the provenance it was inserted with. A rename's copies of the source's manual tags therefore go, and a derived row an admin made manual during the wait stays.
- **Undrop (fix round 1).** Restoring a dropped view goes through lakekeeper's management API, which the hold cannot hold. On a profile with a classified column, a view undrop is refused (409), under the same lock; re-create the view with `CREATE VIEW` instead. A table undrop, or a view undrop on an unclassified profile, is allowed.

### B4. Know which tag version each engine holds

egret serves `download_tags` (`egret-policy/src/ranger/api/download.rs`), so it can record what each plugin last fetched. Each tag download records:
- the service;
- the plugin's id (`pluginId` query parameter);
- the version the plugin held (`lastKnownVersion`);
- the version served;
- the time.

"Engine E holds version ≥ V" means a **later** download from E's plugin reports `lastKnownVersion` ≥ V. A served version is not enough, because the plugin may fail to apply it.

**Mapping a `pluginId` to an engine** (live result §1 Q4). The format is `<appId>@<hostname>-<serviceName>`, for example `trino@e00d4f5be6a4-vg-trino`.
- The host part is the container or pod name, not the engine host registered in egret.
- The mapping uses the service name: each profile binds one engine.
- Trino polls tags under its **resource** service name, not under the tag service, so the ledger is keyed by the service the poll names.
- StarRocks never polls tags, so B2 waits only for Trino plugins.

**Forged polls.** The download endpoint is anonymous, so anyone who can reach egret can forge a confirming poll. A forged poll can only end a hold early: the real plugin still applies the new version on its next poll, within one interval. The guide states this.
A forged id that keeps polling a stale `lastKnownVersion` can also block the hold on that service, so a classified `CREATE VIEW` is refused (fail-closed). Configuring the download `auth_token` closes both the early-confirmation and the blocking forgery. **The lineage transform (B1, R5) is decided per poller from the same self-reported `pluginId`** (revised in Task 7 fix round 2). egret serves the transform only when three things hold:
- the id names the polled service;
- every engine bound to the profile is a Trino engine egret provisions in JWT mode;
- the id's hostname is a pod of one of those bound engines. The provisioner's Deployments are `{engine}-coordinator` and `{engine}-worker`, and their pods are named `<deployment>-<hash>-<suffix>`.

Every other poller gets the stored policies, so its lineage EXPLAIN fails, which is a refusal. That includes an unknown host, an unbound engine's pod, and a missing or malformed id.

**The hostname is self-reported, and an external engine's admin can forge it.** Such an admin would then be served the transform for an eligible service. This grants nothing new beyond their own engine: they already control it, and can, for example, turn its Ranger plugin off. `auth_token` does not close this. It is one shared token that every polling engine holds, so it says nothing about which engine is asking.

Every bind, unbind, and delegation-mode or origin change of a bound engine bumps the profile's `policy_version` (migration 0187's trigger). So no plugin cache or 304 can carry a transform across a change of eligibility.

The confirmation waits for every plugin id seen for that service within the last three poll intervals, not for the first one. (StarRocks runs two plugin instances, §2.6 item 6e, but it does not poll tags. Trino workers may poll too; Task 11 checks this.)

### B5. Audit DEFINER views

A DEFINER view over classified data no longer leaks with B1 in place. Two DEFINER behaviours still surprise operators:
- an uncleared owner blocks every reader, cleared readers included (`vd_u`);
- base masks and row filters apply as the owner, not the reader (`wd_c`).

The classification page lists every view in egret's catalogs whose lineage contains a classified table, or a table with a mask or row-filter rule. For each, it shows the view's security mode, its owner, and whether the owner is cleared. The guide recommends `SECURITY INVOKER`. egret does not rewrite views.

**Where the mode is stored** (measured, live result §1 Q2). A DEFINER view carries `properties["trino.run-as-owner"] = <owner>`; an INVOKER view does not. The audit reads this stored property and never trusts the statement. `load_view_detail` does not return properties today, so the audit reads them separately.

**Trino 483 defect.** `CREATE OR REPLACE … SECURITY INVOKER` does not remove the property, so a view that was ever DEFINER stays DEFINER. The guide says to switch a view to INVOKER with `DROP VIEW` then `CREATE VIEW`.

### B6. What Part B does not cover

These limits are stated in the guide and on the classification page:
- **Views of other connectors** (memory, Hive, or any catalog not in egret's registry) are not seen by the hook, and so are not tagged.
- **Materialized views on Trino.** These could not be created on an Iceberg REST catalog in the measurement (SP4c §4c), so they are out of scope. If a later engine allows them, they need their own measurement.
- **Data a cleared user copies out** (CTAS), as in A5.

## Part C — explain, migration and the evaluator

- **Evaluator and differential.**
  - The folded StarRocks masks (A2) and the engine-wide `create materialized view` deny (A3) are ordinary Ranger shapes that `egret-ranger-eval` already models.
  - The differential corpus gains a folded-mask profile and a materialized-view deny profile.
  - Both run against the real jars at every modelled pair, and must show 0 disagreements.
- **`explain`.**
  - On StarRocks, for a `read` of a column whose deciding mask is `MASK_NULL` (from whichever policy, A1 and final review V-1), explain answers with the mask ("returns NULL") and the policy that decided it, gives no `select` verdict, and says the column is not access-checked on StarRocks and that the query is still refused unless the reader may select another column it reads. Any other mask type (`MASK_HASH` was measured checked), or no mask, keeps the `select` verdict. When the mask verdict is undecidable, the access verdict is kept and the page says it is provisional.
  - Explain does not answer `create materialized view` (Task 12, M-4). The A3 deny is engine-wide, so the page states the rule instead of a verdict for one database.
  - On Trino, a view column denied by a propagated tag names the classification and the base table that caused it, read from the stored lineage.
- **Migration.**
  - A2 and A3 add policies, which count against the probe budget, so the plan measures the new worst shape against `MAX_PROBE_WORK`.
  - The tag-service grouping that refused a small profile (§2.6 item 6b) is not changed here. It is listed as a follow-up.

## Testing

| What | How |
|---|---|
| A2 fold | Unit: a column with a rule mask (exact and wildcard) and a clearance gives a priority-1 clearance mask whose cleared items equal the evaluator's per-user outcome, then `{public}` `MASK_NULL`. Rule policies are unchanged, and `GET /perm/overlaps` is empty. An undecidable cleared user refuses the compile. |
| A3 exceptions | Unit: a profile with columns of classifications X and Y gets one deny on `default_catalog`/`*` that excepts exactly users cleared for both. Real jars: an uncleared user is denied `create materialized view` on `default_catalog.<any db>`; a user cleared for both is not. |
| A4 unreachable engine | The page shows "could not list" when the engine is unreachable, never an empty list. |
| A4 matching | Unit: a view whose base-table map names a classified table matches; a view with an empty map matches by text and is flagged; a view over another engine's catalog does not match; a non-running engine is named "not checked". Live (Task 11): an MV over a classified Iceberg table is listed. |
| B1 no parsing | Mutation: replacing the EXPLAIN result with an empty input set must fail a test. The view is then committed untagged, so a test must see it. |
| B2 hold and refusal | DB test with a fake plugin: the view is not committed until the fake plugin downloads version ≥ V. A plugin that never polls gives a refusal, with the tags and lineage rolled back. |
| B3 transaction | DB test: classifying a base column updates every dependent view's tags in the same tag version. |
| B4 two instances | Unit: two plugin ids for one service; confirmation waits for both. |
| Real jars | Differential at Trino@2.5.0, Trino@2.8.0, StarRocks@2.8.0 and StarRocks@2.9.0, with 0 disagreements. |
| Live | Re-run the view matrix on Trino 483 and StarRocks 4.1.6:<br>• StarRocks: an uncleared reader gets `NULL` through plain, nested and INVOKER views; `create materialized view` is refused.<br>• Trino: a DEFINER view made by a cleared owner is refused to an uncleared reader on the **first** read after `CREATE VIEW` returns, with no window. A view on a classified table returns only after the engines confirm the tags.<br>• Page or preview against engine: any difference is Critical. |

## Not in this spec

- Column-level lineage for Trino. This could follow if Trino's event listener reports per-column lineage for views; that is not measured.
- Protecting data that a cleared user copies out.
- The tag-service grouping in the migration probe cap.
- A tag assigned before its catalog is attached being dropped from the download (§2.6 item 6c). B2 writes tags for catalogs that are already attached, and the plan checks this. The general defect is a separate ticket.
- The `POST /policies` empty-resource defect found in the engine-upgrade live check. That is a separate ticket.
