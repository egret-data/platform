//! DB-gated PolicyStore service tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
use egret_metastore::Db;
use egret_policy::{seed_tag, seed_trino, PolicyStore};
use serde_json::{json, Value};

async fn store() -> Option<PolicyStore> {
    Some(store_and_db().await?.0)
}

/// Same connection, plus the raw `Db` — the second-direction test needs to issue
/// `UPDATE`s the store has no method for, because that is exactly the point:
/// there IS no such method anywhere in `src/`.
async fn store_and_db() -> Option<(PolicyStore, Db)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some((PolicyStore::new(db.clone()), db))
}
fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn seed_def_then_create_service() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino");
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap();
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap(); // idempotent
    assert_eq!(s.get_service_def(&defname).await.unwrap().name, defname);

    let svcname = uniq("trino_prod");
    let svc = s
        .create_service(&svcname, &defname, &json!({}))
        .await
        .unwrap();
    assert_eq!(svc.policy_version, 1);
    assert!(matches!(
        s.create_service(&svcname, &defname, &json!({}))
            .await
            .unwrap_err(),
        egret_policy::PolicyStoreError::AlreadyExists(_)
    ));
    s.delete_service(svc.id).await.unwrap();
    assert!(s.get_service(&svcname).await.is_err());
}

/// Part A (Access P3.2) — the DB-testable slice of tag-service provisioning:
/// the tag service-def seeds, a tag service instance exists with type "tag",
/// and a trino profile's config carries the `tagService` link. (The Ranger
/// runtime correctness of the def + the download wiring is LIVE-ONLY.)
#[tokio::test]
async fn tag_def_seeds_and_tag_service_provisions() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    // 1. Tag service-def seeds (idempotent) and reads back.
    seed_tag(&s).await.unwrap();
    seed_tag(&s).await.unwrap(); // idempotent
    let def = s.get_service_def("tag").await.unwrap();
    assert_eq!(def.name, "tag");
    assert_eq!(def.def_jsonb["resources"][0]["name"], "tag");

    // 2. A tag service instance provisions with type "tag".
    let tag_name = uniq("eng_tag");
    let tag_svc = s
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap();
    assert_eq!(tag_svc.r#type, "tag");
    assert_eq!(s.get_service(&tag_name).await.unwrap().r#type, "tag");

    // 3. The bound trino profile's config carries `tagService = <tag service>`.
    //
    // `create_service(.., "trino", ..)` has an FK onto `authz_service_defs`, and this
    // test never seeded that row — it passed only because some OTHER test binary had
    // left a `trino` def behind in the shared database. A private database per process
    // turned that into `authz_services_type_fkey` violated, which is the dependency
    // being made explicit here rather than a new requirement.
    seed_trino(&s).await.unwrap();
    let trino_name = uniq("eng");
    let trino_svc = s
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap();
    assert_eq!(
        s.get_service(&trino_name).await.unwrap().config_jsonb["tagService"],
        json!(tag_name)
    );

    s.delete_service(trino_svc.id).await.unwrap();
    s.delete_service(tag_svc.id).await.unwrap();
}

/// SP4d debt Task 2, **Step 5 — the other direction, and every write path that
/// can reach the state.**
///
/// A `tagService` on a non-Trino profile makes every tag policy in that
/// profile's `tagPolicies` block silently inert: `ServiceTags::from_assignments`
/// writes the resource levels `catalog`/`schema`/`table`/`column` for every
/// profile type, `starrocks.json` has no `schema` resource (its middle level is
/// `database`), and Ranger's `RangerTagEnricher.processServiceTags` drops every
/// attachment whose key set no hierarchy in the resource def contains.
///
/// `PolicyStore::create_service` refuses the CREATE direction (unit-tested
/// DB-free in `store.rs`, and over HTTP in `admin_routes.rs`). This test exists
/// because a guard on create alone leaves the hole open from the other side, and
/// **there is no Rust writer to hang a second refusal on**: no `UPDATE` of
/// `type` or of `config_jsonb` exists in any `src/` in this workspace. So the
/// other direction is held by migration 0165's CHECK, and the only way to
/// exercise it is to issue the raw `UPDATE`s here — a test that only drove the
/// route would be a green from a population that cannot reach this behaviour.
///
/// Three writes, one per shape that can produce the state:
///   1. flip an existing Trino profile's `type` to `starrocks` while it carries
///      a `tagService`;
///   2. write a `tagService` onto an existing StarRocks profile;
///   3. INSERT the pair directly, bypassing `create_service` entirely.
#[tokio::test]
async fn the_check_constraint_closes_the_update_directions_too() {
    let Some((s, db)) = store_and_db().await else {
        eprintln!("skip");
        return;
    };
    seed_trino(&s).await.unwrap();
    seed_tag(&s).await.unwrap();
    egret_policy::seed_starrocks(&s).await.unwrap();

    let tag_name = uniq("dir_tag");
    let tag_svc = s
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap();
    let trino_name = uniq("dir_trino");
    let trino_svc = s
        .create_service(&trino_name, "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap();
    let sr_name = uniq("dir_sr");
    let sr_svc = s
        .create_service(&sr_name, "starrocks", &json!({}))
        .await
        .unwrap();

    // 1. The type flip. This is the hole the obvious create-side test cannot see.
    let e = sqlx::query("UPDATE policy.authz_services SET type = 'starrocks' WHERE id = $1")
        .bind(trino_svc.id)
        .execute(&db.pool)
        .await
        .expect_err("flipping a tagService-carrying profile to starrocks must be refused");
    assert_constraint_violation(&e, "type flip to starrocks");

    // 2. A `tagService` written onto an existing StarRocks profile.
    let e = sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(sr_svc.id)
        .bind(json!({ "tagService": tag_name }))
        .execute(&db.pool)
        .await
        .expect_err("binding a tag service to a starrocks profile must be refused");
    assert_constraint_violation(&e, "tagService written onto starrocks");

    // 3. A raw INSERT, bypassing `create_service` and its Rust refusal entirely.
    let e = sqlx::query(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,'starrocks',$2)",
    )
    .bind(uniq("dir_raw"))
    .bind(json!({ "tagService": tag_name }))
    .execute(&db.pool)
    .await
    .expect_err("a raw INSERT of the pair must be refused");
    assert_constraint_violation(&e, "raw INSERT");

    // Not an over-refusal: the three legal shapes still write.
    sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
        .bind(sr_svc.id)
        .bind(json!({ "tagService": serde_json::Value::Null }))
        .execute(&db.pool)
        .await
        .expect("a JSON-null tagService is what `->>` answers NULL for, and must be allowed");
    sqlx::query("UPDATE policy.authz_services SET type = 'starrocks' WHERE id = $1")
        .bind(sr_svc.id)
        .execute(&db.pool)
        .await
        .expect("a starrocks profile with no usable tagService still updates");
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1",
    )
    .bind(trino_svc.id)
    .execute(&db.pool)
    .await
    .expect("a trino profile with a tagService still bumps");

    s.delete_service(trino_svc.id).await.unwrap();
    s.delete_service(sr_svc.id).await.unwrap();
    s.delete_service(tag_svc.id).await.unwrap();
}

