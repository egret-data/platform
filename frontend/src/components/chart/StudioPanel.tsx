import type { ReactNode } from "react";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";
import type { ColumnFormat } from "@/components/chart/format-types";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import type { VizControl, VizControlProps, VizDescriptor } from "./viz/types";

/** The one group name the shell promotes to its own tab — see `Props.tab`. */
export const AXES_GROUP = "Axes";

/** Whether this viz has anything to put in an Axes tab; the shell hides the tab otherwise. */
export function hasAxesControls<Cfg>(descriptor: VizDescriptor<Cfg> | undefined): boolean {
  return !!descriptor?.controls.some((c) => c.section === "display" && c.group === AXES_GROUP);
}

/** Whether this viz declares any Data-tab control a dashboard card may override — see
 *  `VizControl.cardOverridable`. The card panel hides the block otherwise, the same way it hides an
 *  empty Axes section, so a pie does not grow a heading with nothing under it. */
export function hasCardDataControls<Cfg>(descriptor: VizDescriptor<Cfg> | undefined): boolean {
  return !!descriptor?.controls.some((c) => c.section === "data" && c.cardOverridable);
}

interface Props<Cfg> {
  descriptor: VizDescriptor<Cfg>;
  config: Cfg;
  onConfigChange: (patch: Partial<Cfg>) => void;
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
  resultColumns: { name: string; type: string }[];
  rows: unknown[][];
  /** The units `rows` were bucketed by — see `VizControlProps.temporalUnits`, which this fills. */
  temporalUnits: Record<string, string>;
  /** The chart's `column_settings` — see `VizControlProps.columnSettings`, which this fills.
   *  Required rather than defaulted here: a caller that owns the map and forgets to pass it would
   *  otherwise get an empty one, and a control silently naming its rows the un-formatted way is
   *  precisely the defect this seam exists to close. */
  columnSettings: Record<string, ColumnFormat>;
  unitRow: (col?: string) => ReactNode;
  /**
   * Which panel tab to render. Only "data" and "display" are `section` values a control declares;
   * the other two are FILTERED VIEWS the shell composes, so a viz says where a control belongs once
   * and each surface decides what that means for it:
   *
   *   - "axes" is the DISPLAY section narrowed to the "Axes" group, so a group is a heading in the
   *     card sidebar and a tab of its own in the builder. Metabase splits its panel the same way
   *     (Data | Display | Axes).
   *   - "card-data" is the DATA section narrowed to the controls flagged `cardOverridable` — the
   *     ones that touch no query input. No builder tab renders it; it exists so `CardVizPanel` can
   *     offer presentation settings that live on the Data tab without also offering the query
   *     settings beside them. See `VizControl.cardOverridable` for why this is an allow-list.
   *
   * Both narrow rather than move: "data" and "display" still return exactly what they always did.
   */
  tab: "data" | "display" | "axes" | "card-data";
  /**
   * The caller renders more content in this tab after this panel — e.g. the shell-owned
   * `SharedDisplaySections` that always follows the Display tab. A descriptor declaring no
   * controls of its own is not the same as the tab being empty, so when this is set we stay
   * silent instead of telling the user "no options" right above the options that follow.
   */
  hasMoreBelow?: boolean;
}

/**
 * Renders the active descriptor's controls for one tab. The panel knows nothing about
 * any specific viz — which is the whole point of the registry.
 */
export function StudioPanel<Cfg>({
  descriptor, config, onConfigChange, columns, metrics, resultColumns, rows, temporalUnits,
  columnSettings, unitRow, tab, hasMoreBelow,
}: Props<Cfg>) {
  const shown = descriptor.controls.filter((c) => {
    if (tab === "axes") return c.section === "display" && c.group === AXES_GROUP;
    if (tab === "card-data") return c.section === "data" && !!c.cardOverridable;
    if (tab === "display") return c.section === "display" && c.group !== AXES_GROUP;
    return c.section === tab;
  });
  if (shown.length === 0) {
    // "card-data" is not a tab anyone navigated to, so there is no empty state to explain — its
    // caller guards on `hasCardDataControls` and simply renders nothing.
    if (hasMoreBelow || tab === "card-data") return null;
    return <p className="text-xs text-muted-foreground">No {tab} options for this chart type.</p>;
  }
  const controlProps = {
    config, onChange: onConfigChange, columns, metrics, resultColumns, rows, temporalUnits,
    columnSettings, unitRow,
  };
  // Inside the Axes TAB the group heading would just repeat the tab's own name, so everything
  // there renders flat — including the advanced ones, which no longer have a group to hide in.
  // "card-data" renders flat for the adjacent reason: it is a handful of hand-picked controls in a
  // sidebar that has no tabs, and each already draws its own heading.
  if (tab === "axes" || tab === "card-data") {
    return (
      <div className="space-y-1.5">
        {shown.map(({ id, render: Control }) => <Control key={id} {...controlProps} />)}
      </div>
    );
  }
  const ungrouped = shown.filter((c) => !c.group);
  // Group order follows first appearance in the descriptor, so a viz controls its own
  // panel order without a separate list to keep in sync.
  const groups: string[] = [];
  for (const c of shown) if (c.group && !groups.includes(c.group)) groups.push(c.group);

  return (
    <>
      {ungrouped.map(({ id, render: Control }) => <Control key={id} {...controlProps} />)}
      {groups.map((g) => (
        // Two descriptors can share a group name (e.g. "Axes"), so the key must include
        // the viz too — otherwise React reuses the ControlGroup instance across a chart-type
        // switch and its disclosure state leaks along with it.
        <ControlGroup key={`${descriptor.key}:${g}`} title={g}
          controls={shown.filter((c) => c.group === g)} controlProps={controlProps} />
      ))}
    </>
  );
}

function ControlGroup<Cfg>({ title, controls, controlProps }: {
  title: string;
  controls: VizControl<Cfg>[];
  controlProps: VizControlProps<Cfg>;
}) {
  // EVERYTHING RENDERS. The `advanced` flag used to hide controls behind a "More options (2)"
  // disclosure; Metabase shows its whole Display list. A disclosure with a count announces that
  // something is hidden without saying what, and the two controls behind it here — a goal line
  // and a trend line — are not advanced by any reading.
  //
  // `advanced` is still honoured by the Axes tab and by `card-data`, which render flat for their
  // own reasons above, so the flag stays on the descriptors rather than being ripped out.
  return (
    <div className="space-y-1.5">
      <SectionLabel>{title}</SectionLabel>
      {controls.map(({ id, render: Control }) => <Control key={id} {...controlProps} />)}
    </div>
  );
}
