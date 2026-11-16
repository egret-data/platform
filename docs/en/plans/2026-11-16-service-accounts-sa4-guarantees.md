# Service accounts SA4 — Guarantees + live — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make "a service account can never hold `platform_admin`" airtight at a chokepoint; pin the other SA guarantees with tests; document the catalog-vs-DG semantics; and live-check the lot. Final package of the programme.

**Architecture:** Enforce the `platform_admin` invariant inside the two shared write functions every route funnels through — the role-assignment recompile (`egret-perm-compile`) and group-member add (`egret-auth`) — so the human routes, the SA routes, and future callers are all covered with no TOCTOU window. Then tests, docs, and a live verification.

**Tech Stack:** Rust, sqlx (Postgres), axum.

## Global Constraints
- Branch `feat/svc-accounts-sa4` (worktree `../egret-sa4`, off main `a641f95e3`). `CARGO_TARGET_DIR=target-sa4` on EVERY cargo command. Throwaway Postgres `DATABASE_URL=postgres://egret:egret@localhost:5471/egret` — DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sa4/` not gitignored — never `git add -A`). Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- No schema change expected (no migration). Run `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before each commit.
- The invariant must be enforced for the `platform_admin` role ONLY, on ASSIGN/ADD only — never on revoke/remove (that direction fails open and must stay permissive so a mistaken grant can always be undone).

---

### Task 1: The `platform_admin` chokepoints (role-assignment + group-membership)

**Files:**
- Modify: `crates/egret-perm-compile/src/role_membership.rs` (the `with_recompile` / `with_recompile_in_tx` inner fns, ~:190/:204, and the `Op::Assign` path which already resolves the role `name`)
- Modify: `crates/egret-auth/src/groups.rs` (`GroupRepo::add_member` / `change_member`, ~:1119/:1153)
- Modify (only if needed to map the new error to 400/409): the routes that call these — `crates/egret-controlplane/src/access/mod.rs` (`assign_user_role` ~:518, `add_group_member` ~:857) and `crates/egret-controlplane/src/access/service_accounts.rs` (its role/group routes). The SA route's own platform_admin checks (service_accounts.rs ~:398/:479) may stay as fast-paths or delegate — do not remove behavior, only ensure the chokepoint is authoritative.
- Test: `crates/egret-perm-compile/tests/*` (role chokepoint) and `crates/egret-auth/tests/*` (group chokepoint) — DB-gated.

**Interfaces:**
- Role chokepoint: in the Assign path of `with_recompile_in_tx`, once the role `name` is known, if `name == "platform_admin"` refuse (a `CompileError` variant, e.g. `CompileError::Refused(String)` — reuse an existing refusal variant if one fits, else add one) when: `ptype='user'` AND `SELECT provider FROM users WHERE id=$pid` is `'service'`; OR `ptype='group'` AND `EXISTS (SELECT 1 FROM user_groups ug JOIN users u ON u.id=ug.user_id WHERE ug.group_id=$pid AND u.provider='service')` (confirm the group-membership table/column names — grep `user_groups`/`group_members` in migrations). The check runs BEFORE the write + recompile, in the same tx.
- Group chokepoint: in `add_member`/`change_member`, when `add` and the member being added is a service account (`users.provider='service'`), refuse if the target group holds `platform_admin` (reuse `RoleRepo::roles_of("group", group_id)` / the existing check from service_accounts.rs:391-401). Return the group store's error type; routes map it to 409/400.

- [ ] **Step 1: Write failing DB tests.**
  - Role chokepoint (egret-perm-compile): seed a service user + a human user + a group with a service member + a group without one. Assert: Assign `platform_admin` to the service user → Err(refused); to the service-member group → Err(refused); to the human user → Ok; to the no-service group → Ok; to a service user for a NON-platform_admin role → Ok; and UNASSIGN `platform_admin` from a service user (seed the holding row directly) → Ok (fail-open preserved).
  - Group chokepoint (egret-auth): adding a service user to a group holding `platform_admin` → Err(refused); adding a human user to it → Ok; adding a service user to a group WITHOUT platform_admin → Ok.
