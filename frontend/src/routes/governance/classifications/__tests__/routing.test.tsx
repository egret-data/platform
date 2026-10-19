import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { queryClient } from "@/api/queryClient";
import { currentUrl, openApp, POLICY_READER, TAG_ONLY, TIMEOUT } from "@/routes/governance/engines/__tests__/appHarness";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

/** Spec §2: the Classification area's URLs, mounted lazily under `tag:read`. */
describe("the Classification area's routes", () => {
  it("opens the index at /governance/classifications", async () => {
    openApp("/governance/classifications", TAG_ONLY);
    expect(await screen.findByRole("heading", { level: 1, name: "Classifications" }, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/classifications");
  }, 15_000);

  it("opens a tag page at /governance/classifications/:id and keeps ?tab=", async () => {
    openApp("/governance/classifications/t1?tab=clearance", TAG_ONLY);
    // Every read rejects here, so the page shows its error, never the index or a 403.
    expect(await screen.findByRole("region", { name: "Classification" }, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: "Classifications" })).not.toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/classifications/t1?tab=clearance");
  }, 15_000);

  it.each([["/governance/classifications"], ["/governance/classifications/t1"]])(
    "refuses %s to a user without tag:read",
    async (url) => {
      openApp(url, POLICY_READER);
      expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
      expect(currentUrl()).toBe(url);
    },
    15_000,
  );

  it("lands an old Control Plane tag URL on the tag page, query kept", async () => {
    openApp("/control-plane/classifications/x?tab=rules", TAG_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/classifications/x?tab=rules"), TIMEOUT);
    expect(await screen.findByRole("region", { name: "Classification" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);
});

describe("tagUrl", () => {
  it("names the tag page, with the tab when given, encoding the id", () => {
    expect(tagUrl("t1")).toBe("/governance/classifications/t1");
    expect(tagUrl("t1", "clearance")).toBe("/governance/classifications/t1?tab=clearance");
    expect(tagUrl("a/b", "rules")).toBe("/governance/classifications/a%2Fb?tab=rules");
  });
});
