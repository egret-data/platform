//! Expanding a `workspace_members` principal into the member list enumerated
//! at compile time — SP4a **D3**, on the branch D7's live measurement forced.
//!
//! **Why enumeration and not a Ranger group.** D7
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) measured on
//! the kind cluster, by mechanism rather than by a failed query, that groups
//! never reach the engine: Trino's coordinator carries no
//! `group-provider.properties`, `egret-provisioner` never writes one, and
//! egret runs no usersync, so `getGroups(user)` is empty for every user on
//! every engine egret provisions. A Ranger policy item naming a GROUP matches
//! nobody. `workspace_members` therefore compiles to the members themselves,
//! as bare usernames, and a membership write must recompile — D7 §3's own
//! words, and what [`crate::affected_services_for_workspace_members`] plus
//! `egret_workspace::members::MemberRepo` deliver.
//!
//! **The name emitted is the STORED `users.username`, never a re-derivation.**
//! `ensure_username` (`crates/egret-auth/src/users.rs`, migration `0040`)
//! lowercases the email local-part, slugifies it to `[a-z0-9_-]`, and resolves
//! collisions with a `-2`, `-3`, … suffix. Two humans whose local parts slug
//! to the same base therefore have usernames that differ ONLY by a suffix no
//! amount of local re-derivation can predict — re-deriving would name `bob`
//! for the person the engine knows as `bob-2`, and the failure is invisible:
//! the policy compiles, deploys, is listed live by egret, and grants that
//! person nothing. So this module reads the column.
//!
//! **A member with no username yet is MINTED, not skipped.** `users.username`
//! is NULLable and nothing on any user-insert path sets it; it is minted
//! lazily the first time someone runs a query, opens a notebook, or is issued
//! an engine token. Reading alone would omit every member who has not yet
//! touched an engine — on a fresh install, that is every member including the
//! bootstrap admin, so D3 would compile to a policy naming nobody. This
//! module calls `egret_auth::users::ensure_username_via` on the compile's own
//! connection, which is idempotent and stable (the value it allocates is the
//! one the engine token minter will hand out later).
//!
//! # What "member" means here, and the two places it is narrower than egret's
//!
//! A member is a principal holding a live, `allow`, workspace-scoped
//! `access_grants` row for the workspace — the same rows
//! `egret_workspace::members::MemberRepo::list` shows — restricted to
//! `principal_type = 'user'`. Two deliberate narrowings, both recorded in the
//! design spec's D3 because they are visible behaviour:
//!
//! 1. **A GROUP added as a workspace member is not expanded.** egret's PDP
//!    does expand it (`egret_auth::grants::AccessGrantStore::grants_for` joins
//!    `user_groups`), so this compiles LESS access than egret's own evaluator
//!    would allow for such a subject. That is the fail-closed direction, and
//!    it is chosen because the opposite cannot be kept true: the only writers
//!    of `user_groups` live in `egret-auth::groups`, BELOW this crate in the
//!    dependency graph (this crate depends on `egret-auth`), so they cannot
//!    call the recompile — and one of them is the per-login OIDC group sync,
//!    which would turn every login into an installation-wide recompile. A
//!    group expansion that cannot be recompiled goes stale on REMOVAL, which
//!    fails open.
//! 2. **An `expires_at` that lapses is not a write, so nothing recompiles.**
//!    A time-boxed membership (the access-request approval path grants one)
//!    stays in the compiled policy until the next recompile of that profile
//!    for any other reason. Also fail-open, also recorded rather than papered
//!    over; closing it needs a sweeper, which SP4a does not have.
//!
//! **[`workspaces_for_username`] (SP4a Task 15, D3 defect 2).** `explain`
//! (`crate::routes`) needs the SAME fact this module resolves, in the OTHER
//! direction: not "which usernames does this workspace's membership expand
//! to" but "which workspaces is this username currently a member of." Both
//! read the identical `access_grants` predicate off the identical
//! `users.username` column — different query, same source of truth — so the
//! evaluator (`egret_perm::eval`) and the compiler cannot silently disagree
//! about who a `workspace_members` principal actually names.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use uuid::Uuid;

