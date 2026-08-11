//! Per-workspace SMTP settings.
//!
//! The password never lives in this table and never leaves the process: it is
//! sealed by `egret-secrets` and referenced by id. The only code that sees the
//! plaintext is [`SmtpConfigRepo::resolve_password`], called by the sender at
//! the moment it opens a connection.
//!
//! That is why the DTO in this module has no password field at all, rather than
//! a redacted one. A field that is *usually* blanked is one serialization
//! change away from not being.

use egret_metastore::Db;
use egret_secrets::SecretStore;
use uuid::Uuid;

use crate::error::AutomationError;

/// How the connection is secured. Spelled out rather than inferred from the
/// port: 587-means-STARTTLS is a convention, not a rule, and guessing wrong
/// silently sends credentials in the clear.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Connect in the clear, then upgrade. The common submission default (587).
    Starttls,
    /// TLS from the first byte (465).
    Implicit,
    /// No TLS at all. For a trusted in-cluster relay or a local mail catcher —
    /// and a real choice an operator has to make, not a fallback.
    None,
}

impl TlsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TlsMode::Starttls => "starttls",
            TlsMode::Implicit => "implicit",
            TlsMode::None => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "starttls" => Some(TlsMode::Starttls),
            "implicit" => Some(TlsMode::Implicit),
            "none" => Some(TlsMode::None),
            _ => None,
        }
    }
}

/// The stored row. `password_secret_id` is a reference, never a value.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SmtpConfig {
    pub workspace_id: Uuid,
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    pub from_address: String,
    pub from_name: Option<String>,
    pub tls_mode: String,
    pub password_secret_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// What a GET returns.
///
/// There is deliberately no password field — not even an empty or masked one.
/// `has_password` is what the editor needs (to show "configured" and to let the
/// user keep the existing one), and it is a boolean, so no serialization change
/// can turn it into the secret.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct SmtpConfigDto {
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    pub from_address: String,
    pub from_name: Option<String>,
    pub tls_mode: TlsMode,
    /// Whether a password is stored. The password itself is never returned.
    pub has_password: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Create/update body.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct SmtpConfigBody {
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub username: Option<String>,
    pub from_address: String,
    #[serde(default)]
    pub from_name: Option<String>,
    pub tls_mode: TlsMode,
    /// Omitted means "keep the stored password"; `""` means "remove it".
    ///
    /// Without the distinction, an editor that renders a blank field (because
    /// it cannot show the real one) would erase the password on every save.
    #[serde(default)]
    pub password: Option<String>,
}

pub fn dto(c: &SmtpConfig) -> SmtpConfigDto {
    SmtpConfigDto {
        host: c.host.clone(),
        port: c.port,
        username: c.username.clone(),
        from_address: c.from_address.clone(),
        from_name: c.from_name.clone(),
        tls_mode: TlsMode::parse(&c.tls_mode).unwrap_or(TlsMode::Starttls),
        has_password: c.password_secret_id.is_some(),
        updated_at: c.updated_at,
    }
}

#[derive(Clone)]
pub struct SmtpConfigRepo {
    db: Db,
    secrets: SecretStore,
}

const COLS: &str = "workspace_id, host, port, username, from_address, from_name, tls_mode, \
                    password_secret_id, updated_at";

/// The vault `kind` for SMTP passwords, so they are identifiable in the store.
const SECRET_KIND: &str = "automation_smtp_password";

impl SmtpConfigRepo {
    pub fn new(db: Db, secrets: SecretStore) -> Self {
        Self { db, secrets }
    }

