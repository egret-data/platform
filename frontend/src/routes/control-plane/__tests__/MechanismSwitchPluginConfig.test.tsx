import { QueryClient } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes, useNavigate } from "react-router-dom";
import { afterEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import EnginesRoutes from "@/routes/governance/engines/EnginesRoutes";
import { E_TR, grantsResponse, pluginResponse, profileDto } from "@/routes/governance/engines/__tests__/engineHarness";
import { ADMIN, EXTERNAL_ENGINE, Harness, MODELLED_PLUGINS, stubFetch } from "./externalEngineFixtures";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/**
 * Final review I1: the external engine page and the governance Advanced tab read the same
 * profile list under two cache keys. A switch made on one must reach the other, or the Plugin
 * config tab picks its route from the old mechanism.
 */
function Nav() {
  const go = useNavigate();
  return (
    <>
      <button onClick={() => go("/governance/engines/" + E_TR + "?tab=advanced")}>to governance</button>
      <button onClick={() => go("/control-plane/engines/x1?tab=plugin-config")}>to engine</button>
    </>
  );
}

const EXT = { id: "x1", name: "legacy-trino", status: "running", origin: "external", type: "trino" };
const PROV = { id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" };

function world(startMechanism: "ranger" | "opa") {
  let mechanism: string = startMechanism;
  const target = startMechanism === "ranger" ? "opa" : "ranger";
  const profiles = () => [profileDto("lake-tr", { id: 2, origin: "generated", mechanism, engines: [PROV, EXT] })];
  return stubFetch({
    "GET /api/control-plane/engines/x1": { body: EXTERNAL_ENGINE },
    "GET /api/control-plane/policies/services": () => ({ body: profiles() }),
    "GET /api/control-plane/catalogs": { body: [] },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    [`GET /api/control-plane/perm/engines/${E_TR}/grants`]: { body: grantsResponse() },
    "GET /api/control-plane/policies": { body: [] },
    "GET /api/control-plane/perm/overlaps": { body: [] },
    [`GET /api/control-plane/governance/engines/${E_TR}/plugin`]: { body: pluginResponse() },
    "PATCH /api/control-plane/policies/services/lake-tr": () => {
      mechanism = target;
      return { body: { name: "lake-tr", mechanism, engines_reconfigured: [], external_engines_unchanged: ["legacy-trino"] } };
    },
    "GET /api/control-plane/engines/x1/ranger-plugin-config": {
      body: {
        serviceName: "lake-tr",
        downloadUrl: "https://egret/x",
        authMode: "anonymous",
        rangerPluginVersion: "2.5.0",
        rangerPluginVersionModelled: true,
        warnings: [],
        assumptions: [],
        adminMustSupply: [],
        files: [{ name: "ranger-trino-security.xml", content: "<configuration/>" }],
      },
    },
    "GET /api/control-plane/engines/x1/opa-plugin-config": {
      body: { files: [{ name: "egret-opa-context.properties", content: "egret.engine=x1" }], warnings: [], adminMustSupply: [] },
    },
  });
}

async function switchTo(user: ReturnType<typeof userEvent.setup>, label: "OPA" | "Ranger") {
  await user.click(screen.getByRole("button", { name: "to governance" }));
  await user.click(await screen.findByLabelText(label, {}, { timeout: 4000 }));
  await user.click(within_dialog().getByRole("button", { name: "Switch" }));
}
import { within } from "@testing-library/react";
const within_dialog = () => within(screen.getByRole("alertdialog"));

function renderBoth(client: QueryClient) {
  render(
    <Harness me={{ ...ADMIN, permissions: [...ADMIN.permissions, "policy:read", "policy:update"] }} entry="/control-plane/engines/x1?tab=plugin-config" client={client}>
      <Nav />
      <Routes>
        <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
        <Route path="/governance/engines/*" element={<EnginesRoutes />} />
      </Routes>
    </Harness>,
  );
}

describe("a mechanism switch reaches the external engine's Plugin config tab with a warm cache", () => {
  test("Ranger to OPA: the tab then asks the OPA route", async () => {
    const calls = world("ranger");
    const user = userEvent.setup();
    renderBoth(new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } }));
    expect(await screen.findByText("ranger-trino-security.xml")).toBeInTheDocument();

    await switchTo(user, "OPA");
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    const mark = calls.length;
    await user.click(screen.getByRole("button", { name: "to engine" }));

    expect(await screen.findByText("egret-opa-context.properties")).toBeInTheDocument();
    const after = calls.slice(mark).map((c) => c.path);
    expect(after.some((p) => p.endsWith("/opa-plugin-config"))).toBe(true);
    expect(screen.queryByText("ranger-trino-security.xml")).not.toBeInTheDocument();
  });

  test("OPA to Ranger: no OPA request after the switch, and no false 'no profile bound'", async () => {
    const calls = world("opa");
    const user = userEvent.setup();
    renderBoth(new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } }));
    expect(await screen.findByText("egret-opa-context.properties")).toBeInTheDocument();

    await switchTo(user, "Ranger");
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    const mark = calls.length;
    await user.click(screen.getByRole("button", { name: "to engine" }));

    expect(await screen.findByText("ranger-trino-security.xml")).toBeInTheDocument();
    expect(calls.slice(mark).some((c) => c.path.endsWith("/opa-plugin-config"))).toBe(false);
    expect(document.body.textContent).not.toContain("No policy profile is bound");
  });
});
