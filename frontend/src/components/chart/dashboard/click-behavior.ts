import type { DashboardParameter } from "@/api/dashboards";
import type { DrillContext } from "@/components/chart/drill";
import { interpolateParams } from "./params";

export type ClickBehavior =
  | { type: "menu" }
  | { type: "crossfilter"; parameterMapping: Record<string, string> }   // { [parameterId]: clickedColumn }
  | { type: "url"; template: string };

export type ResolvedClick =
  | { kind: "url"; href: string | null }
  | { kind: "crossfilter"; sets: { slug: string; value: unknown }[] }
  | { kind: "menu"; ctx: DrillContext };

const SAFE_URL = /^https?:\/\//i;
const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function interpolateUrl(template: string, ctx: DrillContext, parameters: DashboardParameter[], values: Record<string, unknown>): string {
  let out = template;
  const primary = ctx.dims[0];
  out = out.replace(/\{\{\s*value\s*\}\}/gi, primary ? String(primary.value ?? "") : "");
  for (const d of ctx.dims) {
    out = out.replace(new RegExp(`\\{\\{\\s*${escapeRe(d.col)}\\s*\\}\\}`, "g"), String(d.value ?? ""));
  }
  return interpolateParams(out, parameters, values);   // {{slug}} for dashboard params
}

export function resolveClick(
  cb: ClickBehavior | undefined,
  ctx: DrillContext,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
): ResolvedClick {
  if (cb?.type === "url") {
    const href = interpolateUrl(cb.template, ctx, parameters, values);
    return { kind: "url", href: SAFE_URL.test(href) ? href : null };
  }
  if (cb?.type === "crossfilter") {
    const byId = new Map(parameters.map((p) => [p.id, p]));
    const sets: { slug: string; value: unknown }[] = [];
    for (const [paramId, col] of Object.entries(cb.parameterMapping ?? {})) {
      const dim = ctx.dims.find((d) => d.col === col);
      const param = byId.get(paramId);
      if (dim && param) sets.push({ slug: param.slug, value: dim.value });
    }
    return { kind: "crossfilter", sets };
  }
  return { kind: "menu", ctx };
}
