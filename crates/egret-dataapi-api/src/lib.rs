//! `egret-dataapi-api` — the Data API seam's contract crate (P2/P3).
//!
//! Mirrors `egret-query-api` / `egret-catalog-api` / `egret-policy-api`: a pure
//! leaf (no sqlx, axum, or heavy egret-* deps) so any cross-seam consumer can
//! depend on the Data API's wire vocabulary cheaply.
//!
//! P3 adds the endpoint-definition DTOs: `ParamSpec`/`ParamType`/`OutputSpec`
//! describe a draft endpoint's shape, and `EndpointSnapshot` is what a
//! publish freezes into `dataapi.endpoint_versions.snapshot` — self-contained
//! on purpose, since P4's serving path must answer a request from the
//! snapshot alone, without reading the draft row.

use serde::{Deserialize, Serialize};

mod port;
pub use port::DataApiEndpointPort;

/// The Cedar object KIND a Data API endpoint appears as in an authorization
/// decision, and the `access_grants.object_type` of an invoke grant (spec §5,
/// §6.2 step 4).
///
/// Canonical definition. `egret-dataapi`'s `endpoints::OBJECT_KIND`
/// re-exports this rather than keeping a second `"data_api_endpoint"`
/// literal — two string literals that must agree and are not the same
/// literal is exactly the drift a leaf contract crate exists to prevent.
/// `egret-automation` (Z2's alert authorization, Task 4) needs to name the
/// same kind to gate a `dataapi_alert` without depending on `egret-dataapi`
/// itself, which is why this lives here rather than in that crate.
pub const OBJECT_KIND: &str = "data_api_endpoint";

/// Permission that lets a caller list/inspect a Data API endpoint.
///
/// Canonical definition — see [`OBJECT_KIND`]'s doc comment for why this
/// lives here instead of in `egret-dataapi`, where it was previously a
/// private `const` in `routes.rs`.
pub const EP_READ: &str = "data_api_endpoint:read";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Number,
    Boolean,
    Date,
    Timestamp,
    Enum,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Query,
    Body,
}

/// `deny_unknown_fields` on the author-supplied config types.
///
/// `#[serde(deny_unknown_fields)]` on the request-body struct does NOT reach a
/// nested one, and the nested one is where the damage was: posting the
/// documented-but-wrong `{"rate_limit": {"per_client_qps": 5}}` returned 201
/// with `per_key_qps` silently defaulted to `0`. A rate limit the author
/// believes they set and did not is worse than an error.
///
/// Applied to the five types an AUTHOR fills in — [`ParamSpec`], [`CacheSpec`],
/// [`LimitsSpec`], [`RateLimitSpec`], [`OutputSpec`] — and deliberately NOT to
/// [`EndpointSnapshot`] or [`ColumnSpec`], which the server writes for itself
/// and where a typo is impossible.
///
/// **The cost, stated because it is a real constraint on a persisted format.**
/// These types are also how a stored `jsonb` column is read back. Adding a field
/// in some future version and then rolling the binary back would make the older
/// code refuse to deserialize rows the newer one wrote, as a 500 on read. That is
/// a loud failure rather than a silent one, and it is the standard price of
/// strict deserialization; a future field that must be readable by older code has
/// to be introduced some other way.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub location: ParamLocation,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Required when `ty` is `Enum`. The ONLY safe form for anything that
    /// reaches an identifier position — see spec §3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<String>>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    PerPrincipal,
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheSpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u32,
    /// `Shared` is refused at validate time on a delegated connection: Ranger
    /// filters per principal, so a shared cache would leak across identities.
    #[serde(default = "default_scope")]
    pub scope: CacheScope,
}
fn default_ttl() -> u32 {
    60
}
fn default_scope() -> CacheScope {
    CacheScope::PerPrincipal
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LimitsSpec {
    #[serde(default = "default_max_rows")]
    pub max_rows: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}
fn default_max_rows() -> u32 {
    10_000
}
fn default_timeout() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    #[serde(default)]
    pub per_key_qps: u32,
    #[serde(default)]
    pub burst: u32,
    #[serde(default)]
    pub per_endpoint_qps: u32,
}

