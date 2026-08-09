import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Header } from "../Header";
import { AuthContext } from "@/auth/useAuth";

// jsdom has no layout engine, so `scrollWidth` is 0 for everything and the real acceptance
// check — document scrollWidth vs innerWidth at 390px — cannot live here. It lives in
// tests/e2e/narrow-viewport-kind.spec.ts, against a real browser. What is pinned here is the
// mechanism that produces it: which fixed-width children the header stops rendering when the
// viewport is narrow.
function stubMatchMedia(matches: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }));
}

const me = {
  user_id: "u1",
  email: "admin@egret.local",
  platform_roles: ["platform_admin"],
  permissions: ["workspace:read"],
};

const renderHeader = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter initialEntries={["/explore/demo/dashboards/d1"]}>
          <Header />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );

beforeEach(() => {
  localStorage.clear();
});

describe("Header on a narrow viewport", () => {
  it("drops the wordmark and the Control Plane label, keeping both reachable", () => {
    stubMatchMedia(true);
    renderHeader();
    // The wordmark is the only element in the row with no function of its own, so it is the
    // one thing that may disappear outright.
    expect(screen.queryByText("Egret")).toBeNull();
    // The Control Plane entry loses its visible text but keeps its accessible name — a
    // control that is merely narrower, not a control that is gone.
    expect(screen.queryByText("Control Plane")).toBeNull();
    expect(screen.getByRole("link", { name: "Control Plane" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle theme" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "User menu" })).toBeInTheDocument();
  });

  it("lets the workspace switcher shrink instead of holding a fixed 224px", () => {
    // `w-56` cannot shrink; it is what pushes the theme toggle and the avatar past the right
    // edge of a 390px viewport.
    stubMatchMedia(true);
    renderHeader();
    const trigger = screen.getByLabelText("Workspace");
    expect(trigger.className).not.toContain("w-56");
    expect(trigger.className).toContain("min-w-0");
  });

  it("keeps the desktop row exactly as it was", () => {
    stubMatchMedia(false);
    renderHeader();
    expect(screen.getByText("Egret")).toBeInTheDocument();
    expect(screen.getByText("Control Plane")).toBeInTheDocument();
    expect(screen.getByLabelText("Workspace").className).toContain("w-56");
  });
});
