//! StarRocks connector — MySQL protocol (mysql_async) with row-cap + cancellation.

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use egret_core::{ColumnMeta, TabularResult};

use super::{ConnSpec, Connector};
use crate::execution::error::EngineError;
use crate::execution::model::Limits;

pub struct StarRocksConnector {
    opts: mysql_async::Opts,
    /// Set on the impersonate path (service-user login + run AS this user).
    execute_as: Option<String>,
    /// Set (to a refusal message) when this connection carries a per-user JWT but
    /// no TLS — StarRocks sends the JWT as a *cleartext* MySQL password, so without
    /// TLS the token leaks verbatim. Enforced in `execute`/`describe` (fail closed).
    insecure_bearer: Option<String>,
}

impl StarRocksConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        // Under delegation (a bearer is present), StarRocks `authentication_jwt`
        // requires the MySQL login user to equal the JWT principal
        // (`preferred_username`) — so use the resolved per-user `session_user`,
        // mirroring Trino's `X-Trino-User = session_user`. Without a bearer the
        // connection's configured service user is used (unchanged behavior).
        let user = if spec.bearer_token.is_some() {
            spec.session_user
                .clone()
                .or_else(|| spec.options.user.clone())
                .unwrap_or_else(|| "root".into())
        } else {
            spec.options.user.clone().unwrap_or_else(|| "root".into())
        };
        let mut b = mysql_async::OptsBuilder::default()
            .ip_or_hostname(spec.host.clone())
            .tcp_port(u16::try_from(spec.port).unwrap_or(9030))
            .user(Some(user))
            .prefer_socket(false);
        if let Some(jwt) = &spec.bearer_token {
            // StarRocks `authentication_jwt`: present the OIDC JWT as the cleartext
            // "password" via the mysql_clear_password client plugin. mysql_async sends
            // it verbatim (unhashed) when the server requests that plugin.
            b = b.pass(Some(jwt.clone())).enable_cleartext_plugin(true);
        } else if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
        if let Some(db) = &spec.options.database {
            b = b.db_name(Some(db.clone()));
        }
        // Impersonate mode (Trino parity): no bearer, connection opts in, end-user resolved.
        // Log in as the service user (above) and switch identity per-query in `execute`
        // via `EXECUTE AS <session_user> WITH NO REVERT`.
        let execute_as = if spec.bearer_token.is_none() && spec.options.impersonate == Some(true) {
            spec.session_user.clone()
        } else {
            None
        };
        // TLS: pin the FE cert. mysql_clear_password sends the JWT in cleartext, so a
        // bearer connection MUST be TLS — enforced (fail closed) via `insecure_bearer`
        // below, no longer just a comment.
        let has_tls = spec.options.tls_cert.is_some();
        if let Some(pem) = &spec.options.tls_cert {
            let ssl = mysql_async::SslOpts::default()
                .with_root_certs(vec![mysql_async::PathOrBuf::from(pem.clone().into_bytes())])
                // Safe: we pin the exact self-signed FE leaf cert as the trust root, so the
                // chain/signature are still validated (via `with_root_certs`) — only
                // hostname/SAN matching is skipped, needed because the FE's self-signed cert
                // CN won't match the in-cluster Service DNS we dial.
                .with_danger_skip_domain_validation(true);
            b = b.ssl_opts(Some(ssl));
        }
        StarRocksConnector {
            opts: b.into(),
            execute_as,
            // StarRocks' only TLS path here is the pinned FE cert, so the transport is
            // TLS iff `tls_cert` was provided. Without it, the cleartext JWT password
            // would go over a plaintext socket → refuse.
            insecure_bearer: super::insecure_bearer_reason(
                spec.bearer_token.is_some(),
                has_tls,
                super::tls_insecure_optout(),
            ),
        }
    }

    /// The transport-security refusal, if this connection would leak a bearer JWT
    /// (StarRocks sends it as a cleartext password) over a non-TLS socket.
    fn guard(&self) -> Result<(), EngineError> {
        match &self.insecure_bearer {
            Some(reason) => Err(EngineError::Connect(reason.clone())),
            None => Ok(()),
        }
    }

    /// Switch the connection's session identity to `user` (impersonate mode).
    /// Backticks in the identifier are doubled per the MySQL identifier grammar.
    async fn switch_identity(conn: &mut mysql_async::Conn, user: &str) -> Result<(), EngineError> {
        let esc = user.replace('`', "``");
        conn.query_drop(format!("EXECUTE AS `{esc}` WITH NO REVERT"))
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))
    }

    #[cfg(test)]
    pub fn opts(&self) -> &mysql_async::Opts {
        &self.opts
    }

    #[cfg(test)]
    pub fn execute_as(&self) -> Option<&str> {
        self.execute_as.as_deref()
    }

    #[cfg(test)]
    pub fn insecure_bearer(&self) -> Option<&str> {
        self.insecure_bearer.as_deref()
    }
}

