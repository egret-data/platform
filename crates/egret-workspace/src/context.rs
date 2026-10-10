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

        // 3.4 Impersonation containment. A ws-scoped impersonation (a ws_admin
        //      acting as a member) is confined to the ONE workspace it was started
        //      in — even though the target may belong to others. Deny any other
        //      workspace outright, BEFORE membership is resolved. A
        //      platform-scoped overlay (`workspace_scope == None`) is unrestricted
        //      and skips this.
        //
        //      **What this is the choke point FOR, by code path.** `WsContext` is
        //      the only extractor that resolves a `{workspace}` path segment, and
        //      `every_workspace_scoped_route_is_mounted_under_api_ws` (in
        //      egret-server) holds to CI that every `{workspace}` route is mounted
        //      under `/api/ws/`. So this denial covers exactly the routes that name
        //      their workspace in the path — all of `/api/ws/…` — and it covers
        //      them completely.
        //
        //      ⚠ **It is NOT a containment for a route that names its resource some
        //      other way, and this comment used to claim it was** ("the airtight
        //      choke point … no route can reach another workspace under a scoped
        //      overlay"). The premise was true and the conclusion was false: a
        //      route with no `{workspace}` builds no `WsContext`, so nothing here
        //      runs for it. Measured, SP4d debt Tasks 4 and 13: every
        //      `/api/control-plane` route takes the bare `Auth` extractor —
        //      `control_plane_router`, `policy_admin_router`,
        //      `catalog_admin_router`, `engine_mgmt_router` and `perm_router`, all
        //      nested in `egret-server`'s `build_router` — and each gates on a
        //      `Scope::Platform` permission instead, which is a *platform*
        //      authorization question and says nothing about a workspace.
        //
        //      Three places enforce `Impersonator::workspace_scope` today (`grep -rn
        //      workspace_scope crates/ --include='*.rs'`): this guard,
        //      `list_my_workspaces` (which filters the switcher to the scope), and
        //      `egret_perm_compile::routes`' `confine_to_overlay_workspace` (which
        //      confines the `perm` surface to catalogs assigned to the scope, keyed
        //      to `registry.catalog_workspaces`). Nothing in `egret-core`,
        //      `egret-policy`, `egret-controlplane`, `egret-catalog-engine` or
        //      `egret-engine-mgmt` reads it at all.
        //
        //      What makes the gap non-exploitable is therefore NOT this guard.
        //      ⚠ **And it is not `validate_impersonation_target` alone either,
        //      which is what this comment used to say** ("refuses a target
        //      holding ANY platform-scoped role grant — so a scoped overlay's
        //      principal cannot satisfy a `Scope::Platform` permission in the
        //      first place"). That validator runs ONCE, when the overlay row is
        //      written, and a reviewer falsified the claim built on it: nothing
        //      bypassed the validator, the overlay simply OUTLIVED the fact the
        //      validator had measured. `SessionService::validate` rebuilds the
        //      TARGET's platform roles and permissions from live tables on every
        //      request for the overlay's 30-minute life, so a target who gained a
        //      platform role afterwards — `sync_user_oidc_groups` replaying an IdP
        //      group change on the next login, or a console grant, neither needing
        //      any permission from the admin — handed the same `ws_admin` token a
        //      principal carrying platform permissions.
        //
        //      The mechanism that closes it, since `391c3c3a6` (2026-10-10): the
        //      rule lives in
        //      `egret_auth::impersonation::overlay_refused_by_platform_authority`
        //      and `SessionService::validate` RE-EVALUATES it on every request,
        //      falling through to the admin principal when it refuses — so a
        //      ws-scoped overlay over a platform-role-holding target is inert, not
        //      merely un-creatable, and `principal.impersonator` is `None` on
        //      those routers rather than `Some(scope)` that nothing confines.
        //      Read that function's ⚠ paragraph before relying on any of this: the
        //      guard here covers `/api/ws/…`, the inertness rule covers every
        //      route at once, and only their union covers the installation.
        //
        //      ⚠ So when you read a claim of this shape — "X cannot happen, since
        //      nothing does it today" — check whether it names a fact measured
        //      once or a predicate re-evaluated per request. This one was the
        //      first kind twice, and was falsified twice.
        if let Some(imp) = &principal.impersonator {
            if let Some(scope) = imp.workspace_scope {
                if scope != ws.id {
                    return Err(AuthError(Error::Forbidden));
                }
            }
        }

        // 3.5 Feature entitlement. Whether the workspace has this module at all
        //     is a property of the WORKSPACE, so it is checked before — and
        //     independently of — who the caller is: a platform admin gets the
        //     same 403 as anyone else, and a non-member cannot tell membership
        //     from the response. The DB is only touched when the path actually
        //     belongs to a gated module.
        //
        //     `nest()` rewrites the inner router's view of the URI, so
        //     `parts.uri` is the path with the mount prefix already stripped —
        //     and by how much depends on where the router happened to be
        //     mounted. `OriginalUri` is the path as the client sent it, so the
        //     answer no longer depends on mount depth. It falls back to
        //     `parts.uri` for routers that were never nested at all.
        //
        //     One invariant remains, and it is convention rather than
        //     construction: `ws_path_suffix` recognises a path only if it
        //     begins `/api/ws/` or `/ws/`. A gated router mounted at, say,
        //     `/api/v2` would yield `/api/v2/ws/{slug}/…`, match neither, and
        //     fail OPEN. `every_workspace_scoped_route_is_mounted_under_api_ws`
        //     in egret-server holds that invariant to CI.
        let gated = {
            let full_path = parts
                .extensions
                .get::<axum::extract::OriginalUri>()
                .map_or_else(|| parts.uri.path(), |o| o.0.path());
            features::ws_path_suffix(full_path)
                .and_then(|suffix| features::gated_feature(suffix, parts.method.as_str()))
        };
        if let Some(key) = gated {
            let disabled = repo.disabled_features(ws.id).await.map_err(AuthError)?;
            if disabled.contains(key) {
                return Err(AuthError(Error::FeatureDisabled(key)));
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
