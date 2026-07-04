# Config UI UX framework (2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-config-ux-framework-design.md`.

**Goal:** Make the engine Configuration UI dramatically more usable AND build the
reusable framework that later completeness work (Trino tuning props, security/TLS,
observability) plugs into without re-adding UI clutter. **Frontend only — no
backend/API change; the `trinoConfig` PUT payload is unchanged.**

This is sub-project #1 of a 4-part "complete + usable config UI" effort (this = the UX
framework; later: #2 Trino tuning completeness, #3 security/TLS/access-control, #4
observability). It ships first because it fixes the worst current pains and provides the
primitives the others reuse.

**Non-goals:** new Trino config fields (that's #2-#4); a config search box + unsaved-
navigation guard (deferred — YAGNI for now); backend changes.

## Problems this fixes (from the UX audit)
Inconsistent/absent field help (esp. Coordinator/Worker); the 1481-line PodScheduling
single-scroll; three divergent raw-JSON editor variants; no "which fields are set"
indicator; unit-less unvalidated size inputs; ad-hoc section styling; the 5s prefill
effect clobbering in-progress edits; an 8-item horizontal tab strip that overflows.

## The framework (reusable primitives)

1. **`ConfigSection`** (`ConfigSection.tsx`) — one card component: `title`, `description`,
   optional `docsUrl` (a Trino docs link → external icon), children. Every config group
   is refactored onto it, killing the ad-hoc `border p-4` vs bare-`space-y` inconsistency.
2. **`fieldHelp` map + `FieldLabel`** — `fieldHelp.ts` exports `Record<string, {help: string, docsUrl?: string, unit?: string}>` keyed by a stable field key. `FieldLabel` renders the label + an info icon whose tooltip shows `help` (+ a docs link when set). Authored for the high-stakes fields first (Coordinator/Worker resources/heap/memory, Reliability, Autoscaling, Security). Later sub-projects add entries as they add fields.
3. **`RawJsonField`** (`RawJsonField.tsx`) — ONE raw-JSON editor replacing the three
   current variants (`RawJsonOverride`, `RawJsonField`, the affinity textarea). Semantics:
   holds text locally, validates on a short debounce, shows an inline error, and reports
   validity through a shared **validity registry** (a context or a `useValidityRegistry`
   hook) so any invalid raw field anywhere blocks Save — replacing the ad-hoc
   `onValidityChange` prop threading.
4. **`QuantityInput`** (`QuantityInput.tsx`) — a text input for CPU/memory/heap/size
   values: validates the k8s-quantity / Trino data-size shape (`4Gi`, `500m`, `2GB`),
   shows a unit hint + inline error on a bad value. Applied to the free-text size fields.
   Plain number inputs gain sensible `min`/`max` + inline validation.

## Structural changes

5. **Configuration layout → vertical left-nav.** Splitting PodScheduling (below) pushes
   the config sub-tab count to ~10; a horizontal strip overflows. Replace the horizontal
   `TabsList` inside the Configuration card with a **left vertical nav** (settings-page
   style): the section list on the left, the active section's fields on the right. Each nav
   item can carry a **non-default badge** (item 7). The top-level tabs (Overview /
   Configuration / Security & Exposure / Monitor) are unchanged.
6. **Split `PodSchedulingSection`** (1481 lines) into three focused sections, each its own
   nav item + component: **Pod** (serviceAccount, imagePull secrets/policy, pod + container
   securityContext, deployment strategy/annotations/rollout, shareProcessNamespace),
   **Placement** (per-role coord/worker toggle: nodeSelector, tolerations, topologySpread,
   affinity, terminationGrace, extra ports), **Containers & Mounts** (init + sidecar
   containers, envFrom, config/secret mounts). New nav order: General · Coordinator ·
   Worker · Workload · Reliability · Advanced · Pod · Placement · Containers & Mounts ·
   Autoscaling.
7. **Non-default badges** — a small count on each nav item = how many fields in that
   section are set / non-default, derived from the same omit-empty logic the payload
   builder already uses (a per-section selector over the hook state). Zero ⇒ no badge.
8. **Diff-before-save (modal)** — a **Review changes** button beside Save opens a modal
   diffing the built `trinoConfig` against the engine's stored `spec.trinoConfig`: the
   changed field paths (added / removed / changed old→new) + a "saving re-provisions
   (rolls) the pods" note. Uses the existing payload builder; read-only.

## Correctness fix
9. **Prefill no longer clobbers edits** — the `[engine.spec]` prefill effect must NOT reset
   the form while it is `dirty` (guard on `dirty`), so the 5s status poll can't discard
   in-progress input. (Still prefills on first load + when not dirty.)

## Refactor scope
All existing config section components (`EngineConfigGeneral/Reliability/Advanced`,
`EngineRoleConfig`, `EngineResourceGroups`, `EngineSecurity`, the 3 new Pod split
components, `EngineAutoscaling`) are refactored to consume `ConfigSection` + `FieldLabel` +
`RawJsonField` + `QuantityInput`. `useEngineConfigForm` gains the per-section
"non-default count" selectors + the dirty-guarded prefill; its state/payload are otherwise
unchanged (payload parity preserved).

## Test
- `tsc` 0, `eslint` clean, `vitest` green, `vite build` ok.
- Unit: `QuantityInput` validation (accepts `4Gi`/`500m`, rejects `4 gigs`); the
  non-default count selector; the diff builder (added/removed/changed).
- **Playwright**: Configuration shows the vertical nav with all ~10 sections; a section
  with set fields shows a non-default badge; a field's info icon shows help; an invalid
  `RawJsonField` blocks Save; **Review changes** opens the diff modal listing the changed
  paths; editing a field then letting the 5s poll fire does NOT discard the edit; Save
  still issues the `updateEngine` PUT (200) with a payload byte-identical to the
  pre-refactor shape for an unmodified engine.

## Compat
- Pure frontend restructure; no migration, no API/route change; the PUT payload is
  unchanged (verified by the parity check).
