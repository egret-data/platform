import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

/**
 * `fetch` is stubbed — nothing above it is.
 *
 * This file used to `vi.mock` the generated `useDatasetsDsData` hook and hand it a
 * literal `{ isPending: false, data: … }`, which meant every assertion here was
 * about a state the test itself supplied. The tab could — and did — wedge on
 * permanent skeletons in the browser with all four cases green, because the
 * fetch→settle→render lifecycle was never run. Stubbing the global `fetch` leaves
 * that whole path in place — `apiFetch`'s status handling and `ApiError` mapping
 * included — and still keeps the tests offline.
 *
 * Everything renders under `StrictMode`, which is what `main.tsx` does. That is the
 * condition the wedge needed: effects run twice in dev, and the old mount-effect +
 * ref-guard arrangement suppressed the second run and never left `isPending`.
 */
const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

import { PreviewTab } from "@/components/dataset/PreviewTab";
import type { DatasetColumn } from "@/api/datasets";

const jsonOk = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });

function renderWithProviders(ui: React.ReactElement) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <StrictMode>
      <QueryClientProvider client={qc}>
        <MemoryRouter>{ui}</MemoryRouter>
      </QueryClientProvider>
    </StrictMode>,
  );
}

beforeEach(() => fetchMock.mockReset());
afterEach(() => vi.clearAllMocks());

test("PreviewTab: success renders column header + cell value and runs on mount", async () => {
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "order_id", type: "bigint" }], rows: [[42]] }));
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  await waitFor(() => expect(screen.getByText("order_id")).toBeInTheDocument());
  expect(screen.getByText("42")).toBeInTheDocument();

  // Hit the right endpoint with the row cap, once.
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [path, init] = fetchMock.mock.calls[0];
  expect(path).toBe("/api/ws/eng/datasets/ds1/data");
  expect(JSON.parse(init.body as string)).toEqual({ limit: 100 });
});

test("PreviewTab: leaves the loading state even under StrictMode's double effect", async () => {
  // The regression this file exists for. Asserting the ABSENCE of the wedge:
  // the Refresh button must become enabled, which it never did before.
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "c", type: "text" }], rows: [["v"]] }));
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  await waitFor(() => expect(screen.getByRole("button", { name: "Refresh" })).toBeEnabled());
  expect(screen.queryByRole("button", { name: "Loading…" })).not.toBeInTheDocument();
});

test("PreviewTab: applies a column's semantic-layer format to the rendered value", async () => {
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "amount", type: "bigint" }], rows: [[10]] }));
  // amount carries a Currency ColumnFormat (as stored by the FormatPicker).
  const columns = [{
    column_name: "amount", data_type: "bigint", type_generic: "numeric", is_temporal: false,
    ordinal: 1, expression: null, verbose_name: "", description: "", is_calculated: false,
    groupby: true, filterable: true, format: JSON.stringify({ number_style: "currency", decimals: 2 }),
  }] as DatasetColumn[];
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" columns={columns} />);

  // Raw value 10 renders as currency — the format actually takes effect.
  await waitFor(() => expect(screen.getByText("$10.00")).toBeInTheDocument());
});

test("PreviewTab: error state shows the Alert message", async () => {
  // A real 403 body rather than a rejected promise: `apiFetch` turns it into an
  // `ApiError` inside react-query's own chain, so there is no stray rejection for
  // vitest to flag — and this exercises the error mapping the app actually uses.
  fetchMock.mockResolvedValue(
    new Response(JSON.stringify({ error: "Access denied" }), {
      status: 403,
      headers: { "content-type": "application/json" },
    }),
  );
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
  expect(screen.getByText("Access denied")).toBeInTheDocument();
});

test("PreviewTab: Refresh click re-runs the request", async () => {
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "c", type: "text" }], rows: [["v"]] }));
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  await waitFor(() => expect(screen.getByRole("button", { name: "Refresh" })).toBeEnabled());
  expect(fetchMock).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
});

test("PreviewTab: an empty result says so instead of showing a table", async () => {
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "c", type: "text" }], rows: [] }));
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  await waitFor(() => expect(screen.getByText("No rows.")).toBeInTheDocument());
});

test("PreviewTab: a header reads the column's Display name, the data stays keyed by the column (KI-103)", async () => {
  fetchMock.mockResolvedValue(jsonOk({ columns: [{ name: "order_size", type: "varchar" }], rows: [["big"]] }));
  const columns = [{
    column_name: "order_size", data_type: "varchar", type_generic: "string", is_temporal: false,
    ordinal: 1, expression: null, verbose_name: "Order size", description: "", is_calculated: false,
    groupby: true, filterable: true, format: "", semantic_type: "",
  }] as DatasetColumn[];
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" columns={columns} />);

  await waitFor(() => expect(screen.getByText("big")).toBeInTheDocument());
  const headers = screen.getAllByRole("columnheader").map((h) => h.textContent ?? "");
  expect(headers.some((h) => h.includes("Order size"))).toBe(true);
  expect(headers.some((h) => h.includes("order_size"))).toBe(false);
});
