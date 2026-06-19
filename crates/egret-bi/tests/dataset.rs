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
async fn reconcile_add_remove_retype() {
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

    // Reconcile: drop `ts`, keep `id` but retype, add `amount`.
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
    r.reconcile_physical_columns(ds.id, &new_cols)
        .await
        .unwrap();

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
async fn reconcile_preserves_calculated_columns_and_metadata() {
    let Some((repo, wid, db_id, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let phys = vec![
        egret_bi::NewColumn {
            column_name: "id".into(),
            data_type: "bigint".into(),
            type_generic: "numeric".into(),
            is_temporal: false,
        },
        egret_bi::NewColumn {
            column_name: "ts".into(),
            data_type: "timestamp".into(),
            type_generic: "temporal".into(),
            is_temporal: true,
        },
    ];
    let ds = repo
        .create(
            wid,
            db_id,
            "sales",
            "",
            &egret_bi::SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "tiny".into(),
                table: "orders".into(),
            },
            &phys,
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();

    repo.update_column(ds.id, "id", Some("Order ID"), Some("the id"), None)
        .await
        .unwrap();
    repo.add_calc_column(ds.id, "id_x2", "id * 2", "bigint", "numeric", false, "", "")
        .await
        .unwrap();

    let fresh = vec![
        egret_bi::NewColumn {
            column_name: "id".into(),
            data_type: "varchar".into(),
            type_generic: "string".into(),
            is_temporal: false,
        },
        egret_bi::NewColumn {
            column_name: "region".into(),
            data_type: "varchar".into(),
            type_generic: "string".into(),
            is_temporal: false,
        },
    ];
    repo.reconcile_physical_columns(ds.id, &fresh)
        .await
        .unwrap();

    let cols = repo.columns(ds.id).await.unwrap();
    let by = |n: &str| cols.iter().find(|c| c.column_name == n).cloned();
    assert!(by("ts").is_none(), "dropped physical column removed");
    assert_eq!(
        by("region").unwrap().type_generic,
        "string",
        "new physical added"
    );
    let id = by("id").unwrap();
    assert_eq!(id.data_type, "varchar", "physical retyped");
    assert_eq!(
        id.verbose_name, "Order ID",
        "user metadata preserved across refresh"
    );
    let calc = by("id_x2").expect("calculated column preserved");
    assert_eq!(calc.expression.as_deref(), Some("id * 2"));
    repo.delete(ds.id).await.unwrap();
}

#[tokio::test]
async fn metric_and_calc_column_crud() {
    let Some((repo, wid, db_id, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let ds = repo
        .create(
            wid,
            db_id,
            "ds",
            "",
            &egret_bi::SourceSpec::Virtual {
                sql: "SELECT 1".into(),
            },
            &[egret_bi::NewColumn {
                column_name: "n".into(),
                data_type: "bigint".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            None,
        )
        .await
        .unwrap();

    repo.add_metric(ds.id, "total", "SUM(n)", "bigint", "numeric", "Total", "")
        .await
        .unwrap();
    assert!(repo
        .add_metric(ds.id, "total", "COUNT(*)", "bigint", "numeric", "", "")
        .await
        .is_err());
    let ms = repo.metrics(ds.id).await.unwrap();
    assert_eq!(ms.len(), 1);
    repo.update_metric(ms[0].id, "total2", "COUNT(*)", "bigint", "numeric", "", "")
        .await
        .unwrap();
    assert_eq!(repo.metric_dataset(ms[0].id).await.unwrap(), Some(ds.id));
    repo.delete_metric(ms[0].id).await.unwrap();
    assert!(repo.metrics(ds.id).await.unwrap().is_empty());

    repo.add_calc_column(ds.id, "n2", "n*2", "bigint", "numeric", false, "", "")
        .await
        .unwrap();
    assert_eq!(
        repo.column_is_calculated(ds.id, "n2").await.unwrap(),
        Some(true)
    );
    assert_eq!(
        repo.column_is_calculated(ds.id, "n").await.unwrap(),
        Some(false)
    );
    assert!(repo.delete_calc_column(ds.id, "n").await.is_err());
    repo.delete_calc_column(ds.id, "n2").await.unwrap();
    repo.delete(ds.id).await.unwrap();
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
