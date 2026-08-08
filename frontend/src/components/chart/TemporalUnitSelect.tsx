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
}: {
  value: string | undefined;
  onChange: (token: string | undefined) => void;
  /** Accessible name. The visible text beside this control names the COLUMN ("Group created_at
   *  by"), and a bare select next to it announced only its current value — so with two date
   *  dimensions a screen-reader user heard "Month" twice with nothing to tell them apart. */
  label?: string;
}) {
  const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc");
  const extract = TEMPORAL_UNITS.filter((u) => u.group === "extract");
  return (
    <Select value={value ?? NONE} onValueChange={(v) => onChange(v === NONE ? undefined : v)}>
      {/* `w-28`, not the shadcn default `w-fit`: this sits in an inline row and its width otherwise
          tracked the chosen unit's text, so the row visibly reflowed between "Day" and "Quarter of
          year" — and two of these under one zone never lined up with each other. */}
      <SelectTrigger aria-label={label} className="h-7 w-28 text-xs">
        <SelectValue placeholder="None" />
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
