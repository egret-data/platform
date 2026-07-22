import { ArrowDown, ArrowUp } from "lucide-react";

import { cn } from "@/lib/utils";
import { TableHead } from "@/components/ui/table";

export interface SortState<K extends string> { key: K; dir: "asc" | "desc" }

export function SortableHeader<K extends string>({ sortKey, label, current, onSort, className }: {
  sortKey: K;
  label: string;
  current: SortState<K>;
  onSort: (key: K) => void;
  className?: string;
}) {
  const active = current.key === sortKey;
  // Render the shared TableHead so padding, height, and alignment match the
  // non-sortable headers and the body cells exactly — a raw <th> here would
  // drop the base px-2/h-10 and shift every sortable column out of line.
  return (
    <TableHead
      className={cn("cursor-pointer select-none hover:text-foreground", className)}
      aria-sort={active ? (current.dir === "asc" ? "ascending" : "descending") : undefined}
      onClick={() => onSort(sortKey)}
    >
      <span className="inline-flex items-center gap-1">
        {label}
        {active && (current.dir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />)}
      </span>
    </TableHead>
  );
}
