//! Reading people for Governance's People & groups pages (spec
//! `docs/en/specs/2026-10-17-dg-people-groups-design.md` §4).
//!
//! **Membership is never defined here.** A group's member count and a role's
//! holder count come from [`crate::members::group_members_via`] and
//! [`crate::members::role_holders_via`], the reads the compiler expands a
//! `group` and a `role` principal with (spec §4.1, P3). A count is therefore
//! the membership the compiler READS. It is not the same as the people a
//! compiled policy NAMES, and the difference is people with no
//! `users.username`, so it is reported beside the count as `without_username`:
//!
//! a group member or role holder with no username is named only once a grant
//! naming the group or role is compiled, which mints the username
//! (`members.rs`, "A member with no username yet is MINTED"; for a role holder
//! since the 2026-10-18 design R5, `members::role_usernames`). Until then, no
//! policy names them.
//!
//! What this module adds is only the list itself: which users, groups and
//! roles exist, filtered and paged.
//!
//! **What a list row reveals.** Identity only: a user's id, display name,
//! email and engine username; a group's name and source; a role's name and
//! scopes; and a count. The list reads no password hash, session, permission,
//! status or login time, so none can leak into it (Q2: the routes are gated
//! `policy:read`). A single user's page (`UserPrincipal`) also carries the
//! account's `status`, so a steward can see a disabled account; it still reads
//! no password hash, session or login time.
//!
//! **Service accounts are listed, marked.** A Data API service account is a
//! `users` row with `provider = 'service'` (migration `0119`). Engines enforce
//! grants on it like any user: a role it holds expands to its username, and a
//! grant can name its username directly (`crate::principal_refs` has no
//! provider filter). Hiding it would leave a principal engines act on with no
//! row anywhere. Migration 0119 asks human-facing lists to leave it out, so
//! browsing leaves it out by default (`include_service_accounts`). A
//! `username` lookup always finds it, and every row says whether it is one.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use egret_perm::CompileError;

use crate::members::{group_members_via, role_holders_via};

/// Which of the three listable principal kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    User,
    Group,
    Role,
}

/// The filter and page of one list read. `q` and `username` are already
/// trimmed of the "empty means absent" case by the caller.
#[derive(Debug)]
pub struct PeopleFilter<'a> {
    pub kind: ListKind,
    /// Case-insensitive substring of a user's name, email or username, or of a
    /// group's or role's name. Matched with `strpos`, so `%` and `_` are
    /// literal characters, never patterns.
    pub q: Option<&'a str>,
    /// Exact, case-insensitive (`citext`) username. Users only. A lookup
    /// finds a service account whatever `include_service_accounts` says.
    pub username: Option<&'a str>,
    /// Users only: whether browsing lists service accounts too.
    pub include_service_accounts: bool,
    pub limit: i64,
    pub offset: i64,
}

/// One listed user.
#[derive(Debug, sqlx::FromRow)]
pub struct ListedUser {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub username: Option<String>,
    pub service_account: bool,
}

/// One listed group, with its direct member count and how many of those have
/// no username yet.
#[derive(Debug)]
pub struct ListedGroup {
    pub name: String,
    pub source: String,
    pub member_count: i64,
    pub without_username: i64,
}

/// One listed role NAME — once, however many scopes define it — with every
/// scope defining it (sorted) and its live allow holders (users).
#[derive(Debug)]
pub struct ListedRole {
    pub name: String,
    pub scopes: Vec<String>,
    pub holder_count: i64,
    pub without_username: i64,
}

/// A page of one kind, and how many rows match the filter in all.
#[derive(Debug)]
pub enum PeopleList {
    Users(Vec<ListedUser>, i64),
    Groups(Vec<ListedGroup>, i64),
    Roles(Vec<ListedRole>, i64),
}

fn db(e: sqlx::Error) -> CompileError {
    CompileError::Db(e.to_string())
}

/// The users filter, shared by the count and the page so the two cannot
/// disagree. `$1` is `q`, `$2` is `username`, `$3` is
/// `include_service_accounts`. A macro, not a `const`, so
/// `concat!` keeps every statement a `&'static str` literal.
macro_rules! users_where {
    () => {
        "WHERE ($3::bool OR $2::text IS NOT NULL OR provider <> 'service') \
       AND ($1::text IS NULL \
            OR strpos(lower(name), lower($1::text)) > 0 \
            OR strpos(lower(email::text), lower($1::text)) > 0 \
            OR strpos(lower(COALESCE(username::text, '')), lower($1::text)) > 0) \
       AND ($2::text IS NULL OR username = $2::text::citext)"
    };
}

