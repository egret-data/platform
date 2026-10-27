//! Typed, layered configuration: built-in defaults → `egret.toml` → `EGRET_*` env vars.
//!
//! Nested keys use `__` as the env separator, e.g. `EGRET_SERVER__PORT=9000`.

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub login_max_attempts: u32,
    pub login_window_secs: u64,
    pub login_lockout_secs: u64,
    /// Machine-IdP `POST /oauth/token` (`client_credentials`). Keyed on
    /// `ip:client_id`, not on `client_id` — see `egret_core::ratelimit`.
    ///
    /// Looser than the login trio on purpose: this endpoint is polled by
    /// machines, whose tokens are short-lived (the Data API's are 600s), so a
    /// legitimate fleet re-mints far more often than a human logs in. The limit
    /// exists because the endpoint became internet-reachable and every request
    /// with a KNOWN `client_id` costs one Argon2 verify — it is a CPU-cost
    /// bound, not a credential-guessing defence (secrets are 256-bit random and
    /// unknown ids never reach the hash).
    ///
    /// It counts only requests that are NOT a proven repeat: a secret that
    /// already verified for a client whose stored hash is unchanged is served
    /// from the IdP's verification cache without Argon2 and without touching
    /// this limit (I3 — Trino mints a token per query session). Wrong secrets,
    /// first-time secrets and anything after a rotation or revoke still count.
    ///
    /// The RFC 8693 token exchange uses the same numbers. A subject token that
    /// fails verification counts against the IP's shared `ip:exchange` bucket;
    /// one egret signed that names no client (pre-`cgen`) is refused
    /// uncounted; one that names its client but no longer matches it
    /// (deleted, another audience, secret rotated) counts against `ip:<sub>`.
    /// An exchange proven against the client's current state is never counted
    /// (I4) — Iceberg refreshes in bursts, and counting them would lock the
    /// client out of `client_credentials` too.
    pub token_max_attempts: u32,
    pub token_window_secs: u64,
    pub token_lockout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub post_login_redirect: String,
    /// Audience the catalog requires in an engine JWT's `aud` claim.
    pub catalog_audience: String,
}

/// egret's OWN machine-IdP — the issuer this deployment *is*.
///
/// Deliberately NOT a field on [`OidcConfig`]. That section describes the
/// EXTERNAL IdP (Keycloak) egret delegates human login to; this one describes
/// the identity egret signs machine tokens with. They are two different
/// issuers that happen to share a word, and folding one into the other would
/// make `oidc.issuer_url` mean "them" in one line and "us" in the next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdpConfig {
    /// The `iss` of every token egret mints, the `issuer` of
    /// `/.well-known/openid-configuration`, and the `required_issuer` a
    /// StarRocks security integration byte-compares against.
    ///
    /// OIDC Discovery requires all three to be the SAME value, and requires it
    /// to equal the URL the discovery document was fetched from — so it must be
    /// a name a client OUTSIDE the cluster can resolve. Falls back to
    /// `catalog.public_url` (see [`IdpConfig::issuer_url`]) when unset, which is
    /// correct in local dev where the two really are the same address.
    ///
    /// It is NOT the address an engine dials. `engine_jwks_uri` and the catalog
    /// URI baked into engine ConfigMaps stay on `catalog.public_url`, a Service
    /// DNS name, on purpose: routing the data plane's key fetches out through
    /// the ingress and back would make every query depend on the ingress being
    /// healthy, for keys that are identical either way. Identity and address are
    /// two questions; this key exists so they can have two answers.
    ///
    /// **Changing it on a cluster with existing StarRocks engines strands them.**
    /// `CREATE SECURITY INTEGRATION` runs once, in the delegation bootstrap on
    /// engine creation (`egret-engine-mgmt/src/engines.rs`), and is not a
    /// replace — an engine provisioned under the old issuer keeps comparing
    /// against it and will reject every new token, with no automatic repair.
    /// Re-provision such engines, or drop and recreate the integration by hand.
    #[serde(default)]
    pub issuer: Option<String>,
}

impl IdpConfig {
    /// The issuer, resolved once, with `fallback` (`catalog.public_url`, the
    /// in-cluster address) when this key is unset. A widening: absent, every
    /// `iss` and every discovery field is byte-identical to what it was before
    /// this key existed.
    ///
    /// Blank counts as absent, and so does `/` — `EGRET_IDP__ISSUER=` in a
    /// half-written env file deserialises to `Some("")`, and an empty issuer
    /// would mint tokens whose `iss` is the empty string, which StarRocks would
    /// compare against and reject. Same trap, same guard, as
    /// [`DataApiConfig::consumer_base_url`].
    ///
    /// The trailing slash is trimmed HERE, at the source, for the reason
    /// `main.rs` spells out where the fallback is derived: StarRocks compares
    /// `required_issuer` RAW, so a value set with a trailing slash silently
    /// breaks StarRocks JWT auth while Trino — which never looks at the issuer —
    /// keeps working. A confusing partial failure is worth one `trim_end_matches`.
    pub fn issuer_url(&self, fallback: &str) -> String {
        self.issuer
            .as_deref()
            // Trim BEFORE the emptiness check, not after: `/` survives an
            // is-empty filter and only then trims to nothing, which would leave
            // the guard reading as present while the issuer is empty.
            .map(|u| u.trim().trim_end_matches('/'))
            .filter(|u| !u.is_empty())
            .unwrap_or(fallback)
            .trim_end_matches('/')
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// base64-encoded key (≥64 bytes) for encrypting the OIDC state cookie.
    pub secret_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangerSettings {
    pub enabled: bool,
    /// Operator bearer token for the `/ranger` surface; empty accepts no bearer.
    pub auth_token: String,
    /// `required` (default): every request on the `/ranger` surface needs an
    /// engine Basic credential or the operator bearer. `anonymous`: serve
    /// everyone — must be chosen explicitly.
    #[serde(default)]
    pub auth_mode: RangerAuthMode,
}

/// Whether the `/ranger` plugin surface requires a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RangerAuthMode {
    #[default]
    Required,
    Anonymous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// base64-encoded master key (≥32 bytes) for AES-256-GCM credential encryption.
    /// Empty disables credential sealing — registering a database WITH a secret then fails.
    pub master_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub max_rows: usize,
    pub query_timeout_secs: u64,
    pub preview_rows: usize,
    /// Object-store key prefix for query results (under the storage bucket).
    pub results_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSettings {
    pub enabled: bool,
    pub warehouse: String,
    pub auth_token: String,
    /// Externally-reachable base origin of this server (WITHOUT the `/catalog`
    /// suffix), advertised in `/v1/config`'s `s3.signer.uri`, and the fallback
    /// of the machine-IdP `issuer` (and so of its token endpoint).
    ///
    /// When unset (or blank), [`Config::load`] derives it from `server.port`:
    /// `http://localhost:<server.port>`, so moving the port moves this URL and
    /// the issuer with it. When set, it is kept as given whatever the port.
    /// Set it to a client-reachable URL in production.
    pub public_url: Option<String>,
    /// Stable identity of the in-process "system" principal that egret uses to
    /// bootstrap lakekeeper's default project at startup. lakekeeper's
    /// `bootstrap` registers the calling user as the catalog's first
    /// (global-admin) user, so it must be a real, authenticated principal with a
    /// stable `UserId` — the in-process `LakekeeperInternal` actor has no
    /// `UserId` and is rejected with `MissingUserId`. This value is hashed into
    /// a deterministic lakekeeper `UserId` (`oidc~<system_user>`), so the same
    /// configured name yields the same admin principal on every boot. Keep it
    /// stable across deploys; changing it mints a *different* system admin.
    #[serde(default = "default_system_user")]
    pub system_user: String,
}

fn default_system_user() -> String {
    "egret-system".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// When false, the PDP is not wrapped and the best-effort sink drops its
    /// events. Governance changes (`egret_audit::governance`) are still recorded
    /// in their write's transaction.
    pub enabled: bool,
    /// Rows older than this are pruned. Default 90.
    ///
    /// **`0` means "do not prune", not "keep nothing".** `AuditStore::prune(0)`
    /// really would delete every row — `ts < now() - 0 days` is everything
    /// already written — and that is the correct primitive, but a config key is
    /// not a primitive: `0` is what an operator writes when they mean "off", and
    /// what `EGRET_AUDIT__RETENTION_DAYS=` deserialises to when it is blank. On
    /// the authorization-decision record, passed through on an hourly tick
    /// inside a default-on `enabled`, that spelling emptied the table an hour
    /// after boot. `egret-bin` routes this through
    /// `egret_dataapi::serving::scheduled_retention` — the same guard
    /// [`DataApiConfig::access_log_retention_days`] uses, deliberately the same
    /// function and not a second spelling of the same comparison — and logs when
    /// it declines to schedule.
    pub retention_days: i64,
    /// Bounded channel capacity; on overflow events are dropped (best-effort).
    pub channel_capacity: usize,
    /// Max events per INSERT batch.
    pub batch_size: usize,
    /// Flush the batch at least this often.
    pub flush_interval_ms: u64,
}

/// `[apps]` — the Data Apps query gateway (`POST /api/apps/query`) and the
/// per-marimo-session bearer tokens it accepts. Env: `EGRET_APPS__<KEY>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppsConfig {
    /// Row cap per gateway query. A request's own `max_rows` is clamped to it.
    pub max_rows: usize,
    /// Per-query engine timeout.
    pub timeout_secs: u64,
    /// An app token dies after this long unused (sliding).
    pub session_idle_secs: u64,
    /// An app token dies this long after minting, however busy.
    pub session_absolute_secs: u64,
    /// Gateway calls allowed per token per `rate_window_secs`.
    pub rate_max: u32,
    /// Window length. A 429 holds that token for one full window (the limiter's
    /// lockout equals the window; see egret-bin `main.rs`).
    pub rate_window_secs: u64,
    /// Where marimo is served, e.g. `https://apps.egret.example` — a sibling
    /// subdomain of egret's own host, never another port on it (cookies are not
    /// isolated by port). Empty means no iframe can be opened: the ticket endpoint
    /// answers 503.
    pub origin: String,
    /// How long a one-time ticket may wait to be redeemed (spec §6.1: 30 s).
    pub ticket_ttl_secs: u64,
    /// egret's own browser-facing origin, e.g. `https://egret.example`. The proxy tells the
    /// browser that only this origin may frame the apps origin (`frame-ancestors`). Required
    /// whenever `origin` is set.
    pub egret_origin: String,
    /// How long the apps-origin cookie a ticket becomes stays valid (8 h, as an app token).
    pub cookie_ttl_secs: u64,
    /// Where the apps host listens. It is a SEPARATE listener from egret's API: it serves
    /// only the ticket route and the proxy, so a bug in one cannot expose the other.
    pub listen: String,
    /// How marimo pods are started. `launcher = "none"` (the default) starts none: the proxy and
    /// its checks are live and every pod is unavailable.
    #[serde(default)]
    pub pod: AppPodConfig,
    /// Data Apps' draft/version source and grid-layout JSON.
    #[serde(default)]
    pub storage: AppsStorageConfig,
}

/// `[apps.pod]` — the edit-pod launcher (spec §6.3). Env: `EGRET_APPS__POD__<KEY>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppPodConfig {
    /// `"none"`, `"subprocess"` (local dev: one marimo process per author on this host) or
    /// `"kubernetes"` (one hardened Pod per author per workspace).
    pub launcher: String,
    /// How long a request waits for a cold pod to answer.
    pub ready_timeout_secs: u64,
    /// An edit pod nobody has used for this long is stopped.
    pub idle_secs: u64,
    /// The base URL pods call back to (the pod API and the query gateway): egret's in-cluster
    /// Service, e.g. `http://egret:8080`. Not the browser-facing origin.
    pub callback_url: String,
    // ── kubernetes ──
    pub namespace: String,
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    /// An existing `kubernetes.io/dockerconfigjson` Secret for pulling `image`.
    pub pull_secret: Option<String>,
    // ── subprocess ──
    /// The command that starts the launcher entrypoint, e.g. `["apps-image/.venv/bin/egret-apps"]`;
    /// the mode (`edit`) is appended.
    pub command: Vec<String>,
    /// Parent directory of each pod's working directory and home.
    pub work_dir: String,
    /// The locked marimo configuration directory (`apps-image/config`), as the image's
    /// `EGRET_APPS_CONFIG`. Honoured by the launcher only outside the image.
    pub config_dir: String,
    /// The venv shim directory (`apps-image/venv-shim`), as the image's `EGRET_VENV_SHIM`. Run
    /// pods need it to start. Honoured by the launcher only outside the image.
    pub venv_shim_dir: String,
    // ── run pods (D24: one pod per (viewer, published version), never shared) ──
    /// A run pod nobody has used for this long is stopped. Separate from `idle_secs` (the edit
    /// default, 30 min) because an abandoned viewer tab is cheaper to let go of than an author's
    /// unpublished work.
    pub run_idle_secs: u64,
    /// Sessions (browser tabs) of the ONE viewer a run pod serves, open at once, before a new one
    /// gets 503 "app busy". Not a multi-tenant cap — D24 makes one pod one person — it bounds a
    /// reconnect loop, not concurrent strangers.
    pub run_session_cap: u32,
    /// `"thread"` (default: safe because D24 makes one pod one viewer) or `"forced_ipc"` (the P3
    /// patch, kept for a possible future shared-pod revival; not a supported production path).
    pub run_kernel_mode: String,
}

impl Default for AppPodConfig {
    fn default() -> Self {
        Self {
            launcher: "none".into(),
            ready_timeout_secs: 60,
            idle_secs: 1_800,
            callback_url: String::new(),
            namespace: "egret-apps".into(),
            image: "egret-apps:dev".into(),
            image_pull_policy: "IfNotPresent".into(),
            cpu_request: "250m".into(),
            mem_request: "512Mi".into(),
            cpu_limit: "1000m".into(),
            mem_limit: "2Gi".into(),
            pull_secret: None,
            command: Vec::new(),
            work_dir: "/tmp/egret-apps".into(),
            config_dir: String::new(),
            venv_shim_dir: String::new(),
            run_idle_secs: 300,
            run_session_cap: 8,
            run_kernel_mode: "thread".into(),
        }
    }
}

impl Default for AppsConfig {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            timeout_secs: 60,
            session_idle_secs: 1_800,
            session_absolute_secs: 28_800,
            rate_max: 20,
            rate_window_secs: 2,
            origin: String::new(),
            ticket_ttl_secs: 30,
            egret_origin: String::new(),
            cookie_ttl_secs: 28_800,
            listen: "0.0.0.0:8081".into(),
            pod: AppPodConfig::default(),
            storage: AppsStorageConfig::default(),
        }
    }
}

/// Serde's fallback for an `[apps]` section a deployment's TOML predates.
fn default_apps() -> AppsConfig {
    AppsConfig::default()
}

/// The Data API's serving surface (P4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataApiConfig {
    /// Rows in `dataapi.access_log` older than this are pruned hourly, beside
    /// the audit prune. Default 90, matching [`AuditConfig::retention_days`].
    ///
    /// # `0` means NEVER PRUNE here, and that is deliberately NOT
    /// [`AuditConfig::retention_days`]'s reading
    ///
    /// `AccessLogStore::prune(0)` — like `AuditStore::prune(0)` — deletes
    /// everything, because `ts < now() - 0 days` is every row already written.
    /// That is the right semantics for the STORE, which is a primitive an
    /// operator calls with an interval they chose.
    ///
    /// It is the wrong semantics for a CONFIG KEY, where `0` is what an operator
    /// writes when they mean "off" and what a half-written TOML or a
    /// `EGRET_DATA_API__ACCESS_LOG_RETENTION_DAYS=` env var can produce by
    /// accident. On the fastest-growing table in the system, that spelling would
    /// silently discard the whole log an hour later. So the scheduler treats `0`
    /// (and any negative) as "do not schedule the prune at all" and logs that it
    /// did — the store's meaning is untouched, and the trap is closed at the one
    /// place a human types a number.
    ///
    /// `AuditConfig::retention_days` carries the identical trap and is guarded
    /// the identical way — `egret-bin` routes it through the same
    /// `scheduled_retention` rather than a second spelling of the same
    /// comparison. This comment used to say the audit knob was "still unguarded"
    /// and that fixing it was out of scope; that stopped being true and the
    /// sentence stayed, which is the kind of note that tells the next reader not
    /// to look. **If you add a third retention knob, route it through the same
    /// function** — the danger is not this field, it is the shape.
    pub access_log_retention_days: i64,
    /// Base URL a CONSUMER OUTSIDE the cluster uses to reach this deployment's
    /// Data API — the origin stamped into the generated OpenAPI document's
    /// `servers[0].url` and `tokenUrl`, into the `token_endpoint` handed back
    /// with a new client, and into the URL and `curl` line Try-it shows the
    /// author.
    ///
    /// Deliberately separate from `catalog.public_url`, and NOT a replacement
    /// for it. That one is what an ENGINE POD resolves in-cluster to fetch the
    /// catalog and the JWKS, so a chart sets it to a Service DNS name
    /// (`http://egret.<ns>.svc.cluster.local:8080`) on purpose; it is also the
    /// machine-IdP `iss`, byte-compared by StarRocks' `required_issuer` and
    /// baked into every already-provisioned engine's catalog ConfigMap. Handing
    /// that address to a consumer outside the cluster advertises a name they
    /// cannot resolve — which is the defect this key exists to close — and
    /// repointing it at the ingress instead would break the engines. Both
    /// audiences are legitimate and simultaneous, so they get two keys.
    ///
    /// Falls back to the machine-IdP ISSUER when unset (see
    /// [`DataApiConfig::consumer_base_url`]), which is `catalog.public_url` in
    /// local dev where all three really are the same address. Deliberately NOT
    /// `catalog.public_url` directly: a deployment that sets `[idp].issuer` and
    /// leaves this one alone would otherwise hand a consumer a document whose
    /// `tokenUrl` (from the issuer) is externally valid while its
    /// `servers[0].url` and `token_endpoint` (from here) name a Service DNS
    /// name — internally contradictory, and uncallable. Two audiences may
    /// legitimately differ, but when only one is configured the other has no
    /// business pointing somewhere unreachable.
    #[serde(default)]
    pub public_url: Option<String>,
    /// How many replicas of this backend the deployment runs — **what the chart
    /// says**, not what this process can see.
    ///
    /// The Data API's rate-limit buckets and response cache are process-local
    /// unless [`Self::shared_store_url`] is set, so more than one replica
    /// multiplies every published rate limit by the replica count and fragments
    /// the cache. A process cannot discover its own replica count: a pod knows
    /// its name and namespace, and the count is a property of the Deployment —
    /// reading it would mean granting every backend pod RBAC to
    /// `get deployments` for a number the chart already holds as a literal
    /// (`backend.replicas`, `deploy/helm/egret/values.yaml`). So the chart tells
    /// it, through `EGRET_DATA_API__REPLICAS`.
    ///
    /// `egret_dataapi::serving::choose_store` reads this and **refuses to
    /// start** on more than one replica with no shared store configured. That
    /// is §4 decision B's interim guard, and it is a refusal rather than a
    /// warning because the condition is static, known at startup, and has two
    /// one-line remedies.
    ///
    /// Default `1`: a deployment that never sets it is a single-replica
    /// deployment, which is what the process-local state is correct for.
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    /// Redis/Valkey URL backing the serving surface's rate limiter and response
    /// cache across replicas (§4 decision B). Unset (or blank) means
    /// process-local state, which is correct at one replica and refused above
    /// one.
    ///
    /// Set at ONE replica too, if a deployment configures it. That is
    /// deliberate — see `choose_store` — because otherwise the shared path would
    /// first run in production, on the day someone scaled out.
    ///
    /// Numbered logical databases separate two egret deployments that must share
    /// one instance: `redis://valkey:6379/1`.
    #[serde(default)]
    pub shared_store_url: Option<String>,
}

