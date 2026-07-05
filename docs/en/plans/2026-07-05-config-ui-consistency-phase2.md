# Config UI Consistency — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Apply the catalog-assisted "+ Add property" picker pattern (from Phase 1) to the remaining free-text config editors that have a known value-set: **Additional JVM config** (coordinator+worker), **Event listener**, and **Catalog properties** (per-connector). Leave genuinely-arbitrary editors (env vars, annotations, node selector, image-pull secrets) as free-text.

**Architecture:** Event-listener and catalog-props are keyed `key=value` maps with a scoped catalog — structurally identical to config/node/log — so generalize `PropertyList` into a reusable `KeyedPropertyList` (catalog + get/set/remove) and reuse it. JVM config is a `string[]` flag list, so it gets a small dedicated `JvmFlagPicker` over a `jvmFlagCatalog`. Three new catalog data files mirror `trinoCatalog.ts`'s shape.

**Tech Stack:** React 18 + TS + Tailwind v4 + shadcn, Vitest.

**Base branch:** `feat/engine-config-ui-remap` (Phase 1, already on this branch). Continue on the same branch.

**Spec:** design decisions captured inline below (Phase 1 spec: `docs/en/specs/2026-07-05-trino-engine-config-ui-remap-design.md`).

---

## Conventions

- Run frontend commands from `frontend/`. Gate per task: `npm run typecheck && npm run lint && npm run test -- --run`.
- Catalog data files: every entry must be a REAL Trino property/flag from the docs — never invent keys. `help` = one-line paraphrase; `docsUrl` = deep link. Curate the common/high-value keys; omit obscure internals. Custom keys always remain addable via the picker's free-text path.
- Commit identity: repo default. Conventional Commits. One commit per task.
- Guardrail: the built payloads (`builtTrinoConfig`, add-catalog request) stay byte-identical for unchanged inputs — these are UI/UX changes only.

---

## Task 1: `KeyedPropertyList` — generalize the per-file list

**Files:**
- Create: `frontend/src/routes/control-plane/config/KeyedPropertyList.tsx`
- Modify: `frontend/src/routes/control-plane/config/PropertyList.tsx` (delegate to it)

Extract the generic core of `PropertyList` (rows = current keys ∪ locally-added, filter box >3, empty/no-match messages, `PropertyRow`, the `PropertyPicker`) into a component decoupled from `config/node/log`. `PropertyList` becomes a thin wrapper binding the three-file case.

- [ ] **Step 1: Implement `KeyedPropertyList.tsx`** — same body as today's `PropertyList` but parameterised by explicit accessors instead of `file`:

```tsx
import { Search, Trash2 } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PropertyField } from "./PropertyField";
import { PropertyPicker } from "./PropertyPicker";
import { type TrinoProp } from "./trinoCatalog";

/**
 * A generic keyed-property editor: the union of `presentKeys` and any
 * locally-added key not yet present, each rendered via `PropertyField` (typed by
 * `catalogByKey`) or a plain input for custom keys, plus a scoped "+ Add
 * property" picker. Values are read/written through `getValue`/`setValue`
 * (blank `setValue` removes). Decoupled from any specific config file — callers
 * bind it to config/node/log, event-listener, or a connector's props.
 */
export function KeyedPropertyList({
  catalog,
  catalogByKey,
  presentKeys,
  getValue,
  setValue,
  addLabel = "Add property",
}: {
  catalog: TrinoProp[];
  catalogByKey: Record<string, TrinoProp>;
  presentKeys: string[];
  getValue: (key: string) => string;
  setValue: (key: string, value: string) => void;
  addLabel?: string;
}) {
  const [search, setSearch] = useState("");
  const [added, setAdded] = useState<string[]>([]);

  const addKey = (key: string) =>
    setAdded((prev) => (prev.includes(key) ? prev : [...prev, key]));
  const addCatalog = (prop: TrinoProp) => {
    addKey(prop.key);
    if ((prop.default ?? "").trim() !== "") setValue(prop.key, prop.default as string);
  };
  const removeRow = (key: string) => {
    setValue(key, "");
    setAdded((prev) => prev.filter((k) => k !== key));
  };

  const inPresent = new Set(presentKeys);
  const keys = [...presentKeys, ...added.filter((k) => !inPresent.has(k))];
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
          {filtered.map((k) => {
            const cat = catalogByKey[k];
            return (
              <div key={k} className="flex items-start gap-3 py-2">
                <div className="flex w-56 shrink-0 flex-col gap-1 pt-1.5">
                  <span className="break-all font-mono text-xs">{k}</span>
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
                        key: k,
                        section: cat.group,
                        label: k,
                        type: cat.type,
                        help: cat.help,
                        docsUrl: cat.docsUrl,
                        enumOptions: cat.enumOptions,
                        default: cat.default,
                      }}
                      value={getValue(k)}
                      onChange={(v) => setValue(k, v)}
                    />
                  ) : (
                    <Input
                      value={getValue(k)}
                      placeholder="value"
                      onChange={(e) => setValue(k, e.target.value)}
                    />
                  )}
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mt-1 shrink-0 text-muted-foreground"
                  onClick={() => removeRow(k)}
                  aria-label={`Remove ${k}`}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            );
          })}
        </div>
      )}
      <PropertyPicker
        catalog={catalog}
        onAddCatalog={addCatalog}
        onAddCustom={(key) => addKey(key)}
      />
    </div>
  );
}
```