/// List one kind of principal. Runs on the caller's connection, so a route
/// can hold the count and the page in one snapshot.
pub async fn list_people_via(
    conn: &mut sqlx::PgConnection,
    f: &PeopleFilter<'_>,
) -> Result<PeopleList, CompileError> {
    match f.kind {
        ListKind::User => {
            let total: i64 =
                sqlx::query_scalar(concat!("SELECT count(*) FROM users ", users_where!()))
                    .bind(f.q)
                    .bind(f.username)
                    .bind(f.include_service_accounts)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(db)?;
            let rows: Vec<ListedUser> = sqlx::query_as(concat!(
                "SELECT id, name, email::text AS email, username::text AS username, \
                        provider = 'service' AS service_account FROM users ",
                users_where!(),
                " ORDER BY lower(COALESCE(NULLIF(btrim(name), ''), email::text)), id \
                  LIMIT $4 OFFSET $5"
            ))
            .bind(f.q)
            .bind(f.username)
            .bind(f.include_service_accounts)
            .bind(f.limit)
            .bind(f.offset)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?;
            Ok(PeopleList::Users(rows, total))
        }
        ListKind::Group => {
            let total: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM groups \
                  WHERE $1::text IS NULL OR strpos(lower(name), lower($1::text)) > 0",
            )
            .bind(f.q)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT name, source FROM groups \
                  WHERE $1::text IS NULL OR strpos(lower(name), lower($1::text)) > 0 \
                  ORDER BY lower(name), name \
                  LIMIT $2 OFFSET $3",
            )
            .bind(f.q)
            .bind(f.limit)
            .bind(f.offset)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?;
            let names: Vec<String> = rows.iter().map(|(n, _)| n.clone()).collect();
            let mut members: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
            let mut unnamed: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
            for m in group_members_via(conn, &names).await? {
                if m.username.is_none() {
                    unnamed
                        .entry(m.group_name.clone())
                        .or_default()
                        .insert(m.user_id);
                }
                members.entry(m.group_name).or_default().insert(m.user_id);
            }
            let groups = rows
                .into_iter()
                .map(|(name, source)| ListedGroup {
                    member_count: count(members.get(&name)),
                    without_username: count(unnamed.get(&name)),
                    name,
                    source,
                })
                .collect();
            Ok(PeopleList::Groups(groups, total))
        }
        ListKind::Role => {
            let total: i64 = sqlx::query_scalar(
                "SELECT count(DISTINCT name) FROM roles \
                  WHERE $1::text IS NULL OR strpos(lower(name), lower($1::text)) > 0",
            )
            .bind(f.q)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
            let rows: Vec<(String, Vec<String>)> = sqlx::query_as(
                "SELECT name, array_agg(DISTINCT scope ORDER BY scope) AS scopes FROM roles \
                  WHERE $1::text IS NULL OR strpos(lower(name), lower($1::text)) > 0 \
                  GROUP BY name \
                  ORDER BY lower(name), name \
                  LIMIT $2 OFFSET $3",
            )
            .bind(f.q)
            .bind(f.limit)
            .bind(f.offset)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?;
            let names: Vec<String> = rows.iter().map(|(n, _)| n.clone()).collect();
            // Deduplicated by user: one person holding the role at two scopes,
            // or twice at one, is one holder.
            let mut holders: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
            let mut unnamed: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
            for h in role_holders_via(conn, &names).await? {
                if h.username.is_none() {
                    unnamed
                        .entry(h.role_name.clone())
                        .or_default()
                        .insert(h.user_id);
                }
                holders.entry(h.role_name).or_default().insert(h.user_id);
            }
            let roles = rows
                .into_iter()
                .map(|(name, scopes)| ListedRole {
                    holder_count: count(holders.get(&name)),
                    without_username: count(unnamed.get(&name)),
                    name,
                    scopes,
                })
                .collect();
            Ok(PeopleList::Roles(roles, total))
        }
    }
}

fn count(set: Option<&BTreeSet<Uuid>>) -> i64 {
    set.map_or(0, |s| i64::try_from(s.len()).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// One principal: memberships, `standsFor`, clearances (Task 3, spec §4.2)
// ---------------------------------------------------------------------------
//
// **Where each answer comes from.** "Do engines apply this membership" is
// answered by the compiler's own reads, never by a predicate written here:
// a user's group by [`group_members_via`], a role by [`role_holders_via`], a
// workspace by [`workspace_members_via`] (spec §1, §4.1). What this module
// reads itself is only what those reads deliberately leave out, so that the
// page can SHOW it, marked: expired rows (spec P3), and the roles and
// workspaces a GROUP holds, which egret's own authoriser counts
// (`crates/egret-auth/src/grants.rs:47-60`) and the compiler does not
// (`members.rs` module doc, narrowing 1). Deny-effect `access_grants` rows are
// not memberships for either (`members.rs` `role_holders_via`; the PDP's
// clearance roles filter `effect = 'allow'`, `egret_auth::pdp`), so none is
// listed.
//
// **No username, no engine identity.** The bare username is the only name an
// engine sees (`members.rs` module doc), so for a user with none every entry
// is `no_username`. For a group, workspace or role membership that is "not
// YET": a compile of a grant naming the group, workspace or role mints one
// (`members.rs`, "A member with no username yet is MINTED"; a role holder since
// the 2026-10-18 design R5, `members.rs` `role_usernames`).

use chrono::{DateTime, Utc};
use serde::Serialize;

use egret_perm::{PrincipalKind, PUBLIC_GROUP};

use crate::members::workspace_members_via;

/// How a principal comes to stand for another (spec §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Via {
    /// The principal itself (`user:<username>`, `group:<name>`, `role:<name>`).
    Itself,
    /// A group the user is a direct member of.
    Group { name: String },
    /// A role held directly (a user's or, on a group's page, the group's).
    Role { name: String },
    /// A workspace held directly. `name` is the workspace's name, or its id
    /// when no workspace has that id any more.
    Workspace { id: Uuid, name: String },
    /// `group:public`: everyone.
    Public,
    /// A role held by one of the user's groups (egret only).
    GroupRole { group: String, role: String },
    /// A workspace held by one of the user's groups (egret only).
    GroupWorkspace {
        group: String,
        id: Uuid,
        name: String,
    },
}

/// Why engines do not apply a membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotOnEngines {
    /// Held by a group: the compiler expands a `group` principal to its
    /// members, but not a group's own roles or workspace memberships
    /// (`members.rs` module doc, narrowing 1; measured, live doc §1.1 C3b).
    HeldThroughGroup,
    /// Expired. The compiler reads only unexpired rows, but a lapse is not a
    /// write, so engines may apply it until the profile next recompiles
    /// (`members.rs` module doc, narrowing 2).
    Expired,
    /// The user has no `users.username`, so no engine knows them.
    NoUsername,
}

/// The scope of an `access_grants` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MembershipScope {
    Platform,
    Workspace {
        id: Uuid,
        name: String,
    },
    Object {
        id: Uuid,
        #[serde(rename = "objectType")]
        object_type: Option<String>,
    },
}

/// A principal, `(kind, ref)` exactly as a grant or rule stores it, that
/// stands for the principal being read (spec P4). The client lists a grant or
/// rule when one of its principals equals an entry byte for byte — except a
/// `workspace_members` ref, which is compared as a parsed UUID: writes store
/// the canonical form (`crate::principal_refs`), but a non-canonical ref stored
/// before that is kept as written and still applies on engines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StandsFor {
    pub kind: PrincipalKind,
    pub r#ref: String,
    pub via: Via,
    pub on_engines: bool,
    pub not_on_engines: Option<NotOnEngines>,
}

/// One membership: a group, a role grant, or a workspace grant.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub via: Via,
    /// The role of a role or workspace grant; null for a group membership.
    pub role: Option<String>,
    /// The grant's scope; null for a group membership (it has none).
    pub scope: Option<MembershipScope>,
    pub expires_at: Option<DateTime<Utc>>,
    pub expired: bool,
    pub on_engines: bool,
    pub not_on_engines: Option<NotOnEngines>,
}

