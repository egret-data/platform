import { funnelSteps, type FunnelBuildConfig, type FunnelStep, type ResultColumn } from "./echarts/options/funnel";
import { SERIES_PALETTE } from "./echarts/palette";

/**
 * Metabase-style Funnel.
 *
 * NOT an ECharts `funnel` series, which is what this used to be. Two things that series cannot do,
 * both measured on the live Metabase chart:
 *
 *  - **Labels outside the shape.** Metabase puts the step name ABOVE its column and the value and
 *    share BELOW it; the ECharts series can only draw them inside, where a narrow step truncates
 *    them. Measured at three steps on an 830px card the labels fitted (87-182px inside 248px
 *    wedges), but six steps give ~138px columns and the 182px label does not.
 *  - **One trapezoid per ADJACENT PAIR.** Metabase emits N-1 polygons in a unit viewBox, each
 *    running from `value[i]/max` to `value[i+1]/max` in height, centred. `3976/5061 = 0.78562` is
 *    exactly the edge it drew. That is a shape between two steps, not a shape per step.
 *
 * One colour for every step, as Metabase draws it. A palette per step would say these are
 * categories to compare; they are stages of one flow narrowing, and colour should not argue
 * otherwise.
 */
const FUNNEL_COLOR = SERIES_PALETTE[0];

export function FunnelChart({
  columns, rows, dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes, type = "funnel",
}: FunnelBuildConfig & {
  columns: ResultColumn[];
  rows: unknown[][];
  /** Metabase's `funnel.type`. "bar" draws each step as its own column instead of one narrowing
   *  band — the reading it prefers when the steps are not really one flow. */
  type?: "funnel" | "bar";
}) {
  const steps = funnelSteps(columns, rows, {
    dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes,
  });

  if (steps.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }

  return (
    <div className="flex h-full w-full gap-6 p-4"
      role="img"
      aria-label={`Funnel chart of ${metric ?? "a metric"} by ${dimension ?? "a dimension"}. ${
        steps.map((s) => `${s.name} ${s.label}${s.share ? `, ${s.share} of the first step` : ""}`).join("; ")
      }.`}>
      {/* The step key Metabase lists down the left, so the names are readable even when a column
          is too narrow to hold one. */}
      <ul className="hidden w-32 shrink-0 flex-col justify-center gap-1 text-xs sm:flex">
        {steps.map((s) => (
          <li key={s.name} className="truncate font-medium text-foreground/80" title={s.name}>{s.name}</li>
        ))}
      </ul>

      <div className="flex min-w-0 flex-1 flex-col">
        {/* Name above each column. */}
        <div className="flex">
          {steps.map((s) => (
            <div key={s.name} className="min-w-0 flex-1 truncate px-1 text-xs text-muted-foreground" title={s.name}>
              {s.name}
            </div>
          ))}
        </div>

        <div className="min-h-0 flex-1">
          {type === "bar" ? <Bars steps={steps} /> : <Trapezoids steps={steps} />}
        </div>

        {/* Value below, and the share for every step after the first. The first names the metric
            instead — Metabase prints "Count" there, which is what the whole funnel is counting. */}
        <div className="flex">
          {steps.map((s, i) => (
            <div key={s.name} className="min-w-0 flex-1 px-1">
              {s.share && <div className="truncate text-xs text-muted-foreground">{s.share}</div>}
              <div className="truncate text-sm font-semibold tabular-nums text-foreground">{s.label}</div>
              {i === 0 && metric && (
                <div className="truncate text-xs text-muted-foreground" title={metric}>{metric}</div>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

/** One trapezoid per adjacent pair, each in its own unit viewBox — Metabase's own construction.
 *  A single step has no pair and so no band; it draws as a full-height block. */
function Trapezoids({ steps }: { steps: FunnelStep[] }) {
  if (steps.length === 1) {
    return <div className="h-full w-full" style={{ backgroundColor: FUNNEL_COLOR }} data-funnel-band />;
  }
  return (
    <div className="flex h-full w-full">
      {steps.slice(0, -1).map((s, i) => {
        const a = s.fraction;
        const b = steps[i + 1].fraction;
        // Centred vertically: an edge of height `f` runs from (1-f)/2 to 1-(1-f)/2.
        const pts = `0,${(1 - a) / 2} 1,${(1 - b) / 2} 1,${1 - (1 - b) / 2} 0,${1 - (1 - a) / 2}`;
        return (
          <svg key={s.name} data-funnel-band viewBox="0 0 1 1" preserveAspectRatio="none" className="h-full min-w-0 flex-1">
            <polygon points={pts} fill={FUNNEL_COLOR} />
          </svg>
        );
      })}
      {/* The last step has no pair to its right; a sliver keeps its column the same width as the
          others so the labels below stay under their own step. */}
      <svg data-funnel-band viewBox="0 0 1 1" preserveAspectRatio="none" className="h-full min-w-0 flex-1">
        <polygon
          points={`0,${(1 - steps[steps.length - 1].fraction) / 2} 1,${(1 - steps[steps.length - 1].fraction) / 2} 1,${1 - (1 - steps[steps.length - 1].fraction) / 2} 0,${1 - (1 - steps[steps.length - 1].fraction) / 2}`}
          fill={FUNNEL_COLOR}
        />
      </svg>
    </div>
  );
}

/** Metabase's `funnel.type: "bar"` — each step its own column, height by value. */
function Bars({ steps }: { steps: FunnelStep[] }) {
  return (
    <div className="flex h-full w-full items-end gap-1">
      {steps.map((s) => (
        <div key={s.name} data-funnel-band className="min-w-0 flex-1 rounded-t"
          style={{ height: `${Math.max(1, s.fraction * 100)}%`, backgroundColor: FUNNEL_COLOR }} />
      ))}
    </div>
  );
}
