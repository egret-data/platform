import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LAYOUT_FAILED, TopologyView } from "@/components/workflows/topology/TopologyView";
import { FLOW, STATES } from "./fixtures";

// A layout that throws, whatever the reason: the tab must survive it (fix round 2).
vi.mock("@/components/workflows/topology/layout", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/components/workflows/topology/layout")>();
  return {
    ...real,
    layout: vi.fn(() => {
      throw new RangeError("Maximum call stack size exceeded");
    }),
  };
});

describe("TopologyView when the layout fails", () => {
  it("says the graph could not be drawn, offers the list view, and the list works", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <div style={{ width: 800, height: 600 }}>
        <TopologyView graph={FLOW} states={STATES} onSelect={onSelect} onExpandSubflow={vi.fn()} />
      </div>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(LAYOUT_FAILED);
    expect(screen.getByRole("alert")).toHaveTextContent("Use the list view to read its tasks and their states.");
    expect(document.querySelector(".react-flow")).toBeNull();
    // Controls that act on a drawing are not offered; the ones that still make sense are.
    expect(screen.queryByRole("button", { name: "Zoom in" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Vertical" })).toBeNull();
    expect(screen.getByRole("button", { name: "List view" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Collapse all" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Show the list view" }));
    const grid = screen.getByRole("treegrid", { name: "Tasks of the flow" });
    const row = within(grid)
      .getAllByRole("row")
      .find((r) => r.getAttribute("data-uid") === "tasks.a") as HTMLElement;
    expect(within(row).getByText("Success")).toBeInTheDocument();
    await user.click(row);
    expect(onSelect).toHaveBeenCalledWith("tasks.a");
    // Nested by cluster without a layout: the Parallel's members sit one level down.
    const x = within(grid)
      .getAllByRole("row")
      .find((r) => r.getAttribute("data-uid") === "tasks.p.x") as HTMLElement;
    expect(x).toHaveAttribute("aria-level", "2");
  });

  it("in the editor, where nothing has run, does not promise states", () => {
    render(
      <div style={{ width: 800, height: 600 }}>
        <TopologyView graph={FLOW} onSelect={vi.fn()} onExpandSubflow={vi.fn()} />
      </div>,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Use the list view to read its tasks.");
    expect(alert).not.toHaveTextContent("states");
  });
});
