import { useSearchParams } from "react-router-dom";
import type { DashboardParameter } from "@/api/dashboards";

export interface DashboardParamsState {
  values: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
}

/** Parameter types whose value is an ARRAY, and so is JSON-encoded in the URL. `location` is
 *  multi-select (`FilterPill.tsx`) and was missing here, so a multi-value location was written
 *  as JSON and read back as the JSON *string* — `paramToFilters` then emitted
 *  `in ['["Hanoi","Da Nang"]']` and the card silently returned no rows. */
const ARRAY_TYPES = new Set(["category", "location", "id"]);

/** Read/write each parameter's value in the URL by slug. Arrays/ranges are JSON-encoded. */
export function useDashboardParams(parameters: DashboardParameter[]): DashboardParamsState {
  const [params, setParams] = useSearchParams();
  const values: Record<string, unknown> = {};
  for (const p of parameters) {
    const raw = params.get(p.slug);
    if (raw == null) {
      if (p.default !== undefined) values[p.slug] = p.default;
      continue;
    }
    if (ARRAY_TYPES.has(p.type) || p.operator === "between" || p.operator === "range") {
      // Decode only what `setValue` actually encoded. It JSON-encodes objects and arrays and
      // writes everything else through String(), so a raw that is not array-shaped is a scalar —
      // a hand-edited URL, or a drill/crossfilter write from `DashboardView.tsx`. Parsing it
      // anyway turned "42" into the NUMBER 42 and the resulting `in [42]` matched nothing.
      if (raw.startsWith("[")) {
        try { values[p.slug] = JSON.parse(raw); } catch { values[p.slug] = raw; }
      } else {
        values[p.slug] = raw;
      }
    } else {
      values[p.slug] = raw;
    }
  }
  const setValue = (slug: string, value: unknown) =>
    setParams((prev) => {
      const n = new URLSearchParams(prev);
      const blank = value === undefined || value === null || value === "" || (Array.isArray(value) && value.length === 0);
      if (blank) n.delete(slug);
      else n.set(slug, typeof value === "object" ? JSON.stringify(value) : String(value));
      return n;
    }, { replace: true });
  return { values, setValue };
}
