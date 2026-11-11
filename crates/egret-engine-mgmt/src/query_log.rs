//! The Query log of a provisioned StarRocks engine: the audit table the
//! AuditLoader plugin of egret's FE image writes, its loader user, and the
//! plugin itself.
//!
//! [`ensure_query_log`] sets all of it up, idempotently, in the six steps of
//! the design (`docs/en/specs/2026-11-09-starrocks-audit-platform-design.md`,
//! section 1). It is called from the post-Ready bootstrap
//! (`engines::spawn_starrocks_catalog_wire`), from [`spawn_query_log_setup`]
//! after an apply that restarts the FE, and from [`setup_query_logs`] once at
//! start for the engines that existed before.
//!
//! Every SQL value that comes from a secret goes through
//! [`egret_query_api::quote_string`], and every FE error reaches a log or a
//! result only through [`FeError::scrubbed`] with the loader password among its
//! secrets.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use egret_auth::principal::Auth;
use egret_core::engine_principals::QUERY_LOG_LOADER_PRINCIPAL;
use egret_core::http::ErrorBody;
use egret_core::Principal;
use egret_policy_api::infra_names::{QUERY_LOG_DATABASE, QUERY_LOG_TABLE};
use egret_query_api::{quote_string, Engine as QueryEngine};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::engines::{engine_still_current, ensure_query_log_secret, wait_for_fe_root_login};
use crate::starrocks_ddl::{fe_target, FeError, FeTarget};
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;
use crate::{engine_status, Engine};

/// Step 2's first statement: the fork's `audit-table.sql` opens with it.
pub(crate) const QUERY_LOG_DATABASE_DDL: &str =
    "CREATE DATABASE IF NOT EXISTS starrocks_audit_db__";

/// Step 2's table, byte for byte the `CREATE TABLE` of the fork's
/// `tests/e2e/audit-table.sql` at commit `d77446e` (egret-starrocks): the
/// AuditLoader columns plus `command` and `querySource` (patch
/// `auditloader/0001`), one partition per day created on write, 30 days kept.
/// The file's leading comment and its `CREATE DATABASE` line are not part of
/// it ([`QUERY_LOG_DATABASE_DDL`] runs that one first, as its own statement).
pub(crate) const QUERY_LOG_TABLE_DDL: &str =
    "CREATE TABLE IF NOT EXISTS starrocks_audit_db__.starrocks_audit_tbl__ (
  `queryId` VARCHAR(64),
  `timestamp` DATETIME NOT NULL,
  `queryType` VARCHAR(12),
  `clientIp` VARCHAR(32),
  `user` VARCHAR(64),
  `authorizedUser` VARCHAR(64),
  `resourceGroup` VARCHAR(64),
  `catalog` VARCHAR(32),
  `db` VARCHAR(96),
  `state` VARCHAR(8),
  `errorCode` VARCHAR(512),
  `queryTime` BIGINT,
  `scanBytes` BIGINT,
  `scanRows` BIGINT,
  `returnRows` BIGINT,
  `cpuCostNs` BIGINT,
  `memCostBytes` BIGINT,
  `stmtId` INT,
  `isQuery` TINYINT,
  `feIp` VARCHAR(128),
  `stmt` VARCHAR(1048576),
  `digest` VARCHAR(32),
  `planCpuCosts` DOUBLE,
  `planMemCosts` DOUBLE,
  `pendingTimeMs` BIGINT,
  `candidateMVs` VARCHAR(65533) NULL,
  `hitMvs` VARCHAR(65533) NULL,
  `QueriedRelations` ARRAY<VARCHAR(65533)> NULL,
  `warehouse` VARCHAR(128) NULL,
  `command` VARCHAR(64) NULL,
  `querySource` VARCHAR(32) NULL
) ENGINE = OLAP
DUPLICATE KEY (`queryId`, `timestamp`, `queryType`)
PARTITION BY date_trunc('day', `timestamp`)
DISTRIBUTED BY HASH (`queryId`) BUCKETS 1
PROPERTIES (
  \"replication_num\" = \"1\",
  \"partition_retention_condition\" = \"`timestamp` >= current_date() - interval 30 day\"
);";

/// Where egret's FE image (`4.1.7-egret.4` and later) ships the plugin.
const AUDIT_LOADER_ZIP: &str = "/opt/egret/auditloader.zip";

/// The name the plugin registers under, as `SHOW PLUGINS` lists it.
const AUDIT_LOADER_PLUGIN: &str = "AuditLoader";

/// The `SHOW PLUGINS` column that carries a plugin's name.
const SHOW_PLUGINS_NAME_COLUMN: &str = "Name";

/// The `SHOW PLUGINS` column that carries a plugin's load state, and the
/// value a loaded plugin has there (measured in Task 5 on `4.1.7-egret.4`).
const SHOW_PLUGINS_STATUS_COLUMN: &str = "Status";
const PLUGIN_STATUS_INSTALLED: &str = "INSTALLED";

/// The probe statement's marker; a fresh UUID follows it on every run.
const PROBE_MARKER: &str = "egret-query-log-probe-";

/// Step 6's budget: one plugin refresh (a minute), one batch interval (a
/// minute) and margin. Shrunk in the test build like the bootstrap's
/// `MAX_POLLS`, so a test that reaches the wait costs no real sleep.
#[cfg(not(test))]
const PROBE_MAX_POLLS: u32 = 36;
#[cfg(test)]
const PROBE_MAX_POLLS: u32 = 1;
const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// The probe is sent again every this many polls (a minute): StarRocks hands
/// an event only to the audit plugins it knew at its last refresh, once a
/// minute, so a probe sent before the plugin was picked up is never loaded.
const PROBE_RESEND_EVERY: u32 = 12;

/// How many times [`spawn_query_log_setup`] runs the setup when it answers
/// `Failed`, and how long it waits between tries. Just after an apply the old
/// FE pod is often still Ready and logs in, so the first try can reach the pod
/// that is about to be replaced (an `INSTALL` on an official image that has no
/// plugin zip, say); a later try reaches the new one. Five tries a minute apart
/// cover an FE rollout. The test build keeps the tries and drops the wait.
const SETUP_ATTEMPTS: u32 = 5;
#[cfg(not(test))]
const SETUP_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(test)]
const SETUP_RETRY_DELAY: std::time::Duration = std::time::Duration::ZERO;

/// The wait before step 2 for an alive compute node or backend: a
/// shared-data FE refuses the audit table's `CREATE TABLE` with "Number of
/// available CN nodes is 0" until a CN has registered, and on kind the CN
/// joined after the FE's root login (live check, 2026-11-11). Polled every
/// [`NODE_POLL_INTERVAL`] for up to [`NODE_MAX_POLLS`] polls, about five
/// minutes; the test build keeps three polls (so a test can see the wait
/// end) and drops the sleep.
#[cfg(not(test))]
const NODE_MAX_POLLS: u32 = 30;
#[cfg(test)]
const NODE_MAX_POLLS: u32 = 3;
#[cfg(not(test))]
const NODE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
const NODE_POLL_INTERVAL: std::time::Duration = std::time::Duration::ZERO;
/// The node listings the wait reads, compute nodes first (a shared-data FE,
/// egret's default), then backends (a shared-nothing FE).
const NODE_LISTINGS: [&str; 2] = ["SHOW COMPUTE NODES", "SHOW BACKENDS"];
/// The column of both listings that says whether the node is alive, and the
/// value an alive node has there.
const NODE_ALIVE_COLUMN: &str = "Alive";
const NODE_ALIVE: &str = "true";

/// Whether step 6 sends the probe (again) on this poll: on the first, then
/// once every [`PROBE_RESEND_EVERY`] polls (a minute).
fn sends_probe(poll: u32) -> bool {
    poll.is_multiple_of(PROBE_RESEND_EVERY)
}

/// What [`ensure_query_log`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryLogSetup {
    /// The table, the loader user and the plugin are in place. Not yet "events
    /// are flowing": that is step 6's to observe.
    Ready,
    /// The FE has no `AuditLoader` and its image does not ship one. The table
    /// and the user were still created, so the security upgrade has less to do.
    PluginNotShipped,
    /// A step failed; the text is safe to log and to answer (scrubbed).
    Failed(String),
}

/// What step 5 does, decided by [`plugin_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginStep {
    AlreadyInstalled,
    Install,
    /// Listed, but not loaded (a `Status` other than `INSTALLED`, `ERROR`
    /// say: an FE that replayed the `INSTALL` from its meta after its pod was
    /// re-created, without the plugin files). `UNINSTALL`, then `INSTALL`.
    Reinstall,
    NotShipped,
}

/// Steps 2 to 4, in order, with the loader password inline: the database, the
/// table, the loader user (`CREATE … IF NOT EXISTS`, then `ALTER` so an
/// existing user converges on the stored password), and, only when the engine
/// is bound to no policy profile, the native `GRANT INSERT`. A bound engine's
/// FE runs `access_control = ranger`, which does not consult native grants;
/// the Ranger policy `egret-query-log-loader` allows the load there.
pub(crate) fn query_log_setup_sql(password: &str, bound_to_profile: bool) -> Vec<String> {
    let user = quote_string(QueryEngine::StarRocks, QUERY_LOG_LOADER_PRINCIPAL);
    let pw = quote_string(QueryEngine::StarRocks, password);
    let mut out = vec![
        QUERY_LOG_DATABASE_DDL.to_string(),
        QUERY_LOG_TABLE_DDL.to_string(),
        format!("CREATE USER IF NOT EXISTS {user}@'%' IDENTIFIED BY {pw}"),
        format!("ALTER USER {user}@'%' IDENTIFIED BY {pw}"),
    ];
    if !bound_to_profile {
        out.push(format!(
            "GRANT INSERT ON TABLE {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} TO USER {user}@'%'"
        ));
    }
    out
}

/// Step 5's `INSTALL PLUGIN`. `frontend_host_port` is the FE's own HTTP port
/// on loopback (the plugin stream-loads into its own FE);
/// `max_batch_interval_sec` is upstream's default.
pub(crate) fn install_plugin_sql(password: &str) -> String {
    let q = |v: &str| quote_string(QueryEngine::StarRocks, v);
    format!(
        "INSTALL PLUGIN FROM \"{AUDIT_LOADER_ZIP}\" PROPERTIES (\
         \"user\" = {}, \"password\" = {}, \"database\" = {}, \"table\" = {}, \
         \"frontend_host_port\" = {}, \"max_batch_interval_sec\" = {})",
        q(QUERY_LOG_LOADER_PRINCIPAL),
        q(password),
        q(QUERY_LOG_DATABASE),
        q(QUERY_LOG_TABLE),
        q("127.0.0.1:8030"),
        q("60"),
    )
}

