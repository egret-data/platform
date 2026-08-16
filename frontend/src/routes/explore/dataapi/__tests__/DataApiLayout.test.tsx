import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DataApiLayout from "../DataApiLayout";

// DataApiLayout reads the workspace name via useMyWorkspaces (react-query),
// same as WorkspaceSettings' header — so it needs a QueryClient in scope,
// exactly like WorkspaceSettings.test.tsx's renderAt wraps its own subject.
function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/explore/:ws/data-api" element={<DataApiLayout />}>
            <Route path="endpoints" element={<p>endpoints pane</p>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

it("marks the active section and renders the outlet", () => {
  renderAt("/explore/analytics/data-api/endpoints");
  // The shell renders a desktop nav AND a mobile strip; both are in the DOM
  // under JSDOM because there is no CSS, so every link appears twice.
  const links = screen.getAllByRole("link", { name: "Endpoints" });
  expect(links.length).toBeGreaterThan(1);
  expect(links.some((l) => l.getAttribute("aria-current") === "page")).toBe(true);
  expect(screen.getByText("endpoints pane")).toBeInTheDocument();
});
