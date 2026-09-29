//! Expanding a principal that stands for PEOPLE — `workspace_members` (SP4a
//! **D3**, on the branch D7's live measurement forced) and, since **SP4c Task
//! 2b**, `role` — into the member list enumerated at compile time.
//!
//! **Why `role` is here too, and why it was a defect that it was not.** D0
//! (`docs/en/plans/2026-09-28-sp4c-d0-result.md`) measured that a Ranger policy
//! item naming a ROLE matches nobody on the Trino path egret provisions: the
//! roles document is fetched from a `/service/roles/download/<svc>` route egret
//! does not serve, so `lastKnownRoleVersion` never leaves `-1`, and Trino's
//! plugin passes `userRoles` as a hardcoded `null` with `disableRoleResolution`
//! defaulting true. Until SP4c, `compile_grant` put a `role` principal straight
//! into that bucket, so a role grant matched nobody in the engine while
//! `egret_perm::eval::matches_principal` matched it — meaning `explain`
//! reported an access the engine did not grant. A role now expands to its
//! holders' usernames exactly as a workspace does, through the one
//! [`role_usernames`] SP4c's clearance compiler also uses.
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
//! **This module no longer answers the reverse question, and nothing asks it
//! (SP4d-2a Task 5).** `workspaces_for_username` and `roles_for_username`
//! lived here so `explain` could resolve "which workspaces / roles does this
//! username hold" off the identical `access_grants` predicate the expansions
//! below read, keeping the retired model evaluator and the compiler from
//! silently disagreeing about who a `workspace_members` or `role` principal
//! names. `explain` now evaluates the compiled artifact, where both have
//! already been expanded into usernames by [`expand_enumerated_principals`],
//! so there was no caller left for either. The removal note sits at their old
//! site, further down this file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use uuid::Uuid;

use egret_auth::users::{ensure_username_via, usernames_via};
use egret_core::UserId;
use egret_perm::{CompileError, Grant, Principal, PrincipalKind, PrincipalSet, Rule};

