//! DB-gated DatasetRepo tests. Self-skip without DATABASE_URL.
use egret_bi::{DatasetRepo, NewColumn, SourceSpec};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DatasetRepo, Uuid, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let wid: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // datasets.database_id is NOT NULL — seed a minimal databases row.
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (name, engine, host, port) VALUES ($1,'trino','localhost',8080) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Some((DatasetRepo::new(db), wid, db_id, uid))
}

fn cols_2() -> Vec<NewColumn> {
    vec![
        NewColumn {
            column_name: "id".to_string(),
            data_type: "bigint".to_string(),
            type_generic: "numeric".to_string(),
            is_temporal: false,
        },
        NewColumn {
            column_name: "ts".to_string(),
            data_type: "timestamp".to_string(),
            type_generic: "temporal".to_string(),
            is_temporal: true,
        },
    ]
}

#[tokio::test]
async fn create_physical_and_virtual() {
    let Some((r, wid, db_id, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    // Physical dataset.
    let phys = r
        .create(
            wid,
            db_id,
            "orders",
            "orders table",
            &SourceSpec::Physical {
                catalog: Some("tpch".to_string()),
                schema: "sf1".to_string(),
                table: "orders".to_string(),
            },
            &cols_2(),
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();
    assert_eq!(phys.name, "orders");
    assert_eq!(phys.table_name.as_deref(), Some("orders"));
    assert!(phys.sql.is_none());
    assert_eq!(phys.catalog.as_deref(), Some("tpch"));

    // Virtual dataset.
    let virt = r
        .create(
            wid,
            db_id,
            "virt1",
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 1".to_string(),
            },
            &[NewColumn {
                column_name: "one".to_string(),
                data_type: "integer".to_string(),
                type_generic: "numeric".to_string(),
                is_temporal: false,
            }],
            None,
        )
        .await
        .unwrap();
    assert_eq!(virt.sql.as_deref(), Some("SELECT 1"));
    assert!(virt.table_name.is_none());

    r.delete(phys.id).await.unwrap();
    r.delete(virt.id).await.unwrap();
}

#[tokio::test]
async fn list_returns_column_count() {
    let Some((r, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "list_test",
            "",
            &SourceSpec::Physical {
                catalog: None,
                schema: "sf1".to_string(),
                table: "lineitem".to_string(),
            },
            &cols_2(),
            None,
        )
        .await
        .unwrap();

    let listed = r.list(wid).await.unwrap();
    let entry = listed.iter().find(|(d, _)| d.id == ds.id).expect("present");
    assert_eq!(entry.1, 2);

    r.delete(ds.id).await.unwrap();
}

#[tokio::test]
async fn columns_ordered_and_temporal_flag() {
    let Some((r, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "col_order_test",
            "",
            &SourceSpec::Physical {
                catalog: None,
                schema: "s".to_string(),
                table: "t".to_string(),
            },
            &cols_2(),
            None,
        )
        .await
        .unwrap();

    let cols = r.columns(ds.id).await.unwrap();
    assert_eq!(cols.len(), 2);
    assert_eq!(cols[0].column_name, "id");
    assert_eq!(cols[0].ordinal, 0);
    assert!(!cols[0].is_temporal);
    assert_eq!(cols[1].column_name, "ts");
    assert_eq!(cols[1].ordinal, 1);
    assert!(cols[1].is_temporal);

    r.delete(ds.id).await.unwrap();
}

#[tokio::test]
async fn update_name_and_description() {
    let Some((r, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "before",
            "old desc",
            &SourceSpec::Physical {
                catalog: None,
                schema: "s".to_string(),
                table: "t".to_string(),
            },
            &cols_2(),
            None,
        )
        .await
        .unwrap();

    let upd = r.update(ds.id, "after", "new desc", None).await.unwrap();
    assert_eq!(upd.name, "after");
    assert_eq!(upd.description, "new desc");

    r.delete(ds.id).await.unwrap();
}

#[tokio::test]
async fn replace_columns_add_remove_retype() {
    let Some((r, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "refresh_test",
            "",
            &SourceSpec::Physical {
                catalog: None,
                schema: "s".to_string(),
                table: "t".to_string(),
            },
            &cols_2(), // id(bigint/numeric/false), ts(timestamp/temporal/true)
            None,
        )
        .await
        .unwrap();

    // Replace: drop `ts`, keep `id` but retype, add `amount`.
    let new_cols = vec![
        NewColumn {
            column_name: "id".to_string(),
            data_type: "varchar".to_string(),   // retyped
            type_generic: "string".to_string(), // retyped
            is_temporal: false,
        },
        NewColumn {
            column_name: "amount".to_string(), // added
            data_type: "decimal".to_string(),
            type_generic: "numeric".to_string(),
            is_temporal: false,
        },
    ];
    r.replace_columns(ds.id, &new_cols).await.unwrap();

    let cols = r.columns(ds.id).await.unwrap();
    assert_eq!(cols.len(), 2, "ts removed, amount added → 2 cols");
    assert_eq!(cols[0].column_name, "id");
    assert_eq!(cols[0].data_type, "varchar"); // retyped
    assert_eq!(cols[0].type_generic, "string"); // retyped
    assert_eq!(cols[1].column_name, "amount");

    r.delete(ds.id).await.unwrap();
}

#[tokio::test]
async fn delete_cascades_columns() {
    let Some((r, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "cascade_test",
            "",
            &SourceSpec::Physical {
                catalog: None,
                schema: "s".to_string(),
                table: "t".to_string(),
            },
            &cols_2(),
            None,
        )
        .await
        .unwrap();

    r.delete(ds.id).await.unwrap();

    assert!(r.get(ds.id).await.unwrap().is_none(), "dataset gone");
    assert!(
        r.columns(ds.id).await.unwrap().is_empty(),
        "columns cascaded"
    );
}

#[tokio::test]
async fn owner_returns_creator() {
    let Some((r, wid, db_id, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = r
        .create(
            wid,
            db_id,
            "owner_test",
            "",
            &SourceSpec::Physical {
                catalog: None,
                schema: "s".to_string(),
                table: "t".to_string(),
            },
            &cols_2(),
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();

    assert_eq!(r.owner(ds.id).await.unwrap(), Some(uid));
    r.delete(ds.id).await.unwrap();
}
