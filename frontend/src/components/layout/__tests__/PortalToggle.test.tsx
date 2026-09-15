/**
 * PortalToggle tests
 *
 * Tests the Explorer / Governance / Control Plane app switcher rendered inside Header.
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
  platform_roles: ["platform_admin"],
  permissions: [],
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

const withPerms = (permissions: string[]): Me => ({ ...ME_WITH_ROLE, platform_roles: [], permissions });

describe("App switcher – which entries show", () => {
  test("platform_admin sees Explorer, Governance and Control Plane", () => {
    renderHeader({ initialPath: "/explore/my-workspace" });
    expect(screen.getByRole("button", { name: "Explorer" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Governance" })).toHaveAttribute("href", "/governance");
    expect(screen.getByRole("link", { name: "Control Plane" })).toHaveAttribute("href", "/control-plane");
  });

  test("a policy-only user sees Governance but not Control Plane", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["policy:read"]) });
    expect(screen.getByRole("link", { name: "Governance" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Control Plane" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Explorer" })).toBeInTheDocument();
  });

  test("a tag-only user sees Governance", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["tag:read"]) });
    expect(screen.getByRole("link", { name: "Governance" })).toBeInTheDocument();
  });

  test("an infra-only user sees Control Plane but not Governance", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["engine:read"]) });
    expect(screen.getByRole("link", { name: "Control Plane" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Governance" })).not.toBeInTheDocument();
  });

  test("a user with neither app sees no switcher at all", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms([]) });
    expect(screen.queryByRole("link", { name: "Control Plane" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Governance" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Explorer" })).not.toBeInTheDocument();
  });
});

describe("App switcher – active entry follows the URL", () => {
  test.each([
    ["/explore/my-workspace", "Explorer"],
    ["/governance/policies", "Governance"],
    ["/control-plane/users", "Control Plane"],
  ])("on %s only %s is current", (initialPath, active) => {
    renderHeader({ initialPath });
    const entries = {
      Explorer: screen.getByRole("button", { name: "Explorer" }),
      Governance: screen.getByRole("link", { name: "Governance" }),
      "Control Plane": screen.getByRole("link", { name: "Control Plane" }),
    };
    for (const [name, el] of Object.entries(entries)) {
      if (name === active) expect(el).toHaveAttribute("aria-current", "page");
      else expect(el).not.toHaveAttribute("aria-current");
    }
  });

  test("the workspace switcher shows only in Explorer", () => {
    renderHeader({ initialPath: "/governance/policies" });
    expect(screen.queryByLabelText("Workspace")).not.toBeInTheDocument();
  });
});

describe("App switcher – Explorer navigation", () => {
  test("Explorer navigates to the stored workspace from Governance", async () => {
    localStorage.setItem("egret:last-workspace", "my-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/governance/policies" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/explore/my-workspace");
    localStorage.removeItem("egret:last-workspace");
  });

  test("Explorer navigates to the stored workspace from Control Plane", async () => {
    localStorage.setItem("egret:last-workspace", "my-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/control-plane" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/explore/my-workspace");
    localStorage.removeItem("egret:last-workspace");
  });

  test("Explorer falls back to / when no stored workspace", async () => {
    localStorage.removeItem("egret:last-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/control-plane" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/");
  });

  test("visiting /explore/:workspace persists workspace to localStorage", async () => {
    renderHeader({ initialPath: "/explore/sales-team" });
    // useEffect fires after render; give it a tick
    await new Promise((r) => setTimeout(r, 0));
    expect(localStorage.getItem("egret:last-workspace")).toBe("sales-team");
    localStorage.removeItem("egret:last-workspace");
  });
});
