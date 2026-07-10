//! Engine-agnostic authorization primitives. The decision engine (Cedar) lives
//! in `egret-auth`; nothing here depends on it.

use crate::ids::{UserId, WorkspaceId};

/// The authenticated caller, as seen by the PDP.
#[derive(Debug, Clone)]
pub struct Subject {
    pub user_id: UserId,
    /// Group ids the user belongs to (direct memberships).
    pub group_ids: Vec<crate::ids::GroupId>,
}

/// A specific BI content object being acted on, plus the ancestry (folder →
/// workspace) needed to resolve folder- and workspace-scoped grants.
#[derive(Debug, Clone)]
pub struct ObjectRef {
    pub kind: String, // "dashboard" | "chart" | "dataset" | "folder"
    pub id: uuid::Uuid,
    pub folder_id: Option<uuid::Uuid>, // None = Uncategorized
    pub workspace_id: WorkspaceId,
}

/// What the action is being taken on.
#[derive(Debug, Clone)]
pub enum Resource {
    Platform,
    Workspace(WorkspaceId),
    Object(ObjectRef),
}

/// The outcome of an authorization check.
#[derive(Debug, Clone)]
pub struct Decision {
    pub allow: bool,
    /// Ids of the policies that produced the decision (audit reason, B4).
    pub reasons: Vec<String>,
}

/// The single decision point. One implementation (Cedar) lives in `egret-auth`.
#[async_trait::async_trait]
pub trait Pdp: Send + Sync {
    async fn authorize(
        &self,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> crate::Result<Decision>;
}

/// A request context that resolves to a PDP subject + the resource scope of the
/// action. Implemented by the request-context shapes so `require` accepts either.
pub trait AuthTarget {
    fn subject(&self) -> Subject;
    fn resource(&self) -> Resource;
}

impl AuthTarget for crate::Principal {
    fn subject(&self) -> Subject {
        Subject {
            user_id: self.user_id,
            group_ids: self.group_ids.clone(),
        }
    }
    fn resource(&self) -> Resource {
        Resource::Platform
    }
}

impl AuthTarget for crate::WorkspaceContext {
    fn subject(&self) -> Subject {
        self.principal.subject()
    }
    fn resource(&self) -> Resource {
        Resource::Workspace(self.workspace_id)
    }
}

/// Enforce `action` on a specific object. Subject from `ctx`; resource is the
/// object + its ancestry (folder → workspace → platform).
pub async fn require_object(
    pdp: &dyn Pdp,
    ctx: &crate::WorkspaceContext,
    action: &str,
    object: ObjectRef,
) -> crate::Result<()> {
    debug_assert_eq!(object.workspace_id, ctx.workspace_id);
    if pdp
        .authorize(&ctx.principal.subject(), action, &Resource::Object(object))
        .await?
        .allow
    {
        Ok(())
    } else {
        Err(crate::Error::Forbidden)
    }
}

/// Enforce `action` for `target`: `Ok(())` if the PDP allows, `Error::Forbidden` otherwise.
pub async fn require(pdp: &dyn Pdp, target: &impl AuthTarget, action: &str) -> crate::Result<()> {
    if pdp
        .authorize(&target.subject(), action, &target.resource())
        .await?
        .allow
    {
        Ok(())
    } else {
        Err(crate::Error::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_is_constructible() {
        let _ = Resource::Platform;
        let _ = Resource::Workspace(WorkspaceId::new());
    }
}

#[cfg(test)]
mod require_tests {
    use super::*;
    use crate::ids::{UserId, WorkspaceId};

    struct StubPdp {
        allow: bool,
    }
    #[async_trait::async_trait]
    impl Pdp for StubPdp {
        async fn authorize(
            &self,
            _s: &Subject,
            _a: &str,
            _r: &Resource,
        ) -> crate::Result<Decision> {
            Ok(Decision {
                allow: self.allow,
                reasons: vec![],
            })
        }
    }

    fn principal() -> crate::Principal {
        crate::Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: vec![],
            permissions: vec![],
            group_ids: vec![crate::ids::GroupId::new()],
        }
    }

    #[test]
    fn principal_targets_platform_and_carries_groups() {
        let p = principal();
        assert!(matches!(p.resource(), Resource::Platform));
        assert_eq!(p.subject().group_ids.len(), 1);
    }

    #[test]
    fn workspace_context_targets_its_workspace() {
        let ws = WorkspaceId::new();
        let ctx = crate::WorkspaceContext {
            workspace_id: ws,
            principal: principal(),
            workspace_role: "ws_viewer".into(),
        };
        match ctx.resource() {
            Resource::Workspace(w) => assert_eq!(w, ws),
            _ => panic!("expected workspace resource"),
        }
        assert_eq!(ctx.subject().group_ids.len(), 1);
    }

    #[tokio::test]
    async fn require_allows_and_forbids() {
        let p = principal();
        assert!(require(&StubPdp { allow: true }, &p, "x:y").await.is_ok());
        assert!(matches!(
            require(&StubPdp { allow: false }, &p, "x:y").await,
            Err(crate::Error::Forbidden)
        ));
    }

    fn object_for(ws: WorkspaceId) -> ObjectRef {
        ObjectRef {
            kind: "dashboard".into(),
            id: uuid::Uuid::new_v4(),
            folder_id: None,
            workspace_id: ws,
        }
    }

    #[tokio::test]
    async fn require_object_allows_and_forbids() {
        let ws = WorkspaceId::new();
        let ctx = crate::WorkspaceContext {
            workspace_id: ws,
            principal: principal(),
            workspace_role: "ws_viewer".into(),
        };
        assert!(require_object(
            &StubPdp { allow: true },
            &ctx,
            "dashboard:read",
            object_for(ws),
        )
        .await
        .is_ok());
        assert!(matches!(
            require_object(
                &StubPdp { allow: false },
                &ctx,
                "dashboard:read",
                object_for(ws),
            )
            .await,
            Err(crate::Error::Forbidden)
        ));
    }
}
