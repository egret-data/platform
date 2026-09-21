//! The channel egret's control plane uses to reach a StarRocks FE: one trait so
//! the whole apply path can be driven in a test without a MySQL port (design
//! D8), the real `mysql_async` runner, the FE coordinates for an engine row,
//! and the `SHOW CATALOGS` / `SHOW CREATE CATALOG` reads that decide whether a
//! catalog is absent, matching or drifted (D3).
//!
//! D6: no statement passed through here is ever logged, and no error returned
//! from here reaches a column or a response without `scrub_secrets`.
//!
//! SP3b Task 3 introduces this seam without yet wiring it in:
//! `spawn_starrocks_catalog_wire`'s bootstrap and `run_starrocks_ddl` (both in
//! `engines.rs`) keep dialing `mysql_async` directly for now — Tasks 4 and 5
//! are the ones that move the apply path and the bootstrap onto
//! [`StarRocksDdl`] and retire the old call sites.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use egret_catalog_registry::{escape_dq, escape_ident, StarRocksCatalogDdl};

use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

/// Where and how to reach a StarRocks FE.
#[derive(Clone, Debug)]
pub struct FeTarget {
    pub host: String,
    pub port: u16,
    pub root_password: Option<String>,
    /// The engine's pinned FE certificate. `Some` ⇒ the connection runs over
    /// TLS with this cert as its only trust root and domain validation off
    /// (the cert's CN cannot match the in-cluster Service DNS egret dials) —
    /// the same posture `StarRocksConnector::new` and the delegation bootstrap
    /// already use, now applied to EVERY statement (D6).
    pub tls_cert_pem: Option<String>,
}

/// One statement/query channel to a StarRocks FE. A trait so the apply and
/// bootstrap paths (Tasks 4/5) can be driven in a test against
/// [`FakeStarRocksDdl`] instead of a real MySQL port (D8).
#[async_trait::async_trait]
pub trait StarRocksDdl: Send + Sync {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), String>;
    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, String>;
}

/// The real runner: one `mysql_async` connection per statement, exactly like
/// today's `run_starrocks_ddl` (`engines.rs`) — a one-shot admin statement has
/// no caller-facing row cap/cancellation/result shape to thread through, so
/// this stays direct `mysql_async` rather than `egret_query`'s interactive
/// `StarRocksConnector`.
pub struct MysqlDdlRunner;

#[async_trait::async_trait]
impl StarRocksDdl for MysqlDdlRunner {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), String> {
        use mysql_async::prelude::Queryable;
        let opts = crate::engines::starrocks_root_conn_opts(
            &fe.host,
            fe.port,
            fe.root_password.as_deref(),
            fe.tls_cert_pem.as_deref(),
        );
        let mut conn = mysql_async::Conn::new(opts)
            .await
            .map_err(|e| e.to_string())?;
        conn.query_drop(sql).await.map_err(|e| e.to_string())?;
        let _ = conn.disconnect().await;
        Ok(())
    }

    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        use mysql_async::prelude::Queryable;
        let opts = crate::engines::starrocks_root_conn_opts(
            &fe.host,
            fe.port,
            fe.root_password.as_deref(),
            fe.tls_cert_pem.as_deref(),
        );
        let mut conn = mysql_async::Conn::new(opts)
            .await
            .map_err(|e| e.to_string())?;
        let rows: Vec<mysql_async::Row> = conn.query(sql).await.map_err(|e| e.to_string())?;
        let out = rows
            .into_iter()
            .map(|row| {
                (0..row.columns_ref().len())
                    .map(|i| {
                        row.as_ref(i)
                            .cloned()
                            .and_then(|v| mysql_async::from_value_opt::<Option<String>>(v).ok())
                            .flatten()
                    })
                    .collect()
            })
            .collect();
        let _ = conn.disconnect().await;
        Ok(out)
    }
}

