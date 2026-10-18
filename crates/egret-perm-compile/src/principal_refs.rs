//! **Every principal ref a grant or rule names must name someone egret knows**
//! (finding J-F3, `docs/en/specs/2026-10-17-perm-principal-ref-validation-design.md`).
//!
//! # Why an unknown ref is a security defect and not a typo
//!
//! The compiler writes a ref through verbatim (`crate::grants`'s module doc), so a
//! ref that matches nobody compiles to a Ranger item naming nobody. Where that
//! lands decides which way it fails:
//!
//! - an **allow include** fails closed: the grant grants nobody;
//! - a **deny include**, an **allow grant's exclude**, or a **rule exemption's
//!   exclude** fails **OPEN**: the deny or the subtract silently evaporates and
//!   the compiled policy allows more than the model states;
//! - a **deny grant's exclude** fails closed: it compiles to a
//!   `denyExceptions` item (`crate::grants`, `compile_grant`), and an exception
//!   naming nobody makes the deny WIDER than written, not narrower;
//! - a rule's **`exempt.include`** fails closed the other way: the mask applies to
//!   someone the author meant to exempt.
//!
//! Every one of these is refused at write time all the same: a carve-out or a
//! target that matches nobody is never what the author meant.
//!
//! None of these is reported by anything: the policy compiles, deploys and is
//! listed live. The case that surfaced it was `{kind: "user", ref: "<user
//! UUID>"}`, which `POST /perm/grants` accepted and compiled as the Ranger user
//! name. `egret_perm::CompileError::UnusableUserRef` already said where the fix
//! belongs — "canonicalise at the authoring boundary, where `users.username` is
//! readable, not in the compiler" — and could catch only an empty ref or an `@`,
//! because the compiler does no I/O.
//!
//! # The source of truth, per kind
//!
//! Each kind is checked against the table its DOWNSTREAM resolution reads, so a
//! ref this module accepts is one that resolution can actually find:
//!
//! - **`user`** — `users.username` (`migrations/0040_user_username.sql:1`,
//!   `citext UNIQUE`). The bare username is the only identity an engine ever sees
//!   for a person (SP4a D7; `crate::members`' module doc, "the name emitted is the
//!   STORED `users.username`"), and `egret_perm::eval::matches_principal`
//!   (`crates/egret-perm/src/eval.rs:311`) compares it byte-for-byte. citext makes
//!   the lookup case-insensitive, so the ref is REWRITTEN to the stored spelling:
//!   `Analyst` and `analyst` are one person to egret but two strings to Ranger.
//!   A ref that is a user's id or email is refused with the username to use
//!   instead — never silently rewritten, because a write that stores something
//!   other than what the author sent should be one the author sees.
//! - **`group`** — `groups.name` (`migrations/0002_identity.sql:15-19`), plus
//!   [`egret_perm::PUBLIC_GROUP`], which every Ranger plugin resolves for every
//!   user and has no row. egret serves Ranger NO user/group document: its Ranger
//!   router has exactly two download routes, policies and tags
//!   (`crates/egret-policy/src/ranger/routes.rs:15-31`), and egret runs no
//!   usersync, so a group other than `public` reaches no engine at all (D7,
//!   `crate::members`' module doc). There is therefore no "group egret serves to
//!   Ranger" to check against; `groups` is the one group store egret owns — the
//!   one `user_groups` and the OIDC group sync write, and the one the authoring
//!   picker searches (`crates/egret-policy/src/admin/mod.rs:1109-1123`,
//!   `list_principals`). It is also exactly what the compiler resolves a group
//!   through since J-F3 Task 1b: a non-`public` group is expanded into its
//!   members' usernames (`crate::members`, `user_groups` joined to
//!   `groups.name`), so a name with no row expands to nobody — on an exclude,
//!   the same fail open.
//! - **`role`** — `roles.name`, in any scope (`migrations/0003_workspace_rbac.sql:21-27`).
//!   A role is NOT free text: it is expanded at compile time into its holders'
//!   usernames by `crate::members::role_usernames`, which joins `roles` by name
//!   with no scope filter (`crates/egret-perm-compile/src/members.rs:155-162`).
//!   A name with no `roles` row expands to nobody — on an exclude, the same fail
//!   open. The picker's free-text role input (`PrincipalPicker.tsx`) is a UI
//!   shortcut, not a statement that roles are external.
//! - **`workspace_members`** — `workspaces.id`. Expanded at compile time by
//!   `crate::members::expand_enumerated_principals`, which expands a ref that is
//!   not a uuid, or names no workspace, to nobody. (No authoring path may write
//!   one today — `validate_grant_shape` refuses the kind — but a ref that names
//!   nothing is still named as such, so the two refusals stay distinguishable.)
//!   An accepted ref is rewritten to the canonical `Uuid` spelling, so it equals
//!   what a People page's `standsFor` names byte for byte. A non-canonical ref
//!   already STORED is not rewritten (R5) and survives a patch unchanged (R6):
//!   it still applies on engines, and a client matching `standsFor` must compare
//!   a `workspace_members` ref as a parsed UUID to find it.
//!
//! # What this does NOT do
//!
//! It does not rewrite stored rows (design R5), and it does not refuse a patch
//! because of a principal the patch does not touch (R6). PATCH takes the whole
//! body, so every edit re-sends every principal; a ref the STORED row already
//! holds in the same position (same kind, ref and include/exclude) is therefore
//! not re-checked. A USER or ROLE deleted after it was validly written is not
//! a defect — nobody carries the name any more, so a deny or exclude on it
//! still denies everyone who does — and refusing every later edit of the row
//! (even an actions-only one) until the author removed it would be.
//!
//! **A GROUP is different (2026-10-18 design R1, R2).** A group name can be
//! carried again: a new local group, a rename into it, or an IdP claim would
//! inherit every grant and exemption left naming it. So a local group's
//! delete or rename is refused while anything names it (`GroupRepo::rename`,
//! `GroupRepo::delete_local`), checked under the per-group lock that every
//! grant and rule write takes BEFORE it resolves refs (R3), so the check
//! cannot go stale. A leftover reference to a group with no row can still
//! exist — stored before J-F3, or written directly — and the OIDC login
//! refuses to create a group of that name (`GroupRepo::upsert_claimed_oidc_groups`).
//! A new local group of that name is allowed, with a warning (R4): its members
//! are only ever added by a platform admin. The accepted
//! trade-off: a bad ref stored BEFORE this check existed also survives a patch
//! unchanged; Task 2's report ([`unresolved_principals_via`], `GET
//! /perm/unresolved-principals`, and a startup `warn!` per fail-open row)
//! lists such rows. A user ref that is empty or a reserved engine principal is
//! left to `validate_user_ref`, which already refuses both with a sentence of its
//! own.

