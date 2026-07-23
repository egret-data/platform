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
  excludeKeys,
  onAddCatalog,
  onAddCustom,
}: {
  catalog: TrinoProp[];
  /** Keys already present in the list — hidden from the picker so a property
   *  can't be added twice. */
  excludeKeys?: Set<string>;
  onAddCatalog: (prop: TrinoProp) => void;
  onAddCustom: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  const q = query.trim().toLowerCase();
  const available = excludeKeys
    ? catalog.filter((p) => !excludeKeys.has(p.key))
    : catalog;
  const matches = q
    ? available
        .filter(
          (p) =>
            p.key.toLowerCase().includes(q) || p.help.toLowerCase().includes(q),
        )
        .slice(0, MAX_ADD_RESULTS)
    : available.slice(0, MAX_ADD_RESULTS);

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
    // Already in the list — nothing to add.
    if (excludeKeys?.has(key)) {
      close();
      return;
    }
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
            // Prefer the top catalog match (autocomplete); only add a custom
            // key when the query matches nothing — otherwise a partial search
            // like "memory" would create a bogus "memory" key.
            if (matches.length > 0) selectProp(matches[0]);
            else addCustom();
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
        {q && !matches.some((p) => p.key === q) && !excludeKeys?.has(query.trim()) && (
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
