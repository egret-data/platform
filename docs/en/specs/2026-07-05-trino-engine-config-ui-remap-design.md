# Trino Engine Configuration UI Re-map — Design

- **Status:** Approved (design)
- **Date:** 2026-07-05
- **Area:** control-plane frontend (`frontend/src/routes/control-plane/**`) + `egret-orchestrator`
- **Vietnamese reference:** `docs/vi/specs/2026-07-05-trino-engine-config-ui-remap-design.md`

## 1. Motivation

Review feedback on the engine **Configuration** UI raised four issues:

1. The "Add property" control is half-built — a single always-open filter input
   that searches all three property files at once, and drops free-text custom
   keys into `config.properties` regardless of intent.
2. `config.properties` / `node.properties` / `log.properties` are rendered as
   three genuinely separate files by the backend, but the UI flattens them into
   one merged list. They should be presented separately.
3. There is no way to configure the container **image** — it is hardcoded to
   `trinodb/trino:{image_version}` in the manifest renderer, so the UI always
   uses the default image.
4. The overall Configuration information architecture should be reorganised to
   mirror a well-structured Trino Helm `values.yaml`.

## 2. Grounding: current state

### 2.1 Backend (`egret-orchestrator`)

- `crates/egret-orchestrator/src/trino.rs` renders **plain Kubernetes manifests**
  (Deployments / ConfigMaps / Secret / Service / Ingress / HPA / ScaledObject) —
  there is no Helm chart or Stackable CRD in the repo. "values.yaml" is therefore
  a *model for information architecture only*.
- `config.properties`, `node.properties`, `log.properties` are three distinct
  ConfigMap entries (`config-{role}.properties`, `node.properties`,
  `log.properties`).
- The main container image is hardcoded:
  `"image": format!("trinodb/trino:{}", spec.image_version)` (`trino.rs:573`).
  `spec.image_version` (the tag) exists on `EngineSpec`; the repository/registry
  is not configurable.
- `imagePullPolicy` and `imagePullSecrets` already exist on `TrinoConfig`.

### 2.2 Frontend

- Configuration tab left-nav = 5 flat items: **Properties · Resources ·
  Workload · Scheduling · Scaling** (`EngineDetail.tsx` `NAV_ITEMS`). A separate
  top-level tab holds **Security & Exposure**.
- `PropertiesSection.tsx` renders the union of the three property maps with a
  per-row file tag and one merged `AddProperty` combobox.
- `config/trinoCatalog.ts` already tags every catalog entry with
  `file: "config" | "node" | "log"`, so a per-file scoped picker is a filter away.
- `ResourcesSection.tsx` shows a coordinator/worker **toggle** + cluster env.
- `SchedulingSection.tsx` stacks Pod + per-role Placement + Containers & Mounts.

## 3. Reference: official Trino Helm chart top-level keys

From `trinodb/charts` `charts/trino/values.yaml` (file order, abridged):
`image` · `imagePullSecrets` · `server` (workers, node, log, config,
exchangeManager, autoscaling, keda) · `accessControl` · `resourceGroups` ·
`additionalNodeProperties` · `additionalConfigProperties` ·
`additionalLogProperties` · `additionalExchangeManagerProperties` ·
`sessionProperties` · `eventListenerProperties` · `env` / `envFrom` ·
`initContainers` / `sidecarContainers` · `securityContext` ·
`containerSecurityContext` · `shareProcessNamespace` · `service` · `auth` ·
`serviceAccount` · `configMounts` / `secretMounts` · `coordinator` (deployment,
jvm, config, resources, probes, placement) · `worker` (same + gracefulShutdown) ·
`ingress`.

## 4. Target design

### 4.1 Configuration nav — one item per top-level chart key

| # | Nav item | Chart source | egret fields |
|---|----------|--------------|--------------|
| 1 | **Image** *(new)* | `image`, `imagePullSecrets` | image string (overrides `trinodb/trino:{version}`), pull policy, pull secrets |
| 2 | **Server** | `server`, `additional*Properties`, `eventListenerProperties` | Query max memory · Trino log level · **config.properties** · **node.properties** · **log.properties** · Exchange manager (fault-tolerant) · Event listener |
| 3 | **Coordinator** | `coordinator` | replicas, JVM/heap, resources, node config, additional JVM config, probes, exposed ports, **placement** (nodeSelector / tolerations / affinity / topology-spread) |
| 4 | **Worker** | `worker` | same as Coordinator + graceful shutdown |
| 5 | **Workload** | `accessControl`, `resourceGroups`, `sessionProperties` | Resource groups · Session rules |
| 6 | **Pod & Containers** | `env`, `securityContext`, `serviceAccount`, `initContainers`, `secretMounts`, deployment | Env vars, service account, pod/container security contexts, shareProcessNamespace, init/sidecar containers, config/secret mounts, deployment strategy / annotations / revisionHistoryLimit / progressDeadlineSeconds |
| 7 | **Autoscaling** | `server.autoscaling`, `server.keda` | HPA / KEDA (unchanged) |
| — | *Security & Exposure* (existing separate tab) | `auth`, `ingress` | unchanged |

**Delta vs today (5 → 7 nav items):**

- `Properties` → **Server**: absorbs `CommonPropertiesSection` (query max memory,
  log level) and the retired merged list, now as three split property
  sub-sections + Exchange manager + Event listener.
- `Resources` → splits into **Coordinator** and **Worker**, each holding that
  role's full block. Per-role **Placement** is pulled in from Scheduling; the
  cluster-wide **env** editor moves to Pod & Containers.
- `Scheduling` → **Pod & Containers** (Placement removed → folded into the role
  sections).
- New **Image** section.
- `Scaling` → **Autoscaling** (rename only).

### 4.2 Decisions locked