#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        self.guard()?;
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        if let Some(u) = &self.execute_as {
            Self::switch_identity(&mut conn, u).await?;
        }

        let mut result = conn
            .query_iter(sql)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        // Read columns BEFORE consuming any rows — columns_ref() accesses the pending result set.
        let columns: Vec<ColumnMeta> = result
            .columns_ref()
            .iter()
            .map(|c| ColumnMeta {
                name: c.name_str().to_string(),
                ty: format!("{:?}", c.column_type()),
            })
            .collect();

        // Capture per-column MySQL types for type-aware value coercion.
        let col_types: Vec<mysql_async::consts::ColumnType> = result
            .columns_ref()
            .iter()
            .map(|c| c.column_type())
            .collect();

        let cap = limits.max_rows;
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();

        loop {
            if cancel.is_cancelled() {
                drop(result);
                let _ = conn.disconnect().await;
                return Err(EngineError::Cancelled);
            }
            match result
                .next()
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?
            {
                None => break,
                Some(row) => {
                    rows.push(row_to_json(row, &col_types));
                    if rows.len() > cap {
                        break;
                    }
                }
            }
        }

        drop(result);
        let _ = conn.disconnect().await;

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(TabularResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated,
        })
    }

    async fn describe(&self, sql: &str, timeout: Duration) -> Result<Vec<ColumnMeta>, EngineError> {
        self.guard()?;
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let opts = self.opts.clone();
        let execute_as = self.execute_as.clone();
        // Bound the whole connect+query under `timeout` — unlike `execute`, `describe`
        // is called directly (no JobManager outer timeout), so a hung query would block.
        let fut = async move {
            let mut conn = mysql_async::Conn::new(opts)
                .await
                .map_err(|e| EngineError::Connect(e.to_string()))?;
            // Impersonate (Trino parity): switch to the end-user before introspecting so
            // dataset preview/describe honors per-user Ranger, matching `execute`.
            if let Some(u) = &execute_as {
                StarRocksConnector::switch_identity(&mut conn, u).await?;
            }
            let result = conn
                .query_iter(wrapped)
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?;
            let columns: Vec<ColumnMeta> = result
                .columns_ref()
                .iter()
                .map(|c| ColumnMeta {
                    name: c.name_str().to_string(),
                    ty: coltype_name(c.column_type()).to_string(),
                })
                .collect();
            drop(result);
            let _ = conn.disconnect().await;
            Ok::<Vec<ColumnMeta>, EngineError>(columns)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| EngineError::Timeout)?
    }
}

fn row_to_json(
    row: mysql_async::Row,
    col_types: &[mysql_async::consts::ColumnType],
) -> Vec<serde_json::Value> {
    use mysql_async::Value as V;
    (0..row.len())
        .map(|i| match row.as_ref(i) {
            Some(V::NULL) | None => serde_json::Value::Null,
            Some(V::Int(n)) => serde_json::json!(n),
            Some(V::UInt(n)) => serde_json::json!(n),
            Some(V::Float(f)) => serde_json::json!(f),
            Some(V::Double(f)) => serde_json::json!(f),
            // Text protocol returns everything as Bytes; use the declared column type
            // to decide coercion — prevents silent string→number corruption (e.g. "007"→7).
            Some(V::Bytes(b)) => bytes_to_json(b, col_types.get(i).copied()),
            Some(other) => serde_json::Value::String(format!("{:?}", other)),
        })
        .collect()
}

/// Coerce a MySQL text-protocol byte string to a JSON scalar using the declared column type.
///
/// - Integer types  → parse as i64, fall back to u64, else string.
/// - Float types    → parse as f64, else string.
/// - Everything else (VARCHAR, DECIMAL, dates, …) → keep as string to preserve exact value.
/// - Non-UTF-8 binary → hex-encode.
fn bytes_to_json(b: &[u8], ty: Option<mysql_async::consts::ColumnType>) -> serde_json::Value {
    use mysql_async::consts::ColumnType::*;
    let Ok(s) = std::str::from_utf8(b) else {
        return serde_json::Value::String(hex::encode(b));
    };
    match ty {
        Some(
            MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR,
        ) => s
            .parse::<i64>()
            .map(|i| serde_json::json!(i))
            .or_else(|_| s.parse::<u64>().map(|u| serde_json::json!(u)))
            .unwrap_or_else(|_| serde_json::Value::String(s.to_owned())),
        Some(MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(s.to_owned())),
        // DECIMAL/NEWDECIMAL, VARCHAR, STRING, dates, blobs, and everything else:
        // keep as string to preserve exact representation.
        _ => serde_json::Value::String(s.to_owned()),
    }
}

