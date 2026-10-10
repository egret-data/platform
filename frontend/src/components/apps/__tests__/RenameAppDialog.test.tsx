import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const updateMutateAsync = vi.fn();
const useAppMutationsCalledWith: unknown[][] = [];

vi.mock("@/api/apps", () => ({
  useAppMutations: (...a: unknown[]) => {
    useAppMutationsCalledWith.push(a);
    return { update: { mutateAsync: updateMutateAsync, isPending: false } };
  },
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { toast } from "sonner";
import { RenameAppDialog } from "@/components/apps/RenameAppDialog";

const APP = { id: "a1", name: "Revenue", description: "Old words" };

function renderDialog(onOpenChange = vi.fn()) {
  render(<RenameAppDialog ws="acme" app={APP} open onOpenChange={onOpenChange} />);
  return onOpenChange;
}

beforeEach(() => {
  updateMutateAsync.mockReset();
  useAppMutationsCalledWith.length = 0;
  (toast.error as ReturnType<typeof vi.fn>).mockReset();
});

describe("RenameAppDialog", () => {
  it("starts from the app's current name and description, scoped to this workspace", () => {
    renderDialog();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Revenue");
    expect((screen.getByLabelText("Description") as HTMLTextAreaElement).value).toBe("Old words");
    expect(useAppMutationsCalledWith[0]).toEqual(["acme"]);
  });

  it("saves the trimmed name and description, then closes", async () => {
    // Arrange
    updateMutateAsync.mockResolvedValue({ id: "a1" });
    const onOpenChange = renderDialog();

    // Act
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "  Revenue v2  " } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "  New words " } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    // Assert
    await waitFor(() =>
      expect(updateMutateAsync).toHaveBeenCalledWith({ id: "a1", name: "Revenue v2", description: "New words" }),
    );
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("sends an emptied description as an empty string, so it can be cleared", async () => {
    updateMutateAsync.mockResolvedValue({ id: "a1" });
    renderDialog();
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(updateMutateAsync).toHaveBeenCalledWith({ id: "a1", name: "Revenue", description: "" }),
    );
  });

  it("will not save a blank name", () => {
    renderDialog();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "   " } });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.submit(screen.getByLabelText("Name").closest("form")!);
    expect(updateMutateAsync).not.toHaveBeenCalled();
  });

  it("shows the server's message and stays open when the save is refused", async () => {
    updateMutateAsync.mockRejectedValue(new Error("forbidden: app:write"));
    const onOpenChange = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("forbidden: app:write"));
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});
