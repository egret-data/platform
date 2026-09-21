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

fn http_uri(field: &str, v: &str) -> Result<(), String> {
    text(field, v)?;
    if !(v.starts_with("http://") || v.starts_with("https://")) || v.contains(char::is_whitespace) {
        return Err(format!("{field} must be an http(s) URL"));
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
    Ok(())
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
        use crate::render::trino::{render_trino, RenderError, TrinoRenderInput};
        match render_trino("probe", self, &TrinoRenderInput::probe()) {
            Err(RenderError::Collision(key)) => Err(ValidationError::Collision {
                engine: "trino".into(),
                key,
                kind: self.kind().as_str().into(),
            }),
            Err(RenderError::LineBreak(key)) => Err(ValidationError::Config {
                kind: self.kind().as_str().into(),
                msg: format!("{key} contains a line break"),
            }),
            _ => Ok(()),
        }
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

    /// Whether an engine type can read this catalog (spec §3: every typed kind is
    /// readable by both engines; `generic` only by the engines it names).
    pub fn can_read(&self, engine: EngineType) -> bool {
        match &self.config {
            CatalogConfig::Generic(g) => g.engines.contains(&engine),
            _ => true,
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
}
