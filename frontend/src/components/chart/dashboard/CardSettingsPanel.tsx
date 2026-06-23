import type { Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import type { ClickBehavior } from "./click-behavior";
import type { DraftCard, DraftTab } from "./tabs";

interface Props {
  card: DraftCard;
  chart: Chart | undefined;
  parameters: DashboardParameter[];
  tabs: DraftTab[];
  onUpdateOptions: (patch: Record<string, unknown>) => void;
  onMoveToTab: (tabKey: string) => void;
}

const inp = "h-8 w-full rounded border bg-background px-2 text-sm";
const sel = "h-8 rounded border bg-background px-1 text-sm";

export function CardSettingsPanel({ card, chart, parameters, tabs, onUpdateOptions, onMoveToTab }: Props) {
  const moveToTab = tabs.length >= 2 ? (
    <label className="flex items-center justify-between gap-2 text-sm"><span>Move to tab</span>
      <select aria-label="move card to tab" className={sel} value={card.tabKey ?? ""} onChange={(e) => onMoveToTab(e.target.value)}>
        {tabs.map((t) => <option key={t.key} value={t.key}>{t.name}</option>)}
      </select>
    </label>
  ) : null;
  const eligibleChart = card.card_type === "chart" && (chart?.viz_type === "table" || chart?.viz_type === "pivot_table");
  const eligible = card.card_type === "text" || card.card_type === "heading" || card.card_type === "link" || eligibleChart;
  const autoHeightRow = eligible ? (
    <label className="flex items-center justify-between gap-2 text-sm"><span>Auto height</span>
      <input type="checkbox" aria-label="auto height" checked={!!card.options.autoHeight} onChange={(e) => onUpdateOptions({ autoHeight: e.target.checked })} />
    </label>
  ) : null;
  if (card.card_type === "text" || card.card_type === "heading") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">{card.card_type === "heading" ? "Heading" : "Text"}</div>
        <textarea aria-label={card.card_type === "heading" ? "heading text" : "text body"} className={`${inp} h-24 py-1`}
          value={(card.options.text as string) ?? ""} onChange={(e) => onUpdateOptions({ text: e.target.value })} />
        {autoHeightRow}
        {moveToTab}
      </div>
    );
  }
  if (card.card_type === "link") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">Link</div>
        <input aria-label="link url" placeholder="https://…" className={inp} value={(card.options.url as string) ?? ""} onChange={(e) => onUpdateOptions({ url: e.target.value })} />
        <input aria-label="link label" placeholder="Label (optional)" className={inp} value={(card.options.label as string) ?? ""} onChange={(e) => onUpdateOptions({ label: e.target.value })} />
        {autoHeightRow}
        {moveToTab}
      </div>
    );
  }

  const cols = chart ? [...chart.params.spec.dimensions, ...chart.params.spec.columns] : [];
  const cb = (card.options.clickBehavior as ClickBehavior | undefined) ?? { type: "menu" };
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">{chart?.name ?? "Card"} settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Title
        <input aria-label="card title" placeholder="Blank = chart name" className={inp} value={(card.options.title as string) ?? ""} onChange={(e) => onUpdateOptions({ title: e.target.value })} />
      </label>

      <div className="flex flex-col gap-1">
        <label className="flex items-center justify-between gap-2 text-sm"><span>On click</span>
          <select aria-label="on click" className={sel} value={cb.type}
            onChange={(e) => {
              const t = e.target.value as ClickBehavior["type"];
              onUpdateOptions({ clickBehavior: t === "url" ? { type: "url", template: "" } : t === "crossfilter" ? { type: "crossfilter", parameterMapping: {} } : { type: "menu" } });
            }}>
            <option value="menu">Menu</option><option value="crossfilter">Cross-filter</option><option value="url">Custom URL</option>
          </select>
        </label>
        {cb.type === "url" && (
          <input aria-label="url template" placeholder="https://…/{{column}}?p={{slug}}" className={inp} value={cb.template}
            onChange={(e) => onUpdateOptions({ clickBehavior: { type: "url", template: e.target.value } })} />
        )}
        {cb.type === "crossfilter" && parameters.map((p) => {
          const m = cb.parameterMapping ?? {};
          return (
            <label key={p.id} className="flex items-center justify-between gap-2 text-sm"><span>{p.name} ←</span>
              <select aria-label={`crossfilter ${p.name}`} className={sel} value={m[p.id] ?? ""}
                onChange={(e) => onUpdateOptions({ clickBehavior: { type: "crossfilter", parameterMapping: { ...m, [p.id]: e.target.value } } })}>
                <option value="">none</option>
                {cols.map((col) => <option key={col} value={col}>{col}</option>)}
              </select>
            </label>
          );
        })}
      </div>

      {autoHeightRow}
      {moveToTab}
    </div>
  );
}
