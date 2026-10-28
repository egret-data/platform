//! **SP4d debt Task 5, the matrix: a tag-addressed MASK must not coexist with a
//! TABLE-LEVEL attachment of the same tag — and either half can be created
//! second.**
//!
//! The measurement this guards against is live, on Trino 470 (SP4d-2b's live
//! gate): with `tblmask` attached to the whole table `sp4d1.t7` and a `MASK`
//! addressed to `tblmask`, the engine type-checks that one mask against EVERY
//! column of the scanned table, so `SELECT item`, `SELECT id` and even
//! `count(*)` all fail. The table is unreadable, not masked. The row filter was
//! measured and ruled out as the cause in the same run. See
//! `egret_policy::unreadable_table_mask`'s module doc.
//!
//! # What this binary is for, and what it deliberately is NOT
//!
//! This drives [`egret_policy::unreadable_table_mask_for`] — the single function
//! all three authoring routes call — against a real database, so that the SHAPE
//! of the rule is pinned once: which pairs are refused, and (more important)
//! which neighbouring shapes must still be accepted. The route-level 400s live
//! with their routes, because a route handler's guard is not covered from here:
//!
//!   * `crates/egret-perm-compile/tests/routes.rs` — `POST /perm/rules`
//!     (mask second) and the PATCH that is one of the two remedies.
//!   * `crates/egret-bi/tests/catalog_tags_routes.rs` — `PUT …/resource-tags`
//!     (attachment second), against BOTH mask origins.
//!   * `crates/egret-policy/tests/admin_routes.rs` — `POST /policies` with
//!     `kind: "tag-masking"` (editor mask second).
//!
//! # Every accept-control here is load-bearing
//!
//! The refusals are the easy half. Four shapes sit one character away from the
//! refused one and MUST stay writable, and each of them is a product feature
//! somebody is using: a column-level attachment of a masked tag (the ordinary,
//! working configuration), a tag-addressed ROW FILTER (table-granular on both
//! engines by construction, so a table-level attachment is its natural match),
//! a tag ACCESS policy (what a clearance compiles to — it denies rather than
//! masks and type-checks nothing), and a `policyType = 1` policy on a RESOURCE
//! profile (a path-addressed mask, which has no tag to pair with). A guard that
//! refused any of those would be worse than the defect.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test unreadable_table_mask_db

use egret_metastore::Db;
use egret_policy::{
    seed_tag, seed_trino, unreadable_table_mask_for, MaskOrigin, PolicyStore, TagRef, TagWrite,
    LISTED_OTHER_TABLES,
};
use serde_json::json;
use uuid::Uuid;

async fn db() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// One tag, with a `name` that is a DIFFERENT string from its `key` on purpose:
/// `perm.rules.tag_key` references `tags.key` while a tag POLICY addresses
/// `tags.name` (D6), so a fixture where the two coincide cannot tell a
/// key/name confusion in the guard from a correct answer.
struct Tag {
    id: Uuid,
    key: String,
    name: String,
}

async fn tag(pool: &sqlx::PgPool, what: &str) -> Tag {
    let suffix = Uuid::new_v4().simple().to_string();
    let key = format!("utm-k-{what}-{suffix}");
    let name = format!("UTM-N-{what}-{suffix}");
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(&key)
        .bind(&name)
        .fetch_one(pool)
        .await
        .expect("insert tag");
    Tag { id, key, name }
}

/// Attach `tag` to a resource. `column = ""` is migration 0081's TABLE-level
/// sentinel — the half under test — and anything else is a column attachment.
/// `warehouse_id` carries no foreign key (0081), so a bare uuid is a valid
/// warehouse for the purposes of this table.
async fn attach(pool: &sqlx::PgPool, t: &Tag, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', $3, $4)",
    )
    .bind(t.id)
    .bind(Uuid::new_v4())
    .bind(table)
    .bind(column)
    .execute(pool)
    .await
    .expect("attach tag");
}

