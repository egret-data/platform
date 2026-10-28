//! `WorkflowRefs` — the real `WorkflowRefPort` implementation `publish`
//! resolves every reference through (Task 6 introduced the trait and its
//! test double; Task 7 is the first thing that wires a real one to a route).
//!
//! # Per-`RefKind`, what resolves today and what does not
//!
//! | `RefKind` | Resolves? | Through | Port that would close the gap |
//! |---|---|---|---|
//! | [`RefKind::DataApiEndpoint`] | Yes | `egret_dataapi_api::DataApiEndpointPort::workspace_of` | already exists |
//! | [`RefKind::Database`] | Yes (package #4 Task C5, 2026-10-28) | `egret_query_api::DatabaseOwnerPort::workspace_of`, implemented by `egret_query::DatabaseRepo` | already exists |
//! | [`RefKind::Dataset`] | **No — `None`, always** | nothing | an `analytics.datasets`-owning port (none exists in the workspace) |
//! | [`RefKind::Chart`] | **No — `None`, always** | nothing | an `analytics.charts`-owning port (none exists in the workspace) |
//!
//! A flow reference (`Subflow`, `dependsOn`) is not a `RefKind` any more:
//! since debt spec §5.1 (2026-10-19) `publish` resolves it itself, in the
//! caller's workspace only (`FlowRepo::find_in_workspace`), and the unscoped
//! `FlowRepo::find_by_name` this port used for it is gone.
//!
//! **`None` is the correct, deliberate answer for the two unresolved
//! kinds — not a placeholder to widen later without a port.** `publish`
//! treats `None` exactly like a foreign reference: `Error::NotFound`, and a
//! flow naming a `dataset:`/`chart:` therefore cannot be published until a
//! real port exists. That is fail-closed, and it is the right failure mode: a
//! flow referencing an object that cannot be proven to belong to the
//! caller's workspace must not be trusted with one. (`database:` was in this
//! list until package #4 Task C5 gave it a port; its test was updated to
//! route `Database` through that port, as the sentences below ask.)
//! `an_unresolvable_kind_refuses_rather_than_resolving_vacuously` (this
//! module's own test) pins exactly this, specifically so a future change
//! that "fixes" the gap by returning `Some` for an untrusted kind — silently
//! reopening the cross-workspace hole this whole programme exists to close
//! (migration 0128) — fails a test instead of shipping quietly. **The test
//! that actually carries that property is
//! `a_kind_with_no_port_refuses_even_a_well_formed_reference`** (a
//! syntactically valid `Uuid`, not a string that merely fails to parse) —
//! whoever next wires a real port for `Dataset`/`Chart` should
//! expect to UPDATE that test (to route the newly-ported kind through its
//! real port instead of asserting `None`), not delete it, and should add an
//! equivalent one for whichever kind(s) remain unported.
//!
//! # Why this lives in `egret-workflow`, not `egret-workflow-api`
//!
//! `WorkflowRefPort` and `RefKind` live in the leaf `egret-workflow-api`
//! crate so a consumer can depend on the Workflow vocabulary without pulling
//! the engine. This composite is the OPPOSITE direction — it depends on
//! another seam's leaf contract crate (`egret-dataapi-api`) to implement
//! that vocabulary — so it belongs on the engine side, in the crate that
//! already depends on `egret-workflow-api` to re-export
//! `RefKind`/`WorkflowRefPort` in the first place. `egret-automation`
//! already depends on the same leaf crate for an identical reason; this is
//! the same shape, one seam over.

use std::sync::Arc;

use egret_core::Error;
use uuid::Uuid;

use egret_workflow_api::{RefKind, WorkflowRefPort};

/// The real [`WorkflowRefPort`]: dispatches each [`RefKind`] to whichever
/// port (if any — see the module doc's table) can answer for it.
pub struct WorkflowRefs {
    pub dataapi_endpoints: Arc<dyn egret_dataapi_api::DataApiEndpointPort>,
    /// `RefKind::Database` (an `egret.sql` task's `database:`), package #4
    /// Task C5.
    pub databases: Arc<dyn egret_query_api::DatabaseOwnerPort>,
}

