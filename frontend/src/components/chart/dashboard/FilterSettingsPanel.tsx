import { useState, type ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { DashboardParameter, ParamType, ParamOperator, ParamWidget } from "@/api/dashboards";
import { displayName, fieldTitle, type FieldLabels } from "@/components/chart/config/field-names";
import { FIELD_LABEL } from "@/components/chart/settings/FieldLabel";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import { OPERATORS, OPERATOR_LABELS, PARAM_TYPE_LABELS } from "./operators";
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

type ValuesSourceKind = "distinct" | "static" | "card";

/** A labelled field: the label above its control, as every field of the card settings panel. */
function Field({ id, label, children }: { id: string; label: string; children: ReactNode }) {
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id} className={FIELD_LABEL}>{label}</Label>
      {children}
    </div>
  );
}

/** One-of choices: a group label, then one radio per row with its label beside it. */
function Choice<T extends string>({ id, label, value, options, onChange }: {
  id: string;
  label: string;
  value: T;
  options: { v: T; label: string; ariaLabel: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="space-y-1.5">
      <div id={id} className={FIELD_LABEL}>{label}</div>
      <RadioGroup aria-labelledby={id} value={value} onValueChange={(v) => onChange(v as T)} className="gap-1.5">
        {options.map((o) => (
          <div key={o.v} className="flex items-center gap-2">
            <RadioGroupItem id={`${id}-${o.v}`} value={o.v} aria-label={o.ariaLabel} />
            <Label htmlFor={`${id}-${o.v}`} className={FIELD_LABEL}>{o.label}</Label>
          </div>
        ))}
      </RadioGroup>
    </div>
  );
}

/**
 * The filter's slug: its name in URLs and in `{{slug}}` (KI-065). Typed freely and committed on
 * blur or Enter; the route makes the committed text URL-safe and unique and rewrites the
 * `{{slug}}` references on the dashboard's cards. A blank box goes back to the current slug.
 */
function SlugField({ id, slug, onCommit }: { id: string; slug: string; onCommit: (slug: string) => void }) {
  // Seeded once: the caller keys this field on the slug, so a new slug is a new field.
  const [draft, setDraft] = useState(slug);
  const commit = () => {
    if (draft.trim() === "" || draft === slug) { setDraft(slug); return; }
    onCommit(draft.trim());
    // Back to the slug in force: a changed one remounts this field anyway, and text that
    // normalises to the same slug ("Region" for `region`) must not linger as if it were pending.
    setDraft(slug);
  };
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id} className={FIELD_LABEL}>URL name</Label>
      <Input id={id} aria-label="parameter URL name" aria-describedby={`${id}-help`} className="font-mono" value={draft}
        onChange={(e) => setDraft(e.target.value)} onBlur={commit}
        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } }} />
      <p id={`${id}-help`} className="text-xs text-muted-foreground">
        In links as <code>?{slug}=…</code> and in text cards as <code>{`{{${slug}}}`}</code>. Changing it
        updates the cards on this dashboard; links already shared stop setting this filter.
      </p>
    </div>
  );
}

