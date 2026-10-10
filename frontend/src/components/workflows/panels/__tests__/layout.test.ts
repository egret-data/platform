import { describe, expect, it } from "vitest";
import {
  CODE_TAB,
  activateTab,
  DEFAULT_FLOW_LAYOUT,
  closeTab,
  moveTab,
  normalise,
  parseLayout,
  reopenTab,
  resize,
  splitTab,
  type Layout,
} from "@/components/workflows/panels/layout";

/**
 * Three panels at UNEQUAL sizes, so a re-balance that ignored the old sizes
 * (an equal split) cannot pass, and so a test can tell which panel a size
 * came from. Panel ids are not positional (`b` is first), so code that
 * confused an id with an index cannot pass either.
 */
function three(): Layout {
  return {
    panels: [
      { id: "b", size: 50, tabs: [CODE_TAB, "topology"], active: CODE_TAB },
      { id: "a", size: 30, tabs: ["docs", "problems", "files"], active: "problems" },
      { id: "c", size: 20, tabs: ["outline"], active: "outline" },
    ],
  };
}

const sum = (l: Layout) => l.panels.reduce((s, p) => s + p.size, 0);
const ids = (l: Layout) => l.panels.map((p) => p.id);
const tabsOf = (l: Layout, id: string) => l.panels.find((p) => p.id === id)?.tabs;

describe("moveTab", () => {
  it("reorders within a panel with array-move semantics, both directions", () => {
    const right = moveTab(three(), "docs", "a", 2);
    expect(tabsOf(right, "a")).toEqual(["problems", "files", "docs"]);
    const left = moveTab(three(), "files", "a", 0);
    expect(tabsOf(left, "a")).toEqual(["files", "docs", "problems"]);
  });

  it("moves across panels at the index, activates it there, and re-picks the source's active", () => {
    const l = moveTab(three(), "problems", "b", 1);
    expect(tabsOf(l, "b")).toEqual([CODE_TAB, "problems", "topology"]);
    expect(l.panels[0].active).toBe("problems");
    expect(tabsOf(l, "a")).toEqual(["docs", "files"]);
    // `problems` was active at index 1: its right-hand neighbour takes over, not the first tab.
    expect(l.panels[1].active).toBe("files");
  });

  it("keeps the source's active tab when the moved tab was not active", () => {
    const l = moveTab(three(), "docs", "c", 0);
    expect(l.panels[1].active).toBe("problems");
  });

  it("removes a panel whose last tab leaves and gives its size back proportionally", () => {
    const l = moveTab(three(), "outline", "a", 0);
    expect(ids(l)).toEqual(["b", "a"]);
    expect(sum(l)).toBeCloseTo(100);
    // 50:30 kept as a ratio — 62.5 and 37.5, not 50/50 and not 70/30.
    expect(l.panels[0].size).toBeCloseTo(62.5);
    expect(l.panels[1].size).toBeCloseTo(37.5);
  });

  it("does not mutate its input", () => {
    const before = three();
    const snapshot = JSON.stringify(before);
    moveTab(before, "outline", "a", 0);
    expect(JSON.stringify(before)).toBe(snapshot);
  });

  it("returns the same layout for an unknown tab or panel", () => {
    const l = three();
    expect(moveTab(l, "nope", "a", 0)).toBe(l);
    expect(moveTab(l, "docs", "nope", 0)).toBe(l);
  });
});

