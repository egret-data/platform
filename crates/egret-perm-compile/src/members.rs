//! Expanding a principal that stands for PEOPLE — `workspace_members` (SP4a
//! **D3**, on the branch D7's live measurement forced), since **SP4c Task 2b**
//! `role`, and since **J-F3 Task 1b** (design R8) every `group` except
//! `public` — into the member list enumerated at compile time.
//!
//! **Why `group` is here, and why it was a defect that it was not.** D7 (below)
//! measured that a Ranger item naming a group matches nobody on any engine
//! egret provisions, yet until R8 `compile_grant` and `compile_rule` put a
//! `group` principal straight into Ranger's `groups` bucket. An allow on a
//! group therefore granted nobody while `explain` said it did, and a group in a
//! deny include, in a grant's `exclude`, or in a rule exemption failed the
//! other way: the deny or the subtract silently evaporated, or the mask applied
//! to the people the author exempted. A group's membership is knowable —
//! `user_groups`, joined to `groups.name`, the key a principal stores — so a
//! group now expands to its members' usernames exactly as a role does, minted
//! when missing as a workspace member's are. `public` is the exception: every
//! engine resolves it with no group provider, and the compiler's own "applies
//! to everyone" item names it. Every writer of `user_groups` recompiles
//! (`crate::group_membership`).
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
//! holders' usernames exactly as a workspace does — minting a missing one, as
//! below, since the 2026-10-18 design R5 — through the one [`role_usernames`]
//! SP4c's clearance compiler also uses.
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
//! one the engine token minter will hand out later). A ROLE holder is minted
//! too since the 2026-10-18 design R5 ([`role_usernames`] says why reading
//! alone was not enough there either).
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
//!    would allow for such a subject. That is the fail-closed direction. It
//!    was chosen because the writers of `user_groups` live in
//!    `egret-auth::groups`, BELOW this crate, and could not call the
//!    recompile. **J-F3 Task 1b removed that obstacle** — every writer now
//!    recompiles through the `egret_auth::groups::GroupMembershipRecompiler`
//!    port, and the per-login sync recompiles only the groups a login moved —
//!    but it expanded only a `group` PRINCIPAL (design R8), not a group
//!    holding a workspace membership or a role (`role_usernames` reads
//!    `principal_type = 'user'` too). Extending it there needs
//!    `affected_services_for_group_membership` to gain the workspace and role
//!    arms first, or a membership change of such a group goes stale on
//!    REMOVAL, which fails open; until then the narrowing stands.
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

use egret_auth::users::ensure_username_via;
use egret_core::UserId;
use egret_perm::{CompileError, Grant, Principal, PrincipalKind, PrincipalSet, Rule, PUBLIC_GROUP};

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
/// **The name emitted is the STORED `users.username`, never a re-derivation**
/// (module doc): `ensure_username` lowercases, slugifies and appends `-2`/`-3`
/// on collision, so a re-derived slug names the WRONG person while compiling,
/// deploying and reporting success.
///
/// **A holder with no username yet is MINTED by a compile (2026-10-18 design
/// R5), and left out only by a read.** This used to read alone and argue that
/// absent was the right answer, because a person with no username is no
/// principal on any engine. That held at compile time and went stale the
/// moment they got one: the first query mints a username and recompiles
/// nothing, so *allow `{public}`, deny role R* reached a holder of R, an
/// exclude on R stopped subtracting them, and a clearing role's holder was
/// refused a column Cedar clears them for (R6). Group and workspace expansion
/// already minted for the same reason (module doc), so
/// [`MissingUsername::Mint`] does the same here, on the caller's connection,
/// in ascending user-id order. [`MissingUsername::Omit`] keeps the read-only
/// answer for `crate::view_audit::owner_cleared`, which is served on GET
/// routes and compares against a username it was given, so a holder without
/// one can never be its owner. Both read the one predicate below, so the
/// compile and the audit cannot disagree about who holds a role.
///
/// The holders are joined to `users`, as group and workspace expansion are:
/// an `access_grants` row whose user is gone names nobody.
///
/// Ordering is sorted + deduplicated because the compiled body is diffed
/// byte-for-byte against what is stored (`apply_diff_in_tx`); an unstable
/// principal order would rewrite every policy on every recompile.
pub(crate) async fn role_usernames(
    conn: &mut sqlx::PgConnection,
    roles: &[String],
    missing: MissingUsername,
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

    let holders = role_holders_via(conn, &wanted).await?;

    let mut names: HashMap<Uuid, String> = HashMap::new();
    // A `BTreeSet`, so the mints below take each `users` row lock in
    // ascending id order, the order group and workspace expansion mint in:
    // two compiles minting the same holders then queue instead of deadlocking.
    let mut unnamed: BTreeSet<Uuid> = BTreeSet::new();
    for h in &holders {
        match &h.username {
            Some(u) => {
                names.insert(h.user_id, u.clone());
            }
            None => {
                unnamed.insert(h.user_id);
            }
        }
    }
    if missing == MissingUsername::Mint {
        for user_id in unnamed {
            let minted = ensure_username_via(conn, UserId(user_id))
                .await
                .map_err(|e| CompileError::Db(e.to_string()))?;
            tracing::info!(
                %user_id,
                username = %minted,
                "minted a username for a role holder so the compiled policy can name them"
            );
            names.insert(user_id, minted);
        }
    }

    for RoleHolderRow {
        role_name: role,
        user_id,
        ..
    } in &holders
    {
        match names.get(user_id) {
            Some(name) => out.entry(role.clone()).or_default().push(name.clone()),
            None => tracing::warn!(
                %user_id,
                role = %role,
                "a holder of this role has no `users.username` yet, so this read does not name \
                 them — it never mints (only a compile does), and a name is never guessed from \
                 the email"
            ),
        }
    }
    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }
    Ok(out)
}

