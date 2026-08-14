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
    ParamSpec, ParamType, RateLimitSpec,
};
use egret_metastore::Db;
use uuid::Uuid;

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
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

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
        sql: "SELECT id, status FROM orders WHERE status = :status".to_string(),
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
        columns: vec![
            ColumnSpec {
                name: "id".to_string(),
                ty: "uuid".to_string(),
            },
            ColumnSpec {
                name: "status".to_string(),
                ty: "varchar".to_string(),
            },
        ],
    }
}

#[tokio::test]
async fn a_snapshot_round_trips_through_postgres_jsonb_unchanged() {
    let f = skip_without_db!();

    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
         (workspace_id, name, path, method, database_id, sql, created_by) \
         VALUES ($1, 'orders by status', '/orders/by-status', 'GET', $2, 'SELECT 1', $3) \
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
         (workspace_id, name, path, method, database_id, sql, created_by) \
         VALUES ($1, 'dangling', '/dangling', 'GET', $2, 'SELECT 1', $3)",
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
