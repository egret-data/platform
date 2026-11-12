import { QueryClient } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import { enginesQueryKey } from "@/routes/control-plane/queryKeys";
import { ADMIN, CATALOGS, EXTERNAL_ENGINE, Harness, MODELLED_PLUGINS, PROFILES, stubFetch, userWith } from "./externalEngineFixtures";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), warning: vi.fn() }),
}));

beforeEach(() => {
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.error).mockClear();
  vi.mocked(toast.warning).mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = { timeout: 3000 };
const NOT_GOVERNED =
  "Not governed: this engine is bound to no enforcement point, so egret's grants, masks and row filters do not reach it. Bind it to a StarRocks enforcement point in its settings.";
const UNPATCHED =
  "Its FE image is not egret-starrocks-fe 4.1.7-egret.4 or later, so StarRocks lets users past this engine's enforcement point (Iceberg metadata tables, prepared statements, the HTTP SQL API, FILES() and more).";
const NO_ROOT = "Its FE root account has no password: anyone who reaches port 9030 can log in as root.";
const NOT_APPLIED = "The last security upgrade did not finish applying. Run it again.";
const ROOT_DELAY =
  "egret's own admin statements on this engine fail until the FE takes the new password, usually within a minute.";

const OLD_FE = `starrocks/fe-ubuntu:4.1.6@sha256:${"d".repeat(64)}`;
const NEW_FE = `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4@sha256:${"a".repeat(64)}`;

const SR = {
  id: "x1",
  name: "lake-sr",
  type: "starrocks",
  namespace: "engines",
  status: { phase: "running", message: null, pods: [] },
  spec: { starrocksConfig: { feImage: OLD_FE } },
  origin: "provisioned",
  image: OLD_FE,
  notGoverned: false,
  securityUpgradeNeeded: false,
  securityUpgradeReasons: [] as string[],
};

const LEGACY = {
  ...SR,
  securityUpgradeNeeded: true,
  securityUpgradeReasons: ["unpatchedImage", "noRootPassword"],
  securityUpgradeFeImage: NEW_FE,
};

function routes(engine: unknown, extra: Record<string, Parameters<typeof stubFetch>[0][string]> = {}) {
  return {
    "GET /api/control-plane/engines/x1": { body: engine },
    "GET /api/control-plane/policies/services": { body: PROFILES },
    "GET /api/control-plane/catalogs": { body: CATALOGS },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    ...extra,
  };
}

function renderDetail(me = ADMIN, client?: QueryClient) {
  return render(
    <Harness me={me} entry="/control-plane/engines/x1" client={client}>
      <Routes>
        <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
      </Routes>
    </Harness>,
  );
}

/** Wait until the page has rendered the engine, then let its first reads settle. */
async function engineShown() {
  expect(await screen.findByRole("heading", { name: "lake-sr", level: 1 }, T)).toBeInTheDocument();
  await new Promise((r) => setTimeout(r, 50));
}

describe("Not governed (KI-336)", () => {
  test("a provisioned engine the server flags shows the warning", async () => {
    stubFetch(routes({ ...SR, notGoverned: true }));
    renderDetail();
    expect(await screen.findByTestId("starrocks-not-governed", {}, T)).toHaveTextContent(NOT_GOVERNED);
  });

  test("an engine the server does not flag shows none", async () => {
    stubFetch(routes(SR));
    renderDetail();
    await engineShown();
    expect(screen.queryByTestId("starrocks-not-governed")).not.toBeInTheDocument();
  });

  test("an external engine the server flags shows the warning", async () => {
    stubFetch(routes({ ...EXTERNAL_ENGINE, name: "lake-sr", type: "starrocks", port: 9030, notGoverned: true }));
    renderDetail();
    expect(await screen.findByTestId("starrocks-not-governed", {}, T)).toHaveTextContent(NOT_GOVERNED);
  });
});

describe("Security upgrade warning", () => {
  test.each([
    ["unpatchedImage", UNPATCHED],
    ["noRootPassword", NO_ROOT],
    ["notApplied", NOT_APPLIED],
  ])("names %s, and only that reason", async (reason, sentence) => {
    stubFetch(routes({ ...SR, securityUpgradeNeeded: true, securityUpgradeReasons: [reason] }));
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    const items = within(warning)
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(items).toEqual([sentence]);
    expect(within(warning).getByRole("button", { name: "Upgrade security" })).toBeInTheDocument();
  });

  test("names every reason, in the server's order", async () => {
    stubFetch(routes(LEGACY));
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    expect(within(warning).getAllByRole("listitem").map((li) => li.textContent)).toEqual([UNPATCHED, NO_ROOT]);
  });

  test("a reason this client does not know gets no empty line", async () => {
    stubFetch(routes({ ...SR, securityUpgradeNeeded: true, securityUpgradeReasons: ["someFutureReason", "noRootPassword"] }));
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    expect(within(warning).getAllByRole("listitem").map((li) => li.textContent)).toEqual([NO_ROOT]);
  });

  test("when every reason is unknown, one generic line and the button", async () => {
    stubFetch(routes({ ...SR, securityUpgradeNeeded: true, securityUpgradeReasons: ["someFutureReason"] }));
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    expect(within(warning).queryAllByRole("listitem")).toHaveLength(0);
    expect(warning).toHaveTextContent("This engine needs a security upgrade.");
    expect(within(warning).getByRole("button", { name: "Upgrade security" })).toBeInTheDocument();
  });

  test("an engine that needs nothing shows no warning", async () => {
    stubFetch(routes(SR));
    renderDetail();
    await engineShown();
    expect(screen.queryByTestId("starrocks-security-upgrade")).not.toBeInTheDocument();
  });

  test("without engine:update there is no Upgrade security button, and the warning says why", async () => {
    stubFetch(routes(LEGACY));
    renderDetail(userWith("engine:read"));
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    expect(within(warning).queryByRole("button", { name: "Upgrade security" })).not.toBeInTheDocument();
    expect(warning).toHaveTextContent("Upgrading it needs the engine:update permission.");
  });
});

describe("Upgrade security dialog", () => {
  test("lists the changes, warns of the restart, and confirm calls the route and re-reads the engine", async () => {
    const calls = stubFetch(
      routes(LEGACY, {
        "POST /api/control-plane/engines/x1/security-upgrade": {
          body: { engine: SR, imageChanged: true, rootPasswordSet: true },
        },
      }),
    );
    const user = userEvent.setup();
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    await user.click(within(warning).getByRole("button", { name: "Upgrade security" }));

    const dialog = await screen.findByRole("alertdialog");
    const changes = within(dialog).getByTestId("security-upgrade-changes");
    expect(within(changes).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      `FE image: ${OLD_FE} → ${NEW_FE}`,
      "The CN image moves to the same release.",
      "root password will be set",
    ]);
    expect(dialog).toHaveTextContent(
      `The FE and CN pods restart. A client outside egret that logs in as root with no password is refused afterwards. ${ROOT_DELAY}`,
    );
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(0);

    const readsBefore = calls.filter((c) => c.method === "GET" && c.path === "/api/control-plane/engines/x1").length;
    await user.click(within(dialog).getByRole("button", { name: "Upgrade security" }));

    await vi.waitFor(() =>
      expect(calls.filter((c) => c.method === "POST").map((c) => c.path)).toEqual([
        "/api/control-plane/engines/x1/security-upgrade",
      ]),
    );
    await vi.waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "Security upgraded: the FE and CN images were updated and the root password was set. The FE and CN pods are restarting.",
      ),
    );
    expect(toast.warning).not.toHaveBeenCalled();
    await vi.waitFor(() =>
      expect(
        calls.filter((c) => c.method === "GET" && c.path === "/api/control-plane/engines/x1").length,
      ).toBeGreaterThan(readsBefore),
    );
    await vi.waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });

  test("a downgrade-blocked engine: the dialog says the image is kept, and the answer's reason is a warning", async () => {
    const blocked = {
      ...SR,
      image: "starrocks/fe-ubuntu:4.1.8",
      securityUpgradeNeeded: true,
      securityUpgradeReasons: ["unpatchedImage", "noRootPassword"],
      securityUpgradeBlockedRelease: "4.1.8",
    };
    const kept = "No patched image for release 4.1.8 yet: the image was not changed.";
    stubFetch(
      routes(blocked, {
        "POST /api/control-plane/engines/x1/security-upgrade": {
          body: { engine: blocked, imageChanged: false, rootPasswordSet: true, imageKeptBecause: kept },
        },
      }),
    );
    const user = userEvent.setup();
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    await user.click(within(warning).getByRole("button", { name: "Upgrade security" }));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(within(dialog).getByTestId("security-upgrade-changes")).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      kept,
      "root password will be set",
    ]);
    expect(dialog).not.toHaveTextContent("FE image:");

    await user.click(within(dialog).getByRole("button", { name: "Upgrade security" }));
    await vi.waitFor(() => expect(toast.warning).toHaveBeenCalledWith(kept));
    expect(toast.success).toHaveBeenCalledWith(
      "Security upgraded: the root password was set. The FE and CN pods are restarting.",
    );
  });

  test("an engine with no recorded FE image names it as such, and no root delay without a password change", async () => {
    stubFetch(
      routes({
        ...SR,
        image: undefined,
        securityUpgradeNeeded: true,
        securityUpgradeReasons: ["unpatchedImage"],
        securityUpgradeFeImage: NEW_FE,
      }),
    );
    const user = userEvent.setup();
    renderDetail();
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    await user.click(within(warning).getByRole("button", { name: "Upgrade security" }));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(within(dialog).getByTestId("security-upgrade-changes")).getAllByRole("listitem")[0]).toHaveTextContent(
      `FE image: the current image (not recorded) → ${NEW_FE}`,
    );
    expect(dialog).not.toHaveTextContent("unknown");
    expect(dialog).not.toHaveTextContent(ROOT_DELAY);
  });

  test("a successful upgrade invalidates the engines list", async () => {
    stubFetch(
      routes(LEGACY, {
        "POST /api/control-plane/engines/x1/security-upgrade": {
          body: { engine: SR, imageChanged: true, rootPasswordSet: true },
        },
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(enginesQueryKey, []);
    const user = userEvent.setup();
    renderDetail(ADMIN, client);
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    expect(client.getQueryState(enginesQueryKey)?.isInvalidated).toBe(false);
    await user.click(within(warning).getByRole("button", { name: "Upgrade security" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Upgrade security" }));

    await vi.waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(client.getQueryState(enginesQueryKey)?.isInvalidated).toBe(true);
  });

  test("a failed apply (502) shows the server's message, re-reads the engine and keeps the button", async () => {
    const calls = stubFetch(
      routes(LEGACY, {
        "POST /api/control-plane/engines/x1/security-upgrade": {
          status: 502,
          body: { error: "apply failed: the operator did not answer" },
        },
      }),
    );
    // The failed apply leaves the engine flagged `notApplied` on the server:
    // the page must re-read it and mark the list stale.
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(enginesQueryKey, []);
    const user = userEvent.setup();
    renderDetail(ADMIN, client);
    const warning = await screen.findByTestId("starrocks-security-upgrade", {}, T);
    await engineShown();
    await user.click(within(warning).getByRole("button", { name: "Upgrade security" }));
    const readsBefore = calls.filter((c) => c.method === "GET" && c.path === "/api/control-plane/engines/x1").length;
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Upgrade security" }));

    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(expect.stringContaining("apply failed: the operator did not answer")),
    );
    expect(toast.success).not.toHaveBeenCalled();
    expect(client.getQueryState(enginesQueryKey)?.isInvalidated).toBe(true);
    await vi.waitFor(() =>
      expect(
        calls.filter((c) => c.method === "GET" && c.path === "/api/control-plane/engines/x1").length,
      ).toBeGreaterThan(readsBefore),
    );
    expect(
      within(screen.getByTestId("starrocks-security-upgrade")).getByRole("button", { name: "Upgrade security" }),
    ).toBeEnabled();
  });
});
