//! P1b eager boot backfill: convert existing connection credential secrets from
//! `Opaque` to `kubernetes.io/basic-auth`. Runs once at `serve` boot, idempotently.
//!
//! Storage + DB connection WRITE paths now mint basic-auth secrets and READ paths
//! resolve creds by key; this one-shot pass upgrades the secrets that predate that
//! change so every connection converges on the typed basic-auth shape.

use egret_metastore::Db;
use egret_secrets::{secret_type, SecretStore};
use uuid::Uuid;

/// Convert one vault secret (referenced by a connection) to basic-auth, unless it
/// already is. `identity` becomes `username`; the existing single value becomes
/// `password`. Idempotent; returns true iff it converted. Empty identity is skipped.
pub async fn convert_one(
    secrets: &SecretStore,
    vault_ref: Uuid,
    identity: &str,
) -> anyhow::Result<bool> {
    if identity.is_empty() {
        return Ok(false);
    }
    let typed = secrets.get_typed(vault_ref).await?;
    if typed.r#type == secret_type::BASIC_AUTH {
        return Ok(false);
    }
    let password = secrets.get(vault_ref).await?;
    let data = std::collections::BTreeMap::from([
        ("username".to_string(), identity.to_string()),
        ("password".to_string(), password),
    ]);
    secrets
        .update_typed(vault_ref, secret_type::BASIC_AUTH, &data)
        .await?;
    Ok(true)
}

/// One-shot boot backfill: convert every storage + DB connection's Opaque
/// credential secret to `kubernetes.io/basic-auth`. Idempotent.
pub async fn backfill_basic_auth_credentials(db: &Db, secrets: &SecretStore) -> anyhow::Result<()> {
    // `workspace_secrets.secret_ref` is the VAULT `secrets` id that `SecretStore`
    // keys on; the connection FK points at `workspace_secrets.id`.
    let storage: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT s.access_key, ws.secret_ref \
         FROM storage.storage_connections s JOIN workspace_secrets ws ON ws.id = s.secret_ref \
         WHERE s.secret_ref IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;
    let dbs: Vec<(serde_json::Value, Uuid)> = sqlx::query_as(
        "SELECT d.options, ws.secret_ref \
         FROM query.databases d JOIN workspace_secrets ws ON ws.id = d.secret_id \
         WHERE d.secret_id IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await?;
    let mut n = 0usize;
    for (access_key, vault_ref) in storage {
        if convert_one(secrets, vault_ref, &access_key).await? {
            n += 1;
        }
    }
    for (options, vault_ref) in dbs {
        let user = options.get("user").and_then(|u| u.as_str()).unwrap_or("");
        if convert_one(secrets, vault_ref, user).await? {
            n += 1;
        }
    }
    if n > 0 {
        tracing::info!(
            converted = n,
            "P1b: backfilled connection creds to kubernetes.io/basic-auth"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_secrets::Crypto;

    async fn test_secrets() -> Option<(Db, SecretStore)> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let db = Db::connect(&url, 5).await.expect("connect");
        db.migrate().await.expect("migrate");
        let crypto = Crypto::new(&[7u8; 32]).unwrap();
        let secrets = SecretStore::new(db.clone(), Some(crypto));
        Some((db, secrets))
    }

    #[tokio::test]
    async fn backfill_converts_opaque_to_basic_auth_idempotently() {
        let Some((_db, secrets)) = test_secrets().await else {
            eprintln!("skip: no DATABASE_URL");
            return;
        };
        let vault_ref = secrets
            .put_typed(
                "workspace_secret",
                egret_secrets::secret_type::OPAQUE,
                &std::collections::BTreeMap::from([("value".to_string(), "pw".to_string())]),
            )
            .await
            .unwrap();
        assert!(convert_one(&secrets, vault_ref, "ak").await.unwrap());
        let typed = secrets.get_typed(vault_ref).await.unwrap();
        assert_eq!(typed.r#type, egret_secrets::secret_type::BASIC_AUTH);
        assert_eq!(typed.data.get("username").unwrap(), "ak");
        assert_eq!(typed.data.get("password").unwrap(), "pw");
        // idempotent: already basic-auth -> no-op.
        assert!(!convert_one(&secrets, vault_ref, "ak").await.unwrap());
        // empty identity -> skipped.
        let v2 = secrets
            .put_typed(
                "workspace_secret",
                egret_secrets::secret_type::OPAQUE,
                &std::collections::BTreeMap::from([("value".to_string(), "x".to_string())]),
            )
            .await
            .unwrap();
        assert!(!convert_one(&secrets, v2, "").await.unwrap());
    }
}

/// One-shot boot backfill: move any CLEARTEXT webhook signing secret out of
/// `automation.schedule_recipients.config` and into the vault, leaving a
/// `secret_id` reference. Idempotent; a no-op once converged.
///
/// Rows written before the vault existed hold `{"url": …, "secret": "…"}`. The
/// runner now resolves `secret_id` only, so without this pass those webhooks
/// would keep their cleartext at rest AND silently start delivering unsigned —
/// a downgrade no receiver would notice until it started rejecting.
pub async fn backfill_webhook_secrets(db: &Db, secrets: &SecretStore) -> anyhow::Result<()> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT id, config FROM automation.schedule_recipients \
         WHERE kind = 'webhook' AND config ? 'secret'",
    )
    .fetch_all(&db.pool)
    .await?;

    let mut moved = 0usize;
    for (id, config) in rows {
        let Some(plaintext) = config.get("secret").and_then(|v| v.as_str()) else {
            continue;
        };
        // An empty string is not a credential; drop the key rather than mint a
        // vault row holding nothing.
        let new_config = if plaintext.is_empty() {
            serde_json::json!({ "url": config.get("url").cloned().unwrap_or(serde_json::Value::Null) })
        } else {
            let vault_ref = secrets
                .put(egret_automation::WEBHOOK_SECRET_KIND, plaintext)
                .await?;
            serde_json::json!({
                "url": config.get("url").cloned().unwrap_or(serde_json::Value::Null),
                "secret_id": vault_ref.to_string(),
            })
        };
        sqlx::query("UPDATE automation.schedule_recipients SET config = $2 WHERE id = $1")
            .bind(id)
            .bind(&new_config)
            .execute(&db.pool)
            .await?;
        moved += 1;
    }
    if moved > 0 {
        tracing::info!(
            count = moved,
            "moved cleartext webhook secrets into the vault"
        );
    }
    Ok(())
}
