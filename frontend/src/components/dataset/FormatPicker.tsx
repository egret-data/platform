import { useId, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { isNumericType, isTemporalType, type ColumnFormat } from "@/components/chart/format-types";
import { cn } from "@/lib/utils";

/**
 * Format picker, Metabase-style. Stored `value` is a JSON string of the app's
 * canonical `ColumnFormat` (the model the chart/table renderer consumes) — so a
 * format set here actually changes how values display. Sections adapt to the
 * column's type (number / date / text). All edits are committed once, on close.
 */
const NUMBER_PRESETS: { label: string; example: string; cf: ColumnFormat }[] = [
  { label: "Default", example: "1,234.5", cf: {} },
  { label: "Integer", example: "1,235", cf: { decimals: 0 } },
  { label: "1 decimal", example: "1,234.5", cf: { decimals: 1 } },
  { label: "2 decimals", example: "1,234.50", cf: { decimals: 2 } },
  { label: "Percent", example: "12.3%", cf: { number_style: "percent", decimals: 1 } },
  { label: "Currency", example: "$1,234.50", cf: { number_style: "currency", decimals: 2 } },
  { label: "Compact", example: "1.2K", cf: { compact: true } },
  { label: "Scientific", example: "1.23e+3", cf: { number_style: "scientific", decimals: 2 } },
];

const DATE_PRESETS: { label: string; date_style: string }[] = [
  { label: "January 5, 2026", date_style: "MMMM D, YYYY" },
  { label: "Jan 5, 2026", date_style: "MMM D, YYYY" },
  { label: "1/5/2026", date_style: "M/D/YYYY" },
  { label: "2026-01-05", date_style: "YYYY-MM-DD" },
  { label: "January 2026", date_style: "MMMM YYYY" },
  { label: "Jan 5", date_style: "MMM D" },
];

const CURRENCIES = ["USD", "EUR", "GBP", "JPY", "CNY", "VND", "KRW", "INR", "AUD", "CAD", "SGD", "THB"];

const VIEW_AS: { label: string; value: NonNullable<ColumnFormat["view_as"]> }[] = [
  { label: "Text", value: "text" },
  { label: "Link", value: "link" },
  { label: "Email", value: "email" },
  { label: "Image", value: "image" },
];

export function parseFormat(value: string): ColumnFormat {
  if (!value) return {};
  try {
    const v = JSON.parse(value);
    return v && typeof v === "object" ? (v as ColumnFormat) : {};
  } catch {
    return {};
  }
}

function serialize(cf: ColumnFormat): string {
  const entries = Object.entries(cf)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .sort(([a], [b]) => a.localeCompare(b));
  return entries.length ? JSON.stringify(Object.fromEntries(entries)) : "";
}

/** Friendly label for the trigger. */
export function formatLabel(value: string): string {
  const cf = parseFormat(value);
  if (cf.number_style === "currency") return "Currency";
  if (cf.number_style === "percent") return "Percent";
  if (cf.date_style) return "Date";
  if (cf.view_as && cf.view_as !== "text" && cf.view_as !== "auto") return VIEW_AS.find((v) => v.value === cf.view_as)?.label ?? "Custom";
  const p = NUMBER_PRESETS.find((x) => serialize(x.cf) === serialize(cf));
  if (p) return p.label;
  return serialize(cf) === "" ? "Default" : "Custom";
}

function numberShapeMatches(a: ColumnFormat, b: ColumnFormat): boolean {
  return (a.number_style ?? "") === (b.number_style ?? "") &&
    (a.decimals ?? -1) === (b.decimals ?? -1) &&
    !!a.compact === !!b.compact;
}

export function FormatPicker({ value, onChange, type = "", className }: {
  value: string;
  onChange: (v: string) => void;
  type?: string;         // column data_type, to choose number/date/text sections
  className?: string;
}) {
  const kind = isTemporalType(type) ? "date" : type && !isNumericType(type) ? "text" : "number";
  const [open, setOpen] = useState(false);
  const currencyId = useId();
  const [draft, setDraft] = useState<ColumnFormat>({});

  function openChange(o: boolean) {
    if (o) setDraft(parseFormat(value));
    else if (serialize(draft) !== serialize(parseFormat(value))) onChange(serialize(draft));
    setOpen(o);
  }
  const patch = (p: Partial<ColumnFormat>) => setDraft((d) => ({ ...d, ...p }));

  return (
    <Popover open={open} onOpenChange={openChange}>
      <PopoverTrigger asChild>
        <button type="button"
          className={cn("flex h-7 w-28 items-center justify-between gap-1 rounded border px-2 text-xs hover:bg-accent", className)}
          aria-label="Format">
          <span className="truncate">{formatLabel(value)}</span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 p-1">
        <div className="max-h-96 space-y-1 overflow-auto">
          {kind === "number" && (
            <>
              {NUMBER_PRESETS.map((p) => (
                <button key={p.label} type="button"
                  onClick={() => setDraft(p.cf.number_style === "currency" ? { ...p.cf, currency: draft.currency ?? "USD" } : { ...p.cf })}
                  className="flex w-full items-center justify-between gap-3 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
                  <span className="flex items-center gap-2">
                    <Check className={cn("h-3.5 w-3.5", numberShapeMatches(draft, p.cf) ? "opacity-100" : "opacity-0")} />
                    {p.label}
                  </span>
                  <span className="font-mono text-xs text-muted-foreground">{p.example}</span>
                </button>
              ))}
              {draft.number_style === "currency" && (
                <div className="px-2 pt-1">
                  <Label htmlFor={currencyId} className="text-xs">Currency</Label>
                  <Select value={draft.currency ?? "USD"} onValueChange={(v) => patch({ currency: v })}>
                    <SelectTrigger id={currencyId} size="sm" className="mt-1 w-full px-2 text-xs"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      {CURRENCIES.map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              )}
              <div className="grid grid-cols-3 gap-1.5 px-2 pt-1.5">
                <div className="space-y-1">
                  <Label className="text-[11px]">Decimals</Label>
                  <Input type="number" min={0} max={10} className="h-7 px-1 text-xs" placeholder="auto"
                    value={draft.decimals ?? ""} onChange={(e) => patch({ decimals: e.target.value === "" ? undefined : Number(e.target.value) })} />
                </div>
                <div className="space-y-1">
                  <Label className="text-[11px]">Prefix</Label>
                  <Input className="h-7 px-1 text-xs" value={draft.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} />
                </div>
                <div className="space-y-1">
                  <Label className="text-[11px]">Suffix</Label>
                  <Input className="h-7 px-1 text-xs" value={draft.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} />
                </div>
              </div>
            </>
          )}

          {kind === "date" && (
            <>
              {DATE_PRESETS.map((p) => (
                <button key={p.date_style} type="button" onClick={() => patch({ date_style: p.date_style })}
                  className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
                  <Check className={cn("h-3.5 w-3.5", (draft.date_style ?? "MMMM D, YYYY") === p.date_style ? "opacity-100" : "opacity-0")} />
                  {p.label}
                </button>
              ))}
              {/* Inside a Popover, so the primitive's `focus-visible:ring-2 ring-ring` is the only
                  focus treatment that can be seen: a `ring-offset` would paint a band of
                  `--background` against the popover's own `--popover` surface. The primitive
                  already carries the right one, which is a reason to use it rather than to restyle
                  a raw input here.
                  DENSITY, measured on the kind deployment with the `order_date` popover open: the
                  last date preset is a 32px button whose bottom edge sits 8px clear of this
                  checkbox's 24×24 target, so the 4px bleed collides with nothing. The row itself is
                  26px (`pt-1.5`) and toggling it does NOT dismiss the popover — verified, because a
                  `<button>` inside `PopoverContent` is exactly the shape that could have. */}
              <label htmlFor="fp-time-enabled" className="flex cursor-pointer items-center gap-2 px-2 pt-1.5 text-sm">
                <Checkbox id="fp-time-enabled" checked={!!draft.time_enabled}
                  onCheckedChange={(v) => patch({ time_enabled: v === true ? "minutes" : null })} />
                Show time
              </label>
            </>
          )}

          {kind === "text" && (
            <>
              <div className="px-2 pb-1 pt-1 text-[11px] font-medium text-muted-foreground">Display as</div>
              {VIEW_AS.map((v) => (
                <button key={v.value} type="button" onClick={() => patch({ view_as: v.value })}
                  className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
                  <Check className={cn("h-3.5 w-3.5", (draft.view_as ?? "text") === v.value ? "opacity-100" : "opacity-0")} />
                  {v.label}
                </button>
              ))}
              <div className="grid grid-cols-2 gap-1.5 px-2 pt-1.5">
                <div className="space-y-1">
                  <Label className="text-[11px]">Prefix</Label>
                  <Input className="h-7 px-1 text-xs" value={draft.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} />
                </div>
                <div className="space-y-1">
                  <Label className="text-[11px]">Suffix</Label>
                  <Input className="h-7 px-1 text-xs" value={draft.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} />
                </div>
              </div>
            </>
          )}
          <p className="px-2 pb-0.5 pt-1 text-[11px] text-muted-foreground">Applied when you close this.</p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
