# EngineDetail Configuration IA Re-architecture — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorganize the engine detail page into Overview / Configuration / Security & Exposure / Monitor top-level tabs with focused Configuration sub-tabs, and split the 1885-line `EngineDetail.tsx` into a shared config-form hook + focused sub-tab components.

**Architecture:** A `useEngineConfigForm(engine)` hook becomes the single source of truth for all Trino config form state (prefill, dirty, the `updateEngine` save, and the payload builder). `EngineDetail.tsx` becomes a thin orchestrator that distributes the hook's state slices into extracted presentational components. **Frontend only — the PUT payload is byte-identical to today.**

**Tech Stack:** React 18, @tanstack/react-query, shadcn/ui (Tabs, Card, Input, Select, Switch), Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-04-engine-config-ia-design.md`.

---

## Refactor discipline (read first)

This is a **pure move** of existing, working JSX. The invariant that guards every task:
**the `trinoConfig` PUT payload for an unmodified engine must not change.** After each
task run `npm run build` + reload the engine page and confirm every field still prefills.
When a task says "move lines A–B into component X", it means: cut that JSX verbatim,
paste it into the new component, and rename the free variables it references to the props
the component receives (the names are kept identical, so the body is unchanged). Do **not**
rewrite logic, re-order fields, or "improve" anything mid-move.

## File Structure

- **Create** `frontend/src/routes/control-plane/useEngineConfigForm.ts` — all config form state + prefill effect + `dirty` + `saveMutation` + `buildTrinoConfig()` + `save()`.
- **Create** `frontend/src/routes/control-plane/EngineRoleConfig.tsx` — per-role config (coordinator & worker).
- **Create** `frontend/src/routes/control-plane/EngineConfigGeneral.tsx`, `EngineConfigReliability.tsx`, `EngineConfigAdvanced.tsx`, `EngineSecurity.tsx`.
- **Reuse** `frontend/src/routes/control-plane/EngineResourceGroups.tsx` (`ResourceGroupsSection` + `SessionPropertiesSection`) for the Workload sub-tab.
- **Modify** `frontend/src/routes/control-plane/EngineDetail.tsx` — thin orchestrator (top-level Tabs, Overview, distribute hook slices).

---

## Task 1: Extract `useEngineConfigForm` hook (foundation)

**Files:** Create `useEngineConfigForm.ts`; Modify `EngineDetail.tsx`

Move — verbatim — from `EngineDetail.tsx` into the hook: the config `useState` block
(currently lines ~701–771: `queryMaxMemory`, `logLevel`, `configProps`, `env`,
`eventListener`, `nodeProps`, `logProps`, `ftEnabled`, `retryPolicy`,
`exchangeStorageConnId`, `exchangePrefix`, `coordinator`, `worker`, `gsEnabled`,
`gracePeriod`, `resourceGroups`, `sessionRules`, `passwordUsers`, `jwtEnabled`,
`jwksUri`, `requiredAudience`, `ingressEnabled`, `ingressHost`, `ingressClassName`,
`ingressTlsSecret`, `ingressAnnotations`, `error`), the prefill `useEffect` (lines
~774–804), the `saveMutation` (806–810), and the payload-building body of `handleSubmit`
(from line 816 through the `updateEngine`/`saveMutation.mutate` call). Also move the
config-only helpers/types they depend on if they are private to this file: `RoleForm`,
`roleToForm`, `formToRole`, `SessionRuleForm`, `sessionRulesToForm`, `PasswordUserForm`.
Keep `Kv`/`recordToKv`/`kvToRecord` imported from `EngineResourceGroups.tsx` as today.

- [ ] **Step 1: Create the hook** with this exact public shape:

```ts
// useEngineConfigForm.ts
import type { Engine, TrinoConfig, CreateEngineInput } from "@/api/controlPlane";