/// Where and how to reach this engine's FE. The cert is the engine's own
/// (`engines.coordinator_cert`, minted by `ensure_tls_cert`), used whenever the
/// stored spec has `starrocksConfig.delegation.tlsEnabled` — the same condition
/// under which the FE actually serves TLS. Before SP3b the catalog DDL and the
/// root-login probe both passed `None` while the delegation DDL passed the
/// cert, so an OAuth2 credential AND the FE root password crossed the wire in
/// cleartext on a TLS-capable engine (D6).
// `pub(crate)`, not the public API: only this crate's apply/bootstrap code
// (Tasks 4/5) calls it. Until they land it has no production caller — only
// `engines::tests::fe_target_resolves_root_password_and_gates_the_cert_on_delegation_tls`
// exercises it — so the plain (non-test) compilation of this crate would
// otherwise warn `dead_code` under `-D warnings`.
#[allow(dead_code)]
pub(crate) async fn fe_target(st: &CpState, engine: &Engine) -> Result<FeTarget, CpError> {
    let root_password = match engine.root_secret_id {
        Some(sid) => Some(
            st.secrets
                .resolve_plaintext(None, sid)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve root password secret: {e}")))?,
        ),
        None => None,
    };
    let tls = crate::engines::sr_delegation_block(&engine.spec)
        .get("tlsEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(FeTarget {
        host: engine.coordinator_host(),
        port: engine.coordinator_port() as u16,
        root_password,
        tls_cert_pem: tls.then(|| engine.coordinator_cert.clone()).flatten(),
    })
}

/// `SHOW CATALOGS` → column 0 (`Catalog`; the three columns are
/// `Catalog, Type, Comment`, `ShowResultMetaFactory.java:1305-1311`).
pub async fn show_catalogs(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
) -> Result<BTreeSet<String>, String> {
    Ok(ddl
        .query(fe, "SHOW CATALOGS")
        .await?
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

/// `SHOW CREATE CATALOG <name>` → column 1 (`Create Catalog`), parsed into its
/// properties. `None` when the FE says the catalog is unknown
/// (`ERR_BAD_CATALOG_ERROR` 5078, text `Unknown catalog '<name>'`) — that is a
/// SemanticException, not an empty result, so it arrives as an Err string and
/// is translated here rather than at every call site.
pub async fn show_create_catalog(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
    name: &str,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let sql = format!("SHOW CREATE CATALOG `{}`", escape_ident(name));
    match ddl.query(fe, &sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().nth(1).flatten())
            .map(|text| parse_show_create_properties(&text))),
        Err(e) if e.contains("Unknown catalog") => Ok(None),
        Err(e) => Err(e),
    }
}