/// Serde's fallback for [`DataApiConfig::replicas`].
///
/// `1` rather than `0`: the number describes how many of this process exist,
/// and at least this one does. It also has to be a value the multi-replica guard
/// lets through, or every deployment that predates the key would fail to start.
fn default_replicas() -> u32 {
    1
}

impl DataApiConfig {
    /// The consumer-facing origin, resolved once, with `fallback` — the
    /// machine-IdP issuer ([`IdpConfig::issuer_url`], which is itself
    /// `catalog.public_url` when `[idp].issuer` is unset) — when this key is
    /// unset. A widening: with neither key set, every output is what it was
    /// before either existed.
    ///
    /// Blank counts as absent. `EGRET_DATA_API__PUBLIC_URL=` in a half-written
    /// env file deserialises to `Some("")`, and an empty origin would render
    /// `servers[0].url` as a bare path — a document that looks generated and is
    /// uncallable. The neighbouring retention knob carries the same trap for
    /// the same reason.
    ///
    /// The trailing slash is trimmed HERE, at the source, exactly as the
    /// issuer's own derivation trims it: a `//` in the middle of a token URL is
    /// the kind of thing an HTTP client forgives and a byte comparison does not.
    pub fn consumer_base_url(&self, fallback: &str) -> String {
        self.public_url
            .as_deref()
            // Trim BEFORE the emptiness check, not after: `/` survives an
            // is-empty filter and only then trims to nothing, which would leave
            // the guard reading as present while the origin is empty.
            .map(|u| u.trim().trim_end_matches('/'))
            .filter(|u| !u.is_empty())
            .unwrap_or(fallback)
            .trim_end_matches('/')
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
    pub session: SessionConfig,
    pub csrf: CsrfConfig,
    pub rate_limit: RateLimitConfig,
    pub oidc: OidcConfig,
    /// egret's own machine-IdP identity. `oidc` above is the external IdP it
    /// delegates login to — a different issuer entirely.
    #[serde(default = "default_idp")]
    pub idp: IdpConfig,
    pub security: SecurityConfig,
    pub catalog: CatalogSettings,
    pub storage: StorageSettings,
    pub ranger: RangerSettings,
    pub secrets: SecretsConfig,
    pub engine: EngineConfig,
    pub audit: AuditConfig,
    pub automation: AutomationConfig,
    #[serde(default = "default_data_api")]
    pub data_api: DataApiConfig,
    /// The workflow engine's deployment-level switches. Serde-defaulted for the
    /// same reason `data_api` is: a deployment's TOML predates the section.
    #[serde(default = "default_workflow")]
    pub workflow: WorkflowConfig,
    /// Deployment-level overrides for how egret talks to StarRocks FEs.
    /// Serde-defaulted for the same reason `data_api`/`workflow` are: a
    /// deployment's TOML predates the section.
    #[serde(default = "default_starrocks")]
    pub starrocks: StarRocksSettings,
    /// The Data Apps query gateway. Serde-defaulted like `data_api`.
    #[serde(default = "default_apps")]
    pub apps: AppsConfig,
    /// How "Add engine" provisions engines. NOT the query-limits section
    /// `engine` above: this one is `engines` (plural). Serde-defaulted like
    /// `apps`: a deployment's TOML predates the section.
    #[serde(default)]
    pub engines: EnginesConfig,
}

/// Which backend provisions engines (`[engines] provisioner`, env
/// `EGRET_ENGINES__PROVISIONER`). `Docker` is opt-in: `Auto` never selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provisioner {
    /// Today's behaviour, unchanged. The default.
    #[default]
    Auto,
    Kubernetes,
    Docker,
    None,
}

/// `[engines.docker]` — settings for the Docker engine provisioner. Only
/// consulted when `provisioner = "docker"`. List values in env are JSON
/// arrays, e.g. `EGRET_ENGINES__DOCKER__ALLOWED_IMAGES='["trinodb/trino"]'`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnginesDockerConfig {
    /// Docker daemon endpoint (`unix://`, `tcp://` or `http://`). Default
    /// `unix:///var/run/docker.sock`; env `EGRET_ENGINES__DOCKER__HOST`.
    #[serde(default = "default_docker_host")]
    pub host: String,
    /// Network engine containers join. Required (non-empty) when
    /// `provisioner = "docker"`; env `EGRET_ENGINES__DOCKER__NETWORK`.
    #[serde(default)]
    pub network: String,
    /// Value of the `egret.project` label on every container; egret never
    /// touches containers carrying another project. Required (non-empty) when
    /// `provisioner = "docker"`: it is the only boundary between two egret
    /// instances on one daemon. Env `EGRET_ENGINES__DOCKER__PROJECT`.
    #[serde(default)]
    pub project: String,
    /// URL engine containers use to reach egret, when it differs from the
    /// default. Default unset; an empty value is refused. Env
    /// `EGRET_ENGINES__DOCKER__CALLBACK_URL`.
    #[serde(default)]
    pub callback_url: Option<String>,
    /// Image repositories (exact match, tag excluded) an engine may use.
    /// Default `["trinodb/trino"]`; must be non-empty with docker; env
    /// `EGRET_ENGINES__DOCKER__ALLOWED_IMAGES`.
    #[serde(default = "default_allowed_images")]
    pub allowed_images: Vec<String>,
}

fn default_docker_host() -> String {
    "unix:///var/run/docker.sock".into()
}

fn default_allowed_images() -> Vec<String> {
    vec!["trinodb/trino".into()]
}

impl Default for EnginesDockerConfig {
    fn default() -> Self {
        Self {
            host: default_docker_host(),
            network: String::new(),
            project: String::new(),
            callback_url: None,
            allowed_images: default_allowed_images(),
        }
    }
}

/// `[engines]` — engine provisioning (plural; distinct from `[engine]`, the
/// query limits).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnginesConfig {
    /// Default `auto`; env `EGRET_ENGINES__PROVISIONER`.
    #[serde(default)]
    pub provisioner: Provisioner,
    #[serde(default)]
    pub docker: EnginesDockerConfig,
}

impl EnginesConfig {
    fn validate(&self) -> Result<(), String> {
        if self.provisioner != Provisioner::Docker {
            return Ok(());
        }
        let d = &self.docker;
        // Values are used as written, so surrounding whitespace is refused
        // rather than trimmed in one place and not in another.
        let padded = |key: &str, v: &str| {
            (v.trim() != v)
                .then(|| format!("engines.docker.{key} has surrounding whitespace: {v:?}"))
        };
        if d.network.trim().is_empty() {
            return Err("engines.provisioner = \"docker\" requires engines.docker.network".into());
        }
        // The project label is the only boundary between two egret instances
        // on one Docker daemon: without it they would act on each other's engines.
        if d.project.trim().is_empty() {
            return Err("engines.provisioner = \"docker\" requires engines.docker.project".into());
        }
        if let Some(url) = &d.callback_url {
            if url.trim().is_empty() {
                return Err(
                    "engines.docker.callback_url is set but empty; unset it to use the default"
                        .into(),
                );
            }
        }
        for (key, v) in [
            ("network", Some(d.network.as_str())),
            ("project", Some(d.project.as_str())),
            ("host", Some(d.host.as_str())),
            ("callback_url", d.callback_url.as_deref()),
        ] {
            if let Some(err) = v.and_then(|v| padded(key, v)) {
                return Err(err);
            }
        }
        // The same schemes `BollardDocker::connect` accepts.
        if !["unix://", "tcp://", "http://"]
            .iter()
            .any(|p| d.host.starts_with(p))
        {
            return Err(format!(
                "engines.docker.host must start with unix://, tcp:// or http://, not \"{}\"",
                d.host
            ));
        }
        if d.allowed_images.is_empty() {
            return Err("engines.docker.allowed_images must not be empty".into());
        }
        Ok(())
    }
}

/// `[starrocks]` — deployment-level overrides for how egret talks to
/// StarRocks FEs. Today: the JDBC driver jars a `jdbc` catalog makes the FE
/// download (design D5). Every field is optional: `egret-catalog-registry`
/// compiles in a pinned driver per dialect, and this section only REPOINTS
/// one — at an internal mirror, or at a `file:///` path baked into the FE
/// image — so a partially-written table can never leave a dialect without a
/// driver.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct StarRocksSettings {
    #[serde(default)]
    pub jdbc_drivers: JdbcDriverOverrides,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct JdbcDriverOverrides {
    #[serde(default)]
    pub postgresql: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub mysql: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub mariadb: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub oracle: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub sqlserver: Option<JdbcDriverOverride>,
    #[serde(default)]
    pub clickhouse: Option<JdbcDriverOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct JdbcDriverOverride {
    #[serde(default)]
    pub driver_url: Option<String>,
    #[serde(default)]
    pub driver_class: Option<String>,
}

/// Serde's fallback for a `[starrocks]` section a deployment's TOML predates.
/// Every field of [`StarRocksSettings`] is itself optional, so its derived
/// `Default` already is the correct fallback — unlike `[workflow]`, there is
/// no unsafe-by-omission field here that needs its own named `default_*`.
fn default_starrocks() -> StarRocksSettings {
    StarRocksSettings::default()
}

/// `[workflow]` — what a deployment may switch on or off about the workflow
/// engine.
///
/// Eight keys: the webhook exposure lever (W6 Task 7), the five the runtime
/// loops read (W10 Task 2) and the scheduler's two (W12). It is a section
/// rather than a bare env var because
/// **`EGRET_<SECTION>__<KEY>` is this deployment's convention**
/// (`EGRET_CATALOG__ENABLED`, `EGRET_OIDC__ENABLED`, `EGRET_RANGER__ENABLED`
/// in `deploy/helm/egret/templates/configmap-env.yaml`), and W6 Task 7 fix
/// round 2 first shipped it as a flat `EGRET_WORKFLOW_WEBHOOKS_ENABLED` read
/// with `std::env::var` — outside the config type, outside the chart, and
/// outside the convention. Fix round 3's Minors 2 and 3.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct WorkflowConfig {
    /// **The exposure lever for the anonymous webhook surface (W6 Task 7), and
    /// it defaults to `false`.**
    ///
    /// `POST /workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}` is
    /// unauthenticated by construction — the key in the path is the whole
    /// credential — and its security review's verdict is *not approved for
    /// exposure*. When this is false the routes are **not mounted at all**, so
    /// their URLs answer exactly as a URL nobody ever wrote does.
    ///
    /// Off is the only safe default for a surface whose own doc says it should
    /// not be exposed; an operator who wants it says so once, deliberately.
    /// `EGRET_WORKFLOW__WEBHOOKS_ENABLED=true`.
    ///
    /// It gates only that group. Workflow **management**
    /// (`/api/ws/{workspace}/workflows/...`, session-authenticated) is
    /// unaffected.
    #[serde(default)]
    pub webhooks_enabled: bool,

    /// **The switch that makes the workflow engine actually run, and it
    /// defaults to `false` for [`WorkflowConfig::webhooks_enabled`]'s own
    /// argument.**
    ///
    /// Until W10 Task 2 `worker::claim::tick` had **no production caller** at
    /// all: a webhook or an API call created an execution row and nothing ever
    /// advanced it. This flag is what starts the two tickers in
    /// `egret_workflow::runtime::spawn` — the claim loop and the lease reaper —
    /// so with it off the engine behaves exactly as it did before, which is the
    /// property that makes `false` the safe value rather than merely the
    /// conservative one.
    ///
    /// Off is the only safe default for a loop that has never run in
    /// production; an operator who wants it says so once, deliberately.
    /// `EGRET_WORKFLOW__WORKER_ENABLED=true`.
    ///
    /// It gates **both** tickers. The reaper exists only to hand back leases
    /// the claim loop took, so a replica with the worker off has nothing for it
    /// to reclaim and a reaper running alone would be a statement with no
    /// counterparty.
    #[serde(default = "default_worker_enabled")]
    pub worker_enabled: bool,

    /// Seconds between claim ticks — the floor on how promptly a runnable task
    /// run reaches a handler.
    ///
    /// Deliberately much finer than `[automation] tick_interval_secs` (30):
    /// that one's floor is a crontab minute, and nothing about a workflow task
    /// is scheduled on a calendar. A task whose predecessor has just committed
    /// is runnable *now*, and this interval is the whole of the latency between
    /// those two facts.
    #[serde(default = "default_tick_interval_secs")]
    pub tick_interval_secs: u64,

    /// How many task runs one claim tick may take.
    ///
    /// It is `tick`'s `n`, which bounds **both** statements it runs: the waker
    /// names at most this many executions and the claim takes at most this many
    /// runs. A larger batch is more work per round trip and a longer worst-case
    /// tick; there is no correctness edge either way, because every run's result
    /// commits in its own transaction.
    #[serde(default = "default_batch")]
    pub batch: usize,

    /// **How many claimed task runs one replica runs at once** (KI-187).
    ///
    /// The claim loop starts every run it claims and keeps ticking while they
    /// run, so a `Sleep` or any slow task holds one of these slots and not the
    /// whole replica. A tick claims at most the free slots (and at most
    /// [`batch`](Self::batch)), so a claimed run always starts at once and its
    /// lease is renewed from the claim on; a runnable run beyond the bound
    /// stays unclaimed for another replica or a later tick. Default `8`; `0`
    /// is a boot error. `EGRET_WORKFLOW__MAX_CONCURRENT_RUNS=8`.
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,

    /// Seconds between reaper passes. Coarser than the claim tick on purpose: a
    /// pass reclaims only leases already past `working_timeout_s`, so running it
    /// more often cannot reclaim anything sooner.
    #[serde(default = "default_reaper_interval_secs")]
    pub reaper_interval_secs: u64,

    /// How long a lease may go unreleased before the reaper takes it back.
    ///
    /// **Both directions matter**, which is `worker::reaper::reap`'s own point:
    /// too long and a replica that died mid-run leaves its task stuck for that
    /// duration; too short and a live worker still inside a slow handler has its
    /// run handed to a second handler. The default is
    /// `worker::DEFAULT_WORKING_TIMEOUT` (3600s) — the value that crate already
    /// ships as the timeout a run is measured against.
    #[serde(default = "default_working_timeout_s")]
    pub working_timeout_s: u64,

    /// **Seconds a script or container task may run when its author wrote no
    /// `timeout:` (W5a spec §6).**
    ///
    /// Applies to the four script task types
    /// (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) and their
    /// aliases `egret.container` and `egret.script.python`; every other task
    /// with no `timeout:` runs unbounded, as in Kestra. A run that outlives it
    /// is FAILED with `Timeout after HH:mm:ss.SSS`, the same as an explicit
    /// `timeout:`. **A deliberate divergence from Kestra**, which runs a
    /// script task with no `timeout:` unbounded: an unbounded container is one
    /// nobody reclaims. The default is
    /// `egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT` (3600s).
    /// `EGRET_WORKFLOW__CONTAINER_DEFAULT_TIMEOUT_S=3600`.
    #[serde(default = "default_container_default_timeout_s")]
    pub container_default_timeout_s: u64,

    /// **Milliseconds between a running task's checks for a kill** (workflow
    /// kill spec §4.2).
    ///
    /// While a handler runs, the worker reads its run's row this often; a run
    /// that the kill route has moved to `KILLING` under this worker's lease
    /// has its handler dropped and cancelled and is recorded `KILLED`. So this
    /// is the latency between a kill and the stop of the task it reaches. A
    /// lease renewal (every third of `working_timeout_s`) notices a kill too,
    /// so this only makes it faster. Default `5000`, minimum
    /// [`MIN_KILL_POLL_MS`] (a boot error below): each check is one indexed
    /// read per running task. `EGRET_WORKFLOW__KILL_POLL_MS=5000`.
    #[serde(default = "default_kill_poll_ms")]
    pub kill_poll_ms: u64,

    /// **The switch that makes `cron:` schedule triggers fire (W12), and it
    /// defaults to `false` for [`WorkflowConfig::worker_enabled`]'s argument.**
    ///
    /// It starts the third ticker in `egret_workflow::runtime::spawn`, which
    /// calls `scheduler::fire_due`: every due `Schedule` trigger creates its
    /// execution. With it off, `workflow.triggers` is still kept in step with
    /// every flow save, and nothing reads it.
    ///
    /// **Independent of `worker_enabled`**, in both directions: a replica may
    /// run the claim loop without firing schedules, and one may fire schedules
    /// without running anything -- which creates executions nobody advances, so
    /// that combination logs a warning at start.
    /// `EGRET_WORKFLOW__SCHEDULER_ENABLED=true`.
    #[serde(default = "default_scheduler_enabled")]
    pub scheduler_enabled: bool,

    /// Seconds between scheduler passes. A pass evaluates every due trigger
    /// once, so this is the latency between a cron instant and its execution,
    /// and -- for a trigger that is behind -- the catch-up rate: one tick per
    /// trigger per pass.
    #[serde(default = "default_trigger_interval_secs")]
    pub trigger_interval_secs: u64,

    /// **How long stored task-run log lines are kept, in days (W11); `0` keeps
    /// them for ever.**
    ///
    /// One global setting (W11 decision D2), not per workspace or per flow.
    /// When the worker runs (`worker_enabled`), an hourly ticker in
    /// `egret_workflow::runtime` deletes lines produced more than this many
    /// days ago, in batches of 10,000 with a 30 s budget per pass, so no
    /// statement holds locks for long. With `0` no ticker is started and the
    /// table only shrinks when executions are deleted (their lines cascade).
    /// `EGRET_WORKFLOW__LOG_RETENTION_DAYS=30`.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,

    /// **How long an ended execution's files are kept, in days (W9a spec §10);
    /// `0` keeps them for ever.** Counted from the loop ROOT's end, so an
    /// iteration's files live as long as its root's.
    /// `EGRET_WORKFLOW__STORAGE_RETENTION_DAYS=30`.
    #[serde(default = "default_storage_retention_days")]
    pub storage_retention_days: u32,

    /// **How long a kept task container or pod outlives its run, in days
    /// (workflow debt D3); `0` keeps them for ever.**
    ///
    /// A task with `taskRunner: {delete: false}` keeps its container (Docker)
    /// or pod (Kubernetes) when its run ends, whatever the end state: success,
    /// failure, kill or timeout. The reaper's orphan sweep removes it once the
    /// run has been terminal longer than this, counted from the run's
    /// `ended_at`. Kestra keeps them for ever; egret bounds them because the
    /// Docker runner has no disk bound. Not a chart value, like the other
    /// workflow tuning keys.
    ///
    /// **A kept container whose run has no row is never swept.** The sweep
    /// removes only work whose run THIS database sees as terminal; one it has
    /// no row for may be another egret's on a shared daemon or namespace. So
    /// deleting an execution (its task runs cascade) leaves its kept
    /// containers or pods behind for an operator to remove.
    /// `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS=7`.
    #[serde(default = "default_kept_container_retention_days")]
    pub kept_container_retention_days: u32,

    /// `[workflow.storage]` -- where execution files live in the `[storage]`
    /// bucket and how large they may be (W9a spec §3, §4.5).
    /// `EGRET_WORKFLOW__STORAGE__MAX_FILE_BYTES=1073741824`.
    #[serde(default)]
    pub storage: WorkflowStorageConfig,

    /// The most log lines one task-run **attempt** stores (W11 decision D4).
    ///
    /// Past it the attempt keeps running and its further lines are not
    /// stored; when it ends, exactly one WARN line says how many were
    /// withheld, written outside the cap. A retry starts a fresh count. A line
    /// longer than 15,360 characters is split like Kestra's, and each piece
    /// counts here. `EGRET_WORKFLOW__LOG_RUN_MAX_LINES=10000`.
    #[serde(default = "default_log_run_max_lines")]
    pub log_run_max_lines: u64,

    /// The most bytes of log text one task-run attempt stores (W11 decision
    /// D4): the byte half of [`WorkflowConfig::log_run_max_lines`]'s cap,
    /// whichever is reached first, with the same single WARN summary.
    /// `EGRET_WORKFLOW__LOG_RUN_MAX_BYTES=10485760` (10 MiB).
    #[serde(default = "default_log_run_max_bytes")]
    pub log_run_max_bytes: u64,

    /// **Per-plugin-type settings, in Kestra's `kestra.plugins.configurations`
    /// shape (W5a spec §4): a list of `{type, values}`.**
    ///
    /// W5a reads exactly one value from it, `values.allowed-images`, keyed by
    /// the task-runner FQCN (`io.kestra.plugin.scripts.runner.docker.Docker`,
    /// `io.kestra.plugin.ee.kubernetes.runner.Kubernetes`): the glob patterns
    /// a script task's image must match before egret runs it.
    ///
    /// **The default is the empty list, and it allows no image at all.** No
    /// entry for a runner, or an entry with an empty `allowed-images`, refuses
    /// every image on that runner (`egret_workflow::scripts::gate`). This is
    /// stricter than Kestra OSS, which has no allowlist, and it is deliberate
    /// (the user's decision of 2026-09-28).
    ///
    /// **W5b adds a second value, `values.allowed-types`, read only on the
    /// entry of type `kestra`** ([`KESTRA_CONFIG_TYPE`]): glob patterns a
    /// Kestra plugin task's FQCN must match. The opposite default: **no
    /// `kestra` entry allows every catalog type**, an empty list allows none
    /// ([`PluginConfigurations::allowed_types`]). `allowed-types` on any other
    /// entry, and `allowed-images` on the `kestra` entry, are refused at boot
    /// ([`WorkflowConfig::validate_plugin_configurations`]).
    ///
    /// `EGRET_WORKFLOW__PLUGIN_CONFIGURATIONS='[{type="io.kestra.plugin.scripts.runner.docker.Docker",values={allowed-images=["python:*"]}}]'`,
    /// or a `[[workflow.plugin_configurations]]` table in `egret.toml`.
    #[serde(default)]
    pub plugin_configurations: PluginConfigurations,

    /// **Where script tasks run (W5a spec §7), and it defaults to `none`.**
    ///
    /// `none`: the worker registers no script task handler, so its registry is
    /// exactly what it was before W5a (`core.log.Log`, `core.debug.Return`,
    /// `core.execution.Fail`, `core.flow.Sleep`), and publish refuses a script
    /// task with no `taskRunner:` because nothing would run it. `docker`: the
    /// local Docker daemon, through the `docker` CLI -- for development and
    /// tests, never for production (a Docker socket in the server is root on
    /// the node). `kubernetes`: one pod per task-run attempt in
    /// [`WorkflowKubernetesConfig::namespace`], the production runner.
    ///
    /// Either runner registers the four script task types
    /// (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) and the
    /// two aliases `egret.container` and `egret.script.python`, and becomes the
    /// runner of a script task with no `taskRunner:`. The runner is built once,
    /// at start. A task naming the OTHER runner in its `taskRunner:` fails at
    /// run with a message saying which runner this deployment has.
    ///
    /// `none` is the safe default for the same reason `worker_enabled` is
    /// `false`: a deployment that says nothing keeps behaving exactly as it
    /// did, and an operator who wants containers says so once, deliberately --
    /// and still has to switch on the workspace feature `workflow_containers`
    /// and allow images in [`WorkflowConfig::plugin_configurations`].
    /// `EGRET_WORKFLOW__CONTAINER_RUNNER=kubernetes`.
    #[serde(default)]
    pub container_runner: ContainerRunner,

    /// `[workflow.kubernetes]` -- the task pods' namespace and bounds, read
    /// only when [`WorkflowConfig::container_runner`] is `kubernetes`. None of
    /// it is author-settable. `EGRET_WORKFLOW__KUBERNETES__NAMESPACE=egret-tasks`.
    #[serde(default)]
    pub kubernetes: WorkflowKubernetesConfig,

    /// `[workflow.plugin_host]` -- the image and JVM that run Kestra plugin
    /// tasks (`type: kestra.<FQCN>`, W5b spec §7). None of it is
    /// author-settable. `EGRET_WORKFLOW__PLUGIN_HOST__IMAGE=egret-plugin-host:dev`.
    #[serde(default)]
    pub plugin_host: PluginHostConfig,
}

