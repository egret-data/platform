import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, test, vi } from "vitest";

import EngineRegister from "@/routes/control-plane/EngineRegister";
import {
  ADMIN,
  CATALOGS,
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

const READS = {
  "GET /api/control-plane/policies/services": { body: PROFILES },
  "GET /api/control-plane/catalogs": { body: CATALOGS },
  "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
};

function renderRegister(me = ADMIN) {
  return render(
    <Harness me={me} entry="/control-plane/engines/register">
      <Routes>
        <Route path="/control-plane/engines/register" element={<EngineRegister />} />
        <Route path="/control-plane/engines/:id" element={<LocationProbe />} />
      </Routes>
    </Harness>,
  );
}

async function fillRequired(user: ReturnType<typeof userEvent.setup>, version = "2.5.0") {
  await user.type(screen.getByLabelText("Name"), "legacy-trino");
  await user.type(screen.getByLabelText("Host"), "trino.corp.example");
  await user.type(screen.getByLabelText("Port"), "8080");
  await user.click(screen.getByRole("combobox", { name: "Plugin version" }));
  await user.click(await screen.findByRole("option", { name: version }));
}

describe("EngineRegister", () => {
  test("the profile picker lists only profiles of the chosen type, and switching type clears an incompatible pick", async () => {
    stubFetch(READS);
    const user = userEvent.setup();
    renderRegister();

    await user.click(await screen.findByRole("combobox", { name: "Policy profile" }));
    expect(await screen.findByRole("option", { name: /trino-main/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /sr-main/ })).not.toBeInTheDocument();
    // A profile another engine already binds is shown, disabled, with the reason.
    const taken = screen.getByRole("option", { name: /trino-taken/ });
    expect(taken).toHaveAttribute("aria-disabled", "true");
    expect(taken).toHaveTextContent(/bound to other-trino/);
    await user.click(screen.getByRole("option", { name: /trino-main/ }));
    expect(screen.getByRole("combobox", { name: "Policy profile" })).toHaveTextContent("trino-main");

    await user.click(screen.getByRole("combobox", { name: "Engine type" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));

    expect(screen.getByRole("combobox", { name: "Policy profile" })).not.toHaveTextContent("trino-main");
    await user.click(screen.getByRole("combobox", { name: "Policy profile" }));
    expect(await screen.findByRole("option", { name: /sr-main/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /trino-main/ })).not.toBeInTheDocument();
  });

  test("submit posts exactly the fields the API takes and lands on the Plugin config tab", async () => {
    const calls = stubFetch({
      ...READS,
      "POST /api/control-plane/engines/external": { status: 201, body: { id: "x1", name: "legacy-trino" } },
    });
    const user = userEvent.setup();
    renderRegister();

    await fillRequired(user);
    await user.click(await screen.findByRole("combobox", { name: "Policy profile" }));
    await user.click(await screen.findByRole("option", { name: /trino-main/ }));
    await user.click(await screen.findByRole("checkbox", { name: "Declare lake attached" }));
    await user.click(screen.getByRole("button", { name: "Register engine" }));

    await waitFor(() =>
      expect(screen.getByTestId("location")).toHaveTextContent(
        "/control-plane/engines/x1?tab=plugin-config",
      ),
    );
    const post = calls.find((c) => c.method === "POST");
    expect(post?.body).toEqual({
      name: "legacy-trino",
      type: "trino",
      host: "trino.corp.example",
      port: 8080,
      rangerPluginVersion: "2.5.0",
      authzServiceId: 11,
      catalogs: ["c1"],
    });
  });

  test("a 400 shows the server's sentence beside the form and keeps every field", async () => {
    stubFetch({
      ...READS,
      "POST /api/control-plane/engines/external": {
        status: 400,
        body: { error: "rangerPluginVersion 9.9.9 is not a Ranger 2.x release egret knows." },
      },
    });
    const user = userEvent.setup();
    renderRegister();

    await fillRequired(user, "2.8.0");
    await user.click(screen.getByRole("button", { name: "Register engine" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("rangerPluginVersion 9.9.9 is not a Ranger 2.x release egret knows.");
    expect(screen.getByLabelText("Name")).toHaveValue("legacy-trino");
    expect(screen.getByLabelText("Host")).toHaveValue("trino.corp.example");
    expect(screen.getByLabelText("Port")).toHaveValue(8080);
    expect(screen.getByRole("combobox", { name: "Plugin version" })).toHaveTextContent("2.8.0");
    expect(screen.queryByTestId("location")).not.toBeInTheDocument();
  });

  test("a 409 name clash shows the server's sentence and keeps the fields", async () => {
    stubFetch({
      ...READS,
      "POST /api/control-plane/engines/external": {
        status: 409,
        body: { error: "An engine named legacy-trino already exists." },
      },
    });
    const user = userEvent.setup();
    renderRegister();
    await fillRequired(user);
    await user.click(screen.getByRole("button", { name: "Register engine" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("An engine named legacy-trino already exists.");
    expect(screen.getByLabelText("Name")).toHaveValue("legacy-trino");
  });

  test("a rule_overlap 409 renders the pair, not a generic error", async () => {
    stubFetch({
      ...READS,
      "POST /api/control-plane/engines/external": {
        status: 409,
        body: {
          error: "Binding would make two mask policies decide one column differently.",
          code: "rule_overlap",
          kind: "mask",
          coordinate: "lake.sales.orders.email",
          rules: [
            { id: 7, name: "perm-mask-7" },
            { id: null, name: "hand-written-mask" },
          ],
        },
      },
    });
    const user = userEvent.setup();
    renderRegister();
    await fillRequired(user);
    await user.click(screen.getByRole("button", { name: "Register engine" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Binding would make two mask policies decide one column differently.");
    expect(within(alert).getByText("perm-mask-7")).toBeInTheDocument();
    expect(within(alert).getByText("hand-written-mask")).toBeInTheDocument();
    expect(alert).toHaveTextContent("lake.sales.orders.email");
  });

  test("submit is disabled, with the reason beside it, until name, host, port and version are set", async () => {
    stubFetch(READS);
    const user = userEvent.setup();
    renderRegister();

    const submit = screen.getByRole("button", { name: "Register engine" });
    expect(submit).toBeDisabled();
    expect(submit).toHaveAccessibleDescription(/Enter a name, a host, a port and a plugin version/);

    await fillRequired(user);
    expect(screen.getByRole("button", { name: "Register engine" })).toBeEnabled();
  });

  test("an 'Other' plugin version takes a typed x.y.z, and an unmodelled pair says migrations will be refused", async () => {
    const calls = stubFetch({
      ...READS,
      "POST /api/control-plane/engines/external": { status: 201, body: { id: "x2" } },
    });
    const user = userEvent.setup();
    renderRegister();
    await user.type(screen.getByLabelText("Name"), "sr1");
    await user.type(screen.getByLabelText("Host"), "sr.corp");
    await user.type(screen.getByLabelText("Port"), "9030");
    await user.click(screen.getByRole("combobox", { name: "Engine type" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));
    await user.click(screen.getByRole("combobox", { name: "Plugin version" }));
    await user.click(await screen.findByRole("option", { name: "Other…" }));

    const field = screen.getByLabelText("Other plugin version");
    await user.type(field, "2.7");
    expect(screen.getByRole("button", { name: "Register engine" })).toBeDisabled();
    await user.type(field, ".1");
    expect(screen.getByRole("button", { name: "Register engine" })).toBeEnabled();
    expect(screen.getByText(/migrations into the bound profile will be refused/i)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Register engine" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      type: "starrocks",
      rangerPluginVersion: "2.7.1",
    });
  });

  test("the version picker offers only the modelled versions of the chosen type", async () => {
    stubFetch(READS);
    const user = userEvent.setup();
    renderRegister();

    await user.click(screen.getByRole("combobox", { name: "Plugin version" }));
    expect(await screen.findByRole("option", { name: "2.5.0" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "2.8.0" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "2.9.0" })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("combobox", { name: "Engine type" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));
    await user.click(screen.getByRole("combobox", { name: "Plugin version" }));
    expect(await screen.findByRole("option", { name: "2.9.0" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "2.8.0" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "2.5.0" })).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Other…" })).toBeInTheDocument();
  });

  test("a user without engine:create sees no form, only the reason", async () => {
    stubFetch(READS);
    renderRegister(userWith("engine:read"));
    expect(screen.queryByRole("button", { name: "Register engine" })).not.toBeInTheDocument();
    expect(screen.getByText(/engine:create/)).toBeInTheDocument();
  });
});
