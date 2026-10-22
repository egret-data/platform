import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type React from "react";
import { StrictMode, useEffect, useRef, useState } from "react";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/**
 * A minimal fake Monaco, following `components/notebook/__tests__/CellEditor.test.tsx`.
 * Real Monaco cannot load in jsdom (it needs workers, a layout engine and a
 * measurable font), so the alternative to a stub is not testing the editor at all.
 *
 * **The stub is built so the wiring is still what is under test, not the stub.**
 * The `<textarea>` takes its accessible name and its read-only state from the
 * `options` `SqlEditor` actually computes, so a `Query` that forgets
 * `ariaLabel` loses every `findByLabelText("SQL")` in this file and a `Query`
 * that forgets `readOnly` fails the trashed test. And `onMount` runs
 * `SqlEditor`'s REAL `handleMount`, so the ⌘↵ command below is the one the
 * product registers, reached through the keybinding the product computes.
 */
const monacoStub = vi.hoisted(() => {
  const KeyMod = { CtrlCmd: 1 << 11 };
  const KeyCode = { Enter: 3 };
  return {
    KeyMod,
    KeyCode,
    commands: {} as Record<number, () => void>,
    /** Every `addAction`, in order; `disposed` turns true when it is disposed. */
    actions: [] as { id: string; keybindings: number[]; run: () => void; disposed: boolean }[],
    /** Calls the mounted component's `onMount` again, as `@monaco-editor/react` can. */
    remount: undefined as (() => void) | undefined,
    reset() {
      this.commands = {};
      this.actions = [];
    },
    /** The callback registered for ⌘/Ctrl+↵, or `undefined` if none was. */
    runCommand(): (() => void) | undefined {
      return this.commands[KeyMod.CtrlCmd | KeyCode.Enter];
    },
  };
});

