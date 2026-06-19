import { useState } from "react";
import { isNumericType, type Rule, type SingleColorRule, type ColorRangeRule, type FormattingOperator } from "../format-types";

interface ResultColumn { name: string; type: string }

const NUMERIC_OPS: FormattingOperator[] = ["=", "!=", "<", ">", "<=", ">="];
const STRING_OPS: FormattingOperator[] = ["=", "!=", "contains", "does-not-contain", "starts-with", "ends-with"];
const NULL_OPS: FormattingOperator[] = ["is-null", "not-null"];
const NO_VALUE_OPS = new Set<FormattingOperator>(["is-null", "not-null", "is-true", "is-false"]);
const PALETTE = ["#ef4444", "#f59e0b", "#22c55e", "#3b82f6", "#a855f7"];
const RANGE = ["#ffffff", "#3b82f6"];

export function RuleEditor({
  columns, rule, isPivot, onSave, onCancel,
}: {
  columns: ResultColumn[];
  rule: Rule;
  isPivot: boolean;
  onSave: (r: Rule) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<Rule>(rule);
  const selected = columns.filter((c) => draft.columns.includes(c.name));
  const allNumeric = selected.length > 0 && selected.every((c) => isNumericType(c.type));
  const ops = allNumeric ? [...NUMERIC_OPS, ...NULL_OPS] : [...STRING_OPS, ...NULL_OPS];

  function toggleColumn(name: string) {
    setDraft((d) => ({ ...d, columns: d.columns.includes(name) ? d.columns.filter((c) => c !== name) : [...d.columns, name] }));
  }
  function setType(type: "single" | "range") {
    if (type === draft.type) return;
    setDraft((d) =>
      type === "single"
        ? ({ columns: d.columns, type: "single", operator: ">", value: "", color: PALETTE[0] } as SingleColorRule)
        : ({ columns: d.columns, type: "range", colors: RANGE, min_type: null, max_type: null } as ColorRangeRule),
    );
  }

  return (
    <div className="space-y-3 text-sm">
      <div>
        <div className="text-xs font-medium uppercase text-muted-foreground">Columns</div>
        <div className="flex flex-col gap-0.5 pt-1">
          {columns.map((c) => (
            <label key={c.name} className="flex items-center gap-2">
              <input type="checkbox" checked={draft.columns.includes(c.name)} onChange={() => toggleColumn(c.name)} /> {c.name}
            </label>
          ))}
        </div>
      </div>

      {allNumeric && (
        <label className="block">
          <span className="text-xs text-muted-foreground">Formatting</span>
          <select aria-label="Rule type" className="mt-0.5 w-full rounded border px-2 py-1" value={draft.type}
            onChange={(e) => setType(e.target.value as "single" | "range")}>
            <option value="single">Single color</option>
            <option value="range">Color range</option>
          </select>
        </label>
      )}

      {draft.type === "single" ? (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">When a value is</span>
            <select aria-label="Operator" className="mt-0.5 w-full rounded border px-2 py-1" value={draft.operator}
              onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), operator: e.target.value as FormattingOperator }))}>
              {ops.map((op) => <option key={op} value={op}>{op}</option>)}
            </select>
          </label>
          {!NO_VALUE_OPS.has(draft.operator) && (
            <label className="block">
              <span className="text-xs text-muted-foreground">Value</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={String((draft as SingleColorRule).value ?? "")}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), value: e.target.value }))} />
            </label>
          )}
          <div>
            <span className="text-xs text-muted-foreground">Color</span>
            <div className="flex gap-1 pt-1">
              {PALETTE.map((c) => (
                <button key={c} type="button" aria-label={`color ${c}`} onClick={() => setDraft((d) => ({ ...(d as SingleColorRule), color: c }))}
                  className={`h-6 w-6 rounded ${(draft as SingleColorRule).color === c ? "ring-2 ring-offset-1" : ""}`} style={{ backgroundColor: c }} />
              ))}
            </div>
          </div>
          {!isPivot && (
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={!!(draft as SingleColorRule).highlight_row}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), highlight_row: e.target.checked }))} /> Highlight whole row
            </label>
          )}
        </div>
      ) : (
        <div className="space-y-2">
          <div>
            <span className="text-xs text-muted-foreground">Colors (start → end)</span>
            <div className="flex gap-2 pt-1">
              <input type="color" aria-label="start color" value={(draft as ColorRangeRule).colors[0]} onChange={(e) => setDraft((d) => { const r = d as ColorRangeRule; const colors = [...r.colors]; colors[0] = e.target.value; return { ...r, colors }; })} />
              <input type="color" aria-label="end color" value={(draft as ColorRangeRule).colors[(draft as ColorRangeRule).colors.length - 1]} onChange={(e) => setDraft((d) => { const r = d as ColorRangeRule; const colors = [...r.colors]; colors[colors.length - 1] = e.target.value; return { ...r, colors }; })} />
            </div>
          </div>
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <button type="button" className="rounded border px-3 py-1" onClick={onCancel}>Cancel</button>
        <button type="button" className="rounded bg-primary px-3 py-1 text-primary-foreground"
          disabled={draft.columns.length === 0}
          onClick={() => onSave(draft)}>Save rule</button>
      </div>
    </div>
  );
}