/// A `perm.rules` row. `kind` is `mask` or `row_filter`; `selector` is `tag` or
/// `path`.
async fn rule(pool: &sqlx::PgPool, t: &Tag, kind: &str, selector: &str) -> String {
    let name = format!("utm-rule-{}", Uuid::new_v4().simple());
    let (mask_type, filter_expr) = match kind {
        "mask" => (Some("MASK_HASH"), None),
        _ => (None, Some("1=1")),
    };
    // A `path` selector names a catalog and no tag; a `tag` selector is the
    // other way round. `perm.rules`' own CHECKs tie `selector_kind` to exactly
    // one of the two, so this cannot be collapsed into one binding.
    let (tag_key, catalog_id, schema, table): (
        Option<&str>,
        Option<Uuid>,
        Option<&str>,
        Option<&str>,
    ) = match selector {
        "tag" => (Some(&t.key), None, None, None),
        _ => (
            None,
            Some(catalog(pool).await),
            Some("sales"),
            Some("orders"),
        ),
    };
    sqlx::query(
        "INSERT INTO perm.rules \
             (name, kind, mask_type, filter_expr, selector_kind, tag_key, \
              catalog_id, schema_name, table_name) \
         VALUES ($1, $2::perm.rule_kind, $3, $4, $5::perm.selector_kind, $6, $7, $8, $9)",
    )
    .bind(&name)
    .bind(kind)
    .bind(mask_type)
    .bind(filter_expr)
    .bind(selector)
    .bind(tag_key)
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .execute(pool)
    .await
    .expect("insert perm rule");
    name
}

/// A registry catalog, for the path-selector control only. `registry.catalogs`
/// FK-references `catalog.warehouses`, so this seeds the whole workspace →
/// storage connection → warehouse chain; `catalog_tag_assignments.warehouse_id`
/// carries no such FK, which is why [`attach`] can use a bare uuid and this
/// cannot.
async fn catalog(pool: &sqlx::PgPool) -> Uuid {
    let suffix = Uuid::new_v4().simple().to_string();
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("utmws-{suffix}"))
            .bind("UTM Test WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, 'us-east-1', 'egret-utm-test', 'AKIATEST') RETURNING id",
    )
    .bind(format!("utmsc-{suffix}"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let wh_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("utmwh_{suffix}"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(format!("utm_cat_{}", &suffix[..12]))
    .bind(wh_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog")
}

/// A policy under a freshly-minted service of `service_type`, with the given
/// `policy_type` and tag value — the editor half, written the way the editor
/// writes it (`tagify`: `resources.tag.values`, `serviceType: "tag"`).
async fn policy(
    store: &PolicyStore,
    service_type: &str,
    policy_type: i64,
    tag_value: &str,
) -> String {
    let svc = store
        .create_service(
            &format!("utm-{service_type}-{}", Uuid::new_v4().simple()),
            service_type,
            &json!({}),
        )
        .await
        .expect("create service");
    let name = format!("utm-pol-{}", Uuid::new_v4().simple());
    store
        .create_policy(
            svc.id,
            &name,
            &json!({
                "name": name,
                "policyType": policy_type,
                "serviceType": "tag",
                "isEnabled": true,
                "resources": {
                    "tag": { "values": [tag_value], "isExcludes": false, "isRecursive": false }
                },
            }),
        )
        .await
        .expect("create policy");
    name
}

// ── The two orders, for the permission model's mask half ────────────────────

/// **Attachment first, then the mask rule.** The obvious implementation gets
/// this one.
#[tokio::test]
async fn a_tag_mask_rule_is_refused_when_the_tag_is_already_on_a_whole_table() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "a").await;
    attach(&db.pool, &t, "orders", "").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Mask {
            tag: TagRef::Key(&t.key),
            origin: MaskOrigin::Rule,
            name: "mask-pii",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.mask_origin, MaskOrigin::Rule);
    assert_eq!(pair.mask, "mask-pii");
    // The TAG comes back by NAME, never by key: the name is what the engine
    // matches a tag policy on and what the steward picked in the UI, and a guard
    // that echoed the key would print a string the steward never typed.
    assert_eq!(
        pair.tag, t.name,
        "the refusal names the tag by name, not key"
    );
    assert_eq!(
        (pair.schema.as_str(), pair.table.as_str()),
        ("sales", "orders")
    );
}

/// **The mask rule first, then the attachment** — the direction the obvious
/// implementation misses, and the reason this task has two halves at all.
#[tokio::test]
async fn a_table_level_attachment_is_refused_when_a_tag_mask_rule_already_names_the_tag() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "b").await;
    let rule_name = rule(&db.pool, &t, "mask", "tag").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Attachment {
            tag_id: t.id,
            schema: "sales",
            table: "orders",
            column: "",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.mask_origin, MaskOrigin::Rule);
    assert_eq!(pair.mask, rule_name, "the refusal names the RULE to edit");
    assert_eq!(pair.tag, t.name);
    assert_eq!(
        (pair.schema.as_str(), pair.table.as_str()),
        ("sales", "orders")
    );
}