/// The usernames of the users currently holding each role NAME in `roles` —
/// the expansion SP4c needs twice over (**SP4c Task 2 and Task 2b**).
///
/// **Why a role name has to be expanded at all.** D0
/// (`docs/en/plans/2026-09-28-sp4c-d0-result.md`) measured on the Trino path
/// egret provisions that a Ranger policy item naming a ROLE matches nobody: the
/// plugin fetches roles from `/service/roles/download/<service>`, a separate
/// document with its own version counter that egret's six `/service` GET routes
/// do not include, so the poll 404s and `lastKnownRoleVersion` never leaves
/// `-1`; Trino's built-in plugin also passes `userRoles` as a hardcoded `null`
/// and defaults `disableRoleResolution` to `true`. A role name is therefore
/// exactly as unresolvable as a group (D7), and the only enforceable form of
/// "the holders of role X" is the member list enumerated at compile time.
///
/// **A role is scope-free here, deliberately.** `access_grants` scopes a role
/// grant to the platform, a workspace or an object, but the thing being
/// compiled is a COLUMN read, which carries no workspace context on any engine
/// — and `egret_perm::eval::matches_principal` matches
/// `PrincipalKind::Role` against a flat `Subject::roles` with no scope either.
/// Reading any live `allow` grant of the role therefore keeps the compiler and
/// the evaluator in agreement, which is the property SP4c Task 2b exists to
/// restore. It is WIDER than Cedar's object-level check in one corner (Cedar
/// loads the grants for the resource's OWN workspace, so a role held only in
/// another workspace does not clear an object there); an engine read cannot
/// tell those apart, and narrowing to platform scope would instead DENY people
/// Cedar clears.
///
/// **`effect = 'allow'` only**, mirroring the Cedar PDP's own clearance read
/// (`egret_auth::pdp`: a `deny`-effect grant strips access, so its role name
/// must not count as holding the role) and `expand_enumerated_principals`'s
/// membership predicate.
///
/// **The name emitted is the STORED `users.username`, read and never minted.**
/// `usernames_via` is the read-only, never-minting half of the username API:
/// `ensure_username` lowercases, slugifies and appends `-2`/`-3` on collision,
/// so a re-derived slug names the WRONG person while compiling, deploying and
/// reporting success. A holder with no username yet is absent from the map and
/// is logged, not guessed — and absent is the right answer, not merely the safe
/// one: the bare username is the only identity an engine ever sees for a
/// person, so a user without one is not a principal on any engine, cannot
/// appear in an `include` item either, and therefore cannot be granted the
/// access an `exclude` would have subtracted.
///
/// Ordering is sorted + deduplicated because the compiled body is diffed
/// byte-for-byte against what is stored (`apply_diff_in_tx`); an unstable
/// principal order would rewrite every policy on every recompile.
pub(crate) async fn role_usernames(
    conn: &mut sqlx::PgConnection,
    roles: &[String],
) -> Result<BTreeMap<String, Vec<String>>, CompileError> {
    // Every requested role appears in the map, even with no holders, so a
    // caller can tell "expanded to nobody" from "never looked up".
    let mut out: BTreeMap<String, Vec<String>> = roles
        .iter()
        .map(|r| (r.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    if out.is_empty() {
        return Ok(out);
    }
    let wanted: Vec<String> = out.keys().cloned().collect();

    let holders: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT r.name, ag.principal_id \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
          WHERE ag.principal_type = 'user' \
            AND r.name = ANY($1) \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(&wanted)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;

    let ids: Vec<Uuid> = {
        let mut set: BTreeSet<Uuid> = BTreeSet::new();
        for (_, id) in &holders {
            set.insert(*id);
        }
        set.into_iter().collect()
    };
    let names = usernames_via(conn, &ids)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;

    for (role, user_id) in &holders {
        match names.get(user_id) {
            Some(name) => out.entry(role.clone()).or_default().push(name.clone()),
            None => tracing::warn!(
                %user_id,
                role = %role,
                "a holder of this role has no `users.username` yet, so it is not named in the \
                 compiled policy — that identity does not exist on any engine either (the bare \
                 username is what an engine sees), and it is never guessed from the email"
            ),
        }
    }
    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }
    Ok(out)
}

/// One workspace-membership row as read for expansion: who, and the username
/// the engine will see (NULL until it is minted).
#[derive(sqlx::FromRow)]
struct MemberRow {
    workspace_id: Uuid,
    user_id: Uuid,
    username: Option<String>,
}

/// Every workspace id a `workspace_members` principal names, and every role
/// NAME a `role` principal names, across `grants` and `rules`. A workspace ref
/// that does not parse as a uuid is NOT collected — see
/// [`expand_enumerated_principals`] for why that is the same answer the
/// evaluator gives.
fn referenced_principals(grants: &[Grant], rules: &[Rule]) -> (BTreeSet<Uuid>, BTreeSet<String>) {
    let mut workspaces = BTreeSet::new();
    let mut roles = BTreeSet::new();
    let mut scan = |set: &PrincipalSet| {
        for p in set.include.iter().chain(set.exclude.iter()) {
            match p.kind {
                PrincipalKind::WorkspaceMembers => {
                    if let Ok(id) = p.r#ref.parse::<Uuid>() {
                        workspaces.insert(id);
                    }
                }
                PrincipalKind::Role => {
                    roles.insert(p.r#ref.clone());
                }
                _ => {}
            }
        }
    };
    for g in grants {
        scan(&g.principals);
    }
    for r in rules {
        scan(&r.exempt);
    }
    (workspaces, roles)
}

/// Replace every `workspace_members` **and every `role`** principal in `grants`
/// and `rules` with the `User` principals it stands for, read from the database
/// on `conn`.
///
/// Runs ONE query for every workspace referenced by the whole model (minting a
/// username for any member that has none — module doc) and ONE for every role
/// name ([`role_usernames`], which reads and never mints). After this returns,
/// no `PrincipalKind::WorkspaceMembers` and no `PrincipalKind::Role` survives
/// anywhere in `grants`/`rules` — which is what lets
/// `compile_grant`/`compile_rule` refuse both kinds outright as a defence in
/// depth: reaching them with an unexpanded principal is a bug in THIS function,
/// not an authoring mistake, and a refusal says so loudly instead of emitting a
/// policy that silently grants nobody (an `include` that fails closed) or
/// silently grants everybody the subtract was meant to remove (an `exclude`
/// that fails OPEN — the reason dropping was never an option; see `grants.rs`'s
/// module doc).
///
/// **`role` joined this function in SP4c Task 2b, and it is a FIX.** Before it,
/// `compile_grant` put a `role` principal straight into the Ranger `roles`
/// bucket (`grants.rs:105`), which D0 measured is never populated — so a role
/// grant matched nobody in the engine while `egret_perm::eval` matched it
/// happily and `explain` reported an access the engine did not grant. Same
/// divergence class D7 found for groups, on a path D7 did not look at. It is
/// the same expansion SP4c's clearance compiler needs, which is why both go
/// through [`role_usernames`] rather than through two copies of "who holds this
/// role".
///
/// **The ORDER of the two expansions matters, and members go first.** Workspace
/// membership MINTS a missing username; a role expansion only READS one. A
/// person named on one side of a subtract by their workspace and on the other
/// by a role must therefore be resolved to the same name on both sides, or the
/// `exclude` half would silently lose them and the compiled policy would grant
/// more than the model states. Expanding members first means any mint has
/// already happened — on this same connection, so the role read sees it — and
/// the only names a role expansion can still omit belong to people who have no
/// engine identity at all and so cannot be named by any `include` item either.
///
/// **A ref that does not parse as a uuid expands to nothing**, rather than
/// erroring. That is exactly what the evaluator does with it
/// (`egret_perm::eval::matches_principal` → `parse::<Uuid>().map(...).
/// unwrap_or(false)`), and requirement 3 of this work is that the oracle and
/// the compiler answer identically for the same model. It is logged.
pub(crate) async fn expand_enumerated_principals(
    conn: &mut sqlx::PgConnection,
    grants: &mut [Grant],
    rules: &mut [Rule],
) -> Result<(), CompileError> {
    let (wanted, wanted_roles) = referenced_principals(grants, rules);
    if wanted.is_empty() && wanted_roles.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = wanted.iter().copied().collect();

    // The membership predicate, mirroring `egret_auth::grants::
    // AccessGrantStore::grants_for`'s workspace arm plus
    // `egret_auth::effective`'s `effect = 'allow'` filter — minus the
    // `user_groups` join (module doc, narrowing 1).
    let rows: Vec<MemberRow> = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
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
        .map_err(|e| CompileError::Db(e.to_string()))?
    };

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

    // AFTER the mints above, so a person reachable both as a workspace member
    // and as a role holder resolves to the SAME name on both sides of a
    // subtract (see this function's doc comment).
    let roles: Vec<String> = wanted_roles.into_iter().collect();
    let role_members = role_usernames(conn, &roles).await?;

    for g in grants.iter_mut() {
        substitute(&mut g.principals, &members, &role_members);
    }
    for r in rules.iter_mut() {
        substitute(&mut r.exempt, &members, &role_members);
    }
    Ok(())
}

