# StarRocks audit, platform side: the Query log (design)

**Status:** approved by the user. The four open questions were decided on 2026-11-10; the section
"Decided on 2026-11-10" at the end records the answers.

**Package:** A of the StarRocks follow-ups. The order is B (engine posture, merged as platform #847), then A,
then C (a probe of the paths no one has measured yet). A builds on two things B adds:
`egret_perm::is_patched_starrocks_fe` and the `POST /engines/{id}/security-upgrade` action.

> **Corrections (2026-11-10, from the plan).** Already stated in section 1 and pointed at from here:
> (1) "the provisioner creates the table" is not literal: `egret-provisioner` only renders
> Kubernetes objects, every FE statement goes through `st.starrocks_ddl` with `fe_target`, so the
> setup lives in `egret-engine-mgmt` (section 1, first paragraph). (2) The bootstrap does not run
> after every apply: `update_starrocks_engine` spawns `spawn_starrocks_catalog_wire` only when
> `deleg_changed`, and B's `security_upgrade` applies and returns, so an image change restarts the
> FE without a bootstrap; hence call site 2, `spawn_query_log_setup` (section 1, item 2).
> (4) The fork's e2e grants the loader nothing natively (Ranger's policy allows it); an egret
> engine bound to no profile runs native access control, so egret must `GRANT INSERT` there
> (section 1, step 4).

## Problem

egret cannot answer "who ran what on this StarRocks engine, and was it allowed".

- egret turns the Ranger plugin's own audit off (`xasecure.audit.is.enabled=false`, rendered into
  `ranger-starrocks-audit.xml`).
- The FE's `fe.audit.log` does record every SQL path, but it sits in the FE log directory, which
  is not on a volume (`fe-log` is an `emptyDir`, see `starrocks_cluster_cr`), so a pod restart
  loses it, and egret shows none of it.
- egret's delegated queries log in as a service user and run `EXECUTE AS <user>`, so the
  `User` column alone names the service user, not the person.

egret's fork (`ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4` and later) ships the
StarRocks AuditLoader plugin at `/opt/egret/auditloader.zip`, patched to escape every JSON value
and to record `command` and `querySource` (fork patch `auditloader/0001`). Nothing in egret
installs it yet.

## Decided (by the user)

- **Mechanism:** the AuditLoader plugin writes every audit event to a table inside the engine;
  egret reads that table and shows it. Rows are kept 30 days.
- **Where it shows:** a **Query log** tab on the Engine detail page, readable with `engine:read`,
  like the Trino Monitor tab (`crates/egret-engine-mgmt/src/monitor.rs`).
- **Scope:**
  - egret creates `starrocks_audit_db__.starrocks_audit_tbl__` with the fork's DDL (30-day
    `partition_retention_condition`);
  - a loader user with INSERT only on that table, plus a Ranger managed policy so Ranger
    enforcement does not block it;
  - `INSTALL PLUGIN FROM "/opt/egret/auditloader.zip"`, then a wait for the plugin refresh;
  - ordinary users are denied on the audit database;
  - an API and the Query log tab;
  - docs in egret-data/docs.
- **Images:** only egret's patched images (egret.4 and later) ship the plugin. An engine whose
  image has none is told so, and its apply does not fail. The setup runs again after B's security
  upgrade moves the engine to a patched image.

## Measured facts this builds on

From the fork's e2e (`tests/e2e/run.sh`, `audit_checks.py`) and earlier sessions, on egret.3/.4:

- The audit event covers MySQL text, PREPARE/EXECUTE, COM_STMT_PREPARE/EXECUTE, the HTTP SQL API
  (`HTTP.Query`), Arrow Flight SQL (`ARROW_FLIGHT_SQL.Query`) and materialized-view refresh
  (`querySource = MV`).
- `EXECUTE AS bob` by `svc` records `user = svc` and `authorizedUser = 'bob'@'%'`.
- A denial records `state = ERR` with the "Access denied ..." text in `errorCode`.
- Secrets in `FILES()` and catalog properties are redacted by StarRocks before the event.
- COM_STMT_EXECUTE records the policy-rewritten statement with the parameter values inlined, so
  its text shows mask and row-filter expressions and the values a user filtered on.
- StarRocks hands events only to the audit plugins it knew at its last refresh, once a minute
  (`AuditEventProcessor.UPDATE_PLUGIN_INTERVAL_MS`); the e2e waits for a probe row.
