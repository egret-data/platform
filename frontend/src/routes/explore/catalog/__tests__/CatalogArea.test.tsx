import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// Stub child routes so they render something identifiable without their real deps.
vi.mock("../CatalogBrowse", () => ({
  default: () => <div>Browse content</div>,
}));

vi.mock("../CatalogAccess", () => ({
  default: () => <div>Access content</div>,
}));

import CatalogArea from "../CatalogArea";
import CatalogBrowse from "../CatalogBrowse";
import CatalogAccess from "../CatalogAccess";

function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/explore/:workspace/catalog" element={<CatalogArea />}>
            <Route index element={<CatalogBrowse />} />
            <Route path="access" element={<CatalogAccess />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("CatalogArea", () => {
  it("renders the page heading and both sub-nav links", () => {
    renderAt("/explore/w1/catalog");
    expect(screen.getByRole("heading", { level: 1, name: "Catalog" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Browse" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Access" })).toBeInTheDocument();
  });

  it("shows Browse content on the index route", () => {
    renderAt("/explore/w1/catalog");
    expect(screen.getByText("Browse content")).toBeInTheDocument();
  });

  it("shows Access content on /catalog/access", () => {
    renderAt("/explore/w1/catalog/access");
    expect(screen.getByText("Access content")).toBeInTheDocument();
  });

  it("navigates to Access when the Access link is clicked", async () => {
    const user = userEvent.setup();
    renderAt("/explore/w1/catalog");

    // Initially showing Browse content.
    expect(screen.getByText("Browse content")).toBeInTheDocument();

    // Click Access nav link.
    await user.click(screen.getByRole("link", { name: "Access" }));

    // Now showing Access content.
    expect(screen.getByText("Access content")).toBeInTheDocument();
  });
});