use std::fmt::Write as _;

use sqlx::PgConnection;
use uuid::Uuid;

use egret_perm::{Principal, PrincipalKind, PrincipalSet, PUBLIC_GROUP};

use crate::routes::PermHttpError;

/// One ref that matched nobody: what the author wrote, where, and why.
struct Unknown {
    kind: PrincipalKind,
    r#ref: String,
    position: String,
    why: String,
}

/// Resolve every principal in `set` (include AND exclude) against the table its
/// kind is resolved from downstream (module doc), canonicalising a username's
/// spelling in place, and refuse the write if any ref names nobody.
///
/// `field` is the set's name on the wire (`principals` for a grant, `exempt` for
/// a rule), so a refusal can say exactly which entry to fix. EVERY unknown ref is
/// named in one 400, not just the first: an author fixing a long list should not
/// discover the problems one round trip at a time.
///
/// `stored` is the row as it stands on a patch (`None` on a create): an entry the
/// stored set already holds in the same list, byte-for-byte, is accepted
/// without a lookup (module doc, "What this does NOT do").
///
/// Canonicalising can make two entries equal (`Analyst` and `analyst`), and the
/// stored table's primary key is `(id, kind, ref, mode)`, so each list is
/// deduplicated afterwards, first occurrence kept.
pub(crate) async fn resolve_principal_refs(
    conn: &mut PgConnection,
    field: &str,
    set: &mut PrincipalSet,
    stored: Option<&PrincipalSet>,
) -> Result<(), PermHttpError> {
    let mut unknown = Vec::new();
    let empty: &[Principal] = &[];
    for (mode, list, kept) in [
        (
            "include",
            &mut set.include,
            stored.map_or(empty, |s| s.include.as_slice()),
        ),
        (
            "exclude",
            &mut set.exclude,
            stored.map_or(empty, |s| s.exclude.as_slice()),
        ),
    ] {
        for (i, p) in list.iter_mut().enumerate() {
            if kept.contains(p) {
                continue;
            }
            let found = match lookup(conn, p).await? {
                Lookup::Found { canonical } => {
                    p.r#ref = canonical;
                    None
                }
                Lookup::Unknown(why) => Some(why),
            };
            if let Some(why) = found {
                unknown.push(Unknown {
                    kind: p.kind,
                    r#ref: p.r#ref.clone(),
                    position: format!("{field}.{mode}[{i}]"),
                    why,
                });
            }
        }
        dedup_keeping_first(list);
    }
    if unknown.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "these principal refs match nobody egret knows — a policy naming nobody grants \
         nobody as an allow and denies everyone as a deny's exception, and as a deny's \
         target or an allow's exclude it silently allows more than the model states:",
    );
    for u in &unknown {
        let _ = write!(
            msg,
            " {} {:?} ({}): {};",
            u.kind.as_str(),
            u.r#ref,
            u.position,
            u.why
        );
    }
    msg.pop();
    Err(PermHttpError::BadRequest(msg))
}

