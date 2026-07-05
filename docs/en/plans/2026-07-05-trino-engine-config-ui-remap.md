# Trino Engine Configuration UI Re-map — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorganise the Trino engine Configuration UI to mirror the official Trino Helm `values.yaml`, split `config`/`node`/`log` properties into separate scoped lists with a proper "+ Add property" picker, and add an Image configuration section (full image string) backed end-to-end.

**Architecture:** Frontend-heavy refactor of `frontend/src/routes/control-plane/config/**` plus a small `image` field threaded through the backend DTO → orchestrator → manifest. The Configuration left-nav goes from 5 flat items to 7 chart-aligned items (Image · Server · Coordinator · Worker · Workload · Pod & Containers · Autoscaling). Payload output stays **byte-identical** except the new optional `image` key; `sectionCounts` and the review-changes diff are the guardrails.

**Tech Stack:** Rust (`egret-orchestrator`, `egret-controlplane`, `egret-bin`), React 18 + TypeScript + Tailwind v4 + shadcn, Vitest.

**Spec:** `docs/en/specs/2026-07-05-trino-engine-config-ui-remap-design.md`

---

## Conventions for every task

- Run frontend commands from `frontend/`. Backend commands from repo root (`egret/`).
- Frontend gate: `npm run typecheck && npm run lint && npm run test -- --run`. Build gate where noted: `npm run build`.
- Backend gate: `cargo test -p <crate>` for the task, `cargo fmt --all` before commit.
- Commit identity: the repo's configured identity (`hoangquocvietbk2.13@gmail.com`) — never override author.
- Conventional Commits. One commit per task unless a task says otherwise.
- **Guardrail:** except Task 1's new `image` key, no rendered manifest or PUT payload may change. When in doubt, diff `builtTrinoConfig` before/after.

## File structure (created / modified)

**Backend**
- Modify `crates/egret-orchestrator/src/spec.rs` — add `image: Option<String>` to `TrinoConfig`.
- Modify `crates/egret-orchestrator/src/trino.rs` — render `image` with fallback; new test.
- Modify `crates/egret-controlplane/src/engines.rs` — `TrinoConfigReq.image` + both struct literals.
- Modify `crates/egret-bin/src/main.rs` — map `image` in `spec_from`.

**Frontend**
- Modify `frontend/src/api/controlPlane.ts` — `TrinoConfig.image?: string`.
- Modify `frontend/src/routes/control-plane/useEngineConfigForm.ts` — `image` state + re-partitioned `sectionCounts`.
- Modify `frontend/src/routes/control-plane/config/sectionCounts.test.ts` — new keys.
- Create `frontend/src/routes/control-plane/config/PropertyPicker.tsx` — "+ Add property" button + scoped popover.
- Create `frontend/src/routes/control-plane/config/PropertyPicker.test.tsx`.
- Create `frontend/src/routes/control-plane/config/PropertyList.tsx` — one file's list + picker.
- Create `frontend/src/routes/control-plane/config/ServerSection.tsx` — Common + 3 PropertyLists + Exchange.
- Create `frontend/src/routes/control-plane/config/ImageSection.tsx`.
- Create `frontend/src/routes/control-plane/config/CoordinatorSection.tsx` + `WorkerSection.tsx`.
- Modify `frontend/src/routes/control-plane/config/PodSection.tsx` — drop image-pull fields, add env.
- Rename intent: retire `PropertiesSection.tsx`, `ResourcesSection.tsx`, `SchedulingSection.tsx` wrappers (content re-homed) — delete after Task 9.
- Modify `frontend/src/routes/control-plane/EngineDetail.tsx` — new `NAV_ITEMS` + routing.

---

## Task 1: Backend — `image` field end-to-end

**Files:**
- Modify: `crates/egret-orchestrator/src/spec.rs:337` (struct `TrinoConfig`)
- Modify: `crates/egret-orchestrator/src/trino.rs:573`
- Test: `crates/egret-orchestrator/src/trino.rs` (tests module, ~line 1196+)
- Modify: `crates/egret-controlplane/src/engines.rs:370,1098,1233`
- Modify: `crates/egret-bin/src/main.rs:680`

- [ ] **Step 1: Add the failing render test** in `trino.rs` tests module (after `config_properties_per_role_memory_and_shutdown`):

```rust
#[test]
fn image_override_and_default() {
    // Default: repository is trinodb/trino, tag is image_version.
    let s = sample_spec();
    let d = trino_deployment(&s, Role::Coordinator);
    assert_eq!(d["spec"]["template"]["spec"]["containers"][0]["image"], "trinodb/trino:470");

    // Override: full image string wins verbatim.
    let mut s2 = sample_spec();
    s2.config.image = Some("ghcr.io/acme/trino:471-patched".into());
    let d2 = trino_deployment(&s2, Role::Coordinator);
    assert_eq!(
        d2["spec"]["template"]["spec"]["containers"][0]["image"],
        "ghcr.io/acme/trino:471-patched"
    );
}
```

- [ ] **Step 2: Run it — expect a compile error** (`image` is not a field yet)

Run: `cargo test -p egret-orchestrator image_override_and_default`
Expected: FAIL — `no field \`image\` on type \`TrinoConfig\``

- [ ] **Step 3: Add the field** in `spec.rs`. Insert into `TrinoConfig` (place it just above `image_pull_secrets` at line 321 so image-related fields group):

```rust
    /// Full container image (registry/repository:tag). When None, the renderer
    /// falls back to `trinodb/trino:{image_version}`.
    pub image: Option<String>,
    pub image_pull_secrets: Vec<String>,
```

