import type { ReactNode } from "react";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";
import type { VizDescriptor } from "./viz/types";

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
  return (
    <>
      {shown.map(({ id, render: Control }) => (
        <Control key={id} config={config} onChange={onConfigChange}
          columns={columns} metrics={metrics} resultColumns={resultColumns} rows={rows} unitRow={unitRow} />
      ))}
    </>
  );
}
