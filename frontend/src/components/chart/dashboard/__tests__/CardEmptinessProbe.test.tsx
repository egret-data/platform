import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import type { ChartData } from "@/api/useChartData";

/**
 * The probe is the piece that makes hide-when-empty correct rather than one-way. Its
 * whole contract is WHEN it reports, so these assertions are about the calls to
 * `onEmpty`, not about anything rendered — it renders null on purpose.
 */

let next: ChartData = { columns: [], rows: [], isLoading: false, error: null };
const calls: Array<[string, unknown[], Record<string, string>, boolean | undefined]> = [];
vi.mock("@/api/useSavedChartData", () => ({
  useDashboardCardData: (
    _ws: string, _dash: string, cardId: string, _nonce: number | undefined,
    enabled: boolean, filters: unknown[], temporal: Record<string, string>,
  ) => {
    calls.push([cardId, filters, temporal, enabled]);
    return next;
  },
}));

import { CardEmptinessProbe } from "../CardEmptinessProbe";

const onEmpty = vi.fn();
const renderProbe = (enabled = true) =>
  render(
    <CardEmptinessProbe ws="w" dashboardId="d" cardId="c1" refreshNonce={0} enabled={enabled}
      paramFilters={[]} paramTemporal={{}} onEmpty={onEmpty} />,
  );

beforeEach(() => { onEmpty.mockClear(); calls.length = 0; });

describe("CardEmptinessProbe", () => {
  it("renders nothing", () => {
    next = { columns: [], rows: [], isLoading: false, error: null };
    const { container } = renderProbe();
    expect(container).toBeEmptyDOMElement();
  });

  it("reports empty when a settled query returned no rows", () => {
    next = { columns: [{ name: "a", type: "text" }], rows: [], isLoading: false, error: null };
    renderProbe();
    expect(onEmpty).toHaveBeenCalledWith("c1", true);
  });

  it("reports not-empty when the query returned rows", () => {
    next = { columns: [{ name: "a", type: "text" }], rows: [["x"]], isLoading: false, error: null };
    renderProbe();
    expect(onEmpty).toHaveBeenCalledWith("c1", false);
  });

  it("stays silent while loading, so the board keeps its previous answer", () => {
    // This is what stops a visible card blinking out on every refetch: with no report,
    // hiddenEmptyCardIds still reads the last value the probe gave it.
    next = { columns: [], rows: [], isLoading: true, error: null };
    renderProbe();
    expect(onEmpty).not.toHaveBeenCalled();
  });

  it("does NOT report a failed card as empty, so an error stays on the board", () => {
    // Metabase's shouldHideCard requires !getDashcardResultsError(...) as well as no
    // rows. Hiding a card that errored would delete the only evidence it broke.
    next = { columns: [], rows: [], isLoading: false, error: "boom" };
    renderProbe();
    expect(onEmpty).toHaveBeenCalledWith("c1", false);
  });

  it("passes the caller's query arguments straight through, so it shares the tile's cache key", () => {
    // If the probe and the tile disagree on any keyed argument, react-query runs the
    // query twice and a visible card costs two requests instead of one.
    next = { columns: [], rows: [], isLoading: false, error: null };
    render(
      <CardEmptinessProbe ws="w" dashboardId="d" cardId="c9" refreshNonce={3} enabled={false}
        paramFilters={[{ col: "state", op: "is", values: ["CA"] }] as never}
        paramTemporal={{ created_at: "month" }} onEmpty={onEmpty} />,
    );
    expect(calls[0]).toStrictEqual([
      "c9", [{ col: "state", op: "is", values: ["CA"] }], { created_at: "month" }, false,
    ]);
  });
});
