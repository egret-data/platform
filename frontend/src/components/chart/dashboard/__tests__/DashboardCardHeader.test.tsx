import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DashboardCardHeader } from "../DashboardCardHeader";

function renderHeader(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

describe("DashboardCardHeader", () => {
  it("renders the title as a link when there is an href", () => {
    renderHeader(<DashboardCardHeader text="Revenue" href="/explore/demo/charts/ch1" />);
    expect(screen.getByRole("link", { name: "Revenue" })).toHaveAttribute("href", "/explore/demo/charts/ch1");
  });

  it("renders plain text when there is none, and no link at all", () => {
    renderHeader(<DashboardCardHeader text="Revenue" />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offers no description affordance when there is no description", () => {
    renderHeader(<DashboardCardHeader text="Revenue" />);
    expect(screen.queryByRole("button", { name: "Card description" })).toBeNull();
  });

  it("shows the description behind an info affordance", async () => {
    const user = userEvent.setup();
    renderHeader(<DashboardCardHeader text="Revenue" description="Net of refunds." />);
    expect(screen.queryByText("Net of refunds.")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Card description" }));
    expect(await screen.findByText("Net of refunds.")).toBeInTheDocument();
  });

  it("carries dash-nodrag on both affordances, so a click is not swallowed by the grid", () => {
    // react-grid-layout's draggableCancel is `.dash-nodrag` (DashboardGridLayout). Without it the
    // link and the info button are drag handles first and controls second.
    renderHeader(<DashboardCardHeader text="Revenue" href="/x" description="d" />);
    expect(screen.getByRole("link", { name: "Revenue" }).className).toContain("dash-nodrag");
    expect(screen.getByRole("button", { name: "Card description" }).className).toContain("dash-nodrag");
  });
});
