import { useSyncExternalStore, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import type { TabId } from "@/components/workflows/panels/layout";

/**
 * THE TAB REGISTRY (spec §5.1): each kind of editor tab is registered once,
 * and the frame renders whatever is registered. Task 4 registers Code, Docs
 * and Problems; W7c registers Topology and W7d Files, without touching the
 * frame. Modelled on Kestra's `EDITOR_ELEMENTS` (`panelDefinition.ts`).
 *
 * `C` is what the page hands `<MultiPanel context>`, passed unchanged to every
 * tab's `render`. The registry is one module-level list, so it cannot carry
 * one `C` per tab; `registerTab<C>` and `getTabs<C>` are where that is taken
 * on trust, and the page that mounts the frame names the one `C` its tabs use.
 */
export type TabDefinition<C = unknown> = {
  id: TabId;
  /** Sentence case; the tab's label and its accessible name. */
  title: string;
  icon: LucideIcon;
  /** `false` hides the Close item. Code is also refused by `closeTab` itself. */
  closable: boolean;
  render: (context: C) => ReactNode;
  /**
   * Drawn in the tab after its title, from the same `context`; Problems shows
   * its error count here (spec §5.2). It is part of the tab's accessible name,
   * so give it words for a screen reader, not only a number.
   */
  badge?: (context: C) => ReactNode;
  /**
   * The badge in words ("3 errors"), when it has some. The tab is then named
   * `"<title>, <label>"` by an `aria-label`: Chromium joins a visually hidden
   * count to the title with a space ("Problems , 3 errors"; W7a Task 8).
   */
  badgeLabel?: (context: C) => string | undefined;
  /**
   * The tab's accessible name (and hover title) when the title alone says too
   * little: a file tab is titled `a.py` and named `a.py (/scripts/a.py)`. It
   * starts with the title, so the name holds the visible words.
   */
  label?: string;
};

/**
 * A FAMILY OF TABS (W7d spec §4.3): one registration that answers every tab
 * id starting with `prefix`, for tabs the viewer opens by the dozen, such as
 * the flow editor's `file:<path>` tabs. Each field takes the tab's id.
 *
 * A family member is never in {@link getTabs}: it is not a kind the "+" menu
 * can reopen, only a tab some layout names. {@link resolveTab} and
 * {@link isKnownTab} answer for it, so the frame draws it and a stored layout
 * that names it keeps it.
 */
export type TabFamilyDefinition<C = unknown> = {
  prefix: string;
  title: (id: TabId) => string;
  label?: (id: TabId) => string;
  icon: LucideIcon;
  closable: boolean;
  render: (id: TabId, context: C) => ReactNode;
  badge?: (id: TabId, context: C) => ReactNode;
  badgeLabel?: (id: TabId, context: C) => string | undefined;
};

const tabs = new Map<TabId, TabDefinition<never>>();
const families = new Map<string, TabFamilyDefinition<never>>();
let version = 0;
const listeners = new Set<() => void>();

/**
 * Register a tab kind. Registering an id again replaces it (hot reload).
 *
 * A tab may be registered after the editor has mounted, for example from a
 * lazily loaded W7c or W7d chunk: the frame re-renders, and
 * `usePersistedLayout` reads the stored layout again so a tab it names is
 * kept rather than dropped (see there for the one case where it is not).
 */
export function registerTab<C>(def: TabDefinition<C>): void {
  tabs.set(def.id, def as TabDefinition<never>);
  version++;
  for (const l of listeners) l();
}

/**
 * Register a family of tabs under `prefix`. Registering a prefix again
 * replaces it. Late registration works as for {@link registerTab}.
 */
export function registerTabFamily<C>(def: TabFamilyDefinition<C>): void {
  families.set(def.prefix, def as TabFamilyDefinition<never>);
  version++;
  for (const l of listeners) l();
}

function familyOf(id: TabId): TabFamilyDefinition<never> | undefined {
  let best: TabFamilyDefinition<never> | undefined;
  for (const f of families.values()) {
    if (id.startsWith(f.prefix) && id.length > f.prefix.length && (!best || f.prefix.length > best.prefix.length)) best = f;
  }
  return best;
}

/**
 * The definition of tab `id`: a tab registered under that id, else one of
 * the family whose prefix it starts with (the longest prefix wins), bound to
 * `id`. `undefined` when nothing answers for it.
 */
export function resolveTab<C = unknown>(id: TabId): TabDefinition<C> | undefined {
  const own = tabs.get(id);
  if (own) return own as unknown as TabDefinition<C>;
  const f = familyOf(id) as unknown as TabFamilyDefinition<C> | undefined;
  if (!f) return undefined;
  return {
    id,
    title: f.title(id),
    label: f.label?.(id),
    icon: f.icon,
    closable: f.closable,
    render: (context) => f.render(id, context),
    badge: f.badge && ((context) => f.badge!(id, context)),
    badgeLabel: f.badgeLabel && ((context) => f.badgeLabel!(id, context)),
  };
}

/** A tab some registration answers for: its own, or its family's. */
export function isKnownTab(id: TabId): boolean {
  return tabs.has(id) || familyOf(id) !== undefined;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** A number that changes whenever a tab is registered; re-renders the caller then. */
export function useTabsVersion(): number {
  return useSyncExternalStore(subscribe, () => version, () => version);
}

/** Every registered tab, in registration order. Family members are not listed (see {@link resolveTab}). */
export function getTabs<C = unknown>(): TabDefinition<C>[] {
  return [...tabs.values()] as unknown as TabDefinition<C>[];
}
