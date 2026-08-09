import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AppShell } from "../AppShell";

// AppShell mounts Header and Sidebar, and both reach for TanStack Query (workspace
// entitlements, notification counts). The shell cannot render without a client.
const renderShell = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <AppShell>
          <div />
        </AppShell>
      </MemoryRouter>
    </QueryClientProvider>,
  );

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

beforeEach(() => {
  localStorage.clear();
});

describe("AppShell sidebar on a narrow viewport", () => {
  it("collapses the sidebar below the breakpoint even when localStorage says expanded", () => {
    // The stored preference is a DESKTOP preference. Honouring it at 390px is what leaves the
    // dashboard canvas 86px wide.
    localStorage.setItem("egret:sidebar-collapsed", "0");
    stubMatchMedia(true);
    renderShell();
    expect(screen.getByTestId("app-sidebar").getAttribute("data-collapsed")).toBe("true");
  });

  it("honours the stored preference above the breakpoint", () => {
    localStorage.setItem("egret:sidebar-collapsed", "0");
    stubMatchMedia(false);
    renderShell();
    expect(screen.getByTestId("app-sidebar").getAttribute("data-collapsed")).toBe("false");
  });

  it("still collapses to the icon rail width, not to nothing", () => {
    // `collapsed` is what drives the rail's width class. A narrow viewport must land on the
    // 64px rail — the same state the toggle produces — rather than some third rendering.
    localStorage.setItem("egret:sidebar-collapsed", "0");
    stubMatchMedia(true);
    renderShell();
    expect(screen.getByTestId("app-sidebar").className).toContain("w-16");
  });
});
