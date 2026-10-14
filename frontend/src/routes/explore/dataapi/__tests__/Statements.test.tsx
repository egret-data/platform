import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type React from "react";
import { useEffect, useRef } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/**
 * An endpoint's definition is a LIST of named statements (M2), and this file is
 * about the half of that a person can see: adding, naming, reordering and
 * removing them, and the request bodies those actions produce.
 *
 * **The property under test is not "the list works" — it is that the COMMON
 * CASE did not get heavier.** Most endpoints have exactly one statement, and a
 * one-statement endpoint must render what it rendered before this feature
 * existed: an editor labelled `SQL` under a heading of the same name, with no
 * name field, no tab strip, no reorder controls and no remove button anywhere
 * on screen. `only-one-statement` is therefore asserted by ABSENCE, which is
 * the only assertion that can fail when the list UI leaks into it.
 *
 * The Monaco and `react-resizable-panels` stubs are copied from
 * `Query.test.tsx` rather than imported — see the notes on them there for why
 * each exists (real Monaco cannot load in jsdom; every rect in jsdom is 0×0, so
 * the real panel `Separator` wins the hit-test for every click in the group).
 * That file does not export them, and duplicating a stub is cheaper than
 * turning a 1500-line test file into a module.
 */
const monacoStub = vi.hoisted(() => {
  const KeyMod = { CtrlCmd: 1 << 11 };
  const KeyCode = { Enter: 3 };
  return {
    KeyMod,
    KeyCode,
    commands: {} as Record<number, () => void>,
    runCommand(): (() => void) | undefined {
      return this.commands[KeyMod.CtrlCmd | KeyCode.Enter];
    },
  };
});

vi.mock("react-resizable-panels", () => ({
  Group: ({ children, className }: { children?: React.ReactNode; className?: string }) => (
    <div data-testid="panel-group" className={className}>
      {children}
    </div>
  ),
  Panel: ({ children, className }: { children?: React.ReactNode; className?: string }) => (
    <div data-testid="panel" className={className}>
      {children}
    </div>
  ),
  Separator: ({ className }: { className?: string }) => (
    <div data-testid="panel-separator" role="separator" className={className} />
  ),
}));

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub({
    value,
    onChange,
    onMount,
    beforeMount,
    options,
  }: {
    value: string;
    onChange?: (v?: string) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
    beforeMount?: (monaco: unknown) => void;
    options?: { ariaLabel?: string; readOnly?: boolean };
  }) {
    const latest = useRef(value);
    latest.current = value;
    useEffect(() => {
      const monaco = {
        KeyMod: monacoStub.KeyMod,
        KeyCode: monacoStub.KeyCode,
        editor: { defineTheme: () => {} },
        languages: {
          CompletionItemKind: {},
          registerCompletionItemProvider: () => ({ dispose: () => {} }),
        },
      };
      beforeMount?.(monaco);
      onMount?.(
        {
          getValue: () => latest.current,
          getSelection: () => null,
          getModel: () => null,
          // `addAction` only, no `addCommand`: an `addCommand` shortcut is not
          // scoped to its editor and is never removed (see `SqlEditor`), so a
          // regression to it throws here.
          getId: () => "stub-editor",
          addAction: (desc: { id: string; keybindings?: number[]; run: (e: unknown) => unknown }) => {
            const entry = { id: desc.id, keybindings: desc.keybindings ?? [], run: () => void desc.run(undefined), disposed: false };
            for (const k of entry.keybindings) monacoStub.commands[k] = entry.run;
            return { dispose: () => { entry.disposed = true; } };
          },
          onDidChangeCursorSelection: () => ({ dispose: () => {} }),
        },
        monaco,
      );
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return (
      <textarea
        aria-label={options?.ariaLabel}
        readOnly={!!options?.readOnly}
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
      />
    );
  },
}));

vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/dataApi");
vi.mock("@/lib/useUnsavedChangesWarning", () => ({ useUnsavedChangesWarning: () => {} }));

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import * as dataApi from "@/api/dataApi";
import type { DataApiEndpointDto, StatementSpec } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query, { MAX_STATEMENTS } from "@/routes/explore/dataapi/detail/Query";

/**
 * The default is ONE statement named `main` — the shape migration 0129 gave
 * every endpoint that existed before this feature, and so the shape almost
 * every endpoint on an instance has.
 */
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
    live_version_id: null,
    state: "draft",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

const updateMutateAsync = vi.fn();

function renderQuery(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
    data: { status: 200, data: endpoint },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  } as never);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/ws1/data-api/endpoints/${endpoint.id}`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<Query />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** A list of `n` distinctly-named statements, for the tests about the LIMIT. */
function statementList(n: number) {
  return Array.from({ length: n }, (_, i) => ({ name: `s${i + 1}`, sql: "SELECT 1" }));
}

/** The `statements` a Save actually PATCHed, from the one call it made. */
function savedStatements(): StatementSpec[] {
  expect(updateMutateAsync).toHaveBeenCalledTimes(1);
  return updateMutateAsync.mock.calls[0][0].data.statements as StatementSpec[];
}

beforeEach(() => {
  vi.resetAllMocks();
  // The shell reads the version history on EVERY tab now: the header names the
  // version a consumer is actually receiving, and says when the saved draft has
  // moved past it. Automocked, that hook hands back `undefined`, so this default
  // is what keeps the shell mountable at all here — tests that care about
  // versions override it afterwards.
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListEndpointVersions>,
  );
  updateMutateAsync.mockResolvedValue({ status: 200, data: makeEndpoint() });
  vi.mocked(databasesApi.useDatabases).mockReturnValue({
    data: [{ id: "db-1", name: "warehouse" }],
  } as never);
  vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
    mutateAsync: updateMutateAsync,
    isPending: false,
  } as never);
  vi.mocked(dataApi.useRestoreEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
});

describe("Query section — one statement is the common case", () => {
  /**
   * The whole feature, stated as what it must NOT cost. Every control the list
   * needs is asserted absent here, so a list UI that renders unconditionally —
   * a tab strip of one, a name field for a name nobody ever reads, a Remove
   * that could only ever be refused — fails this test and nothing else.
   */
  it("shows a bare SQL editor, with no name, tab strip, reorder or remove control", async () => {
    renderQuery();

    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT 1");
    expect(screen.getByRole("heading", { level: 3, name: "SQL" })).toBeInTheDocument();
    // The name exists in the record (`main`) and is deliberately not on screen:
    // a one-statement endpoint's response is an ARRAY, so no caller ever reads
    // the key, and showing it would invite the author to think one did.
    expect(screen.queryByLabelText("Statement name")).not.toBeInTheDocument();
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /move (up|down)/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^remove statement$/i })).not.toBeInTheDocument();
    expect(screen.queryByText("main")).not.toBeInTheDocument();
  });

  /**
   * The other half of the same property, on the wire.
   *
   * A deep-equal on the whole `statements` array rather than on its length: an
   * editor that always sends a rows/total pair, or that drops the name it was
   * given and invents one, produces a body this refuses and a body a length
   * check would wave through.
   */
  it("saves the one statement it was given — same name, edited SQL, nothing added", async () => {
    renderQuery();
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([{ name: "main", sql: "SELECT 1 -- x" }]);
    // A partial PATCH: nothing but the statements changed, so nothing else is sent.
    expect(Object.keys(updateMutateAsync.mock.calls[0][0].data)).toEqual(["statements"]);
  });

  it("previews the statement on screen", async () => {
    const previewMutateAsync = vi
      .fn()
      .mockResolvedValue({ status: 200, data: { columns: [], rows: [], row_count: 0, truncated: false } });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
    renderQuery();

    await userEvent.type(await screen.findByLabelText("SQL"), " -- edited");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 1 -- edited");
  });
});

describe("Query section — authoring several statements", () => {
  const two = makeEndpoint({
    statements: [
      { name: "rows", sql: "SELECT * FROM t" },
      { name: "total", sql: "SELECT count(*) FROM t" },
    ],
  } as Partial<DataApiEndpointDto>);

  /**
   * The add control, and the fact that adding one makes the list VISIBLE.
   *
   * Both halves in one test on purpose: an "Add statement" button that appends
   * to the form without the strip appearing leaves the author with a second
   * statement they cannot reach, edit or name — which is worse than no button.
   */
  it("adds a second statement, names it uniquely, and reveals the strip", async () => {
    renderQuery();
    await screen.findByLabelText("SQL");
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /add statement/i }));

    const strip = await screen.findByRole("tablist");
    expect(within(strip).getAllByRole("tab").map((t) => t.textContent)).toEqual(["main", "statement_2"]);
    // The new one is selected, so the author lands in the editor they just asked for.
    expect(screen.getByRole("tab", { selected: true })).toHaveTextContent("statement_2");
    expect(screen.getByLabelText("Statement name")).toHaveValue("statement_2");
    expect(screen.getByLabelText("SQL")).toHaveValue("");
  });

  it("sends both statements, in order, when saved", async () => {
    renderQuery();
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("button", { name: /add statement/i }));
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([
      { name: "main", sql: "SELECT 1" },
      { name: "statement_2", sql: "SELECT 2" },
    ]);
  });

  it("shows the selected statement's own SQL, not the first one's", async () => {
    renderQuery(two);
    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT * FROM t");

    await userEvent.click(screen.getByRole("tab", { name: "total" }));

    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT count(*) FROM t");
    expect(screen.getByLabelText("Statement name")).toHaveValue("total");
  });

  it("renames the selected statement and nothing else", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.clear(screen.getByLabelText("Statement name"));
    await userEvent.type(screen.getByLabelText("Statement name"), "row_count");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([
      { name: "rows", sql: "SELECT * FROM t" },
      { name: "row_count", sql: "SELECT count(*) FROM t" },
    ]);
  });

  /**
   * Order is part of the definition — the statements execute in it and `meta`
   * reports it — so it has to be editable, and the control has to move the
   * statement rather than merely re-label the tabs. Asserting the SAVED list
   * is what tells those two apart.
   */
  it("moves a statement up, and the strip and the saved list agree about the new order", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(screen.getByRole("button", { name: /move up/i }));

    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["total", "rows"]);
    // The moved statement stays selected — it is the one the author is working on.
    expect(screen.getByRole("tab", { selected: true })).toHaveTextContent("total");

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([
      { name: "total", sql: "SELECT count(*) FROM t" },
      { name: "rows", sql: "SELECT * FROM t" },
    ]);
  });

  it("cannot move the first statement up or the last one down", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    expect(screen.getByRole("button", { name: /move up/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /move down/i })).toBeEnabled();

    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    expect(screen.getByRole("button", { name: /move up/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /move down/i })).toBeDisabled();
  });

  /**
   * Removing the second statement puts the endpoint back in the common case,
   * so the whole list UI has to leave with it — the tab strip, the name field
   * and the reorder controls. The last statement can never be removed: an
   * empty list is refused by a CHECK constraint, so the control simply does
   * not exist once there is one left.
   */
  it("removes a statement and returns the section to the bare editor", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(screen.getByRole("button", { name: /^remove statement$/i }));

    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Statement name")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^remove statement$/i })).not.toBeInTheDocument();
    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT * FROM t");

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([{ name: "rows", sql: "SELECT * FROM t" }]);
  });

  /**
   * A statement name is a CONTRACT — once an endpoint has more than one, it is
   * the key a caller reads its results under, and `validate_statements`
   * enforces `^[a-z][a-z0-9_]*$` and uniqueness with a 400. Saying so on
   * screen, beside the field, beats a red toast arriving after the Save.
   */
  it("names the rule when a statement name is not an identifier", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.clear(screen.getByLabelText("Statement name"));
    await userEvent.type(screen.getByLabelText("Statement name"), "Rows 1");

    expect(await screen.findByRole("alert")).toHaveTextContent(/lower-case letter/i);
  });

  it("says which name is duplicated", async () => {
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.clear(screen.getByLabelText("Statement name"));
    await userEvent.type(screen.getByLabelText("Statement name"), "rows");

    expect(await screen.findByRole("alert")).toHaveTextContent(/already used/i);
  });

  it("previews the SELECTED statement, not the first one", async () => {
    const previewMutateAsync = vi
      .fn()
      .mockResolvedValue({ status: 200, data: { columns: [], rows: [], row_count: 0, truncated: false } });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.sql).toBe("SELECT count(*) FROM t");
  });

  /**
   * The preview body carries the OTHER statements too — and only their text.
   *
   * Whether a parameter arrives as a scalar or a list is a property of the whole
   * endpoint: one `{% for %}` anywhere makes it a list for every statement. A
   * preview body carrying only the statement on screen made the server judge
   * that from one statement while publish judged it from all of them, so an
   * author could be shown a green preview of a definition publish then refused.
   *
   * Two assertions, and the second is the one that fails if `siblings` is built
   * by appending rather than by excluding: the statement being RUN must not also
   * appear in the list, or the server would count it twice — and a preview that
   * shipped every statement including its own would look identical on the first
   * assertion alone.
   */
  it("sends the other statements' SQL as `siblings`, without the one it runs", async () => {
    const previewMutateAsync = vi
      .fn()
      .mockResolvedValue({ status: 200, data: { columns: [], rows: [], row_count: 0, truncated: false } });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
    renderQuery(two);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    const body = previewMutateAsync.mock.calls[0][0].data;
    expect(body.siblings).toEqual(["SELECT * FROM t"]);
    expect(body.siblings).not.toContain(body.sql);
  });

  /**
   * The compatibility case, from the client's side: a one-statement endpoint
   * sends an EMPTY list, which is the union of one the server computed before
   * this field existed. So the common endpoint's preview is byte-identical in
   * meaning to what it was.
   */
  it("sends an empty `siblings` for a one-statement endpoint", async () => {
    const previewMutateAsync = vi
      .fn()
      .mockResolvedValue({ status: 200, data: { columns: [], rows: [], row_count: 0, truncated: false } });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
    renderQuery(makeEndpoint({ statements: [{ name: "rows", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.siblings).toEqual([]);
  });

  /**
   * The upper bound on the list, said on the control rather than by a toast
   * after the save.
   *
   * `MAX_STATEMENTS` is 16 (`crates/egret-dataapi-api/src/lib.rs`), and the
   * server refuses a longer list on create, on PATCH and at publish. What this
   * pins is the half a person meets first: the button stops offering to make a
   * list the save would refuse.
   *
   * **Both directions, and the negative one is why the endpoint below has
   * fifteen statements rather than sixteen.** A control disabled unconditionally
   * would satisfy the assertion at the limit and nothing else; the first half
   * fails if the guard is off by one in the direction that costs an author a
   * statement they were entitled to.
   *
   * **`toHaveAttribute("disabled")`, not `toBeDisabled()`.** The section renders
   * inside the shell's `<fieldset disabled={trashed}>`, and `toBeDisabled` walks
   * up to an ancestor `fieldset[disabled]` — so on a trashed endpoint it passes
   * with this guard deleted. This endpoint is not trashed, so the two agree
   * today; asserting the attribute is what keeps them agreeing.
   */
  it("stops offering a new statement at the limit, and says why", async () => {
    renderQuery(makeEndpoint({ statements: statementList(16) } as Partial<DataApiEndpointDto>));
    await screen.findByLabelText("SQL");
    const add = screen.getByRole("button", { name: /add statement/i });
    expect(add).toHaveAttribute("disabled");
    // And it says why, and what to do instead — a disabled control with no
    // explanation reads as a bug.
    expect(add).toHaveAttribute("title", expect.stringContaining("at most 16 statements"));

    // The click the disabled attribute prevents changes nothing: still 16.
    fireEvent.click(add);
    expect(within(screen.getByRole("tablist")).getAllByRole("tab")).toHaveLength(16);
  });

  /**
   * The other side, and its own test rather than a second assertion above: a
   * control disabled unconditionally, or one guard off by one, satisfies the
   * test above and costs an author a statement they were entitled to.
   */
  it("still offers the sixteenth statement, which is the last one allowed", async () => {
    renderQuery(makeEndpoint({ statements: statementList(15) } as Partial<DataApiEndpointDto>));
    await screen.findByLabelText("SQL");
    const add = screen.getByRole("button", { name: /add statement/i });
    expect(add).not.toHaveAttribute("disabled");

    await userEvent.click(add);
    expect(within(await screen.findByRole("tablist")).getAllByRole("tab")).toHaveLength(16);
  });

  /**
   * Switching statements is READING, not editing — the same argument the
   * database/parameters strip is a native `<details>` for. The section renders
   * inside the shell's `<fieldset disabled={trashed}>`, which disables every
   * form control under it and cannot be overridden by a child's own
   * `disabled={false}`, so a strip built out of `<button>`s (Radix
   * `TabsTrigger` is one) would leave a trashed endpoint's second statement
   * permanently unreachable.
   *
   * **The tag assertion is the load-bearing one, and `fireEvent` rather than
   * `userEvent` is why.** `user-event`'s `isDisabled` walks the ancestors and
   * treats ANYTHING inside a disabled `fieldset` as disabled
   * (`utils/misc/isDisabled.js`), which the browser does not — the spec
   * disables listed form elements, not every descendant. So `userEvent.click`
   * refuses to dispatch here whatever the element is, and could never tell the
   * two implementations apart. `fireEvent` dispatches the DOM event the browser
   * would, and the tag check is what pins the reason the browser would deliver
   * it: switch the strip to `<button>` and this test reddens on the tag while
   * every other test in this file stays green.
   */
  it("switches statements through an element the disabled fieldset cannot reach", async () => {
    renderQuery(
      makeEndpoint({
        deleted_at: "2026-08-20T00:00:00Z",
        state: "disabled",
        statements: [
          { name: "rows", sql: "SELECT * FROM t" },
          { name: "total", sql: "SELECT count(*) FROM t" },
        ],
      } as Partial<DataApiEndpointDto>),
    );
    await screen.findByLabelText("SQL");
    const tab = screen.getByRole("tab", { name: "total" });
    expect(["BUTTON", "INPUT", "SELECT", "TEXTAREA"]).not.toContain(tab.tagName);
    expect(tab.closest("fieldset")).toHaveAttribute("disabled");

    fireEvent.click(tab);
    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT count(*) FROM t");
    // Read-only, though: the editor and every control that WRITES stays inert.
    expect(screen.getByLabelText("SQL")).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: /add statement/i })).toBeDisabled();
  });
});

/**
 * The statement limit exists three times — `MAX_STATEMENTS` in
 * `crates/egret-dataapi-api/src/lib.rs`, a `#[schema(max_items = 16)]` on the
 * two request bodies, and `MAX_STATEMENTS` in `detail/Query.tsx` — and nothing
 * in this repo shares a constant between Rust and TypeScript.
 *
 * Two mechanisms close that, and this file owns the second:
 *
 * 1. Rust to CONTRACT: a `const _: () = assert!(MAX_STATEMENTS == 16)` beside
 *    the schema attribute, so raising the limit without editing the attribute
 *    fails the build.
 * 2. Contract to FRONTEND: this test, which reads the committed `openapi.json`
 *    and compares its `maxItems` with the constant the UI actually uses.
 *
 * **It derives the number rather than restating it.** A test spelling `16` a
 * fourth time passes exactly when the other three are wrong in the same way,
 * which is no gate at all. Nothing below writes the number down: change either
 * side alone and this fails, naming both values.
 *
 * `openapi.json` is committed at the repo root and asserted byte-for-byte
 * against a fresh emit by `egret-server`, so reading it here is reading the
 * generated contract and not a stale copy of it.
 */
