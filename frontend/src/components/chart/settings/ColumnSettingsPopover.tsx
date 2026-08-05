import { Settings2 } from "lucide-react";
import type { ColumnFormat } from "../format-types";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ColumnFormatFields, colIcon, colKind } from "./FormatSection";

interface ResultColumn { name: string; type: string }

/**
 * One column's settings, drilled into from its row in the table's column list.
 *
 * WHY A DRILL-IN. egret's table used to configure a column from THREE lists standing side by side
 * — `Data ▸ Columns` (what the query projects), `Display ▸ Visible columns` (a checkbox each) and
 * `Display ▸ Formatting` (an accordion each) — so a five-column table printed the same five names
 * three times over, in three different orders, in two different tabs. Metabase gives a column one
 * row: identity on the row, everything else one click away. This is the "everything else".
 *
 * The sibling of `SeriesSettingsPopover`, deliberately, down to the header and the placement: the
 * two lists are the same control in the same panel and should not read as two different ideas.
 * They stay separate components rather than one generic because their BODIES have nothing in
 * common — line appearance versus number/date/link formatting — and the shared part is four lines
 * of Radix.
 *
 * UNLIKE the series popover, this button is never pruned. Every column has at least a title to
 * rename, whatever its type, so there is no column whose popover would open empty.
 */
export function ColumnSettingsPopover({
  column, value, onChange, allowMiniBar = true, allowViewAs = true,
}: {
  column: ResultColumn;
  /** The whole map, because the write is a merge over it — see `onChange` below. */
  value: Record<string, ColumnFormat>;
  onChange: (next: Record<string, ColumnFormat>) => void;
  allowMiniBar?: boolean;
  /** Defaults ON, and stays on for every caller today: this popover is opened from the TABLE's
   *  column list, and the table is the one viz whose renderer reads `view_as` at all. The prop
   *  exists so that a future non-table list reusing this drill-in has to make the same decision
   *  `SharedDisplaySections` makes, rather than inheriting a dead control by default. */
  allowViewAs?: boolean;
}) {
  const fmt = value[column.name] ?? {};
  const shown = fmt.column_title || column.name;
  // Absent for a column whose type nothing knows — see TableColumnsSection's note.
  const Icon = column.type ? colIcon(column.type) : null;
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          // Labelled by the RAW column name, never by the rename. It is the key `column_settings`,
          // `hiddenColumns` and `columnOrder` all address this column by, and a test driving
          // "settings <rename>" would stop finding the row the moment anyone typed a title.
          aria-label={`settings ${column.name}`}
          title={`Settings for ${shown}`}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <Settings2 className="h-3.5 w-3.5" aria-hidden />
        </button>
      </PopoverTrigger>
      {/* `side="right"`, the same as the series popover and for the same measured reason: the
          settings panel is the left column, so the popover lands on the canvas beside it instead
          of covering the very rows it is about. Radix flips it on collision. */}
      <PopoverContent align="start" side="right" className="max-h-[40rem] w-80 overflow-y-auto">
        <div className="mb-3 flex items-center gap-2 border-b pb-2">
          {Icon && <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" aria-hidden />}
          <span className="min-w-0 flex-1 truncate text-sm font-medium" title={column.name}>{shown}</span>
          {column.type && (
            <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
              {colKind(column.type)}
            </span>
          )}
        </div>
        <div className="space-y-3">
          <ColumnFormatFields
            column={column}
            fmt={fmt}
            // A merge over the WHOLE map, not a replace: `column_settings` holds every column's
            // format, and a popover that emitted only its own would wipe the other columns'.
            onPatch={(p) => onChange({ ...value, [column.name]: { ...fmt, ...p } })}
            allowMiniBar={allowMiniBar}
            allowViewAs={allowViewAs}
          />
        </div>
      </PopoverContent>
    </Popover>
  );
}