/// What a published version freezes. Self-contained on purpose: P4's serving
/// path must be able to answer a request from this alone, without reading the
/// draft row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct EndpointSnapshot {
    pub sql: String,
    pub method: String,
    pub path: String,
    pub database_id: uuid::Uuid,
    pub params: Vec<ParamSpec>,
    pub output: OutputSpec,
    pub cache: CacheSpec,
    pub limits: LimitsSpec,
    pub rate_limit: RateLimitSpec,
    /// Whether `database_id` delegated per-user credentials to the engine at the
    /// moment this version was published.
    ///
    /// **Half of a witness PAIR — see [`Self::impersonate`], and do not read
    /// this field alone.** Together they are the two mutable columns that decide
    /// whether the engine runs a query as the CALLER, which is the only thing
    /// that makes a shared cache unsafe. An earlier version of this comment, and
    /// of both checks that read it, treated `delegated` as the whole condition;
    /// it is not, and the gap was a live cross-principal leak.
    pub delegated: bool,
    /// Whether `query.databases.options.impersonate` was set at the moment this
    /// version was published. The other half of the witness pair.
    ///
    /// **`delegated` alone does not decide whether the engine runs per
    /// principal, and believing it did was a real leak.** `delegation_for`'s
    /// NON-delegated arm still returns `Some(principal_email)` as the session
    /// user (`egret-query/src/execution/mod.rs:86`), and both connectors act on
    /// `options.impersonate` independently of `delegated`:
    ///
    /// * `connector/trino.rs:43` — `bearer_token.is_some() || options.impersonate
    ///   == Some(true)` sends `X-Trino-User: <caller>`;
    /// * `connector/starrocks.rs:129` — `bearer_token.is_none() &&
    ///   options.impersonate == Some(true)` issues
    ///   `EXECUTE AS <caller> WITH NO REVERT`.
    ///
    /// So `{delegated: false, options: {impersonate: true}}` — which
    /// `DatabaseInput` accepts, the two fields being independent and unvalidated
    /// — runs every query under the caller's own Ranger policy while looking
    /// non-delegated to anything that reads `delegated`.
    ///
    /// **Both halves must be compared against the live row before serving from a
    /// shared cache.** Freezing only `delegated` let a `shared` endpoint on such
    /// a connection cache principal A's filtered rows under a principal-less key
    /// and serve them to principal B: a 200, `meta.cached: true`, nothing in
    /// `access_log` to distinguish it, and the witness silent because live
    /// `delegated` still equalled frozen `delegated`.
    ///
    /// `#[serde(default)]` so a snapshot written before this field existed
    /// deserialises as `false`. That is the fail-closed direction: such a
    /// snapshot can only have been published against a connection this check now
    /// refuses, so comparing `false` against a live `true` yields
    /// `cache_scope_stale` and a re-publish, which is the correct outcome.
    #[serde(default)]
    pub impersonate: bool,
    /// Resolved by `describe_as` at publish time and frozen here, so the
    /// published contract does not drift when the underlying table changes.
    pub columns: Vec<ColumnSpec>,
    /// Which revision of the publish-time parameter gate this version passed.
    ///
    /// **A stamp, not a note.** P4.5's §7 said "every endpoint with parameters
    /// must be re-published after P4.5" — an instruction to an operator, which
    /// is not an enforcement. A version frozen before the gate existed carries
    /// `0` here (`#[serde(default)]`), so the serving path can REFUSE it rather
    /// than render a definition no rule ever looked at. That turns the migration
    /// from something someone has to remember into something the server checks.
    ///
    /// An integer rather than a bool, because the situation it exists for is
    /// exactly the one that recurs: when a future revision of the gate closes a
    /// rule the current one misses, bumping [`PARAM_GATE`] makes every snapshot
    /// stamped with the older gate refuse to serve until it is re-published and
    /// re-checked. A bool could only ever say "some gate ran", which is the claim
    /// that was not good enough this time either.
    ///
    /// Only consulted when the snapshot declares parameters: a version with no
    /// `params` renders nothing, so there is nothing for the gate to have missed
    /// and nothing to force a re-publish of.
    #[serde(default)]
    pub param_gate: u32,
}