/// A group member or a role holder.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberRef {
    pub user_id: Uuid,
    /// Name, else email.
    pub label: String,
    pub username: Option<String>,
    /// A Data API service account (migration `0119`). Listed like anyone:
    /// a grant can name it and engines enforce on it.
    pub service_account: bool,
    /// False only for a person with no username (`noUsername`).
    pub on_engines: bool,
    pub not_on_engines: Option<NotOnEngines>,
}

/// A group holding a role (egret only).
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupRef {
    pub name: String,
    pub source: String,
}

/// Whether engines clear a principal for a gated tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineClearance {
    /// Directly holds `role`, unexpired, and has a username: the compiled
    /// deny excepts them (`clearance.rs:33-58`, `members.rs:146-160`).
    Cleared { role: String },
    /// Not excepted; `because` says why when they hold a required role in a
    /// way engines do not apply, or are in a required group (whose half of
    /// the gate engines never enforce).
    NotCleared {
        because: Option<EngineNotClearedReason>,
    },
    /// The gate names only groups, so nothing is compiled for it
    /// (`clearance.rs:266-276`; measured, live doc §1.1 C6).
    NotEnforcedGroupsOnly,
}

/// Why engines do not clear a principal egret may clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineNotClearedReason {
    /// A required role held through a group ([`NotOnEngines::HeldThroughGroup`]).
    HeldThroughGroup,
    /// A required role held directly but expired ([`NotOnEngines::Expired`]).
    Expired,
    /// A required role held directly, but no username ([`NotOnEngines::NoUsername`]).
    NoUsername,
    /// In a required group (or, on a group's page, the group itself is
    /// required): the gate's group half compiles to nothing on engines
    /// (`clearance.rs:61-70`; D7: a Ranger item naming a group matches nobody),
    /// so only its roles are enforced there. Given only when no reason above
    /// applies.
    GroupsNotEnforced,
}

/// Whether egret's own object-access check clears a principal for a gated
/// tag (`crates/egret-auth/src/pdp/templates.rs:82-106`): a required role
/// held directly or through a group (`crates/egret-auth/src/grants.rs:47-60`),
/// membership in a required group, or the platform-admin exemption.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EgretClearance {
    /// `scope` is the clearing role grant's: a workspace role clears only that
    /// workspace's objects, an object role only that object. Null for a group
    /// membership or the principal itself.
    Cleared {
        via: Via,
        scope: Option<MembershipScope>,
    },
    NotCleared,
}

/// One gated tag and both answers.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearanceFact {
    pub tag_id: Uuid,
    pub tag_key: String,
    pub tag_name: String,
    pub required_roles: Vec<String>,
    /// Group NAMES (a gate stores ids; an id no group has any more is shown
    /// as the id).
    pub required_groups: Vec<String>,
    pub engines: EngineClearance,
    pub egret: EgretClearance,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserPrincipal {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub username: Option<String>,
    /// `active` or `disabled`.
    pub status: String,
    pub service_account: bool,
    pub groups: Vec<Membership>,
    /// Role grants, direct and through a group.
    pub roles: Vec<Membership>,
    /// Workspace-scoped grants, direct and through a group.
    pub workspaces: Vec<Membership>,
    pub stands_for: Vec<StandsFor>,
    pub clearances: Vec<ClearanceFact>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupPrincipal {
    pub name: String,
    /// `local` or `oidc`; null for `public`.
    pub source: Option<String>,
    pub is_public: bool,
    /// Direct members, as the compiler reads them; empty for `public`.
    pub members: Vec<MemberRef>,
    /// How many of `members` are service accounts.
    pub service_accounts: i64,
    pub roles: Vec<Membership>,
    pub workspaces: Vec<Membership>,
    pub stands_for: Vec<StandsFor>,
    pub clearances: Vec<ClearanceFact>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolePrincipal {
    pub name: String,
    /// Every scope defining the name, sorted.
    pub scopes: Vec<String>,
    /// Live allow holders that are users, as the compiler reads them.
    pub holders: Vec<MemberRef>,
    /// How many of `holders` are service accounts.
    pub service_accounts: i64,
    /// Groups holding the role, unexpired (egret only).
    pub holder_groups: Vec<GroupRef>,
    pub stands_for: Vec<StandsFor>,
    pub clearances: Vec<ClearanceFact>,
}

/// One allow `access_grants` row of a user or group, expired or not.
#[derive(sqlx::FromRow)]
struct HeldRow {
    principal_id: Uuid,
    role_name: String,
    scope_type: String,
    scope_id: Option<Uuid>,
    object_type: Option<String>,
    workspace_name: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    expired: bool,
}

impl HeldRow {
    fn scope(&self) -> MembershipScope {
        let id = self.scope_id.unwrap_or_default();
        match self.scope_type.as_str() {
            "workspace" => MembershipScope::Workspace {
                id,
                name: self.ws_name(),
            },
            "object" => MembershipScope::Object {
                id,
                object_type: self.object_type.clone(),
            },
            _ => MembershipScope::Platform,
        }
    }
    fn ws_name(&self) -> String {
        self.workspace_name.clone().unwrap_or_default()
    }
    fn workspace(&self) -> Option<Uuid> {
        (self.scope_type == "workspace").then(|| self.scope_id.unwrap_or_default())
    }
    /// Platform first, then workspace, then object: the widest clearance.
    fn breadth(&self) -> u8 {
        match self.scope_type.as_str() {
            "platform" => 0,
            "workspace" => 1,
            _ => 2,
        }
    }
}

/// The allow role grants of `ids` (users or groups), expired ones included,
/// so a page can show them marked.
async fn held_rows_via(
    conn: &mut sqlx::PgConnection,
    principal_type: &str,
    ids: &[Uuid],
) -> Result<Vec<HeldRow>, CompileError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT ag.principal_id, r.name AS role_name, ag.scope_type, ag.scope_id, \
                ag.object_type, \
                CASE WHEN ag.scope_type = 'workspace' \
                     THEN COALESCE(w.name, ag.scope_id::text) END AS workspace_name, \
                ag.expires_at, \
                (ag.expires_at IS NOT NULL AND ag.expires_at <= now()) AS expired \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
           LEFT JOIN workspaces w ON ag.scope_type = 'workspace' AND w.id = ag.scope_id \
          WHERE ag.principal_type = $1 AND ag.principal_id = ANY($2) \
            AND ag.effect = 'allow' \
          ORDER BY r.name, ag.scope_type, w.name, ag.scope_id, ag.principal_id",
    )
    .bind(principal_type)
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    name: String,
    email: String,
    username: Option<String>,
    status: String,
    service_account: bool,
}

