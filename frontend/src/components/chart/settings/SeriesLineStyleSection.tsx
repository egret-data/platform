import type { LineSeriesStyle } from "@/api/charts";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

/** The four rows, in Metabase's own order: shape, style, size, dots. */
const ROWS = [
  {
    field: "shape" as const, label: "Line shape",
    // linear|smooth|step is exactly what decorateSeries resolves, and `step` is egret's existing
    // middle-step mode, NOT Metabase's step-after — see the comment at cartesian.ts. A control
    // that offered step-after here would store a value the builder silently rounds to `middle`.
    options: [["linear", "Linear"], ["smooth", "Smooth"], ["step", "Step"]],
  },
  { field: "style" as const, label: "Line style", options: [["solid", "Solid"], ["dashed", "Dashed"], ["dotted", "Dotted"]] },
  { field: "size" as const, label: "Line size", options: [["S", "S"], ["M", "M"], ["L", "L"]] },
  // Named after egret's CHART-WIDE control ("Markers"), not after Metabase's "Show dots on
  // lines": this select's Auto falls back to that control, and two names for one setting is how
  // a user stops believing the fallback exists.
  {
    field: "markers" as const, label: "Markers", options: [["on", "On"], ["off", "Off"]],
    autoTitle: "Auto shows markers only when there are few enough points to read",
  },
];

/**
 * ONE series' line appearance — all four of `LineSeriesStyle`, as the body of that series'
 * drill-in popover (SeriesSettingsPopover).
 *
 * This used to be an N-row grid, one row per series with a Select per field, declared
 * `advanced: true` in viz/cartesian.tsx so it hid behind the Series group's "More options".
 * `shape` and `markers` were left out of it on purpose — a third and fourth Select per row would
 * have made an already-unbounded list worse — even though `decorateSeries`
 * (echarts/options/cartesian.ts) has honoured all four since the per-series map landed. A chart
 * could therefore draw a per-series step line that nobody could ask for: the same shape of defect
 * FormatSection.tsx names for `scale` and `number_separators`.
 *
 * The drill-in removes that premise rather than overriding the conclusion. Per series, the cost of
 * a field is now a row in a panel that opens on demand instead of a column across every row on
 * screen, so the list stays readable at ten series and the two unreachable fields become
 * reachable. It is also Metabase's shape: `ChartNestedSettingSeriesSingle` inside
 * `ChartSettingsWidgetPopover`, off the series row's ellipsis.
 */
export function SeriesLineStyleSection({ name, seriesLine, onChange }: {
  /** The ONE series this renders. Was `names: string[]` when this was the whole list. */
  name: string;
  seriesLine?: Record<string, LineSeriesStyle>;
  onChange: (patch: { seriesLine?: Record<string, LineSeriesStyle> }) => void;
}) {
  // Merge a single field into one series' entry without disturbing the others — seriesLine
  // is a map keyed by series name, so both spreads matter: the outer one keeps every other
  // series' entry, the inner one keeps this series' other fields.
  const patch = (field: keyof LineSeriesStyle, value: string | undefined) =>
    onChange({ seriesLine: { ...seriesLine, [name]: { ...seriesLine?.[name], [field]: value } } });
  return (
    <div className="space-y-1.5">
      {ROWS.map(({ field, label, options, autoTitle }) => (
        <div key={field} className="flex items-center justify-between gap-2">
          <Label className="text-sm font-normal">{label}</Label>
          {/* "Auto" writes `undefined`, not the literal — the builder reads any stored value as an
              override, so a literal "auto" would cut this series off from the chart-wide default
              for good. The fallback chain is one link longer than the chart-wide control's:
              series → chart-wide → the builder's own default. */}
          <Select
            value={seriesLine?.[name]?.[field] ?? "auto"}
            onValueChange={(v) => patch(field, v === "auto" ? undefined : v)}
          >
            <SelectTrigger aria-label={`${field} ${name}`} className="h-7 w-28"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto" title={autoTitle}>Auto</SelectItem>
              {options.map(([value, text]) => <SelectItem key={value} value={value}>{text}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
      ))}
    </div>
  );
}
