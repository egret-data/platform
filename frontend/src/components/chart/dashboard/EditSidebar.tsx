import type { Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "./tabs";
import { AddPanel } from "./AddPanel";
import { CardSettingsPanel } from "./CardSettingsPanel";
import { FilterSettingsPanel } from "./FilterSettingsPanel";

export type EditPanel =
  | { kind: "add" }
  | { kind: "card"; cardId: string }
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
  onAddChart: (c: Chart) => void;
  onAddVirtual: (t: "text" | "heading" | "link") => void;
  onUpdateCardOptions: (patch: Record<string, unknown>) => void;
  onMoveCardToTab: (tabKey: string) => void;
  onChangeParam: (next: DashboardParameter) => void;
  onDeleteParam: () => void;
  onClose: () => void;
}

export function EditSidebar(props: Props) {
  const { panel } = props;
  const onPanel = panel?.kind === "card" || panel?.kind === "filter";
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[340px] shrink-0 self-start overflow-auto rounded-lg border bg-card">
      {onPanel && (
        <div className="flex items-center justify-between border-b px-3 py-2">
          <button type="button" aria-label="back to add" className="text-xs text-muted-foreground hover:text-foreground" onClick={props.onClose}>‹ Add cards</button>
        </div>
      )}
      {panel?.kind === "add" && <AddPanel charts={props.charts} onAddChart={props.onAddChart} onAddVirtual={props.onAddVirtual} />}
      {panel?.kind === "card" && props.cardForPanel && (
        <CardSettingsPanel card={props.cardForPanel} chart={props.chartForCard} parameters={props.parameters} tabs={props.tabs}
          onUpdateOptions={props.onUpdateCardOptions} onMoveToTab={props.onMoveCardToTab} />
      )}
      {panel?.kind === "filter" && props.paramForPanel && (
        <FilterSettingsPanel param={props.paramForPanel} allParameters={props.parameters} cardColumns={props.cardColumns ?? []} onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
      )}
    </aside>
  );
}
