import type { ReactNode } from "react";
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
};

const tabs = new Map<TabId, TabDefinition<never>>();

/** Register a tab kind. Registering an id again replaces it (hot reload). */
export function registerTab<C>(def: TabDefinition<C>): void {
  tabs.set(def.id, def as TabDefinition<never>);
}

/** Every registered tab, in registration order. */
export function getTabs<C = unknown>(): TabDefinition<C>[] {
  return [...tabs.values()] as unknown as TabDefinition<C>[];
}
