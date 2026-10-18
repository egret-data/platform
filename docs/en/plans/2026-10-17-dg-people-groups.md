# Governance People & groups (sub-project #2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "People & groups" area in Governance: an index of users, groups and roles, a page per principal with its
memberships, the egret grants and rule exemptions that reach it (each marked how), its clearances, and "Can <principal>
read <object>?"; Explain access, Data permissions and Mask & filter rules are replaced and redirected.

**Architecture:** New read routes in `egret-perm-compile` (`/perm/people…`, `/perm/explain/principal`) beside
`/perm/explain` and the member expansion they mirror, sharing one definition of membership with the compiler; one
write (`…/username`, pending Q1). The frontend adds a lazy `routes/governance/people/` area, single-item grant and
rule pages, and small additive changes to `PrincipalPicker`, `GrantDialog`, `WhoCanReadDialog`, `TagDetail` and
`Classifications`.

**Tech Stack:** Rust (axum, sqlx, utoipa), React 18 + TypeScript, TanStack Query via the orval client, Radix UI,
vitest + Testing Library, Playwright MCP for the live check.

**Spec:** `docs/en/specs/2026-10-17-dg-people-groups-design.md` (decisions P1–P16, questions Q1–Q2) on the programme
spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (F1–F12).

## Global Constraints

- **Before dispatching Task 2:** Q1 and Q2 are answered by the user. Q2's answer sets the gate of every new read
  route (recommended: `policy:read`). If Q1 is answered "no new write", skip Task 5 and Task 13's allocation half.
- Base main `0741a48f`; branch `feat/dg-people-groups`; worktree `egret-dgpg`. Rebase on fresh main before the final
  gate and regenerate the client after the rebase.
- DB tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only. Never DB `egret`. Never
  `EGRET_ALLOW_DB_SKIP=1`.
- Live runs: own `vg-` containers on `vg-net`, throwaway DB `egret_dgpg_live` in `egret-dev-postgres-1`. Never start,
  stop or modify `egret-dev-*` or `egret-w7*-postgres`. Ports 5173, 5000, 8080 are not ours.
- An engine with `origin = external` is never dialled (commit `19d7ac2db`).
- Never send a group or role to `egret-ranger-eval`: `AccessRequest.groups` and `.roles` stay empty
  (`crates/egret-perm-compile/src/routes.rs:2122-2128`). Never accept caller-supplied membership.
- Never widen an existing route's permission. New routes use `egret_core::authz::require` like `list_grants`
  (`routes.rs:1404`).
- No migration is expected. If one becomes necessary, take the next free number on fresh main and re-check before
  merge (migration numbers are a shared namespace).
- Frontend: `docs/en/frontend-rules.md` in full; egret + jsx-a11y lint; never add a file to `eslint-rules/legacy.js`;
  no prettier; OKLCH tokens as `var(--token)`; regenerate with `cd frontend && npm run gen:api` only, after
  `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`.
- Shared components change additively (FE-TEST-3): `PrincipalPicker`, `GrantDialog`, `ExplainResult`, `TagDetail`,
  `Classifications`, `WhoCanReadDialog`.
- Frontend gate per task: `npm run lint`, `npx tsc -b`, `npx vitest run <touched tests>`. Full gate at the end.
- Rust per task: `cargo test -p egret-perm-compile` (plus any crate touched),
  `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all -- --check`.
- Never `git stash`, `git checkout -- <file>`, `git clean`. Kill only your own PIDs. ≥ 30 GiB free disk. Never commit
  `.superpowers/`. Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- UI copy is English, short and factual. Empty states state the fact. A partial answer always says it is partial.

## File structure

Backend (`crates/egret-perm-compile/src/`):
- `members.rs` — the membership queries become shared `pub(crate)` functions (Task 2).
- `people.rs` — principal reads: list, user, group, role, `standsFor`, clearances (Tasks 2–3).
- `routes/people.rs` — the `/perm/people…` routes, the member explain route and the username route, mounted from
  `routes.rs`'s router and OpenAPI (`routes.rs:115-151`, `:160-240`) (Tasks 2–5).
