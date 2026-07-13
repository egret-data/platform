import { useMemo, useState, type ReactNode } from "react";
import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";

export interface Column<T> {
  key: string;
  label: string;
  sortable?: boolean;
  className?: string;
  /** Value used for search + sort. Omit for non-searchable/non-sortable columns. */
  value?: (row: T) => string | number;
  cell: (row: T) => ReactNode;
}

export function FilterableTable<T>({
  rows,
  columns,
  getId,
  searchPlaceholder = "Search…",
  filters,
  initialSort,
  emptyText = "No results.",
  selection,
}: {
  rows: T[];
  columns: Column<T>[];
  getId: (row: T) => string;
  searchPlaceholder?: string;
  filters?: ReactNode;
  initialSort?: SortState<string>;
  emptyText?: string;
  selection?: {
    selected: Set<string>;
    onToggle: (id: string) => void;
    allSelected: boolean;
    onToggleAll: () => void;
  };
}) {
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<SortState<string> | undefined>(initialSort);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    let r = rows;
    if (needle) {
      r = r.filter((row) =>
        columns.some((c) =>
          String(c.value?.(row) ?? "")
            .toLowerCase()
            .includes(needle),
        ),
      );
    }
    if (sort) {
      const col = columns.find((c) => c.key === sort.key);
      if (col?.value) {
        r = [...r].sort((a, b) => {
          const av = col.value!(a);
          const bv = col.value!(b);
          const cmp =
            typeof av === "number" && typeof bv === "number"
              ? av - bv
              : String(av).localeCompare(String(bv));
          return sort.dir === "asc" ? cmp : -cmp;
        });
      }
    }
    return r;
  }, [rows, q, sort, columns]);

  const onSort = (key: string) =>
    setSort((s) =>
      s?.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" },
    );

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative max-w-xs flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="pl-8"
            placeholder={searchPlaceholder}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        {filters}
      </div>
      <div className="overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow>
              {selection && (
                <TableHead className="w-8">
                  <Checkbox
                    aria-label="Select all"
                    checked={selection.allSelected}
                    onCheckedChange={() => selection.onToggleAll()}
                  />
                </TableHead>
              )}
              {columns.map((c) =>
                c.sortable ? (
                  <SortableHeader
                    key={c.key}
                    sortKey={c.key}
                    label={c.label}
                    current={sort ?? { key: "", dir: "asc" }}
                    onSort={onSort}
                    className={c.className}
                  />
                ) : (
                  <TableHead key={c.key} className={c.className}>
                    {c.label}
                  </TableHead>
                ),
              )}
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell
                  colSpan={columns.length + (selection ? 1 : 0)}
                  className="py-6 text-center text-muted-foreground"
                >
                  {emptyText}
                </TableCell>
              </TableRow>
            ) : (
              shown.map((row) => (
                <TableRow key={getId(row)}>
                  {selection && (
                    <TableCell className="w-8">
                      <Checkbox
                        aria-label="Select row"
                        checked={selection.selected.has(getId(row))}
                        onCheckedChange={() => selection.onToggle(getId(row))}
                      />
                    </TableCell>
                  )}
                  {columns.map((c) => (
                    <TableCell key={c.key} className={c.className}>
                      {c.cell(row)}
                    </TableCell>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
