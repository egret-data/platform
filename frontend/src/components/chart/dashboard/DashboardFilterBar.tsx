import type { DashboardParameter } from "@/api/dashboards";
import { FilterPill } from "./FilterPill";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;
}

function blankValue(v: unknown): boolean {
  if (Array.isArray(v)) return v.filter((x) => !isBlank(x)).length === 0;
  return isBlank(v);
}

export function DashboardFilterBar({ parameters, values, onChange, optionsBySlug }: Props) {
  if (parameters.length === 0) return null;
  const missingRequired = parameters.filter((p) => p.required && blankValue(values[p.slug]));
  return (
    <div className="mb-3 flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        {parameters.map((p) => (
          <FilterPill key={p.id || p.slug} param={p} value={values[p.slug]} options={optionsBySlug[p.slug] ?? []} onChange={(v) => onChange(p.slug, v)} />
        ))}
      </div>
      {missingRequired.length > 0 && (
        <p className="text-xs text-muted-foreground">Set the required filter(s): {missingRequired.map((p) => p.name).join(", ")}</p>
      )}
    </div>
  );
}
