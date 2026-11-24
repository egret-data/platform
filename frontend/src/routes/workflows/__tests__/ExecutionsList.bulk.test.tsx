import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ExecutionListItemDto } from "@/api/generated/model";
import { TooltipProvider } from "@/components/ui/tooltip";
import ExecutionsList from "@/routes/workflows/ExecutionsList";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const WS = "eng";
const LIST = `/api/ws/${WS}/workflows/executions`;

function exec(id: string, over: Partial<ExecutionListItemDto> = {}): ExecutionListItemDto {
  return {
    id,
    flow_id: "0a7c1d2e-0000-4000-8000-000000000001",
    namespace: "company",
    flow_key: "hello",
    state: "SUCCESS",
    history: [],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    attempt_number: 1,
    origin_offset_ms: 0,
    started_at: "2026-10-11T10:00:05.000001Z",
    ended_at: "2026-10-11T10:01:10.000001Z",
    created_at: "2026-10-11T10:00:00.000001Z",
    updated_at: "2026-10-11T10:01:10.000001Z",
    ...over,
  };
}

const OK = "aaaaaaaa-0000-4000-8000-000000000001";
const BAD = "bbbbbbbb-0000-4000-8000-000000000002";
const RUN = "cccccccc-0000-4000-8000-000000000003";
const ROWS = [
  exec(OK),
  exec(BAD, { state: "FAILED" }),
  exec(RUN, { state: "RUNNING", started_at: "2026-10-11T10:00:05.000001Z", ended_at: null }),
];

interface Call {
  method: string;
  path: string;
  search: URLSearchParams;
  body: unknown;
}
type Reply = { status: number; body: unknown };

function serve(total = ROWS.length, bulk: (c: Call) => Reply = () => ({ status: 200, body: { count: 1 } }), rows = ROWS) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = new URL(String(input), "http://x");
      let reply: Reply;
      if (url.pathname === LIST) {
        reply = { status: 200, body: { rows, total, limit: 50, offset: 0, worker_enabled: true } };
      } else if (url.pathname.startsWith(`${LIST}/namespaces`)) {
        reply = { status: 200, body: [] };
      } else if (url.pathname.includes("/by-ids") || url.pathname.includes("/by-query")) {
        const call: Call = {
          method: init?.method ?? "GET",
          path: url.pathname.slice(LIST.length),
          search: url.searchParams,
          body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
        };
        calls.push(call);
        reply = bulk(call);
      } else {
        reply = { status: 404, body: { error: `unexpected ${url.pathname}` } };
      }
      return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
    }),
  );
  return calls;
}

function renderList(at = `/explore/${WS}/workflows/executions`) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[at]}>
          <Routes>
            <Route path="/explore/:workspace/workflows/executions" element={<ExecutionsList />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const rowBox = (id: string) => screen.getByRole("checkbox", { name: `Select execution ${id.slice(0, 8)}` });
const toolbar = () => screen.getByRole("toolbar", { name: "Bulk actions" });
const action = (name: string) => within(toolbar()).getByRole("button", { name });

afterEach(() => {
  vi.unstubAllGlobals();
  toast.success.mockClear();
  toast.error.mockClear();
});

describe("row selection", () => {
  it("has a checkbox per row and a header checkbox that selects the page, and no toolbar until a row is chosen", async () => {
    serve();
    renderList();
    await screen.findByTestId("executions-table");
    expect(screen.queryByRole("toolbar", { name: "Bulk actions" })).toBeNull();
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    expect(within(toolbar()).getByText("1 selected")).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
    expect(within(toolbar()).getByText("3 selected")).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
    expect(screen.queryByRole("toolbar", { name: "Bulk actions" })).toBeNull();
  });
});

describe("bulk toolbar", () => {
  it("offers the eight actions, each enabled only when the whole selection is eligible", async () => {
    serve();
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(BAD)); // FAILED
    for (const n of ["Restart", "Kill", "Delete", "Change status", "Replay", "Resume", "Pause", "Edit labels"]) {
      expect(action(n)).toBeInTheDocument();
    }
    expect(action("Restart")).toBeEnabled();
    expect(action("Delete")).toBeEnabled();
    expect(action("Kill")).toBeDisabled();
    expect(action("Pause")).toBeDisabled();
    expect(action("Resume")).toBeDisabled();
    await user.click(rowBox(RUN)); // RUNNING joins the selection
    expect(action("Restart")).toBeDisabled();
    expect(action("Delete")).toBeDisabled();
  });

  it("restarts the chosen ids through restart/by-ids and says how many", async () => {
    const calls = serve(3, () => ({ status: 200, body: { count: 1 } }));
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(BAD));
    await user.click(action("Restart"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/restart/by-ids");
    expect(calls[0].body).toEqual({ ids: [BAD] });
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith(expect.stringContaining("1 execution")));
    await waitFor(() => expect(screen.queryByRole("toolbar", { name: "Bulk actions" })).toBeNull());
  });
});

