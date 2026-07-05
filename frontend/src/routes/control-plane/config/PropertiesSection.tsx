import { Search, Trash2 } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { PropertyField } from "./PropertyField";
import { trinoCatalog, trinoCatalogByKey } from "./trinoCatalog";

type PropFile = "config" | "node" | "log";

/** A small pill chip (property group, or "custom"). */
function Chip({ children }: { children: React.ReactNode }) {
  return (
    <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
      {children}
    </span>
  );
}

const MAX_ADD_RESULTS = 30;

/**
 * The add-from-catalog combobox: an `<Input>` whose typed text filters the full
 * `trinoCatalog` by key/help. Selecting a result adds a row via `setProp` (with
 * the catalog `default` as its initial value); pressing Enter on text that
 * matches no catalog key adds an empty `config` row with that free-text key.
 */
function AddProperty({ cfg }: { cfg: EngineConfigForm }) {
  const [query, setQuery] = useState("");
  const [focused, setFocused] = useState(false);

  const q = query.trim().toLowerCase();
  const matches = q
    ? trinoCatalog
        .filter(
          (p) =>
            p.key.toLowerCase().includes(q) ||
            p.help.toLowerCase().includes(q),
        )
        .slice(0, MAX_ADD_RESULTS)
    : [];
  const open = focused && q.length > 0;

  const selectProp = (key: string) => {
    const p = trinoCatalogByKey[key];
    if (!p) return;
    cfg.setProp(p.file, p.key, p.default ?? "");
    setQuery("");
    setFocused(false);
  };

  const addCustom = () => {
    const key = query.trim();
    if (!key) return;
    const existing = trinoCatalogByKey[key];
    if (existing) {
      selectProp(key);
      return;
    }
    cfg.setProp("config", key, "");
    setQuery("");
    setFocused(false);
  };

  return (
    <div className="relative w-full sm:w-96">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        placeholder="Add a property…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => window.setTimeout(() => setFocused(false), 150)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setQuery("");
            setFocused(false);
          } else if (e.key === "Enter") {
            e.preventDefault();
            addCustom();
          }
        }}
        className="pl-8"
      />
      {open && (
        <ul className="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md">
          {matches.map((p) => (
            <li key={`${p.file}:${p.key}`}>
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => selectProp(p.key)}
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
        </ul>
      )}
    </div>
  );
}

/** A single property row: mono key + group chip + a typed value editor + remove. */
function PropertyRow({
  cfg,
  file,
  keyName,
}: {
  cfg: EngineConfigForm;
  file: PropFile;
  keyName: string;
}) {
  const cat = trinoCatalogByKey[keyName];
  return (
    <div className="flex items-start gap-3 py-2">
      <div className="flex w-56 shrink-0 flex-col gap-1 pt-1.5">
        <span className="break-all font-mono text-xs">{keyName}</span>
        <div>
          <Chip>{cat?.group ?? "custom"}</Chip>
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
        onClick={() => cfg.setProp(file, keyName, "")}
        aria-label={`Remove ${keyName}`}
      >
        <Trash2 className="h-4 w-4" />
      </Button>
    </div>
  );
}

/**
 * The unified Properties list: the union of the three cluster-wide property maps
 * (`config.properties` / `node.properties` / `log.properties`), each row tagged
 * with its file. A search box filters by key; each row renders a typed editor
 * (via the Trino catalog) or a plain value input for custom keys, plus a remove
 * button. The "Add property" combobox searches the full catalog (or adds a
 * free-text custom key). All writes route through `cfg.setProp(file, key, …)`,
 * so the three payload maps stay correct.
 */
export function PropertiesSection({ cfg }: { cfg: EngineConfigForm }) {
  const [search, setSearch] = useState("");

  const rows: { file: PropFile; key: string }[] = [
    ...cfg.configProps.map((kv) => ({ file: "config" as const, key: kv.key })),
    ...cfg.nodeProps.map((kv) => ({ file: "node" as const, key: kv.key })),
    ...cfg.logProps.map((kv) => ({ file: "log" as const, key: kv.key })),
  ];

  const s = search.trim().toLowerCase();
  const filtered = s
    ? rows.filter((r) => r.key.toLowerCase().includes(s))
    : rows;

  return (
    <ConfigSection
      title="Properties"
      description="Cluster-wide Trino config.properties / node.properties / log.properties. Pick from the catalog or add a custom key."
    >
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

      {rows.length === 0 ? (
        <p className="py-6 text-center text-sm text-muted-foreground">
          No properties set — add one below.
        </p>
      ) : filtered.length === 0 ? (
        <p className="py-6 text-center text-sm text-muted-foreground">
          No properties match “{search}”.
        </p>
      ) : (
        <div className="divide-y">
          {filtered.map((r) => (
            <PropertyRow
              key={`${r.file}:${r.key}`}
              cfg={cfg}
              file={r.file}
              keyName={r.key}
            />
          ))}
        </div>
      )}

      <div className="border-t pt-4">
        <AddProperty cfg={cfg} />
      </div>
    </ConfigSection>
  );
}
