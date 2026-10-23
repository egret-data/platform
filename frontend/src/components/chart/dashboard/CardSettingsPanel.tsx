import { Checkbox } from "@/components/ui/checkbox";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { displayName, fieldTitle, type FieldLabels } from "@/components/chart/config/field-names";
import { showsCardBackground } from "./card-chrome";
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
  /** verbose_name per column, so this list names a field the way the rest of the product does. */
  labels?: FieldLabels;
}

const inp = "h-8 w-full rounded border bg-background px-2 text-sm";
/** The crossfilter picker's "none" item. A Radix item cannot carry "", which is what an unmapped
 *  filter stores, so this sentinel stands in for it and is converted back to "" before it is saved. */
const NONE = "__none__";

export function CardSettingsPanel({ card, chart, parameters, tabs, onUpdateOptions, onMoveToTab, labels }: Props) {
  const moveToTab = tabs.length >= 2 ? (
    <label htmlFor={`cs-move-tab-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>Move to tab</span>
      <Select value={card.tabKey ?? ""} onValueChange={onMoveToTab}>
        <SelectTrigger size="sm" id={`cs-move-tab-${card.id}`} aria-label="move card to tab"><SelectValue /></SelectTrigger>
        <SelectContent>
          {tabs.map((t) => <SelectItem key={t.key} value={t.key}>{t.name}</SelectItem>)}
        </SelectContent>
      </Select>
    </label>
  ) : null;
  const eligibleChart = card.card_type === "chart" && (chart?.viz_type === "table" || chart?.viz_type === "pivot_table");
  const eligible = card.card_type === "text" || card.card_type === "heading" || card.card_type === "link" || eligibleChart;
  // THE PRIMITIVE, NOT `<input type="checkbox">`, and the `htmlFor` is not decoration.
  // #443 gave `Checkbox` a 4px radius (a 16px box at the old 6px read as a radio, and shape
  // is how a reader tells "choose one" from "choose many") and a transparent 24×24 hit area
  // for WCAG 2.2 SC 2.5.8. A raw input in this file got neither. Radix renders the control as
  // a `<button role="checkbox">`; a button IS labelable, so the wrapping `<label>` keeps
  // forwarding clicks from the row's whole width, but the explicit id/htmlFor pair is what
  // `table.tsx` settled on after #441 and matching it costs nothing.
  const autoHeightRow = eligible ? (
    <label htmlFor={`cs-auto-height-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>Auto height</span>
      <Checkbox id={`cs-auto-height-${card.id}`} aria-label="auto height" checked={!!card.options.autoHeight} onCheckedChange={(v) => onUpdateOptions({ autoHeight: v === true })} />
    </label>
  ) : null;
  // Metabase's wording, verbatim, because this panel is measured against its dashcard
  // modal. Only a chart card gets it: a text/heading/link card runs no query and so can
  // never be empty, which is the same line Metabase draws
  // (`getHidden: ([{ card }]) => isVirtualDashCard(card)`).
  const hideEmptyRow = card.card_type === "chart" ? (
    <label htmlFor={`cs-hide-empty-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>Hide this card if there are no results</span>
      <Checkbox id={`cs-hide-empty-${card.id}`} aria-label="hide when empty" checked={!!card.options.hideEmpty} onCheckedChange={(v) => onUpdateOptions({ hideEmpty: v === true })} />
    </label>
  ) : null;
  // Metabase's label verbatim ("Show background", `Text/index.ts:67-73`). EVERY card type gets it,
  // which is the generalisation §4 W5 asks for — Metabase offers it on text, heading, link and
  // iframe cards and egret has no reason to draw the line anywhere else. `showsCardBackground`
  // owns the default so the checkbox and the tile cannot disagree: a heading reads unchecked until
  // someone switches it on.
  const backgroundRow = (
    <label htmlFor={`cs-background-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>Show background</span>
      <Checkbox id={`cs-background-${card.id}`} aria-label="show background" checked={showsCardBackground(card)} onCheckedChange={(v) => onUpdateOptions({ background: v === true })} />
    </label>
  );
  if (card.card_type === "text" || card.card_type === "heading") {
    const slugs = parameters.map((p) => p.slug);
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">{card.card_type === "heading" ? "Heading" : "Text"}</div>
        <p className="text-xs text-muted-foreground">
          Edit the {card.card_type} on the card. Insert a filter value with <code className="rounded bg-muted px-1">{"{{slug}}"}</code>.
          {slugs.length > 0 && (
            <> Available: {slugs.map((s) => <code key={s} className="mr-1 rounded bg-muted px-1">{`{{${s}}}`}</code>)}</>
          )}
        </p>
        {autoHeightRow}
        {backgroundRow}
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
        {backgroundRow}
        {moveToTab}
      </div>
    );
  }

  if (card.card_type === "iframe") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">Iframe</div>
        {/* ONE free-text setting, matching Metabase's single `iframe` visualization setting: it
            takes either a bare URL or a pasted <iframe …> snippet, and `iframe-url.ts` decides
            which. Deliberately says nothing about {{slug}} — egret does not substitute dashboard
            parameters into a frame src (its `interpolateParams` does not URL-encode), so no user
            is promised a placeholder that would not be escaped. */}
        <Textarea aria-label="iframe url or embed code" rows={4}
          placeholder={'https://…  or  <iframe src="https://…" />'}
          value={(card.options.iframe as string) ?? ""} onChange={(e) => onUpdateOptions({ iframe: e.target.value })} />
        <p className="text-xs text-muted-foreground">
          Only https pages on egret's allowed hosts can be embedded. Anything else shows a
          refusal on the card instead of the page.
        </p>
        {/* No auto-height row and no hide-when-empty row: a cross-origin frame's content height
            cannot be measured from here, and an iframe card runs no query so it can never be
            empty. Both are withheld by `eligible`/`card_type === "chart"` above; naming them
            here so a future edit does not "restore" them by mistake. */}
        {backgroundRow}
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
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Description
        <Textarea aria-label="card description" rows={2} placeholder="Shown behind the info button on the card header"
          value={(card.options.description as string) ?? ""} onChange={(e) => onUpdateOptions({ description: e.target.value })} />
      </label>

      <div className="flex flex-col gap-1">
        <label htmlFor={`cs-on-click-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>On click</span>
          <Select value={cb.type}
            onValueChange={(v) => {
              const t = v as ClickBehavior["type"];
              onUpdateOptions({ clickBehavior: t === "url" ? { type: "url", template: "" } : t === "crossfilter" ? { type: "crossfilter", parameterMapping: {} } : { type: "menu" } });
            }}>
            <SelectTrigger size="sm" id={`cs-on-click-${card.id}`} aria-label="on click"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="menu">Menu</SelectItem>
              <SelectItem value="crossfilter">Cross-filter</SelectItem>
              <SelectItem value="url">Custom URL</SelectItem>
            </SelectContent>
          </Select>
        </label>
        {cb.type === "url" && (
          <input aria-label="url template" placeholder="https://…/{{column}}?p={{slug}}" className={inp} value={cb.template}
            onChange={(e) => onUpdateOptions({ clickBehavior: { type: "url", template: e.target.value } })} />
        )}
        {cb.type === "crossfilter" && parameters.map((p) => {
          const m = cb.parameterMapping ?? {};
          return (
            <label key={p.id} htmlFor={`cs-crossfilter-${card.id}-${p.id}`} className="flex items-center justify-between gap-2 text-sm"><span>{p.name} ←</span>
              <Select value={m[p.id] || NONE}
                onValueChange={(v) => onUpdateOptions({ clickBehavior: { type: "crossfilter", parameterMapping: { ...m, [p.id]: v === NONE ? "" : v } } })}>
                <SelectTrigger size="sm" id={`cs-crossfilter-${card.id}-${p.id}`} aria-label={`crossfilter ${p.name}`}><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value={NONE}>none</SelectItem>
                  {cols.map((col) => <SelectItem key={col} value={col} title={fieldTitle(col, labels)}>{displayName(col, labels)}</SelectItem>)}
                </SelectContent>
              </Select>
            </label>
          );
        })}
      </div>

      {autoHeightRow}
      {hideEmptyRow}
      {backgroundRow}
      {moveToTab}
    </div>
  );
}
