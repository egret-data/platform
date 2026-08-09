import type { Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "./tabs";
import type { MoveDestination } from "./inlineParameters";
import { AddPanel } from "./AddPanel";
import type { FieldLabels } from "@/components/chart/config/field-names";
import { CardSettingsPanel } from "./CardSettingsPanel";
import { FilterSettingsPanel } from "./FilterSettingsPanel";
import { CardVizPanel } from "./CardVizPanel";
import { X } from "lucide-react";

export type EditPanel =
  | { kind: "add" }
  | { kind: "card"; cardId: string }
  | { kind: "viz"; cardId: string }
  | { kind: "filter"; paramId: string }
  | null;

interface Props {
  panel: EditPanel;
  charts: Chart[];
  parameters: DashboardParameter[];
  tabs: DraftTab[];
  cardForPanel: DraftCard | undefined;
  chartForCard: Chart | undefined;
  paramForPanel: DashboardParameter | undefined;
  cardColumns?: { cardId: string; label: string; columns: string[] }[];
  /** verbose_name map for the card whose settings are open, and for the value-source card. */
  panelLabels?: FieldLabels;
  valuesSourceLabels?: FieldLabels;
  /** The open filter's loaded option list, so its DEFAULT is picked with the filter's own widget. */
  defaultOptions?: string[];
  /** W7: where the open filter may be moved to, where it is now, and how to move it. */
  moveDestinations?: MoveDestination[];
  hostCardId?: string | null;
  onMoveParam?: (cardId: string | null) => void;
  vizForPanel?: { chart: Chart; override: Record<string, unknown> | undefined; columns: { name: string; type: string }[]; rows?: unknown[][] };
  onChangeVizOverride?: (next: Record<string, unknown> | undefined) => void;
  onAddChart: (c: Chart) => void;
  onUpdateCardOptions: (patch: Record<string, unknown>) => void;
  onMoveCardToTab: (tabKey: string) => void;
  onChangeParam: (next: DashboardParameter) => void;
  onDeleteParam: () => void;
  onClose: () => void;
  onBack: () => void;
}

export function EditSidebar(props: Props) {
  const { panel } = props;
  const onPanel = panel?.kind === "card" || panel?.kind === "filter" || panel?.kind === "viz";
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[340px] shrink-0 self-start overflow-auto rounded-lg border bg-card">
      <div className="flex items-center justify-between border-b px-2 py-1.5">
        {onPanel
          ? <button type="button" aria-label="back to add" className="text-xs text-muted-foreground hover:text-foreground" onClick={props.onBack}>‹ Add cards</button>
          : <span className="px-1 text-xs font-medium text-muted-foreground">Add cards</span>}
        <button type="button" aria-label="close panel" title="Close" className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={props.onClose}><X className="h-4 w-4" /></button>
      </div>
      {panel?.kind === "add" && <AddPanel charts={props.charts} onAddChart={props.onAddChart} />}
      {panel?.kind === "card" && props.cardForPanel && (
        <CardSettingsPanel card={props.cardForPanel} chart={props.chartForCard} parameters={props.parameters} tabs={props.tabs} labels={props.panelLabels}
          onUpdateOptions={props.onUpdateCardOptions} onMoveToTab={props.onMoveCardToTab} />
      )}
      {panel?.kind === "viz" && props.vizForPanel && props.onChangeVizOverride && (
        <CardVizPanel chart={props.vizForPanel.chart} override={props.vizForPanel.override}
          columns={props.vizForPanel.columns} rows={props.vizForPanel.rows} onChange={props.onChangeVizOverride} />
      )}
      {panel?.kind === "filter" && props.paramForPanel && (
        <FilterSettingsPanel param={props.paramForPanel} allParameters={props.parameters}
          cardColumns={props.cardColumns ?? []} defaultOptions={props.defaultOptions ?? []} valuesSourceLabels={props.valuesSourceLabels}
          moveDestinations={props.moveDestinations ?? []} hostCardId={props.hostCardId ?? null}
          onMove={props.onMoveParam}
          onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
      )}
    </aside>
  );
}
