//! Per-request identity and workspace scoping.
//!
//! Middleware constructs a [`WorkspaceContext`] and injects it into handlers;
//! repositories must filter every workspace-scoped query by `context.workspace_id`.

use crate::ids::{UserId, WorkspaceId};

/// The real admin behind an impersonated request. Present on `Principal` iff the
/// request is an impersonation; the principal's own `user_id`/`email`/roles are
/// then the TARGET's.
#[derive(Debug, Clone)]
pub struct Impersonator {
    pub user_id: crate::ids::UserId,
    pub email: String,
    /// NULL for platform_admin (unrestricted); Some(ws) confines a ws_admin.
    pub workspace_scope: Option<crate::ids::WorkspaceId>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// The platform role that holds every platform permission. Held directly or
/// through a group: [`Principal::platform_roles`] carries both.
pub const PLATFORM_ADMIN_ROLE: &str = "platform_admin";

/// Whether a set of platform role names includes [`PLATFORM_ADMIN_ROLE`]: the
/// one predicate for "is a platform admin", for callers that hold a role list
/// rather than a [`Principal`] (an impersonation target's roles, a re-check at
/// an OIDC callback).
pub fn holds_platform_admin(platform_roles: &[String]) -> bool {
    platform_roles.iter().any(|r| r == PLATFORM_ADMIN_ROLE)
}

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: UserId,
    pub email: String,
    /// Platform-level roles (e.g. `platform_admin`, `data_steward`), held
    /// directly or through a group, unexpired (`SessionService::validate`).
    pub platform_roles: Vec<String>,
    /// Effective permission keys resolved at session build.
    pub permissions: Vec<String>,
    /// Group ids the user belongs to (direct memberships), for the PDP subject.
    pub group_ids: Vec<crate::ids::GroupId>,
    /// Set iff this request is an impersonation. See `Impersonator`.
    pub impersonator: Option<Impersonator>,
}

impl Principal {
    /// Whether this caller is a platform admin, directly or through a group.
    /// Every inline "is platform admin" check routes through here (#802).
    pub fn is_platform_admin(&self) -> bool {
        holds_platform_admin(&self.platform_roles)
    }
}

/// Identity + the workspace the current request operates within.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    pub workspace_id: WorkspaceId,
    pub principal: Principal,
    /// Workspace-scoped role (e.g. `ws_admin`, `ws_editor`, `ws_viewer`).
    pub workspace_role: String,
}

impl WorkspaceContext {
    pub fn is_platform_admin(&self) -> bool {
        self.principal.is_platform_admin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(roles: Vec<&str>) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id: WorkspaceId::new(),
            principal: Principal {
                user_id: UserId::new(),
                email: "a@b.c".into(),
                platform_roles: roles.into_iter().map(String::from).collect(),
                permissions: vec![],
                group_ids: vec![],
                impersonator: None,
            },
            workspace_role: "ws_editor".into(),
        }
    }

    #[test]
    fn platform_admin_detected() {
        assert!(ctx(vec!["platform_admin"]).is_platform_admin());
        assert!(!ctx(vec!["data_steward"]).is_platform_admin());
    }
}
