import type { KeyboardEvent, MouseEvent, ReactNode } from "react";
import { useDraggable, useDroppable } from "@dnd-kit/core";
import { Columns2, EllipsisVertical, MoveRight, Plus, X, type LucideIcon } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { IconButton } from "@/components/ui/icon-button";
import {
  CODE_TAB,
  activateTab,
  closeTab,
  moveTab,
  reopenTab,
  splitTab,
  type Layout,
  type PanelLayout,
  type TabId,
} from "@/components/workflows/panels/layout";
import type { TabDefinition } from "@/components/workflows/panels/registry";
import { cn } from "@/lib/utils";
import { dragId, type DragData, type DropData } from "@/components/workflows/panels/dnd";

/** A tab's title, or its id when nothing registered it (a layout is normalised before it gets here). */
export function tabTitle(defs: Map<TabId, TabDefinition<never>>, tab: TabId): string {
  return defs.get(tab)?.title ?? tab;
}

/** "Panel 2 (Docs, Problems)": the name the "Move to panel…" menu uses for a panel. */
export function panelName(defs: Map<TabId, TabDefinition<never>>, layout: Layout, panelId: string): string {
  const i = layout.panels.findIndex((p) => p.id === panelId);
  const p = layout.panels[i];
  return `Panel ${i + 1} (${p.tabs.map((t) => tabTitle(defs, t)).join(", ")})`;
}

const isClosable = (defs: Map<TabId, TabDefinition<never>>, tab: TabId) =>
  tab !== CODE_TAB && (defs.get(tab)?.closable ?? true);

/**
 * One tab: the `tab` of a `tablist`, a drag source, and a drop target that
 * means "insert here".
 *
 * dnd-kit's own attributes would make it `role="button"` with
 * `aria-roledescription="draggable"`, which would hide that it is a tab, so
 * only its `aria-describedby` (the drag instructions) is taken. Its keyboard
 * listener is kept: Space or Enter on a focused tab lifts it (the tab is
 * already selected by then — selection follows focus).
 */
function TabButton({
  tab, title, label, badge, badgeLabel, icon: Icon, panelId, selected, closable, domId, controls, onSelect, onClose, onKeyDown,
}: {
  tab: TabId;
  title: string;
  /** The accessible name when the title says too little (`TabDefinition.label`). */
  label?: string;
  badge?: ReactNode;
  badgeLabel?: string;
  icon: LucideIcon | undefined;
  panelId: string;
  selected: boolean;
  closable: boolean;
  domId: string;
  controls: string;
  onSelect: () => void;
  onClose: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => void;
}) {
  const drag = useDraggable({ id: dragId(tab), data: { tab } satisfies DragData });
  const drop = useDroppable({ id: dragId(tab), data: { kind: "tab", panel: panelId, tab } satisfies DropData });
  const { onKeyDown: liftOnKey, ...pointerListeners } = drag.listeners ?? {};
  const dropping = drop.isOver && !drag.isDragging;

  return (
    <button
      ref={(el) => { drag.setNodeRef(el); drop.setNodeRef(el); }}
      type="button"
      role="tab"
      id={domId}
      aria-selected={selected}
      aria-label={badgeLabel ? `${label ?? title}, ${badgeLabel}` : label}
      title={label}
      aria-controls={selected ? controls : undefined}
      aria-describedby={drag.attributes["aria-describedby"]}
      aria-keyshortcuts={closable ? "Delete" : undefined}
      tabIndex={selected ? 0 : -1}
      {...pointerListeners}
      onKeyDown={(e) => {
        // While a keyboard drag is live the arrows belong to dnd-kit.
        if (!drag.isDragging) onKeyDown(e);
        if (!e.defaultPrevented) liftOnKey?.(e);
      }}
      onClick={(e: MouseEvent<HTMLButtonElement>) => {
        // The × is drawn inside the tab, not as a second control: a `tablist`
        // may only own tabs. Keyboard users close with Delete or the tab menu.
        if (closable && (e.target as Element).closest("[data-tab-close]")) onClose();
        else onSelect();
      }}
      onAuxClick={(e) => {
        // Middle-click closes, as in Kestra and every browser.
        if (e.button === 1 && closable) { e.preventDefault(); onClose(); }
      }}
      className={cn(
        "group relative flex shrink-0 cursor-pointer items-center gap-1.5 border-r border-border px-3 text-sm whitespace-nowrap",
        "text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
        "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none focus-visible:ring-inset",
        // Selected: the panel's surface, full-strength ink, and a bar on top — a shape, not only a colour.
        selected && "bg-background text-foreground after:absolute after:inset-x-0 after:top-0 after:h-0.5 after:bg-primary",
        drag.isDragging && "opacity-50",
        // The insertion point: the dragged tab lands before this one.
        dropping && "before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:bg-primary",
      )}
    >
      {Icon && <Icon aria-hidden className="size-3.5 shrink-0" />}
      <span>{title}</span>
      {badge}
      {closable && (
        <X
          data-tab-close
          aria-hidden
          className={cn(
            "-mr-1 size-4 shrink-0 rounded-sm p-0.5 hover:bg-muted",
            selected ? "visible" : "invisible group-hover:visible",
          )}
        />
      )}
    </button>
  );
}

/**
 * A panel's header: its `tablist`, the active tab's actions menu, and the
 * "+" menu of closed tabs.
 *
 * The actions menu belongs to the panel and acts on its active tab, as
 * Kestra's per-panel menu does. A `tablist` can own only tabs, so a menu
 * button inside each tab is not available; selecting a tab and then its
 * actions is two keystrokes (arrows, then Tab).
 */
