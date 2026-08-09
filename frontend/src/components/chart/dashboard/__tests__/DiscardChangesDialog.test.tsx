import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DiscardChangesDialog } from "../DiscardChangesDialog";

describe("DiscardChangesDialog", () => {
  it("carries Metabase's title, message and button labels", () => {
    // Ported from LeaveConfirmModal.tsx:12-22. Asserting the strings here rather than at the route
    // keeps the route test about BEHAVIOUR, so a wording change breaks one test, not five.
    render(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={vi.fn()} />);
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    expect(screen.getByText("Discard your changes?")).toBeTruthy();
    expect(screen.getByText("Your changes haven't been saved, so you'll lose them if you navigate away.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Discard changes" })).toBeTruthy();
  });

  it("Discard changes calls onDiscard; Cancel does not", async () => {
    const user = userEvent.setup();
    const onDiscard = vi.fn();
    const { rerender } = render(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={onDiscard} />);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onDiscard).not.toHaveBeenCalled();
    rerender(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={onDiscard} />);
    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(onDiscard).toHaveBeenCalledTimes(1);
  });

  it("renders nothing when closed", () => {
    render(<DiscardChangesDialog open={false} onOpenChange={vi.fn()} onDiscard={vi.fn()} />);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
  });
});