/// The CHECK must be what refused, named — a bare `is_err()` would pass just as
/// well for a typo in the column name or a dropped connection.
fn assert_constraint_violation(e: &sqlx::Error, what: &str) {
    const NAME: &str = "authz_services_tag_service_needs_schema_level";
    let named = match e {
        sqlx::Error::Database(d) => d.constraint() == Some(NAME),
        _ => false,
    };
    assert!(
        named,
        "{what}: expected migration 0165's CHECK {NAME:?} to refuse, got: {e}"
    );
}

/// SP4d debt Task 2 fix round — **the clearing `UPDATE` in migration 0165, which
/// the Rust suite could not otherwise reach.**
///
/// The migration deletes a `tagService` from every non-Trino profile and then adds
/// the CHECK as VALID. A test cannot exercise that against the real table:
/// `egret-test-db` migrates the template BEFORE any test runs, so by the time a
/// test can insert a violating row the constraint is already in force and the
/// clearing statement has already run on an empty table. "I cannot test it here"
/// was a statement about the seam, so the seam moved: this reads the migration's
/// OWN TEXT off disk, re-points it at a fixture table of the same shape, and runs
/// it. A migration edited to drop the `UPDATE`, to narrow it, or to re-weaken the
/// CHECK fails here.
///
/// The SQL is taken from the file rather than retyped, because a retyped copy is
/// an oracle built from the same vocabulary as the thing it checks — it would
/// agree with my understanding of the migration instead of with the migration.
/// Only the table name is substituted.
///
/// One connection throughout: a `TEMP` table lives in the session, so running the
/// statements on a different pooled connection would not see it.
#[tokio::test]
async fn migration_0165_clears_the_inert_key_then_validates_the_check() {
    let Some((_s, db)) = store_and_db().await else {
        eprintln!("skip");
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/0165_tag_service_requires_schema_level.sql"
    );
    let sql = std::fs::read_to_string(path).expect("read migration 0165");
    // Comments only; the two statements are what runs.
    let body: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("config_jsonb - 'tagService'") && body.contains("ADD CONSTRAINT"),
        "migration 0165 no longer carries both the clearing UPDATE and the CHECK; body={body}"
    );
    assert!(
        !body.to_uppercase().contains("NOT VALID"),
        "migration 0165's CHECK must be VALIDATED — a NOT VALID one leaves a row that \
         makes `bump_bound_tag_versions`' bulk UPDATE fail for a whole warehouse; body={body}"
    );
    let fixture = body.replace("policy.authz_services", "t0165");

    let mut conn = db.pool.acquire().await.expect("acquire");
    sqlx::query(
        "CREATE TEMP TABLE t0165 (
             id bigserial PRIMARY KEY,
             name text NOT NULL,
             type text NOT NULL,
             config_jsonb jsonb NOT NULL DEFAULT '{}'
         )",
    )
    .execute(&mut *conn)
    .await
    .expect("create fixture");
    // Every config shape the two predicates have to agree about.
    sqlx::query(
        "INSERT INTO t0165 (name, type, config_jsonb) VALUES
           ('sr_dead',  'starrocks', '{\"tagService\":\"dead\",\"jdbc.url\":\"keep\"}'),
           ('tr_live',  'trino',     '{\"tagService\":\"live\"}'),
           ('sr_plain', 'starrocks', '{}'),
           ('sr_null',  'starrocks', '{\"tagService\":null}'),
           ('sr_num',   'starrocks', '5'),
           ('sr_arr',   'starrocks', '[1,2]'),
           ('sr_str',   'starrocks', '\"x\"'),
           ('tag_num',  'tag',       '{\"tagService\":7}')",
    )
    .execute(&mut *conn)
    .await
    .expect("seed fixture");

    // The migration itself. `ADD CONSTRAINT` without NOT VALID validates every
    // existing row, so this failing IS the assertion that the clear was complete.
    // `AssertSqlSafe` because the SQL is the committed migration's own text with one
    // identifier substituted (`sqlx::query` takes `&'static str` since sqlx 0.9).
    for stmt in fixture.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(sqlx::AssertSqlSafe(stmt.to_owned()))
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|e| panic!("migration 0165 statement failed: {e}\n{stmt}"));
    }

    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT name, config_jsonb FROM t0165 ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .expect("read back");
    let got: std::collections::HashMap<String, Value> = rows.into_iter().collect();

    // The dead key is gone and NOTHING ELSE in the config is.
    assert_eq!(
        got["sr_dead"],
        json!({ "jdbc.url": "keep" }),
        "the clear must delete only `tagService`"
    );
    // A non-string value is cleared too — the UPDATE is keyed on the same `->>` the
    // CHECK uses, so it cannot leave a row the CHECK then rejects.
    assert_eq!(
        got["tag_num"],
        json!({}),
        "a numeric tagService must be cleared"
    );
    // Trino keeps its tag service; this is the shape `bind_trino_profile` writes.
    assert_eq!(
        got["tr_live"],
        json!({ "tagService": "live" }),
        "a trino profile's tagService must survive"
    );
    // Untouched rows stay byte-identical, including the scalar and array configs
    // that `jsonb - text` would ERROR on if the WHERE clause ever let them through.
    assert_eq!(got["sr_plain"], json!({}));
    assert_eq!(got["sr_null"], json!({ "tagService": null }));
    assert_eq!(got["sr_num"], json!(5));
    assert_eq!(got["sr_arr"], json!([1, 2]));
    assert_eq!(got["sr_str"], json!("x"));

    // And the constraint is now in force on the fixture, in both directions.
    for (sql, what) in [
        (
            "UPDATE t0165 SET type = 'starrocks' WHERE name = 'tr_live'",
            "type flip",
        ),
        (
            "UPDATE t0165 SET config_jsonb = '{\"tagService\":\"late\"}' WHERE name = 'sr_plain'",
            "late tagService",
        ),
    ] {
        let e = sqlx::query(sql)
            .execute(&mut *conn)
            .await
            .unwrap_err_or_panic(what);
        assert_constraint_violation(&e, what);
    }
}

