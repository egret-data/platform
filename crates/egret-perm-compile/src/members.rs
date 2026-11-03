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
//! 1. **Group membership is expanded for groups and roles and workspaces.**
//!    A group added as a workspace member, or holding a role, is expanded to its
//!    members (`workspace_members_via`, `role_group_holders_via`), as the PDP
//!    does (`egret_auth::grants::AccessGrantStore::grants_for`). The membership
//!    writers recompile through the group-membership trigger, which covers the
//!    role and workspace arms.
//! 2. **An `expires_at` that lapses is not a write, so nothing recompiles.**
//!    A time-boxed membership (the access-request approval path grants one)
//!    stays in the compiled policy until the next recompile of that profile
//!    for any other reason. Fail-open. **CLOSED by the expiry sweep**
//!    ([`crate::expiry::recompile_lapsed_grants`], run hourly by `egret-bin`):
//!    each run recompiles the profiles that a grant lapsed inside the last two
//!    hours can change. The residual window is up to one hour after a lapse
//!    (the next tick) plus the sweep's own compile time. Lapses during a
//!    process downtime longer than two hours are seen only by the boot-time
//!    role recompile, which covers role grants and not workspace memberships.
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
use egret_perm::{
    CompileError, EngineGrant, Grant, PrincipalKind, PrincipalSet, Rule, PUBLIC_GROUP,
};
use egret_perm_ir::audience::{is_expanded_group, Directory};

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

    let mut holders = role_holders_via(conn, &wanted).await?;
    holders.extend(role_group_holders_via(conn, &wanted).await?);

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
            let minted = mint_for_compile(conn, user_id).await?;
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
            // Unreachable after `Mint` (a mint either returns a name or fails
            // the compile), so a warning there means a broken invariant. After
            // `Omit` it is the expected answer for someone who has never used
            // an engine, on a GET route: not worth a warning per request.
            None if missing == MissingUsername::Mint => tracing::warn!(
                %user_id,
                role = %role,
                "a role holder is still unnamed after minting, so the compiled policy does not \
                 name them"
            ),
            None => tracing::debug!(
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

/// Take the one compile-mint lock on `conn`'s transaction: a
/// transaction-scoped advisory lock every username mint a compile makes is
/// taken under ([`mint_for_compile`]), released at commit or rollback.
///
/// **Why there is a lock as well as the ascending sweep.**
/// [`premint_model_usernames`] mints everyone it can see in ascending id
/// order, but it reads statement by statement under READ COMMITTED, and the
/// passes after it read again: a person named, or left without a username, by
/// a write that commits in between is minted by a pass AFTER rows the sweep
/// already locked — out of order, and two compiles doing that over one pair of
/// people deadlock. Under this lock no two compiles mint at once.
///
/// **The order every transaction takes it in: profile locks
/// ([`crate::trigger::lock_services_in_tx`]), then this lock, then any row
/// write or mint.** A mint `UPDATE`s `users.username`, a `citext UNIQUE`
/// column, so it takes `FOR UPDATE` on the row, which waits on the `FOR KEY
/// SHARE` that any insert of a row referencing that user holds (a
/// `user_groups` membership, a `catalog_tag_assignments.assigned_by`, a
/// `classification_policies.created_by`), and on a `DELETE FROM users`. A
/// writer that took such a row lock first and only then reached its recompile
/// would wait here on a compile that waits on its row: a deadlock, surfacing
/// as a failed login or a refused write. So a writer that recompiles after
/// touching a user-referencing row calls this right after its profile locks,
/// BEFORE that write: the group-membership port
/// (`PermGroupRecompiler::lock_affected_in_tx`, which every membership writer
/// and `UserRepo::hard_delete` goes through), the catalog-tag write, the
/// classification write and the tag delete (`egret-bi`; the delete because
/// its row lock on the tag meets those writes' `tag_id` foreign keys). A writer whose own row writes take no
/// lock on a `users` row (a grant, a rule, an `access_grants` row, which has
/// no foreign key to `users`) may leave it to the compile.
///
/// It adds no cycle with the profile locks: it is always taken after them, and
/// nothing asks for a profile lock while holding it, because every recompile
/// locks its whole profile set first, and a request for a profile not locked
/// first is refused rather than waited on (below). `crate::view_tags::lock_view_tags_in_tx`
/// and the group and member advisory locks (`egret_auth::groups`) come before
/// the profile locks in any transaction that takes them. Re-taking it in the
/// same transaction is free. A mint outside a compile — `ensure_username` on
/// login or token issue, the People allocation route — touches one row in its
/// own short transaction and does not take it.
///
/// **What it costs.** The lock is global and held until commit. Every writer
/// that takes it early therefore serialises installation-wide behind every
/// other one, and behind any recompile that has someone to mint: an OIDC login
/// whose claims change a group the model names, an admin's group-member write,
/// a user's hard delete, a catalog-tag write, a clearance `set_policy` and a
/// tag delete. A recompile with nobody to mint never takes it. This is the
/// price of ruling the deadlock out. Taking it per user instead would need
/// every writer to know, before its write, which users the compile will mint.
///
/// **A profile set that grows after this lock (#792) is refused.** A writer
/// that takes this lock early reads its profile set again at its recompile
/// (`retag_bump_and_recompile`'s `services_bound_to_warehouses`,
/// `recompile_for_clearance_in_tx`'s `affected_services_for_classification`,
/// `lock_services_in_tx`'s tag-service expansion). A profile bound or created
/// in between would be locked while this lock is held, against the stated
/// order, and could deadlock with a compile of that profile. So this lock
/// marks the transaction (`egret.compile_mint_held`, transaction-local), and
/// the one profile-lock helper (`crate::trigger::lock_profile_rows_in_tx`)
/// answers a request for a profile this transaction has not locked yet with
/// [`CompileError::ProfileSetChanged`] while the mark is set, before any lock
/// statement: the write is refused (a retryable 409) instead of waiting.
pub async fn lock_compile_mints_in_tx(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('egret-compile-mint', 0)), \
                set_config('egret.compile_mint_held', 'on', true)",
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Mint `user_id`'s username for a compile, under
/// [`lock_compile_mints_in_tx`]. Nothing is locked when nobody needs a
/// username, the steady state.
async fn mint_for_compile(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<String, CompileError> {
    lock_compile_mints_in_tx(conn)
        .await
        .map_err(CompileError::from_db)?;
    ensure_username_via(conn, UserId(user_id))
        .await
        .map_err(crate::db_error::from_core)
}

/// Mint, in ONE ascending sweep, the username of every person a compile of
/// the current model would mint, and return how many were minted (Task 3
/// review; every `crate::trigger` recompile entry calls it after the profile
/// locks and before compiling).
///
/// **Why a sweep before the passes.** Each expansion mints on its own: the
/// member pass in [`expand_enumerated_principals`], then its role pass, then
/// the clearance's role pass, once per profile. Each is ascending, but the
/// sequence is not: one recompile can lock user 3 and then user 2, while a
/// concurrent recompile over other profiles locks 2 and then 3, and Postgres
/// aborts one of them. Minting everyone first, ascending, means the passes
/// find names already set and take no `users` row lock at all.
///
/// **What it guarantees, and what it does not (#790).** The set never names
/// someone no compile of the current model would name, because a mint is a
/// write a compile must not make for someone it does not name. It is meant to
/// COVER this transaction's passes, but that is not provable: it reads
/// statement by statement under READ COMMITTED, so a ref committed after it
/// is minted by a pass. What keeps compile mints deadlock-free is the mint
/// lock ([`lock_compile_mints_in_tx`]), under which no two compiles mint at
/// once in any order; this sweep only keeps that lock to one acquisition and
/// this transaction's `users` row locks ascending. That is also why it reads
/// the whole model rather than the profiles being recompiled: a narrower
/// sweep would have to restate, per profile, which refs each pass reads, the
/// copy this sweep exists to avoid, for little gain (#790, not done).
///
/// It is built from the SAME shared reads the passes and People use —
/// [`role_holders_via`], [`group_members_via`], [`workspace_members_via`] —
/// over the refs the whole model holds (grant principals, rule exemptions,
/// engine-grant principals (F11), a clearance's `required_roles`; `public` excluded; a workspace ref parsed as
/// the member pass parses it), which is what every recompile reads. No
/// membership predicate is restated here, so a change to one moves this sweep
/// with it.
pub(crate) async fn premint_model_usernames(
    conn: &mut sqlx::PgConnection,
) -> Result<usize, CompileError> {
    // The model's refs, by kind. Who each one stands for is then asked of the
    // shared membership reads below, never of a copy of their predicates, so
    // this sweep cannot drift from what the passes (and People) read.
    let refs: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind::text, ref FROM perm.grant_principals \
         UNION SELECT kind::text, ref FROM perm.rule_exemptions \
         UNION SELECT kind::text, ref FROM perm.engine_grant_principals \
         UNION SELECT 'role', unnest(required_roles) FROM classification_policies",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    let mut roles: BTreeSet<String> = BTreeSet::new();
    let mut groups: BTreeSet<String> = BTreeSet::new();
    let mut workspaces: BTreeSet<Uuid> = BTreeSet::new();
    for (kind, r) in refs {
        match kind.as_str() {
            "role" => {
                roles.insert(r);
            }
            "group" if r != PUBLIC_GROUP => {
                groups.insert(r);
            }
            // Parsed, as the member pass parses it, so a non-canonical
            // spelling is swept too; a ref that is no uuid names nobody.
            "workspace_members" => {
                if let Ok(id) = Uuid::parse_str(&r) {
                    workspaces.insert(id);
                }
            }
            _ => {}
        }
    }
    let roles: Vec<String> = roles.into_iter().collect();
    let groups: Vec<String> = groups.into_iter().collect();
    let workspaces: Vec<Uuid> = workspaces.into_iter().collect();

    // A `BTreeSet`: the mints below lock `users` rows in ascending id order.
    let mut unnamed: BTreeSet<Uuid> = BTreeSet::new();
    for h in role_holders_via(conn, &roles).await? {
        if h.username.is_none() {
            unnamed.insert(h.user_id);
        }
    }
    for m in group_members_via(conn, &groups).await? {
        if m.username.is_none() {
            unnamed.insert(m.user_id);
        }
    }
    for m in workspace_members_via(conn, &workspaces).await? {
        if m.username.is_none() {
            unnamed.insert(m.user_id);
        }
    }
    // The engine admins, when any engine carries an `egret-admin` set: every
    // recompile of such a profile names them (`crate::trigger`'s
    // `sync_admin_policy_in_tx`).
    if !egret_policy::default_policies::admin_policy_services_via(conn)
        .await
        .map_err(crate::db_error::from_store)?
        .is_empty()
    {
        let admins = egret_auth::roles::engine_admin_user_ids_via(conn)
            .await
            .map_err(crate::db_error::from_core)?;
        let ids: Vec<Uuid> = admins.iter().map(|u| u.0).collect();
        let named = egret_auth::users::usernames_via(conn, &ids)
            .await
            .map_err(crate::db_error::from_core)?;
        unnamed.extend(ids.into_iter().filter(|id| !named.contains_key(id)));
    }

    for &user_id in &unnamed {
        let minted = mint_for_compile(conn, user_id).await?;
        tracing::info!(
            %user_id,
            username = %minted,
            "minted a username for a person the model names, so the compiled policy can name them"
        );
    }
    Ok(unnamed.len())
}

/// The engine admins' usernames, sorted and deduped: the users `egret-admin`
/// names. A missing one is minted under [`lock_compile_mints_in_tx`] (the
/// pre-mint sweep normally got there first).
pub(crate) async fn engine_admin_usernames_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<String>, CompileError> {
    let admins = egret_auth::roles::engine_admin_user_ids_via(conn)
        .await
        .map_err(crate::db_error::from_core)?;
    let ids: Vec<Uuid> = admins.iter().map(|u| u.0).collect();
    let named = egret_auth::users::usernames_via(conn, &ids)
        .await
        .map_err(crate::db_error::from_core)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match named.get(&id) {
            Some(u) => out.push(u.clone()),
            None => out.push(mint_for_compile(conn, id).await?),
        }
    }
    out.sort();
    out.dedup();
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
    /// The grant's scope (`platform`, `workspace`, `object`) and scope id.
    /// The compiler ignores both (a role is scope-free on engines,
    /// [`role_usernames`]); People and the clearance read report them.
    pub(crate) scope_type: String,
    pub(crate) scope_id: Option<Uuid>,
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
            u.username::text AS username, ag.scope_type, ag.scope_id \
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
    .map_err(CompileError::from_db)
}

/// The members of each group that holds a role in `roles` (a live `allow`
/// grant whose principal is a group), one row per member. A role granted to a
/// group reaches its members on every engine; the PDP already expands it
/// (`egret_auth::grants`), so the compiled role membership must as well. The
/// same member may also hold the role directly, so callers dedup names.
pub(crate) async fn role_group_holders_via(
    conn: &mut sqlx::PgConnection,
    roles: &[String],
) -> Result<Vec<RoleHolderRow>, CompileError> {
    if roles.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT r.name AS role_name, ug.user_id, \
            u.username::text AS username, ag.scope_type, ag.scope_id \
           FROM access_grants ag \
           JOIN roles r ON r.id = ag.role_id \
           JOIN user_groups ug ON ug.group_id = ag.principal_id \
           JOIN users u ON u.id = ug.user_id \
          WHERE ag.principal_type = 'group' \
            AND r.name = ANY($1) \
            AND ag.effect = 'allow' \
            AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(roles)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)
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
    // The direct members, and the members of a group that holds the workspace
    // (the group arm: the PDP expands it, so the compiled list must too).
    sqlx::query_as(
        "SELECT ag.scope_id AS workspace_id, ag.principal_id AS user_id, \
            u.username::text AS username \
       FROM access_grants ag \
       JOIN users u ON u.id = ag.principal_id \
      WHERE ag.principal_type = 'user' \
        AND ag.scope_type = 'workspace' \
        AND ag.scope_id = ANY($1) \
        AND ag.effect = 'allow' \
        AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
     UNION \
     SELECT ag.scope_id AS workspace_id, ug.user_id, u.username::text AS username \
       FROM access_grants ag \
       JOIN user_groups ug ON ug.group_id = ag.principal_id \
       JOIN users u ON u.id = ug.user_id \
      WHERE ag.principal_type = 'group' \
        AND ag.scope_type = 'workspace' \
        AND ag.scope_id = ANY($1) \
        AND ag.effect = 'allow' \
        AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)
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
    .map_err(CompileError::from_db)
}

