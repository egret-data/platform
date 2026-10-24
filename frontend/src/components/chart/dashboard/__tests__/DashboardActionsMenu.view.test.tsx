import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  DASHBOARD_FIXED_WIDTH_PX,
  DashboardViewActionsMenu,
  dashboardWidthClass,
} from "../DashboardActionsMenu.view";

describe("dashboardWidthClass", () => {
  it("centres a fixed dashboard at Metabase's measured width and leaves a full one unconstrained", () => {
    // Measured live at an 1800px viewport on localhost:3033: Metabase's grid stayed 1048px wide.
    // egret's previous measure was 1200px, which is not a number Metabase has anywhere.
    expect(DASHBOARD_FIXED_WIDTH_PX).toBe(1048);
    expect(dashboardWidthClass("fixed")).toBe("mx-auto w-full max-w-[1048px]");
    expect(dashboardWidthClass("full")).toBe("");
  });

  it("keeps the literal class in step with the constant", () => {
    // Tailwind's scanner emits only classes it can see written out in the source, so the class
    // CANNOT be built from the constant — a template literal would render the right class name
    // with no CSS behind it. This assertion is the only thing stopping the two from drifting.
    expect(dashboardWidthClass("fixed")).toContain(`max-w-[${DASHBOARD_FIXED_WIDTH_PX}px]`);
  });
});

// KI-068: the side panel sat INSIDE the 1048px measure, so opening it left the canvas 692px and
// the grid flipped to its one-column phone layout. The measure is the canvas's, and the panel
// sits beside it.
describe("dashboardWidthClass with a side panel open (KI-068)", () => {
  it("widens a fixed dashboard by the panel and its gap, so the canvas keeps its measure", () => {
    expect(dashboardWidthClass("fixed", "edit")).toBe(`mx-auto w-full max-w-[${DASHBOARD_FIXED_WIDTH_PX + 16 + 340}px]`);
    expect(dashboardWidthClass("fixed", "info")).toBe(`mx-auto w-full max-w-[${DASHBOARD_FIXED_WIDTH_PX + 16 + 300}px]`);
  });

  it("leaves a full-width dashboard unconstrained", () => {
    expect(dashboardWidthClass("full", "edit")).toBe("");
    expect(dashboardWidthClass("full", "info")).toBe("");
  });
});

describe("DashboardViewActionsMenu", () => {
  it("shows the switch off for a fixed dashboard and reports 'full' when it is turned on", async () => {
    const user = userEvent.setup();
    const onChangeWidth = vi.fn();
    render(<DashboardViewActionsMenu width="fixed" onChangeWidth={onChangeWidth} />);

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    const sw = await screen.findByRole("switch", { name: "Full width" });
    expect(sw).toHaveAttribute("aria-checked", "false");

    await user.click(sw);
    expect(onChangeWidth).toHaveBeenCalledWith("full");
  });

  it("shows the switch on for a full dashboard and reports 'fixed' when it is turned off", async () => {
    // Both directions are asserted because a handler hard-wired to one of them passes the other
    // test and is exactly the mutation Step 5 introduces.
    const user = userEvent.setup();
    const onChangeWidth = vi.fn();
    render(<DashboardViewActionsMenu width="full" onChangeWidth={onChangeWidth} />);

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    const sw = await screen.findByRole("switch", { name: "Full width" });
    expect(sw).toHaveAttribute("aria-checked", "true");

    await user.click(sw);
    expect(onChangeWidth).toHaveBeenCalledWith("fixed");
  });

  it("renders extra rows, and renders no switch when no width is supplied", async () => {
    // This pins the contract W4 depends on: the same menu is rendered in VIEW mode, where there
    // is no draft width to toggle, carrying only its action rows.
    const user = userEvent.setup();
    render(
      <DashboardViewActionsMenu>
        <button type="button">Duplicate</button>
      </DashboardViewActionsMenu>,
    );

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    expect(await screen.findByRole("button", { name: "Duplicate" })).toBeInTheDocument();
    expect(screen.queryByRole("switch")).toBeNull();
  });
});