/**
 * `react-resizable-panels`, stubbed to the three props `Query` passes.
 *
 * **Not for convenience — the real one is unusable here.** With no layout
 * engine every rect in jsdom is 0×0, so the library's `Separator` wins the
 * hit-test for a click anywhere in the group and takes focus
 * (`data-separator="focus"`). Every `userEvent.type` in this file then goes to
 * the drag handle instead of the field, and the whole section silently reads
 * as if nothing can be typed into it. That is an artifact of the environment,
 * not of the product — the browser pass in the plan is what checks the drag.
 *
 * The stub echoes the SIZES back as attributes rather than swallowing them,
 * because those are exactly what a reader cannot verify by eye: in v4 a bare
 * number means PIXELS and a string means percent, so `defaultSize={55}` is a
 * 55-pixel editor that still looks right in the source.
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

vi.mock("@/components/monaco/MonacoEditor", () => ({
  // A NAMED, capitalised function: this stub calls hooks, and an anonymous
  // arrow assigned to `default` is not a component as far as
  // `react-hooks/rules-of-hooks` can tell.
  default: function MonacoStub({
    value,
    defaultValue,
    onChange,
    onMount,
    beforeMount,
    options,
  }: {
    value?: string;
    defaultValue?: string;
    onChange?: (v?: string) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
    beforeMount?: (monaco: unknown) => void;
    options?: { ariaLabel?: string; readOnly?: boolean };
  }) {
    // The mount effect runs once, but the command it registers must read the
    // CURRENT text — exactly like real Monaco's `editor.getValue()`.
    // Like `@monaco-editor/react`: a defined `value` drives the text; without
    // one (`SqlEditor` passes only `defaultValue`), the editor owns its text and
    // the component writes outside changes through `executeEdits`.
    const latest = useRef(value ?? defaultValue ?? "");
    if (value !== undefined) latest.current = value;
    const [text, setText] = useState(latest.current);
    const write = (t: string) => { latest.current = t; setText(t); };
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
      const editor = {
        getValue: () => latest.current,
        getSelection: () => null,
        getModel: () => ({ getFullModelRange: () => ({}) }),
        setValue: write,
        executeEdits: (_src: string, edits: { text: string }[]) => { write(edits[0].text); return true; },
        pushUndoStop: () => true,
        // `addAction` only, no `addCommand`: an `addCommand` shortcut is not
        // scoped to its editor and is never removed (see `SqlEditor`), so a
        // regression to it throws here.
        getId: () => "stub-editor",
        addAction: (desc: { id: string; keybindings?: number[]; run: (e: unknown) => unknown }) => {
          const entry = { id: desc.id, keybindings: desc.keybindings ?? [], run: () => void desc.run(undefined), disposed: false };
          monacoStub.actions.push(entry);
          for (const k of entry.keybindings) monacoStub.commands[k] = entry.run;
          return { dispose: () => { entry.disposed = true; } };
        },
        onDidChangeCursorSelection: () => ({ dispose: () => {} }),
      };
      onMount?.(editor, monaco);
      monacoStub.remount = () => onMount?.(editor, monaco);
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return (
      <textarea
        aria-label={options?.ariaLabel}
        readOnly={!!options?.readOnly}
        value={value ?? text}
        onChange={(e) => { write(e.target.value); onChange?.(e.target.value); }}
      />
    );
  },
}));

// No msw in this repo — mock the API module directly. `@/api/access` is
// needed only by the Publish/shared-dirty-flag block below (`Versions`
// resolves a version's `created_by` via `listUsers`), and
// `@/lib/useUnsavedChangesWarning` only by the dirtiness block — both mocked
// here anyway since `vi.mock` must be module-scope, not per-describe.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/dataApi");
vi.mock("@/api/access");

// Spy on the real hook rather than replace it with a no-op — see the
// equivalent mock in `EndpointDetail.test.tsx` for why (needs every call's
// argument, in order).
const warnSpy = vi.fn();
vi.mock("@/lib/useUnsavedChangesWarning", () => ({
  useUnsavedChangesWarning: (when: boolean) => warnSpy(when),
}));

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import * as dataApi from "@/api/dataApi";
import * as accessApi from "@/api/access";
import { ParamLocation, ParamType, type DataApiEndpointDto, type ParamSpec } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query from "@/routes/explore/dataapi/detail/Query";
import Config from "@/routes/explore/dataapi/detail/Config";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";
import { expectSectionIntroCapsItsProse, expectSectionLeavesWidthToThePage } from "./sectionWidth";

// Copied verbatim from `EndpointDetail.test.tsx` (near the top of that file)
// rather than imported — that file does not export it, and duplicating a
// fixture builder is cheaper than making a 900-line test file into a module.
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

function renderQuery(endpoint: DataApiEndpointDto = makeEndpoint()) {
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

describe("Query section — preview", () => {
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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as never);
  });

  it("sends the on-screen SQL, not the loaded draft", async () => {
    const endpoint = makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.clear(sql);
    await userEvent.type(sql, "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: /preview/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 2");
  });

  /**
   * Final-review finding (Important #2): the request body had no composed test —
   * only `data.sql` was ever asserted, so 3 of its 4 fields were unpinned on both
   * sides of the wire (the backend's `tests/preview.rs` hand-crafts its own JSON,
   * so nothing joined the two halves). Two mutations survived the whole suite:
   * deleting `sent[p.name] = [v]` (a typed test value silently stops reaching the
   * server, and every required-parameter preview 400s while the author looks at a
   * filled-in field), and sending `database_id: ""` (a 422 with a `text/plain`
   * body, which used to render as nothing at all).
   *
   * So: deep-equal the WHOLE `data`, not a field of it.
   */
  it("sends sql, params, the typed test values and the database id — the whole body", async () => {
    const params = [
      {
        name: "region",
        type: ParamType.string,
        location: ParamLocation.query,
        required: true,
        description: "",
      },
    ];
    const endpoint = makeEndpoint({ params } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [], rows: [], row_count: 0, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);

    renderQuery(endpoint);
    // By label, not by DOM id: the id is index-based (see `PreviewBlock`'s comment
    // on why a name cannot be unique mid-edit), and the label is what the author
    // actually reads.
    await userEvent.type(await screen.findByLabelText(/^region/), "EU");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0]).toMatchObject({ workspace: "ws1", id: "ep-1" });
    // One list per name, matching the serving path. `toEqual` on the whole object
    // so an added, dropped or renamed field cannot pass unnoticed.
    expect(mutateAsync.mock.calls[0][0].data).toEqual({
      sql: "SELECT 1",
      // The one-statement endpoint this fixture is: nothing else to inform the
      // shape decision with. `Statements.test.tsx` is where a NON-empty list is
      // pinned, and where the statement being run is asserted absent from it.
      siblings: [],
      params,
      values: { region: ["EU"] },
      // The DRAFT's `output`, and required by the route (C2 Task 8). The server
      // checks its `transform` with publish's own rule — a script that does not
      // parse is refused here rather than only at Publish — so a body that
      // dropped this field would preview green on drafts publish refuses.
      output: {},
      database_id: "db-1",
    });
  });

  /**
   * Final-review finding (Minor #5): the rows were keyed and id'd by `p.name`,
   * which is not unique while the author is editing — two freshly-added rows are
   * both blank, and a typo can duplicate an existing name. That produced duplicate
   * React keys, duplicate DOM ids, and one input visibly driving both rows.
   */
  it("gives each test-value input its own id even when two declarations share a name", async () => {
    const endpoint = makeEndpoint({
      params: [
        { name: "", type: ParamType.string, location: ParamLocation.query, required: false, description: "" },
        { name: "", type: ParamType.string, location: ParamLocation.query, required: false, description: "" },
      ],
    } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);

    const { container } = renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    const ids = Array.from(container.querySelectorAll('input[id^="preview-param-"]')).map((el) => el.id);
    expect(ids).toHaveLength(2);
    // A duplicate id also means the second `<Label htmlFor>` points at the FIRST
    // input, so the wrong field focuses when its label is clicked.
    expect(new Set(ids).size).toBe(2);
  });

  /**
   * The same finding one level down, and the half the id fix above did NOT
   * reach: the rows were re-keyed by index, but `values` stayed a
   * `Record<name, string>` — so two declarations sharing a name still read and
   * wrote one entry, and the second field echoed the first keystroke by
   * keystroke. Two blank names is not an exotic case, it is what two freshly
   * added rows look like before either is named; a duplicate of a real name is
   * one mistyped character.
   *
   * Asserted as INDEPENDENCE of the two fields, not merely as "the ids differ":
   * distinct ids were already true while the mirroring was happening.
   */
  function twoParams(name: string) {
    const p = { type: ParamType.string, location: ParamLocation.query, required: false, description: "" };
    return [{ name, ...p }, { name, ...p }];
  }
  function previewInputs(container: HTMLElement) {
    return Array.from(container.querySelectorAll<HTMLInputElement>('input[id^="preview-param-"]'));
  }

  it("holds a separate test value per row when two declarations share a name", async () => {
    const endpoint = makeEndpoint({ params: twoParams("region") } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [], rows: [], row_count: 0, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);

    const { container } = renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    const inputs = previewInputs(container);
    expect(inputs).toHaveLength(2);

    await userEvent.type(inputs[0], "EU");
    // The mirror, stated as its own assertion: with one entry behind both rows
    // this already reads "EU" before the second row has been touched.
    expect(inputs[1]).toHaveValue("");
    await userEvent.type(inputs[1], "US");
    expect(inputs[0]).toHaveValue("EU");
    expect(inputs[1]).toHaveValue("US");

    // The wire format is unchanged — one list per NAME — so the two rows must
    // still collapse to one entry, and it is the last filled row's value.
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));
    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data.values).toEqual({ region: ["US"] });
  });

  it("holds a separate test value per row when two freshly added declarations are both unnamed", async () => {
    const endpoint = makeEndpoint({ params: twoParams("") } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);

    const { container } = renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    const inputs = previewInputs(container);
    expect(inputs).toHaveLength(2);

    await userEvent.type(inputs[0], "a");
    expect(inputs[1]).toHaveValue("");
    await userEvent.type(inputs[1], "b");
    expect(inputs[0]).toHaveValue("a");
    expect(inputs[1]).toHaveValue("b");
  });

  it("says the result was capped when the engine truncated it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({
        status: 200,
        data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: true },
      }),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // A capped preview read as a small result set is the failure this exists to
    // stop, so the copy must name the cap.
    expect(await screen.findByText(/first 100 rows/i)).toBeInTheDocument();
  });

  it("shows the engine's own error text inline", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("line 1:8: mismatched input 'FRUM'")),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // Inline, not a toast: a toast vanishes before an author mid-edit reads it,
    // and this is exactly the moment they need the message on screen.
    expect(await screen.findByText(/mismatched input 'FRUM'/)).toBeInTheDocument();
  });

  /**
   * Final-review finding (Important #1): a failure whose `Error` carries an EMPTY
   * message set the error state, cleared the previous rows, and rendered NOTHING —
   * the author sees the button flicker "Running…" and then nothing at all, which
   * is indistinguishable from a dead button. Two live routes to it: `api/client.ts`
   * throws `new UnauthorizedError()` with no message on any 401 (and this frontend
   * has no global 401 handler), and `ApiError.from` falls back to `res.statusText`,
   * which is always `""` over HTTP/2 — so an axum extractor rejection, an ingress
   * 413, or a 502 HTML page all arrive empty too. `TryIt` already had the `|| …`
   * fallback; `PreviewBlock` had dropped it.
   */
  it("still says something actionable when the failure carries no message", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      // Exactly what an expired session produces: an `Error` with `message === ""`.
      mutateAsync: vi.fn().mockRejectedValue(new Error("")),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview unsaved draft/i }));
    expect(await screen.findByText(/could not run the preview/i)).toBeInTheDocument();
    // And it must name the thing the author can actually do about it.
    expect(screen.getByText(/session may have expired/i)).toBeInTheDocument();
  });

  /**
   * C2 Task 8. A transformed endpoint's response is the script's output, not the
   * rows — a different value of a different shape, which is a much wider gap than
   * the naming one the note beside the button already covers. Preview cannot show
   * it: it runs ONE statement, and `transform(params, statements)` is handed every
   * statement's rows keyed by name, so the honest answer is the statement's rows
   * plus a sentence saying what they are.
   *
   * Both halves in one test, because the property is the DIFFERENCE — an
   * untransformed preview must NOT carry the note. `queryByText` on the negative
   * side after the positive one has been proven findable, so "absent" cannot mean
   * "the copy was never on screen for either".
   */
  it("says the rows are the statement's, not the response, when a transform is declared", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const answer = (transform_declared: boolean) => ({
      status: 200,
      data: {
        columns: [{ name: "n", type: "integer" }],
        rows: [[1]],
        row_count: 1,
        truncated: false,
        transform_declared,
      },
    });

    const mutateAsync = vi.fn().mockResolvedValue(answer(true));
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    const transformed = renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview unsaved draft/i }));
    expect(await screen.findByText(/not the response a consumer receives/i)).toBeInTheDocument();
    // And it must send the author somewhere the real response exists.
    expect(screen.getByText(/Try it runs the published endpoint for real/i)).toBeInTheDocument();
    transformed.unmount();

    mutateAsync.mockResolvedValue(answer(false));
    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview unsaved draft/i }));
    // The rows arrived — so the absence below is the flag's doing, not an
    // unrendered result.
    expect(await screen.findByText("1 row.")).toBeInTheDocument();
    expect(screen.queryByText(/not the response a consumer receives/i)).not.toBeInTheDocument();
  });

  it("says preview shows the engine's column names, not the published contract's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.4.1: say which is which or the two get confused.
    expect(await screen.findByText(/column names come from the engine/i)).toBeInTheDocument();
  });

  it("says a green preview does not prove a consumer can call it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.1: "The consequence must reach the UI, not just this document."
    //
    // What the consequence IS changed, and this assertion changed with it. The
    // note used to say a preview "runs as you rather than as a service account",
    // and four tests pinned that sentence — but `preview::run` reaches the engine
    // through `delegation_for`, which sends the SERVICE user on any connection
    // that is neither `delegated` nor impersonating. So the identity is NOT the
    // difference on an ordinary connection; what is always true is that a preview
    // authenticates nothing and checks no grant. Both halves are pinned: the
    // conditional identity, and the negative — the old absolute claim must not
    // come back.
    const note = await screen.findByText(/as you only when that connection delegates or impersonates/i);
    expect(note).toHaveTextContent(/otherwise as the shared service account/i);
    expect(note).toHaveTextContent(/authenticates no token, checks no invoke grant/i);
    expect(screen.queryByText(/runs as you rather than as a service account/i)).toBeNull();
  });

  /**
   * Final-review finding (Minor #4): "Preview draft" was the one run-something
   * button that did not name the version it runs. Beside "Validate saved draft"
   * and "Publish saved draft" the bare noun reads as the same SAVED thing, and
   * spec §3.4's stated failure is exactly that inference: preview, see green,
   * publish, get something else. Both halves of the fix — the label and the note —
   * are pinned here.
   */
  it("names the version it runs: the unsaved, on-screen one", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);

    expect(await screen.findByRole("button", { name: "Preview unsaved draft" })).toBeInTheDocument();
    // Not merely "draft": the note has to say WHICH text runs.
    expect(screen.getByText(/as they are on screen right now/i)).toBeInTheDocument();
    expect(screen.getByText(/edits you have not saved/i)).toBeInTheDocument();
  });
});

