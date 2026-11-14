# Governance SP3d — live half — design

> Extends the authoritative spec [`2026-11-12-gov-sp3-enforcement-points.md` §SP3d](./2026-11-12-gov-sp3-enforcement-points.md). The SP3d **code half** (the per-EP migration-complete gate: `ep_gate` verdict, `GET …/migration-gate`, persisted `perm.compile_runs.migration_complete`, overview `migrationIncomplete`) merged as #862. This design covers the **live half**: show the verdict, switch the demo stack to the new model, prove enforcement live, and close the switch.

## Goal

Bring every demo-stack enforcement point (EP) fully onto egret's permission model, prove on the real engines that enforcement is unchanged, surface the verdict in the UI, and then make the switch irreversible by removing the legacy inputs.

## Decomposition (three packages, built A → B → C, each its own PR)

### Package A — Migration-gate badge (frontend code)

**What:** render the full `GET /api/control-plane/perm/policies/services/{id}/migration-gate` verdict as a badge on the Migrate tab, beside the existing operator-only `MigrationStatusBadge`.

- Add a `useQuery` for the gate verdict (generated hook `serviceMigrationGate` / a `migrationGateQueryKey(serviceId)`), mirroring the existing `migrationStatusQueryKey` query in `frontend/src/routes/governance/engines/tabs/MigrateTab.tsx:99-132`; invalidate it on the same events that invalidate the status query (after a migrate/undo).
- A new `MigrationGateBadge` component: **"Migration complete — fully on the model"** (success tone) when `complete`, otherwise **"N steps remain"** with the refusal reasons from `GateVerdict.refusals[].message()` (one line each, server text verbatim — the operator/role/workspace/clearance sentences already defined in `ep_gate.rs`). A failed read shows no badge (same as the status query).
- The existing operator-only badge stays; the new badge is the full four-condition verdict. Copy clarifies: operator badge = "manual policies"; gate badge = "fully on the model".
- Tests: `MigrateTab.test.tsx` — complete → success badge; incomplete → the reasons render; read error → no badge. Pure vitest, no stack.

**No backend change** (the endpoint shipped in #862). Own PR + docs screenshot slot deferred to Package B.

### Package B — Switch the demo stack, live checks, screenshots (operational)

**What:** on the governance demo stack, migrate every EP to `complete`, prove enforcement live on real Trino **and** StarRocks, and capture screenshots.

Steps (a runbook, recorded as a verification report; mostly existing endpoints, no new product code):
1. **Bring up the governance demo stack** (`deploy/docker-compose.demo.yml` / `.ghcr.yml` + `deploy/seed-demo-catalog.sh`): control-plane, Postgres, OPA, Trino and StarRocks on the egret fork images, seeded catalogs/EPs/users. Own compose project (demo stack is shared state — see `egret-demo-stack-shared-state`); keep >20 GB free (`disk-full-hangs-docker-desktop`).
2. **Switch each EP to the new model** = run the existing migrate endpoints until its gate reads `complete`: `POST …/services/{id}/migrate` (operator policies, SP3b/SP4d), `POST …/migrate/role-principals`, `…/migrate/clearances`, `…/migrate/ws-assignments` (SP3c). Each is proved-equivalent and reversible. Confirm `GET …/migration-gate` → `complete` for every EP, and the overview `migrationIncomplete` → 0.
3. **Live checks** against the real engines (both Trino and StarRocks), per §SP3d:
   - an unprivileged user querying a protected object is denied;
   - a groups-only clearance denies a user who lacks the group;
   - a migrated mask still masks and a migrated row-filter still filters the same rows as before the switch.
   Use the live-check recipes already in memory (`egret-security-flow-e2e`, `egret-frontend-viz-verify` creds `admin@egret.local/Admin12345!`, `egret-manual-trino-integration-test`). Record before/after for each EP.
4. **Screenshots:** the new gate badge (Package A) on a complete EP and on an incomplete one, for `docs` `migrate.md` (replaces the `TODO(SP3d)` slot). Grep the running image binary for the change before trusting a live check (`docker-build-cache-ships-a-stale-binary`).

**Done when:** every demo-stack EP reads `complete`; all live checks pass on both engines; a verification report + screenshots exist; still reversible (Package C not yet run).

### Package C — Close the switch / remove legacy inputs (backend code + ops)

**What:** once every EP is `complete` and proven, make the switch irreversible by removing the legacy inputs, per the concepts' "no long-term backward compatibility".

- **New action** `POST /api/control-plane/perm/migrate/finalize` (requires `policy:update`), refused unless **every** EP is migration-complete (reads the gate; fail-closed). It deletes the four provenance tables' rows (`perm.migrated_from`, `perm.clearance_migration`, `perm.role_principal_migration`, `perm.ws_assignment_migration`) and the switched-off original operator policies in `policy.authz_policies`, in one transaction. After finalize, `undo` has nothing to restore (reversibility is closed). The managed model (groups, withhold rules, explicit grants) is untouched — it is now the sole source.
- Idempotent: a second finalize over already-removed inputs is a no-op success.
- Migration: none required (deletes rows, adds no column).
- Tests: DB-gated — finalize refused while any EP incomplete; after completing, finalize removes provenance + switched-off originals and a subsequent undo is a no-op; the compiled model and engine enforcement are unchanged (re-run a live check on the demo stack after finalize).
- **Then run it on the demo stack** and re-prove the live checks (enforcement must be identical after legacy removal).

## Global constraints
- Chat VI, code/commits/PRs/docs EN. Co-author `Claude Opus 4.8`. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (own target dir not gitignored). No secrets.
- Each package merges on green CI after the user's word. Docs (badge narrative + screenshots) land in `egret-data/docs` as a docs PR.
- SDD per code package (A, C); Package B is a driven verification session, not an SDD code plan.
- Gate/verdict unchanged from #862; no new old/new-model switch field (spec §SP3d).

## Open risk
Package B/C depend on the governance demo stack coming up cleanly (Docker-heavy; prior Docker Desktop crashes/disk hangs). B step 1 verifies feasibility before anything else; if the stack cannot come up, B/C pause and A still ships.
