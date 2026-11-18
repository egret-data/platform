# EO3 — CedarPdp consumes the IR-compiled policy behind a reversible shadow→compiled switch

Status: approved design, 2026-11-18. Sub-project **EO3** of the egret-objects programme. Its umbrella is the EO0 decomposition spec ([2026-11-18](2026-11-18-gov-eo0-egret-objects-decomposition-design.md)); decisions EO-D1…EO-D8 there govern this document, and any change to them is made in EO0 first. EO1 ([plan](../plans/2026-11-18-gov-eo1-object-securables-vocab.md), merged #876) put the pure `egret_object` vocabulary in `egret-perm-ir`. EO2 ([spec](2026-11-18-gov-eo2-egret-cedar-compile-target.md), merged #877) added the `egret-cedar` crate — `compile(load_inputs(db)) -> Compiled{policy_set, role_names, …}` — and a CI differential proving `compile()` is **decision-equivalent** to the live PDP's `load_inner()` over a seeded corpus (6084 cases, 0 diffs) plus a teeth test. EO2 wired nothing to the runtime. **EO3 is the wiring.**

## Goal

Make the live `CedarPdp` serve the `egret-cedar`-**compiled** `PolicySet` instead of the one `load_inner` assembles today — **behind a reversible, operator-controlled switch** that defaults to the legacy path, runs a **shadow comparison on production data** before any cutover, and **rolls back instantly**. Shipping EO3 changes no behaviour on its own (default `legacy`); flipping the switch is an explicit operator action, gated on a clean live equivalence report. This is the programme's highest-risk sub-project: it is the first to change what the runtime PDP actually authorizes against, so every step is reversible and the legacy path stays alive for rollback (its deletion — *finalize* — is a later terminal, out of scope here).

## Scope

In scope:

- A global, single-row switch store `perm.pdp_policy_source` (`legacy` | `shadow` | `compiled`; default `legacy`) read by the PDP **on every reload**, so a flip or rollback takes effect at the next `RolesChanged` without a process restart.
- The reload branching in `egret-auth`'s PDP: `legacy` serves `load_inner`'s set unchanged; `shadow` builds **both** sets, set-compares them, logs/metrics any divergence, and serves **legacy**; `compiled` builds both, compares, and serves the **compiled** set.
- A live equivalence **report** (build both from the same production DB, set-difference the policies) exposed over HTTP, as the **precondition** to switch to `compiled`, mirroring SP5b's catalog-migrate gate but global (one PDP, one switch).
- Operator HTTP endpoints: read state, run report, move `legacy→shadow`, `shadow→compiled` (gated), and `undo`.
- A fail-safe: if the state is `compiled`/`shadow` but `compile()` errors during a reload, the PDP logs, alarms, and **serves the legacy set** — authz never breaks.

Out of scope (later sub-projects):

- **Finalize** — deleting `load_inner`'s bespoke SQL and the effective-perm closure duplicated in `egret-cedar/src/load.rs`, making `compile()` the only path. A separate terminal operation after production soak.
- EO4 — folding `access_grants` into `perm.*` (the grant **source** migration; EO3 keeps reading the legacy tables through `load_inputs`).

## Decisions (EO3-local; all consistent with EO0)

| # | Decision |
|---|---|
| EO3-D1 | **Reversible switch, default legacy, re-read every reload.** The serve-source is a single global row in `perm.pdp_policy_source` with states `legacy` (default) / `shadow` / `compiled`. `load_inner` reads it at the top of every reload (initial `build` and every `RolesChanged`), so a flip or `undo` takes effect at the next reload with no restart. A DB with no row ⇒ `legacy` ⇒ shipping the migration enables nothing. State stored in the DB (not a config key/env var) for auditability (`switched_at`/`switched_by`) and instant hot rollback, exactly the SP5b `catalog_grant_switch` pattern — but **one global row**, not per-object, because there is exactly one process-wide PDP. |
| EO3-D2 | **Shadow before cutover, comparing on production data.** `shadow` serves legacy but builds the compiled set too and compares, surfacing divergences on **real** roles/classifications — closing the gap the EO2 CI differential cannot (a seeded corpus only proves equivalence over the shapes it generates; production shapes may differ). An operator soaks `shadow`, confirms zero divergence, then switches. `compiled` keeps building and comparing both sets each reload (a continuous canary) until *finalize* removes the legacy path. |
| EO3-D3 | **Set-equality on policy text is the comparison oracle.** The two sets are compared as maps `PolicyId → policy text`. Because `compile()` reuses the *same* text formatters `load_inner` uses (EO2-D1), equivalent inputs yield **byte-identical** policy text, so an exact set-equality has zero false positives and needs no runtime probe corpus. A divergence is any id present on one side only, or an id whose text differs. Classification rows skipped by both sides (both-empty, malformed) are absent from both maps and so never diverge. This is strictly stronger than decision-equivalence for identical inputs, and it is cheap (no `cedar_policy::Authorizer` calls, no principal/object sampling). |
| EO3-D4 | **The switch logic lives in `egret-auth`; HTTP handlers are thin.** Only `egret-auth` sees both `load_inner` (private) and `egret_cedar::compile`, so the report and the build-both machinery live there (the same acyclicity reasoning that put the EO2 differential in `egret-auth/tests`). The control-plane router hosts thin handlers that call into the `egret-auth` switch API and read/write the store. Endpoints nest under the existing `/api/control-plane/perm/` surface. |
| EO3-D5 | **Fail-safe to the proven path.** If the state selects the compiled set but `compile()`/`load_inputs()` errors during a reload, the PDP logs at `error`, emits a divergence/failure metric, and serves the **legacy** set for that reload (consistent with the existing "reload failed; keep previous policy set" philosophy and fail-closed EO-D7). The compiled path can never take down authz. The `switch` endpoint's report gate makes this path unlikely, but the runtime guard is unconditional. |
| EO3-D6 | **No new policy decisions, no grant-source change.** EO3 changes only *which assembler* produces the `PolicySet`; the inputs are the same legacy tables EO2's `load_inputs` already reads. The decision semantics are exactly EO2's proven-equivalent set. The grant **source** moves to `perm.*` only at EO4. |

## Architecture

```text
 perm.pdp_policy_source (singleton row)          egret-auth PDP (CedarPdp)
   source: legacy | shadow | compiled    ──read every reload──►  load_inner / spawn_reload
                                                                   │
                      ┌────────────────────────────────────────────┤
                      ▼ legacy                 ▼ shadow / compiled   │
          build legacy (existing SQL)   build legacy  +  egret_cedar::compile(load_inputs(db))
                      │                         │  set-compare (PolicyId → text)
                      │                         │  log + metric on divergence
                      ▼ serve legacy            ▼ serve legacy (shadow) | compiled (compiled)
                                                   fail-safe: compile err ⇒ serve legacy + alarm
                                        ▲
 control-plane router (thin handlers) ──┘  GET state · POST report · shadow · switch(gated) · undo
   calls egret-auth switch API (report = build BOTH + set-diff)
```

`build`/`load_inner` keep their current signatures — the switch is read from the `Db` the PDP already holds — so the ~120 existing `CedarPdp::build(db, &bus)` call sites and every current PDP test compile and behave unchanged under the default `legacy` state.

### Reload behaviour by state

| State | Build | Compare | Serve | On `compile()` error |
|---|---|---|---|---|
| `legacy` (default) | legacy only | — | legacy | n/a (legacy only) |
| `shadow` | legacy **+** compiled | set-equality, log + metric | **legacy** | log + alarm, serve legacy (already serving legacy) |
| `compiled` | legacy **+** compiled | set-equality, log + metric | **compiled** | log + alarm, **fail-safe: serve legacy** |

### Endpoints (SP5b-style, global single switch)

Under `/api/control-plane/perm/pdp-source` (platform-admin only, like the other `/perm/*` admin routes):

| Method · path | Effect |
|---|---|
| `GET  /pdp-source` | current `source`, `switched_at`/`switched_by`, and a summary of the most recent shadow divergence (count + sample), for an operator dashboard/CLI. |
| `POST /pdp-source/report` | one-shot: build both sets from the live DB, return the full divergence list (`{policy_id, side, …}`). Empty ⇒ safe to switch. Read-only; changes no state. |
| `POST /pdp-source/shadow` | `legacy → shadow`. Records audit columns. Idempotent. |
| `POST /pdp-source/switch` | `shadow → compiled`, **refused unless a freshly-run report is clean** (zero divergences). Records audit. |
| `POST /pdp-source/undo` | step back: `compiled → shadow` or `shadow → legacy` (operator chooses target, or a single-step default). Records audit. Instant rollback — the next reload serves the earlier source. |

The switch does not itself force a reload; it takes effect at the next `RolesChanged`. EO3 may publish a `RolesChanged` (or an equivalent PDP-reload nudge) on a state change so a flip applies promptly rather than waiting for the next organic role edit — the plan fixes the exact mechanism; a missed nudge is harmless because the state is re-read on every reload regardless.

## Verification

1. **Switch store unit tests** — get/upsert/flip the singleton row; default is `legacy` when absent; audit columns fill on write; the `CHECK` rejects an unknown source.
2. **Reload-branch tests** (seeded DB) — set each state, (re)build the PDP, and assert the ArcSwap holds the right set: `legacy`/`shadow` serve `load_inner`'s set, `compiled` serves `compile()`'s set. Assert `shadow` and `compiled` both produce a comparison result; `legacy` does not build the compiled set.
3. **Equivalence report** — on a normal seeded DB the report is **empty** (reuse the EO2 differential's state generators for realistic shapes). Under a seeded mutation that makes the two assemblers disagree (e.g. a role perm reachable by only one path), the report is **non-empty** — the **teeth** that prove the report is not vacuous.
4. **Switch gate** — `POST /switch` is refused when the report is non-empty and accepted when clean; `shadow`/`undo` move state as specified; unauthorized callers are denied (platform-admin gate).
5. **End-to-end hot flip** — build a PDP in `shadow`, assert served decisions are identical to `legacy` for a corpus and that a divergence (if injected) is logged/metriced; flip to `compiled`, fire `RolesChanged`, assert the compiled set is now served and decisions still match; `undo`, fire `RolesChanged`, assert the legacy set is served again (rollback).
6. **Fail-safe** — with state `compiled` and a DB rigged so `compile()` errors, a reload logs + serves the legacy set; authz is uninterrupted.
7. **Default-legacy regression** — the existing PDP test suite (hot reload, role hierarchy, classification, object scope, explicit deny, expiry, differential) passes unchanged, proving the default path is byte-for-byte the prior behaviour.

## Errors (fail closed / fail safe, per EO-D7)

- Switch store unreadable at reload ⇒ treat as `legacy` (the proven path), log a warning. Never block boot or a reload on the switch row.
- State is `compiled`/`shadow` but the compiled build errors ⇒ serve legacy + alarm (EO3-D5).
- `POST /switch` with a non-clean or stale report ⇒ refuse with the divergence summary; state unchanged.
- An unknown `source` value (should be impossible behind the `CHECK`) ⇒ treat as `legacy`, log an error.

## Files (indicative; the plan fixes exact paths)

- `migrations/0238_pdp_policy_source.sql` (**renumber above `main` and any open PR at merge** — migration numbers are a shared namespace) — the singleton switch table + a seeded default row (or rely on "no row ⇒ legacy").
- `crates/egret-auth/src/pdp/source_switch.rs` (new) — the `PolicySource` enum, the store accessors (`read`/`write`/audit), the set-equality comparison (`PolicyId → text` diff → `Divergence` list), and the `report`/build-both entry points the HTTP layer calls.
- `crates/egret-auth/src/pdp/mod.rs` — `load_inner`/`spawn_reload` read the state and branch (build-both + compare + serve-by-state + fail-safe); `build` unchanged in signature. The compiled build calls `egret_cedar::{load_inputs, compile}` (egret-auth already depends on `egret-cedar`).
- The control-plane router (in `egret-bin`, beside the other `/perm/*` mounts, or the governance admin crate the plan selects) — thin `GET/POST` handlers over the `egret-auth` switch API, platform-admin gated, with `utoipa` schemas.
- `crates/egret-auth/tests/pdp_source_switch.rs` (new) — the reload-branch, report/teeth, gate, hot-flip, fail-safe, and rollback tests (the crate that sees both assemblers).
- Existing PDP tests — unchanged (default-legacy regression).

## Docs

EO3 adds **operator-facing HTTP endpoints and an operational runbook** (how to shadow, read the report, switch, and roll back the PDP policy source). Per the repo docs rule, EO3 **requires an `egret-data/docs` PR** (Platform section) — unlike EO1/EO2, which were library + tests only. The runbook documents the switch lifecycle, the shadow-divergence signal, and the rollback procedure.