fn label(name: &str, email: &str) -> String {
    match name.trim() {
        "" => email.to_string(),
        n => n.to_string(),
    }
}

/// The identity rows of users `ids`, unordered.
async fn user_rows_via(
    conn: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<Vec<UserRow>, CompileError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT id, name, email::text AS email, username::text AS username, status, \
                provider = 'service' AS service_account \
           FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)
}

/// Members or holders `ids` as rows, sorted by label.
async fn member_refs_via(
    conn: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<Vec<MemberRef>, CompileError> {
    let rows = user_rows_via(conn, ids).await?;
    let mut out: Vec<MemberRef> = rows
        .into_iter()
        .map(|u| {
            let (on_engines, not_on_engines) = mark(u.username.is_some(), true, None);
            MemberRef {
                user_id: u.id,
                label: label(&u.name, &u.email),
                username: u.username,
                service_account: u.service_account,
                on_engines,
                not_on_engines,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.label.to_lowercase(), a.user_id).cmp(&(b.label.to_lowercase(), b.user_id))
    });
    Ok(out)
}

/// The engine mark of one entry: no username wins (no engine knows the
/// person); else it applies, or `otherwise` says why not.
fn mark(
    has_username: bool,
    applies: bool,
    otherwise: Option<NotOnEngines>,
) -> (bool, Option<NotOnEngines>) {
    if !has_username {
        (false, Some(NotOnEngines::NoUsername))
    } else if applies {
        (true, None)
    } else {
        (false, otherwise)
    }
}

/// Collects `standsFor` entries, once per `(kind, ref, via)`.
#[derive(Default)]
struct StandsForList(Vec<StandsFor>);

impl StandsForList {
    fn push(&mut self, kind: PrincipalKind, r: String, via: Via, m: (bool, Option<NotOnEngines>)) {
        if self
            .0
            .iter()
            .any(|s| s.kind == kind && s.r#ref == r && s.via == via)
        {
            return;
        }
        self.0.push(StandsFor {
            kind,
            r#ref: r,
            via,
            on_engines: m.0,
            not_on_engines: m.1,
        });
    }
}

/// One gate: a tag with a non-empty `required_roles` or `required_groups`.
#[derive(sqlx::FromRow)]
struct Gate {
    tag_id: Uuid,
    tag_key: String,
    tag_name: String,
    required_roles: Vec<String>,
    required_groups: Vec<Uuid>,
}

/// Every gate, and the names of the groups they require. Both lists empty is
/// no gate (`clearance.rs`, "BOTH lists empty is NO gate").
async fn gates_via(
    conn: &mut sqlx::PgConnection,
) -> Result<(Vec<Gate>, BTreeMap<Uuid, String>), CompileError> {
    let mut gates: Vec<Gate> = sqlx::query_as(
        "SELECT cp.tag_id, t.key AS tag_key, t.name AS tag_name, cp.required_roles, \
                cp.required_groups \
           FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id \
          WHERE cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0 \
          ORDER BY lower(t.name), t.key",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    for g in &mut gates {
        g.required_roles.sort();
        g.required_roles.dedup();
    }
    let ids: Vec<Uuid> = gates
        .iter()
        .flat_map(|g| g.required_groups.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let names: Vec<(Uuid, String)> = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, name FROM groups WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
    };
    Ok((gates, names.into_iter().collect()))
}

fn fact(
    g: &Gate,
    group_names: &BTreeMap<Uuid, String>,
    engines: EngineClearance,
    egret: EgretClearance,
) -> ClearanceFact {
    let mut required_groups: Vec<String> = g
        .required_groups
        .iter()
        .map(|id| {
            group_names
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string())
        })
        .collect();
    required_groups.sort();
    ClearanceFact {
        tag_id: g.tag_id,
        tag_key: g.tag_key.clone(),
        tag_name: g.tag_name.clone(),
        required_roles: g.required_roles.clone(),
        required_groups,
        engines,
        egret,
    }
}

/// The name of the role whose platform-scoped grant exempts its holder from
/// every classification gate in egret (`egret_auth::pdp`: `is_platform_admin`
/// is a platform-scoped grant of the role with this name).
const PLATFORM_ADMIN: &str = "platform_admin";

/// The widest live row in `rows` granting one of `roles`.
fn clearing_row<'a>(rows: &'a [HeldRow], roles: &[String]) -> Option<&'a HeldRow> {
    rows.iter()
        .filter(|r| !r.expired && roles.contains(&r.role_name))
        .min_by_key(|r| (r.breadth(), r.role_name.clone()))
}

/// A live platform-scoped `platform_admin` row in `rows`.
fn admin_row(rows: &[HeldRow]) -> Option<&HeldRow> {
    rows.iter()
        .find(|r| !r.expired && r.scope_type == "platform" && r.role_name == PLATFORM_ADMIN)
}

