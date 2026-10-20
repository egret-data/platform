import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient } from "@tanstack/react-query";
import { Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { toast } from "sonner";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import { RangerCredentialSection } from "@/routes/control-plane/RangerCredential";
import {
  EXTERNAL_ENGINE,
  Harness,
  MODELLED_PLUGINS,
  PROFILES,
  stubFetch,
  userWith,
} from "./externalEngineFixtures";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
beforeEach(() => {
  vi.mocked(toast.success).mockClear();
});

const C1 = "11111111-1111-4111-8111-111111111111";
const C2 = "22222222-2222-4222-8222-222222222222";
const NEW_PASSWORD = "pw-ROTATED-ONCE-5b8e2a";
const ROTATE = "POST /api/control-plane/engines/x1/rotate-ranger-credential";
const RETIRE = "POST /api/control-plane/engines/x1/retire-previous-ranger-credential";

/**
 * The external engine page against a server whose engine row follows the
 * rotate / retire calls the page makes, the way the backend's does: a rotation
 * makes C2 current with C1 pending; a retire naming C2 clears the pending one.
 */
function externalServer(start: { current: string | null; pending: boolean }) {
  const row = { ...start };
  const calls = stubFetch({
    "GET /api/control-plane/engines/x1": () => ({
      body: { ...EXTERNAL_ENGINE, rangerCredentialId: row.current, rangerCredentialPending: row.pending },
    }),
    "GET /api/control-plane/policies/services": { body: PROFILES },
    "GET /api/control-plane/catalogs": { body: [] },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    "GET /api/control-plane/engines/x1/ranger-plugin-config": {
      body: {
        serviceName: "trino-main",
        downloadUrl: "https://egret.example.com/ranger",
        authMode: "engine-credential",
        rangerPluginVersion: "2.5.0",
        rangerPluginVersionModelled: true,
        warnings: [],
        assumptions: [],
        adminMustSupply: [],
        files: [{ name: "ranger-trino-security.xml", content: "<configuration/>\n" }],
      },
    },
    [ROTATE]: () => {
      if (row.pending)
        return { status: 409, body: { error: "a previous ranger credential is still pending: retire it first" } };
      row.current = C2;
      row.pending = true;
      return { status: 200, body: { username: "engine-x1", password: NEW_PASSWORD, credentialId: C2 } };
    },
    [RETIRE]: (req) => {
      const installed = (req.body as { installed?: string } | undefined)?.installed;
      if (installed !== row.current)
        return {
          status: 409,
          body: { error: "the installed credential is not the engine's current one: install the current credential and confirm it" },
        };
      row.pending = false;
      return { status: 204 };
    },
  });
  return { calls, row };
}

function renderExternal(me = userWith("engine:read", "engine:update"), client?: QueryClient) {
  return render(
    <Harness me={me} entry="/control-plane/engines/x1" client={client}>
      <Routes>
        <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
      </Routes>
    </Harness>,
  );
}

function section() {
  return screen.getByRole("region", { name: "Ranger credential" });
}

describe("Ranger credential — external engine", () => {
  test("rotate shows the new credential once, then Retire sends the id that rotation returned", async () => {
    const { calls } = externalServer({ current: C1, pending: false });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    renderExternal(undefined, client);

    await screen.findByRole("heading", { name: "Ranger credential" });
    expect(within(section()).getByText("Active")).toBeInTheDocument();
    expect(within(section()).getByTestId("ranger-credential-id")).toHaveTextContent(C1);
    // Nothing is pending, so there is nothing to retire.
    expect(within(section()).queryByRole("button", { name: "Retire previous credential" })).not.toBeInTheDocument();

    await user.click(within(section()).getByRole("button", { name: "Rotate credential" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Rotate the Ranger credential?" });
    expect(confirm).toHaveTextContent(/install the new one and confirm it here/);
    await user.click(within(confirm).getByRole("button", { name: "Rotate credential" }));

    const shown = await screen.findByRole("alertdialog", { name: "New Ranger credential" });
    expect(within(shown).getByLabelText("Password")).toHaveValue(NEW_PASSWORD);
    expect(within(shown).getByLabelText("Credential id")).toHaveValue(C2);
    // The copy names the next control; it exists under that name (FE-COPY-3).
    expect(shown).toHaveTextContent(/choose Retire previous credential/);
    await waitFor(() =>
      expect(JSON.stringify(client.getMutationCache().getAll().map((m) => m.state))).not.toContain(NEW_PASSWORD),
    );
    // Escape keeps it; the way out waits for the confirmation.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("alertdialog", { name: "New Ranger credential" })).toBeInTheDocument();
    expect(within(shown).getByRole("button", { name: "Done" })).toBeDisabled();
    await user.click(within(shown).getByRole("checkbox", { name: /I have saved/ }));
    await user.click(within(shown).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(document.body.innerHTML).not.toContain(NEW_PASSWORD);

    // The engine now reads as pending; Rotate is blocked, saying why.
    expect(await within(section()).findByText("Previous credential pending")).toBeInTheDocument();
    const rotate = within(section()).getByRole("button", { name: "Rotate credential" });
    expect(rotate).toBeDisabled();
    expect(rotate).toHaveAccessibleDescription(/blocked until you retire the previous credential/);

    await user.click(within(section()).getByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    expect(retire).toHaveTextContent(C2);
    await user.click(within(retire).getByRole("button", { name: "Retire previous credential" }));

    await waitFor(() => expect(calls.some((c) => `${c.method} ${c.path}` === RETIRE)).toBe(true));
    expect(calls.find((c) => `${c.method} ${c.path}` === RETIRE)?.body).toEqual({ installed: C2 });
    expect(await within(section()).findByText("Active")).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith(expect.stringMatching(/Previous credential retired/));
    expect(within(section()).getByRole("button", { name: "Rotate credential" })).toBeEnabled();
  });

  test("Retire names the credential THIS page's rotation returned, not whatever is current now", async () => {
    const C3 = "33333333-3333-4333-8333-333333333333";
    const { calls, row } = externalServer({ current: C1, pending: false });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    renderExternal(undefined, client);

    await user.click(await screen.findByRole("button", { name: "Rotate credential" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Rotate credential" }),
    );
    const shown = await screen.findByRole("alertdialog", { name: "New Ranger credential" });
    await user.click(within(shown).getByRole("checkbox", { name: /I have saved/ }));
    await user.click(within(shown).getByRole("button", { name: "Done" }));

    // Meanwhile another tab confirmed C2 and rotated again: C3 is current, C2 pending.
    row.current = C3;
    row.pending = true;
    await client.invalidateQueries();
    expect(await within(section()).findByTestId("ranger-credential-id")).toHaveTextContent(C3);

    await user.click(within(section()).getByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    await user.click(within(retire).getByRole("button", { name: "Retire previous credential" }));

    // C2 is what this operator installed; naming C3 would retire C2 out from under the engine.
    await waitFor(() => expect(calls.find((c) => `${c.method} ${c.path}` === RETIRE)?.body).toEqual({ installed: C2 }));
    expect(await screen.findByTestId("ranger-credential-error")).toHaveTextContent("not the engine's current one");
    expect(row.pending).toBe(true);
  });

  test("the rotation's id survives a visit to the Plugin config tab, where the operator installs it", async () => {
    const { calls } = externalServer({ current: C1, pending: false });
    const user = userEvent.setup();
    renderExternal();

    await user.click(await screen.findByRole("button", { name: "Rotate credential" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Rotate credential" }),
    );
    const shown = await screen.findByRole("alertdialog", { name: "New Ranger credential" });
    await user.click(within(shown).getByRole("checkbox", { name: /I have saved/ }));
    await user.click(within(shown).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());

    // Install it from the Plugin config tab (the Overview panel unmounts), then come back.
    await user.click(screen.getByRole("tab", { name: "Plugin config" }));
    expect(await screen.findByText("ranger-trino-security.xml")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Ranger credential" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Overview" }));

    await user.click(await within(section()).findByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    // Not the typed fallback: the token this page's rotation returned.
    expect(within(retire).queryByLabelText("Credential id of the installed credential")).not.toBeInTheDocument();
    expect(retire).toHaveTextContent(C2);
    await user.click(within(retire).getByRole("button", { name: "Retire previous credential" }));
    await waitFor(() => expect(calls.find((c) => `${c.method} ${c.path}` === RETIRE)?.body).toEqual({ installed: C2 }));
  });

  test("in the typed fallback the current id is hidden and the hint warns against copying it", async () => {
    externalServer({ current: C2, pending: true });
    const user = userEvent.setup();
    renderExternal();
    expect(await screen.findByTestId("ranger-credential-id")).toHaveTextContent(C2);
    await user.click(await screen.findByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    expect(screen.queryByTestId("ranger-credential-id")).not.toBeInTheDocument();
    expect(within(retire).getByLabelText("Credential id of the installed credential")).toHaveAccessibleDescription(
      /Do not copy the engine's current credential id from this page unless that is the credential the engine runs/,
    );
  });

  test("copying announces the result to assistive technology", async () => {
    externalServer({ current: C1, pending: false });
    const user = userEvent.setup();
    renderExternal();
    await user.click(await screen.findByRole("button", { name: "Rotate credential" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Rotate credential" }),
    );
    const shown = await screen.findByRole("alertdialog", { name: "New Ranger credential" });
    await user.click(within(shown).getByRole("button", { name: "Copy password" }));
    await waitFor(() =>
      expect(within(shown).getAllByRole("status").map((e) => e.textContent)).toContain("Copied the password"),
    );
    // Only the field that was copied says so.
    expect(within(shown).getAllByRole("status").filter((e) => e.textContent)).toHaveLength(1);
    expect(await navigator.clipboard.readText()).toBe(NEW_PASSWORD);
  });

  test("after a reload the rotation's id is gone: Retire asks for it and sends what the operator entered", async () => {
    const { calls } = externalServer({ current: C2, pending: true });
    const user = userEvent.setup();
    renderExternal();

    await user.click(await screen.findByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    const field = within(retire).getByLabelText("Credential id of the installed credential");
    const send = within(retire).getByRole("button", { name: "Retire previous credential" });
    expect(send).toBeDisabled();
    await user.type(field, "not-an-id");
    expect(send).toBeDisabled();
    await user.clear(field);
    await user.type(field, C2);
    expect(send).toBeEnabled();
    await user.click(send);

    await waitFor(() => expect(calls.find((c) => `${c.method} ${c.path}` === RETIRE)?.body).toEqual({ installed: C2 }));
    expect(await within(section()).findByText("Active")).toBeInTheDocument();
  });

  test("a stale confirmation's 409 shows the server's sentence and retires nothing", async () => {
    const { row } = externalServer({ current: C2, pending: true });
    const user = userEvent.setup();
    renderExternal();

    await user.click(await screen.findByRole("button", { name: "Retire previous credential" }));
    const retire = await screen.findByRole("alertdialog", { name: "Retire the previous credential?" });
    await user.type(within(retire).getByLabelText("Credential id of the installed credential"), C1);
    await user.click(within(retire).getByRole("button", { name: "Retire previous credential" }));

    expect(await screen.findByTestId("ranger-credential-error")).toHaveTextContent(
      "the installed credential is not the engine's current one",
    );
    expect(row.pending).toBe(true);
    expect(within(section()).getByText("Previous credential pending")).toBeInTheDocument();
  });

  test("a rotation refused with 409 shows the server's sentence, and no credential dialog opens", async () => {
    // The page read the row before another tab rotated it: it offers Rotate, the server refuses.
    let first = true;
    const calls = stubFetch({
      "GET /api/control-plane/engines/x1": () => ({
        body: { ...EXTERNAL_ENGINE, rangerCredentialId: C1, rangerCredentialPending: !first },
      }),
      "GET /api/control-plane/policies/services": { body: PROFILES },
      "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
      [ROTATE]: () => {
        first = false;
        return { status: 409, body: { error: "a previous ranger credential is still pending: retire it first" } };
      },
    });
    const user = userEvent.setup();
    renderExternal();
    await user.click(await screen.findByRole("button", { name: "Rotate credential" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Rotate credential" }),
    );
    expect(await screen.findByTestId("ranger-credential-error")).toHaveTextContent(
      "a previous ranger credential is still pending: retire it first",
    );
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(calls.filter((c) => `${c.method} ${c.path}` === ROTATE)).toHaveLength(1);
  });

  test("without engine:update the state is shown and no control is offered", async () => {
    externalServer({ current: C2, pending: true });
    renderExternal(userWith("engine:read"));
    await screen.findByRole("heading", { name: "Ranger credential" });
    expect(within(section()).getByText("Previous credential pending")).toBeInTheDocument();
    expect(within(section()).queryByRole("button")).not.toBeInTheDocument();
    expect(within(section()).getByText(/needs the engine:update permission/)).toBeInTheDocument();
  });
});

// ── Provisioned ──────────────────────────────────────────────────────────────

const PROVISIONED = {
  id: "p1",
  type: "trino",
  origin: "provisioned",
  rangerCredentialId: C1 as string | null,
  rangerCredentialPending: false,
};
const P_ROTATE = "POST /api/control-plane/engines/p1/rotate-ranger-credential";

function renderProvisioned(over: Partial<typeof PROVISIONED> = {}) {
  return render(
    <Harness me={userWith("engine:read", "engine:update")} entry="/">
      <RangerCredentialSection engine={{ ...PROVISIONED, ...over }} />
    </Harness>,
  );
}

async function rotateProvisioned(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Rotate credential" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Rotate the Ranger credential?" });
  expect(confirm).toHaveTextContent(/pods restart/);
  await user.click(within(confirm).getByRole("button", { name: "Rotate credential" }));
}

describe("Ranger credential — provisioned engine", () => {
  test("while the previous credential is pending, Rotate is disabled until the rollout completes, and there is no Retire", () => {
    stubFetch({});
    renderProvisioned({ rangerCredentialPending: true });
    expect(screen.getByText("Previous credential pending")).toBeInTheDocument();
    expect(screen.getByText(/until the rollout of the current one completes/)).toBeInTheDocument();
    const rotate = screen.getByRole("button", { name: "Rotate credential" });
    expect(rotate).toBeDisabled();
    expect(rotate).toHaveAccessibleDescription(/until the rollout of the current credential completes/);
    expect(screen.queryByRole("button", { name: "Retire previous credential" })).not.toBeInTheDocument();
  });

  test("an accepted rotation (204) confirms with a toast, shows no credential, and sends no body", async () => {
    const calls = stubFetch({ [P_ROTATE]: { status: 204 } });
    const user = userEvent.setup();
    renderProvisioned();
    await rotateProvisioned(user);
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(expect.stringMatching(/pods are restarting.*until the rollout completes/)),
    );
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(calls.filter((c) => `${c.method} ${c.path}` === P_ROTATE)).toEqual([
      { method: "POST", path: "/api/control-plane/engines/p1/rotate-ranger-credential", body: undefined },
    ]);
  });

  test("a 409 (rollout still pending) shows the server's sentence", async () => {
    stubFetch({
      [P_ROTATE]: {
        status: 409,
        body: { error: "the previous ranger credential is pending until the current one is rolled out; retry later" },
      },
    });
    const user = userEvent.setup();
    renderProvisioned();
    await rotateProvisioned(user);
    expect(await screen.findByTestId("ranger-credential-error")).toHaveTextContent(
      "the previous ranger credential is pending until the current one is rolled out; retry later",
    );
  });

  test("a 502 (Kubernetes rejected the apply) shows the server's sentence", async () => {
    stubFetch({ [P_ROTATE]: { status: 502, body: { error: "cluster apply failed: admission webhook denied" } } });
    const user = userEvent.setup();
    renderProvisioned();
    await rotateProvisioned(user);
    expect(await screen.findByTestId("ranger-credential-error")).toHaveTextContent(
      "cluster apply failed: admission webhook denied",
    );
    expect(toast.success).not.toHaveBeenCalled();
  });

  test("an engine with no credential yet cannot rotate, and says why", () => {
    stubFetch({});
    renderProvisioned({ rangerCredentialId: null });
    expect(screen.getByText("No credential")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rotate credential" })).toHaveAccessibleDescription(
      /bound to a policy profile/,
    );
  });
});
