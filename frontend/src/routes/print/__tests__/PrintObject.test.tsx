// The flag IS the contract with the renderer. A flag that is set
// unconditionally is the bug this file exists to prevent, so each precondition
// is withheld in turn.
import { it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import PrintObject from "@/routes/print/PrintObject";

const payloadMock = vi.fn();
const dataMock = vi.fn();
vi.mock("@/api/render", () => ({
  fetchRenderPayload: (...a: unknown[]) => payloadMock(...a),
  fetchRenderData: (...a: unknown[]) => dataMock(...a),
}));

// One chart that never reports `finished`, to prove the flag waits for it.
// The brief's own test mocks `@/components/chart/ChartRender`, but the real component that
// dispatches a viz spec to an ECharts option (with an `onFinished` prop added for this task) is
// `ChartRenderer` at `@/components/chart/ChartRenderer` — see task-4-report.md for why.
let finishHandlers: Array<() => void> = [];
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ onFinished }: { onFinished?: () => void }) => {
    if (onFinished) finishHandlers.push(onFinished);
    return <div data-testid="chart" />;
  },
}));

function renderPrint() {
  return render(
    <MemoryRouter initialEntries={["/embed/print/dashboard/d1?t=tok"]}>
      <Routes><Route path="/embed/print/:kind/:id" element={<PrintObject />} /></Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  // Call history accumulates across tests otherwise, so `mock.calls[0]` would
  // be a PREVIOUS test's request — which is how this file first "passed" the
  // parameter test in isolation and failed it in the suite.
  payloadMock.mockClear();
  dataMock.mockClear();
  finishHandlers = [];
  (window as never as Record<string, unknown>).__EGRET_PRINT_READY__ = undefined;
  Object.defineProperty(document, "fonts", { value: { ready: Promise.resolve() }, configurable: true });
  // Step one describes the object; the rows arrive from step two. The split is
  // what lets the page resolve the dashboard's parameters BEFORE the query runs.
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops",
    parameters: [],
    cards: [{
      chart_id: "c1", title: "A", viz_type: "bar",
      params: { spec: { filters: [] } }, param_mappings: {},
    }],
  });
  // `columns` is `ColumnMeta[]` (name + engine type), not bare names — matches the real
  // `RenderCardData` contract now that the backend carries real column metadata through.
  dataMock.mockResolvedValue([
    { chart_id: "c1", columns: [{ name: "n", type: "double" }], rows: [[1]] },
  ]);
});

it("does not signal ready while a chart has not finished", async () => {
  renderPrint();
  await waitFor(() => expect(payloadMock).toHaveBeenCalled());
  await new Promise((r) => setTimeout(r, 50));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});

it("signals ready once every chart finishes and fonts resolve", async () => {
  renderPrint();
  await waitFor(() => expect(finishHandlers.length).toBe(1));
  finishHandlers.forEach((f) => f());
  await waitFor(() =>
    expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBe(true));
});

it("does not signal ready when fonts never resolve", async () => {
  Object.defineProperty(document, "fonts", { value: { ready: new Promise(() => {}) }, configurable: true });
  renderPrint();
  await waitFor(() => expect(finishHandlers.length).toBe(1));
  finishHandlers.forEach((f) => f());
  await new Promise((r) => setTimeout(r, 50));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});

it("marks failure rather than readiness when the payload fetch fails", async () => {
  // A failed page must never be captured. The renderer times out, and the run
  // is recorded `error`.
  payloadMock.mockRejectedValue(new Error("404"));
  renderPrint();
  await waitFor(() =>
    expect((window as never as Record<string, unknown>).__EGRET_PRINT_FAILED__).toBe(true));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});

it("resolves the dashboard's parameter defaults into the data request", async () => {
  // The whole point of the two-step render. Before it, the payload compiled
  // each card from its saved spec alone, so a dashboard's filters never
  // reached the query and the emailed image showed different numbers from the
  // page it claimed to be. A scheduled report has nobody at the keyboard, so
  // the values are the parameters' DEFAULTS — the dashboard as it looks when
  // you open it, which is what the recipient sees if they follow the link.
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops",
    parameters: [
      { id: "p1", slug: "priority", type: "text", operator: "is", default: "1-URGENT" },
    ],
    cards: [{
      chart_id: "c1", title: "A", viz_type: "bar",
      params: { spec: { filters: [] } },
      param_mappings: { p1: "orderpriority" },
    }],
  });

  renderPrint();
  await waitFor(() => expect(dataMock).toHaveBeenCalled());
  const [, cards] = dataMock.mock.calls[0];
  expect(cards).toEqual([
    {
      chart_id: "c1",
      filters: [{ col: "orderpriority", op: "eq", value: "1-URGENT" }],
      temporal_units: {},
    },
  ]);
});

it("asks for nothing extra when a parameter is not mapped to the card", async () => {
  // A parameter with no mapping does not apply to that card — which is why the
  // mapping travels per card. Sending it anyway would filter a chart the
  // dashboard itself leaves alone.
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops",
    parameters: [
      { id: "p1", slug: "priority", type: "text", operator: "is", default: "1-URGENT" },
    ],
    cards: [{
      chart_id: "c1", title: "A", viz_type: "bar",
      params: { spec: { filters: [] } },
      param_mappings: {},
    }],
  });

  renderPrint();
  await waitFor(() => expect(dataMock).toHaveBeenCalled());
  expect(dataMock.mock.calls[0][1][0].filters).toEqual([]);
});

// KI-168: the chart box is a fixed 384px with nothing to contain what is drawn in it, so a table
// taller than that ran past the card's border into the next card. jsdom does no layout, so these
// pin the two halves of the rule on the box itself.
it("contains a chart in its fixed box, and lets a table card grow to its rows", async () => {
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops", parameters: [],
    cards: [
      { chart_id: "c1", title: "A", viz_type: "bar", params: { spec: { filters: [] } }, param_mappings: {} },
      { chart_id: "c2", title: "B", viz_type: "table", params: { spec: { filters: [] } }, param_mappings: {} },
    ],
  });
  dataMock.mockResolvedValue([
    { chart_id: "c1", columns: [{ name: "n", type: "double" }], rows: [[1]] },
    { chart_id: "c2", columns: [{ name: "n", type: "double" }], rows: [[1]] },
  ]);
  const { findAllByTestId } = renderPrint();
  const [bar, table] = (await findAllByTestId("chart")).map((c) => c.closest("[data-print-chart]")!);
  expect(bar.className).toContain("h-96");
  expect(bar.className).toContain("overflow-hidden");
  expect(table.className).not.toContain("h-96");
});