describe("Query section — layout", () => {
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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  /**
   * The sixth guard, added when §5.1 was reversed to one width for all six
   * tabs. Five sections had this assertion and `Query` had none — it was the
   * tab the width pass was FOR, so nobody thought it could regress, which is
   * the same reasoning that left `Config` unguarded until a restored
   * `max-w-3xl` on its own root passed 230/230 (§5.5).
   *
   * "All six leave width to the page" is not a claim five assertions can make.
   *
   * Class name only (§5.5); the six agreeing is measured in
   * `tests/e2e/data-api-detail-tab-widths.spec.ts`.
   */

  /**
   * §5.1's intro rule, pinned where the class actually is.
   *
   * The class assertion is the RIGHT instrument here rather than a stand-in for
   * a measurement — see `expectSectionIntroCapsItsProse`. Before this test the
   * only guard on this cap was a Playwright spec that has never executed, and a
   * population count that `Response` and `Access` could satisfy without their
   * intro being capped at all.
   */
  it("caps its intro at a reading measure, not the section's (§5.1)", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint }, isLoading: false, isError: false, refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    expectSectionIntroCapsItsProse(
      await screen.findByRole("heading", { name: "Query", level: 2 }),
    );
  });

  it("leaves width to the page, like all six tabs (§5.1, as reversed)", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);

    const { container } = renderQuery(endpoint);
    const heading = await screen.findByRole("heading", { name: "Query", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
  });

  it("holds the database picker, the SQL editor and the parameter declarations", async () => {
    const endpoint = makeEndpoint({
      params: [
        {
          name: "region",
          type: ParamType.string,
          location: ParamLocation.query,
          required: true,
          description: "",
        },
      ],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);

    renderQuery(endpoint);
    expect(await screen.findByLabelText("Database")).toBeInTheDocument();
    expect(screen.getByLabelText("SQL")).toBeInTheDocument();
    // The DECLARATION, from ParamEditor…
    expect(screen.getByDisplayValue("region")).toBeInTheDocument();
    // …and a separate test-value input for the same parameter, which is the
    // distinction merging the two would destroy.
    expect(screen.getByLabelText(/^region/)).toBeInTheDocument();
  });

  it("does not hold name, path or the config tables — those are Config's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Path")).not.toBeInTheDocument();
    expect(screen.queryByText("Rate limit")).not.toBeInTheDocument();
  });
});

// Relocated from `EndpointDetail.test.tsx` (Task 4): these tests exercise the
// shared dirty-tracking / save machinery (`EndpointDetailInner`) through the
// SQL field, which now lives here rather than in the old `Definition`.
describe("Query section — dirtiness and save", () => {
  const updateMutateAsync = vi.fn();

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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    updateMutateAsync.mockReset();
    updateMutateAsync.mockImplementation(
      ({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
        Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }),
    );
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: updateMutateAsync,
      isPending: false,
    } as never);
  });

  it("arms the unsaved-changes warning only after a real change, and disarms it when reverted", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    const sql = await screen.findByDisplayValue("SELECT 1");
    // Radix hydration and controlled-input echo must not count as a change —
    // not even a transient one: a flag that latches `true` during hydration
    // and only "catches up" to `false` a render later would still show the
    // phantom dialog to anyone who closes the tab in that window, so the
    // stronger assertion is that `true` is never seen at all, not just that
    // the LAST call happens to be `false`.
    expect(warnSpy).not.toHaveBeenCalledWith(true);
    expect(warnSpy).toHaveBeenLastCalledWith(false);

    await userEvent.type(sql, " -- x");
    expect(warnSpy).toHaveBeenLastCalledWith(true);

    // The property that actually separates "compare to baseline" from "a
    // flag that only latches": reverting the edit back to the loaded value
    // must disarm the warning. A `touched` flag can never do this without
    // extra reset logic bolted on; the comparison gets it for free.
    await userEvent.type(sql, "{Backspace}".repeat(5));
    expect(sql).toHaveValue("SELECT 1");
    expect(warnSpy).toHaveBeenLastCalledWith(false);
  });

  /**
   * Final-review finding (Minor #6b): §3.1's entire reason for splitting the
   * DECLARATION from the TEST VALUE is that a test value is not part of the
   * endpoint — it lives in `PreviewBlock`'s own `useState`, never in the shared
   * `form`, so it cannot be saved by accident and `dirty` cannot latch on typing
   * one. The code was right and nothing asserted it: moving `setValues` into
   * `setForm` passed every test in the suite.
   */
  it("does not count typing a preview test value as an unsaved change", async () => {
    const endpoint = makeEndpoint({
      params: [
        {
          name: "region",
          type: ParamType.string,
          location: ParamLocation.query,
          required: true,
          description: "",
        },
      ],
    } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);

    const testValue = await screen.findByLabelText(/^region/);
    await userEvent.type(testValue, "EU");
    expect(testValue).toHaveValue("EU");

    // Nothing about the endpoint changed, so: no header badge, Save stays
    // disabled, and the beforeunload warning is never armed.
    expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
    expect(warnSpy).not.toHaveBeenCalledWith(true);
  });

  it("re-baselines after a successful save", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(warnSpy).toHaveBeenLastCalledWith(false));
  });

  // Pins the hydrate-once guard: ChartExplore and NotebookSettings have both
  // shipped a regression where re-seeding the form on every data reference
  // (rather than once) clobbers a mid-edit form the instant react-query hands
  // back a new object identity for unchanged data (e.g. a background refetch).
  it("does not clobber a mid-edit field when the query result gets a new object identity", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- x");
    expect(sql).toHaveValue("SELECT 1 -- x");

    // Simulate a react-query refetch that resolves to an equal-but-new object.
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: { ...endpoint } },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    // Force a re-render by re-rendering the same tree is not enough on its own
    // (React Testing Library doesn't re-run hooks without a state change in the
    // tree), so instead assert the value survived the mock swap plus a fresh
    // paint triggered by a further keystroke.
    await userEvent.type(sql, "y");
    expect(sql).toHaveValue("SELECT 1 -- xy");
  });

  // Reviewer finding (Critical #1): a bare "ran once" ref never re-arms when
  // only the `:id` route param changes, because React Router reconciles the
  // SAME `EndpointDetail` instance across `/endpoints/A` → `/endpoints/B`.
  // Without keying the hydration guard (and the nested Outlet) by id, this
  // would show B's name in the header over A's SQL/path/params/database —
  // and a subsequent Save would silently overwrite B's record with A's data.
  it("shows the newly-navigated endpoint's own fields, not the previous one's, after an in-app navigation", async () => {
    const epA = makeEndpoint({ id: "ep-a", name: "Endpoint A", statements: [{ name: "main", sql: "SELECT A" }] } as Partial<DataApiEndpointDto>);
    const epB = makeEndpoint({ id: "ep-b", name: "Endpoint B", statements: [{ name: "main", sql: "SELECT B" }] } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockImplementation(((_ws: string, id: string) => ({
      data: { status: 200, data: id === "ep-a" ? epA : epB },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })) as unknown as typeof api.useDataApiGetEndpoint);

    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints/ep-a"]}>
          <Routes>
            {/* The `Link` inside the SAME route element is what reproduces the
                bug: the route pattern (and so the rendered `EndpointDetail`
                element) never changes across this navigation, only `:id` does —
                matching what a future in-app link between endpoints (Tasks
                8/9) will look like. */}
            <Route
              path="/explore/:workspace/data-api/endpoints/:id"
              element={
                <>
                  <Link to="/explore/eng/data-api/endpoints/ep-b">Go to B</Link>
                  <EndpointDetail />
                </>
              }
            >
              <Route index element={<Query />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await screen.findByDisplayValue("SELECT A");
    await user.click(screen.getByText("Go to B"));

    await waitFor(() => expect(screen.getByLabelText("SQL")).toHaveValue("SELECT B"));
    // The visible breadcrumb heading (`h2`), not `getByText` — the page also
    // carries an `sr-only` `h1` with the same name, so a bare text match is
    // now ambiguous.
    expect(screen.getByRole("heading", { level: 2, name: "Endpoint B" })).toBeInTheDocument();
    expect(screen.queryByDisplayValue("SELECT A")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 2, name: "Endpoint A" })).not.toBeInTheDocument();
  });

  it("saves only the changed fields, not the whole form (a partial PATCH)", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call).toMatchObject({ workspace: "ws1", id: "ep-1" });
    // Reviewer finding (Important #6): `UpdateEndpointInput`'s own doc says an
    // absent field means "leave it alone" — a whole-document PATCH built from
    // a form hydrated once at mount would silently revert any change made
    // elsewhere since. Only `sql` changed, so only `sql` should be sent.
    expect(call.data).toEqual({ statements: [{ name: "main", sql: "SELECT 1 -- x" }] });
  });

  // Only pins Preview's own half of §3.4 (stays enabled, and Save stays
  // enabled beside it) — it does NOT reach Validate or Publish, which live in
  // Config and Versions, different routes from Query. Those two halves of
  // the same property are pinned separately below and in the
  // "Publish and the shared dirty flag" block, each by navigating there with
  // the edit still unsaved (see the doc comment on that block for why the
  // navigation itself is load-bearing).
  it("keeps Preview and Save enabled while dirty", async () => {
    const endpoint = makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>);
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- edited");
    expect(screen.getByRole("button", { name: /preview unsaved draft/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /^save$/i })).toBeEnabled();
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });
});

