import { useId, useState } from "react";
import { Download } from "lucide-react";
import { toast } from "sonner";
import * as RadioGroupPrimitive from "@radix-ui/react-radio-group";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";
import { MAX_CHART_ROWS } from "@/api/charts";
import { chartImageSvg, downloadCsv, downloadJson, downloadPng } from "./export-chart";
import { downloadXlsx } from "./export-xlsx";

/** Metabase's three table formats in its own order (`common/types/export.ts:4-9`); `.png` is
 *  appended only when the card actually rendered a chart image (`chartImageSvg`), matching its
 *  `canSavePng`. */
const TABLE_FORMATS = ["csv", "xlsx", "json"] as const;
type ExportFormat = (typeof TABLE_FORMATS)[number] | "png";

interface Props {
  title: string | undefined;
  columns: ResultColumn[];
  rows: unknown[][];
  /** `| null` since React 19: `useRef<T>(null)` now yields `RefObject<T | null>`
   *  rather than `RefObject<T>`, so every caller's ref would otherwise be rejected. */
  containerRef: React.RefObject<HTMLElement | null>;
  disabled?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
}

/**
 * Metabase's "Download data" popover, not a flat menu: a segmented format control, the
 * "Keep the data formatted" checkbox, the row-limit note, and one Download button
 * (`QuestionDownloadWidget.tsx:157-232` + `ExportSettingsWidget.tsx:80-121`). The shape matters as
 * much as the formats — a flat menu forces the format and the formatting choice into one click and
 * has nowhere to put the note.
 */
export function ChartExportMenu({ title, columns, rows, containerRef, disabled, columnSettings }: Props) {
  // PNG availability is read from the DOM when the popover opens, so it reflects the
  // currently-rendered viz rather than whatever it was the last time this mounted.
  const [hasSvg, setHasSvg] = useState(false);
  const [format, setFormat] = useState<ExportFormat>("csv");
  const [formatted, setFormatted] = useState(true);
  const [busy, setBusy] = useState(false);
  const formattedId = useId();

  const formats: readonly ExportFormat[] = hasSvg ? [...TABLE_FORMATS, "png"] : TABLE_FORMATS;
  // DERIVED, not synced with an effect. A card can stop rendering an <svg> between two opens (a
  // viz override switched it to a table), and a `format` of "png" that is no longer on offer would
  // leave the Download button doing nothing at all.
  const effFormat: ExportFormat = formats.includes(format) ? format : "csv";
  // Metabase's canConfigureFormatting: an image has no cells to format, and no row limit to note.
  const isTable = effFormat !== "png";

  async function run() {
    setBusy(true);
    try {
      if (effFormat === "png") {
        const svg = chartImageSvg(containerRef.current);
        if (svg) await downloadPng(title, svg);
      } else if (effFormat === "xlsx") {
        await downloadXlsx(title, columns, rows, columnSettings, formatted);
      } else if (effFormat === "json") {
        downloadJson(title, columns, rows, columnSettings, formatted);
      } else {
        downloadCsv(title, columns, rows, columnSettings, formatted);
      }
    } catch (e) {
      console.error(e);
      toast.error(`Failed to export ${effFormat.toUpperCase()}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Popover onOpenChange={(open) => { if (open) setHasSvg(!!chartImageSvg(containerRef.current)); }}>
      <PopoverTrigger asChild>
        <Button size="sm" variant="ghost" aria-label="Export chart" disabled={disabled}>
          <Download className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="flex w-72 flex-col gap-4 p-3">
        <div className="text-sm font-medium">Download data</div>
        {/* The Radix primitive rather than `ui/radio-group.tsx`: that wrapper hard-codes a
            `rounded-full` dot, which is the right shape for a list of choices and the wrong one for
            a segmented control. Using the primitive keeps the real radiogroup semantics — roving
            focus, arrow keys, one checked radio — that three styled buttons would not have. */}
        <RadioGroupPrimitive.Root
          aria-label="Download format"
          className="grid auto-cols-fr grid-flow-col gap-1 rounded-md bg-muted p-1"
          value={effFormat}
          onValueChange={(v) => setFormat(v as ExportFormat)}
        >
          {formats.map((f) => (
            <RadioGroupPrimitive.Item
              key={f}
              value={f}
              className="rounded px-2 py-1 text-xs text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=checked]:bg-background data-[state=checked]:text-foreground data-[state=checked]:shadow-sm"
            >
              .{f}
            </RadioGroupPrimitive.Item>
          ))}
        </RadioGroupPrimitive.Root>
        {isTable && (
          <label htmlFor={formattedId} className="flex items-start justify-between gap-2 text-sm">
            <span className="flex flex-col gap-0.5">
              <span>Keep the data formatted</span>
              {/* Metabase swaps the example with the switch, so the reader sees what they are
                  choosing rather than a label they have to imagine the effect of. */}
              <span className="text-xs text-muted-foreground">
                {formatted ? "E.g. September 6, 2026 or $187.50, like on screen" : "E.g. 2026-09-06 or 187.50, like in the database"}
              </span>
            </span>
            <Checkbox id={formattedId} aria-label="keep the data formatted" checked={formatted} onCheckedChange={(v) => setFormatted(v === true)} />
          </label>
        )}
        {/* The file is written in the browser from the rows the card loaded, and a chart query never
            returns more than `MAX_CHART_ROWS` — so that is the limit, not Metabase's server-side
            million, which no egret export goes through. */}
        {isTable && <p className="text-xs text-muted-foreground">{`The maximum download size is ${MAX_CHART_ROWS.toLocaleString()} rows.`}</p>}
        <Button size="sm" className="ml-auto" disabled={busy} onClick={run}>Download</Button>
      </PopoverContent>
    </Popover>
  );
}