/// What one ref resolves to, read without writing anything — the single
/// per-kind rule both the write path ([`resolve_principal_refs`]) and the
/// stored-row report ([`unresolved_principals_via`]) apply.
enum Lookup {
    /// It names someone egret knows. `canonical` is the spelling to store: the
    /// ref itself for `group` and `role`; a `user`'s stored username, which can
    /// differ from the ref in case; a `workspace_members` id in its canonical
    /// (lowercase, hyphenated) form.
    Found { canonical: String },
    /// It names nobody; the reason, as a sentence an author can act on.
    Unknown(String),
}

fn found(p: &Principal) -> Lookup {
    Lookup::Found {
        canonical: p.r#ref.clone(),
    }
}

fn unknown_if(missing: bool, p: &Principal, why: impl FnOnce() -> String) -> Lookup {
    if missing {
        Lookup::Unknown(why())
    } else {
        found(p)
    }
}

async fn lookup(conn: &mut PgConnection, p: &Principal) -> Result<Lookup, PermHttpError> {
    match p.kind {
        PrincipalKind::User => lookup_user(conn, p).await,
        PrincipalKind::Group => {
            if p.r#ref == PUBLIC_GROUP {
                return Ok(found(p));
            }
            let found: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM groups WHERE name = $1)")
                    .bind(&p.r#ref)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            Ok(unknown_if(!found, p, || {
                "no group has this name".to_string()
            }))
        }
        PrincipalKind::Role => {
            let found: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM roles WHERE name = $1)")
                    .bind(&p.r#ref)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            Ok(unknown_if(!found, p, || {
                "no role has this name — a role is expanded into its holders at compile \
                 time, and a name with no role expands to nobody"
                    .to_string()
            }))
        }
        PrincipalKind::WorkspaceMembers => {
            let Ok(id) = p.r#ref.parse::<Uuid>() else {
                return Ok(Lookup::Unknown(
                    "a workspace ref is a workspace id".to_string(),
                ));
            };
            let found: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = $1)")
                    .bind(id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(PermHttpError::db)?;
            // Canonical is `id.to_string()` — lowercase, hyphenated. `parse`
            // accepts uppercase, unhyphenated, braced and `urn:uuid:` forms, and
            // the compiler expands each of them, but a principal's People page
            // (`crate::people`'s `standsFor`) names a workspace in the canonical
            // form and a client matches refs byte for byte (spec P4). Stored
            // verbatim, such a grant would apply on engines and be missing from
            // the page.
            if found {
                Ok(Lookup::Found {
                    canonical: id.to_string(),
                })
            } else {
                Ok(Lookup::Unknown("no workspace has this id".to_string()))
            }
        }
    }
}

