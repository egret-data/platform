//! `WsContext` — authenticate + resolve the workspace from the `{workspace}` path slug.

use std::collections::HashMap;

use axum::extract::{FromRef, FromRequestParts, Path};
use axum::http::request::Parts;
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_core::{Error, WorkspaceContext};

use crate::features;
use crate::model::workspace_role_rank;
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

pub struct WsContext(pub WorkspaceContext);

impl<S> FromRequestParts<S> for WsContext
where
    SessionService: FromRef<S>,
    WorkspaceRepo: FromRef<S>,
    Authorizer: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // 1. Authenticate.
        let Auth(principal) = Auth::from_request_parts(parts, state).await?;

        // 2. Read the {workspace} slug.
        let Path(params): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError(Error::InvalidInput("missing workspace path".into())))?;
        let slug = params
            .get("workspace")
            .ok_or_else(|| AuthError(Error::InvalidInput("missing workspace param".into())))?;

        // 3. Load the active workspace.
        let repo = WorkspaceRepo::from_ref(state);
        let ws = repo
            .get_by_slug(slug)
            .await
            .map_err(AuthError)?
            .filter(|w| w.status == "active")
            .ok_or_else(|| AuthError(Error::NotFound(format!("workspace: {slug}"))))?;

        // 3.5 Feature entitlement. Whether the workspace has this module at all
        //     is a property of the WORKSPACE, so it is checked before — and
        //     independently of — who the caller is: a platform admin gets the
        //     same 403 as anyone else, and a non-member cannot tell membership
        //     from the response. The DB is only touched when the path actually
        //     belongs to a gated module.
        //
        //     `parts.uri.path()` is the inner router's view, which axum's
        //     `nest` has already stripped: a request to
        //     `/api/ws/sales/notebooks` arrives here as `/ws/sales/notebooks`,
        //     because every gated router is nested at exactly `/api`.
        //     `ws_path_suffix` accepts both spellings.
        if let Some(suffix) = features::ws_path_suffix(parts.uri.path()) {
            if let Some(key) = features::gated_feature(suffix, parts.method.as_str()) {
                let disabled = repo.disabled_features(ws.id).await.map_err(AuthError)?;
                if disabled.contains(key) {
                    return Err(AuthError(Error::FeatureDisabled(key)));
                }
            }
        }

        // 4. Membership: platform_admin bypass, else a workspace role, else an
        //    object-guest. An object-guest is a NON-member who holds an
        //    object-scoped grant on some object in this workspace (a share
        //    recipient). We admit them with the display-only sentinel role
        //    `object_guest`, which carries NO permissions: workspace-level
        //    `require(&ctx, …)` still denies them (no workspace grant), so only
        //    the specific objects they were granted are reachable — and only via
        //    `require_object`, which asks the PDP. `workspace_role` is never used
        //    for enforcement; it is display/precedence metadata only.
        let is_admin = principal
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin");
        let workspace_role = if is_admin {
            "platform_admin".to_string()
        } else {
            let authz = Authorizer::from_ref(state);
            let roles = authz
                .workspace_roles_for(principal.user_id, ws.id)
                .await
                .map_err(AuthError)?;
            match roles.into_iter().max_by_key(|r| workspace_role_rank(r)) {
                Some(r) => r,
                None if authz
                    .has_object_grant_in_workspace(principal.user_id, ws.id)
                    .await
                    .map_err(AuthError)? =>
                {
                    "object_guest".to_string()
                }
                None => return Err(AuthError(Error::Forbidden)),
            }
        };

        Ok(WsContext(WorkspaceContext {
            workspace_id: ws.id,
            principal,
            workspace_role,
        }))
    }
}
