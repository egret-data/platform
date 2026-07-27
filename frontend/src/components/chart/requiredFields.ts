import type { VizType } from "@/api/charts";
import { isCartesianViz } from "@/api/charts";

interface Input {
  cartesian?: { xAxis?: string; yMetrics?: string[] };
  pie?: { dimension?: string; metric?: string };
  scalar?: { metric?: string };
  progress?: { metric?: string };
  gauge?: { metric?: string };
  funnel?: { dimension?: string; metric?: string };
  waterfall?: { xAxis?: string; metric?: string };
  smart?: { dimension?: string; metric?: string };
  box?: { category?: string; value?: string };
  sankey?: { source?: string; target?: string; metric?: string };
  map?: { region?: string; metric?: string };
  pivot?: { rows?: string[]; vals?: string[] };
  objectCols?: string[];
}

/** Labels of the fields `viz` still needs before it can run. Empty = ready. */
export function missingFields(viz: VizType, i: Input): string[] {
  const out: string[] = [];
  const need = (cond: boolean, label: string) => { if (cond) out.push(label); };
  if (isCartesianViz(viz)) {
    need(!i.cartesian?.xAxis, "X-axis");
    need(!(i.cartesian?.yMetrics?.length), "a metric");
  } else if (viz === "pie") {
    need(!i.pie?.dimension, "a dimension"); need(!i.pie?.metric, "a metric");
  } else if (viz === "scalar") {
    need(!i.scalar?.metric, "a metric");
  } else if (viz === "progress") {
    need(!i.progress?.metric, "a metric");
  } else if (viz === "gauge") {
    need(!i.gauge?.metric, "a metric");
  } else if (viz === "funnel") {
    need(!i.funnel?.dimension, "a dimension"); need(!i.funnel?.metric, "a metric");
  } else if (viz === "waterfall") {
    need(!i.waterfall?.xAxis, "an x-axis"); need(!i.waterfall?.metric, "a metric");
  } else if (viz === "smartscalar") {
    need(!i.smart?.dimension, "a time dimension"); need(!i.smart?.metric, "a metric");
  } else if (viz === "boxplot") {
    need(!i.box?.category, "a category"); need(!i.box?.value, "a value column");
  } else if (viz === "sankey") {
    need(!i.sankey?.source, "a source"); need(!i.sankey?.target, "a target"); need(!i.sankey?.metric, "a metric");
  } else if (viz === "map") {
    need(!i.map?.region, "a region"); need(!i.map?.metric, "a metric");
  } else if (viz === "pivot_table") {
    need(!(i.pivot?.rows?.length), "a row dimension"); need(!(i.pivot?.vals?.length), "a value");
  } else if (viz === "object") {
    need(!(i.objectCols?.length), "at least one column");
  }
  return out;
}
