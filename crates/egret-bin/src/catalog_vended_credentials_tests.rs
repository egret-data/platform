//! Vended S3 credentials follow the caller's catalog grants (spec
//! `docs/en/specs/2026-10-27-vended-credentials-follow-grants-design.md`).
//!
//! Every test drives the REAL [`EgretCatalogGuard`] in front of lakekeeper's
//! real Iceberg router, wired as `serve` wires it (`mount_catalog_engine`),
//! on a warehouse with egret's own S3 profile (STS on, signing off). The S3
//! endpoint is [`MockS3`], an in-process object store and STS: lakekeeper
//! asks it for every vended credential, so the test reads, for each access
//! key a client was handed, the session policy lakekeeper asked for. The
//! store also enforces that policy, as RustFS does, so a write with a
//! read-only key is refused here as it is on the real store.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use super::{EgretCatalogGuard, CATALOG_AUDIENCE};
use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, create_lakekeeper_warehouse, ensure_bootstrapped, mount_catalog_engine,
    run_lakekeeper_migrations, GrantInput, S3StorageConfig, TableHook,
};
use egret_machine_idp::SigningKeyStore as _;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

const SYSTEM_USER: &str = "egret-system";

/// The S3 actions a read-write-delete session policy allows beyond reading.
const WRITE_ACTIONS: [&str; 2] = ["s3:PutObject", "s3:DeleteObject"];

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

// ── The object store and STS ─────────────────────────────────────────────────

/// One credential the STS minted: the session policy lakekeeper sent.
#[derive(Debug, Clone)]
struct Vended {
    /// `TableAccess`'s actions.
    actions: Vec<String>,
    /// `TableAccess`'s resources, as keys in the bucket: `key/` and `key/*`.
    resources: Vec<String>,
    bucket: String,
}

impl Vended {
    fn can_write(&self) -> bool {
        WRITE_ACTIONS
            .iter()
            .any(|w| self.actions.iter().any(|a| a == w))
    }

    fn allows(&self, action: &str, bucket: &str, key: &str) -> bool {
        bucket == self.bucket
            && self.actions.iter().any(|a| a == action)
            && self.resources.iter().any(|r| match r.strip_suffix('*') {
                Some(prefix) => key.starts_with(prefix),
                None => key == r,
            })
    }