/// Step 5's decision, from the `SHOW PLUGINS` answer of
/// [`crate::starrocks_ddl::StarRocksDdl::query_with_header`] (its first row is
/// the header) and the engine's stored FE image. The name and the status are
/// read from the columns whose headers are `Name` and `Status`, wherever the FE
/// puts them; an answer with no `Name` column lists nothing.
///
/// Only an `AuditLoader` row whose `Status` is `INSTALLED` is installed. A row
/// with any other status is a plugin the FE could not load, so it is
/// re-installed where the image ships the plugin ([`PluginStep::Reinstall`])
/// and [`PluginStep::NotShipped`] elsewhere. An answer with no `Status` column
/// (not what any measured FE answers) keeps the older reading: a listed name
/// is installed.
pub(crate) fn plugin_step(
    show_plugins: &[Vec<Option<String>>],
    fe_image: Option<&str>,
) -> PluginStep {
    let column = |header: &[Option<String>], name: &str| {
        header.iter().position(|c| c.as_deref() == Some(name))
    };
    // `None`: not listed; `Some(loaded)`: listed, and whether it is loaded.
    let listed: Option<bool> = show_plugins.split_first().and_then(|(header, rows)| {
        let name = column(header, SHOW_PLUGINS_NAME_COLUMN)?;
        let status = column(header, SHOW_PLUGINS_STATUS_COLUMN);
        let row = rows
            .iter()
            .find(|r| r.get(name).and_then(|v| v.as_deref()) == Some(AUDIT_LOADER_PLUGIN))?;
        Some(match status {
            Some(i) => row
                .get(i)
                .and_then(|v| v.as_deref())
                .is_some_and(|v| v.trim().eq_ignore_ascii_case(PLUGIN_STATUS_INSTALLED)),
            // No `Status` column to read: trust the listing.
            None => true,
        })
    });
    let ships = fe_image.is_some_and(egret_perm::is_patched_starrocks_fe);
    match listed {
        Some(true) => PluginStep::AlreadyInstalled,
        Some(false) | None if !ships => PluginStep::NotShipped,
        Some(false) => PluginStep::Reinstall,
        None => PluginStep::Install,
    }
}

/// Whether a node listing (`SHOW COMPUTE NODES` or `SHOW BACKENDS`, as
/// [`crate::starrocks_ddl::StarRocksDdl::query_with_header`] answers it, its
/// first row the header) lists a node whose `Alive` column is `true`. The
/// column is found by its header; an answer without one lists no alive node.
pub(crate) fn lists_an_alive_node(listing: &[Vec<Option<String>>]) -> bool {
    let Some((header, rows)) = listing.split_first() else {
        return false;
    };
    let Some(alive) = header
        .iter()
        .position(|c| c.as_deref() == Some(NODE_ALIVE_COLUMN))
    else {
        return false;
    };
    rows.iter().any(|r| {
        r.get(alive)
            .and_then(|v| v.as_deref())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(NODE_ALIVE))
    })
}

/// Wait, up to `max_polls` polls [`NODE_POLL_INTERVAL`] apart, until the FE
/// lists an alive compute node or backend. `Ok` says how many polls it took;
/// `Err` carries the last listing error, scrubbed, if there was one. Both
/// listings start with
/// [`READ_MARKER`], so these polls stay out of the log's list and its
/// newest event.
async fn wait_for_an_alive_node(
    st: &CpState,
    fe: &FeTarget,
    secrets: &BTreeMap<Uuid, String>,
    max_polls: u32,
) -> Result<u32, Option<String>> {
    let mut last_error = None;
    for poll in 0..max_polls {
        for listing in NODE_LISTINGS {
            let sql = format!("{READ_MARKER} {listing}");
            match st.starrocks_ddl.query_with_header(fe, &sql).await {
                Ok(rows) if lists_an_alive_node(&rows) => return Ok(poll + 1),
                Ok(_) => {}
                Err(e) => last_error = Some(e.scrubbed(fe, secrets)),
            }
        }
        if poll + 1 < max_polls {
            tokio::time::sleep(NODE_POLL_INTERVAL).await;
        }
    }
    Err(last_error)
}

/// Set up `engine`'s Query log on the FE `fe` (design section 1, steps 1 to
/// 6). Idempotent, so any caller may run it any number of times. It never
/// fails its caller: what went wrong comes back as [`QueryLogSetup::Failed`]
/// with a scrubbed message, and is logged here. Before step 2 it waits, up
/// to about five minutes, for an alive compute node or backend
/// ([`NODE_MAX_POLLS`]). `wait_for_events` runs step 6 (up to three minutes)
/// and is for background callers only.
pub(crate) async fn ensure_query_log(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    wait_for_events: bool,
) -> QueryLogSetup {
    ensure_query_log_within(st, engine, fe, wait_for_events, NODE_MAX_POLLS).await
}

/// [`ensure_query_log`] with the node wait bounded to `node_polls` polls.
/// The synchronous setup route checks once (`1`): a request must not hang
/// for five minutes on an engine with no node, and its `Failed` says why.
async fn ensure_query_log_within(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    wait_for_events: bool,
    node_polls: u32,
) -> QueryLogSetup {
    let setup = setup_steps(st, engine, fe, node_polls).await;
    if let QueryLogSetup::Failed(why) = &setup {
        tracing::warn!(engine = %engine.name, error = %why, "query log setup failed");
    }
    if wait_for_events && setup == QueryLogSetup::Ready {
        self::wait_for_events(st, fe, &engine.name).await;
    }
    setup
}

/// Steps 1 to 5, with the wait for an alive node (`node_polls` polls at most)
/// before step 2.
async fn setup_steps(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    node_polls: u32,
) -> QueryLogSetup {
    // 1. A human who already holds the loader's name (from before migration
    //    0218 reserved it) would be handed the loader's privileges on the FE.
    match st
        .engine_mgmt
        .username_taken(QUERY_LOG_LOADER_PRINCIPAL)
        .await
    {
        Ok(false) => {}
        Ok(true) => {
            return QueryLogSetup::Failed(format!(
                "a user named {QUERY_LOG_LOADER_PRINCIPAL} exists; the query log loads as a \
                 StarRocks user of that name, so it was not set up. Rename that user, then set \
                 up the query log again"
            ))
        }
        Err(e) => {
            return QueryLogSetup::Failed(format!(
                "could not check whether the loader's username is taken: {e}"
            ))
        }
    }
    let (secret_id, password) = match ensure_query_log_secret(st, engine).await {
        Ok(v) => v,
        Err(e) => {
            return QueryLogSetup::Failed(format!("could not resolve the loader password: {e}"))
        }
    };
    let secrets = BTreeMap::from([(secret_id, password.clone())]);
    let failed = |e: FeError| QueryLogSetup::Failed(e.scrubbed(fe, &secrets));

    // The table's `CREATE` needs an alive compute node (or backend).
    match wait_for_an_alive_node(st, fe, &secrets, node_polls).await {
        Ok(1) => {}
        // It waited: the engine may have been deleted, renamed or failed
        // meanwhile, and the statements below write to its FE.
        Ok(_) => {
            let mut read_errors = 0;
            if !engine_still_current(&st.engine_mgmt, engine.id, &engine.name, &mut read_errors)
                .await
            {
                return QueryLogSetup::Failed(
                    "the engine changed while the setup waited for a compute node; nothing \
                     was created"
                        .to_string(),
                );
            }
        }
        Err(last_error) => {
            let waited = if node_polls > 1 {
                format!(" after {:?}", NODE_POLL_INTERVAL * (node_polls - 1))
            } else {
                String::new()
            };
            return QueryLogSetup::Failed(format!(
                "no compute node or backend of this engine is alive{waited}; the query log \
                 table needs one. Set up the query log again once a node has joined{}",
                last_error
                    .map(|e| format!(" (last check: {e})"))
                    .unwrap_or_default()
            ));
        }
    }

    // 2-4. The table, the loader user, the native grant when unbound.
    for stmt in query_log_setup_sql(&password, engine.authz_service_id.is_some()) {
        if let Err(e) = st.starrocks_ddl.execute(fe, &stmt).await {
            // A node that was alive above and is gone again: the same
            // retryable `Failed`, said plainly.
            if e.is_no_alive_node() {
                return QueryLogSetup::Failed(format!(
                    "the FE has no alive compute node, so the query log table could not be \
                     created; it is set up again once a node has joined: {}",
                    e.scrubbed(fe, &secrets)
                ));
            }
            return failed(e);
        }
    }

    // 5. The plugin.
    let plugins = match st.starrocks_ddl.query_with_header(fe, "SHOW PLUGINS").await {
        Ok(rows) => rows,
        Err(e) => return failed(e),
    };
    let image = egret_perm::stored_engine_image("starrocks", &engine.spec);
    let step = plugin_step(&plugins, image.as_deref());
    if step == PluginStep::Reinstall {
        // A listed row refuses a second `INSTALL` ("already installed"), so
        // the row the FE could not load goes first.
        let uninstall = format!("UNINSTALL PLUGIN {AUDIT_LOADER_PLUGIN}");
        if let Err(e) = st.starrocks_ddl.execute(fe, &uninstall).await {
            return failed(e);
        }
    }
    match step {
        PluginStep::AlreadyInstalled => QueryLogSetup::Ready,
        PluginStep::NotShipped => QueryLogSetup::PluginNotShipped,
        PluginStep::Install | PluginStep::Reinstall => match st
            .starrocks_ddl
            .execute(fe, &install_plugin_sql(&password))
            .await
        {
            Ok(()) => QueryLogSetup::Ready,
            // Another setup on this engine (a bootstrap and an update's spawn
            // that overlap) installed it between our `SHOW PLUGINS` and here.
            Err(e) if e.is_already_installed() => QueryLogSetup::Ready,
            Err(e) => failed(e),
        },
    }
}

/// Step 6: send a probe statement carrying a fresh UUID and poll the table for
/// its row, every [`PROBE_INTERVAL`] for up to [`PROBE_MAX_POLLS`] polls. Only
/// logs; whether the row arrived changes nothing. The poll's own text does not
/// contain the marker (it is split across `concat`), so the poll is never
/// mistaken for the probe once it is logged itself.
///
/// Both statements start with [`READ_MARKER`], like every read of the log, so
/// the list, the detail and the status's newest event leave them out (final
/// review, M1): they run after every create, update, security upgrade and
/// start, and would otherwise fill the log and keep "Latest event" fresh with
/// no user activity. The poll still finds its probe: it matches the probe's
/// UUID and does not apply that exclusion.
pub(crate) async fn wait_for_events(st: &CpState, fe: &FeTarget, engine_name: &str) -> bool {
    let id = Uuid::new_v4();
    let probe = probe_sql(&id);
    let check = probe_check_sql(&id);
    let no_secrets = BTreeMap::new();
    for poll in 0..PROBE_MAX_POLLS {
        if sends_probe(poll) {
            if let Err(e) = st.starrocks_ddl.query(fe, &probe).await {
                tracing::debug!(engine = %engine_name, error = %e.scrubbed(fe, &no_secrets),
                    "query log: the probe statement failed");
            }
        }
        match st.starrocks_ddl.query(fe, &check).await {
            Ok(rows) => {
                let count = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_deref())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                if count > 0 {
                    tracing::info!(engine = %engine_name,
                        "query log: events are being recorded");
                    return true;
                }
            }
            Err(e) => {
                tracing::debug!(engine = %engine_name, error = %e.scrubbed(fe, &no_secrets),
                    "query log: reading the probe back failed");
            }
        }
        if poll + 1 < PROBE_MAX_POLLS {
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    }
    tracing::warn!(engine = %engine_name,
        "query log: the plugin is installed but no event arrived within three minutes; \
         check the FE log for the AuditLoader");
    false
}