- **(a)** Coordinator and Worker are **two separate nav items** (faithful to the
  chart), replacing the single toggled Resources section.
- **(b)** Deployment-level fields (strategy, annotations, revisionHistoryLimit,
  progressDeadlineSeconds) remain **cluster-wide** under Pod & Containers,
  matching egret's data model (applied to both Deployments) — not duplicated
  per-role as the chart does.

### 4.3 Fix 1 — Add-property picker

- Replace the always-open filter `<Input>` with a **"+ Add property" button**
  that opens a **searchable popover picker** (command-palette style): filter by
  key + help text; each result shows key · group · type · default.
- Each of the three property lists (config / node / log) instantiates its own
  picker **scoped to that file**: `trinoCatalog.filter(p => p.file === file)`.
- Selecting a catalog entry adds an editable row (typed via `PropertyField`).
- A free-text key with no catalog match is added to **that section's file**
  (not always `config`).
- The existing per-list **filter** box (to search already-added rows) is
  retained per section.

### 4.4 Fix 2 — Split config / node / log

- Retire the merged union list in `PropertiesSection`. Introduce a reusable
  `PropertyList` component parameterised by `file: "config" | "node" | "log"`
  that renders the keyed rows for that file + the scoped picker (Fix 1).
- Render three `PropertyList`s (one per file) as sub-sections under **Server**.
- Row rendering (`PropertyRow` + `PropertyField`) and the `cfg.getProp/setProp`
  plumbing are reused unchanged — the payload shape (`configProperties`,
  `additionalNodeProperties`, `additionalLogProperties`) is byte-identical.

### 4.5 Fix 3 — Image configuration

**Backend (`egret-orchestrator`):**

- Add `image: Option<String>` to `TrinoConfig` (`spec.rs`).
- In `trino_deployment` (`trino.rs`), the main container image becomes:
  `spec.config.image.clone().unwrap_or_else(|| format!("trinodb/trino:{}", spec.image_version))`.
- No change to `image_version` (still the default tag); no change to
  `imagePullPolicy` / `imagePullSecrets` rendering.

**Frontend:**

- Add `image?: string` to the `TrinoConfig` type (`controlPlane.ts`) and to
  `useEngineConfigForm` (state + prefill + dirty dep + `builtTrinoConfig`,
  omit-empty).
- New **Image** section:
  - **Image** text field — full `registry/repository:tag` string; placeholder
    shows the live default (`trinodb/trino:{imageVersion}`) so leaving it blank
    is clearly "use default".
  - **Pull policy** select (`Always` / `IfNotPresent` / `Never`) — moved from
    Pod section.
  - **Image pull secrets** list — moved from Pod section.

### 4.6 sectionCounts / nav badges

`sectionCounts` (`useEngineConfigForm.ts`) is re-partitioned to the new nav keys
(`image`, `server`, `coordinator`, `worker`, `workload`, `pod`, `autoscaling`)
so each badge counts the field-groups configured in its section. No field-group
is double-counted. `sectionCounts.test.ts` is updated accordingly.

## 5. Components touched

**Backend:** `egret-orchestrator/src/spec.rs` (add `image`),
`egret-orchestrator/src/trino.rs` (render `image`; extend a render test).

**Frontend:**

- `api/controlPlane.ts` — `TrinoConfig.image?: string`.
- `useEngineConfigForm.ts` — `image` state; re-partitioned `sectionCounts`.
- `config/PropertyList.tsx` *(new)* — per-file list + scoped picker (extracted
  from `PropertiesSection`).
- `config/PropertyPicker.tsx` *(new)* — the "+ Add property" button + popover.
- `config/ServerSection.tsx` *(new)* — query max memory / log level / 3 property
  lists / exchange manager / event listener.
- `config/ImageSection.tsx` *(new)*.
- `config/CoordinatorSection.tsx` + `config/WorkerSection.tsx` *(new)* — wrap
  `EngineRoleConfig` + `PlacementSection` per role.
- `config/PodSection.tsx` — drop image-pull fields (moved to Image); gain env.
- `EngineDetail.tsx` — new `NAV_ITEMS`, section routing; retire
  `CommonPropertiesSection` / `PropertiesSection` / `ResourcesSection` /
  `SchedulingSection` wrappers (content re-homed).
- Tests: `sectionCounts.test.ts`, `trinoCatalog.test.ts` (unaffected),
  new `PropertyPicker` behaviour test.

## 6. Non-goals

- Create-engine wizard is untouched (configuration is edited post-create).
- No new entries added to `trinoCatalog`.
- Rendered manifests are unchanged **except** the new image override.
- Security & Exposure tab (auth, ingress) layout is unchanged.
- No migration: `image` is a new optional field; existing engines render exactly
  as before when it is unset.

## 7. Risks / notes

- The nav re-partition is the largest surface: fields must move without being
  dropped or double-counted. `sectionCounts` and the review-changes diff
  (`builtTrinoConfig`) are the guardrails — payload output must stay
  byte-identical except for the added `image` key.
- Per-file catalog subsets are small for node (5 keys) and log (~12 keys); the
  picker must read well even with few results.
- The prefill/dirty effect in `useEngineConfigForm` must include `image` or an
  external update would clobber an in-progress edit.

## 8. Acceptance

- Configuration nav shows 7 items in chart order; every previously-editable
  field is reachable and saves identically (verified against the review-changes
  diff).
- config / node / log each have their own list + a scoped "+ Add property"
  picker; a custom key added under node lands in `additionalNodeProperties`.
- Setting the Image field renders `image: <value>`; clearing it falls back to
  `trinodb/trino:{version}` (backend render test).
- `just` frontend lint/typecheck/test/build and full backend fmt/clippy/test
  pass locally before merge.
