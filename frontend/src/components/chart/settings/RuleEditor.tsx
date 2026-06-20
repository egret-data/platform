import { useState } from "react";
import { isNumericType, type Rule, type SingleColorRule, type ColorRangeRule, type FormattingOperator } from "../format-types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

const NUMERIC_OPS: FormattingOperator[] = ["=", "!=", "<", ">", "<=", ">="];
const STRING_OPS: FormattingOperator[] = ["=", "!=", "contains", "does-not-contain", "starts-with", "ends-with"];
const NULL_OPS: FormattingOperator[] = ["is-null", "not-null"];
const NO_VALUE_OPS = new Set<FormattingOperator>(["is-null", "not-null", "is-true", "is-false"]);
const PALETTE = ["#ef4444", "#f59e0b", "#22c55e", "#3b82f6", "#a855f7"];
const RANGE = ["#ffffff", "#3b82f6"];

// preset two-stop gradients for the color-range (heatmap) rule
const RANGE_PRESETS: { label: string; colors: [string, string] }[] = [
  { label: "White to blue", colors: ["#ffffff", "#3b82f6"] },
  { label: "White to green", colors: ["#ffffff", "#22c55e"] },
  { label: "White to red", colors: ["#ffffff", "#ef4444"] },
  { label: "Red to green", colors: ["#ef4444", "#22c55e"] },
  { label: "Blue to red", colors: ["#3b82f6", "#ef4444"] },
];

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

  const range = draft as ColorRangeRule;
  const start = range.type === "range" ? range.colors[0] : RANGE[0];
  const end = range.type === "range" ? range.colors[range.colors.length - 1] : RANGE[1];
  function setRangeColor(idx: 0 | 1, v: string) {
    setDraft((d) => {
      const r = d as ColorRangeRule;
      const colors = [...r.colors];
      colors[idx === 0 ? 0 : colors.length - 1] = v;
      return { ...r, colors };
    });
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="text-xs font-medium uppercase text-muted-foreground">Columns</div>
        <div className="flex flex-col gap-1 pt-1">
          {columns.map((c) => (
            <div key={c.name} className="flex items-center gap-2">
              <Checkbox id={`rule-col-${c.name}`} checked={draft.columns.includes(c.name)} onCheckedChange={() => toggleColumn(c.name)} />
              <Label htmlFor={`rule-col-${c.name}`} className="cursor-pointer text-sm font-normal">{c.name}</Label>
            </div>
          ))}
        </div>
      </div>

      {allNumeric && (
        <label className="block space-y-1">
          <span className="text-xs text-muted-foreground">Formatting</span>
          <Select value={draft.type} onValueChange={(v) => setType(v as "single" | "range")}>
            <SelectTrigger aria-label="Rule type" className="h-8"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="single">Single color</SelectItem>
              <SelectItem value="range">Color range</SelectItem>
            </SelectContent>
          </Select>
        </label>
      )}

      {draft.type === "single" ? (
        <div className="space-y-2">
          <label className="block space-y-1">
            <span className="text-xs text-muted-foreground">When a value is</span>
            <Select value={draft.operator} onValueChange={(v) => setDraft((d) => ({ ...(d as SingleColorRule), operator: v as FormattingOperator }))}>
              <SelectTrigger aria-label="Operator" className="h-8"><SelectValue /></SelectTrigger>
              <SelectContent>
                {ops.map((op) => <SelectItem key={op} value={op}>{op}</SelectItem>)}
              </SelectContent>
            </Select>
          </label>
          {!NO_VALUE_OPS.has(draft.operator) && (
            <label className="block space-y-1">
              <span className="text-xs text-muted-foreground">Value</span>
              <Input className="h-8" value={String((draft as SingleColorRule).value ?? "")}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), value: e.target.value }))} />
            </label>
          )}
          <div className="space-y-1">
            <span className="text-xs text-muted-foreground">Color</span>
            <div className="flex gap-1 pt-0.5">
              {PALETTE.map((c) => (
                <button key={c} type="button" aria-label={`color ${c}`} onClick={() => setDraft((d) => ({ ...(d as SingleColorRule), color: c }))}
                  className={`h-6 w-6 rounded ${(draft as SingleColorRule).color === c ? "ring-2 ring-primary ring-offset-1" : ""}`} style={{ backgroundColor: c }} />
              ))}
            </div>
          </div>
          {!isPivot && (
            <div className="flex items-center justify-between pt-1">
              <Label htmlFor="rule-highlight-row" className="text-sm font-normal">Highlight whole row</Label>
              <Switch id="rule-highlight-row" checked={!!(draft as SingleColorRule).highlight_row}
                onCheckedChange={(v) => setDraft((d) => ({ ...(d as SingleColorRule), highlight_row: v }))} />
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-2">
          <span className="text-xs text-muted-foreground">Color range</span>
          {/* preset gradients */}
          <div className="flex flex-wrap gap-1.5 pt-0.5">
            {RANGE_PRESETS.map((p) => {
              const active = start.toLowerCase() === p.colors[0] && end.toLowerCase() === p.colors[1];
              return (
                <button key={p.label} type="button" aria-label={p.label} title={p.label}
                  onClick={() => setDraft((d) => ({ ...(d as ColorRangeRule), colors: [...p.colors] }))}
                  className={`h-6 w-14 rounded border ${active ? "ring-2 ring-primary ring-offset-1" : ""}`}
                  style={{ background: `linear-gradient(to right, ${p.colors[0]}, ${p.colors[1]})` }} />
              );
            })}
          </div>
          {/* live preview + custom endpoints */}
          <div className="space-y-1.5 pt-1">
            <div className="h-2.5 w-full rounded border" style={{ background: `linear-gradient(to right, ${start}, ${end})` }} />
            <div className="flex items-center gap-2">
              <input type="color" aria-label="start color" className="h-7 w-9 cursor-pointer rounded border bg-transparent p-0.5"
                value={start} onChange={(e) => setRangeColor(0, e.target.value)} />
              <span className="flex-1 text-center text-xs text-muted-foreground">low → high</span>
              <input type="color" aria-label="end color" className="h-7 w-9 cursor-pointer rounded border bg-transparent p-0.5"
                value={end} onChange={(e) => setRangeColor(1, e.target.value)} />
            </div>
          </div>
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <Button type="button" variant="outline" size="sm" onClick={onCancel}>Cancel</Button>
        <Button type="button" size="sm" disabled={draft.columns.length === 0} onClick={() => onSave(draft)}>Save rule</Button>
      </div>
    </div>
  );
}