describe("splitTab", () => {
  it("splits right: a new panel right after the target, sized from its half", () => {
    const l = splitTab(three(), "docs", "b", "right");
    expect(l.panels).toHaveLength(4);
    expect(l.panels[0].id).toBe("b");
    expect(l.panels[1].tabs).toEqual(["docs"]);
    expect(l.panels[1].active).toBe("docs");
    expect(l.panels[2].id).toBe("a");
    expect(sum(l)).toBeCloseTo(100);
    expect(l.panels[0].size).toBeCloseTo(25);
    expect(l.panels[1].size).toBeCloseTo(25);
    expect(l.panels[2].size).toBeCloseTo(30);
  });

  it("splits left: the new panel lands before the target", () => {
    const l = splitTab(three(), "topology", "c", "left");
    expect(ids(l).slice(0, 2)).toEqual(["b", "a"]);
    expect(l.panels[2].tabs).toEqual(["topology"]);
    expect(l.panels[3].id).toBe("c");
    expect(tabsOf(l, "b")).toEqual([CODE_TAB]);
    expect(sum(l)).toBeCloseTo(100);
  });

  it("gives the new panel an id no other panel has", () => {
    const l = splitTab(three(), "docs", "a", "right");
    expect(new Set(ids(l)).size).toBe(4);
  });

  it("re-balances to 100 when the split empties the source panel", () => {
    const l = splitTab(three(), "outline", "b", "left");
    expect(l.panels.map((p) => p.tabs[0])).toEqual(["outline", CODE_TAB, "docs"]);
    expect(sum(l)).toBeCloseTo(100);
  });

  it("refuses to split a panel's only tab beside its own panel", () => {
    const l = three();
    expect(splitTab(l, "outline", "c", "right")).toBe(l);
  });
});

describe("closeTab", () => {
  it("closes a tab and activates its right-hand neighbour", () => {
    const l = closeTab(three(), "problems");
    expect(tabsOf(l, "a")).toEqual(["docs", "files"]);
    expect(l.panels[1].active).toBe("files");
  });

  it("falls back to the left neighbour when the closed tab was last", () => {
    const start = three();
    start.panels[1].active = "files";
    const l = closeTab(start, "files");
    expect(l.panels[1].active).toBe("problems");
  });

  it("removes a panel whose last tab is closed, and sizes still sum to 100", () => {
    const l = closeTab(three(), "outline");
    expect(ids(l)).toEqual(["b", "a"]);
    expect(sum(l)).toBeCloseTo(100);
  });

  it("refuses to close Code and returns the layout unchanged", () => {
    const l = three();
    expect(closeTab(l, CODE_TAB)).toBe(l);
  });
});

describe("activateTab", () => {
  it("activates the tab in its own panel and leaves the others alone", () => {
    const l = activateTab(three(), "files");
    expect(l.panels.map((p) => p.active)).toEqual([CODE_TAB, "files", "outline"]);
  });

  it("returns the same layout for an unknown or already-active tab", () => {
    const l = three();
    expect(activateTab(l, "nope")).toBe(l);
    expect(activateTab(l, "problems")).toBe(l);
  });
});

describe("reopenTab", () => {
  it("adds the tab to the end of the given panel and activates it", () => {
    const l = reopenTab(three(), "revisions", "a");
    expect(tabsOf(l, "a")).toEqual(["docs", "problems", "files", "revisions"]);
    expect(l.panels[1].active).toBe("revisions");
    expect(tabsOf(l, "b")).toEqual([CODE_TAB, "topology"]);
  });

  it("does not open a tab twice", () => {
    const l = three();
    expect(reopenTab(l, "docs", "c")).toBe(l);
  });
});

describe("resize", () => {
  it("takes sizes in panel order and scales them to 100", () => {
    const l = resize(three(), [20, 20, 10]);
    expect(l.panels.map((p) => p.size)).toEqual([40, 40, 20]);
  });

  it("ignores a size list of the wrong length or with a non-positive value", () => {
    const l = three();
    expect(resize(l, [50, 50])).toBe(l);
    expect(resize(l, [50, 0, 50])).toBe(l);
    expect(resize(l, [50, Number.NaN, 50])).toBe(l);
  });
});