/// Step 6's probe: a statement whose audit row carries `id` after
/// [`PROBE_MARKER`], marked as egret's own read.
pub(crate) fn probe_sql(id: &Uuid) -> String {
    format!(
        "{READ_MARKER} SELECT {}",
        quote_string(QueryEngine::StarRocks, &format!("{PROBE_MARKER}{id}"))
    )
}

/// Step 6's poll: how many audit rows carry the probe `id`. It matches the
/// probe by its UUID alone, without [`own_reads_excluded`], so it finds the
/// marked probe; its own text splits the marker from the UUID, so it never
/// counts itself.
pub(crate) fn probe_check_sql(id: &Uuid) -> String {
    format!(
        "{READ_MARKER} SELECT count(*) FROM {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} \
         WHERE stmt LIKE concat('%{PROBE_MARKER}', {}, '%')",
        quote_string(QueryEngine::StarRocks, &id.to_string())
    )
}

/// Log what a background setup came to. `Failed` is logged where it is
/// decided: by [`ensure_query_log`], or by [`spawn_query_log_setup`]'s last try.
pub(crate) fn log_outcome(engine_name: &str, setup: &QueryLogSetup) {
    match setup {
        QueryLogSetup::Ready => {
            tracing::info!(engine = %engine_name, "query log: set up")
        }
        QueryLogSetup::PluginNotShipped => tracing::info!(engine = %engine_name,
            "query log: the table and loader user are set up, but this engine's image has no \
             audit plugin; Upgrade security moves it to egret's image"),
        QueryLogSetup::Failed(_) => {}
    }
}

/// After an apply that may restart the FE, and at boot: wait for the FE pod
/// and a real `root` login exactly as the bootstrap does
/// ([`wait_for_fe_root_login`]), run the setup, and on `Failed` try again,
/// up to [`SETUP_ATTEMPTS`] times [`SETUP_RETRY_DELAY`] apart
/// ([`retry_query_log_setup`]). Spawned, never awaited by a request; the
/// handle is for tests.
pub(crate) fn spawn_query_log_setup(
    st: CpState,
    engine_id: Uuid,
    name: String,
    namespace: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { retry_query_log_setup(&st, engine_id, &name, &namespace, 1).await })
}

/// The setup's bounded retry, shared by [`spawn_query_log_setup`] (from the
/// first try) and the bootstrap (`engines::spawn_starrocks_catalog_wire`, from
/// the second, after its own first try failed). Tries `first_attempt` to
/// [`SETUP_ATTEMPTS`], [`SETUP_RETRY_DELAY`] apart (a `first_attempt` after
/// the first waits that delay before it too), each behind the waits and
/// checkpoints of [`wait_for_fe_root_login`] and [`engine_still_current`], so
/// a retry stops when the engine is deleted, renamed or failed. `Ready` waits
/// for the first event (step 6); `Ready` and `PluginNotShipped` end it. Only
/// the last `Failed` is a warning.
pub(crate) async fn retry_query_log_setup(
    st: &CpState,
    engine_id: Uuid,
    name: &str,
    namespace: &str,
    first_attempt: u32,
) {
    if first_attempt > 1 {
        tokio::time::sleep(SETUP_RETRY_DELAY).await;
    }
    let mut consecutive_read_errors: u32 = 0;
    for attempt in first_attempt..=SETUP_ATTEMPTS {
        let Some((engine, fe)) =
            wait_for_fe_root_login(st, engine_id, name, namespace, &mut consecutive_read_errors)
                .await
        else {
            return;
        };
        if !engine_still_current(
            &st.engine_mgmt,
            engine_id,
            name,
            &mut consecutive_read_errors,
        )
        .await
        {
            return;
        }
        match setup_steps(st, &engine, &fe, NODE_MAX_POLLS).await {
            QueryLogSetup::Failed(why) if attempt < SETUP_ATTEMPTS => {
                tracing::info!(engine = %name, attempt, error = %why,
                    "query log setup failed; trying again");
                tokio::time::sleep(SETUP_RETRY_DELAY).await;
            }
            QueryLogSetup::Failed(why) => {
                tracing::warn!(engine = %name, attempts = attempt, error = %why,
                    "query log setup failed");
                return;
            }
            setup @ QueryLogSetup::Ready => {
                log_outcome(name, &setup);
                wait_for_events(st, &fe, name).await;
                return;
            }
            setup @ QueryLogSetup::PluginNotShipped => {
                log_outcome(name, &setup);
                return;
            }
        }
    }
}

/// Whether the boot pass sets this engine's Query log up: a StarRocks engine
/// egret provisioned that is not being deleted or given up on.
pub(crate) fn boot_pass_takes(engine: &Engine) -> bool {
    engine.r#type == "starrocks"
        && !crate::external::is_external(engine)
        && engine.status != engine_status::DELETING
        && engine.status != engine_status::FAILED
}

/// Once at start: [`spawn_query_log_setup`] for every provisioned StarRocks
/// engine that is not `deleting` or `failed`, so an engine created before the
/// Query log existed gets one (open question 4, decided (b)). Returns how many
/// were started; each runs in the background and logs its own outcome.
pub async fn setup_query_logs(st: &CpState) -> usize {
    let engines = match st.engine_mgmt.list_engines().await {
        Ok(engines) => engines,
        Err(e) => {
            tracing::error!(error = %e, "query log boot pass: listing the engines failed");
            return 0;
        }
    };
    let mut started = 0;
    for engine in engines.into_iter().filter(boot_pass_takes) {
        spawn_query_log_setup(st.clone(), engine.id, engine.name, engine.namespace);
        started += 1;
    }
    tracing::info!(
        engines = started,
        "query log boot pass: setting up the query log of the StarRocks engines"
    );
    started
}

// ── Reading the log (design section 5) ────────────────────────────────────────
//
// Four routes on the engines router: the status, a page of entries, one entry,
// and the synchronous setup. Every read logs in as `root` through
// [`fe_target`], starts with [`READ_MARKER`] (so the SQL can leave egret's own
// reads out of what it shows), and carries only values egret validated and
// then quoted with `quote_string`, or formatted itself. Every FE error leaves
// through [`FeError::scrubbed`] with the loader password among its secrets
// (the root password is the connection's, which `scrubbed` adds itself), and
// every statement and error text leaves through [`redact_statement`].

/// The comment every read of the log starts with. Rows whose statement starts
/// with it and that `root` ran are egret's own reads and are left out.
pub(crate) const READ_MARKER: &str = "/* egret-query-log-read */";

/// The hint every `SELECT` of the log carries after the marker.
const READ_HINT: &str = "/*+ SET_VAR(query_timeout = 10) */";

/// How long the table keeps a partition (`QUERY_LOG_TABLE_DDL`'s
/// `partition_retention_condition`).
const RETENTION_DAYS: u32 = 30;

/// How much of a statement a page carries; the detail route carries it whole.
const STATEMENT_PREVIEW_CHARS: u32 = 2000;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;
const MAX_USER_CHARS: usize = 128;
const MAX_RANGE_DAYS: i64 = 31;

/// The text form of a `DATETIME` egret writes into SQL and into a cursor.
const SQL_DATETIME: &str = "%Y-%m-%d %H:%M:%S%.6f";
/// What it parses a `DATETIME` back with (the fraction is optional).
const SQL_DATETIME_IN: &str = "%Y-%m-%d %H:%M:%S%.f";

/// The query log's state, as the Query log tab shows it (design section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum QueryLogState {
    /// The plugin is listed and the table exists.
    Ready,
    /// A patched image, but the plugin or the table is missing.
    NotSetUp,
    /// The engine's image does not ship the plugin (design section 4).
    PluginNotShipped,
    /// The FE did not answer; `message` says why.
    Unreachable,
}

impl QueryLogState {
    /// The serialized name, for the `state` of a 409 `query_log_not_ready`.
    fn as_str(self) -> &'static str {
        match self {
            QueryLogState::Ready => "ready",
            QueryLogState::NotSetUp => "notSetUp",
            QueryLogState::PluginNotShipped => "pluginNotShipped",
            QueryLogState::Unreachable => "unreachable",
        }
    }
}

/// `GET …/query-log/status` and `POST …/query-log/setup`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryLogStatusDto {
    pub state: QueryLogState,
    /// Why the state is `unreachable` (the FE's message, scrubbed). `null`
    /// otherwise.
    pub message: Option<String>,
    /// The newest event in the last day (RFC 3339); `null` when there is none
    /// or the table does not exist.
    pub last_event_at: Option<String>,
    /// How many days of events the table keeps.
    pub retention_days: u32,
}

/// One statement the engine ran.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryLogEntryDto {
    pub query_id: String,
    /// When the statement ended (RFC 3339).
    pub timestamp: String,
    /// The session user ("Connected as").
    pub user: Option<String>,
    /// The bare name of the user the statement ran for ("User"), parsed from
    /// `'bob'@'%'`; `null` when empty.
    pub authorized_user: Option<String>,
    pub client_ip: Option<String>,
    pub catalog: Option<String>,
    pub db: Option<String>,
    /// `EOF` or `OK` for a success, `ERR` for a failure.
    pub state: Option<String>,
    /// The FE's error, redacted. Absent (not `null`) for a caller without
    /// `engine:audit`: StarRocks errors quote the statement's text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub query_time_ms: Option<i64>,
    pub scan_rows: Option<i64>,
    pub scan_bytes: Option<i64>,
    pub return_rows: Option<i64>,
    pub cpu_cost_ns: Option<i64>,
    pub mem_cost_bytes: Option<i64>,
    pub command: Option<String>,
    pub query_source: Option<String>,
    pub is_query: Option<bool>,
    pub digest: Option<String>,
    /// The statement, redacted: its first 2,000 characters on a page, whole
    /// on the detail route. Absent for a caller without `engine:audit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    /// Whether `statement` was cut. Always false when it is absent.
    pub statement_truncated: bool,
}

/// `GET …/query-log`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryLogPageDto {
    pub entries: Vec<QueryLogEntryDto>,
    /// Pass it as `cursor` for the next page; `null` on the last.
    pub next_cursor: Option<String>,
}

