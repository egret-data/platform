import { CHART_TYPES } from "@/components/chart/chartTypes";

/** viz_type → lucide icon component, sourced from the canonical chart-type registry. */
export const ICON_BY_VIZ = new Map(CHART_TYPES.map((t) => [t.key, t.icon] as const));
