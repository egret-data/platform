import { useState, useEffect } from "react";
import { isNumericType, isTemporalType, type ColumnFormat } from "../format-types";

interface ResultColumn { name: string; type: string }

export function FormatSection({
  columns, value, onChange,
}: {
  columns: ResultColumn[];
  value: Record<string, ColumnFormat>;
  onChange: (v: Record<string, ColumnFormat>) => void;
}) {
  const [sel, setSel] = useState<string | null>(columns[0]?.name ?? null);

  useEffect(() => {
    if (!sel || !columns.find((c) => c.name === sel)) setSel(columns[0]?.name ?? null);
  }, [columns, sel]);

  const col = columns.find((c) => c.name === sel);
  const fmt = (sel && value[sel]) || {};

  function patch(p: Partial<ColumnFormat>) {
    if (!sel) return;
    onChange({ ...value, [sel]: { ...fmt, ...p } });
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="text-xs font-medium uppercase text-muted-foreground">Column</div>
        <div className="flex flex-col gap-0.5">
          {columns.map((c) => (
            <button
              key={c.name}
              type="button"
              onClick={() => setSel(c.name)}
              className={`rounded px-2 py-1 text-left ${sel === c.name ? "bg-muted font-medium" : "hover:bg-muted/50"}`}
            >
              {c.name}
            </button>
          ))}
        </div>
      </div>

      {col && isNumericType(col.type) && (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">Number style</span>
            <select aria-label="Number style" className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.number_style ?? "decimal"} onChange={(e) => patch({ number_style: e.target.value as ColumnFormat["number_style"] })}>
              <option value="decimal">Normal</option>
              <option value="percent">Percent</option>
              <option value="currency">Currency</option>
              <option value="scientific">Scientific</option>
            </select>
          </label>
          {fmt.number_style === "currency" && (
            <label className="block">
              <span className="text-xs text-muted-foreground">Currency</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.currency ?? "USD"}
                onChange={(e) => patch({ currency: e.target.value.toUpperCase() })} />
            </label>
          )}
          <label className="block">
            <span className="text-xs text-muted-foreground">Decimals</span>
            <input type="number" min={0} className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.decimals ?? ""} onChange={(e) => patch({ decimals: e.target.value === "" ? undefined : Number(e.target.value) })} />
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.compact} onChange={(e) => patch({ compact: e.target.checked })} /> Compact (1.2K)
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.negative_in_parentheses} onChange={(e) => patch({ negative_in_parentheses: e.target.checked })} /> Negatives in ( )
          </label>
          <div className="flex gap-2">
            <label className="block flex-1"><span className="text-xs text-muted-foreground">Prefix</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} /></label>
            <label className="block flex-1"><span className="text-xs text-muted-foreground">Suffix</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} /></label>
          </div>
        </div>
      )}

      {col && isTemporalType(col.type) && (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">Date style</span>
            <select aria-label="Date style" className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.date_style ?? "MMMM D, YYYY"} onChange={(e) => patch({ date_style: e.target.value })}>
              <option value="MMMM D, YYYY">January 31, 2024</option>
              <option value="M/D/YYYY">1/31/2024</option>
              <option value="D/M/YYYY">31/1/2024</option>
              <option value="YYYY-MM-DD">2024-01-31</option>
            </select>
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.date_abbreviate} onChange={(e) => patch({ date_abbreviate: e.target.checked })} /> Abbreviate
          </label>
        </div>
      )}

      {col && !isNumericType(col.type) && !isTemporalType(col.type) && (
        <p className="text-xs text-muted-foreground">No format options for this column type.</p>
      )}
    </div>
  );
}
