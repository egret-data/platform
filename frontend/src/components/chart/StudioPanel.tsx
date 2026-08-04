import { useState, type ReactNode } from "react";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { VizControl, VizControlProps, VizDescriptor } from "./viz/types";

/** The one group name the shell promotes to its own tab — see `Props.tab`. */
export const AXES_GROUP = "Axes";

/** Whether this viz has anything to put in an Axes tab; the shell hides the tab otherwise. */
export function hasAxesControls<Cfg>(descriptor: VizDescriptor<Cfg> | undefined): boolean {
  return !!descriptor?.controls.some((c) => c.section === "display" && c.group === AXES_GROUP);
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
  unitRow: (col?: string) => ReactNode;
  /**
   * Which panel tab to render. "axes" is not a `section` any control declares — it is the
   * DISPLAY section filtered to the "Axes" group, so a viz says where a control belongs once
   * (`group: "Axes"`) and the shell decides whether that group is a heading or a tab. Metabase
   * splits its own panel the same way (Data | Display | Axes).
   */
  tab: "data" | "display" | "axes";
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
  descriptor, config, onConfigChange, columns, metrics, resultColumns, rows, temporalUnits, unitRow,
  tab, hasMoreBelow,
}: Props<Cfg>) {
  const shown = descriptor.controls.filter((c) => {
    if (tab === "axes") return c.section === "display" && c.group === AXES_GROUP;
    if (tab === "display") return c.section === "display" && c.group !== AXES_GROUP;
    return c.section === tab;
  });
  if (shown.length === 0) {
    if (hasMoreBelow) return null;
    return <p className="text-xs text-muted-foreground">No {tab} options for this chart type.</p>;
  }
  const controlProps = {
    config, onChange: onConfigChange, columns, metrics, resultColumns, rows, temporalUnits, unitRow,
  };
  // Inside the Axes TAB the group heading would just repeat the tab's own name, so everything
  // there renders flat — including the advanced ones, which no longer have a group to hide in.
  if (tab === "axes") {
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
  const [open, setOpen] = useState(false);
  const advanced = controls.filter((c) => c.advanced);
  return (
    <div className="space-y-1.5">
      <SectionLabel>{title}</SectionLabel>
      {controls.filter((c) => !c.advanced).map(({ id, render: Control }) => (
        <Control key={id} {...controlProps} />
      ))}
      {advanced.length > 0 && (
        <Collapsible open={open} onOpenChange={setOpen} className="space-y-1.5">
          <CollapsibleContent className="space-y-1.5">
            {advanced.map(({ id, render: Control }) => <Control key={id} {...controlProps} />)}
          </CollapsibleContent>
          <CollapsibleTrigger className="text-xs text-muted-foreground hover:text-foreground">
            {open ? "Fewer options" : `More options (${advanced.length})`}
          </CollapsibleTrigger>
        </Collapsible>
      )}
    </div>
  );
}
