// `Separator` is aliased: `@/components/ui/separator` exports that name for the visual divider,
// and react-resizable-panels v4 renamed `PanelResizeHandle` to `Separator`.
import { Fragment, useId, useLayoutEffect, useRef, useState } from "react";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  useDndContext,
  useDroppable,
  useSensor,
  useSensors,
  type Announcements,
  type DragEndEvent,
  type DragStartEvent,
  type Over,
} from "@dnd-kit/core";
import { Group, Panel, Separator as PanelSeparator } from "react-resizable-panels";
import {
  moveTab,
  resize,
  splitTab,
  type Layout,
  type TabId,
} from "@/components/workflows/panels/layout";
import {
  collisions,
  dropIndex,
  jumpBetweenTargets,
  type DragData,
  type DropData,
} from "@/components/workflows/panels/dnd";
import { PanelTabStrip, tabTitle } from "@/components/workflows/panels/PanelTabStrip";
import { getTabs, useTabsVersion, type TabDefinition } from "@/components/workflows/panels/registry";
import { cn } from "@/lib/utils";

/**
 * A panel's left or right edge, where a dropped tab splits the panel. It is
 * always registered (dnd-kit measures targets when a drag starts) and never
 * takes the pointer, so it cannot block the tab's content; it only paints
 * while a drag is live.
 */
function EdgeZone({ panelId, side }: { panelId: string; side: "left" | "right" }) {
  const { active } = useDndContext();
  const { setNodeRef, isOver } = useDroppable({
    id: `edge:${panelId}:${side}`,
    data: { kind: "edge", panel: panelId, side } satisfies DropData,
  });
  return (
    <div
      ref={setNodeRef}
      aria-hidden
      className={cn(
        "pointer-events-none absolute inset-y-0 z-10 flex w-1/4 items-center justify-center",
        side === "left" ? "left-0" : "right-0",
        active && isOver && "border-2 border-dashed border-primary bg-primary/10",
      )}
    >
      {active && isOver && (
        <span className="rounded-md bg-background px-2 py-1 text-xs text-foreground shadow-sm">
          {side === "left" ? "Split left" : "Split right"}
        </span>
      )}
    </div>
  );
}

type Props<C> = {
  layout: Layout;
  onChange: (next: Layout) => void;
  /** Handed unchanged to every tab's `render`. */
  context: C;
};

/**
 * THE FLOW EDITOR'S MULTI-PANEL FRAME (spec §5.1), modelled on Kestra's
 * `MultiPanelTabs.vue`: a row of resizable panels, each with a tab strip and
 * one visible tab. Tabs move between strips and split panels by dragging
 * (`@dnd-kit`, with its keyboard sensor) and through each panel's menus.
 *
 * It is controlled: `layout` in, `onChange` out. Every change is one of the
 * pure functions in `layout.ts`; the frame keeps nothing but the drag in
 * flight and where focus should land next.
 *
 * Only the active tab of each panel is rendered. A tab's state therefore lives
 * in `context`, not in the tab, which is also what lets a tab move between
 * panels without losing it (moving remounts it under another parent anyway).
 */
