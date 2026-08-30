import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type React from "react";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/**
 * `react-resizable-panels`, stubbed — and as of this task `TryIt` no longer
 * imports it, which makes this a **tripwire rather than a stand-in**.
 *
 * It stays for exactly one reason: it is the only thing in jsdom that can
 * OBSERVE the split coming back. The stub marks every `Group`, `Panel` and
 * `Separator` it renders, so `renders no resizable split` below fails the
 * moment this section reintroduces one — whereas deleting the mock would leave
 * that assertion with nothing to match and it would pass either way, on a
 * section that had regressed. (Deleting it would also put the REAL library into
 * jsdom, where every rect is 0×0 and its `Separator` wins the hit-test for a
 * click anywhere in the group: every `userEvent.type` in this file would go to
 * a drag handle instead of a field. That was this stub's original job.)
 *
 * The stub echoes the SIZES back as attributes rather than swallowing them,
 * because those are what a reader cannot verify by eye: in v4 a bare number
 * means PIXELS and a string means percent, so `defaultSize={40}` is a 40-pixel
 * pane that still looks right in the source. Kept so that a reintroduced split
 * fails LOUDLY on its geometry as well as on its presence.
 */
vi.mock("react-resizable-panels", () => ({
  Group: ({ children, className, orientation }: { children?: React.ReactNode; className?: string; orientation?: string }) => (
    <div data-testid="panel-group" data-orientation={orientation} className={className}>
      {children}
    </div>
  ),
  Panel: ({ children, className, defaultSize, minSize }: { children?: React.ReactNode; className?: string; defaultSize?: number | string; minSize?: number | string }) => (
    <div
      data-testid="panel"
      data-default-size={String(defaultSize)}
      data-default-size-type={typeof defaultSize}
      data-min-size={String(minSize)}
      data-min-size-type={typeof minSize}
      className={className}
    >
      {children}
    </div>
  ),
  Separator: ({ className }: { className?: string }) => (
    <div data-testid="panel-separator" role="separator" className={className} />
  ),
}));

// No msw in this repo — tests mock the API module directly (see
// AccessSection.test.tsx / EndpointDetail.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import { ApiError } from "@/api/client";
import { ParamLocation, ParamType, type DataApiEndpointDto, type DataApiServiceAccountDto } from "@/api/generated/model";
import type { EndpointDetailContext } from "@/routes/explore/dataapi/EndpointDetail";
import TryIt from "@/routes/explore/dataapi/detail/TryIt";
import { expectSectionIntroCapsItsProse, expectSectionLeavesWidthToThePage, sectionRootOf } from "./sectionWidth";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    statements: [{ name: "main", sql: "SELECT 1" }],
    params: [],
    output: {},
    cache: {},
    limits: {},
    rate_limit: {},
    source_kind: "sql",
    source_ref: null,
    live_version_id: "v-1",
    state: "live",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

function makeAccount(id: string, name: string): DataApiServiceAccountDto {
  return { id, name, description: "", email: `${id}@svc.local`, created_at: "2026-01-01T00:00:00Z" };
}

const SA_1 = makeAccount("sa-1", "reporting-bot");

/** A refused trial: the endpoint's OWN status (403) travels inside the `/try` route's 200 body. */
const REFUSED = {
  status: 403,
  body: { error: { code: "not_granted", message: "…" } },
  request: {
    method: "GET",
    url: "https://egret.test/data-api/v1/analytics/probe",
    authorization: "Bearer ••••",
    curl: "curl -H 'Authorization: Bearer $TOKEN' 'https://egret.test/data-api/v1/analytics/probe'",
  },
};

/** A successful trial. */
const OK = {
  status: 200,
  body: { rows: [{ id: 1 }], meta: { row_count: 1 } },
  request: {
    method: "GET",
    url: "https://egret.test/data-api/v1/analytics/probe",
    authorization: "Bearer ••••",
    curl: "curl -H 'Authorization: Bearer $TOKEN' 'https://egret.test/data-api/v1/analytics/probe'",
  },
};