use egret_auth::users::ensure_username_via;
use egret_core::UserId;
use egret_perm::{CompileError, Grant, Principal, PrincipalKind, PrincipalSet, Rule};

/// One workspace-membership row as read for expansion: who, and the username
/// the engine will see (NULL until it is minted).
#[derive(sqlx::FromRow)]
struct MemberRow {
    workspace_id: Uuid,
    user_id: Uuid,
    username: Option<String>,
}

/// Collect every workspace id a `workspace_members` principal in `grants` or
/// `rules` names. A ref that does not parse as a uuid is NOT collected — see
/// [`expand_workspace_members`] for why that is the same answer the evaluator
/// gives.
fn referenced_workspaces(grants: &[Grant], rules: &[Rule]) -> BTreeSet<Uuid> {
    let mut out = BTreeSet::new();
    let mut scan = |set: &PrincipalSet| {
        for p in set.include.iter().chain(set.exclude.iter()) {
            if p.kind == PrincipalKind::WorkspaceMembers {
                if let Ok(id) = p.r#ref.parse::<Uuid>() {
                    out.insert(id);
                }
            }
        }
    };
    for g in grants {
        scan(&g.principals);
    }
    for r in rules {
        scan(&r.exempt);
    }
    out
}

/// Replace every `workspace_members` principal in `grants` and `rules` with
/// the `User` principals it stands for, read from the database on `conn`.
///
/// Runs ONE query for every workspace referenced by the whole model, then
/// mints a username for any member that has none (module doc). After this
/// returns, no `PrincipalKind::WorkspaceMembers` survives anywhere in
/// `grants`/`rules` — which is what lets `compile_grant`/`compile_rule` keep
/// refusing that kind outright as a defence in depth: reaching them with an
/// unexpanded principal is a bug in THIS function, not an authoring mistake,
/// and a refusal says so loudly instead of emitting a policy that silently
/// grants nobody (an `include` that fails closed) or silently grants everybody
/// the subtract was meant to remove (an `exclude` that fails OPEN — the reason
/// dropping was never an option; see `grants.rs`'s module doc).
///
/// **A ref that does not parse as a uuid expands to nothing**, rather than
/// erroring. That is exactly what the evaluator does with it
/// (`egret_perm::eval::matches_principal` → `parse::<Uuid>().map(...).
/// unwrap_or(false)`), and requirement 3 of this work is that the oracle and
/// the compiler answer identically for the same model. It is logged.
pub(crate) async fn expand_workspace_members(
    conn: &mut sqlx::PgConnection,
    grants: &mut [Grant],
    rules: &mut [Rule],
) -> Result<(), CompileError> {
    let wanted = referenced_workspaces(grants, rules);
    if wanted.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = wanted.iter().copied().collect();

    // The membership predicate, mirroring `egret_auth::grants::
    // AccessGrantStore::grants_for`'s workspace arm plus
    // `egret_auth::effective`'s `effect = 'allow'` filter — minus the
    // `user_groups` join (module doc, narrowing 1).
    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT ag.scope_id AS workspace_id, ag.principal_id AS user_id, \
                u.username::text AS username \
           FROM access_grants ag \
           JOIN users u ON u.id = ag.principal_id \
          WHERE ag.principal_type = 'user' \
            AND ag.scope_type = 'workspace' \
            AND ag.scope_id = ANY($1) \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;

    // Mint a username for anyone who has none, so the policy names the
    // principal the engine will actually see rather than silently omitting
    // them (module doc). Deduplicated by user id: one person can be a member
    // of several referenced workspaces.
    let mut resolved: HashMap<Uuid, String> = HashMap::new();
    let mut to_mint: BTreeSet<Uuid> = BTreeSet::new();
    for row in &rows {
        match &row.username {
            Some(u) => {
                resolved.insert(row.user_id, u.clone());
            }
            None => {
                to_mint.insert(row.user_id);
            }
        }
    }
    for user_id in to_mint {
        let minted = ensure_username_via(conn, UserId(user_id))
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?;
        tracing::info!(
            %user_id,
            username = %minted,
            "minted a username for a workspace member so the compiled policy can name them"
        );
        resolved.insert(user_id, minted);
    }

    let mut members: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in &rows {
        let Some(name) = resolved.get(&row.user_id) else {
            continue;
        };
        members
            .entry(row.workspace_id)
            .or_default()
            .push(name.clone());
    }
    // Sorted + deduplicated: the compiled body is diffed byte-for-byte against
    // what is stored (`apply_diff_in_tx`), so an unstable principal order
    // would rewrite every policy on every recompile and bump every plugin's
    // `policy_version` for nothing.
    for names in members.values_mut() {
        names.sort();
        names.dedup();
    }
    // A referenced workspace with no members at all still has to appear in the
    // map, so substitution knows it expanded to zero rather than that it was
    // never looked up.
    for id in &ids {
        members.entry(*id).or_default();
    }

    for g in grants.iter_mut() {
        substitute(&mut g.principals, &members);
    }
    for r in rules.iter_mut() {
        substitute(&mut r.exempt, &members);
    }
    Ok(())
}

