import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import { createElement, type ReactNode } from "react";

// The dashboards module is migrated to the generated client, so every call
// (list read, trash, hard delete) flows through `apiFetch` → the mutator.
const apiFetch = vi.fn();
const listData = { current: [] as unknown[] };
const jsonRes = (body: unknown) => ({ status: 200, headers: new Headers({ "content-type": "application/json" }), json: () => Promise.resolve(body) });
vi.mock("@/api/client", () => ({
  apiJson: vi.fn(),
  apiFetch: (...a: unknown[]) => apiFetch(...a),
  ApiError: class ApiError extends Error {},
  UnauthorizedError: class UnauthorizedError extends Error {},
}));
vi.mock("react-router-dom", async (orig) => ({ ...(await orig() as object), useParams: () => ({ workspace: "eng" }) }));
const folderRemove = vi.fn().mockResolvedValue(undefined);
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [{ id: "f1", name: "F1" }, { id: "f2", name: "F2" }] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: (...a: unknown[]) => folderRemove(...a) },
  }),
}));

import Dashboards from "../Dashboards";

const wrap = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, createElement(TooltipProvider, null, createElement(MemoryRouter, null, children)));

describe("Dashboards trash", () => {
  beforeEach(() => {
    apiFetch.mockReset();
    listData.current = [];
    apiFetch.mockImplementation((url: string) =>
      Promise.resolve(jsonRes(/\/dashboards(\?|$)/.test(url) ? listData.current : null)),
    );
  });
  it("row Move to Trash moves to trash (POST /trash), not hard delete", async () => {
    listData.current = [{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }];
    const user = userEvent.setup();
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    // Row actions now live inside the kebab menu.
    await user.click(screen.getByRole("button", { name: "Actions" }));
    await user.click(await screen.findByText("Move to Trash"));
    // KI-075: asks first.
    expect(apiFetch.mock.calls.some((c) => String(c[0]).includes("/trash"))).toBe(false);
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));
    await waitFor(() => expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/d1/trash", expect.objectContaining({ method: "POST" })));
  });

  it("permanent delete uses ConfirmDeleteDialog and routes to hard DELETE, not trash/restore", async () => {
    listData.current = [{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }];
    const user = userEvent.setup();
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    // Switch to trash view so the permanent-delete row renders.
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    // Permanent delete now lives inside the kebab menu.
    await user.click(await screen.findByRole("button", { name: "Actions" }));
    await user.click(await screen.findByText("Delete permanently"));
    // Dialog opens with the type-to-confirm prompt.
    const input = await screen.findByPlaceholderText("delete");
    // Type the required confirm text, then click the dialog's Delete button.
    fireEvent.change(input, { target: { value: "delete" } });
    const confirmBtn = screen.getByRole("button", { name: /^Delete$/ });
    fireEvent.click(confirmBtn);
    await waitFor(() => expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/d1", expect.objectContaining({ method: "DELETE" })));
    // It must NOT have gone through the trash/restore path for this action.
    const paths = apiFetch.mock.calls.map((c) => c[0] as string);
    expect(paths.some((p) => p.includes("/trash") || p.includes("/restore"))).toBe(false);
  });

  // KI-074: the Trash is a view of what was thrown away; nothing is created there and it shows no
  // folders, so neither the create button nor a folder count belongs on it.
  it("the Trash view offers no + Dashboard and counts no folders", async () => {
    listData.current = [{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }];
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    expect(screen.getByRole("button", { name: "+ Dashboard" })).toBeTruthy();
    expect(screen.getByText("folders")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByRole("heading", { name: "Trash" });
    await screen.findByText("One");
    expect(screen.queryByRole("button", { name: "+ Dashboard" })).toBeNull();
    expect(screen.queryByText("folders")).toBeNull();
    expect(screen.queryByText("folder")).toBeNull();
  });

  // KI-075: deleting a folder asks first, and says what happens to the dashboards in it.
  it("folder Delete asks first, names where its dashboards go, and deletes only on confirm", async () => {
    listData.current = [
      { id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), folder_id: "f1" },
      { id: "d2", name: "Two", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), folder_id: "f1" },
    ];
    folderRemove.mockClear();
    const user = userEvent.setup();
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    await user.click(screen.getByRole("button", { name: "Folder F1 actions" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(folderRemove).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("The 2 dashboards in it move to Uncategorized");
    await user.click(within(dialog).getByRole("button", { name: "Delete folder" }));
    await waitFor(() => expect(folderRemove).toHaveBeenCalledWith("f1"));
  });
});
