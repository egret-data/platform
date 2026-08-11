import { useState } from "react";
import { isNumericType, isBooleanType, type Rule, type SingleColorRule, type ColorRangeRule, type FormattingOperator } from "../format-types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ChevronsUpDown } from "lucide-react";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { GROUP_LABEL } from "./FieldLabel";
import { ColorPicker } from "./ColorPicker";

interface ResultColumn { name: string; type: string }

const NUMERIC_OPS: FormattingOperator[] = ["=", "!=", "<", ">", "<=", ">="];
const STRING_OPS: FormattingOperator[] = ["=", "!=", "contains", "does-not-contain", "starts-with", "ends-with"];
/**
 * The pair `matchOperator` has always implemented and no dropdown could produce.
 *
 * `conditional.ts` handles `is-true`/`is-false` (and `NO_VALUE_OPS` below already knew to hide the
 * value box for them), but the operator list was a two-way choice between numeric and string and
 * neither offered them — so the only way to store one was to hand-edit a chart's params. Metabase
 * names the identical pair `BOOLEAN_OPERATOR_NAMES` and offers it for the same column class.
 *
 * There IS such a column class here: `ColumnMeta.type` carries the engine's own type name, Trino
 * reports `boolean`, and neither `isNumericType` nor `isTemporalType` claims it — so a boolean
 * column is already offered to a rule and, until now, could only be asked about by typing the word
 * "true" into the string editor's value box and leaning on `matchOperator`'s `String(cell)`
 * fallback. That is why this is a list to ADD rather than an implementation to delete.
 */
const BOOLEAN_OPS: FormattingOperator[] = ["is-true", "is-false"];
const NULL_OPS: FormattingOperator[] = ["is-null", "not-null"];
const NO_VALUE_OPS = new Set<FormattingOperator>(["is-null", "not-null", "is-true", "is-false"]);

/**
 * WHAT EACH OPERATOR READS AS. The stored token is not the label, and printing it as one is how
 * `does-not-contain` and `not-null` — kebab-case identifiers — came to be shown to a user.
 *
 * The dropdown was `ops.map((op) => <SelectItem value={op}>{op}</SelectItem>)`, so the label was
 * the identifier by construction and every operator was affected, not just the two hyphenated
 * ones: `!=` and `>=` are mathematical notation in a panel whose every other control is a sentence.
 *
 * COPIED STRING FOR STRING from Metabase's `ALL_OPERATOR_NAMES`
 * (`visualizations/components/settings/ChartSettingsTableFormatting/get-operators-for-columns.ts`),
 * not re-worded, and read back off the running reference at :3033 to confirm the rendering matches
 * the source. The one that would have been guessed wrong is `not-null`: it reads "is not null",
 * with an "is" the token has no trace of.
 *
 * THE KEYS ARE THE STORED VALUES AND STAY EXACTLY AS THEY ARE. `conditional.ts`'s `matchOperator`
 * switches on these tokens, `FormattingOperator` is their type, and every chart already saved
 * carries them in its `conditional_formatting`. Relabelling is one file; renaming would be a
 * migration whose only purchase is a prettier value nobody sees.
 *
 * `Record<FormattingOperator, string>` ON PURPOSE, not a partial map with a `?? op` fallback. A
 * fallback would let a newly added operator quietly print its own token again — which is the whole
 * defect — where this makes `tsc` demand a label the moment `FormattingOperator` grows.
 */
const OPERATOR_LABELS: Record<FormattingOperator, string> = {
  "=": "is equal to",
  "!=": "is not equal to",
  "<": "is less than",
  ">": "is greater than",
  "<=": "is less than or equal to",
  ">=": "is greater than or equal to",
  contains: "contains",
  "does-not-contain": "does not contain",
  "starts-with": "starts with",
  "ends-with": "ends with",
  "is-true": "is true",
  "is-false": "is false",
  "is-null": "is null",
  "not-null": "is not null",
};
const PALETTE = ["#ef4444", "#f59e0b", "#22c55e", "#3b82f6", "#a855f7"];
const RANGE = ["#ffffff", "#3b82f6"];

// preset two-stop gradients for the color-range (heatmap) rule
const RANGE_PRESETS: { label: string; colors: [string, string] }[] = [
  { label: "White to blue", colors: ["#ffffff", "#3b82f6"] },
  { label: "White to green", colors: ["#ffffff", "#22c55e"] },
  { label: "White to red", colors: ["#ffffff", "#ef4444"] },
  { label: "Red to green", colors: ["#ef4444", "#22c55e"] },
  { label: "Blue to red", colors: ["#3b82f6", "#ef4444"] },
];

