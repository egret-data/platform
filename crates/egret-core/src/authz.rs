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

/// What the action is being taken on. Object variants arrive in B2.
#[derive(Debug, Clone)]
pub enum Resource {
    Platform,
    Workspace(WorkspaceId),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_is_constructible() {
        let _ = Resource::Platform;
        let _ = Resource::Workspace(WorkspaceId::new());
    }
}