/// Map a MySQL/StarRocks protocol column type to a canonical lowercase SQL name,
/// matching the vocabulary `DESCRIBE` returns so `typegen` classifies both the
/// physical (DESCRIBE) and virtual (describe) paths identically.
fn coltype_name(ty: mysql_async::consts::ColumnType) -> &'static str {
    use mysql_async::consts::ColumnType::*;
    match ty {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => "datetime",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING => "varchar",
        _ => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::StarRocksConnector;
    use crate::execution::connector::ConnSpec;
    use crate::registry::model::{ConnOptions, Engine};

    fn spec(bearer: Option<String>, secret: Option<String>) -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "h".into(),
            port: 9030,
            options: ConnOptions {
                user: Some("alice".into()),
                ..Default::default()
            },
            secret,
            session_user: Some("alice".into()),
            bearer_token: bearer,
        }
    }

    #[test]
    fn bearer_uses_cleartext_plugin_with_jwt_as_password() {
        let c =
            StarRocksConnector::new(&spec(Some("eyJ.jwt.sig".into()), Some("native-pw".into())));
        assert!(c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("eyJ.jwt.sig"));
    }

    #[test]
    fn no_bearer_keeps_native_password_no_cleartext() {
        let c = StarRocksConnector::new(&spec(None, Some("native-pw".into())));
        assert!(!c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("native-pw"));
    }

    /// Build a spec whose configured service user differs from the per-user
    /// `session_user`, so the two login-user branches are distinguishable.
    fn spec_distinct(bearer: Option<String>) -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "h".into(),
            port: 9030,
            options: ConnOptions {
                user: Some("svc".into()),
                ..Default::default()
            },
            secret: Some("native-pw".into()),
            session_user: Some("alice".into()),
            bearer_token: bearer,
        }
    }

    #[test]
    fn bearer_login_user_is_session_user() {
        // Delegation: the MySQL login must be the JWT principal, not the service user.
        let c = StarRocksConnector::new(&spec_distinct(Some("eyJ.jwt.sig".into())));
        assert_eq!(c.opts().user(), Some("alice"));
    }

    #[test]
    fn no_bearer_login_user_is_service_user() {
        let c = StarRocksConnector::new(&spec_distinct(None));
        assert_eq!(c.opts().user(), Some("svc"));
    }

    fn base_spec() -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "fe".into(),
            port: 9030,
            options: ConnOptions::default(),
            secret: Some("svc-pw".into()),
            session_user: None,
            bearer_token: None,
        }
    }

    #[test]
    fn impersonate_sets_execute_as_and_service_login() {
        let mut s = base_spec();
        s.options.impersonate = Some(true);
        s.options.user = Some("svc".into());
        s.session_user = Some("alice".into());
        let c = StarRocksConnector::new(&s);
        assert_eq!(c.execute_as(), Some("alice"));
        assert_eq!(c.opts().user(), Some("svc"));
    }

    #[test]
    fn jwt_mode_logs_in_as_user_no_execute_as() {
        let mut s = base_spec();
        s.bearer_token = Some("jwt".into());
        s.session_user = Some("alice".into());
        let c = StarRocksConnector::new(&s);
        assert_eq!(c.execute_as(), None);
        assert_eq!(c.opts().user(), Some("alice"));
    }

    #[test]
    fn tls_cert_enables_ssl() {
        let mut s = base_spec();
        s.options.tls_cert =
            Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into());
        assert!(StarRocksConnector::new(&s).opts().ssl_opts().is_some());
    }

    // StarRocks sends the JWT as a CLEARTEXT password, so a bearer connection
    // without TLS (no pinned cert) would leak it → flagged insecure (execute/
    // describe fail closed). With the pinned cert it's fine; no bearer is never
    // flagged. (Assumes EGRET_ENGINE_TLS_INSECURE unset, the default.)
    #[test]
    fn bearer_requires_tls() {
        // bearer, no tls_cert → refused
        let mut s = base_spec();
        s.bearer_token = Some("eyJ.jwt.sig".into());
        s.session_user = Some("alice".into());
        assert!(StarRocksConnector::new(&s).insecure_bearer().is_some());

        // bearer + pinned cert → allowed
        s.options.tls_cert =
            Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into());
        assert!(StarRocksConnector::new(&s).insecure_bearer().is_none());

        // no bearer (service user / impersonate), no tls → never flagged
        let plain = base_spec();
        assert!(StarRocksConnector::new(&plain).insecure_bearer().is_none());
    }

    #[test]
    fn no_tls_cert_no_ssl() {
        assert!(StarRocksConnector::new(&base_spec())
            .opts()
            .ssl_opts()
            .is_none());
    }
}
