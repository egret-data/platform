//! Typed catalog configuration (spec §4.2). `registry.catalogs` stores `kind`,
//! `config` and `advanced` as separate columns; [`CatalogDef::parse`] is the only
//! way in, and it validates. No field of any kind can hold a plaintext secret:
//! secrets are always `*_secret_id` UUIDs, and `advanced` refuses sensitive keys.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::sensitive::is_plaintext_secret;

/// The engines a catalog can be attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EngineType {
    #[serde(rename = "trino")]
    Trino,
    #[serde(rename = "starrocks")]
    StarRocks,
}

impl EngineType {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineType::Trino => "trino",
            EngineType::StarRocks => "starrocks",
        }
    }
    /// The `engine_mgmt.engines.type` value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "trino" => Some(EngineType::Trino),
            "starrocks" => Some(EngineType::StarRocks),
            _ => None,
        }
    }
}

/// `registry.catalogs.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogKind {
    Iceberg,
    Hive,
    DeltaLake,
    Hudi,
    Jdbc,
    Elasticsearch,
    Tpch,
    Tpcds,
    Generic,
}

impl CatalogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogKind::Iceberg => "iceberg",
            CatalogKind::Hive => "hive",
            CatalogKind::DeltaLake => "delta_lake",
            CatalogKind::Hudi => "hudi",
            CatalogKind::Jdbc => "jdbc",
            CatalogKind::Elasticsearch => "elasticsearch",
            CatalogKind::Tpch => "tpch",
            CatalogKind::Tpcds => "tpcds",
            CatalogKind::Generic => "generic",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "iceberg" => CatalogKind::Iceberg,
            "hive" => CatalogKind::Hive,
            "delta_lake" => CatalogKind::DeltaLake,
            "hudi" => CatalogKind::Hudi,
            "jdbc" => CatalogKind::Jdbc,
            "elasticsearch" => CatalogKind::Elasticsearch,
            "tpch" => CatalogKind::Tpch,
            "tpcds" => CatalogKind::Tpcds,
            "generic" => CatalogKind::Generic,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IcebergConfig {
    pub metastore: IcebergMetastore,
    /// Required unless `metastore` is `egret`, which uses its warehouse's storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_connection_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IcebergMetastore {
    /// egret's own warehouse; the catalog IS the warehouse.
    Egret {
        warehouse_id: Uuid,
    },
    Rest {
        uri: String,
        warehouse: String,
        auth: RestAuth,
    },
    Hive {
        uri: String,
    },
    Glue {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_key_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestAuth {
    #[serde(rename = "none")]
    NoAuth,
    Oauth2 {
        credential_secret_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_uri: Option<String>,
    },
}

/// `hive`, `delta_lake`, `hudi`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiveFamilyConfig {
    pub metastore: HiveMetastore,
    pub storage_connection_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HiveMetastore {
    Hive {
        uri: String,
    },
    Glue {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_key_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JdbcDialect {
    Postgresql,
    Mysql,
    Mariadb,
    Oracle,
    Sqlserver,
    Clickhouse,
}

impl JdbcDialect {
    /// Also the Trino connector name.
    pub fn as_str(self) -> &'static str {
        match self {
            JdbcDialect::Postgresql => "postgresql",
            JdbcDialect::Mysql => "mysql",
            JdbcDialect::Mariadb => "mariadb",
            JdbcDialect::Oracle => "oracle",
            JdbcDialect::Sqlserver => "sqlserver",
            JdbcDialect::Clickhouse => "clickhouse",
        }
    }

    /// The StarRocks JDBC `schema_resolver` for this dialect. The FE has no
    /// `mariadb` resolver — `mariadb` (and `mysql`, which StarRocks always
    /// treats as MariaDB, see `render::starrocks`) both use `"mysql"`
    /// (`JDBCMetadata.java:95-96,168-184`: `SUPPORTED_SCHEMA_RESOLVERS` lists
    /// `postgresql, mysql, oracle, sqlserver, clickhouse` — no `mariadb`).
    pub fn starrocks_schema_resolver(self) -> &'static str {
        match self {
            JdbcDialect::Postgresql => "postgresql",
            JdbcDialect::Mysql | JdbcDialect::Mariadb => "mysql",
            JdbcDialect::Oracle => "oracle",
            JdbcDialect::Sqlserver => "sqlserver",
            JdbcDialect::Clickhouse => "clickhouse",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JdbcConfig {
    pub dialect: JdbcDialect,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub user: String,
    pub password_secret_id: Uuid,
    pub ssl: JdbcSsl,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum JdbcSsl {
    Disable,
    Require,
    VerifyFull {
        /// Refused by validation until engines can mount a CA (see plan table #14).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_secret_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticsearchConfig {
    pub hosts: Vec<String>,
    pub port: u16,
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_secret_id: Option<Uuid>,
}

/// `tpch`, `tpcds`. Trino ignores `scale` (schema names carry it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
}

/// Secret-valued properties of a `generic` catalog, per engine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretProperties {
    #[serde(default)]
    pub trino: BTreeMap<String, Uuid>,
    #[serde(default)]
    pub starrocks: BTreeMap<String, Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericConfig {
    pub engines: BTreeSet<EngineType>,
    #[serde(default)]
    pub secret_properties: SecretProperties,
}

/// Free-form properties per engine, rendered after the typed keys (§4.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Advanced {
    #[serde(default)]
    pub trino: BTreeMap<String, String>,
    #[serde(default)]
    pub starrocks: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatalogConfig {
    Iceberg(IcebergConfig),
    Hive(HiveFamilyConfig),
    DeltaLake(HiveFamilyConfig),
    Hudi(HiveFamilyConfig),
    Jdbc(JdbcConfig),
    Elasticsearch(ElasticsearchConfig),
    Tpch(SampleConfig),
    Tpcds(SampleConfig),
    Generic(GenericConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid catalog name {0:?}: use ^[a-z][a-z0-9_]{{0,62}}$")]
    Name(String),
    #[error("unknown catalog kind {0:?}")]
    UnknownKind(String),
    #[error("invalid {kind} configuration: {msg}")]
    Config { kind: String, msg: String },
    #[error("invalid advanced properties: {0}")]
    Advanced(String),
    #[error(
        "advanced.{engine}.{key} looks like a secret; store it as a secret and reference its id"
    )]
    PlaintextSecret { engine: String, key: String },
    #[error("advanced.{engine}.{key} collides with a property the {kind} form sets")]
    Collision {
        engine: String,
        key: String,
        kind: String,
    },
}

fn de<T: DeserializeOwned>(kind: CatalogKind, v: &Value) -> Result<T, ValidationError> {
    serde_json::from_value(v.clone()).map_err(|e| ValidationError::Config {
        kind: kind.as_str().to_string(),
        msg: e.to_string(),
    })
}

impl CatalogConfig {
    pub fn from_json(kind: CatalogKind, v: &Value) -> Result<Self, ValidationError> {
        Ok(match kind {
            CatalogKind::Iceberg => CatalogConfig::Iceberg(de(kind, v)?),
            CatalogKind::Hive => CatalogConfig::Hive(de(kind, v)?),
            CatalogKind::DeltaLake => CatalogConfig::DeltaLake(de(kind, v)?),
            CatalogKind::Hudi => CatalogConfig::Hudi(de(kind, v)?),
            CatalogKind::Jdbc => CatalogConfig::Jdbc(de(kind, v)?),
            CatalogKind::Elasticsearch => CatalogConfig::Elasticsearch(de(kind, v)?),
            CatalogKind::Tpch => CatalogConfig::Tpch(de(kind, v)?),
            CatalogKind::Tpcds => CatalogConfig::Tpcds(de(kind, v)?),
            CatalogKind::Generic => CatalogConfig::Generic(de(kind, v)?),
        })
    }

    pub fn kind(&self) -> CatalogKind {
        match self {
            CatalogConfig::Iceberg(_) => CatalogKind::Iceberg,
            CatalogConfig::Hive(_) => CatalogKind::Hive,
            CatalogConfig::DeltaLake(_) => CatalogKind::DeltaLake,
            CatalogConfig::Hudi(_) => CatalogKind::Hudi,
            CatalogConfig::Jdbc(_) => CatalogKind::Jdbc,
            CatalogConfig::Elasticsearch(_) => CatalogKind::Elasticsearch,
            CatalogConfig::Tpch(_) => CatalogKind::Tpch,
            CatalogConfig::Tpcds(_) => CatalogKind::Tpcds,
            CatalogConfig::Generic(_) => CatalogKind::Generic,
        }
    }

    pub fn to_json(&self) -> Value {
        let v = match self {
            CatalogConfig::Iceberg(c) => serde_json::to_value(c),
            CatalogConfig::Hive(c) | CatalogConfig::DeltaLake(c) | CatalogConfig::Hudi(c) => {
                serde_json::to_value(c)
            }
            CatalogConfig::Jdbc(c) => serde_json::to_value(c),
            CatalogConfig::Elasticsearch(c) => serde_json::to_value(c),
            CatalogConfig::Tpch(c) | CatalogConfig::Tpcds(c) => serde_json::to_value(c),
            CatalogConfig::Generic(c) => serde_json::to_value(c),
        };
        v.expect("typed catalog config always serializes")
    }
}

fn regex(cell: &'static OnceLock<regex::Regex>, pattern: &str) -> &'static regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("static pattern compiles"))
}

fn text(field: &str, v: &str) -> Result<(), String> {
    if v.trim().is_empty() {
        return Err(format!("{field} is required"));
    }
    if v.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(())
}

/// Note for anyone reading `probe_target`: these two validators check the
/// scheme and that a host is present — nothing more. In particular a URI may
/// carry userinfo, and `thrift://hms:9083@evil.example:1234` names the host
/// `evil.example`, not `hms`. That is what the string means under WHATWG and
/// under Java's `URI`, so egret, Trino and StarRocks all dial the same host
/// and there is no divergence to exploit — but the error text below says
/// "thrift://host:port", which promises more than is enforced. Anything that
/// reads a host out of these fields must not assume the shape in the message
/// (SP3c review F8).
fn http_uri(field: &str, v: &str) -> Result<(), String> {
    text(field, v)?;
    if !(v.starts_with("http://") || v.starts_with("https://")) || v.contains(char::is_whitespace) {
        return Err(format!("{field} must be an http(s) URL"));
    }
    // A host is required. `https://` alone passes the prefix check, stores, and
    // then names no endpoint at all — which left the test route telling the
    // operator that the KIND has no endpoint, a different and false statement
    // (SP3c review F2). Refused at the source instead.
    if !has_host(v) {
        return Err(format!("{field} must name a host"));
    }
    Ok(())
}

fn thrift_uri(field: &str, v: &str) -> Result<(), String> {
    text(field, v)?;
    if v.split(',').any(|u| !u.trim().starts_with("thrift://")) || v.contains(char::is_whitespace) {
        return Err(format!(
            "{field} must be thrift://host:port (comma-separated)"
        ));
    }
    if v.split(',').any(|u| !has_host(u.trim())) {
        return Err(format!("{field} must name a host"));
    }
    Ok(())
}

/// Whether `url` parses and carries a non-empty host.
fn has_host(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.host().is_some())
}

fn host(field: &str, v: &str) -> Result<(), String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if !regex(&RE, r"^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$").is_match(v) {
        return Err(format!("{field} must be a host name or address"));
    }
    Ok(())
}

fn database_name(v: &str) -> Result<(), String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if !regex(&RE, r"^[A-Za-z0-9_$.-]+$").is_match(v) {
        return Err("database may contain letters, digits, _ $ . - only".into());
    }
    Ok(())
}

fn property_key(v: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    regex(&RE, r"^[A-Za-z0-9][A-Za-z0-9._-]*$").is_match(v)
}

fn glue(region: &str, access_key: &Option<String>, secret: &Option<Uuid>) -> Result<(), String> {
    text("region", region)?;
    if let Some(a) = access_key {
        text("access_key", a)?;
    }
    if access_key.is_some() != secret.is_some() {
        return Err("access_key and secret_key_secret_id go together".into());
    }
    Ok(())
}

fn validate_config(c: &CatalogConfig) -> Result<(), ValidationError> {
    let kind = c.kind();
    let bad = |msg: String| ValidationError::Config {
        kind: kind.as_str().to_string(),
        msg,
    };
    match c {
        CatalogConfig::Iceberg(i) => {
            let egret = matches!(i.metastore, IcebergMetastore::Egret { .. });
            if egret && i.storage_connection_id.is_some() {
                return Err(bad(
                    "an egret warehouse catalog uses its warehouse's storage; remove storage_connection_id".into(),
                ));
            }
            if !egret && i.storage_connection_id.is_none() {
                return Err(bad("storage_connection_id is required".into()));
            }
            match &i.metastore {
                IcebergMetastore::Egret { .. } => {}
                IcebergMetastore::Rest {
                    uri,
                    warehouse,
                    auth,
                } => {
                    http_uri("uri", uri).map_err(bad)?;
                    text("warehouse", warehouse).map_err(bad)?;
                    if let RestAuth::Oauth2 {
                        scope, token_uri, ..
                    } = auth
                    {
                        if let Some(s) = scope {
                            text("scope", s).map_err(bad)?;
                        }
                        if let Some(t) = token_uri {
                            http_uri("token_uri", t).map_err(bad)?;
                        }
                    }
                }
                IcebergMetastore::Hive { uri } => thrift_uri("uri", uri).map_err(bad)?,
                IcebergMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => glue(region, access_key, secret_key_secret_id).map_err(bad)?,
            }
        }
        CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
            match &h.metastore {
                HiveMetastore::Hive { uri } => thrift_uri("uri", uri).map_err(bad)?,
                HiveMetastore::Glue { .. } if kind == CatalogKind::Hudi => {
                    return Err(bad("Hudi reads a Hive metastore only".into()));
                }
                HiveMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => glue(region, access_key, secret_key_secret_id).map_err(bad)?,
            }
        }
        CatalogConfig::Jdbc(j) => {
            host("host", &j.host).map_err(bad)?;
            if j.port == 0 {
                return Err(bad("port must be 1-65535".into()));
            }
            text("user", &j.user).map_err(bad)?;
            match (j.dialect, &j.database) {
                (JdbcDialect::Postgresql | JdbcDialect::Oracle | JdbcDialect::Sqlserver, None) => {
                    return Err(bad(format!(
                        "database is required for {}",
                        j.dialect.as_str()
                    )));
                }
                (JdbcDialect::Mysql | JdbcDialect::Mariadb, Some(_)) => {
                    return Err(bad(format!(
                        "{} catalogs expose every database as a schema; leave database empty",
                        j.dialect.as_str()
                    )));
                }
                (_, Some(d)) => database_name(d).map_err(bad)?,
                (_, None) => {}
            }
            if let JdbcSsl::VerifyFull {
                ca_secret_id: Some(_),
            } = j.ssl
            {
                return Err(bad(
                    "a custom CA certificate is not supported yet; verify_full uses the engine's default trust store".into(),
                ));
            }
        }
        CatalogConfig::Elasticsearch(e) => {
            if e.hosts.is_empty() {
                return Err(bad("hosts must list at least one host".into()));
            }
            for h in &e.hosts {
                host("hosts[]", h).map_err(bad)?;
            }
            if e.port == 0 {
                return Err(bad("port must be 1-65535".into()));
            }
            if let Some(u) = &e.user {
                text("user", u).map_err(bad)?;
            }
            if e.user.is_some() != e.password_secret_id.is_some() {
                return Err(bad("user and password_secret_id go together".into()));
            }
        }
        CatalogConfig::Tpch(s) | CatalogConfig::Tpcds(s) => {
            if s.scale == Some(0) {
                return Err(bad("scale must be positive".into()));
            }
        }
        CatalogConfig::Generic(g) => {
            if g.engines.is_empty() {
                return Err(bad("engines must name trino, starrocks or both".into()));
            }
        }
    }
    Ok(())
}

