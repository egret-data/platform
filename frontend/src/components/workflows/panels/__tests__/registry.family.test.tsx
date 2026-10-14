import { useState } from "react";
import { act, render, renderHook, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Code, FileCode, FileText } from "lucide-react";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { CODE_TAB, openTab, renameTabs, type Layout } from "@/components/workflows/panels/layout";
import { getTabs, isKnownTab, registerTab, registerTabFamily, resolveTab } from "@/components/workflows/panels/registry";
import { LAYOUT_KEY_PREFIX, usePersistedLayout } from "@/components/workflows/panels/usePersistedLayout";

/**
 * TAB FAMILIES (W7d spec §4.3): one registration answers every tab id under a
 * prefix, the way the flow editor's `file:<path>` tabs need. A `doc:` family
 * stands in for it here, so these tests own their registrations.
 */
type Ctx = { dirty: ReadonlySet<string> };

const nameOf = (id: string) => id.slice("doc:".length).split("/").pop() ?? id;

beforeAll(() => {
  registerTab<Ctx>({ id: CODE_TAB, title: "Code", icon: Code, closable: false, render: () => <p>Code body</p> });
  registerTab<Ctx>({ id: "docs", title: "Docs", icon: FileText, closable: true, render: () => <p>Docs body</p> });
  registerTabFamily<Ctx>({
    prefix: "doc:",
    title: nameOf,
    label: (id) => `${nameOf(id)} (${id.slice("doc:".length)})`,
    icon: FileCode,
    closable: true,
    render: (id) => <p>{`Body of ${id}`}</p>,
    badge: (id, ctx) => (ctx.dirty.has(id) ? <span aria-hidden data-testid="dot">•</span> : null),
    badgeLabel: (id, ctx) => (ctx.dirty.has(id) ? "unsaved" : undefined),
  });
});

beforeEach(() => localStorage.clear());

describe("registerTabFamily", () => {
  it("resolves any id under its prefix to a tab of that id, and nothing else", () => {
    const def = resolveTab("doc:guides/intro.md");
    expect(def?.id).toBe("doc:guides/intro.md");
    expect(def?.title).toBe("intro.md");
    expect(def?.label).toBe("intro.md (guides/intro.md)");
    expect(def?.closable).toBe(true);
    expect(resolveTab("docs")?.title).toBe("Docs");
    expect(resolveTab("nope:x")).toBeUndefined();
    expect(isKnownTab("doc:a.md")).toBe(true);
    expect(isKnownTab("docs")).toBe(true);
    expect(isKnownTab("nope:x")).toBe(false);
  });

  it("is not a tab kind of its own: the list of tabs (and the + menu) never holds a family", () => {
    expect(getTabs().map((t) => t.id)).toEqual([CODE_TAB, "docs"]);
  });
});

const START: Layout = {
  panels: [
    { id: "p1", size: 60, tabs: [CODE_TAB, "doc:guides/intro.md"], active: "doc:guides/intro.md" },
  ],
};

function Harness({ dirty = new Set<string>() }: { dirty?: ReadonlySet<string> }) {
  const [layout, setLayout] = useState(START);
  return <MultiPanel<Ctx> layout={layout} onChange={setLayout} context={{ dirty }} />;
}

describe("MultiPanel with a family tab", () => {
  it("shows its title, names it with its label, and renders its body for its id", () => {
    render(<Harness />);
    const tab = screen.getByRole("tab", { name: "intro.md (guides/intro.md)" });
    expect(tab).toHaveTextContent("intro.md");
    expect(tab).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: "intro.md (guides/intro.md)" })).toHaveTextContent("Body of doc:guides/intro.md");
  });

  it("adds its badge, and the badge's words to its name", () => {
    render(<Harness dirty={new Set(["doc:guides/intro.md"])} />);
    expect(screen.getByRole("tab", { name: "intro.md (guides/intro.md), unsaved" })).toBeInTheDocument();
    expect(screen.getByTestId("dot")).toBeInTheDocument();
  });

  it("closes like any closable tab, and the + menu does not offer it back", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    screen.getByRole("tab", { name: "intro.md (guides/intro.md)" }).focus();
    await user.keyboard("{Delete}");
    expect(screen.queryByRole("tab", { name: /intro\.md/ })).toBeNull();
    screen.getByRole("button", { name: "Reopen a tab in panel 1" }).focus();
    await user.keyboard("{Enter}");
    const items = (await screen.findAllByRole("menuitem")).map((m) => m.textContent);
    expect(items).toEqual(["Docs"]);
  });
});

