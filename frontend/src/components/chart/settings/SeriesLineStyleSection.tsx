import type { LineSeriesStyle } from "@/api/charts";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

/**
 * Per-series line appearance, one row per series name — all four of `LineSeriesStyle`.
 *
 * `shape` and `markers` used to be missing here, and the omission was ARGUED rather than
 * accidental: a third and fourth Select per row would make an already-unbounded list (one row
 * per series) worse, so the control was declared `advanced: true` and hidden behind "More
 * options". The trouble is that `decorateSeries` (echarts/options/cartesian.ts) has always
 * honoured all four, so a chart could draw a per-series step line that nobody could ask for —
 * the same shape of defect FormatSection.tsx names for `scale` and `number_separators`.
 * The drill-in (SeriesSettingsPopover) removes the premise instead of overriding the
 * conclusion: this section now renders ONE series at a time inside a popover, so four Selects
 * are four rows in a panel that opens on demand rather than 4N Selects always on screen.
 */
export function SeriesLineStyleSection({ names, seriesLine, onChange }: {
  names: string[];
  seriesLine?: Record<string, LineSeriesStyle>;
  onChange: (patch: { seriesLine?: Record<string, LineSeriesStyle> }) => void;
}) {
  // Merge a single field into one series' entry without disturbing the others — seriesLine
  // is a map keyed by series name, so both spreads matter: the outer one keeps every other
  // series' entry, the inner one keeps this series' other fields.
  const patch = (name: string, field: keyof LineSeriesStyle, value: string | undefined) =>
    onChange({ seriesLine: { ...seriesLine, [name]: { ...seriesLine?.[name], [field]: value } } });
  return (
    <div className="space-y-1.5">
      {names.map((name) => (
        <div key={name} className="flex items-center gap-2">
          <Label className="min-w-0 flex-1 truncate text-sm font-normal" title={name}>{name}</Label>
          {/* "Auto" writes `undefined`, not the literal — the builder reads any stored value as an
              override, so a literal "auto" would cut this series off from `cfg.lineShape` for good.
              The values match what decorateSeries resolves: linear|smooth|step, and `step` is
              egret's existing middle-step mode, NOT Metabase's step-after. */}
          <Select
            value={seriesLine?.[name]?.shape ?? "auto"}
            onValueChange={(v) => patch(name, "shape", v === "auto" ? undefined : v)}
          >
            <SelectTrigger aria-label={`shape ${name}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">Auto</SelectItem>
              <SelectItem value="linear">Linear</SelectItem>
              <SelectItem value="smooth">Smooth</SelectItem>
              <SelectItem value="step">Step</SelectItem>
            </SelectContent>
          </Select>
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
          <Select
            value={seriesLine?.[name]?.markers ?? "auto"}
            onValueChange={(v) => patch(name, "markers", v === "auto" ? undefined : v)}
          >
            <SelectTrigger aria-label={`markers ${name}`} className="h-7 w-20"><SelectValue /></SelectTrigger>
            <SelectContent>
              {/* Same three options as the chart-wide Markers select, and the same meaning of
                  "auto" — show markers only when there are few enough points to read. Here the
                  fallback chain is one link longer: series → chart-wide → auto. */}
              <SelectItem value="auto" title="Auto shows markers only when there are few enough points to read">Auto</SelectItem>
              <SelectItem value="on">On</SelectItem>
              <SelectItem value="off">Off</SelectItem>
            </SelectContent>
          </Select>
        </div>
      ))}
    </div>
  );
}
