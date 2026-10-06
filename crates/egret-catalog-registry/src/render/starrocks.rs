//! StarRocks rendering: a catalog definition + resolved inputs → the property
//! map of a `CREATE EXTERNAL CATALOG`, plus the `ALTER`/`DROP` forms and a
//! redacted form for display and logs.
//!
//! Unlike Trino's, this output is NOT a file egret hands to Kubernetes: it is a
//! statement egret runs as `root` on a live FE, with secrets INLINE (StarRocks
//! has no `${ENV:…}`). Two consequences shape this module:
//!   - the rendered map keeps secrets as ids (`PropValue::Secret`) so the
//!     plaintext exists only inside `create_sql`/`alter_sql`, and
//!     [`StarRocksCatalogDdl::redacted`] is the only form that may be logged;
//!   - every interpolated value goes through [`escape_dq`] and every identifier
//!     through [`escape_ident`].
//!
//! Property names verified against the vendored StarRocks FE
//! (`../starrocks` relative to `data-portal/`) on 2026-09-21:
//!   - Catalog type `type` ∈ `iceberg, hive, deltalake, hudi, jdbc, es,
//!     benchmark, …` — `fe/fe-core/.../connector/ConnectorType.java:39-67`.
//!   - Iceberg sub-type `iceberg.catalog.type` ∈ `rest, hive, glue, hadoop,
//!     jdbc` — `IcebergCatalogProperties.java:27`, `IcebergCatalogType.java:18-36`.
//!   - Every `iceberg.catalog.<k>` is stripped to `<k>` and handed to the
//!     Iceberg `RESTSessionCatalog` — `IcebergRESTCatalog.java:107-113`.
//!   - Hive-family metastore: `hive.metastore.type` (`hive`\|`glue`\|`dlf`,
//!     default `hive`), `hive.metastore.uris` (**plural**, unlike Trino's
//!     `hive.metastore.uri`) — `HiveConnector.java:31-34`,
//!     `HiveConnectorInternalMgr.java:42,88-97`, `DeltaLakeConnector.java:34`,
//!     `HudiConnector.java:33-34`.
//!   - Glue: `aws.glue.region`, `aws.glue.use_instance_profile`,
//!     `aws.glue.access_key`, `aws.glue.secret_key` —
//!     `CloudConfigurationConstants.java:66-79`. The `aws.hive.metastore.glue.*`
//!     keys in `CREATE_EXTERNAL_CATALOG.md` are stale — they appear nowhere in
//!     the FE as credentials.
//!   - S3: `aws.s3.endpoint`, `aws.s3.region`, `aws.s3.access_key`,
//!     `aws.s3.secret_key`, `aws.s3.enable_path_style_access`,
//!     `aws.s3.enable_ssl`, `aws.s3.use_instance_profile` —
//!     `CloudConfigurationConstants.java:29-64`, parsed in
//!     `AwsCloudConfigurationProvider.java:77-109`.
//!   - JDBC: `jdbc_uri`, `user`, `password`, `driver_url`, `driver_class`,
//!     `schema_resolver` — **never `checksum`**, which the FE computes itself
//!     from `driver_url` — `JDBCResource.java:49-57`, `JDBCConnector.java:46-56,74-100`.
//!   - Elasticsearch: `hosts` (comma-separated URLs), `user`, `password`,
//!     `es.net.ssl` — `EsConfig.java:30-58`, `EsTable.java:69-86`.
//!   - Benchmark: `scale` only (a double > 0, default `1`); tpch/tpcds/ssb is
//!     the database you `USE`, not a property —
//!     `BenchmarkConfig.java:21-24`, `BenchmarkCatalogConfig.java:31-47`,
//!     `BenchmarkSuiteFactory.java:27-44`. **Currently unreachable**: the
//!     `BENCHMARK` connector itself ships on no StarRocks release branch or
//!     tag (`c019a148`, 2026-01-22, `main`-only), so `can_read` refuses
//!     `tpch`/`tpcds` for StarRocks before this shape is ever rendered (D4).
//!   - Grammar: keys *and* values are quoted strings; `PROPERTIES(...)` is
//!     mandatory on `CREATE`; `ALTER CATALOG <n> SET (...)` — **not**
//!     `SET PROPERTIES` — `StarRocks.g4:866-883,1140-1142,3135-3152`.
//!   - `ALTER`: only `type` is refused, by key and regardless of value;
//!     properties merge and cannot be unset; an unknown catalog is a **silent
//!     no-op** — `CatalogAnalyzer.java:39-41,113-126`, `CatalogMgr.java:180-240`.
//!   - Names: `^[a-zA-Z]\w{0,63}$` (64 max); `default_catalog` and the
//!     `resource_mapping_inside_catalog_` prefix are reserved —
//!     `FeNameFormat.java:41,177-195`, `CatalogAnalyzer.java:61-65,89-91`.
//!
//! MySQL is the odd one out: the FE rewrites `jdbc:mysql` → `jdbc:mariadb` and
//! always loads `org.mariadb.jdbc.Driver` ("because of gpl protocol"),
//! `JDBCMetadata.java:123-125,189-191` — so egret sends the MariaDB URL form
//! and pins the MariaDB driver for both `mysql` and `mariadb` dialects. That
//! form's SSL vocabulary is `disable|trust|verify-full`, not Trino/Connector-J's
//! `DISABLED|REQUIRED|VERIFY_IDENTITY`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use uuid::Uuid;

use crate::config::{
    CatalogConfig, CatalogDef, EngineType, HiveMetastore, IcebergMetastore, JdbcConfig,
    JdbcDialect, RestAuth,
};
use crate::sensitive::is_sensitive_key;

use super::{RenderError, StorageRender, WarehouseRender};

/// A rendered property's value: either a literal, or a secret whose plaintext
/// is resolved only inside `create_sql`/`alter_sql` (never in `properties`
/// itself, so a `Debug`/log of the map cannot leak it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    Plain(String),
    Secret(Uuid),
}

#[derive(Clone, Copy, Debug)]
pub struct StarRocksRenderInput<'a> {
    /// In-cluster egret URL (REST catalog + token endpoint).
    pub egret_url: &'a str,
    pub warehouse: Option<WarehouseRender<'a>>,
    pub storage: Option<StorageRender<'a>>,
    /// The attachment's minted Catalog Client credential (egret catalogs only).
    pub credential_secret_id: Option<Uuid>,
    pub drivers: &'a JdbcDrivers,
}

impl StarRocksRenderInput<'static> {
    /// Inputs that satisfy every kind: rendering with these tells which keys
    /// the typed form sets, which is what the save-time collision check needs.
    pub fn probe() -> Self {
        StarRocksRenderInput {
            egret_url: "http://probe",
            warehouse: Some(WarehouseRender {
                name: "probe",
                s3_endpoint: "http://probe",
            }),
            storage: Some(StorageRender {
                endpoint: Some("http://probe"),
                region: "probe",
                path_style: true,
                access_key: "probe",
                secret_key_secret_id: Some(Uuid::nil()),
            }),
            credential_secret_id: Some(Uuid::nil()),
            drivers: &DEFAULT_SR_JDBC_DRIVERS,
        }
    }
}

/// One dialect's pinned JDBC driver jar and class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JdbcDriver {
    pub driver_url: String,
    pub driver_class: String,
}

/// One pinned driver per dialect. StarRocks makes the FE DOWNLOAD the jar
/// named by `driver_url` (`JDBCConnector.java:74-100`, which also MD5s it
/// into `checksum`), so the URL must be reachable from the FE pods — which is
/// why this table is configuration (`[starrocks.jdbc_drivers]`), not a
/// constant: an air-gapped platform repoints it at an internal mirror or a
/// `file:///` path baked into the image (D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JdbcDrivers {
    pub postgresql: JdbcDriver,
    pub mysql: JdbcDriver,
    pub mariadb: JdbcDriver,
    pub oracle: JdbcDriver,
    pub sqlserver: JdbcDriver,
    pub clickhouse: JdbcDriver,
}

