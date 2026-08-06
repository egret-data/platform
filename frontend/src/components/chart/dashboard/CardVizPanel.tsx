import { applyVizOverride } from "./params";
import { StudioPanel, hasAxesControls } from "@/components/chart/StudioPanel";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
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
  //
  // EVERY KEY IT INSPECTS, and the two below were missing. `styleColumnsFor` filters the pie on
  // `params.pie` and the map on `params.map?.metric`; a synthetic object without those keys filters
  // on `undefined`, matches nothing, and the Formatting section simply does not appear — silently,
  // because an empty `styleColumns` is also the legitimate "not configured yet" state. The pie half
  // was fixed in the builder route (`routes/explore/ChartExplore.tsx`) and not here, so a dashboard
  // card could format a pie's slices in one editor and not the other; the map half would have been
  // INTRODUCED by narrowing the map's arm if this were left alone — the arm that used to fall
  // through to every column now filters, and filtering on a key that is not there hides the lot.
  // One omission, two symptoms, so both keys land together.
  const styleParams = {
    spec: eff.params.spec,
    cartesian: config as { yMetrics: string[] },
    scalar: config as { metric?: string },
    pie: config as { dimensions?: string[]; metric?: string },
    map: config as { region?: string; metric?: string },
    metrics: isParamsV2(eff.params) ? (eff.params.settings["pivot.metrics"] as string[]) : undefined,
    rows: isParamsV2(eff.params) ? (eff.params.settings["pivot.rows"] as string[]) : undefined,
    cols: isParamsV2(eff.params) ? (eff.params.settings["pivot.cols"] as string[]) : undefined,
  } as ChartParams;
  const styleColumns = styleColumnsFor(eff.viz_type, styleParams, columns);
  // A rule may only target what the viz can actually colour — narrower than what it can format.
  const ruleColumns = ruleColumnsFor(eff.viz_type, styleParams, columns);

  // Everything the two StudioPanel renders below share. Shared rather than copied so the Axes
  // section cannot drift from the Display one — the `resultColumns`/`rows` pair in particular, since
  // an Axes tab reading a different result from the Display tab beside it is precisely the class of
  // bug this PR exists to close.
  const studioProps = {
    config: config as never,
    columns: [],
    metrics: [],
    resultColumns: columns,
    rows,
    // The units `rows` were fetched with, straight off the effective params — the same field
    // `ChartRenderer` builds the card's chart from, so the panel keys on what was drawn. A
    // dashboard has no unit picker (see `unitRow` below), which is exactly why this cannot be
    // derived here: the answer lives in the spec the card's query ran with.
    temporalUnits: eff.params.spec.temporal_units ?? {},
    unitRow: () => null,
  };

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Visualization</div>
        <button type="button" aria-label="reset visualization" className="text-xs text-muted-foreground hover:text-foreground"
          onClick={() => onChange(undefined)}>Reset to chart defaults</button>
      </div>
      {descriptor && (
        <>
          <StudioPanel
            {...studioProps}
            descriptor={descriptor}
            onConfigChange={((p: object) =>
              // mergeVizSettings, not `patch`: `patch` spreads over the existing override, and a
              // setting the user has put back to "Auto" has to LEAVE it — a spread can only add
              // or overwrite, so Auto would silently re-save the value it was replacing.
              onChange(mergeVizSettings(override ?? {}, descriptor, { ...(config as object), ...p } as never))) as (p: never) => void}
            tab="display"
            hasMoreBelow
          />
          {/* ── AXES ──
              `StudioPanel`'s display filter is `section === "display" && group !== AXES_GROUP`: the
              Axes group is EXCLUDED there because the builder route promotes it out of the Display
              tab and into a tab of its own (`tab="axes"`). This panel asked for the display tab and
              nothing else, so on the waterfall, the box plot and the whole cartesian family the
              exclusion had no matching inclusion anywhere and the group fell on the floor — on a
              dashboard you could not override an axis label, hide gridlines, set a y-range or rotate
              x labels, all of which egret's own builder offers for the same chart. Metabase's
              dashcard modal keeps a full Axes tab (checked on its waterfall and its box plot), so
              this was a defect against the parity target and against ourselves at once.

              A SECTION rather than a tab, because this sidebar has none — and a labelled section is
              exactly how the Display tab already presents every other `group` a descriptor declares
              (`ControlGroup` renders the same `SectionLabel`), so nothing new is being invented.
              Guarded by `hasAxesControls`, the same predicate the builder hides its tab behind, so a
              pie does not grow an empty heading. */}
          {hasAxesControls(descriptor) && (
            <div className="space-y-1.5">
              <SectionLabel>Axes</SectionLabel>
              <StudioPanel
                {...studioProps}
                descriptor={descriptor}
                onConfigChange={((p: object) =>
                  onChange(mergeVizSettings(override ?? {}, descriptor, { ...(config as object), ...p } as never))) as (p: never) => void}
                tab="axes"
              />
            </div>
          )}
        </>
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
