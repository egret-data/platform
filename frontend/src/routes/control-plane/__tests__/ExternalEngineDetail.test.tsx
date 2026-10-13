import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import {
  ADMIN,
  CATALOGS,
  EXTERNAL_ENGINE,
  Harness,
  LocationProbe,
  MODELLED_PLUGINS,
  PROFILES,
  stubFetch,
  userWith,
} from "./externalEngineFixtures";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const BOUND_PROFILES = PROFILES.map((p) =>
  p.id === 11 ? { ...p, engines: [{ id: "x1", name: "legacy-trino", status: "running" }] } : p,
);

const DECLARED_CATALOGS = CATALOGS.map((c) =>
  c.id === "c1"
    ? {
        ...c,
        engines: [
          {
            engineId: "x1",
            engineName: "legacy-trino",
            engineType: "trino",
            applyStatus: "declared",
            isPrimary: false,
            createdAt: "2026-01-02T00:00:00Z",
          },
        ],
      }
    : c,
);

const PLUGIN_CONFIG = {
  serviceName: "trino-main",
  downloadUrl: "https://egret.example.com/service/plugins/policies/download/trino-main",
  authMode: "bearer-configured",
  rangerPluginVersion: "2.5.0",
  rangerPluginVersionModelled: true,
  warnings: ["egret answers on an in-cluster address; set [catalog] public_url."],
  assumptions: ["The plugin polls every 30 seconds."],
  adminMustSupply: ["A path that adds the bearer token to the policy download."],
  files: [
    { name: "access-control.properties", content: "access-control.name=ranger\n" },
    { name: "ranger-trino-security.xml", content: "<configuration/>\n" },
  ],
  // Not part of the contract: a page that rendered unknown fields would leak it.
  bearerToken: "tok-must-never-render",
};

function routes(over: Record<string, unknown> = {}) {
  return {
    "GET /api/control-plane/engines/x1": { body: EXTERNAL_ENGINE },
    "GET /api/control-plane/policies/services": { body: BOUND_PROFILES },
    "GET /api/control-plane/catalogs": { body: DECLARED_CATALOGS },
    "GET /api/control-plane/engines/x1/ranger-plugin-config": { body: PLUGIN_CONFIG },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    ...over,
  } as Parameters<typeof stubFetch>[0];
}

function renderDetail(me = ADMIN, entry = "/control-plane/engines/x1") {
  return render(
    <Harness me={me} entry={entry}>
      <Routes>
        <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
        <Route path="/control-plane/engines" element={<LocationProbe />} />
      </Routes>
    </Harness>,
  );
}

