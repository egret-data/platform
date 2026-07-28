import { CARTESIAN_VIZ } from "@/api/charts";
import type { VizType } from "@/api/charts";
import { cartesianDescriptor } from "./cartesian";
import { pivotDescriptor } from "./pivot";
import { tableDescriptor, objectDescriptor } from "./table";
import { mapDescriptor } from "./map";
import { SIMPLE_DESCRIPTORS } from "./simple";
import type { VizDescriptor } from "./types";

/** Every viz type, each owning its own configuration. The bridge is gone. */
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
  ...Object.fromEntries(SIMPLE_DESCRIPTORS.map((d) => [d.key, entry(d)])),
};

export const getDescriptor = (viz: VizType) => REGISTRY[viz];
export const hasDescriptor = (viz: VizType) => REGISTRY[viz] !== undefined;