/// One user: identity, memberships, `standsFor`, clearances. `None` when no
/// user has `id`. Service accounts are read like anyone (a grant can name
/// one).
pub async fn read_user_via(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<Option<UserPrincipal>, CompileError> {
    let user: Option<UserRow> = sqlx::query_as(
        "SELECT id, name, email::text AS email, username::text AS username, status, \
                provider = 'service' AS service_account \
           FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let Some(user) = user else {
        return Ok(None);
    };
    let has_username = user.username.is_some();

    let groups: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT g.id, g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
          WHERE ug.user_id = $1 ORDER BY g.name",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let group_names: Vec<String> = groups.iter().map(|(_, n)| n.clone()).collect();
    let group_ids: Vec<Uuid> = groups.iter().map(|(g, _)| *g).collect();
    let group_name_of: BTreeMap<Uuid, String> = groups.iter().cloned().collect();

    let direct = held_rows_via(conn, "user", &[id]).await?;
    let through = held_rows_via(conn, "group", &group_ids).await?;
    let (gates, gate_group_names) = gates_via(conn).await?;

    // What the compiler applies, from its own reads (spec §4.1).
    let in_group: BTreeSet<String> = group_members_via(conn, &group_names)
        .await?
        .into_iter()
        .filter(|m| m.user_id == id)
        .map(|m| m.group_name)
        .collect();
    let role_names: Vec<String> = direct
        .iter()
        .map(|r| r.role_name.clone())
        .chain(gates.iter().flat_map(|g| g.required_roles.iter().cloned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let holds: BTreeSet<String> = role_holders_via(conn, &role_names)
        .await?
        .into_iter()
        .filter(|h| h.user_id == id)
        .map(|h| h.role_name)
        .collect();
    let ws_ids: Vec<Uuid> = direct
        .iter()
        .filter_map(HeldRow::workspace)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let member_of: BTreeSet<Uuid> = workspace_members_via(conn, &ws_ids)
        .await?
        .into_iter()
        .filter(|m| m.user_id == id)
        .map(|m| m.workspace_id)
        .collect();

    let group_of = |r: &HeldRow| {
        group_name_of
            .get(&r.principal_id)
            .cloned()
            .unwrap_or_default()
    };
    let expired = Some(NotOnEngines::Expired);
    let through_group = Some(NotOnEngines::HeldThroughGroup);

    // Memberships.
    let group_memberships: Vec<Membership> = group_names
        .iter()
        .map(|name| {
            let (on_engines, not_on_engines) = mark(has_username, in_group.contains(name), None);
            Membership {
                via: Via::Group { name: name.clone() },
                role: None,
                scope: None,
                expires_at: None,
                expired: false,
                on_engines,
                not_on_engines,
            }
        })
        .collect();
    let membership = |via: Via, r: &HeldRow, m: (bool, Option<NotOnEngines>)| Membership {
        via,
        role: Some(r.role_name.clone()),
        scope: Some(r.scope()),
        expires_at: r.expires_at,
        expired: r.expired,
        on_engines: m.0,
        not_on_engines: m.1,
    };
    let mut roles = Vec::new();
    let mut workspaces = Vec::new();
    for r in &direct {
        roles.push(membership(
            Via::Role {
                name: r.role_name.clone(),
            },
            r,
            mark(
                has_username,
                !r.expired && holds.contains(&r.role_name),
                expired,
            ),
        ));
        if let Some(ws) = r.workspace() {
            workspaces.push(membership(
                Via::Workspace {
                    id: ws,
                    name: r.ws_name(),
                },
                r,
                mark(has_username, !r.expired && member_of.contains(&ws), expired),
            ));
        }
    }
    for r in &through {
        roles.push(membership(
            Via::GroupRole {
                group: group_of(r),
                role: r.role_name.clone(),
            },
            r,
            mark(has_username, false, through_group),
        ));
        if let Some(ws) = r.workspace() {
            workspaces.push(membership(
                Via::GroupWorkspace {
                    group: group_of(r),
                    id: ws,
                    name: r.ws_name(),
                },
                r,
                mark(has_username, false, through_group),
            ));
        }
    }

    // standsFor (spec §4.2).
    let mut sf = StandsForList::default();
    if let Some(username) = &user.username {
        sf.push(
            PrincipalKind::User,
            username.clone(),
            Via::Itself,
            (true, None),
        );
    }
    for name in &group_names {
        sf.push(
            PrincipalKind::Group,
            name.clone(),
            Via::Group { name: name.clone() },
            mark(has_username, in_group.contains(name), None),
        );
    }
    for r in &direct {
        sf.push(
            PrincipalKind::Role,
            r.role_name.clone(),
            Via::Role {
                name: r.role_name.clone(),
            },
            mark(has_username, holds.contains(&r.role_name), expired),
        );
    }
    for r in &direct {
        if let Some(ws) = r.workspace() {
            sf.push(
                PrincipalKind::WorkspaceMembers,
                ws.to_string(),
                Via::Workspace {
                    id: ws,
                    name: r.ws_name(),
                },
                mark(has_username, member_of.contains(&ws), expired),
            );
        }
    }
    sf.push(
        PrincipalKind::Group,
        PUBLIC_GROUP.to_string(),
        Via::Public,
        mark(has_username, true, None),
    );
    // A group's lapsed row reaches neither egret nor engines: not listed here
    // (it is on the Memberships, marked expired).
    for r in through.iter().filter(|r| !r.expired) {
        sf.push(
            PrincipalKind::Role,
            r.role_name.clone(),
            Via::GroupRole {
                group: group_of(r),
                role: r.role_name.clone(),
            },
            mark(has_username, false, through_group),
        );
    }
    for r in through.iter().filter(|r| !r.expired) {
        if let Some(ws) = r.workspace() {
            sf.push(
                PrincipalKind::WorkspaceMembers,
                ws.to_string(),
                Via::GroupWorkspace {
                    group: group_of(r),
                    id: ws,
                    name: r.ws_name(),
                },
                mark(has_username, false, through_group),
            );
        }
    }

    // Clearances.
    let clearances = gates
        .iter()
        .map(|g| {
            let engines = if g.required_roles.is_empty() {
                EngineClearance::NotEnforcedGroupsOnly
            } else {
                match g.required_roles.iter().find(|r| holds.contains(*r)) {
                    Some(role) if has_username => EngineClearance::Cleared { role: role.clone() },
                    Some(_) => EngineClearance::NotCleared {
                        because: Some(EngineNotClearedReason::NoUsername),
                    },
                    None => EngineClearance::NotCleared {
                        because: if direct
                            .iter()
                            .any(|r| r.expired && g.required_roles.contains(&r.role_name))
                        {
                            Some(EngineNotClearedReason::Expired)
                        } else if clearing_row(&through, &g.required_roles).is_some() {
                            Some(EngineNotClearedReason::HeldThroughGroup)
                        } else if group_ids.iter().any(|gid| g.required_groups.contains(gid)) {
                            Some(EngineNotClearedReason::GroupsNotEnforced)
                        } else {
                            None
                        },
                    },
                }
            };
            let egret = if let Some(r) = clearing_row(&direct, &g.required_roles) {
                EgretClearance::Cleared {
                    via: Via::Role {
                        name: r.role_name.clone(),
                    },
                    scope: Some(r.scope()),
                }
            } else if let Some((_, name)) = groups
                .iter()
                .find(|(gid, _)| g.required_groups.contains(gid))
            {
                EgretClearance::Cleared {
                    via: Via::Group { name: name.clone() },
                    scope: None,
                }
            } else if let Some(r) = clearing_row(&through, &g.required_roles) {
                EgretClearance::Cleared {
                    via: Via::GroupRole {
                        group: group_of(r),
                        role: r.role_name.clone(),
                    },
                    scope: Some(r.scope()),
                }
            } else if let Some(r) = admin_row(&direct) {
                EgretClearance::Cleared {
                    via: Via::Role {
                        name: r.role_name.clone(),
                    },
                    scope: Some(r.scope()),
                }
            } else if let Some(r) = admin_row(&through) {
                EgretClearance::Cleared {
                    via: Via::GroupRole {
                        group: group_of(r),
                        role: r.role_name.clone(),
                    },
                    scope: Some(r.scope()),
                }
            } else {
                EgretClearance::NotCleared
            };
            fact(g, &gate_group_names, engines, egret)
        })
        .collect();

    Ok(Some(UserPrincipal {
        id: user.id,
        email: user.email,
        name: user.name,
        username: user.username,
        status: user.status,
        service_account: user.service_account,
        groups: group_memberships,
        roles,
        workspaces,
        stands_for: sf.0,
        clearances,
    }))
}

fn service_accounts(refs: &[MemberRef]) -> i64 {
    i64::try_from(refs.iter().filter(|m| m.service_account).count()).unwrap_or(i64::MAX)
}

/// One group. `public` answers the `public` shape whatever the `groups`
/// table holds: the compiler never expands it, every engine resolves it
/// itself (`members.rs` `is_expanded_group`). `None` when no group has `name`.
pub async fn read_group_via(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<Option<GroupPrincipal>, CompileError> {
    if name == PUBLIC_GROUP {
        return Ok(Some(GroupPrincipal {
            name: PUBLIC_GROUP.to_string(),
            source: None,
            is_public: true,
            members: Vec::new(),
            service_accounts: 0,
            roles: Vec::new(),
            workspaces: Vec::new(),
            stands_for: vec![StandsFor {
                kind: PrincipalKind::Group,
                r#ref: PUBLIC_GROUP.to_string(),
                via: Via::Public,
                on_engines: true,
                not_on_engines: None,
            }],
            clearances: Vec::new(),
        }));
    }
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, source FROM groups WHERE name = $1")
            .bind(name)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?;
    let Some((gid, source)) = row else {
        return Ok(None);
    };
    let member_ids: Vec<Uuid> = group_members_via(conn, &[name.to_string()])
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let members = member_refs_via(conn, &member_ids).await?;
    let held = held_rows_via(conn, "group", &[gid]).await?;
    let (gates, gate_group_names) = gates_via(conn).await?;
    let through_group = (false, Some(NotOnEngines::HeldThroughGroup));

    let membership = |via: Via, r: &HeldRow| Membership {
        via,
        role: Some(r.role_name.clone()),
        scope: Some(r.scope()),
        expires_at: r.expires_at,
        expired: r.expired,
        on_engines: false,
        not_on_engines: through_group.1,
    };
    let roles: Vec<Membership> = held
        .iter()
        .map(|r| {
            membership(
                Via::Role {
                    name: r.role_name.clone(),
                },
                r,
            )
        })
        .collect();
    let workspaces: Vec<Membership> = held
        .iter()
        .filter_map(|r| {
            r.workspace().map(|ws| {
                membership(
                    Via::Workspace {
                        id: ws,
                        name: r.ws_name(),
                    },
                    r,
                )
            })
        })
        .collect();

    let mut sf = StandsForList::default();
    // A grant to the group reaches its members: the compiler expands it.
    sf.push(
        PrincipalKind::Group,
        name.to_string(),
        Via::Itself,
        (true, None),
    );
    sf.push(
        PrincipalKind::Group,
        PUBLIC_GROUP.to_string(),
        Via::Public,
        (true, None),
    );
    for r in held.iter().filter(|r| !r.expired) {
        sf.push(
            PrincipalKind::Role,
            r.role_name.clone(),
            Via::Role {
                name: r.role_name.clone(),
            },
            through_group,
        );
    }
    for r in held.iter().filter(|r| !r.expired) {
        if let Some(ws) = r.workspace() {
            sf.push(
                PrincipalKind::WorkspaceMembers,
                ws.to_string(),
                Via::Workspace {
                    id: ws,
                    name: r.ws_name(),
                },
                through_group,
            );
        }
    }

    let clearances = gates
        .iter()
        .map(|g| {
            let held_role = clearing_row(&held, &g.required_roles);
            let engines = if g.required_roles.is_empty() {
                EngineClearance::NotEnforcedGroupsOnly
            } else {
                EngineClearance::NotCleared {
                    because: if held_role.is_some() {
                        Some(EngineNotClearedReason::HeldThroughGroup)
                    } else if g.required_groups.contains(&gid) {
                        Some(EngineNotClearedReason::GroupsNotEnforced)
                    } else {
                        None
                    },
                }
            };
            let egret = if g.required_groups.contains(&gid) {
                EgretClearance::Cleared {
                    via: Via::Itself,
                    scope: None,
                }
            } else if let Some(r) = held_role.or_else(|| admin_row(&held)) {
                EgretClearance::Cleared {
                    via: Via::Role {
                        name: r.role_name.clone(),
                    },
                    scope: Some(r.scope()),
                }
            } else {
                EgretClearance::NotCleared
            };
            fact(g, &gate_group_names, engines, egret)
        })
        .collect();

    Ok(Some(GroupPrincipal {
        name: name.to_string(),
        source: Some(source),
        is_public: false,
        service_accounts: service_accounts(&members),
        members,
        roles,
        workspaces,
        stands_for: sf.0,
        clearances,
    }))
}

/// One role NAME, across every scope defining it. `None` when no role has
/// `name`.
pub async fn read_role_via(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<Option<RolePrincipal>, CompileError> {
    let scopes: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT scope FROM roles WHERE name = $1 ORDER BY scope")
            .bind(name)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?;
    if scopes.is_empty() {
        return Ok(None);
    }
    let holder_ids: Vec<Uuid> = role_holders_via(conn, &[name.to_string()])
        .await?
        .into_iter()
        .map(|h| h.user_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let holders = member_refs_via(conn, &holder_ids).await?;
    let holder_groups: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT g.name, g.source \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
           JOIN groups g ON g.id = ag.principal_id \
          WHERE ag.principal_type = 'group' AND r.name = $1 AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
          ORDER BY g.name",
    )
    .bind(name)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let (gates, gate_group_names) = gates_via(conn).await?;
    let clearances = gates
        .iter()
        .map(|g| {
            let required = g.required_roles.iter().any(|r| r == name);
            let engines = if g.required_roles.is_empty() {
                EngineClearance::NotEnforcedGroupsOnly
            } else if required {
                EngineClearance::Cleared {
                    role: name.to_string(),
                }
            } else {
                EngineClearance::NotCleared { because: None }
            };
            let egret = if required {
                EgretClearance::Cleared {
                    via: Via::Itself,
                    scope: None,
                }
            } else {
                EgretClearance::NotCleared
            };
            fact(g, &gate_group_names, engines, egret)
        })
        .collect();
    Ok(Some(RolePrincipal {
        name: name.to_string(),
        scopes,
        service_accounts: service_accounts(&holders),
        holders,
        holder_groups: holder_groups
            .into_iter()
            .map(|(name, source)| GroupRef { name, source })
            .collect(),
        stands_for: vec![StandsFor {
            kind: PrincipalKind::Role,
            r#ref: name.to_string(),
            via: Via::Itself,
            on_engines: true,
            not_on_engines: None,
        }],
        clearances,
    }))
}

/// The members a group or role principal stands for, as the compiler reads
/// them ([`group_members_via`], [`role_holders_via`]), for
/// `POST /perm/explain/principal` (spec §4.3: "Members come from §4.1, read by
/// the server"). `None` when no group or role has `name`. `public` is not a
/// group with members and is the caller's to refuse.
pub(crate) async fn principal_members_via(
    conn: &mut sqlx::PgConnection,
    kind: PrincipalKind,
    name: &str,
) -> Result<Option<Vec<MemberRef>>, CompileError> {
    let ids: BTreeSet<Uuid> = match kind {
        PrincipalKind::Group => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM groups WHERE name = $1)")
                    .bind(name)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(db)?;
            if !exists {
                return Ok(None);
            }
            group_members_via(conn, &[name.to_string()])
                .await?
                .into_iter()
                .map(|m| m.user_id)
                .collect()
        }
        PrincipalKind::Role => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM roles WHERE name = $1)")
                    .bind(name)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(db)?;
            if !exists {
                return Ok(None);
            }
            role_holders_via(conn, &[name.to_string()])
                .await?
                .into_iter()
                .map(|h| h.user_id)
                .collect()
        }
        PrincipalKind::User | PrincipalKind::WorkspaceMembers => return Ok(None),
    };
    let ids: Vec<Uuid> = ids.into_iter().collect();
    member_refs_via(conn, &ids).await.map(Some)
}

// ---------------------------------------------------------------------------
// Who a clearance admits (classification UX Task 4, spec C9)
// ---------------------------------------------------------------------------

/// At most this many people are listed by [`cleared_people_via`];
/// [`ClearedPeople::count`] still counts every one.
pub const CLEARED_PEOPLE_LIMIT: usize = 200;

/// How a person comes to be listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClearedViaKind {
    /// Holds one of the gate's `required_roles`, directly or through a group.
    Role,
    /// Is a direct member of one of the gate's `required_groups`.
    Group,
}

