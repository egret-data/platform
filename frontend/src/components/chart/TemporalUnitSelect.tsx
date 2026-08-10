import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { TEMPORAL_UNITS } from "./temporal-units";

const NONE = "__none";

/** Picks a temporal grouping unit for one temporal dimension. `undefined` value = no grouping (raw). */
export function TemporalUnitSelect({
  value,
  onChange,
  label,
  prefix,
}: {
  value: string | undefined;
  onChange: (token: string | undefined) => void;
  /** Accessible name. The trigger now shows `column: Unit`, but the column name is decoration to a
   *  screen reader — it still needs to hear which column this select governs, and with two date
   *  dimensions a bare "Month" twice told a user nothing. */
  label?: string;
  /** The column this buckets, rendered on the trigger as `order_month: Month`. Metabase names a
   *  bucketed column exactly this way; it replaces a separate "Group <column> by" row that
   *  repeated a name shown directly above it. */
  prefix?: string;
}) {
  const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc");
  const extract = TEMPORAL_UNITS.filter((u) => u.group === "extract");
  return (
    <Select value={value ?? NONE} onValueChange={(v) => onChange(v === NONE ? undefined : v)}>
      {/* Full width, like every other control in the panel. It used to be `w-28` (112px) sitting
          beside a 255px field picker — the single most visible width mismatch in the whole panel,
          and it was pinned here to stop the row reflowing between "Day" and "Quarter of year".
          Taking the panel's width solves the reflow too, and without inventing a second width. */}
      <SelectTrigger aria-label={label} className="w-full">
        {prefix ? (
          <span className="flex min-w-0 items-baseline gap-1">
            <span className="truncate text-muted-foreground">{prefix}:</span>
            <SelectValue placeholder="None" />
          </span>
        ) : (
          <SelectValue placeholder="None" />
        )}
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={NONE}>None</SelectItem>
        <SelectGroup>
          <SelectLabel>Group by</SelectLabel>
          {trunc.map((u) => (
            <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
          ))}
        </SelectGroup>
        <SelectGroup>
          <SelectLabel>Extract</SelectLabel>
          {extract.map((u) => (
            <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}
