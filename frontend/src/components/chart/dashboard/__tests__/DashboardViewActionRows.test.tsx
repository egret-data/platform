import type { ComponentProps } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DashboardViewActionRows } from "../DashboardActionsMenu.view";

// `vi.mock` factories are hoisted above every top-level binding in the file, so the doubles they
// close over must be created by `vi.hoisted` — a plain `const` here throws
// "Cannot access 'x' before initialization" the moment a factory runs.
const { nav, duplicate, trash, setFolder, createFolder, success } = vi.hoisted(() => ({
  nav: vi.fn(),
  duplicate: { mutateAsync: vi.fn().mockResolvedValue({ id: "d2" }) },
  trash: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  setFolder: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  createFolder: { mutateAsync: vi.fn().mockResolvedValue({ id: "f9" }) },
  success: vi.fn(),
}));

vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => nav,
}));

vi.mock("@/api/dashboards", () => ({
  useDashboardMutations: () => ({ duplicate, trash }),
  // The detail DTO has no folder_id (bi.rs:360-372), so the rows read it from the list query.
  useDashboards: () => ({ data: [{ id: "d1", folder_id: "f1" }] }),
}));

vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [{ id: "f1", name: "Ops" }, { id: "f2", name: "Finance" }] }),
  useFolderMutations: () => ({ create: createFolder }),
  useSetFolder: () => setFolder,
}));

vi.mock("sonner", () => ({ toast: { success, error: vi.fn() } }));

function renderRows(over: Partial<ComponentProps<typeof DashboardViewActionRows>> = {}) {
  return render(
    <MemoryRouter>
      <DashboardViewActionRows
        ws="demo" dashboardId="d1" canResetFilters onResetFilters={vi.fn()} {...over}
      />
    </MemoryRouter>,
  );
}

const openMenu = async (user: ReturnType<typeof userEvent.setup>) =>
  user.click(screen.getByRole("button", { name: "dashboard actions" }));

beforeEach(() => { vi.clearAllMocks(); });

describe("DashboardViewActionRows", () => {
  it("offers Metabase's view-mode actions, by role and name", async () => {
    // Metabase DashboardActionMenu.tsx:88-163, minus subscriptions (design §5, no backend),
    // fullscreen (already its own header button) and Edit settings (no sidebar to open).
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    for (const name of [
      "Reset all filters", 'Move to "Ops"', 'Move to "Finance"', "Remove from folder",
      "Move to new folder…", "Duplicate", "Move to trash",
    ]) {
      expect(await screen.findByRole("button", { name })).toBeInTheDocument();
    }
  });

  it("disables the folder the dashboard is already in", async () => {
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    expect(await screen.findByRole("button", { name: 'Move to "Ops"' })).toBeDisabled();
    expect(screen.getByRole("button", { name: 'Move to "Finance"' })).toBeEnabled();
  });

  it("Duplicate duplicates this dashboard and opens the copy", async () => {
    // Identical to the list page (Dashboards.tsx:114) — same mutation, same navigation.
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: "Duplicate" }));
    expect(duplicate.mutateAsync).toHaveBeenCalledWith("d1");
    expect(trash.mutateAsync).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(nav).toHaveBeenCalledWith("/explore/demo/dashboards/d2"));
  });

  it("Move to trash asks first, then trashes and returns to the list (KI-075)", async () => {
    // The list page confirms this too: the same action must not behave two ways depending on
    // where it was invoked from.
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: "Move to trash" }));
    expect(trash.mutateAsync).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    expect(trash.mutateAsync).toHaveBeenCalledWith("d1");
    await vi.waitFor(() => expect(success).toHaveBeenCalledWith("Moved to Trash"));
    expect(nav).toHaveBeenCalledWith("/explore/demo/dashboards");
  });

  it("cancelling the Move to trash confirmation trashes nothing (KI-075)", async () => {
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: "Move to trash" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(trash.mutateAsync).not.toHaveBeenCalled();
  });

  it("moving to another folder sets it and closes the menu", async () => {
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: 'Move to "Finance"' }));
    expect(setFolder.mutateAsync).toHaveBeenCalledWith({ id: "d1", folder_id: "f2" });
    await vi.waitFor(() => expect(screen.queryByRole("button", { name: "Duplicate" })).toBeNull());
  });

  it("offers no reset row when there is nothing to reset", async () => {
    // Metabase hides it on the same condition (getCanResetFilters, selectors.ts:753-756).
    const user = userEvent.setup();
    renderRows({ canResetFilters: false });
    await openMenu(user);
    expect(await screen.findByRole("button", { name: "Duplicate" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset all filters" })).toBeNull();
  });
});
