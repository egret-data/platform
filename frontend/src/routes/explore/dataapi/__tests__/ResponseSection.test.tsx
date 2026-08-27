import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/**
 * A minimal fake Monaco, following `Query.test.tsx`'s stub (itself following
 * `components/notebook/__tests__/CellEditor.test.tsx`). Real Monaco cannot
 * load in jsdom — no workers, no layout engine, no measurable font — so the
 * alternative to a stub is not testing the editor at all.
 *
 * The stub's `<textarea>` takes its accessible name from `options.ariaLabel`,
 * which `ResponseSection` actually computes, so a component that forgot to
 * pass it loses every `findByLabelText("Response transform script")` below.
 */
vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub({
    value,
    onChange,
    options,
  }: {
    value: string;
    onChange?: (v?: string) => void;
    options?: { ariaLabel?: string };
  }) {
    return (
      <textarea
        aria-label={options?.ariaLabel}
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
      />
    );
  },
}));

vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type {
  DataApiEndpointDto,
  OutputSpec,
} from "@/api/generated/model";
import type { EndpointDetailContext, EndpointFormState } from "@/routes/explore/dataapi/EndpointDetail";
import ResponseSection from "@/routes/explore/dataapi/detail/ResponseSection";

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

function makeForm(output: OutputSpec): EndpointFormState {
  const endpoint = makeEndpoint({ output });
  return {
    name: endpoint.name,
    description: endpoint.description,
    method: endpoint.method,
    path: endpoint.path,
    database_id: endpoint.database_id,
    statements: endpoint.statements,
    params: endpoint.params,
    output: endpoint.output,
    cache: endpoint.cache,
    limits: endpoint.limits,
    rate_limit: endpoint.rate_limit,
  };
}

/**
 * The section WITHOUT `EndpointDetail`'s shell, and WITHOUT its own
 * `useState` either — following `AccessSection.test.tsx`'s
 * `renderAccessWithoutTheShell` (there to keep `<fieldset disabled>` from
 * being the thing under test) and `TryIt.test.tsx`'s stubbed-context shape.
 *
 * A real `useState` lives in this wrapper rather than in a bare static `ctx`
 * object: `setForm` has to actually re-render the tree with the new form for
 * "the script changed but the row is still there" to mean anything, and a
 * spy that swallows the update would leave every test reading the SEEDED
 * form forever.
 */
function Harness({ initialOutput, onFormChange }: {
  initialOutput: OutputSpec;
  onFormChange?: (form: EndpointFormState) => void;
}) {
  const [form, setForm] = useState<EndpointFormState>(() => makeForm(initialOutput));
  const ctx: Partial<EndpointDetailContext> = {
    ws: "ws1",
    id: "ep-1",
    endpoint: makeEndpoint({ output: initialOutput }),
    form,
    setForm: (updater) => {
      setForm((prev) => {
        const next = typeof updater === "function" ? (updater as (f: EndpointFormState) => EndpointFormState)(prev) : updater;
        onFormChange?.(next);
        return next;
      });
    },
    dirty: false,
    saving: false,
    save: vi.fn(),
    trashed: false,
    refetch: vi.fn(),
  };
  return <Outlet context={ctx} />;
}

function renderResponse(initialOutput: OutputSpec, onFormChange?: (form: EndpointFormState) => void) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/response"]}>
        <Routes>
          <Route path="/response" element={<Harness initialOutput={initialOutput} onFormChange={onFormChange} />}>
            <Route index element={<ResponseSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/**
 * `renderResponse` wrapped in `React.StrictMode`, the way `main.tsx` actually
 * mounts the app. React 19 double-invokes a `useState` updater function under
 * `StrictMode` in development, throwing the second call's result away for
 * anything that doesn't come back idempotent — an updater that mutates a
 * `ref` on its way through (`enableTransform`/`disableTransform`'s
 * `clearedByTransform.current`) is exactly that case, and `renderResponse`
 * alone cannot see it: it is the harness diverging from the real app that
 * would hide the defect.
 */
function renderResponseStrict(initialOutput: OutputSpec, onFormChange?: (form: EndpointFormState) => void) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <StrictMode>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/response"]}>
          <Routes>
            <Route path="/response" element={<Harness initialOutput={initialOutput} onFormChange={onFormChange} />}>
              <Route index element={<ResponseSection />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </StrictMode>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  // No live version, no service accounts: "Frozen columns" renders its
  // "never published" state and the OpenAPI preview/download block has
  // nothing to query — neither is what these tests are about.
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
      typeof api.useDataApiListEndpointVersions
    >,
  );
  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
      typeof api.useDataApiListServiceAccounts
    >,
  );
  vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, error: null } as unknown as ReturnType<
      typeof api.useDataApiGetOpenapi
    >,
  );
});

