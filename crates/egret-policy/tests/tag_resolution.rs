//! DB-gated tests for the enricher-equivalent tag resolution (SP4d-2b Task 1).
//! Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! What is under test is `egret_policy::resolved_tags_for_resource_via`: given a
//! resource, produce the tag set the way `RangerTagEnricher.findMatchingTags`
//! would — one entry per matched attachment, carrying the tag's **name** (what
//! the wire and therefore the engine matches on) and the `MatchType` that
//! attachment produces against the request.
//!
//! # Why these fixtures are invented rather than drawn from live data
//!
//! All six live `catalog_tag_assignments` rows are **column-level**, so live data
//! exercises only the `SELF` arm. The `ANCESTOR` arm — a tag attached at table
//! level (`column_name = ''`) and queried at column level — is representable in
//! the schema and exercised by nothing anywhere in the repo, and it is the arm
//! this package's correctness turns on. So is the both-levels case that the
//! no-dedupe rule exists for. Both are built here.
//!
//! # The schema bounds what can be tested
//!
//! `public.catalog_tag_assignments` (migration 0081) is
//! `(tag_id, warehouse_id, schema_name, table_name, column_name)` with
//! `column_name text NOT NULL DEFAULT ''` and the documented convention that
//! `''` means table-level. `schema_name` and `table_name` are `NOT NULL` with
//! **no such convention**, so a schema- or catalog-level attachment is not
//! representable — which is why this resolution handles two levels, not the
//! enricher's four. Checked against the migration, not assumed.
use egret_metastore::Db;
use egret_policy::resolved_tags_for_resource_via;
use egret_ranger_eval::resource::MatchType;
use uuid::Uuid;

async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// A workspace + storage connection + warehouse + registry catalog for that
/// warehouse. Returns `(warehouse_id, catalog_id)` — the resolution is asked by
/// catalog id and joins through to the warehouse the assignments are keyed by.
async fn seed_catalog(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("tagresws-{}", Uuid::new_v4().simple()))
            .bind("Tag Resolution WS")
            .fetch_one(pool)
            .await
            .expect("insert workspace");
    let (sc_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("tagressc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-tagres-test")
    .bind("AKIATEST")
    .fetch_one(pool)
    .await
    .expect("insert storage connection");
    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("tagreswh_{}", Uuid::new_v4().simple()))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("insert warehouse");
    let (catalog_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(format!("tagrescat_{}", Uuid::new_v4().simple()))
    .bind(wh_id)
    .fetch_one(pool)
    .await
    .expect("insert registry catalog");
    (wh_id, catalog_id)
}

/// A tag whose `key` and `name` are the caller's to choose — the two columns are
/// different values and live they differ for one of three tags.
async fn seed_tag_row(pool: &sqlx::PgPool, key: &str, name: &str) -> Uuid {
    let (tag_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(key)
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("insert tag");
    tag_id
}

/// Attach `tag_id` to `schema.table.column` on `warehouse_id`. `column = ""` is
/// the table-level attachment.
async fn assign(
    pool: &sqlx::PgPool,
    tag_id: Uuid,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
    column: &str,
) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(tag_id)
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(pool)
    .await
    .expect("insert assignment");
}

/// Test 1 — the only arm live data exercises: the tag is on exactly the column
/// the request names, which is `SELF`.
#[tokio::test]
async fn column_attachment_against_a_column_request_is_self() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    let tag = seed_tag_row(
        &pool,
        &format!("pii-{}", Uuid::new_v4().simple()),
        "PIIname",
    )
    .await;
    assign(&pool, tag, wh_id, "sales", "orders", "email").await;

    let mut conn = pool.acquire().await.expect("acquire");
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "email")
        .await
        .expect("resolve");

    assert_eq!(got.len(), 1, "one attachment, one entry: {got:?}");
    assert_eq!(got[0].name, "PIIname");
    assert_eq!(got[0].match_type, MatchType::Self_);
}

