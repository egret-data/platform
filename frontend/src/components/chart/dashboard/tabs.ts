import type { CardInput, DashboardCard, DashboardTab } from "@/api/dashboards";

/** A tab in the edit-mode draft. `key` is a client-only id (server ids churn on save). */
export interface DraftTab { key: string; name: string; position: number }

/** A card in the edit-mode draft: the API card plus its draft tab link (`tabKey`). */
export type DraftCard = DashboardCard & { tabKey: string | null };

let seq = 0;
/** A fresh client tab key. Deterministic prefix; unique per call. */
export function newTabKey(): string { return `tab-${seq++}`; }

/** Build the edit draft from the server payload: one DraftTab per server tab (fresh key),
 *  and each card's server `tab_id` mapped to that tab's key. */
export function startTabsDraft(
  tabs: DashboardTab[],
  cards: DashboardCard[],
): { draftTabs: DraftTab[]; draftCards: DraftCard[] } {
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const keyByServerId = new Map<string, string>();
  const draftTabs = ordered.map((t, i) => {
    const key = newTabKey();
    keyByServerId.set(t.id, key);
    return { key, name: t.name, position: i };
  });
  const draftCards = cards.map((c) => ({
    ...c, tabKey: c.tab_id ? (keyByServerId.get(c.tab_id) ?? null) : null,
  }));
  return { draftTabs, draftCards };
}

/** Serialize the draft into the PUT body shape: tabs ordered by position, each card's
 *  `tab_index` resolved from its `tabKey` (or null when there are no tabs). */
export function serializeLayout(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
): { tabs: { name: string; position: number }[]; cards: CardInput[] } {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const indexByKey = new Map(ordered.map((t, i) => [t.key, i] as const));
  return {
    tabs: ordered.map((t, i) => ({ name: t.name, position: i })),
    cards: draftCards.map((c) => ({
      chart_id: c.chart_id, card_type: c.card_type,
      grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options,
      tab_index: c.tabKey != null ? (indexByKey.get(c.tabKey) ?? null) : null,
    })),
  };
}
