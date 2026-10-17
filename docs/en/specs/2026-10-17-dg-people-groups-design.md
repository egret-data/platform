# Data Governance frontend redesign #2: People & groups — design

Date: 2026-10-17. Status: draft, written on the programme frame (decisions F1–F12 of
`docs/en/specs/2026-10-16-dg-frontend-redesign-design.md`, "programme spec" below). Base: main `0741a48f` (#1, the
Data area, merged as #687; J-F3 merged as #685). Branch `feat/dg-people-groups`, worktree `egret-dgpg`.

Part 1 of the programme spec gives this sub-project one row: **People & groups — a user, group or role and their
effective access — replaces Explain access; the flat Data permissions and Mask & filter rules lists.** This document
specifies it. Every claim about existing code carries a `file:line` from `0741a48f`.

## Questions for the user

The plan is written to the recommended answer of each question. Tasks that depend on an answer say so; they are not
dispatched until the question is answered.

**Q1 (security: a new write that allocates an engine identity).** D9 from #1's live check: a user who never queried
has no `users.username`, so the principal picker cannot offer them (`frontend/src/components/perm/PrincipalPicker.tsx:239-255`),
and grant writes refuse a user ref that is not a stored username (`crates/egret-perm-compile/src/principal_refs.rs:35-44`).
The username is allocated lazily by `ensure_username_via` (`crates/egret-auth/src/users.rs:426`) at first query,
notebook or engine token, and also by the compiler for members of a referenced group or workspace
(`crates/egret-perm-compile/src/members.rs:56-64`). Fixing D9 needs a write: allocate the username when a steward
picks the user.
- **Recommended:** a new `POST /api/control-plane/perm/people/users/{user_id}/username`, gated `policy:update` (the
  permission every grant and rule write needs), which calls `ensure_username_via` in a transaction and **refuses with
  409** when a stored grant principal or rule exemption already names the allocated username as a `user` ref. Such a
  row is a stale ref that matches nobody today (`GET /perm/unresolved-principals`,
  `crates/egret-perm-compile/src/routes.rs:1580-1604`); allocating the name would make it start applying to this
  person silently. The same thing can already happen at the person's first query; the guard makes the steward's
  path refuse instead.
- **Alternative:** no new write. The picker keeps refusing such a user and says why ("egret allocates a username at
  the user's first query"); D9's second half stays open.

**Q2 (security: what `policy:read` reveals).** A principal page shows group members, role holders and workspace
memberships. Today those are read through Control Plane routes gated `user:read`, `group:read` and `role:read`
(`crates/egret-controlplane/src/access/mod.rs:191`, `:571`, `:720`, `:963`, `:1221`), which a steward holding only
`policy:read` lacks.
- **Recommended:** the new People routes (§4) are gated `policy:read`, the permission of every page this sub-project
  replaces (`frontend/src/App.tsx:531-533`). They return memberships only as far as the compiler and the clearance
  check use them (§3), never passwords, sessions, permissions or login data. Rationale: `policy:read` already
  reveals the members of every group, role and workspace a grant names, because the compiled Ranger policies list
  them as usernames and `GET /policies/services/{id}/policies` serves them under `policy:read`
  (`crates/egret-policy/src/admin/mod.rs:627-632`, `:670-675`); it also already lists users and groups
  (`/api/control-plane/principals`, `crates/egret-policy/src/admin/mod.rs:1063-1080`).
- **Alternative:** `policy:read` plus the Control Plane read permission of each section (members need `group:read`,
  holders `role:read`, a user's memberships `user:read`); a steward without them sees grants only, with the reason.

## Decisions

| id | decision | by |
|---|---|---|
| P1 | Routes: `/governance/people` (index), `/governance/people/users/:userId`, `/governance/people/groups/:name`, `/governance/people/roles/:name`, plus `/governance/people/users/by-name/:username` (resolves to the id and replaces itself). `?tab=` selects a tab; the index keeps `?kind=` and `?q=` in the URL (FE-PAGE-2). A lazy route area with its own bundle guard (FE-ROUTE-2) | default decision (2026-10-17) |
| P2 | The area, its sidebar item ("People & groups", after Data) and every new route are gated `policy:read`. A steward holding only `tag:read` does not see it. Existing routes keep their permissions | default decision (2026-10-17), pending Q2 |
| P3 | Memberships are shown exactly as the compiler expands them (§3). Memberships egret's own authoriser counts but the compiler does not (a group's roles and workspaces) are shown and marked "egret only — not applied on engines". Expired memberships are shown and marked | default decision (2026-10-17) |
| P4 | The server returns, per principal, the list of `(kind, ref)` principals that stand for it and how (`standsFor`, §4.2). The client lists a grant or rule when one of its principals equals an entry (kind and ref, byte for byte, as the compiler joins). No membership logic in the client | default decision (2026-10-17) |
| P5 | "Can <group or role> read <object>?" evaluates each member as a user, over the same served artifacts `/perm/explain` uses. No group or role is ever sent to the evaluator. At most 200 members are evaluated; the answer says how many were not. Members with no username are listed as not evaluated. There is no aggregate verdict word | default decision (2026-10-17) |
| P6 | `public`, `workspace_members` and `user` subjects are refused by the member route with a sentence (users go to `/perm/explain`) | default decision (2026-10-17) |
| P7 | Data's "Who can read?" takes a user, a group or a role (closes #1 spec §2.2's follow-up) | default decision (2026-10-17) |
| P8 | Explain access, Data permissions and Mask & filter rules leave the sidebar and their URLs redirect to `/governance/people` (F7, the `LegacyGovernanceRedirect` pattern). The old deep links `?grant=<id>` and `?rule=<id>` redirect to new single-item pages `/governance/grants/:id` and `/governance/rules/:id`, which explain's attributions and the overlap report now link to. The three page files are deleted; tests of shared components they carried move to those components | default decision (2026-10-17) |
| P9 | Tag-selected rules get a home outside the flat list: TagDetail lists the rules that reach its tag, and Classifications lists, to `policy:read` holders, the rules whose tag no longer exists. With P8 every stored grant and rule stays listed somewhere (§5) | default decision (2026-10-17) |
| P10 | Principal pages list egret grants and rules only and say so: hand-written and egret-seeded engine policies are not listed (as #1's tabs say), and Check access gives the engines' answer. Hand-written policies per principal come with #4 (F12) | default decision (2026-10-17) |
| P11 | The index has an "Unmatched names" tab rendering `GET /perm/unresolved-principals`, fail-open rows first | default decision (2026-10-17) |
| P12 | D9a: the principal picker's list stays open after a pick while the input has focus, and reopens on typing or click | default decision (2026-10-17) |
| P13 | D9b: the picker offers "Allocate username and add" for a user with no username, through Q1's route | default decision (2026-10-17), pending Q1 |
| P14 | A principal page has **Grant access** (GrantDialog with the principal filled in, new additive `initialPrincipals`), and principal names in Data's Permissions and Masks & filters tabs link to their People page | default decision (2026-10-17) |
| P15 | A live measurement (plan Task 1) checks that group and role grants reach the engines through member expansion and that group-held roles do not, before any UI states it | default decision (2026-10-17) |
| P16 | Out of #2: expanding a group's roles and workspace memberships in the compiler; workspace pages; engine-scoped grants (F11, #4); server-side grant or rule filters; caching | default decision (2026-10-17) |

## 1. What exists

- **Explain** takes `{subject: {user}, action, target}` (`crates/egret-perm-compile/src/routes.rs:409-418`). It
  evaluates the artifact each engine downloads (`routes.rs:1923-2189`) and sends the evaluator **no groups and no
  roles** (`routes.rs:2084-2087`, `:2122-2128`). That is correct, not a gap: the compiled policies name no group
  except `public` and no role, because the compiler replaces every group (except `public`), role and workspace with
  its members' usernames before compiling (`crates/egret-perm-compile/src/members.rs:1-19`, `:335-341`;
  `is_expanded_group`, `members.rs:252-254`). Evaluating a request that carries `groups = {G}` would therefore match
  nothing G's members are granted, so a group or role answer can only be the answers of its members.
  `egret-ranger-eval`'s `AccessRequest` has `groups` and `roles` fields (`crates/egret-ranger-eval/src/eval.rs:148-151`);
  they stay empty.
- **Who a principal stands for, as the compiler reads it:**
  - group → its direct members in `user_groups` joined by `groups.name`; members with no username are minted at
    compile (`members.rs:374-394`, `:396-427`);
  - role → users holding a live, `allow`, unexpired `access_grants` row of any role with that name, in any scope;
    holders with no username are **not** named and not minted (`members.rs:146-160`, `:175-182`);
  - workspace → users with such a row scoped to the workspace (`members.rs:356-372`);
  - `public` → everyone (`members.rs:246-254`).

  Two narrowings, both fail-closed (`members.rs:66-93`): a group's own roles and workspace memberships are not
  expanded (only `principal_type = 'user'` rows are read), though egret's authoriser counts them
  (`crates/egret-auth/src/grants.rs:47-60`); and an expiry is not a write, so a lapsed membership stays in the
  compiled policy until the profile next recompiles.
- **Clearances.** A tag's gate has `required_roles` and `required_groups` (`crates/egret-controlplane/src/tags.rs:48-61`).
  The compiler enforces only the roles on engines, as a deny that excepts the roles' holders
  (`crates/egret-perm-compile/src/clearance.rs:33-58`); a gate naming only groups compiles to nothing on engines and
  is reported as skipped (`clearance.rs:61-70`, `:266-276`). egret's object-access check honours both
  (`crates/egret-auth/src/pdp/templates.rs:82-106`), and counts a workspace role only for that workspace's objects
  (`members.rs:128-139`).
- **Reading people.** `/api/control-plane/principals` (`policy:read`) lists users and groups, 50 at most, with each
  user's username or none (`crates/egret-policy/src/admin/mod.rs:1063-1120`); it lists no roles. Memberships are
  only on Control Plane routes (Q2).
- **The pages being replaced** are `policy:read` routes (`frontend/src/App.tsx:531-533`) and sidebar items
  (`frontend/src/components/layout/Sidebar.tsx:94-105`). Explain access is a subject + action + `ScopePicker` form
  over the shared `ExplainResultView` (`frontend/src/routes/governance/ExplainAccess.tsx:44-163`). Explain's
  attributions link to `/governance/data-permissions?grant=` and `/governance/mask-filter-rules?rule=`
  (`frontend/src/components/perm/ExplainResult.tsx:637`, `:651`), and the overlap report to the latter
  (`frontend/src/routes/governance/OverlapReport.tsx:201`). Data permissions ignores `?grant=` today (no
  `useSearchParams` in `DataPermissions.tsx`); Mask & filter rules opens `?rule=` for editing
  (`frontend/src/routes/governance/MaskFilterRules.tsx:58-61`).
- **Redirect pattern:** `LegacyGovernanceRedirect` (`frontend/src/routes/governance/LegacyGovernanceRedirect.tsx`)
  navigates with `replace` to an absolute target and keeps search and hash.
- **Unmatched refs:** `GET /perm/unresolved-principals` (`policy:read`, `routes.rs:1580-1604`) lists stored refs
  that match nobody with `failsOpen` (`principal_refs.rs:322-350`). No page renders it.
- **D9a** (#1 live doc §2.4): `add` closes the list (`PrincipalPicker.tsx:149-155`) and only `onFocus` opens it
  (`:230`), so a second pick needs a blur and refocus.
- **Tag deletion leaves tag rules behind:** `delete_tag` deletes the tag only (`crates/egret-controlplane/src/tags.rs:217-224`);
  such a rule shows `tag_name: null` (`RuleView`, `routes.rs:347-356`), which #1 calls unresolved
  (`frontend/src/routes/governance/data/coverage.ts:108-110`).

## 2. Routes and pages

**Index `/governance/people`** (`ListPageShell`, FE-PAGE-1). Tabs **Users · Groups · Roles · Unmatched names**,
search box (`?q=`), paging. Users show name, email and username (or "No username yet"); groups their source and
member count; roles their scopes and holder count. Toolbar: **Check access**, a dialog with subject kind (user,
group, role), subject picker, action and `ScopePicker`: Explain access in a dialog.

**Principal page** (header + Radix tabs, as #1's object page):

| kind | header | tabs |
|---|---|---|
| user | name, email, username (or "No username yet" with how one is allocated), status; Grant access; Manage in Control Plane (only with `user:read`) | Overview · Permissions · Masks & filters · Clearances · Check access |
| group | name, source (local / OIDC: OIDC memberships change at login), member count; Grant access | Overview · Permissions · Masks & filters · Clearances · Check access |
| role | name, scopes it exists in; Grant access | Overview · Permissions · Clearances · Check access |
| `public` | "Everyone. Engines resolve `public` themselves; it has no members to list." | Permissions · Masks & filters |

- **Overview.** User: groups, roles held, workspaces. Group: members (username or "No username yet: a grant naming
  this group allocates one at compile"), roles and workspaces the group holds (egret only). Role: users holding it
  (a holder with no username is marked "not named on engines until they have a username", `members.rs:146-160`),
  groups holding it (egret only). Each membership shows its scope, its expiry, and whether engines apply it.
- **Permissions.** Every egret grant one of whose principals stands for this principal, grouped by how it reaches:
  **Directly**, **Through a group**, **Through a role**, **Through a workspace**, **Everyone (`public`)**, then
  **Excluded** (the principal is in the grant's exclude) and **egret only** (through a group's role or workspace:
  "not applied on engines"). Deny marked. Facets for effect and action (what Data permissions had). Edit and delete
  reuse `GrantDialog` and the delete confirmation; workspace-assignment grants are read-only, as everywhere.
- **Masks & filters.** Rules whose exemption names this principal, by the same reach groups. One line states the
  rest: "N other rules apply to everyone they do not exempt; each object's Masks & filters tab lists them."
- **Clearances.** Every gated tag with two answers: **Engines** — cleared (through role R), not cleared, or "this
  gate names only groups, so engines do not enforce it"; **egret objects** — cleared (through role R or group G) or
  not, noting that a workspace role clears only that workspace's objects.
- **Check access.** Action + `ScopePicker`. User: `/perm/explain`, rendered with `ExplainResultView`. Group or role:
  §4.3, a table of members with each profile's verdicts, three counts ("allowed by every profile", "refused by at
  least one", "undecidable on at least one"), a filter for refused, and each member's full `ExplainResultView` on
  expand. A user with no username: "No username yet, so no engine knows this user"; nothing is asked.

**Single items.** `/governance/grants/:id` and `/governance/rules/:id`: the sentence, scope or selector, principals
(each linked to its People page), Open in Data (or the tag for a tag rule), Edit and Delete with `policy:update`.
A missing id says "No grant has this id; it may have been deleted."

## 3. Honesty rules

- Never a partial answer as complete: a member list over the limit says how many were not evaluated; a member with
  no username is listed, not dropped; a failed read shows its error, never an empty list (FE-STATE-1).
- Grant and rule lists say they are egret's only (P10) and point at Check access.
- Each membership says whether engines apply it, with the reason when not: held through a group, expired (engines may
  apply it until the next recompile), or no username.
- A group's grants are shown as what they are on engines: grants to the group's members, following membership
  (the picker's sentence, `PrincipalPicker.tsx:284-299`).
- Only explain evaluates (F10). The page never derives "can read" from grants.

## 4. Backend

All new routes live in `egret-perm-compile` beside `/perm/explain` and the expansion they mirror, are read-only
except §4.4, and are gated `policy:read` (pending Q2) with `egret_core::authz::require`, as `list_grants` is
(`routes.rs:1400-1404`).

### 4.1 One definition of membership

The SQL that `role_usernames` and `expand_enumerated_principals` run to find role holders, workspace members and group
members (`members.rs:175-182`, `:357-367`, `:383-388`) moves into shared functions in `members.rs` that both the
compiler and the People reads call. A test asserts, over one fixture, that the users whose `standsFor` names a
group, role or workspace are exactly the usernames the expansion emits for it.

### 4.2 Reads

- `GET /api/control-plane/perm/people?kind=user|group|role&q=&username=&limit=&offset=` → `{ items, total }`.
  Each item: `kind`, `key` (user id, group name, role name), `label`, `detail` (email, source, scopes), `username`,
  `memberCount`. `username=` is an exact lookup (citext) for P1's by-name route.
- `GET /api/control-plane/perm/people/users/{user_id}`, `/groups/{name}`, `/roles/{name}` → the principal: identity;
  memberships (each with `scope`, `expiresAt`, `expired`, `via`, `onEngines`, `notOnEngines` ∈ `held_through_group`,
  `expired`, `no_username`); `standsFor: [{ kind, ref, via, onEngines, notOnEngines }]`; `clearances`. A name no
  group or role has → 404 with a sentence; `public` → the `public` shape.

### 4.3 Explain for a group or role

`POST /api/control-plane/perm/explain/principal` with `{ principal: { kind: group|role, ref }, action, target }` →
`{ membersTotal, limit, evaluated: [{ userId, label, username, result: ExplainResponse }], notEvaluated: [{ userId,
label, reason: "no_username" }], truncated }`.
- Members come from §4.1, read by the server; nothing about membership is caller-supplied (the rule of
  `ExplainSubject`, `routes.rs:357-408`).
- `explain`'s body is split so the per-profile artifact, service-def, catalog name and tag resolution are computed
  once and evaluated per username; `/perm/explain` calls the same function with one user and answers byte for byte
  as before.
- The same target validation and overlay confinement as `/perm/explain` (`routes.rs:1928-1931`).
- `public`, `workspace_members`, `user`, an unknown name → 400 with a sentence. Members sorted by username;
  the first 200 evaluated.

### 4.4 Allocate a username (pending Q1)

`POST /api/control-plane/perm/people/users/{user_id}/username`, gated `policy:update` → `{ username, allocated }`.
Idempotent. In one transaction: `ensure_username_via`; if the user had none and any `perm.grant_principals` or
`perm.rule_exemptions` row has `kind = 'user'` and `ref` equal to the new name, roll back and answer 409 listing
them. Logged like the compiler's mint (`members.rs:420-425`).

## 5. Transition (F7)

- Sidebar: "People & groups" after Data; Data permissions, Mask & filter rules and Explain access removed.
- `/governance/explain-access`, `/governance/data-permissions`, `/governance/mask-filter-rules` → `/governance/people`;
  with `?grant=<id>` or `?rule=<id>` → `/governance/grants/<id>` or `/governance/rules/<id>`.
- Nothing becomes unlisted: every grant has a catalog scope (`migrations/0154_perm_model.sql:24`) and is listed on
  Data's pages (#1, D4 fix) and on its principals' pages; a path rule on Data's pages; a tag rule on TagDetail; a
  rule whose tag is gone on Classifications; any grant or rule by id on its single-item page.

## 6. Frontend standards

`docs/en/frontend-rules.md` in full; Metabase design language and OKLCH tokens; `ui-ux-pro-max` for the detailed
visual design; works at 390 px; Radix Tabs; client regenerated with `npm run gen:api` only. Shared components
(`PrincipalPicker`, `GrantDialog`, `ExplainResult`, `TagDetail`) change additively (FE-TEST-3). FE-REF: this flow has
no source product in FE-REF-1's list; it follows #1's object-page structure, a deliberate choice recorded here.

## 7. Testing

- Backend DB tests on `egret_test`: §4.1's agreement test; each read's memberships, `standsFor`, expiry and
  egret-only marks; clearances for roles-only, groups-only and mixed gates; the member explain equals `/perm/explain`
  per member, truncation at 200, `notEvaluated`, every 400; the mint is idempotent, refuses on a stale ref, and needs
  `policy:update`; every route refuses without `policy:read`.
- Frontend vitest: index tabs and search, each principal tab and reach group, Check access for each kind, redirects
  and deep links, single-item pages, TagDetail and Classifications rule lists, the picker's second pick and
  allocation, Who can read? per kind, empty, error and permission states.
- Full frontend gate including a new `test:bundle:governance-people`.
- **Live (required).** Task 1 measures engines before the UI claims anything about them (P15). The final task drives
  the pages in a browser against Trino and StarRocks: a group grant and a role grant, a group-held role, a user with no
  username, Check access per kind compared with real queries, the redirects, 390 px and keyboard.