/// The current revision of the publish-time parameter gate — what
/// [`EndpointSnapshot::param_gate`] is stamped with, and the minimum the serving
/// path accepts on a snapshot that declares parameters.
///
/// `0` is reserved for "published before the gate existed" and is never written.
pub const PARAM_GATE: u32 = 1;

fn default_required() -> bool {
    true
}

/// The frozen form of a `ColumnMeta`. Deliberately its own type rather than
/// re-using `egret_core::ColumnMeta`: this one is persisted inside a published
/// snapshot, so it must stay stable even if the engine-facing type gains or
/// renames a field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
    /// Present in EVERY permutation described at publish.
    ///
    /// Defaults to `true` when absent, which is what a snapshot written before
    /// this field existed carries. Those columns were checked for equality on
    /// every call, so `true` is the only default that does not loosen an
    /// already-published contract.
    #[serde(default = "default_required")]
    pub required: bool,
}

impl From<&egret_core::ColumnMeta> for ColumnSpec {
    fn from(c: &egret_core::ColumnMeta) -> Self {
        // ColumnMeta is { name: String, ty: String } — verified at
        // crates/egret-core/src/tabular.rs:11-15.
        Self {
            name: c.name.clone(),
            ty: c.ty.clone(),
            // Publish downgrades this to `false` for a column that only some
            // permutation returns; a single describe means every column is
            // required, which is the pre-M1 behaviour.
            required: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    #[serde(default = "default_shape")]
    pub shape: String, // "objects" | "arrays"
    #[serde(default = "default_naming")]
    pub naming: String, // "as_is" | "camel" | "snake"
}
fn default_shape() -> String {
    "objects".into()
}
fn default_naming() -> String {
    "as_is".into()
}

// Hand-written, not `#[derive(Default)]`: a derive builds each field from
// ITS OWN `Default` impl (`String::default()` is `""`), which has nothing to
// do with the `#[serde(default = "...")]` functions above — a derive here
// would silently diverge from `serde_json::from_str::<OutputSpec>("{}")` and
// hand the first caller of `OutputSpec::default()` empty-string output config
// with no error. Routing through the same functions serde uses makes the two
// paths structurally unable to disagree.
impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            shape: default_shape(),
            naming: default_naming(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins `OutputSpec::default()` to agree with what an absent `output`
    /// field deserializes to. A `#[derive(Default)]` here would pass every
    /// other test in this crate while quietly handing callers `{ shape: "",
    /// naming: "" }` — this is the one assertion that would catch it.
    #[test]
    fn default_output_spec_matches_the_serde_default() {
        let from_empty_json: OutputSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(OutputSpec::default(), from_empty_json);
        assert_eq!(OutputSpec::default().shape, "objects");
        assert_eq!(OutputSpec::default().naming, "as_is");
    }

    #[test]
    fn a_snapshot_written_before_required_existed_deserializes_as_required() {
        // Exactly the shape migration 0121 rows carry today.
        let old = r#"{"name":"id","ty":"integer"}"#;
        let c: ColumnSpec = serde_json::from_str(old).unwrap();
        assert!(
            c.required,
            "a column frozen before M1 was checked for equality on every call; \
             defaulting it to optional would loosen the gate on the whole fleet"
        );
    }

    #[test]
    fn a_described_column_is_required_until_publish_says_otherwise() {
        let meta = egret_core::ColumnMeta {
            name: "id".into(),
            ty: "integer".into(),
        };
        assert!(ColumnSpec::from(&meta).required);
    }
}
