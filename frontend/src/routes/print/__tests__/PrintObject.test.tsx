// The flag IS the contract with the renderer. A flag that is set
// unconditionally is the bug this file exists to prevent, so each precondition
// is withheld in turn.
import { it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import PrintObject from "@/routes/print/PrintObject";

const payloadMock = vi.fn();
vi.mock("@/api/render", () => ({ fetchRenderPayload: (...a: unknown[]) => payloadMock(...a) }));

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
  finishHandlers = [];
  (window as never as Record<string, unknown>).__EGRET_PRINT_READY__ = undefined;
  Object.defineProperty(document, "fonts", { value: { ready: Promise.resolve() }, configurable: true });
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops",
    // `columns` is `ColumnMeta[]` (name + engine type), not bare names — matches the real
    // `RenderCard` contract now that the backend carries real column metadata through.
    cards: [{
      chart_id: "c1", title: "A", viz_type: "bar", params: {},
      columns: [{ name: "n", type: "double" }], rows: [[1]],
    }],
  });
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
