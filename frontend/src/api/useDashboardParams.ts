import { useSearchParams } from "react-router-dom";
import type { DashboardParameter } from "@/api/dashboards";

export interface DashboardParamsState {
  values: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
}

const ARRAY_TYPES = new Set(["category"]);

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
      try { values[p.slug] = JSON.parse(raw); } catch { values[p.slug] = raw; }
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
