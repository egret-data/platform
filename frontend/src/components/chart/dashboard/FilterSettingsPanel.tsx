import type { DashboardParameter, ParamType, ParamOperator } from "@/api/dashboards";
import { OPERATORS } from "./ParameterEditor";

interface Props { param: DashboardParameter; onChange: (next: DashboardParameter) => void; onDelete: () => void }

export function FilterSettingsPanel({ param, onChange, onDelete }: Props) {
  const cls = "h-8 w-full rounded border bg-background px-2 text-sm";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">Filter settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Name
        <input aria-label="parameter name" className={cls} value={param.name} onChange={(e) => onChange({ ...param, name: e.target.value })} />
      </label>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Type
        <select aria-label="parameter type" className={cls} value={param.type}
          onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
          {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
      </label>
      {showOp && (
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">Operator
          <select aria-label="parameter operator" className={cls} value={param.operator}
            onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
            {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        </label>
      )}
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" aria-label="parameter required" checked={!!param.required} onChange={(e) => onChange({ ...param, required: e.target.checked })} /> Required
      </label>
      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