/// `[workflow.storage]`: execution files (W9a spec §3, §4.5, §7.2). The
/// bucket and its credentials are `[storage]`'s; this is the key prefix inside
/// it and the caps.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStorageConfig {
    /// The key prefix: an execution file's key is
    /// `{prefix}{workspace_id}/{ns-as-path}/{flow}/executions/{exec}/{tail}`.
    /// Default `workflow/`.
    #[serde(default = "default_workflow_storage_prefix")]
    pub prefix: String,
    /// The largest one file may be; crossing it fails the task naming the cap.
    /// Default 1 GiB.
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// The most bytes one task run may write in all. Default 5 GiB.
    #[serde(default = "default_max_task_bytes")]
    pub max_task_bytes: u64,
    /// The most bytes `read()` returns into a render. Default 10 MiB.
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,
    /// W9b: the most namespace files one listing, search or namespace-file
    /// operation may see; crossing it fails naming the cap (W9b spec §7).
    /// Default 10 000. `EGRET_WORKFLOW__STORAGE__MAX_NAMESPACE_FILES`.
    #[serde(default = "default_max_namespace_files")]
    pub max_namespace_files: u64,
}

/// The longest `workflow.storage.prefix` may be, in bytes (W9b Task 3, the
/// Task 2 review's R3, 2026-10-05). A namespace file's key is
/// `{prefix}{workspace}/{ns-as-path}/_files{path}.v{N}`: 36 (workspace) + 1 +
/// 150 (namespace) + 1 + 6 (`_files`) + 512 (path, W9b's `MAX_PATH_BYTES`) +
/// 12 (`.v` and a `u32`) = 718 bytes after the prefix, so 200 keeps every key
/// under S3's 1024-byte limit with room to spare.
pub const MAX_WORKFLOW_STORAGE_PREFIX_BYTES: usize = 200;

/// The smallest `workflow.storage.max_read_bytes` may be: 4 KiB (W9c Task 2
/// review I2). The Ion reader that reads a KV value and the preview charges
/// one value per 64 bytes of the cap, so a cap of a few bytes refuses even a
/// small value; 4 KiB reads any value of up to 64 parts.
pub const MIN_MAX_READ_BYTES: u64 = 4096;

impl WorkflowStorageConfig {
    /// The boot check (W9a fix round 1 M4), called from [`Config::validate`]:
    /// `prefix` is a non-empty key prefix ending with `/` and not starting
    /// with one (an object key has no leading `/`, and without the trailing
    /// one `workflow` and `workflowX/` would share keys) and at most
    /// [`MAX_WORKFLOW_STORAGE_PREFIX_BYTES`] bytes, every cap is at least
    /// 1, and `max_read_bytes` is at least [`MIN_MAX_READ_BYTES`]. Each
    /// refusal names its key.
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.prefix;
        if p.is_empty() || !p.ends_with('/') || p.starts_with('/') {
            return Err(format!(
                "workflow.storage.prefix {p:?} must be non-empty, end with '/' and not start \
                 with '/' (e.g. \"workflow/\"; EGRET_WORKFLOW__STORAGE__PREFIX)"
            ));
        }
        if p.len() > MAX_WORKFLOW_STORAGE_PREFIX_BYTES {
            return Err(format!(
                "workflow.storage.prefix is {} bytes long; it may be at most \
                 {MAX_WORKFLOW_STORAGE_PREFIX_BYTES} bytes, so that a namespace file's object key \
                 stays under S3's 1024-byte limit (EGRET_WORKFLOW__STORAGE__PREFIX)",
                p.len()
            ));
        }
        for (key, value) in [
            ("max_file_bytes", self.max_file_bytes),
            ("max_task_bytes", self.max_task_bytes),
            ("max_read_bytes", self.max_read_bytes),
            ("max_namespace_files", self.max_namespace_files),
        ] {
            if value == 0 {
                return Err(format!("workflow.storage.{key} must be at least 1"));
            }
        }
        if self.max_read_bytes < MIN_MAX_READ_BYTES {
            return Err(format!(
                "workflow.storage.max_read_bytes is {} bytes; it must be at least \
                 {MIN_MAX_READ_BYTES} bytes (4 KiB), the least the Ion reader of a KV value \
                 or a preview reads a small value under \
                 (EGRET_WORKFLOW__STORAGE__MAX_READ_BYTES)",
                self.max_read_bytes
            ));
        }
        Ok(())
    }
}

impl Default for WorkflowStorageConfig {
    fn default() -> Self {
        Self {
            prefix: default_workflow_storage_prefix(),
            max_file_bytes: default_max_file_bytes(),
            max_task_bytes: default_max_task_bytes(),
            max_read_bytes: default_max_read_bytes(),
            max_namespace_files: default_max_namespace_files(),
        }
    }
}

fn default_workflow_storage_prefix() -> String {
    "workflow/".to_string()
}

/// 1 GiB -- see [`WorkflowStorageConfig::max_file_bytes`].
fn default_max_file_bytes() -> u64 {
    1 << 30
}

/// 5 GiB -- see [`WorkflowStorageConfig::max_task_bytes`].
fn default_max_task_bytes() -> u64 {
    5 << 30
}

/// 10 MiB -- see [`WorkflowStorageConfig::max_read_bytes`].
fn default_max_read_bytes() -> u64 {
    10 << 20
}

/// `[apps.storage]`: Data Apps' draft/version source and grid-layout JSON. The
/// bucket and its credentials are `[storage]`'s; this is the key prefix inside
/// it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppsStorageConfig {
    /// The key prefix: a draft revision's key is
    /// `{prefix}{workspace_id}/{app_id}/draft/{revision}.py` (and
    /// `draft/{revision}.layout.json`); a version's is
    /// `{prefix}{workspace_id}/{app_id}/versions/{version_no}/source.py`.
    /// Default `apps/`.
    #[serde(default = "default_apps_storage_prefix")]
    pub prefix: String,
}

fn default_apps_storage_prefix() -> String {
    "apps/".to_string()
}

impl Default for AppsStorageConfig {
    fn default() -> Self {
        Self {
            prefix: default_apps_storage_prefix(),
        }
    }
}

impl AppsStorageConfig {
    /// Same two rules as `WorkflowStorageConfig::validate`: `prefix` is a
    /// non-empty key prefix ending with `/` and not starting with one.
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.prefix;
        if p.is_empty() || !p.ends_with('/') || p.starts_with('/') {
            return Err(format!(
                "apps.storage.prefix {p:?} must be non-empty, end with '/' and not start \
                 with '/' (e.g. \"apps/\"; EGRET_APPS__STORAGE__PREFIX)"
            ));
        }
        Ok(())
    }
}

/// 10 000 -- see [`WorkflowStorageConfig::max_namespace_files`].
fn default_max_namespace_files() -> u64 {
    10_000
}

/// `30` -- see [`WorkflowConfig::storage_retention_days`].
fn default_storage_retention_days() -> u32 {
    30
}

/// `7` -- see [`WorkflowConfig::kept_container_retention_days`].
fn default_kept_container_retention_days() -> u32 {
    7
}

/// `[workflow.plugin_host]`: how egret runs the Kestra plugin host image (W5b
/// spec §7).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginHostConfig {
    /// The plugin host image (`deploy/docker/plugin-host.Dockerfile`).
    /// **Required when [`WorkflowConfig::container_runner`] is not `none`**
    /// ([`WorkflowConfig::validate_plugin_host`], a boot error). It is **not**
    /// matched against `allowed-images`: the author cannot choose it.
    #[serde(default)]
    pub image: Option<String>,
    /// The JVM's `-Xmx`, below the pod's default memory limit. Default `384m`.
    /// Digits and one optional unit letter (`k`, `m`, `g`), checked at boot:
    /// it is written into the container's shell command.
    #[serde(default = "default_jvm_max_heap")]
    pub jvm_max_heap: String,
    /// Seconds a timed-out plugin task gets between SIGTERM (the host calls
    /// the plugin's `kill()`, which cancels the query on the engine) and
    /// SIGKILL. Default `15`, minimum [`MIN_KILL_GRACE_S`] (a boot error below).
    /// Script tasks keep grace 0.
    #[serde(default = "default_kill_grace_s")]
    pub kill_grace_s: u64,
}

impl Default for PluginHostConfig {
    fn default() -> Self {
        Self {
            image: None,
            jvm_max_heap: default_jvm_max_heap(),
            kill_grace_s: default_kill_grace_s(),
        }
    }
}

/// `384m` -- see [`PluginHostConfig::jvm_max_heap`].
fn default_jvm_max_heap() -> String {
    "384m".to_string()
}

/// The smallest [`PluginHostConfig::kill_grace_s`] the boot check accepts:
/// the host waits `grace - 2 s` for `kill()`, so anything below leaves it 0 s.
pub const MIN_KILL_GRACE_S: u64 = 3;

/// `15` -- see [`PluginHostConfig::kill_grace_s`].
fn default_kill_grace_s() -> u64 {
    15
}

/// [`WorkflowConfig::container_runner`].
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRunner {
    /// No script task handler is registered.
    #[default]
    None,
    /// The local Docker daemon (development and tests).
    Docker,
    /// One pod per task-run attempt (production).
    Kubernetes,
}

/// `[workflow.kubernetes]`: egret's side of every task pod (W5a spec §5).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowKubernetesConfig {
    /// The namespace task pods are created in. It must exist, enforce Pod
    /// Security `restricted`, and grant egret's ServiceAccount exactly `pods`
    /// get/list/watch/create/delete, `pods/log` get and `pods/exec` get/create --
    /// which is what the chart's `workflow.containerRunner: kubernetes`
    /// renders. Default `egret-tasks`.
    #[serde(default = "default_task_namespace")]
    pub namespace: String,
    /// Requests and limits a task gets where its `taskRunner.resources` says
    /// nothing. Default: request `100m`/`128Mi`/ephemeral-storage `256Mi`,
    /// limit `1`/`512Mi`/ephemeral-storage `10Gi`.
    #[serde(default = "default_task_resources")]
    pub default_resources: TaskResources,
    /// The most a task may ask for; an author's request or limit above it is
    /// capped to it, and an unset side here means no cap on that side.
    /// Default: limit `2`/`2Gi`/ephemeral-storage `10Gi`, no request cap. A
    /// limit left unset everywhere becomes this cap, so with the defaults no
    /// task pod is ever without an ephemeral-storage limit.
    #[serde(default = "default_task_max_resources")]
    pub max_resources: TaskResources,
    /// Names of existing `kubernetes.io/dockerconfigjson` Secrets in the task
    /// namespace, attached to every task pod. Default none.
    /// `EGRET_WORKFLOW__KUBERNETES__IMAGE_PULL_SECRETS='["regcred"]'`.
    #[serde(default)]
    pub image_pull_secrets: Vec<String>,
    /// The uid every container of a task pod runs as. Must be positive: task
    /// pods run with `runAsNonRoot`, and egret refuses to create a pod with
    /// `0` rather than let the kubelet blame the author. Default `1000`.
    #[serde(default = "default_task_run_as_user")]
    pub run_as_user: i64,
    /// egret's helper image (`deploy/docker/task-helper.Dockerfile`), which
    /// moves input and output files in and out of a task pod. Default
    /// `egret-task-helper:dev`.
    #[serde(default = "default_task_helper_image")]
    pub helper_image: String,
}

impl Default for WorkflowKubernetesConfig {
    fn default() -> Self {
        Self {
            namespace: default_task_namespace(),
            default_resources: default_task_resources(),
            max_resources: default_task_max_resources(),
            image_pull_secrets: Vec::new(),
            run_as_user: default_task_run_as_user(),
            helper_image: default_task_helper_image(),
        }
    }
}

/// Kubernetes requests and limits, as quantities written the Kubernetes way
/// (`500m`, `1`, `256Mi`). `None` leaves that side unset.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskResources {
    #[serde(default)]
    pub request: TaskResourceAmounts,
    #[serde(default)]
    pub limit: TaskResourceAmounts,
}

/// One side of [`TaskResources`].
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskResourceAmounts {
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    /// Kubernetes `ephemeral-storage`: the node disk a task pod may use --
    /// its working directory, its stderr file and `main`'s own writable layer
    /// and logs. A task has no say in it (Kestra's `resources` has no such
    /// key). The limit is also the working directory's and the stderr
    /// volume's `emptyDir.sizeLimit`, so a task that writes past it is evicted
    /// by the kubelet and FAILED with the kubelet's reason, instead of filling
    /// the node until `DiskPressure` evicts its neighbours. Spelled
    /// `ephemeral_storage` in TOML and env
    /// (`EGRET_WORKFLOW__KUBERNETES__MAX_RESOURCES__LIMIT__EPHEMERAL_STORAGE`),
    /// like every other key here.
    #[serde(default)]
    pub ephemeral_storage: Option<String>,
}

/// `egret-tasks` -- see [`WorkflowKubernetesConfig::namespace`].
fn default_task_namespace() -> String {
    "egret-tasks".to_string()
}

/// See [`WorkflowKubernetesConfig::default_resources`].
fn default_task_resources() -> TaskResources {
    TaskResources {
        request: TaskResourceAmounts {
            cpu: Some("100m".to_string()),
            memory: Some("128Mi".to_string()),
            ephemeral_storage: Some("256Mi".to_string()),
        },
        limit: TaskResourceAmounts {
            cpu: Some("1".to_string()),
            memory: Some("512Mi".to_string()),
            ephemeral_storage: Some("10Gi".to_string()),
        },
    }
}

/// See [`WorkflowKubernetesConfig::max_resources`].
fn default_task_max_resources() -> TaskResources {
    TaskResources {
        request: TaskResourceAmounts::default(),
        limit: TaskResourceAmounts {
            cpu: Some("2".to_string()),
            memory: Some("2Gi".to_string()),
            ephemeral_storage: Some("10Gi".to_string()),
        },
    }
}

/// `1000` -- see [`WorkflowKubernetesConfig::run_as_user`].
fn default_task_run_as_user() -> i64 {
    1000
}

/// `egret-task-helper:dev` -- see [`WorkflowKubernetesConfig::helper_image`].
fn default_task_helper_image() -> String {
    "egret-task-helper:dev".to_string()
}

/// Kestra's `kestra.plugins.configurations`: one `{type, values}` per plugin
/// type. See [`WorkflowConfig::plugin_configurations`].
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct PluginConfigurations(pub Vec<PluginConfiguration>);

