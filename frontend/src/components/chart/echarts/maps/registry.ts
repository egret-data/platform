// eslint-disable-next-line @typescript-eslint/no-explicit-any
type GeoJSON = unknown;

export interface MapDef {
  key: string;
  label: string;
  echartsName: string;
  load: () => Promise<GeoJSON>;
  normalize: (v: unknown) => string;
  /** Fixed text labels drawn at geo coordinates [lon, lat] (e.g. archipelagos). */
  annotations?: { name: string; coord: [number, number] }[];
}

const upper = (v: unknown) => String(v ?? "").trim().toUpperCase();
const trim = (v: unknown) => String(v ?? "").trim();

/**
 * A region column may hold either the geojson's KEY or its NAME, and egret only ever matched the
 * key. `us_states` is keyed by two-letter code, so a column of "California" matched nothing and the
 * map drew every state in the palest shade with no message at all — the obvious thing to feed it
 * failed silently.
 *
 * Metabase keeps hand-written name→code tables for US states and world countries. This derives the
 * same mapping from the geojson that is already being loaded, so it cannot drift from the shapes it
 * has to match, and it costs no extra bytes. Metabase's own note applies: a name and a key never
 * collide, so accepting both is unambiguous.
 */
const nameToKey = new Map<string, Map<string, string>>();

export function indexRegionNames(mapKey: string, geojson: unknown): void {
  const features = (geojson as { features?: { properties?: Record<string, unknown> }[] }).features ?? [];
  const index = new Map<string, string>();
  for (const f of features) {
    const key = f.properties?.key;
    const name = f.properties?.name;
    if (typeof key !== "string") continue;
    index.set(key.trim().toLowerCase(), key);
    if (typeof name === "string") index.set(name.trim().toLowerCase(), key);
  }
  nameToKey.set(mapKey, index);
}

/** Resolve a raw cell to the geojson key it names, by key or by name; falls back to the map's own
 *  normaliser so a chart still renders before the geojson has been indexed. */
export function resolveRegion(def: MapDef, value: unknown): string {
  const index = nameToKey.get(def.key);
  const hit = index?.get(String(value ?? "").trim().toLowerCase());
  return hit ?? def.normalize(value);
}

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
    annotations: [
      { name: "Hoàng Sa", coord: [112.4, 16.5] },
      { name: "Trường Sa", coord: [114.2, 9.5] },
    ],
  },
};
