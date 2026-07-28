import { useState, type ReactNode } from "react";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { VizControl, VizControlProps, VizDescriptor } from "./viz/types";

interface Props<Cfg> {
  descriptor: VizDescriptor<Cfg>;
  config: Cfg;
  onConfigChange: (patch: Partial<Cfg>) => void;
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
  resultColumns: { name: string; type: string }[];
  rows: unknown[][];
  unitRow: (col?: string) => ReactNode;
  tab: "data" | "display";
}

/**
 * Renders the active descriptor's controls for one tab. The panel knows nothing about
 * any specific viz — which is the whole point of the registry.
 */
export function StudioPanel<Cfg>({
  descriptor, config, onConfigChange, columns, metrics, resultColumns, rows, unitRow, tab,
}: Props<Cfg>) {
  const shown = descriptor.controls.filter((c) => c.section === tab);
  if (shown.length === 0) {
    return <p className="text-xs text-muted-foreground">No {tab} options for this chart type.</p>;
  }
  const controlProps = { config, onChange: onConfigChange, columns, metrics, resultColumns, rows, unitRow };
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
        <Collapsible open={open} onOpenChange={setOpen}>
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
