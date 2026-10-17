//! `GET /perm/people` — the People & groups index — and
//! `GET /perm/people/{users,groups,roles}/…`, one principal (spec §4.2).
//! Gated `policy:read` (Q2, user 2026-10-17), as `list_grants` is. The reads,
//! and why their membership answers are the compiler's, are in
//! [`crate::people`].

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use uuid::Uuid;

use super::{PermHttpError, PermState};
use crate::people::{
    list_people_via, principal_members_via, read_group_via, read_role_via, read_user_via,
    GroupPrincipal, ListKind, PeopleFilter, PeopleList, RolePrincipal, UserPrincipal,
};

/// The default page size, and the largest one a caller may ask for.
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeopleQuery {
    /// `user` | `group` | `role`. Required.
    #[serde(default)]
    pub kind: Option<String>,
    /// Case-insensitive substring of a user's name, email or username, or of
    /// a group's or role's name. `%` and `_` are literal.
    #[serde(default)]
    pub q: Option<String>,
    /// Exact, case-insensitive username lookup. `kind=user` only. Finds a
    /// service account whatever `includeServiceAccounts` says.
    #[serde(default)]
    pub username: Option<String>,
    /// `kind=user` only: list service accounts too. Default false.
    #[serde(default, rename = "includeServiceAccounts")]
    pub include_service_accounts: Option<bool>,
    /// Page size; default 50, at most 200 (a larger value is capped).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Rows to skip; default 0.
    #[serde(default)]
    pub offset: Option<i64>,
}

/// Which kind of principal a row is.
#[derive(Clone, Copy, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PersonKind {
    User,
    Group,
    Role,
}

/// One user, group or role on the People index.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonRow {
    pub kind: PersonKind,
    /// User id, group name or role name: the key of the principal's page.
    pub key: String,
    /// User: name, else email. Group or role: its name.
    pub label: String,
    /// User: email, or `service account` for a service account. Group:
    /// `local` or `oidc`. Role: every scope defining the
    /// name, sorted and joined with ", " (`platform, workspace`).
    pub detail: Option<String>,
    /// The user's stored engine username, or null when none is allocated yet.
    /// Always null for a group or role.
    pub username: Option<String>,
    /// Group: direct members. Role: live allow holders that are users (a group
    /// holding the role is not counted; engines do not apply it). Null for a
    /// user. The same reads the compiler expands these principals with.
    pub member_count: Option<i64>,
    /// Group or role: how many of `memberCount` have no username yet. A role
    /// holder without one is not named on any engine. A group member without
    /// one is named only once a grant naming the group is compiled (which
    /// allocates it). Null for a user.
    pub without_username: Option<i64>,
    /// Whether the row is a Data API service account (users only; false
    /// otherwise). Engines enforce grants on it like on any user.
    pub service_account: bool,
}

/// A page of the People index and how many rows match in all.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeoplePage {
    pub items: Vec<PersonRow>,
    pub total: i64,
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/people",
    tag = "perm",
    params(PeopleQuery),
    responses(
        (status = 200, description = "Users, groups or roles, filtered and paged. Users exclude service accounts unless `includeServiceAccounts=true` or `username` is given (a service account row is marked `serviceAccount`), and are sorted by label; groups and roles by name. A role is one row per name, whatever scopes define it. Group member and role holder counts are the compiler's own membership reads", body = PeoplePage),
        (status = 400, description = "`kind` missing or not `user`, `group` or `role`; `username` or `includeServiceAccounts` with a kind other than `user`; or a negative `limit` or `offset`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn list_people(
    State(st): State<PermState>,
    Auth(p): Auth,
    Query(qp): Query<PeopleQuery>,
) -> Result<Json<PeoplePage>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let kind = match qp.kind.as_deref() {
        Some("user") => ListKind::User,
        Some("group") => ListKind::Group,
        Some("role") => ListKind::Role,
        _ => {
            return Err(PermHttpError::BadRequest(
                "kind must be user, group or role".to_string(),
            ))
        }
    };
    let username = non_empty(&qp.username);
    if username.is_some() && kind != ListKind::User {
        return Err(PermHttpError::BadRequest(
            "username applies to kind=user only".to_string(),
        ));
    }
    if qp.include_service_accounts.is_some() && kind != ListKind::User {
        return Err(PermHttpError::BadRequest(
            "includeServiceAccounts applies to kind=user only".to_string(),
        ));
    }
    let limit = qp.limit.unwrap_or(DEFAULT_LIMIT);
    let offset = qp.offset.unwrap_or(0);
    if limit < 0 || offset < 0 {
        return Err(PermHttpError::BadRequest(
            "limit and offset must not be negative".to_string(),
        ));
    }
    let filter = PeopleFilter {
        kind,
        q: non_empty(&qp.q),
        username,
        include_service_accounts: qp.include_service_accounts.unwrap_or(false),
        limit: limit.min(MAX_LIMIT),
        offset,
    };

    // One read-only snapshot, as `list_overlaps`: the count and the page (and
    // the counts read for the page) see one state.
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(PermHttpError::db)?;
    let list = list_people_via(&mut tx, &filter).await?;
    tx.rollback().await.map_err(PermHttpError::db)?;

    let (items, total) = match list {
        PeopleList::Users(rows, total) => (
            rows.into_iter()
                .map(|u| PersonRow {
                    kind: PersonKind::User,
                    key: u.id.to_string(),
                    label: match u.name.trim() {
                        "" => u.email.clone(),
                        name => name.to_string(),
                    },
                    detail: Some(if u.service_account {
                        "service account".to_string()
                    } else {
                        u.email
                    }),
                    username: u.username,
                    member_count: None,
                    without_username: None,
                    service_account: u.service_account,
                })
                .collect(),
            total,
        ),
        PeopleList::Groups(rows, total) => (
            rows.into_iter()
                .map(|g| PersonRow {
                    kind: PersonKind::Group,
                    key: g.name.clone(),
                    label: g.name,
                    detail: Some(g.source),
                    username: None,
                    member_count: Some(g.member_count),
                    without_username: Some(g.without_username),
                    service_account: false,
                })
                .collect(),
            total,
        ),
        PeopleList::Roles(rows, total) => (
            rows.into_iter()
                .map(|r| PersonRow {
                    kind: PersonKind::Role,
                    key: r.name.clone(),
                    label: r.name,
                    detail: Some(r.scopes.join(", ")),
                    username: None,
                    member_count: Some(r.holder_count),
                    without_username: Some(r.without_username),
                    service_account: false,
                })
                .collect(),
            total,
        ),
    };
    Ok(Json(PeoplePage { items, total }))
}