impl PluginConfigurations {
    /// The `allowed-images` of the entry for `plugin_type`, or `None` when no
    /// entry names that type. When several entries name the same type, their
    /// lists are concatenated (Kestra merges configurations for one type the
    /// same way: every matching entry contributes).
    pub fn allowed_images(&self, plugin_type: &str) -> Option<Vec<&str>> {
        let mut found = false;
        let mut out = Vec::new();
        for entry in self.0.iter().filter(|e| e.r#type == plugin_type) {
            found = true;
            out.extend(
                entry
                    .values
                    .allowed_images
                    .iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        found.then_some(out)
    }

    /// W5b: the `allowed-types` of the [`KESTRA_CONFIG_TYPE`] entry -- glob
    /// patterns a `kestra.<FQCN>` plugin task's FQCN must match -- or `None`
    /// when no entry has that type. **`None` allows every catalog type** (the
    /// catalog was reviewed as a PR); `Some(vec![])` allows none. An entry
    /// with no `allowed-types` key is `Some(vec![])`: it names the type, so it
    /// is a list, and an empty one. Several entries concatenate, as
    /// [`PluginConfigurations::allowed_images`] does.
    pub fn allowed_types(&self) -> Option<Vec<&str>> {
        let mut found = false;
        let mut out = Vec::new();
        for entry in self.0.iter().filter(|e| e.r#type == KESTRA_CONFIG_TYPE) {
            found = true;
            out.extend(
                entry
                    .values
                    .allowed_types
                    .iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        found.then_some(out)
    }
}

/// W5b: the `plugin_configurations` entry type that governs Kestra plugin
/// tasks (`type: kestra.<FQCN>`); `egret_workflow::plugins::CONFIG_TYPE`.
pub const KESTRA_CONFIG_TYPE: &str = "kestra";

impl WorkflowConfig {
    /// **The boot check on [`WorkflowConfig::plugin_configurations`]** (W5b
    /// spec §5): `allowed-types` belongs only on the [`KESTRA_CONFIG_TYPE`]
    /// entry and `allowed-images` never does (the plugin host's image is
    /// `workflow.plugin_host.image`, not the author's to choose). Either one
    /// in the wrong place would be read by nothing, so it is refused by name
    /// rather than accepted and ignored -- the same rule
    /// [`PluginConfigurationValues`]'s `deny_unknown_fields` applies to an
    /// unknown key. Called by `egret_workflow::runtime::handlers_for` before
    /// anything else it does.
    pub fn validate_plugin_configurations(&self) -> Result<(), String> {
        for entry in &self.plugin_configurations.0 {
            let is_kestra = entry.r#type == KESTRA_CONFIG_TYPE;
            if is_kestra && entry.values.allowed_images.is_some() {
                return Err(format!(
                    "workflow.plugin_configurations: 'allowed-images' is not read on the \
                     '{KESTRA_CONFIG_TYPE}' entry (the plugin host image is \
                     workflow.plugin_host.image); remove it"
                ));
            }
            if !is_kestra && entry.values.allowed_types.is_some() {
                return Err(format!(
                    "workflow.plugin_configurations: 'allowed-types' is read only on the \
                     '{KESTRA_CONFIG_TYPE}' entry, not on '{}'; remove it",
                    entry.r#type
                ));
            }
        }
        Ok(())
    }

    /// **The boot check on [`WorkflowConfig::plugin_host`]** (W5b spec §7):
    /// with a container runner configured, `plugin_host.image` is required --
    /// every `kestra.*` task would otherwise fail at run for a setting the
    /// operator never made -- and `jvm_max_heap` must be a JVM size (digits
    /// and one optional `k`/`m`/`g`), because it is written into the
    /// container's shell command. Called by
    /// `egret_workflow::runtime::handlers_for` once it knows a runner exists.
    pub fn validate_plugin_host(&self) -> Result<(), String> {
        let heap = self.plugin_host.jvm_max_heap.as_str();
        let digits = heap.trim_end_matches(['k', 'K', 'm', 'M', 'g', 'G']);
        let unit_len = heap.len() - digits.len();
        if digits.is_empty() || unit_len > 1 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "workflow.plugin_host.jvm_max_heap: {heap:?} is not a JVM size (e.g. 384m)"
            ));
        }
        // The host's kill wait is the grace less 2 s (spec §7.1): below 3 s it is 0 and the hook
        // returns before the cancel is sent, and 0 takes the runner's SIGKILL path.
        if self.plugin_host.kill_grace_s < MIN_KILL_GRACE_S {
            return Err(format!(
                "workflow.plugin_host.kill_grace_s: {} is below the minimum {MIN_KILL_GRACE_S}; \
                 the plugin's kill() needs the grace less 2 s to cancel the query",
                self.plugin_host.kill_grace_s
            ));
        }
        if self.container_runner != ContainerRunner::None
            && self
                .plugin_host
                .image
                .as_deref()
                .is_none_or(|i| i.trim().is_empty())
        {
            return Err(
                "workflow.plugin_host.image is required when workflow.container_runner is not \
                 none: it is the image Kestra plugin tasks (kestra.*) run in \
                 (EGRET_WORKFLOW__PLUGIN_HOST__IMAGE)"
                    .into(),
            );
        }
        Ok(())
    }
}

/// One entry of [`PluginConfigurations`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginConfiguration {
    /// The plugin FQCN the values apply to.
    pub r#type: String,
    #[serde(default)]
    pub values: PluginConfigurationValues,
}

/// The `values` of a [`PluginConfiguration`].
///
/// `deny_unknown_fields`: Kestra's `values` is an open map, but egret reads one
/// key from it, and a value egret would silently ignore (Kestra's Docker
/// `volume-enabled`, say) is refused at boot with its name rather than accepted
/// and not acted on.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginConfigurationValues {
    /// Glob patterns (`*` any run of characters except `/`, `?` one character
    /// other than `/`; no `**`, so a pattern cannot reach another registry
    /// host -- see `egret_workflow::scripts::gate::glob_match`) matched
    /// against a script task's image reference. See W5a spec §4 for the
    /// normalisation applied to the reference before matching.
    ///
    /// `Option` so that an `allowed-images` written where it is not read (on
    /// the `kestra` entry) is visible to
    /// [`WorkflowConfig::validate_plugin_configurations`] even when empty;
    /// `None` and `Some(vec![])` both allow no image.
    #[serde(
        default,
        rename = "allowed-images",
        skip_serializing_if = "Option::is_none"
    )]
    pub allowed_images: Option<Vec<String>>,
    /// W5b: glob patterns matched against a plugin task's FQCN (without
    /// `kestra.`), read only on the `kestra` entry -- see
    /// [`PluginConfigurations::allowed_types`]. `*` spans package segments (a
    /// FQCN holds no `/`): `io.kestra.plugin.jdbc.*` allows every JDBC class.
    #[serde(
        default,
        rename = "allowed-types",
        skip_serializing_if = "Option::is_none"
    )]
    pub allowed_types: Option<Vec<String>>,
}

/// `false` — see [`WorkflowConfig::worker_enabled`]. A named function rather
/// than `#[serde(default)]`'s `bool::default()` so that the safe side of the
/// switch is a literal a reader can see, exactly as [`default_workflow`] does
/// for `webhooks_enabled`.
fn default_worker_enabled() -> bool {
    false
}

/// `1` — see [`WorkflowConfig::tick_interval_secs`].
fn default_tick_interval_secs() -> u64 {
    1
}

/// `8` — see [`WorkflowConfig::batch`].
fn default_batch() -> usize {
    8
}

/// `8` — see [`WorkflowConfig::max_concurrent_runs`].
fn default_max_concurrent_runs() -> usize {
    8
}

/// `60` — see [`WorkflowConfig::reaper_interval_secs`].
fn default_reaper_interval_secs() -> u64 {
    60
}

/// `3600` — see [`WorkflowConfig::working_timeout_s`]. Equal to
/// `egret_workflow::worker::DEFAULT_WORKING_TIMEOUT`, which this crate cannot
/// name: `egret-config` is below `egret-workflow`, not above it. The two are
/// pinned together by `egret-workflow`'s `tests/runtime.rs`,
/// `the_config_default_working_timeout_equals_the_workers_own`, rather than by a
/// shared constant -- without that assertion this `3600` would be a second copy
/// of a constant no compiler compares to the first.
fn default_working_timeout_s() -> u64 {
    3600
}

/// `3600` -- see [`WorkflowConfig::container_default_timeout_s`]. Equal to
/// `egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT`, which this crate cannot
/// name; `egret-workflow`'s `tests/runtime.rs`,
/// `the_config_default_container_timeout_equals_the_workers_own`, pins the two.
fn default_container_default_timeout_s() -> u64 {
    3600
}

/// `5000` -- see [`WorkflowConfig::kill_poll_ms`].
fn default_kill_poll_ms() -> u64 {
    5000
}

/// The smallest [`WorkflowConfig::kill_poll_ms`] egret boots with: below it
/// the checks are a steady read load per running task for a latency nobody
/// can see.
pub const MIN_KILL_POLL_MS: u64 = 100;

/// `false` -- see [`WorkflowConfig::scheduler_enabled`].
fn default_scheduler_enabled() -> bool {
    false
}

/// `1` -- see [`WorkflowConfig::trigger_interval_secs`].
fn default_trigger_interval_secs() -> u64 {
    1
}

/// `30` -- see [`WorkflowConfig::log_retention_days`].
fn default_log_retention_days() -> u32 {
    30
}

/// `10000` -- see [`WorkflowConfig::log_run_max_lines`]. Equal to
/// `egret_workflow::logs::LogLimits::default()`, which this crate cannot name.
fn default_log_run_max_lines() -> u64 {
    10_000
}

/// `10485760` (10 MiB) -- see [`WorkflowConfig::log_run_max_bytes`]. Equal to
/// `egret_workflow::logs::LogLimits::default()`.
fn default_log_run_max_bytes() -> u64 {
    10_485_760
}

/// Serde's fallback for a `[workflow]` section a deployment's TOML predates.
///
/// Every value is the field's own `default_*` function, so the defaults layer
/// (`Config::default()`) and a partially-written `[workflow]` section in TOML
/// cannot disagree about any key — there is one literal per field and both
/// paths read it.
///
/// `webhooks_enabled: false` is written out rather than derived so that the safe
/// side of the security verdict is a literal a reader can see rather than a
/// property of `bool`; `worker_enabled` is `false` for the same reason and is
/// the one key that goes through `default_worker_enabled` to say so.
fn default_workflow() -> WorkflowConfig {
    WorkflowConfig {
        webhooks_enabled: false,
        worker_enabled: default_worker_enabled(),
        tick_interval_secs: default_tick_interval_secs(),
        batch: default_batch(),
        max_concurrent_runs: default_max_concurrent_runs(),
        reaper_interval_secs: default_reaper_interval_secs(),
        working_timeout_s: default_working_timeout_s(),
        container_default_timeout_s: default_container_default_timeout_s(),
        kill_poll_ms: default_kill_poll_ms(),
        scheduler_enabled: default_scheduler_enabled(),
        trigger_interval_secs: default_trigger_interval_secs(),
        log_retention_days: default_log_retention_days(),
        storage_retention_days: default_storage_retention_days(),
        kept_container_retention_days: default_kept_container_retention_days(),
        storage: WorkflowStorageConfig::default(),
        log_run_max_lines: default_log_run_max_lines(),
        log_run_max_bytes: default_log_run_max_bytes(),
        plugin_configurations: PluginConfigurations::default(),
        container_runner: ContainerRunner::None,
        kubernetes: WorkflowKubernetesConfig::default(),
        plugin_host: PluginHostConfig::default(),
    }
}

/// Serde's fallback for a `[data_api]` section a deployment's TOML predates.
///
/// `Config::figment()` layers `Serialized::defaults(Config::default())` under
/// the file, so a missing section is already covered — but only for callers
/// that go through `figment()`. Anything deserialising a `Config` directly
/// (a test, a future admin endpoint) would otherwise fail on a field that has
/// only ever had one sensible value.
fn default_data_api() -> DataApiConfig {
    DataApiConfig {
        access_log_retention_days: 90,
        // Unset: fall back to `catalog.public_url`. See
        // `DataApiConfig::public_url` for why this is a separate key at all.
        public_url: None,
        replicas: default_replicas(),
        // Unset: process-local limiter and cache, which is correct at one
        // replica and refused above one.
        shared_store_url: None,
    }
}

/// Serde's fallback for an `[idp]` section a deployment's TOML predates —
/// the same reason [`default_data_api`] exists, and the reason this key is a
/// widening: absent, the issuer is `catalog.public_url`, exactly as before.
fn default_idp() -> IdpConfig {
    IdpConfig { issuer: None }
}

/// Alerts & Reports (P2). The scheduler is on by default: a schedule the user
/// created and can see in the UI, which silently never runs, is worse than one
/// that runs and errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    /// Turn the ticker off entirely (a replica dedicated to serving, say).
    pub enabled: bool,
    /// Seconds between ticks. The floor on how promptly a due schedule runs;
    /// finer than the finest crontab (1 minute) buys nothing.
    pub tick_interval_secs: u64,
    /// Hosts a webhook may reach even though they resolve into private space.
    ///
    /// The escape hatch from the design's SSRF rule. It exists so that
    /// "webhook our own in-cluster incident bot" has a supported answer —
    /// without one, an operator who needs it turns the whole check off.
    pub webhook_allow_hosts: Vec<String>,
    /// Base URL of the `egret-renderer` service (P4), reachable from THIS
    /// server. In-cluster this is a Service DNS name, not a public hostname.
    pub renderer_url: String,
    /// Base URL the RENDERER can reach to fetch `/embed/print/{kind}/{id}` —
    /// in-cluster, this server's OWN Service name, which is generally not the
    /// same as the user-facing hostname (`catalog.public_url`) an ingress or a
    /// browser would use. Getting this wrong is a silent failure mode: the
    /// renderer's navigation would time out trying to reach an address only a
    /// browser outside the cluster can resolve.
    pub public_url: String,
    /// Base URL a PERSON can open, for the links in notifications.
    ///
    /// Deliberately separate from `public_url` above: that one is what the
    /// renderer resolves in-cluster, and a mail whose "open this" link points
    /// at a Service DNS name is a dead end in the recipient's inbox. Falls back
    /// to `public_url` when unset, which is correct in local dev where the two
    /// really are the same address.
    #[serde(default)]
    pub browser_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub absolute_ttl_hours: u32,
    pub idle_ttl_minutes: u32,
    pub secure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfConfig {
    pub cookie_name: String,
    pub header_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Postgres connection URL, e.g. `postgres://egret:egret@localhost:5432/egret`.
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// `tracing_subscriber` env-filter directive, e.g. `info,egret=debug`.
    pub filter: String,
    /// Emit JSON logs when true, pretty logs when false.
    pub json: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "postgres://egret:egret@localhost:5432/egret".into(),
                max_connections: 10,
            },
            log: LogConfig {
                filter: "info".into(),
                json: false,
            },
            session: SessionConfig {
                cookie_name: "egret_session".into(),
                absolute_ttl_hours: 24,
                idle_ttl_minutes: 60,
                secure: false,
            },
            csrf: CsrfConfig {
                cookie_name: "egret_csrf".into(),
                header_name: "X-Egret-CSRF".into(),
            },
            rate_limit: RateLimitConfig {
                login_max_attempts: 5,
                login_window_secs: 300,
                login_lockout_secs: 300,
                token_max_attempts: 60,
                token_window_secs: 60,
                token_lockout_secs: 60,
            },
            oidc: OidcConfig {
                enabled: false,
                issuer_url: String::new(),
                client_id: String::new(),
                client_secret: String::new(),
                redirect_url: "http://localhost:8080/api/auth/oidc/callback".into(),
                // Groups come from the IdP's group protocol-mapper (emitted on the
                // `groups_claim`), so a `groups` *scope* is NOT requested by default —
                // Keycloak rejects an undefined scope with `invalid_scope`. Add "groups"
                // here only if your IdP exposes it as a client scope.
                scopes: vec!["openid".into(), "email".into(), "profile".into()],
                groups_claim: "groups".into(),
                post_login_redirect: "/".into(),
                catalog_audience: "egret-catalog".into(),
            },
            idp: default_idp(),
            security: SecurityConfig {
                secret_key: String::new(),
            },
            catalog: CatalogSettings {
                enabled: false,
                warehouse: "s3://warehouse".into(),
                auth_token: String::new(),
                // Derived from `server.port` in `Config::load` (KI-141).
                public_url: None,
                system_user: default_system_user(),
            },
            storage: StorageSettings {
                endpoint: Some("http://localhost:9000".into()),
                region: "us-east-1".into(),
                bucket: "warehouse".into(),
                access_key: String::new(),
                secret_key: String::new(),
                path_style: true,
            },
            ranger: RangerSettings {
                enabled: false,
                auth_token: String::new(),
                auth_mode: RangerAuthMode::Required,
            },
            secrets: SecretsConfig {
                master_key: String::new(),
            },
            engine: EngineConfig {
                max_rows: 100_000,
                query_timeout_secs: 60,
                preview_rows: 100,
                results_prefix: "query-results".into(),
            },
            data_api: default_data_api(),
            workflow: default_workflow(),
            starrocks: default_starrocks(),
            apps: default_apps(),
            engines: EnginesConfig::default(),
            audit: AuditConfig {
                enabled: true,
                retention_days: 90,
                channel_capacity: 8192,
                batch_size: 256,
                flush_interval_ms: 1000,
            },
            automation: AutomationConfig {
                enabled: true,
                tick_interval_secs: 30,
                webhook_allow_hosts: vec![],
                renderer_url: "http://egret-renderer:8080".into(),
                public_url: "http://egret:8080".into(),
                browser_url: None,
            },
        }
    }
}

