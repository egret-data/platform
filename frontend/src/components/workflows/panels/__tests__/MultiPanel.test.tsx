import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BookOpen, Code, FileText, ListTree, TriangleAlert } from "lucide-react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { CODE_TAB, type Layout } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";

type Ctx = { flow: string };

beforeAll(() => {
  const tabs = [
    [CODE_TAB, "Code", Code],
    ["docs", "Docs", FileText],
    ["problems", "Problems", TriangleAlert],
    ["outline", "Outline", ListTree],
    ["guide", "Guide", BookOpen],
  ] as const;
  for (const [id, title, icon] of tabs) {
    registerTab<Ctx>({
      id, title, icon, closable: id !== CODE_TAB,
      render: (ctx) => <p>{`${title} body for ${ctx.flow}`}</p>,
    });
  }
});

/**
 * Three tabs in the second panel with the MIDDLE one active, so arrow keys
 * have a neighbour on both sides; `outline` and `guide` are registered and
 * closed, so the "+" menu has something to list and something to leave out.
 */
const START: Layout = {
  panels: [
    { id: "p1", size: 60, tabs: [CODE_TAB], active: CODE_TAB },
    { id: "p2", size: 40, tabs: ["docs", "problems", "guide"], active: "problems" },
  ],
};

/** The real component, held by real state, so a test sees what a change does to the DOM. */
function Harness({ initial = START, spy }: { initial?: Layout; spy?: (l: Layout) => void }) {
  const [layout, setLayout] = useState(initial);
  return (
    <div style={{ height: 400 }}>
      <MultiPanel<Ctx>
        layout={layout}
        onChange={(l) => { spy?.(l); setLayout(l); }}
        context={{ flow: "hello-world" }}
      />
    </div>
  );
}

/**
 * Menus are opened from the keyboard, which is also the path FE-A11Y-3 asks
 * for. A pointer press cannot be used here: jsdom gives every element a zero
 * rect, so react-resizable-panels' document-level `pointerdown` handler sees
 * the press inside a resize hit region and cancels it. A real browser only
 * does that near a gutter; Task 8 drives the pointer there.
 */
async function openMenu(user: ReturnType<typeof userEvent.setup>, name: string) {
  screen.getByRole("button", { name }).focus();
  await user.keyboard("{Enter}");
}

const strip = (n: number) => screen.getByRole("tablist", { name: `Panel ${n}` });

