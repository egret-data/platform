/**
 * SidebarCollapse tests
 *
 * Tests collapsed / expanded visual states of Sidebar.
 *
 * Sidebar requires:
 *   - MemoryRouter (uses useLocation + useParams)
 *   - TooltipProvider (renders Radix Tooltip in collapsed mode)
 *   - QueryClientProvider (useWorkspaceFeatures reads useMyWorkspaces)
 *   - collapsed: boolean + onToggle: () => void props
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, test, vi } from "vitest";
import { Sidebar } from "@/components/layout/Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";

afterEach(() => vi.restoreAllMocks());

function renderSidebar(collapsed: boolean, onToggle = vi.fn()) {
  // /me/workspaces never resolves here — useWorkspaceFeatures sees it as still
  // loading and returns every key, matching this suite's pre-entitlements
  // expectations (every item visible).
  vi.spyOn(globalThis, "fetch").mockImplementation(() => new Promise(() => {}));
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/ws"]}>
          <Sidebar collapsed={collapsed} onToggle={onToggle} />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("Sidebar collapse", () => {
  test("expanded: shows nav label text", () => {
    renderSidebar(false);
    // "SQL Lab" must be visible as a text node inside a NavLink
    expect(screen.getByText("SQL Lab")).toBeInTheDocument();
  });

  test("collapsed: hides nav label text", () => {
    renderSidebar(true);
    // Text nodes are not rendered; label only lives in TooltipContent (portal, not in DOM by default)
    expect(screen.queryByText("SQL Lab")).not.toBeInTheDocument();
  });

  test("toggle button is present in both states", () => {
    const { rerender } = renderSidebar(false);
    expect(
      screen.getByRole("button", { name: "Toggle sidebar" }),
    ).toBeInTheDocument();

    rerender(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/explore/ws"]}>
            <Sidebar collapsed={true} onToggle={vi.fn()} />
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>,
    );
    expect(
      screen.getByRole("button", { name: "Toggle sidebar" }),
    ).toBeInTheDocument();
  });

  test("clicking toggle button calls onToggle", async () => {
    const onToggle = vi.fn();
    const user = userEvent.setup();
    renderSidebar(false, onToggle);

    await user.click(screen.getByRole("button", { name: "Toggle sidebar" }));

    expect(onToggle).toHaveBeenCalledOnce();
  });
});
