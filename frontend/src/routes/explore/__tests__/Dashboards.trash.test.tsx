import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";

const apiJson = vi.fn();
const apiFetch = vi.fn().mockResolvedValue({});
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: (...a: unknown[]) => apiFetch(...a) }));
vi.mock("react-router-dom", async (orig) => ({ ...(await orig() as object), useParams: () => ({ workspace: "eng" }) }));

import Dashboards from "../Dashboards";

const wrap = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, createElement(MemoryRouter, null, children));

describe("Dashboards trash", () => {
  beforeEach(() => { apiJson.mockReset(); apiFetch.mockReset(); apiFetch.mockResolvedValue({}); });
  it("row Delete moves to trash (POST /trash), not hard delete", async () => {
    apiJson.mockResolvedValue([{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }]);
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    fireEvent.click(screen.getByRole("button", { name: "move One to trash" }));
    await waitFor(() => expect(apiJson).toHaveBeenCalledWith("/ws/eng/dashboards/d1/trash", expect.objectContaining({ method: "POST" })));
  });

  it("permanent delete uses ConfirmDeleteDialog and routes to hard DELETE (apiFetch), not trash/restore", async () => {
    apiJson.mockResolvedValue([{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }]);
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    // Switch to trash view so the permanent-delete row renders.
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    const delBtn = await screen.findByRole("button", { name: "delete One permanently" });
    fireEvent.click(delBtn);
    // Dialog opens with the type-to-confirm prompt.
    const input = await screen.findByPlaceholderText("delete");
    // Type the required confirm text, then click the dialog's Delete button.
    fireEvent.change(input, { target: { value: "delete" } });
    const confirmBtn = screen.getByRole("button", { name: /^Delete$/ });
    fireEvent.click(confirmBtn);
    await waitFor(() => expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/d1", expect.objectContaining({ method: "DELETE" })));
    // It must NOT have gone through the trash/restore (apiJson) path for this action.
    const jsonPaths = apiJson.mock.calls.map((c) => c[0] as string);
    expect(jsonPaths.some((p) => p.includes("/trash") || p.includes("/restore"))).toBe(false);
  });
});
