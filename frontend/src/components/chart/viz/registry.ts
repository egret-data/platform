import { CARTESIAN_VIZ } from "@/api/charts";
import type { VizType } from "@/api/charts";
import { cartesianDescriptor } from "./cartesian";
import type { VizDescriptor } from "./types";

/**
 * Viz types that have moved onto a descriptor. Everything absent here still runs on
 * ChartExplore's inline code via the bridge; PR2 and PR3 empty that bridge out.
 */
const REGISTRY: Partial<Record<VizType, VizDescriptor<never>>> = Object.fromEntries(
  CARTESIAN_VIZ.map((k) => [k, cartesianDescriptor(k)]),
) as Partial<Record<VizType, VizDescriptor<never>>>;

export const getDescriptor = (viz: VizType) => REGISTRY[viz];
export const hasDescriptor = (viz: VizType) => REGISTRY[viz] !== undefined;
