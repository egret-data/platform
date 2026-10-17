# Principal refs that match nobody: design and plan

Date: 2026-10-17. Base: main `62921269`. Finding J-F3 from the DG backend cleanup review (#684).

## Problem

`POST /api/control-plane/perm/grants` accepts `{kind: "user", ref: "<user UUID>"}`. The compiler writes the ref as
the Ranger user name, which no engine principal carries, and nothing reports it. The same happens with a typo, or
with a group or role name that does not exist.

- On an **allow** include this fails closed: the grant grants nobody.
- On a **deny** include, an **allow** grant's `principals.exclude`, or a rule's `exempt.exclude`, it fails
  **open**. The deny or the subtraction silently evaporates and the compiled policy allows more than the model
  states.
- On a **deny** grant's `principals.exclude` it fails closed: the exclude compiles to a `denyExceptions` item, and an
  exception naming nobody makes the deny wider than written (corrected in the final review; the first draft said
  "any exclude").
- On a rule's `exempt.include` it fails closed (the mask applies to someone the author meant to exempt), which is
  still wrong.

`egret-perm::actions::CompileError::UnusableUserRef` already says where the fix belongs: "canonicalise at the
authoring boundary, where `users.username` is readable, not in the compiler". It catches only an empty ref or one
containing `@`.

## Decisions

| id | decision | by |
|---|---|---|
| R1 | Validate every principal ref at the authoring boundary (`write_grant`, `write_rule` in `egret-perm-compile/src/routes.rs`), on create, patch and preview, for include AND exclude. An unknown ref is a 400 naming the kind and ref | default decision (2026-10-17) |
| R2 | `user`: the ref must equal an existing `users.username` (citext, so match case-insensitively and store the canonical stored spelling). A ref that is a user's UUID or email gets a 400 that says to use the username, and names it when the lookup by id/email finds one | default decision (2026-10-17) |
| R3 | `group`: the ref must name a group egret serves to Ranger (find the source the user-sync / Ranger download uses for group names; the `public` group is always valid). `role`: the same for roles, using the source the role download uses. If roles are deliberately free text (a Ranger role egret does not own), document the evidence and keep the check to what egret can read; do not invent one | default decision (2026-10-17) |
| R4 | `workspace_members`: the ref must name an existing workspace (it is expanded at compile time) | default decision (2026-10-17) |
| R5 | Existing rows are not rewritten. A read-only report lists stored grant/rule principals whose ref resolves to nothing, flagged `fails_open` when it is a deny include, an allow grant's exclude, or a rule exemption's exclude (a deny grant's exclude is a deny exception and fails closed). Expose it the way `GET /perm/overlaps` exposes pre-existing overlaps (same permission), and log one `warn!` per such row at startup | default decision (2026-10-17) |
| R6 | A principal that is deleted AFTER a valid write is not a defect: no one carries the name any more, so a deny or exclude on it still denies everyone who carries it. The report does flag it (R5), so the author can clean up; writes that do not touch it are not refused | default decision (2026-10-17) |
| R8 | A `group` principal is expanded at compile time into its members' stored usernames (`user_groups` → `users.username`, minted as `members.rs` mints), the way SP4c expanded roles. Every writer of `user_groups` triggers the recompile of the affected profiles in the same transaction, as `role_membership.rs` does for roles. `public` stays a Ranger group (engines resolve it). Reason: D7 measured that no egret engine ever sees a group, so a group in a deny include or an exclude fails open, and an allow on a group grants nobody while `explain` says it does | user, 2026-10-17 |
| R7 | The frontend `PrincipalPicker` free-text role input shows the server's 400 inline; no new UI otherwise | default decision (2026-10-17) |

## Plan

### Task 1: validate refs at the authoring boundary

- Read how each kind is resolved downstream (the Ranger user/group/role download, `members.rs`, `role_membership.rs`,
  `eval.rs:311-315`) and pick the source table per kind. Cite file:line in the module doc.
- One function, e.g. `resolve_principal_refs(tx, &mut PrincipalSet) -> Result<(), PermHttpError>`, called from
  `write_grant` and `write_rule` before `validate_*_shape`; it canonicalises user spelling (R2) and refuses unknown
  refs (R1–R4). Refuse with a 400 body naming every unknown `(kind, ref, position)`, not just the first.
- DB tests (`DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`): UUID user ref → 400 that names the
  username; email → 400; unknown group / role / workspace → 400; deny include and exclude refused the same way; a
  rule exemption refused; a valid username in a different case is stored canonical; patch and preview refused too;
  `public` accepted; the existing route tests stay green.
- Frontend: `PrincipalPicker` / `GrantDialog` / `RuleDialog` show the 400 text (check they already do; add a
  vitest if a path drops it).

### Task 1b: expand groups to their members (R8)

- Read `members.rs` (role expansion, `role_usernames`), `role_membership.rs` (write + recompile in one
  transaction, `affected_services_for_role_membership`), `grants.rs:158` (where a group goes into the Ranger `groups`
  bucket today), `rules.rs:731` (a rule exemption's group), `clearance.rs` (does a clearance name groups?), and every
  writer of `user_groups` (`crates/egret-auth/src/groups.rs`, OIDC/SSO group sync in `egret-auth`, admin routes,
  SCIM if any). List every writer in the report with file:line.
- Expand a non-`public` group into its members' usernames on both include and exclude, for grants and rule
  exemptions, through one enumeration function used by every consumer. A group with no members expands to nobody:
  on an include that compiles to no item (fails closed); on an exclude it subtracts nobody, which is correct because
  nobody is in it.
- Make every `user_groups` writer recompile the affected profiles in the same transaction (one affected-service
  function, the union of all consumers, as `role_membership.rs` argues). A writer that cannot recompile in-transaction
  (for example an OIDC login path) must enqueue a recompile the way the codebase already does for a deferred one;
  find that mechanism and cite it, or report NEEDS_CONTEXT.
- `explain` (`egret_perm::eval::matches_principal`) already matches groups by membership; after expansion the engine
  agrees with it. Add a differential/unit test that a group grant and a deny on a group compile to the members'
  usernames.
- DB tests: allow on group → members' usernames in the compiled item; deny on group and exclude on group → members
  named in the deny / the exception; adding and removing a member recompiles (the compiled policy changes in the
  same transaction); `public` unchanged; empty group.
- PR note: an allow grant on a group that granted nobody before now grants its members.

### Task 2: the report of stored refs that match nobody

- `GET /api/control-plane/perm/unresolved-principals` (same permission as `/perm/overlaps`): every stored grant
  principal and rule exemption whose ref resolves to nothing (after Task 1b, groups resolve through membership; a group with no members is listed as `empty`, not as unresolved), with grant/rule id, kind, ref, position
  (include/exclude), effect, and `failsOpen`.
- Startup `warn!` per row (where the overlap startup pass or the perm startup lives).
- DB tests: a seeded UUID-ref deny grant and an allow grant's exclude are reported `failsOpen: true`; a deny grant's exclude `false`; an allow include `false`;
  a valid ref not reported.
- Regenerate `openapi.json` and the client (`cargo build -p egret-bin && target/debug/egret openapi > openapi.json`,
  then `cd frontend && npm run gen:api`).

### Gate

Whole-branch review; full local CI (fmt, workspace clippy, shipping-resolution, `cargo test --workspace`,
`just test-ranger-gates`, frontend lint/tsc/vitest/build/bundle). No live engine run is needed: the defect and the
fix are in the authoring layer, and the compiled output for a valid ref is unchanged.
