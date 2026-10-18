//! The curated engine capability catalogue (F11, spec §2, decisions E1–E4).
//!
//! An engine grant names one capability of this table and the concrete values
//! its resource keys take. The table is curated, not generated from the
//! service-defs: near-root capabilities (a `trinouser` wildcard, Trino roles,
//! StarRocks `system` `operate`/`node`/`grant`/…) are not grantable and stay
//! admin-only through the seeded `egret-admin - <chain>` policies (E1, E4);
//! [`NOT_GRANTABLE`] says so with the reason, so the API can show it.
//!
//! The ids are also the `CHECK` list of `perm.engine_grants.capability`
//! (`migrations/0195_perm_engine_grants.sql`): one fact in two places, held
//! together by `the_ids_equal_the_migrations_check_list` below.
//!
//! **Read the spec's corrections before adding a row.** Trino's `queryid`
//! `execute` is the right to run any query, not to kill another user's (C1);
//! function execution is `schemafunction`, not the level-10 `function` the
//! plugin never reads (C2); StarRocks resource groups have no usage privilege
//! (C3); StarRocks `operate` is near-root (C4).

use egret_perm::EngineType;

/// What one resource key's value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// A `users.username`, resolved and stored by user id (spec E11, E21).
    Username,
    /// A property, schema or procedure name: `^[A-Za-z0-9_][A-Za-z0-9_.$-]*$`.
    Identifier,
    /// A catalog the engine's profile reaches.
    Catalog,
    /// A StarRocks `Function.getSignature()`, e.g. `my_udf(INT,VARCHAR)`.
    FunctionSignature,
}

impl ValueKind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ValueKind::Username => "username",
            ValueKind::Identifier => "identifier",
            ValueKind::Catalog => "catalog",
            ValueKind::FunctionSignature => "function_signature",
        }
    }
}

/// One grantable capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// `"trino.impersonate_user"`, …
    pub id: &'static str,
    pub engine: EngineType,
    pub label: &'static str,
    pub description: &'static str,
    /// The Ranger resource keys in hierarchy order, each with the kind of value
    /// it takes; `None` = the fixed value `"*"`, supplied by the compiler.
    pub resources: &'static [(&'static str, Option<ValueKind>)],
    /// The one Ranger access type.
    pub access: &'static str,
}

impl Capability {
    /// The value kinds a grant supplies, in key order (fixed keys skipped).
    pub fn value_kinds(&self) -> impl Iterator<Item = ValueKind> + '_ {
        self.resources.iter().filter_map(|(_, k)| *k)
    }

    /// Whether this capability's target is a user (stored by id, E21).
    pub fn is_impersonation(&self) -> bool {
        self.value_kinds().any(|k| k == ValueKind::Username)
    }
}

/// An engine-level permission egret deliberately does not offer, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotGrantable {
    pub engine: EngineType,
    /// A resource key, or `"<key> <access>"`.
    pub what: &'static str,
    pub why: &'static str,
}

use ValueKind::{Catalog, FunctionSignature, Identifier, Username};

