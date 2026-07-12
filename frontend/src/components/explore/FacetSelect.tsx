import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** Sentinel for "no filter". Radix Select values must be non-empty. */
export const FACET_ALL = "__all__";

export interface FacetOption {
  value: string;
  label: string;
}

/**
 * A labeled dropdown for one table facet. Prepends an "All" option
 * (value `FACET_ALL`). The parent owns the value + does the row filtering.
 */
export function FacetSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: FacetOption[];
}) {
  return (
    <Select value={value || FACET_ALL} onValueChange={onChange}>
      <SelectTrigger className="h-9 w-auto min-w-32" aria-label={label}>
        <span className="text-muted-foreground">{label}:</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={FACET_ALL}>All</SelectItem>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