- [ ] **Step 4: Render it** in `trino.rs`. Replace the hardcoded image at line 573:

```rust
        "image": spec
            .config
            .image
            .clone()
            .unwrap_or_else(|| format!("trinodb/trino:{}", spec.image_version)),
```

- [ ] **Step 5: Run the test — expect PASS**

Run: `cargo test -p egret-orchestrator image_override_and_default`
Expected: PASS

- [ ] **Step 6: Thread the DTO** in `engines.rs`. (a) In `struct TrinoConfigReq` (line 370), add above `image_pull_secrets`:

```rust
    pub image: Option<String>,
```

(b) In the `build_trino_config` seed literal (the `TrinoConfigReq { ... }` at ~line 1098), add:

```rust
        image: None,
```

(c) In `trino_config_from_json`'s literal (the `TrinoConfigReq { ... }` at ~line 1233, next to the `image_pull_policy` line 1266):

```rust
        image: v["image"].as_str().map(Into::into),
```

- [ ] **Step 7: Map it in `egret-bin`.** In `spec_from` (`main.rs:680`, the `egret_orchestrator::TrinoConfig { ... }` literal), add next to `image_pull_policy` (line 754):

```rust
                image: cfg.image.clone(),
```

- [ ] **Step 8: Build the workspace + fmt**

Run: `cargo build -p egret-orchestrator -p egret-controlplane -p egret-bin && cargo fmt --all`
Expected: builds clean, no fmt diff.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-orchestrator/src/spec.rs crates/egret-orchestrator/src/trino.rs crates/egret-controlplane/src/engines.rs crates/egret-bin/src/main.rs
git commit -m "feat(engine): allow overriding the Trino container image"
```

---

## Task 2: Frontend type + hook `image` state + `sectionCounts` re-partition

**Files:**
- Modify: `frontend/src/api/controlPlane.ts:691`
- Modify: `frontend/src/routes/control-plane/useEngineConfigForm.ts`
- Test: `frontend/src/routes/control-plane/config/sectionCounts.test.ts`

- [ ] **Step 1: Add `image` to the type.** In `controlPlane.ts`, inside `interface TrinoConfig`, above `imagePullSecrets?` (line 690):

```ts
  image?: string;
  imagePullSecrets?: string[];
```

- [ ] **Step 2: Rewrite `sectionCounts` to the new keys.** Replace the whole `return { ... }` object at the end of `sectionCounts` (`useEngineConfigForm.ts:590-607`) with the 7-key partition below. Keep the helpers (`roleCoreCount`, `rolePlacementCount`, `knownSectionCount`, `keyCount`, `present`, `propertiesCount`, `podCount`, `containersMountsCount`) as-is except `podCount` (Step 3):

```ts
  return {
    image:
      (present(cfg.image) ? 1 : 0) +
      (present(cfg.imagePullPolicy) ? 1 : 0) +
      ((cfg.imagePullSecrets?.length ?? 0) > 0 ? 1 : 0),
    server: propertiesCount + (present(cfg.faultTolerant) ? 1 : 0) + knownSectionCount("reliability"),
    coordinator: roleCoreCount(coordinator) + rolePlacementCount(coordinator),
    worker:
      roleCoreCount(worker) +
      rolePlacementCount(worker) +
      (present(worker?.gracefulShutdown) ? 1 : 0),
    workload: [cfg.resourceGroups, cfg.sessionProperties].filter(present).length,
    pod: podCount + (present(cfg.env) ? 1 : 0) + containersMountsCount,
    autoscaling: [cfg.autoscaling].filter(present).length,
  };
```

- [ ] **Step 3: Move image-pull out of `podCount`.** In `sectionCounts`, edit the `podCount` array (`useEngineConfigForm.ts:569-580`) to drop `cfg.imagePullSecrets` and `cfg.imagePullPolicy` (they now count under `image`):

```ts
  const podCount = [
    cfg.serviceAccount,
    cfg.podSecurityContext,
    cfg.containerSecurityContext,
    cfg.deploymentStrategy,
    cfg.deploymentAnnotations,
    cfg.revisionHistoryLimit,
    cfg.progressDeadlineSeconds,
    cfg.shareProcessNamespace,
  ].filter(present).length;
```

- [ ] **Step 4: Add `image` state to the hook.** Near the image-pull state (`useEngineConfigForm.ts:812-815`), add:

```ts
  const [image, setImage] = useState(tc?.image ?? "");
```

Add to the sync-effect (after the `setImagePullPolicy(...)` line ~915):

```ts
    setImage(next?.image ?? "");
```

Add `image` to the dirty-deps array (the `useEffect` deps at ~963) and to the `builtTrinoConfig` deps array (~1165), next to `imagePullPolicy`.

In `builtTrinoConfig` (omit-empty block, next to the `imagePullPolicy` handling ~1101), add:

```ts
    if (image.trim()) trinoConfig.image = image.trim();