/// `GET …/query-log`'s query parameters. Taken as text and validated by
/// [`parse_filter`], so a malformed value is a 400 naming its parameter.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct QueryLogParams {
    /// RFC 3339; rows with `from <= timestamp`. Default: `to` minus 24 hours.
    #[param(value_type = Option<String>, format = DateTime)]
    pub from: Option<String>,
    /// RFC 3339; rows with `timestamp < to`. Default: now. At most 31 days
    /// after `from`.
    #[param(value_type = Option<String>, format = DateTime)]
    pub to: Option<String>,
    /// The session user or the user inside `authorizedUser`, exact and
    /// case-insensitive; at most 128 characters.
    pub user: Option<String>,
    /// `ok`, `error` or `all` (default).
    pub state: Option<String>,
    /// Rows per page, 1 to 200 (default 50).
    #[param(value_type = Option<u32>, minimum = 1, maximum = 200)]
    pub limit: Option<String>,
    /// The previous page's `nextCursor`.
    pub cursor: Option<String>,
}

/// Which statements a page shows, by their `state` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateFilter {
    All,
    Ok,
    Error,
}

/// A validated [`QueryLogParams`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QueryLogFilter {
    pub(crate) from: DateTime<Utc>,
    pub(crate) to: DateTime<Utc>,
    /// Lowercased.
    pub(crate) user: Option<String>,
    pub(crate) state: StateFilter,
    pub(crate) limit: u32,
    pub(crate) before: Option<(NaiveDateTime, Uuid)>,
}

fn bad(param: &str, why: &str) -> CpError {
    CpError::BadRequest(format!("{param}: {why}"))
}

/// Validate the list route's parameters against `now` (the default `to`).
pub(crate) fn parse_filter(
    q: &QueryLogParams,
    now: DateTime<Utc>,
) -> Result<QueryLogFilter, CpError> {
    let instant = |param: &str, v: &Option<String>| {
        v.as_deref()
            .map(|s| {
                DateTime::parse_from_rfc3339(s)
                    .map(|t| t.with_timezone(&Utc))
                    .map_err(|_| bad(param, "must be an RFC 3339 instant"))
            })
            .transpose()
    };
    let to = instant("to", &q.to)?.unwrap_or(now);
    let from = instant("from", &q.from)?.unwrap_or(to - chrono::Duration::hours(24));
    if from >= to {
        return Err(bad("from", "must be before to"));
    }
    if to - from > chrono::Duration::days(MAX_RANGE_DAYS) {
        return Err(bad("to", "must be at most 31 days after from"));
    }
    let user = match q.user.as_deref() {
        None | Some("") => None,
        Some(u) if u.chars().count() > MAX_USER_CHARS => {
            return Err(bad("user", "must be at most 128 characters"))
        }
        Some(u) => Some(u.to_lowercase()),
    };
    let state = match q.state.as_deref() {
        None | Some("all") => StateFilter::All,
        Some("ok") => StateFilter::Ok,
        Some("error") => StateFilter::Error,
        Some(_) => return Err(bad("state", "must be ok, error or all")),
    };
    let limit = match q.limit.as_deref() {
        None => DEFAULT_LIMIT,
        Some(l) => l
            .parse::<u32>()
            .ok()
            .filter(|l| (1..=MAX_LIMIT).contains(l))
            .ok_or_else(|| bad("limit", "must be a whole number from 1 to 200"))?,
    };
    let before = q.cursor.as_deref().map(decode_cursor).transpose()?;
    Ok(QueryLogFilter {
        from,
        to,
        user,
        state,
        limit,
        before,
    })
}

/// An instant as a `DATETIME` literal for the FE.
fn sql_datetime(t: &NaiveDateTime) -> String {
    // UTC: measured in Task 5 on egret's `4.1.7-egret.4` image, whose
    // `@@time_zone` and `@@system_time_zone` are `Etc/UTC` and whose audit
    // `timestamp` matched `date -u` to the second. This is the one place the
    // instants of a read become FE wall-clock text.
    format!("'{}'", t.format(SQL_DATETIME))
}

/// The self-exclusion clause: egret's own reads of the log, left out.
fn own_reads_excluded() -> String {
    format!("NOT (`user` = 'root' AND stmt LIKE '{READ_MARKER}%')")
}

/// The columns an entry is read from, in [`entry_from_row`]'s order, up to
/// and without the statement pair.
const ENTRY_COLUMNS: &str = "queryId, timestamp, `user`, authorizedUser, clientIp, catalog, db, \
     state, errorCode, queryTime, scanRows, scanBytes, returnRows, cpuCostNs, memCostBytes, \
     command, querySource, isQuery, digest";

/// [`authorized_name`] in SQL, lowercased: the name is everything between the
/// first `'` and the last `'@'…'` (greedy, so a quote inside the name is
/// kept), with a doubled quote `''` read as one. A value not of that form
/// extracts as `''`, which no filter value equals (an empty `user` is no
/// filter).
const AUTHORIZED_NAME_SQL: &str = "lower(replace(regexp_extract(authorizedUser, \
     '^''(.*)''@''[^'']*''$', 1), '''''', ''''))";

/// A page of the log, newest first, one row more than `limit` (the extra row
/// decides `nextCursor`).
pub(crate) fn list_sql(f: &QueryLogFilter) -> String {
    let mut sql = format!(
        "{READ_MARKER} SELECT {READ_HINT} {ENTRY_COLUMNS}, \
         left(stmt, {STATEMENT_PREVIEW_CHARS}), char_length(stmt) > {STATEMENT_PREVIEW_CHARS} \
         FROM {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} \
         WHERE timestamp >= {} AND timestamp < {}",
        sql_datetime(&f.from.naive_utc()),
        sql_datetime(&f.to.naive_utc()),
    );
    if let Some(user) = &f.user {
        let q = quote_string(QueryEngine::StarRocks, user);
        sql.push_str(&format!(
            " AND (lower(`user`) = {q} OR {AUTHORIZED_NAME_SQL} = {q})"
        ));
    }
    match f.state {
        StateFilter::All => {}
        StateFilter::Ok => sql.push_str(" AND state IN ('EOF', 'OK')"),
        StateFilter::Error => sql.push_str(" AND state = 'ERR'"),
    }
    if let Some((ts, id)) = &f.before {
        let ts = sql_datetime(ts);
        let id = quote_string(QueryEngine::StarRocks, &id.to_string());
        sql.push_str(&format!(
            " AND (timestamp < {ts} OR (timestamp = {ts} AND queryId < {id}))"
        ));
    }
    sql.push_str(&format!(
        " AND {} ORDER BY timestamp DESC, queryId DESC LIMIT {}",
        own_reads_excluded(),
        f.limit + 1
    ));
    sql
}

/// One entry, its statement whole.
fn detail_sql(query_id: &Uuid) -> String {
    format!(
        "{READ_MARKER} SELECT {READ_HINT} {ENTRY_COLUMNS}, stmt, false \
         FROM {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} \
         WHERE queryId = {} AND {} \
         ORDER BY timestamp DESC LIMIT 1",
        quote_string(QueryEngine::StarRocks, &query_id.to_string()),
        own_reads_excluded(),
    )
}

/// The newest event of the last day, egret's own reads left out.
fn last_event_sql(now: DateTime<Utc>) -> String {
    let since = (now - chrono::Duration::days(1)).naive_utc();
    format!(
        "{READ_MARKER} SELECT {READ_HINT} max(timestamp) \
         FROM {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} \
         WHERE timestamp >= {} AND {}",
        sql_datetime(&since),
        own_reads_excluded(),
    )
}

/// The bare user name out of StarRocks' `'bob'@'%'` form; `None` for an empty
/// name or anything not of that form. A `''` inside the name is a quote.
pub(crate) fn authorized_name(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix('\'')?;
    // The host part is the last `'@'…'`; the name is everything before it.
    let at = rest.rfind("'@'")?;
    if !rest[at + 3..].ends_with('\'') || rest.len() < at + 4 {
        return None;
    }
    let name = rest[..at].replace("''", "'");
    (!name.is_empty()).then_some(name)
}

/// What [`redact_statement`] puts in place of a secret.
const REDACTED: &str = "***";

/// Redact a statement or an FE error before it leaves the server (design
/// section 5): every `exact` value (the engine's root and loader passwords)
/// becomes `***`, then the value of `IDENTIFIED BY '…'`, of `SET PASSWORD … =
/// '…'` and `PASSWORD('…')`, and of every quoted
/// property whose key names a password, secret or access key becomes `'***'`.
/// The values are read as SQL literals, so one with spaces or escaped quotes
/// is replaced whole; an unterminated one takes the rest of the text with it.
pub(crate) fn redact_statement(text: &str, exact: &[&str]) -> String {
    let mut out = text.to_string();
    for value in exact.iter().filter(|v| !v.is_empty()) {
        out = out.replace(value, REDACTED);
    }
    static SHAPES: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let shapes = SHAPES.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)\bIDENTIFIED\s+(?:WITH\s+\S+\s+)?(?:BY|AS)\s+(?:PASSWORD\s+)?|\bSET\s+PASSWORD\b[^=]*=\s*|\bPASSWORD\s*\(\s*|(['"`])[^'"`]*(?:password|passwd|secret|access_key)[^'"`]*['"`]\s*=\s*"#,
        )
        .expect("static redaction pattern compiles")
    });
    let mut redacted = String::with_capacity(out.len());
    let mut pos = 0;
    while let Some(m) = shapes.find_at(&out, pos) {
        let value_at = m.end();
        redacted.push_str(&out[pos..value_at]);
        match sql_literal_end(&out[value_at..]) {
            // Not a quoted value: nothing to replace, go on after the key.
            None => pos = value_at,
            Some(len) => {
                redacted.push_str(&format!("'{REDACTED}'"));
                pos = value_at + len;
            }
        }
    }
    redacted.push_str(&out[pos..]);
    redacted
}

/// [`redact_statement`] for a statement the FE cut at
/// [`STATEMENT_PREVIEW_CHARS`]: when `cut`, a secret the cut ran through left
/// only a prefix of itself at the very end, which no exact match can see, so
/// the longest end of the text that is a proper prefix of an `exact` value
/// becomes `***` too. Any length counts, down to one character: the cost is a
/// spurious `***` at the end of a cut preview, the alternative is part of a
/// password.
fn redact_preview(text: &str, exact: &[&str], cut: bool) -> String {
    let out = redact_statement(text, exact);
    if !cut {
        return out;
    }
    let longest_tail = exact
        .iter()
        .filter_map(|secret| {
            // Proper prefixes only: a whole secret was replaced above.
            secret
                .char_indices()
                .skip(1)
                .map(|(i, _)| &secret[..i])
                .filter(|prefix| out.ends_with(prefix))
                .map(str::len)
                .max()
        })
        .max();
    match longest_tail {
        Some(len) => format!("{}{REDACTED}", &out[..out.len() - len]),
        None => out,
    }
}

