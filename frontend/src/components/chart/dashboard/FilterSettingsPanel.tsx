import type { DashboardParameter, ParamType, ParamOperator, ParamWidget } from "@/api/dashboards";
import { OPERATORS } from "./ParameterEditor";

interface Props {
  param: DashboardParameter;
  allParameters: DashboardParameter[];
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}

const WIDGETS: { v: ParamWidget; label: string }[] = [
  { v: "dropdown", label: "Dropdown" },
  { v: "search", label: "Search box" },
  { v: "input", label: "Input box" },
];

export function FilterSettingsPanel({ param, allParameters, onChange, onDelete }: Props) {
  const cls = "h-8 w-full rounded border bg-background px-2 text-sm";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  const showWidget = param.type === "category" || param.type === "id" || param.type === "location" || param.type === "text";
  const others = allParameters.filter((o) => o.id !== param.id);
  const linked = param.filteringParameters ?? [];
  const toggleLink = (id: string) =>
    onChange({ ...param, filteringParameters: linked.includes(id) ? linked.filter((x) => x !== id) : [...linked, id] });

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

      {showWidget && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">How should people filter on this?</div>
          {WIDGETS.map((w) => (
            <label key={w.v} className="flex items-center gap-2 text-sm">
              <input type="radio" name={`widget-${param.id}`} aria-label={`widget ${w.v}`}
                checked={(param.widget ?? "dropdown") === w.v}
                onChange={() => onChange({ ...param, widget: w.v })} /> {w.label}
            </label>
          ))}
        </div>
      )}

      {/* MVP: default is entered as plain text; multi-value/range defaults aren't round-tripped. */}
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Default value
        <input aria-label="default value" className={cls}
          value={Array.isArray(param.default) ? (param.default as unknown[]).join(", ") : String(param.default ?? "")}
          onChange={(e) => onChange({ ...param, default: e.target.value === "" ? undefined : e.target.value })} />
      </label>

      {others.length > 0 && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">Linked filters</div>
          <p className="text-xs text-muted-foreground/70">Limit this filter's choices to values that match the selected filters.</p>
          {others.map((o) => (
            <label key={o.id} className="flex items-center gap-2 text-sm">
              <input type="checkbox" aria-label={`link ${o.name}`} checked={linked.includes(o.id)} onChange={() => toggleLink(o.id)} /> {o.name}
            </label>
          ))}
        </div>
      )}

      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" aria-label="parameter required" checked={!!param.required} onChange={(e) => onChange({ ...param, required: e.target.checked })} /> Required
      </label>
      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
