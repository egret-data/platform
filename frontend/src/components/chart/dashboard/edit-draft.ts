import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "./tabs";

/** Everything `startEdit()` puts into the draft, and therefore everything Cancel throws away
 *  (DashboardView.tsx:262-272). Snapshotting the DRAFT rather than the server payload is
 *  deliberate: `startTabsDraft` mints fresh client tab keys and rewrites every card's tab link,
 *  so the two are not the same shape and comparing them would mean running that transform twice.
 *
 *  `width` is here because W3 (fixed/full width) landed after this workstream was planned and
 *  added a fifth draft field: `startEdit` seeds `draftWidth` from the server and `onSave` sends
 *  it, so flipping the switch and pressing Cancel loses real work. */
export interface DashboardDraft {
  name: string;
  width: "fixed" | "full";
  cards: DraftCard[];
  parameters: DashboardParameter[];
  tabs: DraftTab[];
}

/** JSON with object keys sorted at every depth.
 *
 *  `JSON.stringify` preserves INSERTION order, and a card's `options` is a free-form bag written by
 *  `updateCardOptions` as `{ ...c.options, ...patch }` — which reorders keys the moment a patch
 *  overwrites one that is already there. Two drafts that differ only in that order are the same
 *  draft, so the comparison has to be blind to it or opening a settings panel and changing nothing
 *  would report a dirty dashboard and prompt on Cancel. */
function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null";
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    // An absent key and an explicit `undefined` are the same draft: `{ ...p }` copies of a
    // parameter carry whichever optional keys the server sent, and a fresh one carries none.
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${stableStringify(v)}`).join(",")}}`;
}

/** The comparable projection of one draft card. Named fields, not a spread: `DraftCard` also
 *  carries `tab_id`, the SERVER tab id, which `startTabsDraft` copies through unchanged while
 *  `tabKey` is the draft's own link (tabs.ts:26-28). Comparing `tab_id` would make a card moved
 *  between draft tabs look unchanged, and comparing both would make a card on a newly created tab
 *  look changed for a field the draft never edits. */
function cardShape(c: DraftCard) {
  return {
    id: c.id,
    chart_id: c.chart_id,
    card_type: c.card_type,
    grid_col: c.grid_col,
    grid_row: c.grid_row,
    size_x: c.size_x,
    size_y: c.size_y,
    options: c.options,
    tabKey: c.tabKey,
  };
}

/** A stable string identity for a draft. Equal fingerprints mean Save would write the same bytes.
 *
 *  Cards are sorted by id (array order is meaningless — position lives in grid_col/grid_row, and
 *  only `addCard`/`duplicateCard` ever append). Parameters are NOT sorted: filter chips render in
 *  creation order (DashboardFilterBar.tsx), so reordering them is a real change. Tabs are
 *  sorted by `position`, which is their own source of truth (`reorderTab` renumbers it). */
export function draftFingerprint(draft: DashboardDraft): string {
  return stableStringify({
    // onSave writes `draftName.trim()`, so trailing whitespace alone is not a change.
    name: draft.name.trim(),
    width: draft.width,
    cards: [...draft.cards]
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
      .map(cardShape),
    parameters: draft.parameters,
    tabs: [...draft.tabs]
      .sort((a, b) => a.position - b.position)
      .map((t) => ({ key: t.key, name: t.name, position: t.position })),
  });
}

/** Would Cancel lose anything? Compares the live draft against the snapshot `startEdit()` took. */
export function isDraftDirty(baseline: DashboardDraft, current: DashboardDraft): boolean {
  return draftFingerprint(baseline) !== draftFingerprint(current);
}
