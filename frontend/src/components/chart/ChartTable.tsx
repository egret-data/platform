import { useMemo, useState } from "react";
import {
  flexRender, getCoreRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type SortingState,
} from "@tanstack/react-table";
import { formatValue } from "./format";
import { buildConditional } from "./conditional";
import type { ColumnFormat, Rule } from "./format-types";

export interface ResultColumn { name: string; type: string }

const NUMERIC = /int|double|decimal|real|numeric|float|bigint/i;

export function ChartTable({
  columns, rows, columnSettings = {}, conditionalFormatting = [],
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
}) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const colIndexByName = useMemo(
    () => Object.fromEntries(columns.map((c, i) => [c.name, i])),
    [columns],
  );
  const cond = useMemo(
    () => buildConditional(rows, colIndexByName, conditionalFormatting),
    [rows, colIndexByName, conditionalFormatting],
  );

  const cols = useMemo<ColumnDef<unknown[]>[]>(
    () =>
      columns.map((c, i) => {
        const isNum = NUMERIC.test(c.type);
        return {
          id: `col_${i}`,
          accessorFn: (row) => {
            const v = row[i];
            if (isNum && typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v))) {
              return Number(v);
            }
            return v;
          },
          header: c.name,
          cell: (ctx) => formatValue(ctx.getValue(), c.type, columnSettings[c.name]),
          enableSorting: true,
          meta: { isNum, name: c.name },
        };
      }),
    [columns, columnSettings],
  );

  const table = useReactTable({
    data: rows,
    columns: cols,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  return (
    <div className="overflow-auto rounded-md border">
      <table className="w-full border-collapse text-sm">
        <thead className="bg-muted/40">
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id} className="border-b border-border/60 text-left text-muted-foreground">
              {hg.headers.map((h) => {
                const isNum = (h.column.columnDef.meta as { isNum?: boolean } | undefined)?.isNum ?? false;
                return (
                  <th
                    key={h.id}
                    onClick={h.column.getToggleSortingHandler()}
                    className={`cursor-pointer select-none py-2 px-3 font-medium whitespace-nowrap ${isNum ? "text-right" : "text-left"}`}
                  >
                    {flexRender(h.column.columnDef.header, h.getContext())}
                    {{ asc: " ▲", desc: " ▼" }[h.column.getIsSorted() as string] ?? ""}
                  </th>
                );
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((r) => {
            const rowColor = cond.rowBg(r.index);
            return (
              <tr key={r.id} className="border-t border-border/60 hover:bg-muted/20" style={rowColor ? { backgroundColor: rowColor } : undefined}>
                {r.getVisibleCells().map((cell) => {
                  const meta = cell.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                  const isNum = meta?.isNum ?? false;
                  const bg = cond.bg(cell.getValue(), meta?.name ?? cell.column.id);
                  return (
                    <td
                      key={cell.id}
                      className={`py-2 px-3 ${isNum ? "text-right tabular-nums" : ""}`}
                      style={bg ? { backgroundColor: bg } : undefined}
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
