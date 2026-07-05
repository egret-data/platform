import { Plus, Search } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { jvmFlagCatalog } from "./jvmFlagCatalog";

const MAX = 30;

/** A "+ Add flag" button that opens a searchable list of common Trino JVM
 *  flags. Selecting one calls `onAdd` with the flag template (value-taking
 *  flags end with `=` for the caller to complete); Enter on unmatched text adds
 *  it as a custom flag. */
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
