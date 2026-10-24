import { useCallback, useMemo, useRef, useState } from "react";
import { BarChart3 } from "lucide-react";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { ResultsPage } from "@/api/sqllab";

const NUMERIC = /^(bigint|tinyint|smallint|integer|largeint|int|double|decimal|numeric|real|float|number|long|short|byte)\b/i;
const isNumeric = (type: string) => NUMERIC.test(type.trim());

type ChartType = "bar" | "line" | "area";
const MAX_CATEGORIES = 50;
const PAD = { top: 16, right: 24, bottom: 66, left: 64 };

/** About `count` ticks on round steps (1, 2 or 5 × 10ⁿ) that cover [min, max].
 *  For a whole-number measure (a count) the step is at least 1, so no tick is
 *  fractional (KI-162). */
function niceTicks(min: number, max: number, integer: boolean, count = 4): number[] {
  if (min === max) return [min];
  const raw = (max - min) / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  let step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag;
  if (integer) step = Math.max(1, Math.round(step));
  const first = Math.floor(min / step), last = Math.ceil(max / step);
  // Multiply rather than accumulate, so 0.1-steps do not drift (0.30000000000000004).
  return Array.from({ length: last - first + 1 }, (_, i) => Number(((first + i) * step).toPrecision(12)));
}
const fmt = (n: number) => (Math.abs(n) >= 1000 ? n.toLocaleString(undefined, { maximumFractionDigits: 1 }) : String(Math.round(n * 100) / 100));
const short = (s: string) => (s.length > 14 ? s.slice(0, 13) + "…" : s);
/** Rough width of an 11px label: labels that fit their band stay upright. */
const LABEL_CHAR_PX = 6.5;

