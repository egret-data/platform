import { useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/**
 * Number-format presets, Metabase-style: pick a common option (Currency,
 * Percent, …) with a live example, or drop to "Custom" for any d3-format
 * string. The stored value IS the d3-format string the backend keeps.
 */
const PRESETS: { label: string; example: string; value: string }[] = [
  { label: "Plain", example: "1234.5", value: "" },
  { label: "Number", example: "1,234.5", value: "," },
  { label: "Integer", example: "1,235", value: ",d" },
  { label: "2 decimals", example: "1,234.50", value: ",.2f" },
  { label: "Percent", example: "12.3%", value: ".1%" },
  { label: "Currency", example: "$1,234.50", value: "$,.2f" },
  { label: "Compact", example: "1.2k", value: ".2s" },
  { label: "Scientific", example: "1.2e+3", value: ".2e" },
];

const isPreset = (v: string) => PRESETS.some((p) => p.value === v);

/** Friendly label for a stored format string (shown on the trigger). */
export function formatLabel(value: string): string {
  const p = PRESETS.find((x) => x.value === value);
  return p ? p.label : "Custom";
}

export function FormatPicker({ value, onChange, className }: {
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [showCustom, setShowCustom] = useState(false);
  const [draft, setDraft] = useState(value);

  function openChange(o: boolean) {
    if (o) {
      setDraft(value);
      setShowCustom(!isPreset(value)); // a non-preset value means it was custom
    } else if (showCustom && draft !== value) {
      onChange(draft); // commit custom edits once, on close
    }
    setOpen(o);
  }
  function pick(v: string) {
    onChange(v);
    setShowCustom(false);
    setOpen(false);
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
      <PopoverContent align="start" className="w-60 p-1">
        <div className="max-h-72 overflow-auto">
          {PRESETS.map((p) => (
            <button key={p.value || "plain"} type="button" onClick={() => pick(p.value)}
              className="flex w-full items-center justify-between gap-3 rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
              <span className="flex items-center gap-2">
                <Check className={cn("h-3.5 w-3.5", !showCustom && p.value === value ? "opacity-100" : "opacity-0")} />
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
            <div className="space-y-1 px-2 pb-1 pt-1.5">
              <Input autoFocus value={draft} onChange={(e) => setDraft(e.target.value)}
                placeholder="e.g. $,.2f" className="h-7 font-mono text-xs" />
              <p className="text-[11px] text-muted-foreground">
                Any d3-format string. Applied when you close this.
              </p>
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
