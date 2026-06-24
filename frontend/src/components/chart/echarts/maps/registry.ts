// eslint-disable-next-line @typescript-eslint/no-explicit-any
type GeoJSON = unknown;

export interface MapDef {
  key: string;
  label: string;
  echartsName: string;
  load: () => Promise<GeoJSON>;
  normalize: (v: unknown) => string;
}

const upper = (v: unknown) => String(v ?? "").trim().toUpperCase();
const trim = (v: unknown) => String(v ?? "").trim();

export const MAPS: Record<string, MapDef> = {
  us_states: {
    key: "us_states",
    label: "US states",
    echartsName: "us_states",
    load: () => import("./us-states.geo.json").then((m) => m.default as unknown as GeoJSON),
    normalize: upper,
  },
  world: {
    key: "world",
    label: "World countries",
    echartsName: "world",
    load: () => import("./world-countries.geo.json").then((m) => m.default as unknown as GeoJSON),
    normalize: upper,
  },
  vietnam: {
    key: "vietnam",
    label: "Vietnam provinces",
    echartsName: "vietnam",
    load: () => import("./vietnam-provinces.geo.json").then((m) => m.default as unknown as GeoJSON),
    normalize: trim,
  },
};