/// `Result::unwrap_err` needs `T: Debug`; this names the case in the panic instead.
trait UnwrapErrOrPanic<E> {
    fn unwrap_err_or_panic(self, what: &str) -> E;
}
impl<T, E> UnwrapErrOrPanic<E> for Result<T, E> {
    fn unwrap_err_or_panic(self, what: &str) -> E {
        match self {
            Ok(_) => panic!("{what}: migration 0165's CHECK must have refused this"),
            Err(e) => e,
        }
    }
}

/// **SP4d debt Task 9 fix round — the two entry points onto ONE join must agree
/// for every shape of the field.** `usable_tag_service` (single id, used by
/// `PUT /engines/{id}` to refuse a bind) and `usable_tag_services` (bulk, used
/// by `list_profiles` to decide whether to offer a Tags tab) are now the same
/// `usable_tag_services_via` with a wider `WHERE`. This asserts them together,
/// shape by shape, because the whole point of collapsing them was that a second
/// spelling would be free to disagree — and three readers disagreeing about this
/// one field is what Task 9 was.
///
/// The shapes are driven in with raw `UPDATE`s on purpose: `create_service` now
/// refuses the unusable values (that is `refuse_unnameable_tag_service`), and no
/// `UPDATE` of `config_jsonb` exists anywhere in `src/` — so raw SQL is the only
/// way to reach rows a `psql` writer could still leave behind, which is exactly
/// the population these two readers must agree about.
#[tokio::test]
async fn both_entry_points_onto_the_tag_service_join_agree() {
    let Some((s, db)) = store_and_db().await else {
        eprintln!("skip");
        return;
    };
    seed_trino(&s).await.expect("seed trino");
    seed_tag(&s).await.expect("seed tag def");

    let tag_name = uniq("t9join_tag");
    let tag_svc = s
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .expect("create tag service");
    let profile = uniq("t9join_trino");
    let svc = s
        .create_service(&profile, "trino", &json!({ "tagService": tag_name }))
        .await
        .expect("create linked profile");

    // A conventionally-named row beside it, so "resolves the link" cannot be
    // confused with "guesses `<name>_tag`" — the two differ for every shape.
    let convention = s
        .create_service(&format!("{profile}_tag"), "tag", &json!({}))
        .await
        .expect("create the conventionally-named row");

    let set = |cfg: Value| {
        let pool = db.pool.clone();
        async move {
            sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
                .bind(svc.id)
                .bind(cfg)
                .execute(&pool)
                .await
                .expect("set config");
        }
    };

    for (label, cfg, expected) in [
        (
            "the configured, resolvable link",
            json!({ "tagService": tag_name }),
            Some(tag_name.clone()),
        ),
        ("no key at all", json!({}), None),
        ("JSON null", json!({ "tagService": null }), None),
        ("empty string", json!({ "tagService": "" }), None),
        ("whitespace only", json!({ "tagService": "   " }), None),
        ("a number", json!({ "tagService": 7 }), None),
        (
            "dangling — names no row",
            json!({ "tagService": "t9join_nope" }),
            None,
        ),
    ] {
        set(cfg).await;
        let single = s.usable_tag_service(svc.id).await.expect("single");
        assert_eq!(
            single, expected,
            "{label}: `usable_tag_service` must answer {expected:?}"
        );
        // The bulk form, asked for this id among others, must say the same —
        // and ABSENT from the map is how it says `None`.
        let bulk = s
            .usable_tag_services(&[svc.id, tag_svc.id, convention.id])
            .await
            .expect("bulk");
        assert_eq!(
            bulk.get(&svc.id).cloned(),
            expected,
            "{label}: the bulk form must agree with the single form"
        );
        // Neither form invents the `<name>_tag` convention for any shape.
        assert_ne!(
            single,
            Some(convention.name.clone()),
            "{label}: the convention fallback is GONE — see `list_profiles`"
        );
    }

    s.delete_service(convention.id).await.expect("cleanup");
    s.delete_service(svc.id).await.expect("cleanup");
    s.delete_service(tag_svc.id).await.expect("cleanup");
}
