import { Checkbox } from "@/components/ui/checkbox";
import type { DashboardParameter, ParamType, ParamOperator } from "@/api/dashboards";
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";

export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single", "month-year", "quarter-year", "all-options"],
  id: ["is"],
  category: ["is"],
  boolean: ["is"],
  location: ["is", "is-not"],
  temporal_unit: ["is"],
};

export function ParameterEditor({ param, onChange, onDelete }: {
  param: DashboardParameter;
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}) {
  const cls = "h-7 rounded border bg-background px-1 text-xs";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  return (
    <div className="flex items-center gap-1">
      <input aria-label="parameter name" className={cls} value={param.name}
        onChange={(e) => onChange({ ...param, name: e.target.value })} />
      <select aria-label="parameter type" className={cls} value={param.type}
        onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0], ...(t === "temporal_unit" ? { default: "month" } : {}) }); }}>
        {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
      </select>
      {showOp && (
        <select aria-label="parameter operator" className={cls} value={param.operator}
          onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
          {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
      )}
      {param.type === "temporal_unit" && (
        <select aria-label="default unit" className={cls} value={String(param.default ?? "month")}
          onChange={(e) => onChange({ ...param, default: e.target.value })}>
          {TEMPORAL_UNITS.map((u) => <option key={u.token} value={u.token}>{u.label}</option>)}
        </select>
      )}
      {/* CONVERTED, BUT NOT MEASURED, AND THE REASON IS WORTH RECORDING: nothing renders this
          component. A sweep of `src` finds `ParameterEditor` imported only by its own test file;
          the one live import from this module is the `OPERATORS` constant, which
          `FilterSettingsPanel` — the editor that actually ships — pulls out of it. So there is no
          screen on which to measure this row's density, and any number claimed here would be
          arithmetic dressed up as a measurement.
          It is still converted rather than skipped. The component is exported, its tests pass, and
          the day something mounts it, a raw `<input type="checkbox">` here would arrive with the
          platform's control and a 16×16 target — which is precisely the state this change exists to
          end. Whoever mounts it should measure the row then: `gap-1` beside `h-7` selects puts the
          24px target's edge on the select's, and 4px more of row height would separate them. */}
      <label htmlFor={`pe-required-${param.id}`} className="flex cursor-pointer items-center gap-1 text-xs">
        <Checkbox id={`pe-required-${param.id}`} checked={!!param.required} aria-label="parameter required"
          onCheckedChange={(v) => onChange({ ...param, required: v === true })} /> req
      </label>
      <button type="button" aria-label="delete parameter" className="text-xs text-destructive" onClick={onDelete}>✕</button>
    </div>
  );
}
