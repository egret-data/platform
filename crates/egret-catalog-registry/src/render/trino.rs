//! Trino rendering: a catalog definition + resolved inputs → one
//! `<name>.properties` file and the env entries its secret placeholders read.
//! Pure: secrets stay ids; the caller resolves them to plaintext at apply time.

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::config::{
    CatalogConfig, CatalogDef, EngineType, HiveMetastore, IcebergMetastore, JdbcConfig,
    JdbcDialect, JdbcSsl, RestAuth,
};

use super::{RenderError, StorageRender, WarehouseRender};

#[derive(Clone, Copy, Debug)]
pub struct TrinoRenderInput<'a> {
    /// Namespaces this catalog's env vars (`C0`, `C1`, … by attachment order).
    pub env_prefix: &'a str,
    /// In-cluster egret URL (REST catalog + token endpoint).
    pub egret_url: &'a str,
    pub warehouse: Option<WarehouseRender<'a>>,
    pub storage: Option<StorageRender<'a>>,
    /// The attachment's minted Catalog Client credential (egret catalogs only).
    pub credential_secret_id: Option<Uuid>,
}

impl TrinoRenderInput<'static> {
    /// Inputs that satisfy every kind: rendering with these tells which keys the
    /// typed form sets, which is what the save-time collision check needs.
    pub fn probe() -> Self {
        TrinoRenderInput {
            env_prefix: "P",
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrinoCatalogFile {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    /// env var → secret id, for every `${ENV:…}` placeholder in `properties`.
    pub env: BTreeMap<String, Uuid>,
}

impl TrinoCatalogFile {
    /// (plaintext properties, property → secret id).
    pub fn split_secret_properties(&self) -> (BTreeMap<String, String>, BTreeMap<String, Uuid>) {
        let mut plain = BTreeMap::new();
        let mut secret = BTreeMap::new();
        for (k, v) in &self.properties {
            let var = v.strip_prefix("${ENV:").and_then(|r| r.strip_suffix('}'));
            match var.and_then(|var| self.env.get(var)) {
                Some(id) => {
                    secret.insert(k.clone(), *id);
                }
                None => {
                    plain.insert(k.clone(), v.clone());
                }
            }
        }
        (plain, secret)
    }
}

struct Out<'a> {
    props: BTreeMap<String, String>,
    env: BTreeMap<String, Uuid>,
    prefix: &'a str,
    next: usize,
}

impl Out<'_> {
    fn set(&mut self, k: &str, v: impl Into<String>) {
        self.props.insert(k.to_string(), v.into());
    }

    fn secret(&mut self, k: &str, id: Uuid) {
        let var = format!("CAT_{}_{}", self.prefix, self.next);
        self.next += 1;
        self.props.insert(k.to_string(), format!("${{ENV:{var}}}"));
        self.env.insert(var, id);
    }

    fn s3(&mut self, input: &TrinoRenderInput<'_>) -> Result<(), RenderError> {
        let s = input
            .storage
            .as_ref()
            .ok_or(RenderError::MissingInput("storage connection"))?;
        self.set("fs.native-s3.enabled", "true");
        if let Some(ep) = s.endpoint.filter(|e| !e.is_empty()) {
            self.set("s3.endpoint", ep);
        }
        self.set("s3.region", s.region);
        self.set(
            "s3.path-style-access",
            if s.path_style { "true" } else { "false" },
        );
        self.set("s3.aws-access-key", s.access_key);
        if let Some(id) = s.secret_key_secret_id {
            self.secret("s3.aws-secret-key", id);
        }
        Ok(())
    }

    fn glue(&mut self, region: &str, access_key: Option<&str>, secret: Option<Uuid>) {
        self.set("hive.metastore.glue.region", region);
        if let Some(a) = access_key {
            self.set("hive.metastore.glue.aws-access-key", a);
        }
        if let Some(id) = secret {
            self.secret("hive.metastore.glue.aws-secret-key", id);
        }
    }
}

