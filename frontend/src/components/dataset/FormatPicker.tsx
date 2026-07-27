import { useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ColumnFormat } from "@/components/chart/format-types";
import { cn } from "@/lib/utils";

/**
 * Number-format picker, Metabase-style. The stored `value` is a JSON string of
 * the app's canonical `ColumnFormat` (the same model the chart/table renderer
 * consumes via formatValue) — so a format set here actually changes how values
 * display, not just a label. Empty string means "no format" (engine default).
 */
const PRESETS: { label: string; example: string; cf: ColumnFormat }[] = [
  { label: "Default", example: "1,234.5", cf: {} },
  { label: "Integer", example: "1,235", cf: { decimals: 0 } },
  { label: "1 decimal", example: "1,234.5", cf: { decimals: 1 } },
  { label: "2 decimals", example: "1,234.50", cf: { decimals: 2 } },
  { label: "Percent", example: "12.3%", cf: { number_style: "percent", decimals: 1 } },
  { label: "Currency", example: "$1,234.50", cf: { number_style: "currency", decimals: 2 } },
  { label: "Compact", example: "1.2K", cf: { compact: true } },
  { label: "Scientific", example: "1.23e+3", cf: { number_style: "scientific", decimals: 2 } },
];

/** Parse a stored format string into a ColumnFormat (empty → {}). */
export function parseFormat(value: string): ColumnFormat {
  if (!value) return {};
  try {
    const v = JSON.parse(value);
    return v && typeof v === "object" ? (v as ColumnFormat) : {};
  } catch {
    return {}; // tolerate legacy/garbage values as "no format"
  }
}

/** Canonical serialization (sorted keys, drops empties) for storage + matching. */
function serialize(cf: ColumnFormat): string {
  const entries = Object.entries(cf)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .sort(([a], [b]) => a.localeCompare(b));
  return entries.length ? JSON.stringify(Object.fromEntries(entries)) : "";
}

/** Friendly label for a stored format string (shown on the trigger). */
export function formatLabel(value: string): string {
  const canon = serialize(parseFormat(value));
  if (canon === "") return "Default";
  const p = PRESETS.find((x) => serialize(x.cf) === canon);
  return p ? p.label : "Custom";
}

export function FormatPicker({ value, onChange, className }: {
  value: string;            // JSON of ColumnFormat, or "" for none
  onChange: (v: string) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [showCustom, setShowCustom] = useState(false);
  const [draft, setDraft] = useState<ColumnFormat>({});

  const canon = serialize(parseFormat(value));
  const matchedPreset = PRESETS.find((p) => serialize(p.cf) === canon);

  function openChange(o: boolean) {
    if (o) {
      setDraft(parseFormat(value));
      setShowCustom(!matchedPreset && canon !== ""); // a non-preset value was custom
    } else if (showCustom) {
      const next = serialize(draft);
      if (next !== canon) onChange(next); // commit custom edits once, on close
    }
    setOpen(o);
  }
  function pick(cf: ColumnFormat) {
    onChange(serialize(cf));
    setShowCustom(false);
    setOpen(false);
  }
  function patchDraft(p: Partial<ColumnFormat>) {
    setDraft((d) => ({ ...d, ...p }));
  }

  return (
    <Popover open={open} onOpenChange={openChange}>
      <PopoverTrigger asChild>
        <button type="button"
          className={cn("flex h-7 w-28 items-center justify-between gap-1 rounded border px-2 text-xs hover:bg-accent", className)}
          aria-label="Number format">
          <span className="truncate">{formatLabel(value)}</span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 p-1">
        <div className="max-h-80 overflow-auto">
          {PRESETS.map((p) => (
            <button key={p.label} type="button" onClick={() => pick(p.cf)}
              className="flex w-full items-center justify-between gap-3 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
              <span className="flex items-center gap-2">
                <Check className={cn("h-3.5 w-3.5", !showCustom && matchedPreset?.label === p.label ? "opacity-100" : "opacity-0")} />
                {p.label}
              </span>
              <span className="font-mono text-xs text-muted-foreground">{p.example}</span>
            </button>
          ))}
          <div className="my-1 border-t" />
          <button type="button" onClick={() => setShowCustom(true)}
            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
            <Check className={cn("h-3.5 w-3.5", showCustom ? "opacity-100" : "opacity-0")} />
            Custom…
          </button>
          {showCustom && (
            <div className="space-y-2 px-2 pb-1.5 pt-2">
              <div className="space-y-1">
                <Label className="text-xs" htmlFor="fmt-dec">Decimal places</Label>
                <Input id="fmt-dec" type="number" min={0} max={10} className="h-7 text-xs"
                  value={draft.decimals ?? ""} placeholder="auto"
                  onChange={(e) => patchDraft({ decimals: e.target.value === "" ? undefined : Number(e.target.value) })} />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <Label className="text-xs" htmlFor="fmt-pre">Prefix</Label>
                  <Input id="fmt-pre" className="h-7 text-xs" value={draft.prefix ?? ""} placeholder="€"
                    onChange={(e) => patchDraft({ prefix: e.target.value })} />
                </div>
                <div className="space-y-1">
                  <Label className="text-xs" htmlFor="fmt-suf">Suffix</Label>
                  <Input id="fmt-suf" className="h-7 text-xs" value={draft.suffix ?? ""} placeholder=" kg"
                    onChange={(e) => patchDraft({ suffix: e.target.value })} />
                </div>
              </div>
              <p className="text-[11px] text-muted-foreground">Applied when you close this.</p>
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
