import { Switch } from "@/components/ui/switch";

/**
 * The series listed in draw order, each with move-up / move-down and a visibility switch.
 * Backs `seriesOrder` / `hiddenSeries`, which `applySeriesOrder` in echarts/options/cartesian.ts
 * applies to the actual chart — the two must agree on how an unranked name resolves.
 */
export function SeriesOrderSection({ names, order, hidden, onChange }: {
  names: string[];
  order?: string[];
  hidden?: string[];
  onChange: (patch: { seriesOrder?: string[]; hiddenSeries?: string[] }) => void;
}) {
  // The stored order can name series that are no longer plotted, and the data can bring new
  // ones the order has never seen. Rank by the stored position, then let anything unnamed
  // keep its natural place after them — the same rule the option builder applies.
  const rank = new Map((order ?? []).map((n, i) => [n, i]));
  const ranked = [...names].sort(
    (a, b) => (rank.get(a) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b) ?? Number.MAX_SAFE_INTEGER),
  );
  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= ranked.length) return;
    const next = [...ranked];
    [next[i], next[j]] = [next[j], next[i]];
    onChange({ seriesOrder: next });
  };
  return (
    <div className="space-y-1">
      {ranked.map((name, i) => (
        <div key={name} className="flex items-center gap-1.5">
          <button type="button" aria-label={`Move ${name} up`} disabled={i === 0}
            className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, -1)}>↑</button>
          <button type="button" aria-label={`Move ${name} down`} disabled={i === ranked.length - 1}
            className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, 1)}>↓</button>
          <span className="flex-1 truncate text-sm">{name}</span>
          <Switch aria-label={`Show ${name}`} checked={!hidden?.includes(name)}
            onCheckedChange={(v) => onChange({
              hiddenSeries: v ? (hidden ?? []).filter((n) => n !== name) : [...(hidden ?? []), name],
            })} />
        </div>
      ))}
    </div>
  );
}