/**
 * The other half of the same §3.4 property the block above pins: Validate is
 * disabled while dirty, unlike Preview. Mirrors
 * "Query section — Publish and the shared dirty flag" below — Validate lives
 * in `Config`, a different route from `Query`, and the five sections share
 * ONE form that survives section navigation, so this mounts Query AND Config
 * under the real shell and NAVIGATES between them with the edit still
 * unsaved, the same way that block does for Publish and Versions.
 */
describe("Query section — Validate and the shared dirty flag", () => {
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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiValidateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  function renderQueryAndConfig(endpoint: DataApiEndpointDto = makeEndpoint()) {
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
              <Route path="config" element={<Config />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
  }

  it("disables Validate for an edit made in Query and carried unsaved into Config", async () => {
    renderQueryAndConfig();
    const sql = await screen.findByDisplayValue("SELECT 1");
    await userEvent.type(sql, " -- unsaved");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("link", { name: "Config" }));
    const validate = await screen.findByRole("button", { name: "Validate saved draft" });

    expect(validate).toBeDisabled();
    expect(validate).toHaveAttribute("title", expect.stringContaining("Save your changes"));
  });
});

// Relocated from `EndpointDetail.test.tsx` (Task 4): parameters (declaration
// + the allowed-values field) are `ParamEditor`'s, which lives in `Query`.
describe("Query section — ParamEditor", () => {
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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  it("does not offer a location control for parameters", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.string, location: ParamLocation.query, required: false }],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    // Confirms ParamEditor actually rendered a row — otherwise this assertion
    // would pass trivially against an empty editor with nothing to check.
    await screen.findByRole("combobox", { name: "Parameter 1 type" });
    expect(screen.queryByText(/location/i)).not.toBeInTheDocument();
  });

  /**
   * The declarations are a TABLE — one row per parameter — replacing the
   * card-per-parameter stack (a bordered box with a `grid grid-cols-2` inside,
   * per parameter) that `docs/en/ui-conventions.md` §4 records the complaint
   * about.
   *
   * **Every fixture here has TWO parameters, and that is the point.** With one
   * row, "wrote the edit to the row it was made in" and "wrote the edit to
   * whichever row it felt like" are the same observation — `update(0, …)`
   * hard-coded would pass every single-row test in this file. Two rows with
   * different values in every field is the smallest fixture that can tell them
   * apart.
   *
   * They assert the params the shell PATCHes, which is exactly what
   * `ParamEditor`'s `onChange` wrote into the shared form — not that `onChange`
   * fired, which an editor that wrote `[]` on every keystroke would also
   * satisfy.
   */
  describe("the parameters table", () => {
    const TWO: ParamSpec[] = [
      {
        name: "region",
        type: ParamType.string,
        location: ParamLocation.query,
        required: false,
        default: null,
        description: "Sales region",
      },
      {
        name: "limit",
        type: ParamType.number,
        location: ParamLocation.query,
        required: false,
        default: null,
        description: "Row cap",
      },
    ];

    function renderParams(params: ParamSpec[]) {
      const endpoint = makeEndpoint({ params } as Partial<DataApiEndpointDto>);
      vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
        data: { status: 200, data: endpoint },
        isLoading: false,
        isError: false,
        refetch: vi.fn(),
      } as never);
      // Echoes the PATCH back, the way the server does — the shell re-seeds
      // the whole form from this response. A mock that resolved with the
      // ORIGINAL record instead would hand the editor back a params array
      // LONGER than the one it just shortened, which is a state the server
      // cannot produce (it stores what it was sent) but which does make this
      // editor's `rowIds[i] ?? i` fallback emit two rows with the same React
      // key. Worth writing down: it is why this mock is an implementation and
      // not a `mockResolvedValue`.
      const updateMutateAsync = vi.fn().mockImplementation(({ data }: { data: Partial<DataApiEndpointDto> }) =>
        Promise.resolve({ status: 200, data: { ...endpoint, ...data }, headers: new Headers() }),
      );
      vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
        mutateAsync: updateMutateAsync,
        isPending: false,
      } as never);
      return { ...renderQuery(endpoint), updateMutateAsync };
    }

    /** The declaration array as the shell sends it — what `onChange` produced. */
    async function saveAndReadParams(updateMutateAsync: ReturnType<typeof vi.fn>): Promise<ParamSpec[]> {
      await userEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
      const body = updateMutateAsync.mock.calls[0][0] as { data: { params: ParamSpec[] } };
      return body.data.params;
    }

    /** The parameters table, found through a header it owns rather than by
     *  position — Query gains other tables in the sections around this one. */
    function paramRows() {
      const table = screen.getByRole("columnheader", { name: "Required" }).closest("table") as HTMLElement;
      return within(table).getAllByRole("row");
    }

    it("gives each declaration one row, with a column per field", async () => {
      renderParams(TWO);
      await screen.findByRole("combobox", { name: "Parameter 1 type" });

      for (const col of ["Name", "Type", "Required", "Default", "Description"]) {
        expect(screen.getByRole("columnheader", { name: col })).toBeInTheDocument();
      }

      // Header + one row each. Not "at least two": a card-per-parameter stack
      // wrapped in a table would also produce rows.
      const rows = paramRows();
      expect(rows).toHaveLength(3);
      // Row order follows declaration order, and each row holds ITS OWN
      // parameter — the fields are read from inside the row element, so a
      // layout that renders every field of both parameters into one row fails.
      expect(within(rows[1]).getByLabelText("Parameter 1 name")).toHaveValue("region");
      expect(within(rows[1]).getByLabelText("Parameter 1 description")).toHaveValue("Sales region");
      expect(within(rows[2]).getByLabelText("Parameter 2 name")).toHaveValue("limit");
      expect(within(rows[2]).getByLabelText("Parameter 2 description")).toHaveValue("Row cap");
    });

    it("writes a typed value to the row it was typed in", async () => {
      const { updateMutateAsync } = renderParams(TWO);
      const secondDefault = await screen.findByLabelText("Parameter 2 default");
      await userEvent.type(secondDefault, "50");

      // The whole array, not just the row that was touched: `update(1, …)`
      // that also clears its neighbour is as wrong as writing to row 0.
      expect(await saveAndReadParams(updateMutateAsync)).toEqual([
        { ...TWO[0], default: null },
        { ...TWO[1], default: "50" },
      ]);
    });

    it("toggles Required on one row without touching the other", async () => {
      const { updateMutateAsync } = renderParams(TWO);
      // Both start `false`, so "set them all" is distinguishable from "set the
      // one that was clicked" — with one already `true` it would not be.
      await userEvent.click(await screen.findByRole("checkbox", { name: "Parameter 2 required" }));

      expect((await saveAndReadParams(updateMutateAsync)).map((p) => !!p.required)).toEqual([false, true]);
    });

    // The SECOND row, deliberately. Removing the first is the one click a
    // hard-coded `remove(0)` also gets right — written that way first, and the
    // mutation survived: 35/35 green with the index thrown away.
    it("removes the row whose control was used, not the first one", async () => {
      const { updateMutateAsync } = renderParams(TWO);
      await userEvent.click(await screen.findByRole("button", { name: "Remove parameter 2" }));

      // The whole surviving declaration, not just its name: a row-shifting bug
      // (the `rowIds` bookkeeping this editor exists to get right) can leave
      // the correct count with fields from two declarations mixed into one row.
      expect(await saveAndReadParams(updateMutateAsync)).toEqual([TWO[0]]);
    });

    it("appends a blank declaration and renumbers nothing above it", async () => {
      const { updateMutateAsync } = renderParams(TWO);
      await userEvent.click(await screen.findByRole("button", { name: /add parameter/i }));

      expect(paramRows()).toHaveLength(4);
      expect(screen.getByLabelText("Parameter 3 name")).toHaveValue("");
      expect(screen.getByLabelText("Parameter 1 name")).toHaveValue("region");
      expect(await saveAndReadParams(updateMutateAsync)).toEqual([
        ...TWO,
        { name: "", type: ParamType.string, location: ParamLocation.query, required: false },
      ]);
    });

    // The one per-row conditional the cards had, and the thing a table most
    // easily loses: `allowed` belongs to the enum row, and to no other.
    it("shows allowed values inside the enum's own row and nowhere else", async () => {
      renderParams([{ ...TWO[0], type: ParamType.enum, allowed: ["eu", "us"] }, TWO[1]]);
      const allowed = await screen.findByLabelText("Parameter 1 allowed values");

      const rows = paramRows();
      expect(rows[1].contains(allowed)).toBe(true);
      expect(allowed).toHaveValue("eu, us");
      // Not a table-wide field, and not a column every row pays for.
      expect(screen.queryByLabelText("Parameter 2 allowed values")).not.toBeInTheDocument();
    });

    it("keeps the empty state, and the sentence that says where a parameter comes from", async () => {
      renderParams([]);
      expect(await screen.findByText(/No parameters yet/i)).toBeInTheDocument();
      expect(screen.getByText("{{ name }}")).toBeInTheDocument();
      // The columns stay on screen with no rows: they are what tells an author
      // what a parameter is made of before they add one.
      expect(screen.getByRole("columnheader", { name: "Name" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /add parameter/i })).toBeInTheDocument();
    });
  });

  // Reviewer finding (Critical #2): the field used to be fully controlled —
  // `value={allowed.join(", ")}` with an `onChange` that immediately
  // re-split/trimmed/filtered — which silently ate a trailing comma on every
  // keystroke (existing "a", type "," → "a," → normalized straight back to
  // "a"), so a second value could never be started by typing.
  it("accepts a comma typed as a separate keystroke when building an enum's allowed values", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.enum, location: ParamLocation.query, required: true, allowed: [] }],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const updateMutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: makeEndpoint({
        params: [{ name: "region", type: ParamType.enum, location: ParamLocation.query, required: true, allowed: ["a", "b"] }],
      }),
      headers: new Headers(),
    });
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: updateMutateAsync, isPending: false } as never);

    renderQuery(endpoint);
    const allowed = await screen.findByLabelText("Parameter 1 allowed values");

    await userEvent.type(allowed, "a,b");
    expect(allowed).toHaveValue("a,b");

    // Clicking elsewhere blurs the field, committing the projected array —
    // and re-formats the display for a stray typo in spacing.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(allowed).toHaveValue("a, b");

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call.data.params[0].allowed).toEqual(["a", "b"]);
  });

  /**
   * Final-review finding (#8, second half): `AllowedValuesField` holds its
   * text locally so a trailing comma is not eaten mid-typing — but the local
   * copy must not survive a Save. `EndpointDetail` re-seeds the whole form
   * from the SERVER RESPONSE, so a server-normalised `allowed` list has to
   * reach the field, and the field's stale text must never be pushed back
   * over it.
   */
  describe("the allowed-values field resyncs from the server", () => {
    const ENUM_PARAM = {
      name: "region",
      type: ParamType.enum,
      location: ParamLocation.query,
      required: false,
      allowed: ["eu", "us"],
    };

    it("shows the server's normalised list after a Save, not the pre-save text", async () => {
      const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
      vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
        data: { status: 200, data: endpoint },
        isLoading: false,
        isError: false,
        refetch: vi.fn(),
      } as never);
      // The server normalises: it lower-cases and de-duplicates what was sent.
      const updateMutateAsync = vi.fn().mockResolvedValue({
        status: 200,
        data: makeEndpoint({ params: [{ ...ENUM_PARAM, allowed: ["eu", "us", "apac"] }] } as Partial<DataApiEndpointDto>),
        headers: new Headers(),
      });
      vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: updateMutateAsync, isPending: false } as never);
      renderQuery(endpoint);

      const allowed = await screen.findByLabelText("Parameter 1 allowed values");
      await userEvent.clear(allowed);
      await userEvent.type(allowed, "EU, US, APAC");
      await userEvent.tab();

      await userEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());

      // What the server returned, not what was typed.
      await waitFor(() => expect(screen.getByLabelText("Parameter 1 allowed values")).toHaveValue("eu, us, apac"));
    });

    // The other half of the same defect: a focus/blur that typed nothing
    // must not `commit()` a stale local string back over the prop.
    it("commits nothing on a focus/blur that typed nothing", async () => {
      const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
      vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
        data: { status: 200, data: endpoint },
        isLoading: false,
        isError: false,
        refetch: vi.fn(),
      } as never);
      renderQuery(endpoint);

      const allowed = await screen.findByLabelText("Parameter 1 allowed values");
      await userEvent.click(allowed);
      await userEvent.tab();

      expect(screen.getByLabelText("Parameter 1 allowed values")).toHaveValue("eu, us");
      // Touching a field without changing it is not an edit.
      expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
    });
  });
});