    pub async fn get(&self, ws: Uuid) -> Result<Option<SmtpConfig>, AutomationError> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM automation.smtp_config WHERE workspace_id = $1"
        ))
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    /// Upsert the workspace's configuration.
    ///
    /// Password handling is the whole point:
    /// - `None` keeps whatever is stored (the editor cannot render it, so its
    ///   blank field must not mean "delete");
    /// - `Some("")` removes it;
    /// - `Some(p)` replaces it, and the OLD vault row is deleted rather than
    ///   orphaned — a superseded credential left lying in the vault is a
    ///   credential nobody is watching.
    pub async fn upsert(
        &self,
        ws: Uuid,
        body: &SmtpConfigBody,
        actor: Option<Uuid>,
    ) -> Result<SmtpConfig, AutomationError> {
        let existing = self.get(ws).await?;
        let old_secret = existing.as_ref().and_then(|c| c.password_secret_id);

        let new_secret = match body.password.as_deref() {
            None => old_secret,
            Some("") => None,
            Some(p) => Some(
                self.secrets
                    .put(SECRET_KIND, p)
                    .await
                    .map_err(|e| AutomationError::BadRequest(e.to_string()))?,
            ),
        };

        let row: SmtpConfig = sqlx::query_as(&format!(
            "INSERT INTO automation.smtp_config \
               (workspace_id, host, port, username, from_address, from_name, tls_mode, \
                password_secret_id, updated_by, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, now()) \
             ON CONFLICT (workspace_id) DO UPDATE SET \
               host = EXCLUDED.host, port = EXCLUDED.port, username = EXCLUDED.username, \
               from_address = EXCLUDED.from_address, from_name = EXCLUDED.from_name, \
               tls_mode = EXCLUDED.tls_mode, \
               password_secret_id = EXCLUDED.password_secret_id, \
               updated_by = EXCLUDED.updated_by, updated_at = now() \
             RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(&body.host)
        .bind(body.port)
        .bind(&body.username)
        .bind(&body.from_address)
        .bind(&body.from_name)
        .bind(body.tls_mode.as_str())
        .bind(new_secret)
        .bind(actor)
        .fetch_one(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        // Only once the row committed: if the write failed, the old password is
        // still the live one.
        if let (Some(old), true) = (old_secret, old_secret != new_secret) {
            let _ = self.secrets.delete(old).await;
        }
        Ok(row)
    }

    pub async fn delete(&self, ws: Uuid) -> Result<bool, AutomationError> {
        let existing = self.get(ws).await?;
        let done = sqlx::query("DELETE FROM automation.smtp_config WHERE workspace_id = $1")
            .bind(ws)
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        if let Some(id) = existing.and_then(|c| c.password_secret_id) {
            let _ = self.secrets.delete(id).await;
        }
        Ok(done.rows_affected() > 0)
    }

    /// The one place plaintext is produced, called at connection time.
    pub async fn resolve_password(
        &self,
        config: &SmtpConfig,
    ) -> Result<Option<String>, AutomationError> {
        match config.password_secret_id {
            None => Ok(None),
            Some(id) => self
                .secrets
                .get(id)
                .await
                .map(Some)
                .map_err(|e| AutomationError::BadRequest(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SmtpConfig {
        SmtpConfig {
            workspace_id: Uuid::new_v4(),
            host: "smtp.example.com".into(),
            port: 587,
            username: Some("mailer".into()),
            from_address: "alerts@example.com".into(),
            from_name: Some("Egret Alerts".into()),
            tls_mode: "starttls".into(),
            password_secret_id: Some(Uuid::new_v4()),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn the_serialized_config_contains_no_password_anywhere() {
        // Asserted on the SERIALIZED BODY, not on the struct's fields: the
        // struct having no password field is the mechanism, but what actually
        // reaches the browser is this string, and that is what must be checked.
        let json = serde_json::to_string(&dto(&sample())).unwrap();
        assert!(
            !json.contains("password_secret_id"),
            "the secret's id must not leak either: {json}"
        );
        // The word `has_password` is expected; a bare `"password"` key is not.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("password").is_none(), "no password field: {json}");
        assert_eq!(v["has_password"], true);
        assert_eq!(v["host"], "smtp.example.com");
        assert_eq!(v["tls_mode"], "starttls");
    }

    #[test]
    fn has_password_reflects_whether_one_is_stored() {
        let mut c = sample();
        assert!(dto(&c).has_password);
        c.password_secret_id = None;
        assert!(!dto(&c).has_password);
    }

    #[test]
    fn tls_modes_round_trip_through_their_stored_spelling() {
        // The spellings are CHECK-constrained in migration 0111.
        for m in [TlsMode::Starttls, TlsMode::Implicit, TlsMode::None] {
            assert_eq!(TlsMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(TlsMode::parse("ssl"), None);
        assert_eq!(
            serde_json::to_string(&TlsMode::Starttls).unwrap(),
            "\"starttls\""
        );
    }

    #[test]
    fn an_omitted_password_is_distinguishable_from_a_cleared_one() {
        // The distinction the upsert relies on. If serde collapsed these, an
        // editor that cannot render the stored password would erase it on
        // every save.
        let keep: SmtpConfigBody = serde_json::from_value(serde_json::json!({
            "host": "h", "port": 25, "from_address": "a@b.c", "tls_mode": "none"
        }))
        .unwrap();
        assert_eq!(keep.password, None, "omitted means keep");

        let clear: SmtpConfigBody = serde_json::from_value(serde_json::json!({
            "host": "h", "port": 25, "from_address": "a@b.c", "tls_mode": "none",
            "password": ""
        }))
        .unwrap();
        assert_eq!(clear.password.as_deref(), Some(""), "empty means remove");

        let set: SmtpConfigBody = serde_json::from_value(serde_json::json!({
            "host": "h", "port": 25, "from_address": "a@b.c", "tls_mode": "none",
            "password": "hunter2"
        }))
        .unwrap();
        assert_eq!(set.password.as_deref(), Some("hunter2"));
    }
}
