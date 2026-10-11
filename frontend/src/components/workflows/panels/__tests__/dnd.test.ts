import type { ClientRect, DroppableContainer } from "@dnd-kit/core";
import { describe, expect, it, vi } from "vitest";
import { collisions, dropIndex, jumpBetweenTargets, type DropData } from "@/components/workflows/panels/dnd";
import { CODE_TAB, moveTab, type Layout } from "@/components/workflows/panels/layout";

const rect = (left: number, top: number, width: number, height: number): ClientRect =>
  ({ left, top, width, height, right: left + width, bottom: top + height });

/**
 * Two panels side by side, 500 px each: a 36 px tab strip with two tabs, and
 * edge zones over the outer quarters of the body. The middle half of each
 * body (x 125–375 in panel 1) is covered by NO target.
 */
const TARGETS: [string, DropData, ClientRect][] = [
  ["strip:p1", { kind: "strip", panel: "p1" }, rect(0, 0, 500, 36)],
  ["tab:code", { kind: "tab", panel: "p1", tab: CODE_TAB }, rect(0, 0, 80, 36)],
  ["tab:docs", { kind: "tab", panel: "p1", tab: "docs" }, rect(80, 0, 80, 36)],
  ["edge:p1:left", { kind: "edge", panel: "p1", side: "left" }, rect(0, 36, 125, 464)],
  ["edge:p1:right", { kind: "edge", panel: "p1", side: "right" }, rect(375, 36, 125, 464)],
  ["strip:p2", { kind: "strip", panel: "p2" }, rect(500, 0, 500, 36)],
  ["tab:problems", { kind: "tab", panel: "p2", tab: "problems" }, rect(500, 0, 80, 36)],
  ["edge:p2:left", { kind: "edge", panel: "p2", side: "left" }, rect(500, 36, 125, 464)],
];

const containers = TARGETS.map(([id, data, r]) => ({
  id, key: id, data: { current: data }, disabled: false,
  node: { current: null }, rect: { current: r },
})) as unknown as DroppableContainer[];
const droppableRects = new Map(TARGETS.map(([id, , r]) => [id, r]));
const active = { id: "tab:docs", data: { current: { tab: "docs" } }, rect: { current: { initial: null, translated: null } } };
const dragged = rect(80, 0, 80, 36);

function hit(pointer: { x: number; y: number } | null, collisionRect = dragged) {
  return collisions({
    active: active as never,
    collisionRect,
    droppableRects,
    droppableContainers: containers,
    pointerCoordinates: pointer,
  }).map((c) => c.id);
}

describe("collisions", () => {
  it("finds nothing for a pointer over no target, so the drop changes nothing", () => {
    expect(hit({ x: 250, y: 250 })).toEqual([]);
  });

  it("ranks the tab under the pointer above the strip that holds it", () => {
    expect(hit({ x: 520, y: 18 })).toEqual(["tab:problems", "strip:p2"]);
  });

  it("finds the strip alone past its last tab, and an edge zone alone in the body", () => {
    expect(hit({ x: 800, y: 18 })).toEqual(["strip:p2"]);
    expect(hit({ x: 450, y: 250 })).toEqual(["edge:p1:right"]);
  });

  it("falls back to the nearest target only without a pointer (the keyboard)", () => {
    expect(hit(null, rect(450, 250, 80, 36))[0]).toBe("edge:p1:right");
  });
});

describe("jumpBetweenTargets", () => {
  const press = (code: string, from: ClientRect) => {
    const event = { code, preventDefault: vi.fn() } as unknown as KeyboardEvent;
    const enabled = { getEnabled: () => containers };
    const next = jumpBetweenTargets(event, {
      active: "tab:docs",
      currentCoordinates: { x: from.left, y: from.top },
      context: { collisionRect: from, droppableRects, droppableContainers: enabled } as never,
    });
    return { next, event };
  };
  const centreOf = (next: { x: number; y: number } | void) =>
    next ? { x: next.x + dragged.width / 2, y: next.y + dragged.height / 2 } : undefined;

  it("moves right to the next target on the same row, centred on it", () => {
    // From Docs: the next centre to the right on the strip row is strip:p1's (250, 18).
    const { next, event } = press("ArrowRight", dragged);
    expect(centreOf(next)).toEqual({ x: 250, y: 18 });
    expect(event.preventDefault).toHaveBeenCalled();
  });

  it("moves down from a strip into the edge zone below it", () => {
    const { next } = press("ArrowDown", rect(0, 0, 80, 36));
    expect(centreOf(next)).toEqual({ x: 62.5, y: 268 });
  });

  it("does nothing past the last target and ignores other keys", () => {
    expect(press("ArrowRight", rect(920, 0, 80, 36)).next).toBeUndefined();
    const other = press("KeyA", dragged);
    expect(other.next).toBeUndefined();
    expect(other.event.preventDefault).not.toHaveBeenCalled();
  });
});

describe("dropIndex", () => {
  const layout: Layout = {
    panels: [
      { id: "p1", size: 50, tabs: ["a", "b", "c"], active: "a" },
      { id: "p2", size: 50, tabs: ["d", "e"], active: "d" },
    ],
  };
  const drop = (tab: string, panel: string, over: string) =>
    moveTab(layout, tab, panel, dropIndex(layout, tab, panel, over)).panels.find((p) => p.id === panel)?.tabs;

  it("lands BEFORE the target when moving right within a panel", () => {
    expect(drop("a", "p1", "c")).toEqual(["b", "a", "c"]);
  });

  it("lands before the target when moving left within a panel", () => {
    expect(drop("c", "p1", "a")).toEqual(["c", "a", "b"]);
  });

  it("lands before the target in another panel", () => {
    expect(drop("b", "p2", "e")).toEqual(["d", "b", "e"]);
  });
});
