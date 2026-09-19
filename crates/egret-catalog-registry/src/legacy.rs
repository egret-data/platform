//! Classification of an `engine_mgmt.engine_catalogs` row (or a legacy
//! `POST/PUT /engines/{id}/catalogs` body) into a registry definition.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;
use uuid::Uuid;

use crate::config::{
    Advanced, CatalogConfig, CatalogDef, EngineType, GenericConfig, JdbcConfig, JdbcDialect,
    JdbcSsl, SampleConfig, SecretProperties, ValidationError,
};
use crate::render::trino::{render_trino, TrinoRenderInput};
use crate::sensitive::is_plaintext_secret;

/// A legacy catalog as stored in `engine_mgmt.engine_catalogs` or posted to the
/// legacy engine-catalog endpoints.
#[derive(Clone, Copy, Debug)]
pub struct LegacyCatalog<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub config: &'a Value,
    pub secret_ref: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassifyOutcome {
    Typed,
    Generic { reason: String },
}

#[derive(Clone, Debug)]
pub struct Classified {
    pub def: CatalogDef,
    pub outcome: ClassifyOutcome,
}

/// `(connector name, plain properties, secret-backed properties)`.
type LegacyPropsAndSecrets = (String, Vec<(String, String)>, Vec<(String, Uuid)>);

/// The legacy `(connector name, plain properties, secret-backed properties)`
/// an old `engine_catalogs` row (or legacy endpoint body) renders today.
/// Moved verbatim from `egret-engine-mgmt::engines::catalog_props_and_secrets`.
pub fn legacy_props_and_secrets(
    kind: &str,
    config: &Value,
    secret_ref: Option<Uuid>,
) -> LegacyPropsAndSecrets {
    let cfg = config;
    // ── legacy postgres (kind == "postgresql", config.host present, no v2 properties)
    if kind == "postgresql" && cfg.get("properties").is_none() && cfg.get("host").is_some() {
        let mut plain = vec![
            (
                "connection-url".to_string(),
                format!(
                    "jdbc:postgresql://{}:{}/{}",
                    cfg["host"].as_str().unwrap_or_default(),
                    cfg["port"].as_i64().unwrap_or(5432),
                    cfg["database"].as_str().unwrap_or_default()
                ),
            ),
            (
                "connection-user".to_string(),
                cfg["user"].as_str().unwrap_or_default().to_string(),
            ),
        ];
        let mut secrets = vec![];
        if let Some(id) = secret_ref {
            secrets.push(("connection-password".to_string(), id));
        } else {
            plain.push(("connection-password".to_string(), String::new()));
        }
        return ("postgresql".to_string(), plain, secrets);
    }
    // ── connector name: v2 uses kind; legacy generic used config.connectorName
    let connector = if kind == "generic" {
        cfg["connectorName"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    } else {
        kind.to_string()
    };
    // ── secret-backed properties
    let mut secrets: Vec<(String, Uuid)> = vec![];
    // v2: config.secretProperties = { prop: "<uuid>" }
    if let Some(obj) = cfg.get("secretProperties").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(id) = v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                secrets.push((k.clone(), id));
            }
        }
    }
    // legacy generic single-secret: config.secretProperty + secret_ref,
    // defaulting the property name to "connection-password" (matching the
    // old generic arm's default) when the row predates `secretProperty`.
    if secrets.is_empty() && kind == "generic" {
        if let Some(id) = secret_ref {
            let prop = cfg
                .get("secretProperty")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("connection-password")
                .to_string();
            secrets.push((prop, id));
        }
    }
    let secret_keys: std::collections::HashSet<&str> =
        secrets.iter().map(|(k, _)| k.as_str()).collect();
    // ── plaintext properties (skip any key that is secret-backed)
    let mut plain: Vec<(String, String)> = vec![];
    if let Some(obj) = cfg.get("properties").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if secret_keys.contains(k.as_str()) {
                continue;
            }
            plain.push((k.clone(), v.as_str().unwrap_or_default().to_string()));
        }
    }
    (connector, plain, secrets)
}

