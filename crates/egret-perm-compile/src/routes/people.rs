//! `GET /perm/people` — the People & groups index (spec §4.2). Gated
//! `policy:read` (Q2, user 2026-10-17), as `list_grants` is. The reads, and
//! why their counts are the compiler's, are in [`crate::people`].

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;

use super::{PermHttpError, PermState};
use crate::people::{list_people_via, ListKind, PeopleFilter, PeopleList};

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
    /// Exact, case-insensitive username lookup. `kind=user` only.
    #[serde(default)]
    pub username: Option<String>,
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
    /// User: email. Group: `local` or `oidc`. Role: every scope defining the
    /// name, sorted and joined with ", " (`platform, workspace`).
    pub detail: Option<String>,
    /// The user's stored engine username, or null when none is allocated yet.
    /// Always null for a group or role.
    pub username: Option<String>,
    /// Group: direct members. Role: live allow holders that are users (a group
    /// holding the role is not counted; engines do not apply it). Null for a
    /// user. The same reads the compiler expands these principals with.
    pub member_count: Option<i64>,
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
        (status = 200, description = "Users, groups or roles, filtered and paged. Users exclude service accounts and are sorted by label; groups and roles by name. A role is one row per name, whatever scopes define it. Group member and role holder counts are the compiler's own membership reads", body = PeoplePage),
        (status = 400, description = "`kind` missing or not `user`, `group` or `role`; `username` with a kind other than `user`; or a negative `limit` or `offset`", body = ErrorBody),
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
                    label: if u.name.is_empty() {
                        u.email.clone()
                    } else {
                        u.name
                    },
                    detail: Some(u.email),
                    username: u.username,
                    member_count: None,
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
                })
                .collect(),
            total,
        ),
    };
    Ok(Json(PeoplePage { items, total }))
}