/// `name_or_id` for every `RefKind` is a plain
/// `Uuid` in string form (the leaf spec's opaque field carries the object's
/// own id, not a human name — `spec::registry::TASK_TYPES` names the field,
/// never a lookup-by-name). A value that does not parse as a `Uuid` cannot
/// name any real object, so it resolves to `None` exactly like a
/// syntactically valid but nonexistent id — the same "absent and malformed
/// are the same refusal" rule [`crate::publish::ref_not_found`] already
/// applies one level up.
fn as_uuid(name_or_id: &str) -> Option<Uuid> {
    name_or_id.parse().ok()
}

#[async_trait::async_trait]
impl WorkflowRefPort for WorkflowRefs {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, Error> {
        match kind {
            RefKind::DataApiEndpoint => {
                let Some(id) = as_uuid(name_or_id) else {
                    return Ok(None);
                };
                let ws = self.dataapi_endpoints.workspace_of(id).await?;
                Ok(ws.map(|ws| (id, ws)))
            }
            RefKind::Database => {
                let Some(id) = as_uuid(name_or_id) else {
                    return Ok(None);
                };
                let ws = self.databases.workspace_of(id).await?;
                Ok(ws.map(|ws| (id, ws)))
            }
            // No port anywhere in the workspace owns these two yet — see
            // the module doc's table. `None`, always, is the fail-closed
            // answer: `publish` refuses it exactly like a foreign reference,
            // never like a resolved one.
            RefKind::Dataset | RefKind::Chart => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnreachableDataApi;
    #[async_trait::async_trait]
    impl egret_dataapi_api::DataApiEndpointPort for UnreachableDataApi {
        async fn workspace_of(&self, _id: Uuid) -> Result<Option<Uuid>, Error> {
            unreachable!("Database/Dataset/Chart must never reach the Data API port")
        }
    }

    struct UnreachableDatabases;
    #[async_trait::async_trait]
    impl egret_query_api::DatabaseOwnerPort for UnreachableDatabases {
        async fn workspace_of(&self, _id: Uuid) -> Result<Option<Uuid>, Error> {
            unreachable!("only `RefKind::Database`, with a well-formed id, may ask this port")
        }
    }

    /// A database port that knows one `(id, workspace)` pair and records
    /// every id it was asked about.
    struct OneDatabase {
        id: Uuid,
        ws: Uuid,
        asked: std::sync::Mutex<Vec<Uuid>>,
    }
    #[async_trait::async_trait]
    impl egret_query_api::DatabaseOwnerPort for OneDatabase {
        async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, Error> {
            self.asked.lock().unwrap().push(id);
            Ok((id == self.id).then_some(self.ws))
        }
    }

    fn unreachable_refs() -> WorkflowRefs {
        WorkflowRefs {
            dataapi_endpoints: Arc::new(UnreachableDataApi),
            databases: Arc::new(UnreachableDatabases),
        }
    }

    /// Named for what it actually proves: `workspace_of("anything-at-all")`
    /// for `Dataset`/`Chart` refuses -- but `"anything-at-all"` fails `Uuid`
    /// parsing, so this only pins the MALFORMED-input property, which
    /// [`a_non_uuid_value_for_an_id_based_kind_refuses_without_asking_the_port`]
    /// already covers for the id-based kinds. It does NOT, on its own, prove
    /// these kinds refuse a well-formed reference for lack of a port --
    /// [`a_kind_with_no_port_refuses_even_a_well_formed_reference`] (below)
    /// is the test that carries that property. `Database` left this list in
    /// package #4 Task C5, when it got a port.
    #[tokio::test]
    async fn an_unresolvable_kind_refuses_rather_than_resolving_vacuously() {
        let refs = unreachable_refs();
        for kind in [RefKind::Dataset, RefKind::Chart] {
            let got = refs
                .workspace_of(kind, "anything-at-all")
                .await
                .expect("must not error, just refuse");
            assert_eq!(got, None, "{kind:?} must resolve to None, not Some");
        }
    }

    /// THE test this module exists to carry, per fix round 1: a
    /// SYNTACTICALLY VALID reference (a real `Uuid`) for `Dataset`/`Chart`
    /// still refuses, because no port exists for either -- not because the
    /// input happened to be malformed. The `Unreachable*` ports assert the
    /// stronger property: these kinds must not even ASK a port. Pinned so a
    /// future change cannot "fix" one of them into a permissive `Some`
    /// without this test going red first, and silently reopen the
    /// cross-workspace hole migration 0128 is this programme's recorded case
    /// of. `Database` was in this list until package #4 Task C5; it is now
    /// routed through its real port, in the two tests below.
    #[tokio::test]
    async fn a_kind_with_no_port_refuses_even_a_well_formed_reference() {
        let refs = unreachable_refs();
        let well_formed = Uuid::new_v4().to_string();
        for kind in [RefKind::Dataset, RefKind::Chart] {
            let got = refs
                .workspace_of(kind, &well_formed)
                .await
                .expect("must not error, just refuse");
            assert_eq!(
                got, None,
                "{kind:?} must refuse a WELL-FORMED reference too -- no port exists, \
                 so a syntactically valid id must not sneak through"
            );
        }
    }

    /// Package #4 Task C5: `Database` resolves through its port to the
    /// OWNER's workspace, and an id the port has never heard of is `None`.
    /// `publish` compares that workspace with the caller's, so a database of
    /// another workspace is refused exactly like a fictional one (publish's
    /// own tests pin that comparison).
    #[tokio::test]
    async fn a_database_resolves_through_its_port_to_the_owning_workspace() {
        let (id, ws) = (Uuid::new_v4(), Uuid::new_v4());
        let port = Arc::new(OneDatabase {
            id,
            ws,
            asked: Default::default(),
        });
        let refs = WorkflowRefs {
            dataapi_endpoints: Arc::new(UnreachableDataApi),
            databases: port.clone(),
        };
        assert_eq!(
            refs.workspace_of(RefKind::Database, &id.to_string())
                .await
                .unwrap(),
            Some((id, ws)),
            "the id is kept as written and the owner is the port's answer"
        );
        let unknown = Uuid::new_v4();
        assert_eq!(
            refs.workspace_of(RefKind::Database, &unknown.to_string())
                .await
                .unwrap(),
            None
        );
        assert_eq!(*port.asked.lock().unwrap(), vec![id, unknown]);
    }

    /// A port error is the caller's outage, not a refusal: it propagates
    /// rather than turning into `None` (which publish would report as "not
    /// found" and blame the author for).
    #[tokio::test]
    async fn a_database_port_error_propagates_rather_than_reading_as_absent() {
        struct Down;
        #[async_trait::async_trait]
        impl egret_query_api::DatabaseOwnerPort for Down {
            async fn workspace_of(&self, _id: Uuid) -> Result<Option<Uuid>, Error> {
                Err(Error::Internal(anyhow::anyhow!("metastore down")))
            }
        }
        let refs = WorkflowRefs {
            dataapi_endpoints: Arc::new(UnreachableDataApi),
            databases: Arc::new(Down),
        };
        let got = refs
            .workspace_of(RefKind::Database, &Uuid::new_v4().to_string())
            .await;
        assert!(matches!(got, Err(Error::Internal(_))), "{got:?}");
    }

    #[tokio::test]
    async fn a_non_uuid_value_for_an_id_based_kind_refuses_without_asking_the_port() {
        let refs = unreachable_refs();
        for kind in [RefKind::DataApiEndpoint, RefKind::Database] {
            let got = refs
                .workspace_of(kind, "not-a-uuid")
                .await
                .expect("must not error");
            assert_eq!(got, None, "{kind:?}");
        }
    }
}
