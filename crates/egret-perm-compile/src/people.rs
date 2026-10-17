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
//! - a role holder with no username is read but never named:
//!   `members::role_usernames` logs and skips them, and nothing mints one;
//! - a group member with no username is named only once a grant naming the
//!   group is compiled, which mints the username (`members.rs`, "A member with
//!   no username yet is MINTED"). Until then, no policy names them.
//!
//! What this module adds is only the list itself: which users, groups and
//! roles exist, filtered and paged.
//!
//! **What a row reveals.** Identity only: a user's id, display name, email and
//! engine username; a group's name and source; a role's name and scopes; and a
//! count. No password hash, session, permission, status or login time is read,
//! so none can leak into a response (Q2: the routes are gated `policy:read`).
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

use egret_auth::users::usernames_via;
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
            for h in role_holders_via(conn, &names).await? {
                holders.entry(h.role_name).or_default().insert(h.user_id);
            }
            // The username read `members::role_usernames` names holders with:
            // a holder absent from it is not named on any engine.
            let ids: Vec<Uuid> = holders
                .values()
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let named = usernames_via(conn, &ids)
                .await
                .map_err(|e| CompileError::Db(e.to_string()))?;
            let roles = rows
                .into_iter()
                .map(|(name, scopes)| ListedRole {
                    holder_count: count(holders.get(&name)),
                    without_username: holders.get(&name).map_or(0, |s| {
                        i64::try_from(s.iter().filter(|id| !named.contains_key(id)).count())
                            .unwrap_or(i64::MAX)
                    }),
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