/// The scope of a role grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClearedScope {
    /// Egret clears on every object.
    Platform,
    /// Egret clears only on the objects of workspace `scopeId`.
    Workspace,
    /// Egret clears only on object `scopeId` (and what a folder grant
    /// cascades to).
    Object,
}

impl ClearedScope {
    fn parse(scope_type: &str) -> Self {
        match scope_type {
            "platform" => Self::Platform,
            "workspace" => Self::Workspace,
            _ => Self::Object,
        }
    }
}

/// One required role or group a person is listed through.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearedVia {
    pub kind: ClearedViaKind,
    /// The role's or group's name.
    pub name: String,
    /// A role grant's scope; null for a group membership. Engines ignore it
    /// (a column read carries no workspace), egret clears only within it.
    pub scope: Option<ClearedScope>,
    /// The workspace or object id of a `workspace` or `object` scope; null
    /// otherwise.
    pub scope_id: Option<Uuid>,
    /// A role held by this group the person is a direct member of, not by
    /// the person: egret clears them, engines refuse them. Null for a role
    /// held directly and for a group membership.
    pub through_group: Option<String>,
}

/// One person listed for a clearance.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearedPerson {
    pub user_id: Uuid,
    /// Name, else email.
    pub label: String,
    /// The stored engine username; null when none is allocated yet.
    pub username: Option<String>,
    /// The account's status (`active` or `disabled`). A disabled account is
    /// listed: its grants still stand.
    pub status: String,
    /// A Data API service account (migration `0119`).
    pub service_account: bool,
    /// Every way they are listed: role grants (direct, then through a group)
    /// before group memberships, each sorted by name, scope, scope id.
    pub via: Vec<ClearedVia>,
    /// Whether the compiled engine deny excepts them: they hold a required
    /// role DIRECTLY (any scope), unexpired, and have a username. Never
    /// through a group or a group membership. A direct holder with no
    /// username is named once a compile mints one.
    pub on_engines: bool,
}

