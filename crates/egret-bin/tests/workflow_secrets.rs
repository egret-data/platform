//! W5b Task 4: `secret()` against the real `workspace_secrets`, through the
//! production `WorkspaceSecretSource` and `render_with_secrets`. Two
//! workspaces, one secret of the same name in each, plus a secret only the
//! other workspace has and a global one.
//!
//! DB-gated: self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`,
//! and otherwise FAILS.

use std::collections::BTreeMap;

use egret_bin::workflow_secrets::WorkspaceSecretSource;
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore, WorkspaceSecretStore};
use egret_workflow::runners::secrets::render_with_secrets;
use egret_workflow::RenderContext;
use serde_json::{json, Value};
use uuid::Uuid;

struct Fixture {
    ws: Uuid,
    other: Uuid,
    global: String,
    source: WorkspaceSecretSource,
}

async fn seed_workspace(db: &Db) -> Uuid {
    let id = Uuid::new_v4();
    let suffix = id.simple().to_string();
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("ws-{suffix}"))
        .bind(format!("ws {suffix}"))
        .execute(&db.pool)
        .await
        .expect("seed workspace");
    id
}

async fn fixture() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let store = WorkspaceSecretStore::new(db.clone(), SecretStore::new(db.clone(), Some(crypto)));
    let ws = seed_workspace(&db).await;
    let other = seed_workspace(&db).await;
    let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };

    // The same name in both workspaces.
    store
        .create_typed(
            Some(ws),
            "db",
            None,
            "kubernetes.io/basic-auth",
            &map(&[("username", "mine-user"), ("password", "mine-pass")]),
        )
        .await
        .unwrap();
    store
        .create_typed(
            Some(other),
            "db",
            None,
            "kubernetes.io/basic-auth",
            &map(&[("username", "their-user"), ("password", "their-pass")]),
        )
        .await
        .unwrap();
    // A single-value secret in each, created the legacy (untyped) way.
    store
        .create(Some(ws), "token", None, "mine-token")
        .await
        .unwrap();
    store
        .create(Some(other), "token", None, "their-token")
        .await
        .unwrap();
    // Names only the OTHER workspace, or only the platform, has.
    store
        .create(Some(other), "only-theirs", None, "their-only")
        .await
        .unwrap();
    let global = format!("global-{}", Uuid::new_v4().simple());
    store
        .create(None, &global, None, "platform-value")
        .await
        .unwrap();

    Some(Fixture {
        ws,
        other,
        global,
        source: WorkspaceSecretSource::new(store),
    })
}

const THEIR_VALUES: [&str; 5] = [
    "their-user",
    "their-pass",
    "their-token",
    "their-only",
    "platform-value",
];

#[tokio::test]
async fn secret_resolves_value_and_subkey_in_the_executions_workspace() {
    let Some(f) = fixture().await else {
        eprintln!("skip");
        return;
    };
    let props = json!({
        "url": "jdbc:postgresql://h/db",
        "username": "{{ secret('db', subkey='username') }}",
        "password": "{{ secret('db', subkey='password') }}",
        "token": "{{ secret('token') }}",
    });
    let (out, resolved) = render_with_secrets(
        props.as_object().unwrap(),
        &RenderContext::new(),
        f.ws,
        &f.source,
    )
    .await
    .expect("renders");
    assert_eq!(
        Value::Object(out),
        json!({
            "url": "jdbc:postgresql://h/db",
            "username": "mine-user",
            "password": "mine-pass",
            "token": "mine-token",
        })
    );
    let mut values = resolved.values;
    values.sort();
    assert_eq!(values, vec!["mine-pass", "mine-token", "mine-user"]);

    // The other workspace, same template: its own values, never ours.
    let (out, _) = render_with_secrets(
        props.as_object().unwrap(),
        &RenderContext::new(),
        f.other,
        &f.source,
    )
    .await
    .expect("renders");
    assert_eq!(out["password"], "their-pass");
    assert_eq!(out["token"], "their-token");
}

#[tokio::test]
async fn a_secret_of_another_workspace_is_not_found_and_the_message_names_no_value() {
    let Some(f) = fixture().await else {
        eprintln!("skip");
        return;
    };
    for name in ["only-theirs", f.global.as_str()] {
        let props = json!({ "v": format!("{{{{ secret('{name}') }}}}") });
        let e = render_with_secrets(
            props.as_object().unwrap(),
            &RenderContext::new(),
            f.ws,
            &f.source,
        )
        .await
        .expect_err("another workspace's secret, or a global one, must not resolve");
        assert!(
            e.contains(&format!(
                "secret '{name}' was not found in this execution's workspace"
            )),
            "{name}: {e}"
        );
        for value in THEIR_VALUES
            .iter()
            .chain(&["mine-user", "mine-pass", "mine-token"])
        {
            assert!(
                !e.contains(value),
                "{name}: the message carries {value:?}: {e}"
            );
        }
    }
}
