//! RegistryStore against a real database. DB-gated; self-skips only under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test store

use egret_catalog_registry::store::{
    apply_status, CatalogFilter, NewAttachment, WarehouseIdentity,
};
use egret_catalog_registry::{CatalogDef, RegistryError, RegistryStore};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

async fn db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn secrets(db: &Db) -> egret_secrets::WorkspaceSecretStore {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto");
    egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    )
}

async fn workspace(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Reg', 'active') RETURNING id",
    )
    .bind(format!("reg-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("workspace")
}

async fn warehouse(db: &Db, name: &str, ws: Uuid) -> Uuid {
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'b', 'k', true) RETURNING id",
    )
    .bind(format!("reg-sc-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("storage");
    sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(ws)
    .bind(sc)
    .fetch_one(&db.pool)
    .await
    .expect("warehouse")
}

async fn engine(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'trino', 'egret-engines', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg-eng-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("engine")
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

#[tokio::test]
async fn catalog_crud_keeps_secret_refs_and_blocks_secret_deletes() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let vault = secrets(&db);
    let pw = vault
        .create(None, &unique("pw"), None, "hunter2")
        .await
        .unwrap();
    let pw2 = vault
        .create(None, &unique("pw"), None, "hunter3")
        .await
        .unwrap();
    let def = |id: Uuid| {
        CatalogDef::parse(
            "jdbc",
            &json!({
                "dialect": "postgresql", "host": "db", "port": 5432, "database": "d",
                "user": "u", "password_secret_id": id, "ssl": { "mode": "require" }
            }),
            &json!(null),
        )
        .unwrap()
    };

    let name = unique("pg");
    let row = store
        .create_catalog(&name, &def(pw), Some("sales db"))
        .await
        .unwrap();
    assert_eq!(row.kind, "jdbc");
    assert_eq!(row.def().unwrap(), def(pw));
    assert!(matches!(
        store.create_catalog(&name, &def(pw), None).await,
        Err(RegistryError::AlreadyExists(_))
    ));
    assert!(matches!(
        store.create_catalog("Bad-Name", &def(pw), None).await,
        Err(RegistryError::Invalid(_))
    ));

    // A referenced secret cannot be deleted.
    assert!(matches!(
        vault.delete(None, pw).await,
        Err(egret_secrets::SecretsError::InUse(1))
    ));
    let listed = store
        .list_catalogs(&CatalogFilter {
            secret_id: Some(pw),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![row.id]
    );

    // Re-pointing the catalog releases the old secret.
    let updated = store
        .update_catalog(row.id, &def(pw2), None)
        .await
        .unwrap()
        .unwrap();
    assert!(updated.updated_at >= row.updated_at);
    vault
        .delete(None, pw)
        .await
        .expect("old secret no longer referenced");
    assert!(matches!(
        vault.delete(None, pw2).await,
        Err(egret_secrets::SecretsError::InUse(1))
    ));

    assert!(store.delete_catalog(row.id).await.unwrap());
    vault
        .delete(None, pw2)
        .await
        .expect("catalog gone, secret free");
}

#[tokio::test]
async fn warehouse_catalogs_are_created_once_with_their_workspace() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let base = unique("Sales-Data");
    let wh1 = warehouse(&db, &base, ws).await;
    let wh2 = warehouse(&db, &base.to_uppercase(), workspace(&db).await).await;

    let (c1, created) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh1,
                name: &base,
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(
        c1.name,
        egret_catalog_registry::normalize_catalog_name(&base)
    );
    assert_eq!(c1.warehouse_id, Some(wh1));
    assert_eq!(
        store
            .list_catalog_workspaces(c1.id)
            .await
            .unwrap()
            .iter()
            .map(|w| w.workspace_id)
            .collect::<Vec<_>>(),
        vec![ws]
    );

    let (again, created) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh1,
                name: &base,
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, c1.id);

    let (c2, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh2,
                name: &base,
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        c2.name,
        format!("{}_2", c1.name),
        "same normalised name gets a suffix"
    );

    // Removing the assignment is not undone by ensuring again.
    store.set_catalog_workspaces(c1.id, &[]).await.unwrap();
    store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh1,
                name: &base,
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    assert!(store
        .list_catalog_workspaces(c1.id)
        .await
        .unwrap()
        .is_empty());
    assert!(!store.warehouse_visible_to_workspace(wh1, ws).await.unwrap());
    store.set_catalog_workspaces(c1.id, &[ws]).await.unwrap();
    assert!(store.warehouse_visible_to_workspace(wh1, ws).await.unwrap());
}