describe("EngineDetail for an external engine", () => {
  test("shows the External chip, the address, the version and the bound profile, and offers no provisioning tabs or actions", async () => {
    stubFetch(routes());
    renderDetail();

    expect(await screen.findByRole("heading", { name: "legacy-trino", level: 1 })).toBeInTheDocument();
    const tabs = screen.getAllByRole("tab").map((t) => t.textContent);
    expect(tabs).toEqual(["Overview", "Catalogs", "Plugin config", "Settings"]);
    for (const hidden of ["Monitor", "Workspaces"]) {
      expect(screen.queryByRole("tab", { name: hidden })).not.toBeInTheDocument();
    }
    expect(screen.getByText("External")).toBeInTheDocument();
    expect(screen.getByText("trino.corp.example:8080")).toBeInTheDocument();
    expect(screen.getByText("2.5.0")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "trino-main" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-main",
    );
    // Lifecycle routes refuse an external engine with 409, so none is offered.
    for (const name of [/restart/i, /rotate/i, /logs/i, /terminal/i]) {
      expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
    }
    // A provisioned engine polls its cluster; an external one has nothing to poll.
    expect(screen.queryByText(/pods/i)).not.toBeInTheDocument();
  });

  test("an unmodelled (type, version) pair says migrations into the bound profile will be refused", async () => {
    stubFetch(
      routes({
        "GET /api/control-plane/engines/x1": {
          body: { ...EXTERNAL_ENGINE, rangerPluginVersion: "2.6.0", rangerPluginVersionModelled: false },
        },
      }),
    );
    renderDetail();
    const note = await screen.findByTestId("unmodelled-note");
    expect(note).toHaveTextContent(/migrations into the bound profile will be refused/i);
  });

  test("a modelled pair shows no such warning", async () => {
    stubFetch(routes());
    renderDetail();
    await screen.findByRole("heading", { name: "legacy-trino", level: 1 });
    expect(screen.queryByTestId("unmodelled-note")).not.toBeInTheDocument();
  });

  test("delete says egret removes its record and does not touch the cluster, and calls the delete route", async () => {
    const calls = stubFetch(
      routes({ "DELETE /api/control-plane/engines/x1": { status: 204 } }),
    );
    const user = userEvent.setup();
    renderDetail();

    await user.click(await screen.findByRole("button", { name: "More engine actions" }));
    await user.click(await screen.findByRole("menuitem", { name: /Delete engine/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("egret removes its record; your cluster is not touched.");
    await user.type(within(dialog).getByRole("textbox"), "legacy-trino");
    await user.click(within(dialog).getByRole("button", { name: /delete/i }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/control-plane/engines/x1")).toBe(true));
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/control-plane/engines"));
  });

  test("Catalogs tab lists the declared catalogs and attaches and detaches through the registry routes", async () => {
    const calls = stubFetch(
      routes({
        "POST /api/control-plane/catalogs/c2/engines/x1": {
          status: 201,
          body: { engineId: "x1", engineName: "legacy-trino", engineType: "trino", applyStatus: "declared", isPrimary: false, createdAt: "2026-01-03T00:00:00Z" },
        },
        "DELETE /api/control-plane/catalogs/c1/engines/x1": { status: 204 },
      }),
    );
    const user = userEvent.setup();
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=catalogs");

    const row = (await screen.findByText("lake")).closest("tr")!;
    expect(within(row).getByText("declared")).toBeInTheDocument();
    expect(screen.getByText(/does not create these catalogs on your engine/)).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Catalog to declare" }));
    await user.click(await screen.findByRole("option", { name: "sales" }));
    await user.click(screen.getByRole("button", { name: "Declare attached" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/catalogs/c2/engines/x1"))).toBe(true));

    await user.click(within(row).getByRole("button", { name: "Detach" }));
    const confirm = await screen.findByRole("alertdialog");
    await user.click(within(confirm).getByRole("button", { name: "Detach" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path.endsWith("/catalogs/c1/engines/x1"))).toBe(true));
  });

  test("Plugin config tab renders every file, warning and assumption, and never a secret", async () => {
    stubFetch(routes());
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=plugin-config");

    expect(await screen.findByText("access-control.properties")).toBeInTheDocument();
    expect(screen.getByText("ranger-trino-security.xml")).toBeInTheDocument();
    expect(screen.getByText(/access-control.name=ranger/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download access-control.properties" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy ranger-trino-security.xml" })).toBeInTheDocument();
    expect(screen.getByText("trino-main")).toBeInTheDocument();
    expect(screen.getByText(PLUGIN_CONFIG.downloadUrl)).toBeInTheDocument();
    const warning = screen.getByText(/set \[catalog\] public_url/);
    expect(warning.closest('[role="note"]')).toHaveTextContent(/^Warning:/);
    expect(screen.getByText("The plugin polls every 30 seconds.")).toBeInTheDocument();
    expect(screen.getByText("A path that adds the bearer token to the policy download.")).toBeInTheDocument();
    expect(screen.getByText(/bearer token is configured/i)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("tok-must-never-render");
  });

  test("Plugin config with no profile bound says to bind one, in the server's words", async () => {
    stubFetch(
      routes({
        "GET /api/control-plane/policies/services": { body: PROFILES },
        "GET /api/control-plane/engines/x1/ranger-plugin-config": {
          status: 409,
          body: { error: "Bind a profile first: the plugin asks for a service by its name." },
        },
      }),
    );
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=plugin-config");
    expect(await screen.findByText(/Bind a profile first/)).toBeInTheDocument();
  });

  test("Settings: saving sends only what changed", async () => {
    const calls = stubFetch(
      routes({
        "PATCH /api/control-plane/engines/x1/external": { body: { ...EXTERNAL_ENGINE, port: 9090 } },
      }),
    );
    const user = userEvent.setup();
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=settings");

    const save = await screen.findByRole("button", { name: "Save changes" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(/Nothing has changed/);
    const port = screen.getByLabelText("Port");
    await user.clear(port);
    await user.type(port, "9090");
    expect(save).toBeEnabled();
    await user.click(save);
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ port: 9090 });
  });

  test("Settings: unbinding the profile sends authzServiceId null", async () => {
    const calls = stubFetch(
      routes({ "PATCH /api/control-plane/engines/x1/external": { body: EXTERNAL_ENGINE } }),
    );
    const user = userEvent.setup();
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=settings");
    const picker = await screen.findByRole("combobox", { name: "Policy profile" });
    await waitFor(() => expect(picker).toHaveTextContent("trino-main"));
    await user.click(picker);
    await user.click(await screen.findByRole("option", { name: "No profile" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ authzServiceId: null });
  });

  test("Settings: a rebind refused with rule_overlap shows the sentence and the pair, and keeps the form", async () => {
    stubFetch(
      routes({
        "PATCH /api/control-plane/engines/x1/external": {
          status: 409,
          body: {
            error: "Recompiling profile trino-main would emit an order-ambiguous pair.",
            code: "rule_overlap",
            kind: "row_filter",
            coordinate: "lake.sales.orders",
            rules: [
              { id: 3, name: "perm-filter-3" },
              { id: null, name: "analyst-filter" },
            ],
          },
        },
      }),
    );
    const user = userEvent.setup();
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=settings");
    const host = await screen.findByLabelText("Host");
    await user.clear(host);
    await user.type(host, "trino2.corp.example");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Recompiling profile trino-main would emit an order-ambiguous pair.");
    expect(within(alert).getByText("perm-filter-3")).toBeInTheDocument();
    expect(within(alert).getByText("analyst-filter")).toBeInTheDocument();
    expect(alert).toHaveTextContent("lake.sales.orders");
    expect(screen.getByLabelText("Host")).toHaveValue("trino2.corp.example");
  });

  test("Settings: a version that is not modelled warns before saving", async () => {
    stubFetch(routes({ "PATCH /api/control-plane/engines/x1/external": { body: EXTERNAL_ENGINE } }));
    const user = userEvent.setup();
    renderDetail(ADMIN, "/control-plane/engines/x1?tab=settings");
    await user.click(await screen.findByRole("combobox", { name: "Plugin version" }));
    await user.click(await screen.findByRole("option", { name: "Other…" }));
    await user.type(screen.getByLabelText("Other plugin version"), "2.6.0");
    expect(await screen.findByTestId("unmodelled-note")).toHaveTextContent(/will be refused/);
    // A pair egret models (Trino at 2.8.0) carries no warning.
    await user.clear(screen.getByLabelText("Other plugin version"));
    await user.type(screen.getByLabelText("Other plugin version"), "2.8.0");
    expect(screen.queryByTestId("unmodelled-note")).not.toBeInTheDocument();
  });

  test("a read-only user sees the engine and its config but no write control, and is told why", async () => {
    stubFetch(routes());
    const user = userEvent.setup();
    renderDetail(userWith("engine:read", "catalog_registry:read"));

    await screen.findByRole("heading", { name: "legacy-trino", level: 1 });
    expect(screen.queryByRole("button", { name: "More engine actions" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Settings" }));
    expect(await screen.findByText(/needs the engine:update permission/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save changes" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Host")).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Catalogs" }));
    expect(await screen.findByText("lake")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Detach" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Declare attached" })).not.toBeInTheDocument();
    expect(screen.getByText(/needs the catalog_registry:update permission/)).toBeInTheDocument();
  });
});
