//! OIDC login: claims, provisioning, and (Task 4) the OidcService.

use egret_core::{Result, UserId};

use crate::groups::GroupRepo;
use crate::users::UserRepo;

/// Normalized claims extracted from a validated id_token.
#[derive(Debug, Clone)]
pub struct OidcClaims {
    pub subject: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

/// JIT-provision the user (match by `sub`) and sync their OIDC group memberships.
/// Returns the local user id. Roles resolve via the existing group→role path.
pub async fn provision_and_sync(
    users: &UserRepo,
    groups: &GroupRepo,
    claims: &OidcClaims,
) -> Result<UserId> {
    let user_id = match users.find_by_external_subject(&claims.subject).await? {
        Some(u) => {
            users
                .update_profile(u.id, &claims.email, &claims.name)
                .await?;
            u.id
        }
        None => {
            users
                .insert_oidc(&claims.subject, &claims.email, &claims.name)
                .await?
        }
    };
    users.touch_last_login(user_id).await?;
    for g in &claims.groups {
        groups.upsert_oidc_group(g).await?;
    }
    groups
        .sync_user_oidc_groups(user_id, &claims.groups)
        .await?;
    Ok(user_id)
}