/// What [`role_usernames`] does with a holder who has no `users.username`
/// yet (2026-10-18 design R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingUsername {
    /// Mint one on the caller's connection with `ensure_username_via`, so the
    /// compiled policy names the identity the engine will see. Every compile.
    Mint,
    /// Leave the holder out. A read that must not write: `owner_cleared`,
    /// served on GET routes.
    Omit,
}

// ---------------------------------------------------------------------------
// The membership reads (People & groups, spec §4.1)
// ---------------------------------------------------------------------------
//
// **One definition of membership.** These three reads are what the compiler
// expands a `role`, a `workspace_members` and a non-`public` `group` principal
// with, and they are the ONLY place that answers "who holds this role", "who
// is a member of this workspace", "who is in this group" for anything that
// claims to say what engines enforce: `crate::people` (Governance's People
// pages) calls the same functions rather than a copy of their SQL, so a page
// cannot list a member the compiled policy does not name, or miss one it does.
// Change a predicate here and both move together.

/// One role-holding row: which role by NAME (the key a `role` principal
/// stores; one name can exist at platform AND workspace scope, and both count)
/// the holding user, and their username (NULL until minted).
#[derive(sqlx::FromRow)]
pub(crate) struct RoleHolderRow {
    pub(crate) role_name: String,
    pub(crate) user_id: Uuid,
    pub(crate) username: Option<String>,
}

/// One workspace-membership row as read for expansion: who, and the username
/// the engine will see (NULL until it is minted).
#[derive(sqlx::FromRow)]
pub(crate) struct MemberRow {
    pub(crate) workspace_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) username: Option<String>,
}

/// One group-membership row as read for expansion (J-F3 Task 1b): which group
/// by NAME — the key a principal stores — who, and their username (NULL until
/// minted).
#[derive(sqlx::FromRow)]
pub(crate) struct GroupMemberRow {
    pub(crate) group_name: String,
    pub(crate) user_id: Uuid,
    pub(crate) username: Option<String>,
}

/// The live holders of each role NAME in `roles`: users only (a group holding
/// a role is not expanded — module doc, narrowing 1), `effect = 'allow'` only,
/// not expired, at any scope ([`role_usernames`]' doc says why scope-free).
/// One row per grant, so a user holding a role twice appears twice; callers
/// deduplicate. A holder with no `users.username` is still a row here, with
/// `username` NULL: [`role_usernames`] mints one at compile
/// ([`MissingUsername::Mint`]) or leaves them out on a read
/// ([`MissingUsername::Omit`]).
///
/// Joined to `users`, as [`workspace_members_via`] and [`group_members_via`]
/// are (2026-10-18 design R5): a grant row whose user is gone names nobody and
/// could not be minted for, so it is no holder — for the compiler and for
/// People alike. `UserRepo::hard_delete` sweeps a user's `access_grants`, so
/// this drops only rows written around it.
pub(crate) async fn role_holders_via(
    conn: &mut sqlx::PgConnection,
    roles: &[String],
) -> Result<Vec<RoleHolderRow>, CompileError> {
    if roles.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT r.name AS role_name, ag.principal_id AS user_id, \
            u.username::text AS username \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
           JOIN users u ON u.id = ag.principal_id \
          WHERE ag.principal_type = 'user' \
            AND r.name = ANY($1) \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(roles)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))
}