impl Config {
    /// Build the layered figment (defaults < file < env). Exposed for testing.
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("egret.toml"))
            .merge(Env::prefixed("EGRET_").split("__"))
    }

    /// Load and validate configuration from all layers.
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, figment::Error> {
        let mut cfg: Self = Self::figment().extract()?;
        cfg.derive_unset_urls();
        Ok(cfg)
    }

    /// Fill the URLs whose default depends on another key. Today one:
    /// `catalog.public_url`, unset or blank, becomes this server on
    /// `localhost` at `server.port` (see [`CatalogSettings::public_url`]).
    fn derive_unset_urls(&mut self) {
        let unset = self
            .catalog
            .public_url
            .as_deref()
            .is_none_or(|u| u.trim().is_empty());
        if unset {
            self.catalog.public_url = Some(format!("http://localhost:{}", self.server.port));
        }
    }

    /// Validate cross-field invariants after loading. Call at boot.
    pub fn validate(&self) -> Result<(), String> {
        if self.oidc.enabled {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.security.secret_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 64 {
                return Err(
                    "oidc.enabled requires security.secret_key (base64, >=64 bytes)".into(),
                );
            }
            if self.oidc.issuer_url.is_empty() || self.oidc.client_id.is_empty() {
                return Err("oidc.enabled requires issuer_url and client_id".into());
            }
        }
        if self.catalog.enabled {
            if self.catalog.warehouse.is_empty() || self.catalog.auth_token.is_empty() {
                return Err("catalog.enabled requires warehouse and auth_token".into());
            }
            if self.storage.access_key.is_empty()
                || self.storage.secret_key.is_empty()
                || self.storage.bucket.is_empty()
            {
                return Err("catalog.enabled requires storage access_key/secret_key/bucket".into());
            }
        }
        // ranger.auth_token may be empty: an empty token is fine; engines authenticate with
        // their own credentials; `anonymous` (ranger.auth_mode) must be chosen explicitly.
        if !self.secrets.master_key.is_empty() {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.secrets.master_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 32 {
                return Err("secrets.master_key must be base64 of >=32 bytes".into());
            }
        }
        // W9a spec §15: a container runner moves files in and out of task pods
        // through the `[storage]` bucket (the pod itself holds no credential),
        // so it cannot start without one. NOTE: `Config::validate` runs for
        // EVERY `egret` subcommand that loads config (`main.rs`, before the
        // dispatch), so this also gates `egret migrate` and `egret init`, not
        // only `serve`.
        if self.workflow.container_runner != ContainerRunner::None
            && (self.storage.bucket.is_empty()
                || self.storage.access_key.is_empty()
                || self.storage.secret_key.is_empty())
        {
            return Err(
                "workflow.container_runner other than none requires [storage] bucket, \
                        access_key and secret_key: execution files live there \
                        (EGRET_STORAGE__BUCKET, EGRET_STORAGE__ACCESS_KEY, \
                        EGRET_STORAGE__SECRET_KEY)"
                    .into(),
            );
        }
        self.workflow.storage.validate()?;
        if self.workflow.kill_poll_ms < MIN_KILL_POLL_MS {
            return Err(format!(
                "workflow.kill_poll_ms: {} is below the minimum {MIN_KILL_POLL_MS} \
                 (EGRET_WORKFLOW__KILL_POLL_MS)",
                self.workflow.kill_poll_ms
            ));
        }
        if self.workflow.max_concurrent_runs == 0 {
            return Err("workflow.max_concurrent_runs must be >= 1 \
                 (EGRET_WORKFLOW__MAX_CONCURRENT_RUNS)"
                .into());
        }
        if self.engine.max_rows == 0 || self.engine.query_timeout_secs == 0 {
            return Err("engine.max_rows and engine.query_timeout_secs must be >= 1".into());
        }
        if self.engine.preview_rows > self.engine.max_rows {
            return Err("engine.preview_rows must be <= engine.max_rows".into());
        }
        if self.apps.session_idle_secs > self.apps.session_absolute_secs {
            return Err("apps.session_idle_secs must be <= apps.session_absolute_secs".into());
        }
        if !self.apps.origin.is_empty() {
            let is_origin = |o: &str| {
                (o.starts_with("https://") || o.starts_with("http://"))
                    && !o
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .contains('/')
            };
            if !is_origin(&self.apps.origin) || !is_origin(&self.apps.egret_origin) {
                return Err(
                    "apps.origin and apps.egret_origin must both be origins (scheme://host[:port], \
                     no path or trailing slash) when apps.origin is set"
                        .into(),
                );
            }
            if self.apps.origin == self.apps.egret_origin {
                return Err(
                    "apps.origin must differ from apps.egret_origin: author JavaScript on the \
                     apps origin would otherwise run as egret itself"
                        .into(),
                );
            }
            if self.apps.ticket_ttl_secs == 0 || self.apps.cookie_ttl_secs == 0 {
                return Err("apps.ticket_ttl_secs and apps.cookie_ttl_secs must be >= 1".into());
            }
        }
        let pod = &self.apps.pod;
        match pod.launcher.as_str() {
            "none" => {}
            "subprocess" | "kubernetes" => {
                if self.apps.origin.is_empty() {
                    return Err(format!(
                        "apps.pod.launcher = \"{}\" needs apps.origin: pods are reached through the apps host",
                        pod.launcher
                    ));
                }
                if pod.callback_url.is_empty() {
                    return Err(
                        "apps.pod.callback_url is required: pods call egret back there".into(),
                    );
                }
                if pod.ready_timeout_secs == 0 || pod.idle_secs < 60 {
                    return Err(
                        "apps.pod.ready_timeout_secs must be >= 1 and idle_secs >= 60".into(),
                    );
                }
                if pod.launcher == "kubernetes"
                    && (pod.image.is_empty() || pod.namespace.is_empty())
                {
                    return Err(
                        "apps.pod.launcher = \"kubernetes\" needs image and namespace".into(),
                    );
                }
                if pod.launcher == "subprocess"
                    && (pod.command.is_empty()
                        || pod.config_dir.is_empty()
                        || pod.venv_shim_dir.is_empty())
                {
                    return Err(
                        "apps.pod.launcher = \"subprocess\" needs command, config_dir and \
                         venv_shim_dir"
                            .into(),
                    );
                }
                if !matches!(pod.run_kernel_mode.as_str(), "thread" | "forced_ipc") {
                    return Err(format!(
                        "apps.pod.run_kernel_mode must be thread or forced_ipc, not \"{}\"",
                        pod.run_kernel_mode
                    ));
                }
                if pod.run_session_cap == 0 {
                    return Err("apps.pod.run_session_cap must be >= 1".into());
                }
            }
            other => {
                return Err(format!(
                    "apps.pod.launcher must be none, subprocess or kubernetes, not \"{other}\""
                ))
            }
        }
        if self.storage.bucket.is_empty()
            || self.storage.access_key.is_empty()
            || self.storage.secret_key.is_empty()
        {
            return Err(
                "the apps API requires [storage] bucket, access_key, secret_key (it stores draft/version content there regardless of apps.pod.launcher)".into(),
            );
        }
        self.apps.storage.validate()?;
        self.engines.validate()?;
        Ok(())
    }

    /// Every `EGRET_*` environment variable and every `egret.toml` key that no
    /// configuration field consumes (KI-140), each with a did-you-mean hint
    /// when a real key is close. Loading ignores these silently (only a few
    /// sub-tables use `deny_unknown_fields`), so a misspelled
    /// `EGRET_SERVER__PORTT` would otherwise leave the default in force with
    /// no sign. `main.rs` logs them as WARNINGS, not errors: refusing to start
    /// would break deployments that carry a stale key today.
    ///
    /// The variables egret reads with `std::env::var` instead of through this
    /// struct ([`ENV_OUTSIDE_CONFIG`], [`ENV_OUTSIDE_CONFIG_PREFIXES`]) are not
    /// reported.
    pub fn unknown_keys() -> Vec<UnknownKey> {
        unknown_keys_in(&Self::figment(), std::env::vars().map(|(k, _)| k))
    }
}

/// `EGRET_*` variables egret reads directly with `std::env::var`, outside
/// [`Config`]. The one place that knows them, so the unknown-key warning
/// ([`Config::unknown_keys`]) does not flag them. Keep in step with the
/// "Variables outside the configuration file" table of the configuration-keys
/// reference page.
pub const ENV_OUTSIDE_CONFIG: &[&str] = &[
    // `egret init`, `egret workspace add-group` (egret-bin).
    "EGRET_ADMIN_EMAIL",
    "EGRET_ADMIN_PASSWORD",
    // Query connectors and the engine monitor (egret-query).
    "EGRET_ENGINE_TLS_INSECURE",
    // Test-corpus generator switch (egret-workflow).
    "EGRET_WRITE_PROPERTY_TREATMENT",
    // Frozen plan-layout fixture regenerator (egret-workflow).
    "EGRET_WRITE_PLAN_LAYOUT",
];

/// Prefixes of the test-harness variables (`EGRET_TEST_*`, `EGRET_E2E_*`,
/// `EGRET_ALLOW_*_SKIP`). `just` loads `.env` into every recipe, `just serve`
/// included, so a developer's `.env` carrying them must not warn either.
pub const ENV_OUTSIDE_CONFIG_PREFIXES: &[&str] = &["EGRET_TEST_", "EGRET_E2E_", "EGRET_ALLOW_"];

/// Where an [`UnknownKey`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownKeySource {
    /// An `EGRET_*` environment variable.
    Env,
    /// A key in a configuration file; the file as figment found it.
    File(String),
}

/// A configuration entry no field consumes. See [`Config::unknown_keys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKey {
    /// The variable (`EGRET_SERVER__PORTT`) or the dotted file key
    /// (`server.portt`), as the operator wrote it.
    pub name: String,
    pub source: UnknownKeySource,
    /// The closest real key, in the same spelling as `name`.
    pub suggestion: Option<String>,
}

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            UnknownKeySource::Env => write!(f, "ignored environment variable {}", self.name)?,
            UnknownKeySource::File(file) => write!(f, "ignored key {} in {file}", self.name)?,
        }
        f.write_str(": no configuration key has this name")?;
        if let Some(s) = &self.suggestion {
            write!(f, " (did you mean {s}?)")?;
        }
        Ok(())
    }
}

const ENV_PREFIX: &str = "EGRET_";
const ENV_SPLIT: &str = "__";

/// The figment key path of an `EGRET_*` variable, the way `Env::prefixed`
/// with `split("__")` derives it: prefix stripped, lowercased, split on `__`.
fn env_var_path(var: &str) -> Option<Vec<String>> {
    let rest = var.strip_prefix(ENV_PREFIX)?;
    Some(
        rest.split(ENV_SPLIT)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect(),
    )
}

fn env_var_name(path: &[String]) -> String {
    let upper: Vec<String> = path.iter().map(|s| s.to_ascii_uppercase()).collect();
    format!("{ENV_PREFIX}{}", upper.join(ENV_SPLIT))
}

/// The key segments of a `serde_ignored` path (`Some`/newtype hops carry no
/// key and are skipped).
fn ignored_segments(path: &serde_ignored::Path<'_>, out: &mut Vec<String>) {
    use serde_ignored::Path;
    match path {
        Path::Root => {}
        Path::Seq { parent, index } => {
            ignored_segments(parent, out);
            out.push(index.to_string());
        }
        Path::Map { parent, key } => {
            ignored_segments(parent, out);
            out.push(key.clone());
        }
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => ignored_segments(parent, out),
    }
}

/// Every leaf path under `value` (an empty table counts as a leaf).
fn leaf_paths(prefix: &[String], value: &figment::value::Value, out: &mut Vec<Vec<String>>) {
    match value {
        figment::value::Value::Dict(_, dict) if !dict.is_empty() => {
            for (k, v) in dict {
                let mut p = prefix.to_vec();
                p.push(k.clone());
                leaf_paths(&p, v, out);
            }
        }
        _ => out.push(prefix.to_vec()),
    }
}

/// Levenshtein distance, by chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(cur).min(row[j])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// The closest of `known` to `name`, if it is within two edits. Two catches
/// a dropped, doubled or swapped letter and the `_` for `__` slip, and is
/// tight enough that an unrelated name gets no hint at all.
fn closest<'a>(name: &str, known: impl Iterator<Item = &'a String>) -> Option<String> {
    known
        .map(|k| (edit_distance(name, k), k))
        .filter(|(d, _)| *d <= 2)
        .min_by(|(da, ka), (db, kb)| da.cmp(db).then(ka.cmp(kb)))
        .map(|(_, k)| k.clone())
}

fn unknown_keys_in(figment: &Figment, env_vars: impl Iterator<Item = String>) -> Vec<UnknownKey> {
    // The real `extract` reports a config that does not load; here a failure
    // only means there is nothing more to say.
    let Ok(root) = figment.extract::<figment::value::Value>() else {
        return vec![];
    };
    let mut ignored: Vec<Vec<String>> = Vec::new();
    let _: Result<Config, _> = serde_ignored::deserialize(&root, |p| {
        let mut segs = Vec::new();
        ignored_segments(&p, &mut segs);
        ignored.push(segs);
    });
    if ignored.is_empty() {
        return vec![];
    }

    let mut known: Vec<Vec<String>> = Vec::new();
    if let Ok(defaults) = figment::value::Value::serialize(Config::default()) {
        leaf_paths(&[], &defaults, &mut known);
    }
    let is_ignored = |path: &[String]| {
        ignored
            .iter()
            .any(|i| path.len() >= i.len() && path[..i.len()] == i[..])
    };

    let known_env: Vec<String> = known.iter().map(|k| env_var_name(k)).collect();
    let mut out = Vec::new();
    // Environment variables: named exactly as set, which a leaf walk of the
    // merged value could not recover (`EGRET_FOO` set to a table, say).
    let mut env_paths = Vec::new();
    for var in env_vars {
        let Some(path) = env_var_path(&var) else {
            continue;
        };
        if path.is_empty()
            || ENV_OUTSIDE_CONFIG.contains(&var.as_str())
            || ENV_OUTSIDE_CONFIG_PREFIXES
                .iter()
                .any(|p| var.starts_with(p))
            || !is_ignored(&path)
        {
            continue;
        }
        out.push(UnknownKey {
            suggestion: closest(&var, known_env.iter()),
            name: var,
            source: UnknownKeySource::Env,
        });
        env_paths.push(path);
    }
    // File keys: every ignored leaf that no environment variable supplied.
    let known_dotted: Vec<String> = known.iter().map(|k| k.join(".")).collect();
    for segs in &ignored {
        let Some(value) = root.find_ref(&segs.join(".")) else {
            continue;
        };
        let mut leaves = Vec::new();
        leaf_paths(segs, value, &mut leaves);
        for leaf in leaves {
            let dotted = leaf.join(".");
            let Some(md) = figment.find_metadata(&dotted) else {
                continue;
            };
            if env_paths.contains(&leaf) || md.name.contains("environment") {
                continue;
            }
            let file = md
                .source
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| md.name.to_string());
            out.push(UnknownKey {
                suggestion: closest(&dotted, known_dotted.iter()),
                name: dotted,
                source: UnknownKeySource::File(file),
            });
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    /// The repository's `.env.example`, read at compile time so the test sees
    /// exactly the file a new contributor copies (KI-136).
    const ENV_EXAMPLE: &str = include_str!("../../../.env.example");

    /// `KEY=value` pairs of a dotenv file: blank lines and `#` comments are
    /// skipped, surrounding quotes are stripped. Enough for `.env.example`,
    /// which `just` loads with `set dotenv-load`.
    fn dotenv_pairs(text: &str) -> Vec<(String, String)> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (k, v) = l
                    .split_once('=')
                    .unwrap_or_else(|| panic!("not KEY=value: {l}"));
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(v);
                (k.trim().to_string(), v.to_string())
            })
            .collect()
    }

    /// KI-136: `cp .env.example .env; just up; just serve` must get past
    /// config validation. Loads the file through the real loader (`Env`
    /// provider, `__` split, value parsing) in a jail whose environment holds
    /// ONLY the file's variables, then runs the boot check `main.rs` runs.
    #[test]
    fn env_example_loads_and_validates() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            for (k, v) in dotenv_pairs(ENV_EXAMPLE) {
                jail.set_env(k, v);
            }
            let cfg = Config::load()?;
            cfg.validate()
                .map_err(|e| format!(".env.example does not pass Config::validate: {e}"))?;
            Ok(())
        });
    }

    /// KI-136 guard: the storage credentials in `.env.example` are the ones
    /// the dev compose's RustFS really starts with, so a copied file can
    /// reach the bucket and not only pass validation.
    #[test]
    fn env_example_storage_matches_the_dev_compose() {
        const COMPOSE: &str = include_str!("../../../deploy/docker-compose.dev.yml");
        let env = dotenv_pairs(ENV_EXAMPLE);
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("{k} missing from .env.example"))
        };
        assert!(
            COMPOSE.contains(&format!(
                "RUSTFS_ACCESS_KEY: {}",
                get("EGRET_STORAGE__ACCESS_KEY")
            )),
            "EGRET_STORAGE__ACCESS_KEY must equal the compose RUSTFS_ACCESS_KEY"
        );
        assert!(
            COMPOSE.contains(&format!(
                "RUSTFS_SECRET_KEY: {}",
                get("EGRET_STORAGE__SECRET_KEY")
            )),
            "EGRET_STORAGE__SECRET_KEY must equal the compose RUSTFS_SECRET_KEY"
        );
        assert!(
            COMPOSE.contains("\"9000:9000\""),
            "the compose RustFS must publish 9000, the port EGRET_STORAGE__ENDPOINT names"
        );
        assert_eq!(get("EGRET_STORAGE__ENDPOINT"), "http://localhost:9000");
    }

    /// KI-141: with `catalog.public_url` unset, it follows `server.port`, and
    /// with it the machine-IdP issuer that falls back to it.
    #[test]
    fn catalog_public_url_follows_server_port_when_unset() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("EGRET_SERVER__PORT", "9090");
            let cfg = Config::load()?;
            assert_eq!(
                cfg.catalog.public_url.as_deref(),
                Some("http://localhost:9090")
            );
            assert_eq!(
                cfg.idp
                    .issuer_url(cfg.catalog.public_url.as_deref().unwrap_or_default()),
                "http://localhost:9090"
            );
            Ok(())
        });
    }

    /// KI-141: the default port still gives today's value.
    #[test]
    fn catalog_public_url_defaults_to_port_8080() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let cfg = Config::load()?;
            assert_eq!(
                cfg.catalog.public_url.as_deref(),
                Some("http://localhost:8080")
            );
            Ok(())
        });
    }

    /// KI-141: an explicit `catalog.public_url` is kept whatever the port.
    #[test]
    fn explicit_catalog_public_url_is_kept() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("EGRET_SERVER__PORT", "9090");
            jail.set_env("EGRET_CATALOG__PUBLIC_URL", "https://egret.example.com");
            let cfg = Config::load()?;
            assert_eq!(
                cfg.catalog.public_url.as_deref(),
                Some("https://egret.example.com")
            );
            Ok(())
        });
    }

    /// KI-140: a misspelled variable is reported, with a hint.
    #[test]
    fn a_misspelled_env_var_is_reported_with_a_hint() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("EGRET_SERVER__PORTT", "9090");
            let unknown = Config::unknown_keys();
            assert_eq!(
                unknown,
                vec![UnknownKey {
                    name: "EGRET_SERVER__PORTT".into(),
                    source: UnknownKeySource::Env,
                    suggestion: Some("EGRET_SERVER__PORT".into()),
                }]
            );
            assert_eq!(
                unknown[0].to_string(),
                "ignored environment variable EGRET_SERVER__PORTT: no configuration key \
                 has this name (did you mean EGRET_SERVER__PORT?)"
            );
            Ok(())
        });
    }

    /// KI-140: the single-underscore slip (`EGRET_DATABASE_URL`) lands at the
    /// top level, where it was ignored silently too.
    #[test]
    fn a_single_underscore_env_var_is_reported() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("EGRET_DATABASE_URL", "postgres://x@db/x");
            let unknown = Config::unknown_keys();
            assert_eq!(unknown.len(), 1, "{unknown:?}");
            assert_eq!(unknown[0].name, "EGRET_DATABASE_URL");
            assert_eq!(
                unknown[0].suggestion.as_deref(),
                Some("EGRET_DATABASE__URL")
            );
            Ok(())
        });
    }

    /// KI-140: an unknown key in `egret.toml`, a whole unknown section, and a
    /// name too far from any key (no hint).
    #[test]
    fn unknown_file_keys_are_reported() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("egret.toml", "[server]\nportt = 1\n\n[nonsense]\nx = 1\n")?;
            let mut unknown = Config::unknown_keys();
            unknown.sort_by(|a, b| a.name.cmp(&b.name));
            let got: Vec<_> = unknown
                .iter()
                .map(|k| (k.name.as_str(), k.suggestion.as_deref()))
                .collect();
            assert_eq!(
                got,
                vec![("nonsense.x", None), ("server.portt", Some("server.port"))]
            );
            for k in &unknown {
                assert!(
                    matches!(&k.source, UnknownKeySource::File(p) if p.ends_with("egret.toml")),
                    "{k:?}"
                );
            }
            assert!(
                unknown[1].to_string().starts_with("ignored key server.portt in ")
                    && unknown[1].to_string().ends_with(
                        "egret.toml: no configuration key has this name (did you mean server.port?)"
                    ),
                "{}",
                unknown[1]
            );
            Ok(())
        });
    }

    /// KI-140: a correct set stays silent — `.env.example` itself, plus the
    /// variables egret reads outside the config struct.
    #[test]
    fn a_correct_set_and_the_outside_variables_stay_silent() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            for (k, v) in dotenv_pairs(ENV_EXAMPLE) {
                jail.set_env(k, v);
            }
            for k in ENV_OUTSIDE_CONFIG {
                jail.set_env(k, "1");
            }
            for p in ENV_OUTSIDE_CONFIG_PREFIXES {
                jail.set_env(format!("{p}SOMETHING"), "1");
            }
            jail.set_env("EGRET_IDP__ISSUER", "https://idp.example.com");
            assert_eq!(Config::unknown_keys(), vec![]);
            Ok(())
        });
    }

    /// KI-140 guard: every `EGRET_*` variable production code reads with
    /// `std::env::var`/`var_os` by literal name is allow-listed in
    /// [`ENV_OUTSIDE_CONFIG`] (or under a harness prefix), so a new direct
    /// read cannot start warning on every boot unnoticed.
    #[test]
    fn every_direct_env_read_is_allow_listed() {
        fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    rs_files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for c in std::fs::read_dir(&crates).unwrap().flatten() {
            let src = c.path().join("src");
            if src.is_dir() {
                rs_files(&src, &mut files);
            }
        }
        let mut found = Vec::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap();
            for needle in ["env::var(\"EGRET_", "env::var_os(\"EGRET_"] {
                for (i, _) in text.match_indices(needle) {
                    let rest = &text[i + needle.len() - "EGRET_".len()..];
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    found.push((name, f.display().to_string()));
                }
            }
        }
        assert!(
            found.iter().any(|(n, _)| n == "EGRET_ADMIN_EMAIL"),
            "the scan must see the known reads: {found:?}"
        );
        for (name, file) in found {
            assert!(
                ENV_OUTSIDE_CONFIG.contains(&name.as_str())
                    || ENV_OUTSIDE_CONFIG_PREFIXES
                        .iter()
                        .any(|p| name.starts_with(p)),
                "{name} (read in {file}) is not in ENV_OUTSIDE_CONFIG: the unknown-key \
                 warning would flag it on every start"
            );
        }
    }

    #[test]
    fn apps_config_defaults() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.apps,
                AppsConfig {
                    max_rows: 10_000,
                    timeout_secs: 60,
                    session_idle_secs: 1_800,
                    session_absolute_secs: 28_800,
                    rate_max: 20,
                    rate_window_secs: 2,
                    origin: String::new(),
                    ticket_ttl_secs: 30,
                    egret_origin: String::new(),
                    cookie_ttl_secs: 28_800,
                    listen: "0.0.0.0:8081".into(),
                    pod: AppPodConfig::default(),
                    storage: AppsStorageConfig::default(),
                }
            );
            Ok(())
        });
    }

    #[test]
    fn apps_config_env_overrides() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_APPS__MAX_ROWS", "5");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.apps.max_rows, 5);
            assert_eq!(cfg.apps.rate_max, 20, "unset keys keep their default");
            Ok(())
        });
    }

    /// W5a: no plugin configuration by default -- which allows no image.
    #[test]
    fn plugin_configurations_default_to_none() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow.plugin_configurations,
                PluginConfigurations::default()
            );
            assert_eq!(
                cfg.workflow
                    .plugin_configurations
                    .allowed_images("io.kestra.plugin.scripts.runner.docker.Docker"),
                None
            );
            Ok(())
        });
    }

    /// W5a: the allowlist is reachable from the environment and from the file,
    /// in Kestra's `{type, values}` shape with the `allowed-images` key.
    #[test]
    fn plugin_configurations_are_settable_from_env_and_file() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                "EGRET_WORKFLOW__PLUGIN_CONFIGURATIONS",
                r#"[{type="io.kestra.plugin.scripts.runner.docker.Docker",values={allowed-images=["python:*","ubuntu:latest"]}}]"#,
            );
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow
                    .plugin_configurations
                    .allowed_images("io.kestra.plugin.scripts.runner.docker.Docker"),
                Some(vec!["python:*", "ubuntu:latest"])
            );
            assert_eq!(
                cfg.workflow
                    .plugin_configurations
                    .allowed_images("io.kestra.plugin.ee.kubernetes.runner.Kubernetes"),
                None
            );
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "egret.toml",
                r#"
