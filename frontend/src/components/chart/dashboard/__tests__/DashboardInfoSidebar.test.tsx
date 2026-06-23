import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardInfoSidebar } from "../DashboardInfoSidebar";

describe("DashboardInfoSidebar", () => {
  const base = { name: "Sales", description: "old", ownerLabel: "admin@egret.local", updatedAt: "2026-06-22T04:08:30Z" };
  it("shows owner + last edited and renders the description", () => {
    render(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("admin@egret.local")).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
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
