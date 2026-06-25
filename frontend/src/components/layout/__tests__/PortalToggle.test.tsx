/**
 * PortalToggle tests
 *
 * Tests the two-way Admin↔Explorer toggle rendered inside Header.
 *
 * Header depends on:
 *   - AuthContext (me / platform_roles)
 *   - ThemeProvider (theme / setTheme)
 *   - react-router-dom (location, params, navigate)
 *   - QueryClient (for useLogout mutation and WorkspaceSwitcher's useMyWorkspaces)
 *   - fetch (mocked – WorkspaceSwitcher calls /me/workspaces)
 *
 * Polyfills for Radix Select (used by WorkspaceSwitcher) are included so the
 * full Header can be rendered without jsdom errors.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { Header } from "@/components/layout/Header";
import { AuthContext } from "@/auth/useAuth";
import { ThemeProvider } from "@/components/theme-provider";
import type { Me } from "@/api/hooks";

// ---------------------------------------------------------------------------
// Polyfills for Radix UI / jsdom incompatibilities
// ---------------------------------------------------------------------------
beforeEach(() => {
  // Mock window.matchMedia used by ThemeProvider / Header
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
});

afterEach(() => vi.restoreAllMocks());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ME_WITH_ROLE: Me = {
  user_id: "u1",
  email: "admin@example.com",
  platform_roles: ["admin"],
};

/** LocationProbe lets us assert navigate() side-effects */
function LocationProbe() {
  return <span data-testid="loc">{useLocation().pathname}</span>;
}

interface WrapperOptions {
  initialPath: string;
  me?: Me | null;
}

function renderHeader({ initialPath, me = ME_WITH_ROLE }: WrapperOptions) {
  // Mock fetch so WorkspaceSwitcher's /me/workspaces query resolves empty
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify([]), { status: 200 }),
  );

  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: me ?? null, isLoading: false }}>
        <ThemeProvider defaultTheme="light">
          <MemoryRouter initialEntries={[initialPath]}>
            {/* Header must be under a route that exposes :workspace param */}
            <Routes>
              <Route path="/explore/:workspace/*" element={<Header />} />
              <Route path="*" element={<Header />} />
            </Routes>
            <Routes>
              <Route path="*" element={<LocationProbe />} />
            </Routes>
          </MemoryRouter>
        </ThemeProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("PortalToggle – Admin portal", () => {
  test("shows Explorer button when under /admin", () => {
    renderHeader({ initialPath: "/admin" });
    expect(screen.getByRole("button", { name: /explorer/i })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /admin/i })).not.toBeInTheDocument();
  });

  test("Explorer button navigates to stored workspace", async () => {
    localStorage.setItem("egret:last-workspace", "my-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/admin" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/explore/my-workspace");
    localStorage.removeItem("egret:last-workspace");
  });

  test("Explorer button falls back to / when no stored workspace", async () => {
    localStorage.removeItem("egret:last-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/admin" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/");
  });
});

describe("PortalToggle – Explorer portal", () => {
  test("shows Admin link when under /explore/:workspace and user has a platform role", () => {
    renderHeader({ initialPath: "/explore/my-workspace" });
    expect(screen.getByRole("link", { name: /admin/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /explorer/i })).not.toBeInTheDocument();
  });

  test("does NOT show Admin link when user has no platform roles", () => {
    renderHeader({
      initialPath: "/explore/my-workspace",
      me: { ...ME_WITH_ROLE, platform_roles: [] },
    });
    expect(screen.queryByRole("link", { name: /admin/i })).not.toBeInTheDocument();
  });

  test("visiting /explore/:workspace persists workspace to localStorage", async () => {
    renderHeader({ initialPath: "/explore/sales-team" });
    // useEffect fires after render; give it a tick
    await new Promise((r) => setTimeout(r, 0));
    expect(localStorage.getItem("egret:last-workspace")).toBe("sales-team");
    localStorage.removeItem("egret:last-workspace");
  });
});