/// The pinned table (verified live, 200 + matching artifact, on 2026-09-21 —
/// see the task-2 report). `mysql` and `mariadb` are pinned to the same
/// MariaDB Connector/J entry, at the version the FE itself bundles
/// (`fe/pom.xml:1241-1246`: `mariadb-java-client 3.3.2` in the vendored
/// source; egret pins the newer `3.4.1`, verified reachable the same way),
/// because the FE loads `org.mariadb.jdbc.Driver` for both — see the module
/// doc's MySQL note.
impl Default for JdbcDrivers {
    fn default() -> Self {
        let mariadb = JdbcDriver {
            driver_url: "https://repo1.maven.org/maven2/org/mariadb/jdbc/mariadb-java-client/3.4.1/mariadb-java-client-3.4.1.jar".into(),
            driver_class: "org.mariadb.jdbc.Driver".into(),
        };
        JdbcDrivers {
            postgresql: JdbcDriver {
                driver_url: "https://repo1.maven.org/maven2/org/postgresql/postgresql/42.7.4/postgresql-42.7.4.jar".into(),
                driver_class: "org.postgresql.Driver".into(),
            },
            mysql: mariadb.clone(),
            mariadb,
            oracle: JdbcDriver {
                driver_url: "https://repo1.maven.org/maven2/com/oracle/database/jdbc/ojdbc10/19.18.0.0/ojdbc10-19.18.0.0.jar".into(),
                driver_class: "oracle.jdbc.driver.OracleDriver".into(),
            },
            sqlserver: JdbcDriver {
                driver_url: "https://repo1.maven.org/maven2/com/microsoft/sqlserver/mssql-jdbc/12.6.3.jre11/mssql-jdbc-12.6.3.jre11.jar".into(),
                driver_class: "com.microsoft.sqlserver.jdbc.SQLServerDriver".into(),
            },
            clickhouse: JdbcDriver {
                driver_url: "https://repo1.maven.org/maven2/com/clickhouse/clickhouse-jdbc/0.6.3/clickhouse-jdbc-0.6.3.jar".into(),
                driver_class: "com.clickhouse.jdbc.ClickHouseDriver".into(),
            },
        }
    }
}

impl JdbcDrivers {
    /// The pinned (or overridden) driver for a dialect, or `None` when
    /// either `driver_url` or `driver_class` is empty — an operator's way of
    /// saying "not available here" (a `[starrocks.jdbc_drivers]` override
    /// can blank either field deliberately; `render_starrocks` then refuses
    /// with `NoJdbcDriver` unless the catalog's own `advanced.starrocks`
    /// supplies both keys). Checking only `driver_url` would let a
    /// deployment blank `driver_class` alone reach the FE as
    /// `driver_class = ""`, which comes back as an FE error instead of
    /// egret's own clean refusal — exactly what D5's refusal exists to
    /// avoid.
    pub fn get(&self, dialect: JdbcDialect) -> Option<&JdbcDriver> {
        let driver = match dialect {
            JdbcDialect::Postgresql => &self.postgresql,
            JdbcDialect::Mysql => &self.mysql,
            JdbcDialect::Mariadb => &self.mariadb,
            JdbcDialect::Oracle => &self.oracle,
            JdbcDialect::Sqlserver => &self.sqlserver,
            JdbcDialect::Clickhouse => &self.clickhouse,
        };
        if driver.driver_url.is_empty() || driver.driver_class.is_empty() {
            None
        } else {
            Some(driver)
        }
    }

    /// Applies this deployment's `[starrocks.jdbc_drivers]` overrides (D5)
    /// on top of `self` (normally the pinned table). Each dialect's
    /// `driver_url`/`driver_class` are overridden independently — a
    /// deployment that only names one field of one dialect leaves every
    /// other field, and every other dialect, at its compiled-in value, so a
    /// partially-written table can never leave a dialect without a driver by
    /// accident.
    pub fn with_overrides(&self, settings: &egret_config::StarRocksSettings) -> JdbcDrivers {
        fn apply(driver: &mut JdbcDriver, ov: Option<&egret_config::JdbcDriverOverride>) {
            let Some(ov) = ov else { return };
            if let Some(url) = &ov.driver_url {
                driver.driver_url = url.clone();
            }
            if let Some(class) = &ov.driver_class {
                driver.driver_class = class.clone();
            }
        }
        let mut out = self.clone();
        let o = &settings.jdbc_drivers;
        apply(&mut out.postgresql, o.postgresql.as_ref());
        apply(&mut out.mysql, o.mysql.as_ref());
        apply(&mut out.mariadb, o.mariadb.as_ref());
        apply(&mut out.oracle, o.oracle.as_ref());
        apply(&mut out.sqlserver, o.sqlserver.as_ref());
        apply(&mut out.clickhouse, o.clickhouse.as_ref());
        out
    }
}

/// The compiled-in pinned table, lazily built once. `&DEFAULT_SR_JDBC_DRIVERS`
/// derefs to `&JdbcDrivers` wherever an input wants one (`LazyLock<T>: Deref<Target = T>`).
pub static DEFAULT_SR_JDBC_DRIVERS: LazyLock<JdbcDrivers> = LazyLock::new(JdbcDrivers::default);

/// The two `advanced.starrocks` keys a JDBC catalog may override per-catalog
/// instead of colliding at save time (plan D5).
pub const SR_OVERRIDABLE: [&str; 2] = ["driver_url", "driver_class"];

/// StarRocks reserves `default_catalog` (`CatalogAnalyzer.java:61-65`) and the
/// resource-mapping prefix (`CatalogMgr.java:493`).
pub fn is_reserved_starrocks_name(name: &str) -> bool {
    name == "default_catalog" || name.starts_with("resource_mapping_inside_catalog_")
}

pub fn escape_dq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn escape_ident(s: &str) -> String {
    s.replace('`', "``")
}