function mockTry(result: unknown) {
  const mutateAsync = vi.fn().mockResolvedValue({ status: 200, data: result });
  vi.mocked(api.useDataApiTryEndpoint).mockReturnValue(
    { mutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiTryEndpoint>,
  );
  return mutateAsync;
}

function mockTryRejects(error: unknown) {
  const mutateAsync = vi.fn().mockRejectedValue(error);
  vi.mocked(api.useDataApiTryEndpoint).mockReturnValue(
    { mutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiTryEndpoint>,
  );
  return mutateAsync;
}

/**
 * Supplies the outlet context `useEndpointDetail()` reads, WITHOUT mounting
 * the real `EndpointDetail` shell. Mounting the shell would also mount its
 * section nav — which has its own permanent link literally named "Access" —
 * and that collides with `getByRole("link", { name: /access/i })` below the
 * moment this section adds its OWN "go fix it in Access" link, turning a
 * single, expected match into an ambiguous one. `TryIt` only ever reads
 * `ws`/`id`/`endpoint` off the context (per its own contract), so a partial
 * context is a faithful stand-in, not a shortcut that hides real behavior.
 */
function ContextProvider({ ctx }: { ctx: Partial<EndpointDetailContext> }) {
  return <Outlet context={ctx} />;
}

/**
 * `fieldset: true` reproduces the shell's `<fieldset disabled={trashed}>`
 * WITHOUT mounting `EndpointDetail` (whose nav collides with this section's
 * links — see `ContextProvider`). It exists for one class of test only: a
 * `<details>` is exempt from `fieldset[disabled]` and a Radix trigger is not,
 * and that difference is invisible unless a disabled fieldset is really in the
 * tree. Everything else renders WITHOUT one, so `toBeDisabled()` cannot pass by
 * walking up to an ancestor this section did not put there.
 */
function renderTryIt(
  opts: { endpoint?: DataApiEndpointDto; search?: string; trashed?: boolean; fieldset?: boolean } = {},
) {
  const endpoint = opts.endpoint ?? makeEndpoint();
  const search = opts.search ?? `?service_account=${SA_1.id}`;
  const ctx: Partial<EndpointDetailContext> = {
    ws: "eng",
    id: endpoint.id,
    endpoint,
    trashed: opts.trashed ?? false,
  };

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/try${search}`]}>
        <Routes>
          <Route path="/try" element={<ContextProvider ctx={ctx} />}>
            <Route index element={<TryIt />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  const utils = render(opts.fieldset ? <fieldset disabled>{tree}</fieldset> : tree);
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();
  // The shell reads the version history on EVERY tab now: the header names the
  // version a consumer is actually receiving, and says when the saved draft has
  // moved past it. Automocked, that hook hands back `undefined`, so this default
  // is what keeps the shell mountable at all here — tests that care about
  // versions override it afterwards.
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListEndpointVersions>,
  );

  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: [SA_1] }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
  // A DEFAULT for the trial mutation, so a test that never runs a trial still
  // gets a hook that answers.
  //
  // `vi.clearAllMocks()` above is `mockClear`, which resets call history and
  // KEEPS implementations — so before this line, four tests that call
  // `renderTryIt()` without `mockTry(...)` were rendering against a
  // `mockReturnValue` left behind by whichever earlier test happened to set
  // one. `TryIt` reads `isPending` on first paint, so with no leak they crash
  // rather than fail: measured, `mockReset: true` turns exactly those four into
  // `TypeError: Cannot read properties of undefined (reading 'isPending')`.
  //
  // Same defect as `EndpointDetail.test.tsx`'s width block (§5.6a), and the
  // same consequence: `.only`, `--shard`, or reordering this file breaks tests
  // that have nothing to do with what broke them. Defaulted here rather than
  // patched into the four, because the file already defaults the accounts hook
  // one line up — a default that covers every test cannot be outrun by a new
  // one.
  vi.mocked(api.useDataApiTryEndpoint).mockReturnValue(
    { mutateAsync: vi.fn().mockResolvedValue({ status: 200, data: OK }), isPending: false } as unknown as
      ReturnType<typeof api.useDataApiTryEndpoint>,
  );
});

describe("TryIt", () => {
  it("renders a 403 from the endpoint as a result, not as a failure of the trial", async () => {
    mockTry(REFUSED);
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText("403")).toBeInTheDocument();
    expect(screen.getByText(/not_granted/)).toBeInTheDocument();
    // The most common first failure names its fix.
    expect(screen.getByRole("link", { name: /access/i })).toBeInTheDocument();
  });

  it("never shows a token", async () => {
    mockTry(REFUSED);
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText(/Bearer ••••/)).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/eyJ/);
  });

  it("preselects the service account named by ?service_account=, per the Access section's contract", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ search: `?service_account=${SA_1.id}` });
    // Nothing to pick — the Select already carries the preselected id, so
    // Run fires immediately with it.
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ data: expect.objectContaining({ service_account_id: SA_1.id }) }),
    );
  });

  it("does not preselect when the URL carries no ?service_account=", async () => {
    mockTry(OK);
    renderTryIt({ search: "" });
    // Nothing preselected means Run has nothing to submit for — it stays
    // disabled rather than silently sending an empty account id.
    expect(screen.getByRole("button", { name: /run/i })).toBeDisabled();
  });

  it("renders one field per declared parameter", () => {
    mockTry(OK);
    renderTryIt({
      endpoint: makeEndpoint({
        params: [
          { name: "region", type: ParamType.string, location: ParamLocation.query, required: true },
          { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
        ],
      }),
    });
    expect(screen.getByLabelText(/region/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/limit/i)).toBeInTheDocument();
  });

  it("sends a filled parameter as a one-element list, and omits a blank one", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({
      endpoint: makeEndpoint({
        params: [
          { name: "region", type: ParamType.string, location: ParamLocation.query, required: false },
          { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
        ],
      }),
    });
    await userEvent.type(screen.getByLabelText(/region/i), "EU");
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ data: expect.objectContaining({ params: { region: ["EU"] } }) }),
    );
  });

  /**
   * Task 3 moved these behind the consequence that heads them, so this test
   * had to change — and the FIRST rewrite of it was wrong in the way that
   * matters. It clicked the summary and then asserted the five sentences were
   * in the document, which is the presence assertion this file was supposed to
   * stop making: mutating `Caveat` to a `<div>`/`<div>` (standing prose again,
   * `tsc` clean) killed 15 tests in this feature and left THIS one green.
   *
   * A click is not a reachability assertion. The observable is what the click
   * CHANGED — the `open` attribute — plus where the sentences live. Without
   * both, a `Caveat` swapped for a Radix collapsible with `forceMount` and CSS
   * hiding passes here while the copy is unreachable.
   */
  it("states the three consequences a trial has: rate limit, access log, and cache", () => {
    mockTry(OK);
    renderTryIt();

    const summary = screen.getByText("A trial is a real call, not a dry run");
    const details = summary.closest("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");

    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");

    // Inside the disclosure the click opened, and not in its `<summary>` —
    // a summary that swallowed the paragraph would hide nothing.
    for (const re of [
      /rate-limit bucket/i,
      /cannot throttle a live consumer/i,
      /access log/i,
      /Include trial runs/i,
      /per_principal/,
    ]) {
      const node = within(details!).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }
  });

  it("treats a 413 as 'too large to display', not a truncated answer", async () => {
    mockTryRejects(new ApiError(413, "payload too large"));
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText(/too large to display/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /activity/i })).toBeInTheDocument();
    // No stale/truncated result pane left over from a previous run.
    expect(screen.queryByText("Body")).not.toBeInTheDocument();
  });

  it("renders a successful trial's status, body, and echoed request", async () => {
    mockTry(OK);
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText("200")).toBeInTheDocument();
    expect(screen.getByText(/"row_count": 1/)).toBeInTheDocument();
    expect(screen.getByText(/GET https:\/\/egret\.test/)).toBeInTheDocument();
    expect(screen.getByText(OK.request.curl)).toBeInTheDocument();
  });
});

/**
 * Final-review finding (Important #4): the account picker collapsed `isError`
 * into the empty case. A failed fetch rendered "This workspace has no service
 * accounts yet. Create one…" while Run sat disabled for want of a selection —
 * a dead end with a false explanation, sending the author off to create an
 * account that already exists. Eight other surfaces in this feature separate
 * the two states; this one did not.
 */
describe("TryIt — the service-account picker distinguishes a failure from an empty workspace", () => {
  function mockAccounts(over: Record<string, unknown>) {
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
        ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
  }

  it("reports a failed fetch as a failure, never as 'no service accounts yet'", () => {
    mockAccounts({ isError: true, error: new Error("workspace not found") });
    renderTryIt();
    expect(screen.getByText(/could not load this workspace's service accounts/i)).toBeInTheDocument();
    // The empty-state copy is the specific lie this test exists to forbid.
    expect(screen.queryByText(/has no service accounts yet/i)).not.toBeInTheDocument();
  });

  it("surfaces the real error message, not only a generic sentence", () => {
    mockAccounts({ isError: true, error: new Error("workspace not found") });
    renderTryIt();
    expect(screen.getByText(/workspace not found/)).toBeInTheDocument();
  });

  it("still shows the create-one prompt when the workspace is genuinely empty", () => {
    mockAccounts({ data: { status: 200, data: [] } });
    renderTryIt();
    expect(screen.getByText(/has no service accounts yet/i)).toBeInTheDocument();
    expect(screen.queryByText(/could not load/i)).not.toBeInTheDocument();
  });

  /**
   * The prompt bound TWO verbs to ONE destination: "Create one, then grant it
   * invoke, in [Access]". Only the second is true of Access. A service account
   * is created at `/data-api/clients/new`, reached from Clients; `AccessSection`
   * has no create affordance at all — in this exact state its own table renders
   * the same "no service accounts yet" sentence with nothing to act on. So the
   * one link on screen pointed the reader at a page that would repeat the
   * problem back at them.
   *
   * The link now points at the only step available in this state. Access is
   * still named, in prose, because it is genuinely the next step — but it is
   * not a destination worth a click until an account exists.
   */
  it("points 'create one' at the service-account create page, not at Access", () => {
    mockAccounts({ data: { status: 200, data: [] } });
    renderTryIt();

    const prompt = screen.getByText(/has no service accounts yet/i).closest("p") as HTMLElement;
    const links = within(prompt).getAllByRole("link");
    // Exactly one, and it is the create step. Two links in one short sentence
    // is how the reader ends up on the dead end again.
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute("href", "/explore/eng/data-api/clients/new");
    // The href is the claim, but the words are what a reader reads: a link to
    // the create page labelled "Access" would satisfy the line above.
    expect(links[0].textContent).toMatch(/create/i);
    expect(links[0].textContent).not.toMatch(/access/i);
    // Access is still named — the second step is real and this is the only
    // place in this state that says it.
    expect(prompt.textContent).toMatch(/grant[\s\S]*invoke[\s\S]*Access/i);
  });
});

/**
 * A trial traverses the real serving router, which joins `endpoint_versions`
 * on `live_version_id` (`serving/router.rs`) — it runs the PUBLISHED contract.
 * The parameter fields, though, are built from `endpoint.params`, the DRAFT's
 * list. Rather than guard the section on `dirty` the way Publish is guarded
 * (which would take the tool away exactly while an author is iterating), the
 * section says so.
 */
describe("TryIt — says which version a trial runs", () => {
  it("states that draft edits are not part of what runs", () => {
    renderTryIt();
    expect(screen.getByText(/a trial runs the published version/i)).toBeInTheDocument();
  });
});

/** Two parameters, always. One row cannot tell "wrote the row it was typed in"
 *  apart from "wrote row 0" — see `docs/en/ui-conventions.md` §8. */
const TWO_PARAMS = [
  { name: "region", type: ParamType.string, location: ParamLocation.query, required: true },
  { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
];

/**
 * What the trial actually POSTs, not merely that something was POSTed.
 *
 * The pre-existing assertions were `expect.objectContaining`, which pins the
 * one field named and frees every other: `service_account_id` was unpinned in
 * the parameter tests and `params` unpinned in the account test, so a body that
 * dropped either would have passed. These deep-equal the WHOLE call.
 */
describe("TryIt — the payload a trial carries", () => {
  it("puts a typed value under the parameter whose row it was typed in", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // The SECOND row only. An implementation that writes `params[params[0].name]`
    // — or that reads the first input regardless of which one changed — sends
    // `{ region: ["10"] }` here and dies on the deep equality below. With a
    // one-parameter fixture those two implementations are indistinguishable.
    await userEvent.type(screen.getByLabelText(/^limit$/), "10");
    await userEvent.click(screen.getByRole("button", { name: /^run$/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync).toHaveBeenCalledTimes(1);
    expect(mutateAsync.mock.calls[0][0]).toEqual({
      workspace: "eng",
      id: "ep-1",
      // The untouched `region` is OMITTED, not sent as `[""]` — an unfilled
      // optional parameter must behave like it was never supplied.
      data: { service_account_id: SA_1.id, params: { limit: ["10"] } },
    });
  });

  it("keeps two filled parameters on their own keys rather than crossing them", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // Distinct values, so a swap is visible. Neither is a placeholder of the
    // field it is typed into (neither parameter declares a default), so no
    // constant in the component can satisfy this.
    await userEvent.type(screen.getByLabelText(/^region/), "EU");
    await userEvent.type(screen.getByLabelText(/^limit$/), "10");
    await userEvent.click(screen.getByRole("button", { name: /^run$/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data).toEqual({
      service_account_id: SA_1.id,
      params: { region: ["EU"], limit: ["10"] },
    });
  });
});

/**
 * **The split is gone, and this block is what says so.**
 *
 * `Try it` used to have `Query`'s shape: a vertical `Group` whose two panels
 * held the setup and the result, at a fixed `40%` / `60%`. A fraction cannot
 * size a part whose height is decided by its CONTENT, and measured in real
 * Chromium at 1600x1000 it never once did:
 *
 * | declared parameters | setup pane clientHeight | its scrollHeight | hidden |
 * |---|---|---|---|
 * | 0 | 230 | 277 | 47 |
 * | 1 | 230 | 297 | 67 |
 * | 2 | 230 | 342 | 112 |
 * | 4 | 230 | 432 | 202 |
 * | 8 | 230 | 612 | 382 |
 *
 * The FIRST row is the damning one: the pane clipped its own content on an
 * endpoint that declares no parameters at all, so there was no fixture for
 * which the fraction was right. Meanwhile the result pane below it measured
 * 389px and, before a run, held a header and one collapsed caveat.
 *
 * `Query` keeps its split for a reason that does not transfer. Its top half is
 * a Monaco editor, which must be HANDED a height — an auto-height editor is
 * 0px — and `Query.tsx` already states the general rule in its own comment:
 * "the strip can be expanded at any window height, so any fixed split has a
 * window that defeats it". `Try it`'s top half is a table; a table has an
 * intrinsic height and needs no fraction. Nor can the split be repaired in
 * place: `react-resizable-panels` v4 has no content-sizing mode (its
 * `defaultSize` takes %, px, em, rem, vh or vw — read off the shipped `.d.ts`),
 * so "size the top pane to its content" means measuring the content and
 * calling `panelRef.resize()` on mount and on every parameter change, which is
 * machinery whose only job is to imitate normal flow.
 *
 * So: one column. The setup takes its own height and never scrolls; the result
 * takes what is left and is the only part that does.
 *
 * **What jsdom can and cannot say about that.** It cannot say anything is
 * clipped: no layout, every box 0x0 (§10), so an assertion about the pixels
 * above passes with the split still in place. What it CAN pin is the chain
 * that produced them — an `overflow-auto` box between the parameter table and
 * the section root, and a `Panel` around it. The pixels are measured in
 * `tests/e2e/data-api-try-it-height.spec.ts`.
 */
describe("TryIt — one column: the setup at its own height, the result taking the rest", () => {
  /** The section's own root, by the same walk `sectionWidth.ts` uses. */
  function rootOf(container: HTMLElement) {
    return sectionRootOf(container, screen.getByRole("heading", { name: "Try it", level: 2 }));
  }

  /**
   * Every ancestor of `el` strictly between it and the section root.
   *
   * A WALK with a hard stop, never a counted hop: `parentElement.parentElement`
   * re-points at something else the moment a wrapper is added, and the trap
   * this branch already hit twice is that the re-pointed assertion still
   * passes. If the walk leaves the section without meeting the root, that is a
   * failure here rather than an empty chain nothing can be asserted about.
   */
  function ancestorsUpToRoot(container: HTMLElement, el: HTMLElement) {
    const root = rootOf(container);
    const chain: HTMLElement[] = [];
    let n: HTMLElement | null = el.parentElement;
    while (n && n !== root) {
      chain.push(n);
      n = n.parentElement;
    }
    expect(n, "the walk up from the parameter table reached the section root").toBe(root);
    // Premise: there IS a chain. An empty one makes the loop below vacuous.
    expect(chain.length).toBeGreaterThan(0);
    return chain;
  }

  /** The parameter table, resolved from its own heading rather than picked. */
  function paramTableOf() {
    const h4 = screen.getByRole("heading", { name: "Parameter values", level: 4 });
    const table = h4.parentElement!.querySelector("table");
    expect(table, "the Parameter values heading introduces a table").not.toBeNull();
    return table as HTMLTableElement;
  }

  /**
   * VERTICAL scrolling only — `overflow-auto` and `overflow-y-auto`, never
   * `overflow-x-auto`. That distinction is load-bearing rather than pedantic:
   * `ui/table.tsx` wraps every `<Table>` in `relative w-full overflow-x-auto`,
   * which is §5.3's deliberate answer to a wide settings grid on a narrow
   * screen. Catching it here would fail on a section with nothing wrong with
   * it, and the obvious "fix" would be to unwrap the tables. Same regex
   * `EndpointDetail.test.tsx` uses for the shell's single scroll region.
   */
  const SCROLLS = /(?:^|\s)(?:[a-z0-9]+:)*overflow(?:-y)?-(?:auto|scroll)(?:\s|$)/;
  const CAPPED_HEIGHT = /(?:^|\s)(?:[a-z0-9]+:)*max-h-/;

  function classOf(el: Element) {
    return el.getAttribute("class") ?? "";
  }

  it("puts nothing between the parameter table and the section root that can clip it", () => {
    mockTry(OK);
    const { container } = renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // Class names, not geometry — see the block comment. This is the exact
    // chain that hid 47-382px of the setup: a `min-h-0 flex-1 overflow-auto`
    // box, sized by a `Panel` to a fraction of the section.
    for (const a of ancestorsUpToRoot(container, paramTableOf())) {
      expect(classOf(a), `an ancestor of the parameter table scrolls: ${classOf(a)}`).not.toMatch(SCROLLS);
      expect(classOf(a), `an ancestor of the parameter table caps its height: ${classOf(a)}`).not.toMatch(CAPPED_HEIGHT);
    }
  });

  it("sizes the setup to its content and hands the remainder to the result", () => {
    mockTry(OK);
    const { container } = renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });
    const root = rootOf(container);

    const setup = screen.getByRole("region", { name: "Run as" });
    const result = screen.getByRole("region", { name: "Result" });

    // Both are the section's own children — nothing wraps them in a pane.
    expect(setup.parentElement).toBe(root);
    expect(result.parentElement).toBe(root);

    // `shrink-0` is the half that regresses silently: a flex item shrinks by
    // default, so without it a tall result squeezes the setup back to the
    // clipped state this task removed. And the setup must NOT be flexible —
    // taking a share of the height is precisely what it stopped doing.
    expect(setup).toHaveClass("shrink-0");
    expect(classOf(setup)).not.toMatch(/(?:^|\s)(?:[a-z0-9]+:)*flex-1(?:\s|$)/);

    // The result is the flexible one, with a floor so that a setup taller than
    // the section cannot squeeze it to nothing (the whole section overflows the
    // shell's scroll box instead, which is reachable; a squeezed panel is not).
    expect(result).toHaveClass("flex-1");
    expect(classOf(result)).toMatch(/(?:^|\s)min-h-\S+/);
  });

  it("keeps every scrolling box inside the result, and none around the setup", () => {
    mockTry(OK);
    const { container } = renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });
    const root = rootOf(container);
    const setup = screen.getByRole("region", { name: "Run as" });
    const result = screen.getByRole("region", { name: "Result" });

    const scrollers = [...root.querySelectorAll("*")].filter((el) => SCROLLS.test(classOf(el)));
    // Premise, not decoration: with no scrolling box anywhere the loop below
    // asserts nothing and reports green — the vacuous pass this branch's specs
    // have hit before.
    expect(scrollers.length, "the section still has a scrolling result body").toBeGreaterThan(0);
    for (const el of scrollers) {
      expect(result.contains(el), `a scroll box outside the result: ${classOf(el)}`).toBe(true);
      expect(setup.contains(el), `a scroll box around the setup: ${classOf(el)}`).toBe(false);
    }
  });

  /**
   * **The property the split had for free, and that the flex column has to earn
   * back.** A `40%` / `60%` pane always put `Run` on screen, at every parameter
   * count, because the panes could not exceed the section. A column that lets
   * the section overflow the shell's scroll box can push the result's header —
   * `Run`, the status badge, "Running…" — below the fold: measured, the section
   * totals `setup + max(256, 715 - setup)` against a 715px box, so at 8
   * declared parameters the header starts 5px under the fold and at 16 it is
   * ~355px under it.
   *
   * That is not a cosmetic loss. The shortcut is bound to `window` (§11.1), so
   * ⌘↵ fires wherever the page is scrolled. With the result below the fold a
   * real trial runs — a real serving-path call, a rate-limit token, an
   * `access_log` row — and NOTHING on screen changes. The reader presses it
   * again.
   *
   * **`sticky top-0` was the obvious fix and it is measurably a no-op here.**
   * Built, then measured in Chromium: at 8 and 16 parameters, at four scroll
   * offsets each, the header's top equalled the result section's top every
   * time. Sticky engages only when its containing block is scrolled past its
   * own top, and this section is `flex-1` in an `h-full` column — its height is
   * `max(256, available − setup)`, never more than the box, so it can never be
   * scrolled past. It was removed rather than shipped: a class assertion
   * passing over an inert class is the exact failure this branch keeps finding.
   *
   * What works, measured the same way: `onRun` scrolls the result into view.
   * From `scrollTop: 0`, ⌘↵ at 6 / 8 / 16 parameters leaves `Run` inside the
   * shell's visible box in all three, where at rest 8 and 16 had it outside.
   *
   * jsdom judges no pixel of that. What it judges is the IDENTITY of the
   * element the product scrolled to, and that the scroll sits behind the guard.
   * The geometry is `tests/e2e/data-api-try-it-height.spec.ts`.
   */
  it("brings the result into view when ⌘↵ starts a run, so a trial cannot fire invisibly", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // BY IDENTITY, not "was it called". The prototype mock exists so nothing
    // throws (jsdom implements no `scrollIntoView`) AND so it can be asserted
    // NOT to have fired: an own property on the result element shadows the
    // prototype, so `onResult` recording the call while `onAnythingElse` stays
    // at zero is the proof that the product scrolled to the RESULT and not to
    // some other node that happened to be handy.
    const onAnythingElse = vi.fn();
    Element.prototype.scrollIntoView = onAnythingElse;
    const onResult = vi.fn();
    const result = screen.getByRole("region", { name: "Result" });
    (result as unknown as { scrollIntoView: () => void }).scrollIntoView = onResult;

    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(onResult).toHaveBeenCalled();
    expect(onAnythingElse).not.toHaveBeenCalled();
  });

  it("does not scroll for a run it refuses", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ trashed: true });

    const onAnythingElse = vi.fn();
    Element.prototype.scrollIntoView = onAnythingElse;
    const onResult = vi.fn();
    const result = screen.getByRole("region", { name: "Result" });
    (result as unknown as { scrollIntoView: () => void }).scrollIntoView = onResult;

    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    // The scroll must sit BEHIND the `canRun` guard. In front of it, a trashed
    // endpoint — or a held shortcut during a pending run — yanks the page
    // toward a result that is not going to change.
    await waitFor(() => expect(mutateAsync).not.toHaveBeenCalled());
    expect(onResult).not.toHaveBeenCalled();
    expect(onAnythingElse).not.toHaveBeenCalled();
  });

  it("renders no resizable split", () => {
    mockTry(OK);
    renderTryIt();
    // Matched against the STUB at the top of this file, which marks anything
    // the library renders. Re-import `Group`/`Panel` here and these populate.
    expect(screen.queryByTestId("panel-group")).toBeNull();
    expect(screen.queryAllByTestId("panel")).toHaveLength(0);
    expect(screen.queryByTestId("panel-separator")).toBeNull();
  });

  it("names the shortcut in the result's header, beside the button it fires", () => {
    mockTry(OK);
    renderTryIt();
    const result = screen.getByRole("region", { name: "Result" });
    // An affordance nobody can see is not an affordance, and the only place a
    // reader looks for it is next to Run.
    expect(within(result).getByText("⌘↵")).toBeInTheDocument();
    expect(within(result).getByRole("button", { name: /^run$/i })).toBeInTheDocument();
    // Run belongs to the result half, as Query's Preview button does — not to
    // the setup half where the picker is.
    const setup = screen.getByRole("region", { name: "Run as" });
    expect(within(setup).getByLabelText("Service account")).toBeInTheDocument();
    expect(within(setup).queryByRole("button", { name: /^run$/i })).toBeNull();
  });

  it("runs the trial from ⌘↵, carrying the values on screen", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    await userEvent.type(screen.getByLabelText(/^region/), "EU");
    // The listener is on `window`, so this is the event the product receives —
    // no button is clicked here.
    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    // The CURRENT form, not the first render's: a `[]`-dep listener closes over
    // the initial `onRun` and posts an empty `params` with an empty account id.
    expect(mutateAsync.mock.calls[0][0].data).toEqual({
      service_account_id: SA_1.id,
      params: { region: ["EU"] },
    });
  });

  it("does not run from ⌘↵ when no service account is chosen", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ search: "" });
    expect(screen.getByRole("button", { name: /^run$/i })).toBeDisabled();

    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    // The shortcut bypasses the button, so the button's `disabled` is not the
    // guard — without the check in `onRun` this posts `service_account_id: ""`
    // and the serving path answers a bare 422.
    await waitFor(() => expect(mutateAsync).not.toHaveBeenCalled());
  });

  /**
   * The shell wraps every section in `<fieldset disabled={trashed}>`, which
   * greys the Run button — and a window key handler is not a form control, so
   * it walks straight past it. Before this section had a shortcut, `trashed`
   * did not need reading here at all; adding one without this guard would have
   * made a trashed endpoint runnable by keyboard only.
   */
  it("runs nothing for a trashed endpoint, by shortcut or by button", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ trashed: true });

    expect(screen.getByRole("button", { name: /^run$/i })).toBeDisabled();
    fireEvent.keyDown(window, { key: "Enter", metaKey: true });
    await waitFor(() => expect(mutateAsync).not.toHaveBeenCalled());
  });

  it("keeps what a trial costs beside the button that spends it", () => {
    mockTry(OK);
    renderTryIt();
    const result = screen.getByRole("region", { name: "Result" });
    const setup = screen.getByRole("region", { name: "Run as" });
    // `PreviewBlock`'s rule: a reader who has to scroll to find out that this
    // spends a rate limit and writes an access-log row has already pressed the
    // button. jsdom cannot judge that the note is on screen — it can judge that
    // it is in the same half as Run, which is the wiring the rule reduces to.
    //
    // The assertion is on the HEADLINE (the half that is on screen unclicked)
    // plus the mechanism behind it. Asserting the collapsed text alone would
    // say nothing about which half a reader finds it in.
    const summary = within(result).getByText("A trial is a real call, not a dry run");
    expect(summary.closest("details")).toHaveTextContent(/rate-limit bucket/i);
    expect(within(setup).queryByText(/rate-limit bucket/i)).toBeNull();
  });
});

/** §4: settings are `Setting | Details | Value` rows; list-shaped content is a
 *  plain table. This section was eight stacked `Card`s and is now neither. */
describe("TryIt — the setup reads as tables, not as cards", () => {
  it("puts the service-account picker in a config table row rather than loose above the form", () => {
    mockTry(OK);
    renderTryIt();

    const trigger = screen.getByLabelText("Service account");
    const cell = trigger.closest("td");
    expect(cell).not.toBeNull();
    const row = cell!.closest("tr")!;
    // The label is the row's Setting cell, not a `<Label>` floating above a card.
    expect(within(row).getByText("Service account")).toBeInTheDocument();
  });

  it("gives each declared parameter one row of a plain table, with its type", () => {
    mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    const regionRow = screen.getByLabelText(/^region/).closest("tr")!;
    const limitRow = screen.getByLabelText(/^limit$/).closest("tr")!;
    // Two rows, and DIFFERENT ones — one row cannot tell a per-row renderer
    // apart from one that renders row 0 twice.
    expect(regionRow).not.toBe(limitRow);
    expect(within(regionRow).getByText("string")).toBeInTheDocument();
    expect(within(limitRow).getByText("number")).toBeInTheDocument();
    // The required marker belongs to the required one, and only to it.
    expect(within(regionRow).getByText("*")).toBeInTheDocument();
    expect(within(limitRow).queryByText("*")).toBeNull();
  });

  it("says so inside the table when the endpoint declares no parameters", () => {
    mockTry(OK);
    renderTryIt();
    const empty = screen.getByText(/declares no parameters/i);
    // In the table's own body, spanning it, rather than a sentence that reads as
    // a value in the Parameter column.
    expect(empty.closest("td")).toHaveAttribute("colspan", "3");
  });
});

/**
 * The one thing this section exists to say. A trial runs as the chosen SERVICE
 * ACCOUNT; `Query`'s preview runs as the AUTHOR. Measured live on kind: the same
 * endpoint and the same SQL answered `who=svc-kind-smoke-bot` from a trial and
 * `who=admin` from a preview. `PreviewBlock` carries the claim from the other
 * side; these are the three places on this side.
 */
describe("TryIt — says whose identity a trial runs as", () => {
  it("says it in the section description, in the picker's help, and beside Run", () => {
    mockTry(OK);
    renderTryIt();

    expect(screen.getByText(/as a chosen service account/i)).toBeInTheDocument();
    // The second and third places are behind their own disclosures now (Task
    // 3), so this OPENS them: text found inside a closed `<details>` proves the
    // sentence is in the DOM, not that a reader can reach it.
    const picker = screen.getByText("A green preview does not prove a consumer can call this endpoint");
    fireEvent.click(picker);
    expect(picker.closest("details")).toHaveAttribute("open");
    expect(within(picker.closest("details")!).getByText(/Preview in Query runs as you/i)).toBeInTheDocument();

    const run = screen.getByText("A trial is a real call, not a dry run");
    fireEvent.click(run);
    expect(run.closest("details")).toHaveAttribute("open");
    expect(run.closest("details")).toHaveTextContent(/runs as the service account above rather than as you/i);
  });
});

/**
 * REVERSED. This test used to say `Try it` caps its own width at `max-w-5xl`,
 * on `docs/en/ui-conventions.md` §5.1's two-group rule — and the guard existed
 * because until it was written nothing held that cap in place at all, the same
 * hole §5.5 records for `Config`, where restoring `max-w-3xl` to the section's
 * own root passed 230/230.
 *
 * §5.1 was reversed: one width for all six tabs. Measured on the deployed
 * build at 1600×1000, the content box is 1296px on every tab while the widest
 * block was 1024 here and 1296 on Query/Config/Response — a 272px jump on a
 * tab change. The per-section reading argument was not wrong about this tab;
 * it lost to the fact that a reader sees the six in sequence.
 *
 * The walk lives in `sectionWidth.ts` rather than being written a fourth time.
 * It resolves the host, so it works both under the real `EndpointDetail` shell
 * (`AccessSection.test.tsx`, `EndpointDetail.test.tsx`) and under this file's
 * stubbed-context harness, where the section's root is a child of the render
 * container itself.
 *
 * A class name, not a layout: jsdom has no CSS (§10). Whether the split reads
 * well at that width is the browser pass's question — and in #529 the browser
 * was what caught an editor squeezed to 107px that every test had passed.
 */
describe("TryIt — width", () => {

  /**
   * §5.1's intro rule, pinned where the class actually is.
   *
   * The class assertion is the RIGHT instrument here rather than a stand-in for
   * a measurement — see `expectSectionIntroCapsItsProse`. Before this test the
   * only guard on this cap was a Playwright spec that has never executed, and a
   * population count that `Response` and `Access` could satisfy without their
   * intro being capped at all.
   */
  it("caps its intro at a reading measure, not the section's (§5.1)", () => {
    mockTry(OK);
    renderTryIt();
    expectSectionIntroCapsItsProse(screen.getByRole("heading", { name: "Try it", level: 2 }));
  });

  it("leaves width to the page, like all six tabs (§5.1, as reversed)", () => {
    mockTry(OK);
    const { container } = renderTryIt();
    const heading = screen.getByRole("heading", { name: "Try it", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
  });

  /**
   * §5.2, and the debt §5.1 recorded as OWED by this exact section.
   *
   * Removing the cap above is only half a change: `ConfigTable` is
   * `table-fixed`, so every pixel the section gained went to `Details` — the
   * only unsized column — while the picker stayed where it was. Measured in
   * Chromium at 1600x1000 with the cap gone and this table still at
   * `w-52 lg:w-80`: `Setting` 288, `Details` **680**, `Value` **320**, and the
   * sentence inside `Details` laid out at 532px (its own `max-w-prose`
   * measure), leaving **148px of that column dead**.
   *
   * At `w-52 lg:w-96` the same measurement gives 288 / 616 / **384**, the
   * picker 368px wide, and the dead space in `Details` down to 84px — and it
   * is the SAME measure `Config` and `Response` carry, which is the point:
   * three adjacent tabs render this identical three-column grid, and §5.1's
   * whole finding is that a reader sees the tabs in sequence.
   *
   * A wider column measures better in isolation and is deliberately not taken:
   * at 448px the caveat's summary drops from two lines to one (row 85 -> 69px)
   * and `Details`'s dead space falls to 20px, but that is a fourth width for
   * one grid, and at a 1024px viewport — where `lg:` has just engaged —
   * `Details` is already down to 40px at `w-96` and would be crushed. The
   * numbers are in the task report.
   *
   * Both halves asserted, as `Config`'s and `Response`'s guards do: below `lg`
   * the sized columns do not shrink, they scroll (§5.3), so a flat `w-96` is a
   * different and worse thing than `w-52 lg:w-96`.
   */
  it("gives its Value column the same measure as Config's and Response's (§5.2)", () => {
    mockTry(OK);
    renderTryIt();
    // BY IDENTITY. This section renders TWO tables with a "Value" column — the
    // `Setting | Details | Value` settings grid and the `Parameter | Type |
    // Value` list — so a bare `getByRole("columnheader", { name: "Value" })` is
    // ambiguous, and picking `[0]` out of `getAllBy` would silently follow
    // whichever table happens to come first. The one this rule is about is the
    // `ConfigTable`, resolved as the table that also has a "Setting" column.
    const settingsTable = screen.getByRole("columnheader", { name: "Setting" }).closest("table")!;
    const valueHead = within(settingsTable).getByRole("columnheader", { name: "Value" });
    expect(valueHead).toHaveClass("lg:w-96");
    expect(valueHead).toHaveClass("w-52");
  });
});

