//! How a catalog grant covers a namespace or a table: the one rule the
//! catalog guard (egret-bin) and the lakekeeper authorizer
//! ([`crate::authorizer`]) both decide by, so a request the guard lets
//! through and the credentials lakekeeper vends for it cannot disagree
//! (vended-credentials spec §4.2).

use crate::store::{CatalogError, CatalogGrant, CatalogStore};

/// Does this grant carry `write` or `manage`?
pub fn grants_write(g: &CatalogGrant) -> bool {
    g.privilege.eq_ignore_ascii_case("write") || g.privilege.eq_ignore_ascii_case("manage")
}

/// Does one grant authorize the requested `(namespace, table)` at the required
/// privilege? `catalog` covers everything; `namespace` covers its own namespace
/// and everything in it; `table` covers only its exact table (plus, for reads,
/// listing the namespace that holds it). Writes require `write`/`manage`.
/// `eq(granted, requested)` says whether two names are the same; callers pass
/// lakekeeper's collation ([`CatalogNameEq`]).
pub fn catalog_grant_covers_by(
    g: &CatalogGrant,
    target_ns: Option<&[String]>,
    target_table: Option<&str>,
    is_write: bool,
    eq: &dyn Fn(&str, &str) -> bool,
) -> bool {
    if is_write && !grants_write(g) {
        return false;
    }
    let ns_match = || match (target_ns, g.namespace.as_deref()) {
        (Some(t), Some(gn)) => t.len() == gn.len() && gn.iter().zip(t).all(|(a, b)| eq(a, b)),
        _ => false,
    };
    match g.securable_type.as_str() {
        "catalog" => true,
        "namespace" => ns_match(),
        "table" => {
            if !ns_match() {
                return false;
            }
            match target_table {
                // A specific table request must hit this grant's exact table.
                Some(t) => g.table_name.as_deref().is_some_and(|gt| eq(gt, t)),
                // Listing/loading the enclosing namespace is a read affordance for
                // a table-grant holder, never a write (e.g. creating a table).
                None => !is_write,
            }
        }
        _ => false,
    }
}

/// May a role holding this grant see namespace `ns` named in a listing
/// (KI-312)? A `catalog` grant sees every namespace. A `namespace` grant sees
/// its own namespace, the namespaces above it (to reach it) and those under it.
/// A `table` grant sees its table's namespace and the namespaces above it.
/// Any privilege sees. `eq` as for [`catalog_grant_covers_by`].
pub fn grant_sees_namespace(
    g: &CatalogGrant,
    ns: &[String],
    eq: &dyn Fn(&str, &str) -> bool,
) -> bool {
    let Some(granted) = g.namespace.as_deref() else {
        return g.securable_type == "catalog";
    };
    let common = granted.len().min(ns.len());
    let related = granted[..common]
        .iter()
        .zip(&ns[..common])
        .all(|(a, b)| eq(a, b));
    match g.securable_type.as_str() {
        "catalog" => true,
        // On, above or under the granted namespace.
        "namespace" => related,
        // The table's namespace or above it.
        "table" => related && ns.len() <= granted.len(),
        _ => false,
    }
}

/// Name equality as lakekeeper's collation has it, for the names in one
/// decision: byte-equal names are equal, and every other (grant name,
/// requested name) pair is asked of Postgres once
/// ([`CatalogStore::names_equal_in_catalog`]).
pub struct CatalogNameEq(std::collections::HashSet<(String, String)>);

impl CatalogNameEq {
    /// Ask Postgres about every pair of a name in `grants` and a name in
    /// `requested`.
    ///
    /// # Errors
    /// The store's error, if the query fails.
    pub async fn load(
        store: &CatalogStore,
        grants: &[CatalogGrant],
        requested: &[&str],
    ) -> Result<Self, CatalogError> {
        let requested: std::collections::BTreeSet<&str> = requested.iter().copied().collect();
        let granted: std::collections::BTreeSet<&str> = grants
            .iter()
            .flat_map(|g| {
                g.namespace
                    .iter()
                    .flatten()
                    .map(String::as_str)
                    .chain(g.table_name.as_deref())
            })
            .collect();
        let pairs: Vec<(String, String)> = granted
            .iter()
            .flat_map(|g| requested.iter().map(move |r| (*g, *r)))
            .filter(|(g, r)| g != r)
            .map(|(g, r)| (g.to_string(), r.to_string()))
            .collect();
        Ok(Self(store.names_equal_in_catalog(&pairs).await?))
    }

    /// `granted` (a grant's name) and `requested` name the same thing.
    pub fn eq(&self, granted: &str, requested: &str) -> bool {
        granted == requested
            || self
                .0
                .contains(&(granted.to_string(), requested.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(securable: &str, ns: Option<&[&str]>, table: Option<&str>) -> CatalogGrant {
        CatalogGrant {
            id: uuid::Uuid::nil(),
            catalog_role_id: uuid::Uuid::nil(),
            securable_type: securable.into(),
            namespace: ns.map(|n| n.iter().map(|s| s.to_string()).collect()),
            table_name: table.map(str::to_string),
            privilege: "read".into(),
        }
    }

    fn sees(g: &CatalogGrant, ns: &[&str]) -> bool {
        let ns: Vec<String> = ns.iter().map(|s| s.to_string()).collect();
        grant_sees_namespace(g, &ns, &|a, b| a == b)
    }

    #[test]
    fn a_namespace_grant_sees_its_line_and_nothing_beside_it() {
        let g = grant("namespace", Some(&["corp", "fin"]), None);
        assert!(sees(&g, &["corp"]), "above");
        assert!(sees(&g, &["corp", "fin"]), "on");
        assert!(sees(&g, &["corp", "fin", "q1"]), "under");
        assert!(!sees(&g, &["corp", "hr"]), "a sibling");
        assert!(!sees(&g, &["sales"]), "another top level");
        assert!(!sees(&g, &["fin"]), "the same name at another level");
    }

    #[test]
    fn a_table_grant_sees_only_the_way_to_its_table() {
        let g = grant("table", Some(&["corp", "fin"]), Some("ledger"));
        assert!(sees(&g, &["corp"]));
        assert!(sees(&g, &["corp", "fin"]));
        assert!(
            !sees(&g, &["corp", "fin", "q1"]),
            "under the table's namespace"
        );
        assert!(!sees(&g, &["corp", "hr"]));
    }

    #[test]
    fn a_catalog_grant_sees_everything() {
        let g = grant("catalog", None, None);
        assert!(sees(&g, &["anything", "at", "all"]));
    }
}