/// A `user` ref must equal a stored `users.username` (case-insensitively; the
/// stored spelling is returned as the canonical one). Tried as a username FIRST:
/// a username is a `[a-z0-9_-]` slug, so a uuid-shaped string can be a real one.
async fn lookup_user(conn: &mut PgConnection, p: &Principal) -> Result<Lookup, PermHttpError> {
    // Left to `validate_user_ref`, which refuses both with its own sentence.
    if p.r#ref.is_empty() || egret_core::engine_principals::is_reserved_engine_username(&p.r#ref) {
        return Ok(found(p));
    }
    let canonical: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE username = $1::citext")
            .bind(&p.r#ref)
            .fetch_optional(&mut *conn)
            .await
            .map_err(PermHttpError::db)?;
    if let Some(canonical) = canonical {
        return Ok(Lookup::Found { canonical });
    }

    // Not a username. If it is a user's id or email, say whose username to use —
    // that is the mistake the finding was, and the author can act on the answer.
    let (what, row): (&str, Option<Option<String>>) = if let Ok(id) = p.r#ref.parse::<Uuid>() {
        let row = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(PermHttpError::db)?;
        ("id", row)
    } else if p.r#ref.contains('@') {
        let row = sqlx::query_scalar("SELECT username::text FROM users WHERE email = $1::citext")
            .bind(&p.r#ref)
            .fetch_optional(&mut *conn)
            .await
            .map_err(PermHttpError::db)?;
        ("email", row)
    } else {
        ("", None)
    };
    Ok(Lookup::Unknown(match row {
        Some(Some(username)) => format!(
            "this is a user's {what}, and an engine knows a person only by their username — \
             use {username:?}"
        ),
        Some(None) => format!(
            "this is a user's {what}, and that user has no username yet (one is minted the \
             first time they use an engine), so no engine can name them"
        ),
        None => "no user has this username — a user ref is the bare username, never an \
                 email or an id"
            .to_string(),
    }))
}

// ---------------------------------------------------------------------------
// The report of STORED refs that match nobody (J-F3 Task 2, design R5/R6)
// ---------------------------------------------------------------------------

/// One stored grant principal or rule exemption whose ref matches nobody.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedPrincipal {
    /// `"grant"` or `"rule"`.
    pub source: String,
    /// The grant's or the rule's id.
    pub id: Uuid,
    /// The rule's name; `null` for a grant (a grant has none).
    pub name: Option<String>,
    /// `user`, `group`, `role` or `workspace_members`.
    pub kind: String,
    /// The ref as stored, byte-for-byte — the string the compiled policy names.
    pub r#ref: String,
    /// `"include"` or `"exclude"` (of the grant's `principals`, or of the rule's
    /// `exempt`).
    pub position: String,
    /// The grant's `allow` / `deny`; `null` for a rule exemption.
    pub effect: Option<String>,
    /// `"unresolved"`: the ref names nobody egret knows. `"empty"`: it names a
    /// group that exists and has no members, so it matches nobody TODAY.
    pub status: String,
    /// Why it matches nobody, as a sentence an author can act on.
    pub reason: String,
    /// Matching nobody here makes the compiled policy allow MORE than the model
    /// states: an unresolved ref in a deny grant's include, an allow grant's
    /// exclude, or a rule exemption's exclude. An allow include, a deny grant's
    /// exclude (a `denyExceptions` item: naming nobody makes the deny wider)
    /// and an exemption include matching nobody fail closed, and an `empty`
    /// group never fails open: nobody is in it, so matching nobody is exactly
    /// its meaning.
    pub fails_open: bool,
}

/// One stored grant principal or rule exemption, as the report reads it.
#[derive(sqlx::FromRow)]
struct ReportRow {
    source: String,
    id: Uuid,
    name: Option<String>,
    kind: String,
    r#ref: String,
    position: String,
    effect: Option<String>,
}

/// What one stored `(kind, ref)` resolves to, for the report.
#[derive(Clone)]
enum Stored {
    Resolves,
    Empty,
    Unresolved(String),
}

