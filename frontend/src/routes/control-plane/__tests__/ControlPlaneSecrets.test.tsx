import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneSecrets from "@/routes/control-plane/ControlPlaneSecrets";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

const SECRET: api.Secret = {
  id: "sec-1",
  name: "prod-s3-secret",
  description: "S3 credentials",
  created_at: new Date().toISOString(),
  used_by: 0,
  type: "Opaque",
  keys: ["value"],
  system_managed: false,
};

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <ControlPlaneSecrets />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ControlPlaneSecrets", () => {
  beforeEach(() => {
    vi.mocked(api.listSecrets).mockResolvedValue([SECRET]);
  });

  test("renders the secrets list", async () => {
    renderPage();
    expect(screen.getByText("Secrets")).toBeInTheDocument();
    expect(await screen.findByText("prod-s3-secret")).toBeInTheDocument();
    expect(screen.getByText("S3 credentials")).toBeInTheDocument();
  });

  test("opens the Add dialog", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /Add secret/i }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: /Add secret/i })).toBeInTheDocument(),
    );
  });

  test("marks a system secret and does not offer to delete it (KI-207)", async () => {
    vi.mocked(api.listSecrets).mockResolvedValue([
      SECRET,
      {
        ...SECRET,
        id: "sys-1",
        name: "engine-internal-e1-g1",
        description: "Trino internal-communication shared secret",
        system_managed: true,
      },
    ]);
    renderPage();
    await screen.findByText("engine-internal-e1-g1");
    expect(screen.getAllByText("System")).toHaveLength(1);
    const sysDelete = screen.getByRole("button", { name: "Delete engine-internal-e1-g1" });
    expect(sysDelete).toBeDisabled();
    expect(sysDelete).toHaveAttribute("title", expect.stringMatching(/Managed by egret/));
    expect(screen.getByRole("button", { name: "Delete prod-s3-secret" })).toBeEnabled();
  });
});