/// The people listed for a classification's clearance, and what each
/// population means. A person is listed once whatever the number of ways.
///
/// - **Role held directly** (`via.kind = role`, `throughGroup` null): a live,
///   `allow` grant of a required role to the user
///   ([`crate::members::role_holders_via`], the compiler's read). Cleared in
///   egret and, with a username, on engines (`onEngines`).
/// - **Role held through a group** (`throughGroup` set): a live `allow` grant
///   of a required role to a group the user is a direct member of. Egret's
///   object check clears them (`egret_auth::grants`, `user_groups`); engines
///   refuse them (the compiler expands only user holders).
/// - **Group member, gate names no role** (`enginesEnforce` false): egret
///   clears them; engines withhold the data from NOBODY, since a group-only
///   gate compiles to nothing on engines.
/// - **Group member, gate also names a role**: egret clears them; engines
///   refuse them (a Ranger group matches nobody; only role holders are
///   excepted).
/// - **Scoped grant** (`via.scope` workspace or object): egret clears them
///   only within that workspace or object; engines everywhere (a column read
///   carries no scope).
/// - **Platform admins** are always cleared by egret (a scope-correct
///   exemption) and are NOT listed unless they are also listed by a rule
///   above; engines give them no bypass.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClearedPeople {
    /// Every person listed by the rules above (not only the first 200). It
    /// is not everyone egret clears: platform admins are cleared and not
    /// listed (see the type doc).
    pub count: u32,
    /// The first [`CLEARED_PEOPLE_LIMIT`] (200) of them, sorted by label.
    pub people: Vec<ClearedPerson>,
    /// More people are listed than `people` carries.
    pub truncated: bool,
    /// Whether engines enforce the clearance at all: true when it names at
    /// least one required role. False means anyone with SQL access reads the
    /// data on engines.
    pub engines_enforce: bool,
    /// How many of `count` are `onEngines`, independent of the 200 cap.
    pub on_engines_count: u32,
}