/// **Every stored grant principal and rule exemption whose ref matches
/// nobody**, ordered by source, id, position, kind and ref. Reads only; the
/// caller should hold a read-only transaction.
///
/// Each ref goes through [`lookup`], the same per-kind rule the write path
/// refuses with, so "the report lists it" and "a write naming it is refused"
/// cannot drift apart. Two things only the report adds:
///
/// - **A `user` ref the lookup finds only by case** (`Analyst` for `analyst`)
///   is unresolved here, where the write path would have rewritten it: a
///   stored row is compiled byte-for-byte (`crate::grants`' module doc) and
///   compared byte-for-byte downstream, so on an engine it names nobody.
/// - **A group that exists with no members** is `empty`: since J-F3 Task 1b
///   a non-`public` group compiles to its members' usernames through
///   `user_groups` (`crate::members::expand_enumerated_principals`, the same
///   join read here), so it matches nobody, but correctly — never fails open.
///
/// A role with no holders or a workspace with no members also expands to
/// nobody; the design names only the group case, so those resolve here.
pub async fn unresolved_principals_via(
    conn: &mut PgConnection,
) -> Result<Vec<UnresolvedPrincipal>, PermHttpError> {
    let rows: Vec<ReportRow> = sqlx::query_as(
        "SELECT 'grant' AS source, g.id, NULL::text AS name, gp.kind::text AS kind, gp.ref, \
                gp.mode::text AS position, g.effect::text AS effect \
           FROM perm.grant_principals gp JOIN perm.grants g ON g.id = gp.grant_id \
         UNION ALL \
         SELECT 'rule', r.id, r.name, re.kind::text, re.ref, re.mode::text, NULL \
           FROM perm.rule_exemptions re JOIN perm.rules r ON r.id = re.rule_id \
         ORDER BY source, id, position, kind, ref",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;

    // One lookup per distinct (kind, ref): one deleted user excluded from fifty
    // grants is one read, not fifty.
    let mut seen: std::collections::HashMap<(String, String), Stored> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for row in rows {
        let Some(kind) = PrincipalKind::parse(&row.kind) else {
            // The column is the `perm.principal_kind` enum; a value this build
            // cannot name is a schema/code mismatch, not a ref to report.
            return Err(PermHttpError::db(format!(
                "unknown perm.principal_kind {:?}",
                row.kind
            )));
        };
        let key = (row.kind.clone(), row.r#ref.clone());
        let stored = match seen.get(&key) {
            Some(s) => s.clone(),
            None => {
                let p = Principal {
                    kind,
                    r#ref: row.r#ref.clone(),
                };
                let s = classify_stored(conn, &p).await?;
                seen.insert(key, s.clone());
                s
            }
        };
        let (status, reason) = match stored {
            Stored::Resolves => continue,
            Stored::Empty => ("empty", "this group exists and has no members".to_string()),
            Stored::Unresolved(why) => ("unresolved", why),
        };
        let fails_open = status == "unresolved" && widens_access_when_empty(&row);
        out.push(UnresolvedPrincipal {
            source: row.source,
            id: row.id,
            name: row.name,
            kind: row.kind,
            r#ref: row.r#ref,
            position: row.position,
            effect: row.effect,
            status: status.to_string(),
            reason,
            fails_open,
        });
    }
    Ok(out)
}

/// Where a ref matching nobody makes the compiled policy allow MORE than the
/// model states (the `failsOpen` doc). A deny grant's exclude is NOT one: it
/// compiles to `denyExceptions`, and an exception naming nobody denies more.
/// A rule's `exempt.exclude` cannot be written (`validate_rule_shape` refuses
/// it) and is flagged conservatively should one be stored.
fn widens_access_when_empty(row: &ReportRow) -> bool {
    matches!(
        (
            row.source.as_str(),
            row.effect.as_deref(),
            row.position.as_str(),
        ),
        ("grant", Some("deny"), "include")
            | ("grant", Some("allow"), "exclude")
            | ("rule", _, "exclude")
    )
}

async fn classify_stored(conn: &mut PgConnection, p: &Principal) -> Result<Stored, PermHttpError> {
    match lookup(conn, p).await? {
        Lookup::Unknown(why) => Ok(Stored::Unresolved(why)),
        // Users only: a username is named byte for byte, so a case variant
        // names nobody. A non-canonical `workspace_members` spelling is not
        // that — the compiler parses it into the same id
        // (`crate::members::referenced_principals`) — so it resolves.
        Lookup::Found { canonical } if p.kind == PrincipalKind::User && canonical != p.r#ref => {
            Ok(Stored::Unresolved(format!(
            "this matches the username {canonical:?} only case-insensitively, and the compiled \
             policy names the ref byte-for-byte — an engine sees {canonical:?}, never this \
             spelling"
            )))
        }
        Lookup::Found { .. } if p.kind == PrincipalKind::Group && p.r#ref != PUBLIC_GROUP => {
            // The join `crate::members::expand_enumerated_principals` expands
            // a group through.
            let has_members: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM user_groups ug \
                                  JOIN groups g ON g.id = ug.group_id \
                                  JOIN users u ON u.id = ug.user_id \
                                 WHERE g.name = $1)",
            )
            .bind(&p.r#ref)
            .fetch_one(&mut *conn)
            .await
            .map_err(PermHttpError::db)?;
            Ok(if has_members {
                Stored::Resolves
            } else {
                Stored::Empty
            })
        }
        Lookup::Found { .. } => Ok(Stored::Resolves),
    }
}

