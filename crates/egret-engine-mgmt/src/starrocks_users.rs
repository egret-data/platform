//! The StarRocks users of the people who may query an impersonate-mode
//! StarRocks engine (package D, D1a; spec
//! `docs/en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md`,
//! section 2).
//!
//! `EXECUTE AS` needs its target user to exist (StarRocks checks it before
//! any authorizer). egret creates `'<users.username>'@'%'` — exactly the name
//! egret-query's connector sends — with a random password kept nowhere, for
//! each active member of a workspace the engine serves, records it in
//! `engine_mgmt.starrocks_users`, and drops only what it recorded. It runs as
//! `root` from engine-mgmt, never from the query path.
//!
//! Woken by (wired in Task 9): the `pg_notify` triggers of migration 0219
//! (membership, groups, user status and delete, engine assignment), a periodic
//! sweep whose first tick is the boot reconcile, every StarRocks apply, and
//! the post-Ready bootstrap. Those triggers are statement-level and also fire
//! on writes that change no membership (an object-scope grant, say), so a
//! reconcile must stay cheap and idempotent: one that finds nothing to change
//! sends `SHOW USERS` and no statement.

use std::collections::{BTreeMap, BTreeSet};

use egret_core::engine_principals::is_starrocks_reserved_user;
use egret_query_api::{quote_string, Engine as QueryEngine};
use uuid::Uuid;

use crate::engines::random_internal_secret;
use crate::starrocks_ddl::FeTarget;
use crate::EngineMgmtState as CpState;
use crate::{engine_status, Engine};

/// The channel migration 0219's triggers notify.
// Task 9 wires the listener, the sweep and the apply hook that call this.
#[allow(dead_code)]
pub const USER_SYNC_CHANNEL: &str = "egret_starrocks_user_sync";

pub(crate) const SHOW_USERS_SQL: &str = "SHOW USERS";

/// Whether StarRocks accepts `name` as a user name: `FeNameFormat.checkUserName`,
/// `^\w{1,64}/?[.\w-]{0,63}$` and at most 64 characters, `\w` being ASCII.
pub(crate) fn is_starrocks_user_name(name: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let head = name.find(|c: char| !word(c)).unwrap_or(name.len());
    if head == 0 {
        return false;
    }
    let rest = &name[head..];
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    rest.chars().all(|c| word(c) || c == '.' || c == '-')
}

/// `'<name>'@'%'`, the name quoted as a StarRocks string literal.
pub(crate) fn user_identity(name: &str) -> String {
    format!("{}@'%'", quote_string(QueryEngine::StarRocks, name))
}

/// No `IF NOT EXISTS`: a user that is already there is someone else's, and
/// the failure is how the sync learns it without adopting it.
pub(crate) fn create_user_sql(name: &str, password: &str) -> String {
    format!(
        "CREATE USER {} IDENTIFIED BY {}",
        user_identity(name),
        quote_string(QueryEngine::StarRocks, password)
    )
}

pub(crate) fn drop_user_sql(name: &str) -> String {
    format!("DROP USER {}", user_identity(name))
}

/// The user names `SHOW USERS` lists: the first column, read as `'<name>'@'<host>'`
/// when it has that form (measured in Task 1, M2), as is otherwise.
pub(crate) fn listed_users(rows: &[Vec<Option<String>>]) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r.first().cloned().flatten())
        .map(|raw| crate::query_log::authorized_name(&raw).unwrap_or(raw))
        .collect()
}

/// What one reconcile does (spec section 2.3's table), every list sorted.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct UserPlan {
    /// Desired, recorded and on the FE: nothing to do (Task 10 grants these).
    pub kept: Vec<String>,
    /// Desired and absent from the FE: record, then `CREATE USER`.
    pub create: Vec<String>,
    /// Recorded, no longer desired, on the FE: `DROP USER`, then forget.
    pub drop: Vec<String>,
    /// Recorded, no longer desired, gone from the FE: forget.
    pub forget: Vec<String>,
    /// Desired, on the FE, not recorded: someone else's user, left alone.
    pub foreign: Vec<String>,
    /// Desired but never created, as `"<name>: <why>"`.
    pub refused: Vec<String>,
}