/// **Test 1b — the fold, which fix round 1 of SP4d-2b Task 7 added.** The three
/// levels are compared case-INSENSITIVELY, because the matcher is: all four of
/// `trino.json`'s `catalog`/`schema`/`table`/`column` resources declare
/// `{"wildCard": true, "ignoreCase": true}`, and `ignoreCase` makes
/// `RangerDefaultResourceMatcher` compare with `equalsIgnoreCase`.
///
/// Measured on the real jar by
/// `tests/ranger/src/test/java/EgretTagAttachmentMatchTypeTest.java`: a row stored
/// `('SALES','ORDERS','EMAIL')` is `SELF` for the request `hive.sales.orders.email`
/// and `('SALES','ORDERS','')` is `ANCESTOR` — both ADMITTED. Before the fold this
/// query returned neither, so egret reported "no tags" for a resource the engine is
/// masking, and an empty set is the licence to skip the tag phase entirely.
///
/// **And this is what ORDINARY use produces**, not malformed data: `ScopePicker`
/// renders the three levels as free-text inputs and `request_resource` inserts them
/// verbatim, so a steward typing `SALES` reached it with nothing wrong anywhere.
/// That is why case is REPRODUCED here rather than refused by
/// `egret_perm_compile::routes::stored_value_problem`, which is where the two
/// classes the fold cannot reproduce — a wildcard metacharacter and a non-ASCII
/// value — decline instead.
///
/// The two entries also prove the fold did not collapse the LEVELS: one tag at two
/// case-shifted levels is still `SELF` + `ANCESTOR`, two members of Ranger's set.
#[tokio::test]
async fn the_three_levels_are_compared_case_insensitively() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    let tag = seed_tag_row(
        &pool,
        &format!("pii-{}", Uuid::new_v4().simple()),
        "FoldedName",
    )
    .await;
    assign(&pool, tag, wh_id, "SALES", "ORDERS", "EMAIL").await;
    assign(&pool, tag, wh_id, "SALES", "ORDERS", "").await;

    let mut conn = pool.acquire().await.expect("acquire");
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "email")
        .await
        .expect("resolve");

    assert_eq!(
        got.len(),
        2,
        "both attachments resolve despite the case difference: {got:?}"
    );
    assert_eq!(got[0].name, "FoldedName");
    assert_eq!(
        got[0].match_type,
        MatchType::Self_,
        "the COLUMN attachment is SELF — `is_self` is computed by the same \
         case-folded comparison the predicate uses, never by a second one in Rust"
    );
    assert_eq!(
        got[1].match_type,
        MatchType::Ancestor,
        "and the TABLE attachment is still ANCESTOR: {got:?}"
    );
}

/// Test 2 — the arm nothing else in the repo exercises. The tag is on the TABLE
/// (`column_name = ''`) and the request names a column: `RangerTagEnricher`
/// builds the matcher with the tag's resource in the POLICY position, so the
/// tag names fewer levels than the request and the match is `ANCESTOR`, which
/// the enricher admits.
#[tokio::test]
async fn table_attachment_against_a_column_request_is_ancestor() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    let tag = seed_tag_row(
        &pool,
        &format!("tablevel-{}", Uuid::new_v4().simple()),
        "TableLevelName",
    )
    .await;
    // `column_name = ''` — migration 0081's documented table-level convention.
    assign(&pool, tag, wh_id, "sales", "orders", "").await;

    let mut conn = pool.acquire().await.expect("acquire");
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "email")
        .await
        .expect("resolve");

    assert_eq!(
        got.len(),
        1,
        "the table-level attachment must be admitted for a column request: {got:?}"
    );
    assert_eq!(got[0].name, "TableLevelName");
    assert_eq!(
        got[0].match_type,
        MatchType::Ancestor,
        "a tag naming fewer levels than the request is ANCESTOR"
    );
}

