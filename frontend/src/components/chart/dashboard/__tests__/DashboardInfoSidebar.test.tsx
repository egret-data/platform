import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardInfoSidebar } from "../DashboardInfoSidebar";

describe("DashboardInfoSidebar", () => {
  const base = {
    name: "Sales",
    description: "old",
    ownerLabel: "Ref User",
    createdAt: "2026-08-01T12:00:00Z",
    editorLabel: "Other User",
    updatedAt: "2026-08-05T12:00:00Z",
  };
  it("shows both attribution lines by name and renders the description", () => {
    render(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [12], 2026 by Ref User$/)).toBeTruthy();
    expect(screen.getByText(/^August [56], 2026 by Other User$/)).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
  });
  it("falls back to the creator when nothing has edited the dashboard since 0106", () => {
    render(<DashboardInfoSidebar {...base} editorLabel={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [56], 2026 by Ref User$/)).toBeTruthy();
  });
  it("omits the Added row for a dashboard with no recorded creation date", () => {
    render(<DashboardInfoSidebar {...base} createdAt={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByText("Added")).toBeNull();
  });
  it("saves an edited description on blur", () => {
    const onSave = vi.fn();
    render(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    const ta = screen.getByLabelText("description");
    fireEvent.change(ta, { target: { value: "new desc" } });
    fireEvent.blur(ta);
    expect(onSave).toHaveBeenCalledWith("new desc");
  });
  it("does not save when the description is unchanged", () => {
    const onSave = vi.fn();
    render(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    fireEvent.blur(screen.getByLabelText("description"));
    expect(onSave).not.toHaveBeenCalled();
  });
});