describe("select all matching the filter", () => {
  it("switches the action to by-query, sending the filter and no ids", async () => {
    const calls = serve(120, () => ({ status: 200, body: { count: 120 } }));
    renderList(`/explore/${WS}/workflows/executions?state=FAILED&namespace=company`);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
    await user.click(screen.getByRole("button", { name: "Select all 120 matching this filter" }));
    expect(within(toolbar()).getByText("120 selected")).toBeInTheDocument();
    // Restart is offered for the whole match: the server checks the rows the page cannot show.
    await user.click(action("Restart"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/restart/by-query");
    expect(calls[0].search.get("state")).toBe("FAILED");
    expect(calls[0].search.get("namespace")).toBe("company");
    expect(calls[0].search.has("limit")).toBe(false);
    expect(calls[0].body).toBeUndefined();
  });
});

describe("destructive confirmation and errors", () => {
  it("asks first for Delete, with the count, and acts only after the confirm", async () => {
    const calls = serve(3, () => ({ status: 200, body: { count: 2 } }));
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    await user.click(action("Delete"));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Delete 1 execution\?/)).toBeInTheDocument();
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].method).toBe("DELETE");
    expect(calls[0].path).toBe("/by-ids");
    expect(calls[0].body).toEqual({ ids: [OK] });
  });

  it("asks first for Kill", async () => {
    const calls = serve();
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(RUN));
    await user.click(action("Kill"));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Kill 1 execution\?/)).toBeInTheDocument();
    expect(calls).toHaveLength(0);
  });

  it("on a 400 BulkErrorResponse lists the refused ids and why, and says nothing was applied", async () => {
    serve(3, () => ({
      status: 400,
      body: { message: "1 execution cannot be deleted", invalids: [{ id: OK, reason: "its afterExecution tasks are still running" }] },
    }));
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    await user.click(action("Delete"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText(/Nothing was changed/)).toBeInTheDocument();
    expect(within(dialog).getByText(OK)).toBeInTheDocument();
    expect(within(dialog).getByText("its afterExecution tasks are still running")).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
    // The selection survives, so the user can fix it.
    const hiddenToolbar = screen.getByRole("toolbar", { name: "Bulk actions", hidden: true });
    expect(within(hiddenToolbar).getByText("1 selected")).toBeInTheDocument();
  });
});

const QUEUED = "eeeeeeee-0000-4000-8000-000000000005";

describe("bulk Unqueue and Force run (package 8b)", () => {
  const rows = [...ROWS, exec(QUEUED, { state: "QUEUED", ended_at: null })];

  it("are enabled only when every selected execution is QUEUED", async () => {
    serve(4, undefined, rows);
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(QUEUED));
    expect(action("Unqueue")).toBeEnabled();
    expect(action("Force run")).toBeEnabled();
    await user.click(rowBox(RUN));
    expect(action("Unqueue")).toBeDisabled();
    expect(action("Force run")).toBeDisabled();
  });

  it("by ids: unqueue sends state=CANCELLED, force run sends {ids}, each after a confirm", async () => {
    const calls = serve(4, undefined, rows);
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(QUEUED));
    await user.click(action("Unqueue"));
    let dialog = await screen.findByRole("alertdialog");
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Unqueue" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/unqueue/by-ids");
    expect(calls[0].search.get("state")).toBe("CANCELLED");
    expect(calls[0].body).toEqual({ ids: [QUEUED] });

    await user.click(rowBox(QUEUED));
    await user.click(action("Force run"));
    dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Force run" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].path).toBe("/force-run/by-ids");
    expect(calls[1].body).toEqual({ ids: [QUEUED] });
  });

  it("select all: unqueue sends the filter and target_state, force run sends the filter", async () => {
    const calls = serve(120, undefined, rows);
    renderList(`/explore/${WS}/workflows/executions?state=QUEUED&namespace=company`);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
    await user.click(screen.getByRole("button", { name: "Select all 120 matching this filter" }));
    await user.click(action("Unqueue"));
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Unqueue" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/unqueue/by-query");
    expect(calls[0].search.get("target_state")).toBe("CANCELLED");
    expect(calls[0].search.get("state")).toBe("QUEUED");
    expect(calls[0].body).toBeUndefined();

    await selectAllMatching(user, 120);
    await user.click(action("Force run"));
    dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Force run" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].path).toBe("/force-run/by-query");
    expect(calls[1].search.get("namespace")).toBe("company");
    expect(calls[1].body).toBeUndefined();
  });
});

const PAUSED = "dddddddd-0000-4000-8000-000000000004";

/** With the whole page chosen, widens the action to every execution the filter matches. */
async function selectAllMatching(user: ReturnType<typeof userEvent.setup>, total: number) {
  await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
  await user.click(screen.getByRole("button", { name: `Select all ${total} matching this filter` }));
}

const FILTER = `/explore/${WS}/workflows/executions?state=FAILED&namespace=company`;
const expectFilter = (c: Call) => {
  expect(c.search.get("state")).toBe("FAILED");
  expect(c.search.get("namespace")).toBe("company");
  expect(c.search.has("limit")).toBe(false);
  expect(c.search.has("offset")).toBe(false);
};

