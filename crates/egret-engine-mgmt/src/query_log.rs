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

use egret_core::engine_principals::QUERY_LOG_LOADER_PRINCIPAL;
use egret_policy_api::infra_names::{QUERY_LOG_DATABASE, QUERY_LOG_TABLE};
use egret_query_api::{quote_string, Engine as QueryEngine};
use uuid::Uuid;

use crate::engines::{engine_still_current, ensure_query_log_secret, wait_for_fe_root_login};
use crate::starrocks_ddl::{FeError, FeTarget};
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

/// What [`ensure_query_log`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueryLogSetup {
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
/// the header) and the engine's stored FE image. The name is read from the
/// column whose header is `Name`, wherever the FE puts it; an answer with no
/// such column lists nothing.
pub(crate) fn plugin_step(
    show_plugins: &[Vec<Option<String>>],
    fe_image: Option<&str>,
) -> PluginStep {
    let listed = show_plugins.split_first().is_some_and(|(header, rows)| {
        header
            .iter()
            .position(|c| c.as_deref() == Some(SHOW_PLUGINS_NAME_COLUMN))
            .is_some_and(|i| {
                rows.iter()
                    .any(|r| r.get(i).and_then(|v| v.as_deref()) == Some(AUDIT_LOADER_PLUGIN))
            })
    });
    if listed {
        PluginStep::AlreadyInstalled
    } else if fe_image.is_none_or(|i| !egret_perm::is_patched_starrocks_fe(i)) {
        PluginStep::NotShipped
    } else {
        PluginStep::Install
    }
}

/// Set up `engine`'s Query log on the FE `fe` (design section 1, steps 1 to
/// 6). Idempotent, so any caller may run it any number of times. It never
/// fails its caller: what went wrong comes back as [`QueryLogSetup::Failed`]
/// with a scrubbed message, and is logged here. `wait_for_events` runs step 6
/// (up to three minutes) and is for background callers only.
pub(crate) async fn ensure_query_log(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    wait_for_events: bool,
) -> QueryLogSetup {
    let setup = setup_steps(st, engine, fe).await;
    if let QueryLogSetup::Failed(why) = &setup {
        tracing::warn!(engine = %engine.name, error = %why, "query log setup failed");
    }
    if wait_for_events && setup == QueryLogSetup::Ready {
        self::wait_for_events(st, fe, &engine.name).await;
    }
    setup
}

/// Steps 1 to 5.
async fn setup_steps(st: &CpState, engine: &Engine, fe: &FeTarget) -> QueryLogSetup {
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
    let (_, password) = match ensure_query_log_secret(st, engine).await {
        Ok(v) => v,
        Err(e) => {
            return QueryLogSetup::Failed(format!("could not resolve the loader password: {e}"))
        }
    };
    let secrets = BTreeMap::from([(Uuid::from_u128(1), password.clone())]);
    let failed = |e: FeError| QueryLogSetup::Failed(e.scrubbed(fe, &secrets));

    // 2-4. The table, the loader user, the native grant when unbound.
    for stmt in query_log_setup_sql(&password, engine.authz_service_id.is_some()) {
        if let Err(e) = st.starrocks_ddl.execute(fe, &stmt).await {
            return failed(e);
        }
    }

    // 5. The plugin.
    let plugins = match st.starrocks_ddl.query_with_header(fe, "SHOW PLUGINS").await {
        Ok(rows) => rows,
        Err(e) => return failed(e),
    };
    let image = egret_perm::stored_engine_image("starrocks", &engine.spec);
    match plugin_step(&plugins, image.as_deref()) {
        PluginStep::AlreadyInstalled => QueryLogSetup::Ready,
        PluginStep::NotShipped => QueryLogSetup::PluginNotShipped,
        PluginStep::Install => match st
            .starrocks_ddl
            .execute(fe, &install_plugin_sql(&password))
            .await
        {
            Ok(()) => QueryLogSetup::Ready,
            Err(e) => failed(e),
        },
    }
}

/// Step 6: send a probe statement carrying a fresh UUID and poll the table for
/// its row, every [`PROBE_INTERVAL`] for up to [`PROBE_MAX_POLLS`] polls. Only
/// logs; whether the row arrived changes nothing. The poll's own text does not
/// contain the marker (it is split across `concat`), so the poll is never
/// mistaken for the probe once it is logged itself.
pub(crate) async fn wait_for_events(st: &CpState, fe: &FeTarget, engine_name: &str) -> bool {
    let id = Uuid::new_v4();
    let probe = format!(
        "SELECT {}",
        quote_string(QueryEngine::StarRocks, &format!("{PROBE_MARKER}{id}"))
    );
    let check = format!(
        "SELECT count(*) FROM {QUERY_LOG_DATABASE}.{QUERY_LOG_TABLE} \
         WHERE stmt LIKE concat('%{PROBE_MARKER}', '{id}', '%')"
    );
    let no_secrets = BTreeMap::new();
    for poll in 0..PROBE_MAX_POLLS {
        if poll % PROBE_RESEND_EVERY == 0 {
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

/// Log what a background setup came to. `Failed` is already logged by
/// [`ensure_query_log`].
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

/// After an apply that may restart the FE: wait for the FE pod and a real
/// `root` login exactly as the bootstrap does
/// ([`wait_for_fe_root_login`]), then run [`ensure_query_log`] with step 6.
/// Spawned, never awaited by a request; the handle is for tests.
pub(crate) fn spawn_query_log_setup(
    st: CpState,
    engine_id: Uuid,
    name: String,
    namespace: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut consecutive_read_errors: u32 = 0;
        let Some((engine, fe)) = wait_for_fe_root_login(
            &st,
            engine_id,
            &name,
            &namespace,
            &mut consecutive_read_errors,
        )
        .await
        else {
            return;
        };
        if !engine_still_current(
            &st.engine_mgmt,
            engine_id,
            &name,
            &mut consecutive_read_errors,
        )
        .await
        {
            return;
        }
        let setup = ensure_query_log(&st, &engine, &fe, true).await;
        log_outcome(&name, &setup);
    })
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

    #[test]
    fn a_listed_audit_loader_is_already_installed_even_on_an_unpatched_image() {
        let mut row = vec![None; 10];
        row[0] = Some("AuditLoader");
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

    /// The name is read by header, not by position: the same row with the
    /// columns reordered is still found, and `AuditLoader` in another column
    /// (a description, say) is not a listing.
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
}
