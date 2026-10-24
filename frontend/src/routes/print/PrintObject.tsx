import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { fetchRenderData, fetchRenderPayload } from "@/api/render";
import type {
  RenderCard,
  RenderCardData,
  RenderCardRequest,
  RenderPayload,
} from "@/api/generated/model";
import type { DashboardParameter } from "@/api/dashboards";
import { paramFiltersForCard, temporalOverridesForCard } from "@/components/chart/dashboard/params";
import type { AnyChartParams, VizType } from "@/api/charts";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartErrorBoundary } from "@/components/chart/ChartErrorBoundary";

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

/** The viz types whose height is their rows rather than the box they are given. */
const GROWS_WITH_ROWS = new Set(["table", "pivot_table", "object"]);

/** One card's chart, wrapped in its own error boundary — one bad chart marks the whole page
 *  failed (via `onError`) rather than leaving `onFinished` uncalled and the page hung forever.
 *
 *  `data.columns` is the engine's own `ColumnMeta[]` (name + type), carried through unmodified
 *  from `RenderCardData` — the same shape `ChartRenderer`'s `columns` prop wants (`ResultColumn` is
 *  `{ name, type }`). Nothing here guesses a type from sampled cell values: an earlier revision
 *  of this page did, and a numeric-string column, a column whose first row is null, or (worst)
 *  any temporal column all formatted differently from the same chart on its dashboard. The fix is
 *  the backend carrying real column metadata (`crates/egret-automation/src/render/payload.rs`),
 *  not smarter guessing here — if `columns` is ever missing a type, that is a payload bug to
 *  surface, not paper over with a client-side fallback. */
function PrintCard({ card, data, onFinished, onError }: {
  card: RenderCard;
  data: RenderCardData;
  onFinished: () => void;
  onError: () => void;
}) {
  return (
    <div className="mb-8 break-inside-avoid rounded border border-gray-200 bg-white p-4">
      <h2 className="mb-2 text-base font-medium text-black">{card.title}</h2>
      {/* A chart draws into a fixed 384px box and is clipped to its height. A table's height is
          its rows, so its box grows to them instead: on a page there is nothing to scroll, and a
          fixed box let a 50-row table run past the card's border into the next card (KI-168).
          Vertical only (`overflow-y: clip`, which leaves x visible): a chart canvas is sized at
          the renderer's screen width and the PDF shrinks a page that overflows sideways to fit
          it, and `overflow-hidden` cut the right of every chart off instead. */}
      <div data-print-chart className={GROWS_WITH_ROWS.has(card.viz_type) ? "w-full" : "h-96 w-full overflow-y-clip"}>
        <ChartErrorBoundary onError={onError}>
          <ChartRenderer
            vizType={card.viz_type as VizType}
            params={card.params as AnyChartParams}
            columns={data.columns}
            rows={data.rows}
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
 * the renderer's browser carries no session at all; the token in `?t=` is its only credential, and
 * it never leaves this component (see `@/api/render`).
 *
 * The render is TWO calls: describe the object, then ask for the rows. The middle step resolves
 * the dashboard's parameters into filters using the same module `DashboardView` uses, which is
 * why an emailed dashboard matches the dashboard. Only the second call spends the token.
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
  const [data, setData] = useState<Map<string, RenderCardData> | null>(null);
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

  // Two requests, in order, exactly once. No retry and no refetch — even a token change (there is
  // no route that changes it after mount) must not re-fire these: the second call spends the token
  // and a repeat 404s.
  //
  // The middle step is the point of the whole shape. The dashboard's parameters are resolved to
  // filters HERE, by `paramFiltersForCard`/`temporalOverridesForCard` — the same functions
  // `DashboardView` calls — so the report and the page it mirrors cannot disagree about what a
  // relative date window or a "contains" operator means. Compiling them server-side would have
  // meant a second implementation of those rules, and the first time the two drifted the emailed
  // image would quietly stop matching the dashboard.
  useEffect(() => {
    if (!token) {
      fail();
      return;
    }
    (async () => {
      const p = await fetchRenderPayload(token);
      setPayload(p);

      const parameters = (p.parameters ?? []) as DashboardParameter[];
      // Defaults, because a scheduled report has nobody at the keyboard: this is the dashboard as
      // it looks when you open it, which is exactly what the recipient will see if they follow the
      // link in the mail.
      const values: Record<string, unknown> = {};
      for (const param of parameters) values[param.slug] = param.default;

      const rows = await fetchRenderData(
        token,
        p.cards.map((card) => {
          const mappings = (card.param_mappings ?? {}) as Record<string, string>;
          const spec = (card.params as AnyChartParams).spec;
          return {
            chart_id: card.chart_id,
            // The same cast the interactive path makes (`useDashboardCardData`):
            // `ChartFilter` and the generated `Filter` describe the same wire
            // object, one hand-written and one from the schema.
            filters: paramFiltersForCard(parameters, values, mappings) as RenderCardRequest["filters"],
            temporal_units: temporalOverridesForCard(parameters, values, mappings, spec),
          };
        }),
      );
      setData(new Map(rows.map((r) => [r.chart_id, r])));
    })().catch(fail);
  }, []);

  useEffect(() => {
    if (!failed && payload && data && fontsReady && finished >= payload.cards.length) {
      window.__EGRET_PRINT_READY__ = true;
    }
  }, [failed, payload, data, fontsReady, finished]);

  if (failed) return <div className="min-h-screen bg-white" />;
  // Both steps, not just the first: rendering a card before its rows arrive would fire
  // `onFinished` on an empty chart and let the renderer capture a blank page as a finished one.
  if (!payload || !data) return <div className="min-h-screen bg-white" />;

  return (
    <div data-print-root className="min-h-screen w-full bg-white p-6 text-black">
      {/* `data-print-root` marks the content region for the renderer's post-readiness height
          measurement (see `measureContentHeight` in `renderer/server.js`). This wrapper's own
          `min-h-screen` always fills the viewport regardless of content, so the renderer must not
          measure IT — it measures the bounding boxes of its children (this title + each card)
          instead, which size themselves off their own content. */}
      {/* Belt and braces alongside the readiness flag, not a substitute for it: a screenshot taken
          the instant the flag flips should not still be mid-transition on anything CSS-driven.
          ECharts' own mount animation is JS-driven and unaffected by this — the flag is what
          actually waits for it, via each card's `onFinished`. */}
      <style>{"@media (prefers-reduced-motion: reduce), print { *, *::before, *::after { animation-duration: 0.001ms !important; animation-iteration-count: 1 !important; transition-duration: 0.001ms !important; } }"}</style>
      <h1 className="mb-6 text-xl font-semibold">{payload.title}</h1>
      {payload.cards.map((card) => {
        const rows = data.get(card.chart_id);
        // A card the data step did not answer for is a payload bug, not something to render
        // empty — failing loudly beats a report with a silently blank panel in it.
        if (!rows) return null;
        return (
          <PrintCard
            key={card.chart_id}
            card={card}
            data={rows}
            onFinished={() => setFinished((n) => n + 1)}
            onError={fail}
          />
        );
      })}
    </div>
  );
}