/// The properties out of that statement. The FE prints a `HashMap`, so the
/// ORDER is unstable and the text must never be diffed — only the pairs.
pub fn parse_show_create_properties(text: &str) -> BTreeMap<String, String> {
    static PAIR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PAIR.get_or_init(|| {
        // "key" = "value", with \" escapes inside either side.
        regex::Regex::new(r#""((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)""#)
            .expect("static property pattern compiles")
    });
    let body = match (text.find("PROPERTIES ("), text.rfind(')')) {
        (Some(start), Some(end)) if end > start => &text[start..=end],
        _ => return BTreeMap::new(),
    };
    re.captures_iter(body)
        .map(|c| (unescape_dq(&c[1]), unescape_dq(&c[2])))
        .collect()
}

/// The inverse of [`escape_dq`], for values read back off the FE.
fn unescape_dq(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drift {
    Absent,
    Matching,
    /// The keys egret would set that the FE disagrees about, sorted.
    Different(Vec<String>),
}

/// Keys that can never be compared, because the FE does not return what it
/// holds (`CredentialUtil.java:38-91`): credentials come back as
/// `xx******yy` (or `******` when ≤ 4 chars), `password` is deleted outright,
/// and `checksum` is the FE's own MD5 of `driver_url`.
///
/// This is the whole of the D3 addendum: drift comparison is blind to
/// secrets. Egret cannot tell a rotated credential from an unchanged one by
/// reading the FE back — `SHOW CREATE CATALOG` never returns enough to say —
/// so a real rotation never shows up as drift here. That is exactly why
/// re-applying an edited catalog always issues `ALTER` rather than trusting
/// `Drift::Matching` to mean "nothing to send" for a secret-valued key.
pub const FE_UNCOMPARABLE_KEYS: [&str; 3] = ["password", "checksum", "azure_path_key"];

/// Compare what egret would render against what `SHOW CREATE CATALOG` last
/// returned. Only [`StarRocksCatalogDdl::comparable`]'s plain keys are ever
/// looked at — a secret-valued property is never in that map to begin with —
/// and [`FE_UNCOMPARABLE_KEYS`] additionally excludes any plain key the FE is
/// known to mask or delete. `Drift::Absent` is NOT produced here: a name the
/// FE's `SHOW CATALOGS` does not list is absent whatever `SHOW CREATE
/// CATALOG` would say, so that verdict belongs to the caller that has both
/// reads, not to this comparison alone.
pub fn compare_to_fe(rendered: &StarRocksCatalogDdl, fe: &BTreeMap<String, String>) -> Drift {
    let mut differing: Vec<String> = rendered
        .comparable() // Plain values only: secrets are never comparable
        .into_iter()
        .filter(|(k, _)| !FE_UNCOMPARABLE_KEYS.contains(&k.as_str()))
        .filter(|(k, v)| match fe.get(k) {
            None => true,
            Some(actual) => actual != v && !actual.contains("******"),
        })
        .map(|(k, _)| k)
        .collect();
    differing.sort();
    if differing.is_empty() {
        Drift::Matching
    } else {
        Drift::Different(differing)
    }
}

/// A StarRocks FE that lives in memory. Public because the route tests are an
/// external crate; it is the whole of design D8 — without it, every statement
/// egret builds could only be exercised against a real MySQL port, which is
/// why five of the bootstrap's six guard checkpoints used to be pinned by a
/// test that grepped the function's own source.
///
/// It imitates the FE behaviours the apply path actually depends on, and no
/// others: a second `CREATE` errors with `already exists`, `ALTER` MERGES and
/// cannot unset, `DROP … IF EXISTS` on an unknown catalog succeeds, and
/// `SHOW CREATE CATALOG` masks credentials and DELETES `password`, exactly as
/// `CredentialUtil` does. Imitating the masking is the point: a fake that
/// echoed back what was written would make the drift comparison look correct
/// while it silently reported permanent drift against a real FE.
#[derive(Clone, Default)]
pub struct FakeStarRocksDdl {
    executed: Arc<Mutex<Vec<String>>>,
    catalogs: Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>,
    /// When set, every `execute`/`query` fails with this message.
    fail: Arc<Mutex<Option<String>>>,
    /// `(needle, message)`: fail only the statements containing `needle`.
    selective_fail: Arc<Mutex<Vec<(String, String)>>>,
    /// The `FeTarget` of the most recent call, so a test can assert the cert.
    last_target: Arc<Mutex<Option<FeTarget>>>,
    #[allow(clippy::type_complexity)]
    on_create: Arc<
        Mutex<
            Option<
                Arc<dyn Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>,
            >,
        >,
    >,
}

impl FakeStarRocksDdl {
    pub fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
    pub fn clear_executed(&self) {
        self.executed.lock().unwrap().clear()
    }
    /// Catalog names in the order `CREATE EXTERNAL CATALOG` ran for them.
    pub fn created(&self) -> Vec<String> {
        self.executed()
            .iter()
            .filter_map(|s| statement_target("CREATE EXTERNAL CATALOG", s))
            .collect()
    }
    /// Every call fails with this message until it is cleared.
    pub fn set_fail(&self, msg: Option<&str>) {
        *self.fail.lock().unwrap() = msg.map(str::to_string);
    }
    /// Fail only the statements naming this catalog — one bad catalog among
    /// several, which is what the bootstrap loop has to survive.
    pub fn fail_catalog(&self, name: &str, msg: &str) {
        self.fail_matching(&format!("`{}`", escape_ident(name)), msg)
    }
    /// Fail only the statements containing this substring (the `already
    /// exists` branch of the delegation bootstrap).
    pub fn fail_matching(&self, needle: &str, msg: &str) {
        self.selective_fail
            .lock()
            .unwrap()
            .push((needle.to_string(), msg.to_string()));
    }
    /// The `FeTarget` of the most recent call — how a test proves the pinned
    /// certificate really reaches every statement (D6).
    pub fn last_target(&self) -> FeTarget {
        self.last_target
            .lock()
            .unwrap()
            .clone()
            .expect("a call was made")
    }
    /// Run this when a `CREATE EXTERNAL CATALOG` is executed, before it is
    /// applied — the only way to make something happen BETWEEN two iterations
    /// of the bootstrap's loop (deleting the engine, say).
    pub fn on_create(
        &self,
        hook: impl Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) {
        *self.on_create.lock().unwrap() = Some(Arc::new(hook));
    }
    pub fn catalog(&self, name: &str) -> Option<BTreeMap<String, String>> {
        self.catalogs.lock().unwrap().get(name).cloned()
    }
    /// Seed a catalog egret did not create — a hand-made `egret`, or one left
    /// by a pre-SP3a engine. This is how the drift test builds its premise.
    pub fn seed(&self, name: &str, props: &[(&str, &str)]) {
        self.catalogs.lock().unwrap().insert(
            name.to_string(),
            props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
    }

    /// Remove a catalog WITHOUT recording a statement — an FE that lost it to
    /// someone else's `DROP`, which is what detach must tolerate (D7).
    pub fn forget(&self, name: &str) {
        self.catalogs.lock().unwrap().remove(name);
    }
}

#[async_trait::async_trait]
impl StarRocksDdl for FakeStarRocksDdl {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), String> {
        *self.last_target.lock().unwrap() = Some(fe.clone());
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(msg);
        }
        if let Some(msg) = self
            .selective_fail
            .lock()
            .unwrap()
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, msg)| msg.clone())
        {
            return Err(msg);
        }
        self.executed.lock().unwrap().push(sql.to_string());
        if let Some(name) = statement_target("CREATE EXTERNAL CATALOG", sql) {
            let hook = self.on_create.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(name.clone()).await;
            }
        }
        let mut cats = self.catalogs.lock().unwrap();
        if let Some(name) = statement_target("CREATE EXTERNAL CATALOG", sql) {
            if cats.contains_key(&name) {
                return Err(format!("Catalog '{name}' already exists"));
            }
            cats.insert(name, parse_show_create_properties(sql));
        } else if let Some(name) = statement_target("ALTER CATALOG", sql) {
            // Merge, never replace — CatalogMgr.java:212-219. An unknown
            // catalog is a SILENT no-op (CatalogMgr.java:184-187).
            if let Some(existing) = cats.get_mut(&name) {
                existing.extend(parse_show_create_properties(sql));
            }
        } else if let Some(name) = statement_target("DROP CATALOG IF EXISTS", sql) {
            cats.remove(&name);
        }
        Ok(())
    }

    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        *self.last_target.lock().unwrap() = Some(fe.clone());
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(msg);
        }
        if sql == "SELECT 1" {
            return Ok(vec![vec![Some("1".into())]]);
        }
        if sql == "SHOW CATALOGS" {
            return Ok(self
                .catalogs
                .lock()
                .unwrap()
                .keys()
                .map(|n| vec![Some(n.clone()), Some("External".into()), None])
                .collect());
        }
        if let Some(name) = statement_target("SHOW CREATE CATALOG", sql) {
            let Some(props) = self.catalogs.lock().unwrap().get(&name).cloned() else {
                return Err(format!("Unknown catalog '{name}'"));
            };
            return Ok(vec![vec![
                Some(name.clone()),
                Some(show_create_text(&name, &props)),
            ]]);
        }
        Ok(vec![])
    }
}

