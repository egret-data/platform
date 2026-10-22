import { useMemo, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import {
  AlertTriangle, ArrowDown, Ban, ArrowUp, ChevronsUpDown, Copy, Download, Search, TableProperties,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ResultsPage } from "@/api/sqllab";

// Match engine numeric types (Trino + StarRocks) at the start of the type
// string, with a word boundary so `interval`/`varchar` don't false-match. Longer
// tokens precede shorter ones so `integer`/`bigint` win over a bare `int`.
const NUMERIC = /^(bigint|tinyint|smallint|integer|largeint|int|double|decimal|numeric|real|float|number|long|short|byte)\b/i;
const isNumeric = (type: string) => NUMERIC.test(type.trim());
const cellText = (v: unknown) => (v === null || v === undefined ? "" : String(v));
// Binary floating point (Trino `double`/`real`, StarRocks `double`/`float`).
const FLOAT = /^(double|real|float)\b/i;
/** A cell as the grid shows it. A float is rounded to 15 significant digits,
 *  which drops binary representation noise (`63339475.32000001` → `63339475.32`)
 *  and nothing a double carries reliably. Display only: copy, export, CSV and the
 *  expanded cell keep the exact value; exact types (integers, decimals) are not
 *  touched (KI-004). */
function displayText(v: unknown, type: string): string {
  if (typeof v === "number" && Number.isFinite(v) && FLOAT.test(type.trim())) return String(Number(v.toPrecision(15)));
  return String(v);
}

function copyText(text: string, label: string) {
  navigator.clipboard?.writeText(text).then(
    () => toast.success(label),
    () => toast.error("Copy failed"),
  );
}

type SortDir = "asc" | "desc";

export function ResultsGrid({ page, csvHref, truncated, error, errorTitle = "Query failed", cancelled, statusLine, loading }: {
  page: ResultsPage | null; csvHref: string | null; truncated: boolean;
  error?: string | null;
  /** The error panel's heading: "Query failed" unless the query never started. */
  errorTitle?: string;
  /** The run was cancelled: say so instead of the empty state (KI-078). */
  cancelled?: boolean;
  statusLine?: ReactNode; loading?: boolean;
}) {
  const [sortCol, setSortCol] = useState<number | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>("asc");
  const [filter, setFilter] = useState("");
  const [widths, setWidths] = useState<Record<number, number>>({});
  const [expanded, setExpanded] = useState<{ col: string; value: unknown } | null>(null);
  const drag = useRef<{ col: number; startX: number; startW: number } | null>(null);

  const cols = page?.columns ?? [];

  const rows = useMemo(() => {
    if (!page) return [];
    let r = page.rows;
    const q = filter.trim().toLowerCase();
    if (q) r = r.filter((row) => row.some((c) => cellText(c).toLowerCase().includes(q)));
    if (sortCol != null) {
      const numeric = isNumeric(page.columns[sortCol]?.type ?? "");
      const sign = sortDir === "asc" ? 1 : -1;
      r = [...r].sort((a, b) => {
        const av = a[sortCol], bv = b[sortCol];
        if (av == null && bv == null) return 0;
        if (av == null) return 1; // nulls last
        if (bv == null) return -1;
        const cmp = numeric
          ? Number(av) - Number(bv)
          : cellText(av).localeCompare(cellText(bv));
        return cmp * sign;
      });
    }
    return r;
  }, [page, filter, sortCol, sortDir]);

  function toggleSort(i: number) {
    if (sortCol !== i) { setSortCol(i); setSortDir("asc"); }
    else if (sortDir === "asc") setSortDir("desc");
    else { setSortCol(null); setSortDir("asc"); } // asc → desc → off
  }

  function onResizeDown(e: React.PointerEvent, col: number, current: number) {
    e.preventDefault(); e.stopPropagation();
    drag.current = { col, startX: e.clientX, startW: current };
    const move = (ev: PointerEvent) => {
      if (!drag.current) return;
      const w = Math.max(60, drag.current.startW + (ev.clientX - drag.current.startX));
      setWidths((prev) => ({ ...prev, [drag.current!.col]: w }));
    };
    const up = () => { drag.current = null; window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function tsv(): string {
    if (!page) return "";
    const head = cols.map((c) => c.name).join("\t");
    const body = rows.map((row) => row.map(cellText).join("\t")).join("\n");
    return `${head}\n${body}`;
  }
  function downloadJson() {
    if (!page) return;
    const objs = rows.map((row) => Object.fromEntries(cols.map((c, i) => [c.name, row[i]])));
    const blob = new Blob([JSON.stringify(objs, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob); a.download = "results.json"; a.click();
    URL.revokeObjectURL(a.href);
  }

  if (error) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex items-center gap-2 border-b border-destructive/30 bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          <AlertTriangle className="h-4 w-4 shrink-0" /> {errorTitle}
          <Button variant="ghost" size="sm" className="ml-auto h-6 gap-1 px-2 text-xs text-destructive hover:text-destructive"
            onClick={() => copyText(error, "Error copied")}><Copy className="h-3 w-3" /> Copy</Button>
        </div>
        <div className="flex-1 overflow-auto whitespace-pre-wrap p-4 font-mono text-xs text-destructive">{error}</div>
        {statusLine && <div className="border-t px-3 py-1.5 text-xs text-muted-foreground">{statusLine}</div>}
      </div>
    );
  }

  if (loading && !page) {
    return (
      <div className="space-y-2 p-4">
        <Skeleton className="h-8 w-full" />
        {Array.from({ length: 8 }).map((_, i) => <Skeleton key={i} className="h-6 w-full" style={{ opacity: 1 - i * 0.09 }} />)}
      </div>
    );
  }

  if (cancelled && !page) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground">
          <Ban className="h-7 w-7 opacity-40" />
          <div className="font-medium text-foreground">Query cancelled</div>
          <div className="text-xs">The run was stopped before it returned rows. Run it again to see results.</div>
        </div>
        {statusLine && <div className="border-t px-3 py-1.5 text-xs text-muted-foreground">{statusLine}</div>}
      </div>
    );
  }

  if (!page) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground">
        <TableProperties className="h-7 w-7 opacity-40" />
        <div>No results yet.</div>
        <div className="text-xs">Run a query with <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">⌘↵</kbd> to see rows here.</div>
      </div>
    );
  }

  const loadedAll = page.rows.length >= page.row_count;
  return (
    <div className="flex h-full flex-col">
      {/* Result toolbar: filter + export. */}
      <div className="flex items-center gap-2 border-b px-2 py-1.5">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter rows…"
            aria-label="Filter result rows"
            className="h-7 w-48 rounded-md border bg-background pl-7 pr-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring" />
        </div>
        <span className="text-xs tabular-nums text-muted-foreground">
          {filter ? `${rows.length.toLocaleString()} of ${page.row_count.toLocaleString()}` : `${page.row_count.toLocaleString()} rows`}
        </span>
        {truncated && (
          <span className="inline-flex items-center gap-1 rounded-full border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-[11px] text-amber-700 dark:text-amber-400">
            <AlertTriangle className="h-3 w-3" /> Results were limited to {page.row_count.toLocaleString()} rows.
          </span>
        )}
        {!loadedAll && !truncated && (
          <span className="text-[11px] text-muted-foreground">interactive over first {page.rows.length.toLocaleString()} rows</span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <Button variant="outline" size="sm" className="h-7 gap-1" onClick={() => copyText(tsv(), "Copied to clipboard")}>
            <Copy className="h-3.5 w-3.5" /> Copy
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" className="h-7 gap-1"><Download className="h-3.5 w-3.5" /> Export</Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {csvHref && <DropdownMenuItem asChild><a href={csvHref} download>Download CSV</a></DropdownMenuItem>}
              <DropdownMenuItem onClick={downloadJson}>Download JSON</DropdownMenuItem>
              <DropdownMenuItem onClick={() => copyText(tsv(), "Copied to clipboard")}>Copy as TSV</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-separate border-spacing-0 text-sm">
          <thead>
            <tr>
              <th className="sticky left-0 top-0 z-20 w-10 border-b border-r bg-secondary px-2 py-1.5 text-right font-mono text-[11px] font-normal text-muted-foreground">#</th>
              {cols.map((c, i) => {
                const numeric = isNumeric(c.type);
                const active = sortCol === i;
                const SortIcon = !active ? ChevronsUpDown : sortDir === "asc" ? ArrowUp : ArrowDown;
                return (
                  <th key={c.name} aria-sort={active ? (sortDir === "asc" ? "ascending" : "descending") : "none"}
                    style={widths[i] ? { width: widths[i], minWidth: widths[i], maxWidth: widths[i] } : undefined}
                    className={`group/th sticky top-0 z-10 border-b bg-secondary px-3 py-1.5 font-semibold whitespace-nowrap ${numeric ? "text-right" : "text-left"}`}>
                    <span className={`inline-flex items-baseline gap-1.5 ${numeric ? "flex-row-reverse" : ""}`}>
                      <button onClick={() => toggleSort(i)} className="inline-flex items-baseline gap-1 hover:text-primary" title="Sort">
                        <span>{c.name}</span>
                        <SortIcon className={`h-3 w-3 self-center ${active ? "text-primary" : "text-muted-foreground/50"}`} />
                      </button>
                      <span className="font-mono text-[10px] font-normal text-muted-foreground">{c.type}</span>
                    </span>
                    <span onPointerDown={(e) => onResizeDown(e, i, widths[i] ?? (e.currentTarget.parentElement?.offsetWidth ?? 120))}
                      className="absolute right-0 top-0 h-full w-1 cursor-col-resize opacity-0 hover:bg-primary/60 group-hover/th:opacity-100" />
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => (
              <tr key={i} className="group hover:bg-accent/50 [&:nth-child(even)]:bg-muted/30">
                <td className="sticky left-0 z-10 border-b border-r bg-background px-2 py-1 text-right font-mono text-[11px] text-muted-foreground group-hover:bg-accent">
                  {i + 1}
                </td>
                {row.map((cell, j) => {
                  const numeric = isNumeric(cols[j]?.type ?? "");
                  const isNull = cell === null || cell === undefined;
                  return (
                    <td key={j} onClick={() => setExpanded({ col: cols[j]?.name ?? "", value: cell })}
                      title="Click to expand"
                      style={widths[j] ? { maxWidth: widths[j] } : undefined}
                      className={`max-w-[28rem] cursor-pointer truncate border-b px-3 py-1 font-mono text-xs hover:underline ${numeric ? "text-right tabular-nums" : "text-left"} ${isNull ? "italic text-muted-foreground/60" : ""}`}>
                      {isNull ? "NULL" : displayText(cell, cols[j]?.type ?? "")}
                    </td>
                  );
                })}
              </tr>
            ))}
            {rows.length === 0 && (
              <tr><td colSpan={cols.length + 1} className="px-3 py-6 text-center text-xs text-muted-foreground">No rows match “{filter}”.</td></tr>
            )}
          </tbody>
        </table>
      </div>

      {statusLine && <div className="border-t px-3 py-1.5 text-xs text-muted-foreground">{statusLine}</div>}

      {/* Cell value expander. */}
      <Dialog open={expanded != null} onOpenChange={(o) => { if (!o) setExpanded(null); }}>
        <DialogContent className="max-w-2xl">
          <DialogHeader><DialogTitle className="font-mono text-sm">{expanded?.col}</DialogTitle></DialogHeader>
          <pre className="max-h-[60vh] overflow-auto whitespace-pre-wrap break-words rounded-md border bg-muted/40 p-3 font-mono text-xs">
            {expanded == null ? "" : expanded.value === null || expanded.value === undefined ? "NULL" : String(expanded.value)}
          </pre>
          <div className="flex justify-end">
            <Button size="sm" variant="outline" className="gap-1"
              onClick={() => copyText(cellText(expanded?.value), "Value copied")}><Copy className="h-3.5 w-3.5" /> Copy</Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