/// The length of the quoted SQL literal `s` starts with (`'…'` or `"…"`, a
/// doubled quote or a backslash escaping the next character); all of `s` when
/// the literal is never closed. `None` when `s` does not start with a quote.
fn sql_literal_end(s: &str) -> Option<usize> {
    let quote = s.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == quote {
            if chars.peek().map(|(_, n)| *n) == Some(quote) {
                chars.next();
            } else {
                return Some(i + c.len_utf8());
            }
        }
    }
    Some(s.len())
}

/// The cursor for the row `(ts, id)`: opaque to the caller.
pub(crate) fn encode_cursor(ts: &NaiveDateTime, id: &Uuid) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("{}|{id}", ts.format(SQL_DATETIME)))
}

/// The row a cursor names; a 400 naming `cursor` for anything else.
pub(crate) fn decode_cursor(c: &str) -> Result<(NaiveDateTime, Uuid), CpError> {
    use base64::Engine as _;
    let bad_cursor = || bad("cursor", "is not a cursor this server issued");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(c)
        .map_err(|_| bad_cursor())?;
    let text = String::from_utf8(bytes).map_err(|_| bad_cursor())?;
    let (ts, id) = text.split_once('|').ok_or_else(bad_cursor)?;
    let ts = NaiveDateTime::parse_from_str(ts, SQL_DATETIME_IN).map_err(|_| bad_cursor())?;
    let id = Uuid::parse_str(id).map_err(|_| bad_cursor())?;
    Ok((ts, id))
}

/// Whether `p` may read statement text (open question 1, decided (b)).
pub(crate) async fn statement_visible(st: &CpState, p: &Principal) -> bool {
    egret_core::authz::require(&*st.pdp, p, "engine:audit")
        .await
        .is_ok()
}

/// A `DATETIME` the FE returned, as RFC 3339.
fn rfc3339(raw: &str) -> Option<(NaiveDateTime, String)> {
    let t = NaiveDateTime::parse_from_str(raw, SQL_DATETIME_IN).ok()?;
    // The FE's wall clock is UTC (measured in Task 5; see `sql_datetime`).
    let s = Utc
        .from_utc_datetime(&t)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true);
    Some((t, s))
}

/// One row of [`list_sql`] or [`detail_sql`] as an entry, with the statement
/// and the error redacted, and both dropped unless `visible` (the user's
/// ruling of 2026-11-11: an FE error quotes the statement's text).
fn entry_from_row(row: &[Option<String>], visible: bool, exact: &[&str]) -> QueryLogEntryDto {
    let text = |i: usize| row.get(i).cloned().flatten();
    let non_empty = |i: usize| text(i).filter(|s| !s.is_empty());
    let int = |i: usize| text(i).and_then(|s| s.parse::<i64>().ok());
    let raw_ts = text(1).unwrap_or_default();
    let cut = matches!(text(20).as_deref(), Some("1" | "true"));
    let statement = visible
        .then(|| text(19))
        .flatten()
        .map(|s| redact_preview(&s, exact, cut));
    let truncated = statement.is_some() && cut;
    QueryLogEntryDto {
        query_id: text(0).unwrap_or_default(),
        timestamp: rfc3339(&raw_ts).map(|(_, s)| s).unwrap_or(raw_ts),
        user: text(2),
        authorized_user: text(3).as_deref().and_then(authorized_name),
        client_ip: non_empty(4),
        catalog: non_empty(5),
        db: non_empty(6),
        state: non_empty(7),
        error_message: visible
            .then(|| non_empty(8))
            .flatten()
            .map(|s| redact_statement(&s, exact)),
        query_time_ms: int(9),
        scan_rows: int(10),
        scan_bytes: int(11),
        return_rows: int(12),
        cpu_cost_ns: int(13),
        mem_cost_bytes: int(14),
        command: non_empty(15),
        query_source: non_empty(16),
        is_query: text(17).map(|s| s == "1" || s.eq_ignore_ascii_case("true")),
        digest: non_empty(18),
        statement,
        statement_truncated: truncated,
    }
}

/// Load the engine a query-log route names, refusing in the design's order:
/// 404, an external engine (409 `engine_is_external`), a non-StarRocks engine
/// (409), and, for the setup only, one being deleted (409).
async fn load_engine(st: &CpState, id: Uuid, for_setup: bool) -> Result<Engine, CpError> {
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(&engine, "read or set up its query log")?;
    if engine.r#type != "starrocks" {
        return Err(CpError::Conflict(format!(
            "engine {} is not a StarRocks engine; only StarRocks engines have a query log",
            engine.name
        )));
    }
    if for_setup && engine.status == engine_status::DELETING {
        return Err(CpError::Conflict(format!(
            "engine {} is being deleted",
            engine.name
        )));
    }
    Ok(engine)
}

/// What one read request holds: the FE, the secrets its errors are scrubbed
/// of, and the values its texts are redacted of. Resolved once per request.
struct ReadContext {
    fe: FeTarget,
    secrets: BTreeMap<Uuid, String>,
    exact: Vec<String>,
}

impl ReadContext {
    async fn new(st: &CpState, engine: &Engine) -> Result<Self, CpError> {
        let fe = fe_target(st, engine).await?;
        let mut secrets = BTreeMap::new();
        // The loader password, when one was ever stored. A read never mints
        // one: with none stored, no FE has it.
        if let Some(sid) = engine.query_log_secret_id {
            match st.secrets.resolve_plaintext(None, sid).await {
                Ok(pw) => {
                    secrets.insert(sid, pw);
                }
                Err(egret_secrets::SecretsError::NotFound) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let exact = fe
            .root_password
            .iter()
            .chain(secrets.values())
            .filter(|s| !s.is_empty())
            .cloned()
            .collect();
        Ok(Self { fe, secrets, exact })
    }

    fn exact(&self) -> Vec<&str> {
        self.exact.iter().map(String::as_str).collect()
    }

    fn scrub(&self, e: &FeError) -> String {
        redact_statement(&e.scrubbed(&self.fe, &self.secrets), &self.exact())
    }
}

/// Read the query log's state from the FE (design section 5). A failure to
/// reach or read the FE is `Unreachable` with the scrubbed message.
async fn read_status(st: &CpState, engine: &Engine, cx: &ReadContext) -> QueryLogStatusDto {
    let status = |state, message, last_event_at| QueryLogStatusDto {
        state,
        message,
        last_event_at,
        retention_days: RETENTION_DAYS,
    };
    let unreachable = |e: FeError| status(QueryLogState::Unreachable, Some(cx.scrub(&e)), None);
    let ddl = &st.starrocks_ddl;
    let plugins = match ddl
        .query_with_header(&cx.fe, &format!("{READ_MARKER} SHOW PLUGINS"))
        .await
    {
        Ok(rows) => rows,
        Err(e) => return unreachable(e),
    };
    let tables_sql = format!("{READ_MARKER} SHOW TABLES FROM {QUERY_LOG_DATABASE}");
    let has_table = match ddl.query(&cx.fe, &tables_sql).await {
        Ok(rows) => rows
            .iter()
            .any(|r| r.first().and_then(|v| v.as_deref()) == Some(QUERY_LOG_TABLE)),
        Err(e) if e.is_unknown_relation() => false,
        Err(e) => return unreachable(e),
    };
    let image = egret_perm::stored_engine_image("starrocks", &engine.spec);
    let state = match plugin_step(&plugins, image.as_deref()) {
        PluginStep::AlreadyInstalled if has_table => QueryLogState::Ready,
        PluginStep::AlreadyInstalled | PluginStep::Install | PluginStep::Reinstall => {
            QueryLogState::NotSetUp
        }
        PluginStep::NotShipped => QueryLogState::PluginNotShipped,
    };
    let last_event_at = if has_table {
        match ddl.query(&cx.fe, &last_event_sql(Utc::now())).await {
            Ok(rows) => rows
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_deref())
                .and_then(rfc3339)
                .map(|(_, s)| s),
            Err(e) => return unreachable(e),
        }
    } else {
        None
    };
    status(state, None, last_event_at)
}

/// A failed read of the table: "not set up" (409 with the state) when the FE
/// says the table or its database is missing, a 502 otherwise.
async fn read_failure(st: &CpState, engine: &Engine, cx: &ReadContext, e: FeError) -> CpError {
    if e.is_unknown_relation() {
        let status = read_status(st, engine, cx).await;
        // The FE stopped answering between the two reads: that is an FE
        // failure (502), not a state of the query log.
        if status.state == QueryLogState::Unreachable {
            return CpError::FeFailed(status.message.unwrap_or_else(|| cx.scrub(&e)));
        }
        let state = status.state;
        // The table was missing a moment ago; a status that now reads `ready`
        // raced a setup, and the caller's retry will find it.
        let state = if state == QueryLogState::Ready {
            QueryLogState::NotSetUp
        } else {
            state
        };
        return CpError::QueryLogNotReady(state.as_str());
    }
    CpError::FeFailed(cx.scrub(&e))
}

/// The query-log routes, merged onto the engines router.
pub fn routes() -> Router<CpState> {
    Router::new()
        .route("/engines/{id}/query-log", get(list_query_log))
        .route("/engines/{id}/query-log/status", get(query_log_status))
        .route("/engines/{id}/query-log/setup", post(setup_query_log))
        .route(
            "/engines/{id}/query-log/{query_id}",
            get(get_query_log_entry),
        )
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/query-log/status",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "The query log's state, read from the FE now. An FE that does not answer is state `unreachable` with its scrubbed message, still a 200", body = QueryLogStatusDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`), or not a StarRocks engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn query_log_status(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryLogStatusDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id, false).await?;
    let cx = ReadContext::new(&st, &engine).await?;
    Ok(Json(read_status(&st, &engine, &cx).await))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/query-log",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id"), QueryLogParams),
    responses(
        (status = 200, description = "A page of the engine's statements, newest first. `statement` and `errorMessage` are absent without engine:audit", body = QueryLogPageDto),
        (status = 400, description = "A malformed parameter; the message names it", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`), not a StarRocks engine, or its query log table does not exist (`code` = `query_log_not_ready`, with `state`)", body = ErrorBody),
        (status = 502, description = "The FE did not answer or refused the read (`code` = `engine_fe_error`, scrubbed message)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_query_log(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Query(params): Query<QueryLogParams>,
) -> Result<Json<QueryLogPageDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id, false).await?;
    let filter = parse_filter(&params, Utc::now())?;
    let cx = ReadContext::new(&st, &engine).await?;
    let visible = statement_visible(&st, &p).await;
    match read_page(&*st.starrocks_ddl, &cx.fe, &filter, visible, &cx.exact()).await {
        Ok(page) => Ok(Json(page)),
        Err(e) => Err(read_failure(&st, &engine, &cx, e).await),
    }
}

/// One page of the log: [`list_sql`] on the FE, the rows as entries, and the
/// cursor of the last row shown when there are more.
async fn read_page(
    ddl: &dyn crate::starrocks_ddl::StarRocksDdl,
    fe: &FeTarget,
    filter: &QueryLogFilter,
    visible: bool,
    exact: &[&str],
) -> Result<QueryLogPageDto, FeError> {
    let rows = ddl.query(fe, &list_sql(filter)).await?;
    let limit = filter.limit as usize;
    let next_cursor = if rows.len() > limit {
        // The last row shown; a row whose key does not parse ends the paging
        // (StarRocks writes every `queryId` as a UUID).
        let last = &rows[limit - 1];
        let ts = last
            .get(1)
            .cloned()
            .flatten()
            .and_then(|s| rfc3339(&s))
            .map(|(t, _)| t);
        let id = last
            .first()
            .cloned()
            .flatten()
            .and_then(|s| Uuid::parse_str(&s).ok());
        ts.zip(id).map(|(t, id)| encode_cursor(&t, &id))
    } else {
        None
    };
    let entries = rows
        .iter()
        .take(limit)
        .map(|r| entry_from_row(r, visible, exact))
        .collect();
    Ok(QueryLogPageDto {
        entries,
        next_cursor,
    })
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/query-log/{queryId}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("queryId" = String, Path, description = "The statement's query id (a UUID)"),
    ),
    responses(
        (status = 200, description = "One statement, whole. `statement` and `errorMessage` are absent without engine:audit", body = QueryLogEntryDto),
        (status = 400, description = "`queryId` is not a UUID", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine, or no such statement in the log", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`), not a StarRocks engine, or its query log table does not exist (`code` = `query_log_not_ready`, with `state`)", body = ErrorBody),
        (status = 502, description = "The FE did not answer or refused the read (`code` = `engine_fe_error`, scrubbed message)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_query_log_entry(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, query_id)): Path<(Uuid, String)>,
) -> Result<Json<QueryLogEntryDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = load_engine(&st, id, false).await?;
    let query_id = Uuid::parse_str(&query_id).map_err(|_| bad("queryId", "must be a UUID"))?;
    let cx = ReadContext::new(&st, &engine).await?;
    let rows = match st.starrocks_ddl.query(&cx.fe, &detail_sql(&query_id)).await {
        Ok(rows) => rows,
        Err(e) => return Err(read_failure(&st, &engine, &cx, e).await),
    };
    let row = rows
        .first()
        .ok_or_else(|| CpError::NotFoundNamed(format!("query {query_id}")))?;
    let visible = statement_visible(&st, &p).await;
    Ok(Json(entry_from_row(row, visible, &cx.exact())))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/query-log/setup",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Set up: state `ready`, or `pluginNotShipped` when the engine's image has no plugin (the table and loader user were still created)", body = QueryLogStatusDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`), not a StarRocks engine, or being deleted", body = ErrorBody),
        (status = 502, description = "A setup step failed (`code` = `engine_fe_error`, scrubbed message)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn setup_query_log(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryLogStatusDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = load_engine(&st, id, true).await?;
    let fe = fe_target(&st, &engine).await?;
    let state = match ensure_query_log_within(&st, &engine, &fe, false, 1).await {
        QueryLogSetup::Ready => QueryLogState::Ready,
        QueryLogSetup::PluginNotShipped => QueryLogState::PluginNotShipped,
        QueryLogSetup::Failed(why) => return Err(CpError::FeFailed(why)),
    };
    Ok(Json(QueryLogStatusDto {
        state,
        message: None,
        last_event_at: None,
        retention_days: RETENTION_DAYS,
    }))
}

/// OpenAPI fragment for the query-log routes, unioned into
/// `egret_engine_mgmt::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(query_log_status, list_query_log, get_query_log_entry, setup_query_log),
    components(schemas(
        QueryLogStatusDto,
        QueryLogState,
        QueryLogEntryDto,
        QueryLogPageDto,
        ErrorBody
    ))
)]
pub struct QueryLogApi;

