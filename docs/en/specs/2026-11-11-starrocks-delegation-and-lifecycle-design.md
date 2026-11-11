# StarRocks delegation and engine lifecycle (package D): design

**Status:** the user ruled on D1a, D1b, D2 and D3 on 2026-11-11 (section "Decided by the user").
The four questions this design raised were decided by the user on 2026-11-11 as well (section
"Decided on 2026-11-11"); the design below includes the answers.

**Package:** D of the StarRocks follow-ups. It is based on package A's branch
(`feat/starrocks-query-log`, platform PR #849, migration 0218), which is not merged yet. D takes
migration **0219**.

**Root causes:** `.superpowers/sdd/package-d-root-causes.md` (file:line evidence for every claim in
"Problem"). The three defects were found in package A's live check on kind (task 7, defects 5 to 7).

## Problem

1. **An impersonate-mode StarRocks engine under Ranger cannot run a query through egret.** Two
   independent gaps:
   - **1a.** Nothing grants `egret_delegator` the `impersonate` access in Ranger. With
     `access_control = ranger`, StarRocks checks `EXECUTE AS` against Ranger
     (`RangerStarRocksAccessController.checkUserAction`, resource `user=<target>`, access
     `impersonate`) and never reads the native `GRANT IMPERSONATE ON ALL USERS` egret issues.
     egret's IR emits `DelegatorImpersonate` for Trino only
     (`egret_core::engine_delegation::delegator_impersonates`, `lower_infra`'s early return), and
     its body is Trino-shaped (`trinouser`). Live: `ERROR 5204 … IMPERSONATE privilege(s) on USER
     member-sso`.
   - **1b.** The target user must exist. `AuthenticationAnalyzer.visitExecuteAsStatement` calls
     `checkUserExist` before any authorizer, so this holds with or without Ranger. egret creates no
     StarRocks user for a person. Live: `ERROR 1064 … cannot find user 'member-sso'@'%'`.

   Broken since the feature shipped (#263, 2026-07-16). JWT mode is not affected.
2. **The CN crash-loops once the initpwd Job has set root's password.** The CN entrypoint registers
   itself with `mysql -u root`, which reads the password only from `MYSQL_PWD`.
   `starrocks_cluster_cr` puts `MYSQL_PWD` in neither `feEnvVars` nor `cnEnvVars`. The helm chart
   egret copied the initpwd script from injects it into FE, BE and CN. Known as KI-335. Since #847,
   **Upgrade security** on a legacy engine mints a root password and restarts the pods, so its CN is
   guaranteed to crash-loop.
3. **Deleting an engine leaves its initpwd Job pods and its FE meta PVCs.**
   - `StarRocksProvisioner::delete` deletes the Job through `delete_objects`, which uses
     `DeleteParams::default()`. A `batch/v1` Job's default propagation is orphan, so its pods stay
     (measured on kind 1.33).
   - The `fe-meta` volume becomes StatefulSet `volumeClaimTemplates`, and egret neither sets a
     retention policy nor deletes the claims. A new engine with the same name rebinds
     `fe-meta-<name>-fe-0`. Its FE boots with the old users, catalogs and root password, and the new
     initpwd Job's 1045 counts as success, so egret's freshly minted root secret does not match and
     every bootstrap login fails.

## Decided by the user (2026-11-11)

- **D1a: users are synced eagerly.**
  - egret creates the StarRocks user for each egret user who may query an impersonate-mode StarRocks
    engine, with a random, unusable password.
  - Sync happens when the engine is assigned to a workspace and when membership changes, and is
    reconciled on every apply and at boot.
  - egret DROPs the StarRocks user when the egret user no longer has access: they leave every
    workspace the engine serves, or the engine is unassigned.
  - Root is never put into the query path. The sync runs from engine-mgmt, which holds root.
- **D1b: the Ranger impersonate row.** For StarRocks in impersonate mode egret emits a
  `DelegatorImpersonate` infra row that lets `egret_delegator` `impersonate` every user except
  `root`, `egret_delegator` and `egret_query_log`, with Ranger semantics the fork's plugin honours.
- **D2: `MYSQL_PWD`.** When the engine has a root password, inject `MYSQL_PWD` from
  `<name>-initpwd-secret` into both the FE and the CN env, after `apply_pod_fields`, without
  overriding an admin-set `MYSQL_PWD`.
- **D3: delete cleanup.**
  - Delete the initpwd Job with Background propagation, reusing `delete_background`.
  - Set `persistentVolumeClaimRetentionPolicy: {whenDeleted: Delete}` on the FE spec when fe-meta is
    persistent.
  - Also delete the `fe-meta-<name>-fe-<i>` PVCs explicitly on engine delete, for engines that were
    never re-applied.
  - Document that a deleted engine's FE metadata is gone; table data stays in object storage.

## Facts this builds on

Read from source (the root-cause report and this design's own reading):

1. **The name `EXECUTE AS` sends is `users.username`.** `egret_query::execution::delegation_for`
   (`crates/egret-query/src/execution/mod.rs`) resolves an impersonate-mode connection's
   `session_user` with `UserRepo::ensure_username`, and `StarRocksConnector::switch_identity` sends
   ``EXECUTE AS `<session_user>` WITH NO REVERT``. StarRocks resolves the bare name to
   `'<name>'@'%'`. So the StarRocks user egret creates is exactly `'<users.username>'@'%'`.
2. **A username is minted once and never changes in egret.** `ensure_username_via` sets
   `users.username` only where it is NULL. No route renames it. Migration 0209's triggers record
   every name a `users` row takes in `username_ledger`, and a name is never minted again after its
   holder is deleted or renamed. Usernames are lowercase slugs of `[a-z0-9_-]` with an optional
   `-<n>` suffix.
3. **Who may query a workspace connection** is the workspace's members: live `allow` workspace
   grants held directly or through a group (`egret_perm_compile::members::workspace_members_via`,
   `egret_workspace::members::MemberRepo`). A platform admin also passes the workspace gate without
   being a member (`egret_workspace::context`, the `platform_admin` bypass), so platform admins are
   synced too (decided on 2026-11-11, 1). Who is a platform admin is one query,
   `egret_auth::roles::engine_admin_user_ids_via`: active users holding a live `allow` grant in
   `access_grants` (`scope_type = 'platform'`) of the role named `platform_admin` in `roles`,
   directly or through a group. `platform_role_assignments` is legacy (migration 0068 backfilled
   `access_grants` from it); no code in `crates/` reads or writes it.
4. **Membership is written in several crates** (`egret-workspace` members, `egret-auth` groups and
   user delete and status, the access-request approval, the hourly expiry sweep), none of which may
   depend on `egret-engine-mgmt`. Engine assignment is written by
   `EngineAssignmentService::{assign, unassign}` (`crates/egret-engine-mgmt/src/assignment.rs`) from
   three routes.
5. **StarRocks user names:** `FeNameFormat.checkUserName` accepts
   `^\w{1,64}/?[.\w-]{0,63}$` and at most 64 characters
   (`starrocks-egret-417/fe/fe-core/src/main/java/com/starrocks/sql/analyzer/FeNameFormat.java:55,147-151`).
   A name cannot contain `'` (measured in package A).
6. **The `user` resource** in egret's StarRocks service def
   (`crates/egret-policy/src/service-defs/starrocks.json:303-327`) has `excludesSupported: true`,
   `matcherOptions {wildCard: true, ignoreCase: true}` and `accessTypeRestrictions:
   ["impersonate"]`. The def has no `options` block.
7. **Ranger evaluates `isExcludes` in the plugin** whatever `excludesSupported` says
   (`RangerAbstractResourceMatcher.applyExcludes`; `RangerResourceTrie` keeps an excludes resource as
   an inherited evaluator). Only the Admin-side validator reads `excludesSupported`, and egret is the
   Admin. Read on Ranger master; the fork's FE pom pins `ranger.version` **2.9.0**
   (`starrocks-egret-417/fe/pom.xml:59`). The fork's `checkUserAction` passes the bare target name
   in resource `user`, and its root bypass (`RangerAccessController.java:161-164`) applies only when
   the **requester** is root, so `EXECUTE AS root` run by `egret_delegator` is checked by Ranger.
8. **The operator honours `persistentVolumeClaimRetentionPolicy`** on the FE spec
   (`starrocks-kubernetes-operator/pkg/apis/starrocks/v1/component_type.go:121-125`, applied in
   `pkg/k8sutils/templates/statefulset/spec.go:97-103` with `whenScaled` empty). The field is
   mutable on a live StatefulSet. The claim name is `fe-meta-<name>-fe-<ordinal>`.
9. **The NOTIFY-at-commit pattern exists:** `egret_perm_compile::view_tags::notify_classification_in_tx`
   and `egret_catalog_engine::spawn_classification_sweeps` (a `PgListener` loop plus a scheduled
   sweep for notifications lost while it reconnects).
10. **Native StarRocks grammar** (`fe/fe-grammar/src/main/antlr/com/starrocks/grammar/StarRocks.g4:1893,1907,1915`):
   `GRANT IMPERSONATE ON USER <user> TO [USER] <user>`, `REVOKE IMPERSONATE ON USER <user> FROM …`,
   and `REVOKE <privs> ON ALL <plural> FROM …` (so `REVOKE IMPERSONATE ON ALL USERS FROM USER …`).
   `SHOW GRANTS FOR <user>` answers `UserIdentity`, `Catalog`, `Grants`
   (`ShowResultMetaFactory.java:1163-1168`). Today the bootstrap issues
   `GRANT IMPERSONATE ON ALL USERS TO egret_delegator` on every delegating engine, bound or not
   (`engines.rs` `starrocks_impersonation_grant_sql`, pushed in `spawn_starrocks_catalog_wire`).

**Not measured yet** (Task 1 of the plan measures each before code depends on it):

- M1. The fork's plugin (Ranger 2.9.0) denies `EXECUTE AS` to `root`, `ROOT`, `egret_query_log` and
  `egret_delegator` and allows it to another user, under the D1b row with `isExcludes: true`.
- M2. What `SHOW USERS` answers on `4.1.7-egret.4`: its column, and whether a value reads
  `'alice'@'%'`.
- M3. The FE's answer to `CREATE USER` for a user that exists, and to `DROP USER` for one that does
  not.
- M4. On kind: a CN that starts with `MYSQL_PWD` before the initpwd Job has set the password is
  refused, restarts, and joins once the Job has run (the helm chart's behaviour).
- M6. On native access control: what `SHOW GRANTS FOR 'egret_delegator'@'%'` answers, and how
  `IMPERSONATE ON ALL USERS` and `IMPERSONATE ON USER '<u>'@'%'` read in its `Grants` column.
- M7. On native access control: whether `REVOKE IMPERSONATE ON ALL USERS` leaves per-user grants in
  place; what it answers when nothing is granted; what a repeated per-user `GRANT` answers. The
  sync's order (revoke, re-read, grant) is correct whatever M7 shows; M7 pins the fake's answers.
- M5. On kind: after a delete, the claims go, and a same-name re-create gets a fresh FE with egret's
  root password.

## Design

### 1. The Ranger impersonate row for StarRocks (D1b)

**One list of reserved StarRocks users**, in `egret_core::engine_principals`, used by both this row
and the user sync (section 2):

```rust
pub const STARROCKS_ROOT_USER: &str = "root";
/// Sorted, so a rendered body is byte-stable.
pub const STARROCKS_RESERVED_USERS: [&str; 3] =
    [DELEGATOR_PRINCIPAL, QUERY_LOG_LOADER_PRINCIPAL, STARROCKS_ROOT_USER];
pub fn is_starrocks_reserved_user(name: &str) -> bool; // ASCII case-insensitive
```

**`delegator_impersonates`** widens to "a Trino engine in `impersonate` mode with auth, or a
StarRocks engine in `impersonate` mode". That is what it documents ("applying this engine
provisions the `egret_delegator` service user and its impersonation grant"): the StarRocks
bootstrap creates `egret_delegator` in impersonate mode. Its only non-test reader is
`load_infra_inputs`.

**IR.** A new object, so the IR says what the row means rather than leaving it to a renderer:

```rust
pub enum InfraObject { /* … */ AnyUserExcept(Vec<String>) }
```

`lower_infra` emits, for a StarRocks profile that is delegating and whose engine impersonates with
the delegator, after the query-log statements and before the Trino early return:

- `DelegatorImpersonate`, audience user `egret_delegator`, capability `impersonate`, target
  `AnyUserExcept(STARROCKS_RESERVED_USERS)`.

`SelfImpersonate`, `ExecuteQuery` and `QueryTimeout` stay Trino-only. Trino output is unchanged byte
for byte (`AnyUser`, the `trinouser` body).

**Rendering** (`ranger::infra::render`): the `DelegatorImpersonate` arm matches the object and the
engine together. `(AnyUser, Trino)` renders today's body; `(AnyUserExcept(list), StarRocks)` renders
`egret_policy_api::infra_bodies::starrocks_delegator_impersonate_policy(service, &list)`; any other
pairing, and an empty list, is a `CompileError` (an empty `isExcludes` list would read as "excludes
nothing").

```json
{
  "name": "egret-delegator-impersonate", "service": "<svc>", "serviceType": "starrocks",
  "policyType": 0, "isEnabled": true,
  "resources": { "user": { "values": ["egret_delegator", "egret_query_log", "root"],
                           "isExcludes": true, "isRecursive": false } },
  "policyItems": [{ "users": ["egret_delegator"], "groups": [], "roles": [],
                    "accesses": [{ "type": "impersonate", "isAllowed": true }],
                    "delegateAdmin": false }],
  "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": [],
  "isDenyAllElse": false, "policyPriority": 0
}
```

**Why `isExcludes` and not a deny row.** Facts 6 and 7: the def allows it, the plugin evaluates it,
and `ignoreCase` makes `ROOT` excluded too. It is one row with a closed list, under the name
`egret-delegator-impersonate` that `infra_names` already reserves on every profile type, so
convergence adopts, rewrites and removes it like the Trino row. A deny row would be a second row and
a second reserved name, and an allow item plus a deny item in one policy cannot work (both share the
policy's resource). M1 measures the behaviour on the fork's real plugin before anything depends on
it. If M1 shows the plugin ignores the flag, the plan stops and the user is told; the fallback is a
separate deny row on `user = [the three]`.

**Who else may impersonate.** Unchanged: the `egret-admin - user` row gives engine admins every
access, `impersonate` included, on every user. They are people egret made engine admins.

**OPA evaluator.** `egret_perm_eval::Snapshot::decide`'s impersonation arm gains
`InfraObject::AnyUserExcept(ex) => !ex.iter().any(|e| eq_ci(e, target))`. No StarRocks profile is on
OPA today; the arm keeps the evaluator truthful for the IR it is given.

**Blast radius.** Every StarRocks impersonate-mode profile gains one row on its next convergence
(boot repair, any recompile). Convergence removes it on impersonate → jwt. The golden corpus changes
where a fixture binds a StarRocks impersonate engine (G22 at least).

### 2. StarRocks users for the people who may query (D1a)

#### 2.1 What is synced, on which engines

- **Engines:** provisioned (not external) StarRocks engines that are not `deleting` or `failed`,
  whose delegation mode is `impersonate` or `jwt` (both bootstraps create `egret_delegator`, so
  both need 2.7's revoke), or on which egret has users recorded. The **desired set** is empty unless
  the engine is in `impersonate` mode.
- **Desired users:** the egret users who are active (`users.status = 'active'`) and hold a live
  `allow` workspace grant, directly or through a group, on a workspace the engine is assigned to
  (fact 3).
- **Platform admins** (decided on 2026-11-11, 1): every user `engine_admin_user_ids_via` returns
  (fact 3), when the engine is assigned to at least one workspace. An engine assigned nowhere has no
  connection to query through, so it gets none.
- A user with no username yet is minted with `ensure_username_via`, as the compiler does, so their
  StarRocks user exists before their first query.
- **A disabled user** has no access, so their StarRocks user is dropped, and re-created on
  re-enable.
- **The StarRocks name** is the username verbatim (fact 1).

#### 2.2 What egret records

`engine_mgmt.starrocks_users (engine_id, username, created_at)`, primary key
`(engine_id, username)`, `ON DELETE CASCADE` from the engine (migration 0219). A row means "egret
created this StarRocks user on this engine". egret drops only users it recorded. It never touches a
StarRocks user it did not create.

#### 2.3 One reconcile, idempotent

`starrocks_users::sync_starrocks_users(st, engine, fe) -> UserSync`, as `root` through
`st.starrocks_ddl` and `fe_target`:

1. **Lock:** a session advisory lock `pg_try_advisory_lock(hashtextextended('egret-starrocks-users:'
   || engine_id, 0))` on its own pooled connection (`close_on_drop`, so a dropped guard ends the
   session and with it the lock). Not taken: `Busy`, nothing done. Every egret replica listens and
   retries on its own next notification or sweep.
2. **Read** the desired set (2.1), the recorded set (2.2), and the FE's users (`SHOW USERS`, each
   value read with `query_log::authorized_name` when it has the `'<name>'@'<host>'` form, as is).
3. **Plan** (a pure function, `plan_users(desired, recorded, on_fe) -> UserPlan`), every name in
   sorted order:

   | Desired | Recorded | On the FE | Action |
   |---|---|---|---|
   | reserved (`is_starrocks_reserved_user`) | – | – | **refused** "reserved", nothing done |
   | not a valid StarRocks name (fact 5) | – | – | **refused** "not a valid StarRocks user name" |
   | yes | yes | yes | nothing |
   | yes | no | yes | **foreign**: someone else's user; left alone, logged |
   | yes | either | no | **create** (record first, then `CREATE USER`) |
   | no | yes | yes | **drop** (`DROP USER`, then forget the record) |
   | no | yes | no | **forget** the record |

   "Recorded, desired, absent from the FE" is an FE that lost its metadata (an engine without a
   persistent fe-meta whose FE pod was replaced); it is created again. The plan also lists **kept**
   (desired, recorded, on the FE), which 2.7 needs.
4. **Execute:**
   - create: insert the record (`ON CONFLICT DO NOTHING`, noting whether this call inserted it),
     then `CREATE USER '<name>'@'%' IDENTIFIED BY '<password>'`, **without** `IF NOT EXISTS`. A
     failure forgets the record if this call inserted it, so a user that appeared between the read
     and the create (someone else's) is never adopted. The password is
     `engines::random_internal_secret()`, 32 random bytes, used in that one statement and kept
     nowhere: nobody can log in as the user.
   - drop: `DROP USER '<name>'@'%'`; on success forget the record, on failure keep it (the next
     run tries again).
   - Every literal goes through `egret_query_api::quote_string` (StarRocks dialect: `'` doubled,
     `\` escaped). Every FE error reaches a log only through `FeError::scrubbed(&fe, &secrets)`,
     with the statement's password in `secrets`.
5. **Native impersonation** (2.7), on an engine bound to no profile.
6. **Release** the lock and answer:

```rust
pub(crate) enum UserSync {
    NotNeeded,            // no delegator and nothing recorded: no FE call at all
    Busy,                 // another process holds the engine's lock
    Done { created: usize, dropped: usize, granted: usize, revoked_all: bool,
           foreign: Vec<String>, refused: Vec<String>, failed: Vec<String> },
    Failed(String),       // the read of the FE or of the database failed (scrubbed)
}
```

A failure never fails an apply, a create or a membership write: it is logged, and the next
notification or sweep tries again.

#### 2.4 When it runs (the hook points)

Membership is written in crates that cannot call engine-mgmt, inside transactions that must not wait
on an FE. So the writes **notify**, and engine-mgmt **listens**:

- **Migration 0219 adds statement-level triggers** that `pg_notify('egret_starrocks_user_sync',
  '*')`. Postgres delivers a `NOTIFY` only at commit, and only once per payload per transaction, so
  a rolled-back write notifies nothing. Triggers, not Rust calls, for the reason migration 0209 gives
  for the ledger: they cover every writer, present and future.

  | Table | Events | What it covers |
  |---|---|---|
  | `access_grants` | INSERT, UPDATE, DELETE | workspace membership add, remove and role change (`MemberRepo`), the access-request approval, group grants, user delete's grant cleanup, and platform-role grants and revokes (`platform_admin` included) |
  | `roles` | UPDATE OF `name`; DELETE | the row named `platform_admin` renamed or removed (its grants cascade from `access_grants.role_id`) |
  | `user_groups` | INSERT, UPDATE, DELETE | membership through a group, group delete (its rows cascade) |
  | `users` | UPDATE OF `status`, `username`; DELETE | disable and enable, an operator's rename by SQL (fact 2: no egret path renames), user delete |
  | `engine_mgmt.engine_workspaces` | INSERT, DELETE | engine assigned to or unassigned from a workspace (all three routes), workspace delete (cascade) |

- **A listener in engine-mgmt**, `spawn_starrocks_user_sync(st)`, started by `egret-bin` beside the
  query-log boot pass: a `PgListener` on that channel and a 10-minute interval whose first tick is
  immediate. Each wake-up, after folding in every notification already buffered, runs
  `sync_all_starrocks_users`: every engine of 2.1 that is impersonate-mode or has records, one at a
  time. The immediate first tick is the **boot** reconcile. The interval covers what writes nothing:
  a time-boxed membership that lapses, a notification lost while the listener reconnects, an FE that
  lost its metadata on a restart outside an apply, an FE that was down during the last try.
- **After every StarRocks apply** (`finish_apply`, `ApplyOp::Update`, on success: update, restart,
  Ranger rebind, catalog re-apply, security upgrade), `spawn_user_sync(st, id, name, namespace)`:
  if the engine wants a sync (impersonate-mode or has records), it waits for the FE and a real root
  login (`wait_for_fe_root_login`), passes an `engine_still_current` checkpoint, and syncs.
- **The post-Ready bootstrap** (`spawn_starrocks_catalog_wire`, create and delegation change): a new
  checkpoint and the sync right after the delegation statements (which create `egret_delegator`)
  and before the query log (whose node wait can take minutes). The function's checkpoint list grows
  from seven to eight. The bootstrap no longer sends `GRANT IMPERSONATE ON ALL USERS` (2.7).
- **Engine delete:** nothing to do on the FE (the cluster goes); the records cascade with the row.

**Mode changes.** impersonate → jwt runs the bootstrap (delegation changed), whose sync has an empty
desired set and drops every recorded user. jwt → impersonate creates them.

#### 2.5 The username ledger: used for its guarantee, extended by one row

The sync identifies a person's StarRocks user **by name**, so it relies on a name never passing to
another person. That is exactly what the ledger guarantees (migration 0209): a deleted or renamed
user's name is never minted again. So a StarRocks user left behind while an FE was unreachable can
never become a later person's identity: the next reconcile drops it, and until then its password is
unusable and no egret user is named that. The sync reads no ledger row itself; it needs only that
guarantee.

Migration 0219 adds **`root`** to the ledger as `reserved` (`ON CONFLICT DO NOTHING`), as 0209 did
for `egret_delegator` and 0218 for `egret_query_log`. Today a person whose email is `root@…` would be
minted `root`. After 0219 they get `root-2`. A person who already holds `root` keeps it, and the sync
refuses that name and logs it (2.3, the reserved row). Their queries on an impersonate-mode StarRocks
engine are denied by D1b's row (`EXECUTE AS root`) instead of running as StarRocks' superuser.

#### 2.6 Root stays out of the query path (and users egret did not create stay untouched)

The query path is unchanged: egret-query logs in as `egret_delegator` and runs `EXECUTE AS`. Only
engine-mgmt holds root, for the sync, as it already does for the bootstrap and the query log. D1b
makes `EXECUTE AS root` impossible for the delegator under Ranger.

Users egret did not create (decided on 2026-11-11, 3) are never altered, granted or dropped. The
sync logs a warning naming each, every run. The handover is documented: drop the user as root, and
the next sync creates egret's.

#### 2.7 Native access control: per-user IMPERSONATE (decided on 2026-11-11, 2)

An engine bound to no profile runs native access control, where `GRANT IMPERSONATE ON ALL USERS`
lets the delegator become `root`. So:

- **The bootstrap stops granting `ON ALL USERS`**: `starrocks_impersonation_grant_sql` and its push
  go. The sync, which the bootstrap runs right after (checkpoint 7), grants what is needed.
- **The sync**, on an engine with `authz_service_id = NULL` and a delegation mode of `impersonate`
  or `jwt`, after the user steps:
  1. reads `SHOW GRANTS FOR 'egret_delegator'@'%'` (M6) into `(all_users, per_user)`;
  2. if `all_users`: `REVOKE IMPERSONATE ON ALL USERS FROM USER 'egret_delegator'@'%'`, then reads
     the grants again (M7: the revoke may or may not have taken per-user grants with it);
  3. for each user egret manages and the FE has (kept plus created this run) and that is not yet in
     `per_user`: `GRANT IMPERSONATE ON USER '<u>'@'%' TO USER 'egret_delegator'@'%'`. A reserved
     user is never granted (it is never managed; the grant builder also refuses one).
  Grants to users egret did not create are neither added nor removed. A dropped user takes its
  grant with it.
- **On a bound engine** (Ranger), the sync issues no native grant or revoke: Ranger does not read
  them, and D1b's row is the control. An engine whose profile is later unbound converges on its next
  sync.
- **Existing engines** converge on their next apply or sweep (at most ten minutes after the
  upgrade): `ON ALL USERS` is revoked and the per-user grants appear.
- Failures go into `Done.failed`, scrubbed, and the next sync tries again. In the window between
  the revoke and the grants, a member's `EXECUTE AS` is refused; it lasts one sync.

### 3. `MYSQL_PWD` for the FE and the CN (D2)

In `starrocks_cluster_cr`, after `apply_pod_fields` (which writes the admin `pod.env` over the whole
array), when `root_password(spec)` is `Some`, append to `feEnvVars` and to `cnEnvVars`:

```json
{"name": "MYSQL_PWD", "valueFrom": {"secretKeyRef": {"name": "<name>-initpwd-secret", "key": "password"}}}
```

unless an entry named exactly `MYSQL_PWD` is already there (the admin's wins, as in the helm
chart). The Secret is applied before the CR (`apply_all`). The value is a reference: the password
appears in no rendered manifest.

- **FE:** needed only for `feReplicas > 1` (followers register through `mysql -u root`); costs
  nothing otherwise.
- **Effect on running engines:** the FE and CN pod templates change, so both roll **once**, on the
  engine's next apply. An engine whose CN is crash-looping today recovers on that apply (an
  **Update**, **Restart** or **Upgrade security**).
- **Engines without a persistent fe-meta** (created before #599) lose their FE metadata on any
  roll, this one included. That is the existing upgrade note "StarRocks front-end metadata is
  persistent only for new engines", and the re-applied initpwd Job sets root's password again.
- **Passwordless legacy engines** render exactly what they render today.

### 4. Engine delete removes the Job's pods and the FE metadata (D3)

- **The initpwd Job** leaves the `delete_objects` list and is deleted with
  `KubeClient::delete_background`, so its pods go with it.
- **Retention:** when `fe_meta_persistent`, the FE spec carries
  `"persistentVolumeClaimRetentionPolicy": {"whenDeleted": "Delete"}`. The operator copies it onto
  the FE StatefulSet; it is a mutable field, so an existing engine takes it on its next apply without
  the `volumeClaimTemplates` trap (`docs/en/plans/2026-09-26-fe-meta-live-result.md`), and it does not
  change the pod template (no roll). Unmarked engines render no key, byte for byte as today.
- **Explicit claim delete:** after the CR and the objects, `delete` probes
  `fe-meta-<name>-fe-0`, `-1`, … with `get_object` and deletes each claim that exists, stopping at
  the first ordinal that does not (ordinals are created from 0 and kept from 0 to the highest ever,
  so they are contiguous), and at 16 at most. This covers engines never re-applied since this
  package. A claim still bound to a terminating pod is deleted when the pod goes (the
  `pvc-protection` finalizer).
- **Errors:** all three steps are attempted; the error names every object that failed, as
  `delete_objects` already does.
- **RBAC:** the provisioner Role already has `persistentvolumeclaims` `get` and `delete`
  (`deploy/helm/egret/templates/rbac.yaml`).

**Consequence, documented:** deleting a StarRocks engine destroys its FE metadata (users, catalogs,
grants, the root password, the query log table). Table data in object storage stays under the
engine's storage path.

### 5. A same-name create refuses a leftover claim (decided on 2026-11-11, 4)

`create_starrocks_engine`, right after `check_starrocks_engine_name` and before any row exists,
reads `fe-meta-<name>-fe-0` in the engine namespace through `st.kube` (no Kubernetes client, as on
the Docker provisioner: no check). If the claim exists, the create answers **409** with
`code: "engine_metadata_left_over"`, `claim: "<claim>"`, and the message:

> A previous StarRocks engine named "<name>" left its front-end metadata volume <claim>. A new
> engine would start with the old engine's users, catalogs and root password. Delete it first:
> kubectl delete pvc <claim> -n <namespace>

A failure to read the claim answers 502 (`ClusterApplyFailed`) naming the claim: the create does
not guess. This covers claims from deletes before this package and a claim still terminating
seconds after a delete.

## Error handling

- Nothing in this package fails a create, an apply, a delete's success path, or a membership write
  because of an FE. A sync failure is logged with the scrubbed FE message and retried by the next
  notification, apply or sweep.
- A delete that cannot remove a claim or the Job answers the existing 502 cluster-delete failure,
  naming them; the row stays `deleting` and a retry finishes.
- A query by a person whose StarRocks user is not there yet (the seconds between a membership write
  and its sync) fails with StarRocks' own `cannot find user`. That is not changed here.

## Testing

Every behaviour change gets a test that fails without it, and a mutant of the change the test kills,
recorded in the task report. A DB-gated or engine-gated test that skipped proves nothing; the report
gives its duration.

- **Unit (pure):** `STARROCKS_RESERVED_USERS` and its predicate; `delegator_impersonates` for each
  engine and mode; `lower_infra` (StarRocks impersonate emits the row with the list, StarRocks
  without the delegator does not, Trino unchanged); the rendered body as a JSON literal; refusal of
  a mismatched pairing and of an empty list; `decide` for `ann`, `root`, `ROOT`, `egret_query_log`
  and `egret_delegator`; `plan_users` for every row of the table in 2.3; the SQL builders with a
  name and a password containing `'` and `\`; `is_starrocks_user_name` against fact 5; the CR's env
  (passworded, passwordless, admin env kept, admin `MYSQL_PWD` kept); the retention key (marked,
  unmarked); delete (Job by background, claims 0..n deleted and n+1 not probed past, no claims, a
  failing claim reported after the rest).
- **Unit (2.7 and 5):** the grant, revoke and `SHOW GRANTS FOR` builders (quoting, a reserved user
  refused); the `SHOW GRANTS` parser (all-users, per-user lists, other rows); the 409's body.
- **DB:** convergence adds the StarRocks delegator row and removes it on impersonate → jwt;
  migration 0219 reserves `root` (minting `root@…` gives `root-2`) and each trigger notifies at
  commit and not on rollback; the store's desired set (direct, through a group, expired, deny,
  disabled, unassigned); `sync_starrocks_users` against `FakeStarRocksDdl` (create, drop, forget,
  foreign untouched, reserved refused, create failure forgets its own record, FE error scrubbed,
  `Busy` under a held lock, `NotNeeded` makes no FE call); platform admins synced on an assigned
  engine and not on an unassigned one; the `roles` and platform-grant triggers notify; on an unbound
  engine the sync revokes `ON ALL USERS` and grants per user, and on a bound one it issues neither;
  the bootstrap no longer grants `ON ALL USERS`; the bootstrap stops before the sync when the
  engine is deleted; an apply spawns a sync; a create with a leftover claim answers 409 and creates
  no row.
- **Goldens:** re-blessed with `EGRET_GOLDEN_BLESS=1` and checked semantically: the only change is
  the new row on StarRocks impersonate profiles (and the served list's ordinals behind it). Then the
  corpus passes unblessed, and with `EGRET_GOLDEN_MECHANISM=opa`.
- **Real StarRocks** (`EGRET_TEST_STARROCKS_URL`, egret's allin1 `4.1.7-egret.4`): the sync twice
  (the second changes nothing), a foreign user left alone, a drop, the native grants and the revoke;
  M2, M3, M6 and M7 pinned.
- **Fork e2e:** the rendered row in the fork's fake Ranger; M1.
- **Live on kind:** the plan's controller task: the four proofs, native impersonation on an
  engine bound to no profile, and the create refusal (M4, M5).

## Upgrade notes (for the docs)

- **StarRocks engines in impersonate mode start working under Ranger.** On upgrade, convergence
  writes `egret-delegator-impersonate` to their profile, and the user sync creates a StarRocks user
  for each member of each workspace the engine serves, within seconds of start. Users made by hand
  for earlier tests (for example a passwordless `member-sso`) are someone else's to egret: they are
  left as they are, and a warning names them. Drop them as root to let egret create its own.
- **Passworded StarRocks engines roll their FE and CN once** on their next apply (`MYSQL_PWD`). A
  CN that crash-loops today (KI-335) recovers then. Engines without a persistent fe-meta lose their
  FE metadata on that roll, as on any roll.
- **Engine delete now removes the FE metadata volume.** FE metadata of a deleted engine is gone; the
  table data stays in object storage.
- **Claims left by earlier deletes** are not removed by the upgrade. List them with
  `kubectl get pvc -n egret-engines | grep '^fe-meta-'` and delete those whose engine no longer
  exists, before re-creating an engine with the same name.
- **A user named `root`** cannot be minted any more (they get `root-2`); one who already holds it
  keeps it, and is not synced to StarRocks.
- **Platform admins get a StarRocks user** on every impersonate-mode engine assigned to a workspace.
- **Engines bound to no profile lose `GRANT IMPERSONATE ON ALL USERS`**: on their next apply or sweep
  the delegator gets per-user grants for egret's users instead, and can no longer become `root`.
- **Creating a StarRocks engine whose name left a metadata claim** answers 409 with the `kubectl`
  command to remove it.

## Documentation (egret-data/docs, Platform section)

- **Engines and Ranger:** the StarRocks infra rows now include `egret-delegator-impersonate`
  (impersonate mode) with its exclusions; how egret creates and drops the StarRocks users, when, and
  what it never touches.
- **Workspace settings, Databases:** the StarRocks impersonation row no longer asks for a hand-made
  service user.
- **Engine detail, Delete:** FE metadata is destroyed; table data stays.
- **Upgrade notes:** the eight notes above.
- **Handover of a hand-made user:** egret leaves a StarRocks user it did not create untouched and
  logs it; to hand it over, drop it as root, and the next sync creates egret's.
- **REST API:** `POST /engines` can answer 409 `engine_metadata_left_over`.
- **Known issues:** KI-335 fixed; new rows for defect 1 and defect 3, each marked fixed by this
  package.

## Out of scope

- External StarRocks engines: egret is not their root.
- A user-visible sync status (API or UI). The outcome is logged.
- Clearer error text for the seconds between a membership write and its sync.
- `egret-ranger-eval`'s `PluginVersion` for StarRocks says 2.8.0 while the fork's pom pins 2.9.0
  (fact 7). Recorded, not changed here.

## Decided on 2026-11-11 (formerly the open questions)

1. **Platform admins are synced.** Every live `platform_admin` holder joins the desired set of every
   impersonate-mode engine assigned to at least one workspace (2.1). Triggers cover platform-role
   grants and revokes (`access_grants`) and the role table (`roles`) (2.4).
2. **Native access control is fixed in this package.** The sync grants `IMPERSONATE ON USER` per
   user it manages and revokes `ON ALL USERS`; reserved users are never granted; the bootstrap stops
   granting `ON ALL USERS`; existing engines converge on their next apply or sweep (2.7). M6 and M7
   measure the FE's answers.
3. **Users egret did not create stay untouched.** Logged, and the handover is documented (2.6).
4. **`POST /engines` answers 409 on a leftover claim**, code `engine_metadata_left_over`, with the
   `kubectl` cleanup in the message (section 5). The upgrade note stays.

The question texts are in this file's git history (commit `acbc223cd`).