describe("normalise", () => {
  const known = [CODE_TAB, "docs", "problems"];

  it("drops unknown tabs and the panels they leave empty", () => {
    const l = normalise(three(), known);
    expect(ids(l)).toEqual(["b", "a"]);
    expect(tabsOf(l, "b")).toEqual([CODE_TAB]);
    expect(tabsOf(l, "a")).toEqual(["docs", "problems"]);
    expect(sum(l)).toBeCloseTo(100);
  });

  it("adds a missing Code tab back to the front of the first panel", () => {
    const l = normalise({ panels: [
      { id: "x", size: 40, tabs: ["docs"], active: "docs" },
      { id: "y", size: 60, tabs: ["problems"], active: "problems" },
    ] }, known);
    expect(tabsOf(l, "x")).toEqual([CODE_TAB, "docs"]);
    expect(tabsOf(l, "y")).toEqual(["problems"]);
    expect(l.panels[0].active).toBe("docs");
  });

  it("builds a Code panel when nothing survives", () => {
    const l = normalise({ panels: [{ id: "x", size: 100, tabs: ["gone"], active: "gone" }] }, known);
    expect(l.panels).toHaveLength(1);
    expect(l.panels[0].tabs).toEqual([CODE_TAB]);
    expect(l.panels[0].size).toBe(100);
  });

  it("keeps the first copy of a tab listed twice, across and within panels", () => {
    const l = normalise({ panels: [
      { id: "x", size: 50, tabs: [CODE_TAB, "docs", "docs"], active: CODE_TAB },
      { id: "y", size: 50, tabs: ["docs", "problems"], active: "docs" },
    ] }, known);
    expect(tabsOf(l, "x")).toEqual([CODE_TAB, "docs"]);
    expect(tabsOf(l, "y")).toEqual(["problems"]);
    expect(l.panels[1].active).toBe("problems");
  });

  it("fixes an active tab that is not in its panel", () => {
    const l = normalise({ panels: [
      { id: "x", size: 50, tabs: ["docs", CODE_TAB], active: "problems" },
      { id: "y", size: 50, tabs: ["problems"], active: "problems" },
    ] }, known);
    expect(l.panels[0].active).toBe("docs");
  });

  it("re-ids a duplicated panel id and replaces unusable sizes with equal shares", () => {
    const l = normalise({ panels: [
      { id: "x", size: -5, tabs: [CODE_TAB], active: CODE_TAB },
      { id: "x", size: Number.POSITIVE_INFINITY, tabs: ["docs"], active: "docs" },
    ] }, known);
    expect(new Set(ids(l)).size).toBe(2);
    expect(l.panels.map((p) => p.size)).toEqual([50, 50]);
  });

  it("leaves the default flow layout as it is", () => {
    expect(normalise(DEFAULT_FLOW_LAYOUT, known)).toEqual(DEFAULT_FLOW_LAYOUT);
  });
});

describe("parseLayout", () => {
  it("accepts the stored shape", () => {
    expect(parseLayout(JSON.parse(JSON.stringify(DEFAULT_FLOW_LAYOUT)))).toEqual(DEFAULT_FLOW_LAYOUT);
  });

  it.each([
    ["null", null],
    ["a string", "code"],
    ["no panels", {}],
    ["panels not an array", { panels: {} }],
    ["a panel without tabs", { panels: [{ id: "x", size: 1, active: "code" }] }],
    ["a non-string tab", { panels: [{ id: "x", size: 1, tabs: [1], active: "code" }] }],
    ["a string size", { panels: [{ id: "x", size: "50", tabs: ["code"], active: "code" }] }],
  ])("rejects %s", (_name, raw) => {
    expect(parseLayout(raw)).toBeNull();
  });
});

describe("DEFAULT_FLOW_LAYOUT", () => {
  it("is [Code] | [Docs, Problems] at 60/40", () => {
    expect(DEFAULT_FLOW_LAYOUT.panels.map((p) => [p.tabs, p.size])).toEqual([
      [[CODE_TAB], 60],
      [["docs", "problems"], 40],
    ]);
  });
});
