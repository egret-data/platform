//! SP5b: maps the embedded catalog's grant shape onto `perm.grants` scope and
//! action columns. Pure: no database, no enforcement. Later tasks attach the
//! principal and `catalog_id` to build full rows.

use crate::model::Action;

/// A catalog securable's place under a `catalog_id` in `perm.grants`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedScope {
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    /// A single Iceberg namespace level containing `.` cannot be told apart
    /// from the dot-joined multi-level form, so it is refused.
    #[error("namespace level {0:?} contains a '.'")]
    NamespaceLevelHasDot(String),
    #[error("unknown securable type {0:?}")]
    UnknownSecurableType(String),
    #[error("unknown privilege {0:?}")]
    UnknownPrivilege(String),
}

fn join_namespace(namespace: Option<&[String]>) -> Result<Option<String>, MapError> {
    let Some(levels) = namespace.filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if let Some(bad) = levels.iter().find(|l| l.contains('.')) {
        return Err(MapError::NamespaceLevelHasDot(bad.clone()));
    }
    Ok(Some(levels.join(".")))
}

/// `(securable_type, namespace, table)` -> perm scope under a catalog.
/// Multi-level namespaces are dot-joined (`["sales","eu"]` -> `sales.eu`).
pub fn map_securable(
    securable_type: &str,
    namespace: Option<&[String]>,
    table: Option<&str>,
) -> Result<MappedScope, MapError> {
    match securable_type {
        "catalog" => Ok(MappedScope {
            schema_name: None,
            table_name: None,
        }),
        "namespace" => Ok(MappedScope {
            schema_name: join_namespace(namespace)?,
            table_name: None,
        }),
        "table" => Ok(MappedScope {
            schema_name: join_namespace(namespace)?,
            table_name: table.map(str::to_owned),
        }),
        other => Err(MapError::UnknownSecurableType(other.to_owned())),
    }
}

/// Catalog privilege -> the perm action set stored, faithful to the catalog's
/// `grant_match`: read={read}, write={read,write},
/// manage={read,write,create,manage}. Case-insensitive.
pub fn map_privilege(privilege: &str) -> Result<Vec<Action>, MapError> {
    match privilege.to_ascii_lowercase().as_str() {
        "read" => Ok(vec![Action::Read]),
        "write" => Ok(vec![Action::Read, Action::Write]),
        "manage" => Ok(vec![
            Action::Read,
            Action::Write,
            Action::Create,
            Action::Manage,
        ]),
        _ => Err(MapError::UnknownPrivilege(privilege.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }
    fn scope(s: Option<&str>, t: Option<&str>) -> MappedScope {
        MappedScope {
            schema_name: s.map(Into::into),
            table_name: t.map(Into::into),
        }
    }

    #[test]
    fn catalog_has_no_schema_or_table() {
        assert_eq!(map_securable("catalog", None, None), Ok(scope(None, None)));
    }

    #[test]
    fn namespace_levels_are_dot_joined() {
        assert_eq!(
            map_securable("namespace", Some(&ns(&["sales"])), None),
            Ok(scope(Some("sales"), None))
        );
        assert_eq!(
            map_securable("namespace", Some(&ns(&["sales", "eu"])), None),
            Ok(scope(Some("sales.eu"), None))
        );
    }

    #[test]
    fn table_carries_its_dot_joined_namespace() {
        assert_eq!(
            map_securable("table", Some(&ns(&["sales", "eu"])), Some("orders")),
            Ok(scope(Some("sales.eu"), Some("orders")))
        );
    }

    #[test]
    fn a_namespace_level_with_a_dot_is_refused() {
        assert_eq!(
            map_securable("namespace", Some(&ns(&["a.b"])), None),
            Err(MapError::NamespaceLevelHasDot("a.b".into()))
        );
        assert_eq!(
            map_securable("table", Some(&ns(&["ok", "a.b"])), Some("t")),
            Err(MapError::NamespaceLevelHasDot("a.b".into()))
        );
    }

    #[test]
    fn unknown_securable_is_refused() {
        assert_eq!(
            map_securable("view", None, None),
            Err(MapError::UnknownSecurableType("view".into()))
        );
    }

    #[test]
    fn privileges_map_to_their_action_sets() {
        assert_eq!(map_privilege("read"), Ok(vec![Action::Read]));
        assert_eq!(
            map_privilege("write"),
            Ok(vec![Action::Read, Action::Write])
        );
        assert_eq!(
            map_privilege("manage"),
            Ok(vec![
                Action::Read,
                Action::Write,
                Action::Create,
                Action::Manage
            ])
        );
        assert_eq!(map_privilege("READ"), Ok(vec![Action::Read]));
        assert_eq!(map_privilege("Manage").map(|v| v.len()), Ok(4));
        assert_eq!(
            map_privilege("bogus"),
            Err(MapError::UnknownPrivilege("bogus".into()))
        );
    }
}