describe("MultiPanel", () => {
  it("renders one tablist per panel, each showing its active tab's content with the context", () => {
    render(<Harness />);
    expect(screen.getAllByRole("tablist")).toHaveLength(2);
    expect(within(strip(1)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Code"]);
    expect(within(strip(2)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Problems", "Guide"]);
    expect(within(strip(2)).getByRole("tab", { name: "Problems" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Code body for hello-world")).toBeInTheDocument();
    expect(screen.getByText("Problems body for hello-world")).toBeInTheDocument();
    expect(screen.queryByText("Docs body for hello-world")).toBeNull();
    const panel = screen.getByRole("tabpanel", { name: "Problems" });
    expect(panel).toHaveTextContent("Problems body for hello-world");
  });

  it("moves selection and focus with the arrow keys, wrapping, with Home and End", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const tabs = () => within(strip(2)).getAllByRole("tab");
    // Roving tabindex: only the selected tab is in the tab order.
    expect(tabs().map((t) => t.tabIndex)).toEqual([-1, 0, -1]);
    tabs()[1].focus();

    await user.keyboard("{ArrowRight}");
    expect(tabs()[2]).toHaveFocus();
    expect(tabs()[2]).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Guide body for hello-world")).toBeInTheDocument();

    await user.keyboard("{ArrowRight}");
    expect(tabs()[0]).toHaveFocus();
    expect(tabs()[0]).toHaveAttribute("aria-selected", "true");

    await user.keyboard("{ArrowLeft}");
    expect(tabs()[2]).toHaveFocus();
    await user.keyboard("{Home}");
    expect(tabs()[0]).toHaveFocus();
    await user.keyboard("{End}");
    expect(tabs()[2]).toHaveFocus();
    // The other panel is untouched by this strip's keys.
    expect(within(strip(1)).getByRole("tab", { name: "Code" })).toHaveAttribute("aria-selected", "true");
  });

  it("selects a tab on click", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(within(strip(2)).getByRole("tab", { name: "Docs" }));
    expect(within(strip(2)).getByRole("tab", { name: "Docs" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Docs body for hello-world")).toBeInTheDocument();
  });

  it("Split right from the tab menu puts the tab in a new panel to the right", async () => {
    const user = userEvent.setup();
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    await openMenu(user, "Tab actions: Problems");
    await user.click(await screen.findByRole("menuitem", { name: "Split right" }));
    expect(screen.getAllByRole("tablist")).toHaveLength(3);
    expect(within(strip(2)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Guide"]);
    expect(within(strip(3)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Problems"]);
    const sizes = (spy.mock.calls[0][0] as Layout).panels.map((p) => p.size);
    expect(sizes).toEqual([60, 20, 20]);
  });

  it("Split left from the tab menu puts the tab in a new panel to the left", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await openMenu(user, "Tab actions: Problems");
    await user.click(await screen.findByRole("menuitem", { name: "Split left" }));
    expect(within(strip(2)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Problems"]);
    expect(within(strip(3)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Guide"]);
  });

  it("disables splitting a panel's only tab and says why", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await openMenu(user, "Tab actions: Code");
    const item = await screen.findByRole("menuitem", { name: /Split right/ });
    expect(item).toHaveAttribute("aria-disabled", "true");
    expect(item).toHaveTextContent("Only tab in this panel");
  });

  it("Move to panel… moves the tab to the end of the chosen panel", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await openMenu(user, "Tab actions: Problems");
    // The submenu from the keyboard too: ArrowRight opens it on its first item.
    expect(await screen.findByRole("menuitem", { name: "Move to panel…" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("menuitem", { name: "Panel 1 (Code)" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(within(strip(1)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Code", "Problems"]);
    expect(within(strip(1)).getByRole("tab", { name: "Problems" })).toHaveAttribute("aria-selected", "true");
    expect(within(strip(2)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Guide"]);
  });

  it("Move to panel… says why it has nothing to offer when there is one panel", async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "docs"], active: "docs" }] }} />);
    await openMenu(user, "Tab actions: Docs");
    await user.keyboard("{ArrowRight}");
    const item = await screen.findByRole("menuitem", { name: "No other panel. Split a tab to make one." });
    expect(item).toHaveAttribute("aria-disabled", "true");
  });

  it("Close from the tab menu closes the tab, and the last tab's panel goes with it", async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ panels: [
      { id: "p1", size: 60, tabs: [CODE_TAB], active: CODE_TAB },
      { id: "p2", size: 40, tabs: ["docs"], active: "docs" },
    ] }} />);
    await openMenu(user, "Tab actions: Docs");
    await user.click(await screen.findByRole("menuitem", { name: "Close" }));
    expect(screen.getAllByRole("tablist")).toHaveLength(1);
    expect(screen.queryByRole("tab", { name: "Docs" })).toBeNull();
  });

  it("closes the focused tab with Delete", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    within(strip(2)).getByRole("tab", { name: "Problems" }).focus();
    await user.keyboard("{Delete}");
    expect(within(strip(2)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Guide"]);
    // Focus follows to the tab that became active rather than falling to <body>.
    expect(within(strip(2)).getByRole("tab", { name: "Guide" })).toHaveFocus();
  });

  it("has no Close item for the Code tab, and Delete does nothing to it", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await openMenu(user, "Tab actions: Code");
    await screen.findByRole("menuitem", { name: /Split right/ });
    expect(screen.queryByRole("menuitem", { name: "Close" })).toBeNull();
    await user.keyboard("{Escape}");
    within(strip(1)).getByRole("tab", { name: "Code" }).focus();
    await user.keyboard("{Delete}");
    expect(within(strip(1)).getByRole("tab", { name: "Code" })).toBeInTheDocument();
  });

  it("the + menu lists only the closed tabs, and reopens one into its panel", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await openMenu(user, "Reopen a tab in panel 1");
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Outline"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Outline" }));
    expect(within(strip(1)).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Code", "Outline"]);
    expect(screen.getByText("Outline body for hello-world")).toBeInTheDocument();
  });

  it("the + menu says so when every tab is open", async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ panels: [
      { id: "p1", size: 100, tabs: [CODE_TAB, "docs", "problems", "outline", "guide"], active: CODE_TAB },
    ] }} />);
    await openMenu(user, "Reopen a tab in panel 1");
    const item = await screen.findByRole("menuitem", { name: "Every tab is open" });
    expect(item).toHaveAttribute("aria-disabled", "true");
  });

  it("gives each resize gutter a separator role, so it can be reached from the keyboard", () => {
    render(<Harness />);
    expect(screen.getAllByRole("separator")).toHaveLength(1);
  });
});