describe("the statement limit does not drift between the contract and the UI", () => {
  /**
   * Both request bodies, not one. They carry the limit independently (`create`
   * and `PATCH` each accept a whole list), so a `max_items` added to one and
   * forgotten on the other is exactly the drift this is for.
   */
  const bodies = ["CreateEndpointInput", "UpdateEndpointInput"] as const;

  function contractMaxItems(schema: (typeof bodies)[number]): unknown {
    // From this file, not from `process.cwd()`: the suite's `include` also
    // reaches ../scripts/harness, so the working directory is not a fixed point.
    const root = path.resolve(import.meta.dirname, "../../../../../..");
    const doc = JSON.parse(readFileSync(path.join(root, "openapi.json"), "utf8"));
    return doc.components.schemas[schema].properties.statements.maxItems;
  }

  it.each(bodies)("%s.statements declares the limit the UI enforces", (schema) => {
    const declared = contractMaxItems(schema);
    // Present at all, first: `undefined === undefined` would make the
    // comparison below vacuous the moment the attribute is dropped, and a
    // dropped attribute is one of the two ways this drifts.
    expect(typeof declared).toBe("number");
    expect(declared).toBe(MAX_STATEMENTS);
  });
});

/**
 * Package S: a statement's SQL is TEXT or a SCRIPT, and the two are separate
 * from the statement's IDENTITY.
 *
 * The property here is not "a script can be edited" — Task 8's own block in
 * `Query.test.tsx` is where the mode switch lives — it is that **the author's
 * script survives every operation that is not about its content**. The first
 * version of Task 1 rebuilt the edited statement as `{ name, sql: sqlOf(s) }`,
 * which for a scripted statement is `sql: ""`: a pure RENAME, which never
 * touches SQL, silently destroyed the script and sent a text statement in its
 * place. Silent data loss on a rename is worse than a refusal, which is what
 * both tests below are here to keep true.
 */
