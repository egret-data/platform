import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import Policies from "../Policies";

vi.mock("@/api/policies", () => ({
  listProfiles: () =>
    Promise.resolve([
      {
        id: 1,
        name: "trino",
        type: "hive",
        policy_version: 3,
        engines: [{ id: "e1", name: "prod", status: "running" }],
      },
      {
        id: 2,
        name: "starrocks",
        type: "olap",
        policy_version: 1,
        engines: [],
      },
    ]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Policies page (profiles landing)", () => {
  it("renders each profile as a link to its detail page", async () => {
    wrap(<Policies />);
    const link = await screen.findByRole("link", { name: "trino" });
    expect(link).toHaveAttribute("href", "/control-plane/policies/trino");
  });

  it("renders the profile type badge", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("hive")).toBeInTheDocument();
    expect(screen.getByText("olap")).toBeInTheDocument();
  });

  it("renders bound engine names", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("prod")).toBeInTheDocument();
  });

  it("renders an em dash for a profile with no bound engines", async () => {
    wrap(<Policies />);
    const link = await screen.findByRole("link", { name: "starrocks" });
    const row = link.closest("tr");
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("—");
  });

  it("renders the policy version", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("v3")).toBeInTheDocument();
  });
});
