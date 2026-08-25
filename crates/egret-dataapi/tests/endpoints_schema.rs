//! Proves `EndpointSnapshot` is genuinely self-contained and stable across a
//! real jsonb round trip: serialize → insert into `dataapi.endpoint_versions`
//! → select back → deserialize → `assert_eq!`. A field that fails to round
//! trip would silently change a published contract out from under every
//! client already calling it — P4's serving path reads ONLY this column, never
//! the draft row.
//!
//! DB-gated integration test. Skips (passes) when DATABASE_URL is unset so the
//! suite stays green offline; CI / `just up` provides Postgres. Fixture shape
//! copied from `crates/egret-dataapi/tests/clients_store.rs`.
use egret_dataapi_api::{
    CacheScope, CacheSpec, ColumnSpec, EndpointSnapshot, LimitsSpec, OutputSpec, ParamLocation,
    ParamSpec, ParamType, RateLimitSpec, StatementSnapshot, StatementSpec,
};
use egret_metastore::Db;
use uuid::Uuid;
#[path = "test_db/mod.rs"]
mod test_db;

struct Fixture {
    db: Db,
    admin: Uuid,
    ws: Uuid,
    /// A real `query.databases` row, granted to `ws`. `endpoints.database_id`
    /// has a real FK (`endpoints_database_id_fkey`, RESTRICT) as of the fix
    /// round on this task, so every insert needs a row this can point at.
    database: Uuid,
}

async fn fixture() -> Option<Fixture> {
    // One private database per test process, cloned from a migrated template.
    // See tests/test_db/mod.rs for why sharing one database made the suite
    // slower every time it ran.
    let url = test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");

    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'admin','local','active','x') RETURNING id",
    )
    .bind(format!("admin_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("admin user");

    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("workspace");

    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port, created_by) \
         VALUES ($1, $2, 'trino', 'localhost', 8080, $3) RETURNING id",
    )
    .bind(ws)
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(admin)
    .fetch_one(&db.pool)
    .await
    .expect("database");

    Some(Fixture {
        db,
        admin,
        ws,
        database,
    })
}

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!("DATABASE_URL unset — skipping");
                return;
            }
        }
    };
}

/// Builds a snapshot that exercises EVERY field, not just defaults: a
/// `String` param, an `Enum` param carrying a non-empty `allowed` list (the
/// "only safe form" per spec §3), non-default `cache`/`limits`/`rate_limit`
/// values, and a non-empty `columns` vec. A round trip over an
/// all-defaults fixture would prove nothing — this one would catch a field
/// that got dropped, renamed, or silently defaulted back.
fn fixture_snapshot(database_id: Uuid) -> EndpointSnapshot {
    EndpointSnapshot {
        statements: vec![StatementSnapshot {
            name: "main".to_string(),
            sql: "SELECT id, status FROM orders WHERE status = :status".to_string(),
            columns: vec![
                ColumnSpec {
                    name: "id".to_string(),
                    ty: "uuid".to_string(),
                    required: true,
                },
                // Deliberately `false`, unlike `id` above: a round trip that
                // dropped `required` or coerced it back to the pre-M1 default of
                // `true` would leave both columns looking identical and this
                // assertion would not catch it.
                ColumnSpec {
                    name: "status".to_string(),
                    ty: "varchar".to_string(),
                    required: false,
                },
            ],
        }],
        method: "GET".to_string(),
        path: "/orders/by-status".to_string(),
        database_id,
        params: vec![
            ParamSpec {
                name: "region".to_string(),
                ty: ParamType::String,
                location: ParamLocation::Query,
                required: true,
                default: Some("us".to_string()),
                allowed: None,
                description: "Region code".to_string(),
            },
            ParamSpec {
                name: "status".to_string(),
                ty: ParamType::Enum,
                location: ParamLocation::Query,
                required: true,
                default: None,
                allowed: Some(vec![
                    "open".to_string(),
                    "closed".to_string(),
                    "pending".to_string(),
                ]),
                description: "Order status".to_string(),
            },
        ],
        output: OutputSpec {
            shape: "arrays".to_string(),
            naming: "camel".to_string(),
            envelope: "bare".to_string(),
        },
        cache: CacheSpec {
            enabled: true,
            ttl_seconds: 120,
            scope: CacheScope::Shared,
        },
        limits: LimitsSpec {
            max_rows: 500,
            timeout_seconds: 15,
        },
        rate_limit: RateLimitSpec {
            per_key_qps: 5,
            burst: 10,
            per_endpoint_qps: 50,
        },
        // The witness for the one snapshot-external input: `query.databases.
        // delegated` is mutable, so P4 has to be able to tell what publish
        // validated against. `true` here rather than the `false` default, so a
        // field that failed to round-trip could not pass by accident.
        delegated: true,
        // The other half of the witness pair. Deliberately DIFFERENT from
        // `delegated` here, per this fixture's own rule of using distinguishable
        // values: a round trip that set both to `true` could not catch the two
        // being swapped, and swapping them is exactly the mistake that turns a
        // safety check into a coin flip.
        impersonate: false,
        // Non-default on purpose, like every other field here: a round trip that
        // dropped this would leave `0`, which is the value the serving path
        // refuses.
        param_gate: egret_dataapi_api::PARAM_GATE,
    }
}

