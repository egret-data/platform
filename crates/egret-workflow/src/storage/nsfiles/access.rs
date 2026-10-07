//! THE access decision for namespace files (W9b spec §4.6, §5) and the KV
//! store (W9c spec §4.7, §5).
//!
//! [`authorize`] is the only code that decides whether a namespace may be
//! read, written or purged, for both resources ([`Resource`]): the rule is
//! the same, and the resource changes only the refusal's words. Every store
//! function takes the [`Grant`] it returns and checks its resource and access
//! with [`require`], so an unauthorised call does not type-check and a grant
//! of one resource cannot reach the other.
//!
//! [`check_kv_segment`] is W9c's `_kv` rule, the twin of `_files`: publish
//! refuses it, and [`check_namespace`] refuses it for every operation of
//! either resource.
//!
//! [`check_files_segment`] is the `_files` rule: publish (`spec::validate`)
//! refuses a flow whose namespace breaks it, and [`check_namespace`] (which
//! `authorize` and the URI parse call) refuses a target or a running flow's
//! namespace that does -- the save-time refusal and its run-time twin are the
//! same function. [`check_namespace`] also refuses an empty segment (`a..b`,
//! `a.`), which publish still accepts: that rule is namespace files' own (the
//! user's ruling, 2026-10-05, W9b Task 2 fix round 1 M3).

use std::time::Duration;

use uuid::Uuid;

use crate::runners::files::NAMESPACE_FILES_SEGMENT;
use crate::spec::validate::is_valid_namespace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Purge,
}

/// What a grant reaches: W9b's namespace files or W9c's KV store (W9c spec
/// §4.7). The access rule is the same for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    NamespaceFiles,
    Kv,
}

impl Resource {
    /// "namespace files" / "the KV store": the refusal's noun.
    pub fn noun(self) -> &'static str {
        match self {
            Resource::NamespaceFiles => "namespace files",
            Resource::Kv => "the KV store",
        }
    }

    /// "files" / "KV store": "a flow writes only its own namespace's {own_noun}".
    fn own_noun(self) -> &'static str {
        match self {
            Resource::NamespaceFiles => "files",
            Resource::Kv => "KV store",
        }
    }
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
    /// egret's own background work (the KV expiry sweep, W9c Task 3): any
    /// namespace, any access, no lock wait (it only try-locks). Never built
    /// from a request.
    System,
}

/// Proof that [`authorize`] said yes. Fields private: only `authorize` builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    workspace: Uuid,
    namespace: String,
    resource: Resource,
    access: Access,
    /// How long a writer holding this grant waits for a path's advisory
    /// lock: [`API_LOCK_WAIT`] for [`Actor::Api`], `None` (wait as long as
    /// it takes) for [`Actor::Flow`] (W9b final review I-1, 2026-10-07).
    lock_wait: Option<Duration>,
}

/// The longest an API writer (upload, create directory, delete, move) waits
/// for a path's advisory lock -- held by a move or another write of the same
/// chain -- before it answers 409 "the path is busy" (W9b final review I-1,
/// 2026-10-07). A waiter holds a pool connection, so an unbounded wait lets
/// queued requests starve the pool. A task's writer (`UploadFiles`,
/// `DeleteFiles`, `PurgeFiles`) keeps waiting: it runs under a lease, one per
/// task run, and failing it on a busy path would fail a flow for another
/// writer's timing.
pub const API_LOCK_WAIT: Duration = Duration::from_secs(5);

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

    /// What the grant reaches: namespace files or the KV store.
    pub fn resource(&self) -> Resource {
        self.resource
    }

    /// How long a write under this grant waits for a path's lock; `None`
    /// waits as long as it takes.
    pub fn lock_wait(&self) -> Option<Duration> {
        self.lock_wait
    }

    /// Tests only: the same grant with another lock wait (an API grant's
    /// [`API_LOCK_WAIT`] shortened, so a test measures the refusal fast).
    #[cfg(any(test, feature = "test-util"))]
    pub fn with_lock_wait(mut self, wait: Option<Duration>) -> Self {
        self.lock_wait = wait;
        self
    }
}

