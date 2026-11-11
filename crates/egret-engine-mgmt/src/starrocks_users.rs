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
//! Woken by: the `pg_notify` triggers of migration 0219
//! (membership, groups, user status and delete, engine assignment), a periodic
//! sweep whose first tick is the boot reconcile, every StarRocks apply, and
//! the post-Ready bootstrap. Those triggers are statement-level and also fire
//! on writes that change no membership (an object-scope grant, say), so a
//! reconcile must stay cheap and idempotent: one that finds nothing to change
//! sends `SHOW USERS` and no statement.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use egret_core::engine_principals::is_starrocks_reserved_user;
use egret_query_api::{quote_string, Engine as QueryEngine};
use uuid::Uuid;

use crate::engines::{engine_still_current, random_internal_secret, wait_for_fe_root_login};
use crate::starrocks_ddl::{fe_target, FeTarget};
use crate::EngineMgmtState as CpState;
use crate::{engine_status, Engine};

/// The channel migration 0219's triggers notify.
pub const USER_SYNC_CHANNEL: &str = "egret_starrocks_user_sync";

/// How often every engine is reconciled whatever was notified: what a
/// listener that was down missed, and an FE that lost its users on a roll.
const SWEEP_INTERVAL: Duration = Duration::from_secs(600);

/// How long the listener waits before reconnecting after an error.
const LISTEN_RETRY: Duration = Duration::from_secs(5);

/// How long the worker waits after a wake-up before its pass, so that a
/// burst of notifications (one per statement of a multi-statement write, or a
/// run of unrelated grant writes) costs one pass.
const SYNC_DEBOUNCE: Duration = Duration::from_secs(1);

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
pub(crate) async fn sync_starrocks_users(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync {
    if !wants_user_sync(st, engine).await {
        return UserSync::NotNeeded;
    }
    let lock = match st.engine_mgmt.try_lock_user_sync(engine.id).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return UserSync::Busy,
        Err(e) => return UserSync::Failed(format!("the sync lock could not be taken: {e}")),
    };
    // Plan from the row as it is under the lock, never from `engine`: a
    // snapshot that no longer reads as impersonate-mode would plan an empty
    // desired set and drop every user egret recorded. A fresh row that left
    // impersonate mode drops them, as spec 2.4 ("Mode changes") decides.
    let outcome = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(fresh)) if wants_user_sync(st, &fresh).await => sync_locked(st, &fresh, fe).await,
        Ok(_) => UserSync::NotNeeded,
        Err(e) => UserSync::Failed(format!("the engine could not be re-read: {e}")),
    };
    lock.release().await;
    outcome
}

/// The desired names: empty unless impersonate-mode; the members, and the
/// platform admins when the engine serves at least one workspace (decided on
/// 2026-11-11, 1). A user with no username yet is minted, as the permission
/// compiler does. Any error but "the user is gone" fails the whole read: a
/// candidate silently left out would have their StarRocks user dropped.
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
        let admins: Vec<Uuid> = egret_auth::roles::engine_admin_user_ids_via(&mut conn)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|u| u.0)
            .collect();
        // Their stored names in one read; only a NULL one is minted below.
        let named: Vec<(Uuid, Option<String>)> =
            sqlx::query_as("SELECT id, username::text FROM users WHERE id = ANY($1)")
                .bind(&admins)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| e.to_string())?;
        candidates.extend(named);
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
            // Deleted since the candidates were read: no longer desired.
            Err(egret_core::Error::NotFound(_)) => {}
            Err(e) => return Err(format!("user {id}: the username could not be minted: {e}")),
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
                // create: forget the record if this call made it, or if the
                // FE says the user exists (this call did not create it: a
                // recorded user the FE had lost, re-created by someone else).
                if inserted || e.is_already_exists() {
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

/// After a StarRocks apply (`engines::finish_apply`): if the engine wants a
/// sync, wait for the FE and a real root login exactly as the bootstrap does,
/// pass a checkpoint, and reconcile. Spawned, never awaited by a request.
pub(crate) fn spawn_user_sync(
    st: CpState,
    engine_id: Uuid,
    name: String,
    namespace: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(Some(engine)) = st.engine_mgmt.get_engine(engine_id).await else {
            return;
        };
        if !wants_user_sync(&st, &engine).await {
            return;
        }
        let mut errors: u32 = 0;
        let Some((_, fe)) =
            wait_for_fe_root_login(&st, engine_id, &name, &namespace, &mut errors).await
        else {
            return;
        };
        if !engine_still_current(&st.engine_mgmt, engine_id, &name, &mut errors).await {
            return;
        }
        sync_current_engine(&st, engine_id, &fe).await;
    })
}

/// Re-read the engine, then reconcile THAT row and log the outcome. Every
/// caller goes through here (or re-reads the same way, as the pass does), so
/// no sync ever plans from a snapshot taken earlier: a stale snapshot that no
/// longer reads as impersonate-mode would plan an empty desired set and drop
/// every user egret recorded. `None` when the engine is gone or cannot be
/// read; it is skipped, not synced.
pub(crate) async fn sync_current_engine(
    st: &CpState,
    engine_id: Uuid,
    fe: &FeTarget,
) -> Option<UserSync> {
    let engine = current_engine(st, engine_id).await?;
    let outcome = sync_starrocks_users(st, &engine, fe).await;
    log_outcome(&engine.name, &outcome);
    Some(outcome)
}

/// The engine row as it is now; `None` (logged) when gone or unreadable.
async fn current_engine(st: &CpState, engine_id: Uuid) -> Option<Engine> {
    match st.engine_mgmt.get_engine(engine_id).await {
        Ok(Some(engine)) => Some(engine),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(%engine_id, error = %e,
                "starrocks users: the engine could not be re-read; skipped");
            None
        }
    }
}

