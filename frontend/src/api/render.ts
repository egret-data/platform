import type { RenderPayload } from "./generated/model";

/** The print page's ONLY authenticated call. The token rides in a header, not
 *  the path: the navigation already had to put one in a URL, and this request
 *  is ours to control. No retry lives here — the token is single-use, so a
 *  second attempt would 404 rather than recover. */
export async function fetchRenderPayload(token: string): Promise<RenderPayload> {
  const res = await fetch("/api/render/payload", {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`render payload: ${res.status}`);
  return (await res.json()) as RenderPayload;
}
