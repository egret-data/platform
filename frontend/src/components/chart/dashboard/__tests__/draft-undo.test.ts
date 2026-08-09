import { describe, it, expect, vi, beforeEach } from "vitest";
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
import { toast } from "sonner";
import { REMOVED_CARD, REPLACED_CARD, deletedTab, undoableDraftEdit } from "../draft-undo";

const lastCall = () => (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;

beforeEach(() => vi.clearAllMocks());

describe("draft undo messages", () => {
  it("uses Metabase's wording for the three undoable edits", () => {
    // cards-typed.ts:508, DashboardGrid.tsx:699-713, use-dashboard-tabs.ts:57. The replace message
    // has one form here where Metabase has three, because an egret dashboard card is always a chart.
    expect(REMOVED_CARD).toBe("Removed card");
    expect(REPLACED_CARD).toBe("Chart replaced");
    expect(deletedTab("Overview")).toBe('Deleted "Overview"');
  });
});

describe("undoableDraftEdit", () => {
  it("posts one toast carrying an Undo action", () => {
    undoableDraftEdit(REMOVED_CARD, ["a"], () => {});
    const [message, cfg] = lastCall();
    expect(message).toBe("Removed card");
    expect((cfg as { action: { label: string } }).action.label).toBe("Undo");
  });

  it("hands the restore callback the snapshot taken BEFORE the edit", () => {
    // The snapshot is a parameter rather than something `restore` closes over, because closing
    // over it is how a caller silently captures the state AFTER the edit and makes undo a no-op.
    const before = [{ id: "c1" }, { id: "c2" }];
    const restore = vi.fn();
    undoableDraftEdit(REMOVED_CARD, before, restore);
    const [, cfg] = lastCall();
    expect(restore).not.toHaveBeenCalled();          // nothing happens until Undo is pressed
    (cfg as { action: { onClick: () => void } }).action.onClick();
    expect(restore).toHaveBeenCalledTimes(1);
    expect(restore).toHaveBeenCalledWith(before);
  });
});