- `routes.rs` — `explain`'s body split into a per-user evaluation function (Task 4).
- Tests: `crates/egret-perm-compile/tests/people.rs`, `tests/explain_principal.rs`, `tests/allocate_username.rs`.

Frontend (`frontend/src/routes/governance/people/`):
- `PeopleRoutes.tsx` (area routes), `PeopleIndex.tsx`, `PrincipalPage.tsx`, `PrincipalHeader.tsx`,
  `tabs/OverviewTab.tsx`, `tabs/PermissionsTab.tsx`, `tabs/MasksFiltersTab.tsx`, `tabs/ClearancesTab.tsx`,
  `tabs/CheckAccessTab.tsx`, `CheckAccessDialog.tsx`, `GrantPage.tsx`, `RulePage.tsx`, `ByNameRedirect.tsx`,
  `reach.ts`, `__tests__/`.
- Statically importable (outside the lazy guard, like `data/objectPath.ts`): `people/peopleUrl.ts`.
- Shared: `components/perm/MembersExplainView.tsx`, `routes/governance/ruleMatch.ts` (moved from `data/coverage.ts`).
- Modify: `App.tsx`, `components/layout/Sidebar.tsx`, `components/perm/PrincipalPicker.tsx`,
  `components/perm/ExplainResult.tsx`, `routes/governance/GrantDialog.tsx`, `routes/governance/OverlapReport.tsx`,
  `routes/governance/TagDetail.tsx`, `routes/governance/Classifications.tsx`, `data/WhoCanReadDialog.tsx`,
  `data/tabs/PermissionsTab.tsx`, `data/tabs/MasksFiltersTab.tsx`, `permFormat.ts`, `frontend/package.json`,
  new `frontend/scripts/assert-governance-people-lazy.mjs`.
- Delete: `routes/governance/{ExplainAccess,DataPermissions,MaskFilterRules}.tsx` and their tests, after Task 11
  moves what tests shared code.

---

### Task 1: live measurement — do group and role grants reach the engines as the UI will say?

