//! RegistryStore against a real database. DB-gated; self-skips only under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry --test store

use egret_catalog_registry::report::ReportEntry;
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

async fn engine(db: &Db, wh: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reg-eng-{}", Uuid::new_v4().simple()))
    .bind(wh)
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
    let eng = engine(&db, wh).await;
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
            created_at: None,
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
            created_at: None,
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
                created_at: None,
            })
            .await,
        Err(RegistryError::AlreadyExists(_))
    ));

    let listed = store.list_engine_attachments(eng, Some(wh)).await.unwrap();
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
    assert_eq!(store.unmigrated_legacy_catalog_count(eng).await.unwrap(), 0);
}

#[tokio::test]
async fn report_rows_are_listed_until_acknowledged() {
    let Some(db) = db().await else { return };
    let store = RegistryStore::new(db.clone());
    let run = Uuid::new_v4();
    store
        .insert_report(&[
            ReportEntry::info(
                run,
                "warehouse_catalog_created",
                "warehouse a",
                "a is now catalog a",
            ),
            ReportEntry::warning(run, "backfill_failed", "engine x catalog y", "not migrated")
                .detail(json!({ "why": "test" })),
        ])
        .await
        .unwrap();
    let open = store.list_report(false).await.unwrap();
    assert!(open.iter().filter(|r| r.run_id == run).count() == 2);
    assert!(store.acknowledge_report().await.unwrap() >= 2);
    assert!(store
        .list_report(false)
        .await
        .unwrap()
        .iter()
        .all(|r| r.run_id != run));
    assert_eq!(
        store
            .list_report(true)
            .await
            .unwrap()
            .iter()
            .filter(|r| r.run_id == run)
            .count(),
        2
    );
}