```

Add `image, setImage` to the `EngineConfigForm` interface (near `imagePullPolicy`) and to the returned object (~1319).

- [ ] **Step 5: Update `sectionCounts.test.ts`** to assert the new keys. Replace any assertions keyed on `properties`/`resources`/`scheduling`/`scaling` with `server`/`coordinator`/`worker`/`pod`/`image`/`autoscaling`. Add a case:

```ts
it("counts image fields under the image section", () => {
  const counts = sectionCounts({
    coordinator: {},
    worker: {},
    image: "ghcr.io/acme/trino:471",
    imagePullPolicy: "Always",
  } as unknown as TrinoConfig);
  expect(counts.image).toBe(2);
});
```

- [ ] **Step 6: Typecheck + test**

Run (from `frontend/`): `npm run typecheck && npm run test -- --run sectionCounts`
Expected: PASS. (Other components still reference old nav keys only in EngineDetail, changed in Task 9 — typecheck stays green because nav keys are plain strings.)

- [ ] **Step 7: Commit**

```bash
git add frontend/src/api/controlPlane.ts frontend/src/routes/control-plane/useEngineConfigForm.ts frontend/src/routes/control-plane/config/sectionCounts.test.ts
git commit -m "feat(engine-config): add image form state and re-partition section counts"
```

---

## Task 3: `PropertyPicker` — "+ Add property" button + scoped popover

**Files:**
- Create: `frontend/src/routes/control-plane/config/PropertyPicker.tsx`
- Test: `frontend/src/routes/control-plane/config/PropertyPicker.test.tsx`

Extracts the current always-open combobox in `PropertiesSection.tsx` (`AddProperty`) into a reusable, button-triggered picker **scoped to one file's catalog subset**. Uses the same inline-dropdown pattern (relative container + absolute list) — no new UI primitive dependency.

- [ ] **Step 1: Write the failing test**

```tsx
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PropertyPicker } from "./PropertyPicker";
import { trinoCatalog } from "./trinoCatalog";

const nodeCatalog = trinoCatalog.filter((p) => p.file === "node");