/// The direct user members of each workspace in `ids`: the membership
/// predicate, mirroring `egret_auth::grants::AccessGrantStore::grants_for`'s
/// workspace arm plus `egret_auth::effective`'s `effect = 'allow'` filter —
/// minus the `user_groups` join (module doc, narrowing 1).
pub(crate) async fn workspace_members_via(
    conn: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<Vec<MemberRow>, CompileError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
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
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))
}

/// The direct members of each group NAME in `names` (J-F3 Task 1b):
/// `user_groups` → `groups.name`, the key a principal stores and the one
/// `crate::principal_refs` validates against. No expiry exists on
/// `user_groups`, and a nested group does not exist in egret.
pub(crate) async fn group_members_via(
    conn: &mut sqlx::PgConnection,
    names: &[String],
) -> Result<Vec<GroupMemberRow>, CompileError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT g.name AS group_name, ug.user_id, u.username::text AS username \
       FROM user_groups ug \
       JOIN groups g ON g.id = ug.group_id \
       JOIN users u ON u.id = ug.user_id \
      WHERE g.name = ANY($1)",
    )
    .bind(names)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))
}

/// What a model references that has to be enumerated: workspace ids, role
/// names and (since J-F3 Task 1b) group names.
#[derive(Default)]
struct Referenced {
    workspaces: BTreeSet<Uuid>,
    roles: BTreeSet<String>,
    groups: BTreeSet<String>,
}

/// Whether `p` is a group this module expands: every group except
/// [`PUBLIC_GROUP`], the one name every engine resolves without a group
/// provider (`egret_perm::PUBLIC_GROUP`'s doc) — the compiler's own "applies to
/// everyone" item uses it, and expanding it would turn "everyone" into "every
/// user who has a row naming a group called public".
fn is_expanded_group(p: &Principal) -> bool {
    p.kind == PrincipalKind::Group && p.r#ref != PUBLIC_GROUP
}

