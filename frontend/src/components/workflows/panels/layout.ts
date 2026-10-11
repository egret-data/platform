/**
 * THE FLOW EDITOR'S LAYOUT, AS PLAIN DATA (spec §5.1).
 *
 * The editor is a row of panels; each panel holds tabs and shows one. Every
 * operation here is a pure function from a layout to a layout: it never
 * mutates its input, and when it refuses (an unknown tab, closing Code) it
 * returns the SAME object, so a caller can tell "nothing happened" with `===`.
 *
 * The invariants every operation restores before returning:
 * - no panel is empty (a panel whose last tab leaves is removed);
 * - each panel's `active` is one of its own tabs;
 * - sizes are positive and sum to 100.
 *
 * Modelled on Kestra's `MultiPanelTabs.vue` (`moveTab`, `newPanelDrop`,
 * `splitPanel`, `destroyTab`, the empty-panel watcher), which does the same
 * things by mutating a reactive array in place.
 */

export type TabId = string;

export type PanelLayout = {
  id: string;
  /** Percent of the row, 0–100. */
  size: number;
  tabs: TabId[];
  active: TabId;
};

export type Layout = { panels: PanelLayout[] };

/**
 * The one tab that can never be closed: a flow editor with no editor in it is
 * not a state the user can recover from by looking at the screen. `normalise`
 * puts it back when a stored layout lost it.
 */
export const CODE_TAB: TabId = "code";

/** `[Code] | [Docs, Problems]` at 60/40 (spec §5.1). */
export const DEFAULT_FLOW_LAYOUT: Layout = {
  panels: [
    { id: "p1", size: 60, tabs: [CODE_TAB], active: CODE_TAB },
    { id: "p2", size: 40, tabs: ["docs", "problems"], active: "docs" },
  ],
};

const usableSize = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n > 0;

/** Scale sizes to sum to 100. Unusable sizes become an equal share first. */
function rescale(panels: PanelLayout[]): PanelLayout[] {
  if (panels.length === 0) return panels;
  const allUsable = panels.every((p) => usableSize(p.size));
  const raw = allUsable ? panels.map((p) => p.size) : panels.map(() => 1);
  const total = raw.reduce((s, n) => s + n, 0);
  return panels.map((p, i) => ({ ...p, size: (raw[i] / total) * 100 }));
}

/** Drop empty panels, then re-balance what is left. */
function settle(panels: PanelLayout[]): Layout {
  return { panels: rescale(panels.filter((p) => p.tabs.length > 0)) };
}

/**
 * Remove `tab` from `panel`. If it was active, its right-hand neighbour takes
 * over, else its left-hand one — the tab the eye lands on when one closes in
 * a browser.
 */
function without(panel: PanelLayout, tab: TabId): PanelLayout {
  const at = panel.tabs.indexOf(tab);
  if (at === -1) return panel;
  const tabs = panel.tabs.filter((t) => t !== tab);
  const active = panel.active === tab ? (tabs[at] ?? tabs[at - 1] ?? tab) : panel.active;
  return { ...panel, tabs, active };
}

function panelOf(l: Layout, tab: TabId): PanelLayout | undefined {
  return l.panels.find((p) => p.tabs.includes(tab));
}

/** The smallest `pN` no panel uses. Deterministic, so the function stays pure. */
function freshPanelId(l: Layout): string {
  const used = new Set(l.panels.map((p) => p.id));
  let n = 1;
  while (used.has(`p${n}`)) n++;
  return `p${n}`;
}

/**
 * Move `tab` into `toPanel` at `index` of that panel's resulting tab list
 * (clamped), and activate it there. Within one panel this is an array move:
 * `index` is where the tab ends up.
 */
export function moveTab(l: Layout, tab: TabId, toPanel: string, index: number): Layout {
  const from = panelOf(l, tab);
  if (!from || !l.panels.some((p) => p.id === toPanel)) return l;
  const panels = l.panels.map((p) => {
    const rest = p.id === from.id ? without(p, tab) : p;
    if (p.id !== toPanel) return rest;
    const tabs = [...rest.tabs];
    tabs.splice(Math.max(0, Math.min(index, tabs.length)), 0, tab);
    return { ...rest, tabs, active: tab };
  });
  return settle(panels);
}

/**
 * Move `tab` into a new panel on `side` of `besidePanel`. The new panel takes
 * half of `besidePanel`'s width, so the panels the user did not touch keep
 * theirs. Splitting a panel's only tab beside that same panel would remove the
 * panel and put it back where it was, so it is refused.
 */