describe("PropertyPicker", () => {
  it("opens on click, filters, and adds a catalog prop", () => {
    const onAddCatalog = vi.fn();
    render(
      <PropertyPicker catalog={nodeCatalog} onAddCatalog={onAddCatalog} onAddCustom={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    fireEvent.change(screen.getByPlaceholderText(/search properties/i), {
      target: { value: "node.environment" },
    });
    fireEvent.click(screen.getByText("node.environment"));
    expect(onAddCatalog).toHaveBeenCalledWith(
      expect.objectContaining({ key: "node.environment" }),
    );
  });

  it("adds a custom free-text key on Enter", () => {
    const onAddCustom = vi.fn();
    render(
      <PropertyPicker catalog={nodeCatalog} onAddCatalog={vi.fn()} onAddCustom={onAddCustom} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    const input = screen.getByPlaceholderText(/search properties/i);
    fireEvent.change(input, { target: { value: "node.custom-thing" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAddCustom).toHaveBeenCalledWith("node.custom-thing");
  });
});
```

- [ ] **Step 2: Run it — expect FAIL** (module not found)

Run (from `frontend/`): `npm run test -- --run PropertyPicker`
Expected: FAIL — cannot find `./PropertyPicker`.

- [ ] **Step 3: Implement `PropertyPicker.tsx`**

```tsx
import { Plus, Search } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { type TrinoProp } from "./trinoCatalog";

const MAX_ADD_RESULTS = 30;

/** A small pill chip (property group / type). */
function Chip({ children }: { children: React.ReactNode }) {
  return (
    <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
      {children}
    </span>
  );
}

/**
 * A "+ Add property" button that opens a searchable list over `catalog` (already
 * scoped to one property file by the caller). Selecting a result calls
 * `onAddCatalog`; pressing Enter on text that matches no key calls `onAddCustom`
 * with the raw key — the caller decides which file it lands in.
 */
export function PropertyPicker({
  catalog,
  onAddCatalog,
  onAddCustom,
}: {
  catalog: TrinoProp[];
  onAddCatalog: (prop: TrinoProp) => void;
  onAddCustom: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  const q = query.trim().toLowerCase();
  const matches = q
    ? catalog
        .filter(
          (p) =>
            p.key.toLowerCase().includes(q) || p.help.toLowerCase().includes(q),
        )
        .slice(0, MAX_ADD_RESULTS)
    : catalog.slice(0, MAX_ADD_RESULTS);

  const close = () => {
    setOpen(false);
    setQuery("");
  };

  const selectProp = (prop: TrinoProp) => {
    onAddCatalog(prop);
    close();
  };

  const addCustom = () => {
    const key = query.trim();
    if (!key) return;
    const existing = catalog.find((p) => p.key === key);
    if (existing) selectProp(existing);
    else {
      onAddCustom(key);
      close();
    }
  };

  if (!open) {
    return (
      <Button type="button" variant="outline" size="sm" onClick={() => setOpen(true)}>
        <Plus className="h-4 w-4" />
        Add property
      </Button>
    );
  }

  return (
    <div className="relative w-full sm:w-96">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        autoFocus
        placeholder="Search properties…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onBlur={() => window.setTimeout(close, 150)}
        onKeyDown={(e) => {
          if (e.key === "Escape") close();
          else if (e.key === "Enter") {
            e.preventDefault();
            addCustom();
          }
        }}
        className="pl-8"
      />
      <ul className="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md">
        {matches.map((p) => (
          <li key={`${p.file}:${p.key}`}>
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => selectProp(p)}
              className="flex w-full flex-col items-start gap-0.5 rounded-sm px-2 py-1.5 text-left hover:bg-muted"
            >
              <div className="flex w-full items-center gap-2">
                <span className="font-mono text-xs">{p.key}</span>
                <Chip>{p.group}</Chip>
                <Chip>{p.type}</Chip>
              </div>
              <span className="line-clamp-1 text-xs text-muted-foreground">
                {p.help}
              </span>
            </button>
          </li>
        ))}
        {q && !matches.some((p) => p.key === q) && (
          <li>
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={addCustom}
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-muted"
            >
              <span className="text-muted-foreground">+ Add custom key</span>
              <span className="font-mono text-xs">{query.trim()}</span>
            </button>
          </li>
        )}
      </ul>
    </div>
  );
}
```

- [ ] **Step 4: Run the test — expect PASS**

Run (from `frontend/`): `npm run test -- --run PropertyPicker`
Expected: PASS (both cases).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/config/PropertyPicker.tsx frontend/src/routes/control-plane/config/PropertyPicker.test.tsx
git commit -m "feat(engine-config): add scoped PropertyPicker add-property control"
```

---

## Task 4: `PropertyList` — one file's rows + scoped picker

**Files:**
- Create: `frontend/src/routes/control-plane/config/PropertyList.tsx`

Renders the keyed rows for one file (`config` | `node` | `log`) with a per-list filter and the Task-3 picker scoped to that file. Row rendering reuses the existing `PropertyRow`/`PropertyField` pattern. `PropertyRow` currently lives inside `PropertiesSection.tsx`; move it into `PropertyList.tsx` (it is the same code) and export nothing extra.

- [ ] **Step 1: Implement `PropertyList.tsx`**

```tsx
import { Search, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { PropertyField } from "./PropertyField";
import { PropertyPicker } from "./PropertyPicker";
import { type TrinoProp, trinoCatalog, trinoCatalogByKey } from "./trinoCatalog";

type PropFile = "config" | "node" | "log";

/** A single property row: mono key + group chip + typed value editor + remove. */
function PropertyRow({
  cfg,
  file,
  keyName,
  onRemove,
}: {
  cfg: EngineConfigForm;
  file: PropFile;
  keyName: string;
  onRemove: () => void;
}) {
  const cat = trinoCatalogByKey[keyName];
  return (
    <div className="flex items-start gap-3 py-2">
      <div className="flex w-56 shrink-0 flex-col gap-1 pt-1.5">
        <span className="break-all font-mono text-xs">{keyName}</span>
        <div>
          <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
            {cat?.group ?? "custom"}
          </span>
        </div>
      </div>
      <div className="min-w-0 flex-1">
        {cat ? (
          <PropertyField
            prop={{
              key: keyName,
              section: cat.group,
              label: keyName,
              type: cat.type,
              help: cat.help,
              docsUrl: cat.docsUrl,
              enumOptions: cat.enumOptions,
              default: cat.default,
            }}
            value={cfg.getProp(file, keyName)}
            onChange={(v) => cfg.setProp(file, keyName, v)}
          />
        ) : (
          <Input
            value={cfg.getProp(file, keyName)}
            placeholder="value"
            onChange={(e) => cfg.setProp(file, keyName, e.target.value)}
          />
        )}
      </div>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="mt-1 shrink-0 text-muted-foreground"
        onClick={onRemove}
        aria-label={`Remove ${keyName}`}
      >
        <Trash2 className="h-4 w-4" />
      </Button>
    </div>
  );
}

/**
 * The property list for ONE file. Rows = the file's map keys + any locally-added
 * key not yet in the map (a no-default / custom key still shows an empty editable
 * row because `setProp` drops blanks). The picker is scoped to this file's
 * catalog subset; custom keys land in THIS file.
 */
export function PropertyList({
  cfg,
  file,
  rows: mapKeys,
}: {
  cfg: EngineConfigForm;
  file: PropFile;
  rows: { key: string }[];
}) {
  const [search, setSearch] = useState("");
  const [added, setAdded] = useState<string[]>([]);

  const catalog = useMemo(
    () => trinoCatalog.filter((p) => p.file === file),
    [file],
  );

  const addKey = (key: string) =>
    setAdded((prev) => (prev.includes(key) ? prev : [...prev, key]));

  const addCatalog = (prop: TrinoProp) => {
    addKey(prop.key);
    if ((prop.default ?? "").trim() !== "")
      cfg.setProp(file, prop.key, prop.default as string);
  };
  const addCustom = (key: string) => addKey(key);
  const removeRow = (key: string) => {
    cfg.setProp(file, key, "");
    setAdded((prev) => prev.filter((k) => k !== key));
  };

  const inMap = new Set(mapKeys.map((r) => r.key));
  const keys = [...mapKeys.map((r) => r.key), ...added.filter((k) => !inMap.has(k))];
  const s = search.trim().toLowerCase();
  const filtered = s ? keys.filter((k) => k.toLowerCase().includes(s)) : keys;

  return (
    <div className="space-y-3">
      {keys.length > 3 && (
        <div className="relative w-full sm:w-72">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            placeholder="Filter properties…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="pl-8"
          />
        </div>
      )}
      {keys.length === 0 ? (
        <p className="py-4 text-center text-sm text-muted-foreground">
          No properties set — add one below.
        </p>
      ) : filtered.length === 0 ? (
        <p className="py-4 text-center text-sm text-muted-foreground">
          No properties match “{search}”.
        </p>
      ) : (
        <div className="divide-y">
          {filtered.map((k) => (
            <PropertyRow
              key={`${file}:${k}`}
              cfg={cfg}
              file={file}
              keyName={k}
              onRemove={() => removeRow(k)}
            />
          ))}
        </div>
      )}
      <PropertyPicker catalog={catalog} onAddCatalog={addCatalog} onAddCustom={addCustom} />
    </div>
  );
}
```

- [ ] **Step 2: Typecheck**

Run (from `frontend/`): `npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/control-plane/config/PropertyList.tsx
git commit -m "feat(engine-config): add per-file PropertyList with scoped picker"
```

---

## Task 5: `ServerSection` — Common + 3 property lists + Exchange manager

**Files:**
- Create: `frontend/src/routes/control-plane/config/ServerSection.tsx`

Composes: the former `CommonPropertiesSection` (query max memory, log level, event listener — copy its body from `EngineDetail.tsx:344-398`), the three `PropertyList`s under one "Properties" card, and the fault-tolerant editor (`EngineConfigReliability`, moved here from Workload — "Exchange manager").

- [ ] **Step 1: Implement `ServerSection.tsx`**

```tsx
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { KeyValueEditor } from "../EngineResourceGroups";
import { EngineConfigReliability } from "../EngineConfigReliability";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { FieldLabel } from "./FieldLabel";
import { PropertyList } from "./PropertyList";
import { QuantityInput } from "./QuantityInput";

const LOG_LEVELS = ["INFO", "DEBUG", "WARN", "ERROR"] as const;

/**
 * "Server" section — cluster-wide server config mirroring the chart `server` +
 * `additional*Properties` + exchange manager: query memory ceiling & log level,
 * the three split property files (config/node/log), the raw event listener, and
 * fault-tolerant execution (exchange manager).
 */
export function ServerSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    queryMaxMemory,
    setQueryMaxMemory,
    logLevel,
    setLogLevel,
    eventListener,
    setEventListener,
    configProps,
    nodeProps,
    logProps,
    ftEnabled,
    setFtEnabled,
    retryPolicy,
    setRetryPolicy,
    exchangeStorageConnId,
    setExchangeStorageConnId,
    exchangePrefix,
    setExchangePrefix,
    getProp,
    setProp,
  } = cfg;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Server"
        description="Cluster-wide query memory ceiling and server log level."
        docsUrl="https://trino.io/docs/current/admin/properties-resource-management.html"
      >
        <div className="grid grid-cols-2 gap-4">
          <QuantityInput
            fieldKey="queryMaxMemory"
            label="Query max memory"
            value={queryMaxMemory}
            onChange={setQueryMaxMemory}
            placeholder="4GB"
          />
          <div className="space-y-1.5">
            <FieldLabel fieldKey="logLevel" label="Log level" htmlFor="ced-log-level" />
            <Select value={logLevel} onValueChange={setLogLevel}>
              <SelectTrigger id="ced-log-level" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {LOG_LEVELS.map((lvl) => (
                  <SelectItem key={lvl} value={lvl}>
                    {lvl}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      </ConfigSection>

      <ConfigSection
        title="config.properties"
        description="Cluster-wide Trino config.properties. Pick from the catalog or add a custom key."
      >
        <PropertyList cfg={cfg} file="config" rows={configProps} />
      </ConfigSection>

      <ConfigSection
        title="node.properties"
        description="Per-node identity and paths (node.properties)."
      >
        <PropertyList cfg={cfg} file="node" rows={nodeProps} />
      </ConfigSection>

      <ConfigSection
        title="log.properties"
        description="Logger levels and log output (log.properties)."
      >
        <PropertyList cfg={cfg} file="log" rows={logProps} />
      </ConfigSection>

      <ConfigSection
        title="Event listener"
        description="Raw event-listener.properties."
      >
        <Label className="sr-only">Event listener</Label>
        <KeyValueEditor
          rows={eventListener}
          onChange={setEventListener}
          keyPlaceholder="event-listener.name"
          valuePlaceholder="http"
        />
      </ConfigSection>

      <EngineConfigReliability
        ftEnabled={ftEnabled}
        setFtEnabled={setFtEnabled}
        retryPolicy={retryPolicy}
        setRetryPolicy={setRetryPolicy}
        exchangeStorageConnId={exchangeStorageConnId}
        setExchangeStorageConnId={setExchangeStorageConnId}
        exchangePrefix={exchangePrefix}
        setExchangePrefix={setExchangePrefix}
        getProp={getProp}
        setProp={setProp}
      />
    </div>
  );
}
```

- [ ] **Step 2: Typecheck**

Run (from `frontend/`): `npm run typecheck`
Expected: PASS. (`configProps`/`nodeProps`/`logProps` are `Kv[]`, whose elements have `.key`, satisfying `rows: {key: string}[]`.)

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/control-plane/config/ServerSection.tsx
git commit -m "feat(engine-config): add Server section with split property files"
```

---

## Task 6: `ImageSection`

**Files:**
- Create: `frontend/src/routes/control-plane/config/ImageSection.tsx`

Full image string + pull policy + pull secrets. The pull-policy select and pull-secrets list are the same widgets currently in `PodSection.tsx:143-213` (moved here). The image text field's placeholder shows the live default so blank = default.

- [ ] **Step 1: Implement `ImageSection.tsx`**

```tsx
import { Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { SELECT_NONE } from "./podEditors";

/**
 * "Image" section — mirrors the chart `image` + `imagePullSecrets`: the full
 * container image string (overrides the default `trinodb/trino:{version}`), the
 * pull policy, and pull secrets. `defaultImage` is shown as the placeholder so a
 * blank field clearly means "use the default image".
 */
export function ImageSection({
  cfg,
  defaultImage,
}: {
  cfg: EngineConfigForm;
  defaultImage: string;
}) {
  const {
    image,
    setImage,
    imagePullPolicy,
    setImagePullPolicy,
    imagePullSecrets,
    setImagePullSecrets,
  } = cfg;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Image"
        description="Container image for the coordinator and worker. Leave blank to use the default."
      >
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="space-y-1.5 sm:col-span-2">
            <Label htmlFor="img-image">Image</Label>
            <Input
              id="img-image"
              value={image}
              placeholder={defaultImage}
              onChange={(e) => setImage(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">
              Full <code>registry/repository:tag</code>. Default:{" "}
              <code>{defaultImage}</code>.
            </p>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="img-pullpolicy">Image pull policy</Label>
            <Select
              value={imagePullPolicy || SELECT_NONE}
              onValueChange={(v) => setImagePullPolicy(v === SELECT_NONE ? "" : v)}
            >
              <SelectTrigger id="img-pullpolicy" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={SELECT_NONE}>(default)</SelectItem>
                <SelectItem value="Always">Always</SelectItem>
                <SelectItem value="IfNotPresent">IfNotPresent</SelectItem>
                <SelectItem value="Never">Never</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <div className="space-y-2">
          <Label>Image pull secrets</Label>
          <p className="text-xs text-muted-foreground">
            Names of Kubernetes <code>docker-registry</code> secrets in the engine
            namespace.
          </p>
          {imagePullSecrets.map((s, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={s}
                placeholder="regcred"
                onChange={(e) =>
                  setImagePullSecrets(
                    imagePullSecrets.map((v, j) => (j === i ? e.target.value : v)),
                  )
                }
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setImagePullSecrets(imagePullSecrets.filter((_, j) => j !== i))
                }
                aria-label="Remove secret"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setImagePullSecrets([...imagePullSecrets, ""])}
          >
            <Plus className="h-4 w-4" />
            Add secret
          </Button>
        </div>
      </ConfigSection>
    </div>
  );
}
```

- [ ] **Step 2: Typecheck**

Run (from `frontend/`): `npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/control-plane/config/ImageSection.tsx
git commit -m "feat(engine-config): add Image section"
```

---

## Task 7: `CoordinatorSection` + `WorkerSection`

**Files:**
- Create: `frontend/src/routes/control-plane/config/CoordinatorSection.tsx`
- Create: `frontend/src/routes/control-plane/config/WorkerSection.tsx`

Each wraps the existing per-role editor (`EngineRoleConfig`) + that role's placement editor (`RolePlacementEditor`, currently private inside `PlacementSection.tsx`). Export `RolePlacementEditor` from `PlacementSection.tsx` so both new sections can reuse it (both role editors stay mounted across the two nav items anyway — validity registers per role via the stable `rawId`).

- [ ] **Step 1: Export the role placement editor.** In `PlacementSection.tsx`, change `function RolePlacementEditor(` to `export function RolePlacementEditor(`. Leave `PlacementSection` in place for now (deleted in Task 9).

- [ ] **Step 2: Implement `CoordinatorSection.tsx`**

```tsx
import { EngineRoleConfig } from "../EngineRoleConfig";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { RolePlacementEditor } from "./PlacementSection";

/**
 * "Coordinator" section — mirrors the chart `coordinator` block: the role's
 * sizing / JVM / resources / probes editor plus its pod-placement fields.
 */
export function CoordinatorSection({ cfg }: { cfg: EngineConfigForm }) {
  const { coordinator, setCoordinator } = cfg;
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Coordinator resources"
        description="CPU, memory, JVM & node config for the coordinator Deployment."
      >
        <EngineRoleConfig role="coordinator" value={coordinator} onChange={setCoordinator} />
      </ConfigSection>
      <ConfigSection
        title="Coordinator placement"
        description="Node selector, tolerations, topology spread & affinity for the coordinator."
      >
        <RolePlacementEditor
          idPrefix="coord"
          rawId="placement.coordinator.affinity.raw"
          form={coordinator}
          onChange={setCoordinator}
        />
      </ConfigSection>
    </div>
  );
}
```

- [ ] **Step 3: Implement `WorkerSection.tsx`** (adds graceful shutdown via `EngineRoleConfig`'s worker props)

```tsx
import { EngineRoleConfig } from "../EngineRoleConfig";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { RolePlacementEditor } from "./PlacementSection";

/**
 * "Worker" section — mirrors the chart `worker` block: the role's sizing / JVM /
 * resources / probes editor (incl. graceful shutdown) plus its placement fields.
 */
export function WorkerSection({ cfg }: { cfg: EngineConfigForm }) {
  const { worker, setWorker, gsEnabled, setGsEnabled, gracePeriod, setGracePeriod } = cfg;
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Worker resources"
        description="CPU, memory, JVM, node config & graceful shutdown for the worker Deployment."
      >
        <EngineRoleConfig
          role="worker"
          value={worker}
          onChange={setWorker}
          gsEnabled={gsEnabled}
          onGsEnabledChange={setGsEnabled}
          gracePeriod={gracePeriod}
          onGracePeriodChange={setGracePeriod}
        />
      </ConfigSection>
      <ConfigSection
        title="Worker placement"
        description="Node selector, tolerations, topology spread & affinity for the worker."
      >
        <RolePlacementEditor
          idPrefix="worker"
          rawId="placement.worker.affinity.raw"
          form={worker}
          onChange={setWorker}
        />
      </ConfigSection>
    </div>
  );
}
```

- [ ] **Step 4: Typecheck**

Run (from `frontend/`): `npm run typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/config/CoordinatorSection.tsx frontend/src/routes/control-plane/config/WorkerSection.tsx frontend/src/routes/control-plane/config/PlacementSection.tsx
git commit -m "feat(engine-config): add Coordinator and Worker sections"
```

---

## Task 8: `PodSection` — drop image-pull, add env

**Files:**
- Modify: `frontend/src/routes/control-plane/config/PodSection.tsx`

Image-pull moved to Image (Task 6). Cluster env moves here from the old Resources section. This section becomes "Pod & Containers" content (the containers/mounts card is added in Task 9 by composing the existing `ContainersMountsSection` under the same nav item).

- [ ] **Step 1: Remove image-pull from `PodSectionProps`.** Delete `imagePullSecrets`, `setImagePullSecrets`, `imagePullPolicy`, `setImagePullPolicy` from the `PodSectionProps` interface (lines 91-94) and from the destructured params (lines 119-122).

- [ ] **Step 2: Add env to props.** Add to `PodSectionProps` and to the params:

```ts
  env: Kv[];
  setEnv: Dispatch<SetStateAction<Kv[]>>;
```

(`Kv` is already imported from `../EngineResourceGroups` in this file.)

- [ ] **Step 3: Replace the "Service account & image pull" card.** Change the first `ConfigSection` (lines 140-214) to service-account-only + env; remove the image pull policy select and the whole image-pull-secrets block:

```tsx
      <ConfigSection
        title="Service account & environment"
        description="Applied to both the coordinator and worker pods."
      >
        <div className="space-y-1.5">
          <Label htmlFor="ps-sa">Service account</Label>
          <Input
            id="ps-sa"
            value={serviceAccount}
            placeholder="default"
            onChange={(e) => setServiceAccount(e.target.value)}
          />
        </div>
        <div className="space-y-2">
          <Label>Environment variables</Label>
          <p className="text-xs text-muted-foreground">
            Extra env applied to both coordinator and worker pods.
          </p>
          <KeyValueEditor
            rows={env}
            onChange={setEnv}
            keyPlaceholder="MY_VAR"
            valuePlaceholder="value"
            addLabel="Add variable"
          />
        </div>
      </ConfigSection>
```

Remove the now-unused `SELECT_NONE` import only if no longer referenced (it is still used by the Deployment strategy select — keep it). Remove the `Plus` import only if unused (still used by env? no — `KeyValueEditor` owns its add button; the pull-secrets `Plus` is gone — check remaining usages before removing).

- [ ] **Step 4: Typecheck** (PodSection consumers change in Task 9; this will show a temporary type error at the old `SchedulingSection` call site — acceptable until Task 9, so run only the component-local check here)

Run (from `frontend/`): `npm run typecheck`
Expected: MAY fail only at `SchedulingSection.tsx` (old caller passes removed props). That caller is deleted in Task 9. Do not fix `SchedulingSection` — it is being retired. If you prefer a green checkpoint, do Steps in Task 9 before committing Task 8; otherwise commit and proceed.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/config/PodSection.tsx
git commit -m "refactor(engine-config): PodSection drops image-pull, gains env"
```

---

## Task 9: EngineDetail nav re-map + retire old wrappers

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx`
- Delete: `frontend/src/routes/control-plane/config/PropertiesSection.tsx`, `ResourcesSection.tsx`, `SchedulingSection.tsx`

- [ ] **Step 1: Update imports** in `EngineDetail.tsx`. Remove the imports of `PropertiesSection`, `ResourcesSection`, `SchedulingSection` (lines 71-74 region). Add:

```tsx
import { ImageSection } from "./config/ImageSection";
import { ServerSection } from "./config/ServerSection";
import { CoordinatorSection } from "./config/CoordinatorSection";
import { WorkerSection } from "./config/WorkerSection";
import { PodSection } from "./config/PodSection";
import { ContainersMountsSection } from "./config/ContainersMountsSection";
import { WorkloadSection } from "./config/WorkloadSection";
```

(`SchedulingSection` import is dropped; `WorkloadSection` stays.)

- [ ] **Step 2: Replace `NAV_ITEMS`** (lines 328-334):

```tsx
const NAV_ITEMS: { key: string; label: string }[] = [
  { key: "image", label: "Image" },
  { key: "server", label: "Server" },
  { key: "coordinator", label: "Coordinator" },
  { key: "worker", label: "Worker" },
  { key: "workload", label: "Workload" },
  { key: "pod", label: "Pod & Containers" },
  { key: "autoscaling", label: "Autoscaling" },
];
```

- [ ] **Step 3: Delete `CommonPropertiesSection`** (lines 338-398) — its content now lives in `ServerSection`. Remove the now-unused imports it needed (`QuantityInput`, `FieldLabel`, `LOG_LEVELS` const, `Select*` if unused elsewhere in the file — verify each before removing; `KeyValueEditor`/`Label` may still be used by `SecurityTab`).

- [ ] **Step 4: Set the default section + thread `imageVersion`.** Change `useState("properties")` (line 417) to:

```tsx
  const [section, setSection] = useState("image");
```

`ConfigurationTab` currently receives only `cfg` but the Image placeholder needs the stored tag. Change its signature and its single call site:

```tsx
// signature (line ~400):
function ConfigurationTab({ cfg, imageVersion }: { cfg: EngineConfigForm; imageVersion: string }) {
```

```tsx
// call site — find it with: rtk proxy grep -n "<ConfigurationTab" src/routes/control-plane/EngineDetail.tsx
// pass the stored tag (the caller has `engine` in scope):
<ConfigurationTab cfg={cfg} imageVersion={engine.spec.imageVersion} />
```

- [ ] **Step 5: Replace the content-pane routing** (lines 462-484) with:

```tsx
                {section === "image" && (
                  <ImageSection
                    cfg={cfg}
                    defaultImage={`trinodb/trino:${imageVersion}`}
                  />
                )}
                {section === "server" && <ServerSection cfg={cfg} />}
                {section === "coordinator" && <CoordinatorSection cfg={cfg} />}
                {section === "worker" && <WorkerSection cfg={cfg} />}
                {section === "workload" && <WorkloadSection cfg={cfg} />}
                {section === "pod" && (
                  <div className="space-y-5">
                    <PodSection
                      serviceAccount={cfg.serviceAccount}
                      setServiceAccount={cfg.setServiceAccount}
                      podSecurityContext={cfg.podSecurityContext}
                      setPodSecurityContext={cfg.setPodSecurityContext}
                      containerSecurityContext={cfg.containerSecurityContext}
                      setContainerSecurityContext={cfg.setContainerSecurityContext}
                      deploymentStrategy={cfg.deploymentStrategy}
                      setDeploymentStrategy={cfg.setDeploymentStrategy}
                      deploymentAnnotations={cfg.deploymentAnnotations}
                      setDeploymentAnnotations={cfg.setDeploymentAnnotations}
                      revisionHistoryLimit={cfg.revisionHistoryLimit}
                      setRevisionHistoryLimit={cfg.setRevisionHistoryLimit}
                      progressDeadlineSeconds={cfg.progressDeadlineSeconds}
                      setProgressDeadlineSeconds={cfg.setProgressDeadlineSeconds}
                      shareProcessNamespace={cfg.shareProcessNamespace}
                      setShareProcessNamespace={cfg.setShareProcessNamespace}
                      env={cfg.env}
                      setEnv={cfg.setEnv}
                    />
                    <ContainersMountsSection
                      initContainers={cfg.initContainers}
                      setInitContainers={cfg.setInitContainers}
                      sidecarContainers={cfg.sidecarContainers}
                      setSidecarContainers={cfg.setSidecarContainers}
                      envFrom={cfg.envFrom}
                      setEnvFrom={cfg.setEnvFrom}
                      configMounts={cfg.configMounts}
                      setConfigMounts={cfg.setConfigMounts}
                      secretMounts={cfg.secretMounts}
                      setSecretMounts={cfg.setSecretMounts}
                    />
                  </div>
                )}
                {section === "autoscaling" && (
                  <EngineAutoscaling
                    mode={autoscaleMode}
                    setMode={setAutoscaleMode}
                    hpa={autoscaleHpa}
                    setHpa={setAutoscaleHpa}
                    keda={autoscaleKeda}
                    setKeda={setAutoscaleKeda}
                  />
                )}
```

- [ ] **Step 6: Delete the retired wrapper files**

```bash
git rm frontend/src/routes/control-plane/config/PropertiesSection.tsx frontend/src/routes/control-plane/config/ResourcesSection.tsx frontend/src/routes/control-plane/config/SchedulingSection.tsx
```

- [ ] **Step 7: Delete `PlacementSection` wrapper** if unused (its `RolePlacementEditor` export is still used by Coordinator/Worker — keep the file, only the `PlacementSection` export is now dead). Remove the `PlacementSection` function (keep `RolePlacementEditor` + its helpers). Verify no remaining importers of `PlacementSection`:

Run (from `frontend/`): `rtk proxy grep -rn "PlacementSection\b" src` → expect only the definition line.

- [ ] **Step 8: Full frontend gate**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run test -- --run && npm run build`
Expected: all PASS.

- [ ] **Step 9: Commit**

```bash
git add -A frontend/src/routes/control-plane
git commit -m "feat(engine-config): re-map Configuration nav to mirror the Trino chart"
```

---

## Task 10: Verify end-to-end + full CI gate

**Files:** none (verification only).

- [ ] **Step 1: Backend full gate**

Run (repo root): `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS. (If a pre-existing unrelated failure appears — e.g. the known engine-job `open_secret` NULL-ws tests on a fresh DB — confirm it is not caused by this change.)

- [ ] **Step 2: Frontend full gate**

Run (from `frontend/`): `npm run lint && npm run typecheck && npm run test -- --run && npm run build`
Expected: PASS.

- [ ] **Step 3: Manual smoke (per the run/verify skill).** Start the local stack, open an engine's Configuration tab, and confirm:
  - 7 nav items in order: Image · Server · Coordinator · Worker · Workload · Pod & Containers · Autoscaling.
  - Server shows three separate lists; each "+ Add property" picker only offers that file's keys; a custom key added under **node** appears in `additionalNodeProperties` in the review-changes diff (not `config`).
  - Setting Image to `ghcr.io/acme/trino:471` then opening review-changes shows `image` added and nothing else changed; clearing it removes the key.
  - Save an untouched engine → review-changes diff is empty (byte-identical payload).

- [ ] **Step 4: Commit any doc/status updates** (if applicable), otherwise done.

---

## Self-review notes (author)

- **Spec coverage:** point 1 (picker) → Tasks 3–5; point 2 (split) → Tasks 4–5; point 3 (image) → Tasks 1, 2, 6; point 4 (re-map) → Tasks 5–9. `sectionCounts` re-partition → Task 2. Non-goals respected (wizard untouched; catalog unchanged; manifests unchanged except `image`).
- **Type consistency:** `PropertyPicker` props (`catalog`/`onAddCatalog`/`onAddCustom`) match `PropertyList` usage; `RolePlacementEditor` signature (`idPrefix`/`rawId`/`form`/`onChange`) matches Coordinator/Worker usage; `PodSectionProps` after Task 8 (no image-pull, adds `env`/`setEnv`) matches the Task 9 call site; hook field `image`/`setImage` used consistently in Tasks 2 & 6.
- **Guardrail:** every task keeps `builtTrinoConfig` byte-identical except the added `image` key (Task 1/2). Task 10 Step 3 verifies via the review-changes diff.
