import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { VariablesPanel } from "../VariablesPanel";
import type { VarInfo } from "@/lib/kernel/KernelConnection";

const VARS: VarInfo[] = [
  { name: "x", type: "int", columns: null },
  { name: "df", type: "DataFrame", columns: ["a", "b"] },
];

function renderPanel(overrides: Partial<Parameters<typeof VariablesPanel>[0]> = {}) {
  const onRefresh = vi.fn();
  const onDisplay = vi.fn();
  render(
    <VariablesPanel
      vars={overrides.vars ?? VARS}
      loading={overrides.loading ?? false}
      onRefresh={overrides.onRefresh ?? onRefresh}
      onDisplay={overrides.onDisplay ?? onDisplay}
    />,
  );
  return { onRefresh, onDisplay };
}

describe("VariablesPanel", () => {
  it("renders each variable's name and type", () => {
    renderPanel();
    expect(screen.getByText("x")).toBeInTheDocument();
    expect(screen.getByText("int")).toBeInTheDocument();
    expect(screen.getByText("df")).toBeInTheDocument();
    expect(screen.getByText("DataFrame")).toBeInTheDocument();
  });

  it("does not show columns until expanded, then reveals them", async () => {
    const user = userEvent.setup();
    renderPanel();
    expect(screen.queryByText("a, b")).not.toBeInTheDocument();

    await user.click(screen.getByLabelText("Expand df"));
    expect(screen.getByText("a, b")).toBeInTheDocument();
  });

  it("does not render an expander for a variable with no columns", () => {
    renderPanel();
    expect(screen.queryByLabelText("Expand x")).not.toBeInTheDocument();
  });

  it("clicking a variable's name calls onDisplay with its name", async () => {
    const user = userEvent.setup();
    const { onDisplay } = renderPanel();
    await user.click(screen.getByText("df"));
    expect(onDisplay).toHaveBeenCalledWith("df");
  });

  it("shows an empty-state hint when there are no variables and not loading", () => {
    renderPanel({ vars: [] });
    expect(screen.getByText(/no variables yet/i)).toBeInTheDocument();
  });

  it("does not show the empty-state hint while loading", () => {
    renderPanel({ vars: [], loading: true });
    expect(screen.queryByText(/no variables yet/i)).not.toBeInTheDocument();
  });

  it("clicking refresh calls onRefresh", async () => {
    const user = userEvent.setup();
    const { onRefresh } = renderPanel();
    await user.click(screen.getByLabelText("Refresh variables"));
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });
});
