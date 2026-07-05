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
}: {
  catalog: TrinoProp[];
  catalogByKey: Record<string, TrinoProp>;
  presentKeys: string[];
  getValue: (key: string) => string;
  setValue: (key: string, value: string) => void;
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
