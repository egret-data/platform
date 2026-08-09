import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

/**
 * THE `compactType="vertical"` HAZARD, MEASURED RATHER THAN ASSUMED.
 *
 * `DashboardGridLayout` renders RGL with `compactType="vertical"`. If a mount-time compaction
 * reached `onLayoutChange`, the route would write the compacted positions into `draftCards` and an
 * UNTOUCHED dashboard would fingerprint as dirty — so every Cancel would raise the discard dialog
 * and the confirmation would train users to dismiss it on sight. That is the failure this file
 * exists to catch, and it cannot be caught by `DashboardView.editsafety.test.tsx`: that file never
 * gives the grid a container width, so `breakpointFor(0)` is "mobile" and `handleLayoutChange`
 * returns before the route sees anything at all.
 *
 * ── WHAT WAS ACTUALLY MEASURED ──────────────────────────────────────────────────────────────
 * Both `handleLayoutChange` and the route's `onLayoutChange` were instrumented and run against a
 * deliberately un-compacted fixture (two cards parked at row 7 with seven empty rows above them)
 * at a 1136px container, with markers around the Edit click:
 *
 *     MARK before render
 *     PROBE-EMIT  editing=false mobile  [["c1",0],["c2",2]]   ← suppressed by the isMobile guard
 *     PROBE-EMIT  editing=false desktop [["c1",0],["c2",0]]   ← the compaction, c2's y 7 → 0
 *     PROBE-ROUTE editing=false bp=mobile [["c1",0],["c2",0]]
 *     MARK after render, before Edit
 *     MARK after Edit
 *     MARK after observers
 *
 * So RGL really does compact, and really does emit the compacted layout. The reason it never
 * dirties a draft is simpler than the plan supposed, and it is not W1's breakpoint ref: **every
 * emission happens during the VIEW-MODE mount, while `editing` is still false**. Pressing Edit
 * produces no emission at all — RGL's internal state already holds the compacted layout, and it
 * only calls `onLayoutChange` when its own computed layout changes, so rebuilding the `layout`
 * prop from the un-compacted draft is not enough to make it speak. A separate probe confirmed the
 * consequence end to end: mount the gapped fixture at 1136px, press Edit, press Save, and the body
 * carries `grid_row [7,7]` — untouched.
 *
 * (A knock-on worth knowing, and NOT introduced here: between mount and the next drag, the grid
 * *displays* the compacted positions while the model still holds the saved ones. That divergence
 * predates this workstream and is left alone — closing it by writing the compaction back is
 * exactly the change that would make every untouched dashboard dirty.)
 */

vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

/** Settled: two headings on row 0, nothing above them to compact into. */
const SETTLED = [
  { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 12, size_y: 1, options: { text: "a" }, tab_id: null },
  { id: "c2", chart_id: null, card_type: "heading", grid_col: 12, grid_row: 0, size_x: 12, size_y: 1, options: { text: "b" }, tab_id: null },
];
/** Un-settled: the same two cards parked at row 7. Vertical compaction pulls both to row 0, which
 *  is the largest mount-time layout delta a saved dashboard could realistically present. */
const GAPPED = SETTLED.map((c) => ({ ...c, grid_row: 7 }));

let cards: typeof SETTLED = SETTLED;
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: { id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [], width: "full", cards }, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

// jsdom has neither a real clientWidth nor a firing ResizeObserver, so both are driven by hand —
// the same shim `DashboardView.mobilelayout.test.tsx` uses.
const observers: (() => void)[] = [];
vi.stubGlobal("ResizeObserver", class {
  constructor(cb: () => void) { observers.push(cb); }
  observe() {} unobserve() {} disconnect() {}
});
const setViewport = (px: number) =>
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => px });

beforeEach(() => { setViewport(1136); });
afterEach(() => {
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
  observers.length = 0;
  cards = SETTLED;
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

/** Enter edit mode and let the grid's observers settle, as a real mount does. */
function enterEditAndSettle() {
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  act(() => { observers.forEach((cb) => cb()); });
}

/** The banner's Cancel — the one in the same row as Save, not a dialog's. */
const bannerCancel = () => screen.getAllByRole("button", { name: "Cancel" })
  .find((b) => b.closest("div")?.textContent?.includes("Save"))!;

describe("react-grid-layout's mount-time compaction and the dirtiness check", () => {
  it("leaves an already-compacted dashboard clean on the desktop breakpoint", () => {
    renderView();
    enterEditAndSettle();
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
  });

  it("leaves a dashboard whose saved layout NEEDS compaction clean too", () => {
    // The worst case: RGL really does emit y 7 → 0 here (see the header). The draft must not take
    // it, or a dashboard nobody touched would prompt on Cancel.
    //
    // Mutation proof, and it took two edits because TWO independent properties hold this shut.
    // Dropping the `mayPersistLayout` guard alone is not enough — the emission then does land in
    // `draftCards`, but `startEdit` re-seeds them from `dashboard.cards` a moment later and the
    // baseline is taken from that same value, so the draft is clean anyway. Dropping the guard AND
    // changing `setDraftCards(dc)` to `setDraftCards((prev) => prev ?? dc)` — i.e. letting the
    // pre-edit write survive into the edit session — turns exactly this test red and nothing else.
    cards = GAPPED;
    renderView();
    enterEditAndSettle();
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("still prompts for a real edit at the same width, so the two above measure something", () => {
    // Without this control, "no alertdialog" would be equally consistent with a harness in which
    // the dialog can never appear at all.
    cards = GAPPED;
    const user = userEvent.setup();
    renderView();
    enterEditAndSettle();
    return (async () => {
      const nameInput = screen.getByLabelText("dashboard name") as HTMLInputElement;
      await user.clear(nameInput);
      await user.type(nameInput, "Renamed");
      fireEvent.click(bannerCancel());
      expect(screen.getByRole("alertdialog")).toBeTruthy();
    })();
  });
});
