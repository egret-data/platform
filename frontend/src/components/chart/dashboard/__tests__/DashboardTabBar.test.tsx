import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardTabBar } from "../DashboardTabBar";
import type { DraftTab } from "../tabs";

const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];

describe("DashboardTabBar", () => {
  it("renders tabs and switches on click", () => {
    const onSelect = vi.fn();
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={onSelect} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    expect(onSelect).toHaveBeenCalledWith(1);
  });
  it("shows the add button only in edit mode", () => {
    const { rerender } = render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "add tab" })).toBeNull();
    rerender(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.getByRole("button", { name: "add tab" })).toBeTruthy();
  });
  it("double-click commits a rename in edit mode", () => {
    const onRename = vi.fn();
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={onRename} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    fireEvent.doubleClick(screen.getByRole("tab", { name: "A" }));
    const input = screen.getByLabelText("rename tab A") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Sales" } });
    fireEvent.blur(input);
    expect(onRename).toHaveBeenCalledWith("ka", "Sales");
  });
  it("delete is disabled with fewer than 2 tabs", () => {
    render(<DashboardTabBar tabs={[tabs[0]]} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect((screen.getByRole("button", { name: "delete tab A" }) as HTMLButtonElement).disabled).toBe(true);
  });
  it("active tab uses the brand colour", () => {
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.getByRole("tab", { name: "A" }).className).toContain("border-primary");
  });
});