    /// The table prefix the policy names (`key/`).
    fn prefix(&self) -> &str {
        self.resources
            .iter()
            .find(|r| !r.ends_with('*'))
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// An S3-compatible store and STS on one endpoint, in memory. The static
/// key ([`MockS3::STATIC_KEY`]) may do anything; a key the STS minted may do
/// what its session policy allows; any other key nothing.
#[derive(Default)]
struct MockS3 {
    objects: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    vended: Mutex<HashMap<String, Vended>>,
    /// Every access key minted, in order.
    minted: Mutex<Vec<String>>,
}

impl MockS3 {
    const STATIC_KEY: &'static str = "STATICAK";

    async fn start() -> (Arc<Self>, String) {
        let mock = Arc::new(Self::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the mock store");
        let addr = listener.local_addr().expect("mock address");
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(mock.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        (mock, format!("http://{addr}/"))
    }

    fn policy_of(&self, key: &str) -> Option<Vended> {
        self.vended.lock().unwrap().get(key).cloned()
    }

    fn minted(&self) -> Vec<String> {
        self.minted.lock().unwrap().clone()
    }

    /// Mint a credential for an `AssumeRole` call.
    fn assume_role(&self, form: &HashMap<String, String>) -> axum::response::Response {
        let Some(policy) = form.get("Policy") else {
            return s3_error(
                StatusCode::BAD_REQUEST,
                "MissingPolicy",
                "no session policy",
            );
        };
        let policy: Value = serde_json::from_str(policy).expect("a JSON session policy");
        let table = policy["Statement"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["Sid"] == "TableAccess"))
            .expect("a TableAccess statement");
        let actions: Vec<String> = table["Action"]
            .as_array()
            .expect("actions")
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        let mut bucket = String::new();
        let resources: Vec<String> = table["Resource"]
            .as_array()
            .expect("resources")
            .iter()
            .map(|r| {
                let arn = r.as_str().unwrap().trim_start_matches("arn:aws:s3:::");
                let (b, k) = arn.split_once('/').expect("bucket/key");
                bucket = b.to_string();
                k.to_string()
            })
            .collect();
        let vended = Vended {
            actions,
            resources,
            bucket,
        };
        let key = format!(
            "VEND{}{}",
            if vended.can_write() { "RWD" } else { "R" },
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        self.vended.lock().unwrap().insert(key.clone(), vended);
        self.minted.lock().unwrap().push(key.clone());
        let expiration = (chrono::Utc::now() + chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        xml(
            StatusCode::OK,
            format!(
                "<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\">\
                 <AssumeRoleResult><Credentials><AccessKeyId>{key}</AccessKeyId>\
                 <SecretAccessKey>secret</SecretAccessKey><SessionToken>token-{key}</SessionToken>\
                 <Expiration>{expiration}</Expiration></Credentials>\
                 <AssumedRoleUser><AssumedRoleId>AROA:lakekeeper-sts</AssumedRoleId>\
                 <Arn>arn:aws:sts::000000000000:assumed-role/egret/lakekeeper-sts</Arn>\
                 </AssumedRoleUser></AssumeRoleResult>\
                 <ResponseMetadata><RequestId>1</RequestId></ResponseMetadata>\
                 </AssumeRoleResponse>"
            ),
        )
    }

    /// May `key` do `action` on `bucket`/`object`?
    fn permits(&self, access_key: &str, action: &str, bucket: &str, object: &str) -> bool {
        if access_key == Self::STATIC_KEY {
            return true;
        }
        self.policy_of(access_key)
            .is_some_and(|v| v.allows(action, bucket, object))
    }

    /// May `key` list `bucket` under `prefix`? (`ListBucketForFolder`: the
    /// `s3:prefix` must fall under the table's prefix.)
    fn permits_list(&self, access_key: &str, bucket: &str, prefix: &str) -> bool {
        if access_key == Self::STATIC_KEY {
            return true;
        }
        self.policy_of(access_key)
            .is_some_and(|v| v.bucket == bucket && prefix.starts_with(v.prefix()))
    }
}

fn xml(status: StatusCode, body: String) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    (
        status,
        [("content-type", "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{body}"),
    )
        .into_response()
}

fn s3_error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    xml(
        status,
        format!("<Error><Code>{code}</Code><Message>{message}</Message></Error>"),
    )
}

fn denied() -> axum::response::Response {
    s3_error(StatusCode::FORBIDDEN, "AccessDenied", "Access Denied")
}

/// The access key a request is signed with (SigV4 `Credential=`).
fn access_key_of(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split("Credential=").nth(1))
        .and_then(|v| v.split('/').next())
        .unwrap_or("")
        .to_string()
}

fn decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("k={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

async fn handle(
    axum::extract::State(mock): axum::extract::State<Arc<MockS3>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 64 << 20)
        .await
        .unwrap_or_default();
    let query: HashMap<String, String> = parts
        .uri
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let path = parts.uri.path().trim_start_matches('/').to_string();
    let access_key = access_key_of(&parts.headers);

    // STS: a form POST to the root.
    if parts.method == axum::http::Method::POST && path.is_empty() {
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        if form.get("Action").map(String::as_str) == Some("AssumeRole") {
            return mock.assume_role(&form);
        }
        return s3_error(
            StatusCode::BAD_REQUEST,
            "InvalidAction",
            "unknown STS action",
        );
    }

    let (bucket, object) = match path.split_once('/') {
        Some((b, k)) => (b.to_string(), decode(k)),
        None => (path.clone(), String::new()),
    };
    // Path-style keys arrive percent-encoded segment by segment; the form
    // decoder above reads `+` as a space, which S3 keys here never hold.
    let object = object.replace(' ', "+");

    match (parts.method.as_str(), object.is_empty()) {
        ("GET", true) if query.contains_key("list-type") || query.contains_key("prefix") => {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            if !mock.permits_list(&access_key, &bucket, &prefix) {
                return denied();
            }
            let objects = mock.objects.lock().unwrap();
            let contents: String = objects
                .iter()
                .filter(|((b, k), _)| *b == bucket && k.starts_with(&prefix))
                .map(|((_, k), v)| {
                    format!(
                        "<Contents><Key>{k}</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified>\
                         <ETag>\"e\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                        v.len()
                    )
                })
                .collect();
            let count = contents.matches("<Contents>").count();
            xml(
                StatusCode::OK,
                format!(
                    "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <Name>{bucket}</Name><Prefix>{prefix}</Prefix><KeyCount>{count}</KeyCount>\
                     <MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
                ),
            )
        }
        ("GET", true) => xml(StatusCode::OK, "<LocationConstraint/>".into()),
        ("POST", true) if query.contains_key("delete") => {
            let text = String::from_utf8_lossy(&body);
            let keys: Vec<String> = text
                .split("<Key>")
                .skip(1)
                .filter_map(|s| s.split("</Key>").next())
                .map(|k| k.replace("&amp;", "&"))
                .collect();
            let mut result = String::new();
            for k in keys {
                if mock.permits(&access_key, "s3:DeleteObject", &bucket, &k) {
                    mock.objects
                        .lock()
                        .unwrap()
                        .remove(&(bucket.clone(), k.clone()));
                    result.push_str(&format!("<Deleted><Key>{k}</Key></Deleted>"));
                } else {
                    result.push_str(&format!(
                        "<Error><Key>{k}</Key><Code>AccessDenied</Code><Message>Access Denied</Message></Error>"
                    ));
                }
            }
            xml(
                StatusCode::OK,
                format!(
                    "<DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{result}</DeleteResult>"
                ),
            )
        }
        ("PUT", false) => {
            if !mock.permits(&access_key, "s3:PutObject", &bucket, &object) {
                return denied();
            }
            // Lakekeeper sends a plain body (measured: no `aws-chunked`).
            let data = body.to_vec();
            mock.objects.lock().unwrap().insert((bucket, object), data);
            use axum::response::IntoResponse as _;
            (StatusCode::OK, [("etag", "\"e\"")], "").into_response()
        }
        ("GET" | "HEAD", false) => {
            if !mock.permits(&access_key, "s3:GetObject", &bucket, &object) {
                return denied();
            }
            let Some(data) = mock.objects.lock().unwrap().get(&(bucket, object)).cloned() else {
                return s3_error(StatusCode::NOT_FOUND, "NoSuchKey", "The key does not exist");
            };
            let data = match parts
                .headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.split_once('-'))
            {
                Some((a, b)) => {
                    let a: usize = a.parse().unwrap_or(0);
                    let b: usize = b.parse().unwrap_or(data.len().saturating_sub(1));
                    data[a.min(data.len())..(b + 1).min(data.len())].to_vec()
                }
                None => data,
            };
            let len = data.len();
            let body = if parts.method == axum::http::Method::HEAD {
                Body::empty()
            } else {
                Body::from(data)
            };
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-length", len)
                .header("etag", "\"e\"")
                .header("last-modified", "Thu, 01 Jan 2026 00:00:00 GMT")
                .body(body)
                .unwrap()
        }
        ("DELETE", false) => {
            if !mock.permits(&access_key, "s3:DeleteObject", &bucket, &object) {
                return denied();
            }
            mock.objects.lock().unwrap().remove(&(bucket, object));
            use axum::response::IntoResponse as _;
            StatusCode::NO_CONTENT.into_response()
        }
        _ => s3_error(
            StatusCode::NOT_IMPLEMENTED,
            "NotImplemented",
            "not in the mock",
        ),
    }
}

// ── The environment ─────────────────────────────────────────────────────────

struct Env {
    /// The guard and the table hook in front of lakekeeper, as `serve` mounts
    /// them.
    app: axum::Router,
    /// Lakekeeper with nothing in front: the setup's own requests.
    raw: axum::Router,
    keys: egret_machine_idp::PgSigningKeyStore,
    catalog: egret_catalog_engine::CatalogStore,
    prefix: String,
    warehouse: String,
    warehouse_id: uuid::Uuid,
    s3: Arc<MockS3>,
    state: egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>,
}

/// A warehouse on egret's S3 profile over a fresh [`MockS3`], with
/// namespaces `sales` and `other`.
async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&dsn, 10)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(&dsn)
                .await
                .expect("lakekeeper migrations");
            let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
    let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let (s3, endpoint) = MockS3::start().await;
    let bucket = unique("vc-");
    let prefix = create_lakekeeper_warehouse(
        state.clone(),
        SYSTEM_USER,
        &unique("vcwh"),
        &S3StorageConfig {
            bucket,
            region: "us-east-1".into(),
            endpoint: Some(endpoint),
            path_style: true,
            access_key: MockS3::STATIC_KEY.into(),
            secret_key: "STATICSK".into(),
            key_prefix: Some("kp".into()),
        },
    )
    .await
    .expect("lakekeeper warehouse on the mock store");
    let pool = &db.pool;
    let ws: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'VC') RETURNING id")
            .bind(unique("vc-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("vc-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse = unique("vc_wh");
    let warehouse_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(&warehouse)
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");

    let guard: Arc<dyn egret_catalog_engine::CatalogGuard> = Arc::new(EgretCatalogGuard {
        keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
    });
    let hook = Arc::new(TableHook::new(db.clone()));
    let env = Env {
        app: mount_catalog_engine(state.clone(), Some(guard), None, Some(hook)),
        raw: mount_catalog_engine(state.clone(), None, None, None),
        keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db),
        prefix,
        warehouse,
        warehouse_id,
        s3,
        state,
    };
    for ns in ["sales", "other"] {
        let (status, body) = env
            .send(
                &env.raw,
                None,
                "POST",
                "namespaces",
                &[],
                json!({ "namespace": [ns] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create namespace {ns}: {body}");
    }
    Some(env)
}

macro_rules! env_or_skip {
    () => {
        match env().await {
            Some(e) => e,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

/// One grant of a role: `(securable, namespace, table, privilege)`.
type Grant<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str);

const VENDED: (&str, &str) = ("x-iceberg-access-delegation", "vended-credentials");

impl Env {
    async fn send(
        &self,
        app: &axum::Router,
        token: Option<&str>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(format!("/{}/{path}", self.prefix))
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = if method == "GET" {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 4 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    /// A token for a fresh role holding exactly `grants`.
    async fn token_for(&self, consumer: &str, grants: &[Grant<'_>]) -> String {
        let role_name = unique("vc-role");
        let role = self
            .catalog
            .create_role(self.warehouse_id, &role_name, consumer, None)
            .await
            .expect("role");
        for (securable, ns, table, privilege) in grants {
            self.catalog
                .add_grant(
                    role.id,
                    &GrantInput {
                        securable_type: (*securable).into(),
                        namespace: ns.map(|n| vec![n.to_string()]),
                        table_name: table.map(str::to_string),
                        privilege: (*privilege).into(),
                    },
                )
                .await
                .expect("grant");
        }
        let km = self.keys.active().await.expect("active key");
        egret_machine_idp::sign_claims(
            &km,
            &json!({
                "sub": unique("vc-client"), "aud": CATALOG_AUDIENCE,
                "warehouse": self.warehouse, "roles": [role_name],
                "exp": chrono::Utc::now().timestamp() + 600,
            }),
        )
        .expect("sign")
    }

    /// Create `{ns}.{name}` through lakekeeper with no guard. Returns the
    /// load result.
    async fn create_table(&self, ns: &str, name: &str) -> Value {
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "POST",
                &format!("namespaces/{ns}/tables"),
                &[],
                json!({"name": name, "schema": schema()}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create {ns}.{name}: {body}");
        body
    }

    /// Load `{ns}.{name}` as `token`, with `headers`.
    async fn load(&self, token: &str, ns: &str, name: &str, headers: &[(&str, &str)]) -> Value {
        let (status, body) = self
            .send(
                &self.app,
                Some(token),
                "GET",
                &format!("namespaces/{ns}/tables/{name}"),
                headers,
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "load {ns}.{name}: {body}");
        body
    }

    /// The access key in a load result's `config`, and the session policy
    /// lakekeeper asked the STS for when it minted it.
    fn vended_in_config(&self, load: &Value) -> (String, Vended) {
        let key = load["config"]["s3.access-key-id"]
            .as_str()
            .unwrap_or_else(|| panic!("a vended access key in the load result: {load}"))
            .to_string();
        let policy = self
            .s3
            .policy_of(&key)
            .unwrap_or_else(|| panic!("{key} was minted by the STS"));
        (key, policy)
    }

    /// The same, from a `/credentials` answer's one storage credential.
    fn vended_in_credentials(&self, body: &Value) -> (String, Vended) {
        let creds = body["storage-credentials"]
            .as_array()
            .unwrap_or_else(|| panic!("storage-credentials: {body}"));
        assert_eq!(creds.len(), 1, "one storage credential: {body}");
        let key = creds[0]["config"]["s3.access-key-id"]
            .as_str()
            .unwrap_or_else(|| panic!("a vended access key: {body}"))
            .to_string();
        let policy = self
            .s3
            .policy_of(&key)
            .unwrap_or_else(|| panic!("{key} was minted by the STS"));
        (key, policy)
    }

    /// Would the store let `access_key` do `action` on an object under the
    /// table at `location` (`s3://bucket/key`)? The session policy decides,
    /// as on RustFS.
    fn store_permits(&self, access_key: &str, action: &str, location: &str) -> bool {
        let path = location.trim_start_matches("s3://").trim_end_matches('/');
        let (bucket, key) = path.split_once('/').expect("s3://bucket/key");
        self.s3.permits(
            access_key,
            action,
            bucket,
            &format!("{key}/data/probe.parquet"),
        )
    }
}

/// The table schema every test table has.
fn schema() -> Value {
    json!({"type": "struct", "schema-id": 0, "fields": [
        {"id": 1, "name": "id", "type": "string", "required": false}
    ]})
}

/// The table location of a load result.
fn location_of(load: &Value) -> String {
    load["metadata"]["location"]
        .as_str()
        .unwrap_or_else(|| panic!("a table location: {load}"))
        .to_string()
}

/// Does the policy allow anything on an object under `location`?
fn reaches(policy: &Vended, location: &str) -> bool {
    let path = location.trim_start_matches("s3://").trim_end_matches('/');
    let (bucket, key) = path.split_once('/').expect("s3://bucket/key");
    let object = format!("{key}/data/probe.parquet");
    ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
        .iter()
        .any(|a| policy.allows(a, bucket, &object))
}

// ── Load: what a grant vends ────────────────────────────────────────────────

/// F4, the premise (M1/M2): a client holding only a table `read` grant was
/// handed Put and Delete on the table's files, and overwrote and deleted
/// them. It now gets a key that may only read.
#[tokio::test]
async fn a_table_read_grant_loads_read_only_credentials() {
    let env = env_or_skip!();
    let location = location_of(&env.create_table("sales", "orders").await);
    let reader = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "read")],
        )
        .await;

    let load = env.load(&reader, "sales", "orders", &[VENDED]).await;
    let (key, policy) = env.vended_in_config(&load);

    assert!(
        !policy.can_write(),
        "a read grant vends read only: {policy:?}"
    );
    assert!(
        policy.actions.iter().any(|a| a == "s3:GetObject"),
        "and still reads: {policy:?}"
    );
    assert!(env.store_permits(&key, "s3:GetObject", &location));
    assert!(!env.store_permits(&key, "s3:PutObject", &location));
    assert!(!env.store_permits(&key, "s3:DeleteObject", &location));
}

/// E5: lakekeeper vends credentials when the client sends no delegation
/// header at all. That path is read-only for a reader too.
#[tokio::test]
async fn a_read_grant_without_the_delegation_header_is_read_only_too() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let reader = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "read")],
        )
        .await;

    let load = env.load(&reader, "sales", "orders", &[]).await;
    let (_, policy) = env.vended_in_config(&load);

    assert!(
        !policy.can_write(),
        "no header, still read only: {policy:?}"
    );
}

/// A `write` grant on the table keeps read-write-delete credentials: an
/// Iceberg writer must put data files and delete orphans.
#[tokio::test]
async fn a_table_write_grant_loads_read_write_delete_credentials() {
    let env = env_or_skip!();
    let location = location_of(&env.create_table("sales", "orders").await);
    let writer = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "write")],
        )
        .await;

    let load = env.load(&writer, "sales", "orders", &[VENDED]).await;
    let (key, policy) = env.vended_in_config(&load);

    assert!(policy.can_write(), "a write grant vends RWD: {policy:?}");
    assert!(env.store_permits(&key, "s3:PutObject", &location));
    assert!(env.store_permits(&key, "s3:DeleteObject", &location));
}

