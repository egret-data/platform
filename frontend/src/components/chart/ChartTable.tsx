import { useMemo, useState } from "react";
import {
  flexRender, getCoreRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type SortingState,
} from "@tanstack/react-table";

export interface ResultColumn { name: string; type: string }

const NUMERIC = /int|double|decimal|real|numeric|float|bigint/i;

function formatCell(value: unknown, type: string): string {
  if (value == null) return "";
  if (NUMERIC.test(type) && typeof value === "number") return value.toLocaleString();
  return String(value);
}

export function ChartTable({ columns, rows }: { columns: ResultColumn[]; rows: unknown[][] }) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const cols = useMemo<ColumnDef<unknown[]>[]>(
    () =>
      columns.map((c, i) => ({
        id: c.name,
        accessorFn: (row) => row[i],
        header: c.name,
        cell: (ctx) => formatCell(ctx.getValue(), c.type),
        enableSorting: true,
      })),
    [columns],
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
    <div className="overflow-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id} className="border-b text-left text-muted-foreground">
              {hg.headers.map((h) => {
                const isNum = NUMERIC.test(columns.find((c) => c.name === h.column.id)?.type ?? "");
                return (
                  <th
                    key={h.id}
                    onClick={h.column.getToggleSortingHandler()}
                    className={`cursor-pointer select-none py-1 px-3 ${isNum ? "text-right" : "text-left"}`}
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
          {table.getRowModel().rows.map((r) => (
            <tr key={r.id} className="border-t">
              {r.getVisibleCells().map((cell) => {
                const isNum = NUMERIC.test(columns.find((c) => c.name === cell.column.id)?.type ?? "");
                return (
                  <td key={cell.id} className={`py-1 px-3 ${isNum ? "text-right tabular-nums" : ""}`}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
