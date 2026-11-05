//! Governance SP2b Task 6: the OPA snapshot is built from the same lowering
//! the Ranger compile uses (`lower_ep_via`), plus the infrastructure
//! statements and the classified columns of the profile's catalogs.
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   cargo test -p egret-perm-compile --features harness --test opa_snapshot
include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;

use egret_metastore::Db;
use egret_perm::{Action, Effect, Selector};
use egret_perm_eval::{Decision, Resource, Snapshot};
use fixtures::*;
use serde_json::json;
use uuid::Uuid;

static SEEDED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    SEEDED
        .get_or_init(|| async {
            db.migrate().await.expect("migrate");
            seed_service_defs(&db).await;
        })
        .await;
    db
}

fn table(catalog: &str, table: &str, columns: &[&str]) -> Resource {
    Resource::Table {
        catalog: catalog.to_string(),
        schema: "s".to_string(),
        table: table.to_string(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
    }
}

fn is_allow(d: &Decision) -> bool {
    matches!(d, Decision::Allow { .. })
}

fn is_explicit_deny(d: &Decision) -> bool {
    matches!(d, Decision::Deny { explicit: true, .. })
}

async fn catalog_name(tx: &mut Tx<'_>, id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(id)
        .fetch_one(&mut **tx)
        .await
        .expect("catalog name")
}

async fn count_unnamed(tx: &mut Tx<'_>, ids: &[Uuid]) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE id = ANY($1) AND username IS NULL")
        .bind(ids)
        .fetch_one(&mut **tx)
        .await
        .expect("count")
}

async fn build(tx: &mut Tx<'_>, profile: i64) -> Snapshot {
    egret_perm_compile::harness_support::build_snapshot_for_test(tx, profile)
        .await
        .expect("the snapshot builds")
        .expect("a trino profile")
}

/// The store the first two tests share, under `prefix`: a Trino profile with
/// a tag service, catalog `lake` on an `open` engine bound to it, `ann`
/// granted Read on the catalog, a path `MASK_HASH` on `s.t.email` exempting
/// `bob`, an engine grant letting `ann` set `query_max_memory`, and
/// classification `pii` on `s.t.ssn` cleared for `bob`'s role.
struct Store {
    profile: i64,
    warehouse: Uuid,
    pii: Uuid,
    lake: String,
    ann: String,
    bob: String,
}

async fn store(db: &Db, prefix: &str, names: &mut Names, tx: &mut Tx<'_>) -> Store {
    let (profile, tag_service, _) = fixtures::profile(db, prefix, "trino", true, names).await;
    assert!(tag_service.is_some(), "the profile has a tag service");
    let wh = warehouse(tx, prefix, names).await;
    let engine = engine_bound_spec(
        tx,
        &format!("{prefix}.engine"),
        profile,
        "trino",
        json!({"trinoConfig": {"delegationMode": "open"}}),
        names,
    )
    .await;
    let cat = catalog(tx, &format!("{prefix}.lake"), wh, &[engine], names).await;
    let lake = catalog_name(tx, cat).await;
    let ann = format!("{prefix}_ann");
    let bob = format!("{prefix}_bob");
    user(tx, &format!("{prefix}.ann"), Some(&ann), names).await;
    let bob_id = user(tx, &format!("{prefix}.bob"), Some(&bob), names).await;
    grant(
        tx,
        &format!("{prefix}.read"),
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[user_ref(&ann)], &[]),
        names,
    )
    .await;
    rule(
        tx,
        &format!("{prefix}.mask"),
        mask("MASK_HASH", None),
        Selector::Path(scope(cat, Some("s"), Some("t"), Some("email"))),
        set(&[user_ref(&bob)], &[]),
        names,
    )
    .await;
    engine_grant(
        tx,
        &format!("{prefix}.session"),
        engine,
        "trino.set_system_session_property",
        &["query_max_memory"],
        None,
        &[user_ref(&ann)],
        names,
    )
    .await;
    let pii = tag(tx, &format!("{prefix}_pii"), names).await;
    tag_column(tx, pii, wh, "s", "t", "ssn").await;
    let cleared = format!("{prefix}_cleared_role");
    platform_role(tx, &cleared, names).await;
    role_held(tx, bob_id, &cleared).await;
    gate(tx, pii, &[cleared.as_str()], &[]).await;
    Store {
        profile,
        warehouse: wh,
        pii,
        lake,
        ann,
        bob,
    }
}

