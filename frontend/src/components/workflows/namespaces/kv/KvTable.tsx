import type { ReactNode } from "react";
import { ArrowDown, ArrowUp, Copy, Eye, Pencil, Trash2 } from "lucide-react";
import type { KvEntryDto } from "@/api/generated/model";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MANAGE_REASON } from "./kvCopy";

export const KV_SORT_COLUMNS = ["key", "type", "description", "updated", "expiration"] as const;
export type KvSort = { column: (typeof KV_SORT_COLUMNS)[number]; dir: "asc" | "desc" };

/** The URL's `?sort=&dir=`; anything else is the default, key ascending. */
export function parseKvSort(sort: string | null, dir: string | null): KvSort {
  const column = (KV_SORT_COLUMNS as readonly string[]).includes(sort ?? "") ? (sort as KvSort["column"]) : "key";
  return { column, dir: dir === "desc" ? "desc" : "asc" };
}

/**
 * What each column sorts by. A key that never expires sorts after every date
 * (ascending: the soonest expiry first); a key with no description after
 * every description. Ties fall back to the key, so the order is total.
 */
function sortValue(row: KvEntryDto, column: KvSort["column"]): string | number | null {
  switch (column) {
    case "key":
      return row.key;
    case "type":
      return row.type;
    case "description":
      return row.description || null;
    case "updated":
      return Date.parse(row.updated);
    case "expiration":
      return row.expiresAt ? Date.parse(row.expiresAt) : Number.POSITIVE_INFINITY;
  }
}

export function sortKvRows(rows: KvEntryDto[], sort: KvSort): KvEntryDto[] {
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const av = sortValue(a, sort.column);
    const bv = sortValue(b, sort.column);
    const cmp =
      av === null || bv === null
        ? Number(av === null) - Number(bv === null)
        : typeof av === "number" && typeof bv === "number"
          ? av === bv
            ? 0
            : av < bv
              ? -1
              : 1
          : String(av).localeCompare(String(bv));
    return cmp !== 0 ? sign * cmp : a.key.localeCompare(b.key);
  });
}

const LABELS: Record<KvSort["column"], string> = {
  key: "Key",
  type: "Type",
  description: "Description",
  updated: "Last modified",
  expiration: "Expiration",
};

export const formatInstant = (iso: string) => new Date(iso).toLocaleString();

/**
 * The namespace's keys (W7d spec §4.4): five sortable columns, a selection
 * box per row when the viewer may delete, and each row's actions. A header
 * sorts through a button inside it, so the keyboard reaches it (FE-A11Y-3);
 * the direction is the header's `aria-sort`.
 */
export function KvTable({
  rows,
  sort,
  onSort,
  canManage,
  selected,
  onToggle,
  onToggleAll,
  onView,
  onEdit,
  onCopy,
  onDelete,
}: {
  rows: KvEntryDto[];
  sort: KvSort;
  onSort: (column: KvSort["column"]) => void;
  canManage: boolean;
  selected: Set<string>;
  onToggle: (key: string) => void;
  onToggleAll: () => void;
  onView: (row: KvEntryDto) => void;
  onEdit: (row: KvEntryDto) => void;
  onCopy: (row: KvEntryDto) => void;
  onDelete: (row: KvEntryDto) => void;
}) {
  const allSelected = rows.length > 0 && rows.every((r) => selected.has(r.key));
  return (
    <div className="overflow-x-auto rounded-md border">
      <Table data-testid="kv-table">
        <TableHeader>
          <TableRow>
            {canManage ? (
              <TableHead className="w-8">
                <Checkbox aria-label="Select all" checked={allSelected} onCheckedChange={onToggleAll} />
              </TableHead>
            ) : null}
            {KV_SORT_COLUMNS.map((column) => {
              const active = sort.column === column;
              return (
                <TableHead key={column} aria-sort={active ? (sort.dir === "asc" ? "ascending" : "descending") : undefined}>
                  <button
                    type="button"
                    onClick={() => onSort(column)}
                    className="inline-flex items-center gap-1 rounded-sm hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    {LABELS[column]}
                    {active ? (
                      sort.dir === "asc" ? (
                        <ArrowUp aria-hidden className="size-3" />
                      ) : (
                        <ArrowDown aria-hidden className="size-3" />
                      )
                    ) : null}
                  </button>
                </TableHead>
              );
            })}
            <TableHead className="w-10">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={row.key} data-key={row.key}>
              {canManage ? (
                <TableCell className="w-8">
                  <Checkbox aria-label={`Select ${row.key}`} checked={selected.has(row.key)} onCheckedChange={() => onToggle(row.key)} />
                </TableCell>
              ) : null}
              <TableCell className="max-w-xs break-all font-mono text-xs">{row.key}</TableCell>
              <TableCell>
                <Badge variant="outline">{row.type}</Badge>
              </TableCell>
              <TableCell className="max-w-sm break-words text-muted-foreground">{row.description || "—"}</TableCell>
              <TableCell className="whitespace-nowrap tabular-nums">{formatInstant(row.updated)}</TableCell>
              <TableCell className="whitespace-nowrap tabular-nums">{row.expiresAt ? formatInstant(row.expiresAt) : "Never"}</TableCell>
              <TableCell className="w-10">
                <RowActionsMenu label={`Actions for ${row.key}`}>
                  <DropdownMenuItem onSelect={() => onView(row)}>
                    <Eye aria-hidden className="size-4" />
                    View value
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => onCopy(row)}>
                    <Copy aria-hidden className="size-4" />
                    Copy expression
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <WriteItem disabled={!canManage} onSelect={() => onEdit(row)} icon={<Pencil aria-hidden className="size-4" />}>
                    Edit
                  </WriteItem>
                  <WriteItem disabled={!canManage} onSelect={() => onDelete(row)} icon={<Trash2 aria-hidden className="size-4" />}>
                    Delete
                  </WriteItem>
                </RowActionsMenu>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

/** A write in a row's menu; disabled, it says why under its name (FE-ACTION-4). */
function WriteItem({
  disabled,
  onSelect,
  icon,
  children,
}: {
  disabled: boolean;
  onSelect: () => void;
  icon: ReactNode;
  children: string;
}) {
  return (
    <DropdownMenuItem disabled={disabled} onSelect={onSelect} className="items-start">
      {icon}
      <span className="flex flex-col">
        <span>{children}</span>
        {disabled ? <span className="text-xs text-muted-foreground">{MANAGE_REASON}</span> : null}
      </span>
    </DropdownMenuItem>
  );
}
