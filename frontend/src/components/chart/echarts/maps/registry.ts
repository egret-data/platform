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

/** The geojson keys of a map, once it has been indexed; undefined while it is still loading. */
export function regionKeys(mapKey: string): ReadonlySet<string> | undefined {
  const index = nameToKey.get(mapKey);
  return index && new Set(index.values());
}

/** Resolve a raw cell to the geojson key it names, by key or by name; falls back to the map's own
 *  normaliser so a chart still renders before the geojson has been indexed. */
export function resolveRegion(def: MapDef, value: unknown): string {
  const index = nameToKey.get(def.key);
  const hit = index?.get(String(value ?? "").trim().toLowerCase());
  return hit ?? def.normalize(value);
}

/** The lon/lat box a map's shapes occupy. `[minLon, minLat]`–`[maxLon, maxLat]`, in degrees. */
export interface RegionExtent { minLon: number; maxLon: number; minLat: number; maxLat: number }

const extents = new Map<string, RegionExtent>();

/**
 * The geojson's own BOUNDING BOX, derived once when it is loaded — the pin map's answer to
 * "is this coordinate anywhere near this map?".
 *
 * WHY THE BOUNDING BOX IS THE RIGHT QUESTION, and this was measured rather than reasoned. ECharts
 * lays a `geo` out by projecting the map's bounding box into `layoutSize`, so the rectangle the
 * outlines occupy on screen IS that box. SSR-rendering the real pin option against us-states at
 * 540x561 put the geo's view rect at x in [21.5, 518.5], y in [136.9, 424.1] — and every test
 * coordinate inside the box landed inside that rect, every one outside it landed outside. So
 * "inside the box" and "somewhere the reader is looking" are the same predicate, and this one can
 * be answered with no DOM, no layout and no rendered chart, which is what makes it testable at all
 * (jsdom runs no layout, so the projection itself never can be).
 *
 * WHAT IT IS NOT: a point-in-polygon test. A coordinate in the Gulf of Mexico is inside the
 * us-states box and outside every state, and it should be — it draws in the blank sea beside Texas,
 * exactly where it belongs, and calling it "off the map" would be a lie the reader can see through.
 * The box is deliberately the loosest honest test. us-states reaches lon -188.9 because the
 * Aleutians wrap past the antimeridian, which is why Vancouver and Mexico City count as on-map.
 */
export function indexRegionExtent(mapKey: string, geojson: unknown): void {
  const features = (geojson as { features?: { geometry?: { coordinates?: unknown } }[] }).features ?? [];
  let minLon = Infinity, maxLon = -Infinity, minLat = Infinity, maxLat = -Infinity;
  // Recursive because the three bundled geojsons use three different nesting depths — a Polygon is
  // rings of points, a MultiPolygon is polygons of rings of points — and a walk hard-coded to one
  // of them would silently return a box derived from part of the map. The recursion bottoms out on
  // "an array whose first two entries are numbers", which is a position and nothing else is.
  const walk = (node: unknown): void => {
    if (!Array.isArray(node)) return;
    if (typeof node[0] === "number" && typeof node[1] === "number") {
      const [lon, lat] = node as number[];
      if (lon < minLon) minLon = lon;
      if (lon > maxLon) maxLon = lon;
      if (lat < minLat) minLat = lat;
      if (lat > maxLat) maxLat = lat;
      return;
    }
    for (const child of node) walk(child);
  };
  for (const f of features) walk(f.geometry?.coordinates);
  // NO COORDINATES MEANS NO EXTENT, not an inside-out one. Leaving the Infinities in place would
  // hand out a box that contains nothing, and every row on earth would be reported off-map — a
  // confident wrong message, which is worse than the silence this whole change exists to end.
  if (minLon > maxLon || minLat > maxLat) {
    extents.delete(mapKey);
    return;
  }
  extents.set(mapKey, { minLon, maxLon, minLat, maxLat });
}

/** The indexed bounding box, or undefined while the geojson is still loading. Undefined must mean
 *  "no opinion" at every call site: a chart renders before its geojson arrives. */
export function regionExtent(mapKey: string): RegionExtent | undefined {
  return extents.get(mapKey);
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
