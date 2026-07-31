import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { SeriesDisplay } from "@/components/chart/echarts/options/cartesian";

export interface SeriesListPatch {
  seriesOrder?: string[];
  hiddenSeries?: string[];
  seriesColors?: Record<string, string>;
  seriesLabels?: Record<string, string>;
  seriesTypes?: Record<string, SeriesDisplay>;
}

/**
 * One row per series: move up/down, colour, rename, show/hide.
 *
 * These four settings used to live in two lists rendered back to back under the same heading —
 * colour+name in one, order+visibility in the other — so four series filled eight rows and each
 * name was printed twice. Worse, the two disagreed about order: the colour list drew names in
 * their natural order while the order list drew them ranked, so moving a series up moved it in one
 * list and not the other, and the swatch beside a name stopped belonging to the row above it.
 * Metabase gives a series exactly one row; so does this.
 *
 * `defaultColor` is indexed by the series' NATURAL position, not its position in this list: the
 * renderer assigns palette slots before it applies `seriesOrder`, so a reordered series keeps the
 * colour it is actually drawn in.
 */
export function SeriesListSection({ names, order, hidden, colors, labels, types, defaultDisplay, defaultColor, onChange }: {
  names: string[];
  order?: string[];
  hidden?: string[];
  colors?: Record<string, string>;
  labels?: Record<string, string>;
  /** Combo only: how each series is drawn. Absent for every other kind, whose display is fixed. */
  types?: Record<string, SeriesDisplay>;
  /** The display a series falls back to, by its NATURAL index — Metabase's first-is-a-line rule.
   *  Undefined means this chart kind offers no per-series display at all. */
  defaultDisplay?: (naturalIndex: number) => SeriesDisplay;
  defaultColor: (naturalIndex: number) => string;
  onChange: (patch: SeriesListPatch) => void;
}) {
  // The stored order can name series that are no longer plotted, and the data can bring new
  // ones the order has never seen. Rank by the stored position, then let anything unnamed
  // keep its natural place after them — the same rule the option builder applies.
  const rank = new Map((order ?? []).map((n, i) => [n, i]));
  const naturalIndex = new Map(names.map((n, i) => [n, i]));
  const ranked = [...names].sort(
    (a, b) => (rank.get(a) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b) ?? Number.MAX_SAFE_INTEGER),
  );

  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= ranked.length) return;
    const a = ranked[i];
    const b = ranked[j];
    // Swap inside the STORED order, not inside the visible list: a series a filter excluded
    // from this run still has a remembered place, and reordering two other series must not
    // throw it away.
    const base = order?.length ? [...order] : [...ranked];
    for (const n of [a, b]) if (!base.includes(n)) base.push(n);
    const ia = base.indexOf(a);
    const ib = base.indexOf(b);
    [base[ia], base[ib]] = [base[ib], base[ia]];
    onChange({ seriesOrder: base });
  };

  return (
    <div className="space-y-1">
      {ranked.map((name, i) => (
        <div key={name} className="flex items-center gap-1">
          <div className="flex shrink-0 flex-col leading-none">
            <button type="button" aria-label={`Move ${name} up`} disabled={i === 0}
              className="text-[10px] text-muted-foreground disabled:opacity-40" onClick={() => move(i, -1)}>▲</button>
            <button type="button" aria-label={`Move ${name} down`} disabled={i === ranked.length - 1}
              className="text-[10px] text-muted-foreground disabled:opacity-40" onClick={() => move(i, 1)}>▼</button>
          </div>
          <input
            type="color"
            aria-label={`color ${name}`}
            className="h-7 w-7 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
            value={colors?.[name] ?? defaultColor(naturalIndex.get(name) ?? i)}
            onChange={(e) => onChange({ seriesColors: { ...colors, [name]: e.target.value } })}
          />
          {defaultDisplay && (
            <Select
              value={types?.[name] ?? defaultDisplay(naturalIndex.get(name) ?? i)}
              onValueChange={(v) => onChange({ seriesTypes: { ...types, [name]: v as SeriesDisplay } })}
            >
              <SelectTrigger aria-label={`display ${name}`} className="h-7 w-[4.5rem] shrink-0 px-2 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="line">Line</SelectItem>
                <SelectItem value="area">Area</SelectItem>
                <SelectItem value="bar">Bar</SelectItem>
              </SelectContent>
            </Select>
          )}
          <Input
            className="h-7 min-w-0 flex-1"
            aria-label={`label ${name}`}
            placeholder={name}
            value={labels?.[name] ?? ""}
            onChange={(e) => {
              const next = { ...labels };
              if (e.target.value) next[name] = e.target.value; else delete next[name];
              onChange({ seriesLabels: next });
            }}
          />
          <Switch aria-label={`Show ${name}`} checked={!hidden?.includes(name)}
            onCheckedChange={(v) => onChange({
              hiddenSeries: v ? (hidden ?? []).filter((n) => n !== name) : [...(hidden ?? []), name],
            })} />
        </div>
      ))}
    </div>
  );
}
