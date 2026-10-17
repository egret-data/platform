/**
 * Where an object of the Data area lives in the URL (spec §2.1):
 * `/governance/data/:catalog/:schema/:object?column=<name>&tab=<id>`.
 *
 * The catalog is its registry NAME, not its id: registry names are unique,
 * and a name is what a person reads, types and shares. The page maps it to
 * `catalog_id` through the registry list.
 *
 * Each name is one path segment, `encodeURIComponent`'d, so a space, a `%` or
 * a `/` inside a name stays inside its segment. react-router decodes each
 * segment back before it hands out params (it keeps an encoded `/` as `%2F`
 * while matching, then restores it), so `urlToPath` reads params as they come.
 * The one name this cannot carry is one that literally spells `%2F`: the
 * router turns it into `/` on the way back.
 */
export type ObjectPath = { catalog: string; schema?: string; object?: string; column?: string };

export const DATA_BASE = "/governance/data";

/** The URL of `p`. A column needs an object, and an object a schema; a level without its parent is dropped. */
export function pathToUrl(p: ObjectPath, tab?: string): string {
  const segments = [p.catalog];
  if (p.schema !== undefined) {
    segments.push(p.schema);
    if (p.object !== undefined) segments.push(p.object);
  }
  const search = new URLSearchParams();
  if (p.schema !== undefined && p.object !== undefined && p.column !== undefined) search.set("column", p.column);
  if (tab) search.set("tab", tab);
  const query = search.toString();
  return `${DATA_BASE}/${segments.map(encodeURIComponent).join("/")}${query ? `?${query}` : ""}`;
}

/** The object a Data URL names, from the router's params and the search. */
export function urlToPath(params: Record<string, string | undefined>, search: URLSearchParams): ObjectPath {
  const p: ObjectPath = { catalog: params.catalog ?? "" };
  if (params.schema !== undefined) p.schema = params.schema;
  if (params.object !== undefined) p.object = params.object;
  const column = search.get("column");
  if (p.object !== undefined && column !== null) p.column = column;
  return p;
}

/**
 * A typed `catalog.schema.object`, as the "Go to path" box takes it. The first
 * part is the catalog and, with three or more parts, the last is the object;
 * whatever lies between is the schema, dotted, as a nested egret namespace is.
 * Null when the text is empty or a part is.
 */
export function parseDottedPath(text: string): ObjectPath | null {
  const parts = text.trim().split(".");
  if (parts.some((part) => part === "")) return null;
  const [catalog, ...rest] = parts;
  if (rest.length === 0) return { catalog };
  if (rest.length === 1) return { catalog, schema: rest[0] };
  return { catalog, schema: rest.slice(0, -1).join("."), object: rest[rest.length - 1] };
}