- [ ] **Step 2: Rewrite `PropertyList.tsx`** as a thin wrapper (preserves the exact config/node/log behavior; `PropertyRow` inlined into `KeyedPropertyList`, so remove it here):

```tsx
import { useMemo } from "react";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { KeyedPropertyList } from "./KeyedPropertyList";
import { trinoCatalog, trinoCatalogByKey } from "./trinoCatalog";

type PropFile = "config" | "node" | "log";

/** The property list for one Trino config file, backed by the file-scoped
 *  `trinoCatalog` subset and the hook's `getProp`/`setProp` router. */
export function PropertyList({
  cfg,
  file,
  rows,
}: {
  cfg: EngineConfigForm;
  file: PropFile;
  rows: { key: string }[];
}) {
  const catalog = useMemo(
    () => trinoCatalog.filter((p) => p.file === file),
    [file],
  );
  return (
    <KeyedPropertyList
      catalog={catalog}
      catalogByKey={trinoCatalogByKey}
      presentKeys={rows.map((r) => r.key)}
      getValue={(k) => cfg.getProp(file, k)}
      setValue={(k, v) => cfg.setProp(file, k, v)}
    />
  );
}
```

- [ ] **Step 3:** `npm run typecheck && npm run test -- --run`. Expect clean/green (PropertyPicker test unaffected). Manually confirm config/node/log still render identically in the app is deferred to Task 5 smoke.

- [ ] **Step 4: Commit** — `git commit -m "refactor(engine-config): extract reusable KeyedPropertyList"`

---

## Task 2: Additional JVM config — flag catalog + picker

**Files:**
- Create: `frontend/src/routes/control-plane/config/jvmFlagCatalog.ts`
- Create: `frontend/src/routes/control-plane/config/JvmFlagPicker.tsx`
- Modify: `frontend/src/routes/control-plane/EngineRoleConfig.tsx` (the "Additional JVM config" list, ~line 284-291)

JVM config is a `string[]` of raw `jvm.config` lines. The picker appends a chosen flag's **template** (e.g. `-XX:MaxRAMPercentage=`) that the user then edits; boolean flags (`-XX:+UseG1GC`) append as-is.

- [ ] **Step 1: Create `jvmFlagCatalog.ts`.** Shape:

```ts
export interface JvmFlag {
  /** The flag text to insert (a template; value-taking flags end with `=`). */
  flag: string;
  group: string;
  help: string;
  docsUrl?: string;
}

export const jvmFlagCatalog: JvmFlag[] = [ /* entries — see below */ ];
```

Populate with the common, real Trino JVM/GC flags (author `help` from the Trino "JVM config" docs; `docsUrl` = `https://trino.io/docs/current/installation/deployment.html#jvm-config` unless a better anchor exists). Include at least:
`-XX:+UseG1GC`, `-XX:G1HeapRegionSize=`, `-XX:+ExplicitGCInvokesConcurrent`, `-XX:+HeapDumpOnOutOfMemoryError`, `-XX:+ExitOnOutOfMemoryError`, `-XX:-OmitStackTraceInFastThrow`, `-XX:ReservedCodeCacheSize=`, `-XX:PerMethodRecompilationCutoff=`, `-XX:PerBytecodeRecompilationCutoff=`, `-Djdk.attach.allowAttachSelf=true`, `-Dfile.encoding=UTF-8`, `-XX:MaxRAMPercentage=`, `-XX:InitialRAMPercentage=`, `-XX:+UseStringDeduplication`, `-verbose:gc`, `-Xlog:gc*`. Group them (`GC`, `Diagnostics`, `Memory`, `Runtime`).

- [ ] **Step 2: Create `JvmFlagPicker.tsx`** — mirror `PropertyPicker`'s collapsed-button → search-list UX, but over `JvmFlag[]`, calling `onAdd(flag: string)` with the chosen `flag` template (or the raw typed text for a custom flag). Button label "Add flag", placeholder "Search JVM flags…". (Model the structure on `PropertyPicker.tsx`; the list rows show `flag` + group chip + help.)

