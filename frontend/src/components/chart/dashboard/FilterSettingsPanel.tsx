import { Checkbox } from "@/components/ui/checkbox";
import type { DashboardParameter, ParamType, ParamOperator, ParamWidget } from "@/api/dashboards";
import { displayName, fieldTitle, type FieldLabels } from "@/components/chart/config/field-names";
import { OPERATORS } from "./ParameterEditor";
import { canRenderMultiValue, isMultiValue, isSingleOrMultiSelectable } from "./multiSelect";
import { linkedFilterState } from "./linkedFilters";
import { DefaultValueField } from "./DefaultValueField";
import { MoveParameterField } from "./MoveParameterField";
import type { MoveDestination } from "./inlineParameters";

interface Props {
  param: DashboardParameter;
  allParameters: DashboardParameter[];
  cardColumns?: { cardId: string; label: string; columns: string[] }[];
  /** verbose_name per column for the SELECTED value-source card. */
  valuesSourceLabels?: FieldLabels;
  /** The parameter's loaded option list, so a category/location default is picked, not typed. */
  defaultOptions?: string[];
  /** Every heading/question card the filter could be moved to, grouped by tab. Empty on a
   *  dashboard with no hostable card, which hides the control entirely. */
  moveDestinations?: MoveDestination[];
  /** The card that currently hosts this filter, or null for the top bar. */
  hostCardId?: string | null;
  onMove?: (cardId: string | null) => void;
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}

const WIDGETS: { v: ParamWidget; label: string }[] = [
  { v: "dropdown", label: "Dropdown" },
  { v: "search", label: "Search box" },
  { v: "input", label: "Input box" },
];

