import { applyVizOverride } from "./params";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { SharedDisplaySections, styleColumnsFor, ruleColumnsFor } from "@/components/chart/settings/SharedDisplaySections";
import { getDescriptor } from "@/components/chart/viz/registry";
import { vizConfig, shellSettings, mergeVizSettings } from "@/components/chart/viz/renderConfig";
import { isParamsV2 } from "@/api/charts";
import type { Chart, ChartParams } from "@/api/charts";

interface Props {
  chart: Chart;
  override: Record<string, unknown> | undefined;
  columns: { name: string; type: string }[];
  rows?: unknown[][];
  onChange: (next: Record<string, unknown> | undefined) => void;
}

/**
 * Per-card visualization settings on a dashboard. Composes the same two units as the
 * builder's Display tab — the descriptor's display controls and the shell-owned sections —
 * so the two editors cannot drift apart the way they did in #348.
 *
 * It emits a flat partial of `settings`, which is exactly what a v2 override is.
 */
export function CardVizPanel({ chart, override, columns, rows = [], onChange }: Props) {
  const eff = applyVizOverride(chart, override);
  const descriptor = getDescriptor(eff.viz_type);
  const config = vizConfig(eff.viz_type, eff.params);
  const { columnSettings, conditionalFormatting, tableState } = shellSettings(eff.params);
  const patch = (next: Record<string, unknown>) => onChange({ ...(override ?? {}), ...next });

  // styleColumnsFor inspects a handful of fields; hand it those, from the effective config.
  const styleParams = {
    spec: eff.params.spec,
    cartesian: config as { yMetrics: string[] },
    scalar: config as { metric?: string },
    metrics: isParamsV2(eff.params) ? (eff.params.settings["pivot.metrics"] as string[]) : undefined,
    rows: isParamsV2(eff.params) ? (eff.params.settings["pivot.rows"] as string[]) : undefined,
    cols: isParamsV2(eff.params) ? (eff.params.settings["pivot.cols"] as string[]) : undefined,
  } as ChartParams;
  const styleColumns = styleColumnsFor(eff.viz_type, styleParams, columns);
  // A rule may only target what the viz can actually colour — narrower than what it can format.
  const ruleColumns = ruleColumnsFor(eff.viz_type, styleParams, columns);

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Visualization</div>
        <button type="button" aria-label="reset visualization" className="text-xs text-muted-foreground hover:text-foreground"
          onClick={() => onChange(undefined)}>Reset to chart defaults</button>
      </div>
      {descriptor && (
        <StudioPanel
          descriptor={descriptor}
          config={config as never}
          onConfigChange={((p: object) =>
            // mergeVizSettings, not `patch`: `patch` spreads over the existing override, and a
            // setting the user has put back to "Auto" has to LEAVE it — a spread can only add
            // or overwrite, so Auto would silently re-save the value it was replacing.
            onChange(mergeVizSettings(override ?? {}, descriptor, { ...(config as object), ...p } as never))) as (p: never) => void}
          columns={[]}
          metrics={[]}
          resultColumns={columns}
          rows={rows}
          // The units `rows` were fetched with, straight off the effective params — the same field
          // `ChartRenderer` builds the card's chart from, so the panel keys on what was drawn. A
          // dashboard has no unit picker (see `unitRow` below), which is exactly why this cannot be
          // derived here: the answer lives in the saved spec.
          temporalUnits={eff.params.spec.temporal_units ?? {}}
          unitRow={() => null}
          tab="display"
          hasMoreBelow
        />
      )}
      <SharedDisplaySections
        vizType={eff.viz_type}
        resultColumns={columns}
        styleColumns={styleColumns}
        ruleColumns={ruleColumns}
        tableState={tableState}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        onTableState={(next) => patch({ "table.state": next })}
        onColumnSettings={(next) => patch({ column_settings: next })}
        onConditionalFormatting={(next) => patch({ conditional_formatting: next })}
      />
    </div>
  );
}