// ── The two orders, for the policy EDITOR's mask half ───────────────────────
//
// This half is not hypothetical and is not the lesser one: when SP4d-2b's live
// gate swept this development host, every live instance of the pair it found
// came from the editor and none from `perm.rules`. The COUNT is not restated —
// the database holding them was dropped and a re-measurement on 2026-10-10
// finds 0 pairs host-wide, so it is no longer re-derivable (the module doc
// carries that). What the test rests on is the direction, not the number.

/// **Attachment first, then a hand-authored tag masking policy.**
#[tokio::test]
async fn an_editor_tag_mask_policy_is_refused_when_the_tag_is_already_on_a_whole_table() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    let t = tag(&db.pool, "c").await;
    attach(&db.pool, &t, "orders", "").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Mask {
            tag: TagRef::Name(&t.name),
            origin: MaskOrigin::Policy,
            name: "mask-pii-policy",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.mask_origin, MaskOrigin::Policy);
    assert_eq!(pair.mask, "mask-pii-policy");
    assert_eq!(
        (pair.schema.as_str(), pair.table.as_str()),
        ("sales", "orders")
    );
}

/// **A hand-authored tag masking policy first, then the attachment.** The
/// attachment side has to see BOTH mask origins, or the editor's half of the
/// pair is only half guarded.
#[tokio::test]
async fn a_table_level_attachment_is_refused_when_an_editor_tag_mask_policy_names_the_tag() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    let t = tag(&db.pool, "d").await;
    let pol = policy(&store, "tag", 1, &t.name).await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Attachment {
            tag_id: t.id,
            schema: "sales",
            table: "orders",
            column: "",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.mask_origin, MaskOrigin::Policy);
    assert_eq!(pair.mask, pol, "the refusal names the POLICY to edit");
}

// ── The accept-controls ─────────────────────────────────────────────────────

/// **A COLUMN-level attachment of a masked tag is the ordinary, working
/// configuration** — it is what every live assignment on this host is, and it
/// is the remedy the refusal names. If this test ever goes red the guard has
/// started refusing the product.
#[tokio::test]
async fn a_column_level_attachment_of_a_masked_tag_is_accepted() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "e").await;
    rule(&db.pool, &t, "mask", "tag").await;
    attach(&db.pool, &t, "orders", "email").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    // Both directions, because the sentinel is what distinguishes them and a
    // guard that keyed on "any attachment" would fail only one of these.
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Mask {
                tag: TagRef::Key(&t.key),
                origin: MaskOrigin::Rule,
                name: "mask-pii",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a mask over a tag attached only to COLUMNS is the working configuration"
    );
    // And from the ATTACHMENT direction, which is the arm that decides what
    // "table-level" means. Both arms, because the sentinel is the only thing
    // separating the refused shape from this one, and an earlier draft spelled
    // that decision in the CALLER as well as in this function — two guards that
    // must agree, written to different shapes. A mutation that drops the
    // `column.is_empty()` early return now dies HERE and nowhere else.
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "email",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "attaching a masked tag to a COLUMN is the remedy the refusal names; it \
         must never be refused"
    );
}

/// **A tag-addressed ROW FILTER is table-granular on both engines by
/// construction**, so a table-level attachment is its natural match and not a
/// hazard. Nothing about it type-checks against a column.
#[tokio::test]
async fn a_tag_row_filter_with_a_table_level_attachment_is_accepted() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "f").await;
    rule(&db.pool, &t, "row_filter", "tag").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a tag ROW FILTER carries no column type check; refusing it would \
         withdraw a working control"
    );
}