/// Test 3 — the case the no-dedupe rule exists for. `RangerTagForEval`'s
/// `equals`/`hashCode` include `matchType`, so ONE tag attached at two levels is
/// TWO entries in Ranger's set, and the MASK evaluators are ordered by
/// `MATCH_TYPE_COMPARATOR` — so collapsing them by name would silently change
/// the masking answer.
#[tokio::test]
async fn one_tag_at_both_levels_comes_back_twice() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    let tag = seed_tag_row(
        &pool,
        &format!("bothlevels-{}", Uuid::new_v4().simple()),
        "BothLevelsName",
    )
    .await;
    assign(&pool, tag, wh_id, "sales", "orders", "").await;
    assign(&pool, tag, wh_id, "sales", "orders", "email").await;

    let mut conn = pool.acquire().await.expect("acquire");
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "email")
        .await
        .expect("resolve");

    assert_eq!(
        got.len(),
        2,
        "the same tag at two levels is TWO entries, not one: {got:?}"
    );
    assert!(
        got.iter().all(|t| t.name == "BothLevelsName"),
        "both entries name the same tag: {got:?}"
    );
    let types: Vec<MatchType> = got.iter().map(|t| t.match_type).collect();
    assert!(
        types.contains(&MatchType::Self_) && types.contains(&MatchType::Ancestor),
        "one SELF and one ANCESTOR: {types:?}"
    );
    // The documented order: match type ascending, i.e. `MATCH_TYPE_COMPARATOR`'s
    // direction. The evaluator orders for itself; this pins that the answer is
    // stable rather than whatever the planner felt like returning.
    assert_eq!(
        types,
        vec![MatchType::Self_, MatchType::Ancestor],
        "documented order is match type ascending"
    );
}

/// Test 4 — the exclusion. A COLUMN attachment against a TABLE-level request is
/// `DESCENDANT` (the tag names more levels than the request), which the
/// enricher does not admit. Returning it would fail CLOSED on `explain` and
/// still disagree with the engine.
#[tokio::test]
async fn column_attachment_against_a_table_request_is_excluded() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    let tag = seed_tag_row(
        &pool,
        &format!("deeper-{}", Uuid::new_v4().simple()),
        "DeeperName",
    )
    .await;
    assign(&pool, tag, wh_id, "sales", "orders", "email").await;

    let mut conn = pool.acquire().await.expect("acquire");
    // `column = ""` in the REQUEST position: the request names the table.
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "")
        .await
        .expect("resolve");

    assert!(
        got.is_empty(),
        "a DESCENDANT attachment is not admitted by the enricher: {got:?}"
    );
}

/// Test 5 — the identifier. The engine matches tag policies on the wire tag's
/// `type`, which `ServiceTags::from_assignments` fills from `tags.name`;
/// `tag_keys_for_resource_via` returns `tags.key`. The fixture makes the two
/// columns DIFFER, because with a tag whose key and name coincide this test
/// could not tell them apart.
#[tokio::test]
async fn the_returned_identifier_is_the_name_not_the_key() {
    let Some(db) = db().await else {
        eprintln!("skip");
        return;
    };
    let pool = db.pool.clone();
    let (wh_id, catalog_id) = seed_catalog(&pool).await;
    // Shaped on the live pair that actually differs: key `sp4cgate`, name
    // `sp4cgate2`.
    let key = format!("sp4cgate-{}", Uuid::new_v4().simple());
    let name = format!("{key}-NAME-DIFFERS");
    let tag = seed_tag_row(&pool, &key, &name).await;
    assign(&pool, tag, wh_id, "sales", "orders", "email").await;

    let mut conn = pool.acquire().await.expect("acquire");
    let got = resolved_tags_for_resource_via(&mut conn, catalog_id, "sales", "orders", "email")
        .await
        .expect("resolve");

    assert_eq!(got.len(), 1, "one attachment: {got:?}");
    assert_eq!(
        got[0].name, name,
        "the resolution must carry tags.name — the engine matches on it"
    );
    assert_ne!(
        got[0].name, key,
        "tags.key is the WRONG identifier here and the fixture makes them differ"
    );
}
