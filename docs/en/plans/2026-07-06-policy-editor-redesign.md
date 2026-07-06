# Policy Editor Redesign (engine-style, full-page) — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Replace the cramped modal policy editor with a clean **full-page route** matching the Engine pages' visual language (header `Card` + section `Card`s, `FieldLabel`, `space-y-5`), and declutter the resource selector (move Exclude/Recursive into an "Advanced" popover so the default is a readable `catalog.schema.table.column` path).

**Why:** User feedback — the current editor is "quá rối rắm" (too cluttered): 8 inline Exclude/Recursive toggles + everything stacked in one modal. Target style = `EngineDetail.tsx` (full-page, Tabs/Cards, `FieldLabel`).

**Design reference (read these):** `frontend/src/routes/control-plane/EngineDetail.tsx` (header Card + Tabs + Card sections, `space-y-5`), `frontend/src/routes/control-plane/EngineConfigReliability.tsx` (Card + `FieldLabel` + grid fields + `Switch`), `frontend/src/routes/control-plane/config/FieldLabel.tsx` (labeled field w/ tooltip help).

**Depends on:** P1–P4 (branch `feat/data-governance-policies`) — the 3 dialogs (`AccessPolicyDialog`/`MaskingPolicyDialog`/`RowFilterPolicyDialog`), `ResourceLevelEditor`, `PrincipalPicker`, `api/policies.ts` (createPolicy/updatePolicy/getPolicy/getServiceDef/setPolicyEnabled/resourceLookup), `Policies.tsx` list.

**Conventions:** `npm run lint && typecheck && test && build` all green per task. No new deps (shadcn `Card`,`Tabs`,`Switch`,`Popover`,`Textarea`,`Badge`,`Button`,`AlertDialog` all exist).

---

### Task 1: `ResourceLevelEditor` v2 — compact path segment

**Files:** `frontend/src/components/policy/ResourceLevelEditor.tsx`, its test.

Rework so the default is clean and horizontal-friendly:
- Row: the level `label` (small, muted) + the value combobox (badges + input, unchanged) + a small **gear/"Advanced" `Button` (ghost, icon `SlidersHorizontal` or `Settings2`)** that opens a `Popover` containing the two `Switch`es (Exclude, Recursive) with `FieldLabel`-style labels. When either toggle is ON, show a tiny indicator on the gear (e.g. a dot / "•" or make it `variant="secondary"`) so active advanced state is visible without opening.
- Remove the always-visible inline Exclude/Recursive text+switch pair from the header row.
- Keep the same `Props` (`label, level, service, parents, value, onChange`), the same autocomplete query, multi-value badges, and Enter-to-add. Keep `type="text"` on the input (regression guard).
- Keep `aria-label`s (`Exclude {label}` / `Recursive {label}`) on the switches inside the popover so the existing test still finds them (the test opens/queries them — update the test to open the popover first if needed).

- [ ] Update the test: it currently asserts `getByLabelText("Exclude Schema")` is directly present + toggles. Now the switch is inside a Popover — update the test to click the gear/"Advanced" trigger (give it `aria-label="Advanced Schema"`) first, then toggle `Exclude Schema`. Keep asserting `onChange` fires and the "sales" badge renders.
- [ ] `npm run test -- --run ResourceLevelEditor` PASS → `npm run lint && typecheck` → commit `refactor(frontend): compact ResourceLevelEditor (exclude/recursive behind Advanced popover)`.

---

### Task 2: `PolicyEditorPage` — one full-page editor for all 3 kinds

**Files:** create `frontend/src/routes/control-plane/PolicyEditorPage.tsx` + test.

A single page component (route below) that replaces the three Dialog editors. Structure (engine style, `space-y-5`, max width like EngineDetail):