- [ ] **Step 2: Run, verify they fail.** `CARGO_TARGET_DIR=target-sa4 cargo test -p egret-perm-compile <t> ; cargo test -p egret-auth <t>`
- [ ] **Step 3: Implement** both chokepoints per the Interfaces. Resolve the role name once; gate on `platform_admin` only; Assign/Add only. Thread the refusal through to a 400/409 at the routes (check the routes' error mapping; add a `From`/match arm if needed). Keep the SA routes' existing fast-path checks.
- [ ] **Step 4: Run, verify pass.** Also run the existing role_membership + groups + the SA `service_account_principals` tests — unchanged behavior for humans must stay green.
- [ ] **Step 5: Commit.** `feat(sa): a service account can never hold platform_admin — enforced at the assignment chokepoints (SA4)`

---

### Task 2: Guarantee tests (no-login, OIDC-sync-excludes-SA, svc- ledger)

**Files:**
- Test: `crates/egret-auth/tests/*` (login rejection, OIDC sync, ledger) — DB-gated. Add a small guard ONLY if a real gap is found.

- [ ] **Step 1: Write the tests.**
  - **No interactive login:** seed a `provider='service'` user (NULL password); assert the login/authentication path rejects it. Find the login/verify fn (grep `verify_password`/`fn login`/`authenticate` in `crates/egret-auth/src/`); assert a service user cannot authenticate (by the NULL-password path and/or an explicit provider check). If NO code path guards it beyond the NULL password, that is sufficient — assert it; if a path exists that could authenticate a service user, add an explicit `provider != 'service'` guard there (then it's a code change, note it).
  - **OIDC sync excludes SAs:** assert `sync_user_oidc_groups` never adds/affects a `provider='service'` user. Construct the scenario its tests use; confirm (by reading + a test) that a service user is never passed to or mutated by the sync. If the sync is keyed strictly on an OIDC login (it is — SAs never log in), a test that documents "a service user is never a sync subject" + a code-read suffices; state which.
  - **svc- ledger retention:** create an SA, delete it, assert its `svc-…` username remains in `username_ledger` (no takeover). Reuse the username-ledger test harness.
- [ ] **Step 2: Run, verify they pass** (or fail first if a guard is being added, then pass).
- [ ] **Step 3: Commit.** `test(sa): pin the service-account guarantees — no login, no OIDC sync, svc- ledger (SA4)`

---

### Task 3: Docs — the guarantees (egret-data/docs)

**Files:** (worked in a docs worktree at merge time; this task writes the prose)
- `platform/control-plane/access-control/service-accounts.md` (+ a security/guarantees page if one exists).

- [ ] **Step 1:** State the **`platform_admin` guarantee** plainly: a service account can never hold `platform_admin`, directly or through a group; enforced at the role-assignment chokepoint, so every route (including the human user-role route) refuses it.
- [ ] **Step 2:** Promote the **catalog-vs-DG** note into a first-class guarantee: a Catalog credential reads the lake (Iceberg) directly and is therefore outside egret's masking, row-filtering and clearance; the control is disable/revoke/expire of the credential or the service account, not engine masks. Explain the why (direct catalog access bypasses the engines where masks live) and the operational consequence.
- [ ] **Step 3:** If the chokepoint changed a documented error response that the REST reference generator emits, regenerate only that; otherwise no reference change.
- [ ] **Step 4: Commit.** `docs(access-control): the service-account platform_admin and catalog-vs-DG guarantees (SA4)` (NOTE: docs live in the `egret-data/docs` repo; the controller opens that PR — this task produces the prose, committed in a docs worktree.)

> The controller handles T3 in a docs worktree off the docs repo, mirroring SA1–SA3; the implementer may instead be pointed at the docs worktree. Keep code tasks (T1/T2) and this docs task separate.

---

### Task 4: Live-check + verification report (driven, not an SDD code task)

The controller runs this on the demo stack (`just demo-up-ghcr edge`, after the SA4 image rebuilds, or a local build), capturing a verification report. Not a subagent code task.

- Grant `platform_admin` to a service account via the **human** `POST /users/{id}/roles` → **refused** (the closed gap); via a group containing an SA → refused; add an SA to a `platform_admin`-holding group → refused; a normal human `platform_admin` grant still works.
- Disable an SA → both the data-api and catalog token mint stop (re-confirm SA2's kill switch).
- A catalog credential reads a table directly (outside DG masking) — document with evidence, or record what wasn't provable on the demo (per the SP3d live-half honesty).
- Grep the running image binary for the new refusal string before trusting the check (`docker-build-cache-ships-a-stale-binary`).

---

## Self-Review
- **Spec coverage:** platform_admin chokepoints → T1; no-login / OIDC-sync / svc- ledger → T2; docs guarantees → T3; live-check → T4. Programme-close noted in the spec.
- **Security:** T1 enforces at the chokepoint both write directions (role-assign + group-add) so both TOCTOU orderings are closed; Assign/Add only (revoke stays fail-open); `platform_admin` only (humans unaffected).
- **No placeholders:** the only "confirm in code" notes are the group-membership table/column names, the existing `CompileError` refusal variant, the login/auth fn name, and the sync test harness — each with the file to read. Every test lists concrete directions + the safe (allowed) cases so the guard can't over-block.
- **Type consistency:** the refusal flows from `with_recompile*` (role) and `add_member`/`change_member` (group) to the routes' 400/409; `platform_admin` is matched by name in both chokepoints.
