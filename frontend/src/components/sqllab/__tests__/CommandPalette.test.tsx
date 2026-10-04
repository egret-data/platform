import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { CommandPalette, type PaletteAction } from "../CommandPalette";

function renderPalette(actions: PaletteAction[], onOpenChange = vi.fn()) {
  const user = userEvent.setup();
  render(<CommandPalette open actions={actions} onOpenChange={onOpenChange} />);
  return { user, onOpenChange };
}

describe("CommandPalette", () => {
  it("renders the given actions when open", () => {
    renderPalette([
      { id: "a", label: "Add code cell", run: vi.fn() },
      { id: "b", label: "Run all", run: vi.fn() },
      { id: "c", label: "Clear outputs", run: vi.fn() },
    ]);

    expect(screen.getByText("Add code cell")).toBeInTheDocument();
    expect(screen.getByText("Run all")).toBeInTheDocument();
    expect(screen.getByText("Clear outputs")).toBeInTheDocument();
  });

  it("filters the list by a case-insensitive substring match on the query", async () => {
    const { user } = renderPalette([
      { id: "a", label: "Add code cell", run: vi.fn() },
      { id: "b", label: "Run all", run: vi.fn() },
      { id: "c", label: "Clear outputs", run: vi.fn() },
    ]);

    await user.type(screen.getByPlaceholderText("Type a command…"), "RUN");

    expect(screen.getByText("Run all")).toBeInTheDocument();
    expect(screen.queryByText("Add code cell")).not.toBeInTheDocument();
    expect(screen.queryByText("Clear outputs")).not.toBeInTheDocument();
  });

  it("Enter runs the selected action and closes the palette", async () => {
    const runA = vi.fn();
    const runB = vi.fn();
    const { user, onOpenChange } = renderPalette([
      { id: "a", label: "Add code cell", run: runA },
      { id: "b", label: "Run all", run: runB },
    ]);

    const input = screen.getByPlaceholderText("Type a command…");
    await user.type(input, "{Enter}");

    // Selection starts at index 0 → first action's run fires.
    expect(runA).toHaveBeenCalledTimes(1);
    expect(runB).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("ArrowDown moves the selection so Enter runs the second action", async () => {
    const runA = vi.fn();
    const runB = vi.fn();
    const { user, onOpenChange } = renderPalette([
      { id: "a", label: "Add code cell", run: runA },
      { id: "b", label: "Run all", run: runB },
    ]);

    const input = screen.getByPlaceholderText("Type a command…");
    await user.type(input, "{ArrowDown}{Enter}");

    expect(runB).toHaveBeenCalledTimes(1);
    expect(runA).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("clicking a row runs that action and closes the palette", async () => {
    const runA = vi.fn();
    const runB = vi.fn();
    const { user, onOpenChange } = renderPalette([
      { id: "a", label: "Add code cell", run: runA },
      { id: "b", label: "Run all", run: runB },
    ]);

    await user.click(screen.getByText("Run all"));

    expect(runB).toHaveBeenCalledTimes(1);
    expect(runA).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