pub(crate) fn plan_users(
    desired: &BTreeSet<String>,
    recorded: &BTreeSet<String>,
    on_fe: &BTreeSet<String>,
) -> UserPlan {
    let mut plan = UserPlan::default();
    for name in desired {
        if is_starrocks_reserved_user(name) {
            plan.refused
                .push(format!("{name}: reserved for StarRocks or egret"));
            continue;
        }
        if !is_starrocks_user_name(name) {
            plan.refused
                .push(format!("{name}: not a valid StarRocks user name"));
            continue;
        }
        match (recorded.contains(name), on_fe.contains(name)) {
            (true, true) => plan.kept.push(name.clone()),
            (false, true) => plan.foreign.push(name.clone()),
            (_, false) => plan.create.push(name.clone()),
        }
    }
    for name in recorded.difference(desired) {
        if on_fe.contains(name) {
            plan.drop.push(name.clone());
        } else {
            plan.forget.push(name.clone());
        }
    }
    plan
}

/// The outcome of one engine's reconcile.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UserSync {
    /// Not impersonate-mode and nothing recorded: no FE call was made.
    NotNeeded,
    /// Another egret session holds this engine's sync lock.
    Busy,
    Done {
        created: usize,
        dropped: usize,
        /// Native `GRANT IMPERSONATE ON USER` statements sent (Task 10).
        granted: usize,
        /// Whether `ON ALL USERS` was revoked (Task 10).
        revoked_all: bool,
        foreign: Vec<String>,
        refused: Vec<String>,
        /// One scrubbed message per statement or record that failed.
        failed: Vec<String>,
    },
    /// The database or `SHOW USERS` could not be read (scrubbed).
    Failed(String),
}

/// Whether `engine` gets a reconcile at all: a provisioned StarRocks engine,
/// not deleting or failed, whose bootstrap created `egret_delegator`
/// (`impersonate` or `jwt` mode: Task 10 converges its native grants), or
/// that still has users egret created.
pub(crate) async fn wants_user_sync(st: &CpState, engine: &Engine) -> bool {
    if !engine.is_starrocks()
        || crate::external::is_external(engine)
        || engine.status == engine_status::DELETING
        || engine.status == engine_status::FAILED
    {
        return false;
    }
    if matches!(engine.delegation_mode(), "impersonate" | "jwt") {
        return true;
    }
    match st.engine_mgmt.recorded_starrocks_users(engine.id).await {
        Ok(recorded) => !recorded.is_empty(),
        Err(e) => {
            tracing::warn!(engine = %engine.name, error = %e,
                "starrocks users: the records could not be read");
            false
        }
    }
}

/// Reconcile `engine`'s StarRocks users (spec section 2.3) as `root` on `fe`.
// Task 9 wires the listener, the sweep and the apply hook that call this.
#[allow(dead_code)]
pub(crate) async fn sync_starrocks_users(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync {
    if !wants_user_sync(st, engine).await {
        return UserSync::NotNeeded;
    }
    let lock = match st.engine_mgmt.try_lock_user_sync(engine.id).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return UserSync::Busy,
        Err(e) => return UserSync::Failed(format!("the sync lock could not be taken: {e}")),
    };
    let outcome = sync_locked(st, engine, fe).await;
    lock.release().await;
    outcome
}