/// Every workspace id a `workspace_members` principal names, every role NAME a
/// `role` principal names, and every group NAME a non-`public` `group`
/// principal names, across `grants` and `rules`. A workspace ref that does not
/// parse as a uuid is NOT collected — see [`expand_enumerated_principals`] for
/// why that is the same answer the evaluator gives.
fn referenced_principals(grants: &[Grant], rules: &[Rule]) -> Referenced {
    let mut out = Referenced::default();
    let mut scan = |set: &PrincipalSet| {
        for p in set.include.iter().chain(set.exclude.iter()) {
            match p.kind {
                PrincipalKind::WorkspaceMembers => {
                    if let Ok(id) = p.r#ref.parse::<Uuid>() {
                        out.workspaces.insert(id);
                    }
                }
                PrincipalKind::Role => {
                    out.roles.insert(p.r#ref.clone());
                }
                PrincipalKind::Group if is_expanded_group(p) => {
                    out.groups.insert(p.r#ref.clone());
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
    out
}

/// Replace every `workspace_members`, every `role` **and every non-`public`
/// `group`** principal in `grants` and `rules` with the `User` principals it
/// stands for, read from the database on `conn`.
///
/// Runs ONE query for every workspace referenced by the whole model and ONE for
/// every group (minting a username for any member that has none — module doc),
/// and ONE for every role name ([`role_usernames`], which mints the same way
/// since the 2026-10-18 design R5). After this returns, no `PrincipalKind::WorkspaceMembers`, no
/// `PrincipalKind::Role` and no group other than `public` survives anywhere in
/// `grants`/`rules` — which is what lets
/// `compile_grant`/`compile_rule` refuse the first two kinds outright as a
/// defence in depth (a group is not refused there; `grants.rs`' module doc says
/// why): reaching them with an unexpanded principal is a bug in THIS function,
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
/// **The ORDER of the expansions: members first, then roles.** Every kind
/// MINTS a missing username now (a role since the 2026-10-18 design R5), and a
/// person named on one side of a subtract by their workspace and on the other
/// by a role resolves to the same name on both sides either way: a mint is
/// idempotent and runs on this same connection, so the role read sees a name
/// the member pass just minted and mints nobody twice. Before R5 the role
/// expansion only read, and this order was what kept such a person from
/// silently dropping out of the `exclude` half; it is kept so the member pass
/// mints in one ascending sweep and the role pass mints only the holders it
/// did not reach.
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
    let Referenced {
        workspaces: wanted,
        roles: wanted_roles,
        groups: wanted_groups,
    } = referenced_principals(grants, rules);
    if wanted.is_empty() && wanted_roles.is_empty() && wanted_groups.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = wanted.iter().copied().collect();

    // The direct user members of every referenced workspace
    // ([`workspace_members_via`]).
    let rows = workspace_members_via(conn, &ids).await?;

    // J-F3 Task 1b: the direct members of every referenced group, by group
    // NAME ([`group_members_via`]). Read alongside the workspace members so
    // both MINT before the role read below (this function's doc comment, "the
    // ORDER").
    let group_names: Vec<String> = wanted_groups.into_iter().collect();
    let group_rows = group_members_via(conn, &group_names).await?;

    // Mint a username for anyone who has none, so the policy names the
    // principal the engine will actually see rather than silently omitting
    // them (module doc). Deduplicated by user id: one person can be a member
    // of several referenced workspaces and groups.
    let mut resolved: HashMap<Uuid, String> = HashMap::new();
    let mut to_mint: BTreeSet<Uuid> = BTreeSet::new();
    let people = rows
        .iter()
        .map(|r| (r.user_id, &r.username))
        .chain(group_rows.iter().map(|r| (r.user_id, &r.username)));
    for (user_id, username) in people {
        match username {
            Some(u) => {
                resolved.insert(user_id, u.clone());
            }
            None => {
                to_mint.insert(user_id);
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
            "minted a username for a workspace or group member so the compiled policy can \
             name them"
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

    // The same three steps for groups: every referenced group appears (an
    // empty group expands to nobody, not to "never looked up"), names sorted
    // and deduplicated.
    let mut group_members: BTreeMap<String, Vec<String>> = group_names
        .iter()
        .map(|g| (g.clone(), Vec::new()))
        .collect();
    for row in &group_rows {
        if let Some(name) = resolved.get(&row.user_id) {
            group_members
                .entry(row.group_name.clone())
                .or_default()
                .push(name.clone());
        }
    }
    for names in group_members.values_mut() {
        names.sort();
        names.dedup();
    }

    // AFTER the mints above, so the role pass mints only holders the member
    // pass did not reach (see this function's doc comment).
    let roles: Vec<String> = wanted_roles.into_iter().collect();
    let role_members = role_usernames(conn, &roles, MissingUsername::Mint).await?;

    let expansions = Expansions {
        workspaces: &members,
        roles: &role_members,
        groups: &group_members,
    };
    for g in grants.iter_mut() {
        substitute(&mut g.principals, &expansions);
    }
    for r in rules.iter_mut() {
        substitute(&mut r.exempt, &expansions);
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

/// The three member lists a substitution reads, keyed the way each principal
/// kind stores its ref.
struct Expansions<'a> {
    workspaces: &'a HashMap<Uuid, Vec<String>>,
    roles: &'a BTreeMap<String, Vec<String>>,
    groups: &'a BTreeMap<String, Vec<String>>,
}

/// Rewrite one `PrincipalSet` in place: every `WorkspaceMembers`, every `Role`
/// and every non-`public` `Group` entry becomes the `User` entries it stands
/// for, everything else is carried through untouched and in its original
/// order. Duplicates are collapsed — a grant that names `alice` explicitly AND
/// the workspace she belongs to (or a role she holds, or a group she is in)
/// must not emit `alice` twice into the same Ranger item.
fn substitute(set: &mut PrincipalSet, x: &Expansions<'_>) {
    substitute_one(&mut set.include, x);
    substitute_one(&mut set.exclude, x);
}

fn substitute_one(list: &mut Vec<Principal>, x: &Expansions<'_>) {
    if !list.iter().any(|p| {
        matches!(
            p.kind,
            PrincipalKind::WorkspaceMembers | PrincipalKind::Role
        ) || is_expanded_group(p)
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
                    .and_then(|id| x.workspaces.get(&id))
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
                let names = x.roles.get(&p.r#ref).cloned().unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        role = %p.r#ref,
                        "a role principal expanded to no engine principal — no such role, or \
                         nobody holds it with a live allow grant. The \
                         Ranger `roles` bucket is NOT used as a fallback: D0 measured that it \
                         matches nobody, so naming the role there would be a control that does \
                         nothing (in an include) or one that denies everybody (in a \
                         denyException)"
                    );
                }
                as_users(names)
            }
            PrincipalKind::Group if is_expanded_group(&p) => {
                let names = x.groups.get(&p.r#ref).cloned().unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        group = %p.r#ref,
                        "a group principal expanded to no engine principal — no such group, or \
                         it has no members. The Ranger `groups` bucket is NOT used as a \
                         fallback: D7 measured that no engine egret provisions resolves a \
                         group, so naming it there would be a control that does nothing"
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

    fn group(name: &str) -> Principal {
        Principal {
            kind: PrincipalKind::Group,
            r#ref: name.to_string(),
        }
    }

    static NO_ROLES: BTreeMap<String, Vec<String>> = BTreeMap::new();
    static NO_GROUPS: BTreeMap<String, Vec<String>> = BTreeMap::new();

    fn ws_only(members: &HashMap<Uuid, Vec<String>>) -> Expansions<'_> {
        Expansions {
            workspaces: members,
            roles: &NO_ROLES,
            groups: &NO_GROUPS,
        }
    }

    /// J-F3 Task 1b: a group expands to its members on BOTH sides, in place,
    /// deduplicated against an explicitly named member — and an empty group
    /// leaves nothing behind, never an entry naming the group.
    #[test]
    fn a_group_expands_to_its_members_and_an_empty_group_to_nothing() {
        let no_ws = HashMap::new();
        let mut groups = BTreeMap::new();
        groups.insert(
            "analysts".to_string(),
            vec!["alice".to_string(), "bob".to_string()],
        );
        groups.insert("nobody".to_string(), Vec::new());
        let x = Expansions {
            workspaces: &no_ws,
            roles: &NO_ROLES,
            groups: &groups,
        };
        let mut set = PrincipalSet {
            include: vec![user("alice"), group("analysts"), group("nobody")],
            exclude: vec![group("analysts")],
        };
        substitute(&mut set, &x);
        assert_eq!(set.include, vec![user("alice"), user("bob")]);
        assert_eq!(set.exclude, vec![user("alice"), user("bob")]);
    }

    /// `public` is the one group every engine resolves; it is never expanded,
    /// even if a `groups` row carries that name and has members.
    #[test]
    fn the_public_group_is_never_expanded() {
        let no_ws = HashMap::new();
        let mut groups = BTreeMap::new();
        groups.insert(PUBLIC_GROUP.to_string(), vec!["mallory".to_string()]);
        let x = Expansions {
            workspaces: &no_ws,
            roles: &NO_ROLES,
            groups: &groups,
        };
        let mut set = PrincipalSet {
            include: vec![group(PUBLIC_GROUP)],
            exclude: vec![],
        };
        substitute(&mut set, &x);
        assert_eq!(set.include, vec![group(PUBLIC_GROUP)]);

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
            principals: PrincipalSet {
                include: vec![group(PUBLIC_GROUP), group("analysts")],
                exclude: vec![],
            },
            origin: egret_perm::GrantOrigin::Explicit,
        };
        assert_eq!(
            referenced_principals(&[grant], &[])
                .groups
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["analysts".to_string()],
            "`public` is not even looked up"
        );
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
        substitute(&mut set, &ws_only(&members));
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
        substitute(&mut set, &ws_only(&members));
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
        substitute(&mut set, &ws_only(&members));
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
        substitute(&mut set, &ws_only(&HashMap::new()));
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
        let found = referenced_principals(&[grant], &[]).workspaces;
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![ws]);
    }
}