#[tokio::test]
async fn a_snapshot_round_trips_through_postgres_jsonb_unchanged() {
    let f = skip_without_db!();

    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
         (workspace_id, name, path, method, database_id, statements, created_by) \
         VALUES ($1, 'orders by status', '/orders/by-status', 'GET', $2, \
                 jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), $3) \
         RETURNING id",
    )
    .bind(f.ws)
    .bind(f.database)
    .bind(f.admin)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert endpoint draft");

    // The snapshot's own `database_id` is frozen independently of the draft
    // row's FK — a fresh uuid here is deliberate, proving the snapshot
    // doesn't need to reference a live `query.databases` row to round-trip.
    let original = fixture_snapshot(Uuid::new_v4());
    let payload = serde_json::to_value(&original).expect("serialize snapshot");

    let round_tripped_json: serde_json::Value = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoint_versions (endpoint_id, version, snapshot, note, created_by) \
         VALUES ($1, 1, $2, 'first publish', $3) \
         RETURNING snapshot",
    )
    .bind(endpoint_id)
    .bind(&payload)
    .bind(f.admin)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert + select snapshot back");

    let round_tripped: EndpointSnapshot =
        serde_json::from_value(round_tripped_json).expect("deserialize snapshot");

    assert_eq!(
        round_tripped, original,
        "EndpointSnapshot must round-trip through jsonb unchanged — a drifted \
         field here would silently change an already-published contract"
    );
}

/// `endpoints.database_id` must be a real FK (`endpoints_database_id_fkey`),
/// not a bare `uuid NOT NULL`. Without it, nothing stops the column pointing
/// at a database that never existed — and the dangle would only surface as a
/// runtime failure in P4's serving path, with no DB-level signal at write
/// time.
#[tokio::test]
async fn an_endpoint_cannot_reference_a_nonexistent_database() {
    let f = skip_without_db!();

    let err = sqlx::query(
        "INSERT INTO dataapi.endpoints \
         (workspace_id, name, path, method, database_id, statements, created_by) \
         VALUES ($1, 'dangling', '/dangling', 'GET', $2, jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), $3)",
    )
    .bind(f.ws)
    .bind(Uuid::new_v4()) // no such row in query.databases
    .bind(f.admin)
    .execute(&f.db.pool)
    .await
    .expect_err("insert must be rejected by the database_id FK");

    let db_err = err.as_database_error().expect("a Postgres error");
    assert_eq!(
        db_err.constraint(),
        Some("endpoints_database_id_fkey"),
        "must fail specifically on the database_id FK, not some other constraint: {db_err}"
    );
}