export function splitTab(l: Layout, tab: TabId, besidePanel: string, side: "left" | "right"): Layout {
  const from = panelOf(l, tab);
  const beside = l.panels.find((p) => p.id === besidePanel);
  if (!from || !beside) return l;
  if (from.id === beside.id && from.tabs.length === 1) return l;
  const half = beside.size / 2;
  const fresh: PanelLayout = { id: freshPanelId(l), size: half, tabs: [tab], active: tab };
  const panels = l.panels.flatMap((p) => {
    const rest = p.id === from.id ? without(p, tab) : p;
    if (p.id !== beside.id) return [rest];
    const halved = { ...rest, size: half };
    return side === "left" ? [fresh, halved] : [halved, fresh];
  });
  return settle(panels);
}

/** Show `tab` in the panel that holds it. */
export function activateTab(l: Layout, tab: TabId): Layout {
  const from = panelOf(l, tab);
  if (!from || from.active === tab) return l;
  return { panels: l.panels.map((p) => (p.id === from.id ? { ...p, active: tab } : p)) };
}

/** Close `tab`. Closing Code is refused. */
export function closeTab(l: Layout, tab: TabId): Layout {
  if (tab === CODE_TAB) return l;
  const from = panelOf(l, tab);
  if (!from) return l;
  return settle(l.panels.map((p) => (p.id === from.id ? without(p, tab) : p)));
}

/** Open a closed `tab` at the end of `panel`, and activate it. */
export function reopenTab(l: Layout, tab: TabId, panel: string): Layout {
  if (panelOf(l, tab) || !l.panels.some((p) => p.id === panel)) return l;
  return {
    panels: l.panels.map((p) => (p.id === panel ? { ...p, tabs: [...p.tabs, tab], active: tab } : p)),
  };
}

/** Apply sizes given in panel order, scaled to 100. A list that does not fit is ignored. */
export function resize(l: Layout, sizes: number[]): Layout {
  if (sizes.length !== l.panels.length || !sizes.every(usableSize)) return l;
  return { panels: rescale(l.panels.map((p, i) => ({ ...p, size: sizes[i] }))) };
}

/**
 * Bring any layout — typically one read back from storage — onto the
 * invariants, against the tabs that exist now (`known`):
 * - unknown tabs are dropped (a tab from a later package that was rolled
 *   back), and so are the panels they leave empty;
 * - a tab listed twice keeps its first place only;
 * - a missing Code tab goes back to the front of the first panel;
 * - an `active` that is not in its panel becomes the panel's first tab;
 * - a duplicated panel id is replaced; unusable sizes become equal shares.
 */
export function normalise(l: Layout, known: TabId[]): Layout {
  const knownSet = new Set(known);
  const seenTabs = new Set<TabId>();
  const seenIds = new Set<string>();
  let panels: PanelLayout[] = [];
  for (const p of l.panels) {
    const tabs = p.tabs.filter((t) => {
      if (!knownSet.has(t) || seenTabs.has(t)) return false;
      seenTabs.add(t);
      return true;
    });
    if (tabs.length === 0) continue;
    panels.push({ ...p, tabs });
  }
  if (knownSet.has(CODE_TAB) && !seenTabs.has(CODE_TAB)) {
    panels = panels.length === 0
      ? [{ id: "p1", size: 100, tabs: [CODE_TAB], active: CODE_TAB }]
      : panels.map((p, i) => (i === 0 ? { ...p, tabs: [CODE_TAB, ...p.tabs] } : p));
  }
  panels = panels.map((p) => {
    const active = p.tabs.includes(p.active) ? p.active : p.tabs[0];
    if (!seenIds.has(p.id)) {
      seenIds.add(p.id);
      return { ...p, active };
    }
    let n = 1;
    while (seenIds.has(`p${n}`) || l.panels.some((q) => q.id === `p${n}`)) n++;
    seenIds.add(`p${n}`);
    return { ...p, id: `p${n}`, active };
  });
  return { panels: rescale(panels) };
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/**
 * The structural check for a value read back from storage: the right shape, or
 * `null`. It checks types only; `normalise` makes the content sound.
 */
export function parseLayout(raw: unknown): Layout | null {
  if (!isObject(raw) || !Array.isArray(raw.panels)) return null;
  const panels: PanelLayout[] = [];
  for (const p of raw.panels) {
    if (!isObject(p)) return null;
    const { id, size, tabs, active } = p;
    if (typeof id !== "string" || typeof size !== "number" || typeof active !== "string") return null;
    if (!Array.isArray(tabs) || !tabs.every((t): t is string => typeof t === "string")) return null;
    panels.push({ id, size, tabs: [...tabs], active });
  }
  return { panels };
}
