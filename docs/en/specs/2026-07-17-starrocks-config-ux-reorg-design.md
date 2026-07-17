# StarRocks Config UX Reorg (structured editors, merged sections) — Design

**Date:** 2026-07-17 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-config-ux-reorg` (off `main`)
**Skill:** ui-ux-pro-max (Forms & Feedback). User feedback on the deep-config Settings tab: merge Compute + Resources, and replace the raw-JSON textareas in **Pod & Containers** (Env / Tolerations / Affinity / Init containers / Sidecars) with structured "add a property"-style editors.

> Vietnamese reference: `docs/vi/specs/2026-07-17-starrocks-config-ux-reorg-design.md`.

## Problem (review)

The StarRocks Settings tab (from #266/#267) has 9 sections and forces **raw-JSON entry** for the pod fields — the admin types `[{"name":"TZ","value":"UTC"}]`, `[{"key":"dedicated","operator":"Exists"}]`, `{"nodeAffinity":{}}`, `[{"name":"wait","image":"busybox"}]` by hand. This is error-prone and violates Forms best practice (`input-labels`, avoid raw JSON where a structured form fits). Two issues:
1. **Compute (FE/CN replicas) and Resources (CPU/mem) are separate sections** — both are cluster sizing; they should be one.
2. **Pod & Containers is one flat block of raw-JSON textareas** mixing scheduling + containers + env + identity.

egret already has the structured primitives (Trino's config editors): `KeyValueEditor` (`Kv[]` name/value rows), a toleration-rows editor + affinity editor inside `PlacementSection`, and a mini-container editor inside `ContainersMountsSection`. This reorg reuses/extracts them for StarRocks with JSON↔structured bridging.

## Goal

Reorganize the StarRocks Settings tab so cluster sizing is one section and Pod & Containers uses **structured editors** (add-row forms) instead of raw JSON, keeping a raw "Advanced (JSON)" escape only where the k8s shape is genuinely too nested for a form (affinity). Behavior + save/dirty semantics unchanged — the PUT body still carries the same `starrocksConfig.pod` JSON.

**Sections after (8, from 9):** `Image · Compute & Resources · FE config · CN config · Autoscaling · Pod & Containers · Storage · Security & Delegation`.

## Non-goals

- **No backend change** — the wire shape (`pod.{env,tolerations,affinity,initContainers,sidecars,nodeSelector,…}` JSON) is unchanged; only the FE input widgets change + convert to/from that JSON.
- **No change to the other engines** — Trino's config tab is untouched (it already has these editors).
- **No new k8s field support** — same pod fields as #266.

## Background — reusable primitives (verified)

- `KeyValueEditor` (`../EngineResourceGroups`, `Kv[]` = `{key,value}[]`) — used by `PodSection` for env + by `KeyedPropertyList` internals. Reuse for **env** (name/value) and **nodeSelector** (key/value).
- `PlacementSection` (`config/PlacementSection.tsx`) holds a **tolerations rows editor** (key/operator/value/effect) + an **affinity** editor (nodeMatch rows + a raw `RawJsonField` override) — but it's bound to Trino's `RoleForm`. Extract the toleration-rows sub-widget into a standalone `TolerationsEditor({ rows, onChange })`.
- `ContainersMountsSection` (`config/ContainersMountsSection.tsx`) holds a **mini-container editor** (name/image, command/args string-lists, env KV, volume mounts, + a raw override) bound to a `ContainerForm`. Extract it into a standalone `ContainerListEditor({ rows, onChange })`.
- `RawJsonField` + `validityRegistry`/`ValidityProvider` — keep for the affinity "Advanced" escape (invalid JSON blocks Save, as today).

## Architecture — components

### A. Merge Compute + Resources (`StarRocksConfigTab.tsx`)
Replace the `compute` + `resources` nav items with one `{ key: "compute", label: "Compute & Resources" }`; the section renders FE/CN replicas **and** CPU min/max + memory together (reuse the existing `ComputeSection` + `ResourcesSection` bodies stacked under one heading). Nav badge = the union of the two sections' non-default counts. Preserve `?section=compute` deep-link; alias/redirect a stale `?section=resources` to `compute`.

### B. Structured Pod & Containers (`StarRocksConfigTab.tsx` + small extractions in `config/`)
Rebuild `PodContainersSection` grouped into four labelled sub-groups, each field a structured editor bound to the hook's `pod` block via **converters** (parse the stored JSON → structured rows on seed; serialize rows → JSON on change):
- **Environment** — `env`: `KeyValueEditor` (`Kv[]`), converts to/from `[{name,value}]`.
- **Scheduling** — `nodeSelector`: `KeyValueEditor` (`{k:v}`); `tolerations`: the extracted `TolerationsEditor` (key/operator/value/effect rows), converts to/from `[{key,operator,value,effect}]`; `affinity`: keep `RawJsonField` labelled **"Affinity (advanced JSON)"** with a one-line note (the nested nodeAffinity/podAffinity shape has no compact form — the one justified raw field).
- **Containers** — `initContainers` + `sidecars`: the extracted `ContainerListEditor` (name/image/command/env per container + an Advanced-JSON override per container), converts to/from `[Container]`.
- **Identity** — serviceAccount (input), imagePullSecrets (comma list — keep), podLabels + annotations (`KeyValueEditor`).
The single `ValidityProvider` still wraps the raw-JSON escapes (affinity + per-container advanced) so invalid JSON blocks Save; the `SaveBar disabled` gate keeps `autoscalingInvalid || podRawInvalid`.

### C. Converters (in `useStarRocksConfigForm.ts` or a small `starrocksPodShape.ts`)
Pure functions bridging the stored `pod` JSON ↔ the structured editor shapes: `envJsonToKv`/`kvToEnvJson` (`[{name,value}]`↔`Kv[]`), `nodeSelectorJsonToKv`/back (`{k:v}`↔`Kv[]`), `tolerationsJsonToRows`/back, `containersJsonToForms`/back (with a raw fallback preserving unknown keys). `normPod` (dirty/save) is unchanged — the section still writes the same `pod` JSON, so dirty/omit-preserve semantics from #266/#267 are untouched.

## Data flow

Seed: `engine.spec.starrocksConfig.pod` (now surfaced by the #267 DTO fix) → converters → structured rows. Edit a row → converter → the hook's `pod` JSON → `normPod` → dirty/save (unchanged). Invalid Advanced-JSON → ValidityProvider → Save disabled. PUT body identical to today.

## Testing strategy

- **Unit (Vitest):** the converters round-trip (`env`/`nodeSelector`/`tolerations`/`containers` JSON ↔ structured, incl. empty + malformed-preserved); adding an env row (name/value) → the PUT `pod.env` = `[{name,value}]`; adding a toleration row → `pod.tolerations` = `[{key,operator,value,effect}]`; adding an init container (name/image) → `pod.initContainers` = `[{name,image}]`; invalid Affinity JSON blocks Save; Compute & Resources is one section editing both replicas + resources; `?section=resources` redirects to `compute`. Existing StarRocks config tests stay green (the PUT shape is unchanged).
- **Live (browser):** on a StarRocks engine — Pod & Containers shows structured add-row editors (no raw textarea except the labelled Advanced ones); add an env var + a nodeSelector + a toleration + an init container via the forms → Save → confirm the PUT body carries the right JSON; Compute & Resources merged. Screenshots.

## Risks

1. **Converter fidelity** — malformed/unknown keys in a stored `pod` (e.g. an advanced affinity, a container field the form doesn't expose) must be PRESERVED, not dropped: each structured editor pairs with a raw "Advanced" escape that round-trips unknown keys (mirror how Trino's editors keep a raw override).
2. **Extraction without Trino regression** — extracting `TolerationsEditor`/`ContainerListEditor` from `PlacementSection`/`ContainersMountsSection` must not change the Trino sections' behavior (they keep consuming the extracted widget).
3. **Dirty consistency** — converters must be stable (sorted keys / fixed order) so a seed→no-edit round-trip isn't spuriously dirty (the #265/#266 dirty discipline).

## Work breakdown (for the plan)

1. **Merge Compute + Resources** — one nav section + badge + `?section=` alias; tests.
2. **Extract `TolerationsEditor` + `ContainerListEditor`** from the Trino sections (no Trino regression); tests.
3. **Converters** (`starrocksPodShape.ts`) — env/nodeSelector/tolerations/containers JSON↔structured, malformed-preserving; unit tests.
4. **Rebuild `PodContainersSection`** with the structured editors grouped (Environment/Scheduling/Containers/Identity), affinity as the labelled Advanced-JSON escape, ValidityProvider intact; tests.
5. **Live smoke + screenshots.**

## Self-review

- **Scope focused:** FE-only; merge two sections + structured pod editors reusing/extracting the existing Trino primitives; the wire JSON + dirty/save semantics unchanged. ✔
- **No placeholders:** the reused primitives (`KeyValueEditor`, `PlacementSection` tolerations, `ContainersMountsSection` mini-container, `RawJsonField`/`ValidityProvider`), the converters, and the affinity-stays-JSON decision are named. ✔
- **Honest:** affinity keeps a raw (labelled Advanced) field — the one k8s shape with no compact form; every structured editor keeps a raw escape so nothing is dropped. ✔
- **Ambiguity:** 8 sections; env/nodeSelector/podLabels/annotations = KeyValueEditor; tolerations/containers = extracted structured editors; affinity = Advanced JSON. ✔