/// The desired names: empty unless impersonate-mode; the members, and the
/// platform admins when the engine serves at least one workspace (decided on
/// 2026-11-11, 1). A user with no username yet is minted, as the permission
/// compiler does.
async fn desired_users(st: &CpState, engine: &Engine) -> Result<BTreeSet<String>, String> {
    if !engine.is_impersonate_delegation() {
        return Ok(BTreeSet::new());
    }
    let mut candidates = st
        .engine_mgmt
        .starrocks_user_candidates(engine.id)
        .await
        .map_err(|e| e.to_string())?;
    let serves_a_workspace = !st
        .engine_mgmt
        .list_engine_workspace_ids(engine.id)
        .await
        .map_err(|e| e.to_string())?
        .is_empty();
    if serves_a_workspace {
        let mut conn = st
            .engine_mgmt
            .pool()
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        let admins = egret_auth::roles::engine_admin_user_ids_via(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        // `None`: `ensure_username_via` below returns the stored name or mints one.
        candidates.extend(admins.into_iter().map(|u| (u.0, None)));
    }
    let mut out = BTreeSet::new();
    for (id, username) in candidates {
        if let Some(u) = username {
            out.insert(u);
            continue;
        }
        let mut conn = st
            .engine_mgmt
            .pool()
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        match egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(id)).await {
            Ok(u) => {
                out.insert(u);
            }
            Err(e) => tracing::warn!(engine = %engine.name, user = %id, error = %e,
                "starrocks users: a member's username could not be minted; skipped"),
        }
    }
    Ok(out)
}

async fn sync_locked(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync {
    let desired = match desired_users(st, engine).await {
        Ok(d) => d,
        Err(e) => return UserSync::Failed(e),
    };
    let recorded = match st.engine_mgmt.recorded_starrocks_users(engine.id).await {
        Ok(r) => r,
        Err(e) => return UserSync::Failed(e.to_string()),
    };
    let on_fe = match st.starrocks_ddl.query(fe, SHOW_USERS_SQL).await {
        Ok(rows) => listed_users(&rows),
        Err(e) => return UserSync::Failed(e.scrubbed(fe, &BTreeMap::new())),
    };
    let plan = plan_users(&desired, &recorded, &on_fe);
    let (mut created, mut dropped, mut failed) = (0, 0, Vec::new());
    for name in &plan.create {
        let inserted = match st.engine_mgmt.record_starrocks_user(engine.id, name).await {
            Ok(inserted) => inserted,
            Err(e) => {
                failed.push(format!("{name}: record: {e}"));
                continue;
            }
        };
        let password = random_internal_secret();
        let mut secrets = BTreeMap::new();
        secrets.insert(Uuid::from_u128(1), password.clone());
        match st
            .starrocks_ddl
            .execute(fe, &create_user_sql(name, &password))
            .await
        {
            Ok(()) => created += 1,
            Err(e) => {
                // Never adopt a user that appeared between the read and the
                // create: forget the record if this call made it.
                if inserted {
                    if let Err(f) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
                        failed.push(format!("{name}: forget: {f}"));
                    }
                }
                failed.push(format!("{name}: {}", e.scrubbed(fe, &secrets)));
            }
        }
    }
    for name in &plan.drop {
        match st.starrocks_ddl.execute(fe, &drop_user_sql(name)).await {
            Ok(()) => {
                dropped += 1;
                if let Err(e) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
                    failed.push(format!("{name}: forget: {e}"));
                }
            }
            Err(e) => failed.push(format!("{name}: {}", e.scrubbed(fe, &BTreeMap::new()))),
        }
    }
    for name in &plan.forget {
        if let Err(e) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
            failed.push(format!("{name}: forget: {e}"));
        }
    }
    UserSync::Done {
        created,
        dropped,
        granted: 0,
        revoked_all: false,
        foreign: plan.foreign,
        refused: plan.refused,
        failed,
    }
}

