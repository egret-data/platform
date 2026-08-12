//! Running an alert's query and deciding whether it fires.
//!
//! # Whose permissions the query runs with
//!
//! This is the security decision of the whole subsystem (§5.2). A schedule runs
//! with nobody logged in, and the obvious implementation runs it as a service
//! account — which turns every alert into a privilege-escalation primitive:
//! write SQL against a table you cannot read, receive its rows by email.
//!
//! So the query runs as the schedule's **owner**, and authorization is settled
//! by the engine at RUN time, not remembered from save time:
//!
//! - The identity handed to the connector is the owner's — their engine
//!   username, and for a delegated engine a token minted for *them*.
//! - If the owner has lost access, the engine refuses and the run is recorded
//!   as `error`. It does not fall back to a service identity, and it does not
//!   quietly keep delivering data the owner may no longer see.
//! - If no token can be minted for the owner (deactivated, no refresh token),
//!   that is an error too. A missing owner credential must never degrade into
//!   "run it as whoever the process is".
//!
//! Identity resolution is deliberately its own function, and execution is
//! behind a trait, so both are assertable without an engine — a fake runner can
//! be asked which identity it was handed.

use egret_analytics_api::engine_token::EngineTokenMinter;
use egret_auth::users::UserRepo;
use egret_core::{TabularResult, UserId};
use egret_query::DatabaseRepo;
use uuid::Uuid;

use crate::validator::{should_fire, ValidatorError};
use egret_automation_api::ValidatorType;

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("schedule has no owner to run as")]
    NoOwner,
    #[error("could not obtain engine credentials for the owner: {0}")]
    OwnerCredentials(String),
    #[error("query failed: {0}")]
    Query(String),
    #[error("alert query must return a number or NULL in its first column, got {0}")]
    NotANumber(String),
    #[error("{0}")]
    Validator(#[from] ValidatorError),
    #[error("schedule is not a runnable alert: {0}")]
    NotRunnable(&'static str),
}

/// The engine identity a run executes under. Never constructed from the caller
/// or the process — only from an owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIdentity {
    pub session_user: Option<String>,
    pub bearer_token: Option<String>,
}

/// Execution seam, so the evaluator (and, via the same trait, the render
/// payload builder — see `render::payload`) can be tested without an engine.
///
/// `limit` is the caller's row cap, not a fixed constant of this trait: an
/// alert only ever reads the first cell of the first row (`evaluate` passes
/// `Some(1)`), while a rendered chart card needs its full saved `row_limit`.
/// Baking `1` into the runner itself — as this used to do — would silently
/// truncate every rendered report to one row per card.
#[async_trait::async_trait]
pub trait QueryRunner: Send + Sync {
    async fn run(
        &self,
        database_id: Uuid,
        sql: &str,
        identity: &RunIdentity,
        limit: Option<usize>,
    ) -> Result<TabularResult, String>;
}

/// The production runner: the same `run_query` path the interactive endpoints
/// use, so a scheduled query, a hand-run one, and a rendered report card all
/// go through identical code.
pub struct EngineRunner {
    pub databases: DatabaseRepo,
    pub factory: std::sync::Arc<dyn egret_query::execution::connector::ConnectorFactory>,
    pub limits: egret_query::execution::model::Limits,
}

