import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect, useRef } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

/**
 * A minimal fake Monaco, following `Query.test.tsx`'s stub (itself following
 * `components/notebook/__tests__/CellEditor.test.tsx`). Real Monaco cannot load
 * in jsdom — no workers, no layout engine, no measurable font — so the
 * alternative to a stub is not testing the editor at all.
 *
 * **The stub is built so the wiring is still what is under test.** It echoes
 * back the two things only `SqlEditor` supplies — `defaultLanguage="sql"` and
 * the `ariaLabel` it puts in `options` — so a page that went back to a bare
 * `<Textarea>`, or that embedded `MonacoEditor` directly, fails rather than
 * passing on "there is a textbox called SQL". And `onMount` runs `SqlEditor`'s
 * REAL `handleMount`, so the ⌘↵ command asserted below is the one the product
 * registers.
 */
const monacoStub = vi.hoisted(() => {
  const KeyMod = { CtrlCmd: 1 << 11 };
  const KeyCode = { Enter: 3 };
  return {
    KeyMod,
    KeyCode,
    commands: {} as Record<number, () => void>,
    reset() {
      this.commands = {};
    },
    /** The callback registered for ⌘/Ctrl+↵, or `undefined` if none was. */
    runCommand(): (() => void) | undefined {
      return this.commands[KeyMod.CtrlCmd | KeyCode.Enter];
    },
  };
});

vi.mock("@/components/monaco/MonacoEditor", () => ({
  // A NAMED, capitalised function: this stub calls hooks, and an anonymous
  // arrow assigned to `default` is not a component as far as
  // `react-hooks/rules-of-hooks` can tell.
  default: function MonacoStub({
    value,
    onChange,
    onMount,
    beforeMount,
    defaultLanguage,
    options,
  }: {
    value: string;
    onChange?: (v?: string) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
    beforeMount?: (monaco: unknown) => void;
    defaultLanguage?: string;
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
        data-testid="monaco-editor"
        data-language={defaultLanguage}
        aria-label={options?.ariaLabel}
        readOnly={!!options?.readOnly}
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
      />
    );
  },
}));

vi.mock("@/api/dataApi");
vi.mock("@/api/databases");

import * as dataApi from "@/api/dataApi";
import * as databasesApi from "@/api/databases";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";