export interface EngineConfigForm {
  // cluster-wide
  queryMaxMemory: string; setQueryMaxMemory: (v: string) => void;
  logLevel: string; setLogLevel: (v: string) => void;
  configProps: Kv[]; setConfigProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  env: Kv[]; setEnv: React.Dispatch<React.SetStateAction<Kv[]>>;
  // advanced raw
  eventListener: Kv[]; setEventListener: React.Dispatch<React.SetStateAction<Kv[]>>;
  nodeProps: Kv[]; setNodeProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  logProps: Kv[]; setLogProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  // reliability
  ftEnabled: boolean; setFtEnabled: (v: boolean) => void;
  retryPolicy: "TASK" | "QUERY"; setRetryPolicy: (v: "TASK" | "QUERY") => void;
  exchangeStorageConnId: string; setExchangeStorageConnId: (v: string) => void;
  exchangePrefix: string; setExchangePrefix: (v: string) => void;
  // per-role
  coordinator: RoleForm; setCoordinator: React.Dispatch<React.SetStateAction<RoleForm>>;
  worker: RoleForm; setWorker: React.Dispatch<React.SetStateAction<RoleForm>>;
  gsEnabled: boolean; setGsEnabled: (v: boolean) => void;
  gracePeriod: string; setGracePeriod: (v: string) => void;
  // workload
  resourceGroups: TrinoConfig["resourceGroups"]; setResourceGroups: React.Dispatch<React.SetStateAction<TrinoConfig["resourceGroups"]>>;
  sessionRules: SessionRuleForm[]; setSessionRules: React.Dispatch<React.SetStateAction<SessionRuleForm[]>>;
  // security & exposure
  passwordUsers: PasswordUserForm[]; setPasswordUsers: React.Dispatch<React.SetStateAction<PasswordUserForm[]>>;
  jwtEnabled: boolean; setJwtEnabled: (v: boolean) => void;
  jwksUri: string; setJwksUri: (v: string) => void;
  requiredAudience: string; setRequiredAudience: (v: string) => void;
  ingressEnabled: boolean; setIngressEnabled: (v: boolean) => void;
  ingressHost: string; setIngressHost: (v: string) => void;
  ingressClassName: string; setIngressClassName: (v: string) => void;
  ingressTlsSecret: string; setIngressTlsSecret: (v: string) => void;
  ingressAnnotations: Kv[]; setIngressAnnotations: React.Dispatch<React.SetStateAction<Kv[]>>;
  // meta
  error: string | null;
  dirty: boolean;
  saving: boolean;
  save: () => void;   // builds the payload + saveMutation.mutate; sets error on throw
}

export function useEngineConfigForm(engine: Engine): EngineConfigForm { /* moved code */ }
```

  Convert `handleSubmit(e)` into `save()` (drop the `e.preventDefault()` — the caller
  is a button, not a form submit; keep everything after it identical, ending in
  `saveMutation.mutate(input)`).

- [ ] **Step 2: dirty flag** — add inside the hook:

```ts
  const [dirty, setDirty] = useState(false);
  const skipDirty = useRef(true); // skip the initial mount + every prefill
  useEffect(() => {
    if (skipDirty.current) { skipDirty.current = false; return; }
    setDirty(true);
  }, [queryMaxMemory, logLevel, configProps, env, eventListener, nodeProps, logProps,
      ftEnabled, retryPolicy, exchangeStorageConnId, exchangePrefix, coordinator, worker,
      gsEnabled, gracePeriod, resourceGroups, sessionRules, passwordUsers, jwtEnabled,
      jwksUri, requiredAudience, ingressEnabled, ingressHost, ingressClassName,
      ingressTlsSecret, ingressAnnotations]);