#[async_trait::async_trait]
impl QueryRunner for EngineRunner {
    async fn run(
        &self,
        database_id: Uuid,
        sql: &str,
        identity: &RunIdentity,
        limit: Option<usize>,
    ) -> Result<TabularResult, String> {
        egret_query::execution::run_query(
            &self.databases,
            &*self.factory,
            database_id,
            sql,
            limit,
            identity.session_user.clone(),
            identity.bearer_token.clone(),
            &self.limits,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// Build the engine identity for `owner`.
///
/// A delegated engine gets a token minted for the owner; a non-delegated one
/// gets the owner's engine username. Either way the identity is the owner's —
/// there is no branch here that yields a service identity, which is the point.
pub async fn owner_identity(
    owner: Uuid,
    database_id: Uuid,
    databases: &DatabaseRepo,
    users: &UserRepo,
    minter: &dyn EngineTokenMinter,
) -> Result<RunIdentity, EvalError> {
    let db = databases
        .get(database_id)
        .await
        .map_err(|e| EvalError::Query(e.to_string()))?;
    let username = users
        .ensure_username(UserId(owner))
        .await
        .map_err(|e| EvalError::OwnerCredentials(e.to_string()))?;
    if !db.delegated {
        return Ok(RunIdentity {
            session_user: Some(username),
            bearer_token: None,
        });
    }
    // A delegated engine without a token for THIS owner is a hard stop. The
    // tempting fallback — run it unauthenticated, or as the service — is
    // exactly the escalation this design exists to prevent.
    let token = minter
        .mint(owner)
        .await
        .map_err(|e| EvalError::OwnerCredentials(e.to_string()))?;
    Ok(RunIdentity {
        session_user: Some(username),
        bearer_token: Some(token.access_token),
    })
}

/// What a completed evaluation decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub fired: bool,
    pub value: Option<f64>,
    /// The triggering row, for the notification body.
    pub rows: serde_json::Value,
    /// The query's own column names, carried so an attached CSV can be headed
    /// with what the user actually selected. Without them the header falls back
    /// to `column_1`, which is worse than useless in a 3am email: it makes the
    /// reader map positions by hand against SQL they cannot see.
    pub columns: Vec<String>,
}

/// Run `sql` as `identity` and apply the validator.
pub async fn evaluate(
    runner: &dyn QueryRunner,
    database_id: Uuid,
    sql: &str,
    validator: ValidatorType,
    validator_config: &serde_json::Value,
    identity: &RunIdentity,
) -> Result<Evaluation, EvalError> {
    // An alert only ever reads the first cell of the first row (§5.2's
    // `first_cell`), so it asks the runner for exactly one.
    let result = runner
        .run(database_id, sql, identity, Some(1))
        .await
        .map_err(EvalError::Query)?;

    let value = first_cell(&result)?;
    let fired = should_fire(validator, validator_config, value)?;
    let rows = serde_json::to_value(&result.rows).unwrap_or(serde_json::Value::Null);
    let columns = result.columns.iter().map(|c| c.name.clone()).collect();
    Ok(Evaluation {
        fired,
        value,
        rows,
        columns,
    })
}

/// The first column of the first row (§5.2), as a number.
///
/// A non-numeric, non-null cell is an ERROR rather than "no value": an alert
/// whose query returns a string has been written wrongly, and reporting that as
/// "condition not met" would hide the mistake for as long as the alert exists.
fn first_cell(result: &TabularResult) -> Result<Option<f64>, EvalError> {
    let Some(row) = result.rows.first() else {
        return Ok(None); // no rows: nothing observed
    };
    let Some(cell) = row.first() else {
        return Ok(None);
    };
    match cell {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => Ok(n.as_f64()),
        serde_json::Value::Bool(b) => Ok(Some(if *b { 1.0 } else { 0.0 })),
        // Engines return numerics as strings often enough (decimals, bigints)
        // that refusing a numeric string would break ordinary alerts.
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map(Some)
            .map_err(|_| EvalError::NotANumber(format!("{s:?}"))),
        other => Err(EvalError::NotANumber(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::tabular::ColumnMeta;
    use std::sync::Mutex;

    fn result_of(cells: Vec<serde_json::Value>) -> TabularResult {
        TabularResult {
            columns: vec![ColumnMeta {
                name: "v".into(),
                ty: "bigint".into(),
            }],
            row_count: cells.len() as u64,
            rows: cells.into_iter().map(|c| vec![c]).collect(),
            bytes_scanned: None,
            truncated: false,
        }
    }

    /// Records the identity it was handed, so "ran as the owner" is checkable.
    struct SpyRunner {
        seen: Mutex<Option<RunIdentity>>,
        answer: Result<TabularResult, String>,
    }

    impl SpyRunner {
        fn returning(cells: Vec<serde_json::Value>) -> Self {
            Self {
                seen: Mutex::new(None),
                answer: Ok(result_of(cells)),
            }
        }
        fn refusing(msg: &str) -> Self {
            Self {
                seen: Mutex::new(None),
                answer: Err(msg.to_string()),
            }
        }
    }

    #[async_trait::async_trait]
    impl QueryRunner for SpyRunner {
        async fn run(
            &self,
            _db: Uuid,
            _sql: &str,
            identity: &RunIdentity,
            _limit: Option<usize>,
        ) -> Result<TabularResult, String> {
            *self.seen.lock().unwrap() = Some(identity.clone());
            self.answer.clone()
        }
    }

    fn owner_id() -> RunIdentity {
        RunIdentity {
            session_user: Some("alice".into()),
            bearer_token: Some("owner-token".into()),
        }
    }

    #[tokio::test]
    async fn the_query_is_executed_with_the_identity_it_was_given() {
        // The evaluator must pass the owner's identity through untouched — a
        // dropped token would silently downgrade the run to whatever the engine
        // does for an anonymous connection.
        let runner = SpyRunner::returning(vec![serde_json::json!(7)]);
        evaluate(
            &runner,
            Uuid::new_v4(),
            "SELECT count(*) FROM orders",
            ValidatorType::NotNull,
            &serde_json::json!({}),
            &owner_id(),
        )
        .await
        .unwrap();
        assert_eq!(
            runner.seen.lock().unwrap().clone(),
            Some(owner_id()),
            "the engine must see the owner's identity, unmodified"
        );
    }

    #[tokio::test]
    async fn an_owner_denied_by_the_engine_is_an_error_not_a_silent_pass() {
        // THE privilege-escalation guard. The owner lost access; the engine
        // refuses. The run must fail loudly rather than return rows, and
        // nothing may retry it under another identity.
        let runner = SpyRunner::refusing("Access Denied: Cannot select from table orders");
        let err = evaluate(
            &runner,
            Uuid::new_v4(),
            "SELECT count(*) FROM orders",
            ValidatorType::NotNull,
            &serde_json::json!({}),
            &owner_id(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, EvalError::Query(m) if m.contains("Access Denied")),
            "expected the engine's refusal to surface: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_firing_evaluation_carries_the_value_and_the_row() {
        let runner = SpyRunner::returning(vec![serde_json::json!(42)]);
        let out = evaluate(
            &runner,
            Uuid::new_v4(),
            "q",
            ValidatorType::NotNull,
            &serde_json::json!({}),
            &owner_id(),
        )
        .await
        .unwrap();
        assert!(out.fired);
        assert_eq!(out.value, Some(42.0));
        assert_eq!(out.rows[0][0], 42, "the triggering row rides along");
    }

    #[tokio::test]
    async fn the_evaluation_carries_the_querys_column_names() {
        // They end up as the CSV header in the alert email. Dropping them
        // leaves a reader mapping `column_1` against SQL they cannot see.
        let runner = SpyRunner::returning(vec![serde_json::json!(7)]);
        let out = evaluate(
            &runner,
            Uuid::new_v4(),
            "q",
            ValidatorType::NotNull,
            &serde_json::json!({}),
            &owner_id(),
        )
        .await
        .unwrap();
        assert_eq!(out.columns, vec!["v".to_string()]);
    }

    #[tokio::test]
    async fn an_empty_result_does_not_fire() {
        let runner = SpyRunner::returning(vec![]);
        let out = evaluate(
            &runner,
            Uuid::new_v4(),
            "q",
            ValidatorType::NotNull,
            &serde_json::json!({}),
            &owner_id(),
        )
        .await
        .unwrap();
        assert!(!out.fired, "no rows is not a trigger");
        assert_eq!(out.value, None);
    }

    #[test]
    fn the_first_cell_reads_the_shapes_engines_actually_return() {
        assert_eq!(first_cell(&result_of(vec![])).unwrap(), None);
        assert_eq!(
            first_cell(&result_of(vec![serde_json::Value::Null])).unwrap(),
            None
        );
        assert_eq!(
            first_cell(&result_of(vec![serde_json::json!(3)])).unwrap(),
            Some(3.0)
        );
        assert_eq!(
            first_cell(&result_of(vec![serde_json::json!(2.5)])).unwrap(),
            Some(2.5)
        );
        // Decimals and bigints commonly arrive as strings.
        assert_eq!(
            first_cell(&result_of(vec![serde_json::json!("12.75")])).unwrap(),
            Some(12.75)
        );
        assert_eq!(
            first_cell(&result_of(vec![serde_json::json!(true)])).unwrap(),
            Some(1.0)
        );
        assert_eq!(
            first_cell(&result_of(vec![serde_json::json!(false)])).unwrap(),
            Some(0.0)
        );
    }

    #[test]
    fn a_non_numeric_cell_is_an_error_rather_than_a_quiet_no_fire() {
        // An alert selecting a name instead of a count is a mistake. Reporting
        // it as "condition not met" would hide that for the life of the alert.
        assert!(matches!(
            first_cell(&result_of(vec![serde_json::json!("pending")])),
            Err(EvalError::NotANumber(_))
        ));
        assert!(matches!(
            first_cell(&result_of(vec![serde_json::json!({"a": 1})])),
            Err(EvalError::NotANumber(_))
        ));
    }

    // ── owner_identity: where the security property is actually decided ─────

    /// Records which user a token was minted for, and can refuse.
    struct SpyMinter {
        minted_for: Mutex<Vec<Uuid>>,
        refuse: bool,
    }

    #[async_trait::async_trait]
    impl EngineTokenMinter for SpyMinter {
        async fn mint(
            &self,
            user_id: Uuid,
        ) -> Result<
            egret_analytics_api::engine_token::EngineToken,
            egret_analytics_api::engine_token::BrokerError,
        > {
            self.minted_for.lock().unwrap().push(user_id);
            if self.refuse {
                return Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken);
            }
            Ok(egret_analytics_api::engine_token::EngineToken {
                access_token: format!("token-for-{user_id}"),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            })
        }
    }

    async fn seed_db_and_users(db: &egret_metastore::Db, delegated: bool) -> (Uuid, Uuid, Uuid) {
        let mk_user = |email: String| async move {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id",
            )
            .bind(email)
            .fetch_one(&db.pool)
            .await
            .unwrap()
        };
        let owner = mk_user(format!("owner_{}@t.local", Uuid::new_v4().simple())).await;
        let other = mk_user(format!("other_{}@t.local", Uuid::new_v4().simple())).await;
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
        )
        .bind(format!("w-{}", Uuid::new_v4().simple()))
        .bind(owner)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let database: Uuid = sqlx::query_scalar(
            "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
             VALUES ($1,'trino','localhost',8080,$2,$3) RETURNING id",
        )
        .bind(format!("db-{}", Uuid::new_v4().simple()))
        .bind(ws)
        .bind(delegated)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        (owner, other, database)
    }

    fn repos(db: &egret_metastore::Db) -> (DatabaseRepo, UserRepo) {
        let secrets = egret_secrets::SecretStore::new(db.clone(), None);
        let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
        (
            DatabaseRepo::new(db.clone(), secrets, ws_secrets),
            UserRepo::new(db.clone()),
        )
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_token_is_minted_for_the_owner_and_nobody_else(pool: sqlx::PgPool) {
        // THE assertion of §5.2. A schedule runs with nobody logged in, so the
        // only thing standing between "alert" and "read any table by email" is
        // that this token belongs to the owner. `other` exists precisely so the
        // assertion cannot pass by there being only one user to choose.
        let db = egret_metastore::Db { pool };
        let (owner, other, database) = seed_db_and_users(&db, true).await;
        let (databases, users) = repos(&db);
        let minter = SpyMinter {
            minted_for: Mutex::new(vec![]),
            refuse: false,
        };

        let identity = owner_identity(owner, database, &databases, &users, &minter)
            .await
            .unwrap();

        assert_eq!(
            *minter.minted_for.lock().unwrap(),
            vec![owner],
            "the engine token must be minted for the OWNER"
        );
        assert_ne!(owner, other);
        assert_eq!(
            identity.bearer_token.as_deref(),
            Some(format!("token-for-{owner}").as_str())
        );
        assert!(
            identity.session_user.is_some(),
            "the engine principal must be named too"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_owner_with_no_mintable_token_fails_instead_of_falling_back(pool: sqlx::PgPool) {
        // A deactivated owner, or one whose refresh token is gone. The only
        // safe answer is to stop: running anyway — unauthenticated, or as the
        // service — is the escalation this whole design exists to prevent, and
        // it would look like an alert that simply kept working.
        let db = egret_metastore::Db { pool };
        let (owner, _other, database) = seed_db_and_users(&db, true).await;
        let (databases, users) = repos(&db);
        let minter = SpyMinter {
            minted_for: Mutex::new(vec![]),
            refuse: true,
        };

        let err = owner_identity(owner, database, &databases, &users, &minter)
            .await
            .unwrap_err();
        assert!(
            matches!(err, EvalError::OwnerCredentials(_)),
            "must refuse, not degrade: {err:?}"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_non_delegated_engine_still_runs_under_the_owners_name(pool: sqlx::PgPool) {
        // No token to mint, but the identity must still be the owner's rather
        // than a shared connection user.
        let db = egret_metastore::Db { pool };
        let (owner, _other, database) = seed_db_and_users(&db, false).await;
        let (databases, users) = repos(&db);
        let minter = SpyMinter {
            minted_for: Mutex::new(vec![]),
            refuse: true, // would fail if it were called at all
        };

        let identity = owner_identity(owner, database, &databases, &users, &minter)
            .await
            .unwrap();
        assert!(
            minter.minted_for.lock().unwrap().is_empty(),
            "a non-delegated engine must not mint"
        );
        assert!(identity.session_user.is_some());
        assert_eq!(identity.bearer_token, None);
    }

    #[tokio::test]
    async fn a_broken_validator_config_surfaces_rather_than_not_firing() {
        let runner = SpyRunner::returning(vec![serde_json::json!(5)]);
        let err = evaluate(
            &runner,
            Uuid::new_v4(),
            "q",
            ValidatorType::Operator,
            &serde_json::json!({"op": ">"}), // no threshold
            &owner_id(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, EvalError::Validator(_)), "{err:?}");
    }
}
