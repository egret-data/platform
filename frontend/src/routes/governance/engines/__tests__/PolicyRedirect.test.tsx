import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { queryClient } from "@/api/queryClient";
import { currentUrl, openApp, POLICY_READER, profile, TIMEOUT } from "./appHarness";

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

/**
 * Spec §3: every old Policies URL lands where the table says, with `replace`.
 * The fixtures are not in the order the answer needs: the shared profile
 * lists engine `b` before `a`, so a redirect taking the first in list order
 * goes to the wrong engine (Q3: the first by name).
 */
const PROFILES = [
  profile("shared", [
    { id: "eb", name: "b" },
    { id: "ea", name: "a" },
  ]),
  profile("p2"),
  profile("p1", [{ id: "E1", name: "trino-1" }]),
  profile("a/b"),
];

describe("old Policies URLs", () => {
  it.each([
    ["/governance/policies", "/governance/engines"],
    ["/governance/policies/p1", "/governance/engines/E1?tab=policies"],
    ["/governance/policies/p1?tab=access", "/governance/engines/E1?tab=policies&type=access"],
    ["/governance/policies/p1?tab=masking", "/governance/engines/E1?tab=policies&type=masking"],
    ["/governance/policies/p1?tab=rowfilter", "/governance/engines/E1?tab=policies&type=rowfilter"],
    ["/governance/policies/p1?tab=tags", "/governance/engines/E1?tab=policies&type=tag"],
    ["/governance/policies/p2", "/governance/engines/profiles/p2?tab=policies"],
    ["/governance/policies/a%2Fb", "/governance/engines/profiles/a%2Fb?tab=policies"],
    ["/governance/policies/shared", "/governance/engines/ea?tab=policies"],
    ["/governance/policies/p1/migrate", "/governance/engines/E1?tab=migrate"],
    ["/governance/policies/p2/migrate", "/governance/engines/profiles/p2?tab=migrate"],
    [
      "/governance/policies/p1/masking/new?scope=tag",
      "/governance/engines/E1/advanced/policies/masking/new?scope=tag",
    ],
    [
      "/governance/policies/p1/access/new?scope=tag#deny",
      "/governance/engines/E1/advanced/policies/access/new?scope=tag#deny",
    ],
    ["/governance/policies/p1/access/42", "/governance/engines/E1/advanced/policies/access/42"],
    ["/governance/policies/p2/rowfilter/7", "/governance/engines/profiles/p2/advanced/policies/rowfilter/7"],
    ["/governance/policies/shared/access/42", "/governance/engines/ea/advanced/policies/access/42"],
    ["/control-plane/policies/p1", "/governance/engines/E1?tab=policies"],
    ["/control-plane/policies/p1/access/new?scope=tag", "/governance/engines/E1/advanced/policies/access/new?scope=tag"],
  ])("%s → %s, replacing the old URL", async (from, to) => {
    window.history.pushState({}, "", "/before");
    openApp(from, POLICY_READER, PROFILES);
    await waitFor(() => expect(currentUrl()).toBe(to), TIMEOUT);
    // `replace`: Back returns to the page before, not to the dead URL.
    window.history.back();
    await waitFor(() => expect(currentUrl()).toBe("/before"), TIMEOUT);
  }, 15_000);

  it.each([["/governance/policies/zz"], ["/governance/policies/zz/migrate"], ["/governance/policies/zz/access/3"]])(
    "%s, naming no profile, lands on the index with a notice",
    async (from) => {
      openApp(from, POLICY_READER, PROFILES);
      await waitFor(() => expect(currentUrl()).toBe("/governance/engines"), TIMEOUT);
      expect(await screen.findByText("No policy profile named zz.", {}, TIMEOUT)).toBeInTheDocument();
    },
    15_000,
  );

  it("says why while the profile list cannot be read, with Retry, and stays on the old URL", async () => {
    openApp("/governance/policies/p1", POLICY_READER, "error");
    expect(await screen.findByRole("button", { name: "Retry" }, { timeout: 8000 })).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/policies/p1");
  }, 15_000);
});
