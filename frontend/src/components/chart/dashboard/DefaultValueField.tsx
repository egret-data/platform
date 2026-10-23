import type { DashboardParameter } from "@/api/dashboards";
import { FIELD_LABEL } from "@/components/chart/settings/FieldLabel";
import { FilterControl } from "./FilterPill";

interface Props {
  param: DashboardParameter;
  /** The parameter's option list, when it has one — the same `optionsBySlug` entry the filter bar
   *  is given, so a category default is chosen from the real values rather than typed. */
  options: string[];
  onChange: (next: unknown) => void;
}

/** Nothing the URL can hold. `useDashboardParams` only seeds `values` when `default !== undefined`,
 *  so an emptied default must collapse to `undefined` — "" would be a default that exists and is
 *  blank, which the required-filter notice treats differently. */
function isEmptyDefault(v: unknown): boolean {
  return v === undefined || v === null || v === "" || (Array.isArray(v) && v.filter((x) => x !== "" && x != null).length === 0);
}

/** The default value, picked with the parameter's own widget: a date with the date picker, a
 *  category from its value list, a number with a number input. Replaces the plain text box that
 *  stored every default as a string and could not round-trip a multi-value or range default. */
export function DefaultValueField({ param, options, onChange }: Props) {
  const hasDefault = !isEmptyDefault(param.default);
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className={FIELD_LABEL}>Default value</span>
        {hasDefault && (
          <button type="button" aria-label="clear default value"
            className="text-xs text-muted-foreground hover:text-foreground"
            onClick={() => onChange(undefined)}>Clear</button>
        )}
      </div>
      <FilterControl param={param} value={param.default} options={options}
        onChange={(v) => onChange(isEmptyDefault(v) ? undefined : v)} />
    </div>
  );
}
