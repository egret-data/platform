//! EO1: the pure, target-neutral representation of egret's object securables
//! and the mapping of `egret_core::rbac` permission keys onto the common grant
//! vocabulary. Additive and unused by any existing code path: no I/O, no
//! migration, no compiler or runtime wiring (that is EO2–EO4). See
//! docs/en/specs/2026-11-18-gov-eo0-egret-objects-decomposition-design.md.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_kind_str_round_trips() {
        for k in ObjectKind::ALL {
            assert_eq!(ObjectKind::from_str_opt(k.as_str()), Some(k));
        }
        assert_eq!(ObjectKind::from_str_opt("notebook"), None);
        assert_eq!(ObjectKind::from_str_opt("workflow"), None);
        assert_eq!(ObjectKind::from_str_opt(""), None);
    }
}