/// Replace every known secret's plaintext with `******`. The only thing that
/// may stand between an FE error (which can quote the statement it rejected,
/// secrets and all) and `apply_error`/an API response.
///
/// `create_sql`/`alter_sql` never interpolate a secret's raw plaintext — they
/// interpolate [`escape_dq`]'s output (`:483`), and an FE error quoting the
/// rejected statement quotes THAT escaped text back. A secret containing a
/// `"` or `\` therefore appears in the wild only in its escaped form, so both
/// forms must be scrubbed. The two passes are independent substring
/// replacements, so which runs first does not change the result — the
/// escaped form is NOT a superset of the raw one (`escape_dq("p4ss\"w0rd\\x")`
/// = `p4ss\"w0rd\\x`, which does not contain `p4ss"w0rd\x`), so a raw-first
/// order would leave nothing behind either. Escaped is scrubbed first simply
/// because it is the form actually rendered into the statement; the raw pass
/// is a second, defensive layer for any other path a secret's plaintext might
/// reach.
pub fn scrub_secrets(text: &str, secrets: &BTreeMap<Uuid, String>) -> String {
    let mut out = text.to_string();
    for v in secrets.values() {
        if v.is_empty() {
            continue;
        }
        let escaped = escape_dq(v);
        if !escaped.is_empty() {
            out = out.replace(escaped.as_str(), "******");
        }
        out = out.replace(v.as_str(), "******");
    }
    out
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

/// The region an egret warehouse is addressed with. egret's own object store
/// is S3-compatible rather than AWS, so the region is a required-but-unused
/// signing input; Trino's `Egret` branch pins the same value.
const EGRET_WAREHOUSE_S3_REGION: &str = "us-east-1";

struct Out {
    props: BTreeMap<String, PropValue>,
}

impl Out {
    fn set(&mut self, k: &str, v: impl Into<String>) {
        self.props.insert(k.to_string(), PropValue::Plain(v.into()));
    }

    fn secret(&mut self, k: &str, id: Uuid) {
        self.props.insert(k.to_string(), PropValue::Secret(id));
    }

    fn s3(&mut self, input: &StarRocksRenderInput<'_>) -> Result<(), RenderError> {
        let s = input
            .storage
            .ok_or(RenderError::MissingInput("storage connection"))?;
        if let Some(ep) = s.endpoint {
            self.set("aws.s3.endpoint", ep);
            // The FE defaults this to `true` (`AwsCloudConfiguration.java:40`),
            // so an http endpoint (RustFS, most in-cluster S3-compatible stores)
            // needs it set explicitly to `false`.
            self.set(
                "aws.s3.enable_ssl",
                if ep.starts_with("https://") {
                    "true"
                } else {
                    "false"
                },
            );
        }
        self.set("aws.s3.region", s.region);
        self.set("aws.s3.enable_path_style_access", bool_str(s.path_style));
        self.set("aws.s3.use_instance_profile", "false");
        self.set("aws.s3.access_key", s.access_key);
        self.secret(
            "aws.s3.secret_key",
            s.secret_key_secret_id
                .ok_or(RenderError::MissingInput("storage secret"))?,
        );
        Ok(())
    }

    /// S3 configuration for an egret-warehouse (`IcebergMetastore::Egret`)
    /// catalog, which has NO storage connection of its own: `validate` refuses
    /// `storage_connection_id` on that variant ("an egret warehouse catalog
    /// uses its warehouse's storage"), so `input.storage` is always `None`
    /// here and [`Out::s3`] — which requires one, plus a
    /// `secret_key_secret_id` — can never be used on this branch. The two
    /// sources therefore cannot disagree; there is only one.
    ///
    /// No `aws.s3.access_key`/`aws.s3.secret_key`: the FE gets its object
    /// credentials VENDED by egret's Iceberg REST catalog. It still has to be
    /// told where to talk, exactly as Trino's own `Egret` branch is
    /// (`render/trino.rs`) — without an endpoint the FE falls back to its
    /// default AWS endpoint and every object read fails
    /// `Bad Request (Service: S3, Status Code: 400)`.
    ///
    /// MEASURED on StarRocks 4.1.3-8a8e186 (kind `kind-egret`,
    /// `egret-engines/sp3c2-sr-live-fe-0`, 2026-09-26): this exact property
    /// map, with no static keys, reads real rows from an egret warehouse on
    /// MinIO, while the same MinIO refuses anonymous GET/LIST with 403 — so
    /// the read is genuinely served by vended credentials. The same catalog
    /// without these properties reproduced the S3 400 above.
    ///
    /// `vended-credentials-enabled` already defaults to `true`
    /// (`IcebergRESTCatalog.java`), but is set explicitly so a default flip
    /// cannot silently break object reads — and so the rendered map states
    /// which credential path this catalog depends on.
    ///
    /// `region` and path-style are fixed the way Trino's `Egret` branch fixes
    /// them: the warehouse render carries only a name and an endpoint, and an
    /// egret warehouse is always addressed path-style.
    fn egret_warehouse_s3(&mut self, s3_endpoint: &str) {
        self.set("iceberg.catalog.vended-credentials-enabled", "true");
        self.set("aws.s3.endpoint", s3_endpoint);
        // The FE defaults `enable_ssl` to `true` (`AwsCloudConfiguration.java:40`),
        // so an http endpoint (RustFS, most in-cluster S3-compatible stores)
        // needs it set explicitly to `false`.
        self.set(
            "aws.s3.enable_ssl",
            if s3_endpoint.starts_with("https://") {
                "true"
            } else {
                "false"
            },
        );
        self.set("aws.s3.region", EGRET_WAREHOUSE_S3_REGION);
        self.set("aws.s3.enable_path_style_access", "true");
        self.set("aws.s3.use_instance_profile", "false");
    }

    fn glue(&mut self, region: &str, access_key: Option<&str>, secret: Option<Uuid>) {
        self.set("aws.glue.region", region);
        self.set(
            "aws.glue.use_instance_profile",
            bool_str(access_key.is_none()),
        );
        if let Some(a) = access_key {
            self.set("aws.glue.access_key", a);
        }
        if let Some(id) = secret {
            self.secret("aws.glue.secret_key", id);
        }
    }
}

/// The `jdbc_uri` a StarRocks JDBC catalog gets. Identical to Trino's URL for
/// every dialect EXCEPT MySQL: `JDBCMetadata.getDriverName`/`getJdbcUrl`
/// (JDBCMetadata.java:121-128, 187-194) force `org.mariadb.jdbc.Driver` and
/// rewrite `jdbc:mysql` → `jdbc:mariadb` "because of gpl protocol", so a URL
/// carrying Connector/J's `sslMode=VERIFY_IDENTITY` would reach a driver whose
/// vocabulary is `disable|trust|verify-full`. egret therefore sends the MariaDB
/// form for both dialects, and pins the MariaDB driver for both.
pub fn starrocks_jdbc_uri(c: &JdbcConfig) -> String {
    match c.dialect {
        JdbcDialect::Mysql => {
            let mut c = c.clone();
            c.dialect = JdbcDialect::Mariadb;
            crate::render::trino::jdbc_url(&c)
        }
        _ => crate::render::trino::jdbc_url(c),
    }
}

pub fn render_starrocks(
    name: &str,
    def: &CatalogDef,
    input: &StarRocksRenderInput<'_>,
) -> Result<StarRocksCatalogDdl, RenderError> {
    if is_reserved_starrocks_name(name) {
        return Err(RenderError::ReservedName(name.into()));
    }
    if !def.can_read(EngineType::StarRocks) {
        return Err(RenderError::NotReadable(name.into()));
    }
    let mut out = Out {
        props: BTreeMap::new(),
    };
    match &def.config {
        CatalogConfig::Iceberg(c) => {
            out.set("type", "iceberg");
            match &c.metastore {
                IcebergMetastore::Egret { .. } => {
                    let wh = input
                        .warehouse
                        .ok_or(RenderError::MissingInput("warehouse"))?;
                    let credential = input
                        .credential_secret_id
                        .ok_or(RenderError::MissingInput("catalog client credential"))?;
                    out.set("iceberg.catalog.type", "rest");
                    out.set(
                        "iceberg.catalog.uri",
                        format!("{}/catalog", input.egret_url),
                    );
                    out.set("iceberg.catalog.warehouse", wh.name);
                    out.secret("iceberg.catalog.credential", credential);
                    out.set(
                        "iceberg.catalog.oauth2-server-uri",
                        format!("{}/oauth/token", input.egret_url),
                    );
                    out.egret_warehouse_s3(wh.s3_endpoint);
                }
                IcebergMetastore::Rest {
                    uri,
                    warehouse,
                    auth,
                } => {
                    out.set("iceberg.catalog.type", "rest");
                    out.set("iceberg.catalog.uri", uri.as_str());
                    out.set("iceberg.catalog.warehouse", warehouse.as_str());
                    if let RestAuth::Oauth2 {
                        credential_secret_id,
                        scope,
                        token_uri,
                    } = auth
                    {
                        out.secret("iceberg.catalog.credential", *credential_secret_id);
                        if let Some(s) = scope {
                            out.set("iceberg.catalog.scope", s.as_str());
                        }
                        if let Some(t) = token_uri {
                            out.set("iceberg.catalog.oauth2-server-uri", t.as_str());
                        }
                    }
                    out.s3(input)?;
                }
                IcebergMetastore::Hive { uri } => {
                    out.set("iceberg.catalog.type", "hive");
                    out.set("hive.metastore.uris", uri.as_str());
                    out.s3(input)?;
                }
                IcebergMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => {
                    out.set("iceberg.catalog.type", "glue");
                    out.glue(region, access_key.as_deref(), *secret_key_secret_id);
                    out.s3(input)?;
                }
            }
        }
        CatalogConfig::Hive(c) | CatalogConfig::DeltaLake(c) | CatalogConfig::Hudi(c) => {
            out.set(
                "type",
                def.starrocks_catalog_type()
                    .expect("hive-family kinds always have a StarRocks type"),
            );
            match &c.metastore {
                HiveMetastore::Hive { uri } => {
                    out.set("hive.metastore.type", "hive");
                    out.set("hive.metastore.uris", uri.as_str());
                }
                HiveMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => {
                    out.set("hive.metastore.type", "glue");
                    out.glue(region, access_key.as_deref(), *secret_key_secret_id);
                }
            }
            out.s3(input)?;
        }
        CatalogConfig::Jdbc(c) => {
            out.set("type", "jdbc");
            out.set("jdbc_uri", starrocks_jdbc_uri(c));
            out.set("user", c.user.as_str());
            out.secret("password", c.password_secret_id);
            let pinned = input.drivers.get(c.dialect);
            let driver_url = match def.advanced.starrocks.get("driver_url") {
                Some(v) => v.clone(),
                None => pinned
                    .ok_or(RenderError::NoJdbcDriver(c.dialect))?
                    .driver_url
                    .clone(),
            };
            let driver_class = match def.advanced.starrocks.get("driver_class") {
                Some(v) => v.clone(),
                None => pinned
                    .ok_or(RenderError::NoJdbcDriver(c.dialect))?
                    .driver_class
                    .clone(),
            };
            out.set("driver_url", driver_url);
            out.set("driver_class", driver_class);
            out.set("schema_resolver", c.dialect.starrocks_schema_resolver());
        }
        CatalogConfig::Elasticsearch(c) => {
            out.set("type", "es");
            let scheme = if c.tls { "https" } else { "http" };
            out.set(
                "hosts",
                c.hosts
                    .iter()
                    .map(|h| format!("{scheme}://{h}:{}", c.port))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.set("es.net.ssl", bool_str(c.tls));
            if let Some(user) = &c.user {
                out.set("user", user.as_str());
                if let Some(id) = c.password_secret_id {
                    out.secret("password", id);
                }
            }
        }
        CatalogConfig::Tpch(_) | CatalogConfig::Tpcds(_) => {
            // Unreachable today: `can_read` above already refused StarRocks
            // for tpch/tpcds (D4 correction: StarRocks' `BENCHMARK`
            // connector — `c019a148`, 2026-01-22 — ships on no release
            // branch or tag, verified against the vendored source). This arm
            // stays explicit, rather than folded into the `can_read` guard's
            // silence, for two reasons: the match must remain exhaustive
            // over `CatalogConfig`'s kinds without a wildcard, and a second,
            // independent refusal here means this path can never regress
            // into emitting `type = "benchmark"` DDL a real FE rejects, even
            // if a future edit ever let the top-of-function guard and
            // `can_read` disagree.
            return Err(RenderError::NotReadable(name.into()));
        }
        CatalogConfig::Generic(g) => {
            for (k, v) in &def.advanced.starrocks {
                out.set(k, v.as_str());
            }
            for (k, id) in &g.secret_properties.starrocks {
                out.secret(k, *id);
            }
        }
    }
    if !matches!(def.config, CatalogConfig::Generic(_)) {
        // `SR_OVERRIDABLE` is exempt from the collision check only where the
        // typed form actually consumes it — JDBC, which already read
        // `driver_url`/`driver_class` out of `def.advanced.starrocks` above.
        // On every other kind these are ordinary advanced properties: the
        // normal collision rule applies, so a `driver_url` on e.g. a `hive`
        // catalog is rendered (or refused as a collision), never silently
        // dropped (M2).
        let jdbc = matches!(def.config, CatalogConfig::Jdbc(_));
        for (k, v) in &def.advanced.starrocks {
            if jdbc && SR_OVERRIDABLE.contains(&k.as_str()) {
                continue;
            }
            if out.props.contains_key(k) {
                return Err(RenderError::Collision(k.clone()));
            }
            out.set(k, v.as_str());
        }
    }
    if let Some((k, _)) = out.props.iter().find(|(_, v)| match v {
        PropValue::Plain(s) => s.contains(['\n', '\r']),
        PropValue::Secret(_) => false,
    }) {
        return Err(RenderError::LineBreak(k.clone()));
    }
    Ok(StarRocksCatalogDdl {
        name: name.to_string(),
        properties: out.props,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct StarRocksCatalogDdl {
    pub name: String,
    pub properties: BTreeMap<String, PropValue>,
}

/// Which statement [`StarRocksCatalogDdl::redacted`] renders. Exists so the
/// log line next to a failed statement names the statement that actually ran:
/// `converge_attachment` sends a `CREATE` or an `ALTER` depending on what the
/// FE already holds, and `redacted` used to say `CREATE` either way (final
/// whole-branch review, F4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementForm {
    Create,
    Alter,
}

/// Whether a plain property value is shaped like a URL, and so may carry
/// credentials in its userinfo or its query — `https://user:pass@mirror/…`,
/// the shape `driver_url` can legitimately take against an internal artifact
/// mirror.
///
/// The same judgement `egret_engine_mgmt::starrocks_ddl::redact_urls` makes
/// about the same strings when they appear in an FE error, expressed here for
/// property VALUES rather than prose: a scheme, `://`, and a non-empty
/// remainder. `jdbc:postgresql://…` matches (on its inner `postgresql://`),
/// which is what `redact_urls`' regex does too.
fn looks_like_url(value: &str) -> bool {
    let Some(pos) = value.find("://") else {
        return false;
    };
    // Nothing after the `://` means the value names no resource. (Nothing
    // BEFORE it needs no guard of its own: the scheme walk below finds no
    // character at all and answers `false`.)
    if pos + 3 >= value.len() {
        return false;
    }
    // Walk back from `://` over scheme-legal characters; the earliest one
    // reached is the scheme's first character, which a scheme must start with
    // a letter at.
    value[..pos]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        .last()
        .is_some_and(|c| c.is_ascii_alphabetic())
}

impl StarRocksCatalogDdl {
    /// Builds the `"key" = "value"` list. `render_starrocks`'s line-break
    /// refusal only ever sees `PropValue::Plain` values — a secret's
    /// plaintext isn't available until it is resolved here (M4), so this is
    /// the one place that can make "one statement, no embedded newline" hold
    /// unconditionally, for a secret's plaintext as much as for a typed or
    /// `advanced` value. A secret containing a line break is therefore
    /// refused here rather than rendered across two lines.
    fn body(
        &self,
        secrets: &BTreeMap<Uuid, String>,
        skip_type: bool,
    ) -> Result<String, RenderError> {
        let mut parts = Vec::new();
        for (k, v) in &self.properties {
            if skip_type && k == "type" {
                continue;
            }
            let value = match v {
                PropValue::Plain(s) => s.clone(),
                PropValue::Secret(id) => secrets
                    .get(id)
                    .cloned()
                    .ok_or(RenderError::MissingSecret(*id))?,
            };
            if value.contains(['\n', '\r']) {
                return Err(RenderError::LineBreak(k.clone()));
            }
            parts.push(format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(&value)));
        }
        Ok(parts.join(", "))
    }

    /// `CREATE` carries no `IF NOT EXISTS`: D3 decides absent/matching/drifted
    /// from `SHOW CATALOGS` first, and `IF NOT EXISTS` would silently adopt a
    /// catalog egret did not create.
    pub fn create_sql(&self, secrets: &BTreeMap<Uuid, String>) -> Result<String, RenderError> {
        Ok(format!(
            "CREATE EXTERNAL CATALOG `{}` PROPERTIES ({})",
            escape_ident(&self.name),
            self.body(secrets, false)?
        ))
    }

    /// `ALTER CATALOG … SET (…)` — not `SET PROPERTIES`, which is not in the
    /// grammar (StarRocks.g4:1140-1142). `type` is dropped: the analyzer
    /// refuses it by key even when the value is unchanged
    /// (`CatalogAnalyzer.java:39-41`). Properties merge; a key this render no
    /// longer emits stays on the FE, which is why a kind change is impossible
    /// by design (the registry forbids it) rather than by ALTER.
    pub fn alter_sql(&self, secrets: &BTreeMap<Uuid, String>) -> Result<String, RenderError> {
        Ok(format!(
            "ALTER CATALOG `{}` SET ({})",
            escape_ident(&self.name),
            self.body(secrets, true)?
        ))
    }

    /// `IF EXISTS` is in the grammar (StarRocks.g4:874-875), so a detach of a
    /// catalog the FE never had succeeds without parsing an error string (D7).
    pub fn drop_sql(&self) -> String {
        format!("DROP CATALOG IF EXISTS `{}`", escape_ident(&self.name))
    }

    /// The ONLY form that may be logged or shown (D6). Secret-valued
    /// properties become `******`, and so does any plain value under a
    /// sensitive key — `advanced` cannot hold one (validation refuses it), but
    /// a redaction that depends on that invariant holding elsewhere is a
    /// redaction waiting to fail.
    ///
    /// …and so does any plain value shaped like a URL (final whole-branch
    /// review, F4). `driver_url` matches no sensitive key, so it used to be
    /// printed verbatim — while `egret_engine_mgmt::starrocks_ddl`'s
    /// `redact_urls` masks exactly that value out of an FE error message, with
    /// a doc saying why: *"a `driver_url` (which can carry embedded HTTP Basic
    /// credentials)"*. An air-gapped deployment repointing
    /// `[starrocks.jdbc_drivers]` at an internal mirror behind basic auth, or a
    /// per-catalog `advanced.starrocks.driver_url` (validated for presence, not
    /// shape), wrote that credential into a WARN log through this function
    /// while the other half of the codebase called the same string a secret.
    /// The two now agree, and they agree by the same rule — the whole URL, not
    /// a cleverer partial mask that has to be right about where the userinfo
    /// ends.
    ///
    /// `form` picks the statement this renders as. It is NOT cosmetic: the
    /// only caller logs this next to a failed statement, and hard-coding
    /// `CREATE EXTERNAL CATALOG` reported a statement that was never sent
    /// whenever the failure was an `ALTER`.
    pub fn redacted(&self, form: StatementForm) -> String {
        let props: Vec<String> = self
            .properties
            .iter()
            // `ALTER CATALOG … SET` never re-sends `type` (CatalogAnalyzer
            // refuses it by key), so neither does its redaction — same rule as
            // `body(.., alter = true)`.
            .filter(|(k, _)| !(form == StatementForm::Alter && k.as_str() == "type"))
            .map(|(k, v)| {
                let shown = match v {
                    PropValue::Secret(_) => "******",
                    PropValue::Plain(_) if is_sensitive_key(k) => "******",
                    PropValue::Plain(s) if looks_like_url(s) => "******",
                    PropValue::Plain(s) => s.as_str(),
                };
                format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(shown))
            })
            .collect();
        let name = escape_ident(&self.name);
        match form {
            StatementForm::Create => format!(
                "CREATE EXTERNAL CATALOG `{name}` PROPERTIES ({})",
                props.join(", ")
            ),
            StatementForm::Alter => {
                format!("ALTER CATALOG `{name}` SET ({})", props.join(", "))
            }
        }
    }

    pub fn secret_ids(&self) -> BTreeSet<Uuid> {
        self.properties
            .values()
            .filter_map(|v| match v {
                PropValue::Secret(id) => Some(*id),
                PropValue::Plain(_) => None,
            })
            .collect()
    }

    /// The properties a drift comparison may use (Task 3): the plain ones only.
    /// A secret value can never be compared — `SHOW CREATE CATALOG` masks some
    /// of them and DELETES `password` outright (`CredentialUtil.java:88-91`).
    pub fn comparable(&self) -> BTreeMap<String, String> {
        self.properties
            .iter()
            .filter_map(|(k, v)| match v {
                PropValue::Plain(s) => Some((k.clone(), s.clone())),
                PropValue::Secret(_) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogDef, ValidationError};
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";
    const S3: &str = "44444444-4444-4444-4444-444444444444";
    const CRED: &str = "55555555-5555-5555-5555-555555555555";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn input() -> StarRocksRenderInput<'static> {
        StarRocksRenderInput {
            egret_url: "http://egret:8080",
            warehouse: Some(WarehouseRender {
                name: "wh-a",
                s3_endpoint: "http://rustfs:9000",
            }),
            storage: Some(StorageRender {
                endpoint: Some("http://rustfs:9000"),
                region: "eu-west-1",
                path_style: true,
                access_key: "AKIA",
                secret_key_secret_id: Some(id(S3)),
            }),
            credential_secret_id: Some(id(CRED)),
            drivers: &DEFAULT_SR_JDBC_DRIVERS,
        }
    }

    fn def(kind: &str, config: serde_json::Value, advanced: serde_json::Value) -> CatalogDef {
        CatalogDef::parse(kind, &config, &advanced).unwrap()
    }

    fn plain(pairs: &[(&str, &str)]) -> BTreeMap<String, PropValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), PropValue::Plain(v.to_string())))
            .collect()
    }

    fn secrets() -> BTreeMap<Uuid, String> {
        [
            (id(CRED), "cid:csecret".to_string()),
            (id(PW), "p4ssw0rd".to_string()),
            (id(S3), "s3kr3t".to_string()),
        ]
        .into_iter()
        .collect()
    }

    /// The egret warehouse catalog must render EXACTLY what
    /// `engines.rs::starrocks_external_catalog_sql` produced before SP3b, so an
    /// upgraded engine's FE sees no drift. OAuth2 uses the Iceberg-STANDARD
    /// property names (`credential`, `oauth2-server-uri`) reached through the
    /// `iceberg.catalog.` pass-through, NOT StarRocks' own
    /// `iceberg.catalog.oauth2.*` form: verified live on 3.3, where the dot-form
    /// is silently ignored and catalog reads fail with "missing bearer token"
    /// (StarRocks issue #57766).
    #[test]
    fn an_egret_warehouse_renders_the_ddl_the_bootstrap_used_to_build() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        assert_eq!(
            ddl.properties,
            [
                ("type".to_string(), PropValue::Plain("iceberg".into())),
                (
                    "iceberg.catalog.type".to_string(),
                    PropValue::Plain("rest".into())
                ),
                (
                    "iceberg.catalog.uri".to_string(),
                    PropValue::Plain("http://egret:8080/catalog".into())
                ),
                (
                    "iceberg.catalog.warehouse".to_string(),
                    PropValue::Plain("wh-a".into())
                ),
                (
                    "iceberg.catalog.credential".to_string(),
                    PropValue::Secret(id(CRED))
                ),
                (
                    "iceberg.catalog.oauth2-server-uri".to_string(),
                    PropValue::Plain("http://egret:8080/oauth/token".into())
                ),
                (
                    "iceberg.catalog.vended-credentials-enabled".to_string(),
                    PropValue::Plain("true".into())
                ),
                (
                    "aws.s3.endpoint".to_string(),
                    PropValue::Plain("http://rustfs:9000".into())
                ),
                (
                    "aws.s3.enable_ssl".to_string(),
                    PropValue::Plain("false".into())
                ),
                (
                    "aws.s3.region".to_string(),
                    PropValue::Plain("us-east-1".into())
                ),
                (
                    "aws.s3.enable_path_style_access".to_string(),
                    PropValue::Plain("true".into())
                ),
                (
                    "aws.s3.use_instance_profile".to_string(),
                    PropValue::Plain("false".into())
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        );
        let sql = ddl.create_sql(&secrets()).unwrap();
        assert!(
            sql.starts_with("CREATE EXTERNAL CATALOG `egret` PROPERTIES ("),
            "sql={sql}"
        );
        assert!(
            sql.contains(r#""iceberg.catalog.credential" = "cid:csecret""#),
            "sql={sql}"
        );
        assert!(
            sql.contains(
                r#""iceberg.catalog.oauth2-server-uri" = "http://egret:8080/oauth/token""#
            ),
            "sql={sql}"
        );
        assert!(
            !sql.contains("IF NOT EXISTS"),
            "D3: create is preceded by a read, never adopted blindly"
        );
        assert!(!sql.contains("iceberg.catalog.token"), "no static bearer");
        // D6: the redacted form is the only one that may be logged.
        let red = ddl.redacted(StatementForm::Create);
        assert!(
            red.contains(r#""iceberg.catalog.credential" = "******""#),
            "red={red}"
        );
        assert!(!red.contains("csecret"), "red={red}");
    }

    /// An egret-warehouse catalog carries its S3 configuration, and carries it
    /// WITHOUT a storage connection.
    ///
    /// `CatalogDef::validate` refuses `storage_connection_id` on the `egret`
    /// metastore, so `input.storage` is always `None` in production — this
    /// fixture models that. Before the fix the branch emitted no `aws.s3.*` at
    /// all, the FE fell back to its default AWS endpoint, and every object read
    /// failed `Bad Request (Service: S3, Status Code: 400)` (reproduced live on
    /// StarRocks 4.1.3, kind `kind-egret`).
    ///
    /// The assertions are on the rendered property MAP, not on the SQL string:
    /// the broken render produced perfectly valid SQL, so a test that only
    /// checked the statement parses would pass against the bug.
    ///
    /// The other discriminating half is the negative: NO static
    /// `aws.s3.access_key`/`aws.s3.secret_key`. MEASURED on 4.1.3 that vended
    /// credentials from the REST catalog serve the read (real rows returned)
    /// while the same MinIO refuses anonymous access with 403 — and there is no
    /// key to emit here anyway, since there is no storage connection.
    #[test]
    fn egret_warehouse_renders_s3_endpoint_region_and_path_style_without_storage() {
        let mut inp = input();
        inp.storage = None;
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &inp).unwrap();
        let props = &ddl.properties;

        assert_eq!(
            props.get("aws.s3.endpoint"),
            Some(&PropValue::Plain("http://rustfs:9000".into())),
            "the FE must be told where to talk; props={props:?}"
        );
        assert_eq!(
            props.get("aws.s3.region"),
            Some(&PropValue::Plain("us-east-1".into())),
            "props={props:?}"
        );
        assert_eq!(
            props.get("aws.s3.enable_path_style_access"),
            Some(&PropValue::Plain("true".into())),
            "props={props:?}"
        );
        // http endpoint -> ssl off, since the FE defaults it on.
        assert_eq!(
            props.get("aws.s3.enable_ssl"),
            Some(&PropValue::Plain("false".into())),
            "props={props:?}"
        );
        assert_eq!(
            props.get("iceberg.catalog.vended-credentials-enabled"),
            Some(&PropValue::Plain("true".into())),
            "props={props:?}"
        );

        // Negative: no static object credentials on this branch.
        assert!(
            !props.contains_key("aws.s3.access_key"),
            "an egret warehouse has no storage connection to take a key from; props={props:?}"
        );
        assert!(!props.contains_key("aws.s3.secret_key"), "props={props:?}");
        assert_eq!(
            props.get("aws.s3.use_instance_profile"),
            Some(&PropValue::Plain("false".into())),
            "props={props:?}"
        );
    }

    /// An https warehouse endpoint leaves `aws.s3.enable_ssl` on.
    #[test]
    fn egret_warehouse_https_endpoint_keeps_ssl_enabled() {
        let mut inp = input();
        inp.storage = None;
        inp.warehouse = Some(WarehouseRender {
            name: "wh-a",
            s3_endpoint: "https://s3.example.com",
        });
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &inp).unwrap();
        assert_eq!(
            ddl.properties.get("aws.s3.enable_ssl"),
            Some(&PropValue::Plain("true".into()))
        );
        assert_eq!(
            ddl.properties.get("aws.s3.endpoint"),
            Some(&PropValue::Plain("https://s3.example.com".into()))
        );
    }

    #[test]
    fn alter_omits_type_and_drop_is_if_exists() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        let alter = ddl.alter_sql(&secrets()).unwrap();
        assert!(
            alter.starts_with("ALTER CATALOG `egret` SET ("),
            "alter={alter}"
        );
        // CatalogAnalyzer refuses `type` by key, even unchanged.
        assert!(!alter.contains(r#""type" ="#), "alter={alter}");
        assert!(
            alter.contains(r#""iceberg.catalog.credential" = "cid:csecret""#),
            "alter={alter}"
        );
        assert_eq!(ddl.drop_sql(), "DROP CATALOG IF EXISTS `egret`");
    }

    #[test]
    fn iceberg_rest_hive_and_glue_with_s3() {
        let d = def(
            "iceberg",
            json!({
                "metastore": { "type": "rest", "uri": "https://cat/api", "warehouse": "lake",
                               "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "ALL", "token_uri": "https://idp/token" } },
                "storage_connection_id": SC
            }),
            json!(null),
        );
        let p = render_starrocks("lake", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("iceberg".into()));
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("rest".into()));
        assert_eq!(
            p["iceberg.catalog.uri"],
            PropValue::Plain("https://cat/api".into())
        );
        assert_eq!(
            p["iceberg.catalog.warehouse"],
            PropValue::Plain("lake".into())
        );
        assert_eq!(p["iceberg.catalog.credential"], PropValue::Secret(id(PW)));
        assert_eq!(p["iceberg.catalog.scope"], PropValue::Plain("ALL".into()));
        assert_eq!(
            p["iceberg.catalog.oauth2-server-uri"],
            PropValue::Plain("https://idp/token".into())
        );
        // The S3 block every non-egret lakehouse kind carries.
        assert_eq!(
            p["aws.s3.endpoint"],
            PropValue::Plain("http://rustfs:9000".into())
        );
        assert_eq!(p["aws.s3.region"], PropValue::Plain("eu-west-1".into()));
        assert_eq!(
            p["aws.s3.enable_path_style_access"],
            PropValue::Plain("true".into())
        );
        assert_eq!(
            p["aws.s3.use_instance_profile"],
            PropValue::Plain("false".into())
        );
        assert_eq!(p["aws.s3.access_key"], PropValue::Plain("AKIA".into()));
        assert_eq!(p["aws.s3.secret_key"], PropValue::Secret(id(S3)));
        // http endpoint ⇒ TLS off; the FE defaults this to `true`
        // (`AwsCloudConfiguration.java:40`), so it must be set explicitly.
        assert_eq!(p["aws.s3.enable_ssl"], PropValue::Plain("false".into()));

        let d = def(
            "iceberg",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!(null),
        );
        let p = render_starrocks("h", &d, &input()).unwrap().properties;
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("hive".into()));
        assert_eq!(
            p["hive.metastore.uris"],
            PropValue::Plain("thrift://hms:9083".into())
        );

        let d = def(
            "iceberg",
            json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKG", "secret_key_secret_id": PW }, "storage_connection_id": SC }),
            json!(null),
        );
        let p = render_starrocks("g", &d, &input()).unwrap().properties;
        assert_eq!(p["iceberg.catalog.type"], PropValue::Plain("glue".into()));
        assert_eq!(p["aws.glue.region"], PropValue::Plain("eu-west-1".into()));
        assert_eq!(
            p["aws.glue.use_instance_profile"],
            PropValue::Plain("false".into())
        );
        assert_eq!(p["aws.glue.access_key"], PropValue::Plain("AKG".into()));
        assert_eq!(p["aws.glue.secret_key"], PropValue::Secret(id(PW)));
    }

    /// StarRocks spells Delta Lake `deltalake`, with no underscore.
    #[test]
    fn hive_family_type_names() {
        for (kind, ty) in [
            ("hive", "hive"),
            ("delta_lake", "deltalake"),
            ("hudi", "hudi"),
        ] {
            let d = def(
                kind,
                json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
                json!(null),
            );
            let p = render_starrocks("x", &d, &input()).unwrap().properties;
            assert_eq!(p["type"], PropValue::Plain(ty.into()), "{kind}");
            assert_eq!(
                p["hive.metastore.type"],
                PropValue::Plain("hive".into()),
                "{kind}"
            );
            assert_eq!(
                p["hive.metastore.uris"],
                PropValue::Plain("thrift://hms:9083".into()),
                "{kind}"
            );
        }
        let d = def(
            "hive",
            json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC }),
            json!(null),
        );
        let p = render_starrocks("x", &d, &input()).unwrap().properties;
        assert_eq!(p["hive.metastore.type"], PropValue::Plain("glue".into()));
        assert_eq!(
            p["aws.glue.use_instance_profile"],
            PropValue::Plain("true".into()),
            "no key ⇒ instance profile"
        );
    }

    /// One `jdbc` type for every dialect, with the pinned driver — and the URI
    /// in the vocabulary the driver the FE actually loads understands.
    #[test]
    fn jdbc_per_dialect_with_the_pinned_driver() {
        let cases = [
            ("postgresql", Some("sales"), "org.postgresql.Driver", "postgresql",
             "jdbc:postgresql://db:5432/sales?sslmode=require"),
            // The FE rewrites `jdbc:mysql` → `jdbc:mariadb` and loads
            // org.mariadb.jdbc.Driver whatever we send (GPL avoidance,
            // JDBCMetadata.java:121-128,187-194) — so egret sends the MariaDB
            // form itself, whose sslMode vocabulary is `disable|trust|verify-full`,
            // not MySQL's `DISABLED|REQUIRED|VERIFY_IDENTITY`.
            ("mysql", None, "org.mariadb.jdbc.Driver", "mysql",
             "jdbc:mariadb://db:5432?sslMode=trust"),
            ("mariadb", None, "org.mariadb.jdbc.Driver", "mysql",
             "jdbc:mariadb://db:5432?sslMode=trust"),
            ("oracle", Some("ORCL"), "oracle.jdbc.driver.OracleDriver", "oracle",
             "jdbc:oracle:thin:@tcps://db:5432/ORCL"),
            ("sqlserver", Some("sales"), "com.microsoft.sqlserver.jdbc.SQLServerDriver", "sqlserver",
             "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=true"),
            ("clickhouse", Some("events"), "com.clickhouse.jdbc.ClickHouseDriver", "clickhouse",
             "jdbc:clickhouse://db:5432/events?ssl=true&sslmode=none"),
        ];
        for (dialect, database, class, resolver, uri) in cases {
            let mut cfg = json!({ "dialect": dialect, "host": "db", "port": 5432, "user": "u",
                                  "password_secret_id": PW, "ssl": { "mode": "require" } });
            if let Some(d) = database {
                cfg["database"] = json!(d);
            }
            let d = def("jdbc", cfg, json!(null));
            let p = render_starrocks("j", &d, &input()).unwrap().properties;
            assert_eq!(p["type"], PropValue::Plain("jdbc".into()), "{dialect}");
            assert_eq!(p["jdbc_uri"], PropValue::Plain(uri.into()), "{dialect}");
            assert_eq!(p["user"], PropValue::Plain("u".into()), "{dialect}");
            assert_eq!(p["password"], PropValue::Secret(id(PW)), "{dialect}");
            assert_eq!(
                p["driver_class"],
                PropValue::Plain(class.into()),
                "{dialect}"
            );
            assert_eq!(
                p["schema_resolver"],
                PropValue::Plain(resolver.into()),
                "{dialect}"
            );
            assert_eq!(
                p["driver_url"],
                PropValue::Plain(
                    DEFAULT_SR_JDBC_DRIVERS
                        .get(d.jdbc_dialect().unwrap())
                        .unwrap()
                        .driver_url
                        .clone()
                ),
                "{dialect}"
            );
            // The FE computes `checksum` itself from `driver_url`; sending one
            // would show up as permanent drift (JDBCConnector.java:52-56).
            assert!(!p.contains_key("checksum"), "{dialect}");
        }
    }

    /// D5: the two overridable keys, and the one that is still a collision.
    #[test]
    fn the_driver_can_be_overridden_per_catalog_but_nothing_else_can() {
        let d = def(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            json!({ "starrocks": { "driver_url": "file:///opt/pg.jar", "driver_class": "org.postgresql.Driver" } }),
        );
        let p = render_starrocks("j", &d, &input()).unwrap().properties;
        assert_eq!(
            p["driver_url"],
            PropValue::Plain("file:///opt/pg.jar".into())
        );
        assert_eq!(
            p["driver_class"],
            PropValue::Plain("org.postgresql.Driver".into())
        );

        // Anything else that the form sets is still refused — at parse time.
        let err = CatalogDef::parse(
            "jdbc",
            &json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                     "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            &json!({ "starrocks": { "jdbc_uri": "jdbc:postgresql://evil/x" } }),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::Collision {
                engine: "starrocks".into(),
                key: "jdbc_uri".into(),
                kind: "jdbc".into()
            }
        );
    }

    /// D5: `with_overrides` repoints only the field(s) a deployment actually
    /// named — one dialect's `driver_url`, say — and leaves every other field
    /// of every other dialect at its compiled-in pinned value.
    #[test]
    fn with_overrides_repoints_only_the_named_field_of_the_named_dialect() {
        let mut settings = egret_config::StarRocksSettings::default();
        settings.jdbc_drivers.mysql = Some(egret_config::JdbcDriverOverride {
            driver_url: Some("file:///opt/mirror/mariadb.jar".into()),
            driver_class: None,
        });
        let drivers = DEFAULT_SR_JDBC_DRIVERS.with_overrides(&settings);

        // The named field changed.
        assert_eq!(
            drivers.get(JdbcDialect::Mysql).unwrap().driver_url,
            "file:///opt/mirror/mariadb.jar"
        );
        // The field the override left unnamed on that SAME dialect kept its
        // pinned value.
        assert_eq!(
            drivers.get(JdbcDialect::Mysql).unwrap().driver_class,
            DEFAULT_SR_JDBC_DRIVERS
                .get(JdbcDialect::Mysql)
                .unwrap()
                .driver_class
        );
        // `mariadb` shares mysql's pinned entry but has no override of its
        // own, so it is untouched.
        assert_eq!(
            drivers.get(JdbcDialect::Mariadb).unwrap(),
            DEFAULT_SR_JDBC_DRIVERS.get(JdbcDialect::Mariadb).unwrap()
        );
        // Every other dialect is untouched too.
        for d in [
            JdbcDialect::Postgresql,
            JdbcDialect::Oracle,
            JdbcDialect::Sqlserver,
            JdbcDialect::Clickhouse,
        ] {
            assert_eq!(
                drivers.get(d).unwrap(),
                DEFAULT_SR_JDBC_DRIVERS.get(d).unwrap(),
                "{d:?}"
            );
        }
    }

    /// A `[starrocks.jdbc_drivers]` entry overridden to an empty `driver_url`
    /// is an operator's deliberate "not available here" (module doc on
    /// `JdbcDrivers::get`): `get` reads that as `None`, and a `jdbc` catalog
    /// of that dialect with no PER-CATALOG override then fails to render.
    #[test]
    fn a_blanked_override_disables_the_dialect_unless_the_catalog_overrides_it_too() {
        let mut settings = egret_config::StarRocksSettings::default();
        settings.jdbc_drivers.postgresql = Some(egret_config::JdbcDriverOverride {
            driver_url: Some(String::new()),
            driver_class: None,
        });
        let drivers = DEFAULT_SR_JDBC_DRIVERS.with_overrides(&settings);
        assert!(drivers.get(JdbcDialect::Postgresql).is_none());

        let d = def(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            json!(null),
        );
        let mut in_: StarRocksRenderInput<'_> = input();
        in_.drivers = &drivers;
        assert_eq!(
            render_starrocks("j", &d, &in_).unwrap_err(),
            RenderError::NoJdbcDriver(JdbcDialect::Postgresql)
        );

        // A per-catalog override still fills the gap.
        let d = def(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            json!({ "starrocks": { "driver_url": "file:///opt/pg.jar", "driver_class": "org.postgresql.Driver" } }),
        );
        let p = render_starrocks("j", &d, &in_).unwrap().properties;
        assert_eq!(
            p["driver_url"],
            PropValue::Plain("file:///opt/pg.jar".into())
        );
    }

    /// Same as the `driver_url` case above, but for `driver_class` alone.
    /// `get` must treat EITHER field being blanked as "not available here" —
    /// checking only `driver_url` would let a deployment override that blanks
    /// `driver_class` alone reach the FE as `driver_class = ""`, which comes
    /// back as an FE error instead of egret's own clean `NoJdbcDriver` refusal.
    #[test]
    fn a_blanked_driver_class_override_also_disables_the_dialect() {
        let mut settings = egret_config::StarRocksSettings::default();
        settings.jdbc_drivers.postgresql = Some(egret_config::JdbcDriverOverride {
            driver_url: None,
            driver_class: Some(String::new()),
        });
        let drivers = DEFAULT_SR_JDBC_DRIVERS.with_overrides(&settings);
        assert!(drivers.get(JdbcDialect::Postgresql).is_none());

        let d = def(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            json!(null),
        );
        let mut in_: StarRocksRenderInput<'_> = input();
        in_.drivers = &drivers;
        assert_eq!(
            render_starrocks("j", &d, &in_).unwrap_err(),
            RenderError::NoJdbcDriver(JdbcDialect::Postgresql)
        );
    }

    #[test]
    fn elasticsearch() {
        let d = def(
            "elasticsearch",
            json!({ "hosts": ["es-a", "es-b"], "port": 9200, "tls": true,
                                             "user": "u", "password_secret_id": PW }),
            json!(null),
        );
        let p = render_starrocks("e", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("es".into()));
        assert_eq!(
            p["hosts"],
            PropValue::Plain("https://es-a:9200,https://es-b:9200".into())
        );
        assert_eq!(p["es.net.ssl"], PropValue::Plain("true".into()));
        assert_eq!(p["user"], PropValue::Plain("u".into()));
        assert_eq!(p["password"], PropValue::Secret(id(PW)));
    }

    /// D4 correction: StarRocks has no `BENCHMARK` connector on any release
    /// branch or tag (`c019a148`, 2026-01-22, `main`-only) — a `tpch`/`tpcds`
    /// catalog must never reach a live FE as `type = "benchmark"` DDL. Two
    /// independent refusals are proven here: `can_read` itself, and
    /// `render_starrocks` refusing even when handed a `tpch`/`tpcds`
    /// `CatalogDef` directly (the only production entry point, and it
    /// refuses before the per-kind match is reached).
    #[test]
    fn tpch_and_tpcds_are_refused_on_starrocks_pending_a_supporting_fe() {
        for kind in ["tpch", "tpcds"] {
            let d = def(kind, json!({ "scale": 10 }), json!(null));
            assert!(!d.can_read(EngineType::StarRocks), "{kind}");
            assert_eq!(
                render_starrocks("b", &d, &input()).unwrap_err(),
                RenderError::NotReadable("b".into()),
                "{kind}"
            );
            // Trino is unaffected — same catalog, other engine.
            assert!(d.can_read(EngineType::Trino), "{kind}");
        }
    }

    #[test]
    fn generic_is_advanced_plus_its_secret_properties() {
        let d = def(
            "generic",
            json!({ "engines": ["starrocks"], "secret_properties": { "starrocks": { "paimon.password": PW } } }),
            json!({ "starrocks": { "type": "paimon", "paimon.catalog.type": "filesystem" } }),
        );
        let p = render_starrocks("pm", &d, &input()).unwrap().properties;
        assert_eq!(p["type"], PropValue::Plain("paimon".into()));
        assert_eq!(
            p["paimon.catalog.type"],
            PropValue::Plain("filesystem".into())
        );
        assert_eq!(p["paimon.password"], PropValue::Secret(id(PW)));

        // A Trino-only generic is not renderable here.
        let d = def(
            "generic",
            json!({ "engines": ["trino"] }),
            json!({ "trino": { "connector.name": "memory" } }),
        );
        assert_eq!(
            render_starrocks("m", &d, &input()).unwrap_err(),
            RenderError::NotReadable("m".into())
        );
    }

    /// The escaping tests that guarded `starrocks_external_catalog_sql`, moved
    /// verbatim: this DDL runs as `root` on the FE and its inputs (warehouse
    /// name, registry name) are not charset-validated everywhere upstream.
    #[test]
    fn values_and_identifiers_cannot_break_out() {
        let evil = r#"wh" ) ; DROP DATABASE x ; -- "#;
        assert_eq!(escape_dq(evil), r#"wh\" ) ; DROP DATABASE x ; -- "#);
        assert_eq!(escape_dq(r#"a\b"#), r#"a\\b"#);
        assert_eq!(escape_ident("cat`evil"), "cat``evil");
        let ddl = StarRocksCatalogDdl {
            name: "cat`evil".into(),
            properties: plain(&[("type", "iceberg"), ("iceberg.catalog.warehouse", evil)]),
        };
        let sql = ddl.create_sql(&secrets()).unwrap();
        assert!(sql.contains("`cat``evil`"), "sql={sql}");
        assert!(!sql.contains(r#"wh" ) ; DROP DATABASE x"#), "sql={sql}");
        assert_eq!(ddl.drop_sql(), "DROP CATALOG IF EXISTS `cat``evil`");
    }

    #[test]
    fn a_reserved_name_is_refused_and_a_missing_secret_is_an_error_not_an_empty_value() {
        let d = CatalogDef::egret_warehouse(id(WH));
        assert_eq!(
            render_starrocks("default_catalog", &d, &input()).unwrap_err(),
            RenderError::ReservedName("default_catalog".into())
        );
        assert!(is_reserved_starrocks_name(
            "resource_mapping_inside_catalog_x"
        ));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        assert_eq!(
            ddl.create_sql(&BTreeMap::new()).unwrap_err(),
            RenderError::MissingSecret(id(CRED))
        );
    }

    /// D6: `scrub_secrets` is what stands between an FE error and
    /// `apply_error`. `create_sql`/`alter_sql` never interpolate a secret's
    /// raw plaintext, only `escape_dq`'s output — so an FE error that quotes
    /// the rejected statement back quotes the ESCAPED form. A password with a
    /// `"`, a `\` and a newline exercises exactly that (C1): the raw text
    /// never appears verbatim in valid DDL, only its escaped form does.
    #[test]
    fn an_fe_error_quoting_the_statement_is_scrubbed() {
        let nasty = "p4ss\"w0rd\\x\ntrailer";
        let mut secrets = secrets();
        secrets.insert(id(PW), nasty.to_string());
        let escaped = escape_dq(nasty);
        let err = format!(
            "Getting analyzing error. Detail message: \
             CREATE EXTERNAL CATALOG `x` PROPERTIES (\"password\" = \"{escaped}\") is invalid"
        );
        let out = scrub_secrets(&err, &secrets);
        assert!(!out.contains(nasty), "out={out}");
        assert!(!out.contains(&escaped), "out={out}");
        assert!(out.contains("******"), "out={out}");
        assert!(
            out.contains("Getting analyzing error"),
            "the diagnosis survives; out={out}"
        );
    }

    /// M2: `driver_url`/`driver_class` are exempt from the collision check
    /// only where JDBC's typed arm actually consumes them. On any other kind
    /// they are ordinary `advanced.starrocks` properties — rendered, not
    /// silently dropped, and still subject to the normal collision rule.
    #[test]
    fn sr_overridable_keys_are_only_exempt_for_jdbc_catalogs() {
        let d = def(
            "hive",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!({ "starrocks": { "driver_url": "file:///opt/x.jar" } }),
        );
        let p = render_starrocks("h", &d, &input()).unwrap().properties;
        assert_eq!(
            p["driver_url"],
            PropValue::Plain("file:///opt/x.jar".into())
        );
    }

    /// M4: a secret whose plaintext contains a line break cannot be resolved
    /// into DDL text — `render_starrocks`'s line-break refusal only ever sees
    /// `PropValue::Plain` values (a secret's plaintext isn't known at render
    /// time), so the invariant is enforced again once `create_sql`/
    /// `alter_sql` resolve the secret. It must also assert nothing else
    /// changed about the DDL (the refusal is a refusal, not a rewrite).
    #[test]
    fn a_secret_line_break_is_refused_at_ddl_time_not_silently_split_across_lines() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();
        let before = ddl.clone();
        let mut secrets = secrets();
        secrets.insert(id(CRED), "line1\nline2".to_string());
        assert_eq!(
            ddl.create_sql(&secrets).unwrap_err(),
            RenderError::LineBreak("iceberg.catalog.credential".into())
        );
        assert_eq!(
            ddl.alter_sql(&secrets).unwrap_err(),
            RenderError::LineBreak("iceberg.catalog.credential".into())
        );
        assert_eq!(ddl, before, "a refused render must not mutate the ddl");
    }

    /// M7: `secret_ids`, `comparable` and `redacted` for a `generic` catalog
    /// (no typed arm at all), whose one secret comes from
    /// `secret_properties.starrocks` (`paimon.password`) rather than any
    /// typed field — the pass-through path, as opposed to the typed-kind
    /// coverage the other tests in this module already give.
    #[test]
    fn secret_ids_comparable_and_redacted_for_a_non_egret_kind() {
        let d = def(
            "generic",
            json!({ "engines": ["starrocks"], "secret_properties": { "starrocks": { "paimon.password": PW } } }),
            json!({ "starrocks": { "type": "paimon", "paimon.catalog.type": "filesystem" } }),
        );
        let ddl = render_starrocks("pm", &d, &input()).unwrap();

        assert_eq!(
            ddl.secret_ids(),
            [id(PW)].into_iter().collect::<BTreeSet<_>>()
        );

        let comparable = ddl.comparable();
        assert_eq!(
            comparable,
            [
                ("type".to_string(), "paimon".to_string()),
                ("paimon.catalog.type".to_string(), "filesystem".to_string()),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        );
        assert!(
            !comparable.contains_key("paimon.password"),
            "a secret-valued property must never be comparable: comparable={comparable:?}"
        );

        let red = ddl.redacted(StatementForm::Create);
        assert!(red.contains(r#""type" = "paimon""#), "red={red}");
        assert!(
            red.contains(r#""paimon.catalog.type" = "filesystem""#),
            "red={red}"
        );
        assert!(red.contains(r#""paimon.password" = "******""#), "red={red}");
        assert!(!red.contains("p4ssw0rd"), "red={red}");
    }

    /// F4 (final whole-branch review), half one: `redacted` is the only form
    /// of a statement that may be logged, and it used to print `driver_url`
    /// verbatim — while `egret_engine_mgmt::starrocks_ddl::redact_urls` masks
    /// exactly that value out of an FE ERROR, documenting it as able to carry
    /// embedded HTTP Basic credentials. The two halves of the codebase
    /// disagreed about the same string; the log was the half that leaked.
    ///
    /// The rule is now the URL SHAPE, the same judgement `redact_urls` makes,
    /// so `jdbc_uri` goes with it: a JDBC URI can carry a password in its
    /// query string just as a driver mirror can carry one in its userinfo.
    /// What a reader of the log keeps is the catalog name (in the log line
    /// itself), every property KEY, and every non-URL value.
    #[test]
    fn redacted_masks_url_valued_properties_the_way_fe_errors_are_redacted() {
        let d = def(
            "jdbc",
            json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "s",
                    "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            json!({ "starrocks": {
                "driver_url": "https://mirror-user:hunter2@artifacts.internal/pg.jar",
                "driver_class": "org.postgresql.Driver" } }),
        );
        let ddl = render_starrocks("j", &d, &input()).unwrap();
        // Premise: the credential really is in the rendered property, so this
        // is about the redaction and not about the render.
        assert_eq!(
            ddl.properties["driver_url"],
            PropValue::Plain("https://mirror-user:hunter2@artifacts.internal/pg.jar".into())
        );

        let red = ddl.redacted(StatementForm::Create);
        assert!(
            !red.contains("hunter2"),
            "a driver-mirror credential reached a log: {red}"
        );
        assert!(
            red.contains(r#""driver_url" = "******""#),
            "the key still names what was withheld: {red}"
        );
        assert!(
            red.contains(r#""jdbc_uri" = "******""#),
            "same rule for the other URL-valued property: {red}"
        );
        // Everything that is not a URL and not a secret still reads.
        assert!(red.contains(r#""user" = "u""#), "red={red}");
        assert!(red.contains(r#""type" = "jdbc""#), "red={red}");
        assert!(
            red.contains(r#""driver_class" = "org.postgresql.Driver""#),
            "a class name is not a URL: {red}"
        );
        assert!(red.contains(r#""password" = "******""#), "red={red}");
    }

    /// The predicate itself, since it decides what a WARN log may carry.
    /// Positives are the shapes `redact_urls`' regex matches; negatives are
    /// the ordinary property values that must stay readable.
    #[test]
    fn looks_like_url_matches_what_redact_urls_matches() {
        assert!(looks_like_url("https://artifacts.internal/pg.jar"));
        assert!(looks_like_url("file:///opt/pg.jar"));
        assert!(looks_like_url("thrift://hms:9083"));
        assert!(looks_like_url("s3://bucket/prefix"));
        // A JDBC URI's inner scheme is what `redact_urls` matches on too.
        assert!(looks_like_url("jdbc:postgresql://db:5432/s"));

        assert!(!looks_like_url("org.postgresql.Driver"));
        assert!(!looks_like_url("hive"));
        assert!(!looks_like_url("us-east-1"));
        // One negative per guard, so each can be killed on its own:
        assert!(!looks_like_url("://no-scheme"), "nothing before the `://`");
        assert!(!looks_like_url("https://"), "a bare scheme names nothing");
        assert!(
            !looks_like_url("42://not-a-scheme"),
            "a scheme starts with a letter — the same rule `redact_urls`' \
             `[A-Za-z][A-Za-z0-9+.-]*://` encodes"
        );
    }

    /// F4, half two: the failure log for an `ALTER CATALOG … SET` used to
    /// report a `CREATE EXTERNAL CATALOG` — a statement that was never sent.
    /// `redacted` now renders the form it is told, and drops `type` for an
    /// ALTER exactly as `alter_sql` does (CatalogAnalyzer refuses the key).
    #[test]
    fn redacted_renders_the_statement_that_actually_ran() {
        let d = CatalogDef::egret_warehouse(id(WH));
        let ddl = render_starrocks("egret", &d, &input()).unwrap();

        let create = ddl.redacted(StatementForm::Create);
        assert!(
            create.starts_with("CREATE EXTERNAL CATALOG `egret` PROPERTIES ("),
            "create={create}"
        );
        assert!(create.contains(r#""type" = "iceberg""#), "create={create}");

        let alter = ddl.redacted(StatementForm::Alter);
        assert!(
            alter.starts_with("ALTER CATALOG `egret` SET ("),
            "alter={alter}"
        );
        assert!(
            !alter.contains(r#""type" ="#),
            "an ALTER never re-sends `type`, so neither does its redaction: {alter}"
        );
        assert!(
            alter.contains(r#""iceberg.catalog.credential" = "******""#),
            "and the secret is still masked in this form: {alter}"
        );
    }
}