/// **A PATH-addressed mask has no tag to pair with.** Its scope names the
/// column it masks, so the engine type-checks it against that column alone —
/// which is the whole point of the remedy the refusal names.
#[tokio::test]
async fn a_path_addressed_mask_with_a_table_level_attachment_is_accepted() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "g").await;
    rule(&db.pool, &t, "mask", "path").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "the remedy must not itself be refused"
    );
}

/// **A tag ACCESS policy is what a clearance compiles to** (`policyType = 0`,
/// `COMPILER_CLEARANCE_PREFIX`): it DENIES rather than masks, and a deny
/// type-checks nothing. A clearance on a table-level tag is the designed
/// behaviour — withhold the whole table from everyone without the clearing role
/// — so refusing it would delete SP4c.
///
/// And a `policyType = 2` tag ROW FILTER policy is the editor twin of the row
/// filter control above.
#[tokio::test]
async fn a_tag_access_or_row_filter_policy_with_a_table_level_attachment_is_accepted() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    for policy_type in [0, 2] {
        let t = tag(&db.pool, "h").await;
        policy(&store, "tag", policy_type, &t.name).await;
        let mut conn = db.pool.acquire().await.expect("acquire");
        assert!(
            unreadable_table_mask_for(
                &mut conn,
                TagWrite::Attachment {
                    tag_id: t.id,
                    schema: "sales",
                    table: "orders",
                    column: "",
                },
            )
            .await
            .expect("query")
            .is_none(),
            "policyType {policy_type} is not a mask; refusing it would withdraw a \
             working control (a clearance, for 0)"
        );
    }
}

/// **A `policyType = 1` policy on a RESOURCE profile is a path-addressed mask**,
/// not a tag one — the plugin matches it by `catalog.schema.table.column` and
/// never consults a tag. Keying the guard on `policyType` alone, without
/// `authz_services.type = 'tag'`, would refuse every ordinary column mask in
/// the installation the moment any tag was attached to a table.
#[tokio::test]
async fn a_masking_policy_on_a_resource_profile_is_accepted() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_trino(&store).await.expect("seed trino def");
    let t = tag(&db.pool, "i").await;
    // Deliberately the SAME body shape a tag policy has, so the only thing
    // separating this from the refused case is the service's type.
    policy(&store, "trino", 1, &t.name).await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a masking policy under a non-tag service addresses a PATH, not this tag"
    );
}

/// **A DIFFERENT tag's table-level attachment is not this tag's problem.** The
/// anchor has to be the tag, or one table-level attachment anywhere in the
/// installation would refuse every tag mask ever authored.
#[tokio::test]
async fn a_table_level_attachment_of_another_tag_is_accepted() {
    let Some(db) = db().await else { return };
    let mine = tag(&db.pool, "j").await;
    let other = tag(&db.pool, "k").await;
    attach(&db.pool, &other, "orders", "").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Mask {
                tag: TagRef::Key(&mine.key),
                origin: MaskOrigin::Rule,
                name: "mask-pii",
            },
        )
        .await
        .expect("query")
        .is_none(),
        "the guard is anchored on the tag being written, not on the table"
    );
}

// ── The RENAME arm: the third surface (SP4d debt Task 15) ───────────────────
//
// A rename writes NEITHER half. It moves `tags.name`, which is the column the
// editor-policy half of the pair is joined on, so it can complete the pair from
// the outside — confirmed end to end through the real routes, down to what the
// Ranger plugin downloads, in `egret-bi/tests/tag_rename_unreadable_pair.rs`
// before this arm existed. The route-level 400 lives with its route
// (`PATCH /api/control-plane/tags/{id}` → `egret_bi::TagRepo::update`), because
// a route handler's guard is not covered from here.