/// **The startup pass** (design R5): one `warn!` per stored ref that fails
/// OPEN, read in one read-only snapshot. Returns the rows it logged. Logs ids,
/// kind, ref and position only — nothing a principal set holds is secret.
///
/// Best-effort by design: the caller logs an error and boots anyway, because
/// the rows are already being served and refusing to start would not unserve
/// them. `GET /api/control-plane/perm/unresolved-principals` is where an
/// operator acts on them.
pub async fn warn_fail_open_principals(
    pool: &sqlx::PgPool,
) -> Result<Vec<UnresolvedPrincipal>, PermHttpError> {
    let mut tx = pool.begin().await.map_err(PermHttpError::db)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(PermHttpError::db)?;
    let report = unresolved_principals_via(&mut tx).await?;
    tx.rollback().await.map_err(PermHttpError::db)?;
    let open: Vec<UnresolvedPrincipal> = report.into_iter().filter(|r| r.fails_open).collect();
    for r in &open {
        tracing::warn!(
            source = %r.source,
            id = %r.id,
            kind = %r.kind,
            principal_ref = %r.r#ref,
            position = %r.position,
            effect = r.effect.as_deref().unwrap_or("-"),
            "a stored principal ref matches nobody where that FAILS OPEN: the compiled policy \
             allows more than the model states. See GET /api/control-plane/perm/\
             unresolved-principals"
        );
    }
    Ok(open)
}