const navigate = vi.fn();
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => navigate,
}));

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/ws1/data-api/endpoints/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/new" element={<EndpointCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("EndpointCreate", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    monacoStub.reset();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }, { id: "db-2", name: "lake" }],
    } as never);
  });

  it("sends what the author typed, and only the fields a row needs", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("Description (optional)"), "Yesterday's orders");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    // Two databases in the picker, so choosing the SECOND proves the value is
    // read from the choice rather than defaulted to the only option available.
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: "lake" }));
    // Method is picked AWAY from its default for the same reason the second
    // database is: asserting `method: "GET"` on a form that defaults to GET
    // passes just as well for a page that ignores the picker, or has no picker
    // at all. POST is the only value that can only come from the control.
    await userEvent.click(screen.getByRole("combobox", { name: "Method" }));
    await userEvent.click(await screen.findByRole("option", { name: "POST" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    const body = mutateAsync.mock.calls[0][0].data;
    expect(body).toMatchObject({
      name: "Daily report",
      path: "reports/daily",
      database_id: "db-2",
      method: "POST",
      description: "Yesterday's orders",
    });
    // ONE statement, named `main` — the shape migration 0129 gave every
    // endpoint that pre-dated the statement list, so a new endpoint and a
    // migrated one are the same shape from here on. A deep-equal, not a length
    // check: a page that sent a rows/total pair, or that left the name blank
    // (which the server refuses with a 400), passes any weaker assertion.
    expect(body.statements).toEqual([{ name: "main", sql: "SELECT 1" }]);
    // The create page must not invent config the editor owns.
    expect(Object.keys(body)).not.toContain("cache");
    expect(Object.keys(body)).not.toContain("rate_limit");
  });

  // The other half of BOTH optional-ish fields.
  //
  // `description`: blank must mean ABSENT, not `""` — a page that always sent
  // the key would pass the test above.
  //
  // `method`: this test never touches the Method picker, so it pins the
  // DEFAULT. Asserting only the non-default value (POST, above) turns out not
  // to be enough — it just moves which constant is unfalsifiable: hard-coding
  // `method: "POST"` in the request body passes a suite that only ever picks
  // POST. Measured, not assumed: that exact mutation survived until this line
  // existed. With both pinned, only reading the control passes.
  it("leaves description out and method at GET when the author touches neither", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    const body = mutateAsync.mock.calls[0][0].data;
    expect(Object.keys(body)).not.toContain("description");
    expect(body).toMatchObject({ method: "GET" });
  });

  it("hands off to the created endpoint's editor, not back to the list", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } }),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "x");
    await userEvent.type(screen.getByLabelText("Path"), "x");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));
    expect(navigate).toHaveBeenCalledWith("/explore/ws1/data-api/endpoints/e-9");
  });

  it("keeps what the author typed when creation fails", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("path already taken")),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    // A dialog could be dismissed and retried; a page that clears itself on
    // failure loses work the author cannot get back. The engine's own message
    // must be on screen, and every field must still hold its value.
    expect(await screen.findByText(/path already taken/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("Daily report");
    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT 1");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("renders exactly one h1", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderCreate();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("writes SQL in the editor the endpoint gets a moment later, not a bare textarea", async () => {
    // The audit's complaint: the weakest tool at the moment of most need. The
    // same SQL becomes a Monaco editor (`SqlEditor`) as soon as the endpoint
    // exists, so an author writes their FIRST statement with no syntax
    // colouring, no bracket matching and no completion, then everything after
    // it with all three.
    //
    // Asserted through the two things only `SqlEditor` supplies —
    // `defaultLanguage="sql"` and `options.ariaLabel` — rather than "a textbox
    // named SQL exists", which the `<Textarea>` this replaces satisfied just
    // as well. `toBe` on the element identity, so the editor must be the SQL
    // field itself and not a second one rendered somewhere on the page.
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderCreate();

    const sql = screen.getByLabelText("SQL");
    expect(screen.getByTestId("monaco-editor")).toBe(sql);
    expect(sql).toHaveAttribute("data-language", "sql");
  });

  it("does not create the endpoint from ⌘↵ in the editor", async () => {
    // `SqlEditor` registers ⌘↵ unconditionally in `handleMount` and calls
    // `onRun?.()`. There is nothing to run before the endpoint exists — Preview
    // is a `POST …/endpoints/{id}/preview` — so this page passes no `onRun`,
    // and the guard is that the command is inert rather than that it is absent.
    //
    // The form is filled FIRST, deliberately: with an incomplete form `submit()`
    // returns at its `canSubmit` check, so an editor that really did submit on
    // ⌘↵ would still leave `mutateAsync` uncalled and this test would pass
    // while the defect shipped.
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    expect(screen.getByRole("button", { name: /create endpoint/i })).toBeEnabled();

    // The command the PRODUCT registered — the stub runs `SqlEditor`'s real
    // `handleMount`, so its absence would mean the editor is not `SqlEditor`.
    const run = monacoStub.runCommand();
    expect(run).toBeTypeOf("function");
    run!();

    expect(mutateAsync).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("puts the way back above the title, not against the primary action", async () => {
    // Every detail screen puts `← Endpoints` top-left, above the name
    // (`EndpointDetail.tsx`). This page had it in `ListPageShell`'s `toolbar`
    // slot, which renders into the same right-hand group as `action` — so the
    // escape sat one control away from the commit.
    //
    // Two assertions because either alone can be satisfied by the defect: DOM
    // order alone is true of a link rendered first INSIDE the action group,
    // and "not in the action group" alone is true of a link dropped anywhere
    // below the header.
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    const { container } = renderCreate();

    const back = screen.getByRole("link", { name: /endpoints/i });
    const h1 = screen.getByRole("heading", { level: 1 });
    const submit = screen.getByRole("button", { name: /create endpoint/i });

    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    const actionGroup = container.querySelector("header")!.lastElementChild as HTMLElement;
    expect(actionGroup).toContainElement(submit);
    expect(actionGroup).not.toContainElement(back);
  });
});
