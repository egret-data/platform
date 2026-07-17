# StarRocks Config UX Reorg Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`). Follow superpowers:test-driven-development per task.

**Goal:** Merge the StarRocks Settings "Compute" + "Resources" sections into one, and replace the raw-JSON textareas in "Pod & Containers" (Env / Tolerations / nodeSelector / Init / Sidecars) with structured add-row editors — reusing egret's existing Trino config primitives + JSON↔structured converters. Affinity stays a labelled Advanced-JSON field.

**Architecture:** FE-only. The wire `starrocksConfig.pod` JSON + dirty/save (`normPod`) are UNCHANGED — only the input widgets change + convert to/from that JSON. Reuse `KeyValueEditor` (Kv[]) + extract `TolerationsEditor`/`ContainerListEditor` from the Trino sections.

**Tech Stack:** React 18 + TS + Vite + Vitest + Tailwind v4 + shadcn.

**Spec:** `docs/en/specs/2026-07-17-starrocks-config-ux-reorg-design.md`. **Branch:** `feat/starrocks-config-ux-reorg` (off `main`).

**Reference anchors** (`frontend/src/routes/control-plane/`): `EngineResourceGroups.tsx` (`KeyValueEditor`@93 `{rows:Kv[], onChange}`, `Kv = {key,value}`@12, `kvToRecord`@18), `config/PlacementSection.tsx` (toleration rows @72-193, `TolerationForm = {key,operator,value,effect,tolerationSeconds}`), `config/ContainersMountsSection.tsx` (`MiniContainerEditor`@198 / `MiniContainerList`@338 — currently private), `config/RawJsonField.tsx` + `config/validityRegistry`, `StarRocksConfigTab.tsx` (`NAV_ITEMS`@32, `compute`@166 / `resources`@167, `ConfigMapEditor`@314, `CommaListInput`@433, `PodContainersSection`@473), `useStarRocksConfigForm.ts` (the `pod` block + `normPod`).

---

## Task 1: Merge Compute + Resources into one section

**Files:** `StarRocksConfigTab.tsx`.

- [ ] **Step 1: Failing test** (`__tests__/StarRocksConfigTab.test.tsx`): the nav has ONE "Compute & Resources" item (not separate "Compute"/"Resources"); selecting it renders BOTH the FE/CN replica inputs AND the CPU min/max + memory inputs; `?section=resources` (legacy) resolves to the merged section. Run → FAIL.

- [ ] **Step 2: Merge.** Replace the `{ key:"compute", label:"Compute" }` + `{ key:"resources", label:"Resources" }` `NAV_ITEMS` entries with one `{ key:"compute", label:"Compute & Resources" }`. In the content switch, `section === "compute"` renders both `<ComputeSection cfg={cfg}/>` + `<ResourcesSection cfg={cfg}/>` stacked (a heading each, or subheadings). Alias `?section=resources` → treat as `compute` (in the section-resolve: `const section = raw === "resources" ? "compute" : (raw ?? "image")`). Nav badge for `compute` = the sum of the compute + resources non-default counts (`srSectionCounts` — combine the two).

- [ ] **Step 3: Run → PASS.** `cd frontend`: `npx vitest run src/routes/control-plane` green; `npm run typecheck` (known error only); `npx eslint` touched files clean.

- [ ] **Step 4: Commit.** `feat(fe): merge StarRocks Compute + Resources into one section`.

---

## Task 2: Extract reusable `TolerationsEditor` + `ContainerListEditor`

**Files:** `config/PlacementSection.tsx`, `config/ContainersMountsSection.tsx` (extract + export), no Trino behavior change.

- [ ] **Step 1: Failing test** (`config/__tests__/`): `TolerationsEditor` renders toleration rows (key/operator/value/effect) + add/remove, calls `onChange(rows)`; `ContainerListEditor` renders container rows (name/image/command/env + advanced) + add/remove. Run → FAIL.

- [ ] **Step 2: Extract `TolerationsEditor({ rows, onChange })`** — pull the toleration-rows JSX from `PlacementSection` (@72-193) into an exported standalone component taking `rows: TolerationForm[]` + `onChange: (rows) => void`; have `PlacementSection` consume it (so Trino is unchanged — same rendered output). Export `TolerationForm`.

- [ ] **Step 3: Export `ContainerListEditor`** — `MiniContainerList` (@338) is already a list of `MiniContainerEditor`; export it (rename to `ContainerListEditor` if clearer) with a `{ rows: ContainerForm[], onChange }` surface; keep `ContainersMountsSection` consuming it unchanged. Export the `ContainerForm` type.

- [ ] **Step 4: Run → PASS.** `npx vitest run src/routes/control-plane/config` (Trino config tests still green — no regression); typecheck; eslint.

- [ ] **Step 5: Commit.** `refactor(fe): extract reusable TolerationsEditor + ContainerListEditor`.

---

## Task 3: JSON↔structured converters (`starrocksPodShape.ts`)

**Files:** new `frontend/src/routes/control-plane/starrocksPodShape.ts` + test.