/// The namespace rule of every namespace-file and KV operation:
/// `is_valid_namespace`, then no empty segment (`a..b`, `a.`: its key would
/// hold `//` and no URI could name its files), then [`check_files_segment`],
/// then [`check_kv_segment`]. [`authorize`] and `storage::uri::parse_any` call
/// it; publish calls only the two segment rules, so a namespace with an empty
/// segment still publishes (as before W9b) but can hold no namespace file and
/// no KV entry.
pub fn check_namespace(namespace: &str) -> Result<(), String> {
    if !is_valid_namespace(namespace) {
        return Err(format!(
            "namespace '{namespace}' must match ^[a-z0-9][a-z0-9._-]* and be 1-150 characters long"
        ));
    }
    if namespace.split('.').any(str::is_empty) {
        return Err(format!(
            "namespace '{namespace}' has an empty segment ('..', or a '.' at its start or \
             end), and egret stores nothing under a namespace with an empty segment"
        ));
    }
    check_files_segment(namespace)?;
    check_kv_segment(namespace)
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

/// `_kv`, the KV marker segment (W9c spec §3): a namespace's KV values live
/// under `{ns-as-path}/_kv/`.
pub const KV_SEGMENT: &str = "_kv";

/// No `_kv` segment: publish (validate.rs) and, through [`check_namespace`],
/// every KV and namespace-file operation call it. A namespace `a._kv` would
/// put its flows' execution files and its namespace files under namespace
/// `a`'s KV prefix. The message names the namespace and the segment.
pub fn check_kv_segment(namespace: &str) -> Result<(), String> {
    if namespace.split('.').any(|s| s == KV_SEGMENT) {
        return Err(format!(
            "namespace '{namespace}' has a segment '{KV_SEGMENT}', which marks the KV \
             store's values in internal storage; no namespace may have it"
        ));
    }
    Ok(())
}

/// Read: any namespace of `workspace`. Write: `target == own` exactly. Purge:
/// `target == own` or `target.starts_with(own + ".")`. Api: any access (the
/// permission was checked by the route). System: as Api, with no lock wait.
///
/// The rule is the same for both resources; `resource` changes only the
/// refusal's words and is recorded in the [`Grant`]. A refusal names the
/// namespace and the rule, never whether a file or a key exists there.
pub fn authorize(
    workspace: Uuid,
    actor: Actor<'_>,
    target: &str,
    resource: Resource,
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
                        "a flow of namespace '{own}' may not write {} of \
                         namespace '{target}': a flow writes only its own namespace's {}",
                        resource.noun(),
                        resource.own_noun()
                    ));
                }
            }
            Access::Purge => {
                let child = target
                    .strip_prefix(own)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'));
                if !child {
                    return Err(format!(
                        "a flow of namespace '{own}' may not purge {} of \
                         namespace '{target}': a flow purges only its own namespace's {} \
                         and its child namespaces' ('{own}.…')",
                        resource.noun(),
                        resource.own_noun()
                    ));
                }
            }
        }
    }
    let lock_wait = match actor {
        Actor::Api => Some(API_LOCK_WAIT),
        Actor::Flow { .. } | Actor::System => None,
    };
    Ok(Grant {
        workspace,
        namespace: target.to_string(),
        resource,
        access,
        lock_wait,
    })
}