/**
 * Task 3 — prose moves beside the control it describes.
 *
 * The audit measured ~950 characters of standing prose on this screen against
 * one dropdown and one button. Nothing here was deleted: the rule applied is
 * `docs/en/ui-conventions.md` §4.4 — **the consequence in the summary, the
 * reason behind it** — so every sentence is still on the page and every one of
 * them is still reachable without leaving it.
 *
 * These tests assert REACHABILITY, not presence. jsdom does not hide a closed
 * `<details>` (§11.3), so `getByText` finds a sentence a user cannot see; the
 * only observable that distinguishes the two is the `open` attribute plus the
 * activation that sets it.
 */
describe("TryIt — the prose sits beside the control it describes", () => {
  /** The `<details>` that contains `text`, asserted to be genuinely collapsed. */
  function collapsedDisclosureAround(el: HTMLElement): HTMLDetailsElement {
    const details = el.closest("details") as HTMLDetailsElement | null;
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
    return details!;
  }

  it("keeps the Service account Details cell to one sentence and the rest beside the picker", () => {
    mockTry(OK);
    renderTryIt();

    // The Details cell itself. `ConfigTableRow` renders its help text TWICE —
    // once in the Details column at `lg`, once under the label below it — so
    // this counts both rather than asserting a single node.
    const kept = screen.getAllByText("The identity the trial authenticates as.");
    expect(kept.length).toBeGreaterThan(0);
    // The rule §4.4 caps at about two lines. The failing shape is the 314-char
    // literal this replaced, so assert the CELL is short rather than that some
    // node somewhere holds the short sentence.
    for (const node of kept) {
      expect(node.textContent!.length).toBeLessThan(120);
    }

    // …and the two sentences that were cut from it are still here, behind a
    // disclosure whose summary carries the consequence.
    const summary = screen.getByText("A green preview does not prove a consumer can call this endpoint");
    const details = collapsedDisclosureAround(summary);
    const body = within(details).getByText(/60-second token is minted/i);
    expect(body.closest("summary")).toBeNull();
    expect(body).toHaveTextContent(/Preview in Query runs as you instead/i);

    // Reachable: the gesture a user makes opens it.
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  it("puts the trial's consequences behind the consequence, in the half that holds Run", () => {
    mockTry(OK);
    renderTryIt();

    const resultPanel = screen.getByRole("region", { name: "Result" });
    // The load-bearing half, on screen with nothing clicked. `PreviewBlock`'s
    // rule survives the move: a reader who must open something to learn a call
    // is real has already pressed the button, so THAT sentence is the summary.
    const summary = within(resultPanel).getByText("A trial is a real call, not a dry run");
    const details = collapsedDisclosureAround(summary);
    expect(within(resultPanel).getByRole("button", { name: /^run$/i })).toBeInTheDocument();

    // Every consequence still reachable, none of it standing.
    for (const re of [/rate-limit bucket/i, /cannot throttle a live consumer/i, /access log/i, /Include trial runs/i, /per_principal/]) {
      const node = within(details).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }

    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  /**
   * §11.2's trap, on this section for the first time. The shell wraps the body
   * in `<fieldset disabled={trashed}>`; a Radix `Collapsible` trigger is a
   * `<button>` and would be dead there, and every text assertion above would
   * still pass because jsdom does not hide a closed disclosure either way.
   *
   * `fireEvent`, not `userEvent`: `userEvent.click` refuses a click under a
   * disabled fieldset even though `fieldset[disabled]` disables only
   * form-associated descendants and `<summary>` is not one. The failure mode is
   * a FALSE NEGATIVE.
   */
  it("keeps both disclosures openable inside a disabled fieldset", () => {
    mockTry(OK);
    renderTryIt({ trashed: true, fieldset: true });

    for (const label of [
      "A green preview does not prove a consumer can call this endpoint",
      "A trial is a real call, not a dry run",
    ]) {
      const summary = screen.getByText(label);
      // Guard the premise: the fieldset really is disabled, so this is a test
      // about surviving it and not one where nothing was disabled.
      const fieldset = summary.closest("fieldset");
      expect(fieldset).not.toBeNull();
      expect(fieldset).toBeDisabled();

      const details = summary.closest("details")!;
      // A `ui/collapsible` renders a `<button>` here, which the fieldset kills.
      expect(within(details).queryByRole("button")).not.toBeInTheDocument();
      expect(details).not.toHaveAttribute("open");
      fireEvent.click(summary);
      expect(details).toHaveAttribute("open");
    }
  });

  /**
   * The counter-rule, and the reason this section did not collapse everything.
   * §11.2: collapse a caveat because it is BIG, not because its neighbour is.
   * Two lines with no explanation to hold costs a click to reveal nothing.
   */
  it("leaves the short published-version note as plain text, not as an empty disclosure", () => {
    mockTry(OK);
    renderTryIt();
    const note = screen.getByText(/a trial runs the published version/i);
    expect(note.closest("details")).toBeNull();
  });
});