/**
 * Final-review finding (Important #3): Publish ignored the shared `dirty` flag.
 *
 * The five sections share ONE form that survives section navigation
 * (`EndpointDetail` keys the `Outlet` by id, not by pathname), so an author
 * could edit SQL in Query, walk to Versions with "Unsaved changes" still in
 * the header, and publish — freezing the SAVED draft while the dialog said
 * "Freezes the current draft", which they would read as the one on screen.
 * Config already guards the far cheaper Validate this way; Publish changes
 * what every deployed consumer receives.
 *
 * This mounts Query AND Versions under the real shell and NAVIGATES between
 * them, because the bug only exists across that navigation — a test that
 * mounted Versions alone could never see it.
 */
describe("Query section — Publish and the shared dirty flag", () => {
  const publishAsync = vi.fn();

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
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(accessApi.listUsers).mockResolvedValue([]);

    publishAsync.mockReset();
    publishAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-3", version: 3, state: "live" },
      headers: new Headers(),
    });
    vi.mocked(dataApi.usePublishEndpoint).mockReturnValue({ mutateAsync: publishAsync, isPending: false } as never);
    vi.mocked(dataApi.useRollbackEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue({
      data: { status: 200, data: [] },
      isLoading: false,
      isError: false,
    } as never);
  });

  function renderBoth(endpoint: DataApiEndpointDto = makeEndpoint({ live_version_id: "v-2", state: "live" })) {
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
              <Route index element={<Query />} />
              <Route path="versions" element={<VersionsSection />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
  }

  it("refuses to publish an edit made in Query and carried unsaved into Versions", async () => {
    renderBoth();
    const sql = await screen.findByDisplayValue("SELECT 1");
    await userEvent.type(sql, " -- unsaved");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    const publish = await screen.findByRole("button", { name: "Publish saved draft" });

    expect(publish).toBeDisabled();
    expect(publish).toHaveAttribute("title", expect.stringContaining("Save your changes"));

    // Not merely visually disabled: the confirmation never opens, so the
    // publish cannot be reached at all.
    await userEvent.click(publish);
    expect(screen.queryByText(/does not grant data access/i)).not.toBeInTheDocument();
    expect(publishAsync).not.toHaveBeenCalled();
  });

  it("publishes normally once there is nothing unsaved", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));

    const publish = await screen.findByRole("button", { name: "Publish saved draft" });
    expect(publish).toBeEnabled();
    await userEvent.click(publish);
    expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
  });

  // The dialog's own copy is part of the fix: "the current draft" is what the
  // author reads as "the one I am looking at".
  it("says the SAVED draft is what gets frozen", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish saved draft" }));
    expect(await screen.findByText(/freezes the saved draft/i)).toBeInTheDocument();
  });
});

/**
 * The Query section as a full-height editing surface: a collapsible strip of
 * tables, then the SQL editor above the preview with a drag handle between
 * them (`components/sqllab/SqlTab.tsx`'s shape, deliberately reused).
 *
 * **What these can and cannot see.** jsdom does no layout, so no test here
 * observes that the editor fills the height or that the handle drags — and
 * `react-resizable-panels` is stubbed above precisely because its real
 * implementation is meaningless without rects. What IS checkable, and is where
 * the regressions live, is the wiring: which panel each half is in, that the
 * sizes are percentages rather than pixels, that ⌘↵ reaches the preview, and
 * that a trashed endpoint's editor refuses edits. The drag itself is the
 * browser pass in the plan.
 */