/// Run one principal read in a read-only snapshot, as `list_people` does: the
/// memberships, the compiler's reads and the gates see one state.
macro_rules! snapshot {
    ($st:expr, |$conn:ident| $read:expr) => {{
        let mut tx = $st.perm.pool().begin().await.map_err(PermHttpError::db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(PermHttpError::db)?;
        let $conn: &mut sqlx::PgConnection = &mut tx;
        let out = $read.await?;
        tx.rollback().await.map_err(PermHttpError::db)?;
        out
    }};
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/people/users/{user_id}",
    tag = "perm",
    params(("user_id" = Uuid, Path, description = "The user's id")),
    responses(
        (status = 200, description = "The user: identity, memberships (groups, roles and workspaces, direct and through a group, expired ones included and marked), `standsFor` (every grant principal that reaches the user, and whether engines apply it) and a clearance answer per gated tag, for engines and for egret. A service account is read like any user", body = UserPrincipal),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No user has this id", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn read_user(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(user_id): Path<Uuid>,
) -> Result<Json<UserPrincipal>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let user = snapshot!(st, |conn| read_user_via(conn, user_id));
    user.map(Json)
        .ok_or_else(|| PermHttpError::Missing("No user has this id.".to_string()))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/people/groups/{name}",
    tag = "perm",
    params(("name" = String, Path, description = "The group's name, as a grant principal stores it; `public` for everyone")),
    responses(
        (status = 200, description = "The group: source, direct members (as the compiler reads them; service accounts included), the roles and workspaces it holds (egret only), `standsFor` and clearances. `public` answers `isPublic: true` with no members", body = GroupPrincipal),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No group has this name", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn read_group(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<GroupPrincipal>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let group = snapshot!(st, |conn| read_group_via(conn, &name));
    group
        .map(Json)
        .ok_or_else(|| PermHttpError::Missing(format!("No group is named {name}.")))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/people/roles/{name}",
    tag = "perm",
    params(("name" = String, Path, description = "The role's name, across every scope defining it")),
    responses(
        (status = 200, description = "The role: scopes, live holders that are users (as the compiler reads them; service accounts included), groups holding it (egret only), `standsFor` and clearances", body = RolePrincipal),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No role has this name", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn read_role(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(name): Path<String>,
) -> Result<Json<RolePrincipal>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let role = snapshot!(st, |conn| read_role_via(conn, &name));
    role.map(Json)
        .ok_or_else(|| PermHttpError::Missing(format!("No role is named {name}.")))
}

// ---------------------------------------------------------------------------
// POST /perm/explain/principal (Task 4, spec §4.3, P5, P6)
// ---------------------------------------------------------------------------

/// At most this many members are evaluated per request (P5).
pub const EXPLAIN_MEMBER_LIMIT: usize = 200;

/// A group or a role, as a grant principal stores it.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainPrincipalRef {
    pub kind: egret_perm::PrincipalKind,
    pub r#ref: String,
}

/// "Can this group or role do `action` on `target`?", answered as its
/// members' answers. **Nothing about membership is caller-supplied**: the
/// members are read by the server, as `ExplainSubject` reads nothing but a
/// user name.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExplainPrincipalRequest {
    pub principal: ExplainPrincipalRef,
    pub action: egret_perm::Action,
    pub target: egret_perm::Scope,
}

