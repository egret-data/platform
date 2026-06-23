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
});