describe("each operation's request shape", () => {
  it("change status: by ids sends {ids, state}; by query sends the filter and {state} with no ids", async () => {
    const calls = serve(120);
    renderList(FILTER);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    await user.click(action("Change status"));
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("combobox", { name: "New status" }));
    await user.click(await screen.findByRole("option", { name: "Failed" }));
    await user.click(within(dialog).getByRole("button", { name: "Change status" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/change-status/by-ids");
    expect(calls[0].body).toEqual({ ids: [OK], state: "FAILED" });

    await selectAllMatching(user, 120);
    await user.click(action("Change status"));
    dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("combobox", { name: "New status" }));
    await user.click(await screen.findByRole("option", { name: "Success" }));
    await user.click(within(dialog).getByRole("button", { name: "Change status" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].path).toBe("/change-status/by-query");
    expect(calls[1].body).toEqual({ state: "SUCCESS" });
    expectFilter(calls[1]);
  });

  it("edit labels: by ids sends {ids, labels}; by query sends the filter and the labels array", async () => {
    const calls = serve(120);
    renderList(FILTER);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    const fill = async () => {
      const dialog = await screen.findByRole("alertdialog");
      await user.type(within(dialog).getByRole("textbox", { name: "Label 1 key" }), "team");
      await user.type(within(dialog).getByRole("textbox", { name: "Label 1 value" }), "data");
      await user.click(within(dialog).getByRole("button", { name: "Edit labels" }));
    };
    await user.click(rowBox(OK));
    await user.click(action("Edit labels"));
    await fill();
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/labels/by-ids");
    expect(calls[0].body).toEqual({ ids: [OK], labels: [{ key: "team", value: "data" }] });

    await selectAllMatching(user, 120);
    await user.click(action("Edit labels"));
    await fill();
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].path).toBe("/labels/by-query");
    expect(calls[1].body).toEqual([{ key: "team", value: "data" }]);
    expectFilter(calls[1]);
  });

  it("resume: by ids sends {ids} or {ids, inputs}; by query sends the filter and {} or {inputs}", async () => {
    const calls = serve(120, undefined, [...ROWS, exec(PAUSED, { state: "PAUSED" })]);
    renderList(FILTER);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(PAUSED));
    await user.click(action("Resume"));
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].path).toBe("/resume/by-ids");
    expect(calls[0].body).toEqual({ ids: [PAUSED] });

    await user.click(rowBox(PAUSED));
    await user.click(action("Resume"));
    dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox", { name: /Inputs/ }), '{{"who":"me"}');
    await user.click(within(dialog).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].body).toEqual({ ids: [PAUSED], inputs: { who: "me" } });

    await selectAllMatching(user, 120);
    await user.click(action("Resume"));
    dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox", { name: /Inputs/ }), '{{"who":"me"}');
    await user.click(within(dialog).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(calls).toHaveLength(3));
    expect(calls[2].path).toBe("/resume/by-query");
    expect(calls[2].body).toEqual({ inputs: { who: "me" } });
    expectFilter(calls[2]);
  });

  it("replay by query sends the filter, with no ids and no body", async () => {
    const calls = serve(120);
    renderList(FILTER);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await selectAllMatching(user, 120);
    await user.click(action("Replay"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].method).toBe("POST");
    expect(calls[0].path).toBe("/replay/by-query");
    expect(calls[0].body).toBeUndefined();
    expectFilter(calls[0]);
  });

  it("delete sends deleteStorage, by ids and by query; by query sends the filter and no ids", async () => {
    const calls = serve(120);
    renderList(FILTER);
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    await user.click(action("Delete"));
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].search.get("deleteStorage")).toBe("true");

    await selectAllMatching(user, 120);
    await user.click(action("Delete"));
    dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("checkbox", { name: "Delete stored files" }));
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].method).toBe("DELETE");
    expect(calls[1].path).toBe("/by-query");
    expect(calls[1].body).toBeUndefined();
    expect(calls[1].search.get("deleteStorage")).toBe("false");
    expectFilter(calls[1]);
  });
});

describe("refusal overflow and the select-all offer", () => {
  it("caps a long refusal list in a scrolling box with a '+N more' line", async () => {
    const invalids = Array.from({ length: 80 }, (_, i) => ({ id: `id-${i}`, reason: "running" }));
    serve(3, () => ({ status: 400, body: { message: "80 cannot be deleted", invalids } }));
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    await user.click(action("Delete"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText(/Nothing was changed/)).toBeInTheDocument();
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(51);
    expect(within(dialog).getByText("and 30 more")).toBeInTheDocument();
    expect(within(dialog).getByRole("list").className).toMatch(/overflow-auto/);
  });

  it("offers select-all only while every row of the current page is chosen", async () => {
    serve(120);
    renderList();
    await screen.findByTestId("executions-table");
    const user = userEvent.setup();
    await user.click(rowBox(OK));
    expect(screen.queryByRole("button", { name: /Select all 120 matching/ })).toBeNull();
    await user.click(screen.getByRole("checkbox", { name: "Select all executions on this page" }));
    expect(screen.getByRole("button", { name: "Select all 120 matching this filter" })).toBeInTheDocument();
  });
});