export function PanelTabStrip({
  layout, panel, index, defs, tabDomId, tabpanelId, onChange, badgeOf, badgeLabelOf,
}: {
  layout: Layout;
  panel: PanelLayout;
  index: number;
  defs: Map<TabId, TabDefinition<never>>;
  tabDomId: (tab: TabId) => string;
  tabpanelId: string;
  /** `focus`: the tab to move keyboard focus to once the change has rendered. */
  onChange: (next: Layout, focus?: TabId) => void;
  /** A tab's badge (`TabDefinition.badge`), already given the frame's context. */
  badgeOf?: (tab: TabId) => ReactNode;
  /** A tab's badge in words (`TabDefinition.badgeLabel`), for its accessible name. */
  badgeLabelOf?: (tab: TabId) => string | undefined;
}) {
  const { setNodeRef: setStripRef, isOver: overStrip } = useDroppable({ id: `strip:${panel.id}`, data: { kind: "strip", panel: panel.id } satisfies DropData });
  const active = panel.active;
  const activeTitle = tabTitle(defs, active);
  const others = layout.panels.filter((p) => p.id !== panel.id);
  const onlyTab = panel.tabs.length === 1;
  const open = new Set(layout.panels.flatMap((p) => p.tabs));
  const closed = [...defs.values()].filter((d) => !open.has(d.id));

  // After a close, focus goes to the tab that took over in the same panel, or
  // to Code when the panel went away — never to <body>.
  const close = (tab: TabId) => {
    const next = closeTab(layout, tab);
    if (next === layout) return;
    const stayed = next.panels.find((p) => p.id === panel.id);
    onChange(next, stayed ? stayed.active : CODE_TAB);
  };

  const onTabKeyDown = (tab: TabId) => (e: KeyboardEvent<HTMLButtonElement>) => {
    const at = panel.tabs.indexOf(tab);
    const n = panel.tabs.length;
    const target =
      e.key === "ArrowRight" ? panel.tabs[(at + 1) % n]
      : e.key === "ArrowLeft" ? panel.tabs[(at - 1 + n) % n]
      : e.key === "Home" ? panel.tabs[0]
      : e.key === "End" ? panel.tabs[n - 1]
      : undefined;
    if (target !== undefined) {
      e.preventDefault();
      onChange(activateTab(layout, target), target);
    } else if (e.key === "Delete" && isClosable(defs, tab)) {
      e.preventDefault();
      close(tab);
    }
  };

  return (
    <div className="flex h-9 shrink-0 items-stretch border-b border-border bg-muted/40">
      <div
        ref={setStripRef}
        role="tablist"
        aria-label={`Panel ${index + 1}`}
        className={cn("flex min-w-0 flex-1 overflow-x-auto", overStrip && "bg-accent")}
      >
        {panel.tabs.map((tab) => (
          <TabButton
            key={tab}
            tab={tab}
            title={tabTitle(defs, tab)}
            label={defs.get(tab)?.label}
            badge={badgeOf?.(tab)}
            badgeLabel={badgeLabelOf?.(tab)}
            icon={defs.get(tab)?.icon}
            panelId={panel.id}
            selected={tab === active}
            closable={isClosable(defs, tab)}
            domId={tabDomId(tab)}
            controls={tabpanelId}
            onSelect={() => onChange(activateTab(layout, tab))}
            onClose={() => close(tab)}
            onKeyDown={onTabKeyDown(tab)}
          />
        ))}
      </div>
      <div className="flex shrink-0 items-center gap-0.5 px-1">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <IconButton aria-label={`Tab actions: ${activeTitle}`} title={`Tab actions: ${activeTitle}`}>
              <EllipsisVertical aria-hidden className="size-4" />
            </IconButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <MoveRight aria-hidden className="mr-2 size-4" />Move to panel…
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                {others.length === 0 ? (
                  <DropdownMenuItem disabled>No other panel. Split a tab to make one.</DropdownMenuItem>
                ) : others.map((p) => (
                  <DropdownMenuItem key={p.id} onSelect={() => onChange(moveTab(layout, active, p.id, p.tabs.length), active)}>
                    {panelName(defs, layout, p.id)}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            {(["left", "right"] as const).map((side) => (
              <DropdownMenuItem
                key={side}
                disabled={onlyTab}
                onSelect={() => onChange(splitTab(layout, active, panel.id, side), active)}
              >
                <Columns2 aria-hidden className="mr-2 size-4" />
                {side === "left" ? "Split left" : "Split right"}
                {onlyTab && <span className="ml-auto pl-3 text-xs text-muted-foreground">Only tab in this panel</span>}
              </DropdownMenuItem>
            ))}
            {isClosable(defs, active) && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => close(active)}>
                  <X aria-hidden className="mr-2 size-4" />Close
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <IconButton aria-label={`Reopen a tab in panel ${index + 1}`} title="Reopen a tab">
              <Plus aria-hidden className="size-4" />
            </IconButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {closed.length === 0 ? (
              <DropdownMenuItem disabled>Every tab is open</DropdownMenuItem>
            ) : closed.map((d) => (
              <DropdownMenuItem key={d.id} onSelect={() => onChange(reopenTab(layout, d.id, panel.id), d.id)}>
                <d.icon aria-hidden className="mr-2 size-4" />{d.title}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  );
}
