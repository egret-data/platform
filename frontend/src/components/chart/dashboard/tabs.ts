import type { CardInput, DashboardCard, DashboardTab } from "@/api/dashboards";
import { withoutInlineParameters } from "./inlineParameters";

/**
 * A tab in the edit-mode draft.
 *
 * `key` is a client-only handle: the draft reorders, renames and deletes tabs freely, and a tab
 * the user has just added has no server id at all, so the draft cannot key off one.
 *
 * `serverId` is the id the tab already has, carried through so the save can UPDATE it rather than
 * replace it. It used to be dropped here — the comment on this line read "server ids churn on
 * save", which was true and was the defect: `dashboard_cards.tab_id` cascades on delete, so
 * recreating a tab recreated every card on it.
 */
export interface DraftTab { key: string; name: string; position: number; serverId?: string }

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
    return { key, name: t.name, position: i, serverId: t.id };
  });
  const draftCards = cards.map((c) => ({
    ...c, tabKey: c.tab_id ? (keyByServerId.get(c.tab_id) ?? null) : null,
  }));
  return { draftTabs, draftCards };
}

/**
 * A server id, as opposed to one this session invented.
 *
 * The draft fabricates ids for cards the user has just added — `new-<chart>-3`, `new-dup-1`,
 * `new-section-0` — so the grid has something to key on before anything is saved. Those must not
 * be sent: the field is a `Option<Uuid>` on the wire, so a `new-…` string is not "ignored", it
 * fails to deserialize and takes the whole save down with a 400.
 *
 * Tested for SHAPE rather than for the `new-` prefix, because the prefix is a detail of how
 * `DashboardView` names its drafts and this must not silently start sending garbage the day
 * somebody renames it.
 */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const serverId = (id: string | undefined): string | undefined =>
  id !== undefined && UUID.test(id) ? id : undefined;

/** Serialize the draft into the PUT body shape: tabs ordered by position, each card's
 *  `tab_index` resolved from its `tabKey` (or null when there are no tabs).
 *
 *  Cards and tabs carry their existing `id` so the server updates them in place instead of
 *  recreating them — see `replace_layout`. A draft-only row sends none and is inserted. */
export function serializeLayout(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
): { tabs: { id?: string; name: string; position: number }[]; cards: CardInput[] } {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const indexByKey = new Map(ordered.map((t, i) => [t.key, i] as const));
  return {
    tabs: ordered.map((t, i) => ({ id: serverId(t.serverId), name: t.name, position: i })),
    cards: draftCards.map((c) => ({
      id: serverId(c.id),
      chart_id: c.chart_id, card_type: c.card_type,
      grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options,
      tab_index: c.tabKey != null ? (indexByKey.get(c.tabKey) ?? null) : null,
    })),
  };
}

/** Add a tab. On a tabless dashboard that has cards, the first add creates Tab 1 (holding the
 *  existing cards) + an empty Tab 2 and activates Tab 2 (Metabase's first-tab behaviour);
 *  every later add appends one empty tab and activates it. Returns the new active position. */
export function addTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
): { draftTabs: DraftTab[]; draftCards: DraftCard[]; activeTabPos: number } {
  if (draftTabs.length === 0) {
    const tab1 = { key: newTabKey(), name: "Tab 1", position: 0 };
    const tab2 = { key: newTabKey(), name: "Tab 2", position: 1 };
    return {
      draftTabs: [tab1, tab2],
      draftCards: draftCards.map((c) => ({ ...c, tabKey: tab1.key })),
      activeTabPos: 1,
    };
  }
  const next = [...draftTabs, { key: newTabKey(), name: `Tab ${draftTabs.length + 1}`, position: draftTabs.length }];
  return { draftTabs: next, draftCards, activeTabPos: next.length - 1 };
}

/** Rename the tab with `key`. */
export function renameTab(draftTabs: DraftTab[], key: string, name: string): DraftTab[] {
  return draftTabs.map((t) => (t.key === key ? { ...t, name } : t));
}

/** Move the tab at `fromPos` to `toPos` (positions renumbered 0..n-1). */
export function reorderTab(draftTabs: DraftTab[], fromPos: number, toPos: number): DraftTab[] {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const [moved] = ordered.splice(fromPos, 1);
  ordered.splice(toPos, 0, moved);
  return ordered.map((t, i) => ({ ...t, position: i }));
}

/** Next free grid row on a tab: below the current max (row + height) of its cards. */
function nextRowFor(cards: DraftCard[]): number {
  return cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
}

/** Delete the tab with `key`. `reassign` moves its cards to `toKey`; `delete` drops them.
 *  Positions on the remaining tabs are renumbered. (Callers gate this to >= 2 tabs.) */
export function deleteTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
  key: string,
  opts: { mode: "reassign"; toKey: string } | { mode: "delete" },
): { draftTabs: DraftTab[]; draftCards: DraftCard[] } {
  if (draftTabs.length < 2) return { draftTabs, draftCards };
  const remaining = draftTabs
    .filter((t) => t.key !== key)
    .sort((a, b) => a.position - b.position)
    .map((t, i) => ({ ...t, position: i }));
  let cards: DraftCard[];
  if (opts.mode === "reassign") {
    const dest = draftCards.filter((c) => c.tabKey === opts.toKey);
    let row = nextRowFor(dest);
    cards = draftCards.map((c) => {
      if (c.tabKey !== key) return c;
      const placed = { ...c, tabKey: opts.toKey, grid_row: row };
      row += c.size_y;
      return placed;
    });
  } else {
    cards = draftCards.filter((c) => c.tabKey !== key);
  }
  return { draftTabs: remaining, draftCards: cards };
}

/** Duplicate the tab with `key`: a new tab inserted right after it, holding deep copies of its
 *  cards (fresh client ids, same grid). Activates the new tab. */
export function duplicateTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
  key: string,
): { draftTabs: DraftTab[]; draftCards: DraftCard[]; activeTabPos: number } {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const srcPos = ordered.findIndex((t) => t.key === key);
  const newKey = newTabKey();
  const newTab = { key: newKey, name: `${ordered[srcPos].name} copy`, position: srcPos + 1 };
  const next = [...ordered.slice(0, srcPos + 1), newTab, ...ordered.slice(srcPos + 1)]
    .map((t, i) => ({ ...t, position: i }));
  const clones = draftCards
    .filter((c) => c.tabKey === key)
    // A clone must not claim the original's inline filters — two cards holding one parameter id
    // is not a state the model can render. See inlineParameters.ts.
    .map((c) => ({ ...c, id: `new-dup-${seq++}`, tabKey: newKey, options: withoutInlineParameters(c.options) }));
  return { draftTabs: next, draftCards: [...draftCards, ...clones], activeTabPos: srcPos + 1 };
}

/** Move the card `cardId` to tab `toKey`, stacking it below that tab's current max row. */
export function moveCardToTab(draftCards: DraftCard[], cardId: string, toKey: string): DraftCard[] {
  const row = nextRowFor(draftCards.filter((c) => c.tabKey === toKey));
  return draftCards.map((c) => (c.id === cardId ? { ...c, tabKey: toKey, grid_col: 0, grid_row: row } : c));
}
