import {
  closestCenter,
  pointerWithin,
  type CollisionDetection,
  type KeyboardCoordinateGetter,
} from "@dnd-kit/core";
import type { Layout, TabId } from "@/components/workflows/panels/layout";

/** What a drop target says it is, read back in `MultiPanel`'s `onDragEnd`. */
export type DropData =
  | { kind: "tab"; panel: string; tab: TabId }
  | { kind: "strip"; panel: string }
  | { kind: "edge"; panel: string; side: "left" | "right" };

/** What a dragged tab says it is. */
export type DragData = { tab: TabId };

export const dragId = (tab: TabId) => `tab:${tab}`;

/**
 * Where a drag lands.
 *
 * - **Pointer:** only what is under the pointer, ranked so the most specific
 *   target wins. A tab sits inside its strip, so both are hit, and the tab
 *   ("insert before it") means more than the strip ("append"). Nothing under
 *   the pointer means NO target: a release in neutral space (the middle of a
 *   panel's body) abandons the drag, it does not snap to the nearest target.
 * - **Keyboard:** there is no pointer, so the nearest target to the dragged
 *   rect wins. `jumpBetweenTargets` puts that rect on a target's centre.
 */
const RANK: Record<DropData["kind"], number> = { tab: 0, strip: 1, edge: 2 };
export const collisions: CollisionDetection = (args) => {
  if (!args.pointerCoordinates) return closestCenter(args);
  const rank = (id: string | number) => {
    const data = args.droppableContainers.find((c) => c.id === id)?.data.current as DropData | undefined;
    return data ? RANK[data.kind] : 3;
  };
  return [...pointerWithin(args)].sort((a, b) => rank(a.id) - rank(b.id));
};

const DIRECTION: Record<string, [number, number]> = {
  ArrowRight: [1, 0],
  ArrowLeft: [-1, 0],
  ArrowDown: [0, 1],
  ArrowUp: [0, -1],
};

/**
 * dnd-kit's keyboard sensor moves 25 px per arrow by default; this moves the
 * dragged tab to the next drop target in the arrow's direction instead (a
 * tab, a strip or a panel edge), centred on it. Nearer along the arrow wins,
 * and drifting sideways counts double, so ArrowRight prefers the next target
 * on the same row.
 */
export const jumpBetweenTargets: KeyboardCoordinateGetter = (event, { active, context }) => {
  const dir = DIRECTION[event.code];
  if (!dir) return undefined;
  event.preventDefault();
  const { collisionRect, droppableRects, droppableContainers } = context;
  if (!collisionRect) return undefined;
  const from = { x: collisionRect.left + collisionRect.width / 2, y: collisionRect.top + collisionRect.height / 2 };
  let best: { x: number; y: number; score: number } | null = null;
  for (const c of droppableContainers.getEnabled()) {
    if (c.id === active) continue;
    const rect = droppableRects.get(c.id);
    if (!rect) continue;
    const to = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    const along = (to.x - from.x) * dir[0] + (to.y - from.y) * dir[1];
    if (along <= 1) continue;
    const across = Math.abs((to.x - from.x) * dir[1]) + Math.abs((to.y - from.y) * dir[0]);
    const score = along + 2 * across;
    if (!best || score < best.score) best = { ...to, score };
  }
  if (!best) return undefined;
  return { x: best.x - collisionRect.width / 2, y: best.y - collisionRect.height / 2 };
};

/**
 * The index `moveTab` takes for "drop `tab` before `overTab` in `panelId`".
 * `moveTab`'s index is a position in the list AFTER the tab has left it, so a
 * rightward move inside one panel is one less: dropping A on C in [A, B, C]
 * gives [B, A, C] — before C, as the insertion bar and the announcement say.
 */
export function dropIndex(layout: Layout, tab: TabId, panelId: string, overTab: TabId): number {
  const tabs = layout.panels.find((p) => p.id === panelId)?.tabs ?? [];
  const at = tabs.indexOf(overTab);
  const from = tabs.indexOf(tab);
  return from !== -1 && from < at ? at - 1 : at;
}
