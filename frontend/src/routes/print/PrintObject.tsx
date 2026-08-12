import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { fetchRenderPayload } from "@/api/render";
import type { RenderCard, RenderPayload } from "@/api/generated/model";
import type { AnyChartParams, VizType } from "@/api/charts";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartErrorBoundary } from "@/components/chart/ChartErrorBoundary";
import type { ResultColumn } from "@/components/chart/ChartTable";

declare global {
  interface Window {
    /** Set exactly once every card has fired its own `onFinished` AND `document.fonts.ready` has
     *  resolved. This is the whole contract with the headless renderer — it polls for this flag
     *  rather than screenshotting on a fixed delay, which is what makes a report image reliable
     *  instead of "usually" reliable. */
    __EGRET_PRINT_READY__?: true;
    /** Set on the fetch's own failure or on a caught chart error, and checked before
     *  `__EGRET_PRINT_READY__` is ever set — the two are mutually exclusive by construction (see
     *  `PrintObject`'s readiness effect below). A failed page must never be captured. */
    __EGRET_PRINT_FAILED__?: true;
  }
}

/**
 * `RenderCard.columns` is a bare list of names — the render payload carries no column TYPES, only
 * what a raw `SELECT` returned. `ChartRenderer` needs a type per column to decide numeric vs.
 * temporal vs. text formatting, so one is inferred here from the first non-null cell seen in that
 * column. This is a print-only approximation (a card whose first sampled value is, say, a numeric
 * string typed as `varchar` would format looser than the original chart did) — good enough for a
 * report thumbnail, and not a new problem to solve in the route that turns the flag on.
 */
function inferColumnType(rows: unknown[][], index: number): string {
  for (const row of rows) {
    const v = row[index];
    if (v == null) continue;
    if (typeof v === "number") return "double";
    if (typeof v === "boolean") return "boolean";
    return "varchar";
  }
  return "varchar";
}

function toResultColumns(card: RenderCard): ResultColumn[] {
  return card.columns.map((name, i) => ({ name, type: inferColumnType(card.rows, i) }));
}

/** One card's chart, wrapped in its own error boundary — one bad chart marks the whole page
 *  failed (via `onError`) rather than leaving `onFinished` uncalled and the page hung forever. */
function PrintCard({ card, onFinished, onError }: {
  card: RenderCard;
  onFinished: () => void;
  onError: () => void;
}) {
  const columns = useMemo(() => toResultColumns(card), [card]);
  return (
    <div className="mb-8 break-inside-avoid rounded border border-gray-200 bg-white p-4">
      <h2 className="mb-2 text-base font-medium text-black">{card.title}</h2>
      <div className="h-96 w-full">
        <ChartErrorBoundary onError={onError}>
          <ChartRenderer
            vizType={card.viz_type as VizType}
            params={card.params as AnyChartParams}
            columns={columns}
            rows={card.rows}
            onFinished={onFinished}
          />
        </ChartErrorBoundary>
      </div>
    </div>
  );
}

/**
 * The chrome-free page a headless renderer screenshots to produce a report image. No nav, no
 * auth-guarded shell — deliberately outside `RequireAuth`/`RequireWorkspace` in `App.tsx`, because
 * the renderer's browser carries no session at all; the single-use token in `?t=` is its only
 * credential, and it never leaves this component (see `fetchRenderPayload`).
 *
 * Readiness is a FLAG, never a timer. ECharts animates on mount and web fonts resolve after first
 * paint, so a renderer that waits a fixed delay gets a finished chart most of the time and a
 * half-drawn one occasionally — a defect no test sees and that reproduces on roughly one run in
 * twenty. `__EGRET_PRINT_READY__` is set only once every card has reported its own `onFinished`
 * AND `document.fonts.ready` has resolved; nothing here ever sets it on a fallback timer.
 */
export default function PrintObject() {
  const [searchParams] = useSearchParams();
  const token = searchParams.get("t");

  const [payload, setPayload] = useState<RenderPayload | null>(null);
  const [finished, setFinished] = useState(0);
  const [fontsReady, setFontsReady] = useState(false);
  const [failed, setFailed] = useState(false);

  const fail = () => {
    setFailed(true);
    window.__EGRET_PRINT_FAILED__ = true;
  };

  useEffect(() => {
    document.fonts.ready.then(() => setFontsReady(true));
  }, []);

  // The page's ONLY authenticated request. No retry and no refetch — even a token change (there
  // is no route that changes it after mount) must not re-fire this: the token is single-use and a
  // second call 404s.
  useEffect(() => {
    if (!token) {
      fail();
      return;
    }
    fetchRenderPayload(token).then(setPayload).catch(fail);
  }, []);

  useEffect(() => {
    if (!failed && payload && fontsReady && finished >= payload.cards.length) {
      window.__EGRET_PRINT_READY__ = true;
    }
  }, [failed, payload, fontsReady, finished]);

  if (failed) return <div className="min-h-screen bg-white" />;
  if (!payload) return <div className="min-h-screen bg-white" />;

  return (
    <div className="min-h-screen w-full bg-white p-6 text-black">
      {/* Belt and braces alongside the readiness flag, not a substitute for it: a screenshot taken
          the instant the flag flips should not still be mid-transition on anything CSS-driven.
          ECharts' own mount animation is JS-driven and unaffected by this — the flag is what
          actually waits for it, via each card's `onFinished`. */}
      <style>{"@media (prefers-reduced-motion: reduce), print { *, *::before, *::after { animation-duration: 0.001ms !important; animation-iteration-count: 1 !important; transition-duration: 0.001ms !important; } }"}</style>
      <h1 className="mb-6 text-xl font-semibold">{payload.title}</h1>
      {payload.cards.map((card) => (
        <PrintCard
          key={card.chart_id}
          card={card}
          onFinished={() => setFinished((n) => n + 1)}
          onError={fail}
        />
      ))}
    </div>
  );
}
