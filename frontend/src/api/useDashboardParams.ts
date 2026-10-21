import { useSearchParams } from "react-router-dom";
import type { DashboardParameter } from "@/api/dashboards";

export interface DashboardParamsState {
  values: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
  setValues: (patch: Record<string, unknown>) => void;
  /** Does any filter differ from its default, or has a viewer cleared one that has a default?
   *  Drives whether "Reset all filters" is offered. A URL that names exactly the default is not a
   *  change, and a cleared default IS one (KI-061). */
  canReset: boolean;
  /** Back to every filter's default, in ONE history write: drops every parameter slug from the URL
   *  and leaves the view controls (`refresh`, `fullscreen`) alone. One write because a loop of
   *  per-slug writes would hand each call the same `prev` and only the last would land. */
  reset: () => void;
}

/** Parameter types whose value is an ARRAY, and so is JSON-encoded in the URL. `location` is
 *  multi-select (`FilterPill.tsx`) and was missing here, so a multi-value location was written
 *  as JSON and read back as the JSON *string* — `paramToFilters` then emitted
 *  `in ['["Hanoi","Da Nang"]']` and the card silently returned no rows. */
const ARRAY_TYPES = new Set(["category", "location", "id"]);

const isArrayParam = (p: DashboardParameter) =>
  ARRAY_TYPES.has(p.type) || p.operator === "between" || p.operator === "range";
const isObjectParam = (p: DashboardParameter) => p.type === "date" && p.operator === "all-options";

const isBlankValue = (v: unknown) =>
  v === undefined || v === null || v === "" || (Array.isArray(v) && v.length === 0);

/** A filter that has a default can be cleared by a viewer, and the clear has to outlive a reload
 *  and a shared link, or the default comes straight back (KI-061). So a cleared default is written
 *  as the slug with an EMPTY value (`?order_date=`); a filter without a default simply loses its
 *  slug. A REQUIRED filter cannot be emptied: clearing one returns it to its default, which is the
 *  only value the cards can run on. */
const clearsToEmpty = (p: DashboardParameter | undefined) =>
  !!p && !p.required && !isBlankValue(p.default);

/** The URL text for one value. Strings and numbers are written as they are; arrays and objects
 *  (a range, a multi-select, a date picker's `{mode, …}`) as JSON. */
function encode(value: unknown): string {
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

/** Read back what `encode` wrote, by the shape the parameter's control emits. Only the shape a
 *  parameter can hold is decoded: a raw that is not array-shaped on a list type is a scalar — a
 *  hand-edited URL, or a drill/crossfilter write from `DashboardView.tsx` — and parsing it anyway
 *  turned "42" into the NUMBER 42, whose `in [42]` matched nothing. */
function decode(p: DashboardParameter, raw: string): unknown {
  if (isArrayParam(p) && raw.startsWith("[")) {
    try { return JSON.parse(raw); } catch { return raw; }
  }
  // The all-options date picker emits an object (KI-060). Reading it back as a string left the
  // chip blank and the cards unfiltered.
  if (isObjectParam(p) && raw.startsWith("{")) {
    try {
      const v: unknown = JSON.parse(raw);
      return v && typeof v === "object" && !Array.isArray(v) ? v : raw;
    } catch { return raw; }
  }
  // The number input emits a number; give it back as one.
  if (p.type === "number" && !isArrayParam(p) && raw.trim() !== "") {
    const n = Number(raw);
    return Number.isFinite(n) ? n : raw;
  }
  return raw;
}

/** Compare two values the way a viewer sees them: all blank spellings are one, and arrays and
 *  objects by content (a value read from the URL is a new object every render). */
function sameValue(a: unknown, b: unknown): boolean {
  if (isBlankValue(a) && isBlankValue(b)) return true;
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Read/write each parameter's value in the URL by slug. See `encode`, `decode` and
 *  `clearsToEmpty` for the encoding. */
export function useDashboardParams(parameters: DashboardParameter[]): DashboardParamsState {
  const [params, setParams] = useSearchParams();
  const bySlug = new Map(parameters.map((p) => [p.slug, p]));
  const values: Record<string, unknown> = {};
  for (const p of parameters) {
    const raw = params.get(p.slug);
    if (raw == null) {
      if (p.default !== undefined) values[p.slug] = p.default;
      continue;
    }
    // `?slug=` is a cleared default: no value, and NOT the default.
    if (raw === "" && clearsToEmpty(p)) continue;
    if (raw === "") {
      if (p.default !== undefined) values[p.slug] = p.default;
      continue;
    }
    values[p.slug] = decode(p, raw);
  }
  /** Write several parameters in ONE URL update. `setValue` in a loop does NOT work: each call
   *  reads the same current location, so the last write wins and the rest vanish. Apply writes
   *  every drafted filter at once, which is exactly that case. */
  const setValues = (patch: Record<string, unknown>) =>
    setParams((prev) => {
      const n = new URLSearchParams(prev);
      for (const [slug, value] of Object.entries(patch)) {
        if (!isBlankValue(value)) n.set(slug, encode(value));
        else if (clearsToEmpty(bySlug.get(slug))) n.set(slug, "");
        else n.delete(slug);
      }
      return n;
    }, { replace: true });
  const setValue = (slug: string, value: unknown) => setValues({ [slug]: value });
  const canReset = parameters.some((p) => !sameValue(values[p.slug], p.default));
  const reset = () =>
    setParams((prev) => {
      const n = new URLSearchParams(prev);
      for (const p of parameters) n.delete(p.slug);
      return n;
    }, { replace: true });
  return { values, setValue, setValues, canReset, reset };
}
