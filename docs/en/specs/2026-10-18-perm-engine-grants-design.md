# Engine-scoped grants (F11): design

Date: 2026-10-18. Status: decisions recorded below; the user ruled on scope (curated catalogue) on 2026-10-18; every
other row marked "default decision" is this document's recommendation. Base: main `4a67321b` (#692). Branch
`feat/perm-engine-grants`, worktree `egret-f11`. Backend only.

Decision F11 of the programme spec (`docs/en/specs/2026-10-16-dg-frontend-redesign-design.md`) makes engine-level
permissions real grants in egret's model, compiled by `egret-perm-compile`, granted later from an "Engine permissions"
tab (sub-project #4, F12). This document specifies the backend package #4 depends on.

**Sources of claims.** Every claim about egret code carries a `file:line` from `4a67321b`, read in this worktree.
Engine semantics are read in the sibling checkouts: Trino `data-portal/trino` at `482-75-g280b81bbc4e` (egret runs
Trino 483; `access-control.name=ranger` selects Trino's in-tree `trino-ranger` plugin,
`crates/egret-engine-mgmt-api/src/ranger_config.rs:74`, `RangerSystemAccessControlFactory.java:30`); StarRocks
`data-portal/starrocks` main at `148a7fdc44e` (egret runs 4.1.6; the engine version actually run is measured by the
differential and the live check, not by this reading); Apache Ranger `data-portal/ranger`. Trino paths below are
under `plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/`; StarRocks paths under
`fe/fe-core/src/main/java/com/starrocks/`.

## Corrections to the brief

The brief this spec was written from carried premises that the code or the engines contradict. Each is corrected here
and the design follows the correction.

| # | brief said | what is true | consequence |
|---|---|---|---|
| C1 | "view/kill other users' queries" is `queryid` execute | Trino checks viewing and killing another user's query as **`trinouser` = the owner, `impersonate`** (`RangerSystemAccessControl.java:185-216`). `queryid` `execute` is `checkCanExecuteQuery` (`:177-182`), the right to run any query at all, which egret already grants everyone through `egret-user-execute-query` (`crates/egret-engine-mgmt/src/delegator_policy.rs:90-99`) | no `queryid` capability; "impersonate user X" is one capability whose label says it also lets the grantee view and kill X's queries (they are one Ranger permission) |
| C2 | function execute is the `function` resource | the plugin checks `catalog`/`schema`/**`schemafunction`** with `execute` (`RangerSystemAccessControl.java:753-756`); the level-10 `function` resource of the service-def is never read (`RangerTrinoResource.java:26-37` declares no such key). `schemafunction`'s `accessTypeRestrictions` omit `execute` (`crates/egret-policy/src/service-defs/trino.json`, identical to Ranger's `ranger-servicedef-trino.json`), but restrictions only shape default policies, never evaluation (`agents-common/.../RangerBaseService.java:195-213`) | the capability compiles to `schemafunction` + `execute`; the catalogue test asserts it is the one entry outside its resource's restriction list |
| C3 | StarRocks "usage of a resource group" | no such privilege: resource groups are checked only for `DROP` and `ALTER` (`sql/analyzer/AuthorizerStmtVisitor.java:920-940`); creating one is `system` `create resource group` (`:915`) | dropped from the catalogue (E5) |
| C4 | StarRocks `system` `operate` is grantable | `OPERATE` gates `ADMIN SET CONFIG`, `EXECUTE SCRIPT` (code run on the FE), fail points, `ADMIN SKIP COMMITTED TRANSACTION`, replica status (`AuthorizerStmtVisitor.java:1519-1524`, `:2300`, `:2315`, `:2343`, `:2449`) and killing another user's connection (`qe/StmtExecutor.java:1878-1880`, `qe/ConnectScheduler.java:326-328`) | near-root: not grantable (E4); "kill other users' queries" has no curated capability on StarRocks |
| C5 | several engines may share a profile | a bind refuses a profile another engine binds (I2, `crates/egret-engine-mgmt/src/engines.rs:5265-5281`, shared by create and external registration). There is no unique index (`migrations/0058_engine_authz_service.sql:2-3`), so legacy rows can still share | the reach list (E9) stays, and is one engine in every installation created since I2 |
| C6 | the refused targets `egret-lineage` / `egret_delegator` are "reserved" | only `egret-lineage` is in `RESERVED_ENGINE_USERNAMES` (`crates/egret-core/src/engine_principals.rs:16-19`); `egret_delegator` is a constant in `egret-engine-mgmt` (`delegator_policy.rs:15`), which `egret-perm-compile` cannot depend on (it is a dev-dependency the other way, `crates/egret-engine-mgmt/Cargo.toml:93`) | the constant moves to `egret_core::engine_principals` (re-exported where it is used today); the impersonation refusal names both. Adding `egret_delegator` to the reserved *username* list is a separate fix, not done here (E21) |
| C7 | People DTOs gain engine grants | People routes return a principal's `standsFor`; the client lists grants by matching it (P4, `crates/egret-perm-compile/src/people.rs:356-362`, `:473-521`). No DTO carries grants | a list route `GET /perm/engine-grants` instead (E15); no People DTO changes |
| C8 | extend `/perm/explain` with a capability target | `ExplainRequest` requires `action` (`read`/`write`) and `target: Scope` (`crates/egret-perm-compile/src/routes.rs:460-464`); a capability question has neither. Making them optional would change every generated caller | a sibling route `POST /perm/explain/capability` reusing the same per-profile pieces (E13) |
| C9 | `{USER}` values make explain Undecidable | wider: on every delegating Trino profile the seeded `egret-user-impersonate-self` puts `{USER}` on the `trinouser` level (`delegator_policy.rs:78-86`), and the evaluator refuses every request that carries that level (`crates/egret-ranger-eval/tests/eval.rs:1108-1135`) | every Trino impersonation question is Undecidable on a delegating profile; explain says so (E13). Teaching the evaluator `{USER}` is out of scope |
| C10 | the sweep only needs the new prefix | four places select by the compiler prefixes (E7), and `perm.compiled` cannot record an engine grant (`source_kind IN ('grant','rule')`, `migrations/0154_perm_model.sql:120-127`) | provenance is the policy name, as for a clearance (`crates/egret-perm-compile/src/trigger.rs:1262-1276`) |
| C11 | (implicit) the existing recompile paths keep an engine's profile current | a provisioned engine's rebind (`PUT /engines/{id}`) recompiles only the NEW profile (`engines.rs:5367-5440`, log "the previous profile row is untouched", `:5436`), and neither delete path recompiles (`engines.rs:6007-6009`). An external engine's update already recompiles both (`crates/egret-engine-mgmt/src/external.rs:406`, `:430-439`). Harmless for catalog-keyed grants; an engine-keyed grant would stay compiled in the old profile | the provisioned rebind and both delete paths recompile the former profile (E10) |
| C12 | a governance audit helper may exist | `PermState` carries no audit sink (`routes.rs:103-112`), `egret-perm-compile` does not depend on `egret-audit`, and `feat/dg-overview` (#3) has no commit past `4a67321b` | a named hook that logs the event shape; #3 or this package, whichever merges second, wires `egret_audit::mutation_for` (E18) |

True as stated: migration numbers end at `0194` on main and `0195` is free; the duplicate-version gate is
filename-based (`crates/egret-metastore/src/lib.rs:173-221`); `apply_diff_in_tx` (`trigger.rs:975`) sweeps by prefix
(`trigger.rs:1016-1031`); `is_compiler_owned_policy_name` (`crates/egret-policy/src/store.rs:119-123`) and
`COMPILER_GRANT_PREFIX` (`store.rs:87`); the 1000-policy profile ceiling (`trigger.rs:194`, enforced `:1052-1066`);
`validate_user_ref` (`crates/egret-perm-compile/src/grants.rs:106-124`); `write_grant` (`routes.rs:1047`);
`request_resource` (`routes.rs:2834`); `egret_perm::eval::evaluate` is only a test oracle
(`crates/egret-perm/src/eval.rs:79`); the migration grammar refuses non-data keys (`UnknownLevel`,
`crates/egret-perm-compile/src/grammar.rs:166`, raised at `:688`).

## Decisions

| id | decision | by |
|---|---|---|
| E1 | Scope is a **curated capability catalogue** (§2). Near-root capabilities are not grantable; they stay admin-only through the seeded `egret-admin - <chain>` policies, and hand-written policies remain the escape hatch (F12) | user, 2026-10-18 |
| E2 | Trino catalogue: impersonate a named user (also view and kill their queries, C1); set a named system session property; set a named session property of a named catalog; execute a named procedure; execute a named catalog function (C2) | default decision (2026-10-18), refined from the user's list |
| E3 | StarRocks catalogue: impersonate a named user (`EXECUTE AS`, `AuthorizerStmtVisitor.java:1504-1516`); use a named global function (`sql/analyzer/AnalyzerUtils.java:301`); manage blacklists (`system` `blacklist`, `AuthorizerStmtVisitor.java:1296-1415`) | default decision (2026-10-18); StarRocks impersonation added by symmetry with Trino |
| E4 | Not grantable: Trino `trinouser` with a wildcard, `role`, `sysinfo` write, `queryid` (C1); StarRocks `system` `node`/`plugin`/`file`/`grant`/`operate` (C4)/`repository`/every `create …`, `storage_volume`, `resource`, `resource_group` (C3). `GET /perm/engine-capabilities` lists them with the reason | user (near-root list) + default (C3, C4) |
| E5 | Allow only (no deny). Principals: `user`, `group`, `role`; never `workspace_members` (engines are not workspace-scoped) and never the `public` group (an engine capability for everyone is what the seeded baselines are for) | default decision (2026-10-18) |
| E6 | New tables `perm.engine_grants` and `perm.engine_grant_principals` (migration **0195**), not `perm.grants` (§3) | default decision (2026-10-18) |
| E7 | Compiled as one Ranger access policy per engine grant named `perm-engine-{uuid}`, `managed_by = 'compiler'`, into the engine's profile (`engine_mgmt.engines.authz_service_id`); the prefix joins the three existing ones everywhere they are listed (§4.3) | default decision (2026-10-18) |
| E8 | Values are concrete: no `*`, `?`, `{`, `}`, no `{USER}`-style macro, no empty value, checked in the route and by a `CHECK` | default decision (2026-10-18) |
| E9 | Every engine-grant response carries `reach`: every engine bound to the profile the grant compiles into, so a label never claims fewer engines than the grant reaches (C5) | user, 2026-10-18 |
| E10 | A provisioned engine's rebind recompiles the former profile too; an engine delete recompiles its former profile when another engine still binds it (C11). Both fire-and-forget after commit, logged, as `restore_engine_authz_service` does (`engines.rs:5201-5226`) | default decision (2026-10-18) |
| E11 | Impersonation targets must be a current `users.username` (resolved to the user's id and stored by id, E21), never `egret-lineage`, `egret_delegator` or `root` (case-insensitive; egret runs as `root` on StarRocks, security review C1, refused on both engines), never a user who holds a not-grantable (near-root) key/access pair on the profile (security review I1: any `NOT_GRANTABLE` row but `queryid execute`, which everyone holds, plus `trinouser`/`user` with a wildcard value) through any enabled access policy that is not a compiled engine grant, operator and infra policies included, named by username, `{USER}`, a group they belong to or `public` (a policy scan, not an evaluation: the evaluator answers every Trino impersonation question Undecidable on a delegating profile, C9, which would refuse every target); and never a user from whom a chain of concrete impersonation hops (security re-review I-a: through every enabled access policy of the profile, compiled engine grants included, and the current engine-grant model, so an uncompiled grant is a hop too; `{USER}` is a no-op hop; bounded at 256 users, a longer walk is refused) reaches a reserved user or a near-root holder, on both engines (Trino chain semantics unmeasured, so each hop is treated as the current identity); group names match ignoring case, as Ranger does (M-c); re-checked at compile, where a target that fails is skipped and reported | default decision (2026-10-18) |
| E12 | Seeded infrastructure policies of the profile are listed per engine as `managed` read-only entries, mapped to a capability where their resource keys match one (§5.2) | user, 2026-10-18 |
| E13 | `POST /perm/explain/capability` evaluates one user and one capability on the artifact the engine's profile serves, with `egret-ranger-eval`; Undecidable is returned as such (C8, C9) | default decision (2026-10-18), in place of extending `/perm/explain` |
| E14 | Routes, permissions and transaction shape as `write_grant`: `policy:read` reads, `policy:update` writes, one transaction holding group locks, the engine row (`FOR SHARE`), the profile locks, the row, and the recompile (§5) | user, 2026-10-18 |
| E15 | People pages get engine grants through `GET /perm/engine-grants` matched against `standsFor` on the client (C7) | default decision (2026-10-18) |
| E16 | Engine grants count as references everywhere grants do: group delete/rename refusal, membership recompiles, the pre-mint sweep, unresolved principals, the username-allocation 409 (§4.5) | user, 2026-10-18 |
| E17 | Migrate (`/perm/policies/services/{id}/migrate`) does not recognise hand-written engine-level policies; the grammar keeps refusing non-data keys (`grammar.rs:166`) | default decision (2026-10-18) |
| E18 | Audit: a named hook in the three write handlers that logs the `Mutation` shape; wired to `egret_audit` by whichever of #3 and F11 merges second (C12) | default decision (2026-10-18) |
| E19 | Migration 0195 renames any pre-existing policy whose name starts with `perm-engine-` and is not compiler-owned, so the sweep can never reap a hand-written policy (§3.3) | default decision (2026-10-18) |
| E20 | Out of scope: the "Engine permissions" tab (#4), deny, workspace scoping, teaching the evaluator `{USER}`, RisingWave | user, 2026-10-18 |
| E21 | **Amended 2026-10-18 (controller, safety tight):** an impersonation TARGET is stored by identity, `perm.engine_grants.target_user_id uuid REFERENCES users(id) ON DELETE CASCADE`, never by name; the compiler renders the user's CURRENT `users.username` from the id and skips the grant when the user has none. A freed username re-minted to another person therefore never inherits an impersonation grant, and deleting the user deletes the grant. Recorded, not fixed (security re-review M-b): the group arm of the target scan (I1, I-a) is inert today, since no engine egret provisions resolves groups (D7) and a group item matches nobody there; it only over-refuses, and becomes exact once an engine resolves groups. Recorded, not fixed: `egret_delegator` and `root` are not reserved usernames (C6, security review C1; reserving them changes user creation and is a follow-up, so a human can still be minted either name, and only the impersonation-target refusal stops it from being used); grantee principals keep the name-based ref machinery, so a hard-deleted user's freed username can still be re-minted to another person who then inherits the GRANTEE refs (`crates/egret-perm-compile/src/principal_refs.rs:82-96`); the external engine update recompiles new-then-old profile in one transaction, each through its own `lock_services_in_tx`, so the two profile locks are not taken in one ascending order (`external.rs:406`, `:430-439`) | default decision (2026-10-18), target-by-id amended by the controller |

## 1. What exists

- **Model and compile.** `perm.grants` is catalog-scoped: `catalog_id … NOT NULL`, `actions <@ ARRAY['read','write']`
  (`migrations/0154_perm_model.sql:13-24`). `compile_desired` reads every grant and rule, expands `workspace_members`,
  `role` and non-`public` `group` principals into usernames (`trigger.rs:439-445`, `members.rs:668`), compiles grants
  whose catalog the profile reaches (`trigger.rs:450-510`), rules, clearances, then refuses created overlaps
  (`trigger.rs:637-644`). `apply_diff_in_tx` locks the swept services, reads existing compiler-owned policies by
  prefix, enforces the ceiling, stamps `origin = 'generated'`, inserts with `managed_by = 'compiler'`, updates, reaps
  and bumps `policy_version` (`trigger.rs:975-1232`).
- **Write path.** `write_grant` (`routes.rs:1047-1263`): confinement, non-empty include, read the old affected set,
  take group locks (`lock_named_groups`, `routes.rs:1018-1031`) BEFORE resolving refs (`resolve_principal_refs`,
  `principal_refs.rs:167`), shape check, lock profiles ascending (`lock_services_in_tx`, `trigger.rs:1833`), write,
  `recompile_services_in_tx` (`trigger.rs:1913`). Create/patch/delete handlers wrap it in one transaction
  (`routes.rs:1531-1640`).
- **Engines and profiles.** `engine_mgmt.engines` has `id uuid`, `name`, `type IN ('trino','starrocks','risingwave')`
  (`migrations/0056_engines.sql:4-13`), `authz_service_id` nullable (`migrations/0058_engine_authz_service.sql:5-7`),
  `origin IN ('provisioned','external')` (`migrations/0184_engine_external.sql:20-24`). A trigger bumps the bound
  profiles' `policy_version` on every bind change (`migrations/0187_view_lineage.sql:65-89`). The compiler already
  joins `engine_mgmt.engines` (`crates/egret-policy/src/store.rs:1336-1340`, `migrate.rs:448`).
- **Seeded policies.** Per delegating Trino engine: `egret-user-impersonate-self` (`trinouser={USER}`, impersonate,
  `{USER}`), `egret-user-execute-query` (`queryid=*`, execute, `{USER}`), `egret-query-timeout`
  (`systemproperty=query_max_run_time`, alter, `{USER}`) (`delegator_policy.rs:78-125`), and on impersonate-mode
  engines `egret-delegator-impersonate` (`trinouser=*` for `egret_delegator`, `:53-61`). Per profile:
  `egret-admin - <chain>` all-access for platform admins on every resource hierarchy (`default_policies.rs:53-107`,
  `:167-195`), and on Trino profiles seeded through `seed-defaults` the `all - <chain>` set for group `admin`
  (`:116-160`). All are `managed_by = 'infra'` (`infra_names.rs:93-100`).
- **Explain.** `explain_users` evaluates each profile's served artifact (`served_artifact_via`, `routes.rs:2903`) with
  `EngineConfig { engine, plugin_version, ..without_super_users() }` and no groups or roles (`routes.rs:2096-2300`).
- **Group guard (#692).** A group's delete or rename asks `group_references_in_tx`, answered by
  `group_references_via` over grant principals and rule exemptions (`trigger.rs:1788-1810`), and refuses with the
  list (`crates/egret-auth/src/groups.rs:312-355`). Membership writes recompile
  `affected_services_for_group_membership` / `_role_membership` (`trigger.rs:1722`, `:1574`).

## 2. The catalogue

`crates/egret-perm-compile/src/engine_capabilities.rs`, a static table. Each entry: `id`, engine type, label,
description (what the grantee can do, in one or two sentences), the Ranger resource keys in hierarchy order, the
value each key takes (`username`, `identifier`, `catalog`, `function_signature`, or `fixed "*"`), and the one access
type.

| id | engine | resource → access | values | verified |
|---|---|---|---|---|
| `trino.impersonate_user` | trino | `trinouser` → `impersonate` | 1 username | `RangerSystemAccessControl.java:160-165` (set user), `:185-216` (view, list, kill the user's queries) |
| `trino.set_system_session_property` | trino | `systemproperty` → `alter` | 1 identifier | `:235-240` |
| `trino.set_catalog_session_property` | trino | `catalog`, `sessionproperty` → `alter` | catalog, identifier | `:265-270` |
| `trino.execute_procedure` | trino | `catalog`, `schema`, `procedure` → `execute` | catalog, identifier, identifier | `:705-710` |
| `trino.execute_function` | trino | `catalog`, `schema`, `schemafunction` → `execute` | catalog, identifier, identifier | `:753-756`; built-in and inline functions are never checked (`core/trino-main/.../metadata/FunctionResolver.java:365-370`) |
| `starrocks.impersonate_user` | starrocks | `user` → `impersonate` | 1 username | `AuthorizerStmtVisitor.java:1504-1516`, `authorization/ranger/starrocks/RangerStarRocksAccessController.java:55-62` |
| `starrocks.use_global_function` | starrocks | `global_function` → `usage` | 1 function signature | `AnalyzerUtils.java:301`; the value is `Function.getSignature()`, e.g. `my_udf(INT,VARCHAR)` (`catalog/Function.java:955-968`) |
| `starrocks.manage_blacklist` | starrocks | `system` → `blacklist` | fixed `*` (the def's `validationRegEx` is `\*`; the plugin always asks `*`, `RangerAccessResourceBuilder.java:30-33`) | `AuthorizerStmtVisitor.java:1296-1415` |

Labels state the consequences: impersonation is "act as <user> on this engine: run queries with their permissions,
and see and kill their queries"; a procedure "runs with what the procedure itself checks; some connector procedures
change data without a further check"; function and procedure execution also need catalog access, which a data grant
gives (`core/trino-main/.../security/AccessControlManager.java:1281-1283`); blacklist "affects every user of the
engine: it can block queries and take nodes out of scheduling".

**Not offered, with the reason returned by the API (E4):** `trinouser` `*` or any pattern (become anyone, including
admins), `role` (manage Trino roles), `sysinfo` `write_sysinfo`, `queryid` (already everyone's, C1), StarRocks
`system` `node`, `plugin`, `file`, `grant`, `operate` (C4), `repository`, `create …`, `storage_volume`, `resource`,
`resource_group` (no usage privilege, C3). Also not offered (not requested, low demand): Trino `sysinfo`
`read_sysinfo`, `schemafunction` create/drop/show. Each can be added later as a catalogue row plus its test rows.

**Value rules (E8, E11).** Every value is non-empty, at most 255 characters, has no leading or trailing whitespace, and
contains none of `*`, `?`, `{`, `}` (Ranger's wildcards and macro markers). `username` values must resolve through
`users.username` (citext, rewritten to the stored spelling, as `lookup_user` does, `principal_refs.rs:321`), must not be
`egret-lineage` or `egret_delegator`, and must not be named in the `users` of an enabled `egret-admin - …` policy on
the profile. `catalog` values must be a catalog the profile reaches (`catalogs_for_service_via`,
`crates/egret-policy/src/store.rs:1331`); `identifier` values match `^[A-Za-z0-9_][A-Za-z0-9_.$-]*$`;
`function_signature` matches `^[A-Za-z0-9_]+\([A-Z0-9_, .]*\)$`. Matching is case-insensitive on the engine for these
keys (`ignoreCase: true` in both defs), so the route stores the value lower-cased except a function signature, whose
argument types StarRocks prints upper-case.

## 3. Schema (migration 0195)

`migrations/0195_perm_engine_grants.sql`. The number is reserved for this package (#3 uses 0196 and up); re-check
`origin/main` before merge, as the filename gate's message says (`crates/egret-metastore/src/lib.rs:214-221`).

### 3.1 Tables

```sql
CREATE TABLE perm.engine_grants (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    engine_id   uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    capability  text NOT NULL CHECK (capability IN (
                    'trino.impersonate_user', 'trino.set_system_session_property',
                    'trino.set_catalog_session_property', 'trino.execute_procedure',
                    'trino.execute_function', 'starrocks.impersonate_user',
                    'starrocks.use_global_function', 'starrocks.manage_blacklist')),
    resource_values text[] NOT NULL,
    target_user_id uuid REFERENCES users(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (array_position(resource_values, NULL) IS NULL),
    CHECK (array_position(resource_values, '') IS NULL),
    CHECK (array_to_string(resource_values, chr(31))
           !~ ('(^|' || chr(31) || ')\s|\s($|' || chr(31) || ')')),
    CHECK (array_to_string(resource_values, chr(31)) !~ '[*?{}]'),
    CHECK (cardinality(resource_values) = CASE capability
             WHEN 'trino.set_catalog_session_property' THEN 2
             WHEN 'trino.execute_procedure' THEN 3
             WHEN 'trino.execute_function' THEN 3
             WHEN 'trino.impersonate_user' THEN 0
             WHEN 'starrocks.impersonate_user' THEN 0
             WHEN 'starrocks.manage_blacklist' THEN 0
             ELSE 1 END),
    CHECK ((capability IN ('trino.impersonate_user', 'starrocks.impersonate_user'))
           = (target_user_id IS NOT NULL)),
    UNIQUE NULLS NOT DISTINCT (engine_id, capability, resource_values, target_user_id)
);
CREATE INDEX engine_grants_engine_idx ON perm.engine_grants (engine_id);

CREATE TABLE perm.engine_grant_principals (
    engine_grant_id uuid NOT NULL REFERENCES perm.engine_grants(id) ON DELETE CASCADE,
    kind  perm.principal_kind NOT NULL CHECK (kind IN ('user', 'group', 'role')),
    ref   text NOT NULL CHECK (ref <> ''),
    CHECK (NOT (kind = 'group' AND ref = 'public')),
    PRIMARY KEY (engine_grant_id, kind, ref)
);
CREATE INDEX engine_grant_principals_ref_idx ON perm.engine_grant_principals (kind, ref);
```

`resource_values`, not `values`, because `VALUES` is reserved. An impersonation capability stores its target in
`target_user_id` and no `resource_values` (E21, amended): the API still takes and returns the username, which the route
resolves to the id and every read renders from it. No `mode` column: allow-only, include-only (E5).
`manage_blacklist` stores no value; the compiler supplies `*`. The capability list in the `CHECK` and the Rust
catalogue are one fact in two places, so a DB test inserts every catalogue id and one unknown id (§7).

**Why not `perm.grants`.** Its `catalog_id` is `NOT NULL` and every reader assumes it: the affected-service set is
derived from it (`trigger.rs:1403-1416`), `covers`/targeting and Data's tabs filter by it, the People pages group
grants by scope, the action vocabulary is a `CHECK` (`0154_perm_model.sql:23`), and the migration grammar compiles
to it. Relaxing any of these touches every one of those readers for rows none of them can use.

### 3.2 What the engine row's lifecycle does

`ON DELETE CASCADE` from the engine: deleting an engine deletes its engine grants. The compiled policies in its
former profile are reaped by the next recompile of that profile (E10).

### 3.3 Reserving the prefix (E19)

Before this migration `perm-engine-` was an ordinary name an operator could use; after it, the sweep reaps any
policy under it that no engine grant produces (`trigger.rs:1016-1031`). The migration renames every
`policy.authz_policies` row whose name starts with `perm-engine-` and whose `managed_by` is not `compiler`, to
`renamed by 0195: <name>`, sets `policy_jsonb->'name'` to the same string, bumps the row's `version` and its
service's `policy_version`, and `RAISE NOTICE`s the count. (No compiler-owned row can carry the prefix before this
package.) A DB test runs the statement on a fixture (§7).

## 4. Compile

### 4.1 The model

`egret-perm` gains `engine_grants.rs`: `EngineGrant { id, engine_id, capability: String, values: Vec<String>,
principals: PrincipalSet /* exclude always empty */ }` and connection-generic `list_engine_grants_via`,
`list_engine_grants_for_profile_via(conn, service_id)` (join `engine_mgmt.engines` on `authz_service_id`),
`get_engine_grant_via`, `create_engine_grant_in_tx`, `update_engine_grant_in_tx`, `delete_engine_grant_in_tx` —
the same split as `list_grants_via`/`create_grant_in_tx` (`crates/egret-perm/src/store.rs`). `egret-perm` takes no
`egret-policy` dependency (its `Cargo.toml:13-17` says why); the catalogue lives in `egret-perm-compile`.

### 4.2 compile_engine_grant

`egret_perm_compile::engine_grants::compile_engine_grant(g, profile_name, engine) -> Result<AccessPolicyInput,
CompileError>`, pure: resources from the catalogue entry and `g.values` (`isExcludes: false, isRecursive: false`),
one allow item with the expanded usernames and the one access type, `delegateAdmin: false`, rendered through
`access_to_ranger_json` and `stamp_service_type` as grants are (`trigger.rs:477-481`). It refuses an engine type
that does not match the capability, a value that breaks §2's rules, and a `User` ref that fails `validate_user_ref`.

`compile_desired` (`trigger.rs:373`) adds, after the grants: read `list_engine_grants_for_profile_via(conn,
service_id)`, expand their principal sets in the same `expand_enumerated_principals` call as grants and rules
(it gains an `engine_grants: &mut [EngineGrant]` argument and `referenced_principals` scans them), then per grant:

- an empty include after expansion → `SkippedItem { source_kind: EngineGrant, … }`, as a grant (`trigger.rs:454-475`);
- an impersonation target that no longer resolves, is reserved, or is now named in an `egret-admin - …` policy of
  the profile → skipped with that reason (E11). Not an error: a user deleted elsewhere must not fail every recompile
  of the profile;
- otherwise inserted into `desired` keyed `(service_id, "perm-engine-{id}")`, `source_kind: EngineGrant`, no
  provenance row (C10, extended `record_compiled` early return).

The policy counts toward `PROFILE_EXPANSION_CEILING` like every other desired policy (`trigger.rs:1052-1066`); an
engine grant is one policy, so it can only push a profile over the ceiling, never multiply.

### 4.3 The prefix, everywhere it is listed (E7)

`egret_policy::COMPILER_ENGINE_GRANT_PREFIX = "perm-engine-"` beside the others (`store.rs:87-112`), and:

1. `is_compiler_owned_policy_name` (`store.rs:119-123`);
2. `refuse_reserved_name`'s message (`store.rs:145-156`);
3. `refuse_if_compiler_owned_policies_remain`'s count (`store.rs:644-671`);
4. `apply_diff_in_tx`'s existing-state query (`trigger.rs:1016-1031`);
5. `attribute_policy_name` → `PolicyAttribution::EngineGrant(Uuid)` (`crates/egret-perm-compile/src/attribution.rs:91-106`),
   and `ExplainAttribution::EngineGrant { engine_grant_id }` (`routes.rs:503-554`).

`grep -rn "COMPILER_CLEARANCE_PREFIX" crates` is the checklist: every production site that lists the clearance prefix
lists this one too. `managed_by_agreement.rs` stays as it is (it pins 0180's backfill, which predates the prefix); the
0195 rename has its own test.

### 4.4 Which profiles a write recompiles

An engine grant reaches exactly one profile: its engine's `authz_service_id`. Create and delete recompile it; a patch
cannot move a grant between engines (the body has no `engine_id`), so it recompiles the same one. An engine with no
profile, of type `risingwave`, or with `status = 'deleting'` refuses the write (409, with the reason).

### 4.5 References (E16)

| reader | change |
|---|---|
| `group_references_via` (`trigger.rs:1788-1810`) | third `UNION ALL` arm over `perm.engine_grant_principals`, `source = 'engine_grant'`, `effect = 'allow'`, `position = 'include'` |
| `GroupReference.source` doc and the refusal text (`crates/egret-auth/src/groups.rs:169-185`, `:336-344`) | `"engine_grant"` → `engine grant <id> (allow, include)` |
| `affected_services_for_group_membership` (`trigger.rs:1722`) and `_role_membership` (`:1574`) | add the profiles of engines whose grants name the group / role |
| `premint_model_usernames` (`members.rs:357-367`) | `UNION SELECT kind::text, ref FROM perm.engine_grant_principals` |
| `unresolved_principals_via` (`principal_refs.rs:447-459`) | an `engine_grant` arm; `failsOpen` is always false (allow include) |
| `stored_user_refs_naming_via` (`principal_refs.rs:637-650`) | engine-grant `user` principals (`source = 'engine_grant'`). No arm for impersonation targets: they are stored by user id (E21, amended), so no stored name can go stale |
| `UserRepo::hard_delete` (`crates/egret-auth/src/users.rs:295`), via `GroupMembershipRecompiler::lock_affected_by_user_delete_in_tx` | also locks and recompiles the profiles of engine grants that TARGET the user (E21, amended): the grants cascade with the user, and the same transaction reaps their compiled policies, which name the username the delete frees |
| `recompile_role_referencing_profiles` (`crates/egret-perm-compile/src/role_membership.rs:251`) | engine-grant `role` principals join the boot-time role sweep |
| the policy editor's create/edit/enable/delete (`crates/egret-policy/src/admin/mod.rs`, `write_policy_checked`, `fold_profile_of_stored`) | security re-review M-a: on a profile whose engines carry impersonation grants, every editor write locks the profile's unit rows and recompiles it in the same transaction, so a target the write makes near-root (or gives a hop) loses its `perm-engine-` policy at once. Infra seeding (`seed_admin_all_access`, the delegator and per-user baselines at engine apply) does not go through the editor and does not recompile: recorded as a follow-up |

The workspace-member and tag recompile paths need no change: engine grants name no workspace and no tag.

## 5. API

All under `/api/control-plane`, in `crates/egret-perm-compile/src/routes/engine_grants.rs`, mounted from
`perm_router` (`routes.rs:120-166`) and registered in `PermApi` (`routes.rs:176-315`). Reads gated `policy:read`,
writes `policy:update`, through `egret_core::authz::require` as `list_grants` is (`routes.rs:1502`). DTOs camelCase.

### 5.1 Catalogue

`GET /perm/engine-capabilities` → `{ engines: [{ engineType, capabilities: [Capability], notGrantable: [{ what,
why }] }] }`, `Capability = { id, label, description, resources: [{ key, valueKind }], access }`.

### 5.2 Per engine

`GET /perm/engines/{engine_id}/grants` →

```text
{ engine: { id, name, type, origin, status },
  profile: { serviceId, name } | null,
  reach: { engines: [{ id, name, origin }], sharedProfile: bool },
  grants: [EngineGrantView],
  managed: [ManagedEntry] }