/// Test-only (feature `test-util`): the setup and the list read, for the
/// engine-gated live suite `tests/query_log_live.rs`, which runs them against
/// a real FE through a state it builds itself. Every function here is the
/// crate's own, unchanged: the setup is [`ensure_query_log`], and a page is
/// [`parse_filter`] then the list route's own [`read_page`]. Nothing in
/// production reaches this module.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub mod live_testing {
    use super::*;

    pub use super::{QueryLogEntryDto, QueryLogParams, QueryLogSetup};

    /// [`super::READ_MARKER`].
    pub const READ_MARKER: &str = super::READ_MARKER;

    /// [`super::ensure_query_log`].
    pub async fn ensure_query_log(
        st: &CpState,
        engine: &Engine,
        fe: &FeTarget,
        wait_for_events: bool,
    ) -> QueryLogSetup {
        super::ensure_query_log(st, engine, fe, wait_for_events).await
    }

    /// One page as `GET …/query-log` reads it, statements visible. A bad
    /// parameter or an FE error (scrubbed) is the `Err`.
    pub async fn list_page(
        st: &CpState,
        fe: &FeTarget,
        params: &QueryLogParams,
    ) -> Result<QueryLogPageDto, String> {
        let filter = parse_filter(params, Utc::now()).map_err(|e| format!("{e:?}"))?;
        read_page(&*st.starrocks_ddl, fe, &filter, true, &[])
            .await
            .map_err(|e| e.scrubbed(fe, &BTreeMap::new()))
    }

    /// The detail route's SQL.
    pub fn detail_sql(query_id: &Uuid) -> String {
        super::detail_sql(query_id)
    }

    /// The status route's newest-event SQL.
    pub fn last_event_sql(now: DateTime<Utc>) -> String {
        super::last_event_sql(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EGRET_4: &str = "ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4";
    const OFFICIAL_416: &str = "starrocks/fe-ubuntu:4.1.6";

    fn show_plugins(header: &[&str], rows: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
        let mut out = vec![header.iter().map(|c| Some(c.to_string())).collect()];
        out.extend(
            rows.iter()
                .map(|r| r.iter().map(|v| v.map(str::to_string)).collect()),
        );
        out
    }

    /// The header measured on a real FE in Task 5 (`4.1.7-egret.4`).
    fn real_header() -> Vec<&'static str> {
        crate::starrocks_ddl::SHOW_PLUGINS_HEADER.to_vec()
    }

    #[test]
    fn unbound_setup_is_five_statements_ending_with_the_native_grant() {
        let sql = query_log_setup_sql("pw", false);
        assert_eq!(sql.len(), 5, "{sql:?}");
        assert_eq!(sql[0], "CREATE DATABASE IF NOT EXISTS starrocks_audit_db__");
        assert_eq!(sql[1], QUERY_LOG_TABLE_DDL);
        assert_eq!(
            sql[2],
            "CREATE USER IF NOT EXISTS 'egret_query_log'@'%' IDENTIFIED BY 'pw'"
        );
        assert_eq!(
            sql[3],
            "ALTER USER 'egret_query_log'@'%' IDENTIFIED BY 'pw'"
        );
        assert_eq!(
            sql[4],
            "GRANT INSERT ON TABLE starrocks_audit_db__.starrocks_audit_tbl__ \
             TO USER 'egret_query_log'@'%'"
        );
    }

    #[test]
    fn bound_setup_has_no_native_grant() {
        let sql = query_log_setup_sql("pw", true);
        assert_eq!(sql, query_log_setup_sql("pw", false)[..4].to_vec());
        assert!(!sql.iter().any(|s| s.contains("GRANT")), "{sql:?}");
    }

    #[test]
    fn the_database_statement_names_the_shared_database_constant() {
        assert_eq!(
            QUERY_LOG_DATABASE_DDL,
            format!("CREATE DATABASE IF NOT EXISTS {QUERY_LOG_DATABASE}")
        );
        assert!(QUERY_LOG_TABLE_DDL.starts_with(&format!(
            "CREATE TABLE IF NOT EXISTS {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} ("
        )));
    }

    /// The password reaches SQL only as `quote_string`'s output: its quote and
    /// backslash are escaped wherever it appears.
    #[test]
    fn the_password_appears_only_quoted() {
        let pw = r"p'\x";
        let quoted = quote_string(QueryEngine::StarRocks, pw);
        assert_eq!(quoted, r"'p''\\x'");
        let mut all = query_log_setup_sql(pw, false);
        all.push(install_plugin_sql(pw));
        let mut carried = 0;
        for s in &all {
            let with_quoted_removed = s.replace(&quoted, "");
            assert!(
                !with_quoted_removed.contains("p'") && !with_quoted_removed.contains(r"\x"),
                "the password appears outside quote_string's output: {s}"
            );
            carried += s.matches(&quoted).count();
        }
        assert_eq!(carried, 3, "CREATE USER, ALTER USER and INSTALL carry it");
    }

    #[test]
    fn the_install_statement_carries_the_six_properties() {
        assert_eq!(
            install_plugin_sql("pw"),
            "INSTALL PLUGIN FROM \"/opt/egret/auditloader.zip\" PROPERTIES (\
             \"user\" = 'egret_query_log', \"password\" = 'pw', \
             \"database\" = 'starrocks_audit_db__', \"table\" = 'starrocks_audit_tbl__', \
             \"frontend_host_port\" = '127.0.0.1:8030', \"max_batch_interval_sec\" = '60')"
        );
    }

    /// A row of the measured header: `Name` and `Status` set, the rest NULL.
    fn plugin_row(name: &'static str, status: Option<&'static str>) -> Vec<Option<&'static str>> {
        let mut row = vec![None; real_header().len()];
        let at = |c: &str| real_header().iter().position(|h| *h == c).unwrap();
        row[at("Name")] = Some(name);
        row[at("Status")] = status;
        row
    }

    #[test]
    fn a_listed_audit_loader_is_already_installed_even_on_an_unpatched_image() {
        let row = plugin_row("AuditLoader", Some("INSTALLED"));
        let rows = show_plugins(&real_header(), &[&row]);
        for image in [Some(OFFICIAL_416), Some(EGRET_4), None] {
            assert_eq!(plugin_step(&rows, image), PluginStep::AlreadyInstalled);
        }
    }

    #[test]
    fn an_unlisted_plugin_is_installed_on_egret_4_only() {
        let rows = show_plugins(&real_header(), &[]);
        assert_eq!(plugin_step(&rows, Some(EGRET_4)), PluginStep::Install);
        assert_eq!(
            plugin_step(&rows, Some(OFFICIAL_416)),
            PluginStep::NotShipped
        );
        assert_eq!(plugin_step(&rows, None), PluginStep::NotShipped);
    }

    /// Final review, I1: a listed row the FE could not load (`ERROR`, or any
    /// status but `INSTALLED`, or none) is re-installed on an image that ships
    /// the plugin, and is `NotShipped` on one that does not.
    #[test]
    fn a_listed_audit_loader_that_is_not_loaded_is_reinstalled() {
        for status in [Some("ERROR"), Some("UNINSTALLED"), Some(""), None] {
            let row = plugin_row("AuditLoader", status);
            let rows = show_plugins(&real_header(), &[&row]);
            assert_eq!(
                plugin_step(&rows, Some(EGRET_4)),
                PluginStep::Reinstall,
                "{status:?}"
            );
            assert_eq!(
                plugin_step(&rows, Some(OFFICIAL_416)),
                PluginStep::NotShipped,
                "{status:?}"
            );
            assert_eq!(plugin_step(&rows, None), PluginStep::NotShipped);
        }
        // The status is read by header too, and from the AuditLoader row only.
        let reordered = show_plugins(
            &["Status", "Name"],
            &[
                &[Some("INSTALLED"), Some("other")],
                &[Some("ERROR"), Some("AuditLoader")],
            ],
        );
        assert_eq!(
            plugin_step(&reordered, Some(EGRET_4)),
            PluginStep::Reinstall
        );
    }

    /// No `Status` column (no measured FE answers so): a listed name is
    /// installed, as before the column was read.
    #[test]
    fn without_a_status_column_a_listed_name_is_installed() {
        let rows = show_plugins(&["Name", "Type"], &[&[Some("AuditLoader"), None]]);
        assert_eq!(
            plugin_step(&rows, Some(EGRET_4)),
            PluginStep::AlreadyInstalled
        );
    }

    /// Final review, M1: the probe and its poll are marked as egret's own
    /// reads, so the list and the newest event leave them out; the poll
    /// matches the probe's UUID without that exclusion, and not itself.
    #[test]
    fn the_probe_and_its_poll_carry_the_read_marker() {
        let id = Uuid::new_v4();
        let probe = probe_sql(&id);
        let check = probe_check_sql(&id);
        assert_eq!(
            probe,
            format!("/* egret-query-log-read */ SELECT 'egret-query-log-probe-{id}'")
        );
        assert!(check.starts_with(READ_MARKER), "{check}");
        assert!(check.contains(&format!("'{id}'")), "{check}");
        assert!(!check.contains(&own_reads_excluded()), "{check}");
        assert!(!check.contains(&format!("{PROBE_MARKER}{id}")), "{check}");
    }

    #[test]
    fn the_probe_is_sent_on_the_first_poll_then_once_a_minute() {
        let sent: Vec<u32> = (0..PROBE_MAX_POLLS.max(36))
            .filter(|p| sends_probe(*p))
            .collect();
        assert_eq!(sent, vec![0, 12, 24]);
    }

    /// The name is read by header, not by position: the same row with the
    /// columns reordered is still found, and `AuditLoader` in another column
    /// (a description, say) is not a listing.
    /// D1: the node wait reads `Alive` by its header, wherever it is.
    #[test]
    fn an_alive_node_is_read_by_the_alive_header() {
        let listing = |header: &[&str], rows: &[&[&str]]| -> Vec<Vec<Option<String>>> {
            std::iter::once(header.iter().map(|c| Some(c.to_string())).collect())
                .chain(
                    rows.iter()
                        .map(|r| r.iter().map(|c| Some(c.to_string())).collect()),
                )
                .collect()
        };
        let header = ["ComputeNodeId", "IP", "LastHeartbeat", "Alive", "Status"];
        assert!(lists_an_alive_node(&listing(
            &header,
            &[
                &["1", "a", "x", "false", "OK"],
                &["2", "b", "x", "true", "OK"]
            ]
        )));
        assert!(!lists_an_alive_node(&listing(
            &header,
            &[&["1", "a", "x", "false", "OK"]]
        )));
        // No rows (a shared-data FE's SHOW BACKENDS), no answer, no column.
        assert!(!lists_an_alive_node(&listing(&header, &[])));
        assert!(!lists_an_alive_node(&[]));
        assert!(!lists_an_alive_node(&listing(
            &["Id", "Status"],
            &[&["1", "true"]]
        )));
        // A `true` in another column is not an alive node.
        assert!(!lists_an_alive_node(&listing(
            &["Alive", "Other"],
            &[&["false", "true"]]
        )));
        assert!(lists_an_alive_node(&listing(&["Alive"], &[&["TRUE"]])));
    }

    #[test]
    fn the_name_column_is_found_by_its_header() {
        let reordered = show_plugins(
            &["Type", "Description", "Name"],
            &[&[Some("AUDIT"), None, Some("AuditLoader")]],
        );
        assert_eq!(
            plugin_step(&reordered, Some(OFFICIAL_416)),
            PluginStep::AlreadyInstalled
        );
        let elsewhere = show_plugins(
            &["Name", "Description"],
            &[&[Some("other"), Some("AuditLoader")]],
        );
        assert_eq!(plugin_step(&elsewhere, Some(EGRET_4)), PluginStep::Install);
        assert_eq!(plugin_step(&[], Some(EGRET_4)), PluginStep::Install);
    }

    // ── reading the log ──────────────────────────────────────────────────

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap()
    }

    fn params(pairs: &[(&str, &str)]) -> QueryLogParams {
        let mut q = QueryLogParams::default();
        for (k, v) in pairs {
            let v = Some(v.to_string());
            match *k {
                "from" => q.from = v,
                "to" => q.to = v,
                "user" => q.user = v,
                "state" => q.state = v,
                "limit" => q.limit = v,
                "cursor" => q.cursor = v,
                other => panic!("no parameter {other}"),
            }
        }
        q
    }

    fn bad_param(q: &QueryLogParams) -> String {
        match parse_filter(q, now()) {
            Err(CpError::BadRequest(m)) => m,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    fn filter() -> QueryLogFilter {
        parse_filter(&QueryLogParams::default(), now()).unwrap()
    }

    #[test]
    fn the_filter_defaults_to_the_last_day_all_states_fifty_rows() {
        let f = filter();
        assert_eq!(f.to, now());
        assert_eq!(f.from, now() - chrono::Duration::hours(24));
        assert_eq!(f.user, None);
        assert_eq!(f.state, StateFilter::All);
        assert_eq!(f.limit, 50);
        assert_eq!(f.before, None);
    }

    #[test]
    fn the_filter_reads_every_parameter() {
        let id = Uuid::parse_str("0193a2b4-0000-4000-8000-000000000001").unwrap();
        let ts = NaiveDateTime::parse_from_str("2026-11-10 11:00:00.5", SQL_DATETIME_IN).unwrap();
        let f = parse_filter(
            &params(&[
                ("from", "2026-11-01T00:00:00Z"),
                ("to", "2026-11-02T00:00:00+07:00"),
                ("user", "Bob"),
                ("state", "error"),
                ("limit", "200"),
                ("cursor", &encode_cursor(&ts, &id)),
            ]),
            now(),
        )
        .unwrap();
        assert_eq!(f.from, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        assert_eq!(f.to, Utc.with_ymd_and_hms(2026, 11, 1, 17, 0, 0).unwrap());
        assert_eq!(f.user.as_deref(), Some("bob"), "lowercased");
        assert_eq!(f.state, StateFilter::Error);
        assert_eq!(f.limit, 200);
        assert_eq!(f.before, Some((ts, id)));
        let only_to = parse_filter(&params(&[("to", "2026-11-05T00:00:00Z")]), now()).unwrap();
        assert_eq!(only_to.from, only_to.to - chrono::Duration::hours(24));
    }

    #[test]
    fn each_malformed_parameter_is_a_400_naming_it() {
        let cases: &[(&[(&str, &str)], &str)] = &[
            (&[("from", "yesterday")], "from:"),
            (&[("to", "2026-11-10")], "to:"),
            (
                &[
                    ("from", "2026-11-10T00:00:00Z"),
                    ("to", "2026-11-10T00:00:00Z"),
                ],
                "from:",
            ),
            (
                &[
                    ("from", "2026-10-01T00:00:00Z"),
                    ("to", "2026-11-01T00:00:01Z"),
                ],
                "to:",
            ),
            (&[("limit", "0")], "limit:"),
            (&[("limit", "201")], "limit:"),
            (&[("limit", "ten")], "limit:"),
            (&[("state", "failed")], "state:"),
            (&[("user", &"u".repeat(129))], "user:"),
            (&[("cursor", "not-a-cursor")], "cursor:"),
        ];
        for (pairs, prefix) in cases {
            let m = bad_param(&params(pairs));
            assert!(m.starts_with(prefix), "{pairs:?}: {m}");
        }
        // The edges are allowed.
        let ok = params(&[
            ("from", "2026-10-01T00:00:00Z"),
            ("to", "2026-11-01T00:00:00Z"),
            ("limit", "1"),
            ("user", &"u".repeat(128)),
            ("state", "ok"),
        ]);
        assert!(parse_filter(&ok, now()).is_ok());
    }

    #[test]
    fn the_list_sql_is_marked_hinted_bounded_and_leaves_egrets_reads_out() {
        let sql = list_sql(&filter());
        assert_eq!(
            sql,
            "/* egret-query-log-read */ SELECT /*+ SET_VAR(query_timeout = 10) */ \
             queryId, timestamp, `user`, authorizedUser, clientIp, catalog, db, state, errorCode, \
             queryTime, scanRows, scanBytes, returnRows, cpuCostNs, memCostBytes, command, \
             querySource, isQuery, digest, left(stmt, 2000), char_length(stmt) > 2000 \
             FROM starrocks_audit_db__.starrocks_audit_tbl__ \
             WHERE timestamp >= '2026-11-09 12:00:00.000000' \
             AND timestamp < '2026-11-10 12:00:00.000000' \
             AND NOT (`user` = 'root' AND stmt LIKE '/* egret-query-log-read */%') \
             ORDER BY timestamp DESC, queryId DESC LIMIT 51"
        );
    }

    #[test]
    fn the_user_filter_is_quoted_and_lowercased() {
        let mut f = filter();
        f.user = Some(r"o'b\x".to_string());
        let sql = list_sql(&f);
        let q = r"'o''b\\x'";
        assert!(
            sql.contains(&format!(
                "(lower(`user`) = {q} OR lower(replace(regexp_extract(authorizedUser, \
                 '^''(.*)''@''[^'']*''$', 1), '''''', '''')) = {q})"
            )),
            "{sql}"
        );
        assert!(!sql.contains(r"o'b\x"), "the raw value reached SQL: {sql}");
    }

    #[test]
    fn the_state_filter_narrows_by_state() {
        let mut f = filter();
        assert!(!list_sql(&f).contains("state IN"), "all: no clause");
        assert!(!list_sql(&f).contains("state ="), "all: no clause");
        f.state = StateFilter::Ok;
        assert!(list_sql(&f).contains(" AND state IN ('EOF', 'OK') "));
        f.state = StateFilter::Error;
        assert!(list_sql(&f).contains(" AND state = 'ERR' "));
    }

    #[test]
    fn the_cursor_asks_for_rows_strictly_before_it() {
        let mut f = filter();
        assert!(!list_sql(&f).contains("queryId <"), "no cursor: no clause");
        let id = Uuid::parse_str("0193a2b4-0000-4000-8000-000000000001").unwrap();
        let ts = NaiveDateTime::parse_from_str("2026-11-10 11:00:00.25", SQL_DATETIME_IN).unwrap();
        f.before = Some((ts, id));
        assert!(
            list_sql(&f).contains(
                " AND (timestamp < '2026-11-10 11:00:00.250000' \
                 OR (timestamp = '2026-11-10 11:00:00.250000' \
                 AND queryId < '0193a2b4-0000-4000-8000-000000000001')) "
            ),
            "{}",
            list_sql(&f)
        );
    }

    #[test]
    fn the_detail_sql_reads_one_whole_statement() {
        let id = Uuid::parse_str("0193a2b4-0000-4000-8000-000000000001").unwrap();
        let sql = detail_sql(&id);
        assert!(sql.starts_with(
            "/* egret-query-log-read */ SELECT /*+ SET_VAR(query_timeout = 10) */ queryId, "
        ));
        assert!(sql.contains("digest, stmt, false FROM"), "{sql}");
        assert!(
            sql.contains("WHERE queryId = '0193a2b4-0000-4000-8000-000000000001' AND NOT"),
            "{sql}"
        );
    }

    #[test]
    fn authorized_name_reads_the_bare_name() {
        assert_eq!(authorized_name("'bob'@'%'").as_deref(), Some("bob"));
        assert_eq!(
            authorized_name("'o''brien'@'10.%'").as_deref(),
            Some("o'brien")
        );
        assert_eq!(authorized_name(""), None);
        assert_eq!(authorized_name("''@'%'"), None, "an empty name is absent");
        assert_eq!(authorized_name("bob"), None);
        assert_eq!(authorized_name("'bob"), None);
        assert_eq!(authorized_name("'bob'@"), None);
    }

    #[test]
    fn a_cursor_round_trips_and_a_tampered_one_is_a_400() {
        let id = Uuid::new_v4();
        let ts =
            NaiveDateTime::parse_from_str("2026-11-10 11:00:00.123456", SQL_DATETIME_IN).unwrap();
        let c = encode_cursor(&ts, &id);
        assert_eq!(decode_cursor(&c).unwrap(), (ts, id));
        let whole = NaiveDateTime::parse_from_str("2026-11-10 11:00:00", SQL_DATETIME_IN).unwrap();
        assert_eq!(
            decode_cursor(&encode_cursor(&whole, &id)).unwrap(),
            (whole, id)
        );
        let mut tampered = c.clone();
        tampered.replace_range(0..1, if c.starts_with('A') { "B" } else { "A" });
        for bad in [tampered.as_str(), "", "!!!", &c[..c.len() - 4]] {
            match decode_cursor(bad) {
                Err(CpError::BadRequest(m)) => assert!(m.starts_with("cursor:"), "{m}"),
                other => panic!("{bad:?}: expected a 400, got {other:?}"),
            }
        }
    }

    #[test]
    fn identified_by_loses_its_whole_value() {
        assert_eq!(
            redact_statement("CREATE USER 'u' IDENTIFIED BY 'x y'", &[]),
            "CREATE USER 'u' IDENTIFIED BY '***'"
        );
        assert_eq!(
            redact_statement("alter user u identified by 'it''s \\' x' ;", &[]),
            "alter user u identified by '***' ;"
        );
    }

    #[test]
    fn a_secret_property_keeps_its_key_and_loses_its_value() {
        assert_eq!(
            redact_statement(
                r#"CREATE CATALOG c PROPERTIES ("password" = "a b", "user" = "u")"#,
                &[]
            ),
            r#"CREATE CATALOG c PROPERTIES ("password" = '***', "user" = "u")"#
        );
        assert_eq!(
            redact_statement(
                "SELECT * FROM FILES('secret_key'='k', 'aws.s3.access_key'='a')",
                &[]
            ),
            "SELECT * FROM FILES('secret_key'='***', 'aws.s3.access_key'='***')"
        );
        assert_eq!(
            redact_statement("x ('jdbc.Passwd' = 'unterminated", &[]),
            "x ('jdbc.Passwd' = '***'",
            "an unterminated value takes the rest of the text"
        );
    }

    #[test]
    fn an_exact_value_is_redacted_wherever_it_appears() {
        let root = "R00t-pw";
        assert_eq!(
            redact_statement(
                &format!("SELECT 'hello {root} world', note"),
                &[root, "ld-pw"]
            ),
            "SELECT 'hello *** world', note"
        );
        assert_eq!(
            redact_statement("ERROR: ld-pw rejected", &[root, "ld-pw"]),
            "ERROR: *** rejected"
        );
        assert_eq!(
            redact_statement("SELECT 1", &[""]),
            "SELECT 1",
            "an empty value is no value"
        );
    }

    #[test]
    fn a_statement_with_nothing_to_redact_is_unchanged() {
        let s = "SELECT `password`, 'secret' AS k FROM t WHERE note = 'IDENTIFIED' -- by";
        assert_eq!(redact_statement(s, &["absent-pw"]), s);
    }

    #[test]
    fn a_row_becomes_an_entry_and_the_statement_needs_visibility() {
        let row: Vec<Option<String>> = [
            Some("0193a2b4-0000-4000-8000-000000000001"),
            Some("2026-11-10 11:00:00.5"),
            Some("egret_x"),
            Some("'bob'@'%'"),
            Some("10.0.0.1"),
            Some("default_catalog"),
            Some(""),
            Some("ERR"),
            Some("Access denied; pw R00t-pw"),
            Some("12"),
            Some("3"),
            Some("4"),
            Some("5"),
            Some("6"),
            Some("7"),
            Some("MySQL.COM_QUERY"),
            Some("External"),
            Some("1"),
            Some("abc"),
            Some("CREATE USER x IDENTIFIED BY 'p q'"),
            Some("1"),
        ]
        .iter()
        .map(|v| v.map(str::to_string))
        .collect();
        let e = entry_from_row(&row, true, &["R00t-pw"]);
        assert_eq!(e.timestamp, "2026-11-10T11:00:00.500Z");
        assert_eq!(e.user.as_deref(), Some("egret_x"));
        assert_eq!(e.authorized_user.as_deref(), Some("bob"));
        assert_eq!(e.db, None, "an empty column is absent");
        assert_eq!(e.error_message.as_deref(), Some("Access denied; pw ***"));
        assert_eq!(e.query_time_ms, Some(12));
        assert_eq!(e.is_query, Some(true));
        assert_eq!(
            e.statement.as_deref(),
            Some("CREATE USER x IDENTIFIED BY '***'")
        );
        assert!(e.statement_truncated);

        let hidden = entry_from_row(&row, false, &["R00t-pw"]);
        assert_eq!(hidden.statement, None);
        assert!(!hidden.statement_truncated);
        let json = serde_json::to_value(&hidden).unwrap();
        assert!(json.get("statement").is_none(), "absent, not null: {json}");
        assert_eq!(json["statementTruncated"], false);
        assert!(
            json.get("errorMessage").is_none(),
            "the error quotes the statement, so it needs engine:audit too: {json}"
        );
        assert_eq!(hidden.query_time_ms, Some(12), "every other field stays");
        assert_eq!(
            hidden.state.as_deref(),
            Some("ERR"),
            "every other field stays"
        );
    }

    /// The SQL literal that starts `sql`, unescaped (`''` read as `'`).
    fn sql_literal(sql: &str) -> String {
        let len = sql_literal_end(sql).expect("a literal");
        sql[1..len - 1].replace("''", "'")
    }

    /// The SQL side of the user filter reads a name the way
    /// [`authorized_name`] does, so the row shown as `o'brien` is found by
    /// `o'brien`. Checked by running the clause's own pattern and replacement
    /// (read back out of [`AUTHORIZED_NAME_SQL`]) with the `regex` crate.
    #[test]
    fn the_sql_authorized_name_agrees_with_authorized_name() {
        let rest = AUTHORIZED_NAME_SQL
            .split_once("regexp_extract(authorizedUser, ")
            .unwrap()
            .1;
        let pattern = sql_literal(rest);
        assert_eq!(pattern, "^'(.*)'@'[^']*'$");
        let after = &rest[sql_literal_end(rest).unwrap()..];
        let replace_args = after.split_once("1), ").unwrap().1;
        let from = sql_literal(replace_args);
        let to = sql_literal(
            replace_args[sql_literal_end(replace_args).unwrap()..].trim_start_matches(", "),
        );
        assert_eq!((from.as_str(), to.as_str()), ("''", "'"));
        let re = regex::Regex::new(&pattern).unwrap();
        for raw in [
            "'bob'@'%'",
            "'Bob'@'10.%'",
            "'o''brien'@'10.%'",
            "'o'brien'@'%'",
            "'a'@'b'@'%'",
            "''@'%'",
            "",
            "bob",
            "'bob'@",
        ] {
            let sql_side = re
                .captures(raw)
                .map(|c| c[1].to_string())
                .unwrap_or_default()
                .replace(&from, &to)
                .to_lowercase();
            let rust_side = authorized_name(raw).unwrap_or_default().to_lowercase();
            assert_eq!(sql_side, rust_side, "{raw:?}");
        }
    }

    #[test]
    fn a_secret_the_cut_runs_through_leaves_no_prefix() {
        let pw = "S3cret-Passw0rd";
        let preview = format!("SELECT note FROM t WHERE x = {}", &pw[..7]);
        assert_eq!(
            redact_preview(&preview, &[pw, "other"], true),
            "SELECT note FROM t WHERE x = ***"
        );
        assert_eq!(
            redact_preview(&preview, &[pw], false),
            preview,
            "an uncut statement's end is its own"
        );
        assert_eq!(
            redact_preview("SELECT 1", &[pw], true),
            "SELECT 1",
            "nothing at the end is a prefix"
        );
        assert_eq!(
            redact_preview(&format!("x {pw} y S3c"), &[pw], true),
            "x *** y ***",
            "the whole value and the cut one"
        );
    }

    #[test]
    fn set_password_loses_its_value() {
        assert_eq!(
            redact_statement("SET PASSWORD FOR 'bob'@'%' = PASSWORD('x y')", &[]),
            "SET PASSWORD FOR 'bob'@'%' = PASSWORD('***')"
        );
        assert_eq!(
            redact_statement("set password = 'plain'", &[]),
            "set password = '***'"
        );
    }
}
