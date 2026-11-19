# EO5 — Soak defaults (begin the EO3/EO4 soak)

Status: design, 2026-11-19. Follow-on to the egret-objects programme [EO0–EO4](2026-11-18-gov-eo0-egret-objects-decomposition-design.md), which left two reversible switches shipped but **defaulting to `legacy`** (zero behaviour change). EO5 moves both defaults to their **observe** state (`shadow`), so every installation begins serving the pre-EO decisions *while comparing the new paths*, logging any divergence. This starts the clean soak that the deferred **finalize** terminal (EO3-D5 / EO4-D5) requires. EO5 serves **no** new path and **deletes nothing**.

## Why

After EO4, the PDP has two switches, both default `legacy`:

- `perm.pdp_policy_source` (EO3): `legacy` / `shadow` / `compiled` — where the compiled **templates** come from. `shadow` serves legacy but builds the egret-cedar-compiled set alongside each reload and logs divergences (`select_served` → `log_shadow`).
- `perm.cedar_grant_source` (EO4): `legacy` / `unified` — where the per-request grant **links** come from. It has **no observe state**: `unified` serves the mirror directly.

Finalize (deleting the legacy policy generation and the `access_grants` read paths) is gated on "a long clean soak on compiled/unified". Nothing can soak while both switches default `legacy` and no installation has flipped them. EO5 flips the shipped default to **observe for both**, symmetric and decision-identical, so the soak begins everywhere with full rollback intact.

## Decisions

