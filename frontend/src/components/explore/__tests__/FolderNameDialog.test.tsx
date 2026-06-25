import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FolderNameDialog } from "../FolderNameDialog";

describe("FolderNameDialog", () => {
  it("submits the trimmed name and closes", async () => {
    const onSubmit = vi.fn();
    const onOpenChange = vi.fn();
    const user = userEvent.setup();
    render(
      <FolderNameDialog
        open
        title="New folder"
        submitLabel="Create"
        onOpenChange={onOpenChange}
        onSubmit={onSubmit}
      />,
    );
    await user.type(screen.getByPlaceholderText("Folder name"), "  Reports  ");
    await user.click(screen.getByRole("button", { name: "Create" }));
    expect(onSubmit).toHaveBeenCalledWith("Reports");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("disables submit when the name is empty", () => {
    render(
      <FolderNameDialog open title="New folder" onOpenChange={vi.fn()} onSubmit={vi.fn()} />,
    );
    expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
  });
});
