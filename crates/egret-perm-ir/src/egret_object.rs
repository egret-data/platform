//! EO1: the pure, target-neutral representation of egret's object securables
//! and the mapping of `egret_core::rbac` permission keys onto the common grant
//! vocabulary. Additive and unused by any existing code path: no I/O, no
//! migration, no compiler or runtime wiring (that is EO2–EO4). See
//! docs/en/specs/2026-11-18-gov-eo0-egret-objects-decomposition-design.md.

use crate::statement::Privilege;
use egret_core::rbac;
use uuid::Uuid;

/// An egret object kind that is a securable (EO-D6). The string spellings are
/// the ones the control plane already uses in `egret-bi::shares` and the
/// Data API crate, so they line up with the Cedar `Object` id `"{kind}:{uuid}"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Dashboard,
    Chart,
    Dataset,
    Folder,
    SavedQuery,
    App,
    DataApiEndpoint,
}

impl ObjectKind {
    /// Every kind, for exhaustive tests and iteration.
    pub const ALL: [ObjectKind; 7] = [
        ObjectKind::Dashboard,
        ObjectKind::Chart,
        ObjectKind::Dataset,
        ObjectKind::Folder,
        ObjectKind::SavedQuery,
        ObjectKind::App,
        ObjectKind::DataApiEndpoint,
    ];

    /// The control-plane string spelling of this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectKind::Dashboard => "dashboard",
            ObjectKind::Chart => "chart",
            ObjectKind::Dataset => "dataset",
            ObjectKind::Folder => "folder",
            ObjectKind::SavedQuery => "saved_query",
            ObjectKind::App => "app",
            ObjectKind::DataApiEndpoint => "data_api_endpoint",
        }
    }

    /// Parse a control-plane kind string; `None` if it is not a securable kind.
    pub fn from_str_opt(s: &str) -> Option<ObjectKind> {
        ObjectKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// The parent of a securable in the egret-object tree. A `Workspace` parent
/// means the node hangs directly under its workspace (un-foldered objects and
/// top-level folders); a `Folder` parent means it is nested in that folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Parent {
    Folder(Uuid),
    Workspace(Uuid),
}

/// A node in the egret-object securable tree (EO0):
/// `Platform -> Workspace -> Folder* -> Object`. Looser than the data tree:
/// folders nest and objects may or may not be foldered. This is a pure shape;
/// owner and grant resolution land in later sub-projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EgretSecurable {
    Platform,
    Workspace { id: Uuid },
    Folder { id: Uuid, parent: Parent },
    Object { kind: ObjectKind, id: Uuid, parent: Parent },
}

impl EgretSecurable {
    /// The securable's parent. `None` for the platform root and for a
    /// workspace (whose parent is the platform; there is no `Parent::Platform`).
    pub fn parent(&self) -> Option<Parent> {
        match self {
            EgretSecurable::Platform | EgretSecurable::Workspace { .. } => None,
            EgretSecurable::Folder { parent, .. } | EgretSecurable::Object { parent, .. } => {
                Some(*parent)
            }
        }
    }
}

/// Where a permission key sits in the one grant model (EO-D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMapping {
    /// An object-data grant on a securable of this kind.
    ObjectGrant { kind: ObjectKind, privilege: Privilege },
    /// A typed privilege on the `egret-cedar` enforcement point (an
    /// entitlement, not a securable grant): engine/catalog/workspace/etc.
    /// capabilities and non-data object verbs (`certify`, `invoke`).
    TypedEpPrivilege,
    /// A non-delegatable admin-graph action the PDP `forbid`s for everyone but
    /// `platform_admin`. Never a grant. These are `rbac::ADMIN_GRAPH_ACTIONS`,
    /// which are not in `rbac::PERMISSIONS`.
    AdminGraphForbid,
}

