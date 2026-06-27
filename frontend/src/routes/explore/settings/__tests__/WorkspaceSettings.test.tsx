import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import WorkspaceSettings from "../WorkspaceSettings";
import CatalogAccess from "../CatalogAccess";
import StorageSettings from "../StorageSettings";

function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/explore/:workspace/settings" element={<WorkspaceSettings />}>
            <Route path="catalog" element={<CatalogAccess />} />
            <Route path="storage" element={<StorageSettings />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("WorkspaceSettings shell", () => {
  it("renders the page heading and both sub-nav links", () => {
    renderAt("/explore/w1/settings/catalog");
    expect(screen.getByText("Workspace settings")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Catalog access" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Storage" })).toBeInTheDocument();
  });

  it("shows Catalog access section on /settings/catalog", () => {
    renderAt("/explore/w1/settings/catalog");
    // The h2 heading inside the CatalogAccess stub
    expect(screen.getByRole("heading", { level: 2, name: "Catalog access" })).toBeInTheDocument();
  });

  it("shows Storage section on /settings/storage", () => {
    renderAt("/explore/w1/settings/storage");
    // The h2 heading inside the StorageSettings stub
    expect(screen.getByRole("heading", { level: 2, name: "Storage" })).toBeInTheDocument();
  });

  it("navigates to Storage section when Storage link is clicked", async () => {
    const user = userEvent.setup();
    renderAt("/explore/w1/settings/catalog");

    // Initially showing Catalog access content (h2 inside the stub)
    expect(screen.getByRole("heading", { level: 2, name: "Catalog access" })).toBeInTheDocument();

    // Click Storage nav link
    await user.click(screen.getByRole("link", { name: "Storage" }));

    // Now showing Storage h2 inside stub
    expect(screen.getByRole("heading", { level: 2, name: "Storage" })).toBeInTheDocument();
  });
});
