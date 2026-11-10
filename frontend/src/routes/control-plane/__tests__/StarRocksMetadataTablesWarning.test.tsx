import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import EngineRegister from "@/routes/control-plane/EngineRegister";
import {
  ADMIN,
  CATALOGS,
  EXTERNAL_ENGINE,
  Harness,
  LocationProbe,
  MODELLED_PLUGINS,
  PROFILES,
  stubFetch,
} from "./externalEngineFixtures";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = { timeout: 3000 };
const WARNING =
  /Unless this engine runs egret-starrocks-fe 4\.1\.7-egret\.4 or later.*metadata tables of Iceberg tables .* without asking Ranger.*prepared statements.*HTTP SQL API or Arrow Flight SQL.*FILES\(\).*materialized view.*EXPLAIN COSTS.*BROKER LOAD, EXPORT and SELECT INTO OUTFILE/;

const SR_EXTERNAL = { ...EXTERNAL_ENGINE, name: "legacy-sr", type: "starrocks", port: 9030, rangerPluginVersion: "2.8.0" };
const SR_PROVISIONED = {
  id: "x1",
  name: "lake-sr",
  type: "starrocks",
  namespace: "engines",
  status: { phase: "running", message: null, pods: [] },
  spec: { starrocksConfig: {} },
  origin: "provisioned",
};

/** The profile list with `profileId` bound to engine x1. */
function profilesBinding(profileId: number | null) {
  return PROFILES.map((p) =>
    p.id === profileId ? { ...p, engines: [{ id: "x1", name: "engine-x1", status: "running" }] } : p,
  );
}

function detailRoutes(engine: unknown, profileId: number | null) {
  return {
    "GET /api/control-plane/engines/x1": { body: engine },
    "GET /api/control-plane/policies/services": { body: profilesBinding(profileId) },
    "GET /api/control-plane/catalogs": { body: CATALOGS },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
  };
}

function renderDetail(entry = "/control-plane/engines/x1") {
  return render(
    <Harness me={ADMIN} entry={entry}>
      <Routes>
        <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
        <Route path="/control-plane/engines" element={<LocationProbe />} />
      </Routes>
    </Harness>,
  );
}

