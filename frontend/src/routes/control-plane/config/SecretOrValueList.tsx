import { KeyRound, Search, Trash2, Type } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { PropertyField } from "./PropertyField";
import { PropertyPicker } from "./PropertyPicker";
import { SELECT_NONE } from "./podEditors";
import { type TrinoProp } from "./trinoCatalog";

/**
 * Like `KeyedPropertyList`, but each row's value is either a typed plaintext
 * value or a reference to an egret secret, switched by a per-row toggle. Used
 * by the v2 catalog form so a connector property can pull from a secret
 * instead of being typed in plain text.
 *
 * A key is in "secret mode" iff it is present in `secrets` (`key in secrets`);
 * otherwise it's in "value mode". Toggling calls `setSecret(key, "")` or
 * `setValue(key, "")` — the caller is responsible for dropping the key from
 * the other map (per the prop contract), so `secrets`/`values` stay disjoint.
 */
export function SecretOrValueList({
  catalog,
  catalogByKey,
  values,
  secrets,
  secretOptions,
  setValue,
  setSecret,
  remove,
}: {
  catalog: TrinoProp[];
  catalogByKey: Record<string, TrinoProp>;
  values: Record<string, string>;
  secrets: Record<string, string>;
  secretOptions: { id: string; name: string }[];
  setValue: (key: string, value: string) => void;
  setSecret: (key: string, secretId: string) => void;
  remove: (key: string) => void;
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
    remove(key);
    setAdded((prev) => prev.filter((k) => k !== key));
  };

  const present = new Set([...Object.keys(values), ...Object.keys(secrets)]);
  const keys = [...present, ...added.filter((k) => !present.has(k))];
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
            const isSecret = k in secrets;
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
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mt-1 shrink-0 text-muted-foreground"
                  onClick={() =>
                    isSecret ? setValue(k, "") : setSecret(k, "")
                  }
                  aria-label={isSecret ? "Use value" : "Use secret"}
                  title={isSecret ? "Use value" : "Use secret"}
                >
                  {isSecret ? (
                    <Type className="h-4 w-4" />
                  ) : (
                    <KeyRound className="h-4 w-4" />
                  )}
                </Button>
                <div className="min-w-0 flex-1">
                  {isSecret ? (
                    <Select
                      value={secrets[k] || SELECT_NONE}
                      onValueChange={(v) =>
                        setSecret(k, v === SELECT_NONE ? "" : v)
                      }
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={SELECT_NONE}>
                          (select a secret)
                        </SelectItem>
                        {secretOptions.map((opt) => (
                          <SelectItem key={opt.id} value={opt.id}>
                            {opt.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : cat ? (
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
                      value={values[k] ?? ""}
                      onChange={(v) => setValue(k, v)}
                    />
                  ) : (
                    <Input
                      value={values[k] ?? ""}
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
