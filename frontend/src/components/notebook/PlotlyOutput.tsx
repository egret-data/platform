import { useEffect, useRef } from "react";

export interface PlotlyFigure {
  data: unknown[];
  layout?: Record<string, unknown>;
  config?: Record<string, unknown>;
}

type Plotly = (typeof import("plotly.js-dist-min"))["default"];

/** Render a plotly figure (from application/vnd.plotly.v1+json) with a lazily
 *  imported plotly.js. Theme-aware (transparent bg + themed font), responsive,
 *  purged on unmount. Mirrors EChartsRenderer's init/resize/dispose lifecycle. */
export default function PlotlyOutput({ figure }: { figure: PlotlyFigure }) {
  const elRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    let disposed = false;
    let ro: ResizeObserver | undefined;
    let plotly: Plotly | undefined;

    const dark = document.documentElement.classList.contains("dark");
    const themedLayout = {
      paper_bgcolor: "rgba(0,0,0,0)",
      plot_bgcolor: "rgba(0,0,0,0)",
      font: { color: dark ? "#e5e7eb" : "#111827" },
      ...(figure.layout ?? {}),
      autosize: true,
    };
    const config = { responsive: true, displaylogo: false, ...(figure.config ?? {}) };

    void import("plotly.js-dist-min").then((mod) => {
      if (disposed) return;
      plotly = mod.default;
      plotly.react(el, figure.data, themedLayout, config);
      ro = new ResizeObserver(() => plotly?.Plots?.resize(el));
      ro.observe(el);
    });

    return () => {
      disposed = true;
      ro?.disconnect();
      if (plotly && el) plotly.purge(el);
    };
  }, [figure]);

  return <div ref={elRef} className="h-full w-full" />;
}