- [ ] **Step 1: Failing tests** (`__tests__/starrocksPodShape.test.ts`): round-trip each converter (structured → JSON → structured is stable), incl. empty (`[]`/`{}` → `undefined`/omit) and malformed-preserving:
  - `envJsonToKv([{name:"TZ",value:"UTC"}]) → [{key:"TZ",value:"UTC"}]` and `kvToEnvJson` back.
  - `recordFromKv`/`kvFromRecord` for nodeSelector/podLabels/annotations (`{k:v}`↔`Kv[]`).
  - `tolerationsJsonToRows`/`rowsToTolerationsJson` (`[{key,operator,value,effect}]`↔`TolerationForm[]`).
  - `containersJsonToForms`/`formsToContainersJson` (`[Container]`↔`ContainerForm[]`), preserving unknown container keys via the form's raw-override.
  Run → FAIL.

- [ ] **Step 2: Implement** the pure converters. Determinism: preserve array order; for records, iterate `Object.entries` in a stable order. Malformed/unknown: if a value isn't the expected array/object, keep it in the paired raw-override rather than dropping (env/tolerations/containers each round-trip an `advanced` raw string when the structured form can't represent it).

- [ ] **Step 3: Run → PASS.** vitest; typecheck; eslint.

- [ ] **Step 4: Commit.** `feat(fe): StarRocks pod JSON<->structured converters`.

---

## Task 4: Rebuild `PodContainersSection` with structured editors

**Files:** `StarRocksConfigTab.tsx`, `useStarRocksConfigForm.ts` (expose pod sub-setters if needed).

- [ ] **Step 1: Failing tests** (`StarRocksConfigTab.test.tsx`): in Pod & Containers, adding an Env row (name=TZ, value=UTC) → Save → PUT `starrocks.pod.env == [{name:"TZ",value:"UTC"}]`; adding a nodeSelector row → `pod.nodeSelector == {k:v}`; adding a toleration row → `pod.tolerations == [{key,operator,value,effect}]`; adding an init container (name/image) → `pod.initContainers == [{name,image}]`; **invalid Affinity Advanced-JSON blocks Save**; a config-only save still carries an existing pod block. Run → FAIL.

- [ ] **Step 2: Rebuild `PodContainersSection`** grouped into 4 labelled sub-groups, each editor bound to the hook's `pod` block via the Task-3 converters (seed = parse pod JSON → structured; onChange = structured → pod JSON via `cfg.setPod`):
  - **Environment** — env → `KeyValueEditor` (name/value; label the columns "name"/"value" via the editor's labels).
  - **Scheduling** — nodeSelector → `KeyValueEditor`; tolerations → `TolerationsEditor` (Task 2); affinity → keep `RawJsonField` labelled **"Affinity (advanced JSON)"** + a one-line note.
  - **Containers** — initContainers + sidecars → `ContainerListEditor` (Task 2) each.
  - **Identity** — serviceAccount (Input), imagePullSecrets (`CommaListInput`, keep), podLabels + annotations → `KeyValueEditor`.
  Keep the `ValidityProvider` wrapping the affinity + per-container advanced raw fields so invalid JSON blocks Save; keep `<SaveBar disabled={cfg.autoscalingInvalid || podRawInvalid}/>`. Use the functional `cfg.setPod((prev)=>…)` updater (from #267) for each editor's onChange.

- [ ] **Step 3: Run → PASS.** `npx vitest run src/routes/control-plane` green; typecheck (known error only); eslint.

- [ ] **Step 4: Commit.** `feat(fe): structured StarRocks Pod & Containers editors (replace raw JSON)`.

---

## Task 5: FE CI + live smoke + screenshots

- [ ] **Step 1: FE CI.** `cd frontend`: `npm run lint` (0 errors; 3 pre-existing non-epic warnings allowed), `npm run typecheck` (known `useEngineConfigForm.dirty.test.tsx` error only), `npx vitest run src/routes/control-plane`, `npm run build` (blocked only by the known error).

- [ ] **Step 2: Live (browser).** On a StarRocks engine → Settings → Pod & Containers: confirm structured add-row editors (Env name/value, Tolerations key/operator/value/effect, nodeSelector, init/sidecar container editors) — NO raw textarea except the labelled "Affinity (advanced JSON)"; Compute & Resources is one section. Add rows via the forms → Save → confirm the PUT `pod` JSON. Screenshots at 1440.

- [ ] **Step 3: Report** exactly what was verified.

---

## Self-Review

- **Spec coverage:** merge → T1; extract editors → T2; converters → T3; rebuild pod section → T4; live → T5. Covered.
- **Type consistency:** `Kv`/`TolerationForm`/`ContainerForm` (T2 exports) are the structured shapes the T3 converters bridge to the `pod` JSON that T4's editors read/write via `cfg.setPod`. `normPod`/dirty (from #266/#267) unchanged — same PUT shape.
- **No placeholders:** the reused `KeyValueEditor`, the extracted `TolerationsEditor`/`ContainerListEditor`, the named converters, and the affinity-stays-Advanced-JSON decision are concrete.
- **No regression:** T2 extracts without changing the Trino sections' output; T4 keeps the ValidityProvider + SaveBar gate.