/// The backtick-quoted name out of a statement that starts with `prefix`,
/// with doubled backticks undone — the inverse of what [`escape_ident`] wrote.
fn statement_target(prefix: &str, sql: &str) -> Option<String> {
    let rest = sql.strip_prefix(prefix)?.trim_start();
    let rest = rest.strip_prefix('`')?;
    let mut name = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '`' if chars.clone().next() == Some('`') => {
                chars.next();
                name.push('`');
            }
            '`' => return Some(name),
            c => name.push(c),
        }
    }
    None
}

/// What the FE would print back: the same `"k" = "v"` shape, with
/// `CredentialUtil`'s masking applied — `password` and `azure_path_key`
/// DELETED (`CredentialUtil.java:88,91`), the listed credential keys masked as
/// first-two + `******` + last-two, or `******` when the value is ≤ 4 chars
/// (`CredentialUtil.java:86-103`).
fn show_create_text(name: &str, props: &BTreeMap<String, String>) -> String {
    const MASKED: [&str; 6] = [
        "aws.s3.access_key",
        "aws.s3.secret_key",
        "aws.glue.access_key",
        "aws.glue.secret_key",
        "iceberg.catalog.oauth2.credential",
        "iceberg.catalog.jdbc.password",
    ];
    let body: Vec<String> = props
        .iter()
        .filter(|(k, _)| k.as_str() != "password" && k.as_str() != "azure_path_key")
        .map(|(k, v)| {
            let shown = if MASKED.contains(&k.as_str()) {
                if v.chars().count() <= 4 {
                    "******".to_string()
                } else {
                    let c: Vec<char> = v.chars().collect();
                    format!("{}{}{}", c[0], c[1], "******")
                        + &c[c.len() - 2..].iter().collect::<String>()
                }
            } else {
                v.clone()
            };
            format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(&shown))
        })
        .collect();
    format!(
        "CREATE EXTERNAL CATALOG `{}`\nPROPERTIES ({}\n)",
        escape_ident(name),
        body.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_catalog_registry::PropValue;
    use uuid::Uuid;

    /// The shape `SHOW CREATE CATALOG` really returns: the statement text with
    /// `"k" = "v"` pairs, `,\n` between them, `\"` escaped inside values, and
    /// NO stable order (the FE prints a HashMap — `PrintableMap`,
    /// `ShowExecutor.java:3079-3081`). So egret parses the pairs; it never
    /// diffs the text.
    const SHOW_CREATE: &str = "CREATE EXTERNAL CATALOG `sales`\nPROPERTIES (\"type\" = \"jdbc\",\n\"user\" = \"u\",\n\"jdbc_uri\" = \"jdbc:postgresql://db:5432/sales?sslmode=require\",\n\"driver_class\" = \"org.postgresql.Driver\",\n\"checksum\" = \"2b1c…\",\n\"aws.s3.access_key\" = \"AK******LE\"\n)";

    /// The `StarRocksCatalogDdl` the constant above describes: a jdbc catalog
    /// named `sales` with a plain `type`/`user`/`jdbc_uri`/`driver_class` and a
    /// secret-valued `password` — hand-built rather than run through
    /// `render_starrocks`, because this module tests `compare_to_fe` in
    /// isolation from rendering.
    fn jdbc_ddl() -> StarRocksCatalogDdl {
        let mut properties = BTreeMap::new();
        properties.insert("type".to_string(), PropValue::Plain("jdbc".into()));
        properties.insert("user".to_string(), PropValue::Plain("u".into()));
        properties.insert(
            "jdbc_uri".to_string(),
            PropValue::Plain("jdbc:postgresql://db:5432/sales?sslmode=require".into()),
        );
        properties.insert(
            "driver_class".to_string(),
            PropValue::Plain("org.postgresql.Driver".into()),
        );
        properties.insert("password".to_string(), PropValue::Secret(Uuid::nil()));
        StarRocksCatalogDdl {
            name: "sales".into(),
            properties,
        }
    }

    #[test]
    fn properties_are_parsed_out_of_the_statement_the_fe_prints() {
        let p = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(p["type"], "jdbc");
        assert_eq!(p["user"], "u");
        assert_eq!(
            p["jdbc_uri"],
            "jdbc:postgresql://db:5432/sales?sslmode=require"
        );
        assert_eq!(p["checksum"], "2b1c…");
        assert_eq!(p.len(), 6);
        // An escaped quote inside a value does not end it.
        let p = parse_show_create_properties("PROPERTIES (\"a\" = \"x\\\"y\", \"b\" = \"z\")");
        assert_eq!(p["a"], "x\"y");
        assert_eq!(p["b"], "z");
    }

    #[test]
    fn drift_is_decided_on_the_comparable_properties_only() {
        let rendered = jdbc_ddl(); // the same catalog the constant above describes
        let fe = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);

        // `checksum` is the FE's own (`JDBCConnector.java:74-100`) and must not
        // count as drift; a masked credential must not either.
        let mut fe2 = fe.clone();
        fe2.insert("checksum".into(), "different".into());
        fe2.insert("aws.s3.access_key".into(), "ZZ******QQ".into());
        assert_eq!(compare_to_fe(&rendered, &fe2), Drift::Matching);

        // A real difference names the key, and only that key.
        let mut fe3 = fe.clone();
        fe3.insert("user".into(), "someone_else".into());
        assert_eq!(
            compare_to_fe(&rendered, &fe3),
            Drift::Different(vec!["user".into()])
        );

        // A key egret sets that the FE does not have is drift too.
        let mut fe4 = fe.clone();
        fe4.remove("jdbc_uri");
        assert_eq!(
            compare_to_fe(&rendered, &fe4),
            Drift::Different(vec!["jdbc_uri".into()])
        );
    }

    /// `password` is not masked — it is DELETED from the output
    /// (`CredentialUtil.java:88,91`), and so are our own secret-valued keys by
    /// construction. A rotated credential is therefore INVISIBLE to drift
    /// detection, which is exactly why re-apply always ALTERs (D3).
    #[test]
    fn a_secret_valued_key_is_never_compared() {
        let rendered = jdbc_ddl();
        let mut fe = parse_show_create_properties(SHOW_CREATE);
        assert!(!fe.contains_key("password"));
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
        fe.insert("password".into(), "whatever".into());
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
    }
}
