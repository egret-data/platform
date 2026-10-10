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
};

const tabs = new Map<TabId, TabDefinition<never>>();
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

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** A number that changes whenever a tab is registered; re-renders the caller then. */
export function useTabsVersion(): number {
  return useSyncExternalStore(subscribe, () => version, () => version);
}

/** Every registered tab, in registration order. */
export function getTabs<C = unknown>(): TabDefinition<C>[] {
  return [...tabs.values()] as unknown as TabDefinition<C>[];
}
