import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { DashboardInfoSidebar } from "../DashboardInfoSidebar";

const wrap = (ui: React.ReactElement) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe("DashboardInfoSidebar", () => {
  const base = {
    name: "Sales",
    description: "old",
    ownerLabel: "Ref User",
    createdAt: "2026-08-01T12:00:00Z",
    editorLabel: "Other User",
    updatedAt: "2026-08-05T12:00:00Z",
    folderName: "Ops",
    folderHref: "/explore/demo/dashboards?folder=f1",
  };
  it("shows both attribution lines by name and renders the description", () => {
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [12], 2026 by Ref User$/)).toBeTruthy();
    expect(screen.getByText(/^August [56], 2026 by Other User$/)).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
  });
  it("falls back to the creator when nothing has edited the dashboard since 0106", () => {
    wrap(<DashboardInfoSidebar {...base} editorLabel={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [56], 2026 by Ref User$/)).toBeTruthy();
  });
  it("omits the Added row for a dashboard with no recorded creation date", () => {
    wrap(<DashboardInfoSidebar {...base} createdAt={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByText("Added")).toBeNull();
  });
  it("links Saved in at the folder's section on the list", () => {
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    const link = screen.getByRole("link", { name: "Ops" });
    expect(link.getAttribute("href")).toBe("/explore/demo/dashboards?folder=f1");
  });
  it("says Uncategorized, with no link, for a dashboard in no folder", () => {
    wrap(<DashboardInfoSidebar {...base} folderName={null} folderHref={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("Uncategorized")).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });
  it("saves an edited description on blur", () => {
    const onSave = vi.fn();
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    const ta = screen.getByLabelText("description");
    fireEvent.change(ta, { target: { value: "new desc" } });
    fireEvent.blur(ta);
    expect(onSave).toHaveBeenCalledWith("new desc");
  });
  it("does not save when the description is unchanged", () => {
    const onSave = vi.fn();
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    fireEvent.blur(screen.getByLabelText("description"));
    expect(onSave).not.toHaveBeenCalled();
  });
});