/// The workspace ids `username` currently has a live, direct
/// (`principal_type = 'user'`), `allow`, unexpired membership in — the exact
/// predicate [`expand_workspace_members`] reads, in the OTHER direction
/// (workspace -> members here reads member -> workspaces), off the SAME
/// `access_grants` rows joined on the STORED `users.username`.
///
/// **SP4a Task 15 (D3 defect 2).** `/perm/explain` used to take its subject's
/// workspace memberships from the REQUEST BODY (`ExplainSubject::workspaces`,
/// caller-supplied) instead of resolving them here, so a caller naming just a
/// `user` — the natural way to call the endpoint, and the only way that
/// works at all once the compiled policy is what actually decides access —
/// got `workspaces: []` by `#[serde(default)]` and therefore `denied` for
/// every `workspace_members` grant, however real the compiled Ranger policy
/// made that access. The evaluator and the compiler now read the same table
/// through the same join on `users.username`: this function is that read for
/// `explain`, called with `body.subject.user` in place of a caller-supplied
/// list. A username with no `users` row (never minted, or simply wrong)
/// resolves to no workspaces, which is correct — that identity does not
/// exist on any engine either.
pub(crate) async fn workspaces_for_username(
    conn: &mut sqlx::PgConnection,
    username: &str,
) -> Result<Vec<Uuid>, CompileError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT ag.scope_id FROM access_grants ag \
           JOIN users u ON u.id = ag.principal_id \
          WHERE u.username = $1 \
            AND ag.principal_type = 'user' \
            AND ag.scope_type = 'workspace' \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(username)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;
    Ok(ids)
}

/// Rewrite one `PrincipalSet` in place: every `WorkspaceMembers` entry becomes
/// the `User` entries it stands for, everything else is carried through
/// untouched and in its original order. Duplicates are collapsed — a grant
/// that names `alice` explicitly AND the workspace she belongs to must not
/// emit `alice` twice into the same Ranger item.
fn substitute(set: &mut PrincipalSet, members: &HashMap<Uuid, Vec<String>>) {
    substitute_one(&mut set.include, members);
    substitute_one(&mut set.exclude, members);
}