export function RuleEditor({
  columns, rule, isPivot, onSave, onCancel,
}: {
  columns: ResultColumn[];
  rule: Rule;
  isPivot: boolean;
  onSave: (r: Rule) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<Rule>(rule);
  const selected = columns.filter((c) => draft.columns.includes(c.name));
  const allNumeric = selected.length > 0 && selected.every((c) => isNumericType(c.type));
  const allBoolean = selected.length > 0 && selected.every((c) => isBooleanType(c.type));
  const ops = allBoolean
    ? [...BOOLEAN_OPS, ...NULL_OPS]
    : allNumeric
      ? [...NUMERIC_OPS, ...NULL_OPS]
      : [...STRING_OPS, ...NULL_OPS];

  /**
   * The operator actually in force, which is not always the one in the draft.
   *
   * The operator list depends on the COLUMNS, and the columns can change under a chosen operator: a
   * new rule is minted with `>` (`ConditionalFormattingSection`), so picking a text column left the
   * Select with a value none of its options carried — Radix renders that as an EMPTY trigger — and
   * saving stored a `>` that `matchOperator` went on to evaluate as a string comparison. That is
   * pre-existing, and adding `BOOLEAN_OPS` above adds a third way into it, so it is resolved here
   * rather than widened.
   *
   * Falls back to `ops[0]` — `=` for text, `>` for numbers, `is-true` for booleans — which is the
   * first thing the reader would have picked anyway, and never to a hidden state: what the trigger
   * reads is what `onSave` writes, because both go through this.
   */
  const operator = draft.type === "single" && ops.includes(draft.operator) ? (draft as SingleColorRule).operator : ops[0];

  /** What leaves this editor. See `operator` — the draft may hold one the columns cannot express. */
  const save = () => onSave(draft.type === "single" ? { ...draft, operator } : draft);

  function setType(type: "single" | "range") {
    if (type === draft.type) return;
    setDraft((d) =>
      type === "single"
        ? ({ columns: d.columns, type: "single", operator: ">", value: "", color: PALETTE[0] } as SingleColorRule)
        : ({ columns: d.columns, type: "range", colors: RANGE, min_type: null, max_type: null } as ColorRangeRule),
    );
  }

  const range = draft as ColorRangeRule;
  const start = range.type === "range" ? range.colors[0] : RANGE[0];
  const end = range.type === "range" ? range.colors[range.colors.length - 1] : RANGE[1];
  function setRangeColor(idx: 0 | 1, v: string) {
    setDraft((d) => {
      const r = d as ColorRangeRule;
      const colors = [...r.colors];
      colors[idx === 0 ? 0 : colors.length - 1] = v;
      return { ...r, colors };
    });
  }

  /**
   * WHERE A COLOUR RANGE STARTS AND ENDS — the four fields `conditional.ts` has always honoured and
   * nothing could write.
   *
   * `min_type`/`max_type` are a three-way answer: `null` re-bases the gradient on EACH column's own
   * extent, `"all"` takes one extent across every column the rule targets so they can be compared
   * against each other, and `"custom"` pins an endpoint to a fixed number so two cards drawn a week
   * apart mean the same thing. The editor hard-coded `null` for both on creation and offered no
   * control, so only the first was ever reachable.
   *
   * THE OPTION SET IS METABASE'S, read off the running reference rather than guessed: with one
   * column it is "Smallest value in this column" / "Custom value"; with two or more the first
   * becomes "…in each column" and "…in all of these columns" appears between them. The conditional
   * third option is the part worth copying — over a single column "all" and "each" compute the
   * identical extent, so offering it there would be a choice with no consequence.
   *
   * Rendered as a `Select` where Metabase uses a radio group. Deliberate: every other control in
   * this editor is a Select, and two stacked three-option radio groups with labels this long is a
   * different weight of control in a panel this narrow. The labels and the stored values are
   * Metabase's exactly.
   */
  const CUSTOM_DEFAULT = { min: 0, max: 100 };
  const setEndpointType = (end: "min" | "max", v: string) =>
    setDraft((d) => {
      const r = { ...(d as ColorRangeRule) };
      r[`${end}_type`] = v === "each" ? null : (v as "all" | "custom");
      if (v === "custom") {
        r[`${end}_value`] = r[`${end}_value`] ?? CUSTOM_DEFAULT[end];
      } else {
        // DELETED, not set to undefined. A stale `max_value` sitting under `max_type: null` is
        // ignored by `conditional.ts` only by accident of its `=== "custom"` guard, and it would
        // reappear the moment anyone chose Custom again — the box would be pre-filled with a number
        // the reader had already taken back. `toStrictEqual` in the test is what holds this: both
        // `toEqual` and `toHaveBeenCalledWith` skip an `undefined` property and would not see it.
        delete r[`${end}_value`];
      }
      return r;
    });
  const setEndpointValue = (end: "min" | "max", raw: string) =>
    setDraft((d) => {
      const r = { ...(d as ColorRangeRule) };
      const n = Number(raw);
      // An empty box is not zero. Left undefined, `conditional.ts` falls back to the column's own
      // extent for that end, which is the same thing the reader sees while they are mid-edit.
      r[`${end}_value`] = raw.trim() === "" || Number.isNaN(n) ? undefined : n;
      return r;
    });

  /**
   * One endpoint's control pair.
   *
   * A FUNCTION RETURNING JSX, called inline — not a nested component. A component declared inside
   * a render body is a new type on every render, so React unmounts and remounts its subtree: the
   * number input below would lose focus after a single keystroke and the rest of a typed value
   * would land nowhere.
   */
  const endpointField = (which: "min" | "max") => {
    const label = which === "min" ? "Start the range at" : "End the range at";
    const extreme = which === "min" ? "Smallest" : "Largest";
    const many = range.columns.length > 1;
    const type = range[`${which}_type`];
    return (
      <div className="space-y-1" key={which}>
        <span className="text-xs text-muted-foreground">{label}</span>
        <Select value={type ?? "each"} onValueChange={(v) => setEndpointType(which, v)}>
          <SelectTrigger aria-label={label} className="w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="each">{extreme} value in {many ? "each column" : "this column"}</SelectItem>
            {/* Only with something to compare ACROSS. Over one column this computes the same number
                as "each", so Metabase hides it and so does this. */}
            {many && <SelectItem value="all">{extreme} value in all of these columns</SelectItem>}
            <SelectItem value="custom">Custom value</SelectItem>
          </SelectContent>
        </Select>
        {type === "custom" && (
          <Input className="w-full"
            type="number"
            aria-label={which === "min" ? "Range start value" : "Range end value"}
           
            value={range[`${which}_value`] ?? ""}
            onChange={(e) => setEndpointValue(which, e.target.value)}
          />
        )}
      </div>
    );
  };

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className={GROUP_LABEL}>Columns</div>
        {/* A dropdown, not one standing checkbox per column: the list is as long as the result
            is wide, and on a table twenty columns of rows pushed the operator, value and colour
            off the panel. Closed, the trigger reads like a select and states the choice; open, it
            is the same tick-list as the field pickers in the Data tab.

            Multi-select rather than a plain Select because a rule genuinely targets several
            columns — `buildConditional` iterates them, and a colour range takes its extent across
            all of them at once, so narrowing this to one would change what a rule can express. */}
        <Popover>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label="Columns"
              className="flex w-full items-center justify-between gap-2 rounded-md border px-2 text-left text-sm hover:bg-muted/30"
            >
              <span className={`truncate ${draft.columns.length ? "" : "text-muted-foreground"}`}>
                {draft.columns.length === 0
                  ? "Pick columns…"
                  : draft.columns.length <= 2
                    ? draft.columns.join(", ")
                    : `${draft.columns.length} columns`}
              </span>
              <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground opacity-50" />
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="max-h-64 w-64 overflow-y-auto p-1">
            {columns.map((c) => (
              <label
                key={c.name}
                className="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm hover:bg-accent"
              >
                <Checkbox
                  checked={draft.columns.includes(c.name)}
                  onCheckedChange={() =>
                    setDraft((d) => ({
                      ...d,
                      columns: d.columns.includes(c.name)
                        ? d.columns.filter((x) => x !== c.name)
                        : [...d.columns, c.name],
                    }))
                  }
                />
                <span className="flex-1 truncate">{c.name}</span>
                <span className="text-xs text-muted-foreground">{c.type}</span>
              </label>
            ))}
          </PopoverContent>
        </Popover>
      </div>

      {allNumeric && (
        <label className="block space-y-1">
          <span className="text-xs text-muted-foreground">Formatting</span>
          <Select value={draft.type} onValueChange={(v) => setType(v as "single" | "range")}>
            <SelectTrigger aria-label="Rule type" className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="single">Single color</SelectItem>
              <SelectItem value="range">Color range</SelectItem>
            </SelectContent>
          </Select>
        </label>
      )}

      {draft.type === "single" ? (
        <div className="space-y-2">
          <label className="block space-y-1">
            <span className="text-xs text-muted-foreground">When a value is</span>
            <Select value={operator} onValueChange={(v) => setDraft((d) => ({ ...(d as SingleColorRule), operator: v as FormattingOperator }))}>
              <SelectTrigger aria-label="Operator" className="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                {ops.map((op) => <SelectItem key={op} value={op}>{OPERATOR_LABELS[op]}</SelectItem>)}
              </SelectContent>
            </Select>
          </label>
          {!NO_VALUE_OPS.has(operator) && (
            <label className="block space-y-1">
              <span className="text-xs text-muted-foreground">Value</span>
              <Input className="w-full" value={String((draft as SingleColorRule).value ?? "")}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), value: e.target.value }))} />
            </label>
          )}
          <div className="space-y-1">
            <span className="text-xs text-muted-foreground">Color</span>
            <div className="flex gap-1 pt-0.5">
              {PALETTE.map((c) => (
                <button key={c} type="button" aria-label={`color ${c}`} onClick={() => setDraft((d) => ({ ...(d as SingleColorRule), color: c }))}
                  // `ring-offset-background`: the offset is the 1px gap between the swatch and
                  // its selection ring, and an uncoloured offset fills that gap with white. On a
                  // swatch the damage is worse than on a focus ring — a white hairline drawn around
                  // a COLOUR reads as part of the colour, so in dark mode the selected swatch looked
                  // like a paler shade than the one it stands for.
                  className={`h-6 w-6 rounded ${(draft as SingleColorRule).color === c ? "ring-2 ring-primary ring-offset-1 ring-offset-background" : ""}`} style={{ backgroundColor: c }} />
              ))}
            </div>
          </div>
          {!isPivot && (
            <div className="flex items-center justify-between pt-1">
              <Label htmlFor="rule-highlight-row" className="text-sm font-normal">Highlight whole row</Label>
              <Switch id="rule-highlight-row" checked={!!(draft as SingleColorRule).highlight_row}
                onCheckedChange={(v) => setDraft((d) => ({ ...(d as SingleColorRule), highlight_row: v }))} />
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-2">
          <span className="text-xs text-muted-foreground">Color range</span>
          {/* preset gradients */}
          <div className="flex flex-wrap gap-1.5 pt-0.5">
            {RANGE_PRESETS.map((p) => {
              const active = start.toLowerCase() === p.colors[0] && end.toLowerCase() === p.colors[1];
              return (
                <button key={p.label} type="button" aria-label={p.label} title={p.label}
                  onClick={() => setDraft((d) => ({ ...(d as ColorRangeRule), colors: [...p.colors] }))}
                  // Same uncoloured-offset fix as the single-colour swatches above.
                  className={`h-6 w-14 rounded border ${active ? "ring-2 ring-primary ring-offset-1 ring-offset-background" : ""}`}
                  style={{ background: `linear-gradient(to right, ${p.colors[0]}, ${p.colors[1]})` }} />
              );
            })}
          </div>
          {/* live preview + custom endpoints */}
          <div className="space-y-1.5 pt-1">
            <div className="h-2.5 w-full rounded border" style={{ background: `linear-gradient(to right, ${start}, ${end})` }} />
            <div className="flex items-center gap-2">
              <ColorPicker label="start color" value={start} onChange={(next) => setRangeColor(0, next)} />
              <span className="flex-1 text-center text-xs text-muted-foreground">low → high</span>
              <ColorPicker label="end color" value={end} onChange={(next) => setRangeColor(1, next)} />
            </div>
          </div>
          {/* The colours say what the gradient looks like; these say what it MEANS. Both endpoints,
              not just a custom minimum: a range pinned at one end and floating at the other is the
              case that reads as a bug on a dashboard. */}
          {endpointField("min")}
          {endpointField("max")}
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <Button type="button" variant="outline" size="sm" onClick={onCancel}>Cancel</Button>
        <Button type="button" size="sm" disabled={draft.columns.length === 0} onClick={save}>Save rule</Button>
      </div>
    </div>
  );
}
