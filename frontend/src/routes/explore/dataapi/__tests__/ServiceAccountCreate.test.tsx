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

  it("puts the way back above the title, not against the primary action", () => {
    // The same assertion `EndpointCreate.test.tsx` carries, and the reason it
    // is here is that the two pages disagreed in the browser: `EndpointCreate`
    // was converted to `ListPageShell`'s `back` slot and this page was not, so
    // at 1440px `← Clients` sat directly beside "Create service account" here
    // while `← Endpoints` sat in the opposite corner one click away.
    //
    // Two assertions because either alone can be satisfied by the defect: DOM
    // order alone is true of a link rendered first INSIDE the action group, and
    // "not in the action group" alone is true of a link dropped anywhere below
    // the header.
    mockCreate();
    const { container } = renderCreate();

    const back = screen.getByRole("link", { name: /clients/i });
    const h1 = screen.getByRole("heading", { level: 1 });
    const submit = screen.getByRole("button", { name: /create service account/i });

    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    const actionGroup = container.querySelector("header")!.lastElementChild as HTMLElement;
    expect(actionGroup).toContainElement(submit);
    expect(actionGroup).not.toContainElement(back);
  });
});

/**
 * The subtitle is an INSTRUCTION, and it named a control that has never
 * existed: "issue it OAuth2 clients and grant it endpoints from the Clients
 * list". The Clients list grants nothing. Its account menu offers
 * `Issue client…` and `Delete` (`ServiceAccountActionsMenu`), and its client
 * rows offer `Revoke…`; invoke is granted per ENDPOINT, on that endpoint's
 * Access tab (`detail/AccessSection.tsx` is the only caller of
 * `useDataApiGrantEndpointInvoke`). A reader who followed the sentence went
 * hunting the Clients list for a control that is not there — which is worse
 * than the silence it was written to fix, because silence does not send anyone
 * anywhere.
 *
 * The page's own doc comment had it right the whole time. These tests are the
 * halves of the claim, split so that neither can carry the other: one pins
 * where the sentence sends a grant, one pins that it no longer sends it to the
 * surface that cannot serve one. `Clients.test.tsx` holds the other end of the
 * reconciliation — that the Clients account menu really has no grant item —
 * because a sentence can only be checked against the tree it describes.
 */
describe("ServiceAccountCreate — the hand-off sentence is true of the shipped tree", () => {
  /**
   * The subtitle BY IDENTITY: `ListPageShell` renders it as the `<p>`
   * immediately after the `h1`, in the same heading block. A `getByText`
   * regex would match the sentence wherever it drifted to — including a
   * second copy of it below the form — and would still pass if the header
   * lost its description entirely and some other prose happened to match.
   */
  function subtitle(): string {
    const h1 = screen.getByRole("heading", { level: 1, name: "New service account" });
    const p = h1.nextElementSibling;
    expect(p?.tagName).toBe("P");
    const text = (p as HTMLElement).textContent ?? "";
    // Guards the assertions below against the empty string, on which a
    // `not.toMatch` is vacuously true.
    expect(text.length).toBeGreaterThan(40);
    return text;
  }

  it("sends a grant to an endpoint's Access tab, under Endpoints", () => {
    mockCreate();
    renderCreate();
    // Not "mentions Access somewhere": the destination has to be specific
    // enough to walk to. `Access` alone is ambiguous with the workspace
    // Access-control console, which is a different screen in a different
    // sidebar group and grants nothing about an endpoint.
    expect(subtitle()).toMatch(/grant[\s\S]*Access tab under Endpoints/i);
  });

  it("does not send a grant to the Clients list, which has no grant control", () => {
    mockCreate();
    renderCreate();
    const text = subtitle();
    // The clause, not the sentence. Naming Clients is CORRECT here — that is
    // where a client is issued — so a flat `not.toMatch(/Clients/)` would
    // forbid the one true half. The defect was the binding: the verb `grant`
    // and the noun `Clients` in one clause. Clauses are split on the
    // separators the sentence actually uses.
    const clauses = text.split(/[.;\u2014]/).map((c) => c.trim()).filter(Boolean);
    const grantClauses = clauses.filter((c) => /\bgrant/i.test(c));
    // Fails loudly rather than passing on a subtitle that stopped talking
    // about grants at all — silence is the outcome this fix is NOT.
    expect(grantClauses).toHaveLength(1);
    expect(grantClauses[0]).not.toMatch(/Clients/i);
  });
});