fn dedup_keeping_first(list: &mut Vec<Principal>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|p| seen.insert((p.kind.as_str(), p.r#ref.clone())));
}

// ---------------------------------------------------------------------------
// Stored `user` refs naming a username (People & groups Task 5, spec §4.4)
// ---------------------------------------------------------------------------

/// One stored grant principal or rule exemption that names a username as a
/// `user` ref.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct StoredUserRef {
    /// `"grant"` or `"rule"`.
    pub source: String,
    /// The grant's or the rule's id.
    pub id: Uuid,
    /// `"include"` or `"exclude"` (of the grant's `principals`, or of the
    /// rule's `exempt`).
    pub position: String,
    /// The ref as stored, byte for byte.
    pub r#ref: String,
}

/// **Every stored grant principal and rule exemption whose `user` ref names
/// `username`, compared case-insensitively**, ordered by source, id, position
/// and ref.
///
/// Case-insensitively because that is how egret resolves a user ref:
/// `users.username` is `citext UNIQUE` and [`lookup_user`] matches a ref
/// through it, so to egret a stored `Bob` names whoever holds `bob` — the next
/// edit of the row that re-sends it in another position would be canonicalised
/// to that person. That the compiled policy names the ref byte for byte (and so
/// `Bob` reaches no engine today) makes it a stale ref, which is exactly what
/// the caller is looking for.
pub(crate) async fn stored_user_refs_naming_via(
    conn: &mut PgConnection,
    username: &str,
) -> Result<Vec<StoredUserRef>, PermHttpError> {
    let rows: Vec<(String, Uuid, String, String)> = sqlx::query_as(
        "SELECT 'grant' AS source, grant_id AS id, mode::text AS position, ref \
           FROM perm.grant_principals \
          WHERE kind = 'user' AND ref::citext = $1::citext \
         UNION ALL \
         SELECT 'rule', rule_id, mode::text, ref \
           FROM perm.rule_exemptions \
          WHERE kind = 'user' AND ref::citext = $1::citext \
         ORDER BY source, id, position, ref",
    )
    .bind(username)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    Ok(rows
        .into_iter()
        .map(|(source, id, position, r#ref)| StoredUserRef {
            source,
            id,
            position,
            r#ref,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// People & groups Task 3 review: a workspace ref in any form `Uuid`
    /// parses is stored canonical, so it matches `standsFor` byte for byte.
    #[tokio::test]
    async fn a_workspace_ref_is_stored_in_its_canonical_form() {
        let Some(dsn) = egret_test_db::url().await else {
            return;
        };
        let db = egret_metastore::Db::connect(&dsn, 2)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        let slug = format!("canon-{}", &Uuid::new_v4().simple().to_string()[..12]);
        let ws: Uuid =
            sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
                .bind(&slug)
                .fetch_one(&db.pool)
                .await
                .expect("workspace");
        let mut conn = db.pool.acquire().await.expect("conn");
        let forms = [
            ws.to_string().to_uppercase(),
            ws.simple().to_string(),
            format!("{{{ws}}}"),
            ws.urn().to_string(),
            ws.to_string(),
        ];
        for form in forms {
            let mut set = PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::WorkspaceMembers,
                    r#ref: form.clone(),
                }],
                exclude: Vec::new(),
            };
            resolve_principal_refs(&mut conn, "principals", &mut set, None)
                .await
                .unwrap_or_else(|e| panic!("{form}: {e}"));
            assert_eq!(set.include[0].r#ref, ws.to_string(), "{form}");
        }
        // Two spellings of one workspace collapse to one entry.
        let mut set = PrincipalSet {
            include: vec![
                Principal {
                    kind: PrincipalKind::WorkspaceMembers,
                    r#ref: ws.simple().to_string(),
                },
                Principal {
                    kind: PrincipalKind::WorkspaceMembers,
                    r#ref: ws.to_string(),
                },
            ],
            exclude: Vec::new(),
        };
        resolve_principal_refs(&mut conn, "principals", &mut set, None)
            .await
            .expect("resolve");
        assert_eq!(set.include.len(), 1);

        // A non-canonical spelling already STORED still names the workspace
        // the compiler expands: it resolves, it is not "unmatched".
        for form in [
            ws.to_string().to_uppercase(),
            format!("{{{ws}}}"),
            ws.urn().to_string(),
        ] {
            let stored = classify_stored(
                &mut conn,
                &Principal {
                    kind: PrincipalKind::WorkspaceMembers,
                    r#ref: form.clone(),
                },
            )
            .await
            .expect("classify");
            assert!(matches!(stored, Stored::Resolves), "{form}");
        }
        // A user ref differing from the stored username in case still does not.
        let username = format!("CanonUser{}", &slug[6..]);
        sqlx::query(
            "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, $1, $2)",
        )
        .bind(format!("{}@egret.local", username.to_lowercase()))
        .bind(username.to_lowercase())
        .execute(&db.pool)
        .await
        .expect("user");
        let stored = classify_stored(
            &mut conn,
            &Principal {
                kind: PrincipalKind::User,
                r#ref: username.clone(),
            },
        )
        .await
        .expect("classify");
        assert!(matches!(stored, Stored::Unresolved(_)), "{username}");
    }

    fn user(r: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            r#ref: r.to_string(),
        }
    }

    #[test]
    fn dedup_keeps_the_first_of_equal_entries_and_every_distinct_one() {
        let mut list = vec![
            user("analyst"),
            Principal {
                kind: PrincipalKind::Group,
                r#ref: "analyst".to_string(),
            },
            user("analyst"),
            user("bob"),
        ];
        dedup_keeping_first(&mut list);
        assert_eq!(
            list.iter()
                .map(|p| (p.kind.as_str(), p.r#ref.as_str()))
                .collect::<Vec<_>>(),
            vec![("user", "analyst"), ("group", "analyst"), ("user", "bob")]
        );
    }
}
