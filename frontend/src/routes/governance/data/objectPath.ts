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
 *
 * A name that is only dots (`.`, `..`) would be a dot-segment: react-router's
 * `resolvePath` and the browser's own URL parser (which also reads `%2E` as a
 * dot) both collapse it, and the URL would name the parent instead. Such a
 * segment is written with each dot as `%252E`, which neither touches; the
 * router hands it back as `%2E`, and `urlToPath` turns that back into dots. So
 * a name that literally spells only `%2E`s is read back as dots.
 */
export type ObjectPath = { catalog: string; schema?: string; object?: string; column?: string };

export const DATA_BASE = "/governance/data";

/** A rule's page, opened from the Data page at `back`, so its way back returns there (`?from=data:`, spec C1). */
export function dataRuleUrl(ruleId: string, back: string): string {
  return `/governance/rules/${encodeURIComponent(ruleId)}?from=${encodeURIComponent(`data:${encodeURIComponent(back)}`)}`;
}

/** A grant's page, opened from the Data page at `back`, as {@link dataRuleUrl} (KI-131). */
export function dataGrantUrl(grantId: string, back: string): string {
  return `/governance/grants/${encodeURIComponent(grantId)}?from=${encodeURIComponent(`data:${encodeURIComponent(back)}`)}`;
}

/**
 * The way back a `?from=data:<url-encoded path>` names: that Data-area page,
 * labelled by its last path segment. Only a path inside the Data area is
 * followed; anything else (an absolute URL, `//host`, an empty or `..`
 * segment) is `null`, so `from` cannot send the reader off egret or out of
 * the Data area. `encoded` is what follows `data:`.
 */
export function dataBackFrom(encoded: string): { to: string; label: string } | null {
  let to: string;
  try {
    to = decodeURIComponent(encoded);
  } catch {
    return null;
  }
  if (!to.startsWith(`${DATA_BASE}/`)) return null;
  const segments = to.slice(DATA_BASE.length + 1).split(/[?#]/)[0].split("/");
  // An empty, `.` or `..` segment (raw or encoded) is not a Data page; a router would resolve it elsewhere.
  const dotty = (s: string) => /^(\.|%2e){1,2}$/i.test(s);
  if (segments.some((s) => s === "" || dotty(s))) return null;
  const segment = segments[segments.length - 1];
  let label = segment;
  try {
    label = decodeURIComponent(segment);
  } catch {
    // A segment that is not valid percent-encoding reads as written.
  }
  return { to, label };
}

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
  return `${DATA_BASE}/${segments.map(encodeSegment).join("/")}${query ? `?${query}` : ""}`;
}

const DOTS_ONLY = /^\.+$/;
const ENCODED_DOTS_ONLY = /^(%2E)+$/i;

/** One name as one path segment: encoded, and a dot-only name kept from collapsing (see the module doc). */
export function encodeSegment(name: string): string {
  return DOTS_ONLY.test(name) ? name.replace(/\./g, "%252E") : encodeURIComponent(name);
}

/** A segment as the router hands it back, with a dot-only name restored. */
export function decodeSegment(param: string): string {
  return ENCODED_DOTS_ONLY.test(param) ? ".".repeat(param.length / 3) : param;
}

/** The object a Data URL names, from the router's params and the search. */
export function urlToPath(params: Record<string, string | undefined>, search: URLSearchParams): ObjectPath {
  const p: ObjectPath = { catalog: decodeSegment(params.catalog ?? "") };
  if (params.schema !== undefined) p.schema = decodeSegment(params.schema);
  if (params.object !== undefined) p.object = decodeSegment(params.object);
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