#[tokio::test]
async fn attachments_order_status_and_block_deletes() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let wh = warehouse(&db, &unique("wh"), ws).await;
    let eng = engine(&db).await;
    let (primary, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh,
                name: &unique("wh"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    let tpch = store
        .create_catalog(
            &unique("bench"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();

    store
        .insert_attachment(&NewAttachment {
            engine_id: eng,
            catalog_id: tpch.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: true,
            is_primary: false,
            created_at: None,
            apply_status: apply_status::APPLIED,
        })
        .await
        .unwrap();
    store
        .insert_attachment(&NewAttachment {
            engine_id: eng,
            catalog_id: primary.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: false,
            is_primary: true,
            created_at: None,
            apply_status: apply_status::APPLIED,
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .insert_attachment(&NewAttachment {
                engine_id: eng,
                catalog_id: tpch.id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: true,
                is_primary: false,
                created_at: None,
                apply_status: apply_status::APPLIED,
            })
            .await,
        Err(RegistryError::AlreadyExists(_))
    ));

    let listed = store.list_engine_attachments(eng).await.unwrap();
    assert_eq!(
        listed.iter().map(|a| a.catalog.id).collect::<Vec<_>>(),
        vec![primary.id, tpch.id],
        "primary first"
    );
    assert_eq!(store.attached_catalog_names(eng).await.unwrap().len(), 2);

    store.set_apply_failed(eng, tpch.id, "boom").await.unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!(
        (a.apply_status.as_str(), a.apply_error.as_deref()),
        (apply_status::FAILED, Some("boom"))
    );

    // A successful apply that read catalogs BEFORE this edit must not mark it applied.
    let read_at = store.db_now().await.unwrap();
    store
        .update_catalog(
            tpch.id,
            &CatalogDef::parse("tpch", &json!({ "scale": 2 }), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();
    store.mark_engine_applied(eng, read_at).await.unwrap();
    assert_eq!(
        store
            .get_attachment(eng, tpch.id)
            .await
            .unwrap()
            .unwrap()
            .apply_status,
        apply_status::FAILED
    );
    store
        .mark_engine_applied(eng, store.db_now().await.unwrap())
        .await
        .unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!(a.apply_status, apply_status::APPLIED);
    assert!(a.apply_error.is_none() && a.applied_at.is_some());

    assert_eq!(
        store.list_catalog_attachments(tpch.id).await.unwrap()[0].engine_id,
        eng
    );
    assert!(matches!(
        store.delete_catalog(tpch.id).await,
        Err(RegistryError::Conflict(_))
    ));
    assert!(store.delete_attachment(eng, tpch.id).await.unwrap());
    assert!(store.delete_catalog(tpch.id).await.unwrap());
}

/// Trino numbers its catalog env vars by this order (SP3b plan row 9), so a
/// change here silently renumbers a live cluster's catalogs. SP3c-1's backfill
/// makes `is_primary` row-for-row identical to the old
/// `warehouse_id IS NOT DISTINCT FROM` predicate, and this test is what says so:
/// it builds three attachments whose creation order differs from their name
/// order, with the primary created LAST, and expects primary-first-then-created.
///
/// The fixture discriminates all three plausible orderings — each of
/// `name`-only (`aa, mm, zz`), `created_at`-only (`zz, aa, mm`) and
/// `is_primary DESC, name` (`mm, aa, zz`) differs from the expected
/// `mm, zz, aa`. `created_at` is passed explicitly so the row order cannot
/// depend on how fast three INSERTs run.
#[tokio::test]
async fn attachments_still_come_back_primary_first_then_created_then_name() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    // The warehouse catalog's NAME comes from the warehouse's; `mm` sorts
    // between the two others, so "primary first" cannot be an accident of it.
    let wh = warehouse(&db, &unique("mm"), ws).await;
    let eng = engine(&db).await;
    let (primary, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh,
                name: &unique("mm"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    let zz = store
        .create_catalog(
            &unique("zz"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();
    let aa = store
        .create_catalog(
            &unique("aa"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();

    let at = |secs: i64| {
        Some(
            chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc)
                + chrono::Duration::seconds(secs),
        )
    };
    // Creation order zz, aa, primary — the reverse of neither name order nor
    // the expected render order.
    for (catalog_id, created_at) in [(zz.id, at(0)), (aa.id, at(1)), (primary.id, at(2))] {
        store
            .insert_attachment(&NewAttachment {
                engine_id: eng,
                catalog_id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: false,
                is_primary: catalog_id == primary.id,
                created_at,
                apply_status: apply_status::APPLIED,
            })
            .await
            .unwrap();
    }

    let listed = store.list_engine_attachments(eng).await.unwrap();
    assert_eq!(
        listed.iter().map(|a| a.catalog.id).collect::<Vec<_>>(),
        vec![primary.id, zz.id, aa.id],
        "primary first, then attachment time, then name — Trino's env-var numbering rides on this"
    );
    assert_eq!(
        listed.iter().map(|a| a.is_primary()).collect::<Vec<_>>(),
        vec![true, false, false],
        "exactly the warehouse catalog is primary"
    );
}

/// SP3b D1/migration 0149: an attachment can be written before its DDL runs
/// (`pending`), then `set_apply_applied` marks it converged — the same
/// transition `spawn_starrocks_catalog_wire`'s bootstrap (Task 5) drives.
#[tokio::test]
async fn a_pending_attachment_converges_to_applied() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let _wh = warehouse(&db, &unique("wh"), ws).await;
    let eng = engine(&db).await;
    let tpch = store
        .create_catalog(
            &unique("bench"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();

    store
        .insert_attachment(&NewAttachment {
            engine_id: eng,
            catalog_id: tpch.id,
            catalog_client_id: None,
            credential_secret_id: None,
            minted_client: false,
            applied_now: false,
            is_primary: false,
            created_at: None,
            apply_status: apply_status::PENDING,
        })
        .await
        .unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!(a.apply_status, apply_status::PENDING);
    assert!(a.applied_at.is_none());

    store.set_apply_applied(eng, tpch.id).await.unwrap();
    let a = store.get_attachment(eng, tpch.id).await.unwrap().unwrap();
    assert_eq!(a.apply_status, apply_status::APPLIED);
    assert!(a.applied_at.is_some());
    assert!(a.apply_error.is_none());
}

/// Fix round 1 (review F-12): a vanished attachment — deleted out from under
/// a converging bootstrap, the exact race D2's `engine_still_current`
/// checkpoints exist for — must not report success for a write that changed
/// nothing.
#[tokio::test]
async fn set_apply_applied_on_a_gone_attachment_is_not_found() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let _wh = warehouse(&db, &unique("wh"), ws).await;
    let eng = engine(&db).await;
    let tpch = store
        .create_catalog(
            &unique("bench"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();

    // No attachment was ever inserted for (eng, tpch) — nothing to converge.
    assert!(matches!(
        store.set_apply_applied(eng, tpch.id).await,
        Err(RegistryError::NotFound(_))
    ));
}

/// `WorkspaceSecretStore::used_by` counts
/// `engine_catalog_attachments.credential_secret_id` (T5, promoted by the
/// whole-branch review §2): that subquery is the single mechanism keeping a
/// minted Catalog Client credential — a secret the engine's env Secret holds
/// right now — from being deleted out from under a live attachment. Drop the
/// term and this delete succeeds.
///
/// It also pins the OTHER half of the rule: the refusal is the attachment's
/// alone, so it lifts the moment the attachment is gone, without needing a
/// catalog `secret_refs` row to stand in for it.
#[tokio::test]
async fn an_attachments_credential_secret_cannot_be_deleted() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let vault = secrets(&db);
    let ws = workspace(&db).await;
    let wh = warehouse(&db, &unique("wh"), ws).await;
    let eng = engine(&db).await;
    let (catalog, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh,
                name: &unique("wh"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    // A warehouse catalog references no secret of its own, so the count below
    // can only ever come from the attachment's own term.
    let refs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.catalog_secret_refs WHERE catalog_id = $1",
    )
    .bind(catalog.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(refs, 0, "no catalog-config secret to confuse used_by with");
    let credential = vault
        .create(None, &unique("cred"), None, "client:secret")
        .await
        .unwrap();

    store
        .insert_attachment(&NewAttachment {
            engine_id: eng,
            catalog_id: catalog.id,
            catalog_client_id: Some(Uuid::new_v4()),
            credential_secret_id: Some(credential),
            minted_client: true,
            applied_now: true,
            is_primary: false,
            created_at: None,
            apply_status: apply_status::APPLIED,
        })
        .await
        .unwrap();

    assert!(
        matches!(
            vault.delete(None, credential).await,
            Err(egret_secrets::SecretsError::InUse(1))
        ),
        "a live attachment must block the delete of the credential it names"
    );

    assert!(store.delete_attachment(eng, catalog.id).await.unwrap());
    vault
        .delete(None, credential)
        .await
        .expect("once nothing attaches it, the credential can go");
}

fn sample_def() -> CatalogDef {
    CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap()
}

/// Plan decision #27 ("adopt the orphan"): an unattached, unassigned catalog is
/// replaced; one in use, or a warehouse's, is not.
#[tokio::test]
async fn create_replacing_orphan_replaces_only_an_unused_catalog() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;

    let fresh = unique("fresh");
    let (row, replaced) = store
        .create_catalog_replacing_orphan(&fresh, &sample_def(), None)
        .await
        .unwrap();
    assert!(!replaced, "nothing to replace");
    assert_eq!(row.name, fresh);

    let (again, replaced) = store
        .create_catalog_replacing_orphan(&fresh, &sample_def(), Some("new"))
        .await
        .unwrap();
    assert!(replaced, "the orphan is replaced");
    assert_ne!(again.id, row.id);
    assert!(store.get_catalog(row.id).await.unwrap().is_none());

    store.set_catalog_workspaces(again.id, &[ws]).await.unwrap();
    assert!(matches!(
        store
            .create_catalog_replacing_orphan(&fresh, &sample_def(), None)
            .await,
        Err(RegistryError::AlreadyExists(_))
    ));
    assert!(
        store.get_catalog(again.id).await.unwrap().is_some(),
        "in-use catalog kept"
    );

    let wh = warehouse(&db, &unique("wh"), ws).await;
    let (wh_cat, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: wh,
                name: &unique("wh"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    store.set_catalog_workspaces(wh_cat.id, &[]).await.unwrap();
    assert!(
        matches!(
            store
                .create_catalog_replacing_orphan(&wh_cat.name, &sample_def(), None)
                .await,
            Err(RegistryError::AlreadyExists(_))
        ),
        "a warehouse catalog is never an orphan"
    );
}

/// The adopt-the-orphan replace is atomic against a racing assignment: an
/// assignment insert that holds its foreign-key lock blocks the replace, which
/// then sees the committed assignment (409, catalog kept) — or, if that insert
/// rolled back, replaces the orphan.
#[tokio::test]
async fn create_replacing_orphan_serialises_with_a_racing_assignment() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;

    for commit in [true, false] {
        let name = unique("race");
        let orphan = store
            .create_catalog(&name, &sample_def(), None)
            .await
            .unwrap();

        // An assignment in flight: its FK check holds FOR KEY SHARE on the row.
        let mut racer = db.pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO registry.catalog_workspaces (catalog_id, workspace_id) VALUES ($1, $2)",
        )
        .bind(orphan.id)
        .bind(ws)
        .execute(&mut *racer)
        .await
        .unwrap();

        let replacing = {
            let store = store.clone();
            let name = name.clone();
            tokio::spawn(async move {
                store
                    .create_catalog_replacing_orphan(&name, &sample_def(), None)
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(
            !replacing.is_finished(),
            "the replace waits for the racing assignment"
        );

        if commit {
            racer.commit().await.unwrap();
            let res = replacing.await.unwrap();
            assert!(
                matches!(res, Err(RegistryError::AlreadyExists(_))),
                "the committed assignment wins: {res:?}"
            );
            assert!(
                store.get_catalog(orphan.id).await.unwrap().is_some(),
                "catalog kept"
            );
            assert_eq!(
                store
                    .list_catalog_workspaces(orphan.id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        } else {
            racer.rollback().await.unwrap();
            let (row, replaced) = replacing.await.unwrap().unwrap();
            assert!(replaced);
            assert_ne!(row.id, orphan.id);
            assert!(
                store.get_catalog(orphan.id).await.unwrap().is_none(),
                "orphan replaced"
            );
        }
    }
}

/// The two reads SP3c-2 Task 5 moved off `engines.warehouse_id` (Task 6 has
/// since dropped the column entirely): the warehouse an engine LEADS with
/// (its primary attachment's), and every warehouse it READS (one per attached
/// egret warehouse catalog).
///
/// `other`'s only role now is a second, non-primary attachment to prove
/// `warehouse_ids_by_engine` returns every attached warehouse (primary
/// first), not just the primary's — the column this test used to also set to
/// `other`, to prove reads did NOT fall back to it, is gone, so that half of
/// the proof no longer applies; the attachment-vs-attachment ordering proof
/// still does.
#[tokio::test]
async fn warehouse_reads_come_from_the_attachments_not_the_engine_column() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let ws = workspace(&db).await;
    let led = warehouse(&db, &unique("wh_led"), ws).await;
    let other = warehouse(&db, &unique("wh_other"), ws).await;
    // `led`'s catalog is attached PRIMARY; `other`'s is attached SECONDARY.
    let eng = engine(&db).await;
    let bare = engine(&db).await;

    let (led_catalog, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: led,
                name: &unique("led"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    let (other_catalog, _) = store
        .ensure_warehouse_catalog(
            &WarehouseIdentity {
                id: other,
                name: &unique("other"),
                workspace_id: ws,
            },
            None,
        )
        .await
        .unwrap();
    // No warehouse at all: must not appear in either answer.
    let tpch = store
        .create_catalog(
            &unique("bench"),
            &CatalogDef::parse("tpch", &json!({}), &json!(null)).unwrap(),
            None,
        )
        .await
        .unwrap();

    for (catalog_id, is_primary) in [
        (led_catalog.id, true),
        (other_catalog.id, false),
        (tpch.id, false),
    ] {
        store
            .insert_attachment(&NewAttachment {
                engine_id: eng,
                catalog_id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: true,
                is_primary,
                created_at: None,
                apply_status: apply_status::APPLIED,
            })
            .await
            .unwrap();
    }

    assert_eq!(
        store.primary_warehouse_id(eng).await.unwrap(),
        Some(led),
        "the primary attachment's warehouse ({led}), not the secondary's ({other})"
    );
    let by_engine = store.warehouse_ids_by_engine(&[eng, bare]).await.unwrap();
    assert_eq!(
        by_engine.get(&eng),
        Some(&vec![led, other]),
        "every attached warehouse, primary first; the tpch catalog has none"
    );

    // An engine with no attachments at all: both reads must say "nothing".
    assert_eq!(
        store.primary_warehouse_id(bare).await.unwrap(),
        None,
        "no primary attachment"
    );
    assert_eq!(by_engine.get(&bare), None, "absent, not an empty vec");
}