No product code. Decides whether the "applied on engines" marks of P3 are true (P15). J-F3 (#685) shipped group
expansion without a live run.

- [ ] Write predictions first: append `## Measurements (Task 1)` with `### 1.0 Predictions` to a new
  `docs/en/plans/2026-10-17-dg-people-groups-live.md`, commit, before any container starts. Predict each cell.
- [ ] Stack as `docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.2 and §2.2 (own `vg-` containers, image from
  this branch, DB `egret_dgpg_live`, Trino 483 JWT + StarRocks 4.1.6 with Ranger, one egret Iceberg catalog `ice`
  with table `s.t`, attached to both).
- [ ] Fixture (through the API where one exists; SQL only for what has no API, recorded as DEV-n):
  - group `g1` with users `ua` (has a username) and `ub` (no username); user `uc` outside it;
  - role `r1` held directly by `ud`; group `g2` holding `r1`, with member `ue` who does not hold `r1` directly;
  - user `uf` with no username holding `r1` directly;
  - allow grants: read `ice.s.t` to group `g1`; read `ice.s.t` to role `r1`.
- [ ] Cells (each a real query on both engines as that user, JWT per #1 live doc DEV-4, plus `/perm/explain` for
  each user):
  - C1 `ua` reads; `uc` does not.
  - C2 `ub` got a username at compile (`users.username` set) and reads.
  - C3 `ud` reads; `ue` does not (a group's role is not expanded, `members.rs:74-88`).
  - C4 `uf` is not named in the compiled policy (`role_usernames` does not mint, `members.rs:146-160`).
  - C5 remove `ua` from `g1`: `ua` loses read within one poll.
  - C6 a tag gate with only `required_groups = [g1]` on a tagged column: `uc` still reads it on both engines.
  - C7 `/perm/explain` agrees with every engine answer above.
- [ ] If C1, C2, C3's first half, C5 or C7 fails: **STOP, report BLOCKED** with the cell; it is a compiler defect
  outside #2.
- [ ] Tear down (containers, network, DB, image, secrets in the scratchpad), record `### 1.1 Result` and
  `### 1.2 Stack and deviations`, commit `docs(dg): People & groups live measurement (Task 1)`.

---

### Task 2: one membership definition, and the people list

**Files:**
- Modify: `crates/egret-perm-compile/src/members.rs`
- Create: `crates/egret-perm-compile/src/people.rs`, `crates/egret-perm-compile/src/routes/people.rs`
- Modify: `crates/egret-perm-compile/src/routes.rs` (router, OpenAPI), `src/lib.rs` (module)
- Test: `crates/egret-perm-compile/tests/people.rs`

**Interfaces (produces):**

```rust
// members.rs — the three reads expand_enumerated_principals and role_usernames run today
// (members.rs:175-182, :357-367, :383-388), now called by both the compiler and people.rs.
pub(crate) async fn role_holders_via(conn, roles: &[String]) -> Result<Vec<RoleHolderRow>, CompileError>;
pub(crate) async fn workspace_members_via(conn, ids: &[Uuid]) -> Result<Vec<MemberRow>, CompileError>;
pub(crate) async fn group_members_via(conn, names: &[String]) -> Result<Vec<GroupMemberRow>, CompileError>;

// routes/people.rs
// GET /api/control-plane/perm/people?kind=user|group|role&q=&username=&limit=&offset=
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct PeoplePage { pub items: Vec<PersonRow>, pub total: i64 }
pub struct PersonRow {
    pub kind: PersonKind,              // user | group | role
    pub key: String,                   // user id | group name | role name
    pub label: String,                 // name, else email | group name | role name
    pub detail: Option<String>,        // email | "local"/"oidc" | "platform, workspace"
    pub username: Option<String>,      // users only
    pub member_count: Option<i64>,     // groups: direct members; roles: live allow holders (users)
}
```

- [ ] **Step 1: failing tests:**
  - the compiler's output is unchanged: the existing `group_grants`, `role_grants` and `workspace_members` tests pass
    untouched after the refactor (run them first, green, then refactor);
  - list users with and without a username; `q` matches name, email and username; `username=` is exact and
    case-insensitive; groups with member counts; roles listed once per name with every scope in `detail` and the
    holder count `role_holders_via` gives; `limit`/`offset` and `total`; a `kind` outside the three → 400;
  - 403 without the gate permission (Global Constraints, Q2).
- [ ] **Step 2: run** `cargo test -p egret-perm-compile --test people` → fails.
- [ ] **Step 3: implement.** Move the SQL, do not copy it. `limit` default 50, max 200.
- [ ] **Step 4: run** the crate tests + clippy + fmt → pass.
- [ ] **Step 5: commit** `feat(perm): list people for Governance, on the compiler's membership reads`.

---

### Task 3: principal reads — memberships, standsFor, clearances

**Files:** `people.rs`, `routes/people.rs`, `tests/people.rs`.

**Interfaces (produces):**

```rust
// GET /perm/people/users/{user_id} · /perm/people/groups/{name} · /perm/people/roles/{name}
pub struct StandsFor {
    pub kind: PrincipalKind, pub r#ref: String,   // compared byte for byte with grant/rule principals
    pub via: Via, pub on_engines: bool, pub not_on_engines: Option<NotOnEngines>,
}
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Via { Itself, Group { name: String }, Role { name: String }, Workspace { id: Uuid, name: String },
               Public, GroupRole { group: String, role: String },
               GroupWorkspace { group: String, id: Uuid, name: String } }
pub enum NotOnEngines { HeldThroughGroup, Expired, NoUsername }
pub struct Membership { pub via: Via, pub role: Option<String>, pub scope: MembershipScope,
                        pub expires_at: Option<DateTime<Utc>>, pub expired: bool,
                        pub on_engines: bool, pub not_on_engines: Option<NotOnEngines> }
pub struct ClearanceFact {
    pub tag_id: Uuid, pub tag_key: String, pub tag_name: String,
    pub required_roles: Vec<String>, pub required_groups: Vec<String>,  // group NAMES
    pub engines: EngineClearance,  // cleared { role } | not_cleared | not_enforced_groups_only
    pub egret: EgretClearance,     // cleared { via } | not_cleared
}
pub struct UserPrincipal  { id, email, name, username: Option<String>, status,
                            groups, roles: Vec<Membership>, workspaces: Vec<Membership>,
                            stands_for: Vec<StandsFor>, clearances: Vec<ClearanceFact> }
pub struct GroupPrincipal { name, source, is_public: bool, members: Vec<MemberRef>,   // empty for public
                            roles: Vec<Membership>, workspaces: Vec<Membership>,
                            stands_for: Vec<StandsFor>, clearances: Vec<ClearanceFact> }
pub struct RolePrincipal  { name, scopes: Vec<String>, holders: Vec<HolderRef>, holder_groups: Vec<MemberRef>,
                            stands_for: Vec<StandsFor>, clearances: Vec<ClearanceFact> }
```

Rules (spec §1, §4.2), each with its citation in a doc comment:
- user `standsFor`: `user:<username>` (`Itself`, none without a username), each direct group, each directly held role,
  each workspace (all `on_engines` when unexpired and the user has a username); `group:public`; each role and workspace
  held by one of the user's groups (`on_engines: false`, `HeldThroughGroup`). Expired direct rows: listed with
  `Expired`. No username: every entry `NoUsername`.
- group `standsFor`: `group:<name>`, `group:public`, and its roles and workspaces (`HeldThroughGroup`).
- role `standsFor`: `role:<name>`.
- clearances: engines cleared iff the user directly holds a required role, unexpired, and has a username
  (`clearance.rs:33-58`, `members.rs:146-160`); `not_enforced_groups_only` when `required_roles` is empty
  (`clearance.rs:266-276`); egret cleared through a held role (direct or through a group) or a required group
  (`crates/egret-auth/src/pdp/templates.rs:82-106`, `crates/egret-auth/src/grants.rs:47-60`).

- [ ] **Step 1: failing tests:**
  - **agreement:** over one fixture with groups, roles, workspaces, expired rows and a user with no username, the users
    whose `standsFor` holds an `on_engines` entry for group G / role R / workspace W are exactly the usernames
    `expand_enumerated_principals` emits for G / R / W (call it on a grant naming each);
  - each `Via`, `Expired`, `HeldThroughGroup`, `NoUsername`; members and holders with and without usernames;
  - clearances for a roles-only gate, a groups-only gate and a mixed gate, for a cleared, an uncleared and a
    group-cleared user;
  - `public` returns `is_public` and no members; an unknown group or role → 404 with a sentence; an unknown user id → 404;
  - 403 without the gate permission; responses carry no password hash, session or permission data (assert the JSON
    keys).
- [ ] **Steps 2–5:** run → implement → run + clippy → commit `feat(perm): read a principal's memberships, reach and clearances`.

---

### Task 4: explain for a group or role

**Files:** `routes.rs` (split `explain`), `routes/people.rs`, `tests/explain_principal.rs`.

**Interfaces (produces):**

```rust
// POST /api/control-plane/perm/explain/principal
pub struct ExplainPrincipalRequest { pub principal: ExplainPrincipalRef, pub action: Action, pub target: Scope }
pub struct ExplainPrincipalRef { pub kind: PrincipalKind, pub r#ref: String }   // group | role only
pub struct ExplainPrincipalResponse {
    pub members_total: usize, pub limit: usize, pub truncated: bool,
    pub evaluated: Vec<MemberVerdict>,          // sorted by username
    pub not_evaluated: Vec<MemberNotEvaluated>, // reason: no_username
}
pub struct MemberVerdict { pub user_id: Uuid, pub label: String, pub username: String, pub result: ExplainResponse }
pub const EXPLAIN_MEMBER_LIMIT: usize = 200;
// routes.rs
async fn explain_users(conn, users: &[String], action: Action, target: &Scope) -> Result<Vec<ExplainResponse>, PermHttpError>;
```

- [ ] **Step 1: failing tests:**
  - for a group and a role with three members, each `evaluated[i].result` equals `POST /perm/explain` for that
    username, compared as `serde_json::Value` (never through `to_string()`, key order differs);
  - a member with no username is in `not_evaluated`, not dropped; a group with no members → `membersTotal: 0`;
  - 201 members → 200 evaluated, `truncated: true`, `membersTotal: 201`;
  - `public`, `workspace_members`, `user`, an unknown name, and an invalid target (each `validate_explain_target`
    case) → 400 with a sentence; 403 without `policy:read`;
  - `/perm/explain`'s existing tests stay green untouched.
- [ ] **Step 2: run** → fail.
- [ ] **Step 3: implement.** Move the per-profile loop of `explain` (`routes.rs:1993-2177`) into `explain_users`,
  computing artifact, service-def, catalog name, plugin version and tag resolution once per profile and evaluating
  each username; `explain` calls it with one user. Same `require`, `validate_explain_target` and
  `confine_to_overlay_workspace` as `explain` (`routes.rs:1928-1931`). Members from Task 2's functions.
- [ ] **Step 4: run** the crate tests + clippy → pass.
- [ ] **Step 5: commit** `feat(perm): explain a group or role by evaluating its members`.

---

### Task 5: allocate a username (pending Q1)

**Files:** `routes/people.rs`, `tests/allocate_username.rs`.

**Interfaces (produces):** `POST /api/control-plane/perm/people/users/{user_id}/username` →
`{ username: String, allocated: bool }`; 409 `{ error, refs: [{ source: "grant"|"rule", id, position }] }`.

- [ ] **Step 1: failing tests:** a user with no username gets one equal to what `ensure_username_via` would allocate,
  `allocated: true`; a second call returns the same, `allocated: false`; a stored grant principal or rule exemption
  `{kind: user, ref: <that name>}` (inserted directly, as a pre-J-F3 row) → 409 naming it and `users.username` still
  NULL afterwards; unknown user → 404; 403 without `policy:update` (and with `policy:read` only).
- [ ] **Step 2: run** → fail.
- [ ] **Step 3: implement.** Read `ensure_username_via` (`crates/egret-auth/src/users.rs:426`) first: if it begins
  its own transaction, call it on the route's transaction connection (it becomes a savepoint) so the 409 rolls the
  allocation back. Log as `members.rs:420-425` does. Never log anything else about the user.
- [ ] **Step 4: run** + clippy.
- [ ] **Step 5:** regenerate `openapi.json` and the client (Global Constraints); run `cargo test -p egret-server --lib`.
  Commit `feat(perm): allocate a user's engine username from Governance`.

(If Q1 is "no", do Step 5's regeneration at the end of Task 4 instead.)

---

### Task 6: the People area, index and sidebar

**Files:** create `people/{PeopleRoutes,PeopleIndex}.tsx`, `people/peopleUrl.ts`,
`frontend/scripts/assert-governance-people-lazy.mjs`, tests; modify `App.tsx`, `components/layout/Sidebar.tsx`,
`frontend/package.json`.

**Interfaces (produces):**

```ts
export type PrincipalLink =
  | { kind: "user"; userId: string } | { kind: "user"; username: string }   // by name → ByNameRedirect
  | { kind: "group"; name: string } | { kind: "role"; name: string };
export function principalUrl(p: PrincipalLink, tab?: string): string; // names and ids encoded
export function principalLinkFor(p: PermPrincipal): PrincipalLink | null; // workspace_members → null
```

- [ ] **Step 1: failing tests:**
  - `principalUrl` round-trips through the area's route params for each kind; names with `/`, `%`, space, `.`;
  - `/governance/people` renders the index under `policy:read`; is refused without it; the sidebar shows "People &
    groups" directly after Data, gated `policy:read`;
  - index tabs Users · Groups · Roles · Unmatched names in `?kind=`; `?q=` searches; paging; each row links to its
    page; a user with no username reads "No username yet"; loading, empty (and a filter miss offering to clear),
    error states (FE-STATE-1..3);
  - Unmatched names: rows from `/perm/unresolved-principals`, fail-open first, each with its reason and a link to
    `/governance/grants/:id` or `/governance/rules/:id`;
  - `ByNameRedirect`: `/governance/people/users/by-name/ana` → `/governance/people/users/<id>` with `replace`; a name
    nobody has → "No user has the username ana".
- [ ] **Step 2: run** `npx vitest run src/routes/governance/people` → fail.
- [ ] **Step 3: implement.** Lazy area as `DataRoutes` (`App.tsx:46-60`, `LazyRouteArea`); bundle guard modelled on
  `scripts/assert-governance-data-lazy.mjs`, one marker per module, and `npm run test:bundle:governance-people`.
- [ ] **Step 4: run** tests + lint + tsc + the new bundle test after `npm run build`.
- [ ] **Step 5: commit** `feat(governance): the People & groups area and index`.

---

### Task 7: principal page, header and Overview

**Files:** `people/{PrincipalPage,PrincipalHeader}.tsx`, `people/tabs/OverviewTab.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - tabs per kind exactly as spec §2's table (`public`: Permissions and Masks & filters only); `?tab=` selects;
    unknown tab → first tab; Radix keyboard;
  - header per kind; "Manage in Control Plane" only with `user:read` (user), `group:read` (group), `role:read` (role);
    a user with no username shows the sentence of spec §2;
  - Overview lists each membership with scope, expiry, and an "Applied on engines" cell that says **Yes** or the
    reason (held through a group / expired / no username) in words, never colour alone (FE-A11Y-4);
  - group members and role holders with no username carry their spec §2 sentences;
  - 404 from the API → "No group named x. Grants naming it match nobody; see Unmatched names." (and the user/role
    equivalents); other errors leave the header working.
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): principal page and overview`.

---

### Task 8: Permissions tab, reach, Grant access, links from Data

**Files:** `people/tabs/PermissionsTab.tsx`, `people/reach.ts`, modify `GrantDialog.tsx`, `permFormat.ts`,
`data/tabs/PermissionsTab.tsx`, `data/tabs/MasksFiltersTab.tsx`, tests.

**Interfaces (produces):**

```ts
export type Reach = { position: "include" | "exclude"; via: Via; onEngines: boolean; notOnEngines?: NotOnEngines };
/** Every way `standsFor` meets one of the set's principals: same kind and same ref, byte for byte. */
export function setReach(set: PrincipalSet, standsFor: StandsFor[]): Reach[];
export function reachGroup(r: Reach[]): "direct" | "group" | "role" | "workspace" | "public" | "excluded" | "egret_only";
// GrantDialog gains: initialPrincipals?: PrincipalSet   (used only when `existing` is not set)
```

- [ ] **Step 1: failing tests:**
  - `setReach` truth table: each `Via`, include vs exclude, a ref differing only by case does **not** match, a
    `workspace_members` ref matches the workspace id, an empty set;
  - `reachGroup` precedence: excluded beats included (Ranger's exclude), and `egret_only` only when every reach is
    not on engines;
  - the tab groups grants Directly / Through a group / Through a role / Through a workspace / Everyone / Excluded /
    egret only; deny marked; effect and action facets; a workspace-assignment grant read-only; edit and delete via
    `GrantDialog` gated `policy:update`; the "egret grants only" sentence and its link to Check access (P10); empty
    → "No egret grant reaches <name>";
  - Grant access opens `GrantDialog` with this principal in include (user by username; disabled with the reason
    for a user with no username);
  - Data's Permissions and Masks & filters tabs: each principal name links to its People page
    (`principalLinkFor`; a workspace stays text); existing Data tests stay green.
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): a principal's grants and how each reaches it`.

---

### Task 9: Masks & filters and Clearances tabs

**Files:** `people/tabs/{MasksFiltersTab,ClearancesTab}.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - Masks & filters: rules whose `exempt` meets `standsFor` (Task 8's `setReach`), grouped as Permissions; each rule
    links to `/governance/rules/:id` and, for a path rule, to its Data page; the line "N other rules apply to
    everyone they do not exempt…" with the right N (fixture where N ≠ total);
  - Clearances: one row per gated tag with the Engines and egret objects answers in words; groups-only gate reads
    "Engines do not enforce this gate (it names only groups)"; a workspace role's egret answer carries the
    workspace note; a tag links to its classification page only with `tag:read`; empty → "No classification
    is gated".
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): a principal's exemptions and clearances`.

---

### Task 10: Check access and Who can read? for any principal

**Files:** `people/tabs/CheckAccessTab.tsx`, `people/CheckAccessDialog.tsx`, `components/perm/MembersExplainView.tsx`,
modify `data/WhoCanReadDialog.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - user: action + `ScopePicker` → `/perm/explain`, rendered with `ExplainResultView`; a user with no username → the
    spec §2 sentence, no request;
  - group/role: `/perm/explain/principal`; `MembersExplainView` shows the three counts, a row per member with each
    profile's per-access verdicts, a "Refused only" filter, `ExplainResultView` on expand, `notEvaluated` members with
    their reason, and when `truncated` "Evaluated 200 of 201 members; the others were not asked" — fixture where an
    aggregate word would be wrong (one member allowed, one refused) and the view shows no single verdict;
  - an answer shows only under the subject it was asked for (as `WhoCanReadDialog` does today);
  - the index's Check access dialog: subject kind, subject picker, action, target; the same rendering;
  - Data's Who can read? offers user, group and role; WHO_CAN_READ_INTRO updated; its existing tests stay green.
- [ ] **Steps 2–5:** run → implement → run → commit `feat(governance): check a user's, group's or role's access`.

---

### Task 11: single-item pages, redirects, and removing the three pages

**Files:** `people/{GrantPage,RulePage}.tsx`, modify `App.tsx`, `Sidebar.tsx`, `ExplainResult.tsx`,
`OverlapReport.tsx`; move tests; delete `ExplainAccess.tsx`, `DataPermissions.tsx`, `MaskFilterRules.tsx` and their
tests.

- [ ] **Step 1: failing tests:**
  - `/governance/grants/:id` and `/governance/rules/:id`: sentence, scope or selector, principals linked, Open in
    Data (path) or the tag (tag rule, with `tag:read`), Edit and Delete with `policy:update`; an unknown id → "No grant
    has this id; it may have been deleted.";
  - redirects with `replace`: `/governance/explain-access`, `/governance/data-permissions`,
    `/governance/mask-filter-rules` → `/governance/people`; `?grant=<id>` → `/governance/grants/<id>`; `?rule=<id>` →
    `/governance/rules/<id>`;
  - explain attributions and the overlap report link to the single-item pages;
  - the sidebar no longer lists the three items (update `DataGovernanceEmptyPages.test.tsx` and
    `ProfileDetailPage.test.tsx:498`).
- [ ] **Step 2:** move every test in `ExplainAccess.test.tsx` that exercises `ExplainResultView` or
  `SubjectUserField` to `components/perm/__tests__/ExplainResult.test.tsx`, rendering the component directly; the
  form-level ones become Check access tests (Task 10's files). List in the report which tests moved where and which
  were dropped because they tested only the deleted page. Same for `DataPermissions.test.tsx` and
  `MaskFilterRules.test.tsx` (shared: `GrantDialog`, `RuleDialog`, `permFormat`, `openInData`).
- [ ] **Steps 3–5:** implement → run every touched test + lint + tsc → commit
  `feat(governance): replace Explain access and the flat grant and rule lists with People & groups`.

---

### Task 12: tag rules on TagDetail, orphan rules on Classifications

**Files:** create `routes/governance/ruleMatch.ts`; modify `data/coverage.ts`, `TagDetail.tsx`, `Classifications.tsx`,
tests.

- [ ] **Step 1: failing tests:**
  - `globMatch` and `tagUnresolved` move unchanged from `data/coverage.ts:199` and `:108` to `ruleMatch.ts`;
    `coverage.ts` imports them; its tests stay green; the Data bundle guard still passes;
  - TagDetail, with `policy:read`: "Mask and filter rules" lists every rule whose `tag_name` glob-matches this tag's
    name (a rule written on another tag of the same name is listed and says so), each linked to its rule page; hidden
    without `policy:read`;
  - Classifications, with `policy:read`: a callout lists rules whose tag no longer exists (`tag_name` null), each
    linked to its rule page; absent when there are none.
- [ ] **Steps 2–5:** run → implement → run + `npm run test:bundle:governance-data` → commit
  `feat(governance): list tag rules on their tag and rules whose tag is gone`.

---

### Task 13: PrincipalPicker D9

**Files:** `components/perm/PrincipalPicker.tsx`, `components/perm/__tests__/PrincipalPicker.test.tsx`.

- [ ] **Step 1: failing tests:**
  - D9a: focus the user search, pick one user, pick a second without blurring: both chips present; typing reopens
    the list; Escape closes it;
  - D9b (pending Q1): a user with no username shows "Allocate username and add" (a button, keyboard reachable) only
    when the viewer holds `policy:update`; clicking calls the allocation route, then adds `{kind: user, ref: <name>}`
    and shows "stored as <name>"; a 409 shows the server's message beside the field (FE-FORM-4) and adds nothing;
    without `policy:update` the row keeps "no allocated username yet" and says how one is allocated.
- [ ] **Steps 2–5:** run → implement → run (and `GrantDialog`, `RuleDialog` tests) → commit
  `fix(governance): pick several principals in a row, and allocate a username to grant a new user`.

---

### Task 14: live browser check

- [ ] Predictions first in `2026-10-17-dg-people-groups-live.md` `## 2.` (commit before the stack starts).
- [ ] Stack and fixture as Task 1; vite dev server on a free port (not 5173/5000/8080), logged in as the admin, and
  once as a steward holding `policy:read` and `policy:update` only.
- [ ] With Playwright MCP:
  - index: each tab, search, Unmatched names (seed one pre-J-F3 row by SQL, recorded as DEV-n);
  - `ua`, `ue`, `ub`, `uf`, `g1`, `g2`, `r1` pages: memberships and their engine marks match Task 1's results;
    Permissions groups; Masks & filters with an exemption through a group; Clearances for the C6 gate;
  - Check access for `ua`, `g1` and `r1` on `ice.s.t`, compared with real queries on both engines;
  - Who can read? on the Data table page with a group;
  - Grant access from a user page; allocate a username for a new user from the picker and grant them (if Q1 yes);
    two picks in a row;
  - the three old URLs and both deep links; an explain attribution link; TagDetail's rule list;
  - 390 px, dark mode, keyboard-only through index, tabs and Check access. Screenshots to the scratchpad, checked
    with `sips`.
- [ ] Record every cell in `## 2.` with verdicts; defects are fixed in a fix round, not here.
- [ ] Tear down; commit the doc.

---

## Final gate (controller)

Whole-branch review on the most capable model; one fix dispatch + scoped re-review; rebase on fresh main; Rust gate
(`cargo fmt --check`, workspace clippy `-D warnings`, `just test-shipping-resolution`,
`cargo test --workspace --no-fail-fast` with the shared store, `just test-ranger-gates`); frontend gate
(`npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, all bundle tests including
`test:bundle:governance-people` and `test:bundle:governance-data`).