describe("StarRocks metadata-table warning, Control plane", () => {
  test("external StarRocks engine with a bound profile: Overview shows it", async () => {
    stubFetch(detailRoutes(SR_EXTERNAL, 21));
    renderDetail();
    expect(await screen.findByRole("link", { name: "sr-main" }, T)).toBeInTheDocument();
    expect(screen.getByTestId("starrocks-metadata-tables")).toHaveTextContent(WARNING);
    expect(screen.getByTestId("starrocks-metadata-tables")).toHaveTextContent(/serve masked or filtered data through Trino/);
  });

  test("external StarRocks engine without a profile: no warning", async () => {
    stubFetch(detailRoutes(SR_EXTERNAL, null));
    renderDetail();
    expect(await screen.findByText("None bound. Bind one on the Settings tab.", {}, T)).toBeInTheDocument();
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("external Trino engine with a bound profile: no warning", async () => {
    stubFetch(detailRoutes(EXTERNAL_ENGINE, 11));
    renderDetail();
    expect(await screen.findByRole("link", { name: "trino-main" }, T)).toBeInTheDocument();
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("external StarRocks Settings: shown when a profile is picked, gone when it is cleared", async () => {
    stubFetch(detailRoutes(SR_EXTERNAL, null));
    const user = userEvent.setup();
    renderDetail("/control-plane/engines/x1?tab=settings");
    const picker = await screen.findByRole("combobox", { name: "Policy profile" }, T);
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
    await user.click(picker);
    await user.click(await screen.findByRole("option", { name: /sr-main/ }));
    expect(screen.getByTestId("starrocks-metadata-tables")).toHaveTextContent(WARNING);
    await user.click(screen.getByRole("combobox", { name: "Policy profile" }));
    await user.click(await screen.findByRole("option", { name: "No profile" }));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("provisioned StarRocks engine with no image verdict in its payload, bound: shown above the tabs", async () => {
    stubFetch(detailRoutes(SR_PROVISIONED, 21));
    renderDetail();
    expect(await screen.findByTestId("starrocks-metadata-tables", {}, T)).toHaveTextContent(WARNING);
  });

  // The security-upgrade warning names the image and offers the fix, so the page carries one
  // message, not two. The image string is the official one the old frontend rule flagged; only
  // the server's verdict (`securityUpgradeReasons`) is read.
  test.each([
    ["the official image", `starrocks/fe-ubuntu:4.1.6@sha256:${"d".repeat(64)}`],
    ["an earlier patched image (egret.3)", `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.3@sha256:${"a".repeat(64)}`],
  ])("provisioned StarRocks engine on %s, bound: hidden while the upgrade warning says it", async (_, feImage) => {
    const unpatched = {
      ...SR_PROVISIONED,
      image: feImage,
      spec: { starrocksConfig: { feImage } },
      securityUpgradeNeeded: true,
      securityUpgradeReasons: ["unpatchedImage"],
    };
    const calls = stubFetch(detailRoutes(unpatched, 21));
    renderDetail();
    expect(await screen.findByTestId("starrocks-security-upgrade", {}, T)).toBeInTheDocument();
    await vi.waitFor(() => expect(calls.some((c) => c.path === "/api/control-plane/policies/services")).toBe(true));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("provisioned StarRocks engine on egret's complete patched image, bound: no warning", async () => {
    const patched = {
      ...SR_PROVISIONED,
      spec: {
        starrocksConfig: {
          feImage: `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4@sha256:${"a".repeat(64)}`,
        },
      },
      securityUpgradeNeeded: false,
      securityUpgradeReasons: [],
    };
    const calls = stubFetch(detailRoutes(patched, 21));
    renderDetail();
    expect(await screen.findByRole("heading", { name: "lake-sr", level: 1 }, T)).toBeInTheDocument();
    await vi.waitFor(() => expect(calls.some((c) => c.path === "/api/control-plane/policies/services")).toBe(true));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("provisioned StarRocks engine bound to no profile: no warning", async () => {
    const calls = stubFetch(detailRoutes(SR_PROVISIONED, null));
    renderDetail();
    expect(await screen.findByRole("heading", { name: "lake-sr", level: 1 }, T)).toBeInTheDocument();
    await vi.waitFor(() => expect(calls.some((c) => c.path === "/api/control-plane/policies/services")).toBe(true));
    // Let the profile list answer before asserting the absence.
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
  });

  test("Register: shown only for StarRocks with a profile picked", async () => {
    stubFetch({
      "GET /api/control-plane/policies/services": { body: PROFILES },
      "GET /api/control-plane/catalogs": { body: CATALOGS },
      "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    });
    const user = userEvent.setup();
    render(
      <Harness me={ADMIN} entry="/control-plane/engines/register">
        <Routes>
          <Route path="/control-plane/engines/register" element={<EngineRegister />} />
        </Routes>
      </Harness>,
    );
    // Trino with a Trino profile: nothing.
    await user.click(await screen.findByRole("combobox", { name: "Policy profile" }, T));
    await user.click(await screen.findByRole("option", { name: /trino-main/ }));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
    // StarRocks, no profile yet: nothing.
    await user.click(screen.getByRole("combobox", { name: "Engine type" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));
    expect(screen.queryByTestId("starrocks-metadata-tables")).not.toBeInTheDocument();
    // StarRocks with a profile: the warning.
    await user.click(screen.getByRole("combobox", { name: "Policy profile" }));
    await user.click(await screen.findByRole("option", { name: /sr-main/ }));
    expect(screen.getByTestId("starrocks-metadata-tables")).toHaveTextContent(WARNING);
  });
});