/// One evaluated member: exactly `POST /perm/explain`'s answer for them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberVerdict {
    pub user_id: Uuid,
    pub label: String,
    pub username: String,
    pub result: super::ExplainResponse,
}

/// Why a member was not evaluated.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotEvaluatedReason {
    /// No `users.username`: no engine knows this person, so there is no
    /// request any engine could make for them.
    NoUsername,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberNotEvaluated {
    pub user_id: Uuid,
    pub label: String,
    pub reason: NotEvaluatedReason,
}

/// The members' answers. There is no overall verdict (P5): a group's access
/// is its members' access, and they can differ.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainPrincipalResponse {
    /// Every member, with or without a username.
    pub members_total: usize,
    /// [`EXPLAIN_MEMBER_LIMIT`].
    pub limit: usize,
    /// More members with a username than `limit`: the rest were not evaluated
    /// (`membersTotal - evaluated - notEvaluated` of them).
    pub truncated: bool,
    /// Sorted by username; the first `limit` members with one.
    pub evaluated: Vec<MemberVerdict>,
    /// Every member with no username, sorted by label.
    pub not_evaluated: Vec<MemberNotEvaluated>,
}

#[utoipa::path(
    post,
    path = "/api/control-plane/perm/explain/principal",
    tag = "perm",
    request_body = ExplainPrincipalRequest,
    responses(
        (status = 200, description = "Each member of the group or role (as the compiler reads them) evaluated as a user over the artifacts each engine downloads, exactly as `/perm/explain` answers for that user. At most 200 are evaluated, sorted by username; members with no username are listed as not evaluated. There is no overall verdict", body = ExplainPrincipalResponse),
        (status = 400, description = "The principal is `public`, a workspace, a user (ask `/perm/explain`), or a group or role no one has; or the target is refused as `/perm/explain` refuses it", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission, or a workspace-scoped impersonation overlay asking about a catalog that workspace is not assigned", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn explain_principal(
    State(st): State<PermState>,
    Auth(p): Auth,
    Json(body): Json<ExplainPrincipalRequest>,
) -> Result<Json<ExplainPrincipalResponse>, PermHttpError> {
    use egret_perm::PrincipalKind;
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let name = body.principal.r#ref.as_str();
    match body.principal.kind {
        PrincipalKind::User => {
            return Err(PermHttpError::BadRequest(
                "a user is explained by POST /perm/explain; this route takes a group or a role"
                    .to_string(),
            ))
        }
        PrincipalKind::WorkspaceMembers => {
            return Err(PermHttpError::BadRequest(
                "a workspace is not explained here: ask about one of its members with \
                 POST /perm/explain"
                    .to_string(),
            ))
        }
        PrincipalKind::Group if name == egret_perm::PUBLIC_GROUP => {
            return Err(PermHttpError::BadRequest(
                "public is everyone and has no members to list: ask about one user with \
                 POST /perm/explain"
                    .to_string(),
            ))
        }
        PrincipalKind::Group | PrincipalKind::Role => {}
    }
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    super::check_explain_target(&mut conn, &p, &body.target).await?;
    let Some(members) = principal_members_via(&mut conn, body.principal.kind, name).await? else {
        return Err(PermHttpError::BadRequest(format!(
            "no {} is named {name}",
            body.principal.kind.as_str()
        )));
    };
    let members_total = members.len();
    let (mut named, unnamed): (Vec<_>, Vec<_>) =
        members.into_iter().partition(|m| m.username.is_some());
    named.sort_by(|a, b| (&a.username, a.user_id).cmp(&(&b.username, b.user_id)));
    let truncated = named.len() > EXPLAIN_MEMBER_LIMIT;
    named.truncate(EXPLAIN_MEMBER_LIMIT);
    let usernames: Vec<String> = named
        .iter()
        .map(|m| m.username.clone().unwrap_or_default())
        .collect();
    // Nobody to evaluate: no artifact is read (a group of people with no
    // username is answered by `notEvaluated` alone).
    let results = if usernames.is_empty() {
        Vec::new()
    } else {
        super::explain_users(&mut conn, &usernames, body.action, &body.target).await?
    };
    let evaluated = named
        .into_iter()
        .zip(results)
        .map(|(m, result)| MemberVerdict {
            user_id: m.user_id,
            label: m.label,
            username: m.username.unwrap_or_default(),
            result,
        })
        .collect();
    let not_evaluated = unnamed
        .into_iter()
        .map(|m| MemberNotEvaluated {
            user_id: m.user_id,
            label: m.label,
            reason: NotEvaluatedReason::NoUsername,
        })
        .collect();
    Ok(Json(ExplainPrincipalResponse {
        members_total,
        limit: EXPLAIN_MEMBER_LIMIT,
        truncated,
        evaluated,
        not_evaluated,
    }))
}