/// The JDBC URL Trino's connector for `c.dialect` gets. Every form states its TLS
/// mode explicitly — driver defaults (`prefer`, `PREFERRED`) are never implied.
pub fn jdbc_url(c: &JdbcConfig) -> String {
    let (h, p) = (&c.host, c.port);
    let db = c.database.as_deref().unwrap_or("");
    let pick = |off: &'static str, req: &'static str, full: &'static str| match c.ssl {
        JdbcSsl::Disable => off,
        JdbcSsl::Require => req,
        JdbcSsl::VerifyFull { .. } => full,
    };
    match c.dialect {
        JdbcDialect::Postgresql => format!(
            "jdbc:postgresql://{h}:{p}/{db}?sslmode={}",
            pick("disable", "require", "verify-full")
        ),
        JdbcDialect::Mysql => format!(
            "jdbc:mysql://{h}:{p}?sslMode={}",
            pick("DISABLED", "REQUIRED", "VERIFY_IDENTITY")
        ),
        JdbcDialect::Mariadb => format!(
            "jdbc:mariadb://{h}:{p}?sslMode={}",
            pick("disable", "trust", "verify-full")
        ),
        JdbcDialect::Oracle => match c.ssl {
            JdbcSsl::Disable => format!("jdbc:oracle:thin:@//{h}:{p}/{db}"),
            JdbcSsl::Require => format!("jdbc:oracle:thin:@tcps://{h}:{p}/{db}"),
            JdbcSsl::VerifyFull { .. } => {
                format!("jdbc:oracle:thin:@tcps://{h}:{p}/{db}?oracle.net.ssl_server_dn_match=true")
            }
        },
        JdbcDialect::Sqlserver => format!(
            "jdbc:sqlserver://{h}:{p};databaseName={db};{}",
            pick(
                "encrypt=false",
                "encrypt=true;trustServerCertificate=true",
                "encrypt=true;trustServerCertificate=false"
            )
        ),
        JdbcDialect::Clickhouse => format!(
            "jdbc:clickhouse://{h}:{p}/{db}?{}",
            pick(
                "ssl=false",
                "ssl=true&sslmode=none",
                "ssl=true&sslmode=strict"
            )
        ),
    }
}

