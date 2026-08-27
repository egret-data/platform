import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
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
