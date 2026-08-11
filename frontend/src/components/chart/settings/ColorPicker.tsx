import { Check } from "lucide-react";

import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

/**
 * A SWATCH PICKER over the chart palette, with the OS colour dialog kept as the way out.
 *
 * WHAT IT REPLACES. Every colour in this panel was a bare `<input type="color">`, so choosing a
 * series colour opened the operating system's picker — a gradient field and three RGB boxes — for
 * a decision whose answer is almost always "one of the eight colours this app already draws in".
 * Metabase offers those eight as swatches and nothing else on the first click; picking a chart
 * colour there is one click, and here it was: open a modal dialog, read 242/168/111, guess.
 *
 * THE EIGHT ARE ALREADY OURS. `SERIES_PALETTE` is Metabase's `accent-colors.ts` lifted verbatim,
 * and it is what `colorsForKeys` assigns from — so these are not "suggested" colours, they are the
 * ones the chart picks between when the author says nothing. Offering anything else here would
 * have made the palette and the picker two different lists, which is the defect this whole
 * programme has been removing everywhere else.
 *
 * CUSTOM STAYS, one click further in. The native input is exactly right for "I need our brand
 * teal" and exactly wrong as the default path; it is the second row, not the first.
 */
export function ColorPicker({ value, onChange, label, className }: {
  value: string;
  onChange: (next: string) => void;
  /** Accessible name for the trigger — a row with four controls needs to say which is which. */
  label: string;
  className?: string;
}) {
  // `<input type="color">` normalises to lower case (HTML spec) and the palette is written upper,
  // so a stored value that CAME from a swatch would not match it. Compared case-insensitively for
  // that reason, and only for the tick — nothing here rewrites what is stored.
  const isActive = (hex: string) => value?.toLowerCase() === hex.toLowerCase();

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={label}
          className={`size-6 shrink-0 cursor-pointer rounded border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${className ?? ""}`}
          style={{ backgroundColor: value }}
        />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-auto p-2">
        <div className="grid grid-cols-4 gap-1.5">
          {SERIES_PALETTE.map((hex) => (
            <button
              key={hex}
              type="button"
              // The hex IS the name. A screen-reader user choosing between eight swatches needs
              // them told apart, and "colour 3" tells them nothing they can act on.
              aria-label={hex}
              aria-pressed={isActive(hex)}
              onClick={() => onChange(hex)}
              className="flex size-7 items-center justify-center rounded border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              style={{ backgroundColor: hex }}
            >
              {isActive(hex) && <Check className="h-3.5 w-3.5 text-white drop-shadow" />}
            </button>
          ))}
        </div>
        <label className="mt-2 flex items-center gap-2 border-t pt-2 text-xs text-muted-foreground">
          {/* The native input, kept and demoted. `aria-label` names it "custom …" rather than
              repeating the trigger's label, so the two are distinguishable to a screen reader. */}
          <input
            type="color"
            aria-label={`custom ${label}`}
            className="size-6 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
          Custom…
        </label>
      </PopoverContent>
    </Popover>
  );
}
