import React, { useState, lazy, Suspense } from "react";
import DOMPurify from "dompurify";
import { AlertCircle, ChevronRight } from "lucide-react";
import { ansiToHtml } from "./ansiToHtml";
import type { CellOutput } from "@/lib/kernel/protocol";

import DataFrameTable, { type DataFrameTableData } from "./DataFrameTable";

// Lazy so plotly.js (~3.5MB) only loads when a cell actually has a plotly output.
const PlotlyOutput = lazy(() => import("./PlotlyOutput"));
const PLOTLY_MIME = "application/vnd.plotly.v1+json";
// Structured DataFrame table (from the kernel's display() helper) → styled table.
const TABLE_MIME = "application/vnd.egret.table+json";

// Bounded ANSI→HTML cache. Keys are the exact output strings already held in
// memory by the cell's outputs state; FIFO eviction caps growth. Avoids
// re-converting a long-running cell's whole scrollback on every stream flush.
const ansiCache = new Map<string, string>();
function ansi(text: string): string {
  const hit = ansiCache.get(text);
  if (hit !== undefined) return hit;
  const html = DOMPurify.sanitize(ansiToHtml(text));
  if (ansiCache.size >= 500) {
    const oldest = ansiCache.keys().next().value;
    if (oldest !== undefined) ansiCache.delete(oldest);
  }
  ansiCache.set(text, html);
  return html;
}

// A malformed MIME bundle (corrupt HTML/JSON) can throw during render. Without
// a boundary the whole notebook unmounts; this contains it to one cell.
class OutputErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("CellOutputRenderer render error:", error, info.componentStack);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="border-t px-3 py-2 text-xs text-destructive">
          Failed to render output: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

function MimeBundle({ data }: { data: Record<string, unknown> }) {
  // Interactive plotly (application/vnd.plotly.v1+json) → the lazy plotly.js
  // renderer. Checked first so it wins over plotly's text/html fallback in the
  // same bundle (which the DOMPurify path would strip to a non-interactive shell).
  const plotlyRaw = data[PLOTLY_MIME];
  if (plotlyRaw != null) {
    const figure = (typeof plotlyRaw === "string" ? JSON.parse(plotlyRaw) : plotlyRaw) as {
      data: unknown[];
      layout?: Record<string, unknown>;
      config?: Record<string, unknown>;
    };
    return (
      <div className="h-96 w-full" data-testid="notebook-plotly">
        <Suspense
          fallback={<div className="text-xs text-muted-foreground">Loading chart…</div>}
        >
          <PlotlyOutput figure={figure} />
        </Suspense>
      </div>
    );
  }
  // Structured DataFrame table (kernel display() of a Daft frame) → styled table.
  // Checked before text/html so it wins over any HTML fallback in the bundle.
  const tableRaw = data[TABLE_MIME];
  if (tableRaw != null) {
    const table = (typeof tableRaw === "string" ? JSON.parse(tableRaw) : tableRaw) as DataFrameTableData;
    return (
      <div data-testid="notebook-dataframe">
        <DataFrameTable data={table} />
      </div>
    );
  }
  const html = data["text/html"] as string | undefined;
  const png = data["image/png"] as string | undefined;
  const jpeg = data["image/jpeg"] as string | undefined;
  const plain = data["text/plain"] as string | undefined;
  if (html)
    return (
      <div
        className="max-h-96 overflow-auto text-sm"
        dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(html) }}
      />
    );
  if (png) return <img alt="output" className="max-w-full" src={`data:image/png;base64,${png}`} />;
  if (jpeg) return <img alt="output" className="max-w-full" src={`data:image/jpeg;base64,${jpeg}`} />;
  if (plain)
    return (
      <pre
        className="whitespace-pre-wrap text-sm"
        dangerouslySetInnerHTML={{ __html: ansi(plain) }}
      />
    );
  return null;
}

function InnerRenderer({ outputs }: { outputs: CellOutput[] }) {
  const [showLogs, setShowLogs] = useState(false);
  if (!outputs?.length) return null;

  // stderr streams collapse into a single trailing "Logs" disclosure; stdout,
  // results, and errors render inline in order.
  const stderr = outputs
    .filter((o): o is Extract<CellOutput, { type: "stream" }> => o.type === "stream" && o.name === "stderr")
    .map((o) => o.text)
    .join("");

  return (
    <div className="mt-2 space-y-1 border-t bg-muted/30 px-3 py-2">
      {outputs.map((o, i) => {
        switch (o.type) {
          case "stream":
            if (o.name === "stderr") return null; // shown in Logs
            return (
              <pre
                key={i}
                className="whitespace-pre-wrap text-sm text-foreground"
                dangerouslySetInnerHTML={{ __html: ansi(o.text) }}
              />
            );
          case "execute_result":
          case "display_data":
            return <MimeBundle key={i} data={o.data} />;
          case "error":
            return (
              <div
                key={i}
                className="overflow-x-auto rounded border border-destructive/30 bg-destructive/5 p-2 text-sm text-destructive"
              >
                <div className="mb-1 flex items-center gap-1.5 font-semibold">
                  <AlertCircle className="h-4 w-4" />
                  <span className="whitespace-pre-wrap">
                    {o.ename}: {o.evalue}
                  </span>
                </div>
                {o.traceback.length > 0 && (
                  <pre
                    className="whitespace-pre-wrap break-words border-t border-destructive/20 pt-2 font-mono text-xs opacity-90"
                    dangerouslySetInnerHTML={{ __html: ansi(o.traceback.join("\n")) }}
                  />
                )}
              </div>
            );
          case "truncated":
            return (
              <div key={i} className="text-xs text-muted-foreground">
                … output truncated ({o.dropped} dropped)
              </div>
            );
        }
      })}

      {stderr.trim() && (
        <div className="mt-1">
          <button
            type="button"
            onClick={() => setShowLogs((v) => !v)}
            className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
          >
            <ChevronRight
              className={`h-3 w-3 transition-transform ${showLogs ? "rotate-90" : ""}`}
            />
            Logs ({stderr.split("\n").filter(Boolean).length} lines)
          </button>
          {showLogs && (
            <pre
              className="mt-1 max-h-48 overflow-auto rounded border bg-background/50 p-2 text-[11px] text-muted-foreground"
              dangerouslySetInnerHTML={{ __html: ansi(stderr) }}
            />
          )}
        </div>
      )}
    </div>
  );
}

export function CellOutputRenderer({ outputs }: { outputs: CellOutput[] }) {
  return (
    <OutputErrorBoundary>
      <InnerRenderer outputs={outputs} />
    </OutputErrorBoundary>
  );
}
