import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

/**
 * B2's dataset dialogs, with `fetch` stubbed and nothing above it: the generated client, the
 * `ApiError` mapping of a 409 `in_use` and the mutations all run as in the browser, so a request
 * the dialog SENDS (and a confirmation it carries) is what is asserted.
 */
const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);
vi.mock("@/auth/useAuth", () => ({ useAuth: () => ({ me: { platform_roles: [], permissions: [] } }) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { DeleteDatasetDialog } from "@/components/dataset/DeleteDatasetDialog";
import { TrashDatasetDialog } from "@/components/dataset/TrashDatasetDialog";
import { DuplicateDatasetDialog } from "@/components/dataset/DuplicateDatasetDialog";
import { MetricsTab } from "@/components/dataset/MetricsTab";
import { ColumnsTab } from "@/components/dataset/ColumnsTab";
import { UsageTab } from "@/components/dataset/UsageTab";
import { DatasetActionsMenu } from "@/components/dataset/DatasetActionsMenu";
import { refreshSummary } from "@/components/dataset/refreshSummary";
import type { DatasetColumn, DatasetMetric, DatasetSummary } from "@/api/datasets";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function renderUi(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Every request the stub saw, as `METHOD path`. */
const calls = () => fetchMock.mock.calls.map(([url, init]) => `${(init?.method ?? "GET").toUpperCase()} ${String(url)}`);

// Braces: `mockReset()` returns the mock, and a function returned from `beforeEach` is run as its
// cleanup — which would call `fetch()` with no arguments after every test.
beforeEach(() => { fetchMock.mockReset(); });
afterEach(() => vi.clearAllMocks());

const CHARTS = [
  { id: "c1", name: "Revenue by region", viz_type: "pie", trashed: false },
  { id: "c2", name: "Old table", viz_type: "table", trashed: true },
];

describe("DeleteDatasetDialog (KI-098)", () => {
  test("lists every chart the delete takes, and needs the dataset's exact name", async () => {
    fetchMock.mockImplementation(async (url: string, init?: RequestInit) => {
      if (String(url).includes("/usage?include_trashed=true")) return json(CHARTS);
      if (init?.method === "DELETE") return json({ ok: true });
      throw new Error(`unexpected ${init?.method ?? "GET"} ${url}`);
    });
    const onDeleted = vi.fn();
    renderUi(<DeleteDatasetDialog ws="eng" dataset={{ id: "ds1", name: "Orders" }} onOpenChange={() => {}} onDeleted={onDeleted} />);

    expect(await screen.findByText("Revenue by region")).toBeInTheDocument();
    expect(screen.getByText("Old table")).toBeInTheDocument();
    expect(screen.getByText("in Trash")).toBeInTheDocument();
    expect(screen.getByText(/2 charts are built on this dataset and will be deleted with it/)).toBeInTheDocument();

    const confirm = screen.getByRole("button", { name: "Delete dataset and 2 charts" });
    const input = screen.getByPlaceholderText("Orders");
    await userEvent.type(input, "delete");
    expect(confirm).toBeDisabled();
    await userEvent.clear(input);
    await userEvent.type(input, "orders");
    expect(confirm, "the name is matched exactly, case included").toBeDisabled();
    await userEvent.clear(input);
    await userEvent.type(input, "Orders");
    await userEvent.click(confirm);

    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(calls()).toContain("DELETE /api/ws/eng/datasets/ds1?confirm=Orders");
  });

  test("a dataset no chart uses is deleted with the plain confirmation and no name", async () => {
    fetchMock.mockImplementation(async (url: string, init?: RequestInit) => {
      if (String(url).includes("/usage")) return json([]);
      if (init?.method === "DELETE") return json({ ok: true });
      throw new Error(`unexpected ${init?.method ?? "GET"} ${url}`);
    });
    const onDeleted = vi.fn();
    renderUi(<DeleteDatasetDialog ws="eng" dataset={{ id: "ds1", name: "Orders" }} onOpenChange={() => {}} onDeleted={onDeleted} />);
    expect(await screen.findByText(/The underlying table or query is not affected/)).toBeInTheDocument();
    await userEvent.type(screen.getByPlaceholderText("delete"), "delete");
    await waitFor(() => expect(screen.getByRole("button", { name: "Delete" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(calls()).toContain("DELETE /api/ws/eng/datasets/ds1");
  });
});

describe("TrashDatasetDialog (KI-111)", () => {
  test("says the charts keep working until the dataset is deleted", async () => {
    fetchMock.mockResolvedValue(json([CHARTS[0]]));
    const onConfirm = vi.fn();
    renderUi(<TrashDatasetDialog ws="eng" dataset={{ id: "ds1", name: "Orders" }} onOpenChange={() => {}} onConfirm={onConfirm} />);
    expect(await screen.findByText(/1 chart uses this dataset and will keep working until the dataset is deleted/)).toBeInTheDocument();
    expect(screen.getByText("Revenue by region")).toBeInTheDocument();
    expect(onConfirm).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});

const metric: DatasetMetric = {
  id: "m1", metric_name: "total_revenue", expression: "SUM(revenue)", data_type: "double", type_generic: "numeric",
  verbose_name: "", description: "", ordinal: 0, format: "", certified: false, certified_by: "", certification_details: "",
};

describe("MetricsTab delete (KI-105, KI-099)", () => {
  test("asks first, and when charts use the metric names them and needs the metric's name", async () => {
    let n = 0;
    fetchMock.mockImplementation(async (_url: string, init?: RequestInit) => {
      if (init?.method !== "DELETE") return json({});
      n += 1;
      return n === 1
        ? json({ error: "1 chart uses this metric", code: "in_use", charts: [CHARTS[0]] }, 409)
        : json({ metrics: [] });
    });
    renderUi(<MetricsTab ws="eng" datasetId="ds1" metrics={[metric]} />);

    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(fetchMock, "nothing is deleted on the first click").not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    await userEvent.type(within(dialog).getByPlaceholderText("delete"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete metric" }));

    // The server refused: the dialog comes back naming the chart and asking for the name.
    expect(await screen.findByText("Revenue by region")).toBeInTheDocument();
    expect(screen.getByText(/1 chart uses this metric and will stop running/)).toBeInTheDocument();
    await userEvent.type(screen.getByPlaceholderText("total_revenue"), "total_revenue");
    await userEvent.click(screen.getByRole("button", { name: "Delete metric" }));
    await waitFor(() => expect(calls()).toEqual([
      "DELETE /api/ws/eng/datasets/ds1/metrics/m1",
      "DELETE /api/ws/eng/datasets/ds1/metrics/m1?confirm=total_revenue",
    ]));
  });
});

const calc: DatasetColumn = {
  column_name: "net", data_type: "double", type_generic: "numeric", is_temporal: false, ordinal: 0,
  expression: "price * 0.9", verbose_name: "", description: "", is_calculated: true, groupby: true,
  filterable: true, format: "", semantic_type: "",
};

describe("ColumnsTab delete (KI-105)", () => {
  test("a calculated column is deleted only after the confirmation", async () => {
    fetchMock.mockResolvedValue(json({}));
    renderUi(<ColumnsTab ws="eng" datasetId="ds1" columns={[calc]} />);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(fetchMock).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Delete calculated column “net”?")).toBeInTheDocument();
    await userEvent.type(within(dialog).getByPlaceholderText("delete"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete column" }));
    await waitFor(() => expect(calls()).toContain("DELETE /api/ws/eng/datasets/ds1/columns/net"));
  });
});

describe("UsageTab (KI-166)", () => {
  test("each chart carries its own type's icon", async () => {
    fetchMock.mockResolvedValue(json([CHARTS[0], { ...CHARTS[1], trashed: false }]));
    renderUi(<UsageTab ws="eng" datasetId="ds1" />);
    expect(await screen.findByRole("img", { name: "Pie" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Table" })).toBeInTheDocument();
  });
});

describe("Duplicate (KI-110)", () => {
  const physical: DatasetSummary = {
    id: "ds1", name: "Orders", description: "", database_id: "db1", kind: "physical",
    catalog: "tpch", schema: "tiny", table: "orders", column_count: 9, updated_at: "2026-10-21T00:00:00Z", folder_id: null,
  };

  test("is offered for a physical dataset", async () => {
    const onDuplicate = vi.fn();
    renderUi(
      <DatasetActionsMenu ws="eng" ds={physical} trashed={false} folders={[]} onDuplicate={onDuplicate}
        onMove={vi.fn()} onNewFolder={vi.fn()} onTrash={vi.fn()} onRestore={vi.fn()} onDeletePermanent={vi.fn()} />,
    );
    await userEvent.click(screen.getByRole("button"));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Duplicate" }));
    expect(onDuplicate).toHaveBeenCalledWith(physical);
  });

  test("sends the name, and shows the server's refusal under the field", async () => {
    fetchMock.mockResolvedValue(json({ error: "conflict: a dataset with this name already exists" }, 409));
    renderUi(<DuplicateDatasetDialog ws="eng" dataset={{ id: "ds1", name: "Orders" }} onOpenChange={() => {}} />);
    const name = screen.getByLabelText("Name");
    expect(name).toHaveValue("copy of Orders");
    await userEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    expect(await screen.findByText("conflict: a dataset with this name already exists")).toBeInTheDocument();
    expect(name, "the typed name is kept").toHaveValue("copy of Orders");
    const [url, init] = fetchMock.mock.calls[0];
    expect(`${init.method} ${url}`).toBe("POST /api/ws/eng/datasets/ds1/duplicate");
    expect(JSON.parse(init.body as string)).toEqual({ name: "copy of Orders" });
  });
});

describe("refreshSummary (KI-102)", () => {
  test("names what the refresh added and removed", () => {
    expect(refreshSummary({ added_columns: [], removed_columns: [] })).toBe("Refreshed: no columns added or removed");
    expect(refreshSummary({ added_columns: ["clerk"], removed_columns: ["comment", "x"] }))
      .toBe("Refreshed: 1 new column (clerk); 2 columns removed (comment, x)");
    expect(refreshSummary({ added_columns: ["a", "b", "c", "d", "e"], removed_columns: [] }))
      .toBe("Refreshed: 5 new columns (a, b, c and 2 more)");
  });
});