/// The catalogue, in spec §2's order. Labels state the consequences.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        id: "trino.impersonate_user",
        engine: EngineType::Trino,
        label: "Impersonate a user",
        description: "Act as this user on this engine: run queries with their permissions, \
                      and see and kill their queries (one Ranger permission covers all three).",
        resources: &[("trinouser", Some(Username))],
        access: "impersonate",
    },
    Capability {
        id: "trino.set_system_session_property",
        engine: EngineType::Trino,
        label: "Set a system session property",
        description: "Set this system session property for the grantee's own sessions.",
        resources: &[("systemproperty", Some(Identifier))],
        access: "alter",
    },
    Capability {
        id: "trino.set_catalog_session_property",
        engine: EngineType::Trino,
        label: "Set a catalog session property",
        description: "Set this session property of this catalog for the grantee's own sessions.",
        resources: &[
            ("catalog", Some(Catalog)),
            ("sessionproperty", Some(Identifier)),
        ],
        access: "alter",
    },
    Capability {
        id: "trino.execute_procedure",
        engine: EngineType::Trino,
        label: "Execute a procedure",
        description: "Call this procedure. It runs with what the procedure itself checks; some \
                      connector procedures change data without a further check. Calling it also \
                      needs access to the catalog, which a data grant gives.",
        resources: &[
            ("catalog", Some(Catalog)),
            ("schema", Some(Identifier)),
            ("procedure", Some(Identifier)),
        ],
        access: "execute",
    },
    Capability {
        id: "trino.execute_function",
        engine: EngineType::Trino,
        label: "Execute a catalog function",
        description: "Call this catalog function. Built-in and inline functions are never \
                      checked; calling it also needs access to the catalog, which a data grant \
                      gives.",
        resources: &[
            ("catalog", Some(Catalog)),
            ("schema", Some(Identifier)),
            ("schemafunction", Some(Identifier)),
        ],
        access: "execute",
    },
    Capability {
        id: "starrocks.impersonate_user",
        engine: EngineType::StarRocks,
        label: "Impersonate a user",
        description: "Act as this user on this engine (`EXECUTE AS`): run statements with their \
                      permissions.",
        resources: &[("user", Some(Username))],
        access: "impersonate",
    },
    Capability {
        id: "starrocks.use_global_function",
        engine: EngineType::StarRocks,
        label: "Use a global function",
        description: "Call this global function, named by its signature, e.g. \
                      `my_udf(INT,VARCHAR)`.",
        resources: &[("global_function", Some(FunctionSignature))],
        access: "usage",
    },
    Capability {
        id: "starrocks.manage_blacklist",
        engine: EngineType::StarRocks,
        label: "Manage the SQL blacklist",
        description: "Add and remove SQL blacklist entries. This affects every user of the \
                      engine: it can block queries and take nodes out of scheduling.",
        resources: &[("system", None)],
        access: "blacklist",
    },
];

const NOT_REQUESTED: &str =
    "not offered yet (not requested); it can be added later as a catalogue row and its tests";

/// Spec §2 "Not offered", with the reason the API returns (E4).
pub const NOT_GRANTABLE: &[NotGrantable] = &[
    NotGrantable {
        engine: EngineType::Trino,
        what: "trinouser impersonate",
        why: "with a wildcard or pattern (any user): not grantable, it would let the grantee \
              become anyone, admins included. Impersonating ONE named user is the grantable \
              `trino.impersonate_user` capability",
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "role",
        why: "managing Trino roles is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "sysinfo write_sysinfo",
        why: "writing system information is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "queryid execute",
        why: "this is the right to run any query at all, which every user already has; \
              viewing and killing another user's queries is impersonating them",
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "sysinfo read_sysinfo",
        why: NOT_REQUESTED,
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "schemafunction create",
        why: NOT_REQUESTED,
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "schemafunction drop",
        why: NOT_REQUESTED,
    },
    NotGrantable {
        engine: EngineType::Trino,
        what: "schemafunction show",
        why: NOT_REQUESTED,
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system node",
        why: "adding and removing nodes is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system plugin",
        why: "installing plugins runs code on the engine; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system file",
        why: "managing files on the engine is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system grant",
        why: "granting privileges is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system operate",
        why: "near-root: it gates engine configuration, running scripts on the frontend, \
              fail points, skipping committed transactions and killing other users' \
              connections",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system repository",
        why: "managing backup repositories is near-root; it stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system create resource",
        why: "creating engine-wide objects stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system create resource group",
        why: "creating engine-wide objects stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system create external catalog",
        why: "creating engine-wide objects stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system create global function",
        why: "creating engine-wide objects stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "system create storage volume",
        why: "creating engine-wide objects stays with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "storage_volume",
        why: "storage volumes are engine infrastructure; they stay with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "resource",
        why: "external resources are engine infrastructure; they stay with engine admins",
    },
    NotGrantable {
        engine: EngineType::StarRocks,
        what: "resource_group",
        why: "a resource group has no usage privilege: it is checked only to alter or drop \
              it, which stays with engine admins",
    },
];