export function MultiPanel<C>({ layout, onChange, context }: Props<C>) {
  // A tab registered after the first render (a lazily loaded package) re-renders the frame.
  useTabsVersion();
  const defs = new Map(getTabs<C>().map((d) => [d.id, d as unknown as TabDefinition<never>]));
  const badgeOf = (tab: TabId) => defs.get(tab)?.badge?.(context as never);
  const baseId = useId();
  const tabDomId = (tab: TabId) => `${baseId}-tab-${tab}`;
  const [dragging, setDragging] = useState<TabId | null>(null);

  // Focus must follow a keyboard change (arrows, Delete, a menu item) to the
  // tab it selected; that tab may not exist until the change renders.
  const focusNext = useRef<TabId | null>(null);
  useLayoutEffect(() => {
    if (focusNext.current === null) return;
    document.getElementById(tabDomId(focusNext.current))?.focus();
    focusNext.current = null;
  });
  const change = (next: Layout, focus?: TabId) => {
    if (focus !== undefined) focusNext.current = focus;
    if (next !== layout) onChange(next);
    else if (focus !== undefined) {
      document.getElementById(tabDomId(focus))?.focus();
      focusNext.current = null;
    }
  };

  // A split or a close changes which panels exist, and the group is
  // remounted with the model's sizes. Patching a mounted group does not hold:
  // it registers new panels a render later, and it caches a layout per panel
  // set, which would bring back old sizes when a set recurs (split, then close).
  // The remount re-renders each panel's active tab; a tab keeps its state in
  // `context` for that reason (moving a tab remounts it regardless).
  const structureKey = layout.panels.map((p) => p.id).join("|");
  const sizes = Object.fromEntries(layout.panels.map((p) => [p.id, p.size]));

  const sensors = useSensors(
    // 6px before a press becomes a drag, so a click still selects the tab.
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    // Arrows jump from one drop target to the next rather than 25 px at a time.
    useSensor(KeyboardSensor, { coordinateGetter: jumpBetweenTargets }),
  );

  const panelIndex = (id: string) => layout.panels.findIndex((p) => p.id === id) + 1;
  const describe = (over: Over | null): string => {
    const d = over?.data.current as DropData | undefined;
    if (!d) return "Not over a drop target.";
    if (d.kind === "tab") return `Over the ${tabTitle(defs, d.tab)} tab in panel ${panelIndex(d.panel)}; drop to move before it.`;
    if (d.kind === "strip") return `Over the tab strip of panel ${panelIndex(d.panel)}; drop to move there.`;
    return `Over the ${d.side} edge of panel ${panelIndex(d.panel)}; drop to split.`;
  };
  const titleOf = (id: string | number) => tabTitle(defs, String(id).replace(/^tab:/, ""));
  const announcements: Announcements = {
    onDragStart: ({ active }) => `Picked up the ${titleOf(active.id)} tab.`,
    onDragOver: ({ over }) => describe(over),
    onDragEnd: ({ active, over }) => over ? `Dropped the ${titleOf(active.id)} tab. ${describe(over)}` : "Dropped where nothing changes.",
    onDragCancel: ({ active }) => `Stopped moving the ${titleOf(active.id)} tab.`,
  };

  const onDragStart = (e: DragStartEvent) => setDragging((e.active.data.current as DragData).tab);
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    setDragging(null);
    const tab = (active.data.current as DragData | undefined)?.tab;
    const d = over?.data.current as DropData | undefined;
    if (!tab || !d) return;
    if (d.kind === "tab") {
      if (d.tab === tab) return;
      change(moveTab(layout, tab, d.panel, dropIndex(layout, tab, d.panel, d.tab)), tab);
    } else if (d.kind === "strip") {
      const target = layout.panels.find((p) => p.id === d.panel);
      if (target) change(moveTab(layout, tab, d.panel, target.tabs.length), tab);
    } else {
      change(splitTab(layout, tab, d.panel, d.side), tab);
    }
  };

  const draggedDef = dragging ? defs.get(dragging) : undefined;

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={collisions}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragCancel={() => setDragging(null)}
      accessibility={{
        announcements,
        screenReaderInstructions: {
          draggable:
            "To move this tab, press Space or Enter, use the arrow keys to reach another tab strip or a panel's edge, "
            + "then press Space or Enter to drop it, or Escape to cancel. The tab actions menu offers the same moves.",
        },
      }}
    >
      <Group
        key={structureKey}
        defaultLayout={sizes}
        orientation="horizontal"
        className="h-full min-h-0"
        onLayoutChanged={(next, meta) => {
          if (!meta.isUserInteraction) return;
          const resized = resize(layout, layout.panels.map((p) => next[p.id]));
          if (resized !== layout) onChange(resized);
        }}
      >
        {layout.panels.map((panel, index) => {
          const tabpanelId = `${baseId}-panel-${panel.id}`;
          const def = defs.get(panel.active) as TabDefinition<C> | undefined;
          return (
            <Fragment key={panel.id}>
              {index > 0 && (
                <PanelSeparator
                  className="w-1 bg-border transition-colors hover:bg-primary/60 focus-visible:bg-primary focus-visible:outline-none active:bg-primary"
                />
              )}
              <Panel id={panel.id} defaultSize={`${panel.size}%`} minSize="10%">
                <div className="flex h-full min-w-0 flex-col bg-background">
                  <PanelTabStrip
                    layout={layout}
                    panel={panel}
                    index={index}
                    defs={defs}
                    tabDomId={tabDomId}
                    tabpanelId={tabpanelId}
                    onChange={change}
                    badgeOf={badgeOf}
                  />
                  <div
                    role="tabpanel"
                    id={tabpanelId}
                    aria-labelledby={tabDomId(panel.active)}
                    className="relative min-h-0 flex-1 overflow-auto"
                  >
                    {/* Keyed by tab: two tabs that render the same component type must
                        not share one React instance when the panel switches between them. */}
                    <Fragment key={panel.active}>{def?.render(context)}</Fragment>
                    <EdgeZone panelId={panel.id} side="left" />
                    <EdgeZone panelId={panel.id} side="right" />
                  </div>
                </div>
              </Panel>
            </Fragment>
          );
        })}
      </Group>
      <DragOverlay dropAnimation={null}>
        {dragging && (
          <div className="flex items-center gap-1.5 rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground shadow-md">
            {draggedDef && <draggedDef.icon aria-hidden className="size-3.5" />}
            {tabTitle(defs, dragging)}
          </div>
        )}
      </DragOverlay>
    </DndContext>
  );
}
