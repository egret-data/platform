import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SidebarIconRail } from "../SidebarIconRail";

describe("SidebarIconRail", () => {
  it("calls onPick with the tab key", async () => {
    const onPick = vi.fn();
    render(<SidebarIconRail tab="toc" open onPick={onPick} />);
    await userEvent.click(screen.getByRole("button", { name: /data sources/i }));
    expect(onPick).toHaveBeenCalledWith("dataSources");
  });
});