/// The catalogue entry `id`, or `None`.
pub fn capability(id: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.id == id)
}

/// The shape rules of spec §2 (no database): the value count, non-empty, at
/// most 255 characters, no surrounding whitespace, none of `*?{}`, and the
/// per-kind pattern. Returns the normalised values: lower-cased (both engines
/// match these keys case-insensitively) except a function signature, whose
/// argument types StarRocks prints upper-case.
pub fn check_values(cap: &Capability, values: &[String]) -> Result<Vec<String>, String> {
    let kinds: Vec<ValueKind> = cap.value_kinds().collect();
    if values.len() != kinds.len() {
        return Err(format!(
            "{} takes {} value(s) ({}), got {}",
            cap.id,
            kinds.len(),
            kinds
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            values.len()
        ));
    }
    kinds
        .iter()
        .zip(values)
        .map(|(kind, value)| check_value(*kind, value))
        .collect()
}

/// Ranger's wildcards (`*`, `?`) and macro markers (`{`, `}`).
const FORBIDDEN: &[char] = &['*', '?', '{', '}'];
const MAX_VALUE_LEN: usize = 255;

fn check_value(kind: ValueKind, value: &str) -> Result<String, String> {
    let kind_name = kind.as_str();
    if value.is_empty() {
        return Err(format!("a {kind_name} value must not be empty"));
    }
    if value.chars().count() > MAX_VALUE_LEN {
        return Err(format!(
            "a {kind_name} value is at most {MAX_VALUE_LEN} characters"
        ));
    }
    if value.trim() != value {
        return Err(format!(
            "{value:?}: a {kind_name} value has no leading or trailing whitespace"
        ));
    }
    if value.contains(FORBIDDEN) {
        return Err(format!(
            "{value:?}: values are concrete; `*`, `?`, `{{` and `}}` are Ranger wildcards \
             and macro markers"
        ));
    }
    match kind {
        ValueKind::Username | ValueKind::Catalog => Ok(value.to_lowercase()),
        ValueKind::Identifier => {
            let mut chars = value.chars();
            let first_ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if first_ok && chars.all(|c| c.is_ascii_alphanumeric() || "_.$-".contains(c)) {
                Ok(value.to_lowercase())
            } else {
                Err(format!(
                    "{value:?} is not an identifier (letters, digits and `_`, then also \
                     `.`, `$`, `-`)"
                ))
            }
        }
        ValueKind::FunctionSignature => {
            let shape_ok = value.split_once('(').is_some_and(|(name, rest)| {
                !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && rest.strip_suffix(')').is_some_and(|args| {
                        args.chars().all(|c| {
                            c.is_ascii_uppercase() || c.is_ascii_digit() || "_, .".contains(c)
                        })
                    })
            });
            if shape_ok {
                Ok(value.to_string())
            } else {
                Err(format!(
                    "{value:?} is not a function signature such as `my_udf(INT,VARCHAR)`"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn def(engine: EngineType) -> Value {
        match engine {
            EngineType::Trino => egret_policy::trino_service_def().expect("trino def"),
            EngineType::StarRocks => egret_policy::starrocks_service_def().expect("starrocks def"),
        }
    }

    fn resource<'a>(def: &'a Value, name: &str) -> Option<&'a Value> {
        def["resources"]
            .as_array()
            .expect("resources")
            .iter()
            .find(|r| r["name"] == name)
    }

    fn access_types(def: &Value) -> Vec<String> {
        def["accessTypes"]
            .as_array()
            .expect("accessTypes")
            .iter()
            .map(|a| a["name"].as_str().expect("name").to_string())
            .collect()
    }

    fn restrictions(r: &Value) -> Vec<String> {
        r["accessTypeRestrictions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().expect("restriction").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_catalogue_is_the_eight_rows_of_the_spec_in_order() {
        let ids: Vec<&str> = CAPABILITIES.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "trino.impersonate_user",
                "trino.set_system_session_property",
                "trino.set_catalog_session_property",
                "trino.execute_procedure",
                "trino.execute_function",
                "starrocks.impersonate_user",
                "starrocks.use_global_function",
                "starrocks.manage_blacklist",
            ]
        );
        for c in CAPABILITIES {
            let prefix = match c.engine {
                EngineType::Trino => "trino.",
                EngineType::StarRocks => "starrocks.",
            };
            assert!(
                c.id.starts_with(prefix),
                "{} is filed under the wrong engine",
                c.id
            );
            assert!(!c.label.is_empty() && !c.description.is_empty(), "{}", c.id);
            assert_eq!(capability(c.id), Some(c));
        }
        assert_eq!(capability("trino.kill_any"), None);
    }

    /// Every key is declared by its engine's service-def, the keys form ONE
    /// parent chain in order, the access type is declared, and it is in the
    /// leaf's `accessTypeRestrictions` for every entry but one:
    /// `trino.execute_function`, whose leaf is `schemafunction` (spec C2: the
    /// restriction list omits `execute`, but restrictions only shape default
    /// policies, never evaluation).
    #[test]
    fn every_entry_is_a_parent_chain_of_its_service_def_with_a_declared_access() {
        let mut outside_restrictions = Vec::new();
        for c in CAPABILITIES {
            let d = def(c.engine);
            let mut parent: Option<&str> = None;
            for (key, _) in c.resources {
                let r = resource(&d, key)
                    .unwrap_or_else(|| panic!("{}: `{key}` is not in the service-def", c.id));
                let declared_parent = r["parent"].as_str().filter(|p| !p.is_empty());
                assert_eq!(
                    declared_parent, parent,
                    "{}: `{key}`'s parent is {declared_parent:?}, the entry puts it under {parent:?}",
                    c.id
                );
                parent = Some(key);
            }
            assert!(
                access_types(&d).iter().any(|a| a == c.access),
                "{}: access `{}` is not declared",
                c.id,
                c.access
            );
            let (leaf, _) = c.resources.last().expect("at least one key");
            let leaf_def = resource(&d, leaf).expect("leaf");
            if !restrictions(leaf_def).iter().any(|a| a == c.access) {
                outside_restrictions.push((c.id, *leaf));
            }
        }
        assert_eq!(
            outside_restrictions,
            [("trino.execute_function", "schemafunction")],
            "exactly one entry may sit outside its leaf's restriction list (spec C2)"
        );
    }

    #[test]
    fn no_entry_reaches_a_near_root_or_unread_key() {
        for c in CAPABILITIES {
            for (key, kind) in c.resources {
                assert!(
                    ![
                        "role",
                        "sysinfo",
                        "queryid",
                        "storage_volume",
                        "resource",
                        "resource_group",
                        "function",
                    ]
                    .contains(key),
                    "{} reaches `{key}`",
                    c.id
                );
                if *key == "system" {
                    assert_eq!(c.access, "blacklist", "{}: the only system access", c.id);
                    assert_eq!(*kind, None, "{}: system is the fixed `*`", c.id);
                }
            }
        }
        assert_eq!(
            CAPABILITIES
                .iter()
                .filter(|c| c.resources.iter().any(|(k, _)| *k == "system"))
                .count(),
            1
        );
    }

    /// The one not-grantable row that shares its key and access with a
    /// grantable capability must say it is about the wildcard, so a UI cannot
    /// read it as "impersonation is not grantable".
    #[test]
    fn the_wildcard_impersonation_row_says_it_is_the_wildcard() {
        let row = NOT_GRANTABLE
            .iter()
            .find(|n| n.what == "trinouser impersonate")
            .expect("row");
        assert!(row.why.contains("wildcard"), "{}", row.why);
        assert!(row.why.contains("any user"), "{}", row.why);
        assert!(row.why.contains("trino.impersonate_user"), "{}", row.why);
    }

    /// Each `what` names a key, or a `key access` pair, that the def declares,
    /// so the list cannot name a permission that does not exist.
    #[test]
    fn every_not_grantable_row_names_something_the_defs_declare() {
        assert!(NOT_GRANTABLE.iter().any(|n| n.engine == EngineType::Trino));
        assert!(NOT_GRANTABLE
            .iter()
            .any(|n| n.engine == EngineType::StarRocks));
        assert!(NOT_GRANTABLE.iter().any(|n| n.what == "system operate"));
        for n in NOT_GRANTABLE {
            let d = def(n.engine);
            let (key, access) = match n.what.split_once(' ') {
                Some((k, a)) => (k, Some(a)),
                None => (n.what, None),
            };
            assert!(resource(&d, key).is_some(), "{:?}: no key `{key}`", n);
            if let Some(a) = access {
                assert!(
                    access_types(&d).iter().any(|x| x == a),
                    "{:?}: no access `{a}`",
                    n
                );
            }
            assert!(!n.why.is_empty(), "{n:?}");
        }
    }

    /// The Rust ids and the migration's `CHECK` list are one fact in two places.
    #[test]
    fn the_ids_equal_the_migrations_check_list() {
        const MIGRATION: &str = include_str!("../../../migrations/0195_perm_engine_grants.sql");
        let start = MIGRATION.find("capability IN (").expect("the CHECK") + "capability IN (".len();
        let end = start + MIGRATION[start..].find(')').expect("closing paren");
        let mut in_sql: Vec<&str> = MIGRATION[start..end]
            .split(',')
            .map(|s| s.trim().trim_matches('\''))
            .collect();
        in_sql.sort_unstable();
        let mut in_rust: Vec<&str> = CAPABILITIES.iter().map(|c| c.id).collect();
        in_rust.sort_unstable();
        assert_eq!(in_sql.len(), 8, "parsed {in_sql:?}");
        assert_eq!(in_sql, in_rust);
    }

    fn v(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn cap(id: &str) -> &'static Capability {
        capability(id).expect(id)
    }

    #[test]
    fn check_values_refuses_wildcards_macros_whitespace_and_wrong_counts() {
        let imp = cap("trino.impersonate_user");
        for bad in [
            v(&[]),
            v(&["*"]),
            v(&["al*"]),
            v(&["a?b"]),
            v(&["{USER}"]),
            v(&[" bob"]),
            v(&["bob "]),
            v(&[""]),
            v(&["a", "b"]),
            vec!["x".repeat(256)],
        ] {
            assert!(check_values(imp, &bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(check_values(imp, &v(&["Bob"])), Ok(v(&["bob"])));

        let catalog_prop = cap("trino.set_catalog_session_property");
        assert!(check_values(catalog_prop, &v(&["ice"])).is_err());
        assert_eq!(
            check_values(catalog_prop, &v(&["Ice", "Max_Splits"])),
            Ok(v(&["ice", "max_splits"]))
        );
        assert!(check_values(catalog_prop, &v(&["ice", "bad name"])).is_err());

        let udf = cap("starrocks.use_global_function");
        assert_eq!(
            check_values(udf, &v(&["my_udf(INT,VARCHAR)"])),
            Ok(v(&["my_udf(INT,VARCHAR)"]))
        );
        assert!(check_values(udf, &v(&["my_udf"])).is_err());

        let blacklist = cap("starrocks.manage_blacklist");
        assert_eq!(check_values(blacklist, &[]), Ok(vec![]));
        assert!(check_values(blacklist, &v(&["*"])).is_err());
        assert!(check_values(blacklist, &v(&["x"])).is_err());
    }
}