```

  In the existing prefill `useEffect` (the one keyed on `[engine.spec]`), add at the end:
  `skipDirty.current = true; setDirty(false);` (so a re-sync doesn't mark the form dirty).
  In `saveMutation.onSuccess`, add `setDirty(false)`. Expose `saving: saveMutation.isPending`.

- [ ] **Step 3: Consume in EngineDetail** — replace the removed state/effect/mutation/
  handler with `const cfg = useEngineConfigForm(engine);` and destructure the names the
  existing JSX uses: `const { queryMaxMemory, setQueryMaxMemory, /* …all… */, save, saving, dirty, error } = cfg;`.
  The existing `<form onSubmit={handleSubmit}>` becomes a `<div>` (or keep the form but
  make the button `type="button" onClick={save}`); the "Save changes" button becomes
  `<Button type="button" onClick={save} disabled={!dirty || saving}>`. The rest of the JSX
  is unchanged.

- [ ] **Step 4: Verify** — `cd frontend && npx tsc --noEmit` clean; `npm run build`. Load an
  engine: every field prefills; edit a field → Save enables; Save → PUT succeeds (Network
  tab shows the same `trinoConfig` shape as before). Keep vitest green.
- [ ] **Step 5: Commit** `refactor(ui): extract useEngineConfigForm hook from EngineDetail`.

---

## Task 2: Extract `EngineRoleConfig` (coordinator + worker)

**Files:** Create `EngineRoleConfig.tsx`; Modify `EngineDetail.tsx`

The current Coordinator tab body (~lines 1313–1320) and Worker tab body (~1321–1414)
render per-role fields off `coordinator`/`setCoordinator` and `worker`/`setWorker` (+ the
worker-only graceful-shutdown `gsEnabled`/`gracePeriod`).

- [ ] **Step 1: Create the component**:

```tsx
// EngineRoleConfig.tsx
import type { RoleForm } from "./useEngineConfigForm";
export function EngineRoleConfig(props: {
  role: "coordinator" | "worker";
  value: RoleForm;
  onChange: React.Dispatch<React.SetStateAction<RoleForm>>;
  // worker-only graceful shutdown (undefined for coordinator):
  gsEnabled?: boolean; onGsEnabledChange?: (v: boolean) => void;
  gracePeriod?: string; onGracePeriodChange?: (v: string) => void;
}) { /* moved role JSX */ }
```

  Export `RoleForm` from `useEngineConfigForm.ts`. Move the shared per-role JSX (replicas,
  resources, heap, probes, additional JVM config, heap-headroom) into the body, referencing
  `props.value`/`props.onChange` in place of the role's state var. Render the graceful-shutdown
  block only when `props.role === "worker"` (guarded by the `gs*` props being present).

- [ ] **Step 2: Use it** — replace the Coordinator tab body with
  `<EngineRoleConfig role="coordinator" value={coordinator} onChange={setCoordinator} />`
  and the Worker tab body with
  `<EngineRoleConfig role="worker" value={worker} onChange={setWorker} gsEnabled={gsEnabled} onGsEnabledChange={setGsEnabled} gracePeriod={gracePeriod} onGracePeriodChange={setGracePeriod} />`.
- [ ] **Step 3: Verify** `tsc` + build; both role tabs render + edit + save as before.
- [ ] **Step 4: Commit** `refactor(ui): extract EngineRoleConfig (coordinator + worker)`.

---

## Task 3: Extract General + Reliability + Advanced

**Files:** Create `EngineConfigGeneral.tsx`, `EngineConfigReliability.tsx`, `EngineConfigAdvanced.tsx`; Modify `EngineDetail.tsx`

From the current Cluster tab (~lines 928–1312), split three groups into components. Each
takes exactly the hook slice it needs:

- [ ] **Step 1: `EngineConfigGeneral.tsx`** — props `{ queryMaxMemory, setQueryMaxMemory, logLevel, setLogLevel, configProps, setConfigProps, env, setEnv }`. Move the "Query max memory", "Log level", "Config properties" (KeyValueEditor), and "Environment variables" sections into it.
- [ ] **Step 2: `EngineConfigReliability.tsx`** — props `{ ftEnabled, setFtEnabled, retryPolicy, setRetryPolicy, exchangeStorageConnId, setExchangeStorageConnId, exchangePrefix, setExchangePrefix }` + it needs the storage-connection list (keep the existing `listStorage` query — either move that `useQuery` into this component or pass the storages down as a prop; prefer moving the `useQuery` in). Move the "Fault-tolerant execution" section in.
- [ ] **Step 3: `EngineConfigAdvanced.tsx`** — props `{ eventListener, setEventListener, nodeProps, setNodeProps, logProps, setLogProps }`. Move the "Event listener", "Additional node properties", "Additional log properties" sections in.
- [ ] **Step 4: Wire** — in EngineDetail, temporarily render these three inside the existing Cluster tab in place of the moved JSX (the IA flip is Task 5). `tsc` + build; each section renders + edits + saves.
- [ ] **Step 5: Commit** `refactor(ui): extract General/Reliability/Advanced config sections`.

---

## Task 4: Extract `EngineSecurity`

**Files:** Create `EngineSecurity.tsx`; Modify `EngineDetail.tsx`

- [ ] **Step 1** Move the "Exposure & auth" section (password users, JWT switch + jwks/audience,
  ingress switch + host/className/tlsSecret/annotations) into `EngineSecurity.tsx` with props
  `{ passwordUsers, setPasswordUsers, jwtEnabled, setJwtEnabled, jwksUri, setJwksUri, requiredAudience, setRequiredAudience, ingressEnabled, setIngressEnabled, ingressHost, setIngressHost, ingressClassName, setIngressClassName, ingressTlsSecret, setIngressTlsSecret, ingressAnnotations, setIngressAnnotations }`. It also uses the `listSecrets` query — move that `useQuery` into the component.
- [ ] **Step 2** Temporarily render `<EngineSecurity {...} />` where the section was (still in the Cluster tab; moved to its own top-level tab in Task 5). `tsc` + build; renders + edits + saves.
- [ ] **Step 3: Commit** `refactor(ui): extract EngineSecurity (exposure & auth)`.

---

## Task 5: Restructure the top-level IA

**Files:** Modify `EngineDetail.tsx`

Now flip the layout. The extracted components make this a composition change.

- [ ] **Step 1: Top-level tabs** — change the top `<TabsList>` from `Overview | Monitor` to
  four triggers: `Overview | Configuration | Security & Exposure | Monitor`. Keep the
  `Tabs` value state.
- [ ] **Step 2: Overview tab** — contains only the Status/pods card, the Catalogs card, and
  the Danger zone (remove the Configuration card from Overview).
- [ ] **Step 3: Configuration tab** — a `Card` titled "Configuration" whose body is a
  sub-tab strip with SIX triggers in this order: `General | Coordinator | Worker | Workload | Reliability | Advanced`, mapping to:
  - General → `<EngineConfigGeneral {...} />`
  - Coordinator → `<EngineRoleConfig role="coordinator" .../>`
  - Worker → `<EngineRoleConfig role="worker" .../>`
  - Workload → `<ResourceGroupsSection groups={resourceGroups} onChange={setResourceGroups} />` + `<SessionPropertiesSection rules={sessionRules} onChange={setSessionRules} />` (imported from `EngineResourceGroups.tsx` — match their current prop names as used today).
  - Reliability → `<EngineConfigReliability {...} />`
  - Advanced → `<EngineConfigAdvanced {...} />`
  Below the sub-tabs render `{error && <p …>{error}</p>}` and the **Save changes** button
  (`onClick={save} disabled={!dirty || saving}`).
- [ ] **Step 4: Security & Exposure tab** — renders `<EngineSecurity {...} />` plus its own
  **Save changes** button (same `save`/`dirty`/`saving`). Remove the old `cluster` sub-tab
  and the Configuration-card-inside-Overview entirely.
- [ ] **Step 5: Verify** `tsc` + `eslint` + `build`. The four top tabs render; the six
  Configuration sub-tabs render; Workload shows resource groups + session; Security shows
  password users + ingress; Save works from both Configuration and Security.
- [ ] **Step 6: Commit** `feat(ui): re-architect EngineDetail into Overview/Configuration/Security/Monitor tabs`.

---

## Task 6: Gate + Playwright + PR

- [ ] **Step 1** `cd frontend`: `npx tsc --noEmit` (0), `npm run lint` (0 errors), `npx vitest run` (green apart from the pre-existing Dashboards failure), `npm run build`.
- [ ] **Step 2: Playwright** (stack already runnable; login admin@egret.local / Admin12345!):
  navigate to an engine → confirm the 4 top tabs. Open **Configuration** → click through all
  6 sub-tabs (each renders its fields; e.g. Worker shows replicas prefilled). Change worker
  replicas → Save → the `updateEngine` PUT returns 200 and the pods roll. Open **Security &
  Exposure** → the existing config prefills and its Save works. Confirm 0 console errors.
- [ ] **Step 3: Payload parity** — before/after: with an unmodified engine, trigger Save and
  confirm the request body `trinoConfig` matches the pre-refactor shape (no dropped fields).
- [ ] **Step 4** Commit; open PR; run the full frontend gate before an admin squash-merge.

---

## Self-Review notes

- **Spec coverage:** top-level IA (Task 5) ✓; 6 Configuration sub-tabs (Task 5) ✓; component
  extraction incl. shared `EngineRoleConfig` + `useEngineConfigForm` (Tasks 1–4) ✓; Workload
  reuses `EngineResourceGroups` (Task 5) ✓; Save on both Configuration + Security (Task 5) ✓;
  no-backend-change / identical payload (refactor discipline + Task 6 Step 3) ✓; prefill/re-sync
  preserved (Task 1) ✓.
- **Type consistency:** `useEngineConfigForm` exports `RoleForm`, `SessionRuleForm`,
  `PasswordUserForm`; `EngineRoleConfig` imports `RoleForm` from it. All setter prop names on
  the sub-components equal the hook's setter names (mechanical move).
- **`dirty`** uses a `skipDirty` ref reset in the prefill effect so a re-sync doesn't dirty the
  form; reset to `false` on save success.
- **No behavior change:** the PUT payload is built by the same (moved) code path; Task 6 Step 3
  verifies parity.
- **Deferred:** no new config fields (1.5.3/1.5.4/1.5.5 later); Monitor tab untouched.
