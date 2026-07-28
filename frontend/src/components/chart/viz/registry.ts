import { CARTESIAN_VIZ } from "@/api/charts";
import type { VizType } from "@/api/charts";
import { cartesianDescriptor } from "./cartesian";
import { pivotDescriptor } from "./pivot";
import { tableDescriptor, objectDescriptor } from "./table";
import { mapDescriptor } from "./map";
import type { VizDescriptor } from "./types";

/**
 * Viz types that have moved onto a descriptor. Everything absent here still runs on
 * ChartExplore's inline code via the bridge; PR3 empties out the last nine.
 */
/**
 * Erases a descriptor's config type at the registry boundary: the shell treats every
 * config as opaque, so the registry stores them uniformly and only the descriptor itself
 * ever sees the real shape.
 */
const entry = <C>(d: VizDescriptor<C>) => d as unknown as VizDescriptor<never>;

const REGISTRY: Partial<Record<VizType, VizDescriptor<never>>> = {
  ...Object.fromEntries(CARTESIAN_VIZ.map((k) => [k, entry(cartesianDescriptor(k))])),
  pivot_table: entry(pivotDescriptor()),
  table: entry(tableDescriptor()),
  object: entry(objectDescriptor()),
  map: entry(mapDescriptor()),
};

export const getDescriptor = (viz: VizType) => REGISTRY[viz];
export const hasDescriptor = (viz: VizType) => REGISTRY[viz] !== undefined;
