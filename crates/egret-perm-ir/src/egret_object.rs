//! EO1: the pure, target-neutral representation of egret's object securables
//! and the mapping of `egret_core::rbac` permission keys onto the common grant
//! vocabulary. Additive and unused by any existing code path: no I/O, no
//! migration, no compiler or runtime wiring (that is EO2–EO4). See
//! docs/en/specs/2026-11-18-gov-eo0-egret-objects-decomposition-design.md.

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
}
