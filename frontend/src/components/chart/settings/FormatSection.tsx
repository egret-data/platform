import { useState, useEffect } from "react";
import { Hash, Calendar, Type, ChevronDown, type LucideIcon } from "lucide-react";
import { isNumericType, isTemporalType, type ColumnFormat } from "../format-types";
import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
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
  columns, value, onChange, allowMiniBar = true,
}: {
  columns: ResultColumn[];
  value: Record<string, ColumnFormat>;
  onChange: (v: Record<string, ColumnFormat>) => void;
  /** Only the flat table draws mini bars. The pivot has no renderer for `show_mini_bar`, so the
   * switch sat there doing nothing — offering a setting that cannot take effect is worse than
   * not offering it. */
  allowMiniBar?: boolean;
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
              className={`flex w-full items-center gap-2 px-2 py-1.5 text-left ${isOpen ? "bg-muted/40" : "hover:bg-muted/30"}`}
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
                <ColumnFormatFields column={c} fmt={fmt} onPatch={patch} allowMiniBar={allowMiniBar} />
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
export function ColumnFormatFields({ column: c, fmt, onPatch: patch, allowMiniBar = true }: {
  column: ResultColumn;
  fmt: ColumnFormat;
  onPatch: (p: Partial<ColumnFormat>) => void;
  /** Only the flat table draws mini bars — see FormatSection. */
  allowMiniBar?: boolean;
}) {
  return (
    <>
        <Field label="Column title">
          <Input
            className="h-8"
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
                <SelectTrigger aria-label="Number style" className="h-8"><SelectValue /></SelectTrigger>
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
                <Field label="Currency">
                  <Input className="h-8" value={fmt.currency ?? "USD"}
                    onChange={(e) => patch({ currency: e.target.value.toUpperCase() })} />
                </Field>
                <Field label="Display">
                  <Select value={fmt.currency_style ?? "symbol"}
                    onValueChange={(v) => patch({ currency_style: v as ColumnFormat["currency_style"] })}>
                    <SelectTrigger aria-label="Currency display" className="h-8"><SelectValue /></SelectTrigger>
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
                `maximumFractionDigits`, which throw a RangeError outside 0..100 — and the
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
                than Intl's own 100 because this is a Metabase-parity precision box, not an
                arbitrary-precision one — `format.ts` allows the full 0..100 for a value
                stored by anything else, so the ceiling here narrows what the panel offers
                without making a legal saved value unrenderable. */}
            <Field label="Decimal places">
              <Input type="number" min={0} max={10} step={1} className="h-8" placeholder="Auto"
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
              <Select value={fmt.number_separators ?? ".,"}
                onValueChange={(v) => patch({ number_separators: v })}>
                <SelectTrigger aria-label="Separator style" className="h-8"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value=".,">100,000.00</SelectItem>
                  <SelectItem value=", ">100 000,00</SelectItem>
                  <SelectItem value=",.">100.000,00</SelectItem>
                </SelectContent>
              </Select>
            </Field>

            {/* DecimalInput. A scale below 1 is the everyday use of this field — 0.001 to
                show thousands as units — and its leading "0." is exactly the state a
                `type="number"` box cannot hold: the browser reports it EMPTY, which arrived
                here as `undefined` and dropped the point. See DecimalInput. */}
            <Field label="Multiply by a number">
              <DecimalInput className="h-8" placeholder="1"
                value={fmt.scale}
                onChange={(scale) => patch({ scale })} />
            </Field>

            <div className="flex gap-2">
              <Field label="Prefix">
                <Input className="h-8" value={fmt.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} />
              </Field>
              <Field label="Suffix">
                <Input className="h-8" value={fmt.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} />
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
                <SelectTrigger aria-label="Date style" className="h-8"><SelectValue /></SelectTrigger>
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
          </div>
        )}

        {!isNumericType(c.type) && !isTemporalType(c.type) && (
          <div className="space-y-3">
            <Field label="Display as">
              <Select value={fmt.view_as ?? "auto"}
                onValueChange={(v) => patch({ view_as: v as ColumnFormat["view_as"] })}>
                <SelectTrigger aria-label="Display as" className="h-8"><SelectValue /></SelectTrigger>
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
                  <Input className="h-8" placeholder="{{value}}" value={fmt.link_text ?? ""}
                    onChange={(e) => patch({ link_text: e.target.value || undefined })} />
                </Field>
                <Field label="Link URL">
                  <Input className="h-8" placeholder="{{value}}" value={fmt.link_url ?? ""}
                    onChange={(e) => patch({ link_url: e.target.value || undefined })} />
                </Field>
              </>
            )}
          </div>
        )}
    </>
  );
}