1. **Header `Card`** (`overflow-hidden`, mirror EngineDetail's header): a back link (`ArrowLeft` → the list, `/control-plane/policies`), the policy **Name** `Input` inline-editable (large, `text-2xl`-ish) OR a `FieldLabel` + Input, a **type `Badge`** (Access/Masking/Row-filter), an **Enabled `Switch`**, and right-aligned **Save** (primary) + **Cancel** + **Delete** (`AlertDialog`, edit mode only) buttons.
2. **`Card` "Policy details"**: Description (`Textarea` + `FieldLabel`), Labels (tag input, existing pattern).
3. **`Card` "Resource"**: the four `ResourceLevelEditor`s laid out as a readable path — a responsive grid (`grid gap-4 sm:grid-cols-2 lg:grid-cols-4`) OR a single flex row with "·" separators; for row-filter render only catalog/schema/table (no column). `FieldLabel` "Resource" with help "The Trino resource this policy applies to (wildcards with *)".
4. **`Card` (kind-specific)**:
   - **access**: a `Switch` "Deny everything else not explicitly allowed" (`FieldLabel`); an Allow/Deny `Tabs` (Deny hidden when deny-all-else); each tab = item rows (reuse an `ItemRow`: `PrincipalPicker` + access-type `Checkbox`es from the fetched service-def + remove) + "Add item"; a collapsed `Accordion` "Exceptions" per tab (allow/denyExceptions).
   - **masking**: item rows with `PrincipalPicker` + mask-type `Select` (MASK_TYPES) + CUSTOM `valueExpr` Input.
   - **rowfilter**: item rows with `PrincipalPicker` + filter-expr `Input`.

Port the form-state logic + reverse-maps from the existing dialogs (`AccessPolicyDialog`/`MaskingPolicyDialog`/`RowFilterPolicyDialog` + `Policies.tsx`'s `toInput`/`maskingToInput`/`rowfilterToInput`). The page:
- reads route params `service`, `kind`, optional `id`;
- on edit (`id` present) fetches `getPolicy(id)` + reverse-maps to the form input; on new, uses `blankInput(kind, service)`;
- fetches `getServiceDef(service)` for access-type/mask-type options;
- Save → `createPolicy`/`updatePolicy` (with `kind`), invalidate `["control-plane","policies"]`, `navigate` back to `/control-plane/policies`;
- Delete → `deletePolicy` (edit mode) → navigate back.
Keep it `RequireGlobalAdmin`-gated by living under `ControlPlaneRoutes`.

- [ ] Test (`PolicyEditorPage.test.tsx`): mock `@/api/policies` + `react-router` params; render for `kind=access, new`; fill Name + a catalog value; Save calls `createPolicy` with `kind:"access"` + the catalog value; toggling deny-all-else hides the Deny tab. Mock `resourceLookup`/`getServiceDef`/`listPrincipals`.
- [ ] Gate → commit `feat(frontend): full-page PolicyEditorPage (engine-style) for all 3 policy kinds`.

---

### Task 3: Routing + list wiring; retire the dialogs

**Files:** `frontend/src/App.tsx`, `frontend/src/routes/control-plane/Policies.tsx`, `Policies.test.tsx`.

- [ ] **Routes** in `App.tsx` `ControlPlaneRoutes`:
```tsx
<Route path="policies/:service/:kind/new" element={<PolicyEditorPage />} />
<Route path="policies/:service/:kind/:id" element={<PolicyEditorPage />} />
```
(keep `policies` = the list.)
- [ ] **`Policies.tsx`**: `New policy` → `navigate(`/control-plane/policies/${activeService}/${policyType}/new`)`; row **Edit** → `navigate(`.../${activeService}/${policyType}/${r.id}`)`. Remove the `AccessPolicyDialog`/`MaskingPolicyDialog`/`RowFilterPolicyDialog` imports + render + `editorOpen`/`editing`/`toInput` reverse-map state that moved into the page (keep `deletePolicy`/`setPolicyEnabled`/`ConfirmDeleteDialog` for inline delete + the enable toggle; keep `TrinoConfigDialog`). The three `*PolicyDialog.tsx` files + their tests can be **deleted** (their form logic now lives in `PolicyEditorPage`) — do delete them to avoid dead code, and remove their test files.
- [ ] Update `Policies.test.tsx`: New/Edit now navigate (assert `navigate`/URL) instead of opening a dialog; keep the enable-toggle + labels + render assertions.
- [ ] Gate (`lint && typecheck && test && build`) → commit `feat(frontend): route New/Edit to full-page PolicyEditorPage; retire modal dialogs`.

---

### Task 4: Full gate + manual smoke

- [ ] `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` — all green.
- [ ] Manual smoke (stack already running — backend :8080, frontend :5173, seeded demo): at `/control-plane/policies`, click **New policy** → lands on the full-page editor; the Resource section shows a clean path (no visible Exclude/Recursive until the Advanced gear); create an Access policy; Edit an existing one; confirm the page matches the Engine pages' look (header Card + section Cards + `FieldLabel` spacing). Screenshot access + one other kind. State what was exercised.

---

## Self-Review notes
- **Goal coverage:** full-page route (Tasks 2,3) ✓; engine-style Cards + FieldLabel + space-y-5 (Task 2) ✓; decluttered resource (Exclude/Recursive → Advanced popover) (Task 1) ✓; retire the cramped modal (Task 3) ✓; all 3 kinds unified (Task 2) ✓.
- **Behavior parity:** the page must preserve ALL P4 capability (exceptions, deny-all-else, description, labels, multi-value/exclude/recursive resources, mask types + CUSTOM, filter expr, enabled) — it's a re-layout, not a feature cut.
- **Open confirmations:** keep the `type="text"` fix on inputs; keep `getServiceDef(service)` (backend already resolves service→def); the exact reverse-map field reads (allowExceptions/denyExceptions/isDenyAllElse/policyLabels) copy from the current dialogs verbatim.