/// An endpoint that existed before M2 must come out of migration 0129 with its
/// SQL intact, as a one-element `statements` list named `main`.
///
/// A migration is a one-way door on data, so this runs the **embedded bytes of
/// migration 0129** — the same `Migration` value the migrator applies — rather
/// than a transcription of its statements into the test. A transcription would
/// pass against SQL no database ever ran; this fails if the file is renumbered,
/// and it fails if `egret-metastore` was not touched and the embedded set is
/// stale, which is a recorded failure mode in this repo.
///
/// The pre-M2 shape is rebuilt inside a transaction that is never committed.
/// Postgres DDL is transactional, so dropping `statements`, putting `sql` back,
/// inserting an old-shape row and re-running the migration all disappear on
/// rollback — the shared test database is left exactly as it was.
#[tokio::test]
async fn an_endpoint_written_before_m2_survives_the_migration() {
    let f = skip_without_db!();

    // Quotes and a trailing comment, not `SELECT 1`: the conversion goes through
    // `jsonb_build_object`, and a string that survives only because it contains
    // nothing worth mangling proves nothing about one that does.
    const ORIGINAL: &str = "SELECT id, name FROM orders WHERE region = 'EU' -- ships";

    let migration = egret_metastore::MIGRATOR
        .migrations
        .iter()
        .find(|m| m.version == 129)
        .expect("migration 0129 must be embedded — touch egret-metastore's lib.rs if it is not");

    let mut tx = f.db.pool.begin().await.expect("begin");

    // Rewind this table to its pre-0129 shape. Dropping the column takes the
    // non-empty CHECK with it, since that constraint names no other column.
    sqlx::raw_sql(
        "ALTER TABLE dataapi.endpoints DROP COLUMN statements; \
         ALTER TABLE dataapi.endpoints ADD COLUMN sql text NOT NULL DEFAULT ''",
    )
    .execute(&mut *tx)
    .await
    .expect("rewind to the pre-M2 shape");

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
         (workspace_id, name, path, method, database_id, sql, created_by) \
         VALUES ($1, 'legacy', '/legacy', 'GET', $2, $3, $4) RETURNING id",
    )
    .bind(f.ws)
    .bind(f.database)
    .bind(ORIGINAL)
    .bind(f.admin)
    .fetch_one(&mut *tx)
    .await
    .expect("insert a pre-M2 draft");

    sqlx::raw_sql(migration.sql.clone())
        .execute(&mut *tx)
        .await
        .expect("apply migration 0129");

    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT statements FROM dataapi.endpoints WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .expect("read the converted row");

    assert_eq!(
        stored,
        serde_json::json!([{ "name": "main", "sql": ORIGINAL }]),
        "an existing draft must convert to exactly one statement named `main`, \
         carrying its SQL byte for byte"
    );

    // And the shape the DRAFT ROW is read back through, not just the shape a
    // human reads: `StatementSpec` is `deny_unknown_fields`, so a conversion
    // that emitted an extra key would fail here rather than in production.
    let parsed: Vec<StatementSpec> =
        serde_json::from_value(stored).expect("the converted list must be a Vec<StatementSpec>");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name, "main");
    assert_eq!(parsed[0].sql, ORIGINAL);

    // The old column is gone, so nothing can go on reading it and quietly serve
    // the pre-M2 definition.
    let sql_column_still_there: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = 'dataapi' AND table_name = 'endpoints' AND column_name = 'sql')",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("read the column list");
    assert!(
        !sql_column_still_there,
        "migration 0129 must drop `sql`, not leave two definitions of the same thing"
    );

    // An empty list is refused by the schema itself, so a draft with nothing to
    // run cannot be stored by any writer, present or future.
    let empty = sqlx::query("UPDATE dataapi.endpoints SET statements = '[]'::jsonb WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .expect_err("an empty statement list must be refused");
    assert_eq!(
        empty
            .as_database_error()
            .expect("a Postgres error")
            .constraint(),
        Some("dataapi_endpoints_statements_nonempty"),
        "must fail on the non-empty CHECK, not on some other constraint: {empty}"
    );

    tx.rollback().await.expect("rollback");
}