- With Ranger on, the loader needs only a Ranger allow of `insert` on the table; the e2e grants
  no native privilege (`policies.py`, policy `audit-loader`).
- `root` is not checked by Ranger (fork README, "Known limits"; upstream #72198).

**Not measured yet** (a step in the plan, before the code depends on it):
1. whether a plugin installed from a local path survives an FE pod restart, since the FE's
   `plugin_dir` is not on a volume.

**Measured in Task 5** (2026-11-10, `ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4`;
the setup and the read are also run by `crates/egret-engine-mgmt/tests/query_log_live.rs`
against it and against the official `starrocks/allin1-ubuntu:4.1.7`):
2. StarRocks redacts the passwords itself before the audit event: the stored `stmt` reads
   `CREATE USER … IDENTIFIED BY '***'`, `ALTER USER … IDENTIFIED BY '***'`,
   `SET PASSWORD FOR … = PASSWORD('***')` and `INSTALL PLUGIN … PROPERTIES (…, "password" = ***)`.
   egret's exact-value redaction stays as a second layer.
3. An absent zip answers `ERROR 1064 (HY000): Download plugin zip failed. zip file does not
   exist. source: <path>`. A second `INSTALL` answers `ERROR 1064 (HY000): plugin AuditLoader has
   already been installed.` An absent database answers `ERROR 5501 (3F000): Getting analyzing
   error. Detail message: Unknown database '<db>'.`, an absent table `ERROR 5502 (42602): …
   Unknown table '<db>.<table>'.`
4. `@@time_zone` and `@@system_time_zone` are `Etc/UTC`, and an event's `timestamp` matches
   `date -u` to the second, so the reads keep UTC.

Also measured: the `SHOW PLUGINS` header is `Name Type Description Version JavaVersion
ClassName SoName Sources Status Properties`; the audit `stmt` keeps a leading
`/* egret-query-log-read */` comment verbatim; `EXECUTE AS bob WITH NO REVERT` run by `carol`
records `user` = `carol` and `authorizedUser` = `'bob'@'%'`; `state` is `EOF` for a query, `OK`
for DDL and `ERR` for an error; events sent before the plugin's first refresh (about a minute
after `INSTALL`) are never loaded, and an `UNINSTALL` then `INSTALL` loses the events queued in
between; a user name cannot contain `'` (`invalid user name`).

## Design

### 1. Where the setup runs

