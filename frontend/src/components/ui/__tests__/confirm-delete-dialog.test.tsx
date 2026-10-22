import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";

function renderDialog(over: Partial<React.ComponentProps<typeof ConfirmDeleteDialog>> = {}) {
  const onConfirm = vi.fn();
  const onOpenChange = vi.fn();
  render(
    <ConfirmDeleteDialog
      open
      onOpenChange={onOpenChange}
      title="Delete PII?"
      description="Gone for good."
      confirmText="PII"
      onConfirm={onConfirm}
      {...over}
    />,
  );
  return { onConfirm, onOpenChange };
}

describe("ConfirmDeleteDialog", () => {
  it("by default asks to type the confirm text, case-insensitively, then confirms", async () => {
    const user = userEvent.setup();
    const { onConfirm } = renderDialog();
    const del = screen.getByRole("button", { name: "Delete" });
    expect(del).toBeDisabled();
    await user.type(screen.getByRole("textbox"), "pii");
    expect(del).toBeEnabled();
    await user.click(del);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("with confirmDisabledReason: no field, Delete aria-disabled with the reason tied, and a click does nothing", async () => {
    const user = userEvent.setup();
    const { onConfirm } = renderDialog({
      confirmDisabledReason: (
        <p>
          Delete or change these rules first: <a href="/r1">Hash email</a>
        </p>
      ),
    });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    const del = screen.getByRole("button", { name: "Delete" });
    expect(del).toHaveAttribute("aria-disabled", "true");
    expect(del).not.toBeDisabled();
    expect(del).toHaveAccessibleDescription("Delete or change these rules first: Hash email");
    expect(screen.getByRole("link", { name: "Hash email" })).toHaveAttribute("href", "/r1");
    await user.click(del);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
