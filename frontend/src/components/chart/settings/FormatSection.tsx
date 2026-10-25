import { useState, useEffect } from "react";
import { Hash, Calendar, Type, ChevronDown, type LucideIcon } from "lucide-react";
import { isNumericType, isTemporalType, type ColumnFormat } from "../format-types";
import { isIdentifierColumn } from "../format";
import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
import { CurrencySelect } from "./CurrencySelect";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

export function colIcon(type: string): LucideIcon {
  if (isNumericType(type)) return Hash;
  if (isTemporalType(type)) return Calendar;
  return Type;
}

export function colKind(type: string): string {
  if (isNumericType(type)) return "Number";
  if (isTemporalType(type)) return "Date";
  return "Text";
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function ToggleRow({ id, label, checked, onChange }: {
  id: string; label: string; checked: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function FormatSection({
  columns, value, onChange, allowMiniBar = true, allowViewAs = true,
}: {
  columns: ResultColumn[];
  value: Record<string, ColumnFormat>;
  onChange: (v: Record<string, ColumnFormat>) => void;
  /** Only the flat table draws mini bars. The pivot has no renderer for `show_mini_bar`, so the
   * switch sat there doing nothing — offering a setting that cannot take effect is worse than
   * not offering it. */
  allowMiniBar?: boolean;
  /** The same gate, for the same reason — see `ColumnFormatFields`. */
  allowViewAs?: boolean;
}) {
  // which column's settings are expanded (accordion); null = all collapsed
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    // collapse a stale selection if its column disappears
    if (open && !columns.find((c) => c.name === open)) setOpen(null);
  }, [columns, open]);

  const fmt = (open && value[open]) || {};
  function patch(p: Partial<ColumnFormat>) {
    if (!open) return;
    onChange({ ...value, [open]: { ...fmt, ...p } });
  }

  return (
    <div className="space-y-1 text-sm">
      {columns.map((c) => {
        const Icon = colIcon(c.type);
        const isOpen = open === c.name;
        return (
          <div key={c.name} className="overflow-hidden rounded-md border">
            <button
              type="button"
              onClick={() => setOpen(isOpen ? null : c.name)}
              className={`flex h-10 w-full items-center gap-2 px-2 text-left ${isOpen ? "bg-muted/40" : "hover:bg-muted/30"}`}
            >
              <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span className="flex-1 truncate">
                {value[c.name]?.column_title || c.name}
                {value[c.name]?.column_title && (
                  <span className="ml-1.5 text-xs text-muted-foreground">{c.name}</span>
                )}
              </span>
              <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">{colKind(c.type)}</span>
              <ChevronDown className={`h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform ${isOpen ? "" : "-rotate-90"}`} />
            </button>

            {isOpen && (
              <div className="space-y-3 border-t p-3">
                <ColumnFormatFields column={c} fmt={fmt} onPatch={patch}
                  allowMiniBar={allowMiniBar} allowViewAs={allowViewAs} />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

/**
 * ONE COLUMN'S formatting fields, with no chrome of their own.
 *
 * Extracted from `FormatSection`'s accordion body so that the table's merged column list can open
 * the SAME editor inside its per-column drill-in (`ColumnSettingsPopover`). Copying it would have
 * left two editors over one `column_settings` key, and this file already records what that costs:
 * `scale` and `number_separators` were honoured by `formatValue` and had no control anywhere for
 * as long as nobody was looking at the one list that could have grown them.
 *
 * It takes the resolved `fmt` and a `patch` rather than the whole map, because both callers
 * already hold "the column being edited" — the accordion in its `open` state, the popover in the
 * row it was opened from. Neither can address the wrong column by construction.
 */
export function ColumnFormatFields({
  column: c, fmt, onPatch: patch, allowMiniBar = true, allowViewAs = true,
}: {
  column: ResultColumn;
  fmt: ColumnFormat;
  onPatch: (p: Partial<ColumnFormat>) => void;
  /** Only the flat table draws mini bars — see FormatSection. */
  allowMiniBar?: boolean;
  /**
   * Whether to offer "Display as" and its two link templates.
   *
   * ONLY THE FLAT TABLE READS THEM. `ChartTable.renderTableCell` is the single place in the app
   * that branches on `view_as`; everything else turns a cell value into text through `formatValue`,
   * which does not consult it — `format.ts`'s `CONSULTED_BY_FORMAT_DATE` says so in as many words
   * (`view_as: false, link_text: false, link_url: false`) and is `satisfies`-checked so it cannot
   * go stale. So a pivot row-dimension, a cartesian x-axis, a pie dimension, a sankey node column
   * and an object-detail field were all offering a Display-as → Link select and two template boxes
   * that saved and painted nothing.
   *
   * THE SAME GATE AS `allowMiniBar`, deliberately, rather than a second mechanism: `show_mini_bar`
   * had exactly this defect and was closed exactly this way, and two shapes of "this control is
   * table-only" in one component is one more than the next reader can hold.
   *
   * VERIFIED BY RUNNING, not by reading. A probe rendered all nineteen viz types through
   * `ChartRenderer` with these three fields behind recording getters: `table` was the only type
   * whose access stacks reached a CONSUMING read. Every other touch was an object spread into a
   * `ColumnFormat` bound for `formatValue` (`cartesian.ts`'s `{...col, compact}`, `scalar.ts`'s
   * `compactifyValue`, `boxplot.ts`'s `fmt`) or a `JSON.stringify` memo key — copies, not reads.
   * `__tests__/viewAsIsTableOnly.test.tsx` is what keeps that true.
   *
   * ALREADY-STORED VALUES ARE LEFT ALONE. A pie dimension that was given `view_as: "link"` before
   * this gate keeps it in `column_settings`; nothing outside the table ever read it, so it paints
   * exactly as it did, and stripping it would mean a migration that could only ever remove a value
   * whose sole effect is to occupy a key. It becomes reachable again the moment that column is
   * shown in a table, which is the one place it means something.
   */
  allowViewAs?: boolean;
}) {
  return (
    <>
        <Field label="Column title">
          <Input className="w-full"
           
            placeholder={c.name}
            value={fmt.column_title ?? ""}
            onChange={(e) =>
              // Blank means "use the column's own name", so a title of only spaces stores
              // nothing. The RAW value is kept otherwise — trimming on every keystroke
              // would eat the space in "Rev Total" as the user typed it.
              patch({ column_title: e.target.value.trim() ? e.target.value : undefined })
            }
          />
        </Field>

        {isNumericType(c.type) && (
          <div className="space-y-3">
            <Field label="Number style">
              <Select value={fmt.number_style ?? "decimal"}
                onValueChange={(v) => patch({ number_style: v as ColumnFormat["number_style"] })}>
                <SelectTrigger aria-label="Number style" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="decimal">Normal</SelectItem>
                  <SelectItem value="percent">Percent</SelectItem>
                  <SelectItem value="currency">Currency</SelectItem>
                  <SelectItem value="scientific">Scientific</SelectItem>
                </SelectContent>
              </Select>
            </Field>

            {fmt.number_style === "currency" && (
              <div className="flex gap-2">
                {/* A PICKER, NOT A TEXT BOX — see `CurrencySelect`, and `currencies.ts` for where
                    the list comes from and why it is vendored rather than taken from `Intl`. The
                    box this replaces was pre-filled `USD` and upper-cased whatever was typed, so
                    it accepted `VNND` and `dong` alike; `formatNumber` then hands the string to
                    `Intl.NumberFormat` as `opts.currency`, which throws a RangeError on an unknown
                    code — during the option build, which is the failure mode `safeDecimals` in the
                    same file records as replacing the whole card with an error message.

                    NOT a `Field`: `Field` wraps its child in a `<label>`, and a `<label>` around a
                    combobox trigger makes the click on the label re-fire on the button, which
                    toggles the popover shut the moment it opens. The name comes from the control's
                    own `aria-label` instead, which is what every other non-native control in this
                    panel does. */}
                <div className="block min-w-0 flex-1 space-y-1">
                  <span className="text-xs text-muted-foreground">Currency</span>
                  <CurrencySelect value={fmt.currency} onChange={(currency) => patch({ currency })} />
                </div>
                <Field label="Display">
                  <Select value={fmt.currency_style ?? "symbol"}
                    onValueChange={(v) => patch({ currency_style: v as ColumnFormat["currency_style"] })}>
                    <SelectTrigger aria-label="Currency display" className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="symbol">$</SelectItem>
                      <SelectItem value="code">USD</SelectItem>
                      <SelectItem value="name">dollars</SelectItem>
                      <SelectItem value="narrowSymbol">$ (narrow)</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
              </div>
            )}

            {/* CLAMPED IN THE HANDLER, not by `min`/`max`. `formatValue` passes this
                straight to `Intl.NumberFormat` as both `minimumFractionDigits` and
                `maximumFractionDigits`, which throw a RangeError outside the engine's own
                fraction-digit range — and the
                throw lands during the OPTION BUILD (`baseOption` formats the widest axis
                tick to size the gutter), so a typed `-1` did not draw a wrong number, it
                replaced the whole card with `ChartErrorBoundary`'s message, and did so
                again on every load of a chart saved that way.

                `min={0}` had been here all along and stopped none of it: PR #398 measured
                in a real Chrome probe that `min`/`max`/`step` only set `validity.*` flags
                at form submission and block no keystroke. They stay because the spinner
                arrows and the wheel DO respect them, so the element now agrees with its
                own handler instead of contradicting it.

                The bounds and the clamp are the pie's, copied rather than re-derived
                (`PieDisplaySection`'s Decimal places, which has done this since it was
                written): 0..10, truncating, and an empty box still means Auto. Ten rather
                than `format.ts`'s own 20 because this is a Metabase-parity precision box,
                not an arbitrary-precision one — `format.ts` allows the full 0..20 for a
                value stored by anything else, so the ceiling here narrows what the panel
                offers without making a legal saved value unrenderable. (20, not Intl's
                nominal 100: that upper bound is an ES2023 addition and older engines still
                throw above 20. See `safeDecimals`.) */}
            <Field label="Decimal places">
              <Input className="w-full" type="number" min={0} max={10} step={1} placeholder="Auto"
                value={fmt.decimals ?? ""}
                onChange={(e) => {
                  const raw = e.target.value;
                  if (raw === "") return patch({ decimals: undefined });
                  const n = Number(raw);
                  patch({ decimals: Number.isNaN(n) ? undefined : Math.min(10, Math.max(0, Math.trunc(n))) });
                }} />
            </Field>

            {/* `scale` and `number_separators` were both honoured by `formatValue` and had
                no control anywhere — implemented and unreachable, the same shape of defect
                that cost the pie four settings. Metabase offers both. */}
            <Field label="Separator style">
              {/* With nothing stored, the style the column is drawn with: none for an identifier
                  such as `orderkey` (KI-058), grouped thousands otherwise. */}
              <Select value={fmt.number_separators ?? (isIdentifierColumn(c.name, c.type ?? "") ? "." : ".,")}
                onValueChange={(v) => patch({ number_separators: v })}>
                <SelectTrigger aria-label="Separator style" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value=".,">100,000.00</SelectItem>
                  <SelectItem value=", ">100 000,00</SelectItem>
                  <SelectItem value=",.">100.000,00</SelectItem>
                  {/* The two Metabase offers that egret did not, and the reason the gap mattered:
                      there was NO WAY to render a number with no thousands separator. The stored
                      string is "[decimal][thousands]", so "no separator" is a ONE-character value
                      whose missing second character `applySeparators` reads as the empty string.

                      The apostrophe is U+2019 (’), which is what Metabase's option is — `".’"`.
                      An ASCII `'` would draw identically and be a different stored string, so a
                      chart moved between the two apps would silently lose the setting. */}
                  <SelectItem value=".">100000.00</SelectItem>
                  <SelectItem value=".’">100’000.00</SelectItem>
                </SelectContent>
              </Select>
            </Field>

            {/* DecimalInput. A scale below 1 is the everyday use of this field — 0.001 to
                show thousands as units — and its leading "0." is exactly the state a
                `type="number"` box cannot hold: the browser reports it EMPTY, which arrived
                here as `undefined` and dropped the point. See DecimalInput. */}
            <Field label="Multiply by a number">
              <DecimalInput placeholder="1"
                value={fmt.scale}
                onChange={(scale) => patch({ scale })} />
            </Field>

            <div className="flex gap-2">
              <Field label="Prefix">
                <Input className="w-full" value={fmt.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} />
              </Field>
              <Field label="Suffix">
                <Input className="w-full" value={fmt.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} />
              </Field>
            </div>

            <div className="space-y-2.5 pt-1">
              <ToggleRow id={`fmt-compact-${c.name}`} label="Compact (1.2K)" checked={!!fmt.compact} onChange={(v) => patch({ compact: v })} />
              <ToggleRow id={`fmt-negatives-${c.name}`} label="Negatives in ( )" checked={!!fmt.negative_in_parentheses} onChange={(v) => patch({ negative_in_parentheses: v })} />
              {allowMiniBar && (
                <ToggleRow id={`fmt-minibar-${c.name}`} label="Show a mini bar chart" checked={!!fmt.show_mini_bar} onChange={(v) => patch({ show_mini_bar: v })} />
              )}
            </div>
          </div>
        )}

        {isTemporalType(c.type) && (
          <div className="space-y-3">
            <Field label="Date style">
              <Select value={fmt.date_style ?? "MMMM D, YYYY"} onValueChange={(v) => patch({ date_style: v })}>
                <SelectTrigger aria-label="Date style" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="MMMM D, YYYY">January 31, 2024</SelectItem>
                  <SelectItem value="M/D/YYYY">1/31/2024</SelectItem>
                  <SelectItem value="D/M/YYYY">31/1/2024</SelectItem>
                  <SelectItem value="YYYY-MM-DD">2024-01-31</SelectItem>
                  {/* Bucket-shaped styles. A pivot grouped by month arrives as the first
                      of the month, so a full date reads "1/1/2026" and the day is noise —
                      Metabase labels the same bucket "January 2026" / "Q1 2026". */}
                  <SelectItem value="MMMM YYYY">January 2024</SelectItem>
                  <SelectItem value="[Q]Q YYYY">Q1 2024</SelectItem>
                  <SelectItem value="YYYY">2024</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <ToggleRow id={`fmt-abbreviate-${c.name}`} label="Abbreviate" checked={!!fmt.date_abbreviate} onChange={(v) => patch({ date_abbreviate: v })} />

            {/* THE SEPARATOR, ONLY WHERE THERE IS ONE TO REPLACE. `formatDate` applies this as
                `dateFmt.replace(/\//g, fmt.date_separator)` — so on "January 31, 2024" or "Q1 2024"
                it has nothing to act on and the control would save a value that changes no pixel.
                Gated on the CHOSEN style rather than on a stored value, so an older chart carrying
                a separator under an unslashed style does not resurrect it.

                Live Metabase (:3033) renders this unconditionally; egret deliberately does not, for
                the reason the rest of this PR exists. The three options are Metabase's. */}
            {(fmt.date_style ?? "MMMM D, YYYY").includes("/") && (
              <Field label="Date separator">
                <Select value={fmt.date_separator ?? "/"}
                  onValueChange={(v) => patch({ date_separator: v as ColumnFormat["date_separator"] })}>
                  <SelectTrigger aria-label="Date separator" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="/">2024/01/31</SelectItem>
                    <SelectItem value="-">2024-01-31</SelectItem>
                    <SelectItem value=".">2024.01.31</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
            )}

            {/* `time_enabled` is a BUCKET, not a switch: it says how much of the time to show, and
                `formatDate` widens `h:mm` to `h:mm:ss` / `h:mm:ss.SSS` from it. Metabase's own four
                options and their wording.

                "Off" writes `null`, not `undefined`. `null` is what the type declares for "no time"
                and what `specifiesDateRendering` already treats as silence; `undefined` would leave
                the key present-and-absent, which is the state `format.ts` records as having cost an
                axis its bucket label once already. */}
            <Field label="Show the time">
              <Select value={fmt.time_enabled ?? "off"}
                onValueChange={(v) => patch({ time_enabled: v === "off" ? null : (v as ColumnFormat["time_enabled"]) })}>
                <SelectTrigger aria-label="Show the time" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="off">Off</SelectItem>
                  <SelectItem value="minutes">HH:MM</SelectItem>
                  <SelectItem value="seconds">HH:MM:SS</SelectItem>
                  <SelectItem value="milliseconds">HH:MM:SS.MS</SelectItem>
                </SelectContent>
              </Select>
            </Field>

            {/* `formatDate` reads `time_style` ONLY inside `if (fmt.time_enabled)`, so with the time
                off this is a 12-versus-24-hour choice about a clock that is not drawn. Measured on
                live Metabase, which shows it anyway; not copied, for the same reason as above. */}
            {fmt.time_enabled && (
              <Field label="Time style">
                <Select value={fmt.time_style ?? "h:mm A"}
                  onValueChange={(v) => patch({ time_style: v as ColumnFormat["time_style"] })}>
                  <SelectTrigger aria-label="Time style" className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="h:mm A">5:24 PM (12-hour clock)</SelectItem>
                    <SelectItem value="HH:mm">17:24 (24-hour clock)</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
            )}
          </div>
        )}

        {allowViewAs && !isNumericType(c.type) && !isTemporalType(c.type) && (
          <div className="space-y-3">
            <Field label="Display as">
              <Select value={fmt.view_as ?? "auto"}
                onValueChange={(v) => patch({ view_as: v as ColumnFormat["view_as"] })}>
                <SelectTrigger aria-label="Display as" className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="auto">Automatic</SelectItem>
                  <SelectItem value="text">Text</SelectItem>
                  <SelectItem value="link">Link</SelectItem>
                  <SelectItem value="email">Email</SelectItem>
                  <SelectItem value="image">Image</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            {fmt.view_as === "link" && (
              <>
                <Field label="Link text">
                  <Input className="w-full" placeholder="{{value}}" value={fmt.link_text ?? ""}
                    onChange={(e) => patch({ link_text: e.target.value || undefined })} />
                </Field>
                <Field label="Link URL">
                  <Input className="w-full" placeholder="{{value}}" value={fmt.link_url ?? ""}
                    onChange={(e) => patch({ link_url: e.target.value || undefined })} />
                </Field>
              </>
            )}
          </div>
        )}
    </>
  );
}