/// A live `allow` grant of a role to a group, and one direct member of the
/// group: egret's own authoriser clears that member through it
/// (`egret_auth::grants::AccessGrantStore::grants_for`), engines do not.
#[derive(sqlx::FromRow)]
struct GroupRoleHolderRow {
    role_name: String,
    group_name: String,
    user_id: Uuid,
    scope_type: String,
    scope_id: Option<Uuid>,
}

/// The members of groups holding any role NAME in `roles`, by a live `allow`
/// grant. Egret-only: the compiler deliberately leaves this population out
/// (`members.rs` module doc, narrowing 1), so it is read here, not there.
async fn group_role_holders_via(
    conn: &mut sqlx::PgConnection,
    roles: &[String],
) -> Result<Vec<GroupRoleHolderRow>, CompileError> {
    if roles.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT r.name AS role_name, g.name AS group_name, ug.user_id, \
                ag.scope_type, ag.scope_id \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
           JOIN groups g ON g.id = ag.principal_id \
           JOIN user_groups ug ON ug.group_id = g.id \
          WHERE ag.principal_type = 'group' \
            AND r.name = ANY($1) \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(roles)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)
}

/// Who the clearance of tag `tag_id` lists ([`ClearedPeople`] says who and
/// what each population means). `None` when no tag has this id. A tag with
/// no clearance, or one whose lists are both empty (no gate), lists nobody:
/// it gates nothing.
///
/// Direct role holders and group members come from the compiler's own reads
/// ([`role_holders_via`], [`group_members_via`]), never a predicate written
/// here, so the people marked `on_engines` are exactly those a compile
/// excepts from the deny (`crate::clearance`, which reads
/// [`crate::members::role_usernames`] over the same [`role_holders_via`]).
pub async fn cleared_people_via(
    conn: &mut sqlx::PgConnection,
    tag_id: Uuid,
) -> Result<Option<ClearedPeople>, CompileError> {
    let gate: Option<(Vec<String>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT required_roles, required_groups FROM classification_policies WHERE tag_id = $1",
    )
    .bind(tag_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let Some((roles, group_ids)) = gate else {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tags WHERE id = $1)")
            .bind(tag_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        return Ok(exists.then(|| ClearedPeople {
            count: 0,
            people: Vec::new(),
            truncated: false,
            engines_enforce: false,
            on_engines_count: 0,
        }));
    };
    let roles: Vec<String> = roles
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // `crate::clearance`: a gate with no required role compiles to nothing
    // on engines.
    let engines_enforce = !roles.is_empty();
    // A gate stores group ids; membership is read by name, the key the
    // compiler expands. An id no group has any more admits nobody.
    let groups: Vec<String> = if group_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar("SELECT name FROM groups WHERE id = ANY($1)")
            .bind(&group_ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
    };

    #[derive(Default)]
    struct Listed {
        via: BTreeSet<ClearedVia>,
        on_engines: bool,
    }
    let scoped = |scope_type: &str, scope_id: Option<Uuid>| {
        let scope = ClearedScope::parse(scope_type);
        let scope_id = (scope != ClearedScope::Platform)
            .then_some(scope_id)
            .flatten();
        (Some(scope), scope_id)
    };
    let mut listed: BTreeMap<Uuid, Listed> = BTreeMap::new();
    for h in role_holders_via(conn, &roles).await? {
        let (scope, scope_id) = scoped(&h.scope_type, h.scope_id);
        let l = listed.entry(h.user_id).or_default();
        l.via.insert(ClearedVia {
            kind: ClearedViaKind::Role,
            name: h.role_name,
            scope,
            scope_id,
            through_group: None,
        });
        l.on_engines |= h.username.is_some();
    }
    for h in group_role_holders_via(conn, &roles).await? {
        let (scope, scope_id) = scoped(&h.scope_type, h.scope_id);
        listed.entry(h.user_id).or_default().via.insert(ClearedVia {
            kind: ClearedViaKind::Role,
            name: h.role_name,
            scope,
            scope_id,
            through_group: Some(h.group_name),
        });
    }
    for m in group_members_via(conn, &groups).await? {
        listed.entry(m.user_id).or_default().via.insert(ClearedVia {
            kind: ClearedViaKind::Group,
            name: m.group_name,
            scope: None,
            scope_id: None,
            through_group: None,
        });
    }

    let count = u32::try_from(listed.len()).unwrap_or(u32::MAX);
    let on_engines_count =
        u32::try_from(listed.values().filter(|l| l.on_engines).count()).unwrap_or(u32::MAX);
    let truncated = listed.len() > CLEARED_PEOPLE_LIMIT;
    let ids: Vec<Uuid> = listed.keys().copied().collect();
    let mut rows = user_rows_via(conn, &ids).await?;
    rows.sort_by_cached_key(|u| (label(&u.name, &u.email).to_lowercase(), u.id));
    rows.truncate(CLEARED_PEOPLE_LIMIT);
    let people = rows
        .into_iter()
        .map(|u| {
            let l = listed.remove(&u.id).unwrap_or_default();
            ClearedPerson {
                user_id: u.id,
                label: label(&u.name, &u.email),
                username: u.username,
                status: u.status,
                service_account: u.service_account,
                via: l.via.into_iter().collect(),
                on_engines: l.on_engines,
            }
        })
        .collect();
    Ok(Some(ClearedPeople {
        count,
        people,
        truncated,
        engines_enforce,
        on_engines_count,
    }))
}