/// **Renaming a table-attached tag INTO a masked name is refused.** The mask
/// was authored against a name nothing carried; the attachment was authored for
/// a tag nothing masked; the rename is what makes them the same tag.
#[tokio::test]
async fn renaming_a_table_attached_tag_into_a_masked_name_is_refused() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    let t = tag(&db.pool, "l").await;
    attach(&db.pool, &t, "orders", "").await;
    // The masked name belongs to NO tag — which is exactly why the editor
    // accepted this policy, and why neither of Task 5's guards ran.
    let masked_name = format!("UTM-N-renamed-{}", Uuid::new_v4().simple());
    let pol = policy(&store, "tag", 1, &masked_name).await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Rename {
            tag_id: t.id,
            new_name: &masked_name,
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.mask_origin, MaskOrigin::Policy);
    assert_eq!(pair.mask, pol, "the refusal names the POLICY to edit");
    assert_eq!(
        pair.tag, masked_name,
        "the refusal names the NEW name — the one the engine would match on, and the one \
         the steward just typed; the old name is not part of the state being refused"
    );
    assert_eq!(
        (pair.schema.as_str(), pair.table.as_str()),
        ("sales", "orders"),
        "and the table that would become unreadable"
    );
}

/// **⚠ Renaming a table-attached tag OUT of a masked name is ACCEPTED, and this
/// test is load-bearing.**
///
/// The fixture here is a pair that ALREADY EXISTS — the state four databases on
/// this development host are in, left by `psql` or by a pre-guard installation.
/// The engine matches a tag policy on the NAME, so moving the name away
/// genuinely removes the pair (Task 5's concern 5, which judged this direction
/// correctly and must not be undone), and it is the only product remedy for
/// that state: a tag policy cannot be edited at all today
/// (`PUT /policies/{id}` has no tag branch), so refusing the rename would leave
/// the steward with nothing to do. If this test ever goes red, a steward
/// somewhere is trapped.
#[tokio::test]
async fn renaming_a_table_attached_tag_out_of_a_masked_name_is_accepted() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    let t = tag(&db.pool, "m").await;
    attach(&db.pool, &t, "orders", "").await;
    policy(&store, "tag", 1, &t.name).await; // the pair, already in the database

    let mut conn = db.pool.acquire().await.expect("acquire");
    // Proof the fixture really holds the state this test guards — a test whose
    // fixture cannot hold it is not a test.
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "",
            },
        )
        .await
        .expect("query")
        .is_some(),
        "the fixture must BE the pair, or this test is measuring nothing"
    );
    let free_name = format!("UTM-N-escaped-{}", Uuid::new_v4().simple());
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Rename {
                tag_id: t.id,
                new_name: &free_name,
            },
        )
        .await
        .expect("query")
        .is_none(),
        "renaming OUT of the masked name is how the steward escapes the pair; refusing it \
         would trap them with no remedy at all"
    );
}

