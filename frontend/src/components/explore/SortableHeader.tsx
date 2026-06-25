import { ArrowDown, ArrowUp } from "lucide-react";

export interface SortState<K extends string> { key: K; dir: "asc" | "desc" }

export function SortableHeader<K extends string>({ sortKey, label, current, onSort, className }: {
  sortKey: K;
  label: string;
  current: SortState<K>;
  onSort: (key: K) => void;
  className?: string;
}) {
  const active = current.key === sortKey;
  return (
    <th className={`cursor-pointer select-none py-1 hover:text-foreground ${className ?? ""}`} onClick={() => onSort(sortKey)}>
      <span className="inline-flex items-center gap-1">
        {label}
        {active && (current.dir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />)}
      </span>
    </th>
  );
}
