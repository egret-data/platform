import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { TEMPORAL_UNITS } from "./temporal-units";

const NONE = "__none";

/** Picks a temporal grouping unit for one temporal dimension. `undefined` value = no grouping (raw). */
export function TemporalUnitSelect({
  value,
  onChange,
}: {
  value: string | undefined;
  onChange: (token: string | undefined) => void;
}) {
  const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc");
  const extract = TEMPORAL_UNITS.filter((u) => u.group === "extract");
  return (
    <Select value={value ?? NONE} onValueChange={(v) => onChange(v === NONE ? undefined : v)}>
      <SelectTrigger className="h-7 text-xs">
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
