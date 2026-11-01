# P6 — compile-mint ordering follow-ups (#790, #791, #792)

Status: **spec, waiting for the owner** (phase 1; nothing implemented). Branch `fix/compile-mint-ordering`,
from `origin/main` at `33f4c83d`. Background: username-takeover spec 2026-10-23 §11 items 4–6, and the doc
comments of `lock_compile_mints_in_tx` / `premint_model_usernames` (`crates/egret-perm-compile/src/members.rs`).

Words used below. **Profile lock**: `SELECT … FROM policy.authz_services WHERE id = $1 FOR UPDATE`.
**M**: the compile-mint lock, `pg_advisory_xact_lock(hashtextextended('egret-compile-mint', 0))`. **Early
locker**: a writer that takes its profile locks and then M *before* its own row write (catalog-tag write and
table-hook move through `ViewReach`; clearance `set_policy`; tag delete; group-membership port; user hard
delete; `egret-admin` status sync). **Stated order**: view-tag / group / member advisory locks → profile locks
ascending → M → user rows and other row writes.

## 1. #792 — the profile set grows after the early mint lock (owner: option (a), retryable 409)

### Premise: reproduces

An early locker reads its profile set twice: once before M, once at the recompile. The second read can be
wider, and the extra profiles are then locked while M is held. Re-read sites on `33f4c83d`:

| Writer | Early set | Late re-read |
|---|---|---|
| catalog-tag write, table-hook move (`ViewReach`, `view_tags.rs`) | `services_bound_to_warehouses(reach.warehouses)` | `retag_bump_and_recompile`: same query over `touched`, then `bump_tag_versions_for_warehouses` (its own `FOR UPDATE` per id) and `recompile_bound_services_in_tx` |
| clearance `set_policy`, tag delete (`egret-bi/src/tags.rs`) | `affected_services_for_classification` (∪ bound profiles of the views' warehouses) | `recompile_for_clearance_in_tx` reads `affected_services_for_classification` again; `TagViewReach::retag_and_bump` bumps `touched` |
| every early locker | `lock_services_in_tx` expands with tag services by name | `recompile_services_in_tx` / `recompile_service_in_tx` re-expand; `apply_diff_in_tx` locks `Compiled::sweep` (profile + tag service resolved by name) |

A binding (`engine_catalog_attachments`, `engines.authz_service_id`), a new engine on a trino/starrocks
profile, or a newly created service whose name a locked profile's `tagService` already names, committed
between the two reads, grows the set. None of those writes takes a profile lock.

**Red test** `crates/egret-perm-compile/tests/compile_mint_ordering.rs` (committed on the branch). No seam was
needed: `ViewReach::of_tables` and `retag_bump_and_recompile` are public and the test runs them on its own
transaction. A locks S1 then M; B attaches S2's engine to W's catalog and commits; C takes S2's profile lock
then waits on M (polled in `pg_locks`); A's re-read finds S2 and locks it. Result on `33f4c83d`: see §5.

### Design

One guard at the profile-lock chokepoint of `egret-perm-compile`, not at each re-read site:

1. `lock_compile_mints_in_tx` also sets a transaction-local marker in the same statement:
   `SELECT pg_advisory_xact_lock(…), set_config('egret.compile_mint_held', 'on', true)`.
2. A new private `lock_profile_rows_in_tx(tx, ids)` is the only place `egret-perm-compile` takes a profile
   lock. It reads `current_setting('egret.compile_mint_held', true)` and `current_setting('egret.profile_locks',
   true)` (a transaction-local, comma-separated set of the ids this transaction has locked). If M is held and
   `ids` contains an id not in that set, it returns `CompileError::ProfileSetChanged { added }` **before any
   lock statement**. Otherwise it calls `egret_policy::lock_service_rows_via` and appends `ids` to the set.
3. `lock_services_in_tx` and `apply_diff_in_tx`'s per-sweep lock call it (they lock today with their own
   `FOR UPDATE`).
4. The two bumps that run after M (`ViewReach::retag_bump_and_recompile`, `TagViewReach::retag_and_bump`) call
   `lock_services_in_tx` over the bound profiles of `touched` **before** `bump_tag_versions_for_warehouses`, so
   the bump's own `FOR UPDATE`s are re-takes. (`egret-catalog-registry` does not depend on `egret-policy` or
   `egret-perm-compile`, so the bump cannot carry the guard itself.)

A savepoint rollback reverts both settings together with the row locks taken inside it, so the set never
claims a lock the transaction lost. A transaction that never takes M is unchanged: the guard is inert.

