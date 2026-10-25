import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const useAppVersionsMock = vi.fn();
const restoreMutateAsync = vi.fn();

vi.mock("@/api/apps", () => ({
  useAppVersions: (...a: unknown[]) => useAppVersionsMock(...a),
  useRestoreVersion: (...a: unknown[]) => {
    useRestoreVersionCalledWith.push(a);
    return { mutateAsync: restoreMutateAsync, isPending: false };
  },
}));
const useRestoreVersionCalledWith: unknown[][] = [];

vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { toast } from "sonner";
import VersionsDialog from "@/components/apps/VersionsDialog";

const v = (over: Record<string, unknown>) => ({
  id: "v1",
  version_no: 1,
  published_at: "2026-09-01T00:00:00Z",
  published_by: "11111111-1111-1111-1111-111111111111",
  from_revision: 4,
  restored_from: null,
  ...over,
});

function renderDialog(open = true) {
  return render(<VersionsDialog ws="acme" appId="a1" open={open} onOpenChange={() => {}} />);
}

beforeEach(() => {
  useAppVersionsMock.mockReset();
  restoreMutateAsync.mockReset();
  useRestoreVersionCalledWith.length = 0;
  (toast.success as ReturnType<typeof vi.fn>).mockReset?.();
});

describe("VersionsDialog", () => {
  it("lists versions newest first, as the backend already orders them", () => {
    useAppVersionsMock.mockReturnValue({
      data: [v({ id: "v2", version_no: 2 }), v({ id: "v1", version_no: 1 })],
    });
    renderDialog();
    const rows = screen.getAllByText(/^v\d+$/);
    expect(rows.map((r) => r.textContent)).toEqual(["v2", "v1"]);
  });

  it("asks for useAppVersions/useRestoreVersion scoped to this workspace and app", () => {
    useAppVersionsMock.mockReturnValue({ data: [] });
    renderDialog();
    expect(useAppVersionsMock).toHaveBeenCalledWith("acme", "a1");
    expect(useRestoreVersionCalledWith[0]).toEqual(["acme", "a1"]);
  });

  it("restoring asks for confirmation that frames it as creating a NEW version, not a rollback", async () => {
    useAppVersionsMock.mockReturnValue({ data: [v({ id: "v1", version_no: 1 })] });
    restoreMutateAsync.mockResolvedValue({ version_no: 2 });
    renderDialog();
    fireEvent.click(screen.getByRole("button", { name: /Restore/ }));
    const dialogText = screen.getByRole("alertdialog").textContent ?? "";
    expect(dialogText.toLowerCase()).toContain("new");
    fireEvent.click(screen.getByRole("button", { name: "Restore this version" }));
    await waitFor(() => expect(restoreMutateAsync).toHaveBeenCalledWith({ versionId: "v1" }));
  });

  it("names the publisher, and the shortened id only when the server has no name (KI-027)", () => {
    useAppVersionsMock.mockReturnValue({
      data: [
        v({ id: "v2", version_no: 2, published_by_name: "admin@egret.local" }),
        v({ id: "v1", version_no: 1, published_by_name: null }),
      ],
    });
    renderDialog();
    expect(screen.getByText(/by admin@egret\.local/)).toBeTruthy();
    expect(screen.getByText(/by 11111111/)).toBeTruthy();
  });

  it("names the version a restore republished (KI-184)", () => {
    useAppVersionsMock.mockReturnValue({
      data: [
        v({ id: "v3", version_no: 3, from_revision: null, restored_from: "v1" }),
        v({ id: "v2", version_no: 2 }),
        v({ id: "v1", version_no: 1 }),
      ],
    });
    renderDialog();
    expect(screen.getByText(/restored from v1/)).toBeTruthy();
    expect(screen.queryByText(/an earlier version/)).toBeNull();
  });

  it("shows a toast naming the new version number on success", async () => {
    useAppVersionsMock.mockReturnValue({ data: [v({ id: "v1", version_no: 1 })] });
    restoreMutateAsync.mockResolvedValue({ version_no: 5 });
    renderDialog();
    fireEvent.click(screen.getByRole("button", { name: /Restore/ }));
    fireEvent.click(screen.getByRole("button", { name: "Restore this version" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Published version 5"));
  });
});