/// For store functions: `Err(Error::Internal)` when the grant's resource is
/// not `resource` (a namespace-file grant never reaches the KV store, nor the
/// reverse), or when a write is attempted with a Read grant -- egret's bug,
/// not the author's. A Read is satisfied by any grant of the resource
/// (`authorize` only grants Write or Purge where Read is granted too); Write
/// and Purge need a grant of exactly that access (a Purge grant reaches child
/// namespaces a Write may not).
pub(crate) fn require(grant: &Grant, resource: Resource, access: Access) -> egret_core::Result<()> {
    if grant.resource != resource {
        return Err(egret_core::Error::Internal(anyhow::anyhow!(
            "egret asked for {resource:?} {access:?} access to namespace '{}' holding a \
             grant for {:?}",
            grant.namespace,
            grant.resource
        )));
    }
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
            let g = authorize(
                ws,
                flow("a.b"),
                target,
                Resource::NamespaceFiles,
                Access::Read,
            )
            .unwrap();
            assert_eq!((g.namespace(), g.access()), (target, Access::Read));
        }
    }

    #[test]
    fn a_flow_writes_only_its_own_namespace_compared_exactly() {
        let ws = Uuid::new_v4();
        assert!(authorize(ws, flow("a"), "a", Resource::NamespaceFiles, Access::Write).is_ok());
        assert!(authorize(
            ws,
            flow("a.b"),
            "a.b",
            Resource::NamespaceFiles,
            Access::Write
        )
        .is_ok());
        for (own, target) in [("a", "a.b"), ("a.b", "a"), ("a.b", "a.bc"), ("a", "b")] {
            let err = authorize(
                ws,
                flow(own),
                target,
                Resource::NamespaceFiles,
                Access::Write,
            )
            .unwrap_err();
            assert!(
                err.contains(&format!("'{target}'")) && err.contains("writes only its own"),
                "{own} -> {target}: {err}"
            );
        }
        // "A" vs "a": an uppercase namespace is not a namespace at all.
        assert!(authorize(ws, flow("a"), "A", Resource::NamespaceFiles, Access::Write).is_err());
    }

    #[test]
    fn a_flow_purges_its_own_and_child_namespaces_on_a_dot_boundary() {
        let ws = Uuid::new_v4();
        for target in ["a.b", "a.b.c", "a.b.c.d"] {
            assert!(
                authorize(
                    ws,
                    flow("a.b"),
                    target,
                    Resource::NamespaceFiles,
                    Access::Purge
                )
                .is_ok(),
                "{target}"
            );
        }
        for target in ["a.bc", "a", "a.c", "b.a.b", "a.b-c"] {
            let err = authorize(
                ws,
                flow("a.b"),
                target,
                Resource::NamespaceFiles,
                Access::Purge,
            )
            .unwrap_err();
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
                    let err =
                        authorize(ws, actor, bad, Resource::NamespaceFiles, access).unwrap_err();
                    assert!(err.contains(bad), "{bad}: {err}");
                }
                // As the running flow's own namespace, against a good target.
                let err =
                    authorize(ws, flow(bad), "a", Resource::NamespaceFiles, access).unwrap_err();
                assert!(err.contains(bad) && err.contains("running flow"), "{err}");
            }
        }
        // `a._files` passes the pattern: only the segment rule refuses it.
        assert!(is_valid_namespace("a._files"));
        let err = authorize(
            ws,
            Actor::Api,
            "a._files",
            Resource::NamespaceFiles,
            Access::Read,
        )
        .unwrap_err();
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
                    let err =
                        authorize(ws, actor, bad, Resource::NamespaceFiles, access).unwrap_err();
                    assert!(
                        err.contains(&format!("namespace '{bad}'"))
                            && err.contains("empty segment"),
                        "{bad}: {err}"
                    );
                }
                let err =
                    authorize(ws, flow(bad), "a", Resource::NamespaceFiles, access).unwrap_err();
                assert!(
                    err.contains("running flow") && err.contains("empty segment"),
                    "{err}"
                );
            }
        }
        // W9c Task 2 review M2: the sentence names neither resource, and
        // is the same for both.
        for bad in ["a..b", "a."] {
            let files =
                authorize(ws, Actor::Api, bad, Resource::NamespaceFiles, Access::Read).unwrap_err();
            let kv = authorize(ws, Actor::Api, bad, Resource::Kv, Access::Read).unwrap_err();
            assert_eq!(files, kv);
            assert_eq!(
                kv,
                format!(
                    "namespace '{bad}' has an empty segment ('..', or a '.' at its start or \
                     end), and egret stores nothing under a namespace with an empty segment"
                )
            );
        }
        // `.a` is refused by the pattern, named once.
        let err = check_namespace(".a").unwrap_err();
        assert!(
            err.contains("must match") && !err.contains("empty segment"),
            "{err}"
        );
        // Before M3 `a.` was a child of `a` for Purge.
        assert!(authorize(ws, flow("a"), "a.", Resource::NamespaceFiles, Access::Purge).is_err());
        assert!(
            authorize(
                ws,
                flow("a"),
                "a.b",
                Resource::NamespaceFiles,
                Access::Purge
            )
            .is_ok(),
            "the control"
        );
    }

    #[test]
    fn an_invalid_namespace_is_refused_naming_it() {
        let ws = Uuid::new_v4();
        for bad in ["", "A", "a/b", "-a", "a b", &"a".repeat(151)] {
            for access in ALL {
                let err =
                    authorize(ws, Actor::Api, bad, Resource::NamespaceFiles, access).unwrap_err();
                assert!(err.contains(&format!("namespace '{bad}'")), "{err}");
                let err =
                    authorize(ws, flow(bad), "a", Resource::NamespaceFiles, access).unwrap_err();
                assert!(err.contains(&format!("namespace '{bad}'")), "{err}");
            }
        }
        assert!(authorize(
            ws,
            Actor::Api,
            &"a".repeat(150),
            Resource::NamespaceFiles,
            Access::Read
        )
        .is_ok());
    }

    #[test]
    fn a_refusal_never_mentions_existence() {
        let ws = Uuid::new_v4();
        let mut refusals = 0;
        for (own, target) in [("a", "b"), ("a.b", "a.bc"), ("a", "a._files"), ("A", "a")] {
            for access in ALL {
                if let Err(e) = authorize(ws, flow(own), target, Resource::NamespaceFiles, access) {
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
            let g = authorize(ws, actor, "a", Resource::NamespaceFiles, Access::Write).unwrap();
            assert_eq!(g.workspace(), ws);
            assert_ne!(g.workspace(), other);
        }
    }

    #[test]
    fn the_api_may_do_anything_to_a_valid_namespace() {
        for access in ALL {
            assert!(authorize(
                Uuid::new_v4(),
                Actor::Api,
                "x.y",
                Resource::NamespaceFiles,
                access
            )
            .is_ok());
        }
    }

    #[test]
    fn require_refuses_a_write_with_a_read_grant() {
        let ws = Uuid::new_v4();
        let read = authorize(ws, Actor::Api, "a", Resource::NamespaceFiles, Access::Read).unwrap();
        let write =
            authorize(ws, Actor::Api, "a", Resource::NamespaceFiles, Access::Write).unwrap();
        let purge =
            authorize(ws, Actor::Api, "a", Resource::NamespaceFiles, Access::Purge).unwrap();
        assert!(require(&read, Resource::NamespaceFiles, Access::Read).is_ok());
        assert!(matches!(
            require(&read, Resource::NamespaceFiles, Access::Write),
            Err(egret_core::Error::Internal(_))
        ));
        assert!(require(&read, Resource::NamespaceFiles, Access::Purge).is_err());
        assert!(require(&write, Resource::NamespaceFiles, Access::Write).is_ok());
        assert!(require(&write, Resource::NamespaceFiles, Access::Read).is_ok());
        assert!(require(&write, Resource::NamespaceFiles, Access::Purge).is_err());
        assert!(require(&purge, Resource::NamespaceFiles, Access::Purge).is_ok());
        assert!(require(&purge, Resource::NamespaceFiles, Access::Write).is_err());
    }

    // ─── W9c: the KV resource (W9c spec §3, §4.7, §5) ───────────────────

    const RESOURCES: [Resource; 2] = [Resource::NamespaceFiles, Resource::Kv];

    #[test]
    fn a_kv_segment_namespace_is_refused_for_every_resource_access_and_actor() {
        let ws = Uuid::new_v4();
        for bad in ["a._kv", "a._kv.b", "x.y._kv"] {
            assert!(
                is_valid_namespace(bad),
                "{bad}: only the segment rule refuses it"
            );
            assert!(check_files_segment(bad).is_ok(), "{bad}");
            for resource in RESOURCES {
                for access in ALL {
                    for actor in [Actor::Api, Actor::System, flow("a"), flow(bad)] {
                        let err = authorize(ws, actor, bad, resource, access).unwrap_err();
                        assert!(
                            err.contains(&format!("namespace '{bad}'"))
                                && err.contains("segment '_kv'"),
                            "{bad}: {err}"
                        );
                    }
                    // As the running flow's own namespace, against a good target.
                    let err = authorize(ws, flow(bad), "a", resource, access).unwrap_err();
                    assert!(
                        err.contains(bad) && err.contains("running flow") && err.contains("'_kv'"),
                        "{err}"
                    );
                }
            }
        }
        // `_kv` alone is refused by the pattern, named once.
        let err = check_namespace("_kv").unwrap_err();
        assert!(
            err.contains("'_kv' must match") && !err.contains("segment"),
            "{err}"
        );
        assert_eq!(check_kv_segment("a._kv"), check_namespace("a._kv"));
        // The controls: `_kv` inside a segment is not the segment.
        for ok in ["a.kv", "a.x_kv", "a._kvx", "a.b_kv.c", "kv"] {
            assert!(check_namespace(ok).is_ok(), "{ok}");
            assert!(check_kv_segment(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_flow_reads_any_kv_namespace_and_writes_only_its_own() {
        let ws = Uuid::new_v4();
        for target in ["a", "a.b", "a.b.c", "other.ns", "z9-x_y"] {
            let g = authorize(ws, flow("a.b"), target, Resource::Kv, Access::Read).unwrap();
            assert_eq!(
                (g.namespace(), g.resource(), g.access()),
                (target, Resource::Kv, Access::Read)
            );
        }
        let g = authorize(ws, flow("a.b"), "a.b", Resource::Kv, Access::Write).unwrap();
        assert_eq!((g.resource(), g.access()), (Resource::Kv, Access::Write));
        for (own, target) in [("a", "a.b"), ("a.b", "a"), ("a.b", "a.bc"), ("a", "b")] {
            let err = authorize(ws, flow(own), target, Resource::Kv, Access::Write).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "a flow of namespace '{own}' may not write the KV store of namespace \
                     '{target}': a flow writes only its own namespace's KV store"
                )
            );
        }
    }

    #[test]
    fn a_flow_purges_its_own_and_child_kv_namespaces_on_a_dot_boundary() {
        let ws = Uuid::new_v4();
        for target in ["a.b", "a.b.c", "a.b.c.d"] {
            let g = authorize(ws, flow("a.b"), target, Resource::Kv, Access::Purge).unwrap();
            assert_eq!((g.resource(), g.access()), (Resource::Kv, Access::Purge));
        }
        for target in ["a.bc", "a", "a.c", "b.a.b", "a.b-c"] {
            let err = authorize(ws, flow("a.b"), target, Resource::Kv, Access::Purge).unwrap_err();
            assert!(
                err.contains(&format!("'{target}'"))
                    && err.contains("purges only its own namespace's KV store"),
                "{target}: {err}"
            );
        }
    }

    #[test]
    fn the_resource_changes_only_the_words() {
        let ws = Uuid::new_v4();
        let mut compared = 0;
        let owns = ["a", "a.b", "A", "a._files", "a._kv", "a..b"];
        let targets = [
            "a", "a.b", "a.bc", "a.b.c", "b", "a._kv", "a._files", "a..b", "",
        ];
        for own in owns {
            for target in targets {
                for access in ALL {
                    for actor in [flow(own), Actor::Api, Actor::System] {
                        let files = authorize(ws, actor, target, Resource::NamespaceFiles, access);
                        let kv = authorize(ws, actor, target, Resource::Kv, access);
                        match (files, kv) {
                            (Ok(f), Ok(k)) => {
                                assert_eq!(
                                    (f.workspace(), f.namespace(), f.access(), f.lock_wait()),
                                    (k.workspace(), k.namespace(), k.access(), k.lock_wait())
                                );
                                assert_eq!(f.resource(), Resource::NamespaceFiles);
                                assert_eq!(k.resource(), Resource::Kv);
                            }
                            (Err(f), Err(k)) => {
                                let reworded = f
                                    .replace("namespace files of", "the KV store of")
                                    .replace("namespace's files", "namespace's KV store");
                                assert_eq!(k, reworded, "{own} -> {target} {access:?}");
                                compared += 1;
                            }
                            (f, k) => panic!("{own} -> {target} {access:?}: {f:?} vs {k:?}"),
                        }
                    }
                }
            }
        }
        assert!(compared > 100, "{compared} refusals compared");
        // W9b's three sentences are byte-identical for namespace files.
        let ws = Uuid::new_v4();
        assert_eq!(
            authorize(ws, flow("a"), "b", Resource::NamespaceFiles, Access::Write).unwrap_err(),
            "a flow of namespace 'a' may not write namespace files of namespace 'b': a flow \
             writes only its own namespace's files"
        );
        assert_eq!(
            authorize(ws, flow("a"), "b", Resource::NamespaceFiles, Access::Purge).unwrap_err(),
            "a flow of namespace 'a' may not purge namespace files of namespace 'b': a flow \
             purges only its own namespace's files and its child namespaces' ('a.…')"
        );
    }

    #[test]
    fn the_system_actor_is_granted_like_the_api_with_no_lock_wait() {
        let ws = Uuid::new_v4();
        for resource in RESOURCES {
            for access in ALL {
                let g = authorize(ws, Actor::System, "x.y", resource, access).unwrap();
                assert_eq!(g.lock_wait(), None);
                let api = authorize(ws, Actor::Api, "x.y", resource, access).unwrap();
                assert_eq!(api.lock_wait(), Some(API_LOCK_WAIT));
            }
        }
    }

    #[test]
    fn require_refuses_a_grant_of_the_other_resource() {
        let ws = Uuid::new_v4();
        for access in ALL {
            let files = authorize(ws, Actor::Api, "a", Resource::NamespaceFiles, access).unwrap();
            let kv = authorize(ws, Actor::Api, "a", Resource::Kv, access).unwrap();
            for asked in ALL {
                assert!(matches!(
                    require(&files, Resource::Kv, asked),
                    Err(egret_core::Error::Internal(_))
                ));
                assert!(matches!(
                    require(&kv, Resource::NamespaceFiles, asked),
                    Err(egret_core::Error::Internal(_))
                ));
            }
            // The controls: the same grants, asked for their own resource.
            assert!(require(&files, Resource::NamespaceFiles, access).is_ok());
            assert!(require(&kv, Resource::Kv, access).is_ok());
            assert!(require(&kv, Resource::Kv, Access::Read).is_ok());
        }
        let read = authorize(ws, Actor::Api, "a", Resource::Kv, Access::Read).unwrap();
        assert!(require(&read, Resource::Kv, Access::Write).is_err());
        assert!(require(&read, Resource::Kv, Access::Purge).is_err());
    }

    #[test]
    fn a_kv_refusal_never_mentions_existence() {
        let ws = Uuid::new_v4();
        let mut refusals = 0;
        for (own, target) in [("a", "b"), ("a.b", "a.bc"), ("a", "a._kv"), ("A", "a")] {
            for access in ALL {
                if let Err(e) = authorize(ws, flow(own), target, Resource::Kv, access) {
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
}