describe("ResponseSection — response transform", () => {
  /**
   * Publish refuses `output.transform` beside a non-default `output.shape` or
   * `output.naming` (`ValidationError::TransformWithShapeOrNaming`). An editor
   * that still shows those two controls on a transformed endpoint produces an
   * error about a control the editor is still showing the author — the same
   * defect `ParamEditor`'s `clearInapplicableConstraints` exists to prevent.
   */
  it("hides shape and naming once a transform is present, but keeps envelope", async () => {
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns: [{ name: "id", ty: "integer" }] },
    });

    expect(screen.queryByLabelText("Shape")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Key naming")).not.toBeInTheDocument();
    // Envelope is not one of the two inert fields — `check_transform` never
    // refuses it beside a transform — so it must stay on screen.
    expect(await screen.findByLabelText("Envelope")).toBeInTheDocument();
    // And the reason is said on screen, not left for the author to find in
    // the generated OpenAPI document. Scoped to the one `role="note"` element
    // this branch renders — a plain `getByText` regex would also match every
    // ancestor whose `textContent` happens to contain the phrase.
    // "not shown here", not "hidden below": the note is rendered in the two
    // rows' own place and only Envelope follows it, so "below" sent a reader
    // looking down the table for controls that had been removed (Task 10,
    // seen in a browser).
    expect(screen.getByRole("note").textContent).toContain("Shape and Naming are not shown here");
  });

  /** The mirror of the test above: no transform, both controls present. */
  it("shows shape and naming when there is no transform", async () => {
    renderResponse({});
    expect(await screen.findByLabelText("Shape")).toBeInTheDocument();
    expect(screen.getByLabelText("Key naming")).toBeInTheDocument();
  });

  /**
   * Turning the transform ON is the moment a stale non-default shape/naming
   * would otherwise survive underneath the controls that just vanished — set
   * them here (not `toBeUndefined()` on a field that was never touched) so the
   * assertion proves they were CLEARED, not merely never set.
   */
  it("clears a non-default shape and naming when a transform is turned on", async () => {
    let latest: EndpointFormState | undefined;
    renderResponse({ shape: "arrays", naming: "camel" }, (f) => {
      latest = f;
    });

    expect(screen.getByLabelText("Shape")).toBeInTheDocument();
    const enable = screen.getByRole("checkbox", { name: "Build the response with a script" });
    await userEvent.click(enable);

    expect(await screen.findByLabelText("Response transform script")).toBeInTheDocument();
    expect(latest?.output.shape).toBeUndefined();
    expect(latest?.output.naming).toBeUndefined();
    expect(latest?.output.transform).toBeTruthy();
    // And the controls whose values just got cleared are also gone from the
    // DOM — clearing the data without hiding the control would leave a Select
    // sitting at "Objects" that stopped meaning anything.
    expect(screen.queryByLabelText("Shape")).not.toBeInTheDocument();
  });

  /**
   * `returns` is the declared CONTRACT, not a view derived from the script —
   * a keystroke in the editor must not drop it.
   */
  it("keeps a declared return row when the script changes", async () => {
    renderResponse({
      transform: {
        script: "def transform(params, statements):\n    return {}",
        returns: [{ name: "id", ty: "integer", required: true }],
      },
    });

    expect(await screen.findByLabelText("Return 1 name")).toHaveValue("id");

    const script = screen.getByLabelText("Response transform script");
    // A single change event carrying the WHOLE new text, exactly like real
    // Monaco's `onChange` — not `userEvent.type`, which would exercise the
    // mock's re-render loop rather than `updateTransform` itself.
    fireEvent.change(script, { target: { value: 'def transform(params, statements):\n    return {"id": 1}' } });

    // The row declared before the edit is still there, unchanged.
    expect(await screen.findByLabelText("Return 1 name")).toHaveValue("id");
  });

  /**
   * Serving checks a transform's response against `returns`; with none it
   * would fail OPEN. `check_transform` (which is what refuses this) runs
   * only from `/validate` (publish) and `/preview` — create/update never
   * call it, so this SAVES successfully. The editor states the refusal the
   * author will meet at Preview or Publish, rather than only Save's silence
   * followed by a red toast later.
   */
  it("warns that a transform with no declared returns will be refused at preview or publish", async () => {
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns: [] },
    });

    expect(
      await screen.findByText(/No return columns declared/i),
    ).toBeInTheDocument();
  });

  it("flags two declared returns sharing a name", async () => {
    renderResponse({
      transform: {
        script: "def transform(params, statements):\n    return {}",
        returns: [
          { name: "total", ty: "integer" },
          { name: "total", ty: "integer" },
        ],
      },
    });

    const rows = await screen.findAllByText(/Declared more than once/i);
    expect(rows).toHaveLength(2);
  });

  /**
   * Two unnamed rows are two unnamed rows.
   *
   * The editor used to trim and skip blanks, so this state looked FINE here
   * and publish refused it with ``duplicate name ""`` — a refusal quoting a
   * column that does not exist, for an author with two blank rows on screen
   * and nothing to tell them apart. `check_transform_shape` now refuses a
   * blank name as blank, before its duplicate rule, and this is the editor
   * saying the same thing.
   *
   * The second assertion is the load-bearing one: "not flagged as duplicates"
   * is what was wrong before, and a test that only checked for *some* error
   * would have passed on the old message.
   */
  it("reports two unnamed return rows as unnamed, not as duplicates of each other", async () => {
    renderResponse({
      transform: {
        script: "def transform(params, statements):\n    return {}",
        returns: [
          { name: "", ty: "integer" },
          { name: "   ", ty: "integer" },
        ],
      },
    });

    expect(await screen.findAllByText(/Name this column/i)).toHaveLength(2);
    expect(screen.queryByText(/Declared more than once/i)).not.toBeInTheDocument();
  });

  /**
   * `"total"` beside `"total "` — the disagreement in the other direction.
   *
   * The editor flagged this pair as a collision while publish accepted it and
   * froze two OpenAPI properties nobody can tell apart. `check_transform_shape`
   * now refuses the padded name as padded, and the editor names the same
   * problem: the padded row is the one at fault, and `"total"` on its own is
   * fine.
   */
  it("reports a whitespace-padded return name as padded, and leaves its twin alone", async () => {
    renderResponse({
      transform: {
        script: "def transform(params, statements):\n    return {}",
        returns: [
          { name: "total", ty: "integer" },
          { name: "total ", ty: "integer" },
        ],
      },
    });

    expect(await screen.findAllByText(/Remove the leading or trailing space/i)).toHaveLength(1);
    expect(screen.queryByText(/Declared more than once/i)).not.toBeInTheDocument();
  });

  /**
   * `MAX_TRANSFORM_RETURNS`, previewed. `returns` was the one
   * author-controlled list with no cap at all, and the editor is where an
   * author meets a cap while they can still change something.
   */
  it("warns when more return columns are declared than the cap allows", async () => {
    const returns = Array.from({ length: 65 }, (_, i) => ({ name: `c${i}`, ty: "integer" }));
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns },
    });

    expect(await screen.findByText(/65 return columns declared, over the 64 column cap/i)).toBeInTheDocument();
  });

  /** Exactly at the cap is fine — so the test above discriminates a limit
   *  rather than the presence of many rows. */
  it("does not warn at exactly the cap", async () => {
    const returns = Array.from({ length: 64 }, (_, i) => ({ name: `c${i}`, ty: "integer" }));
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns },
    });

    expect(await screen.findByLabelText("Return 1 name")).toBeInTheDocument();
    expect(screen.queryByText(/over the 64 column cap/i)).not.toBeInTheDocument();
  });

  /**
   * The script byte cap — counted in UTF-8 BYTES, which is what
   * `String::len()` counts on the Rust side.
   *
   * This is the assertion `scriptByteLength` exists for and had none: the
   * script below is 8192 UTF-16 code units and 8193 UTF-8 bytes, because one
   * character is a two-byte `é`. A `.length` counter reads it as exactly at
   * the cap and says nothing; the server refuses it. So a test that used an
   * ASCII script could not tell the two implementations apart, and that is
   * precisely how this went untested through 51 new tests.
   */
  it("counts the script cap in UTF-8 bytes, not UTF-16 code units", async () => {
    const head = "def transform(params, statements):\n  # é";
    // Pad to exactly MAX_TRANSFORM_SCRIPT_BYTES UTF-16 units.
    const script = head + "x".repeat(8192 - head.length);
    expect(script.length).toBe(8192);
    expect(new TextEncoder().encode(script).length).toBe(8193);

    renderResponse({ transform: { script, returns: [{ name: "id", ty: "integer" }] } });

    expect(await screen.findByText(/8,193 \/ 8,192 bytes/)).toBeInTheDocument();
    expect(screen.getByText(/over the cap/i)).toBeInTheDocument();
  });

  /** An ASCII script one byte under the cap is not flagged — the mirror, so
   *  the test above pins the boundary rather than "any long script warns". */
  it("does not flag a script one byte under the cap", async () => {
    const head = "def transform(params, statements):\n  # ";
    const script = head + "x".repeat(8191 - head.length);
    expect(new TextEncoder().encode(script).length).toBe(8191);

    renderResponse({ transform: { script, returns: [{ name: "id", ty: "integer" }] } });

    expect(await screen.findByText(/8,191 \/ 8,192 bytes/)).toBeInTheDocument();
    expect(screen.queryByText(/over the cap/i)).not.toBeInTheDocument();
  });

  /**
   * Ticking the box and unticking it must leave `output` as it was found.
   *
   * `enableTransform` clears `shape` and `naming` deliberately — leaving a
   * stale non-default underneath a control that has vanished is the defect it
   * exists to prevent — but nothing put them back, so an author who ticked the
   * box to look at the script and unticked it again silently published an
   * `objects`/`as_is` endpoint that had been `arrays`/`camel`.
   *
   * Seeded with NON-default values, so the assertion proves a restore rather
   * than agreeing with a field that was never set.
   */
  it("restores the shape and naming it cleared when the transform is turned off again", async () => {
    let latest: EndpointFormState | undefined;
    renderResponse({ shape: "arrays", naming: "camel" }, (f) => {
      latest = f;
    });

    const toggle = screen.getByRole("checkbox", { name: "Build the response with a script" });
    await userEvent.click(toggle);
    expect(await screen.findByLabelText("Response transform script")).toBeInTheDocument();
    expect(latest?.output.shape).toBeUndefined();

    await userEvent.click(screen.getByRole("checkbox", { name: "Build the response with a script" }));

    expect(await screen.findByLabelText("Shape")).toBeInTheDocument();
    expect(latest?.output.shape).toBe("arrays");
    expect(latest?.output.naming).toBe("camel");
    expect(latest?.output.transform).toBeNull();
  });

  /**
   * The same restore, under `React.StrictMode` — which is how `main.tsx`
   * actually mounts this app, and which the test above cannot see because
   * `renderResponse` does not wrap in it.
   *
   * React 19 invokes a `useState` updater function TWICE in development under
   * `StrictMode`, discarding the first call's return value and keeping only
   * the second's. `disableTransform` clears `clearedByTransform.current`
   * *inside* its updater, so the first invocation reads the remembered
   * `{ shape: "arrays", naming: "camel" }` and restores it, and the second
   * invocation — running against the same, now-`null` ref, because clearing
   * it is a side effect that does not rewind between the two calls — falls
   * through to `f.output.shape`/`f.output.naming`, which are `undefined` on
   * the very state this updater is trying to patch. React keeps that second,
   * empty result: an author who ticks the box and unticks it again ends the
   * visit with no shape or naming at all, in dev, which is exactly the
   * defect `disableTransform` exists to prevent.
   */
  it("restores the shape and naming under StrictMode's double-invoked updater", async () => {
    let latest: EndpointFormState | undefined;
    renderResponseStrict({ shape: "arrays", naming: "camel" }, (f) => {
      latest = f;
    });

    const toggle = await screen.findByRole("checkbox", { name: "Build the response with a script" });
    await userEvent.click(toggle);
    expect(await screen.findByLabelText("Response transform script")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("checkbox", { name: "Build the response with a script" }));

    expect(await screen.findByLabelText("Shape")).toBeInTheDocument();
    expect(latest?.output.shape).toBe("arrays");
    expect(latest?.output.naming).toBe("camel");
    expect(latest?.output.transform).toBeNull();
  });

  it("adds and removes a declared return row", async () => {
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns: [] },
    });

    await userEvent.click(screen.getByRole("button", { name: "Add return column" }));
    expect(await screen.findByLabelText("Return 1 name")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Remove return 1" }));
    expect(screen.queryByLabelText("Return 1 name")).not.toBeInTheDocument();
    // Back to the empty-returns refusal.
    expect(await screen.findByText(/No return columns declared/i)).toBeInTheDocument();
  });

  it("turning the transform off restores shape and naming", async () => {
    renderResponse({
      transform: { script: "def transform(params, statements):\n    return {}", returns: [{ name: "id", ty: "integer" }] },
    });

    expect(screen.queryByLabelText("Shape")).not.toBeInTheDocument();
    const toggle = screen.getByRole("checkbox", { name: "Build the response with a script" });
    await userEvent.click(toggle);

    expect(await screen.findByLabelText("Shape")).toBeInTheDocument();
    expect(screen.getByLabelText("Key naming")).toBeInTheDocument();
    expect(screen.queryByLabelText("Response transform script")).not.toBeInTheDocument();
  });
});