/// O1 (M4): a namespace `read` grant was handed RWD on every table in the
/// namespace. Every one of them is read-only now.
#[tokio::test]
async fn a_namespace_read_grant_is_read_only_on_every_table_in_it() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    env.create_table("sales", "peer").await;
    let reader = env
        .token_for("external", &[("namespace", Some("sales"), None, "read")])
        .await;

    for table in ["orders", "peer"] {
        let load = env.load(&reader, "sales", table, &[VENDED]).await;
        let (_, policy) = env.vended_in_config(&load);
        assert!(!policy.can_write(), "sales.{table}: {policy:?}");
    }
}

/// A namespace `write` grant covers every table in it, so it vends RWD.
#[tokio::test]
async fn a_namespace_write_grant_loads_read_write_delete_credentials() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let writer = env
        .token_for("external", &[("namespace", Some("sales"), None, "write")])
        .await;

    let load = env.load(&writer, "sales", "orders", &[VENDED]).await;
    let (_, policy) = env.vended_in_config(&load);

    assert!(policy.can_write(), "{policy:?}");
}

/// An egret-provisioned engine's catalog client holds `catalog` `manage`
/// (`ensure_engine_role`). Trino and StarRocks keep RWD and keep writing.
#[tokio::test]
async fn an_engine_client_with_catalog_manage_loads_read_write_delete() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let engine = env
        .token_for("engine", &[("catalog", None, None, "manage")])
        .await;

    let load = env.load(&engine, "sales", "orders", &[VENDED]).await;
    let (_, policy) = env.vended_in_config(&load);

    assert!(policy.can_write(), "{policy:?}");
}