/// Classify an `egret_core::rbac` permission key (either a `PERMISSIONS` key or
/// an `ADMIN_GRAPH_ACTIONS` key) into the one grant model. `None` for a string
/// that is neither — callers treat an unknown key as un-mappable (fail closed).
pub fn classify(key: &str) -> Option<PermissionMapping> {
    // Object-data grants: the only keys that become securable grants (EO-D3).
    let object_grant = |kind: ObjectKind, privilege: Privilege| {
        Some(PermissionMapping::ObjectGrant { kind, privilege })
    };
    match key {
        "dashboard:read" => return object_grant(ObjectKind::Dashboard, Privilege::Read),
        "dashboard:manage" => return object_grant(ObjectKind::Dashboard, Privilege::Manage),
        "chart:read" => return object_grant(ObjectKind::Chart, Privilege::Read),
        "chart:manage" => return object_grant(ObjectKind::Chart, Privilege::Manage),
        "dataset:read" => return object_grant(ObjectKind::Dataset, Privilege::Read),
        "dataset:manage" => return object_grant(ObjectKind::Dataset, Privilege::Manage),
        "saved_query:read" => return object_grant(ObjectKind::SavedQuery, Privilege::Read),
        "saved_query:manage" => return object_grant(ObjectKind::SavedQuery, Privilege::Manage),
        "app:read" => return object_grant(ObjectKind::App, Privilege::Read),
        "app:write" => return object_grant(ObjectKind::App, Privilege::Write),
        "app:manage" => return object_grant(ObjectKind::App, Privilege::Manage),
        "data_api_endpoint:read" => {
            return object_grant(ObjectKind::DataApiEndpoint, Privilege::Read)
        }
        "data_api_endpoint:manage" => {
            return object_grant(ObjectKind::DataApiEndpoint, Privilege::Manage)
        }
        _ => {}
    }
    // Admin-graph actions are forbids, never grants. These are not in
    // `PERMISSIONS`; check them explicitly.
    if rbac::is_admin_graph(key) {
        return Some(PermissionMapping::AdminGraphForbid);
    }
    // Every other known permission is a typed EP privilege (EO-D3): engine,
    // catalog, storage, secret, policy, tag, workspace, user, group, role,
    // service_account, workflow, alert, query, and the non-data object verbs
    // (certify, invoke).
    if rbac::is_known(key) {
        return Some(PermissionMapping::TypedEpPrivilege);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn object_kind_str_round_trips() {
        for k in ObjectKind::ALL {
            assert_eq!(ObjectKind::from_str_opt(k.as_str()), Some(k));
        }
        assert_eq!(ObjectKind::from_str_opt("notebook"), None);
        assert_eq!(ObjectKind::from_str_opt("workflow"), None);
        assert_eq!(ObjectKind::from_str_opt(""), None);
    }

    #[test]
    fn securable_parents() {
        let ws = Uuid::from_u128(1);
        let folder = Uuid::from_u128(2);
        let dash = Uuid::from_u128(3);

        assert_eq!(EgretSecurable::Platform.parent(), None);
        // A workspace's parent is the platform root, reported as None.
        assert_eq!(EgretSecurable::Workspace { id: ws }.parent(), None);
        assert_eq!(
            EgretSecurable::Folder { id: folder, parent: Parent::Workspace(ws) }.parent(),
            Some(Parent::Workspace(ws)),
        );
        assert_eq!(
            EgretSecurable::Object {
                kind: ObjectKind::Dashboard,
                id: dash,
                parent: Parent::Folder(folder),
            }
            .parent(),
            Some(Parent::Folder(folder)),
        );
    }

    #[test]
    fn classify_object_data_grants() {
        use ObjectKind::*;
        use Privilege::*;
        let cases: &[(&str, ObjectKind, Privilege)] = &[
            ("dashboard:read", Dashboard, Read),
            ("dashboard:manage", Dashboard, Manage),
            ("chart:read", Chart, Read),
            ("chart:manage", Chart, Manage),
            ("dataset:read", Dataset, Read),
            ("dataset:manage", Dataset, Manage),
            ("saved_query:read", SavedQuery, Read),
            ("saved_query:manage", SavedQuery, Manage),
            ("app:read", App, Read),
            ("app:write", App, Write),
            ("app:manage", App, Manage),
            ("data_api_endpoint:read", DataApiEndpoint, Read),
            ("data_api_endpoint:manage", DataApiEndpoint, Manage),
        ];
        assert_eq!(cases.len(), 13);
        for (key, kind, priv_) in cases {
            assert_eq!(
                classify(key),
                Some(PermissionMapping::ObjectGrant { kind: *kind, privilege: *priv_ }),
                "key {key}",
            );
        }
    }

    #[test]
    fn every_permissions_key_is_classified() {
        // Count guard (chokepoint): adding an rbac permission trips this until
        // the author classifies the new key as an object grant or a typed EP
        // privilege. If you add a key and this fails, update classify() and
        // this count, deciding deliberately which bucket the key belongs in.
        const OBJECT_GRANT_KEYS: usize = 13;

        let mut object_grants = 0usize;
        for p in rbac::PERMISSIONS {
            match classify(p.key) {
                Some(PermissionMapping::ObjectGrant { .. }) => object_grants += 1,
                Some(PermissionMapping::TypedEpPrivilege) => {}
                other => panic!("rbac key {} classified as {other:?}", p.key),
            }
        }
        assert_eq!(object_grants, OBJECT_GRANT_KEYS, "object-grant key count drifted");
    }

    #[test]
    fn admin_graph_actions_are_forbids() {
        for key in rbac::ADMIN_GRAPH_ACTIONS {
            assert_eq!(
                classify(key),
                Some(PermissionMapping::AdminGraphForbid),
                "admin-graph key {key}",
            );
        }
    }

    #[test]
    fn non_data_object_verbs_are_typed() {
        for key in ["dashboard:certify", "dataset:certify", "data_api_endpoint:invoke"] {
            assert_eq!(classify(key), Some(PermissionMapping::TypedEpPrivilege), "key {key}");
        }
    }

    #[test]
    fn unknown_key_is_none() {
        assert_eq!(classify("nonsense:frobnicate"), None);
        assert_eq!(classify(""), None);
    }
}