fn validate_advanced(def: &CatalogDef) -> Result<(), ValidationError> {
    for (engine, map) in [
        (EngineType::Trino, &def.advanced.trino),
        (EngineType::StarRocks, &def.advanced.starrocks),
    ] {
        for (k, v) in map {
            if !property_key(k) {
                return Err(ValidationError::Advanced(format!(
                    "{}: invalid property key {k:?}",
                    engine.as_str()
                )));
            }
            if v.chars().any(char::is_control) {
                return Err(ValidationError::Advanced(format!(
                    "{}.{k}: value contains a control character",
                    engine.as_str()
                )));
            }
            if is_plaintext_secret(k, v) {
                return Err(ValidationError::PlaintextSecret {
                    engine: engine.as_str().to_string(),
                    key: k.clone(),
                });
            }
        }
    }
    if let CatalogConfig::Generic(g) = &def.config {
        for (engine, required, adv, secrets) in [
            (
                EngineType::Trino,
                "connector.name",
                &def.advanced.trino,
                &g.secret_properties.trino,
            ),
            (
                EngineType::StarRocks,
                "type",
                &def.advanced.starrocks,
                &g.secret_properties.starrocks,
            ),
        ] {
            let e = engine.as_str();
            if g.engines.contains(&engine) {
                if adv.get(required).is_none_or(|v| v.trim().is_empty()) {
                    return Err(ValidationError::Config {
                        kind: "generic".into(),
                        msg: format!("advanced.{e}.{required} is required"),
                    });
                }
            } else if !adv.is_empty() || !secrets.is_empty() {
                return Err(ValidationError::Config {
                    kind: "generic".into(),
                    msg: format!("advanced.{e} or secret_properties.{e} is set but engines does not include {e}"),
                });
            }
            for k in secrets.keys() {
                if !property_key(k) {
                    return Err(ValidationError::Advanced(format!(
                        "{e}: invalid secret property key {k:?}"
                    )));
                }
                if adv.contains_key(k) {
                    return Err(ValidationError::Collision {
                        engine: e.to_string(),
                        key: k.clone(),
                        kind: "generic".into(),
                    });
                }
            }
        }
    }
    // D5: `driver_url`/`driver_class` are the only advanced keys that may
    // override what the typed JDBC form sets (plan table #2), so a
    // half-written override is refused here rather than silently mixing a
    // pinned class with an operator's jar.
    if matches!(def.config, CatalogConfig::Jdbc(_)) {
        let url = def.advanced.starrocks.get("driver_url");
        let class = def.advanced.starrocks.get("driver_class");
        if url.is_some() != class.is_some()
            || url.is_some_and(|v| v.trim().is_empty())
            || class.is_some_and(|v| v.trim().is_empty())
        {
            return Err(ValidationError::Config {
                kind: "jdbc".into(),
                msg: "advanced.starrocks.driver_url and driver_class go together, and neither may be empty".into(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogDef {
    pub config: CatalogConfig,
    pub advanced: Advanced,
}

impl CatalogDef {
    /// Parse and validate the three stored columns.
    pub fn parse(kind: &str, config: &Value, advanced: &Value) -> Result<Self, ValidationError> {
        let kind = CatalogKind::parse(kind)
            .ok_or_else(|| ValidationError::UnknownKind(kind.to_string()))?;
        let config = CatalogConfig::from_json(kind, config)?;
        let advanced = if advanced.is_null() {
            Advanced::default()
        } else {
            serde_json::from_value(advanced.clone())
                .map_err(|e| ValidationError::Advanced(e.to_string()))?
        };
        let def = CatalogDef { config, advanced };
        def.validate()?;
        Ok(def)
    }

    /// The catalog of an egret warehouse.
    pub fn egret_warehouse(warehouse_id: Uuid) -> Self {
        CatalogDef {
            config: CatalogConfig::Iceberg(IcebergConfig {
                metastore: IcebergMetastore::Egret { warehouse_id },
                storage_connection_id: None,
            }),
            advanced: Advanced::default(),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_config(&self.config)?;
        validate_advanced(self)?;
        // What the typed form sets is exactly what rendering produces; probing
        // with inputs that satisfy every kind finds any advanced key that would
        // silently override the form (§4.3). SP3b adds the StarRocks probe.
        use crate::render::starrocks::{render_starrocks, StarRocksRenderInput};
        use crate::render::trino::{render_trino, TrinoRenderInput};
        use crate::render::RenderError;
        for (engine, result) in [
            (
                "trino",
                render_trino("probe", self, &TrinoRenderInput::probe()).map(|_| ()),
            ),
            (
                "starrocks",
                render_starrocks("probe", self, &StarRocksRenderInput::probe()).map(|_| ()),
            ),
        ] {
            match result {
                Err(RenderError::Collision(key)) => {
                    return Err(ValidationError::Collision {
                        engine: engine.into(),
                        key,
                        kind: self.kind().as_str().into(),
                    })
                }
                Err(RenderError::LineBreak(key)) => {
                    return Err(ValidationError::Config {
                        kind: self.kind().as_str().into(),
                        msg: format!("{key} contains a line break"),
                    })
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> CatalogKind {
        self.config.kind()
    }

    pub fn config_json(&self) -> Value {
        self.config.to_json()
    }

    pub fn advanced_json(&self) -> Value {
        serde_json::to_value(&self.advanced).expect("advanced always serializes")
    }

    /// The warehouse an egret catalog is (mirrored into `registry.catalogs.warehouse_id`).
    pub fn warehouse_id(&self) -> Option<Uuid> {
        match &self.config {
            CatalogConfig::Iceberg(IcebergConfig {
                metastore: IcebergMetastore::Egret { warehouse_id },
                ..
            }) => Some(*warehouse_id),
            _ => None,
        }
    }

    /// Mirrored into `registry.catalogs.storage_connection_id`.
    pub fn storage_connection_id(&self) -> Option<Uuid> {
        match &self.config {
            CatalogConfig::Iceberg(i) => i.storage_connection_id,
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                Some(h.storage_connection_id)
            }
            _ => None,
        }
    }

    /// Every secret the configuration references (mirrored into
    /// `registry.catalog_secret_refs`, which blocks deleting them).
    pub fn secret_ids(&self) -> BTreeSet<Uuid> {
        let mut out = BTreeSet::new();
        let glue_secret = |out: &mut BTreeSet<Uuid>, s: &Option<Uuid>| {
            if let Some(id) = s {
                out.insert(*id);
            }
        };
        match &self.config {
            CatalogConfig::Iceberg(i) => match &i.metastore {
                IcebergMetastore::Rest {
                    auth:
                        RestAuth::Oauth2 {
                            credential_secret_id,
                            ..
                        },
                    ..
                } => {
                    out.insert(*credential_secret_id);
                }
                IcebergMetastore::Glue {
                    secret_key_secret_id,
                    ..
                } => glue_secret(&mut out, secret_key_secret_id),
                _ => {}
            },
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                if let HiveMetastore::Glue {
                    secret_key_secret_id,
                    ..
                } = &h.metastore
                {
                    glue_secret(&mut out, secret_key_secret_id)
                }
            }
            CatalogConfig::Jdbc(j) => {
                out.insert(j.password_secret_id);
            }
            CatalogConfig::Elasticsearch(e) => glue_secret(&mut out, &e.password_secret_id),
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) => {}
            CatalogConfig::Generic(g) => {
                out.extend(g.secret_properties.trino.values().copied());
                out.extend(g.secret_properties.starrocks.values().copied());
            }
        }
        out
    }

    /// Whether an engine type can read this catalog (spec §3, design D4).
    ///
    /// Exhaustive on purpose: no `_` arm, so a new kind or a new JDBC dialect
    /// cannot be added without someone deciding, for both engines, whether it
    /// is readable. StarRocks 4.x is the floor egret provisions
    /// (`DEFAULT_SR_FE_IMAGE`), and almost every typed kind is readable by
    /// both engines at that floor — but `tpch`/`tpcds` is now the first pair
    /// whose answer actually DIFFERS per engine, not merely a structural
    /// possibility the exhaustiveness leaves room for: StarRocks' `BENCHMARK`
    /// connector was added by commit `c019a148` (2026-01-22) and, verified
    /// against the vendored StarRocks source, has shipped on no release
    /// branch and no tag — not 4.0, not 4.1, only `main`. So a `benchmark`
    /// catalog fails to create on every FE egret can actually provision, and
    /// `can_read` says so rather than leaving the next engine bump to
    /// discover it live. An engine pinned to an older (or newer, once
    /// `BENCHMARK` ships) FE can still disagree with the platform-wide
    /// answer here; that disagreement arrives as the attachment's
    /// `apply_error`, not as a validation error, because egret cannot know
    /// an overridden image's exact version.
    pub fn can_read(&self, engine: EngineType) -> bool {
        match (&self.config, engine) {
            (CatalogConfig::Generic(g), e) => g.engines.contains(&e),
            (CatalogConfig::Iceberg(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Hive(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::DeltaLake(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Hudi(_), EngineType::Trino | EngineType::StarRocks) => true,
            (CatalogConfig::Elasticsearch(_), EngineType::Trino | EngineType::StarRocks) => true,
            // `BENCHMARK` (StarRocks' tpch/tpcds connector) is `main`-only —
            // see the doc comment above. Trino's tpch/tpcds connectors are
            // unaffected.
            (CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_), EngineType::Trino) => true,
            (CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_), EngineType::StarRocks) => false,
            (CatalogConfig::Jdbc(_), EngineType::Trino) => true,
            (CatalogConfig::Jdbc(j), EngineType::StarRocks) => match j.dialect {
                // v3.0; Oracle/SQL Server v3.2.9/3.3.1; ClickHouse v3.3.0
                // (docs/en/data_source/catalog/jdbc_catalog.md:5,15,21). All
                // below the 4.x floor.
                JdbcDialect::Postgresql
                | JdbcDialect::Mysql
                | JdbcDialect::Mariadb
                | JdbcDialect::Oracle
                | JdbcDialect::Sqlserver
                | JdbcDialect::Clickhouse => true,
            },
        }
    }

    /// The Trino connector this catalog renders to — also the `kind` the legacy
    /// engine-catalog endpoints report for a non-warehouse catalog.
    pub fn trino_connector_name(&self) -> String {
        match &self.config {
            CatalogConfig::Iceberg(_) => "iceberg".into(),
            CatalogConfig::Hive(_) => "hive".into(),
            CatalogConfig::DeltaLake(_) => "delta_lake".into(),
            CatalogConfig::Hudi(_) => "hudi".into(),
            CatalogConfig::Jdbc(j) => j.dialect.as_str().into(),
            CatalogConfig::Elasticsearch(_) => "elasticsearch".into(),
            CatalogConfig::Tpch(_) => "tpch".into(),
            CatalogConfig::Tpcds(_) => "tpcds".into(),
            CatalogConfig::Generic(_) => self
                .advanced
                .trino
                .get("connector.name")
                .cloned()
                .unwrap_or_else(|| "generic".into()),
        }
    }

    /// The StarRocks `"type"` property, or `None` for a catalog StarRocks
    /// cannot read. Note the spellings differ from Trino's connector names:
    /// `deltalake` (no underscore), `es`, `benchmark` for both TPC suites, and
    /// one `jdbc` for every dialect (spec §3).
    pub fn starrocks_catalog_type(&self) -> Option<String> {
        Some(match &self.config {
            CatalogConfig::Iceberg(_) => "iceberg".into(),
            CatalogConfig::Hive(_) => "hive".into(),
            CatalogConfig::DeltaLake(_) => "deltalake".into(),
            CatalogConfig::Hudi(_) => "hudi".into(),
            CatalogConfig::Jdbc(_) => "jdbc".into(),
            CatalogConfig::Elasticsearch(_) => "es".into(),
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) => "benchmark".into(),
            CatalogConfig::Generic(_) => self.advanced.starrocks.get("type")?.clone(),
        })
    }

    pub fn jdbc_dialect(&self) -> Option<JdbcDialect> {
        match &self.config {
            CatalogConfig::Jdbc(j) => Some(j.dialect),
            _ => None,
        }
    }

    /// The host and port a connect-only reachability probe should dial, read
    /// from the STORED configuration — the probe never takes a target from
    /// request input (D4), so this is the only function that decides one.
    ///
    /// `None` means there is nothing to dial. It does NOT on its own mean the
    /// kind has no endpoint: a stored URI that names no host answers `None`
    /// too. Ask [`CatalogDef::names_an_endpoint`] which of the two it is before
    /// telling an operator anything — reporting a broken URI as "this kind has
    /// no endpoint" is false and points at the wrong remedy (SP3c review F2).
    ///
    /// The kinds that genuinely have nothing to dial: an egret warehouse IS
    /// egret, Glue is an AWS region and not a host, `tpch`/`tpcds` generate
    /// their data, and `generic`'s properties are strings egret does not
    /// interpret.
    ///
    /// The match is exhaustive with no wildcard on purpose: a kind added later
    /// must decide, rather than inherit "probe nothing" by omission.
    pub fn probe_target(&self) -> Option<(String, u16)> {
        match &self.config {
            CatalogConfig::Iceberg(IcebergConfig { metastore, .. }) => match metastore {
                IcebergMetastore::Egret { .. } => None,
                IcebergMetastore::Rest { uri, .. } => host_port_from_uri(uri),
                IcebergMetastore::Hive { uri } => host_port_from_uri(uri),
                IcebergMetastore::Glue { .. } => None,
            },
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                match &h.metastore {
                    HiveMetastore::Hive { uri } => host_port_from_uri(uri),
                    HiveMetastore::Glue { .. } => None,
                }
            }
            CatalogConfig::Jdbc(j) => Some((j.host.clone(), j.port)),
            CatalogConfig::Elasticsearch(e) => e.hosts.first().map(|h| (strip_scheme(h), e.port)),
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) | CatalogConfig::Generic(_) => None,
        }
    }

    /// Whether this configuration NAMES a network endpoint at all, regardless
    /// of whether one could be read out of it.
    ///
    /// `probe_target()` returning `None` while this is `true` means the stored
    /// URI is unusable — not that the kind has nothing to dial. The two are
    /// different facts with different remedies ("fix the URI" against "attach
    /// it to an engine"), and blurring them under one sentence is the same
    /// failure the `check` discriminant exists to stop (SP3c review F2).
    ///
    /// Exhaustive with no wildcard, for the same reason `probe_target` is: it
    /// is the other half of the same decision, and the two must be added to
    /// together.
    pub fn names_an_endpoint(&self) -> bool {
        match &self.config {
            CatalogConfig::Iceberg(IcebergConfig { metastore, .. }) => match metastore {
                IcebergMetastore::Egret { .. } | IcebergMetastore::Glue { .. } => false,
                IcebergMetastore::Rest { .. } | IcebergMetastore::Hive { .. } => true,
            },
            CatalogConfig::Hive(h) | CatalogConfig::DeltaLake(h) | CatalogConfig::Hudi(h) => {
                match &h.metastore {
                    HiveMetastore::Hive { .. } => true,
                    HiveMetastore::Glue { .. } => false,
                }
            }
            CatalogConfig::Jdbc(_) | CatalogConfig::Elasticsearch(_) => true,
            CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) | CatalogConfig::Generic(_) => false,
        }
    }
}

/// The port a scheme implies when the stored URI carries none. `thrift` is not
/// a scheme `url` knows a default for, so the Hive metastore's 9083 is named
/// here; `http`/`https` are named too rather than leaning on
/// `Url::port_or_known_default`, so the whole rule reads in one place.
fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme {
        "https" => Some(443),
        "http" => Some(80),
        "thrift" => Some(9083),
        _ => None,
    }
}

/// `thrift://hms:9083` or `https://cat.example/api` → the host and port to
/// dial. A Hive metastore URI may list several, comma-separated
/// (`thrift_uri`'s own rule), and a probe dials one: the first, which is the
/// one the engines try first too.
///
/// The host comes back in the form a socket wants, which for IPv6 means
/// WITHOUT the URI's brackets: `Url::host_str` keeps them, and
/// `TcpStream::connect(("[::1]", p))` neither parses `[::1]` as an address nor
/// resolves it as a name, so a perfectly healthy `thrift://[::1]:9083` read
/// red for ever (SP3c review F1). `Host::Ipv6` is therefore formatted from the
/// parsed address, not from the source text.
fn host_port_from_uri(uri: &str) -> Option<(String, u16)> {
    let first = uri.split(',').next()?.trim();
    let parsed = url::Url::parse(first).ok()?;
    let host = match parsed.host()? {
        url::Host::Domain("") => return None,
        url::Host::Domain(d) => d.to_string(),
        url::Host::Ipv4(a) => a.to_string(),
        url::Host::Ipv6(a) => a.to_string(),
    };
    let port = parsed
        .port()
        .or_else(|| default_port_for_scheme(parsed.scheme()))?;
    Some((host, port))
}

/// The host out of an Elasticsearch `hosts[]` entry. Validation (`host()`)
/// already refuses anything but a bare host name or address, so the scheme and
/// port stripping here is a belt on top of that brace: it keeps a row written
/// before that rule — or by a future looser one — from turning `http://es-1`
/// into a DNS lookup for the literal string `http://es-1`.
fn strip_scheme(h: &str) -> String {
    let rest = h.split_once("://").map_or(h, |(_, r)| r);
    let rest = rest.split('/').next().unwrap_or(rest);
    let host = match rest.rsplit_once(':') {
        // An unbracketed IPv6 literal is all colons and hex digits, so only a
        // BRACKETED host can be carrying a port here (F1's sibling case).
        Some((host, port))
            if !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit())
                && (!host.contains(':') || host.ends_with(']')) =>
        {
            host
        }
        _ => rest,
    };
    match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => v6.to_string(),
        None => host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";

    fn parse(kind: &str, config: serde_json::Value) -> Result<CatalogDef, ValidationError> {
        CatalogDef::parse(kind, &config, &serde_json::Value::Null)
    }

    fn parse_with_advanced(
        kind: &str,
        config: serde_json::Value,
        advanced: serde_json::Value,
    ) -> Result<CatalogDef, ValidationError> {
        CatalogDef::parse(kind, &config, &advanced)
    }

    fn jdbc(dialect: &str, database: Option<&str>) -> serde_json::Value {
        let mut v = json!({
            "dialect": dialect, "host": "db.internal", "port": 5432,
            "user": "reader", "password_secret_id": PW, "ssl": { "mode": "require" }
        });
        if let Some(d) = database {
            v["database"] = json!(d);
        }
        v
    }

    #[test]
    fn every_kind_accepts_its_documented_shape() {
        parse(
            "iceberg",
            json!({ "metastore": { "type": "egret", "warehouse_id": WH } }),
        )
        .unwrap();
        parse("iceberg", json!({
            "metastore": { "type": "rest", "uri": "https://cat.example/api", "warehouse": "lake",
                           "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "PRINCIPAL_ROLE:ALL" } },
            "storage_connection_id": SC
        })).unwrap();
        parse("iceberg", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC })).unwrap();
        parse("hive", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("delta_lake", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKIA", "secret_key_secret_id": PW }, "storage_connection_id": SC })).unwrap();
        parse("hudi", json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC })).unwrap();
        parse("jdbc", jdbc("postgresql", Some("sales"))).unwrap();
        parse("jdbc", jdbc("mysql", None)).unwrap();
        parse("jdbc", jdbc("clickhouse", None)).unwrap();
        parse("elasticsearch", json!({ "hosts": ["es1", "es2"], "port": 9200, "tls": true, "user": "elastic", "password_secret_id": PW })).unwrap();
        parse("tpch", json!({})).unwrap();
        parse("tpcds", json!({ "scale": 10 })).unwrap();
        CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.credentials-key": PW } } }),
            &json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }),
        )
        .unwrap();
    }

    #[test]
    fn unknown_kind_and_unknown_fields_are_refused() {
        assert_eq!(
            parse("mongo", json!({})),
            Err(ValidationError::UnknownKind("mongo".into()))
        );
        let mut v = jdbc("postgresql", Some("sales"));
        v["bogus"] = json!(1);
        assert!(matches!(
            parse("jdbc", v),
            Err(ValidationError::Config { .. })
        ));
        let err = parse(
            "iceberg",
            json!({
                "metastore": { "type": "hive", "uri": "thrift://hms:9083", "extra": true },
                "storage_connection_id": SC
            }),
        );
        assert!(
            matches!(err, Err(ValidationError::Config { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn missing_required_fields_are_refused() {
        let mut v = jdbc("postgresql", Some("sales"));
        v.as_object_mut().unwrap().remove("password_secret_id");
        assert!(parse("jdbc", v).is_err());
        assert!(
            parse("jdbc", jdbc("postgresql", None)).is_err(),
            "postgresql needs a database"
        );
        assert!(
            parse(
                "iceberg",
                json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" } })
            )
            .is_err(),
            "storage required"
        );
        assert!(parse(
            "hive",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" } })
        )
        .is_err());
        assert!(parse(
            "elasticsearch",
            json!({ "hosts": [], "port": 9200, "tls": false })
        )
        .is_err());
    }

    #[test]
    fn a_plaintext_secret_cannot_be_stored() {
        let mut v = jdbc("postgresql", Some("sales"));
        v["password_secret_id"] = json!("hunter2");
        assert!(matches!(
            parse("jdbc", v),
            Err(ValidationError::Config { .. })
        ));
        let err = CatalogDef::parse(
            "tpch",
            &json!({}),
            &json!({ "trino": { "some.password": "hunter2" } }),
        );
        assert_eq!(
            err,
            Err(ValidationError::PlaintextSecret {
                engine: "trino".into(),
                key: "some.password".into()
            })
        );
        CatalogDef::parse(
            "tpch",
            &json!({}),
            &json!({ "trino": { "x.token-refresh-enabled": "true" } }),
        )
        .unwrap();
    }

    #[test]
    fn per_kind_rules() {
        assert!(
            parse("jdbc", jdbc("mysql", Some("shop"))).is_err(),
            "mysql exposes databases as schemas"
        );
        let mut v = jdbc("postgresql", Some("sales"));
        v["ssl"] = json!({ "mode": "verify_full", "ca_secret_id": PW });
        assert!(parse("jdbc", v).is_err(), "custom CA not supported yet");
        let mut v = jdbc("postgresql", Some("sales"));
        v["host"] = json!("db.internal/x?y=1");
        assert!(parse("jdbc", v).is_err(), "host cannot smuggle URL parts");
        assert!(parse("hudi", json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC })).is_err());
        assert!(parse("iceberg", json!({ "metastore": { "type": "egret", "warehouse_id": WH }, "storage_connection_id": SC })).is_err());
        assert!(parse("tpch", json!({ "scale": 0 })).is_err());
        assert!(
            parse(
                "elasticsearch",
                json!({ "hosts": ["es1"], "port": 9200, "tls": false, "password_secret_id": PW })
            )
            .is_err(),
            "password needs a user"
        );
        assert!(parse("iceberg", json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKIA" }, "storage_connection_id": SC })).is_err(), "access key without secret");
    }

    /// D5: `advanced.starrocks.driver_url`/`driver_class` on a `jdbc` catalog
    /// go together, and neither may be blank — an incomplete override is
    /// refused at parse time rather than silently mixing a pinned class with
    /// an operator's jar (or vice versa) at render time.
    #[test]
    fn a_jdbc_driver_override_must_be_complete() {
        let cfg = jdbc("postgresql", Some("sales"));
        // Both keys: accepted.
        parse_with_advanced(
            "jdbc",
            cfg.clone(),
            json!({ "starrocks": { "driver_url": "file:///opt/pg.jar", "driver_class": "org.postgresql.Driver" } }),
        )
        .unwrap();
        // Neither key: accepted (falls back to the pinned table).
        parse_with_advanced("jdbc", cfg.clone(), json!({})).unwrap();
        // Only `driver_url`: refused.
        assert!(matches!(
            parse_with_advanced(
                "jdbc",
                cfg.clone(),
                json!({ "starrocks": { "driver_url": "file:///opt/pg.jar" } })
            ),
            Err(ValidationError::Config { kind, .. }) if kind == "jdbc"
        ));
        // Only `driver_class`: refused.
        assert!(matches!(
            parse_with_advanced(
                "jdbc",
                cfg.clone(),
                json!({ "starrocks": { "driver_class": "org.postgresql.Driver" } })
            ),
            Err(ValidationError::Config { kind, .. }) if kind == "jdbc"
        ));
        // Both keys present but one blank: refused.
        assert!(matches!(
            parse_with_advanced(
                "jdbc",
                cfg,
                json!({ "starrocks": { "driver_url": "  ", "driver_class": "org.postgresql.Driver" } })
            ),
            Err(ValidationError::Config { kind, .. }) if kind == "jdbc"
        ));
    }

    #[test]
    fn generic_rules() {
        let adv = json!({ "trino": { "connector.name": "bigquery" } });
        assert!(CatalogDef::parse("generic", &json!({ "engines": [] }), &adv).is_err());
        assert!(
            CatalogDef::parse("generic", &json!({ "engines": ["trino"] }), &json!({})).is_err(),
            "connector.name required"
        );
        assert!(
            CatalogDef::parse(
                "generic",
                &json!({ "engines": ["trino"] }),
                &json!({ "trino": { "connector.name": "x" }, "starrocks": { "type": "y" } })
            )
            .is_err(),
            "starrocks keys without starrocks"
        );
        let err = CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.project-id": PW } } }),
            &json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }),
        );
        assert!(
            matches!(err, Err(ValidationError::Collision { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn a_line_break_cannot_reach_a_properties_file() {
        let err = CatalogDef::parse("tpch", &json!({}), &json!({ "trino": { "a.b": "x\ny=z" } }));
        assert!(matches!(err, Err(ValidationError::Advanced(_))), "{err:?}");
    }

    /// D4: `can_read` names every (kind, engine) pair rather than leaving it
    /// as an implication of the match arms — the matrix is a document.
    #[test]
    fn every_kind_and_dialect_has_a_stated_answer_for_both_engines() {
        let cases: &[(&str, serde_json::Value, bool, bool)] = &[
            (
                "iceberg",
                json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }),
                true,
                true,
            ),
            (
                "hive",
                json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }),
                true,
                true,
            ),
            (
                "delta_lake",
                json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }),
                true,
                true,
            ),
            (
                "hudi",
                json!({ "metastore": { "type": "hive", "uri": "thrift://h:9083" }, "storage_connection_id": SC }),
                true,
                true,
            ),
            (
                "elasticsearch",
                json!({ "hosts": ["e"], "port": 9200, "tls": false }),
                true,
                true,
            ),
            // D4 correction: StarRocks' `BENCHMARK` connector is `main`-only
            // (`c019a148`, 2026-01-22) — ships on no release branch or tag —
            // so `tpch`/`tpcds` is readable by Trino but refused for
            // StarRocks. The first pair in this table whose two engine
            // answers actually differ.
            ("tpch", json!({}), true, false),
            ("tpcds", json!({}), true, false),
        ];
        for (kind, cfg, trino, sr) in cases {
            let d = parse(kind, cfg.clone()).unwrap();
            assert_eq!(d.can_read(EngineType::Trino), *trino, "{kind}/trino");
            assert_eq!(d.can_read(EngineType::StarRocks), *sr, "{kind}/starrocks");
        }
        for dialect in [
            "postgresql",
            "mysql",
            "mariadb",
            "oracle",
            "sqlserver",
            "clickhouse",
        ] {
            let db = matches!(dialect, "postgresql" | "oracle" | "sqlserver").then_some("d");
            let d = parse("jdbc", jdbc(dialect, db)).unwrap();
            assert!(d.can_read(EngineType::Trino), "{dialect}/trino");
            assert!(d.can_read(EngineType::StarRocks), "{dialect}/starrocks");
        }
        // A generic catalog naming only `trino` is refused for `starrocks`
        // too — a per-catalog refusal, unlike tpch/tpcds' per-kind one above.
        let d = parse_with_advanced(
            "generic",
            json!({ "engines": ["trino"] }),
            json!({ "trino": { "connector.name": "memory" } }),
        )
        .unwrap();
        assert!(d.can_read(EngineType::Trino));
        assert!(!d.can_read(EngineType::StarRocks));
    }

    #[test]
    fn derived_facts() {
        let d = parse(
            "iceberg",
            json!({ "metastore": { "type": "egret", "warehouse_id": WH } }),
        )
        .unwrap();
        assert_eq!(d.warehouse_id(), Some(Uuid::parse_str(WH).unwrap()));
        assert_eq!(d, CatalogDef::egret_warehouse(Uuid::parse_str(WH).unwrap()));
        assert_eq!(d.trino_connector_name(), "iceberg");
        let d = parse("jdbc", jdbc("postgresql", Some("sales"))).unwrap();
        assert_eq!(
            d.secret_ids(),
            [Uuid::parse_str(PW).unwrap()].into_iter().collect()
        );
        assert_eq!(d.trino_connector_name(), "postgresql");
        assert!(d.can_read(EngineType::Trino) && d.can_read(EngineType::StarRocks));
        let g = CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"] }),
            &json!({ "trino": { "connector.name": "bigquery" } }),
        )
        .unwrap();
        assert!(g.can_read(EngineType::Trino));
        assert!(!g.can_read(EngineType::StarRocks));
        assert_eq!(g.trino_connector_name(), "bigquery");
        // Round trip through the stored JSON.
        let again =
            CatalogDef::parse(g.kind().as_str(), &g.config_json(), &g.advanced_json()).unwrap();
        assert_eq!(again, g);
    }

    /// SP3c D4. One assertion per kind, both the `Some` and the `None` arms,
    /// because `probe_target` is the ONLY function that decides what the
    /// connect-only probe dials. A kind that silently fell through to "probe
    /// nothing" would make the Test button quietly useless for it, and a kind
    /// that guessed a target would point egret's own socket at something the
    /// operator never configured.
    #[test]
    fn probe_target_reads_every_kind_and_refuses_to_guess() {
        // iceberg/egret: the catalog IS egret. Nothing to dial.
        assert_eq!(
            CatalogDef::egret_warehouse(Uuid::nil()).probe_target(),
            None
        );

        // iceberg/rest: the port in the URI, then the scheme's default.
        let rest = |uri: &str| {
            parse(
                "iceberg",
                json!({
                    "metastore": { "type": "rest", "uri": uri, "warehouse": "lake",
                                   "auth": { "type": "none" } },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap()
            .probe_target()
        };
        assert_eq!(
            rest("https://cat.example:8181/api"),
            Some(("cat.example".into(), 8181))
        );
        assert_eq!(
            rest("https://cat.example/api"),
            Some(("cat.example".into(), 443))
        );
        assert_eq!(
            rest("http://cat.example/api"),
            Some(("cat.example".into(), 80))
        );

        // iceberg/hive: thrift has no port `url` knows, so 9083 is named here.
        let ice_hive = |uri: &str| {
            parse(
                "iceberg",
                json!({
                    "metastore": { "type": "hive", "uri": uri },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap()
            .probe_target()
        };
        assert_eq!(ice_hive("thrift://hms:9083"), Some(("hms".into(), 9083)));
        assert_eq!(ice_hive("thrift://hms"), Some(("hms".into(), 9083)));
        // A comma-separated list dials the first, the one the engines try first.
        assert_eq!(
            ice_hive("thrift://hms-a:9083,thrift://hms-b:9084"),
            Some(("hms-a".into(), 9083))
        );

        // iceberg/glue: an AWS region is not a host.
        assert_eq!(
            parse(
                "iceberg",
                json!({
                    "metastore": { "type": "glue", "region": "ap-southeast-1" },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap()
            .probe_target(),
            None
        );

        // hive / delta_lake / hudi share `HiveFamilyConfig`, and all three must
        // answer — a `match` that named only `Hive(_)` would leave two kinds
        // unprobed.
        for kind in ["hive", "delta_lake", "hudi"] {
            let def = parse(
                kind,
                json!({
                    "metastore": { "type": "hive", "uri": "thrift://hms:9083" },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap();
            assert_eq!(
                def.probe_target(),
                Some(("hms".into(), 9083)),
                "{kind} metastore"
            );
            // Not about probing, and deliberately here anyway: this loop is the
            // only place in either crate that parses all three hive-family
            // kinds, and `storage_connection_id`'s SAME three-variant arm was
            // unpinned — flipping `delta_lake`/`hudi` to `None` there left the
            // whole two-crate suite green, while `validate_references`, the
            // Trino/StarRocks render and the stored column all read it (SP3c
            // task-3 review §5). Three lines here, no new fixture.
            assert_eq!(
                def.storage_connection_id(),
                Some(Uuid::parse_str(SC).unwrap()),
                "{kind} storage connection"
            );
        }
        // Glue, for the two of the three that may use it — validation refuses
        // `hudi` on Glue ("Hudi reads a Hive metastore only"), so there is no
        // stored row of that shape to probe.
        for kind in ["hive", "delta_lake"] {
            assert_eq!(
                parse(
                    kind,
                    json!({
                        "metastore": { "type": "glue", "region": "eu-west-1" },
                        "storage_connection_id": SC,
                    }),
                )
                .unwrap()
                .probe_target(),
                None,
                "{kind} on glue"
            );
        }

        // jdbc: the typed host and port, verbatim.
        assert_eq!(
            parse(
                "jdbc",
                json!({ "dialect": "postgresql", "host": "pg.internal", "port": 5432,
                        "database": "analytics", "user": "ro",
                        "password_secret_id": PW, "ssl": { "mode": "disable" } }),
            )
            .unwrap()
            .probe_target(),
            Some(("pg.internal".into(), 5432))
        );

        // elasticsearch: the first host plus the one shared port.
        assert_eq!(
            parse(
                "elasticsearch",
                json!({ "hosts": ["es1", "es2"], "port": 9200, "tls": false }),
            )
            .unwrap()
            .probe_target(),
            Some(("es1".into(), 9200))
        );

        // tpch / tpcds generate their own data; generic is strings egret does
        // not interpret. All three: nothing to dial.
        assert_eq!(parse("tpch", json!({})).unwrap().probe_target(), None);
        assert_eq!(parse("tpcds", json!({})).unwrap().probe_target(), None);
        assert_eq!(
            CatalogDef::parse(
                "generic",
                &json!({ "engines": ["trino"] }),
                &json!({ "trino": { "connector.name": "bigquery" } }),
            )
            .unwrap()
            .probe_target(),
            None
        );
    }

    /// The two private parsers, on the shapes the validated forms actually
    /// produce plus the ones they used to.
    #[test]
    fn uri_and_host_parsing_names_the_real_shapes() {
        assert_eq!(
            host_port_from_uri("thrift://hms.svc.cluster.local:9083"),
            Some(("hms.svc.cluster.local".into(), 9083))
        );
        assert_eq!(
            host_port_from_uri(" thrift://a:1 , thrift://b:2 "),
            Some(("a".into(), 1))
        );
        assert_eq!(
            host_port_from_uri("thrift://10.1.2.3:9083"),
            Some(("10.1.2.3".into(), 9083))
        );
        // Not a URL at all, and a scheme with no default: no target, no guess.
        assert_eq!(host_port_from_uri("hms:9083"), None);
        assert_eq!(host_port_from_uri("ftp://hms"), None);
        assert_eq!(host_port_from_uri(""), None);
        assert_eq!(host_port_from_uri("thrift://"), None);

        assert_eq!(strip_scheme("es-1"), "es-1");
        assert_eq!(strip_scheme("http://es-1:9200"), "es-1");
        assert_eq!(strip_scheme("https://es-1"), "es-1");
        assert_eq!(strip_scheme("http://es-1:9200/prefix"), "es-1");
        // A trailing colon-and-not-a-number is part of the host, not a port.
        assert_eq!(strip_scheme("es-1:abc"), "es-1:abc");
    }

    /// SP3c review F1: an IPv6 metastore is a storable value, and the host must
    /// come back in the form a socket wants. `Url::host_str` keeps the URI's
    /// brackets, and `TcpStream::connect(("[::1]", p))` neither parses `[::1]`
    /// as an address nor resolves it as a name — so the bracketed form read red
    /// for ever against a listener that was up.
    #[test]
    fn an_ipv6_metastore_is_dialled_unbracketed() {
        assert_eq!(
            host_port_from_uri("thrift://[::1]:9083"),
            Some(("::1".into(), 9083))
        );
        // No port: the scheme's default still applies, and the brackets still go.
        assert_eq!(
            host_port_from_uri("thrift://[::1]"),
            Some(("::1".into(), 9083))
        );
        assert_eq!(
            host_port_from_uri("https://[2001:db8::1]:8181/api"),
            Some(("2001:db8::1".into(), 8181))
        );
        // Through the stored configuration, not just the private helper.
        assert_eq!(
            parse(
                "hive",
                json!({
                    "metastore": { "type": "hive", "uri": "thrift://[::1]:9083" },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap()
            .probe_target(),
            Some(("::1".into(), 9083))
        );
        // The Elasticsearch path has its own stripper, and its colons must not
        // be read as a port either.
        assert_eq!(strip_scheme("[::1]"), "::1");
        assert_eq!(strip_scheme("http://[::1]:9200"), "::1");
        assert_eq!(strip_scheme("::1"), "::1");
    }

    /// SP3c review F2: `thrift://` and `https://` used to pass validation, so a
    /// row could store a URI naming no host — and the test route then told the
    /// operator the KIND had no endpoint, which is false and points at the
    /// wrong fix. Refused at the source now, and `names_an_endpoint` keeps the
    /// two facts apart for every row written before this rule.
    #[test]
    fn a_uri_naming_no_host_is_refused_and_the_two_facts_stay_apart() {
        for uri in ["thrift://", "thrift://a:1,thrift://"] {
            let e = parse(
                "hive",
                json!({
                    "metastore": { "type": "hive", "uri": uri },
                    "storage_connection_id": SC,
                }),
            )
            .unwrap_err();
            assert!(
                matches!(&e, ValidationError::Config { msg, .. } if msg.contains("must name a host")),
                "{uri:?} must be refused, got {e:?}"
            );
        }
        let e = parse(
            "iceberg",
            json!({
                "metastore": { "type": "rest", "uri": "https://", "warehouse": "lake",
                               "auth": { "type": "none" } },
                "storage_connection_id": SC,
            }),
        )
        .unwrap_err();
        assert!(
            matches!(&e, ValidationError::Config { msg, .. } if msg.contains("must name a host")),
            "got {e:?}"
        );

        // The kinds that name an endpoint, against the kinds that do not. A
        // `hive` catalog ALWAYS names one; that is what makes "the stored URI
        // could not be read" the honest sentence when `probe_target` is `None`.
        let hive = parse(
            "hive",
            json!({
                "metastore": { "type": "hive", "uri": "thrift://hms:9083" },
                "storage_connection_id": SC,
            }),
        )
        .unwrap();
        assert!(hive.names_an_endpoint() && hive.probe_target().is_some());
        let glue = parse(
            "hive",
            json!({
                "metastore": { "type": "glue", "region": "eu-west-1" },
                "storage_connection_id": SC,
            }),
        )
        .unwrap();
        assert!(!glue.names_an_endpoint() && glue.probe_target().is_none());
        assert!(!parse("tpch", json!({})).unwrap().names_an_endpoint());
        assert!(!parse("tpcds", json!({})).unwrap().names_an_endpoint());
        assert!(!CatalogDef::egret_warehouse(Uuid::nil()).names_an_endpoint());
        assert!(!CatalogDef::parse(
            "generic",
            &json!({ "engines": ["trino"] }),
            &json!({ "trino": { "connector.name": "bigquery" } }),
        )
        .unwrap()
        .names_an_endpoint());
        assert!(parse(
            "elasticsearch",
            json!({ "hosts": ["es1"], "port": 9200, "tls": false }),
        )
        .unwrap()
        .names_an_endpoint());
        assert!(parse(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "h", "port": 5432, "database": "d",
                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "disable" } }),
        )
        .unwrap()
        .names_an_endpoint());
    }
}
