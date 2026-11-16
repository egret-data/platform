# Service accounts SA4 — Guarantees + live — design

> Final package of the "service accounts as principals" programme (SA1 #867,
> SA2 #869, SA3 #870). Closes the cross-cutting guarantees the earlier packages
> deferred, documents the catalog-vs-DG semantics, and live-checks the lot.
> Extends `2026-11-15-service-accounts-as-principals-design.md`.

## Background — what's deferred into SA4

The SA invariants were enforced package-locally; SA4 makes them airtight and
proven:

- **"A service account can never hold `platform_admin`"** is only partly
  enforced. The SA routes refuse it (direct grant; adding an SA to a group that
  holds `platform_admin`). But the **human** route `POST /users/{id}/roles`
  (`assign_user_role`, `crates/egret-controlplane/src/access/mod.rs:518`) calls
  `assign_platform_role_and_recompile("user", id, role_id)` with NO service-account
  check, so a platform admin can still grant `platform_admin` to an SA's user id.
  Group-mediated grants (grant `platform_admin` to a group that already contains
  an SA) are also unguarded on the human side.
- **No interactive login** is enforced by `users_service_has_no_password` (a
  service user has a NULL password and the login handler requires one), but has
  no dedicated test.
- **OIDC group sync never adds an SA** is true by construction (`sync_user_oidc_groups`
  runs at an OIDC user's login; SAs are `provider='service'` and never log in),
  but is untested.
- **Catalog credentials read Iceberg directly, outside DG masking** — stated in
  the SA2/SA3 docs as a note, but not documented as a first-class guarantee nor
  proven on a stack.

## Goal

Make "an SA never holds `platform_admin`" hold at a **chokepoint** so every
present and future caller is covered; add tests pinning the other guarantees;
document the catalog-vs-DG semantics as a guarantee; and live-check all of it.

## Decisions (brainstorming 2026-11-16)

- **Chokepoint enforcement** (not per-route): "unrepresentable beats
  unforgettable." Enforce the `platform_admin` invariant at the two shared
  write chokepoints, so the human route, the SA routes, and any future caller
  are all covered, with no TOCTOU window.
- **Full scope:** enforcement + guarantee tests + docs + live-check.

## 1. The `platform_admin` invariant — two chokepoints

**Role-assignment chokepoint** — `crates/egret-perm-compile/src/role_membership.rs`,
inside `with_recompile` / `with_recompile_in_tx` (the single inner fn both
`assign_platform_role_and_recompile` and its `_in_tx` sibling, and thus both the
human and SA routes, funnel through). On an **Assign** of the `platform_admin`
role, refuse with a domain error when:
- `ptype = 'user'` and that user is a service account (`users.provider = 'service'`), OR
- `ptype = 'group'` and the group has **any** member that is a service account.

This closes the human `POST /users/{id}/roles` direct grant AND the
"grant `platform_admin` to a group that contains an SA" direction. Scope the
check to the `platform_admin` role only (resolve the role name once); never
touch the Unassign/revoke path (that direction fails open and must stay
permissive so a mistaken grant can always be removed).

**Group-membership chokepoint** — `crates/egret-auth/src/groups.rs`,
inside `GroupRepo::add_member` / `change_member` (the single fn the human
`add_group_member` route and the SA `add_service_account_group` route both use).
Refuse adding a **service-account** user to a group that currently holds
`platform_admin`. The SA route's own equivalent check (service_accounts.rs:398)
becomes redundant and may delegate to this chokepoint (or stay as a fast-path;
the plan decides) — but the chokepoint is the guarantee.

Together these cover both orderings (grant-then-join and join-then-grant), so
there is no TOCTOU hole. A domain error maps to 400/409 at the routes.

**Tests (DB-gated), all four directions + the two safe directions:**
- direct `platform_admin` to a service user → refused (via the human route path);
- `platform_admin` to a group containing a service member → refused;
- adding a service user to a `platform_admin`-holding group → refused;
- `platform_admin` to a human user / a group with no service member → ALLOWED (not over-blocked);
- revoke `platform_admin` from a service user that somehow holds it → ALLOWED (fail-open revoke preserved);
- a non-`platform_admin` role to a service user / group → ALLOWED.

## 2. Guarantee tests (+ small guards only if a gap is found)

- **No interactive login:** a `provider='service'` user cannot authenticate —
  assert the login path rejects it (NULL password / the service guard). If the
  login handler has a path that doesn't rely on the NULL password, add an
  explicit `provider != 'service'` guard; otherwise test-only.
- **OIDC sync excludes SAs:** assert `sync_user_oidc_groups` (and the provisioning
  path) never adds/sees a `provider='service'` user — construct a scenario and
  assert no service user is touched. Confirm there is no code path that could
  sync a service user.
- **Username ledger covers `svc-`:** already ledger-correct (SA1/SA2). A test
  asserts a deleted SA's `svc-` name is retained in `username_ledger` (no
  takeover) — reuse the existing ledger-test harness.

## 3. Docs — the guarantees as first-class statements

In `egret-data/docs` (the service-accounts page + any security/guarantees page):
- **The `platform_admin` guarantee:** an SA can never hold `platform_admin`,
  directly or through a group, enforced at the assignment chokepoint — state it
  plainly.
- **Catalog-vs-DG semantics:** promote the existing note into a clear guarantee —
  a Catalog credential reads the lake (Iceberg) directly and is therefore
  **outside** egret's data-governance masking, row-filtering and clearance; the
  control surface for it is disable/revoke/expire of the credential or the
  service account, not engine masks. Explain why (direct catalog access bypasses
  the engines where masks live) and what that means operationally.
- No REST reference change (no new endpoints; the chokepoint refusals surface on
  existing routes — update their documented error responses if the generator
  picks them up).

## 4. Live-check (verification report on a stack)

On the governance demo stack (`just demo-up-ghcr edge`), prove end-to-end:
- Granting `platform_admin` to a service account via the **human** `POST /users/{id}/roles`
  route is **refused** (was the gap); via a group containing an SA is refused;
  adding an SA to a `platform_admin`-holding group is refused. A normal human
  `platform_admin` grant still works.
- Disabling an SA stops **both** audiences (data-api token mint + catalog token
  mint) — re-confirm SA2's kill switch live.
- A **catalog credential reads a table directly** (outside DG masking) — show it
  returns unmasked data where an engine query under a mask would not, documenting
  the guarantee with evidence. (If the demo lacks a seeded masked table + an
  Iceberg-queryable path, record what was and wasn't provable, per the SP3d
  live-half honesty.)
Record a verification report; grep the running image binary for the new refusal
before trusting the live check (`docker-build-cache-ships-a-stale-binary`).

## Decomposition (plan tasks, each its own review)

- **T1** — the `platform_admin` chokepoint (role-assignment + group-membership)
  + DB-gated tests for all four refusal directions and the safe directions.
- **T2** — guarantee tests: no-interactive-login, OIDC-sync-excludes-SA, `svc-`
  ledger retention (+ a guard only if a gap surfaces).
- **T3** — docs: the `platform_admin` guarantee + the catalog-vs-DG guarantee
  (egret-data/docs), REST reference regen only if the spec changed.
- **T4** — live-check on the demo stack + a verification report.

## Global constraints

- Branch `feat/svc-accounts-sa4` off main `a641f95e3`. `CARGO_TARGET_DIR=target-sa4`
  on every cargo command. Throwaway Postgres; DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Co-author `Claude Opus 4.8`. Conventional
  Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit
  path (own target dir not gitignored). No secrets.
- Migration numbers are a shared namespace (none expected — no schema change).
- Each package merges on green CI after the user's word. Docs land in egret-data/docs.
- SDD per code task; the live-check (T4) is a driven verification, not an SDD code task.

## Programme close

SA4 is the last package. After it, the programme "service accounts as
principals" is complete: SA a first-class principal (SA1), unified credentials +
kill switch (SA2), the console tab (SA3), and the airtight guarantees (SA4).
Next programme item after this: SP5 (one grant model) per the CP/DG roadmap.