EngineGrantView = { id, engineId, engineName, capability, values, targetUserId, principals: [PermPrincipal], policyName, reach }
ManagedEntry    = { policyId, policyName, kind: user_baseline | delegator | admin_all_access | default_all_access,
                    capability: string | null, resources: {key: [value]}, accesses: [string],
                    users: [string], groups: [string], enabled }
```

`reach` lists every engine with `authz_service_id = profile` ordered by name (E9). `grants` lists every engine grant that reaches the engine: on a legacy shared profile that includes those written on its other engines, each marked by `engineId`/`engineName` (security review M3). `managed` lists every
`managed_by = 'infra'` policy of the profile; `capability` is the catalogue id whose resource key set equals the
policy's, else `null` (data hierarchies and `queryid`); `kind` from the name (`infra_names.rs` constants). An engine
with no profile answers `profile: null`, empty `grants`/`managed`, and 200. Unknown engine → 404.

`GET /perm/engine-grants` → every engine grant (`EngineGrantView`), for the People pages (E15).

### 5.3 Writes

`POST /perm/engines/{engine_id}/grants` with `NewEngineGrant { capability, values, principals: [PermPrincipal] }`
→ 201 `{ id, compile: CompileResponse, reach }`. `PATCH /perm/engine-grants/{id}` with `{ capability, values,
principals }` (the whole body; the engine is fixed) → 200 `{ compile, reach }`. `DELETE /perm/engine-grants/{id}` →
200 `{ compile, reach }`.

`write_engine_grant(tx, engine_id, id, body, p)`, one implementation for create and patch, in this order:

1. read the engine; 404, or 409 when it has no profile, is `risingwave`, is `deleting`, or the capability's engine
   type differs (400 for the last);
2. validate values against the catalogue (400 with the rule broken); principals non-empty, kinds `user`/`group`/
   `role`, no `public` (400);
3. `lock_named_groups` over the body's and the stored row's principals (group locks first, `routes.rs:1018-1031`);
4. `resolve_principal_refs` (`principal_refs.rs:167`), with the R6 keep-set for a patch exactly as `write_grant`;
5. `SELECT authz_service_id FROM engine_mgmt.engines WHERE id = $1 FOR SHARE`, re-read under the lock: a rebind
   (`set_engine_authz_service_in_tx`, which UPDATEs the row) now waits for this transaction, so the grant cannot be
   written against a profile the engine has just left. `FOR KEY SHARE` would not conflict with that UPDATE;
6. impersonation targets: resolve (`users.username`), refuse reserved and admin targets (409 naming the reason);
   `catalog` values: refuse a catalog the profile does not reach (400 listing the ones it does);
7. `lock_services_in_tx(tx, &[profile])` (expands with the tag service, `trigger.rs:1833`);
8. insert or update (unique violation → 409 "this engine already has that grant");
9. `recompile_services_in_tx(tx, &[profile])`.

Lock order is the J-F3 order plus the engine row: group locks, engine row, profile locks. The rebind path takes the
engine row then the profile (`engines.rs:5399`, then `:5410`), and no path takes a profile lock before an engine row, so no
cycle is added.

### 5.4 Explain (E13)

`POST /perm/explain/capability` with `{ subject: { user }, engineId, capability, values }` →
`{ engine, profile, reach, outcome: per_profile | no_profile, decision: WireDecision | null, attribution:
ExplainAttribution | null, policyName: string | null, tagResolution: "not_applicable" | "declined" }`.

Same subject rule as `ExplainSubject` (`routes.rs:455-457`): a username, no groups or roles sent. The artifact is
`served_artifact_via` of the engine's profile; `EngineConfig` as `explain_users` builds it
(`routes.rs:2207-2211`); the request is the catalogue entry's resource with `values`, the entry's access type,
`owner: None`.

Tags: a capability whose resource has no `catalog` level (`trinouser`, `systemproperty`, StarRocks `user`,
`global_function`, `system`) is asked with `resolved_tags: Some(∅)`, because egret's service tags only ever carry
`catalog`/`schema`/`table`/`column` resource elements (`crates/egret-policy-api/src/dto.rs:142-204`), so no tag can
attach to such a resource. This is argued from egret's writer, not measured: the differential bypasses the enricher
(`crates/egret-ranger-eval/tests/differential.rs:56-73`). A capability with a `catalog` level is asked with `None`
whenever the download's tag block holds a policy, and the evaluator's `TagsUnresolved` is returned as Undecidable,
`tagResolution: "declined"`.

Every Trino `trino.impersonate_user` question on a delegating profile is Undecidable (C9). The response carries the
evaluator's cause; the label to show is "egret cannot decide impersonation on this engine: its baseline lets every
user become themself through `{USER}`, which egret's evaluator does not expand".

### 5.5 Status codes

400: unknown capability, wrong engine type, a value that breaks a rule, empty or invalid principals, an unresolvable
ref. 403: missing permission. 404: no engine / no grant. 409: engine without profile, `deleting`, `risingwave`;
reserved or admin impersonation target; duplicate grant; an overlap or ceiling refusal from the recompile (existing
mappings). 500: internal.

## 6. Engine lifecycle (E10)

- `update_engine` (`engines.rs:5531-5547`): after `apply_authz_service_update` commits a rebind, recompile the
  previous profile with `recompile_profile` (fire-and-forget, logged with the repair URL, as
  `restore_engine_authz_service` at `engines.rs:5212-5226`). Not in the binding's transaction: the port recompiles
  one profile per call, each with its own `lock_services_in_tx`, so two calls would not take both locks in one
  ascending order (the shape E21 records for `external.rs`). Since I2 the former profile has no other engine, so the
  window after commit serves nobody except a legacy shared profile. External engines need nothing: their update
  already recompiles both profiles.
- `delete_engine` (`engines.rs:5953-5965`) and `delete_external_engine` (`engines.rs:5979-6011`): after the row is
  deleted, if another engine still binds the former profile (`count_engines_bound_to_authz_service`,
  `crates/egret-engine-mgmt/src/store.rs:378-389`), recompile it. With no engine left nothing downloads the profile,
  and the next bind recompiles it (`engines.rs:5367-5440`), which reaps the stale policies.

## 7. Audit hook (E18)

Each write handler calls `audit_engine_grant_write(&p, action, id, &detail)` after commit, where `action` is
`perm.engine_grant.create|update|delete`, `resource_type` `engine_grant`, and `detail` is `{ engineId, capability,
values, principals, services }`. Today the function emits `tracing::info!(target: "egret::audit", …)` with those
fields, so it is not dead code and the shape is fixed by a unit test. #3 adds an audit sink to `PermState`; whichever
of #3 and this package merges second replaces the body with `st.audit.emit(egret_audit::mutation_for(p, None, action,
"engine_grant", Some(id), detail))`, the shape `crates/egret-audit/src/lib.rs:34-51` builds.

## 8. Testing

- **Pure (catalogue).** For every entry: each resource key exists in its engine's service-def, in hierarchy order;
  the access type is in `accessTypes`; it is in the leaf's `accessTypeRestrictions` except `trino.execute_function`,
  which is asserted to be the only exception (C2). No entry uses `role`, `sysinfo`, `queryid`, `storage_volume`,
  `resource`, `resource_group`, or a `system` access other than `blacklist`; no entry's value kind accepts `*`.
  Every `notGrantable` row names a key or access the defs declare. The capability ids equal the migration's `CHECK`
  list (read with `include_str!`).
- **Pure (compile).** The body for each capability; value-rule refusals (`*`, `?`, `{USER}`, empty, whitespace);
  `validate_user_ref` refusals; engine-type mismatch.
- **DB (`egret_test_db::url()`, scoped to the fixture's own service ids).** Migration 0195: tables, `CHECK`s,
  the rename of a pre-seeded `perm-engine-x` hand-written row (name, body name, versions bumped) and that a
  compiler-owned name is untouched. Compile: a grant compiles once into its engine's profile with
  `managed_by = 'compiler'` and `origin = 'generated'`; delete reaps it; a group grant expands to members and recompiles
  on a membership change; a role grant likewise; an empty group skips; a deleted impersonation target skips with the
  reason; the ceiling counts engine grants. Shared profile (legacy, by SQL): `reach` lists both engines and the policy
  is in the one profile. Rebind: the old profile loses the policy, the new one gains it. Delete engine: cascade, and
  the shared former profile is recompiled. References: group delete and rename refused while an engine grant names
  the group (the #692 path); `/perm/unresolved-principals` lists an engine grant's stale ref; the username allocation
  answers 409 for a stale impersonation value; the pre-mint sweep mints a grantee group member.
- **Routes with a real `CedarPdp`** (`crates/egret-perm-compile/tests/people.rs:131-172` is the pattern): every route
  403 without its permission, 200/201 with it; each 400/404/409 of §5.5; `FOR SHARE` serialises a concurrent rebind
  (the `tests/support/pausing.rs` pattern).
- **Explain.** Trino `set_system_session_property` allowed for the grantee and denied for another user;
  impersonation Undecidable on a delegating profile with the cause; StarRocks `use_global_function` allowed;
  `execute_procedure` declined when the tag block has a policy.
- **Differential (`EGRET_TEST_RANGER_E2E=1`).** A new test in `crates/egret-ranger-eval/tests/differential.rs`,
  modelled on `an_editor_saved_table_policy_grants_select_at_column_depth_on_the_real_jars` (`:8413`), asks the real
  2.5.0, 2.8.0 and 2.9.0 engines and this crate the same cases, bodies built by `compile_engine_grant`: Trino
  `trinouser` (grantee as target, as another user, case variants), `queryid` (seeded `*`, to pin C1's baseline),
  `systemproperty`, `catalog`+`sessionproperty`, `catalog`+`schema`+`procedure`, `catalog`+`schema`+`schemafunction`
  with `execute` (C2's restriction question, on the real engine); StarRocks `resource_group` (`alter` allowed by an
  explicit policy, to pin that the key evaluates), `global_function` `usage`, `system` `blacklist` and `operate` (the
  control). Each with a non-grantee control.

## 9. Out of scope

The Engine permissions tab (#4); deny; workspace scoping; RisingWave; expanding `{USER}` in the evaluator; migrating
hand-written engine-level policies (E17); listing hand-written engine-level policies per engine (F12, #4); the two
recorded gaps of E21.
