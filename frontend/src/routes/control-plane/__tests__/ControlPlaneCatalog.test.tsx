import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneCatalog from "@/routes/control-plane/ControlPlaneCatalog";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ControlPlaneCatalog />
    </QueryClientProvider>,
  );
}

describe("ControlPlaneCatalog", () => {
  beforeEach(() => {
    vi.mocked(api.listCatalogs).mockResolvedValue([]);
    vi.mocked(api.listStorage).mockResolvedValue([]);
    vi.mocked(api.polarisHealth).mockResolvedValue({ ok: true });
  });

  test("shows the Connect Polaris prompt when no connection is set", async () => {
    vi.mocked(api.getPolarisConnection).mockResolvedValue(null);
    renderPage();
    expect(
      await screen.findByRole("button", { name: /Connect Polaris/i }),
    ).toBeInTheDocument();
  });

  test("renders a catalog row when a connection is set", async () => {
    vi.mocked(api.getPolarisConnection).mockResolvedValue({
      baseUrl: "https://polaris.corp.local/api/catalog",
      kcTokenUrl: "https://kc.corp.local/token",
      kcClientId: "polaris-admin",
    });
    vi.mocked(api.listCatalogs).mockResolvedValue([
      { name: "lakehouse", catalogType: "INTERNAL" },
    ]);
    renderPage();
    expect(await screen.findByText("lakehouse")).toBeInTheDocument();
    expect(screen.getByText("INTERNAL")).toBeInTheDocument();
  });
});