/** Label on the left, switch on the right: the card settings panel's on/off row. */
function Toggle({ id, label, ariaLabel, checked, onChange }: {
  id: string; label: ReactNode; ariaLabel: string; checked: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label htmlFor={id} className={FIELD_LABEL}>{label}</Label>
      <Switch id={id} aria-label={ariaLabel} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

/**
 * The dashboard filter's settings, in the side panel where a card's settings open. It is built
 * from the same parts as that panel (`CardVizPanel` and the chart studio's sections): a field label
 * above a full-width `Select` or `Input`, a `Switch` for an on/off row, a `SectionLabel` over a
 * group (KI-235).
 */
export function FilterSettingsPanel({ param, allParameters, cardColumns = [], valuesSourceLabels, defaultOptions = [], moveDestinations = [], hostCardId = null, onMove, onChange, onDelete }: Props) {
  const id = `fsp-${param.id}`;
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  const showWidget = param.type === "category" || param.type === "id" || param.type === "location" || param.type === "text";
  const showValueSource = param.type === "category" || param.type === "id" || param.type === "location";
  const others = allParameters.filter((o) => o.id !== param.id);
  const linkState = linkedFilterState(param, others);
  const linked = param.filteringParameters ?? [];
  const toggleLink = (otherId: string) =>
    onChange({ ...param, filteringParameters: linked.includes(otherId) ? linked.filter((x) => x !== otherId) : [...linked, otherId] });
  const sourceKind: ValuesSourceKind = param.valuesSource?.type ?? "distinct";
  const setSource = (kind: ValuesSourceKind) =>
    onChange({
      ...param,
      valuesSource: kind === "distinct" ? { type: "distinct" }
        : kind === "static" ? { type: "static", static: param.valuesSource?.static ?? [] }
        : { type: "card", cardId: cardColumns[0]?.cardId, column: undefined },
    });
  const sourceCardColumns = cardColumns.find((c) => c.cardId === param.valuesSource?.cardId)?.columns ?? [];

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">Filter settings</div>

      <Field id={`${id}-name`} label="Name">
        <Input id={`${id}-name`} aria-label="parameter name" value={param.name} onChange={(e) => onChange({ ...param, name: e.target.value })} />
      </Field>
      <SlugField key={`${param.id}:${param.slug}`} id={`${id}-slug`} slug={param.slug} onCommit={(slug) => onChange({ ...param, slug })} />
      <Field id={`${id}-type`} label="Type">
        <Select value={param.type}
          onValueChange={(v) => { const t = v as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
          <SelectTrigger id={`${id}-type`} aria-label="parameter type" className="w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            {(Object.keys(OPERATORS) as ParamType[]).map((t) => <SelectItem key={t} value={t}>{PARAM_TYPE_LABELS[t]}</SelectItem>)}
          </SelectContent>
        </Select>
      </Field>
      {showOp && (
        <Field id={`${id}-operator`} label="Operator">
          <Select value={param.operator} onValueChange={(v) => onChange({ ...param, operator: v as ParamOperator })}>
            <SelectTrigger id={`${id}-operator`} aria-label="parameter operator" className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              {OPERATORS[param.type].map((o) => <SelectItem key={o} value={o}>{OPERATOR_LABELS[o]}</SelectItem>)}
            </SelectContent>
          </Select>
        </Field>
      )}

      {showWidget && (
        <Choice id={`${id}-widget`} label="How should people filter on this?"
          value={param.widget ?? "dropdown"}
          options={WIDGETS.map((w) => ({ v: w.v, label: w.label, ariaLabel: `widget ${w.v}` }))}
          onChange={(v) => onChange({ ...param, widget: v })} />
      )}

      {showValueSource && (
        <div className="space-y-2">
          <Choice<ValuesSourceKind> id={`${id}-source`} label="Where do values come from?"
            value={sourceKind}
            options={[
              { v: "distinct", label: "From the data (distinct)", ariaLabel: "values from distinct" },
              { v: "static", label: "Custom list", ariaLabel: "values from custom" },
              { v: "card", label: "From a card column", ariaLabel: "values from card" },
            ]}
            onChange={setSource} />
          {param.valuesSource?.type === "static" && (
            <Textarea aria-label="custom values" rows={4} placeholder="One value per line"
              value={(param.valuesSource.static ?? []).join("\n")}
              onChange={(e) => onChange({ ...param, valuesSource: { type: "static", static: e.target.value.split("\n").map((s) => s.trim()).filter(Boolean) } })} />
          )}
          {param.valuesSource?.type === "card" && (
            <div className="grid grid-cols-2 gap-2">
              <Select value={param.valuesSource.cardId ?? ""}
                onValueChange={(v) => onChange({ ...param, valuesSource: { type: "card", cardId: v, column: param.valuesSource?.column } })}>
                <SelectTrigger aria-label="values card" className="w-full"><SelectValue placeholder="Card…" /></SelectTrigger>
                <SelectContent>
                  {cardColumns.map((c) => <SelectItem key={c.cardId} value={c.cardId}>{c.label}</SelectItem>)}
                </SelectContent>
              </Select>
              <Select value={param.valuesSource.column ?? ""}
                onValueChange={(v) => onChange({ ...param, valuesSource: { type: "card", cardId: param.valuesSource?.cardId, column: v } })}>
                <SelectTrigger aria-label="values column" className="w-full"><SelectValue placeholder="Column…" /></SelectTrigger>
                <SelectContent>
                  {sourceCardColumns.map((col) => (
                    <SelectItem key={col} value={col} title={fieldTitle(col, valuesSourceLabels)}>{displayName(col, valuesSourceLabels)}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
        </div>
      )}

      {/* Metabase gives "People can pick" exactly this position (ParameterSettings.tsx:221-241):
          between "How should people filter on this column?" and "Default value". */}
      {isSingleOrMultiSelectable(param) && canRenderMultiValue(param) && (
        <Choice id={`${id}-multi`} label="People can pick"
          value={isMultiValue(param) ? "multiple" : "single"}
          options={[
            { v: "multiple", label: "Multiple values", ariaLabel: "people can pick multiple" },
            { v: "single", label: "A single value", ariaLabel: "people can pick single" },
          ]}
          onChange={(v) => onChange({ ...param, isMultiSelect: v === "multiple" })} />
      )}

      <DefaultValueField param={param} options={defaultOptions}
        onChange={(d) => onChange({ ...param, default: d })} />

      <Toggle id={`${id}-required`} label="Required" ariaLabel="parameter required"
        checked={!!param.required} onChange={(v) => onChange({ ...param, required: v })} />

      {/* Metabase's framing, verbatim (ParameterLinkedFilters.tsx:52,93-103,120-127,143-160). The
          three non-list states used to render as NOTHING AT ALL, because the whole section sat
          behind `others.length > 0` — a user whose only other filter was a time grouping, or who
          had put this filter on a custom value list, saw a panel that simply stopped. */}
      <div className="flex flex-col gap-2">
        <SectionLabel>Limit this filter&apos;s choices</SectionLabel>

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
            <p className="text-xs text-muted-foreground">
              If you toggle on one of these dashboard filters, selecting a value for that filter will
              limit the available choices for <strong>this</strong> filter.
            </p>
            {/* A switch per filter, the panel's on/off row, which is what "toggle on" above names. */}
            {linkState.usable.map((o) => (
              <Toggle key={o.id} id={`${id}-link-${o.id}`} label={o.name} ariaLabel={`link ${o.name}`}
                checked={linked.includes(o.id)} onChange={() => toggleLink(o.id)} />
            ))}
          </>
        )}
      </div>

      {/* Metabase's position for this control (ParameterSettings.tsx:293): after the
          default/required block and before the destructive actions. */}
      {onMove && (
        <MoveParameterField value={hostCardId} destinations={moveDestinations} onChange={onMove} />
      )}

      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
