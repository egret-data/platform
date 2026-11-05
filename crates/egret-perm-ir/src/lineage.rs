//! What the reserved lineage principal keeps (view governance R5): the access
//! types view-lineage EXPLAIN needs on each leaf resource of Trino's
//! service-def, and the two that it keeps on one value only. Everything else
//! is refused to the principal.
//!
//! One source for both targets: the Ranger download's lineage transform
//! (`egret_policy::lineage_transform`) denies every other access type at
//! override priority, and the OPA evaluator (`egret-perm-eval`) refuses the
//! same accesses (governance SP2b Task 18).

/// The one system session property egret's client sets on every statement;
/// the principal may set it and no other.
pub const LINEAGE_SYSTEM_PROPERTY: &str = "query_max_run_time";

/// What EXPLAIN needs on a leaf resource, never refused to the principal.
///
/// - the catalog chain: `use` on the catalog, `select` on every referenced
///   column (live result §1 Q3, the measured minimum);
/// - `trinouser`: `impersonate`, which Trino 483 checks for the session user
///   on every query (the Q3 baseline);
/// - `queryid`: `execute`, likewise;
/// - `systemproperty`: `alter`, because egret's client sets the system session
///   property [`LINEAGE_SYSTEM_PROPERTY`] on every statement.
///
/// Every other access type on every leaf is refused.
pub fn kept_accesses(leaf: &str) -> &'static [&'static str] {
    match leaf {
        // The data chain, at whatever level a resource stops (Trino's
        // service-def lets one stop at the catalog, schema or table).
        "catalog" | "schema" | "table" | "column" => &["select", "use"],
        "trinouser" => &["impersonate"],
        "queryid" => &["execute"],
        "systemproperty" => &["alter"],
        _ => &[],
    }
}

/// Which values a narrowed kept access stays kept on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptOn {
    /// The principal's own name only (`trinouser`: impersonate itself).
    Itself,
    /// This one value only.
    Value(&'static str),
}

/// A kept access that is kept only on some values of its leaf, as
/// `(access, the values it stays kept on)`; refused on every other value.
///
/// - `trinouser`: `impersonate` of itself only. With an egret JWT the session
///   user is the principal, so EXPLAIN never needs another user.
/// - `systemproperty`: `alter` of [`LINEAGE_SYSTEM_PROPERTY`] only.
///
/// `queryid` keeps `execute` on every value: its value is the id of the query
/// being run, which egret cannot know before the engine assigns it.
pub fn kept_only_on(leaf: &str) -> Option<(&'static str, KeptOn)> {
    match leaf {
        "trinouser" => Some(("impersonate", KeptOn::Itself)),
        "systemproperty" => Some(("alter", KeptOn::Value(LINEAGE_SYSTEM_PROPERTY))),
        _ => None,
    }
}