// **`workspaces_for_username` and `roles_for_username` lived here and are
// gone (SP4d-2a Task 5).** They were `/perm/explain`'s reads of the subject's
// CURRENT workspace memberships (SP4a Task 15, D3 defect 2) and roles (SP4c
// Task 7) — each added after a live gate measured the caller-supplied field
// next to it answering for a subject the engine did not see. `explain` now
// evaluates the compiled artifact the engine downloads, where a
// `workspace_members` grant has already been expanded into usernames by
// [`expand_enumerated_principals`] below and a Ranger `roles` bucket matches
// nobody at all, so neither read had a caller left. The lesson they recorded
// survives where it still applies: on `routes::ExplainSubject`, which no
// longer HAS a `workspaces`, `roles` or `groups` field for a caller to answer
// with instead of the database. `role_usernames` below is the same predicate
// read from the other direction (role -> holders) and is still live, because
// the compiler needs it.

/// Rewrite one `PrincipalSet` in place: every `WorkspaceMembers` and every
/// `Role` entry becomes the `User` entries it stands for, everything else is
/// carried through untouched and in its original order. Duplicates are
/// collapsed — a grant that names `alice` explicitly AND the workspace she
/// belongs to (or a role she holds) must not emit `alice` twice into the same
/// Ranger item.
fn substitute(
    set: &mut PrincipalSet,
    members: &HashMap<Uuid, Vec<String>>,
    role_members: &BTreeMap<String, Vec<String>>,
) {
    substitute_one(&mut set.include, members, role_members);
    substitute_one(&mut set.exclude, members, role_members);
}

fn substitute_one(
    list: &mut Vec<Principal>,
    members: &HashMap<Uuid, Vec<String>>,
    role_members: &BTreeMap<String, Vec<String>>,
) {
    if !list.iter().any(|p| {
        matches!(
            p.kind,
            PrincipalKind::WorkspaceMembers | PrincipalKind::Role
        )
    }) {
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
                as_users(names)
            }
            PrincipalKind::Role => {
                let names = role_members.get(&p.r#ref).cloned().unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        role = %p.r#ref,
                        "a role principal expanded to no engine principal — no such role, or \
                         nobody holds it with a live allow grant and a minted username. The \
                         Ranger `roles` bucket is NOT used as a fallback: D0 measured that it \
                         matches nobody, so naming the role there would be a control that does \
                         nothing (in an include) or one that denies everybody (in a \
                         denyException)"
                    );
                }
                as_users(names)
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

fn as_users(names: Vec<String>) -> Vec<Principal> {
    names
        .into_iter()
        .map(|name| Principal {
            kind: PrincipalKind::User,
            r#ref: name,
        })
        .collect()
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
        substitute(&mut set, &members, &BTreeMap::new());
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
        substitute(&mut set, &members, &BTreeMap::new());
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
        substitute(&mut set, &members, &BTreeMap::new());
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
        substitute(&mut set, &HashMap::new(), &BTreeMap::new());
        assert!(set.include.is_empty());
    }

    #[test]
    fn referenced_principals_skips_unparseable_workspace_refs() {
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
        let (found, _) = referenced_principals(&[grant], &[]);
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![ws]);
    }
}
