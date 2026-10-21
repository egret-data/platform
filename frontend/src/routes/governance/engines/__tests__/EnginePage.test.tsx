import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  E_OTHER,
  E_SR,
  E_TR,
  fail,
  grantsResponse,
  ok,
  OPERATOR,
  openArea,
  pluginResponse,
  policy,
  profileDto,
  READER,
  serve,
  where,
  type Answer,
} from "./engineHarness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const tabNames = () => screen.getAllByRole("tab").map((t) => t.textContent?.replace(/\d+$/, "").trim());

/** The reads an engine page makes, for engine E_TR bound to `lake-tr`. */
function engineAnswers(over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse()),
    "/policies/services": ok([profileDto("lake-tr", { engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }] })]),
    "/policies?service=lake-tr": ok([policy(1, "compiler"), policy(2, "operator")]),
    [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    ...over,
  };
}

describe("the engine page's header (spec §6)", () => {
  it("names the engine, its type, origin and profile, and the plugin version with engine:read", async () => {
    serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}`, OPERATOR);
    expect(await screen.findByRole("heading", { level: 1, name: "lake-trino" }, T)).toBeInTheDocument();
    const header = screen.getByRole("region", { name: "Engine" });
    expect(within(header).getByText("Trino")).toBeInTheDocument();
    expect(within(header).getByText("Provisioned")).toBeInTheDocument();
    expect(within(header).getByText("lake-tr")).toBeInTheDocument();
    expect(await within(header).findByText("Ranger plugin 2.5.0", {}, T)).toBeInTheDocument();
    expect(within(header).getByRole("link", { name: "Open in Control plane" })).toHaveAttribute(
      "href",
      `/control-plane/engines/${E_TR}`,
    );
  });

  it("says the plugin version is not modelled, with the server's note", async () => {
    serve(
      engineAnswers({
        [`/governance/engines/${E_TR}/plugin`]: ok(
          pluginResponse({ plugin: { version: "2.4.0", modelled: false, note: "egret models 2.5.0 only." } }),
        ),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, OPERATOR);
    const header = await screen.findByRole("region", { name: "Engine" }, T);
    expect(await within(header).findByText("Ranger plugin 2.4.0", {}, T)).toBeInTheDocument();
    expect(within(header).getByText("Not modelled: egret models 2.5.0 only.")).toBeInTheDocument();
  });

  it("without engine:read: one line naming the permission, no plugin request, no Control plane link (EP19)", async () => {
    const { asked } = serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const header = await screen.findByRole("region", { name: "Engine" }, T);
    expect(await within(header).findByText("lake-tr", {}, T)).toBeInTheDocument();
    expect(within(header).getByText("Plugin state needs the engine:read permission.")).toBeInTheDocument();
    expect(within(header).queryByRole("link", { name: "Open in Control plane" })).not.toBeInTheDocument();
    expect(asked.some((a) => a.includes("/plugin"))).toBe(false);
  });

  it("says No profile for an engine that downloads none", async () => {
    serve(engineAnswers({ [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ profile: null })) }));
    openArea(`/governance/engines/${E_TR}`, READER);
    const header = await screen.findByRole("region", { name: "Engine" }, T);
    expect(await within(header).findByText("No profile", {}, T)).toBeInTheDocument();
  });
});

describe("the engine page's banners (EP14)", () => {
  it("names the other engines of a shared profile, by name", async () => {
    serve(
      engineAnswers({
        [`/perm/engines/${E_TR}/grants`]: ok(
          grantsResponse({
            reach: {
              engines: [
                { id: E_TR, name: "lake-trino", origin: "provisioned" },
                { id: E_OTHER, name: "zeta", origin: "provisioned" },
                { id: E_SR, name: "alpha", origin: "external" },
              ],
              sharedProfile: true,
            },
          }),
        ),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(
      await screen.findByText(
        "Profile lake-tr is shared with engines alpha, zeta. Everything on this page applies to all of them.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/cannot see whether the external engine downloads them/)).not.toBeInTheDocument();
  });

  it("says egret cannot see downloads for an external engine", async () => {
    serve(
      engineAnswers({
        [`/perm/engines/${E_TR}/grants`]: ok(
          grantsResponse({
            engine: { id: E_TR, name: "lake-trino", type: "trino", origin: "external", status: "registered" },
          }),
        ),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(
      await screen.findByText(
        "egret compiles this engine's policies but cannot see whether the external engine downloads them.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/is shared with engines/)).not.toBeInTheDocument();
  });

  it("shows neither banner for an unshared provisioned engine", async () => {
    serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByRole("heading", { level: 1, name: "lake-trino" }, T)).toBeInTheDocument();
    expect(screen.queryByText(/is shared with engines/)).not.toBeInTheDocument();
    expect(screen.queryByText(/cannot see whether the external engine/)).not.toBeInTheDocument();
  });
});

describe("the engine page's tabs (spec §6, FE-PAGE-2)", () => {
  it("lists the six tabs in order when the profile has a manual policy", async () => {
    serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}`, READER);
    await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(6), T);
    expect(tabNames()).toEqual(["Engine permissions", "Policies", "Overlaps", "Migrate", "Plugin", "Advanced"]);
    expect(screen.getByRole("tab", { name: "Engine permissions" })).toHaveAttribute("aria-selected", "true");
  });

  it("hides Migrate while the profile and its tag service hold no manual policy", async () => {
    serve(
      engineAnswers({
        "/policies/services": ok([
          profileDto("lake-tr", { tag_service: "lake-tag", engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }] }),
          profileDto("lake-tag", { type: "tag" }),
        ]),
        "/policies?service=lake-tr": ok([policy(1, "compiler"), policy(3, "infra")]),
        "/policies?service=lake-tag": ok([policy(4, "compiler", { policy_type: "tag" })]),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByRole("tab", { name: "Advanced" }, T)).toBeInTheDocument();
    await waitFor(() => expect(tabNames()).toEqual(["Engine permissions", "Policies", "Overlaps", "Plugin", "Advanced"]));
  });

  it("shows Migrate when only the tag service holds a manual policy", async () => {
    serve(
      engineAnswers({
        "/policies/services": ok([
          profileDto("lake-tr", { tag_service: "lake-tag", engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }] }),
          profileDto("lake-tag", { type: "tag" }),
        ]),
        "/policies?service=lake-tr": ok([policy(1, "compiler")]),
        "/policies?service=lake-tag": ok([policy(4, "operator", { policy_type: "tag" })]),
      }),
    );
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByRole("tab", { name: "Migrate" }, T)).toBeInTheDocument();
  });

  it("selects the tab ?tab= names, and a click writes it to the URL", async () => {
    serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}?tab=plugin`, OPERATOR);
    expect(await screen.findByRole("tab", { name: "Plugin" }, T)).toHaveAttribute("aria-selected", "true");
    await userEvent.click(screen.getByRole("tab", { name: "Advanced" }));
    expect(where()).toBe(`/governance/engines/${E_TR}?tab=advanced`);
    await userEvent.click(screen.getByRole("tab", { name: "Engine permissions" }));
    expect(where()).toBe(`/governance/engines/${E_TR}`);
  });

  it("falls back to the first tab when ?tab= names a hidden tab", async () => {
    serve(engineAnswers({ "/policies?service=lake-tr": ok([policy(1, "compiler")]) }));
    openArea(`/governance/engines/${E_TR}?tab=migrate`, READER);
    expect(await screen.findByRole("tab", { name: "Advanced" }, T)).toBeInTheDocument();
    await waitFor(() => expect(tabNames()).not.toContain("Migrate"));
    expect(screen.getByRole("tab", { name: "Engine permissions" })).toHaveAttribute("aria-selected", "true");
  });

  it("falls back to the first tab for a name that is no tab at all", async () => {
    serve(engineAnswers());
    openArea(`/governance/engines/${E_TR}?tab=nope`, READER);
    expect(await screen.findByRole("tab", { name: "Engine permissions" }, T)).toHaveAttribute("aria-selected", "true");
  });
});

describe("the engine page's failures (FE-STATE-1)", () => {
  it("says no engine has the id, with a link to the index, on a 404", async () => {
    serve(engineAnswers({ [`/perm/engines/${E_TR}/grants`]: fail(404, "engine not found") }));
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByText("No engine with this id.", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Engine & policy" })).toHaveAttribute("href", "/governance/engines");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("shows a failing grants read with Retry, and keeps the tab strip", async () => {
    serve(engineAnswers({ [`/perm/engines/${E_TR}/grants`]: fail(500) }));
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByRole("button", { name: "Retry" }, T)).toBeInTheDocument();
    expect(screen.getByText(/could not be loaded/)).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Engine permissions" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Plugin" })).toBeInTheDocument();
  });
});

describe("the profile page (EP13, EP15)", () => {
  const unbound = (over: Record<string, Answer> = {}) => ({
    "/policies/services": ok([
      profileDto("loose", { origin: "manual", type: "starrocks" }),
      profileDto("lake-tr", {
        engines: [
          { id: E_TR, name: "zz-trino", status: "running", origin: "provisioned", type: "trino" },
          { id: E_OTHER, name: "aa-trino", status: "running", origin: "provisioned", type: "trino" },
        ],
      }),
    ]),
    "/policies?service=loose": ok([policy(9, "operator", { service: "loose" })]),
    ...over,
  });

  it("frames an unbound profile without Engine permissions and Plugin", async () => {
    serve(unbound());
    openArea("/governance/engines/profiles/loose", READER);
    expect(await screen.findByRole("heading", { level: 1, name: "loose" }, T)).toBeInTheDocument();
    expect(await screen.findByText("No engine is bound to this profile.", {}, T)).toBeInTheDocument();
    const header = screen.getByRole("region", { name: "Profile" });
    expect(within(header).getByText("StarRocks")).toBeInTheDocument();
    expect(within(header).getByText("Manual")).toBeInTheDocument();
    await waitFor(() => expect(tabNames()).toEqual(["Policies", "Overlaps", "Migrate", "Advanced"]));
    expect(screen.getByRole("tab", { name: "Policies" })).toHaveAttribute("aria-selected", "true");
  });

  it("falls back to Policies when ?tab= names Engine permissions or Plugin", async () => {
    serve(unbound());
    openArea("/governance/engines/profiles/loose?tab=plugin", READER);
    expect(await screen.findByRole("tab", { name: "Policies" }, T)).toHaveAttribute("aria-selected", "true");
  });

  it("replaces a bound profile's URL with its engine page, first engine by name, keeping the query", async () => {
    serve(unbound());
    openArea("/governance/engines/profiles/lake-tr?tab=overlaps", READER);
    await waitFor(() => expect(where()).toBe(`/governance/engines/${E_OTHER}?tab=overlaps`), T);
  });

  it("says no profile has the name, with a link to the index", async () => {
    serve(unbound());
    openArea("/governance/engines/profiles/ghost", READER);
    expect(await screen.findByText("No policy profile named ghost.", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Engine & policy" })).toHaveAttribute("href", "/governance/engines");
  });

  it("shows a failing profile list with Retry", async () => {
    serve(unbound({ "/policies/services": fail(500) }));
    openArea("/governance/engines/profiles/loose", READER);
    expect(await screen.findByRole("button", { name: "Retry" }, T)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "loose" })).toBeInTheDocument();
  });
});