```tsx
import { Plus, Search } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { type JvmFlag, jvmFlagCatalog } from "./jvmFlagCatalog";

const MAX = 30;

export function JvmFlagPicker({ onAdd }: { onAdd: (flag: string) => void }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const matches = (q
    ? jvmFlagCatalog.filter(
        (f) => f.flag.toLowerCase().includes(q) || f.help.toLowerCase().includes(q),
      )
    : jvmFlagCatalog
  ).slice(0, MAX);
  const close = () => { setOpen(false); setQuery(""); };
  const add = (flag: string) => { const t = flag.trim(); if (!t) return; onAdd(t); close(); };

  if (!open)
    return (
      <Button type="button" variant="outline" size="sm" onClick={() => setOpen(true)}>
        <Plus className="h-4 w-4" />
        Add flag
      </Button>
    );

  return (
    <div className="relative w-full sm:w-96">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        autoFocus
        placeholder="Search JVM flags…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onBlur={() => window.setTimeout(close, 150)}
        onKeyDown={(e) => {
          if (e.key === "Escape") close();
          else if (e.key === "Enter") { e.preventDefault(); add(query); }
        }}
        className="pl-8"
      />
      <ul className="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md">
        {matches.map((f) => (
          <li key={f.flag}>
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => add(f.flag)}
              className="flex w-full flex-col items-start gap-0.5 rounded-sm px-2 py-1.5 text-left hover:bg-muted"
            >
              <div className="flex w-full items-center gap-2">
                <span className="font-mono text-xs">{f.flag}</span>
                <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">{f.group}</span>
              </div>
              <span className="line-clamp-1 text-xs text-muted-foreground">{f.help}</span>
            </button>
          </li>
        ))}
        {q && !matches.some((f) => f.flag === query.trim()) && (
          <li>
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => add(query)}
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-muted"
            >
              <span className="text-muted-foreground">+ Add custom flag</span>
              <span className="font-mono text-xs">{query.trim()}</span>
            </button>
          </li>
        )}
      </ul>
    </div>
  );
}
```

- [ ] **Step 3: Wire into `EngineRoleConfig.tsx`.** In the "Additional JVM config" `ConfigSection` (~line 284), keep the existing per-row editable inputs (each flag line stays a text `Input` so templates like `-XX:MaxRAMPercentage=` are editable + removable), but REPLACE the plain "Add JVM flag" button with `<JvmFlagPicker onAdd={(flag) => set("additionalJvmConfig", [...value.additionalJvmConfig, flag])} />`. Read the current list rendering first and adapt minimally — do not change the payload shape (`additionalJvmConfig: string[]`).

- [ ] **Step 4:** `npm run typecheck && npm run lint`. Expect clean.

- [ ] **Step 5: Commit** — `git commit -m "feat(engine-config): catalog-assisted JVM flag picker"`

---

## Task 3: Event listener — catalog + KeyedPropertyList

**Files:**
- Create: `frontend/src/routes/control-plane/config/eventListenerCatalog.ts`
- Modify: `frontend/src/routes/control-plane/config/ServerSection.tsx` (Event listener section)
- Modify: `frontend/src/routes/control-plane/useEngineConfigForm.ts` (add a getter/setter for the eventListener map, mirroring `getProp`/`setProp`)

- [ ] **Step 1: Create `eventListenerCatalog.ts`** — a `TrinoProp[]`-compatible export (reuse the `TrinoProp` shape from `trinoCatalog.ts`, `file` omitted or set to a dummy — since KeyedPropertyList ignores `file`, define a local minimal type or reuse `TrinoProp` with `file: "config"`). Author real `event-listener.properties` keys from the Trino event-listener docs:
`event-listener.name` (enum: `http`, `mysql`, `openlineage`), and the common `http-event-listener.*` keys (`http-event-listener.connect-ingest-uri`, `.connect-http-headers`, `.log-created`, `.log-completed`, `.log-split`, `.connect-retry-count`, `.connect-retry-delay`, `.connect-backoff-base`, `.connect-max-delay`), `mysql-event-listener.db.url`, and the common `openlineage-event-listener.*` keys. Provide a `eventListenerCatalogByKey` map like `trinoCatalogByKey`.

- [ ] **Step 2: Add `getEventListener`/`setEventListener` accessors** to `useEngineConfigForm` mirroring `getProp`/`setProp` but over the `eventListener` `Kv[]` state (blank removes). Expose them on `EngineConfigForm` + the returned object. (The existing `eventListener`/`setEventListener` Kv-array stays for prefill/build; add key-level helpers `getEventListenerProp(key)` / `setEventListenerProp(key, val)`.)

- [ ] **Step 3: In `ServerSection.tsx`**, replace the Event-listener `KeyValueEditor` with:

