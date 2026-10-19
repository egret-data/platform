import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { queryClient } from "@/api/queryClient";
import { currentUrl, openApp, POLICY_READER, TAG_ONLY, TIMEOUT } from "./appHarness";

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

/** Spec §2: the area's URLs, mounted lazily under `policy:read`. */
describe("the Engine & policy area's routes", () => {
  it("opens the index at /governance/engines", async () => {
    openApp("/governance/engines", POLICY_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "Engine & policy" }, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/engines");
  }, 15_000);

  it("opens an engine page at /governance/engines/:id", async () => {
    openApp("/governance/engines/7f1c0e8a-0000-4000-8000-000000000001?tab=plugin", POLICY_READER);
    expect(await screen.findByTestId("engine-page-stub", {}, TIMEOUT)).toHaveTextContent(
      "7f1c0e8a-0000-4000-8000-000000000001",
    );
    expect(currentUrl()).toBe("/governance/engines/7f1c0e8a-0000-4000-8000-000000000001?tab=plugin");
  }, 15_000);

  it("opens the profile page at /governance/engines/profiles/:service: the static segment wins over :id", async () => {
    openApp("/governance/engines/profiles/p1", POLICY_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "p1" }, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByTestId("engine-page-stub")).not.toBeInTheDocument();
  }, 15_000);

  it("keeps a profile named with a / in its segment", async () => {
    openApp("/governance/engines/profiles/a%2Fb", POLICY_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "a/b" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);

  it("sends /governance/engines/profiles, which names no profile, to the index", async () => {
    openApp("/governance/engines/profiles", POLICY_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "Engine & policy" }, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/engines");
  }, 15_000);

  it.each([
    ["/governance/engines/e1/advanced/policies/access/new", "engine e1, access, new"],
    ["/governance/engines/e1/advanced/policies/masking/42", "engine e1, masking, 42"],
    ["/governance/engines/profiles/p2/advanced/policies/rowfilter/new", "profile p2, rowfilter, new"],
    ["/governance/engines/profiles/p2/advanced/policies/access/9", "profile p2, access, 9"],
  ])("opens the editor at %s (Q2: the kind stays in the URL)", async (url, said) => {
    openApp(url, POLICY_READER);
    expect(await screen.findByTestId("editor-stub", {}, TIMEOUT)).toHaveTextContent(said);
  }, 15_000);

  it.each([
    ["/governance/engines"],
    ["/governance/engines/e1"],
    ["/governance/engines/profiles/p1"],
    ["/governance/engines/e1/advanced/policies/access/new"],
  ])("refuses %s to a user without policy:read", async (url) => {
    openApp(url, TAG_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe(url);
  }, 15_000);

  it("refuses an old profile URL to a user without policy:read, without a profile lookup", async () => {
    const fetchMock = openApp("/governance/policies/p1", TAG_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies/p1"));
    expect(fetchMock.mock.calls.map((c) => String(c[0]))).not.toContain("/api/control-plane/policies/services");
  }, 15_000);
});