describe("Query section — the editor, the split and the preview", () => {
  const previewMutateAsync = vi.fn();

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
    monacoStub.reset();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    // The shell builds this on every render, trashed or not — hooks cannot be
    // conditional — so the automock's `undefined` breaks every test here
    // without it, not only the trashed ones.
    vi.mocked(dataApi.useRestoreEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    previewMutateAsync.mockResolvedValue({
      status: 200,
      data: { columns: [], rows: [], row_count: 0, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
  });

  function show(endpoint: DataApiEndpointDto) {
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    return renderQuery(endpoint);
  }

  it("splits the section vertically — editor above, preview below — and sizes it in PERCENT", async () => {
    show(makeEndpoint());
    await screen.findByLabelText("SQL");

    const group = screen.getByTestId("panel-group");
    expect(group).toHaveAttribute("data-orientation", "vertical");
    // `min-h-0` is the half that regresses silently: a flex item's default
    // `min-height: auto` refuses to shrink below its content, and this
    // panel's content is an editor that will happily claim the whole page.
    expect(group).toHaveClass("min-h-0", "flex-1");

    // The section's own root is the full-height column the split lives in,
    // walked from the group rather than picked out by selector.
    const root = group.parentElement!;
    expect(root).toHaveClass("flex", "h-full", "flex-col");
    expect(root.parentElement?.tagName).toBe("FIELDSET");

    const panels = screen.getAllByTestId("panel");
    expect(panels).toHaveLength(2);
    // The editor is in the FIRST panel and the preview in the SECOND — the
    // whole point of the change is that the preview stopped being below the
    // parameter editor, so which panel holds what is the assertion.
    expect(within(panels[0]).getByLabelText("SQL")).toBeInTheDocument();
    expect(
      within(panels[1]).getByRole("button", { name: /preview unsaved draft/i }),
    ).toBeInTheDocument();

    // …with the drag handle between them, not before or after both.
    const handle = screen.getByTestId("panel-separator");
    expect(panels[0].compareDocumentPosition(handle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panels[1].compareDocumentPosition(handle) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();

    // STRINGS, and percentages. In react-resizable-panels v4 a bare number is
    // PIXELS: `defaultSize={55}` compiles, type-checks, reads correctly in a
    // diff, and gives a 55-pixel editor. Both halves of this are needed — a
    // string `"55"` is also percent, so the type alone is not enough, and a
    // `%` suffix cannot be written on a number at all.
    for (const panel of panels) {
      expect(panel).toHaveAttribute("data-default-size-type", "string");
      expect(panel).toHaveAttribute("data-min-size-type", "string");
      expect(panel.getAttribute("data-default-size")).toMatch(/^\d+%$/);
      expect(panel.getAttribute("data-min-size")).toMatch(/^\d+%$/);
    }
    // The editor gets the larger share — the plan's approved layout.
    expect(panels[0]).toHaveAttribute("data-default-size", "55%");
    expect(panels[1]).toHaveAttribute("data-default-size", "45%");
  });

  it("runs the preview from the editor's ⌘↵, on the SQL that is on screen", async () => {
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- edited");

    // The command the PRODUCT registered, at the keybinding the product
    // computed (`SqlEditor`'s real `handleMount` ran — see the Monaco stub).
    const run = monacoStub.runCommand();
    expect(run).toBeTypeOf("function");
    await act(async () => {
      run!();
    });

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    // Not merely "a preview ran": the unsaved edit is in the body, which is
    // what makes this the same run the button does.
    expect(previewMutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 1 -- edited");
  });

  /**
   * Task 3: the ~570-character note became a disclosure headed by the one
   * sentence a reader must not miss. The panel is unchanged — it still travels
   * with the button, which is `PreviewBlock`'s rule — and the reason the
   * headline is NOT collapsed with the rest is that same rule: a reader who has
   * to open something to learn what the button does has already pressed it.
   */
  it("keeps the preview's note in the panel with the button it describes", async () => {
    show(makeEndpoint());
    await screen.findByLabelText("SQL");

    const previewPanel = screen.getAllByTestId("panel")[1];
    // The load-bearing sentence, on screen with nothing clicked.
    const summary = within(previewPanel).getByText(
      "A green preview does not prove a consumer can call this endpoint",
    );
    const details = summary.closest("details")!;
    expect(details).not.toHaveAttribute("open");

    // The rest is still here and still reachable — nothing was cut.
    for (const re of [
      /as they are on screen right now/i,
      /including edits you have not saved/i,
      /does not save, publish, spend a rate limit/i,
      /Column names come from the engine/i,
      /otherwise as the shared service account/i,
    ]) {
      const node = within(details).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");

    // …and the shortcut is written down where it can be discovered.
    expect(within(previewPanel).getByText("⌘↵")).toBeInTheDocument();
  });

  it("makes the SQL editor read-only for a trashed endpoint, and ⌘↵ runs nothing", async () => {
    const user = userEvent.setup();
    show(makeEndpoint({ state: "disabled", deleted_at: "2026-08-18T09:00:00Z" }));
    const sql = await screen.findByLabelText("SQL");

    // Not through the shell's `<fieldset disabled>`: a fieldset disables form
    // controls, and Monaco is not one. Without an explicit `readOnly` this is
    // a fully editable editor on a page whose banner says everything below is
    // read-only.
    expect(sql).toHaveAttribute("readonly");
    await user.type(sql, "DROP");
    expect(sql).toHaveValue("SELECT 1");

    // And the shortcut bypasses BOTH guards — a read-only Monaco still fires
    // its commands, and the fieldset never sees a keystroke — so it needs one
    // of its own.
    const run = monacoStub.runCommand();
    if (run) {
      await act(async () => {
        run();
      });
    }
    expect(previewMutateAsync).not.toHaveBeenCalled();
  });

  it("leaves the SQL editor writable, and ⌘↵ live, when the endpoint is not in the trash", async () => {
    // The other half of the test above: an editor that is ALWAYS read-only and
    // a ⌘↵ that never fires would pass everything there.
    show(makeEndpoint());
    const sql = await screen.findByLabelText("SQL");

    expect(sql).not.toHaveAttribute("readonly");
    await userEvent.type(sql, " -- x");
    expect(sql).toHaveValue("SELECT 1 -- x");

    await act(async () => {
      monacoStub.runCommand()!();
    });
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
  });

  it("puts the database picker in a config table row rather than loose above the editor", async () => {
    show(makeEndpoint());
    const picker = await screen.findByLabelText("Database");

    // "Everything except the SQL becomes a table" — the picker is a
    // `Setting | Details | Value` row, so it is inside a table with the
    // parameters, not a labelled control floating over the editor.
    const cell = picker.closest("td");
    expect(cell).not.toBeNull();
    const row = cell!.closest("tr")!;
    expect(within(row).getByText("Database")).toBeInTheDocument();
    // And it is in the strip, above the split, not inside either panel.
    for (const panel of screen.getAllByTestId("panel")) {
      expect(panel.contains(picker)).toBe(false);
    }
  });

  /**
   * Measured in a browser at a 900px viewport, where this section gets 615px:
   * with the strip open it took 370 of them and left the SQL editor **107px**,
   * about four lines — the exact problem this redesign exists to fix. Collapsed,
   * the editor gets 295px and the preview 241px.
   *
   * Asserted on the `open` ATTRIBUTE, not on visibility: jsdom does not hide a
   * closed `<details>`'s contents from queries, so `toBeVisible` would pass
   * whether or not the strip were collapsed — which is why the other 41 tests
   * in this file went on passing unchanged when the default flipped.
   */
  it("starts with the strip collapsed, and says what is inside it", async () => {
    show(makeEndpoint({
      params: [
        { name: "region", type: ParamType.string, location: ParamLocation.query, required: true },
      ],
    }));
    const summary = await screen.findByText(/Database and parameters/);
    const details = summary.closest("details")!;

    expect(details.open).toBe(false);
    // The count is what makes a collapsed strip honest — the reader can see
    // there is one declaration in there without opening it.
    expect(summary).toHaveTextContent("Database and parameters (1)");
  });
});

/**
 * Package S Task 8: a statement's SQL can come from a `build_sql` script, and
 * this is the editor for it.
 *
 * **The rule the whole block is about is C2's, in `ResponseSection.tsx`: a
 * control a mode makes inert is REMOVED, and the value it held is CLEARED in
 * the same state patch.** Here that rule is not a preference — `StatementSpec`
 * is `{name, sql}` **or** `{name, script}`, and the backend's hand-written
 * reader refuses a body carrying both, so an editor that hid the SQL box while
 * leaving `sql: "SELECT 1"` underneath would produce a 422 with nothing on
 * screen to point at.
 *
 * Every assertion about a cleared field is therefore a deep equal on the whole
 * statement, never `toBeUndefined()` on `sql`: the fixtures below all start
 * with a real `sql`, so the assertion proves the key went AWAY rather than
 * passing because it was never set.
 */
describe("Query section — a statement's SQL from a script", () => {
  const updateMutateAsync = vi.fn();
  const previewMutateAsync = vi.fn();

  /** The default the editor offers when a statement is switched to a script. */
  const DEFAULT_SCRIPT = 'def build_sql(params):\n    return "SELECT 1"\n';
  const BRANCHING = 'def build_sql(params):\n    if params["grain"] == "daily":\n        return "SELECT 1 AS daily"\n    return "SELECT 1 AS hourly"\n';

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
    monacoStub.reset();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    updateMutateAsync.mockReset();
    updateMutateAsync.mockImplementation(
      ({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
        Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }),
    );
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: updateMutateAsync,
      isPending: false,
    } as never);
    vi.mocked(dataApi.useRestoreEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    // Only the navigation test renders `Config`, but a hook mock cannot be
    // conditional on which route is open.
    vi.mocked(api.useDataApiValidateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    previewMutateAsync.mockResolvedValue({
      status: 200,
      data: { columns: [], rows: [], row_count: 0, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: previewMutateAsync,
      isPending: false,
    } as never);
  });

  /**
   * `withConfig` registers the sibling section route, so a test can leave Query
   * and come back — which UNMOUNTS this section, exactly as the real app does
   * (`App.tsx` gives every section its own child route).
   */
  function show(endpoint: DataApiEndpointDto, strict = false, withConfig = false) {
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const tree = (
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/explore/ws1/data-api/endpoints/${endpoint.id}`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
              <Route index element={<Query />} />
              {withConfig && <Route path="config" element={<Config />} />}
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    );
    return render(strict ? <StrictMode>{tree}</StrictMode> : tree);
  }

  /** The `statements` the one Save actually PATCHed. */
  function savedStatements() {
    expect(updateMutateAsync).toHaveBeenCalledTimes(1);
    return updateMutateAsync.mock.calls[0][0].data.statements;
  }

  const modeToggle = () =>
    screen.getByRole("checkbox", { name: /build this statement.s sql with a script/i });

  it("switches a statement between written SQL and a script", async () => {
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));

    // Written SQL to start with, and no script editor anywhere.
    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT 1");
    expect(screen.queryByLabelText("SQL script")).not.toBeInTheDocument();
    expect(modeToggle()).not.toBeChecked();

    await userEvent.click(modeToggle());

    // REMOVED, not disabled — the C2 rule. A SQL box left on screen `readOnly`
    // would still be showing text that is no longer part of the statement.
    const script = await screen.findByLabelText("SQL script");
    expect(script).toHaveValue(DEFAULT_SCRIPT);
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();

    fireEvent.change(script, { target: { value: BRANCHING } });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    // …and CLEARED. A deep equal on the whole list: `sql` must be gone, not
    // merely hidden — a statement carrying both keys is refused by the reader,
    // and `toBeUndefined()` on a field would pass even if the editor had never
    // written one.
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([{ name: "main", script: BRANCHING }]);
  });

  it("switches a script back to written SQL, and does not leave the script behind", async () => {
    // The other direction, from a statement that has only ever been a script —
    // so "the `script` key went away" cannot be an artifact of the fixture.
    show(
      makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>),
    );

    expect(await screen.findByLabelText("SQL script")).toHaveValue(BRANCHING);
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();
    expect(modeToggle()).toBeChecked();

    await userEvent.click(modeToggle());

    const sql = await screen.findByLabelText("SQL");
    expect(screen.queryByLabelText("SQL script")).not.toBeInTheDocument();
    await userEvent.type(sql, "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([{ name: "main", sql: "SELECT 2" }]);
  });

  /**
   * C2's `clearedByTransform` precedent: an author who ticks the box to look at
   * what a script would be, then unticks it, must get their SQL back. Without
   * this the switch is silent data loss on a control whose whole purpose is to
   * be tried out.
   *
   * Under `StrictMode`, because that is where the C2 version of this broke:
   * React double-invokes a `useState` updater and keeps only the second call's
   * result, so a ref read INSIDE the updater is `null` by the second pass and
   * the restore silently undoes itself.
   */
  it("gives the written SQL back when the script mode is switched off again", async () => {
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>), true);

    await screen.findByLabelText("SQL");
    await userEvent.click(modeToggle());
    expect(await screen.findByLabelText("SQL script")).toBeInTheDocument();
    await userEvent.click(modeToggle());

    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT 1");
    // Stronger than reading the box: the endpoint is back to EXACTLY what was
    // loaded. `dirty` is a `JSON.stringify` comparison against the baseline, so
    // a restore that left a stray `script` key beside the SQL — invisible on
    // screen and a 422 on Save — would still be showing "Unsaved changes" here.
    expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
  });

  /**
   * M2 gave each statement its own contract; the mode belongs to the statement,
   * not to the endpoint. The failure this catches is the natural one — a single
   * `scripted` piece of state for the section — which would flip every tab at
   * once and, worse, write the script into whichever statement was on screen.
   */
  it("keeps each statement's mode independent", async () => {
    show(
      makeEndpoint({
        statements: [
          { name: "rows", sql: "SELECT * FROM t" },
          { name: "total", sql: "SELECT count(*) FROM t" },
        ],
      } as Partial<DataApiEndpointDto>),
    );

    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(modeToggle());
    fireEvent.change(await screen.findByLabelText("SQL script"), { target: { value: BRANCHING } });

    // Back to the first statement: still written SQL, still its own text, and
    // the toggle reads the statement on screen rather than the section.
    await userEvent.click(screen.getByRole("tab", { name: "rows" }));
    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT * FROM t");
    expect(screen.queryByLabelText("SQL script")).not.toBeInTheDocument();
    expect(modeToggle()).not.toBeChecked();

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    // The whole list: the untouched statement must be byte-identical, and the
    // switched one must carry exactly one source key.
    expect(savedStatements()).toEqual([
      { name: "rows", sql: "SELECT * FROM t" },
      { name: "total", script: BRANCHING },
    ]);
  });

  it("previews a generated statement by sending its script, never an empty sql", async () => {
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    // Deep equal on the whole body: `/preview` reads the same two-case source
    // as `StatementSpec` and refuses both keys, so an added `sql: ""` here is a
    // 422 — and `sql: ""` is exactly what a body built from the old `sqlOf`
    // would carry.
    expect(previewMutateAsync.mock.calls[0][0].data).toEqual({
      script: BRANCHING,
      siblings: [],
      params: [],
      values: {},
      output: {},
      database_id: "db-1",
    });
  });

  it("shows the SQL the script built, beside the rows", async () => {
    previewMutateAsync.mockResolvedValue({
      status: 200,
      data: {
        columns: [{ name: "n", type: "integer" }],
        rows: [[1]],
        row_count: 1,
        truncated: false,
        generated_sql: "SELECT 1 AS daily WHERE region = {{ region }}",
      },
    });
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    // The template, `{{ }}` intact — the artefact the script is responsible for
    // and the only way an author sees which branch their test values took.
    expect(await screen.findByText("SELECT 1 AS daily WHERE region = {{ region }}")).toBeInTheDocument();
  });

  it("shows no generated-SQL block for a statement the author wrote", async () => {
    // The discriminating half: a block that always renders would pass the test
    // above while telling every author their typed SQL was "built by a script".
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL");
    await userEvent.click(screen.getByRole("button", { name: /preview unsaved draft/i }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(screen.queryByText(/sql this script built/i)).not.toBeInTheDocument();
  });

  it("runs the preview from the script editor's ⌘↵ too", async () => {
    // The note beside the Preview button promises ⌘↵ "in the editor"; with the
    // SQL box removed, the script editor is the editor.
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    const run = monacoStub.runCommand();
    expect(run).toBeTypeOf("function");
    await act(async () => {
      run!();
    });
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.script).toBe(BRANCHING);
  });

  it("binds the script editor's ⌘↵ as its own action, disposed when the script editor goes", async () => {
    // An `addCommand` would outlive the script editor: switching back to SQL
    // removes it, and its ⌘↵ would still answer in the SQL editor that replaced
    // it (the newest registration wins only until then; it is never removed).
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    const runKey = monacoStub.KeyMod.CtrlCmd | monacoStub.KeyCode.Enter;
    const script = monacoStub.actions.filter((a) => a.id === "egret.dataapi.preview");
    expect(script).toHaveLength(1);
    expect(script[0].keybindings).toEqual([runKey]);
    expect(script[0].disposed).toBe(false);

    await userEvent.click(screen.getByRole("checkbox", { name: /build this statement.s sql with a script/i }));
    await screen.findByLabelText("SQL");
    expect(script[0].disposed).toBe(true);
    // The SQL editor that replaced it binds its own, live.
    const sql = monacoStub.actions.filter((a) => a.id === "egret.sql.run");
    expect(sql.at(-1)?.disposed).toBe(false);
  });

  it("disposes the script editor's first action when onMount runs a second time", async () => {
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    const script = () => monacoStub.actions.filter((a) => a.id === "egret.dataapi.preview");
    expect(script()).toHaveLength(1);

    act(() => monacoStub.remount!());

    expect(script()).toHaveLength(2);
    expect(script()[0].disposed).toBe(true);
    expect(script()[1].disposed).toBe(false);
  });

  it("makes the script editor read-only for a trashed endpoint, and ⌘↵ runs nothing", async () => {
    show(
      makeEndpoint({
        statements: [{ name: "main", script: BRANCHING }],
        state: "disabled",
        deleted_at: "2026-08-18T09:00:00Z",
      } as Partial<DataApiEndpointDto>),
    );

    // Monaco is not a form control, so the shell's `<fieldset disabled>` does
    // not reach it — asserted on the editor's own attribute, and NOT with
    // `toBeDisabled()`, which would walk up to that fieldset and pass with the
    // guard deleted.
    const script = await screen.findByLabelText("SQL script");
    expect(script).toHaveAttribute("readonly");
    await userEvent.type(script, "fail(");
    expect(script).toHaveValue(BRANCHING);

    const run = monacoStub.runCommand();
    if (run) {
      await act(async () => {
        run();
      });
    }
    expect(previewMutateAsync).not.toHaveBeenCalled();
  });

  it("leaves the script editor writable when the endpoint is not in the trash", async () => {
    // The other half: an editor that is always read-only passes the test above.
    show(makeEndpoint({ statements: [{ name: "main", script: BRANCHING }] } as Partial<DataApiEndpointDto>));

    const script = await screen.findByLabelText("SQL script");
    expect(script).not.toHaveAttribute("readonly");
    fireEvent.change(script, { target: { value: DEFAULT_SCRIPT } });
    expect(script).toHaveValue(DEFAULT_SCRIPT);
  });

  /**
   * The three structural operations, one test each — the reviewer deleted all
   * three `discarded.current = {}` clears and 75/75 tests still passed.
   *
   * The memory is keyed by POSITION, which is the only key available (a
   * statement has no id, and its name is not unique while it is being renamed).
   * A position is exactly what `move` and `removeActive` change under it, so
   * those two must forget; `addStatement` only ever appends, so it must NOT —
   * and that asymmetry is why "clear it everywhere, defensively" is not the
   * answer. Each test below fails for a different one of the three.
   */
  it("does not resurrect a removed statement's SQL into the one that takes its place", async () => {
    show(
      makeEndpoint({
        statements: [
          { name: "rows", sql: "SELECT A" },
          { name: "total", script: BRANCHING },
        ],
      } as Partial<DataApiEndpointDto>),
    );

    // Switch `rows` to a script: its SQL is now remembered at position 0.
    await screen.findByLabelText("SQL");
    await userEvent.click(modeToggle());
    await screen.findByLabelText("SQL script");

    // Remove it. `total` slides into position 0 and is selected — and it is
    // scripted, so the remembered entry's key matches the mode being switched
    // TO and nothing else stands between it and the restore.
    await userEvent.click(screen.getByRole("button", { name: /^remove statement$/i }));
    await userEvent.click(modeToggle());

    // The statement on screen is `total`, and it must be EMPTY. Anything else
    // is a deleted statement's SQL wearing a surviving statement's name.
    expect(await screen.findByLabelText("SQL")).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([{ name: "total", sql: "" }]);
  });

  it("does not carry an undo across a reorder", async () => {
    show(
      makeEndpoint({
        statements: [
          { name: "rows", sql: "SELECT A" },
          { name: "total", script: BRANCHING },
        ],
      } as Partial<DataApiEndpointDto>),
    );

    await screen.findByLabelText("SQL");
    await userEvent.click(modeToggle());
    await screen.findByLabelText("SQL script");

    // `rows` moves to the end; `total` is now at position 0, where `rows`'s
    // discarded SQL is remembered.
    await userEvent.click(screen.getByRole("button", { name: "Move down" }));
    await userEvent.click(screen.getByRole("tab", { name: "total" }));
    await userEvent.click(modeToggle());

    expect(await screen.findByLabelText("SQL")).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([
      { name: "total", sql: "" },
      { name: "rows", script: DEFAULT_SCRIPT },
    ]);
  });

  it("keeps a statement's undo when another statement is added", async () => {
    // The other side of the asymmetry, and the reason a blanket clear is wrong:
    // adding appends, so every existing position still means what it meant, and
    // forgetting here would throw away an undo for no reason at all.
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL");
    await userEvent.click(modeToggle());
    await screen.findByLabelText("SQL script");

    await userEvent.click(screen.getByRole("button", { name: /add statement/i }));
    await userEvent.click(screen.getByRole("tab", { name: "main" }));
    await userEvent.click(modeToggle());

    expect(await screen.findByLabelText("SQL")).toHaveValue("SELECT 1");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(savedStatements()).toEqual([
      { name: "main", sql: "SELECT 1" },
      { name: "statement_2", sql: "" },
    ]);
  });

  /**
   * The undo is a within-visit convenience and cannot be anything else — the
   * discarded text is not in `form`, which is the whole point of discarding it.
   * So the limit is stated BEFORE the click rather than discovered after it:
   * an author who cannot tell "switching back restores this" from "switching
   * back does not" has been promised something the section only sometimes
   * keeps.
   */
  it("says what the switch discards, and how long the undo lasts, before the box is ticked", async () => {
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL");
    // Task 3: one line beside the checkbox — the CONSEQUENCE — with the limit
    // of the undo behind it. Three lines of standing prose above a Monaco
    // editor is height subtracted from the editor on every render.
    const warning = screen.getByText(/discards this statement.s SQL/i);
    expect(warning.closest("summary")).not.toBeNull();
    const details = warning.closest("details")!;
    expect(details).not.toHaveAttribute("open");
    // Still here, still reachable — and INSIDE the disclosure, or nothing has
    // been taken off the editor's height.
    const body = within(details).getByText(/leaving this section or reloading the page does not/i);
    expect(body.closest("summary")).toBeNull();
    expect(body).toHaveTextContent(/a statement has one body, not both/i);
    fireEvent.click(warning);
    expect(details).toHaveAttribute("open");

    // And it follows the mode: after the switch it is the SCRIPT that the next
    // switch would discard, so that is what it names.
    await userEvent.click(modeToggle());
    expect(await screen.findByText(/discards this statement.s script/i)).toBeInTheDocument();
  });

  /**
   * The other block that stood above the editor: the script's template rule,
   * 330 characters explaining one checkbox's ON state. Its first sentence IS
   * the rule, so it heads the disclosure and the mechanics sit behind it.
   */
  it("heads the script's template rule and holds the mechanics behind it", async () => {
    show(makeEndpoint({ statements: [{ name: "main", script: "def build_sql(params):\n    return \"SELECT 1\"\n" }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL script");
    const summary = screen.getByText(/returns this statement.s SQL as a template/i);
    expect(summary.closest("summary")).not.toBeNull();
    const details = summary.closest("details")!;
    expect(details).not.toHaveAttribute("open");

    for (const re of [/bound on every call/i, /refused at publish/i]) {
      const node = within(details).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  /**
   * §11.2's trap. This section renders inside the shell's
   * `<fieldset disabled={trashed}>`, which disables every BUTTON under it — a
   * Radix `Collapsible` trigger included — so on a trashed endpoint these
   * caveats would be unopenable, and every text assertion above would still
   * pass because jsdom does not hide a closed disclosure either way.
   *
   * `fireEvent`, not `userEvent`: `userEvent.click` refuses a click under a
   * disabled fieldset even though `fieldset[disabled]` reaches only
   * form-associated descendants, and `<summary>` is not one. The failure mode
   * is a false negative.
   */
  it("keeps the caveats openable on a trashed endpoint", async () => {
    show(
      makeEndpoint({
        statements: [{ name: "main", sql: "SELECT 1" }],
        state: "disabled",
        deleted_at: "2026-08-18T09:00:00Z",
      } as Partial<DataApiEndpointDto>),
    );

    await screen.findByLabelText("SQL");
    const summary = screen.getByText(/discards this statement.s SQL/i);
    const fieldset = summary.closest("fieldset");
    expect(fieldset).not.toBeNull();
    expect(fieldset).toBeDisabled();

    const details = summary.closest("details")!;
    expect(within(details).queryByRole("button")).not.toBeInTheDocument();
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  it("says nothing about discarding when there is nothing to discard", async () => {
    // The discriminating half: a warning that always renders is one an author
    // stops reading, and a blank editor loses nothing.
    show(makeEndpoint({ statements: [{ name: "main", sql: "" }] } as Partial<DataApiEndpointDto>));

    await screen.findByLabelText("SQL");
    expect(screen.queryByText(/discards this statement/i)).not.toBeInTheDocument();
  });

  /**
   * The case the warning exists for, driven end to end: `Query` and `Config`
   * are sibling routes (`App.tsx`), so opening another section UNMOUNTS this
   * one and the memory goes with it. That is not a bug to fix by hoisting the
   * ref into the shell — see `Query.tsx` — it is a limit, and the test pins
   * that the limit is what the sentence above says it is.
   */
  it("does not offer the undo after the section has been left and reopened", async () => {
    show(makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>), false, true);

    await screen.findByLabelText("SQL");
    await userEvent.click(modeToggle());
    await screen.findByLabelText("SQL script");

    await userEvent.click(screen.getByRole("link", { name: "Config" }));
    await screen.findByLabelText("Path");
    await userEvent.click(screen.getByRole("link", { name: "Query" }));

    await userEvent.click(await screen.findByRole("checkbox", { name: /build this statement.s sql with a script/i }));
    // Empty, exactly as the sentence beside the box says. The script is gone
    // too — a statement has one body — which is why the sentence is on screen
    // before the first click and not after the second.
    expect(await screen.findByLabelText("SQL")).toHaveValue("");
  });
});
