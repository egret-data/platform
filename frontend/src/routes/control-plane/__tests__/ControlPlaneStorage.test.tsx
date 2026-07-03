import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneStorage from "@/routes/control-plane/ControlPlaneStorage";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

const STORAGE: api.Storage = {
  id: "s1",
  name: "prod-lakehouse",
  purpose: "catalog",
  provider: "s3",
  endpoint: null,
  region: "us-east-1",
  bucket: "my-data-lake",
  key_prefix: null,
  access_key: "AKIA",
  path_style: true,
  secret_ref: "sec-1",
  created_at: new Date().toISOString(),
};

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <ControlPlaneStorage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ControlPlaneStorage", () => {
  beforeEach(() => {
    vi.mocked(api.listStorage).mockResolvedValue([STORAGE]);
    vi.mocked(api.listSecrets).mockResolvedValue([]);
  });

  test("renders the storage list", async () => {
    renderPage();
    expect(screen.getByText("Storage")).toBeInTheDocument();
    expect(await screen.findByText("prod-lakehouse")).toBeInTheDocument();
    expect(screen.getByText("my-data-lake")).toBeInTheDocument();
  });

  test("opens the Add dialog", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /Add connection/i }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: /Add storage connection/i }),
      ).toBeInTheDocument(),
    );
  });
});
