import type { Cell } from "@/api/notebooks";

export interface TocEntry {
  id: string;
  label: string;
  level: number;
}

/** Derive a table-of-contents from cells: markdown headings become entries at
 *  their heading level; code/sql cells become level-3 entries labelled with
 *  their execution count + first non-blank source line. */
export function deriveToc(cells: Cell[]): TocEntry[] {
  const sorted = [...cells].sort((a, b) => a.cell_order - b.cell_order);
  const entries: TocEntry[] = [];
  for (const c of sorted) {
    if (c.cell_type === "markdown") {
      const heading = c.source
        .split("\n")
        .map((l) => l.match(/^(#{1,6})\s+(.*)$/))
        .find(Boolean);
      if (heading) {
        entries.push({ id: c.id, label: heading[2].trim(), level: heading[1].length });
      }
      continue;
    }
    const first = c.source.split("\n").find((l) => l.trim()) ?? "(empty)";
    const n = c.execution_count == null ? " " : String(c.execution_count);
    entries.push({ id: c.id, label: `In [${n}] · ${first.trim()}`, level: 3 });
  }
  return entries;
}