#[tokio::test]
async fn the_snapshot_answers_what_the_store_says() {
    // Arrange
    let db = db().await;
    let mut names = Names::default();
    let mut tx = db.pool.begin().await.expect("tx");
    let st = store(&db, "opa1", &mut names, &mut tx).await;
    let key: (i64, i64) = sqlx::query_as(
        "SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(st.profile)
    .fetch_one(&mut *tx)
    .await
    .expect("key");

    // Act
    let snap = build(&mut tx, st.profile).await;

    // Assert
    let op = "SelectFromColumns";
    let id = snap.decide(&st.ann, op, &table(&st.lake, "t", &["id"]));
    assert!(is_allow(&id), "ann reads t.id: {id:?}");
    let ssn = snap.decide(&st.ann, op, &table(&st.lake, "t", &["ssn"]));
    assert!(is_explicit_deny(&ssn), "pii is withheld from ann: {ssn:?}");
    let bob = snap.decide(&st.bob, op, &table(&st.lake, "t", &["id"]));
    assert!(
        matches!(
            bob,
            Decision::Deny {
                explicit: false,
                ..
            }
        ),
        "bob holds no grant: {bob:?}"
    );
    let cols = [("email".to_string(), "varchar".to_string())];
    let ann_masks = snap
        .column_masks(&st.ann, &st.lake, "s", "t", &cols)
        .expect("masks");
    assert!(
        ann_masks.contains_key("email"),
        "ann is masked: {ann_masks:?}"
    );
    let bob_masks = snap
        .column_masks(&st.bob, &st.lake, "s", "t", &cols)
        .expect("masks");
    assert!(bob_masks.is_empty(), "bob is exempt: {bob_masks:?}");
    let prop = Resource::SystemProperty("query_max_memory".to_string());
    let d = snap.decide(&st.ann, "SetSystemSessionProperty", &prop);
    assert!(is_allow(&d), "ann's engine grant sets the property: {d:?}");
    let d = snap.decide(&st.bob, "SetSystemSessionProperty", &prop);
    assert!(!is_allow(&d), "bob holds no engine grant: {d:?}");
    assert_eq!(snap.service_id(), st.profile);
    assert_eq!(snap.key(), key);
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn a_table_level_classification_reaches_every_column_of_the_table() {
    // Arrange
    let db = db().await;
    let mut names = Names::default();
    let mut tx = db.pool.begin().await.expect("tx");
    let st = store(&db, "opa2", &mut names, &mut tx).await;
    tag_table(&mut tx, st.pii, st.warehouse, "s", "u").await;
    // A catalog no engine of the profile reaches: its classifications are not
    // the profile's (`catalogs_for_service_via`, C3).
    let elsewhere = warehouse(&mut tx, "opa2x", &mut names).await;
    catalog(&mut tx, "opa2x.other", elsewhere, &[], &mut names).await;
    tag_table(&mut tx, st.pii, elsewhere, "s", "elsewhere").await;

    // Act
    let snap = build(&mut tx, st.profile).await;
    let classified =
        egret_perm_compile::opa::build::load_classified_columns_via(&mut tx, st.profile)
            .await
            .expect("classified columns");

    // Assert
    let d = snap.decide(
        &st.ann,
        "SelectFromColumns",
        &table(&st.lake, "u", &["untagged"]),
    );
    assert!(
        is_explicit_deny(&d),
        "the table's pii reaches u.untagged: {d:?}"
    );
    let other = snap.decide(&st.ann, "SelectFromColumns", &table(&st.lake, "t", &["id"]));
    assert!(
        is_allow(&other),
        "an untagged table stays readable: {other:?}"
    );
    let pii = "opa2_pii".to_string();
    let whole: Vec<_> = classified
        .iter()
        .filter(|c| c.table == "u")
        .cloned()
        .collect();
    assert_eq!(
        whole,
        vec![egret_perm_eval::ClassifiedColumn {
            catalog: st.lake.clone(),
            schema: "s".into(),
            table: "u".into(),
            column: None,
            classification: pii.clone(),
        }]
    );
    assert!(classified
        .iter()
        .any(|c| c.table == "t" && c.column.as_deref() == Some("ssn") && c.classification == pii));
    assert!(
        classified.iter().all(|c| c.catalog == st.lake),
        "only the bound catalog's classifications: {classified:?}"
    );
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn a_qualifying_engine_puts_the_infra_statements_in_the_snapshot() {
    // Arrange
    let db = db().await;
    let mut names = Names::default();
    let (profile, _, _) = fixtures::profile(&db, "opa3", "trino", true, &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, "opa3", &mut names).await;
    let engine = engine_bound_spec(
        &mut tx,
        "opa3.engine",
        profile,
        "trino",
        json!({"trinoConfig": {"coordinator": {}, "delegationMode": "impersonate",
                               "auth": {"password": {}}}}),
        &mut names,
    )
    .await;
    catalog(&mut tx, "opa3.lake", wh, &[engine], &mut names).await;
    user(&mut tx, "opa3.ann", Some("opa3_ann"), &mut names).await;
    let root = user(&mut tx, "opa3.root", Some("opa3_root"), &mut names).await;
    role_held(&mut tx, root, "platform_admin").await;

    // Act
    let snap = build(&mut tx, profile).await;

    // Assert
    let ann = "opa3_ann";
    let user = |u: &str| Resource::User(u.to_string());
    let d = snap.decide(ann, "ExecuteQuery", &Resource::None);
    assert!(is_allow(&d), "everyone executes queries: {d:?}");
    let d = snap.decide(ann, "KillQueryOwnedBy", &user(ann));
    assert!(is_allow(&d), "ann kills her own query: {d:?}");
    let d = snap.decide(ann, "KillQueryOwnedBy", &user("opa3_bob"));
    assert!(!is_allow(&d), "ann does not kill bob's query: {d:?}");
    let d = snap.decide(
        egret_core::engine_principals::DELEGATOR_PRINCIPAL,
        "ImpersonateUser",
        &user(ann),
    );
    assert!(is_allow(&d), "the delegator impersonates ann: {d:?}");
    let d = snap.decide("opa3_root", "KillQueryOwnedBy", &user("opa3_bob"));
    assert!(is_allow(&d), "the engine admin holds all-access: {d:?}");
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn the_snapshot_build_never_mints_a_username() {
    // Arrange (committed, in a profile of its own): an unnamed holder at
    // every place a compile mints — a role
    // holder, a workspace member, a group member, an engine admin and the
    // holder of a classification's clearing role.
    let db = db().await;
    let mut names = Names::default();
    let (profile, _, _) = fixtures::profile(&db, "opa4", "trino", true, &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, "opa4", &mut names).await;
    let engine = engine_bound_spec(
        &mut tx,
        "opa4.engine",
        profile,
        "trino",
        json!({"trinoConfig": {"coordinator": {}, "delegationMode": "impersonate",
                               "auth": {"password": {}}}}),
        &mut names,
    )
    .await;
    let cat = catalog(&mut tx, "opa4.lake", wh, &[engine], &mut names).await;
    let lake = catalog_name(&mut tx, cat).await;
    let named = user(&mut tx, "opa4.named", Some("opa4_named"), &mut names).await;
    let by_role = user(&mut tx, "opa4.by_role", None, &mut names).await;
    let by_ws = user(&mut tx, "opa4.by_ws", None, &mut names).await;
    let by_group = user(&mut tx, "opa4.by_group", None, &mut names).await;
    let admin = user(&mut tx, "opa4.admin", None, &mut names).await;
    let by_gate = user(&mut tx, "opa4.by_gate", None, &mut names).await;
    let pii = tag(&mut tx, "opa4_pii", &mut names).await;
    tag_column(&mut tx, pii, wh, "s", "t", "ssn").await;
    platform_role(&mut tx, "opa4_cleared", &mut names).await;
    role_held(&mut tx, by_gate, "opa4_cleared").await;
    gate(&mut tx, pii, &["opa4_cleared"], &[]).await;
    platform_role(&mut tx, "opa4_readers", &mut names).await;
    role_held(&mut tx, named, "opa4_readers").await;
    role_held(&mut tx, by_role, "opa4_readers").await;
    let ws = workspace(&mut tx, "opa4.ws", &mut names).await;
    workspace_member(&mut tx, by_ws, ws).await;
    let grp = group(&mut tx, "opa4_group", &mut names).await;
    group_member(&mut tx, by_group, grp).await;
    role_held(&mut tx, admin, "platform_admin").await;
    grant(
        &mut tx,
        "opa4.read",
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(
            &[
                role_ref("opa4_readers"),
                members_ref(ws),
                group_ref("opa4_group"),
            ],
            &[],
        ),
        &mut names,
    )
    .await;
    // Committed, so the build can run as the refresher runs it: in a
    // `REPEATABLE READ, READ ONLY` transaction, where any write fails.
    tx.commit().await.expect("commit the arranged state");
    let mut tx = db.pool.begin().await.expect("tx");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .expect("read only");
    let unnamed = [by_role, by_ws, by_group, admin, by_gate];
    let before = count_unnamed(&mut tx, &unnamed).await;
    assert_eq!(before, 5);

    // Act
    let snap = build(&mut tx, profile).await;

    // Assert
    let after = count_unnamed(&mut tx, &unnamed).await;
    assert_eq!(after, before, "the snapshot build minted a username");
    let d = snap.decide(
        "opa4_named",
        "SelectFromColumns",
        &table(&lake, "t", &["id"]),
    );
    assert!(is_allow(&d), "the named role holder reads: {d:?}");
    tx.rollback().await.expect("rollback");
}

/// `opa/build.rs`' safety argument for `Omit`, pinned: allow `{public}`, deny
/// a role whose one holder has no username. The read-only build cannot name
/// the holder, so its deny names nobody; what closes that is the recompile,
/// which mints the holder first (premint) and moves the key, and the rebuild
/// at the new key denies them.
#[tokio::test]
async fn an_unnamed_deny_holder_is_denied_once_a_recompile_names_them() {
    // Arrange
    let db = db().await;
    let mut names = Names::default();
    let (profile, _, _) = fixtures::profile(&db, "opa5", "trino", false, &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, "opa5", &mut names).await;
    let engine = engine_bound_spec(
        &mut tx,
        "opa5.engine",
        profile,
        "trino",
        json!({"trinoConfig": {"delegationMode": "open"}}),
        &mut names,
    )
    .await;
    let cat = catalog(&mut tx, "opa5.lake", wh, &[engine], &mut names).await;
    let lake = catalog_name(&mut tx, cat).await;
    let x = user(&mut tx, "opa5.x", None, &mut names).await;
    platform_role(&mut tx, "opa5_denied", &mut names).await;
    role_held(&mut tx, x, "opa5_denied").await;
    grant(
        &mut tx,
        "opa5.allow",
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[group_ref(egret_perm::PUBLIC_GROUP)], &[]),
        &mut names,
    )
    .await;
    grant(
        &mut tx,
        "opa5.deny",
        Effect::Deny,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[role_ref("opa5_denied")], &[]),
        &mut names,
    )
    .await;
    let read = table(&lake, "t", &["id"]);

    // Act 1: the read-only build, before any recompile has named x.
    let before = build(&mut tx, profile).await;
    assert_eq!(count_unnamed(&mut tx, &[x]).await, 1, "Omit minted x");

    // Act 2: a recompile mints x's username first, then compiles.
    egret_perm_compile::recompile_service_in_tx(&mut tx, profile)
        .await
        .expect("recompile");
    let minted: String = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(x)
        .fetch_one(&mut *tx)
        .await
        .expect("the recompile minted x a username");
    let after = build(&mut tx, profile).await;

    // Assert: the snapshot built while x was unnamed left x out of the deny
    // (it allows the name x got later), the recompile moved the key, and the
    // snapshot at the new key denies x.
    let d = before.decide(&minted, "SelectFromColumns", &read);
    assert!(
        is_allow(&d),
        "the unnamed holder was left out of the deny: {d:?}"
    );
    assert_ne!(before.key(), after.key(), "the recompile moved the key");
    let d = after.decide(&minted, "SelectFromColumns", &read);
    assert!(is_explicit_deny(&d), "named, x is denied: {d:?}");
    let d = after.decide("opa5_somebody", "SelectFromColumns", &read);
    assert!(is_allow(&d), "public still reads: {d:?}");
    tx.rollback().await.expect("rollback");
}

/// SP2b Task 18: every snapshot the profile builds names the reserved
/// lineage principal (`egret_core::engine_principals::LINEAGE_PRINCIPAL`),
/// so the evaluator refuses it what the lineage transform denies on Ranger,
/// even where a `public` grant allows everyone else.
#[tokio::test]
async fn the_snapshot_names_the_lineage_principal() {
    // Arrange
    let db = db().await;
    let mut names = Names::default();
    let mut tx = db.pool.begin().await.expect("tx");
    let st = store(&db, "opa9", &mut names, &mut tx).await;
    let cat: Uuid = sqlx::query_scalar("SELECT id FROM registry.catalogs WHERE name = $1")
        .bind(&st.lake)
        .fetch_one(&mut *tx)
        .await
        .expect("catalog id");
    grant(
        &mut tx,
        "opa9.public_write",
        Effect::Allow,
        &[Action::Write],
        scope(cat, None, None, None),
        set(&[group_ref("public")], &[]),
        &mut names,
    )
    .await;

    // Act
    let snap = build(&mut tx, st.profile).await;

    // Assert
    let lineage = egret_core::engine_principals::LINEAGE_PRINCIPAL;
    assert_eq!(snap.lineage_principal(), Some(lineage));
    let t = table(&st.lake, "t", &[]);
    assert!(is_allow(&snap.decide(&st.ann, "TruncateTable", &t)));
    let d = snap.decide(lineage, "TruncateTable", &t);
    assert!(is_explicit_deny(&d), "{d:?}");
    tx.rollback().await.expect("rollback");
}