/// An engine attached with a role that holds only `read` gets what the role
/// says: a read-only key (spec decision 3).
#[tokio::test]
async fn an_engine_client_with_a_catalog_read_role_is_read_only() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let engine = env
        .token_for("engine", &[("catalog", None, None, "read")])
        .await;

    let load = env.load(&engine, "sales", "orders", &[VENDED]).await;
    let (_, policy) = env.vended_in_config(&load);

    assert!(!policy.can_write(), "{policy:?}");
}

// ── /credentials ────────────────────────────────────────────────────────────

/// `GET …/tables/{t}/credentials` asks the same question as a load, and
/// answers it the same way.
#[tokio::test]
async fn the_credentials_endpoint_follows_the_grant() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let reader = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "read")],
        )
        .await;
    let writer = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "write")],
        )
        .await;

    for (who, token, writes) in [("reader", &reader, false), ("writer", &writer, true)] {
        let (status, body) = env
            .send(
                &env.app,
                Some(token),
                "GET",
                "namespaces/sales/tables/orders/credentials",
                &[VENDED],
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        let (_, policy) = env.vended_in_credentials(&body);
        assert_eq!(policy.can_write(), writes, "{who}: {policy:?}");
    }
}

// ── The credential cache (E10) ──────────────────────────────────────────────

