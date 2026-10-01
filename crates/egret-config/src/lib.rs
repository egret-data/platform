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
    pub auth_token: String,
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
    /// suffix), advertised in `/v1/config`'s `s3.signer.uri`. Defaults to
    /// `http://localhost:8080`; set this to a client-reachable URL in production.
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
pub struct KernelConfig {
    /// Which `KernelLauncher` impl to use: `"subprocess"` (default, local dev)
    /// or `"kubernetes"` (one pod per user×workspace; requires an in-cluster
    /// or kubeconfig-reachable API server).
    pub launcher: String,
    /// Namespace the `KubernetesLauncher` creates kernel Pods in.
    pub namespace: String,
    /// Kernel container image.
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    /// Name of an existing `kubernetes.io/dockerconfigjson` Secret for pulling
    /// `image`, if the registry requires auth. `None` omits imagePullSecrets.
    pub pull_secret: Option<String>,
    /// Externally/cluster-reachable base URL kernels use to call back into
    /// egret (`EGRET_API_URL`). Overrides `catalog.public_url`-derived
    /// `api_url` for the kernel-env path only — set this when the server's
    /// own public URL isn't reachable from inside the cluster (e.g. behind an
    /// ingress the kernel pods can't resolve), such as a Service DNS name.
    pub callback_url: Option<String>,
    /// Accept the engine's self-signed TLS cert in kernel `trino()`/`starrocks()`
    /// — sets EGRET_ENGINE_TLS_INSECURE=1 on kernel pods. Dev/self-signed deploys
    /// only.
    pub engine_tls_insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// When false, the PDP is not wrapped and no events are emitted.
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
    pub rate_window_secs: u64,
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
    pub kernel: KernelConfig,
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
        reaper_interval_secs: default_reaper_interval_secs(),
        working_timeout_s: default_working_timeout_s(),
        container_default_timeout_s: default_container_default_timeout_s(),
        scheduler_enabled: default_scheduler_enabled(),
        trigger_interval_secs: default_trigger_interval_secs(),
        log_retention_days: default_log_retention_days(),
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
                public_url: Some("http://localhost:8080".into()),
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
            audit: AuditConfig {
                enabled: true,
                retention_days: 90,
                channel_capacity: 8192,
                batch_size: 256,
                flush_interval_ms: 1000,
            },
            kernel: KernelConfig {
                launcher: "subprocess".into(),
                namespace: "egret-notebooks".into(),
                image: "egret-kernel:dev".into(),
                image_pull_policy: "IfNotPresent".into(),
                cpu_request: "250m".into(),
                mem_request: "512Mi".into(),
                cpu_limit: "1000m".into(),
                mem_limit: "2Gi".into(),
                pull_secret: None,
                callback_url: None,
                engine_tls_insecure: false,
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
        Self::figment().extract()
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
        // ranger.auth_token may be empty: that selects anonymous `/service` access for stock
        // Ranger plugins (e.g. Trino) that authenticate out-of-band. Set a token to require bearer.
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
        if self.engine.max_rows == 0 || self.engine.query_timeout_secs == 0 {
            return Err("engine.max_rows and engine.query_timeout_secs must be >= 1".into());
        }
        if self.engine.preview_rows > self.engine.max_rows {
            return Err("engine.preview_rows must be <= engine.max_rows".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

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
        // W5b: the plugin host's three settings, under the runner's block.
        let env = read("templates/configmap-env.yaml");
        for key in [
            "EGRET_WORKFLOW__PLUGIN_HOST__IMAGE: {{ required",
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
    fn validate_ok_when_oidc_disabled() {
        let cfg = Config::default();
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
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn catalog_disabled_by_default_validates() {
        let cfg = Config::default();
        assert!(!cfg.catalog.enabled);
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
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_disabled_by_default_validates() {
        let cfg = Config::default();
        assert!(!cfg.ranger.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_enabled_validates_with_or_without_token() {
        let mut cfg = Config::default();
        cfg.ranger.enabled = true;
        // Empty token = anonymous /service (for stock Ranger plugins). Valid.
        assert!(cfg.validate().is_ok());
        // A configured token is also valid (requires bearer).
        cfg.ranger.auth_token = "t".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_defaults_empty_and_validates() {
        let cfg = Config::default();
        assert!(cfg.secrets.master_key.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_must_be_base64_32_bytes_when_set() {
        let mut cfg = Config::default();
        cfg.secrets.master_key = "tooshort".into(); // not valid base64 of >=32 bytes
        assert!(cfg.validate().is_err());
        use base64::Engine;
        cfg.secrets.master_key = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engine_defaults_present_and_validate() {
        let cfg = Config::default();
        assert_eq!(cfg.engine.max_rows, 100_000);
        assert_eq!(cfg.engine.query_timeout_secs, 60);
        assert_eq!(cfg.engine.preview_rows, 100);
        assert!(cfg.validate().is_ok());
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

    #[test]
    fn kernel_defaults_to_subprocess_launcher() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.kernel.launcher, "subprocess");
            assert_eq!(cfg.kernel.namespace, "egret-notebooks");
            assert!(cfg.kernel.pull_secret.is_none());
            assert!(cfg.kernel.callback_url.is_none());
            Ok(())
        });
    }

    #[test]
    fn kernel_launcher_selectable_via_env() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_KERNEL__LAUNCHER", "kubernetes");
            jail.set_env("EGRET_KERNEL__NAMESPACE", "custom-ns");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.kernel.launcher, "kubernetes");
            assert_eq!(cfg.kernel.namespace, "custom-ns");
            Ok(())
        });
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
}