[[workflow.plugin_configurations]]
type = "io.kestra.plugin.ee.kubernetes.runner.Kubernetes"
values = { allowed-images = [] }
"#,
            )?;
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow
                    .plugin_configurations
                    .allowed_images("io.kestra.plugin.ee.kubernetes.runner.Kubernetes"),
                Some(vec![])
            );
            Ok(())
        });
    }

    /// W5b: `allowed-types` on the `kestra` entry is reachable from the
    /// environment; with no `kestra` entry it is `None` (every catalog type).
    #[test]
    fn allowed_types_is_settable_from_env() {
        figment::Jail::expect_with(|jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.workflow.plugin_configurations.allowed_types(), None);
            jail.set_env(
                "EGRET_WORKFLOW__PLUGIN_CONFIGURATIONS",
                r#"[{type="kestra",values={allowed-types=["io.kestra.plugin.jdbc.*"]}}]"#,
            );
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow.plugin_configurations.allowed_types(),
                Some(vec!["io.kestra.plugin.jdbc.*"])
            );
            cfg.workflow.validate_plugin_configurations().unwrap();
            Ok(())
        });
    }

    /// W5b: `workflow.plugin_host` has the spec's defaults (`384m`, 15 s, no
    /// image), is settable from the environment, and the boot check refuses a
    /// runner without an image and a heap that is not a JVM size.
    #[test]
    fn plugin_host_defaults_env_and_boot_check() {
        figment::Jail::expect_with(|jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow.plugin_host,
                PluginHostConfig {
                    image: None,
                    jvm_max_heap: "384m".into(),
                    kill_grace_s: 15,
                }
            );
            // `none` needs no image.
            cfg.workflow.validate_plugin_host().unwrap();

            jail.set_env("EGRET_WORKFLOW__CONTAINER_RUNNER", "docker");
            let cfg: Config = Config::figment().extract()?;
            let e = cfg.workflow.validate_plugin_host().unwrap_err();
            assert!(e.contains("workflow.plugin_host.image is required"), "{e}");

            jail.set_env(
                "EGRET_WORKFLOW__PLUGIN_HOST__IMAGE",
                "egret-plugin-host:dev",
            );
            jail.set_env("EGRET_WORKFLOW__PLUGIN_HOST__KILL_GRACE_S", "20");
            jail.set_env("EGRET_WORKFLOW__PLUGIN_HOST__JVM_MAX_HEAP", "1g");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow.plugin_host.image.as_deref(),
                Some("egret-plugin-host:dev")
            );
            assert_eq!(cfg.workflow.plugin_host.kill_grace_s, 20);
            cfg.workflow.validate_plugin_host().unwrap();

            for grace in [0, 1, 2] {
                let mut w = cfg.workflow.clone();
                w.plugin_host.kill_grace_s = grace;
                let e = w.validate_plugin_host().unwrap_err();
                assert!(
                    e.contains("workflow.plugin_host.kill_grace_s") && e.contains("minimum 3"),
                    "{grace}: {e}"
                );
            }
            let mut w = cfg.workflow.clone();
            w.plugin_host.kill_grace_s = 3;
            w.validate_plugin_host().expect("3 is the minimum");
            for bad in ["384m; rm -rf /", "", "m", "12mm", "1.5g", "$(id)"] {
                let mut w = cfg.workflow.clone();
                w.plugin_host.jvm_max_heap = bad.into();
                let e = w.validate_plugin_host().unwrap_err();
                assert!(e.contains("jvm_max_heap"), "{bad}: {e}");
            }
            Ok(())
        });
    }

    /// W9a Task 3: `[workflow.storage]` and `storage_retention_days` have the
    /// documented defaults on BOTH paths (the figment defaults layer and the
    /// serde fallback), and every key is settable from the environment.
    #[test]
    fn workflow_storage_defaults_and_env() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            let want = WorkflowStorageConfig {
                prefix: "workflow/".into(),
                max_file_bytes: 1_073_741_824,
                max_task_bytes: 5_368_709_120,
                max_read_bytes: 10_485_760,
                max_namespace_files: 10_000,
            };
            assert_eq!(cfg.workflow.storage, want);
            assert_eq!(cfg.workflow.storage_retention_days, 30);
            assert_eq!(default_workflow().storage, want);
            assert_eq!(default_workflow().storage_retention_days, 30);
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__STORAGE__PREFIX", "wf/");
            jail.set_env("EGRET_WORKFLOW__STORAGE__MAX_FILE_BYTES", "11");
            jail.set_env("EGRET_WORKFLOW__STORAGE__MAX_TASK_BYTES", "22");
            jail.set_env("EGRET_WORKFLOW__STORAGE__MAX_READ_BYTES", "33");
            jail.set_env("EGRET_WORKFLOW__STORAGE__MAX_NAMESPACE_FILES", "44");
            jail.set_env("EGRET_WORKFLOW__STORAGE_RETENTION_DAYS", "0");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.workflow.storage,
                WorkflowStorageConfig {
                    prefix: "wf/".into(),
                    max_file_bytes: 11,
                    max_task_bytes: 22,
                    max_read_bytes: 33,
                    max_namespace_files: 44,
                }
            );
            assert_eq!(cfg.workflow.storage_retention_days, 0);
            Ok(())
        });
    }

    /// Debt D3: `kept_container_retention_days` defaults to 7 on BOTH paths
    /// (the figment defaults layer and the serde fallback), and the
    /// environment sets it, `0` included.
    #[test]
    fn workflow_kept_container_retention_default_and_env() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.workflow.kept_container_retention_days, 7);
            assert_eq!(default_workflow().kept_container_retention_days, 7);
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS", "0");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.workflow.kept_container_retention_days, 0);
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS", "3");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.workflow.kept_container_retention_days, 3);
            Ok(())
        });
    }

    /// W9a fix round 1 M4: `[workflow.storage]` is checked at boot, each
    /// refusal naming its key; the default passes (the control).
    ///
    /// Final review I4: `[storage]` (the bucket/access_key/secret_key, not
    /// `[workflow.storage]`'s prefix/size limits) is now required
    /// unconditionally, so the control config needs one set to isolate what
    /// this test is actually about.
    #[test]
    fn workflow_storage_is_validated_at_boot() {
        let check = |f: &dyn Fn(&mut WorkflowStorageConfig)| {
            let mut cfg = Config::default();
            cfg.storage.bucket = "b".into();
            cfg.storage.access_key = "a".into();
            cfg.storage.secret_key = "s".into();
            f(&mut cfg.workflow.storage);
            cfg.validate()
        };
        assert_eq!(check(&|_| {}), Ok(()));
        assert_eq!(check(&|s| s.prefix = "wf/nested/".into()), Ok(()));
        for bad in ["", "workflow", "/workflow/", "/"] {
            let err = check(&|s| s.prefix = bad.into()).expect_err(bad);
            assert!(err.contains("workflow.storage.prefix"), "{bad:?}: {err}");
        }
        // W9b Task 3 (R3 of Task 2's review): 200 bytes pass, 201 do not --
        // counted in bytes ('é' is two), so a rule counting characters fails.
        let at = format!("{}/", "é".repeat(99) + "a"); // 198 + 1 + 1 = 200 bytes
        assert_eq!(at.len(), 200);
        assert_eq!(check(&|s| s.prefix = at.clone()), Ok(()));
        let over = format!("a{at}");
        assert_eq!((over.len(), over.chars().count()), (201, 102));
        let err = check(&|s| s.prefix = over.clone()).unwrap_err();
        assert!(
            err.contains("workflow.storage.prefix")
                && err.contains("201 bytes")
                && err.contains("at most 200"),
            "{err}"
        );
        let err = check(&|s| s.max_file_bytes = 0).unwrap_err();
        assert!(err.contains("workflow.storage.max_file_bytes"), "{err}");
        let err = check(&|s| s.max_task_bytes = 0).unwrap_err();
        assert!(err.contains("workflow.storage.max_task_bytes"), "{err}");
        let err = check(&|s| s.max_read_bytes = 0).unwrap_err();
        assert!(err.contains("workflow.storage.max_read_bytes"), "{err}");
        // W9c Task 2 review I2: at least 4 KiB, and 4 KiB passes.
        for small in [1, 4095] {
            let err = check(&|s| s.max_read_bytes = small).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "workflow.storage.max_read_bytes is {small} bytes; it must be at least \
                     4096 bytes (4 KiB), the least the Ion reader of a KV value or a preview \
                     reads a small value under (EGRET_WORKFLOW__STORAGE__MAX_READ_BYTES)"
                )
            );
        }
        assert_eq!(check(&|s| s.max_read_bytes = 4096), Ok(()));
        let err = check(&|s| s.max_namespace_files = 0).unwrap_err();
        assert!(
            err.contains("workflow.storage.max_namespace_files"),
            "{err}"
        );
        assert_eq!(check(&|s| s.max_namespace_files = 1), Ok(()));
    }

    /// W9a spec §15: a container runner needs `[storage]` at boot; `none`
    /// adds no requirement of its own. Each missing key is refused on its
    /// own, and the control (all three set) passes.
    ///
    /// Final review I4: `[storage]` is now ALSO required unconditionally for
    /// the always-mounted apps API, independent of `container_runner`. So
    /// `none` no longer validates with empty storage either -- but for that
    /// separate reason, not because `container_runner` itself started
    /// requiring it. The two Docker/Kubernetes refusal loops below still
    /// isolate the container-runner-specific message.
    #[test]
    fn a_container_runner_requires_storage_at_boot() {
        let with = |runner: ContainerRunner, bucket: &str, ak: &str, sk: &str| {
            let mut cfg = Config::default();
            cfg.workflow.container_runner = runner;
            cfg.storage.bucket = bucket.into();
            cfg.storage.access_key = ak.into();
            cfg.storage.secret_key = sk.into();
            cfg.validate()
        };
        for runner in [ContainerRunner::Docker, ContainerRunner::Kubernetes] {
            assert_eq!(
                with(runner, "b", "a", "s"),
                Ok(()),
                "{runner:?} with storage"
            );
            for (b, a, k) in [("", "a", "s"), ("b", "", "s"), ("b", "a", "")] {
                let err = with(runner, b, a, k).expect_err("refused");
                assert!(err.contains("[storage]"), "{err}");
            }
        }
        assert_eq!(
            with(ContainerRunner::None, "b", "a", "s"),
            Ok(()),
            "none adds no requirement of its own, given storage is set for the apps API"
        );
        let err = with(ContainerRunner::None, "", "", "").unwrap_err();
        assert!(
            err.contains("[storage]"),
            "now refused by the apps API's unconditional check, not container_runner: {err}"
        );
    }

    /// W5a Task 5: `container_runner` is `none` unless a deployment says
    /// otherwise, and `[workflow.kubernetes]` has the documented defaults.
    #[test]
    fn container_runner_defaults_to_none_and_kubernetes_to_its_documented_values() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.workflow.container_runner, ContainerRunner::None);
            let k = &cfg.workflow.kubernetes;
            assert_eq!(k.namespace, "egret-tasks");
            assert_eq!(k.run_as_user, 1000);
            assert_eq!(k.helper_image, "egret-task-helper:dev");
            assert!(k.image_pull_secrets.is_empty());
            assert_eq!(k.default_resources.request.cpu.as_deref(), Some("100m"));
            assert_eq!(k.default_resources.limit.memory.as_deref(), Some("512Mi"));
            assert_eq!(k.max_resources.limit.cpu.as_deref(), Some("2"));
            assert_eq!(k.max_resources.request, TaskResourceAmounts::default());
            // Final review I5: the node's disk is bounded by default.
            assert_eq!(
                k.default_resources.limit.ephemeral_storage.as_deref(),
                Some("10Gi")
            );
            assert_eq!(
                k.default_resources.request.ephemeral_storage.as_deref(),
                Some("256Mi")
            );
            assert_eq!(
                k.max_resources.limit.ephemeral_storage.as_deref(),
                Some("10Gi")
            );
            // The serde fallback for a `[workflow]` section that predates the
            // keys agrees with the defaults layer.
            assert_eq!(default_workflow().container_runner, ContainerRunner::None);
            assert_eq!(
                default_workflow().kubernetes,
                WorkflowKubernetesConfig::default()
            );
            Ok(())
        });
    }

    /// W5a Task 5: every `container_runner` spelling and every
    /// `[workflow.kubernetes]` key is reachable from the environment.
    #[test]
    fn container_runner_and_kubernetes_are_settable_from_env() {
        for (raw, want) in [
            ("none", ContainerRunner::None),
            ("docker", ContainerRunner::Docker),
            ("kubernetes", ContainerRunner::Kubernetes),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("EGRET_WORKFLOW__CONTAINER_RUNNER", raw);
                let cfg: Config = Config::figment().extract()?;
                assert_eq!(cfg.workflow.container_runner, want, "{raw}");
                Ok(())
            });
        }
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__KUBERNETES__NAMESPACE", "tasks-x");
            jail.set_env("EGRET_WORKFLOW__KUBERNETES__RUN_AS_USER", "4242");
            jail.set_env(
                "EGRET_WORKFLOW__KUBERNETES__HELPER_IMAGE",
                "reg.io/helper:1",
            );
            jail.set_env(
                "EGRET_WORKFLOW__KUBERNETES__IMAGE_PULL_SECRETS",
                r#"["regcred"]"#,
            );
            jail.set_env(
                "EGRET_WORKFLOW__KUBERNETES__MAX_RESOURCES__LIMIT__MEMORY",
                "1Gi",
            );
            jail.set_env(
                "EGRET_WORKFLOW__KUBERNETES__DEFAULT_RESOURCES__REQUEST__CPU",
                "250m",
            );
            jail.set_env(
                "EGRET_WORKFLOW__KUBERNETES__MAX_RESOURCES__LIMIT__EPHEMERAL_STORAGE",
                "4Gi",
            );
            let cfg: Config = Config::figment().extract()?;
            let k = &cfg.workflow.kubernetes;
            assert_eq!(k.namespace, "tasks-x");
            assert_eq!(k.run_as_user, 4242);
            assert_eq!(k.helper_image, "reg.io/helper:1");
            assert_eq!(k.image_pull_secrets, vec!["regcred".to_string()]);
            assert_eq!(k.max_resources.limit.memory.as_deref(), Some("1Gi"));
            assert_eq!(k.default_resources.request.cpu.as_deref(), Some("250m"));
            assert_eq!(
                k.max_resources.limit.ephemeral_storage.as_deref(),
                Some("4Gi")
            );
            Ok(())
        });
    }

    /// W5a Task 5: the chart carries the lever with `none` as its default, and
    /// the task namespace's Role is exactly the RBAC the spec allows --
    /// `pods` get/list/watch/create/delete, `pods/log` get, `pods/exec`
    /// get/create -- nothing more. (`get` on `pods/exec` because WebSocket
    /// exec on Kubernetes >= 1.30 is authorised as `get`; measured on kind
    /// 1.33 in W5a Task 6's e2e.) (`helm template` is the full check; this pins
    /// the text so an added verb or resource fails a unit test.)
    #[test]
    fn the_chart_carries_the_container_runner_and_the_exact_task_rbac() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/egret");
        let read = |p: &str| {
            std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("cannot read {p}: {e}"))
        };
        assert!(read("templates/configmap-env.yaml").contains(
            r#"EGRET_WORKFLOW__CONTAINER_RUNNER: "{{ .Values.workflow.containerRunner }}""#
        ));
        assert!(read("values.yaml").contains("  containerRunner: none\n"));
        // W5b: the plugin host's three settings, under the runner's block. The image is never
        // rendered empty: unset, it is the published image at the chart's appVersion.
        let env = read("templates/configmap-env.yaml");
        for key in [
            r#"EGRET_WORKFLOW__PLUGIN_HOST__IMAGE: {{ $h.image | default (include "egret.image" (dict "name" "egret-plugin-host" "root" $)) | quote }}"#,
            "EGRET_WORKFLOW__PLUGIN_HOST__JVM_MAX_HEAP: {{ $h.jvmMaxHeap | quote }}",
            "EGRET_WORKFLOW__PLUGIN_HOST__KILL_GRACE_S: {{ $h.killGraceS | quote }}",
        ] {
            assert!(env.contains(key), "{key}");
        }
        let values = read("values.yaml");
        assert!(values.contains("    jvmMaxHeap: 384m\n"));
        assert!(values.contains("    killGraceS: 15\n"));
        let rbac = read("templates/rbac-tasks.yaml");
        let rules: Vec<&str> = rbac
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("resources:") || l.starts_with("verbs:"))
            .collect();
        assert_eq!(
            rules,
            vec![
                r#"resources: ["pods"]"#,
                r#"verbs: ["get","list","watch","create","delete"]"#,
                r#"resources: ["pods/log"]"#,
                r#"verbs: ["get"]"#,
                r#"resources: ["pods/exec"]"#,
                r#"verbs: ["get","create"]"#,
            ]
        );
        assert_eq!(rbac.matches("apiGroups:").count(), 3);
        assert!(!rbac.contains("ClusterRole"), "only in the task namespace");
    }

    /// An unknown runner is refused at boot, by name, not read as `none`.
    #[test]
    fn an_unknown_container_runner_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__CONTAINER_RUNNER", "process");
            let err = Config::figment().extract::<Config>().unwrap_err();
            assert!(err.to_string().contains("process"), "{err}");
            Ok(())
        });
    }

    /// A value egret does not read is refused at boot, by name, rather than
    /// accepted and ignored.
    #[test]
    fn an_unknown_plugin_configuration_value_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                "EGRET_WORKFLOW__PLUGIN_CONFIGURATIONS",
                r#"[{type="io.kestra.plugin.scripts.runner.docker.Docker",values={volume-enabled=true}}]"#,
            );
            let err = Config::figment().extract::<Config>().unwrap_err();
            assert!(err.to_string().contains("volume-enabled"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn defaults_load_without_any_env() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 8080);
            assert_eq!(cfg.server.host, "0.0.0.0");
            assert_eq!(cfg.database.max_connections, 10);
            Ok(())
        });
    }

    /// A fresh deployment must prune `dataapi.access_log`, and must not prune it
    /// to nothing.
    ///
    /// The two halves are separate assertions on purpose. A default of 0 would
    /// deserialise fine, read as "off" through `scheduled_retention`, and leave
    /// the fastest-growing table in the system unbounded on every deployment
    /// that never set the key — which is every deployment today, since the
    /// `[data_api]` section is new.
    #[test]
    fn the_access_log_retention_default_is_positive() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert!(
                cfg.data_api.access_log_retention_days > 0,
                "a fresh deployment must prune the access log, got {}",
                cfg.data_api.access_log_retention_days
            );
            assert_eq!(
                cfg.data_api.access_log_retention_days, cfg.audit.retention_days,
                "the two retention knobs should not disagree by accident"
            );
            Ok(())
        });
    }

    /// The key is reachable from the environment under the name the operator
    /// docs will name. A section whose env path is wrong is a knob that silently
    /// cannot be turned.
    #[test]
    fn the_access_log_retention_is_settable_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_DATA_API__ACCESS_LOG_RETENTION_DAYS", "7");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.data_api.access_log_retention_days, 7);
            Ok(())
        });
    }

    /// Absent, the consumer-facing origin IS the issuer — this key is a
    /// widening, so an existing deployment's generated document must not move.
    #[test]
    fn the_consumer_base_url_falls_back_to_the_issuer() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.data_api.public_url, None);
            assert_eq!(
                cfg.data_api
                    .consumer_base_url("http://egret.egret-system.svc.cluster.local:8080"),
                "http://egret.egret-system.svc.cluster.local:8080",
                "unset must leave every advertised URL exactly where it was"
            );
            Ok(())
        });
    }

    /// Set, it wins — and it is reachable under the name the operator docs and
    /// the chart will use. A section whose env path is wrong is a knob that
    /// silently cannot be turned.
    #[test]
    fn the_consumer_base_url_is_settable_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_DATA_API__PUBLIC_URL", "http://egret.localtest.me/");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.data_api.public_url.as_deref(),
                Some("http://egret.localtest.me/")
            );
            // Trimmed AT THE SOURCE, like the issuer's own derivation: the
            // trailing slash never reaches a call site to be forgotten there.
            assert_eq!(
                cfg.data_api.consumer_base_url("http://egret.svc:8080"),
                "http://egret.localtest.me"
            );
            Ok(())
        });
    }

    /// The replica count reaches the process from the environment, under the
    /// name the CHART writes — and the chart is asserted to write it.
    ///
    /// This is the one knob whose failure mode is silence in the safe-looking
    /// direction. `default_replicas()` is `1`, and `1` is the value that lets
    /// `choose_store` through; so a mis-spelled section, a renamed field or a
    /// changed env separator does not error — it reads as a single-replica
    /// deployment, and a four-replica deployment boots with four private
    /// limiters and four private caches, which is exactly the defect Package 4
    /// exists to remove. Nothing else in the tree fails when that mapping
    /// breaks, so it is asserted here.
    ///
    /// Both halves in ONE test on purpose. The chart's key and the config's
    /// path are only correct together, and two tests that each assert their own
    /// half agree with a literal string would both stay green while the pair
    /// disagreed.
    #[test]
    fn the_replica_count_arrives_under_the_name_the_chart_writes() {
        const KEY: &str = "EGRET_DATA_API__REPLICAS";
        let chart = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/helm/egret/templates/configmap-env.yaml"
        ))
        .expect("reading configmap-env.yaml");
        let line = chart
            .lines()
            .find(|l| l.trim_start().starts_with(KEY))
            .unwrap_or_else(|| panic!("the chart no longer writes {KEY}"));
        assert!(
            line.contains(".Values.backend.replicas"),
            "{KEY} must be rendered from the SAME value the Deployment's \
             spec.replicas uses, or the two can disagree: {line}"
        );

        figment::Jail::expect_with(|jail| {
            jail.set_env(KEY, "4");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.data_api.replicas, 4,
                "{KEY} is what the chart sets; if it does not land on \
                 data_api.replicas the multi-replica refusal never fires"
            );
            Ok(())
        });
    }

    /// Absent, it reads as one replica — stated as a test because it is a
    /// DEFAULT THAT DISARMS A GUARD, not a neutral fallback.
    ///
    /// It is the right default (a deployment that never heard of the key runs
    /// one process, and erroring instead would fail every deployment that
    /// predates it), and it is also the reason the test above exists.
    #[test]
    fn an_absent_replica_count_reads_as_one_and_therefore_disarms_the_guard() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.data_api.replicas, 1);
            assert_eq!(cfg.data_api.shared_store_url, None);
            Ok(())
        });
    }

    /// And the store URL under the name the chart writes for it.
    ///
    /// Same seam, milder failure: a URL that did not arrive is a process that
    /// refuses to start (above one replica) or runs process-local (at one),
    /// neither of which is silent.
    #[test]
    fn the_shared_store_url_arrives_under_the_name_the_chart_writes() {
        const KEY: &str = "EGRET_DATA_API__SHARED_STORE_URL";
        let chart = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/helm/egret/templates/configmap-env.yaml"
        ))
        .expect("reading configmap-env.yaml");
        assert!(
            chart.lines().any(|l| l.trim_start().starts_with(KEY)),
            "the chart no longer writes {KEY}"
        );
        figment::Jail::expect_with(|jail| {
            jail.set_env(KEY, "redis://valkey:6379/1");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.data_api.shared_store_url.as_deref(),
                Some("redis://valkey:6379/1")
            );
            Ok(())
        });
    }

    /// Blank is absent, not an empty origin.
    ///
    /// `EGRET_DATA_API__PUBLIC_URL=` is what a half-written env file produces,
    /// and it deserialises to `Some("")`. Taken literally it would render
    /// `servers[0].url` as a bare `/data-api/v1/<slug>` — a document that looks
    /// generated and is uncallable.
    #[test]
    fn a_blank_consumer_base_url_reads_as_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_DATA_API__PUBLIC_URL", "");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.data_api.consumer_base_url("http://egret.svc:8080"),
                "http://egret.svc:8080"
            );
            Ok(())
        });
    }

    /// A value of `/` must read as unset too, for the same reason `""` does.
    ///
    /// It survives an emptiness check that runs before the slash is trimmed,
    /// and then trims to nothing — so the guard reads as present while the
    /// origin is empty, and `servers[0].url` renders as the bare path
    /// `/data-api/v1/<slug>`. That is the exact "looks generated, is
    /// uncallable" document the blank case exists to prevent, reached by the
    /// one input that walks past its filter.
    #[test]
    fn a_slash_only_consumer_base_url_reads_as_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_DATA_API__PUBLIC_URL", "/");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.data_api.consumer_base_url("http://egret.svc:8080"),
                "http://egret.svc:8080"
            );
            Ok(())
        });
    }

    /// Absent, egret's identity IS the in-cluster address it has always been.
    /// This key is a widening: an existing deployment's `iss`, its discovery
    /// document and every StarRocks `required_issuer` already provisioned must
    /// not move a byte.
    #[test]
    fn the_idp_issuer_falls_back_to_the_in_cluster_public_url() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.idp.issuer, None);
            assert_eq!(
                cfg.idp
                    .issuer_url("http://egret.egret-system.svc.cluster.local:8080"),
                "http://egret.egret-system.svc.cluster.local:8080",
                "unset must leave every `iss` exactly where it was"
            );
            Ok(())
        });
    }

    /// Set, it wins — under the name the chart and the operator docs use. A
    /// section whose env path is wrong is a knob that silently cannot be turned,
    /// and this one is the whole point of the key.
    #[test]
    fn the_idp_issuer_is_settable_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_IDP__ISSUER", "http://egret.localtest.me/");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.idp.issuer.as_deref(),
                Some("http://egret.localtest.me/")
            );
            // Trimmed AT THE SOURCE: StarRocks byte-compares `required_issuer`,
            // so a trailing slash here would break StarRocks JWT auth while
            // Trino (which never reads the issuer) kept working.
            assert_eq!(
                cfg.idp.issuer_url("http://egret.svc:8080"),
                "http://egret.localtest.me"
            );
            Ok(())
        });
    }

    /// Blank is absent, not an empty issuer. `EGRET_IDP__ISSUER=` in a
    /// half-written env file deserialises to `Some("")`; taken literally it
    /// would mint tokens whose `iss` is `""` and advertise a discovery document
    /// with an empty issuer — rejected by every conforming client, and by
    /// StarRocks' byte comparison.
    #[test]
    fn a_blank_idp_issuer_reads_as_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_IDP__ISSUER", "");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.idp.issuer_url("http://egret.svc:8080"),
                "http://egret.svc:8080"
            );
            Ok(())
        });
    }

    /// `/` must read as unset too, for the same reason `""` does — it is the one
    /// input that walks past an emptiness check placed after the trim, leaving
    /// the guard reading as present while the issuer is empty.
    #[test]
    fn a_slash_only_idp_issuer_reads_as_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_IDP__ISSUER", "/");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.idp.issuer_url("http://egret.svc:8080"),
                "http://egret.svc:8080"
            );
            Ok(())
        });
    }

    #[test]
    fn env_overrides_nested_keys() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_SERVER__PORT", "9999");
            jail.set_env("EGRET_DATABASE__URL", "postgres://x/y");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 9999);
            assert_eq!(cfg.database.url, "postgres://x/y");
            Ok(())
        });
    }

    #[test]
    fn toml_file_is_merged() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("egret.toml", "[server]\nport = 7000\n")?;
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 7000);
            Ok(())
        });
    }

    #[test]
    fn session_and_csrf_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.session.absolute_ttl_hours, 24);
            assert_eq!(cfg.session.idle_ttl_minutes, 60);
            assert_eq!(cfg.csrf.header_name, "X-Egret-CSRF");
            Ok(())
        });
    }

    #[test]
    fn rate_limit_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.rate_limit.login_max_attempts, 5);
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_an_app_idle_window_longer_than_the_absolute_one() {
        // `app_sessions` has CHECK (idle_expires_at <= absolute_expires_at): the
        // reverse would pass boot and then fail every mint with a 500.
        let mut cfg = Config::default();
        // [storage] is required unconditionally (final review I4); set it so
        // the final assertion below reaches the session-window check's Ok arm
        // rather than the storage one.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        cfg.apps.session_idle_secs = cfg.apps.session_absolute_secs + 1;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("apps.session_idle_secs"), "{err}");
        cfg.apps.session_idle_secs = cfg.apps.session_absolute_secs;
        assert!(cfg.validate().is_ok(), "equal windows are allowed");
    }

    #[test]
    fn validate_refuses_a_kill_poll_below_the_minimum() {
        let mut cfg = Config::default();
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert_eq!(cfg.workflow.kill_poll_ms, 5000, "the default");
        cfg.workflow.kill_poll_ms = MIN_KILL_POLL_MS - 1;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("workflow.kill_poll_ms"), "{err}");
        cfg.workflow.kill_poll_ms = MIN_KILL_POLL_MS;
        assert!(cfg.validate().is_ok(), "the minimum itself is allowed");
    }

    #[test]
    fn validate_refuses_zero_concurrent_runs() {
        let mut cfg = Config::default();
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert_eq!(cfg.workflow.max_concurrent_runs, 8, "the default");
        cfg.workflow.max_concurrent_runs = 0;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("workflow.max_concurrent_runs"), "{err}");
        cfg.workflow.max_concurrent_runs = 1;
        assert!(cfg.validate().is_ok(), "one is allowed");
    }

    #[test]
    fn validate_requires_distinct_origins_without_paths_when_the_apps_origin_is_set() {
        let mut cfg = Config::default();
        // [storage] is required unconditionally (final review I4), independent
        // of apps.origin, so set it up front to isolate the origin checks.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(
            cfg.validate().is_ok(),
            "unset origin: apps iframe is simply off"
        );
        cfg.apps.origin = "https://apps.egret.example".into();
        assert!(cfg.validate().is_err(), "egret_origin missing");
        cfg.apps.egret_origin = "https://egret.example".into();
        assert!(cfg.validate().is_ok());
        cfg.apps.egret_origin = "https://apps.egret.example".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("must differ"), "{err}");
        cfg.apps.egret_origin = "https://egret.example/".into();
        assert!(cfg.validate().is_err(), "trailing slash is a path");
        cfg.apps.egret_origin = "egret.example".into();
        assert!(cfg.validate().is_err(), "no scheme");
        cfg.apps.egret_origin = "https://egret.example".into();
        cfg.apps.cookie_ttl_secs = 0;
        assert!(cfg.validate().is_err(), "a cookie that never lives");
    }

    #[test]
    fn validate_checks_the_pod_launcher_choice_and_what_each_needs() {
        let mut cfg = Config::default();
        // The apps API requires [storage] unconditionally (it is mounted
        // regardless of apps.pod.launcher), so it must be set before any of
        // the launcher-specific assertions below.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(
            cfg.validate().is_ok(),
            "launcher none is valid, given storage is configured"
        );
        cfg.apps.pod.launcher = "docker".into();
        assert!(cfg
            .validate()
            .unwrap_err()
            .contains("none, subprocess or kubernetes"));

        cfg.apps.pod.launcher = "kubernetes".into();
        assert!(cfg.validate().unwrap_err().contains("needs apps.origin"));
        cfg.apps.origin = "https://apps.egret.example".into();
        cfg.apps.egret_origin = "https://egret.example".into();
        assert!(cfg.validate().unwrap_err().contains("callback_url"));
        cfg.apps.pod.callback_url = "http://egret:8080".into();
        assert!(cfg.validate().is_ok());
        cfg.apps.pod.idle_secs = 10;
        assert!(cfg.validate().unwrap_err().contains("idle_secs"));
        cfg.apps.pod.idle_secs = 1_800;
        cfg.apps.pod.image = String::new();
        assert!(cfg.validate().unwrap_err().contains("image"));

        cfg.apps.pod.launcher = "subprocess".into();
        assert!(cfg.validate().unwrap_err().contains("command"));
        cfg.apps.pod.command = vec!["egret-apps".into()];
        cfg.apps.pod.config_dir = "apps-image/config".into();
        assert!(cfg.validate().unwrap_err().contains("venv_shim_dir"));
        cfg.apps.pod.venv_shim_dir = "apps-image/venv-shim".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_checks_run_pod_settings_d24() {
        let mut cfg = Config::default();
        cfg.apps.origin = "https://apps.egret.example".into();
        cfg.apps.egret_origin = "https://egret.example".into();
        cfg.apps.pod.launcher = "subprocess".into();
        cfg.apps.pod.command = vec!["egret-apps".into()];
        cfg.apps.pod.config_dir = "apps-image/config".into();
        cfg.apps.pod.venv_shim_dir = "apps-image/venv-shim".into();
        cfg.apps.pod.callback_url = "http://egret:8080".into();
        // Set storage to allow validation to proceed
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok(), "defaults (thread, cap 8) are valid");

        cfg.apps.pod.run_kernel_mode = "pool".into();
        assert!(cfg.validate().unwrap_err().contains("thread or forced_ipc"));
        cfg.apps.pod.run_kernel_mode = "forced_ipc".into();
        assert!(
            cfg.validate().is_ok(),
            "forced_ipc is the documented fallback, still accepted"
        );
        cfg.apps.pod.run_kernel_mode = "thread".into();

        cfg.apps.pod.run_session_cap = 0;
        assert!(cfg.validate().unwrap_err().contains("run_session_cap"));
    }

    #[test]
    fn validate_ok_when_oidc_disabled() {
        let mut cfg = Config::default();
        // [storage] is required unconditionally (final review I4), independent
        // of oidc.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_requires_secret_key_when_oidc_enabled() {
        let mut cfg = Config::default();
        cfg.oidc.enabled = true;
        cfg.oidc.issuer_url = "http://idp".into();
        cfg.oidc.client_id = "egret".into();
        assert!(cfg.validate().is_err()); // empty secret_key
    }

    #[test]
    fn validate_ok_for_dev_oidc_config() {
        // Mirrors the dev `.env` / `.env.example`: OIDC on against local Keycloak,
        // with a dev secret_key (base64 of 64 random bytes). Must match .env.example.
        let mut cfg = Config::default();
        cfg.oidc.enabled = true;
        cfg.oidc.issuer_url = "http://localhost:8081/realms/egret".into();
        cfg.oidc.client_id = "egret".into();
        cfg.oidc.client_secret = "egret-secret".into();
        cfg.oidc.redirect_url = "http://localhost:5173/api/auth/oidc/callback".into();
        cfg.security.secret_key =
            "Vi2XPRkbfyTLbSzhWv0VBYeOozGGoUnVSL3pIx2Oy1kbBphJyL0CXpomCNKdrYndzYnOX8MH0oaxhcY6Bt/x8g=="
                .into();
        // [storage] is required unconditionally (final review I4), independent
        // of oidc.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn catalog_disabled_by_default_validates() {
        let mut cfg = Config::default();
        assert!(!cfg.catalog.enabled);
        // [storage] is required unconditionally (final review I4), independent
        // of catalog.enabled.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn catalog_enabled_requires_token_and_creds() {
        let mut cfg = Config::default();
        cfg.catalog.enabled = true;
        assert!(cfg.validate().is_err());
        cfg.catalog.auth_token = "t".into();
        cfg.storage.access_key = "a".into();
        cfg.storage.secret_key = "s".into();
        // Final review I4: [storage] is also required unconditionally, and
        // that check also needs `bucket`, which catalog.enabled's own check
        // does not look at.
        cfg.storage.bucket = "b".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_disabled_by_default_validates() {
        let mut cfg = Config::default();
        assert!(!cfg.ranger.enabled);
        // [storage] is required unconditionally (final review I4), independent
        // of ranger.enabled.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_enabled_validates_with_or_without_token() {
        let mut cfg = Config::default();
        cfg.ranger.enabled = true;
        // [storage] is required unconditionally (final review I4), independent
        // of ranger.enabled.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        // Empty token: no operator bearer; engines use their own credentials. Valid.
        assert!(cfg.validate().is_ok());
        // A configured token is also valid (the operator bearer).
        cfg.ranger.auth_token = "t".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_auth_mode_defaults_to_required_and_parses_anonymous() {
        figment::Jail::expect_with(|jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.ranger.auth_mode, RangerAuthMode::Required);
            jail.set_env("EGRET_RANGER__AUTH_MODE", "anonymous");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.ranger.auth_mode, RangerAuthMode::Anonymous);
            jail.set_env("EGRET_RANGER__AUTH_MODE", "open");
            assert!(
                Config::figment().extract::<Config>().is_err(),
                "an unknown mode must not silently become `required` or `anonymous`"
            );
            Ok(())
        });
    }

    #[test]
    fn secrets_master_key_defaults_empty_and_validates() {
        let mut cfg = Config::default();
        assert!(cfg.secrets.master_key.is_empty());
        // [storage] is required unconditionally (final review I4), independent
        // of secrets.master_key.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_must_be_base64_32_bytes_when_set() {
        let mut cfg = Config::default();
        cfg.secrets.master_key = "tooshort".into(); // not valid base64 of >=32 bytes
        assert!(cfg.validate().is_err());
        use base64::Engine;
        cfg.secrets.master_key = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        // [storage] is required unconditionally (final review I4), independent
        // of secrets.master_key.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engine_defaults_present_and_validate() {
        let mut cfg = Config::default();
        assert_eq!(cfg.engine.max_rows, 100_000);
        assert_eq!(cfg.engine.query_timeout_secs, 60);
        assert_eq!(cfg.engine.preview_rows, 100);
        // [storage] is required unconditionally (final review I4), independent
        // of engine settings.
        cfg.storage.bucket = "test-bucket".into();
        cfg.storage.access_key = "test-key".into();
        cfg.storage.secret_key = "test-secret".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engines_defaults_and_validate() {
        let mut cfg = Config::default();
        assert_eq!(cfg.engines.provisioner, Provisioner::Auto);
        assert_eq!(cfg.engines.docker.host, "unix:///var/run/docker.sock");
        assert_eq!(cfg.engines.docker.allowed_images, vec!["trinodb/trino"]);
        cfg.storage.bucket = "b".into();
        cfg.storage.access_key = "k".into();
        cfg.storage.secret_key = "s".into();
        assert!(cfg.validate().is_ok());
    }

    fn engines_valid_base(cfg: &mut Config) {
        cfg.storage.bucket = "b".into();
        cfg.storage.access_key = "k".into();
        cfg.storage.secret_key = "s".into();
    }

    #[test]
    fn engines_docker_requires_network() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_ENGINES__PROVISIONER", "docker");
            let mut cfg: Config = Config::figment().extract()?;
            engines_valid_base(&mut cfg);
            let err = cfg.validate().unwrap_err();
            assert!(err.contains("engines.docker.network"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn engines_docker_with_network_validates() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_ENGINES__PROVISIONER", "docker");
            jail.set_env("EGRET_ENGINES__DOCKER__NETWORK", "egret-demo_default");
            jail.set_env("EGRET_ENGINES__DOCKER__PROJECT", "egret-demo");
            let mut cfg: Config = Config::figment().extract()?;
            engines_valid_base(&mut cfg);
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn engines_allowed_images_parse_json_array_from_env() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                "EGRET_ENGINES__DOCKER__ALLOWED_IMAGES",
                r#"["trinodb/trino","acme/trino"]"#,
            );
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.engines.docker.allowed_images,
                vec!["trinodb/trino", "acme/trino"]
            );
            Ok(())
        });
    }

    #[test]
    fn engines_unknown_provisioner_fails_to_load() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_ENGINES__PROVISIONER", "bogus");
            assert!(Config::figment().extract::<Config>().is_err());
            Ok(())
        });
    }

    #[test]
    fn engines_docker_rejects_bad_host_and_empty_images() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_ENGINES__PROVISIONER", "docker");
            jail.set_env("EGRET_ENGINES__DOCKER__NETWORK", "n");
            jail.set_env("EGRET_ENGINES__DOCKER__PROJECT", "p");
            jail.set_env("EGRET_ENGINES__DOCKER__HOST", "ftp://x");
            let mut cfg: Config = Config::figment().extract()?;
            engines_valid_base(&mut cfg);
            let err = cfg.validate().unwrap_err();
            assert!(err.contains("engines.docker.host"), "{err}");
            cfg.engines.docker.host = "tcp://docker:2375".into();
            assert!(cfg.validate().is_ok());
            cfg.engines.docker.allowed_images.clear();
            let err = cfg.validate().unwrap_err();
            assert!(err.contains("allowed_images"), "{err}");
            Ok(())
        });
    }

    fn docker_engines_ok() -> Config {
        let mut cfg = Config::default();
        engines_valid_base(&mut cfg);
        cfg.engines.provisioner = Provisioner::Docker;
        cfg.engines.docker.network = "n".into();
        cfg.engines.docker.project = "p".into();
        assert!(cfg.validate().is_ok());
        cfg
    }

    #[test]
    fn engines_docker_requires_a_project() {
        let mut cfg = docker_engines_ok();
        cfg.engines.docker.project = " ".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("requires engines.docker.project"), "{err}");
    }

    #[test]
    fn engines_docker_rejects_an_empty_callback_url() {
        let mut cfg = docker_engines_ok();
        cfg.engines.docker.callback_url = Some(String::new());
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("callback_url is set but empty"), "{err}");
        cfg.engines.docker.callback_url = Some("http://egret:8080".into());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engines_docker_refuses_padded_values() {
        for key in ["network", "project", "host", "callback_url"] {
            let mut cfg = docker_engines_ok();
            let d = &mut cfg.engines.docker;
            match key {
                "network" => d.network = "n ".into(),
                "project" => d.project = " p".into(),
                "host" => d.host = "tcp://docker:2375 ".into(),
                _ => d.callback_url = Some(" http://egret:8080".into()),
            }
            let err = cfg.validate().unwrap_err();
            assert!(
                err.contains(&format!("engines.docker.{key} has surrounding whitespace")),
                "{err}"
            );
        }
    }

    #[test]
    fn engines_docker_host_schemes_match_the_client() {
        let mut cfg = docker_engines_ok();
        for ok in [
            "unix:///var/run/docker.sock",
            "tcp://docker:2375",
            "http://docker:2375",
        ] {
            cfg.engines.docker.host = ok.into();
            assert!(cfg.validate().is_ok(), "{ok}");
        }
        cfg.engines.docker.host = "https://docker:2376".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audit_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert!(cfg.audit.enabled);
            assert_eq!(cfg.audit.retention_days, 90);
            assert_eq!(cfg.audit.channel_capacity, 8192);
            assert_eq!(cfg.audit.batch_size, 256);
            assert_eq!(cfg.audit.flush_interval_ms, 1000);
            Ok(())
        });
    }

    #[test]
    fn engine_preview_must_not_exceed_max_rows() {
        let mut cfg = Config::default();
        cfg.engine.preview_rows = 200_000; // > max_rows
        assert!(cfg.validate().is_err());
    }

    /// **The exposure lever, at the layer that parses it.** Fix round 3,
    /// Minor 3: the `String`→`bool` step had no test at all. Fix round 4,
    /// Minor 1: it then had one whose middle arm was **dead code** —
    /// `if let Ok(..)` over three values that are all errors, so `assert!(false)`
    /// inside it passed, and **the behaviour that distinguishes round 3's lever
    /// from round 2's was unasserted**.
    ///
    /// Measured, on this figment, rather than assumed:
    ///
    /// | value | result |
    /// |---|---|
    /// | `true` | `Ok(true)` |
    /// | `false` | `Ok(false)` |
    /// | `1`, `on`, `yes` | **startup error** |
    /// | `TRUE`, `True`, `FALSE` | **startup error** |
    /// | `yep`, `2`, empty | **startup error** |
    ///
    /// **Exactly two spellings are accepted.** That is the change from round 2,
    /// whose hand-rolled `matches!("1" | "true" | "yes" | "on")` accepted three
    /// of the rows above and silently answered `false` for the rest. Strictness
    /// is the right trade here because the only producer a deployment has is
    /// helm, which renders a Go bool as lowercase `true`/`false` — asserted
    /// below against the chart itself, because "the chart can only emit a legal
    /// spelling" is the sentence that makes refusing everything else safe.
    #[test]
    fn the_webhook_lever_defaults_off_and_refuses_what_it_cannot_parse() {
        // **Every arm below counts its own iterations and the counts are
        // asserted at the end.** Fix round 4, Minor 1: the arm this replaces
        // was `if let Ok(..)` over three values that are all errors, so its
        // body never ran and `assert!(false)` inside it passed. A test whose
        // arm cannot execute is worse than no test, and the only cheap way to
        // know is to count.
        let mut accepted = 0usize;
        let mut refused = 0usize;
        let mut padded = 0usize;

        // 1. The default.
        figment::Jail::expect_with(|_jail| {
            let c: Config = Config::figment().extract().expect("defaults");
            assert!(
                !c.workflow.webhooks_enabled,
                "the anonymous webhook surface must default to OFF"
            );
            Ok(())
        });

        // 2. The two spellings that ARE accepted, both directions.
        for (spelling, expected) in [("true", true), ("false", false)] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("EGRET_WORKFLOW__WEBHOOKS_ENABLED", spelling);
                let c: Config = Config::figment()
                    .extract()
                    .unwrap_or_else(|e| panic!("`{spelling}` must parse: {e}"));
                assert_eq!(c.workflow.webhooks_enabled, expected);
                accepted += 1;
                Ok(())
            });
        }

        // 3. Everything else is a STARTUP ERROR, not a silent `false`.
        //
        // The first three are round 2's accepted spellings: this arm is the
        // running record of what changed. The rest include the review's own
        // fullwidth and zero-width-space probes.
        for spelling in [
            "1",
            "on",
            "yes", // round 2 accepted these
            "TRUE",
            "True",
            "FALSE",
            "ＴＲＵＥ",
            "tRuE\u{200b}", // case and lookalikes
            "yep",
            "enabled",
            "2",
            "", // junk
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("EGRET_WORKFLOW__WEBHOOKS_ENABLED", spelling);
                match Config::figment().extract::<Config>() {
                    Err(_) => refused += 1,
                    Ok(c) => panic!(
                        "{spelling:?} was ACCEPTED as {} -- it must be a startup error, so an \
                         operator who mistyped finds out at boot and a typo can never turn an \
                         anonymous surface on",
                        c.workflow.webhooks_enabled
                    ),
                }
                Ok(())
            });
        }

        // 3b. **Whitespace padding IS tolerated**, and that is a measurement
        //     rather than a design choice: figment trims before parsing, so
        //     `" true"` is `true`. Recorded because nothing else in this test
        //     says it, and because a reader of arm 3 would otherwise conclude
        //     the parser is byte-exact. It is the benign direction -- the value
        //     is unambiguous either way -- and it is a spelling round 2's
        //     `matches!` also accepted, so it is not part of what changed.
        for spelling in [" true", "true ", "\ttrue\n"] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("EGRET_WORKFLOW__WEBHOOKS_ENABLED", spelling);
                let c: Config = Config::figment()
                    .extract()
                    .unwrap_or_else(|e| panic!("{spelling:?} must parse: {e}"));
                assert!(c.workflow.webhooks_enabled, "{spelling:?}");
                padded += 1;
                Ok(())
            });
        }

        // 4. **The chart can only emit a legal spelling**, which is what makes
        //    the strictness above safe rather than a trap. Helm renders a Go
        //    bool as lowercase `true`/`false`; the assertion is that the
        //    template interpolates the value rather than hard-coding a word
        //    this parser would refuse.
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/templates/configmap-env.yaml");
        let src = std::fs::read_to_string(&chart)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", chart.display()));
        assert!(
            src.contains(
                r#"EGRET_WORKFLOW__WEBHOOKS_ENABLED: "{{ .Values.workflow.webhooksEnabled }}""#
            ),
            "the chart must interpolate the Go bool, not a literal this parser would refuse"
        );
        let values = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/values.yaml");
        let vsrc = std::fs::read_to_string(&values).expect("values.yaml");
        assert!(
            vsrc.contains("webhooksEnabled: false"),
            "and the chart's default must be off"
        );

        // The counts. Without these every loop above is `if let`-shaped dead
        // code away from asserting nothing, which is exactly what Minor 1 was.
        assert_eq!(accepted, 2, "arm 2 did not run for both spellings");
        assert_eq!(refused, 12, "arm 3 did not run for every refused spelling");
        assert_eq!(padded, 3, "arm 3b did not run");
    }

    /// **The OTHER lever, W10 Task 2: `worker_enabled` defaults off, is read
    /// through the section convention, and the chart can emit it.**
    ///
    /// A second test rather than a widened first one, because the two levers
    /// gate different things and a shared test would make "which one is off"
    /// unreadable in a failure message.
    ///
    /// The strictness arm is the same parser as above — `bool` via figment —
    /// and is spot-checked rather than re-enumerated: what is worth asserting
    /// separately is that the **key spelling** works, because W6 Task 7 fix
    /// round 2 shipped its sibling as a flat `EGRET_WORKFLOW_WEBHOOKS_ENABLED`
    /// read with `std::env::var`, outside the config type and outside the
    /// chart. A key that is silently never read looks exactly like a key an
    /// operator set to `false`.
    ///
    /// **The four tuning knobs are asserted here too**, and for a reason the
    /// lever does not need: each has a `#[serde(default = ...)]` of its own, so
    /// an `[workflow]` section that names only `webhooks_enabled` must still
    /// produce all five. That is the arm a plain field would fail.
    #[test]
    fn the_worker_lever_defaults_off_and_is_read_through_the_section_convention() {
        let mut accepted = 0usize;

        // 1. The default, and the ruling: a loop that has never run in
        //    production is opt-in, for the webhook lever's own reason.
        figment::Jail::expect_with(|_jail| {
            let c: Config = Config::figment().extract().expect("defaults");
            assert!(
                !c.workflow.worker_enabled,
                "the claim loop must default to OFF"
            );
            assert_eq!(c.workflow.tick_interval_secs, 1);
            assert_eq!(c.workflow.batch, 8);
            assert_eq!(c.workflow.reaper_interval_secs, 60);
            assert_eq!(c.workflow.working_timeout_s, 3600);
            Ok(())
        });

        // 2. Both spellings, through `EGRET_<SECTION>__<KEY>`.
        for (spelling, expected) in [("true", true), ("false", false)] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("EGRET_WORKFLOW__WORKER_ENABLED", spelling);
                let c: Config = Config::figment()
                    .extract()
                    .unwrap_or_else(|e| panic!("`{spelling}` must parse: {e}"));
                assert_eq!(
                    c.workflow.worker_enabled, expected,
                    "EGRET_WORKFLOW__WORKER_ENABLED={spelling} was not read"
                );
                // ...and it must not have disturbed the lever beside it.
                assert!(!c.workflow.webhooks_enabled);
                accepted += 1;
                Ok(())
            });
        }

        // 3. A junk value is a STARTUP ERROR, not a silent `false`, exactly as
        //    for the webhook lever: an operator who mistyped finds out at boot
        //    rather than by wondering why nothing runs.
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__WORKER_ENABLED", "yes");
            match Config::figment().extract::<Config>() {
                Err(_) => {}
                Ok(c) => panic!(
                    "`yes` was ACCEPTED as {} -- it must be a startup error",
                    c.workflow.worker_enabled
                ),
            }
            Ok(())
        });

        // 4. A `[workflow]` section that names only ONE key still produces all
        //    six. This is what the per-field `#[serde(default = ...)]`s buy and
        //    a plain field would fail here.
        figment::Jail::expect_with(|jail| {
            jail.create_file("egret.toml", "[workflow]\nwebhooks_enabled = true\n")?;
            let c: Config = Config::figment().extract().expect("partial section");
            assert!(c.workflow.webhooks_enabled, "the key that WAS named");
            assert!(!c.workflow.worker_enabled, "and the five that were not");
            assert_eq!(c.workflow.batch, 8);
            Ok(())
        });

        // 5. The chart can emit it, and its default is off. Same argument as
        //    the webhook lever's arm 4: strict parsing is only safe when the
        //    one producer a deployment has can only emit a legal spelling.
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/templates/configmap-env.yaml");
        let src = std::fs::read_to_string(&chart)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", chart.display()));
        assert!(
            src.contains(
                r#"EGRET_WORKFLOW__WORKER_ENABLED: "{{ .Values.workflow.workerEnabled }}""#
            ),
            "the chart must carry the worker lever -- a switch reachable only by \
             hand-written env is the shape fix round 2 shipped and round 3 undid"
        );
        let values = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/values.yaml");
        let vsrc = std::fs::read_to_string(&values).expect("values.yaml");
        assert!(
            vsrc.contains("workerEnabled: false"),
            "and the chart's default must be off"
        );

        assert_eq!(accepted, 2, "arm 2 did not run for both spellings");
    }

    /// The W12 scheduler lever, held to the worker lever's standard: off by
    /// default, read through `EGRET_WORKFLOW__SCHEDULER_ENABLED`, independent
    /// of the worker's key, and **carried by the chart** -- the backend
    /// Deployment has no `extraEnv` hook, so a lever missing from the
    /// configmap is one an operator can only set by patching rendered
    /// manifests. `trigger_interval_secs` is a tuning knob and is deliberately
    /// not a chart value, like the worker's.
    #[test]
    fn the_scheduler_lever_defaults_off_and_the_chart_carries_it() {
        // 1. The default, with its interval.
        figment::Jail::expect_with(|_jail| {
            let c: Config = Config::figment().extract().expect("defaults");
            assert!(
                !c.workflow.scheduler_enabled,
                "the scheduler must default to OFF"
            );
            assert_eq!(c.workflow.trigger_interval_secs, 1);
            Ok(())
        });

        // 2. The env spelling is read, and moves only its own key.
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_WORKFLOW__SCHEDULER_ENABLED", "true");
            let c: Config = Config::figment().extract().expect("`true` must parse");
            assert!(
                c.workflow.scheduler_enabled,
                "EGRET_WORKFLOW__SCHEDULER_ENABLED=true was not read"
            );
            assert!(
                !c.workflow.worker_enabled,
                "and the worker lever is untouched"
            );
            Ok(())
        });

        // 3. The chart emits it, and its default is off.
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/templates/configmap-env.yaml");
        let src = std::fs::read_to_string(&chart)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", chart.display()));
        assert!(
            src.contains(
                r#"EGRET_WORKFLOW__SCHEDULER_ENABLED: "{{ .Values.workflow.schedulerEnabled }}""#
            ),
            "the chart must carry the scheduler lever: the backend Deployment has \
             no extraEnv hook, so without this line an operator cannot turn it on"
        );
        let values = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/helm/egret/values.yaml");
        let vsrc = std::fs::read_to_string(&values).expect("values.yaml");
        assert!(
            vsrc.contains("schedulerEnabled: false"),
            "and the chart's default must be off"
        );
    }

    /// `Env::split("__")` is recursive, so three levels — section, map key,
    /// field — work; this test is what proves that rather than assuming it
    /// (D5: `[starrocks.jdbc_drivers]` reaches a per-dialect field from the
    /// environment, which no other section in this file needs to do).
    #[test]
    fn a_driver_override_arrives_under_the_nested_env_name() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                "EGRET_STARROCKS__JDBC_DRIVERS__MYSQL__DRIVER_URL",
                "file:///opt/mariadb.jar",
            );
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(
                cfg.starrocks
                    .jdbc_drivers
                    .mysql
                    .unwrap()
                    .driver_url
                    .as_deref(),
                Some("file:///opt/mariadb.jar")
            );
            Ok(())
        });
    }

    /// Final review I4: the Data Apps HTTP API (create/save/publish/draft/
    /// version routes) is mounted unconditionally in `egret-bin`, regardless
    /// of `apps.pod.launcher` -- that setting only gates whether interactive
    /// notebook PODS can launch. So `[storage]` must be required at boot even
    /// with the documented default `launcher = "none"`, instead of failing
    /// every CRUD request with a 500 at request time.
    #[test]
    fn apps_api_always_requires_storage_regardless_of_launcher() {
        let cfg = Config::default();
        assert_eq!(cfg.apps.pod.launcher, "none");
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("[storage]"), "{err}");
    }

    #[test]
    fn apps_pod_launcher_other_than_none_requires_storage() {
        let mut cfg = Config::default();
        // Set up required fields so we reach the storage requirement check
        cfg.apps.origin = "https://apps.egret.example".into();
        cfg.apps.egret_origin = "https://egret.example".into();
        cfg.apps.pod.launcher = "subprocess".into();
        cfg.apps.pod.callback_url = "http://egret:8080".into();
        cfg.apps.pod.command = vec!["egret-apps".into()];
        cfg.apps.pod.config_dir = "apps-image/config".into();
        cfg.apps.pod.venv_shim_dir = "apps-image/venv-shim".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("[storage]"), "{err}");
    }

    #[test]
    fn apps_storage_prefix_must_end_with_a_slash() {
        let cfg = AppsStorageConfig {
            prefix: "apps".into(),
        };
        assert!(cfg.validate().is_err());
    }
}
