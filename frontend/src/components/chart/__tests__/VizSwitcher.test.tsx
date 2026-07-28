import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { VizSwitcher } from "../VizSwitcher";

describe("VizSwitcher", () => {
  it("shows the active viz name", () => {
    render(<VizSwitcher viz="bar" onChange={vi.fn()} />);
    expect(screen.getByText("Bar")).toBeInTheDocument();
  });

  it("opens the gallery and reports the chosen type", async () => {
    const onChange = vi.fn();
    render(<VizSwitcher viz="bar" onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Change" }));
    await userEvent.click(await screen.findByText("Pie"));
    expect(onChange).toHaveBeenCalledWith("pie");
  });

  it("closes the gallery after a choice", async () => {
    render(<VizSwitcher viz="bar" onChange={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Change" }));
    await userEvent.click(await screen.findByText("Pie"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