/// Plaintext secrets inside `properties`: each must become a named secret
/// before [`classify`] runs (pass the new ids as `extracted`).
pub fn plaintext_secrets(legacy: &LegacyCatalog<'_>) -> Vec<(String, String)> {
    let (_, plain, _) = legacy_props_and_secrets(legacy.kind, legacy.config, legacy.secret_ref);
    plain
        .into_iter()
        .filter(|(k, v)| is_plaintext_secret(k, v))
        .collect()
}

pub fn classify(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> Result<Classified, ValidationError> {
    let (want_plain, want_secret) = legacy_target(legacy, extracted);

    let reason = match typed_candidate(&want_plain, &want_secret) {
        Ok(def)
            if def.validate().is_ok()
                && round_trips(legacy.name, &def, &want_plain, &want_secret) =>
        {
            return Ok(Classified {
                def,
                outcome: ClassifyOutcome::Typed,
            });
        }
        Ok(_) => "the typed form would not render the same Trino properties".to_string(),
        Err(reason) => reason,
    };

    Ok(Classified {
        def: generic_def(want_plain, want_secret)?,
        outcome: ClassifyOutcome::Generic { reason },
    })
}

/// The legacy catalog as a `generic` Trino catalog, without trying a typed
/// form — what a legacy `PUT` of a catalog that is already generic keeps.
pub fn classify_generic(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> Result<CatalogDef, ValidationError> {
    let (want_plain, want_secret) = legacy_target(legacy, extracted);
    generic_def(want_plain, want_secret)
}

/// What the engine renders today: `connector.name` first, then the plain
/// properties (which may override it, exactly as the old renderer did), and
/// every secret-backed property.
fn legacy_target(
    legacy: &LegacyCatalog<'_>,
    extracted: &BTreeMap<String, Uuid>,
) -> (BTreeMap<String, String>, BTreeMap<String, Uuid>) {
    let (connector, plain, secrets) =
        legacy_props_and_secrets(legacy.kind, legacy.config, legacy.secret_ref);
    let mut want_plain: BTreeMap<String, String> = BTreeMap::new();
    want_plain.insert("connector.name".into(), connector);
    for (k, v) in plain {
        if !extracted.contains_key(&k) {
            want_plain.insert(k, v);
        }
    }
    let mut want_secret: BTreeMap<String, Uuid> = secrets.into_iter().collect();
    want_secret.extend(extracted.iter().map(|(k, v)| (k.clone(), *v)));
    (want_plain, want_secret)
}

fn generic_def(
    want_plain: BTreeMap<String, String>,
    want_secret: BTreeMap<String, Uuid>,
) -> Result<CatalogDef, ValidationError> {
    let def = CatalogDef {
        config: CatalogConfig::Generic(GenericConfig {
            engines: [EngineType::Trino].into_iter().collect(),
            secret_properties: SecretProperties {
                trino: want_secret,
                starrocks: BTreeMap::new(),
            },
        }),
        advanced: Advanced {
            trino: want_plain,
            starrocks: BTreeMap::new(),
        },
    };
    def.validate()?;
    Ok(def)
}

fn round_trips(
    name: &str,
    def: &CatalogDef,
    want_plain: &BTreeMap<String, String>,
    want_secret: &BTreeMap<String, Uuid>,
) -> bool {
    let input = TrinoRenderInput {
        env_prefix: "R",
        egret_url: "",
        warehouse: None,
        storage: None,
        credential_secret_id: None,
    };
    match render_trino(name, def, &input) {
        Ok(file) => {
            let (plain, secret) = file.split_secret_properties();
            &plain == want_plain && &secret == want_secret
        }
        Err(_) => false,
    }
}

fn dialect_of(connector: &str) -> Option<JdbcDialect> {
    Some(match connector {
        "postgresql" => JdbcDialect::Postgresql,
        "mysql" => JdbcDialect::Mysql,
        "mariadb" => JdbcDialect::Mariadb,
        "oracle" => JdbcDialect::Oracle,
        "sqlserver" => JdbcDialect::Sqlserver,
        "clickhouse" => JdbcDialect::Clickhouse,
        _ => return None,
    })
}

fn typed_candidate(
    plain: &BTreeMap<String, String>,
    secret: &BTreeMap<String, Uuid>,
) -> Result<CatalogDef, String> {
    let connector = plain
        .get("connector.name")
        .map(String::as_str)
        .unwrap_or("");
    let rest = |skip: &[&str]| -> BTreeMap<String, String> {
        plain
            .iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    if connector == "tpch" || connector == "tpcds" {
        if !secret.is_empty() {
            return Err("carries secret properties the typed form has no field for".into());
        }
        let config = if connector == "tpch" {
            CatalogConfig::Tpch(SampleConfig::default())
        } else {
            CatalogConfig::Tpcds(SampleConfig::default())
        };
        let advanced = Advanced {
            trino: rest(&["connector.name"]),
            starrocks: BTreeMap::new(),
        };
        return Ok(CatalogDef { config, advanced });
    }
    let Some(dialect) = dialect_of(connector) else {
        return Err(format!("no typed form for connector {connector:?}"));
    };
    let url = plain.get("connection-url").ok_or("no connection-url")?;
    let user = plain.get("connection-user").ok_or("no connection-user")?;
    if secret.len() != 1 || !secret.contains_key("connection-password") {
        return Err("the typed form needs exactly one secret, connection-password".into());
    }
    let (host, port, database, ssl) = parse_jdbc_url(dialect, url)
        .ok_or("connection-url is not in the canonical form egret renders")?;
    let advanced = Advanced {
        trino: rest(&["connector.name", "connection-url", "connection-user"]),
        starrocks: BTreeMap::new(),
    };
    Ok(CatalogDef {
        config: CatalogConfig::Jdbc(JdbcConfig {
            dialect,
            host,
            port,
            database,
            user: user.clone(),
            password_secret_id: secret["connection-password"],
            ssl,
        }),
        advanced,
    })
}

fn caps<'u>(
    cell: &'static OnceLock<regex::Regex>,
    pattern: &str,
    url: &'u str,
) -> Option<regex::Captures<'u>> {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("static jdbc pattern"))
        .captures(url)
}

/// Inverse of [`crate::jdbc_url`] for the canonical forms only; anything else is
/// `None` and the row stays generic. The round-trip check in [`classify`] is the
/// real gate, so a too-lenient pattern here cannot change what Trino sees.
fn parse_jdbc_url(
    dialect: JdbcDialect,
    url: &str,
) -> Option<(String, u16, Option<String>, JdbcSsl)> {
    static PG: OnceLock<regex::Regex> = OnceLock::new();
    static MY: OnceLock<regex::Regex> = OnceLock::new();
    static MA: OnceLock<regex::Regex> = OnceLock::new();
    static OR: OnceLock<regex::Regex> = OnceLock::new();
    static SS: OnceLock<regex::Regex> = OnceLock::new();
    static CH: OnceLock<regex::Regex> = OnceLock::new();
    let verify = JdbcSsl::VerifyFull { ca_secret_id: None };
    let (c, ssl) = match dialect {
        JdbcDialect::Postgresql => {
            let c = caps(
                &PG,
                r"^jdbc:postgresql://(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]+)\?sslmode=(?P<s>disable|require|verify-full)$",
                url,
            )?;
            let ssl = match &c["s"] {
                "disable" => JdbcSsl::Disable,
                "require" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Mysql => {
            let c = caps(
                &MY,
                r"^jdbc:mysql://(?P<h>[^:/?]+):(?P<p>\d+)\?sslMode=(?P<s>DISABLED|REQUIRED|VERIFY_IDENTITY)$",
                url,
            )?;
            let ssl = match &c["s"] {
                "DISABLED" => JdbcSsl::Disable,
                "REQUIRED" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Mariadb => {
            let c = caps(
                &MA,
                r"^jdbc:mariadb://(?P<h>[^:/?]+):(?P<p>\d+)\?sslMode=(?P<s>disable|trust|verify-full)$",
                url,
            )?;
            let ssl = match &c["s"] {
                "disable" => JdbcSsl::Disable,
                "trust" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Oracle => {
            let c = caps(
                &OR,
                r"^jdbc:oracle:thin:@(?P<s>//|tcps://)(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]+)(?P<dn>\?oracle\.net\.ssl_server_dn_match=true)?$",
                url,
            )?;
            let ssl = match (&c["s"], c.name("dn").is_some()) {
                ("//", _) => JdbcSsl::Disable,
                (_, false) => JdbcSsl::Require,
                (_, true) => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Sqlserver => {
            let c = caps(
                &SS,
                r"^jdbc:sqlserver://(?P<h>[^:;]+):(?P<p>\d+);databaseName=(?P<d>[^;]+);(?P<s>encrypt=false|encrypt=true;trustServerCertificate=true|encrypt=true;trustServerCertificate=false)$",
                url,
            )?;
            let ssl = match &c["s"] {
                "encrypt=false" => JdbcSsl::Disable,
                "encrypt=true;trustServerCertificate=true" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
        JdbcDialect::Clickhouse => {
            let c = caps(
                &CH,
                r"^jdbc:clickhouse://(?P<h>[^:/?]+):(?P<p>\d+)/(?P<d>[^?]*)\?(?P<s>ssl=false|ssl=true&sslmode=none|ssl=true&sslmode=strict)$",
                url,
            )?;
            let ssl = match &c["s"] {
                "ssl=false" => JdbcSsl::Disable,
                "ssl=true&sslmode=none" => JdbcSsl::Require,
                _ => verify,
            };
            (c, ssl)
        }
    };
    let host = c.name("h")?.as_str().to_string();
    let port = c.name("p")?.as_str().parse().ok()?;
    let database = c
        .name("d")
        .map(|m| m.as_str().to_string())
        .filter(|d| !d.is_empty());
    Some((host, port, database, ssl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogConfig, JdbcSsl};
    use serde_json::json;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn legacy<'a>(kind: &'a str, config: &'a Value, secret_ref: Option<Uuid>) -> LegacyCatalog<'a> {
        LegacyCatalog {
            name: "cat",
            kind,
            config,
            secret_ref,
        }
    }

    #[test]
    fn a_canonical_postgres_row_becomes_typed_jdbc() {
        let cfg = json!({
            "properties": { "connection-url": "jdbc:postgresql://db:5432/sales?sslmode=require", "connection-user": "reader" },
            "secretProperties": { "connection-password": id(7).to_string() }
        });
        let c = classify(&legacy("postgresql", &cfg, None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        let CatalogConfig::Jdbc(j) = &c.def.config else {
            panic!("{:?}", c.def)
        };
        assert_eq!(
            (j.host.as_str(), j.port, j.database.as_deref()),
            ("db", 5432, Some("sales"))
        );
        assert_eq!(j.ssl, JdbcSsl::Require);
        assert_eq!(j.password_secret_id, id(7));
    }

    #[test]
    fn a_url_without_an_explicit_ssl_mode_stays_generic() {
        let cfg = json!({
            "properties": { "connection-url": "jdbc:postgresql://db:5432/x", "connection-user": "u", "connection-password": "hunter2" }
        });
        let l = legacy("postgresql", &cfg, None);
        assert_eq!(
            plaintext_secrets(&l),
            vec![("connection-password".to_string(), "hunter2".to_string())]
        );
        let extracted: BTreeMap<String, Uuid> = [("connection-password".to_string(), id(9))]
            .into_iter()
            .collect();
        let c = classify(&l, &extracted).unwrap();
        assert!(
            matches!(c.outcome, ClassifyOutcome::Generic { .. }),
            "{:?}",
            c.outcome
        );
        let CatalogConfig::Generic(g) = &c.def.config else {
            panic!()
        };
        assert_eq!(g.secret_properties.trino["connection-password"], id(9));
        assert_eq!(c.def.advanced.trino["connector.name"], "postgresql");
        assert_eq!(
            c.def.advanced.trino["connection-url"],
            "jdbc:postgresql://db:5432/x"
        );
        assert!(
            !c.def.advanced.trino.contains_key("connection-password"),
            "plaintext never stored"
        );
    }

    #[test]
    fn tpch_rows_become_typed_with_their_extra_properties_in_advanced() {
        let c = classify(&legacy("tpch", &json!({}), None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        assert!(matches!(c.def.config, CatalogConfig::Tpch(_)));
        let cfg = json!({ "properties": { "tpch.column-naming": "STANDARD" } });
        let c = classify(&legacy("tpch", &cfg, None), &BTreeMap::new()).unwrap();
        assert_eq!(c.outcome, ClassifyOutcome::Typed);
        assert_eq!(c.def.advanced.trino["tpch.column-naming"], "STANDARD");
    }

    /// The pre-v2 postgres shape (`host/port/database/user` + `secret_ref`) and
    /// the pre-v2 generic shape (`connectorName` + `secretProperty`) still map.
    #[test]
    fn v1_rows_keep_their_rendered_properties() {
        let cfg = json!({ "host": "pg", "port": 5433, "database": "d", "user": "u" });
        let c = classify(&legacy("postgresql", &cfg, Some(id(3))), &BTreeMap::new()).unwrap();
        let f = crate::render_trino("cat", &c.def, &probe_free()).unwrap();
        let (plain, secret) = f.split_secret_properties();
        assert_eq!(plain["connection-url"], "jdbc:postgresql://pg:5433/d");
        assert_eq!(plain["connection-user"], "u");
        assert_eq!(secret["connection-password"], id(3));

        let cfg = json!({ "connectorName": "mysql", "properties": { "connection-url": "jdbc:mysql://m:3306" }, "secretProperty": "connection-password" });
        let c = classify(&legacy("generic", &cfg, Some(id(4))), &BTreeMap::new()).unwrap();
        assert_eq!(c.def.trino_connector_name(), "mysql");
        let f = crate::render_trino("cat", &c.def, &probe_free()).unwrap();
        assert_eq!(f.split_secret_properties().1["connection-password"], id(4));
    }

    #[test]
    fn unknown_connectors_are_generic_and_flags_are_not_secrets() {
        let cfg = json!({ "properties": {
            "iceberg.catalog.type": "rest",
            "iceberg.rest-catalog.vended-credentials-enabled": "true"
        } });
        let l = legacy("iceberg", &cfg, None);
        assert!(plaintext_secrets(&l).is_empty());
        let c = classify(&l, &BTreeMap::new()).unwrap();
        assert!(matches!(c.outcome, ClassifyOutcome::Generic { .. }));
        assert_eq!(
            c.def.advanced.trino["iceberg.rest-catalog.vended-credentials-enabled"],
            "true"
        );
    }

    #[test]
    fn two_secret_properties_survive() {
        let cfg = json!({ "properties": { "a.user": "x" }, "secretProperties": { "a.password": id(1).to_string(), "b.token": id(2).to_string() } });
        let c = classify(&legacy("bigquery", &cfg, None), &BTreeMap::new()).unwrap();
        let CatalogConfig::Generic(g) = &c.def.config else {
            panic!()
        };
        assert_eq!(g.secret_properties.trino.len(), 2);
    }

    fn probe_free() -> crate::TrinoRenderInput<'static> {
        crate::TrinoRenderInput {
            env_prefix: "T",
            egret_url: "",
            warehouse: None,
            storage: None,
            credential_secret_id: None,
        }
    }
}