The provisioner crate only renders Kubernetes objects; every SQL statement egret runs on an FE
goes through `st.starrocks_ddl` with the target from `starrocks_ddl::fe_target` (`root`, the
engine's root password, its pinned certificate). The setup therefore lives in
`egret-engine-mgmt`, in a new module `query_log.rs`, as one idempotent function:

```rust
pub(crate) async fn ensure_query_log(st: &CpState, engine: &Engine, fe: &FeTarget)
    -> QueryLogSetup;
```

It is called from three places:

1. **The post-Ready bootstrap** (`spawn_starrocks_catalog_wire`), after
   `converge_starrocks_catalogs` and before the delegation statements, behind its own
   `engine_still_current` checkpoint. This covers create and a delegation-changing update.
2. **After any accepted StarRocks apply that may restart the FE**, which the bootstrap does not
   cover: an image or config update in `update_starrocks_engine` and B's `security_upgrade`. Both
   spawn `spawn_query_log_setup(st, engine_id, name, namespace)`. It is the bootstrap's two wait
   loops (FE pod ready, then a real `root` login), factored out of `spawn_starrocks_catalog_wire`
   into one shared helper, followed by `ensure_query_log`.
3. **`POST /api/control-plane/engines/{id}/query-log/setup`** (`engine:update`): runs it once,
   synchronously, and answers the resulting status. This is how an engine created before this
   package gets its Query log, and the recovery after a failed or lost setup.

Every step is idempotent, so any of the three may run any number of times:

1. **Refuse a taken name.** If a human user holds the loader's username (section 2), stop with
   a failure that says so. Nothing is created.
2. **Database and table:** `CREATE DATABASE IF NOT EXISTS starrocks_audit_db__`, then the fork's
   `CREATE TABLE IF NOT EXISTS` (copied byte for byte from `tests/e2e/audit-table.sql` at fork
   commit `d77446e`, held as one constant).
3. **Loader user:** `CREATE USER IF NOT EXISTS 'egret_query_log'@'%' IDENTIFIED BY <pw>`, then
   `ALTER USER 'egret_query_log'@'%' IDENTIFIED BY <pw>`, so an existing user converges on the
   stored password. Both literals go through `egret_query_api::quote_string`, like
   `starrocks_create_service_user_sql`.
4. **Native grant, only when the engine is bound to no policy profile:**
   `GRANT INSERT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ TO USER 'egret_query_log'@'%'`.
   With a profile bound the FE runs `access_control = ranger`, native grants are not consulted,
   and the Ranger policy of section 3 is what allows the load.
5. **Plugin:** read `SHOW PLUGINS`. If a row named `AuditLoader` is there, stop: the plugin is
   installed. Otherwise:
   - if the stored FE image is not `is_patched_starrocks_fe`, stop with `pluginNotShipped`
     (section 4) and run no `INSTALL`;
   - else `INSTALL PLUGIN FROM "/opt/egret/auditloader.zip" PROPERTIES (...)` with `user`,
     `password`, `database`, `table`, `frontend_host_port = "127.0.0.1:8030"` and
     `max_batch_interval_sec = "60"` (upstream's default).
6. **Wait for events (background callers only).** Run a probe statement carrying a fresh UUID
   and poll the table for its row, every 5 seconds for up to 180 seconds (one plugin refresh, one
   batch interval, and margin). Log the outcome. The synchronous route does not wait; the tab
   shows the latest event time instead (section 6).

Every FE error is logged and returned only through `FeError::scrubbed(&fe, &secrets)`, with the
loader password in `secrets`.

**What `ensure_query_log` answers:**

```rust
pub(crate) enum QueryLogSetup {
    Ready,
    PluginNotShipped,
    Failed(String),
}
```

`Ready` means installed, not yet "events flowing".

### 2. The loader user and its password

- **Name:** `egret_query_log`, a constant `QUERY_LOG_LOADER_PRINCIPAL` in
  `egret_core::engine_principals`, beside `DELEGATOR_PRINCIPAL`. It is not the fork e2e's
  `egret_audit`, which reads like the `egret-audit` crate.
- **Reserved:** migration 0218 seeds it into `username_ledger` with source `reserved`, the way
  0209 step 5 reserved `egret_delegator`, so no human is minted that name. The setup's step 1
  covers a human who already held it before 0218.
- **Password:** 24 random bytes, URL-safe base64, stored as a global system secret named
  `egret-query-log-<engine id>`, minted and recovered by name exactly like
  `ensure_root_password_secret`. Its id goes on a new column
  `engine_mgmt.engines.query_log_secret_id` (migration 0218), set by a new
  `EngineMgmtStore::set_query_log_secret`. It is never rotated in this package.
- **Where the password goes:** the `CREATE`/`ALTER USER` statements and the plugin's install
  properties. StarRocks keeps the plugin's properties in its metadata; only `root` reads them.

### 3. Ranger: the loader's allow and the guard

Both are IR-owned infrastructure rows on the engine's StarRocks profile, converged by
`converge_infra_in_tx` like the `egret-admin - …` set, so a profile created later, a profile
re-bound, and the boot pass all get them.

- **Inputs:** `InfraInputs` gains `query_log: bool`, true when a provisioned (not external)
  StarRocks engine is bound to the profile. `load_infra_inputs` computes it.
- **Statements:** `lower_infra` emits two more statements for a StarRocks profile with
  `query_log`, whether or not it is delegating:
  - `InfraKind::QueryLogLoader`: user `egret_query_log`, capability `insert`, on
    `InfraObject::QueryLogTable`;
  - `InfraKind::QueryLogGuard`: everyone, a deny, on `InfraObject::QueryLogDatabase`, with
    `egret_query_log` excepted for `insert` only.
- **Rendered rows** (`ranger::infra::render`), with bodies built in `egret_policy`:
  - `egret-query-log-loader`: `default_catalog` / `starrocks_audit_db__` /
    `starrocks_audit_tbl__`, allow `insert` to `egret_query_log`;
  - `egret-query-log-guard - <chain>`: one row per resource hierarchy of the StarRocks
    service-def whose chain passes through `database` (database; table and column; view;
    materialized view; function). Each row denies group `public` every access type the def
    allows at that level, and its deny exception lets `egret_query_log` `insert` on the table.
- **Names:** both names go into `egret_policy_api::infra_names`, so `is_ir_infra_name` and
  `is_egret_infrastructure_policy` (StarRocks profiles only) recognise them, and a human cannot
  create or rename a policy into them.

**Consequence.** A Ranger deny beats an allow, so the guard also denies engine admins, whose
`egret-admin - …` rows allow everything, through any SQL client. Whether that is wanted is open
question 2. egret's own reads are unaffected: they log in as `root`, which Ranger does not check.
The FE sees no deny for `root`, and egret's own read is gated by `engine:read` (section 5).

An engine bound to no profile has no Ranger. Its users have no privilege on a database they did
not create unless someone grants one natively, which egret does not do.

### 4. The engine whose image has no plugin

An engine is `pluginNotShipped` when the FE has no `AuditLoader` plugin and its stored FE image
is not `is_patched_starrocks_fe` (official images, egret.1 to egret.3). This is the same rule
that drives B's `unpatchedImage`: the AuditLoader arrived at egret.4, the first complete level.

- The setup still creates the table and the loader user, so the upgrade has less to do, and does
  not run `INSTALL`. The apply and the bootstrap do not fail.
- The Query log tab says: "This engine's image has no audit plugin. **Upgrade security** moves
  it to egret's image, and the Query log starts after that." The button is B's.
- After the upgrade, call site 2 of section 1 runs the setup again, and the plugin is installed.

A patched image that answers `INSTALL` with an error (a hand-built image without the file) is
`Failed` with the scrubbed FE message. Its text, measured in Task 5 (fact 3), is pinned by the
test `a_missing_plugin_zip_is_failed_with_the_fes_diagnosis`.

### 5. Reading the log

All reads log in as `root` through `fe_target` and `st.starrocks_ddl.query`. Every value from the
request is validated, then quoted with `quote_string`. Each read statement starts with a fixed
marker comment, `/* egret-query-log-read */`, and the hint
`/*+ SET_VAR(query_timeout = 10) */`.

**`GET /api/control-plane/engines/{id}/query-log/status`** (`engine:read`) answers:
- `state`: one of
  - `ready`: the plugin is listed and the table exists;
  - `notSetUp`: a patched image, but the plugin or the table is missing;
  - `pluginNotShipped`: section 4;
  - `unreachable`: the FE did not answer, with the scrubbed message;
- `lastEventAt`: the newest `timestamp` in the last day, or absent;
- `retentionDays`: 30.

Every field is read from the FE at request time; nothing is stored, so the state describes the
engine as it is.

**`GET /api/control-plane/engines/{id}/query-log`** (`engine:read`), query parameters:

| Parameter | Meaning | Default and limits |
|---|---|---|
| `from`, `to` | RFC 3339 instants; rows with `from <= timestamp < to` | the last 24 hours; `to - from` at most 31 days; `from` before `to` |
| `user` | matches the session `user` or the name inside `authorizedUser` (exact, case-insensitive) | none |
| `state` | `ok` (`EOF`, `OK`), `error` (`ERR`) or `all` | `all` |
| `limit` | rows per page | 50; 1 to 200 |
| `cursor` | opaque, from the previous page's `nextCursor` | none |

Order is `timestamp` descending, then `queryId` descending. The cursor encodes the last row's
pair, and the next page asks for rows strictly before it (keyset paging, no offset). A malformed
value is a 400 naming the parameter.

**Each entry has:** `queryId`, `timestamp`, `user` (the session user), `authorizedUser` (the
bare name parsed from `'bob'@'%'`; absent when empty), `clientIp`, `catalog`, `db`, `state`,
`errorMessage` (from `errorCode`), `queryTimeMs`, `scanRows`, `scanBytes`, `returnRows`,
`cpuCostNs`, `memCostBytes`, `command`, `querySource`, `isQuery`, `digest`, and the statement:
`statement` cut to 2,000 characters, with `statementTruncated: bool`. The UI labels
`authorizedUser` as **User** and the session user as **Connected as**, so a delegated query
shows the person first.

**`GET /api/control-plane/engines/{id}/query-log/{queryId}`** (`engine:read`): one entry with
the whole statement, up to the column's 1 MiB. A `queryId` that is not a UUID is a 400; a
missing row is a 404.

**Who sees statement text** is open question 1. The design keeps it one decision: the statement
fields are filled by one function, `statement_visible(st, principal) -> bool`. Today it returns
true for `engine:read`.

**Redaction.** StarRocks redacts `FILES()` and catalog secrets itself. egret applies two more
rules to `statement` and `errorMessage` before they leave the server:
1. **Exact values:** the engine's root password and loader password, whenever either appears,
   are replaced by `***`.
2. **Shapes:** `IDENTIFIED BY '<x>'` and quoted property pairs whose key contains `password`,
   `passwd`, `secret`, `access_key` or `secret_key` keep the key and get `'***'` as the value.
   This is a SQL-literal-aware sibling of `log_redaction::redact_log_text`, whose pattern does
   not handle a quoted value with spaces.

**egret's own reads** are left out: rows with `user = 'root'` whose statement starts with the
marker comment are excluded in the SQL. Every other `root` statement, egret's own bootstrap DDL
included, is shown.

**Errors:**
- 404 for an unknown engine;
- 409 `engine_is_external` for an external engine;
- 409 for a non-StarRocks engine;
- 409 with code `query_log_not_ready` and the state when the table does not exist;
- 502 when the FE does not answer or rejects the read, with the scrubbed message.

### 6. The Query log tab

- **When it shows:** on `EngineDetail` (provisioned engines), only for StarRocks, as a new
  top-level tab after Monitor: `?tab=query-log`, added to `KNOWN_TABS`.
- **Above the table:** a status line from `/query-log/status`.
  - `ready`: "Kept 30 days. Latest event: <time>." When `lastEventAt` is absent: "No events
    yet. Events reach the log about a minute after a statement runs."
  - `notSetUp`: a `WarningCallout` that says "The query log is not set up on this engine: its
    audit plugin or its log table is missing.", with **Set up query log**, which calls
    `POST …/setup` and needs `engine:update`. When the button is disabled, it says why ("Setting
    it up needs the engine:update permission.").
  - `pluginNotShipped`: section 4's text, in one of two variants (Task 6):
    - when the engine carries B's `securityUpgradeNeeded`, the callout points at B's button, which
      sits in the security warning above the tabs: "This engine's image has no audit plugin.
      **Upgrade security**, in the security warning on this page, moves it to egret's image, and
      the Query log starts after that.";
    - without that flag there is no button to name: "This engine's image has no audit plugin.
      The Query log starts once the engine runs egret's image."
  - `unreachable`: the message, with a retry.
- **Filters**, kept in the URL: a time range (last hour, last 24 hours, last 7 days, last 30
  days, custom), User, and State (All / Succeeded / Failed).
- **Table** (`FilterableTable` or `ui/table`, FE-PAGE-4): Time, User, Connected as, State
  (`StatusBadge`), Command, Duration, Rows, and the statement's first line. A row opens a side
  panel with the full statement (`/query-log/{queryId}`), the error, and the counters. Paging is
  "Load more" with the cursor.
- **States:** follows FE-STATE-1/2/3: error first, then a skeleton, then `EmptyState`. A filter
  that matches nothing says so and offers to clear the filters.
- **Client:** regenerated with `npm run gen:api` only, through orval hooks and keys from
  `routes/control-plane/queryKeys.ts`.

### 7. Hardening that comes with it

- `plugin_enable` and `plugin_dir` join `SR_PROTECTED_FE_KEYS`, so an `feConfig` override cannot
  switch the plugin off or move it.
- If unmeasured fact 1 shows that the plugin is lost on an FE restart, the setup already
  re-installs it on every apply and on **Set up query log**. An FE that restarts on its own,
  outside an apply, shows `notSetUp` until then. The plan's measurement step decides whether
  this package also moves `plugin_dir` onto the FE meta volume for meta-persistent engines; the
  result is recorded and the user is told before that is built.

## Error handling

- The setup never fails an apply, a create or an upgrade. Its failure is logged with the scrubbed
  FE message, and the tab's state shows it as `notSetUp`.
- The synchronous setup route answers 200 with the status. A `Failed` setup is a 502 with the
  scrubbed message. Its other answers are 404, 409 `engine_is_external`, 409 for a non-StarRocks
  engine, and 409 while the engine is deleting.
- The read routes never return an unscrubbed FE message.

## Testing

Every behaviour change gets a test that fails without it, and a mutant of the change that the test
kills, recorded in the task report. A test that skips proves nothing: a gated test's report
gives its duration and its environment.

- **Unit:**
  - the setup's statements and their quoting (a password with `'` and `\`);
  - the native grant only when unbound;
  - the plugin decision table (listed, not patched, patched);
  - the redactor;
  - the `authorizedUser` parser;
  - the cursor round-trip;
  - parameter validation (range, window, limit, state);
  - the SQL each filter builds.
- **IR and rendering:** `lower_infra` emits the two query-log statements only for a StarRocks
  profile with `query_log`; the rendered bodies; the names in `infra_names`.
- **DB tests:**
  - convergence adds and removes the query-log rows as a provisioned StarRocks engine is bound
    and unbound;
  - the loader secret is minted once and recovered by name;
  - migration 0218 reserves the name;
  - the routes' 403/404/409 answers, with a `FakeStarRocksDdl` answering the FE.
- **Real StarRocks** (`EGRET_TEST_STARROCKS_URL`, against egret's `egret-starrocks-allin1`
  egret.4 image in a container the controller starts):
  - `ensure_query_log` twice (the second changes nothing);
  - a probe row arrives;
  - the read API's SQL returns it with the right `authorizedUser` for an `EXECUTE AS` statement;
  - egret's own read is excluded;
  - the same run against the official `starrocks/allin1-ubuntu:4.1.7` gives `pluginNotShipped`;
  - this run also measured facts 2 to 4 (see "Measured facts this builds on").
- **Fork e2e:** the guard and loader bodies, rendered by egret, are dropped into the fork's
  `policies.py` run (a controller step) to prove, on the real plugin, that an all-access admin
  and `eve` are denied `SELECT` and `DROP`, and that the loader still loads.
- **Frontend (vitest):** the tab exists only for StarRocks; each status state; the filters in
  the URL; paging; the side panel; the Set up button and its permission.
- **Live on kind:**
  - create a StarRocks engine and see events, a delegated query showing the person, and a
    denied query showing `Failed`;
  - restart the FE pod (measure fact 1);
  - upgrade a legacy engine with B's action and see the Query log start.

## Documentation (egret-data/docs, Platform section)

- **Engine detail:** the Query log tab, its states, what each column means, why "User" and
  "Connected as" differ, the 30-day retention, and the Set up action.
- **Security:**
  - the audit database, the loader user and the Ranger guard;
  - that statements can show parameter values and mask expressions, and who can see them (per
    open question 1);
  - what is redacted.
- **Upgrade notes:** existing engines on egret.4 or later get the Query log from **Set up query
  log**; older ones need **Upgrade security** first.
- **REST API:** the four routes.
- **Screenshots:** the tab with rows, the side panel, and the `pluginNotShipped` and `notSetUp`
  states.

## Out of scope

- External StarRocks engines: egret is not their root and does not install plugins on them.
- Trino query history: the Trino Monitor tab shows live queries only; a Trino event listener is
  another package.
- Rotating the loader password.
- Exporting the log to a store outside the engine (SIEM, object storage), and alerting on it.
- Ranger's own audit, unless open question 3 brings it in.

## Decided on 2026-11-10 (formerly the open questions)

1. **Who may read statement text: (b).** A new platform permission, `engine:audit` ("Read
   statement text in engine query logs"), granted by migration 0218 to the roles that hold
   `engine:update`. Without it, the tab shows every column but the statement.
   `statement_visible(st, principal)` checks `engine:audit`.
2. **Direct SQL on the audit database: (a).** The guard denies everyone, engine admins included.
   egret is the only reader, through `root`.
3. **Ranger's own audit: (a).** Not surfaced; `xasecure.audit.is.enabled=false` stays.
4. **Existing engines when egret is upgraded: (b), plus the button.** egret's boot pass runs the
   setup once for every reachable provisioned StarRocks engine, and **Set up query log**
   (section 6) stays for recovery.

The question texts and the options that were not taken are in this file's git history (commit
`2ec2461dd`).

## Decided on 2026-11-11 (Task 4 review)

1. **Error text needs `engine:audit` too.** StarRocks error messages quote the statement they
   reject (`Unexpected input 'xyz'`, `Cast '<literal>' to INT failed`), so `errorMessage` is
   statement text. Without `engine:audit`, an entry carries neither `statement` nor
   `errorMessage` (both absent from the JSON, not `null`), and `statementTruncated` is false.
   Every other field, `state` included, is unchanged, so a failed statement still reads as
   failed. This applies to the list and the detail route alike.
2. **The `user` filter reads `authorizedUser` the way the UI shows it.** The name is everything
   between the first `'` and the last `'@'<host>'`, with a doubled quote read as one, then
   lowercased: `` lower(replace(regexp_extract(authorizedUser, '^''(.*)''@''[^'']*''$', 1),
   '''''', '''')) ``. So `'o''brien'@'10.%'` is found by the user `o'brien`, the same name
   `authorizedUser` shows.