/// One pass over every engine that wants a sync, one at a time. Returns how
/// many reconciled (`Done`).
pub(crate) async fn sync_all_starrocks_users(st: &CpState) -> usize {
    let engines = match st.engine_mgmt.list_engines().await {
        Ok(engines) => engines,
        Err(e) => {
            tracing::warn!(error = %e, "starrocks users: listing the engines failed");
            return 0;
        }
    };
    let mut synced = 0;
    for listed in engines {
        // The pass is serial, so a listed row can be minutes old by its turn.
        let Some(engine) = current_engine(st, listed.id).await else {
            continue;
        };
        if !wants_user_sync(st, &engine).await {
            continue;
        }
        let fe = match fe_target(st, &engine).await {
            Ok(fe) => fe,
            Err(e) => {
                tracing::warn!(engine = %engine.name, error = ?e,
                    "starrocks users: the FE connection could not be resolved");
                continue;
            }
        };
        let outcome = sync_starrocks_users(st, &engine, &fe).await;
        log_outcome(&engine.name, &outcome);
        if matches!(outcome, UserSync::Done { .. }) {
            synced += 1;
        }
    }
    synced
}

/// Ask the worker for a pass. The channel holds one wake-up, so a full
/// channel already holds this one: dropping it is the coalescing.
fn wake_worker(wake: &tokio::sync::mpsc::Sender<()>) {
    let _ = wake.try_send(());
}

/// The worker: on a wake-up, wait `debounce`, take every wake-up that came
/// meanwhile, then run one `pass`. Wake-ups during a pass leave one queued,
/// which costs exactly one more pass. Ends when every sender is gone.
async fn run_coalesced<F, Fut>(
    mut wakes: tokio::sync::mpsc::Receiver<()>,
    debounce: Duration,
    mut pass: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    while wakes.recv().await.is_some() {
        tokio::time::sleep(debounce).await;
        while wakes.try_recv().is_ok() {}
        pass().await;
    }
}

/// Start the sync's three tasks: a `LISTEN` on [`USER_SYNC_CHANNEL`], a
/// sweep every [`SWEEP_INTERVAL`] whose first tick is immediate (the boot
/// reconcile), and the worker that runs [`sync_all_starrocks_users`] once per
/// burst of wake-ups ([`run_coalesced`]): migration 0219's triggers are
/// statement-level and also fire on unrelated `access_grants` writes and on
/// writes that change no row, so N notifications must not cost N passes. The
/// listener reconnects on its own; what it misses meanwhile, the sweep
/// catches. Returns the worker's handle.
pub fn spawn_starrocks_user_sync(st: CpState) -> tokio::task::JoinHandle<()> {
    let (wake, wakes) = tokio::sync::mpsc::channel::<()>(1);
    {
        let wake = wake.clone();
        let pool = st.engine_mgmt.pool().clone();
        tokio::spawn(async move {
            loop {
                let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, "starrocks users: cannot connect the listener");
                        tokio::time::sleep(LISTEN_RETRY).await;
                        continue;
                    }
                };
                if let Err(e) = listener.listen(USER_SYNC_CHANNEL).await {
                    tracing::warn!(error = %e, "starrocks users: LISTEN failed");
                    tokio::time::sleep(LISTEN_RETRY).await;
                    continue;
                }
                // A (re)connect may have missed notifications.
                wake_worker(&wake);
                loop {
                    match listener.recv().await {
                        Ok(_) => wake_worker(&wake),
                        Err(e) => {
                            tracing::warn!(error = %e, "starrocks users: listener error");
                            tokio::time::sleep(LISTEN_RETRY).await;
                            break;
                        }
                    }
                }
            }
        });
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            tick.tick().await;
            wake_worker(&wake);
        }
    });
    tokio::spawn(run_coalesced(wakes, SYNC_DEBOUNCE, move || {
        let st = st.clone();
        async move {
            let synced = sync_all_starrocks_users(&st).await;
            tracing::debug!(engines = synced, "starrocks users: sync pass done");
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    /// A burst of wake-ups (the statement triggers also fire on unrelated
    /// grant writes and on writes that change no row) runs ONE pass, even
    /// when it is spread over the debounce; and wake-ups during a pass run
    /// exactly one more.
    #[tokio::test]
    async fn a_burst_of_wake_ups_runs_one_pass() {
        let passes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (wake, wakes) = tokio::sync::mpsc::channel(1);
        let worker = {
            let passes = passes.clone();
            tokio::spawn(run_coalesced(
                wakes,
                Duration::from_millis(300),
                move || {
                    let passes = passes.clone();
                    async move {
                        passes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                },
            ))
        };
        let count = || passes.load(std::sync::atomic::Ordering::SeqCst);
        let burst = || async {
            for _ in 0..10 {
                wake_worker(&wake);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };

        // A burst inside the debounce, then quiet.
        burst().await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(count(), 1, "a burst coalesces into one pass");

        // One wake-up, then a burst while its pass runs (300 ms after it).
        wake_worker(&wake);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(count(), 2, "premise: the pass is running");
        burst().await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(count(), 3, "a burst during a pass costs one more pass");

        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(count(), 3, "and nothing more");
        worker.abort();
    }

    #[test]
    fn nothing_desired_drops_every_recorded_user_and_touches_no_other() {
        let plan = plan_users(&set(&[]), &set(&["a", "b"]), &set(&["root", "a", "other"]));
        assert_eq!(plan.drop, vec!["a".to_string()]);
        assert_eq!(plan.forget, vec!["b".to_string()]);
        assert!(plan.create.is_empty() && plan.foreign.is_empty());
    }
}