export function ResultChart({ page }: { page: ResultsPage | null }) {
  const cols = page?.columns ?? [];
  const numericIdx = cols.map((c, i) => (isNumeric(c.type) ? i : -1)).filter((i) => i >= 0);
  const [type, setType] = useState<ChartType>("bar");
  const [xCol, setXCol] = useState<number>(() => cols.findIndex((c) => !isNumeric(c.type)) >= 0 ? cols.findIndex((c) => !isNumeric(c.type)) : 0);
  const [yCol, setYCol] = useState<number>(() => (numericIdx[0] ?? 1));

  // Fill the available area instead of letterboxing a fixed viewBox: measure the
  // plot container and draw the SVG 1:1 at its real pixel size.
  const [size, setSize] = useState({ w: 800, h: 320 });
  const roRef = useRef<ResizeObserver | null>(null);
  const measureRef = useCallback((node: HTMLDivElement | null) => {
    roRef.current?.disconnect();
    if (!node) return;
    const update = () => setSize({ w: Math.max(360, node.clientWidth), h: Math.max(220, node.clientHeight) });
    update();
    roRef.current = new ResizeObserver(update);
    roRef.current.observe(node);
  }, []);
  const W = size.w, H = size.h;

  const data = useMemo(() => {
    if (!page || yCol == null) return [];
    const agg = new Map<string, number>();
    for (const row of page.rows) {
      const key = row[xCol] == null ? "∅" : String(row[xCol]);
      const val = Number(row[yCol]);
      if (!Number.isFinite(val)) continue;
      agg.set(key, (agg.get(key) ?? 0) + val);
      if (agg.size > MAX_CATEGORIES) break;
    }
    return [...agg.entries()].map(([label, value]) => ({ label, value }));
  }, [page, xCol, yCol]);

  if (!page) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Run a query to chart its results.</div>;
  if (numericIdx.length === 0) return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
      <BarChart3 className="h-7 w-7 opacity-40" /> No numeric column to chart.
    </div>
  );

  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const values = data.map((d) => d.value);
  const ticks = niceTicks(Math.min(0, ...values), Math.max(0, ...values), values.every(Number.isInteger));
  // The axis runs from the lowest to the highest tick, so the top tick sits on the plot.
  const yMin = ticks[0];
  const yMax = ticks[ticks.length - 1];
  const yScale = (v: number) => PAD.top + plotH - ((v - yMin) / (yMax - yMin || 1)) * plotH;
  const n = data.length || 1;
  const band = plotW / n;
  const xCenter = (i: number) => PAD.left + band * i + band / 2;
  const rotate = data.some((d) => short(d.label).length * LABEL_CHAR_PX > band - 4);
  const zeroY = yScale(0);

  const linePts = data.map((d, i) => `${xCenter(i)},${yScale(d.value)}`).join(" ");
  const areaPath = data.length
    ? `M ${PAD.left + band / 2},${zeroY} L ${linePts.split(" ").join(" L ")} L ${xCenter(data.length - 1)},${zeroY} Z`
    : "";

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-2 py-1.5 text-xs">
        <Select value={type} onValueChange={(v) => setType(v as ChartType)}>
          <SelectTrigger className="h-7 w-24"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="bar">Bar</SelectItem>
            <SelectItem value="line">Line</SelectItem>
            <SelectItem value="area">Area</SelectItem>
          </SelectContent>
        </Select>
        <span className="text-muted-foreground">X</span>
        <Select value={String(xCol)} onValueChange={(v) => setXCol(Number(v))}>
          <SelectTrigger className="h-7 w-36"><SelectValue /></SelectTrigger>
          <SelectContent>{cols.map((c, i) => <SelectItem key={i} value={String(i)}>{c.name}</SelectItem>)}</SelectContent>
        </Select>
        <span className="text-muted-foreground">Y</span>
        <Select value={String(yCol)} onValueChange={(v) => setYCol(Number(v))}>
          <SelectTrigger className="h-7 w-36"><SelectValue /></SelectTrigger>
          <SelectContent>{numericIdx.map((i) => <SelectItem key={i} value={String(i)}>{cols[i].name}</SelectItem>)}</SelectContent>
        </Select>
        {data.length >= MAX_CATEGORIES && <span className="text-[11px] text-muted-foreground">showing first {MAX_CATEGORIES} categories</span>}
      </div>
      <div ref={measureRef} className="min-h-0 flex-1 overflow-hidden">
        <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-label={`${cols[yCol]?.name} by ${cols[xCol]?.name}`}>
          {/* gridlines + y ticks */}
          {ticks.map((t, i) => (
            <g key={i}>
              <line x1={PAD.left} x2={W - PAD.right} y1={yScale(t)} y2={yScale(t)} stroke="var(--border)" strokeWidth={1} />
              <text x={PAD.left - 8} y={yScale(t) + 3} textAnchor="end" fontSize={11} fill="var(--muted-foreground)" fontFamily="monospace">{fmt(t)}</text>
            </g>
          ))}
          {/* marks */}
          {type === "bar" && data.map((d, i) => {
            const y = yScale(d.value); const h = Math.abs(y - zeroY);
            const bw = Math.min(band * 0.7, 72); // cap width so few bars aren't huge slabs
            return <rect key={i} x={xCenter(i) - bw / 2} y={Math.min(y, zeroY)} width={bw} height={h || 1} rx={2} fill="var(--primary)" />;
          })}
          {type === "area" && <path d={areaPath} fill="var(--primary)" fillOpacity={0.18} stroke="none" />}
          {(type === "line" || type === "area") && (
            <>
              <polyline points={linePts} fill="none" stroke="var(--primary)" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
              {data.map((d, i) => <circle key={i} cx={xCenter(i)} cy={yScale(d.value)} r={2.5} fill="var(--primary)" />)}
            </>
          )}
          {/* x labels */}
          {data.map((d, i) => (
            <text key={i} x={xCenter(i)} y={H - PAD.bottom + 16} textAnchor={rotate ? "end" : "middle"} fontSize={11} fill="var(--muted-foreground)"
              transform={rotate ? `rotate(-35 ${xCenter(i)} ${H - PAD.bottom + 16})` : undefined}>{short(d.label)}</text>
          ))}
        </svg>
      </div>
    </div>
  );
}