### Why the lock order still holds

1. Every lock a transaction takes in these paths belongs to one class: (a) view-tag / group / member advisory,
   (b) profile rows, (c) M, (d) user rows and other row writes. Claim: no transaction *waits* on a lock of a
   lower class than one it holds, and profile rows are waited on in ascending id order. Then no wait-for cycle
   exists, because a cycle needs some edge from a higher class to a lower one, or a descending edge in (b).
2. (a) → (b) → (c): unchanged and already documented ("nothing asks for a profile lock while holding it, because
   every recompile locks its whole profile set first"). The gap is that this sentence is false when the set
   grows: the late lock is an edge (c) → (b).
3. With the guard, a profile lock requested while M is held is either a re-take of an id in
   `egret.profile_locks` or a refusal. A re-take of a row this transaction already holds `FOR UPDATE` is granted
   at once (no wait), so it adds no edge. A refusal returns before any statement, so it adds no edge, and the
   rollback that follows releases everything.
4. Every profile-lock site that can run after M goes through the guard: `lock_services_in_tx` and
   `apply_diff_in_tx` (guarded directly); `bump_tag_versions_for_warehouses` (only re-takes, by step 4 of the
   design); the `policy_version` / `tag_version` `UPDATE`s in `trigger.rs`, `default_policies.rs` and
   `tag_version.rs` (only on a profile this transaction already locked). The `egret-policy` editor locks
   (`admin/mod.rs`) never run after M: the editor takes no M early, and its recompile goes through
   `recompile_services_in_tx`, which locks before its pre-mint. The implementation re-checks this list with a
   grep of `authz_services` + `FOR UPDATE` / `UPDATE policy.authz_services` and records it in this file.
5. (c) → (d): unchanged. M is only waited on by transactions holding (a)/(b) locks, which never hold (d) locks
   at that point (the early-locker rule in `lock_compile_mints_in_tx`'s doc).

So the order is restored without NOWAIT: the late lock is never requested, it is refused.

### User-visible effect

`CompileError::ProfileSetChanged { added: Vec<i64> }`, with `retryable_body()` beside
`expansion_ceiling_body()` in `egret-perm`: `409 {"error": "the engines this write recompiles changed while it
ran (profile N was bound); nothing was written, retry", "code": "profile_set_changed", "retryable": true}`.

| Write | Route / caller | Answer |
|---|---|---|
| catalog-tag assign / unassign | `egret-bi` `catalog_tags.rs` (`reach_error` → `From<CompileError> for BiError`) | 409 coded (today such errors become `BiError::Engine`, a 502) |
| clearance set, tag delete | `egret-bi` `tags.rs` | 409 coded |
| table move (rename) | `egret-catalog-engine` `table_hook.rs` `reach_refusal` | the existing `busy()` refusal ("…retry") |
| group-member write, user hard delete, OIDC group sync, user status | `group_membership::to_core`, `role_membership.rs` | stays `Internal` (500, logged). These have no re-read site; only the tag-service re-expansion can grow their set (a service created under a name a locked profile already names), which needs an admin creating a tag service at the same instant. `to_core` must not become `Conflict` (the OIDC callback reads `Conflict` as "email already used"). |
| policy editor, `/perm` routes | `CpError::from(CompileError)` | 409 coded, same body (rare: same tag-service case) |

When it happens: only when an engine is bound or attached, or a profile/tag service is created, between the two
reads of one write: an admin action racing a steward's write. No client retries today: the generated React
Query client sets `retry: 1` for queries and nothing for mutations, and no `egret-bi` / `egret-catalog-engine`
caller retries a 409. Proposal: no automatic retry anywhere. The UI already shows a refusal's `error`
sentence; it says "retry", and a second click succeeds because the set is now stable.

### Test plan

- The red test above turns green: A gets `ProfileSetChanged` (no 40P01), C commits.
- Same interleaving for `set_policy` (a new engine on a starrocks profile between the two
  `affected_services_for_classification` reads), through `ClassificationPolicyRepo`, in `egret-bi` tests.
- Tag-service growth: a profile whose `tagService` names a missing service; the service is created between
  `lock_services_in_tx` and `recompile_services_in_tx` → refusal.
- Guard unit tests: re-take under M passes; new id under M refuses without waiting (another session holds the
  row; the call returns in under `deadlock_timeout`); no M → new id locks as today.
- HTTP: catalog-tag route answers 409 `profile_set_changed`; table hook answers its busy refusal.

Size: **M** (~250 lines + tests; 4 crates: `egret-perm`, `egret-perm-compile`, `egret-bi`,
`egret-catalog-engine`). Does not touch `egret-auth`.

## 2. #790 — pre-mint driven by the referenced sets

### Premise: reproduces as described

`premint_model_usernames` reads refs from the whole model (`grant_principals`, `rule_exemptions`,
`engine_grant_principals`, every clearance's `required_roles`, the engine admins when any `egret-admin` set
exists), whatever profiles the transaction recompiles.

### Does narrowing keep the superset invariant? Not provably — and the invariant is no longer what keeps it safe

To be a superset of each pass's mints, a narrowed sweep must restate, per locked profile, which refs each pass
reads: grants through `services_for_catalog`, rules and exemptions through tag reach, engine grants of the
bound engine, every clearance on every compiler-targeted profile, the admins on `egret-admin` holders. That is
the copy of the passes' selection the current sweep was written to avoid ("no membership predicate is restated
here"); the F11 engine-grant arm is the example of a pass added later. And even today the sweep is not a
superset under READ COMMITTED: a ref committed between the sweep and a pass is minted by the pass. The doc
comment says so, and that is why M exists.

What keeps compile mints deadlock-free is M, not the sweep: every compile mint takes M first
(`mint_for_compile`) and holds it to commit, so two compiles never mint at the same time, in any order. The sweep
only keeps M to one acquisition and the `users` row locks ascending within one transaction. A narrowed sweep
would therefore be safe, but its gain is small: an unnamed person named only by another profile is minted now
instead of at that profile's next compile (once; afterwards nobody is unnamed), and a recompile takes M a bit
less often during that transient.

**Proposal: do not narrow.** Close #790 with this reasoning, and fix the doc comments, which are now partly
wrong: `premint_model_usernames` says the set is a "SUBSET of what the passes mint, never wider" (it is a subset
of what *some* compile of the current model mints, and is meant to cover this transaction's passes), and the
issue text attributes deadlock-freedom to the superset. Size: **XS** (comments only).

## 3. #791 — skip the pre-mint when nothing can mint

### Premise: reproduces, narrow

`recompile_service_in_tx` runs the sweep for any id; `recompile_services_in_tx` for any non-empty set.
`compile_desired` returns `None` for any type other than `trino` / `starrocks`. Production reaches that with a
non-targeted id only through `POST /perm/compile/{id}` on a tag service, or a service holding an `egret-admin`
set that is not trino/starrocks (none exists today). Before `None`, `sync_admin_policy_in_tx` runs and can
mint the engine admins (`engine_admin_usernames_via` → `mint_for_compile`).

### Design (with the egret-admin sync guard)

Skip the sweep when **nothing in this recompile can mint**: no id in the set has an `engine_type_for` type,
and no id is in `admin_policy_services_via`. One read of the set's types (a `type = ANY` query) plus the admin
list the sweep already reads. A service with an `egret-admin` set keeps the sweep, so its admin sync finds the
names already minted and the mint order is unchanged.

Order argument: skipping a sweep removes lock requests, it adds none. If an admin sync still mints (a guard
bug, or a concurrent role grant), it mints under M after the profile locks, the stated order. Safe either way.

Value is low (a manual compile of a tag service no longer takes M when someone is unnamed). Proposal: **do it**,
because it is small and its guard is the same question #792's guard asks ("can this transaction mint?"). Size:
**S** (~30 lines + 2 tests: tag-service compile with an unnamed grantee takes no M, checked in `pg_locks` from a
second session; a service with an `egret-admin` set still pre-mints).

## 4. Decisions for the owner

1. **#792 body and status**: 409 `{"error", "code": "profile_set_changed", "retryable": true}`, no client
   auto-retry, the UI shows the sentence. *Recommended.* (Alternative: 503 + `Retry-After`.)
2. **#792 on the group/login path**: keep 500 there (only a concurrent tag-service creation can trigger it).
   *Recommended*; the alternative is a server-side single retry of the login transaction, which touches
   `egret-auth` (P5b's area).
3. **#790: close as won't-do**, doc comments corrected. *Recommended.*
4. **#791: implement** with the egret-admin guard. *Recommended*; closing it is also defensible.

## 5. Red-test result on `33f4c83d`

**Reproduces, deterministically (3 of 3 runs, ~1.8 s each).** C, which kept the stated order, is aborted with
`deadlock detected` (40P01) about one `deadlock_timeout` after A requests S2; A then recompiles S2 and commits.
In production that is a 500 on whichever write Postgres picks (a grant write, a login's group sync, another tag
write). Run with:
`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-perm-compile --test compile_mint_ordering`.
