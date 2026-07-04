# Config UI UX Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the engine Configuration UI far more usable and build the reusable primitives later completeness work plugs into — without changing the `trinoConfig` payload.

**Architecture:** New primitives (`ConfigSection`, `FieldLabel`+`fieldHelp`, unified `RawJsonField`+validity registry, `QuantityInput`); Configuration switches from a horizontal sub-tab strip to a vertical left-nav; the 1481-line `PodSchedulingSection` splits into three focused sections; nav items carry non-default badges; a Review-changes diff modal precedes Save; the prefill effect no longer clobbers in-progress edits. Existing section components are refactored onto the primitives. **Frontend only — payload byte-identical.**

**Tech Stack:** React 18, shadcn/ui (Card, Tooltip, Dialog, Badge, ScrollArea), Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-04-config-ux-framework-design.md`.

---

## Refactor discipline
Payload invariant: **an unmodified engine's `trinoConfig` PUT body must not change.** After each task: `npx tsc --noEmit`, `npm run build`, reload an engine, confirm fields still prefill. Moves are mechanical (relocate JSX, swap a raw `<Label>`/`<Input>`/`<Textarea>` for the primitive) — do not alter form state, payload logic, or field semantics.

## File Structure
- **Create** `frontend/src/routes/control-plane/config/ConfigSection.tsx`, `FieldLabel.tsx`, `fieldHelp.ts`, `RawJsonField.tsx`, `validityRegistry.ts`, `QuantityInput.tsx`, `configDiff.ts`, `ReviewChangesDialog.tsx`, `PodSection.tsx`, `PlacementSection.tsx`, `ContainersMountsSection.tsx`. (A `config/` subfolder keeps the growing set tidy; adjust import paths accordingly.)
- **Modify** `EngineDetail.tsx` (vertical nav + badges + Review button), `useEngineConfigForm.ts` (prefill guard + non-default selectors), `EngineAutoscaling.tsx`, `PodSchedulingSection.tsx` (deleted after split), `EngineConfigGeneral/Reliability/Advanced.tsx`, `EngineRoleConfig.tsx`, `EngineResourceGroups.tsx`, `EngineSecurity.tsx`.

---

## Task 1: Primitives — ConfigSection + FieldLabel + fieldHelp

**Files:** Create `config/ConfigSection.tsx`, `config/FieldLabel.tsx`, `config/fieldHelp.ts`

- [ ] **Step 1: `fieldHelp.ts`**

```tsx
export interface FieldHelp { help: string; docsUrl?: string; unit?: string }
export const fieldHelp: Record<string, FieldHelp> = {
  "coordinator.heap.maxSize": { help: "Fixed JVM max heap (-Xmx). Leave blank to use a % of the memory limit.", docsUrl: "https://trino.io/docs/current/installation/deployment.html#jvm-config", unit: "size" },
  "queryMaxMemory": { help: "Cluster-wide user memory a single query may use across all workers.", docsUrl: "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-memory" },
  "worker.gracefulShutdown": { help: "Drain the worker before termination so in-flight queries finish.", docsUrl: "https://trino.io/docs/current/admin/graceful-shutdown.html" },
  // ...authored for the high-stakes Coordinator/Worker/Reliability/Autoscaling/Security fields; extend as needed.
};
```

- [ ] **Step 2: `FieldLabel.tsx`** — `FieldLabel({ fieldKey, label, htmlFor }: { fieldKey?: string; label: string; htmlFor?: string })`: renders the label text; if `fieldKey` has a `fieldHelp` entry, append an `Info` icon (lucide) wrapped in a shadcn `Tooltip` showing `help`, and (when `docsUrl`) a small external-link icon anchor. Reuse the app's `Tooltip`/`TooltipTrigger`/`TooltipContent`.
- [ ] **Step 3: `ConfigSection.tsx`** — `ConfigSection({ title, description, docsUrl, children }: { title: string; description?: string; docsUrl?: string; children: React.ReactNode })`: a `rounded-lg border p-4 space-y-4` card with a header (title + optional external docs link) + `description` in `text-xs text-muted-foreground` + the children. This becomes the ONE section wrapper.
- [ ] **Step 4** `tsc` + `build`. Commit `feat(ui): ConfigSection + FieldLabel + fieldHelp primitives`.

---

## Task 2: Unified RawJsonField + validity registry

**Files:** Create `config/validityRegistry.ts`, `config/RawJsonField.tsx`; Modify `EngineAutoscaling.tsx` (first consumer)

- [ ] **Step 1: `validityRegistry.ts`** — a React context tracking invalid raw-JSON fields by id:

```tsx
import { createContext, useContext, useState, useCallback, useMemo } from "react";
interface Registry { setInvalid: (id: string, invalid: boolean) => void; anyInvalid: boolean }
const Ctx = createContext<Registry | null>(null);
export function ValidityProvider({ children, onAnyInvalidChange }: { children: React.ReactNode; onAnyInvalidChange?: (v: boolean) => void }) {
  const [ids, setIds] = useState<Set<string>>(() => new Set());
  const setInvalid = useCallback((id: string, invalid: boolean) => setIds(prev => {
    const next = new Set(prev); invalid ? next.add(id) : next.delete(id);
    return next;
  }), []);
  const anyInvalid = ids.size > 0;
  useMemo(() => onAnyInvalidChange?.(anyInvalid), [anyInvalid, onAnyInvalidChange]);
  return <Ctx.Provider value={{ setInvalid, anyInvalid }}>{children}</Ctx.Provider>;
}
export function useValidity() { return useContext(Ctx); }
```

- [ ] **Step 2: `RawJsonField.tsx`** — `RawJsonField({ id, label, fieldKey, value, onChange, placeholder })` where `value` is the parsed JSON (`unknown`) and `onChange(parsed: unknown)`. Internally: hold text state initialised from `JSON.stringify(value, null, 2)`; on change, update text + debounce (~300ms) a `JSON.parse`; on valid → `onChange(parsed)` + clear error + `registry.setInvalid(id, false)`; on invalid → show inline error + `registry.setInvalid(id, true)`. Uses `useValidity()` (no-op if no provider). Header via `FieldLabel`.
- [ ] **Step 3: Convert `EngineAutoscaling.tsx`** — replace its local `RawJsonField` variant (behavior/fallback/raw-trigger textareas) with the new shared `RawJsonField`; wrap the Autoscaling content (or the whole Configuration form) so a provider is available — for now, keep Autoscaling's existing `onValidityChange` prop working by reading `anyInvalid` (bridge). Verify Autoscaling raw-JSON still blocks Save.
- [ ] **Step 4** `tsc`/`build`/`vitest run src/routes/control-plane`. Commit `feat(ui): unified RawJsonField + validity registry`.

---

## Task 3: QuantityInput

**Files:** Create `config/QuantityInput.tsx`; Test `config/QuantityInput.test.tsx`

- [ ] **Step 1: Failing test**

```tsx
import { isValidQuantity } from "./QuantityInput";
test("accepts k8s quantities + trino sizes", () => {
  ["4Gi","500m","2GB","1000","1.5","250Mi",""].forEach(v => expect(isValidQuantity(v)).toBe(true));
});
test("rejects garbage", () => {
  ["4 gigs","abc","4G B","-1x"].forEach(v => expect(isValidQuantity(v)).toBe(false));
});
```

- [ ] **Step 2** Run → FAIL.
- [ ] **Step 3: Implement** — export `isValidQuantity(v: string): boolean` (`v.trim()===""` ok; else match `/^\d+(\.\d+)?\s*(m|k|Ki|Mi|Gi|Ti|Pi|Ei|k|M|G|T|P|E|KB|MB|GB|TB|kB)?$/`) and a `QuantityInput({ fieldKey, label, value, onChange, placeholder })` component wrapping an `<Input>` with a `FieldLabel`, a unit hint (from `fieldHelp[fieldKey]?.unit`), and an inline error when `!isValidQuantity(value)` (does not block save — advisory; the backend validates hard).
- [ ] **Step 4** Run test → pass. `tsc`/`build`. Commit `feat(ui): QuantityInput + k8s-quantity validation`.

---

## Task 4: useEngineConfigForm — dirty-guarded prefill + non-default selectors

**Files:** Modify `useEngineConfigForm.ts`; Test `useEngineConfigForm.nondefault.test.ts` (if a pure selector is extracted)

- [ ] **Step 1: Dirty-guarded prefill** — in the `[engine.spec]` prefill effect, wrap the reset body in `if (!dirty) { ...existing prefill... }` (read the current `dirty` via a ref to avoid stale closure: keep a `dirtyRef` updated each render, check `dirtyRef.current`). So a 5s poll re-render does not discard in-progress edits. First-load prefill (dirty false) still runs.
- [ ] **Step 2: Non-default counts** — export a pure `sectionCounts(payload: TrinoConfig): Record<SectionKey, number>` that, given the built `trinoConfig`, returns how many set fields belong to each section (General/Coordinator/Worker/Workload/Reliability/Advanced/Pod/Placement/ContainersMounts/Autoscaling). Base it on the same omit-empty payload the builder produces (e.g. count present keys per section grouping). The hook exposes `sectionCounts` computed from the current built payload.
- [ ] **Step 3: Test** the counts: a config with `coordinator.replicas` + `autoscaling` set returns `{Coordinator: >=1, Autoscaling: >=1, General: 0, ...}`.
- [ ] **Step 4** `tsc`/`build`/`vitest`. Commit `feat(ui): dirty-guarded prefill + per-section non-default counts`.

---

## Task 5: Configuration → vertical left-nav + badges

**Files:** Modify `EngineDetail.tsx`

- [ ] **Step 1** In the `ConfigurationTab`, replace the horizontal `Tabs`/`TabsList` sub-tab strip with a two-column layout: a left `<nav>` (`w-48 shrink-0`, a vertical list of buttons — one per section, active state styled) + a right content pane rendering the active section. Keep a local `const [section, setSection] = useState("general")`. Section order: `general, coordinator, worker, workload, reliability, advanced, pod, placement, containersMounts, autoscaling`.
- [ ] **Step 2** Each nav button shows the section label + a shadcn `Badge` (secondary, `size` small) with `sectionCounts[key]` when `> 0`. Wrap the content pane in the `ValidityProvider` (from Task 2) with `onAnyInvalidChange={setRawInvalid}`.
- [ ] **Step 3** Fold `rawInvalid` into the Save `disabled` (`|| rawInvalid`), replacing the old `podSchedValid`/`autoscaleValid` booleans (the registry now covers all raw fields). Keep the existing `dirty`/`saving` disables.
- [ ] **Step 4** On narrow screens the left nav stacks above (responsive: `flex-col md:flex-row`). `tsc`/`build`; the nav renders all 10 sections + badges. Commit `feat(ui): Configuration vertical nav + non-default badges`.

---

## Task 6: Split PodSchedulingSection → Pod / Placement / Containers & Mounts

**Files:** Create `config/PodSection.tsx`, `config/PlacementSection.tsx`, `config/ContainersMountsSection.tsx`; Modify `EngineDetail.tsx`; Delete `PodSchedulingSection.tsx` (after moving its parts)

- [ ] **Step 1** Move the JSX + local sub-editors out of `PodSchedulingSection.tsx` (1481 lines) into three components, each taking the relevant `cfg` slices as props:
  - **`PodSection`**: serviceAccount, imagePullSecrets, imagePullPolicy, pod + container securityContext, deployment strategy/annotations/revisionHistoryLimit/progressDeadlineSeconds, shareProcessNamespace.
  - **`PlacementSection`**: the per-role coordinator/worker toggle + nodeSelector, tolerations, topologySpread, affinity, terminationGraceSeconds, additionalExposedPorts.
  - **`ContainersMountsSection`**: initContainers, sidecarContainers (the `MiniContainerEditor`), envFrom, configMounts, secretMounts.
  Move shared sub-editors (`MiniContainerEditor`, toleration/topology/affinity row editors, `RawJsonOverride` — now the shared `RawJsonField`) into whichever new file uses them or a small shared module. Wrap groups in `ConfigSection`.
- [ ] **Step 2** In `EngineDetail.tsx`, render `<PodSection>`, `<PlacementSection>`, `<ContainersMountsSection>` for the `pod`/`placement`/`containersMounts` nav sections. Delete `PodSchedulingSection.tsx` once nothing imports it (move `validateRawJson` to a shared util or replace with `RawJsonField`).
- [ ] **Step 3** `tsc`/`eslint`/`build`; all three sections render + edit + save. Commit `refactor(ui): split PodScheduling into Pod/Placement/Containers&Mounts`.

---

## Task 7: Refactor remaining sections onto the primitives

**Files:** Modify `EngineConfigGeneral/Reliability/Advanced.tsx`, `EngineRoleConfig.tsx`, `EngineResourceGroups.tsx`, `EngineSecurity.tsx`, `EngineAutoscaling.tsx`

- [ ] **Step 1** In each, wrap groups in `ConfigSection` (title/description/docsUrl), replace bare `<Label>` with `<FieldLabel fieldKey=... label=... />` for the fields that have `fieldHelp` entries, swap CPU/mem/heap/size `<Input>`s for `<QuantityInput>`, and replace any remaining raw-JSON textareas with `RawJsonField`. Prioritize `EngineRoleConfig` (Coordinator/Worker — the audit's #1 help gap).
- [ ] **Step 2** Add `fieldHelp` entries as you touch fields (extend `fieldHelp.ts`).
- [ ] **Step 3** `tsc`/`eslint`/`build`/`vitest`; every section renders with consistent section cards + help icons; save unchanged. Commit `refactor(ui): adopt ConfigSection/FieldLabel/QuantityInput across config sections`.

---

## Task 8: Review-changes diff modal

**Files:** Create `config/configDiff.ts`, `config/ReviewChangesDialog.tsx`; Modify `EngineDetail.tsx`; Test `config/configDiff.test.ts`

- [ ] **Step 1: Failing test**

```ts
import { diffConfig } from "./configDiff";
test("diffs added/removed/changed leaf paths", () => {
  const before = { coordinator: { replicas: 1 }, logLevel: "INFO" };
  const after = { coordinator: { replicas: 2 }, queryMaxMemory: "4GB" };
  const d = diffConfig(before, after);
  expect(d).toContainEqual({ path: "coordinator.replicas", kind: "changed", from: 1, to: 2 });
  expect(d).toContainEqual({ path: "logLevel", kind: "removed", from: "INFO" });
  expect(d).toContainEqual({ path: "queryMaxMemory", kind: "added", to: "4GB" });
});
```

- [ ] **Step 2** Run → FAIL.
- [ ] **Step 3** Implement `diffConfig(before, after): {path,kind:"added"|"removed"|"changed",from?,to?}[]` — recursive leaf-path walk over the two objects (JSON-value semantics; arrays compared whole by `JSON.stringify`). `ReviewChangesDialog({ before, after, open, onOpenChange })` — a shadcn `Dialog` listing the diff rows (color-coded add/remove/change) + the note "Saving re-provisions (rolls) the engine pods."
- [ ] **Step 4** In `EngineDetail.tsx`, add a **Review changes** button beside Save (in `SaveBar`) that opens the dialog with `before = engine.spec.trinoConfig ?? {}` and `after = the built trinoConfig`. Disabled when `!dirty`.
- [ ] **Step 5** Run test → pass. `tsc`/`build`. Commit `feat(ui): Review-changes diff modal before save`.

---

## Task 9: Gate + Playwright + PR

- [ ] **Step 1** `cd frontend`: `npx tsc --noEmit` (0), `npm run lint` (0 errors), `npx vitest run` (green), `npm run build`.
- [ ] **Step 2: Playwright** (stack running; login admin@egret.local / Admin12345!): open an engine → Configuration shows the **vertical nav** with all 10 sections; a section with set fields shows a **badge**; hovering a field info icon shows **help**; typing invalid JSON in a `RawJsonField` **disables Save**; **Review changes** opens the **diff modal** listing changed paths; type in a field, wait ~6s (a status poll), confirm the edit is **NOT discarded**; change a field + Save → `updateEngine` PUT **200**.
- [ ] **Step 3: Payload parity** — for an unmodified engine, Save's request body `trinoConfig` matches the pre-refactor shape (no dropped fields).
- [ ] **Step 4** Commit; open PR; run the full frontend gate before an admin squash-merge.

---

## Self-Review notes
- **Spec coverage:** ConfigSection/FieldLabel/fieldHelp (T1) ✓; unified RawJsonField + registry (T2) ✓; QuantityInput (T3) ✓; dirty-guarded prefill + non-default counts (T4) ✓; vertical nav + badges (T5) ✓; split PodScheduling into 3 (T6) ✓; refactor sections onto primitives (T7) ✓; Review-changes diff modal (T8) ✓; verify incl. parity + no-clobber (T9) ✓.
- **Type consistency:** `fieldHelp` key strings ↔ `FieldLabel`/`QuantityInput` `fieldKey` props; `RawJsonField` value is parsed JSON (`unknown`) end-to-end; the validity registry replaces the old `podSchedValid`/`autoscaleValid` booleans (T5 Step 3) so those are removed everywhere.
- **Payload parity:** enforced by refactor discipline + T9 Step 3; no state/payload logic changes.
- **Deferred (YAGNI):** config search box; sticky save bar + unsaved-navigation guard; new Trino config fields (sub-projects #2-#4).
