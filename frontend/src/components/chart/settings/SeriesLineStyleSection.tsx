import type { LineSeriesStyle } from "@/api/charts";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

/**
 * Per-series line dash style and width, one row per series name. Shape and markers render
 * correctly per-series too (see decorateSeries in echarts/options/cartesian.ts) but stay
 * chart-wide-only here — a third and fourth Select per row would make an already-unbounded
 * list (one row per series) worse, so this control is declared `advanced: true` in
 * viz/cartesian.tsx and hides behind the group's "More options" disclosure.
 */
export function SeriesLineStyleSection({ names, seriesLine, onChange }: {
  names: string[];
  seriesLine?: Record<string, LineSeriesStyle>;
  onChange: (patch: { seriesLine?: Record<string, LineSeriesStyle> }) => void;
}) {
  // Merge a single field into one series' entry without disturbing the others — seriesLine
  // is a map keyed by series name, so both spreads matter: the outer one keeps every other
  // series' entry, the inner one keeps this series' other field (style vs size).
  const patch = (name: string, field: "style" | "size", value: string | undefined) =>
    onChange({ seriesLine: { ...seriesLine, [name]: { ...seriesLine?.[name], [field]: value } } });
  return (
    <div className="space-y-1.5">
      {names.map((name) => (
        <div key={name} className="flex items-center gap-2">
          <Label className="min-w-0 flex-1 truncate text-sm font-normal" title={name}>{name}</Label>
          <Select
            value={seriesLine?.[name]?.style ?? "auto"}
            onValueChange={(v) => patch(name, "style", v === "auto" ? undefined : v)}
          >
            <SelectTrigger aria-label={`style ${name}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">Auto</SelectItem>
              <SelectItem value="solid">Solid</SelectItem>
              <SelectItem value="dashed">Dashed</SelectItem>
              <SelectItem value="dotted">Dotted</SelectItem>
            </SelectContent>
          </Select>
          <Select
            value={seriesLine?.[name]?.size ?? "auto"}
            onValueChange={(v) => patch(name, "size", v === "auto" ? undefined : v)}
          >
            <SelectTrigger aria-label={`size ${name}`} className="h-7 w-20"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">Auto</SelectItem>
              <SelectItem value="S">S</SelectItem>
              <SelectItem value="M">M</SelectItem>
              <SelectItem value="L">L</SelectItem>
            </SelectContent>
          </Select>
        </div>
      ))}
    </div>
  );
}