/// The rename accept-controls, each one character away from the refused shape
/// and each a configuration somebody is using.
#[tokio::test]
async fn the_renames_that_cannot_create_the_pair_are_accepted() {
    let Some(db) = db().await else { return };
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.expect("seed tag def");
    let mut conn = db.pool.acquire().await.expect("acquire");

    // 1. The tag is attached to a COLUMN, not to the whole table — the ordinary
    //    working configuration, and the remedy the refusal names.
    let masked = format!("UTM-N-col-{}", Uuid::new_v4().simple());
    let t = tag(&db.pool, "n").await;
    attach(&db.pool, &t, "orders", "email").await;
    policy(&store, "tag", 1, &masked).await;
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Rename {
                tag_id: t.id,
                new_name: &masked,
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a column-attached tag may be renamed into a masked name: the mask then type-checks \
         against that column alone, which is the whole point of the remedy"
    );

    // 2. Nothing masks the new name — the ordinary rename.
    let t = tag(&db.pool, "o").await;
    attach(&db.pool, &t, "orders", "").await;
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Rename {
                tag_id: t.id,
                new_name: &format!("UTM-N-plain-{}", Uuid::new_v4().simple()),
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a rename into a name no mask addresses is just a rename"
    );

    // 3. The new name is addressed by a tag ACCESS policy (`policyType = 0`,
    //    what a clearance compiles to) or a tag ROW FILTER policy
    //    (`policyType = 2`). Neither type-checks against a column, and a
    //    clearance on a table-level tag is SP4c's designed behaviour.
    for policy_type in [0, 2] {
        let t = tag(&db.pool, "p").await;
        attach(&db.pool, &t, "orders", "").await;
        let name = format!("UTM-N-pt{policy_type}-{}", Uuid::new_v4().simple());
        policy(&store, "tag", policy_type, &name).await;
        assert!(
            unreadable_table_mask_for(
                &mut conn,
                TagWrite::Rename {
                    tag_id: t.id,
                    new_name: &name,
                },
            )
            .await
            .expect("query")
            .is_none(),
            "policyType {policy_type} is not a mask; refusing this rename would withdraw a \
             working control (a clearance, for 0)"
        );
    }

    // 4. **The rules-direction pair is name-blind, so a rename cannot create it
    //    and must not be refused on account of it.** `perm.rules.tag_key`
    //    references `tags.key`, which no rename touches: this tag is ALREADY in
    //    the rules-direction pair, the rename leaves that pair exactly as it
    //    was, and refusing it would be refusing a write that changes nothing
    //    about the hazard — with the tag's own steward left no way to rename it.
    let t = tag(&db.pool, "q").await;
    attach(&db.pool, &t, "orders", "").await;
    rule(&db.pool, &t, "mask", "tag").await;
    // The fixture really is the rules-direction pair, measured from the
    // attachment arm rather than assumed — otherwise this would be a green from
    // a state that cannot exercise the behaviour.
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Attachment {
                tag_id: t.id,
                schema: "sales",
                table: "orders",
                column: "",
            },
        )
        .await
        .expect("query")
        .is_some(),
        "the fixture must BE the rules-direction pair, or case 4 measures nothing"
    );
    assert!(
        unreadable_table_mask_for(
            &mut conn,
            TagWrite::Rename {
                tag_id: t.id,
                new_name: &format!("UTM-N-rules-{}", Uuid::new_v4().simple()),
            },
        )
        .await
        .expect("query")
        .is_none(),
        "a rename must not be refused because a `perm.rules` mask selects the tag by KEY — \
         the rename cannot change that pair in either direction, and refusing it would leave \
         the tag unrenameable forever"
    );
}

/// The refusal names the OTHER tables the tag is on, not only the first one
/// (the defect was `LIMIT 1`: `moreTables` said there were more, and listed none).
#[tokio::test]
async fn a_refused_mask_lists_every_other_table_the_tag_is_on() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "listed").await;
    attach(&db.pool, &t, "orders", "").await;
    attach(&db.pool, &t, "returns", "").await;
    attach(&db.pool, &t, "refunds", "").await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Mask {
            tag: TagRef::Key(&t.key),
            origin: MaskOrigin::Rule,
            name: "mask-pii",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(
        (pair.schema.as_str(), pair.table.as_str()),
        ("sales", "orders"),
        "the first table in (schema, table, warehouse) order is named"
    );
    let others: Vec<&str> = pair.other_tables.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(
        others,
        vec!["refunds", "returns"],
        "every other table is listed, in order"
    );
    assert!(!pair.other_tables_truncated);
}

/// A tag on more tables than a refusal lists says so, and lists the first
/// `LISTED_OTHER_TABLES` of them.
#[tokio::test]
async fn a_refused_mask_on_many_tables_says_the_list_is_cut_short() {
    let Some(db) = db().await else { return };
    let t = tag(&db.pool, "many").await;
    for i in 0..(LISTED_OTHER_TABLES + 3) {
        attach(&db.pool, &t, &format!("t{i:03}"), "").await;
    }

    let mut conn = db.pool.acquire().await.expect("acquire");
    let pair = unreadable_table_mask_for(
        &mut conn,
        TagWrite::Mask {
            tag: TagRef::Key(&t.key),
            origin: MaskOrigin::Rule,
            name: "mask-pii",
        },
    )
    .await
    .expect("query")
    .expect("the pair must be refused");

    assert_eq!(pair.other_tables.len(), LISTED_OTHER_TABLES);
    assert!(
        pair.other_tables_truncated,
        "more tables exist than are listed"
    );
}
