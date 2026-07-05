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