```tsx
<KeyedPropertyList
  catalog={eventListenerCatalog}
  catalogByKey={eventListenerCatalogByKey}
  presentKeys={eventListener.map((kv) => kv.key)}
  getValue={(k) => cfg.getEventListenerProp(k)}
  setValue={(k, v) => cfg.setEventListenerProp(k, v)}
/>
```

Remove the now-unused `KeyValueEditor` import if nothing else in the file uses it.

- [ ] **Step 4:** `npm run typecheck && npm run lint && npm run test -- --run`. Expect clean/green. (Payload: `eventListener` map still built identically.)

- [ ] **Step 5: Commit** — `git commit -m "feat(engine-config): catalog-assisted Event listener editor"`

---

## Task 4: Catalog properties — per-connector catalog + picker

**Files:**
- Create: `frontend/src/routes/control-plane/config/connectorCatalog.ts`
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx` (`AddCatalogDialog` generic-connector branch, ~line 944-975)

- [ ] **Step 1: Create `connectorCatalog.ts`.** Export a curated map of connector → property list, plus the connector list for a combobox:

```ts
import { type TrinoProp } from "./trinoCatalog";
export interface Connector { name: string; label: string; props: TrinoProp[]; }
export const connectors: Connector[] = [ /* ... */ ];
export const connectorByName: Record<string, Connector>;
```

Curate the common connectors and their high-value real properties from the Trino connector docs (never invent keys). Cover at least: **mysql** (`connection-url`, `connection-user`, `connection-password`, `case-insensitive-name-matching`, …), **postgresql**, **sqlserver**, **clickhouse**, **iceberg** (`iceberg.catalog.type`, `hive.metastore.uri`, `fs.native-s3.enabled`, `s3.endpoint`, `s3.region`, …), **hive**, **delta_lake**, **kafka** (`kafka.nodes`, `kafka.table-names`, …), **mongodb** (`mongodb.connection-url`). Each entry: `{ key, group, type, help, docsUrl, default?, enumOptions? }` (reuse `TrinoProp`; `file` may be set to `"config"` and is ignored here). Keep the set curated (5-15 key props per connector); custom keys stay addable.

- [ ] **Step 2: In `AddCatalogDialog` (`EngineDetail.tsx`), upgrade the `generic` branch:** make `connectorName` a combobox seeded from `connectors` (still allowing a free-typed connector via an "Other" option that keeps the current text input), and replace the `genericProps` `KeyValueEditor` with a `KeyedPropertyList` scoped to the selected connector's catalog:

```tsx
<KeyedPropertyList
  catalog={connectorByName[connectorName]?.props ?? []}
  catalogByKey={Object.fromEntries((connectorByName[connectorName]?.props ?? []).map((p) => [p.key, p]))}
  presentKeys={genericProps.map((kv) => kv.key)}
  getValue={(k) => genericProps.find((kv) => kv.key === k)?.value ?? ""}
  setValue={(k, v) =>
    setGenericProps((prev) => {
      const rest = prev.filter((kv) => kv.key !== k);
      return v.trim() === "" ? rest : [...rest, { key: k, value: v }];
    })
  }
/>
```

Preserve the existing `AddCatalogInput` payload shape (connectorName + properties map) exactly — this is a UI change only. When the connector isn't in the catalog, the picker offers only custom keys (empty catalog) — acceptable.

- [ ] **Step 3:** `npm run typecheck && npm run lint`. Expect clean.

- [ ] **Step 4: Commit** — `git commit -m "feat(engine-config): per-connector catalog in add-catalog wizard"`

---

## Task 5: Verify + live smoke

- [ ] **Step 1: Full frontend gate** — `npm run typecheck && npm run lint && npm run test -- --run && npm run build`. All pass.
- [ ] **Step 2: Live smoke** (stack already running from Phase 1): Server → each property list still works; Coordinator/Worker → Additional JVM config picker lists flags; Event listener picker lists `event-listener.*` keys; Add catalog → Generic → connector combobox + per-connector property picker. 0 console errors; Review-changes payloads unchanged for untouched fields.
- [ ] **Step 3: Commit** any final tidy; done.

---

## Self-review notes

- **Coverage:** JVM (T2), Event listener (T3), Catalog props (T4); shared generalization (T1); verify (T5). Arbitrary-value editors intentionally excluded (documented in the audit).
- **Type consistency:** `KeyedPropertyList` props (`catalog`/`catalogByKey`/`presentKeys`/`getValue`/`setValue`) are consumed identically in T1 (PropertyList), T3 (event listener), T4 (connector). `JvmFlag`/`JvmFlagPicker.onAdd(flag)` consistent T2. New hook accessors `getEventListenerProp`/`setEventListenerProp` used only in T3.
- **Guardrail:** all four tasks are UI-only; no payload field changes (config/node/log, eventListener map, additionalJvmConfig[], add-catalog request all keep their existing shapes).
