import { Search, Trash2 } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ConfigTable, ConfigTableRow } from "./ConfigTable";
import { PropertyValue } from "./PropertyField";
import { PropertyPicker } from "./PropertyPicker";
import { type TrinoProp } from "./trinoCatalog";

/**
 * A generic keyed-property editor rendered as a `Property | Details | Value`
 * table (matching the scalar config tables): the union of `presentKeys` and any
 * locally-added key, each row showing the key (monospace) with its group, the
 * catalog help + docs link, and the typed value control (or a plain input for
 * custom keys) plus a remove button. Values are read/written through
 * `getValue`/`setValue` (blank `setValue` removes). A scoped "+ Add property"
 * picker and, past a few rows, a filter box bracket the table. Decoupled from
 * any specific config file — callers bind it to config/node/log, event-listener,
 * or a connector's props.
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
        <ConfigTable settingLabel="Property">
          {filtered.map((k) => {
            const cat = catalogByKey[k];
            return (
              <ConfigTableRow
                key={k}
                mono
                label={k}
                details={cat?.help ?? "Custom property — not in the catalog."}
                docsUrl={cat?.docsUrl}
              >
                <div className="flex items-start gap-1">
                  <div className="min-w-0 flex-1">
                    {cat ? (
                      <PropertyValue
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
                        ariaLabel={k}
                      />
                    ) : (
                      <Input
                        className="w-full"
                        aria-label={k}
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
                    className="h-9 w-9 shrink-0 text-muted-foreground"
                    onClick={() => removeRow(k)}
                    aria-label={`Remove ${k}`}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </ConfigTableRow>
            );
          })}
        </ConfigTable>
      )}
      <PropertyPicker
        catalog={catalog}
        excludeKeys={new Set(keys)}
        onAddCatalog={addCatalog}
        onAddCustom={(key) => addKey(key)}
      />
    </div>
  );
}
