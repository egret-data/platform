//! W9c Task 6: every KV route without internal storage (no `[storage]`
//! section) answers 503 naming it, and writes nothing. Its own binary: the
//! store is installed once per process, and this one installs none.
//! Requires DATABASE_URL.

mod common;

/// A method, a URI, its headers and its body.
type Request<'a> = (&'a str, String, Vec<(&'a str, &'a [u8])>, Vec<u8>);

use serde_json::Value;

#[tokio::test]
async fn without_storage_every_route_is_503() {
    let Some(db) = common::db().await else { return };
    assert!(
        egret_workflow::storage::current().is_none(),
        "this binary installs no store"
    );
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    let base = format!("/api/ws/{ws}/workflows/namespaces/w9c.nostore/kv");
    let text: (&str, &[u8]) = ("content-type", b"text/plain");
    let json: (&str, &[u8]) = ("content-type", b"application/json");
    let requests: Vec<Request> = vec![
        ("GET", base.clone(), vec![], vec![]),
        ("GET", format!("{base}/inheritance"), vec![], vec![]),
        ("GET", format!("{base}/k"), vec![], vec![]),
        ("GET", format!("{base}/k/revisions"), vec![], vec![]),
        ("PUT", format!("{base}/k"), vec![text], b"1".to_vec()),
        (
            "PUT",
            format!("{base}/inheritance"),
            vec![text],
            b"1".to_vec(),
        ),
        ("DELETE", format!("{base}/k"), vec![], vec![]),
        ("DELETE", format!("{base}/inheritance"), vec![], vec![]),
        (
            "DELETE",
            base.clone(),
            vec![json],
            br#"{"keys":["k"]}"#.to_vec(),
        ),
    ];
    for (method, uri, headers, body) in requests {
        let res = common::send_with(&app, method, &uri, user, &headers, body.into()).await;
        assert_eq!(res.status(), 503, "{method} {uri}");
        let v: Value = serde_json::from_str(&common::body_text(res).await).unwrap();
        let msg = v["error"].as_str().unwrap();
        assert!(
            msg.contains("no internal storage configured") && msg.contains("KV value"),
            "{method} {uri}: {msg}"
        );
    }
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.kv_entries WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}
