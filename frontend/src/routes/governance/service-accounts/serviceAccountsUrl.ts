/** URL helpers for the Service accounts area (SA3), mounted at `/governance/service-accounts`. */
export const SERVICE_ACCOUNTS_BASE = "/governance/service-accounts";

export type SaScope = "platform" | "workspace";

/** The list, with its shareable filters (FE-PAGE-2): `?scope=` and `?workspace=`. */
export function serviceAccountsUrl(filters: { scope?: SaScope | null; workspace?: string | null } = {}): string {
  const q = new URLSearchParams();
  if (filters.scope) q.set("scope", filters.scope);
  if (filters.scope === "workspace" && filters.workspace) q.set("workspace", filters.workspace);
  const s = q.toString();
  return s ? `${SERVICE_ACCOUNTS_BASE}?${s}` : SERVICE_ACCOUNTS_BASE;
}

/** One service account's page. */
export function serviceAccountUrl(id: string): string {
  return `${SERVICE_ACCOUNTS_BASE}/${encodeURIComponent(id)}`;
}
