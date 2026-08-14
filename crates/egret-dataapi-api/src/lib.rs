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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
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
    /// **The one input to the frozen contract that lives outside the snapshot.**
    /// `query.databases.delegated` is a mutable column: `cache.scope = shared` is
    /// refused at publish time on a delegated connection (Ranger filters rows per
    /// principal, so a shared cache serves one caller's rows to another), but a
    /// non-delegated connection can be flipped to delegated afterwards, and no
    /// re-validation runs anywhere when it is. Freezing the value publish
    /// validated against is what lets the serving path notice.
    ///
    /// **P4 must compare this against the live `delegated` before serving from a
    /// shared cache, and refuse when they disagree.** Every other field here is
    /// self-contained; this one is a witness, and it is only useful if it is
    /// checked.
    pub delegated: bool,
    /// Resolved by `describe_as` at publish time and frozen here, so the
    /// published contract does not drift when the underlying table changes.
    pub columns: Vec<ColumnSpec>,
}

/// The frozen form of a `ColumnMeta`. Deliberately its own type rather than
/// re-using `egret_core::ColumnMeta`: this one is persisted inside a published
/// snapshot, so it must stay stable even if the engine-facing type gains or
/// renames a field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
}

impl From<&egret_core::ColumnMeta> for ColumnSpec {
    fn from(c: &egret_core::ColumnMeta) -> Self {
        // ColumnMeta is { name: String, ty: String } — verified at
        // crates/egret-core/src/tabular.rs:11-15.
        Self {
            name: c.name.clone(),
            ty: c.ty.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
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
}
