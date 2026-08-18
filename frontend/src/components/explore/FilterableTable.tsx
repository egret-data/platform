import { useMemo, useState, type ReactNode } from "react";
import { ChevronLeft, ChevronRight, Search } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
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
  noMatchText = "No matches.",
  selection,
  pageSize,
  bordered,
  hideSearch,
}: {
  rows: T[];
  columns: Column<T>[];
  getId: (row: T) => string;
  searchPlaceholder?: string;
  filters?: ReactNode;
  initialSort?: SortState<string>;
  /** Shown when there is genuinely no data. Pass an `<EmptyState>` for a CTA. */
  emptyText?: ReactNode;
  /** Shown when a search query matches nothing (data exists). Kept plain so a
   * first-run CTA never appears just because a search missed. */
  noMatchText?: ReactNode;
  selection?: {
    selected: Set<string>;
    onToggle: (id: string) => void;
  };
  /**
   * When set, only this many rows render at a time and a pager is shown once
   * the filtered set exceeds it. Opt-in — omitting it keeps the table
   * un-paginated (renders every row), matching the original behavior.
   */
  pageSize?: number;
  /** Wrap the table in a rounded border so it reads as a contained block
   * (for card-less pages). */
  bordered?: boolean;
  /**
   * Suppress this component's own search box, for a page that owns its search
   * in its toolbar and hands `rows` in already filtered (see `Clients.tsx`).
   *
   * The internal `q` then stays `""` for the component's lifetime, so the
   * empty body always renders `emptyText` and never `noMatchText` — a page
   * using this must pass the "nothing matched" copy as `emptyText`, because
   * the component can no longer tell the two apart. `noMatchText` is only
   * meaningful while this box is the thing doing the filtering.
   *
   * Opt-in: omitted, the box renders exactly as it always has.
   */
  hideSearch?: boolean;
}) {
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<SortState<string> | undefined>(initialSort);
  const [page, setPage] = useState(0);

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

  const onSort = (key: string) => {
    setPage(0);
    setSort((s) =>
      s?.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" },
    );
  };

  // Pagination is opt-in via `pageSize`. `clampedPage` keeps us on a valid page
  // even when the filtered set shrinks under us (e.g. an external filter
  // narrows `rows` while we're on a later page).
  const size = pageSize != null && pageSize > 0 ? pageSize : 0;
  const paginated = size > 0;
  const pageCount = paginated ? Math.max(1, Math.ceil(shown.length / size)) : 1;
  const clampedPage = Math.min(page, pageCount - 1);
  const pageRows = paginated
    ? shown.slice(clampedPage * size, clampedPage * size + size)
    : shown;
  const rangeStart = shown.length === 0 ? 0 : clampedPage * size + 1;
  const rangeEnd = paginated ? clampedPage * size + pageRows.length : shown.length;

  return (
    <div className="space-y-3">
      {(!hideSearch || filters) && (
        <div className="flex flex-wrap items-center gap-2">
          {!hideSearch && (
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder={searchPlaceholder}
                value={q}
                onChange={(e) => {
                  setPage(0);
                  setQ(e.target.value);
                }}
              />
            </div>
          )}
          {filters}
        </div>
      )}
      <div className={cn("overflow-x-auto", bordered && "rounded-md border")}>
        <Table>
          <TableHeader>
            <TableRow>
              {selection && (() => {
                const shownIds = shown.map(getId);
                const allShownSelected =
                  shownIds.length > 0 && shownIds.every((id) => selection.selected.has(id));
                return (
                  <TableHead className="w-8">
                    <Checkbox
                      aria-label="Select all"
                      checked={allShownSelected}
                      onCheckedChange={() => {
                        const target = !allShownSelected;
                        for (const id of shownIds) {
                          if (selection.selected.has(id) !== target) selection.onToggle(id);
                        }
                      }}
                    />
                  </TableHead>
                );
              })()}
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
                  className="py-10 text-center text-muted-foreground"
                >
                  {q.trim() ? noMatchText : emptyText}
                </TableCell>
              </TableRow>
            ) : (
              pageRows.map((row) => (
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
      {paginated && shown.length > size && (
        <div className="flex items-center justify-between gap-2 text-sm text-muted-foreground">
          <span>
            Showing <span className="tabular-nums">{rangeStart}</span>–
            <span className="tabular-nums">{rangeEnd}</span> of{" "}
            <span className="tabular-nums">{shown.length}</span>
          </span>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage((p) => Math.max(0, p - 1))}
              disabled={clampedPage === 0}
            >
              <ChevronLeft className="h-4 w-4" />
              Previous
            </Button>
            <span className="tabular-nums">
              {clampedPage + 1} / {pageCount}
            </span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
              disabled={clampedPage >= pageCount - 1}
            >
              Next
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
