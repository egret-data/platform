//! Workspace + RBAC domain types and the fixed role catalog.

use egret_core::WorkspaceId;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub slug: String,
    pub name: String,
    pub status: String, // active | archived
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceMember {
    pub principal_type: String, // user | group
    pub principal_id: Uuid,
    pub role: String,
    pub principal_name: Option<String>,
    pub granted_by_name: Option<String>,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub is_platform_admin: bool,
}

pub const WORKSPACE_ROLES: &[&str] = &["ws_admin", "ws_editor", "ws_viewer"];
pub const PLATFORM_ROLES: &[&str] = &["platform_admin", "data_steward"];

/// Display precedence for a workspace role (higher = more privileged). Enforcement is
/// always union-based; this is only used to pick a single role name for display.
pub fn workspace_role_rank(role: &str) -> u8 {
    match role {
        "ws_admin" => 3,
        "ws_editor" => 2,
        "ws_viewer" => 1,
        _ => 0,
    }
}

/// Validate a workspace slug: non-empty, lowercase letters/digits/hyphen only.
pub fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 63
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(is_valid_slug("sales-eu"));
        assert!(!is_valid_slug("Sales"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("a b"));
    }

    #[test]
    fn role_precedence_orders_admin_highest() {
        assert!(workspace_role_rank("ws_admin") > workspace_role_rank("ws_editor"));
        assert!(workspace_role_rank("ws_editor") > workspace_role_rank("ws_viewer"));
        assert_eq!(workspace_role_rank("nope"), 0);
    }
}