// Task 9 wires the listener, the sweep and the apply hook that call this.
#[allow(dead_code)]
pub(crate) fn log_outcome(engine_name: &str, outcome: &UserSync) {
    match outcome {
        UserSync::NotNeeded => {}
        UserSync::Busy => tracing::debug!(engine = %engine_name,
            "starrocks users: another sync of this engine is running"),
        UserSync::Done {
            created,
            dropped,
            granted,
            revoked_all,
            foreign,
            refused,
            failed,
        } => {
            if *created > 0 || *dropped > 0 || *granted > 0 || *revoked_all {
                tracing::info!(engine = %engine_name, created, dropped, granted, revoked_all,
                    "starrocks users synced");
            }
            if !foreign.is_empty() {
                tracing::warn!(engine = %engine_name, users = ?foreign,
                    "starrocks users: a member's name is held by a StarRocks user egret did not \
                     create; it is left as it is");
            }
            if !refused.is_empty() {
                tracing::warn!(engine = %engine_name, users = ?refused,
                    "starrocks users: not created");
            }
            if !failed.is_empty() {
                tracing::warn!(engine = %engine_name, errors = ?failed,
                    "starrocks users: some changes failed; the next sync tries again");
            }
        }
        UserSync::Failed(why) => tracing::warn!(engine = %engine_name, error = %why,
            "starrocks users: the sync failed; the next sync tries again"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn starrocks_user_names_follow_fe_name_format() {
        for ok in ["alice", "member-sso", "bob-2", "a_b.c", "x1", "a/b"] {
            assert!(is_starrocks_user_name(ok), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in [
            "",
            "-lead",
            ".lead",
            "a b",
            "o'brien",
            "é",
            "a/b/c",
            long.as_str(),
        ] {
            assert!(!is_starrocks_user_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_statements_quote_the_name_and_the_password() {
        assert_eq!(
            create_user_sql("bob", "p'w\\x"),
            "CREATE USER 'bob'@'%' IDENTIFIED BY 'p''w\\\\x'"
        );
        assert_eq!(drop_user_sql("o'b"), "DROP USER 'o''b'@'%'");
    }

    #[test]
    fn listed_users_reads_the_identity_form_and_a_bare_name() {
        let rows = vec![
            vec![Some("'root'@'%'".to_string())],
            vec![Some("'o''b'@'10.%'".to_string())],
            vec![Some("plain".to_string())],
            vec![None],
        ];
        assert_eq!(listed_users(&rows), set(&["o'b", "plain", "root"]));
    }

    #[test]
    fn the_plan_follows_the_table() {
        let desired = set(&[
            "keep",
            "new",
            "lost",
            "theirs",
            "root",
            "egret_query_log",
            "bad name",
        ]);
        let recorded = set(&["keep", "lost", "gone", "left"]);
        let on_fe = set(&["root", "keep", "theirs", "gone"]);
        assert_eq!(
            plan_users(&desired, &recorded, &on_fe),
            UserPlan {
                kept: vec!["keep".into()],
                create: vec!["lost".into(), "new".into()],
                drop: vec!["gone".into()],
                forget: vec!["left".into()],
                foreign: vec!["theirs".into()],
                refused: vec![
                    "bad name: not a valid StarRocks user name".into(),
                    "egret_query_log: reserved for StarRocks or egret".into(),
                    "root: reserved for StarRocks or egret".into(),
                ],
            }
        );
    }

    /// Task 7 review: the listener must hear what the triggers send.
    #[test]
    fn the_channel_is_the_one_migration_0219_notifies() {
        let migration = include_str!("../../../migrations/0219_starrocks_users.sql");
        assert!(
            migration.contains(&format!("pg_notify('{USER_SYNC_CHANNEL}'")),
            "migration 0219 notifies another channel"
        );
    }

    /// A reserved name is refused in any case, even when the FE has it and egret
    /// recorded it: never created, kept (granted), dropped or called foreign.
    #[test]
    fn a_reserved_name_is_only_ever_refused() {
        let plan = plan_users(
            &set(&["root", "Root", "EGRET_DELEGATOR"]),
            &set(&[]),
            &set(&["root"]),
        );
        assert_eq!(
            plan,
            UserPlan {
                refused: vec![
                    "EGRET_DELEGATOR: reserved for StarRocks or egret".into(),
                    "Root: reserved for StarRocks or egret".into(),
                    "root: reserved for StarRocks or egret".into(),
                ],
                ..UserPlan::default()
            }
        );
    }

    #[test]
    fn nothing_desired_drops_every_recorded_user_and_touches_no_other() {
        let plan = plan_users(&set(&[]), &set(&["a", "b"]), &set(&["root", "a", "other"]));
        assert_eq!(plan.drop, vec!["a".to_string()]);
        assert_eq!(plan.forget, vec!["b".to_string()]);
        assert!(plan.create.is_empty() && plan.foreign.is_empty());
    }
}
