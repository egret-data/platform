import { applyVizOverride } from "./params";
import { ChartSettingsSections } from "@/components/chart/settings/ChartSettingsSections";
import type { Chart, ChartParams } from "@/api/charts";

interface Props {
  chart: Chart;
  override: Record<string, unknown> | undefined;
  columns: { name: string; type: string }[];
  rows?: unknown[][];
  onChange: (next: Record<string, unknown> | undefined) => void;
}

export function CardVizPanel({ chart, override, columns, rows, onChange }: Props) {
  const eff = applyVizOverride(chart, override);
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Visualization</div>
        <button type="button" aria-label="reset visualization" className="text-xs text-muted-foreground hover:text-foreground"
          onClick={() => onChange(undefined)}>Reset to chart defaults</button>
      </div>
      <ChartSettingsSections
        vizType={eff.viz_type}
        params={eff.params}
        columns={columns}
        rows={rows}
        onChange={(next: ChartParams) => {
          const { spec: _s, ...display } = next;
          onChange({ ...(override ?? {}), ...display });
        }}
      />
    </div>
  );
}