describe("Query section — a statement whose SQL a script builds", () => {
  const SCRIPT = 'def build_sql(params):\n  return "SELECT 1"';
  const scripted = makeEndpoint({
    statements: [
      { name: "rows", sql: "SELECT * FROM t" },
      { name: "total", script: SCRIPT },
    ],
  } as Partial<DataApiEndpointDto>);

  it("keeps the script when the statement is renamed", async () => {
    renderQuery(scripted);
    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.clear(screen.getByLabelText("Statement name"));
    await userEvent.type(screen.getByLabelText("Statement name"), "row_count");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    // A deep equal on the WHOLE list: the renamed entry must still carry
    // `script` and must NOT have grown an `sql` key beside it, because a
    // statement carrying both is refused by the backend reader.
    expect(savedStatements()).toEqual([
      { name: "rows", sql: "SELECT * FROM t" },
      { name: "row_count", script: SCRIPT },
    ]);
  });

  it("shows the script editor for a scripted statement and the SQL editor for a text one", async () => {
    renderQuery(scripted);
    // The discriminating half FIRST, on the text statement, so that neither
    // absence below can be something this section does to every statement.
    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT * FROM t");
    expect(screen.queryByLabelText("SQL script")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "total" }));

    // The editor that does not apply is REMOVED, not disabled — there is no
    // `sql` on this statement to show read-only, and a box showing one would be
    // showing a value the save cannot carry.
    expect(screen.getByLabelText("SQL script")).toHaveValue(SCRIPT);
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();
  });
});
