import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { DashboardHistoryTab } from "../DashboardHistoryTab";
import type { DashboardRevision } from "@/api/dashboards";

const revisions: DashboardRevision[] = [
  { id: "r3", kind: "revert", created_at: "2026-08-05T12:00:00Z", created_by: "u1", created_by_name: "Ref User", is_current: true },
  { id: "r2", kind: "edit", created_at: "2026-08-03T12:00:00Z", created_by: "u2", created_by_name: null, is_current: false },
  { id: "r1", kind: "baseline", created_at: "2026-08-01T12:00:00Z", created_by: "u1", created_by_name: "Ref User", is_current: false },
];

describe("DashboardHistoryTab", () => {
  it("lists every revision newest first with its sentence and date", () => {
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[0].textContent).toContain("Ref User reverted to an earlier version");
    expect(items[1].textContent).toContain("Someone edited this");
    expect(items[2].textContent).toContain("Ref User created this");
    expect(items[2].textContent).toMatch(/August [12], 2026/);
  });

  it("offers revert on every entry except the current one", () => {
    // is_current comes from the API. Deriving "newest" from array position here would go silently
    // wrong the day the endpoint gains a sort parameter.
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    expect(screen.getAllByRole("button", { name: "Revert" })).toHaveLength(2);
  });

  it("keys the revert action on is_current, not on array position", () => {
    // The same three revisions with is_current on the OLDEST row. A component that hid the
    // action on index 0 would still show exactly two buttons above, so only this fixture — where
    // position and is_current disagree — can tell the two implementations apart.
    const currentIsOldest = revisions.map((r) => ({ ...r, is_current: r.id === "r1" }));
    render(<DashboardHistoryTab revisions={currentIsOldest} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    const buttons = screen.getAllByRole("button", { name: "Revert" });
    expect(buttons).toHaveLength(2);
    const items = screen.getAllByRole("listitem");
    expect(items[0].querySelector("button")).not.toBeNull();
    expect(items[2].querySelector("button")).toBeNull();
  });

  it("passes the revision id, not the index, to onRevert", () => {
    const onRevert = vi.fn();
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={onRevert} reverting={false} />);
    fireEvent.click(screen.getAllByRole("button", { name: "Revert" })[1]);
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Revert" }));
    expect(onRevert).toHaveBeenCalledWith("r1");
  });

  it("says so when there is no history yet", () => {
    render(<DashboardHistoryTab revisions={[]} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    expect(screen.getByText("No history yet — this dashboard hasn't been saved since revisions were turned on.")).toBeTruthy();
  });

  it("asks before reverting, and reverts only on confirm (KI-075)", () => {
    const onRevert = vi.fn();
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={onRevert} reverting={false} />);
    fireEvent.click(screen.getAllByRole("button", { name: "Revert" })[0]);
    expect(onRevert).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onRevert).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByRole("button", { name: "Revert" })[0]);
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Revert" }));
    expect(onRevert).toHaveBeenCalledWith("r2");
  });
});
