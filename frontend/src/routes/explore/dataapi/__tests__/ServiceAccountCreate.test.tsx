import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// There is no msw in this repo — tests mock the API module directly and hand
// the hook a return value with `vi.mocked(...).mockReturnValue(...)`, the same
// way `EndpointCreate.test.tsx` does.
vi.mock("@/api/dataApi");

import * as dataApi from "@/api/dataApi";
import ServiceAccountCreate from "@/routes/explore/dataapi/ServiceAccountCreate";

const navigate = vi.fn();
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => navigate,
}));

// The workspace is a PARAMETER so one test can mount a different one. The
// route pattern below is `:workspace`, which is the name `App.tsx` registers;
// this repo has already shipped a `useParams().ws`-versus-`:workspace`
// mismatch that no test could see, because the test mounted a route tree the
// app did not have. Mounting the real pattern is what makes that catchable,
// and the `other-ws` test below is what makes the READ of it catchable.
function renderCreate(ws = "ws1") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/${ws}/data-api/clients/new`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/clients/new" element={<ServiceAccountCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

function mockCreate(over: Record<string, unknown> = {}) {
  const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "sa-9" } });
  vi.mocked(dataApi.useCreateServiceAccount).mockReturnValue({ mutateAsync, isPending: false, ...over } as never);
  return mutateAsync;
}

describe("ServiceAccountCreate", () => {
  beforeEach(() => {
    vi.resetAllMocks();
  });

  it("sends the two fields a service account row needs, and nothing more", async () => {
    const mutateAsync = mockCreate();
    renderCreate();

    // NOT "reporting-bot": that is the field's own placeholder, and it was the
    // string every test here typed. `name: "reporting-bot"` hard-coded into the
    // request body passed all five — the same unfalsifiable-constant trap this
    // file already documents for `description`, standing on the REQUIRED field.
    // No two tests type the same name now, so no single constant satisfies them.
    await userEvent.type(screen.getByLabelText("Name"), "nightly-exporter");
    await userEvent.type(screen.getByLabelText("Description (optional)"), "Nightly finance export");
    await userEvent.click(screen.getByRole("button", { name: /create service account/i }));

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    expect(mutateAsync.mock.calls[0][0].workspace).toBe("ws1");
    const body = mutateAsync.mock.calls[0][0].data;
    expect(body).toMatchObject({ name: "nightly-exporter", description: "Nightly finance export" });
    // "and nothing more" asserted on the WHOLE key set rather than as a list
    // of names not to send: a `not.toContain("clients")` style check only rules
    // out the fields I happened to think of, while the exact key set rules out
    // every field the page could invent — the clients, grants and expiry that
    // belong to the account once it exists, and anything added later.
    expect(Object.keys(body).sort()).toEqual(["description", "name"]);
  });

  // The other half of the optional field, and it is the half that carries the
  // falsifiability. The test above passes for a page that ALWAYS sends
  // `description`, so on its own it pins nothing about the field being
  // optional — exactly the trap Task 1 measured on `method`, where asserting
  // only the non-default value moved which constant was unfalsifiable instead
  // of removing it. Blank must mean the key is ABSENT, not `""` and not
  // `undefined`: an empty string is a value, and would overwrite nothing with
  // something.
  it("leaves description out entirely when the author writes none", async () => {
    const mutateAsync = mockCreate();
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "audit-sync");
    await userEvent.click(screen.getByRole("button", { name: /create service account/i }));

    // `toEqual` on the whole body, not just its keys: it pins the absent
    // description AND a second, different name in one assertion.
    expect(mutateAsync.mock.calls[0][0].data).toEqual({ name: "audit-sync" });
  });

  // The one place this page differs from `EndpointCreate`, and it is not an
  // inconsistency: an endpoint has a detail route to hand off to, a service
  // account does not — it is a row in a list with a menu — so success returns
  // to the list where that row now is.
  it("returns to the Clients list on success", async () => {
    mockCreate();
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "reporting-bot");
    await userEvent.click(screen.getByRole("button", { name: /create service account/i }));

    expect(navigate).toHaveBeenCalledWith("/explore/ws1/data-api/clients");
  });

  // Everything else here mounts `ws1`, so `const ws = "ws1"` — a page that
  // ignored the route entirely — would pass all of them. This one mounts a
  // different workspace and follows it through BOTH places it is used: the
  // workspace the write goes to, and the list the author is returned to.
  it("takes the workspace from the route, into the write and the return trip", async () => {
    const mutateAsync = mockCreate();
    renderCreate("other-ws");

    await userEvent.type(screen.getByLabelText("Name"), "cross-ws-bot");
    await userEvent.click(screen.getByRole("button", { name: /create service account/i }));

    expect(mutateAsync.mock.calls[0][0].workspace).toBe("other-ws");
    expect(navigate).toHaveBeenCalledWith("/explore/other-ws/data-api/clients");
  });

  // Name is the one field a row cannot exist without, and the gate on it is
  // the only thing stopping a submit the backend would reject. Untested, a
  // page that always enabled Create passed every other test in this file.
  it("cannot be submitted until a name is typed", async () => {
    mockCreate();
    renderCreate();

    const create = screen.getByRole("button", { name: /create service account/i });
    expect(create).toBeDisabled();
    // Whitespace is not a name: `canSubmit` trims, so the gate must still hold.
    await userEvent.type(screen.getByLabelText("Name"), "   ");
    expect(create).toBeDisabled();
    await userEvent.type(screen.getByLabelText("Name"), "reporting-bot");
    expect(create).toBeEnabled();
  });

  it("keeps what the author typed and shows the server's message when creation fails", async () => {
    vi.mocked(dataApi.useCreateServiceAccount).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("name already taken")),
      isPending: false,
    } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "half-typed-bot");
    await userEvent.type(screen.getByLabelText("Description (optional)"), "Nightly finance export");
    await userEvent.click(screen.getByRole("button", { name: /create service account/i }));

    // A dialog could be dismissed and retried; a page that clears itself on
    // failure loses work the author cannot get back. The server's own message
    // must be on screen — in a live region, so it is announced and not merely
    // painted — and every field must still hold its value.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/name already taken/i);
    expect(screen.getByLabelText("Name")).toHaveValue("half-typed-bot");
    expect(screen.getByLabelText("Description (optional)")).toHaveValue("Nightly finance export");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("renders exactly one h1", () => {
    mockCreate();
    renderCreate();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
});