fn substitute_one(list: &mut Vec<Principal>, members: &HashMap<Uuid, Vec<String>>) {
    if !list
        .iter()
        .any(|p| p.kind == PrincipalKind::WorkspaceMembers)
    {
        return;
    }
    let mut out: Vec<Principal> = Vec::with_capacity(list.len());
    let mut seen: BTreeMap<(&'static str, String), ()> = BTreeMap::new();
    for p in list.drain(..) {
        let expanded: Vec<Principal> = match p.kind {
            PrincipalKind::WorkspaceMembers => {
                let names = p
                    .r#ref
                    .parse::<Uuid>()
                    .ok()
                    .and_then(|id| members.get(&id))
                    .cloned()
                    .unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        workspace_ref = %p.r#ref,
                        "a workspace_members principal expanded to no engine principal — it \
                         names no workspace, or the workspace has no direct user members with \
                         a live allow grant"
                    );
                }
                names
                    .into_iter()
                    .map(|name| Principal {
                        kind: PrincipalKind::User,
                        r#ref: name,
                    })
                    .collect()
            }
            _ => vec![p],
        };
        for e in expanded {
            if seen
                .insert((e.kind.as_str(), e.r#ref.clone()), ())
                .is_none()
            {
                out.push(e);
            }
        }
    }
    *list = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_principal(id: Uuid) -> Principal {
        Principal {
            kind: PrincipalKind::WorkspaceMembers,
            r#ref: id.to_string(),
        }
    }

    fn user(name: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            r#ref: name.to_string(),
        }
    }

    #[test]
    fn a_workspace_expands_to_its_members_in_place() {
        let ws = Uuid::new_v4();
        let mut members = HashMap::new();
        members.insert(ws, vec!["alice".to_string(), "bob-2".to_string()]);
        let mut set = PrincipalSet {
            include: vec![user("carol"), ws_principal(ws)],
            exclude: vec![],
        };
        substitute(&mut set, &members);
        assert_eq!(
            set.include,
            vec![user("carol"), user("alice"), user("bob-2")],
            "explicit principals keep their place; the expansion follows, sorted"
        );
    }

    #[test]
    fn an_already_named_member_is_not_named_twice() {
        let ws = Uuid::new_v4();
        let mut members = HashMap::new();
        members.insert(ws, vec!["alice".to_string()]);
        let mut set = PrincipalSet {
            include: vec![user("alice"), ws_principal(ws)],
            exclude: vec![],
        };
        substitute(&mut set, &members);
        assert_eq!(set.include, vec![user("alice")]);
    }

    /// The `exclude` half is the one that fails OPEN when it is got wrong: a
    /// subtract that evaporates grants MORE than the model states. Expanding
    /// it to zero principals must leave the list EMPTY (so `compile_grant`
    /// emits no exception block at all), never an entry naming nobody.
    #[test]
    fn an_empty_workspace_expands_to_nothing_rather_than_to_an_empty_item() {
        let ws = Uuid::new_v4();
        let mut members = HashMap::new();
        members.insert(ws, Vec::new());
        let mut set = PrincipalSet {
            include: vec![ws_principal(ws)],
            exclude: vec![ws_principal(ws)],
        };
        substitute(&mut set, &members);
        assert!(set.include.is_empty());
        assert!(set.exclude.is_empty());
    }

    /// Same answer the evaluator gives a ref it cannot parse
    /// (`matches_principal` → `unwrap_or(false)`): it matches nobody.
    #[test]
    fn a_ref_that_is_not_a_uuid_expands_to_nothing() {
        let mut set = PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::WorkspaceMembers,
                r#ref: "not-a-uuid".into(),
            }],
            exclude: vec![],
        };
        substitute(&mut set, &HashMap::new());
        assert!(set.include.is_empty());
    }

    #[test]
    fn referenced_workspaces_skips_unparseable_refs() {
        let ws = Uuid::new_v4();
        let mut set = PrincipalSet::default();
        set.include.push(ws_principal(ws));
        set.exclude.push(Principal {
            kind: PrincipalKind::WorkspaceMembers,
            r#ref: "nope".into(),
        });
        let grant = Grant {
            id: Uuid::nil(),
            effect: egret_perm::Effect::Allow,
            actions: vec![egret_perm::Action::Read],
            scope: egret_perm::Scope {
                catalog_id: Uuid::nil(),
                schema: None,
                table: None,
                column: None,
            },
            principals: set,
            origin: egret_perm::GrantOrigin::Explicit,
        };
        let found = referenced_workspaces(&[grant], &[]);
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![ws]);
    }
}
