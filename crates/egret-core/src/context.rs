//! Per-request identity and workspace scoping.
//!
//! Middleware constructs a [`WorkspaceContext`] and injects it into handlers;
//! repositories must filter every workspace-scoped query by `context.workspace_id`.

use crate::ids::{UserId, WorkspaceId};

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: UserId,
    pub email: String,
    /// Platform-level roles (e.g. `platform_admin`, `data_steward`).
    pub platform_roles: Vec<String>,
    /// Effective permission keys resolved at session build.
    pub permissions: Vec<String>,
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
        self.principal
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin")
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
