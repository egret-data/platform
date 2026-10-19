import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * The People area through the REAL route table (as
 * `DataGovernanceEmptyPages.test.tsx` does): `/governance/people` is gated
 * `policy:read` (spec P2), mounted lazily, and its sidebar item sits directly
 * after Data.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[]): Me => ({ user_id: "u1", email: "u1@example.com", platform_roles: [], permissions });

function openApp(url: string, as: Me) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL(String(input), "http://localhost").pathname;
      const body =
        path === "/api/auth/me" ? as : path === "/api/control-plane/perm/people" ? { items: [], total: 0 } : null;
      return body === null
        ? Promise.reject(new Error("no network in test"))
        : Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }));
    }),
  );
  window.history.pushState({}, "", url);
  render(<App />);
}

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("the People area's route", () => {
  it("renders the index for a policy:read user, with People & groups after Overview and Data in the sidebar", async () => {
    openApp("/governance/people", me(["policy:read"]));
    expect(await screen.findByRole("heading", { level: 1, name: "People & groups" }, TIMEOUT)).toBeInTheDocument();
    expect(await screen.findByText("No users.", {}, TIMEOUT)).toBeInTheDocument();
    const labels = within(screen.getByTestId("app-sidebar"))
      .getAllByRole("link")
      .map((a) => a.textContent);
    // DG #3 (O11): the Overview leads the Governance sidebar.
    expect(labels.slice(0, 3)).toEqual(["Overview", "Data", "People & groups"]);
    expect(screen.getByRole("link", { name: "People & groups" })).toHaveAttribute("href", "/governance/people");
  }, 15_000);

  it("refuses a user without policy:read, a tag:read steward included", async () => {
    openApp("/governance/people", me(["tag:read"]));
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "People & groups" })).not.toBeInTheDocument();
  }, 15_000);
});