| # | Decision |
|---|---|
| EO5-D1 | **Observe, don't serve.** EO5 flips both defaults to `shadow` — serve the proven legacy decisions, build/compare the new path, log divergence. It does **not** serve `compiled` or `unified`. Serving the new paths is a later, separate flip PR; finalize is the terminal after that. |
| EO5-D2 | **Seed the default, keep the fail-safe.** The shipped default is changed by **seeding a `shadow` row** in each switch table via migration (`INSERT … ON CONFLICT (id) DO NOTHING` — never clobber an operator's explicit choice). `read_source`'s "absent/unreadable ⇒ `legacy`" fail-safe is **unchanged**: a corrupt or unreadable switch still falls to the known-good legacy path. Because `shadow` itself serves legacy decisions, the seeded default is as safe as `legacy`. |
| EO5-D3 | **EO4 gains a symmetric `shadow` state.** `GrantSource` gets a `Shadow` variant: `grants_for` under `shadow` reads `access_grants` (serves legacy, errors propagate exactly as `legacy` — no fallback needed, it is already the proven table), and each PDP reload runs the equivalence `report()` and logs it (empty ⇒ info; non-empty ⇒ warn per divergence + count), mirroring EO3's `log_shadow`. This makes EO4's soak decision-identical and observable, matching EO3. |
| EO5-D4 | **Fold in the migration.** Before (and independent of) any flip to `unified`, the migration folds all existing `access_grants` into `perm.cedar_grants` (the idempotent `INSERT…SELECT…ON CONFLICT DO UPDATE` already in `AccessGrantStore::fold`). This makes the mirror complete from the first boot, so EO4's `shadow` report is meaningful (not "everything only_legacy") and a later `unified` flip is safe. The trigger (migration 0240) already mirrors writes from its creation onward; the fold backfills the pre-trigger rows. |
| EO5-D5 | **Reversible; undo steps back one level.** EO5 changes nothing about rollback except making EO4's `undo` step back by one level (`unified → shadow → legacy`), mirroring EO3's `undo`, and adding a `POST /shadow` endpoint to EO4 so an operator can re-enter observe after an `undo` to `legacy`. Every state remains reachable and reversible; `access_grants` stays the write target throughout. |
| EO5-D6 | **No new decisions, no deletion, no vocab change.** EO5 adds no permission, changes no grant or policy semantics, and removes no code path. It only changes which *default state* the switches ship in and adds EO4's observe machinery. Finalize stays out (EO3-D5 / EO4-D5). |

## Concepts

### Symmetric observe

After EO5, a fresh installation boots with both switches at `shadow`:

- **Templates (EO3):** serve the legacy (SQL) template set; also build the egret-cedar-compiled set each reload and log any divergence (`log_shadow`). Decisions are the legacy decisions, byte-for-byte.
- **Links (EO4):** read grants from `access_grants` (legacy); also run the `report()` row-set equivalence between `access_grants` and the folded `perm.cedar_grants` each reload and log it. Decisions are the legacy decisions, byte-for-byte.

Neither path is *served*; both are *observed*. The soak is the window in which operators (and CI, and the demo stack) watch these logs stay clean before the later flip-to-serve PR.

### The seeded default

A migration seeds `shadow` into both singleton switch rows with `ON CONFLICT (id) DO NOTHING`. This gives every installation — fresh or upgrading — the observe default, while leaving an operator who already set a value untouched. The in-code `read_source` fallback for an absent/unreadable row stays `legacy`, so the seeded row is the *intended* default and `legacy` remains the *fail-safe floor*.

## Architecture

```text
 perm.pdp_policy_source  default → shadow   (EO3: build legacy+compiled, serve legacy, log diff)
 perm.cedar_grant_source default → shadow   (EO4: read access_grants, run report() each reload, log diff)
                                            both OBSERVE; neither SERVES the new path; nothing deleted
```

- **Migration (one file, 0241):** fold `access_grants → perm.cedar_grants`; seed `perm.pdp_policy_source` = `shadow` (ON CONFLICT DO NOTHING); seed `perm.cedar_grant_source` = `shadow` (ON CONFLICT DO NOTHING).
- **EO4 `Shadow` state:** `GrantSource::Shadow` (`as_db`/`from_db` = `"shadow"`); `grants_for` routes `Legacy | Shadow → access_grants`; load path runs `report()` + logs when `grant_source == Shadow`.
- **EO4 endpoints:** add `POST /api/control-plane/perm/cedar-grants/shadow` (enter observe); `undo` steps back one level.
- **Docs:** both runbooks (`pdp-policy-source.md`, `cedar-grant-source.md`) updated — the default is now `shadow`/observe, with the soak explained. (Separate `egret-data/docs` PR.)

## Scope

**In:** the one migration (fold + two seeds); `GrantSource::Shadow` and its `grants_for` / load-path / endpoint / undo handling; test updates for the new shipped default (`shadow`, serving legacy); an EO4 shadow-observe test; a fold-in-migration test; runbook updates.

**Out:** flipping to serve `compiled` / `unified` (a later flip PR after a clean soak); finalize — deleting `build_legacy`'s bespoke templates, the `access_grants` / `effective.rs` read paths, and the switch tables themselves (the combined post-soak terminal, EO3-D5 / EO4-D5); any change to roles / `role_permissions`, vocab, or decisions; the workflow/alert securables sub-project (tracked separately).

## Errors

Fail closed, unchanged from EO3/EO4:

- **Switch row absent or unreadable** → `legacy` (the fail-safe floor); `read_source` stays infallible. The seeded default is `shadow`, but a broken switch still serves legacy.
- **`shadow` and the compiled/report build errors** → serve legacy, log the error; a reload never breaks on observation.
- **A `unified` read errors** (only reachable by an operator who flips to `unified`) → fail-safe to `access_grants` + alarm, exactly as EO4 shipped.

## Verification

1. **Seeded default:** a fresh migrated DB reports both switches at `shadow`; an installation with an explicit prior value keeps it (ON CONFLICT DO NOTHING).
2. **Decision-identical:** with both at the seeded `shadow`, every EO2/EO3/EO4 differential and the existing authz suites stay green — `shadow` serves exactly the legacy decisions.
3. **EO3 observe:** `shadow` logs the compiled-vs-legacy comparison each reload (already covered; extend if the default change shifts a baseline).
4. **EO4 observe:** under `GrantSource::Shadow`, `grants_for` reads `access_grants`, and a reload runs `report()` and logs it; a seeded mirror-only divergence makes the log non-empty (teeth).
5. **Fold-in-migration:** after the migration, `perm.cedar_grants` equals `access_grants` row-for-row (the `report()` is empty) with no application writes in between.
6. **Reversible:** EO4 `undo` steps `unified → shadow → legacy`; `POST /shadow` re-enters observe; `access_grants` is never not the write target.

## After EO5

Both switches ship at `shadow`; the soak is running and observable. What remains is unchanged in shape, only unblocked:

1. **Flip to serve** — a small PR moving both defaults (or operator state) to `compiled` / `unified` once the soak logs are clean.
2. **Finalize** — the combined post-soak terminal (EO3-D5 / EO4-D5): delete `build_legacy`'s bespoke templates, the `access_grants` / `effective.rs` read paths, and the switch machinery.
3. **Workflow/alert securables** — the separate deferred sub-project (EO0 EO-D6; notebook is already removed and marimo "app" is already a securable).
