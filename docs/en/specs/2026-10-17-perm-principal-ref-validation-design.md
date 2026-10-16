# Principal refs that match nobody: design and plan

Date: 2026-10-17. Base: main `62921269`. Finding J-F3 from the DG backend cleanup review (#684).

## Problem

`POST /api/control-plane/perm/grants` accepts `{kind: "user", ref: "<user UUID>"}`. The compiler writes the ref as
the Ranger user name, which no engine principal carries, and nothing reports it. The same happens with a typo, or
with a group or role name that does not exist.

- On an **allow** include this fails closed: the grant grants nobody.
- On a **deny** include, or on any **exclude** (a grant's `principals.exclude`, a rule's `exempt.exclude`), it
  fails **open**. The deny or the subtraction silently evaporates and the compiled policy allows more than the model
  states.
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
| R5 | Existing rows are not rewritten. A read-only report lists stored grant/rule principals whose ref resolves to nothing, flagged `fails_open` when it is a deny include or any exclude. Expose it the way `GET /perm/overlaps` exposes pre-existing overlaps (same permission), and log one `warn!` per such row at startup | default decision (2026-10-17) |
| R6 | A principal that is deleted AFTER a valid write is not a defect: no one carries the name any more, so a deny or exclude on it still denies everyone who carries it. The report does flag it (R5), so the author can clean up; writes that do not touch it are not refused | default decision (2026-10-17) |
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

### Task 2: the report of stored refs that match nobody

- `GET /api/control-plane/perm/unresolved-principals` (same permission as `/perm/overlaps`): every stored grant
  principal and rule exemption whose ref resolves to nothing, with grant/rule id, kind, ref, position
  (include/exclude), effect, and `failsOpen`.
- Startup `warn!` per row (where the overlap startup pass or the perm startup lives).
- DB tests: a seeded UUID-ref deny grant and an exclude are reported `failsOpen: true`; an allow include `false`;
  a valid ref not reported.
- Regenerate `openapi.json` and the client (`cargo build -p egret-bin && target/debug/egret openapi > openapi.json`,
  then `cd frontend && npm run gen:api`).

### Gate

Whole-branch review; full local CI (fmt, workspace clippy, shipping-resolution, `cargo test --workspace`,
`just test-ranger-gates`, frontend lint/tsc/vitest/build/bundle). No live engine run is needed: the defect and the
fix are in the authoring layer, and the compiled output for a valid ref is unchanged.
