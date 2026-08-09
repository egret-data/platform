import type { DashboardParameter } from "@/api/dashboards";
import { Switch } from "@/components/ui/switch";
import { FilterPill } from "./FilterPill";
import { FilterApplyBar } from "./FilterApplyBar";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;
  /** Auto-apply defaults to ON, which is exactly today's behaviour — so a caller that has not been
   *  updated yet behaves as it always did. */
  autoApply?: boolean;
  onAutoApplyChange?: (v: boolean) => void;
  changedCount?: number;
  onApply?: () => void;
  onCancel?: () => void;
}

function blankValue(v: unknown): boolean {
  if (Array.isArray(v)) return v.filter((x) => !isBlank(x)).length === 0;
  return isBlank(v);
}

export function DashboardFilterBar({
  parameters, values, onChange, optionsBySlug,
  autoApply = true, onAutoApplyChange, changedCount = 0, onApply, onCancel,
}: Props) {
  if (parameters.length === 0) return null;
  const missingRequired = parameters.filter((p) => p.required && blankValue(values[p.slug]));
  return (
    <div className="mb-3 flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        {parameters.map((p) => (
          <FilterPill key={p.id || p.slug} param={p} value={values[p.slug]} options={optionsBySlug[p.slug] ?? []} onChange={(v) => onChange(p.slug, v)} />
        ))}
        {onAutoApplyChange && (
          <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground"
            title="Saved in this browser, for this dashboard">
            <span>Auto-apply filters</span>
            <Switch aria-label="auto-apply filters" checked={autoApply}
              onCheckedChange={(v) => onAutoApplyChange(v === true)} />
          </label>
        )}
      </div>
      <FilterApplyBar autoApply={autoApply} changedCount={changedCount}
        onApply={onApply ?? (() => {})} onCancel={onCancel ?? (() => {})} />
      {missingRequired.length > 0 && (
        <p className="text-xs text-muted-foreground">Set the required filter(s): {missingRequired.map((p) => p.name).join(", ")}</p>
      )}
    </div>
  );
}
