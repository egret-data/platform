import { useMemo, useState } from "react";
import {
  flexRender, getCoreRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type SortingState,
} from "@tanstack/react-table";

export interface ResultColumn { name: string; type: string }

const NUMERIC = /int|double|decimal|real|numeric|float|bigint/i;

function formatCell(value: unknown, type: string): string {
  if (value == null) return "";
  if (NUMERIC.test(type)) {
    const n = typeof value === "number" ? value : Number(value);
    if (Number.isFinite(n) && !(typeof value === "string" && value.trim() === "")) {
      return n.toLocaleString();
    }
  }
  return String(value);
}

export function ChartTable({ columns, rows }: { columns: ResultColumn[]; rows: unknown[][] }) {
  const [sorting, setSorting] = useState<SortingState>([]);

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
          cell: (ctx) => formatCell(ctx.getValue(), c.type),
          enableSorting: true,
          meta: { isNum },
        };
      }),
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
                const isNum = (h.column.columnDef.meta as { isNum?: boolean } | undefined)?.isNum ?? false;
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
                const isNum = (cell.column.columnDef.meta as { isNum?: boolean } | undefined)?.isNum ?? false;
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
