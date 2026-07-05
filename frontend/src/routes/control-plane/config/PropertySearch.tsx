import { useState } from "react";
import { Search } from "lucide-react";

import { Input } from "@/components/ui/input";
import { knownProperties } from "./knownProperties";

// Display labels for the section keys used by `knownProperties`. Keeps the
// search results readable and matches the vertical nav's section labels.
const SECTION_LABELS: Record<string, string> = {
  queryMemory: "Query & Memory",
  spilling: "Spilling",
  execution: "Execution & Optimizer",
  sqlWebUi: "SQL & Web UI",
  reliability: "Reliability",
  advanced: "Advanced",
};

function sectionLabel(section: string): string {
  return SECTION_LABELS[section] ?? section;
}

const MAX_RESULTS = 8;

/**
 * A search box over the `knownProperties` catalog. Filters by key / label /
 * help (case-insensitive substring) and, on selecting a match, jumps the
 * Configuration vertical nav to that property's section via `onJump`.
 */
export function PropertySearch({ onJump }: { onJump: (section: string) => void }) {
  const [query, setQuery] = useState("");

  const q = query.trim().toLowerCase();
  const matches = q
    ? knownProperties
        .filter(
          (p) =>
            p.key.toLowerCase().includes(q) ||
            p.label.toLowerCase().includes(q) ||
            p.help.toLowerCase().includes(q),
        )
        .slice(0, MAX_RESULTS)
    : [];

  const select = (section: string) => {
    onJump(section);
    setQuery("");
  };

  return (
    <div className="relative w-full sm:w-72">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        type="search"
        placeholder="Search properties…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") setQuery("");
        }}
        onBlur={() => {
          // Delay so a click on a result registers before we clear.
          window.setTimeout(() => setQuery(""), 150);
        }}
        className="pl-8"
      />
      {matches.length > 0 && (
        <ul className="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md">
          {matches.map((p) => (
            <li key={p.key}>
              <button
                type="button"
                // onMouseDown fires before the input's blur, so the jump runs.
                onMouseDown={(e) => {
                  e.preventDefault();
                  select(p.section);
                }}
                className="flex w-full flex-col items-start gap-0.5 rounded-sm px-2 py-1.5 text-left hover:bg-muted"
              >
                <div className="flex w-full items-center justify-between gap-2">
                  <span className="text-sm font-medium">{p.label}</span>
                  <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                    {sectionLabel(p.section)}
                  </span>
                </div>
                <span className="font-mono text-xs text-muted-foreground">{p.key}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
