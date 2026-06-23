import type { DashboardParameter } from "@/api/dashboards";

type CardLike = { id: string; card_type: string; chart_id: string | null; options: Record<string, unknown> };
type ChartLike = { dataset_id: string };

export type ResolvedSource =
  | { kind: "static"; values: string[] }
  | { kind: "query"; datasetId: string; column: string }
  | null;

/** Resolve a parameter's value source to either a static list or a (dataset, column) query target. */
export function resolveValuesSource(
  param: DashboardParameter,
  cards: CardLike[],
  chartById: Map<string, ChartLike>,
): ResolvedSource {
  const src = param.valuesSource;
  if (src?.type === "static") return { kind: "static", values: src.static ?? [] };
  if (src?.type === "card") {
    if (!src.cardId || !src.column) return null;
    const card = cards.find((c) => c.id === src.cardId && c.card_type === "chart");
    const chart = card?.chart_id ? chartById.get(card.chart_id) : undefined;
    if (!chart) return null;
    return { kind: "query", datasetId: chart.dataset_id, column: src.column };
  }
  // distinct (default)
  const card = cards.find((c) => c.card_type === "chart" && (((c.options.paramMappings as Record<string, string>) ?? {})[param.id]));
  const chart = card?.chart_id ? chartById.get(card.chart_id) : undefined;
  const column = card ? ((card.options.paramMappings as Record<string, string>) ?? {})[param.id] : undefined;
  if (!chart || !column) return null;
  return { kind: "query", datasetId: chart.dataset_id, column };
}