/// Lakekeeper caches a vended key by (location, permission), not by caller.
/// A reader that loads a table right after a writer must not be handed the
/// writer's cached RWD key.
#[tokio::test]
async fn a_reader_after_a_writer_is_not_handed_the_cached_writer_key() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let writer = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "write")],
        )
        .await;
    let reader = env
        .token_for(
            "external",
            &[("table", Some("sales"), Some("orders"), "read")],
        )
        .await;

    let (writer_key, writer_policy) =
        env.vended_in_config(&env.load(&writer, "sales", "orders", &[VENDED]).await);
    assert!(writer_policy.can_write(), "{writer_policy:?}");
    let (reader_key, reader_policy) =
        env.vended_in_config(&env.load(&reader, "sales", "orders", &[VENDED]).await);

    assert_ne!(
        reader_key, writer_key,
        "the reader got the writer's cached key"
    );
    assert!(!reader_policy.can_write(), "{reader_policy:?}");
    // The writer's own key is still served from the cache.
    let (again, _) = env.vended_in_config(&env.load(&writer, "sales", "orders", &[VENDED]).await);
    assert_eq!(
        again, writer_key,
        "the writer's key is cached per permission"
    );
}

// ── Create, stage-create, register: a client-chosen location (O3) ───────────

/// A namespace writer chooses the location of a table it creates or
/// registers, and is handed RWD on it. Lakekeeper must refuse a location
/// that is, or holds, another table's, and no key it mints in the attempt
/// may reach that table's files.
#[tokio::test]
async fn a_create_or_register_cannot_take_another_tables_location() {
    let env = env_or_skip!();
    let victim = env.create_table("sales", "victim").await;
    let victim_location = location_of(&victim);
    let victim_metadata = victim["metadata-location"]
        .as_str()
        .unwrap_or_else(|| panic!("a metadata location: {victim}"))
        .to_string();
    let parent = victim_location
        .trim_end_matches('/')
        .rsplit_once('/')
        .expect("a parent")
        .0
        .to_string();
    let attacker = env
        .token_for("external", &[("namespace", Some("other"), None, "write")])
        .await;
    // A metadata file of the attacker's own making: the victim's metadata
    // under a new table uuid, so only its location collides.
    let mut forged = victim["metadata"].clone();
    forged["table-uuid"] = json!(uuid::Uuid::new_v4().to_string());
    let (bucket, victim_key) = victim_location
        .trim_start_matches("s3://")
        .split_once('/')
        .expect("s3://bucket/key");
    let forged_key = format!(
        "{}/forged/metadata/00000-{}.metadata.json",
        victim_key.rsplit_once('/').expect("a namespace prefix").0,
        uuid::Uuid::new_v4()
    );
    env.s3.objects.lock().unwrap().insert(
        (bucket.to_string(), forged_key.clone()),
        forged.to_string().into_bytes(),
    );
    let forged_location = format!("s3://{bucket}/{forged_key}");

    let attempts = [
        (
            "create on the victim's location",
            "namespaces/other/tables",
            json!({"name": "t1", "schema": schema(), "location": victim_location}),
        ),
        (
            "stage-create on the victim's location",
            "namespaces/other/tables",
            json!({"name": "t2", "schema": schema(), "location": victim_location,
                   "stage-create": true}),
        ),
        (
            "create on a parent of the victim's location",
            "namespaces/other/tables",
            json!({"name": "t3", "schema": schema(), "location": parent}),
        ),
        (
            "stage-create on a parent of the victim's location",
            "namespaces/other/tables",
            json!({"name": "t4", "schema": schema(), "location": parent,
                   "stage-create": true}),
        ),
        (
            "register of the victim's metadata file",
            "namespaces/other/register",
            json!({"name": "t5", "metadata-location": victim_metadata}),
        ),
        (
            "register of a forged metadata file on the victim's location",
            "namespaces/other/register",
            json!({"name": "t6", "metadata-location": forged_location}),
        ),
    ];
    for (what, path, body) in attempts {
        let before = env.s3.minted().len();
        let (status, answer) = env
            .send(&env.app, Some(&attacker), "POST", path, &[VENDED], body)
            .await;
        // Lakekeeper refuses it as a location (or table id) already taken.
        assert_eq!(status, StatusCode::CONFLICT, "{what}: {answer}");
        for key in &env.s3.minted()[before..] {
            let policy = env.s3.policy_of(key).expect("minted");
            assert!(
                !reaches(&policy, &victim_location),
                "{what}: a key minted in the attempt reaches the victim's files: {policy:?}"
            );
        }
    }
    // The victim is untouched.
    let (status, _) = env
        .send(
            &env.raw,
            None,
            "GET",
            "namespaces/sales/tables/victim",
            &[],
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// The control: a stage-create with no location gets RWD on its own fresh
/// location, and that key reaches no other table.
#[tokio::test]
async fn a_stage_create_gets_credentials_on_its_own_location_only() {
    let env = env_or_skip!();
    let victim_location = location_of(&env.create_table("sales", "victim").await);
    let writer = env
        .token_for("external", &[("namespace", Some("other"), None, "write")])
        .await;

    let (status, body) = env
        .send(
            &env.app,
            Some(&writer),
            "POST",
            "namespaces/other/tables",
            &[VENDED],
            json!({"name": "staged", "schema": schema(), "stage-create": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let own = location_of(&body);
    let (key, policy) = env.vended_in_config(&body);

    assert!(
        policy.can_write(),
        "the creator writes its new table: {policy:?}"
    );
    assert!(env.store_permits(&key, "s3:PutObject", &own));
    assert!(!reaches(&policy, &victim_location), "{policy:?}");
}

// ── egret's own principal ───────────────────────────────────────────────────

/// egret's in-process paths (browse, recovery, warehouse admin) run as its
/// system principal, which the authorizer lets read and write every table,
/// as before. The table is created without credentials, so the browse load
/// is the one that asks the STS.
#[tokio::test]
async fn egret_s_own_principal_still_loads_read_write_delete() {
    let env = env_or_skip!();
    let (status, created) = env
        .send(
            &env.raw,
            None,
            "POST",
            "namespaces/sales/tables",
            &[("x-iceberg-access-delegation", "client-managed")],
            json!({"name": "orders", "schema": schema()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let location = location_of(&created);
    let before = env.s3.minted().len();

    egret_catalog_engine::load_table_detail(
        env.state.clone(),
        SYSTEM_USER,
        &env.prefix,
        &["sales".to_string()],
        "orders",
    )
    .await
    .expect("browse loads the table");

    let minted: Vec<Vended> = env.s3.minted()[before..]
        .iter()
        .filter_map(|k| env.s3.policy_of(k))
        .filter(|p| reaches(p, &location))
        .collect();
    assert_eq!(
        minted.len(),
        1,
        "the browse load minted one key: {minted:?}"
    );
    assert!(minted[0].can_write(), "{:?}", minted[0]);
}
