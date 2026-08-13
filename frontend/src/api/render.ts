import type { RenderCardData, RenderCardRequest, RenderPayload } from "./generated/model";

/**
 * Step one: what is being rendered.
 *
 * Titles and saved specs, no rows — the page cannot resolve a dashboard's
 * parameters against a query the server has already run. This step does NOT
 * spend the token (`RenderTokenRepo::peek`), so it is safe to reason about it
 * as a plain read; [`fetchRenderData`] is the call that burns it.
 *
 * The token rides in a header, not the path: the navigation already had to put
 * one in a URL, and this request is ours to control.
 */
export async function fetchRenderPayload(token: string): Promise<RenderPayload> {
  const res = await fetch("/api/render/payload", {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`render payload: ${res.status}`);
  return (await res.json()) as RenderPayload;
}

/**
 * Step two: the rows, for the filters this page resolved.
 *
 * One call for every card rather than one each, because this is the call that
 * spends the token — the render gets its data exactly once, which is what keeps
 * a screenshot service from being a data tap.
 *
 * The body carries FILTERS, never a query. The server merges them onto each
 * card's saved spec and validates every column against the dataset, the same
 * way the interactive dashboard's card-data endpoint does with the output of
 * the same `applyParamsToChart`. No retry: a second attempt 404s rather than
 * recovering.
 */
export async function fetchRenderData(
  token: string,
  cards: RenderCardRequest[],
): Promise<RenderCardData[]> {
  const res = await fetch("/api/render/data", {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify(cards),
  });
  if (!res.ok) throw new Error(`render data: ${res.status}`);
  return (await res.json()) as RenderCardData[];
}
