import { useState } from "react";
import type { TableState } from "@/api/charts";
import type { ColumnFormat } from "@/components/chart/format-types";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

const PAGE_SIZES = [25, 50, 100];

/** Below this many columns a search box is clutter rather than help. */
const COLUMN_FILTER_MIN = 12;

export function TableColumnsSection({
  columns, value, onChange, columnSettings,
}: {
  columns: ResultColumn[];
  value: TableState;
  onChange: (next: TableState) => void;
  columnSettings?: Record<string, ColumnFormat>;
}) {
  const [filter, setFilter] = useState("");
  const hidden = new Set(value.hiddenColumns ?? []);
  const pinnedLeft = value.pinnedLeft ?? 0;
  const pagination = value.pagination ?? { enabled: false, pageSize: 25 };

  // What the user sees for a column. Everything besides this label and the filter match below
  // keeps dealing in the raw `c.name` — hiddenColumns, pinnedLeft and setAllHidden all key off
  // the column identity the query actually returns, not its display title.
  const titleFor = (c: ResultColumn) => columnSettings?.[c.name]?.column_title || c.name;

  const q = filter.trim().toLowerCase();
  const visibleColumns = q
    ? columns.filter(
        (c) => titleFor(c).toLowerCase().includes(q) || c.name.toLowerCase().includes(q),
      )
    : columns;

  function toggleHidden(name: string) {
    const next = new Set(hidden);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({ ...value, hiddenColumns: [...next] });
  }

  // Deliberately `columns`, not the filtered list: a filter is a way to FIND a column, not a
  // selection. "Hide all" while a filter is active must not quietly hide only what is on screen.
  const setAllHidden = (hide: boolean) =>
    onChange({ ...value, hiddenColumns: hide ? columns.map((c) => c.name) : [] });

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <div className="text-xs font-medium uppercase text-muted-foreground">Visible columns</div>
          <div className="flex gap-2">
            <button type="button" className="text-xs text-muted-foreground hover:text-foreground hover:underline"
              onClick={() => setAllHidden(false)}>
              Show all
            </button>
            <button type="button" className="text-xs text-muted-foreground hover:text-foreground hover:underline"
              onClick={() => setAllHidden(true)}>
              Hide all
            </button>
          </div>
        </div>
        {columns.length >= COLUMN_FILTER_MIN && (
          <Input
            aria-label="search columns" placeholder="Search columns…" value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="h-8 text-sm"
          />
        )}
        <div className="flex flex-col gap-1">
          {visibleColumns.map((c) => (
            <div key={c.name} className="flex items-center gap-2">
              <Checkbox id={`tcol-${c.name}`} checked={!hidden.has(c.name)} onCheckedChange={() => toggleHidden(c.name)} />
              <Label htmlFor={`tcol-${c.name}`} className="cursor-pointer text-sm font-normal">{titleFor(c)}</Label>
            </div>
          ))}
        </div>
      </div>

      <div className="space-y-1">
        <Label htmlFor="freeze-left" className="text-xs font-medium uppercase text-muted-foreground">Freeze first columns</Label>
        <Input
          id="freeze-left" type="number" min={0} max={columns.length} value={pinnedLeft}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ ...value, pinnedLeft: Number.isNaN(n) ? 0 : Math.max(0, Math.min(n, columns.length)) });
          }}
          className="h-8 text-sm"
        />
      </div>

      <div className="space-y-2">
        <div className="text-xs font-medium uppercase text-muted-foreground">Pagination</div>
        <div className="flex items-center justify-between">
          <Label htmlFor="paginate" className="text-sm font-normal">Paginate</Label>
          <Switch id="paginate" checked={pagination.enabled}
            onCheckedChange={(v) => onChange({ ...value, pagination: { ...pagination, enabled: v } })} />
        </div>
        {pagination.enabled && (
          <div className="space-y-1">
            <Label className="text-xs font-medium uppercase text-muted-foreground">Page size</Label>
            <Select value={String(pagination.pageSize)} onValueChange={(v) => onChange({ ...value, pagination: { ...pagination, pageSize: Number(v) } })}>
              <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
              <SelectContent>
                {PAGE_SIZES.map((s) => <SelectItem key={s} value={String(s)}>{s}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>
    </div>
  );
}
