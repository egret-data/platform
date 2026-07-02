import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneRbac from "@/routes/control-plane/ControlPlaneRbac";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ControlPlaneRbac />
    </QueryClientProvider>,
  );
}

describe("ControlPlaneRbac", () => {
  beforeEach(() => {
    vi.mocked(api.listPrincipals).mockResolvedValue([{ name: "egret-ws-x" }]);
    vi.mocked(api.listPrincipalRoles).mockResolvedValue([]);
    vi.mocked(api.listCatalogs).mockResolvedValue([]);
    vi.mocked(api.listCatalogRoles).mockResolvedValue([]);
  });

  test("renders a principal row", async () => {
    renderPage();
    expect(await screen.findByText("egret-ws-x")).toBeInTheDocument();
  });

  test("creating a principal shows its one-time credentials", async () => {
    vi.mocked(api.createPrincipal).mockResolvedValue({
      name: "svc-new",
      clientId: "client-abc",
      clientSecret: "secret-xyz",
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /Add principal/i }));
    const dialog = await screen.findByRole("dialog");
    const nameInput = within(dialog).getByLabelText("Name");
    await user.type(nameInput, "svc-new");
    await user.click(
      within(dialog).getByRole("button", { name: /^Add principal$/i }),
    );

    await waitFor(() =>
      expect(api.createPrincipal).toHaveBeenCalledWith("svc-new"),
    );
    expect(await screen.findByText("client-abc")).toBeInTheDocument();
    expect(screen.getByText("secret-xyz")).toBeInTheDocument();
  });
});
