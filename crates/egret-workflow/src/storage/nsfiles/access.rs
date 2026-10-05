//! THE access decision for namespace files (W9b spec §4.6, §5).
//!
//! [`authorize`] is the only code that decides whether a namespace may be
//! read, written or purged. Every store function takes the [`Grant`] it
//! returns, so an unauthorised call does not type-check.
//!
//! [`check_files_segment`] is the `_files` rule: publish (`spec::validate`)
//! refuses a flow whose namespace breaks it, and [`check_namespace`] (which
//! `authorize` and the URI parse call) refuses a target or a running flow's
//! namespace that does -- the save-time refusal and its run-time twin are the
//! same function. [`check_namespace`] also refuses an empty segment (`a..b`,
//! `a.`), which publish still accepts: that rule is namespace files' own (the
//! user's ruling, 2026-10-05, W9b Task 2 fix round 1 M3).

use uuid::Uuid;

use crate::runners::files::NAMESPACE_FILES_SEGMENT;
use crate::spec::validate::is_valid_namespace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Purge,
}

#[derive(Clone, Copy, Debug)]
pub enum Actor<'a> {
    /// A running execution's flow; `namespace` comes from the execution (the
    /// flow row), never from a URI.
    Flow { namespace: &'a str },
    /// An API route that has ALREADY checked the caller's permission:
    /// `workflow:read` for Read, `workflow:manage` for Write/Purge (route
    /// tests pin both, Task 8).
    Api,
}

/// Proof that [`authorize`] said yes. Fields private: only `authorize` builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    workspace: Uuid,
    namespace: String,
    access: Access,
}