export function FilterSettingsPanel({ param, allParameters, cardColumns = [], valuesSourceLabels, defaultOptions = [], moveDestinations = [], hostCardId = null, onMove, onChange, onDelete }: Props) {
  const cls = "h-8 w-full rounded border bg-background px-2 text-sm";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  const showWidget = param.type === "category" || param.type === "id" || param.type === "location" || param.type === "text";
  const showValueSource = param.type === "category" || param.type === "id" || param.type === "location";
  const others = allParameters.filter((o) => o.id !== param.id);
  const linkState = linkedFilterState(param, others);
  const linked = param.filteringParameters ?? [];
  const toggleLink = (id: string) =>
    onChange({ ...param, filteringParameters: linked.includes(id) ? linked.filter((x) => x !== id) : [...linked, id] });

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">Filter settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Name
        <input aria-label="parameter name" className={cls} value={param.name} onChange={(e) => onChange({ ...param, name: e.target.value })} />
      </label>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Type
        <select aria-label="parameter type" className={cls} value={param.type}
          onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
          {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
      </label>
      {showOp && (
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">Operator
          <select aria-label="parameter operator" className={cls} value={param.operator}
            onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
            {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        </label>
      )}

      {showWidget && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">How should people filter on this?</div>
          {WIDGETS.map((w) => (
            <label key={w.v} className="flex items-center gap-2 text-sm">
              <input type="radio" name={`widget-${param.id}`} aria-label={`widget ${w.v}`}
                checked={(param.widget ?? "dropdown") === w.v}
                onChange={() => onChange({ ...param, widget: w.v })} /> {w.label}
            </label>
          ))}
        </div>
      )}

      {showValueSource && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">Where do values come from?</div>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from distinct" checked={(param.valuesSource?.type ?? "distinct") === "distinct"}
              onChange={() => onChange({ ...param, valuesSource: { type: "distinct" } })} /> From the data (distinct)
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from custom" checked={param.valuesSource?.type === "static"}
              onChange={() => onChange({ ...param, valuesSource: { type: "static", static: param.valuesSource?.static ?? [] } })} /> Custom list
          </label>
          {param.valuesSource?.type === "static" && (
            <textarea aria-label="custom values" className="h-20 w-full rounded border bg-background px-2 py-1 text-sm"
              value={(param.valuesSource.static ?? []).join("\n")}
              onChange={(e) => onChange({ ...param, valuesSource: { type: "static", static: e.target.value.split("\n").map((s) => s.trim()).filter(Boolean) } })} />
          )}
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`vs-${param.id}`} aria-label="values from card" checked={param.valuesSource?.type === "card"}
              onChange={() => onChange({ ...param, valuesSource: { type: "card", cardId: cardColumns[0]?.cardId, column: undefined } })} /> From a card column
          </label>
          {param.valuesSource?.type === "card" && (
            <div className="flex gap-1">
              <select aria-label="values card" className={cls} value={param.valuesSource.cardId ?? ""}
                onChange={(e) => onChange({ ...param, valuesSource: { type: "card", cardId: e.target.value, column: param.valuesSource?.column } })}>
                <option value="">card…</option>
                {cardColumns.map((c) => <option key={c.cardId} value={c.cardId}>{c.label}</option>)}
              </select>
              <select aria-label="values column" className={cls} value={param.valuesSource.column ?? ""}
                onChange={(e) => onChange({ ...param, valuesSource: { type: "card", cardId: param.valuesSource?.cardId, column: e.target.value } })}>
                <option value="">column…</option>
                {(cardColumns.find((c) => c.cardId === param.valuesSource?.cardId)?.columns ?? []).map((col) => <option key={col} value={col} title={fieldTitle(col, valuesSourceLabels)}>{displayName(col, valuesSourceLabels)}</option>)}
              </select>
            </div>
          )}
        </div>
      )}

      {/* Metabase gives "People can pick" exactly this position (ParameterSettings.tsx:221-241):
          between "How should people filter on this column?" and "Default value". */}
      {isSingleOrMultiSelectable(param) && canRenderMultiValue(param) && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">People can pick</div>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`multi-${param.id}`} aria-label="people can pick multiple"
              checked={isMultiValue(param)}
              onChange={() => onChange({ ...param, isMultiSelect: true })} /> Multiple values
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`multi-${param.id}`} aria-label="people can pick single"
              checked={!isMultiValue(param)}
              onChange={() => onChange({ ...param, isMultiSelect: false })} /> A single value
          </label>
        </div>
      )}

      <DefaultValueField param={param} options={defaultOptions}
        onChange={(d) => onChange({ ...param, default: d })} />

      {/* Metabase's framing, verbatim (ParameterLinkedFilters.tsx:52,93-103,120-127,143-160). The
          three non-list states used to render as NOTHING AT ALL, because the whole section sat
          behind `others.length > 0` — a user whose only other filter was a time grouping, or who
          had put this filter on a custom value list, saw a panel that simply stopped. */}
      <div className="flex flex-col gap-1">
        <div className="text-xs font-medium">Limit this filter&apos;s choices</div>

        {linkState.kind === "no-usable" && (
          <p className="text-xs text-muted-foreground">
            If you have another dashboard filter, you can limit the choices that are listed for this
            filter based on the selection of the other one. So first, add another dashboard filter.
          </p>
        )}

        {linkState.kind === "custom-source" && (
          <p className="text-xs text-muted-foreground">
            If the filter has values that are from another question or model, or a custom list, then
            this filter can&apos;t be limited by another dashboard filter.
          </p>
        )}

        {linkState.kind === "input-widget" && (
          <p className="text-xs text-muted-foreground">
            This filter can&apos;t be limited by another dashboard filter because its widget type is an
            input box.
          </p>
        )}

        {linkState.kind === "list" && (
          <>
            <p className="text-xs text-muted-foreground/70">
              If you toggle on one of these dashboard filters, selecting a value for that filter will
              limit the available choices for <strong>this</strong> filter.
            </p>
          {/* THE DENSEST LIST THIS CHANGE TOUCHES, and the one worth measuring rather than
              assuming. #443 sized the primitive's transparent hit area at 24×24 against the
              permission picker's 26px pitch, which leaves 2px between neighbouring targets.
              These rows are `text-sm` (a 20px line box) in a `gap-1` column: a 24px pitch, so
              the 24px targets ABUT exactly — 0px between, and still no overlap, so no click
              can land on the wrong filter. Measured in the browser at 24.00px pitch with
              neighbouring target boxes sharing an edge and never crossing it.

              `min-h-8` was considered and NOT applied. It would give a 36px pitch and a
              comfortable gap, but it also grows a list that is already one of several stacked
              in a 255px side panel, and #441's argument for 32px rows was that THE ROW BECOMES
              THE TARGET — which it already is here, because the `<label>` wraps the control and
              spans the row. Height would buy nothing this list does not have. */}
            {linkState.usable.map((o) => (
              <label key={o.id} htmlFor={`fsp-link-${o.id}`} className="flex cursor-pointer items-center gap-2 text-sm">
                <Checkbox id={`fsp-link-${o.id}`} aria-label={`link ${o.name}`} checked={linked.includes(o.id)} onCheckedChange={() => toggleLink(o.id)} /> {o.name}
              </label>
            ))}
          </>
        )}
      </div>

      <label htmlFor={`fsp-required-${param.id}`} className="flex cursor-pointer items-center gap-2 text-sm">
        <Checkbox id={`fsp-required-${param.id}`} aria-label="parameter required" checked={!!param.required} onCheckedChange={(v) => onChange({ ...param, required: v === true })} /> Required
      </label>

      {/* Metabase's position for this control (ParameterSettings.tsx:293): after the
          default/required block and before the destructive actions. */}
      {onMove && (
        <MoveParameterField value={hostCardId} destinations={moveDestinations} onChange={onMove} />
      )}

      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
