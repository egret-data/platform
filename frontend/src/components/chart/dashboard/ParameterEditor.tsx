import type { DashboardParameter, ParamType, ParamOperator } from "@/api/dashboards";

export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single", "month-year", "quarter-year"],
  id: ["is"],
  category: ["is"],
  boolean: ["is"],
  location: ["is", "is-not"],
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
        onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
        {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
      </select>
      {showOp && (
        <select aria-label="parameter operator" className={cls} value={param.operator}
          onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
          {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
      )}
      <label className="flex items-center gap-1 text-xs">
        <input type="checkbox" checked={!!param.required} aria-label="parameter required"
          onChange={(e) => onChange({ ...param, required: e.target.checked })} /> req
      </label>
      <button type="button" aria-label="delete parameter" className="text-xs text-destructive" onClick={onDelete}>✕</button>
    </div>
  );
}