impl Grant {
    /// The caller's workspace (the execution's, or the route's `{ws}`).
    pub fn workspace(&self) -> Uuid {
        self.workspace
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn access(&self) -> Access {
        self.access
    }
}

/// The namespace rule of every namespace-file operation: `is_valid_namespace`,
/// then no empty segment (`a..b`, `a.`: its key would hold `//` and no URI
/// could name its files), then [`check_files_segment`]. [`authorize`] and
/// `storage::uri::parse_any` call it; publish calls only
/// [`check_files_segment`], so a namespace with an empty segment still
/// publishes (as before W9b) but can hold no namespace file.
pub fn check_namespace(namespace: &str) -> Result<(), String> {
    if !is_valid_namespace(namespace) {
        return Err(format!(
            "namespace '{namespace}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long"
        ));
    }
    if namespace.split('.').any(str::is_empty) {
        return Err(format!(
            "namespace '{namespace}' has an empty segment ('..', or a '.' at its start or \
             end); a namespace that holds namespace files names every segment"
        ));
    }
    check_files_segment(namespace)
}

/// No `_files` segment: publish (validate.rs) and, through
/// [`check_namespace`], every namespace-file operation call it.
pub fn check_files_segment(namespace: &str) -> Result<(), String> {
    if namespace.split('.').any(|s| s == NAMESPACE_FILES_SEGMENT) {
        return Err(format!(
            "namespace '{namespace}' has a segment '{NAMESPACE_FILES_SEGMENT}', which marks \
             namespace files in a storage URI; no namespace may have it"
        ));
    }
    Ok(())
}

/// Read: any namespace of `workspace`. Write: `target == own` exactly. Purge:
/// `target == own` or `target.starts_with(own + ".")`. Api: any access (the
/// permission was checked by the route).
///
/// A refusal names the namespace and the rule, never whether a file exists there.
pub fn authorize(
    workspace: Uuid,
    actor: Actor<'_>,
    target: &str,
    access: Access,
) -> Result<Grant, String> {
    check_namespace(target)?;
    if let Actor::Flow { namespace: own } = actor {
        // A flow published before the `_files` rule.
        check_namespace(own).map_err(|e| format!("the running flow's {e}"))?;
        match access {
            Access::Read => {}
            Access::Write => {
                if target != own {
                    return Err(format!(
                        "a flow of namespace '{own}' may not write namespace files of \
                         namespace '{target}': a flow writes only its own namespace's files"
                    ));
                }
            }
            Access::Purge => {
                let child = target
                    .strip_prefix(own)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'));
                if !child {
                    return Err(format!(
                        "a flow of namespace '{own}' may not purge namespace files of \
                         namespace '{target}': a flow purges only its own namespace's files \
                         and its child namespaces' ('{own}.…')"
                    ));
                }
            }
        }
    }
    Ok(Grant {
        workspace,
        namespace: target.to_string(),
        access,
    })
}

/// For store functions: `Err(Error::Internal)` when a write is attempted with
/// a Read grant -- egret's bug, not the author's. A Read is satisfied by any
/// grant (`authorize` only grants Write or Purge where Read is granted too);
/// Write and Purge need a grant of exactly that access (a Purge grant reaches
/// child namespaces a Write may not).
pub(crate) fn require(grant: &Grant, access: Access) -> egret_core::Result<()> {
    if access == Access::Read || grant.access == access {
        return Ok(());
    }
    Err(egret_core::Error::Internal(anyhow::anyhow!(
        "egret asked for {access:?} access to namespace '{}' holding a {:?} grant",
        grant.namespace,
        grant.access
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Access; 3] = [Access::Read, Access::Write, Access::Purge];

    fn flow(own: &str) -> Actor<'_> {
        Actor::Flow { namespace: own }
    }

    #[test]
    fn a_flow_reads_any_namespace_of_its_workspace() {
        let ws = Uuid::new_v4();
        for target in ["a", "a.b", "other.ns", "z9-x_y"] {
            let g = authorize(ws, flow("a.b"), target, Access::Read).unwrap();
            assert_eq!((g.namespace(), g.access()), (target, Access::Read));
        }
    }

    #[test]
    fn a_flow_writes_only_its_own_namespace_compared_exactly() {
        let ws = Uuid::new_v4();
        assert!(authorize(ws, flow("a"), "a", Access::Write).is_ok());
        assert!(authorize(ws, flow("a.b"), "a.b", Access::Write).is_ok());
        for (own, target) in [("a", "a.b"), ("a.b", "a"), ("a.b", "a.bc"), ("a", "b")] {
            let err = authorize(ws, flow(own), target, Access::Write).unwrap_err();
            assert!(
                err.contains(&format!("'{target}'")) && err.contains("writes only its own"),
                "{own} -> {target}: {err}"
            );
        }
        // "A" vs "a": an uppercase namespace is not a namespace at all.
        assert!(authorize(ws, flow("a"), "A", Access::Write).is_err());
    }

    #[test]
    fn a_flow_purges_its_own_and_child_namespaces_on_a_dot_boundary() {
        let ws = Uuid::new_v4();
        for target in ["a.b", "a.b.c", "a.b.c.d"] {
            assert!(
                authorize(ws, flow("a.b"), target, Access::Purge).is_ok(),
                "{target}"
            );
        }
        for target in ["a.bc", "a", "a.c", "b.a.b", "a.b-c"] {
            let err = authorize(ws, flow("a.b"), target, Access::Purge).unwrap_err();
            assert!(
                err.contains(&format!("'{target}'")) && err.contains("purges only"),
                "{target}: {err}"
            );
        }
    }

    #[test]
    fn a_files_segment_namespace_is_refused_for_every_access_and_actor() {
        let ws = Uuid::new_v4();
        for bad in ["a._files", "a._files.b", "_files.a.b"] {
            for access in ALL {
                for actor in [Actor::Api, flow("a"), flow(bad)] {
                    let err = authorize(ws, actor, bad, access).unwrap_err();
                    assert!(err.contains(bad), "{bad}: {err}");
                }
                // As the running flow's own namespace, against a good target.
                let err = authorize(ws, flow(bad), "a", access).unwrap_err();
                assert!(err.contains(bad) && err.contains("running flow"), "{err}");
            }
        }
        // `a._files` passes the pattern: only the segment rule refuses it.
        assert!(is_valid_namespace("a._files"));
        let err = authorize(ws, Actor::Api, "a._files", Access::Read).unwrap_err();
        assert!(err.contains("segment '_files'"), "{err}");
        // `_files` alone is refused by the pattern, named once.
        let err = check_namespace("_files").unwrap_err();
        assert!(err.contains("'_files' must match"), "{err}");
        // The controls: `_files` inside a segment is not the segment.
        for ok in ["a.files", "a.x_files", "a._filesx", "a.b_files.c"] {
            assert!(check_namespace(ok).is_ok(), "{ok}");
        }
    }

    /// M3: an empty segment is refused by every namespace-file operation,
    /// as a target and as the running flow's own namespace; `a.` is no
    /// longer a child for Purge. Publish is unchanged (validate.rs's
    /// `an_empty_namespace_segment_still_publishes`).
    #[test]
    fn an_empty_namespace_segment_is_refused_by_namespace_files_only() {
        let ws = Uuid::new_v4();
        for bad in ["a..b", "a.", "a.b..c", "a.b."] {
            assert!(is_valid_namespace(bad), "{bad}: the pattern accepts it");
            assert!(check_files_segment(bad).is_ok(), "{bad}: publish's rule");
            for access in ALL {
                for actor in [Actor::Api, flow("a")] {
                    let err = authorize(ws, actor, bad, access).unwrap_err();
                    assert!(
                        err.contains(&format!("namespace '{bad}'"))
                            && err.contains("empty segment"),
                        "{bad}: {err}"
                    );
                }
                let err = authorize(ws, flow(bad), "a", access).unwrap_err();
                assert!(
                    err.contains("running flow") && err.contains("empty segment"),
                    "{err}"
                );
            }
        }
        // `.a` is refused by the pattern, named once.
        let err = check_namespace(".a").unwrap_err();
        assert!(
            err.contains("must match") && !err.contains("empty segment"),
            "{err}"
        );
        // Before M3 `a.` was a child of `a` for Purge.
        assert!(authorize(ws, flow("a"), "a.", Access::Purge).is_err());
        assert!(
            authorize(ws, flow("a"), "a.b", Access::Purge).is_ok(),
            "the control"
        );
    }

    #[test]
    fn an_invalid_namespace_is_refused_naming_it() {
        let ws = Uuid::new_v4();
        for bad in ["", "A", "a/b", "-a", "a b", &"a".repeat(151)] {
            for access in ALL {
                let err = authorize(ws, Actor::Api, bad, access).unwrap_err();
                assert!(err.contains(&format!("namespace '{bad}'")), "{err}");
                let err = authorize(ws, flow(bad), "a", access).unwrap_err();
                assert!(err.contains(&format!("namespace '{bad}'")), "{err}");
            }
        }
        assert!(authorize(ws, Actor::Api, &"a".repeat(150), Access::Read).is_ok());
    }

    #[test]
    fn a_refusal_never_mentions_existence() {
        let ws = Uuid::new_v4();
        let mut refusals = 0;
        for (own, target) in [("a", "b"), ("a.b", "a.bc"), ("a", "a._files"), ("A", "a")] {
            for access in ALL {
                if let Err(e) = authorize(ws, flow(own), target, access) {
                    refusals += 1;
                    let lower = e.to_lowercase();
                    for word in ["exist", "found", "missing"] {
                        assert!(!lower.contains(word), "{e:?} says {word:?}");
                    }
                }
            }
        }
        assert_eq!(refusals, 2 + 2 + 3 + 3, "every refusal was examined");
    }

    #[test]
    fn the_grant_carries_the_callers_workspace() {
        let (ws, other) = (Uuid::new_v4(), Uuid::new_v4());
        for actor in [Actor::Api, flow("a")] {
            let g = authorize(ws, actor, "a", Access::Write).unwrap();
            assert_eq!(g.workspace(), ws);
            assert_ne!(g.workspace(), other);
        }
    }

    #[test]
    fn the_api_may_do_anything_to_a_valid_namespace() {
        for access in ALL {
            assert!(authorize(Uuid::new_v4(), Actor::Api, "x.y", access).is_ok());
        }
    }

    #[test]
    fn require_refuses_a_write_with_a_read_grant() {
        let ws = Uuid::new_v4();
        let read = authorize(ws, Actor::Api, "a", Access::Read).unwrap();
        let write = authorize(ws, Actor::Api, "a", Access::Write).unwrap();
        let purge = authorize(ws, Actor::Api, "a", Access::Purge).unwrap();
        assert!(require(&read, Access::Read).is_ok());
        assert!(matches!(
            require(&read, Access::Write),
            Err(egret_core::Error::Internal(_))
        ));
        assert!(require(&read, Access::Purge).is_err());
        assert!(require(&write, Access::Write).is_ok());
        assert!(require(&write, Access::Read).is_ok());
        assert!(require(&write, Access::Purge).is_err());
        assert!(require(&purge, Access::Purge).is_ok());
        assert!(require(&purge, Access::Write).is_err());
    }
}