describe("a stored layout naming family tabs", () => {
  it("keeps a tab under a registered prefix and drops one under an unknown prefix", () => {
    const stored: Layout = {
      panels: [
        { id: "p1", size: 50, tabs: [CODE_TAB, "doc:a.md", "nope:b"], active: "nope:b" },
        { id: "p2", size: 50, tabs: ["nope:c"], active: "nope:c" },
      ],
    };
    localStorage.setItem(`${LAYOUT_KEY_PREFIX}flow-1`, JSON.stringify(stored));
    const { result } = renderHook(() => usePersistedLayout("flow-1", START));
    expect(result.current[0]).toEqual({
      panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "doc:a.md"], active: CODE_TAB }],
    });
  });

  it("keeps a family tab named by a layout read before the family registered", () => {
    localStorage.setItem(
      `${LAYOUT_KEY_PREFIX}flow-2`,
      JSON.stringify({ panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "late:x"], active: "late:x" }] }),
    );
    const { result } = renderHook(() => usePersistedLayout("flow-2", START));
    expect(result.current[0].panels[0].tabs).toEqual([CODE_TAB]);
    act(() => registerTabFamily({ prefix: "late:", title: (id) => id, icon: FileCode, closable: true, render: () => null }));
    expect(result.current[0].panels[0].tabs).toEqual([CODE_TAB, "late:x"]);
  });
});

describe("openTab", () => {
  const TWO: Layout = {
    panels: [
      { id: "p1", size: 50, tabs: ["docs"], active: "docs" },
      { id: "p2", size: 50, tabs: [CODE_TAB, "doc:a.md"], active: CODE_TAB },
    ],
  };

  it("opens a closed tab at the end of the panel holding Code, and selects it", () => {
    expect(openTab(TWO, "doc:b.md")).toEqual({
      panels: [
        TWO.panels[0],
        { id: "p2", size: 50, tabs: [CODE_TAB, "doc:a.md", "doc:b.md"], active: "doc:b.md" },
      ],
    });
  });

  it("selects an open tab where it is rather than opening it twice", () => {
    const moved = openTab(TWO, "doc:a.md");
    expect(moved.panels[1]).toEqual({ ...TWO.panels[1], active: "doc:a.md" });
    expect(moved.panels.flatMap((p) => p.tabs).filter((t) => t === "doc:a.md")).toHaveLength(1);
  });

  it("returns the same layout when the tab is open and selected already", () => {
    const shown = openTab(TWO, "docs");
    expect(shown).toBe(TWO);
  });
});

describe("renameTabs", () => {
  it("renames tabs where they stand, the selection with them", () => {
    const l: Layout = {
      panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "doc:a/x.md", "doc:b.md", "doc:a/y.md"], active: "doc:a/y.md" }],
    };
    const next = renameTabs(l, (t) => (t.startsWith("doc:a/") ? `doc:z/${t.slice("doc:a/".length)}` : t));
    expect(next.panels[0]).toEqual({ id: "p1", size: 100, tabs: [CODE_TAB, "doc:z/x.md", "doc:b.md", "doc:z/y.md"], active: "doc:z/y.md" });
  });

  it("keeps one tab when a rename lands on a tab already open, and returns the same layout when nothing changes", () => {
    const l: Layout = {
      panels: [
        { id: "p1", size: 50, tabs: [CODE_TAB, "doc:b.md"], active: "doc:b.md" },
        { id: "p2", size: 50, tabs: ["doc:a.md"], active: "doc:a.md" },
      ],
    };
    const next = renameTabs(l, (t) => (t === "doc:a.md" ? "doc:b.md" : t));
    expect(next.panels.flatMap((p) => p.tabs)).toEqual([CODE_TAB, "doc:b.md"]);
    expect(next.panels).toHaveLength(1);
    expect(next.panels[0].size).toBe(100);
    expect(renameTabs(l, (t) => t)).toBe(l);
  });
});

describe("MultiPanel's + menu in a panel", () => {
  it("lists closed concrete tabs only", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    screen.getByRole("button", { name: "Reopen a tab in panel 1" }).focus();
    await user.keyboard("{Enter}");
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Docs"]);
  });
});
