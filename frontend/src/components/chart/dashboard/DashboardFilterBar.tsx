import type { DashboardParameter } from "@/api/dashboards";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;
}

const RELATIVE_OPTS = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

export function DashboardFilterBar({ parameters, values, onChange, optionsBySlug }: Props) {
  if (parameters.length === 0) return null;
  return (
    <div className="mb-3 flex flex-wrap items-end gap-3 rounded-md border p-2">
      {parameters.map((p) => (
        <label key={p.id} className="flex flex-col gap-0.5 text-xs text-muted-foreground">
          <span>{p.name}{p.required ? " *" : ""}</span>
          <Widget p={p} value={values[p.slug]} onChange={(v) => onChange(p.slug, v)} options={optionsBySlug[p.slug] ?? []} />
        </label>
      ))}
    </div>
  );
}

function Widget({ p, value, onChange, options }: { p: DashboardParameter; value: unknown; onChange: (v: unknown) => void; options: string[] }) {
  const cls = "h-8 rounded border bg-background px-1 text-sm";
  if (p.type === "category") {
    const sel = Array.isArray(value) ? (value as string[]) : [];
    return (
      <select multiple aria-label={p.name} className={cls} value={sel}
        onChange={(e) => onChange(Array.from(e.target.selectedOptions).map((o) => o.value))}>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </select>
    );
  }
  if (p.type === "id") {
    return options.length
      ? <select aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>{options.map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
      : <input aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }
  if (p.type === "number" && p.operator === "between") {
    const parts = Array.isArray(value) ? (value as (number | null)[]) : [null, null];
    const [min, max] = parts;
    return (
      <span className="flex gap-1">
        <input aria-label={`${p.name} min`} type="number" className={cls + " w-20"} value={min ?? ""} onChange={(e) => onChange([e.target.value === "" ? null : Number(e.target.value), max ?? null])} />
        <input aria-label={`${p.name} max`} type="number" className={cls + " w-20"} value={max ?? ""} onChange={(e) => onChange([min ?? null, e.target.value === "" ? null : Number(e.target.value)])} />
      </span>
    );
  }
  if (p.type === "number") {
    return <input aria-label={p.name} type="number" className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))} />;
  }
  if (p.type === "date" && p.operator === "range") {
    const parts = Array.isArray(value) ? (value as string[]) : ["", ""];
    const [start, end] = parts;
    return (
      <span className="flex gap-1">
        <input aria-label={`${p.name} min`} type="date" className={cls} value={start ?? ""} onChange={(e) => onChange([e.target.value, end ?? ""])} />
        <input aria-label={`${p.name} max`} type="date" className={cls} value={end ?? ""} onChange={(e) => onChange([start ?? "", e.target.value])} />
      </span>
    );
  }
  if (p.type === "date" && p.operator === "relative") {
    return <select aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
      <option value="">—</option>{RELATIVE_OPTS.map((o) => <option key={o.v} value={o.v}>{o.label}</option>)}
    </select>;
  }
  if (p.type === "date") {
    return <input aria-label={p.name} type="date" className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }
  return <input aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
}