pub fn render_trino(
    name: &str,
    def: &CatalogDef,
    input: &TrinoRenderInput<'_>,
) -> Result<TrinoCatalogFile, RenderError> {
    if !def.can_read(EngineType::Trino) {
        return Err(RenderError::NotReadable(name.to_string()));
    }
    let mut out = Out {
        props: BTreeMap::new(),
        env: BTreeMap::new(),
        prefix: input.env_prefix,
        next: 0,
    };
    match &def.config {
        CatalogConfig::Iceberg(c) => {
            out.set("connector.name", "iceberg");
            match &c.metastore {
                IcebergMetastore::Egret { .. } => {
                    let wh = input
                        .warehouse
                        .as_ref()
                        .ok_or(RenderError::MissingInput("warehouse"))?;
                    let credential = input
                        .credential_secret_id
                        .ok_or(RenderError::MissingInput("catalog client credential"))?;
                    let var = format!("EGRET_{}", input.env_prefix);
                    out.set("iceberg.catalog.type", "rest");
                    out.set(
                        "iceberg.rest-catalog.uri",
                        format!("{}/catalog", input.egret_url),
                    );
                    out.set("iceberg.rest-catalog.warehouse", wh.name);
                    out.set("iceberg.rest-catalog.security", "OAUTH2");
                    out.set(
                        "iceberg.rest-catalog.oauth2.server-uri",
                        format!("{}/oauth/token", input.egret_url),
                    );
                    out.set(
                        "iceberg.rest-catalog.oauth2.credential",
                        format!("${{ENV:{var}}}"),
                    );
                    out.env.insert(var, credential);
                    out.set("iceberg.rest-catalog.vended-credentials-enabled", "true");
                    out.set("fs.native-s3.enabled", "true");
                    out.set("s3.endpoint", wh.s3_endpoint);
                    out.set("s3.region", "us-east-1");
                    out.set("s3.path-style-access", "true");
                }
                IcebergMetastore::Rest {
                    uri,
                    warehouse,
                    auth,
                } => {
                    out.set("iceberg.catalog.type", "rest");
                    out.set("iceberg.rest-catalog.uri", uri.as_str());
                    out.set("iceberg.rest-catalog.warehouse", warehouse.as_str());
                    if let RestAuth::Oauth2 {
                        credential_secret_id,
                        scope,
                        token_uri,
                    } = auth
                    {
                        out.set("iceberg.rest-catalog.security", "OAUTH2");
                        out.secret(
                            "iceberg.rest-catalog.oauth2.credential",
                            *credential_secret_id,
                        );
                        if let Some(s) = scope {
                            out.set("iceberg.rest-catalog.oauth2.scope", s.as_str());
                        }
                        if let Some(t) = token_uri {
                            out.set("iceberg.rest-catalog.oauth2.server-uri", t.as_str());
                        }
                    }
                    out.s3(input)?;
                }
                IcebergMetastore::Hive { uri } => {
                    out.set("iceberg.catalog.type", "hive_metastore");
                    out.set("hive.metastore.uri", uri.as_str());
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
            out.set("connector.name", def.trino_connector_name());
            match &c.metastore {
                HiveMetastore::Hive { uri } => out.set("hive.metastore.uri", uri.as_str()),
                HiveMetastore::Glue {
                    region,
                    access_key,
                    secret_key_secret_id,
                } => {
                    out.set("hive.metastore", "glue");
                    out.glue(region, access_key.as_deref(), *secret_key_secret_id);
                }
            }
            out.s3(input)?;
        }
        CatalogConfig::Jdbc(c) => {
            out.set("connector.name", c.dialect.as_str());
            out.set("connection-url", jdbc_url(c));
            out.set("connection-user", c.user.as_str());
            out.secret("connection-password", c.password_secret_id);
        }
        CatalogConfig::Elasticsearch(c) => {
            out.set("connector.name", "elasticsearch");
            out.set("elasticsearch.host", c.hosts.join(","));
            out.set("elasticsearch.port", c.port.to_string());
            out.set(
                "elasticsearch.tls.enabled",
                if c.tls { "true" } else { "false" },
            );
            if let Some(user) = &c.user {
                out.set("elasticsearch.security", "PASSWORD");
                out.set("elasticsearch.auth.user", user.as_str());
                if let Some(id) = c.password_secret_id {
                    out.secret("elasticsearch.auth.password", id);
                }
            }
        }
        CatalogConfig::Tpch(_) => out.set("connector.name", "tpch"),
        CatalogConfig::Tpcds(_) => out.set("connector.name", "tpcds"),
        CatalogConfig::Generic(g) => {
            for (k, v) in &def.advanced.trino {
                out.set(k, v.as_str());
            }
            for (k, id) in &g.secret_properties.trino {
                out.secret(k, *id);
            }
        }
    }
    if !matches!(def.config, CatalogConfig::Generic(_)) {
        for (k, v) in &def.advanced.trino {
            if out.props.contains_key(k) {
                return Err(RenderError::Collision(k.clone()));
            }
            out.set(k, v.as_str());
        }
    }
    if let Some((k, _)) = out.props.iter().find(|(_, v)| v.contains(['\n', '\r'])) {
        return Err(RenderError::LineBreak(k.clone()));
    }
    Ok(TrinoCatalogFile {
        name: name.to_string(),
        properties: out.props,
        env: out.env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CatalogDef;
    use serde_json::json;

    const PW: &str = "11111111-1111-1111-1111-111111111111";
    const SC: &str = "22222222-2222-2222-2222-222222222222";
    const WH: &str = "33333333-3333-3333-3333-333333333333";
    const S3: &str = "44444444-4444-4444-4444-444444444444";
    const CRED: &str = "55555555-5555-5555-5555-555555555555";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn input() -> TrinoRenderInput<'static> {
        TrinoRenderInput {
            env_prefix: "C0",
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
        }
    }

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn def(kind: &str, config: serde_json::Value, advanced: serde_json::Value) -> CatalogDef {
        CatalogDef::parse(kind, &config, &advanced).unwrap()
    }

    const S3_PROPS: [(&str, &str); 6] = [
        ("fs.native-s3.enabled", "true"),
        ("s3.endpoint", "http://rustfs:9000"),
        ("s3.region", "eu-west-1"),
        ("s3.path-style-access", "true"),
        ("s3.aws-access-key", "AKIA"),
        ("s3.aws-secret-key", "${ENV:CAT_C0_0}"),
    ];

    /// Parity with the `egret-warehouse` arm `build_catalog_req` had before SP3a.
    #[test]
    fn an_egret_warehouse_renders_exactly_as_before_the_registry() {
        let f = render_trino("egret", &CatalogDef::egret_warehouse(id(WH)), &input()).unwrap();
        assert_eq!(f.name, "egret");
        assert_eq!(
            f.properties,
            props(&[
                ("connector.name", "iceberg"),
                ("iceberg.catalog.type", "rest"),
                ("iceberg.rest-catalog.uri", "http://egret:8080/catalog"),
                ("iceberg.rest-catalog.warehouse", "wh-a"),
                ("iceberg.rest-catalog.security", "OAUTH2"),
                (
                    "iceberg.rest-catalog.oauth2.server-uri",
                    "http://egret:8080/oauth/token"
                ),
                ("iceberg.rest-catalog.oauth2.credential", "${ENV:EGRET_C0}"),
                ("iceberg.rest-catalog.vended-credentials-enabled", "true"),
                ("fs.native-s3.enabled", "true"),
                ("s3.endpoint", "http://rustfs:9000"),
                ("s3.region", "us-east-1"),
                ("s3.path-style-access", "true"),
            ])
        );
        assert_eq!(
            f.env,
            [("EGRET_C0".to_string(), id(CRED))].into_iter().collect()
        );
    }

    #[test]
    fn iceberg_rest_with_oauth2_and_s3() {
        let d = def(
            "iceberg",
            json!({
                "metastore": { "type": "rest", "uri": "https://cat/api", "warehouse": "lake",
                               "auth": { "type": "oauth2", "credential_secret_id": PW, "scope": "ALL", "token_uri": "https://idp/token" } },
                "storage_connection_id": SC
            }),
            json!(null),
        );
        let f = render_trino("lake", &d, &input()).unwrap();
        let mut want = props(&[
            ("connector.name", "iceberg"),
            ("iceberg.catalog.type", "rest"),
            ("iceberg.rest-catalog.uri", "https://cat/api"),
            ("iceberg.rest-catalog.warehouse", "lake"),
            ("iceberg.rest-catalog.security", "OAUTH2"),
            ("iceberg.rest-catalog.oauth2.credential", "${ENV:CAT_C0_0}"),
            ("iceberg.rest-catalog.oauth2.scope", "ALL"),
            (
                "iceberg.rest-catalog.oauth2.server-uri",
                "https://idp/token",
            ),
        ]);
        want.extend(props(&S3_PROPS[..5]));
        want.insert("s3.aws-secret-key".into(), "${ENV:CAT_C0_1}".into());
        assert_eq!(f.properties, want);
        assert_eq!(
            f.env,
            [
                ("CAT_C0_0".to_string(), id(PW)),
                ("CAT_C0_1".to_string(), id(S3))
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn iceberg_hive_and_glue_metastores() {
        let d = def(
            "iceberg",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!(null),
        );
        let mut want = props(&[
            ("connector.name", "iceberg"),
            ("iceberg.catalog.type", "hive_metastore"),
            ("hive.metastore.uri", "thrift://hms:9083"),
        ]);
        want.extend(props(&S3_PROPS));
        assert_eq!(render_trino("h", &d, &input()).unwrap().properties, want);

        let d = def(
            "iceberg",
            json!({ "metastore": { "type": "glue", "region": "eu-west-1", "access_key": "AKG", "secret_key_secret_id": PW }, "storage_connection_id": SC }),
            json!(null),
        );
        let f = render_trino("g", &d, &input()).unwrap();
        assert_eq!(f.properties["iceberg.catalog.type"], "glue");
        assert_eq!(f.properties["hive.metastore.glue.region"], "eu-west-1");
        assert_eq!(f.properties["hive.metastore.glue.aws-access-key"], "AKG");
        assert_eq!(
            f.properties["hive.metastore.glue.aws-secret-key"],
            "${ENV:CAT_C0_0}"
        );
        assert_eq!(f.properties["s3.aws-secret-key"], "${ENV:CAT_C0_1}");
    }

    #[test]
    fn hive_family() {
        let d = def(
            "hive",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!(null),
        );
        let mut want = props(&[
            ("connector.name", "hive"),
            ("hive.metastore.uri", "thrift://hms:9083"),
        ]);
        want.extend(props(&S3_PROPS));
        assert_eq!(render_trino("h", &d, &input()).unwrap().properties, want);

        let d = def(
            "delta_lake",
            json!({ "metastore": { "type": "glue", "region": "eu-west-1" }, "storage_connection_id": SC }),
            json!(null),
        );
        let f = render_trino("d", &d, &input()).unwrap();
        assert_eq!(f.properties["connector.name"], "delta_lake");
        assert_eq!(f.properties["hive.metastore"], "glue");
        assert_eq!(f.properties["hive.metastore.glue.region"], "eu-west-1");

        let d = def(
            "hudi",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!(null),
        );
        assert_eq!(
            render_trino("u", &d, &input()).unwrap().properties["connector.name"],
            "hudi"
        );
    }

    #[test]
    fn jdbc_urls_for_every_dialect_and_ssl_mode() {
        let cases = [
            (
                "postgresql",
                Some("sales"),
                "disable",
                "jdbc:postgresql://db:5432/sales?sslmode=disable",
            ),
            (
                "postgresql",
                Some("sales"),
                "require",
                "jdbc:postgresql://db:5432/sales?sslmode=require",
            ),
            (
                "postgresql",
                Some("sales"),
                "verify_full",
                "jdbc:postgresql://db:5432/sales?sslmode=verify-full",
            ),
            ("mysql", None, "disable", "jdbc:mysql://db:5432?sslMode=DISABLED"),
            ("mysql", None, "require", "jdbc:mysql://db:5432?sslMode=REQUIRED"),
            (
                "mysql",
                None,
                "verify_full",
                "jdbc:mysql://db:5432?sslMode=VERIFY_IDENTITY",
            ),
            ("mariadb", None, "disable", "jdbc:mariadb://db:5432?sslMode=disable"),
            ("mariadb", None, "require", "jdbc:mariadb://db:5432?sslMode=trust"),
            (
                "mariadb",
                None,
                "verify_full",
                "jdbc:mariadb://db:5432?sslMode=verify-full",
            ),
            ("oracle", Some("ORCL"), "disable", "jdbc:oracle:thin:@//db:5432/ORCL"),
            (
                "oracle",
                Some("ORCL"),
                "require",
                "jdbc:oracle:thin:@tcps://db:5432/ORCL",
            ),
            (
                "oracle",
                Some("ORCL"),
                "verify_full",
                "jdbc:oracle:thin:@tcps://db:5432/ORCL?oracle.net.ssl_server_dn_match=true",
            ),
            (
                "sqlserver",
                Some("sales"),
                "disable",
                "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=false",
            ),
            (
                "sqlserver",
                Some("sales"),
                "require",
                "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=true",
            ),
            (
                "sqlserver",
                Some("sales"),
                "verify_full",
                "jdbc:sqlserver://db:5432;databaseName=sales;encrypt=true;trustServerCertificate=false",
            ),
            (
                "clickhouse",
                Some("events"),
                "disable",
                "jdbc:clickhouse://db:5432/events?ssl=false",
            ),
            (
                "clickhouse",
                None,
                "require",
                "jdbc:clickhouse://db:5432/?ssl=true&sslmode=none",
            ),
            (
                "clickhouse",
                Some("events"),
                "verify_full",
                "jdbc:clickhouse://db:5432/events?ssl=true&sslmode=strict",
            ),
        ];
        for (dialect, database, mode, url) in cases {
            let mut cfg = json!({ "dialect": dialect, "host": "db", "port": 5432, "user": "u", "password_secret_id": PW, "ssl": { "mode": mode } });
            if let Some(d) = database {
                cfg["database"] = json!(d);
            }
            let d = def("jdbc", cfg, json!(null));
            let f = render_trino("j", &d, &input()).unwrap();
            assert_eq!(
                f.properties,
                props(&[
                    ("connector.name", dialect),
                    ("connection-url", url),
                    ("connection-user", "u"),
                    ("connection-password", "${ENV:CAT_C0_0}"),
                ]),
                "{dialect}/{mode}"
            );
            assert_eq!(
                f.env,
                [("CAT_C0_0".to_string(), id(PW))].into_iter().collect()
            );
        }
    }

    #[test]
    fn elasticsearch_with_and_without_a_password() {
        let d = def(
            "elasticsearch",
            json!({ "hosts": ["es1", "es2"], "port": 9200, "tls": true, "user": "elastic", "password_secret_id": PW }),
            json!(null),
        );
        assert_eq!(
            render_trino("es", &d, &input()).unwrap().properties,
            props(&[
                ("connector.name", "elasticsearch"),
                ("elasticsearch.host", "es1,es2"),
                ("elasticsearch.port", "9200"),
                ("elasticsearch.tls.enabled", "true"),
                ("elasticsearch.security", "PASSWORD"),
                ("elasticsearch.auth.user", "elastic"),
                ("elasticsearch.auth.password", "${ENV:CAT_C0_0}"),
            ])
        );
        let d = def(
            "elasticsearch",
            json!({ "hosts": ["es1"], "port": 9200, "tls": false }),
            json!(null),
        );
        assert_eq!(
            render_trino("es", &d, &input()).unwrap().properties,
            props(&[
                ("connector.name", "elasticsearch"),
                ("elasticsearch.host", "es1"),
                ("elasticsearch.port", "9200"),
                ("elasticsearch.tls.enabled", "false"),
            ])
        );
    }

    /// SP3b D4's tpch/tpcds coverage lived only on the StarRocks side
    /// (`elasticsearch_and_benchmark`, deleted when StarRocks' `can_read`
    /// started refusing both kinds — there is no `type = "benchmark"`
    /// spelling to pin on this engine at all, Trino's connector names are
    /// literally `tpch`/`tpcds`). What DOES transfer is the other half of
    /// that deleted test: `scale` must never become a rendered property here
    /// — Trino's tpch/tpcds connectors take it from the SCHEMA name
    /// (`tiny`/`sf1`/`sf10`/…, `SampleConfig`'s doc: "Trino ignores `scale`
    /// (schema names carry it)"), not from a catalog property, so a `scale`
    /// key must be absent whether it's set on the registry row or not (fix
    /// round 1: restoring the coverage the coordinator asked to keep).
    #[test]
    fn tpch_and_tpcds_never_render_scale_trino_takes_it_from_the_schema_name() {
        for kind in ["tpch", "tpcds"] {
            let d = def(kind, json!({ "scale": 10 }), json!(null));
            let p = render_trino("b", &d, &input()).unwrap().properties;
            assert_eq!(p["connector.name"], kind, "{kind}: connector.name spelling");
            assert!(
                !p.contains_key("scale"),
                "{kind}: scale must never be rendered, even when set: {p:?}"
            );

            let d = def(kind, json!({}), json!(null));
            let p = render_trino("b", &d, &input()).unwrap().properties;
            assert!(
                !p.contains_key("scale"),
                "{kind}: scale must never be rendered when absent either: {p:?}"
            );
        }
    }

    #[test]
    fn samples_generic_and_advanced_merge() {
        let d = def(
            "tpch",
            json!({ "scale": 10 }),
            json!({ "trino": { "tpch.column-naming": "STANDARD" } }),
        );
        assert_eq!(
            render_trino("t", &d, &input()).unwrap().properties,
            props(&[
                ("connector.name", "tpch"),
                ("tpch.column-naming", "STANDARD")
            ])
        );
        let d = def("tpcds", json!({}), json!(null));
        assert_eq!(
            render_trino("t", &d, &input()).unwrap().properties,
            props(&[("connector.name", "tpcds")])
        );

        let d = def(
            "generic",
            json!({ "engines": ["trino"], "secret_properties": { "trino": { "bigquery.credentials-key": PW } } }),
            json!({ "trino": { "connector.name": "bigquery", "bigquery.project-id": "p" } }),
        );
        let f = render_trino("bq", &d, &input()).unwrap();
        assert_eq!(
            f.properties,
            props(&[
                ("connector.name", "bigquery"),
                ("bigquery.project-id", "p"),
                ("bigquery.credentials-key", "${ENV:CAT_C0_0}"),
            ])
        );
        let (plain, secret) = f.split_secret_properties();
        assert_eq!(
            plain,
            props(&[("connector.name", "bigquery"), ("bigquery.project-id", "p")])
        );
        assert_eq!(
            secret,
            [("bigquery.credentials-key".to_string(), id(PW))]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn refusals() {
        // An advanced key the typed form sets is refused at save time.
        let err = CatalogDef::parse(
            "tpch",
            &json!({}),
            &json!({ "trino": { "connector.name": "memory" } }),
        );
        assert!(
            matches!(err, Err(crate::ValidationError::Collision { .. })),
            "{err:?}"
        );
        let err = CatalogDef::parse(
            "jdbc",
            &json!({ "dialect": "postgresql", "host": "db", "port": 5432, "database": "d", "user": "u", "password_secret_id": PW, "ssl": { "mode": "require" } }),
            &json!({ "trino": { "connection-url": "jdbc:postgresql://evil/x" } }),
        );
        assert!(
            matches!(err, Err(crate::ValidationError::Collision { .. })),
            "{err:?}"
        );
        // StarRocks-only generic cannot render for Trino.
        let d = def(
            "generic",
            json!({ "engines": ["starrocks"] }),
            json!({ "starrocks": { "type": "paimon" } }),
        );
        assert_eq!(
            render_trino("p", &d, &input()),
            Err(RenderError::NotReadable("p".into()))
        );
        // Missing inputs.
        let mut i = input();
        i.storage = None;
        let d = def(
            "hive",
            json!({ "metastore": { "type": "hive", "uri": "thrift://hms:9083" }, "storage_connection_id": SC }),
            json!(null),
        );
        assert_eq!(
            render_trino("h", &d, &i),
            Err(RenderError::MissingInput("storage connection"))
        );
        let mut i = input();
        i.credential_secret_id = None;
        assert_eq!(
            render_trino("e", &CatalogDef::egret_warehouse(id(WH)), &i),
            Err(RenderError::MissingInput("catalog client credential"))
        );
    }
}