/// What a model references that has to be enumerated: workspace ids, role
/// names and (since J-F3 Task 1b) group names.
#[derive(Default)]
struct Referenced {
    workspaces: BTreeSet<Uuid>,
    roles: BTreeSet<String>,
    groups: BTreeSet<String>,
}

/// Every workspace id a `workspace_members` principal names, every role NAME a
/// `role` principal names, and every group NAME a non-`public` `group`
/// principal names, across `grants` and `rules`. A workspace ref that does not
/// parse as a uuid is NOT collected — see [`expand_enumerated_principals`] for
/// why that is the same answer the evaluator gives.
fn referenced_principals(
    grants: &[Grant],
    rules: &[Rule],
    engine_grants: &[EngineGrant],
) -> Referenced {
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
    for g in engine_grants {
        scan(&g.principals);
    }
    out
}

/// Read who every `workspace_members`, every `role` **and every non-`public`
/// `group`** principal in `grants`, `rules` and `engine_grants` stands for, from
/// the database on `conn`, as the [`Directory`] each family expands its
/// audience against (`egret_perm_ir::audience::audience`). The model itself
/// is not rewritten (governance sub-project 1, Task 10).
///
/// Runs ONE query for every workspace referenced by the whole model and ONE for
/// every group (minting a username for any member that has none — module doc),
/// and ONE for every role name ([`role_usernames`], which mints the same way
/// since the 2026-10-18 design R5). Expanded against the directory, no
/// `PrincipalKind::WorkspaceMembers`, no `PrincipalKind::Role` and no group
/// other than `public` survives in an audience — which is what lets
/// `compile_grant`/`compile_rule` refuse the first two kinds outright as a
/// defence in depth (a group is not refused there; `egret_perm_ir::lower::grants`' module doc says
/// why): reaching them with an unexpanded principal is a bug in THIS function,
/// not an authoring mistake, and a refusal says so loudly instead of emitting a
/// policy that silently grants nobody (an `include` that fails closed) or
/// silently grants everybody the subtract was meant to remove (an `exclude`
/// that fails OPEN — the reason dropping was never an option; see `egret_perm_ir::lower::grants`'
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
/// did not reach. Under `crate::trigger::recompile_services_in_tx` neither
/// pass mints anyone: [`premint_model_usernames`] already has, in one
/// ascending sweep across every pass.
///
/// **A ref that does not parse as a uuid expands to nothing**, rather than
/// erroring. That is exactly what the evaluator does with it
/// (`egret_perm::eval::matches_principal` → `parse::<Uuid>().map(...).
/// unwrap_or(false)`), and requirement 3 of this work is that the oracle and
/// the compiler answer identically for the same model. It is logged.
pub(crate) async fn expand_enumerated_principals(
    conn: &mut sqlx::PgConnection,
    grants: &[Grant],
    rules: &[Rule],
    engine_grants: &[EngineGrant],
) -> Result<Directory, CompileError> {
    let Referenced {
        workspaces: wanted,
        roles: wanted_roles,
        groups: wanted_groups,
    } = referenced_principals(grants, rules, engine_grants);
    if wanted.is_empty() && wanted_roles.is_empty() && wanted_groups.is_empty() {
        return Ok(Directory::default());
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
        let minted = mint_for_compile(conn, user_id).await?;
        tracing::info!(
            %user_id,
            username = %minted,
            "minted a username for a workspace or group member so the compiled policy can \
             name them"
        );
        resolved.insert(user_id, minted);
    }

    let mut members: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
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
    // directory, so the audience knows it expanded to zero rather than that it was
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

    Ok(Directory {
        workspaces: members,
        roles: role_members,
        groups: group_members,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::Principal;

    fn ws_principal(id: Uuid) -> Principal {
        Principal {
            kind: PrincipalKind::WorkspaceMembers,
            r#ref: id.to_string(),
        }
    }

    fn group(name: &str) -> Principal {
        Principal {
            kind: PrincipalKind::Group,
            r#ref: name.to_string(),
        }
    }

    fn read_grant(principals: PrincipalSet) -> Grant {
        Grant {
            id: Uuid::nil(),
            effect: egret_perm::Effect::Allow,
            actions: vec![egret_perm::Action::Read],
            scope: egret_perm::Scope {
                catalog_id: Uuid::nil(),
                schema: None,
                table: None,
                column: None,
            },
            principals,
            origin: egret_perm::GrantOrigin::Explicit,
        }
    }

    /// `public` is the one group every engine resolves; it is not even looked
    /// up. (That it is never expanded is `egret_perm_ir::audience`'s
    /// `the_public_group_is_never_expanded`.)
    #[test]
    fn the_public_group_is_never_looked_up() {
        let grant = read_grant(PrincipalSet {
            include: vec![group(PUBLIC_GROUP), group("analysts")],
            exclude: vec![],
        });
        assert_eq!(
            referenced_principals(&[grant], &[], &[])
                .groups
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["analysts".to_string()],
            "`public` is not even looked up"
        );
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
        let found = referenced_principals(&[read_grant(set)], &[], &[]).workspaces;
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![ws]);
    }
}
