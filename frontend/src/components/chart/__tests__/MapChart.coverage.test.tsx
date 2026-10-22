// src/components/chart/__tests__/MapChart.coverage.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { MapChart } from "../MapChart";
import type { AnyChartParams } from "@/api/charts";

/**
 * THE SEAM, NOT THE ARITHMETIC. `pinCoverage`, `pinProblem` and `pinNote` are unit-tested against
 * hand-written extents in `echarts/__tests__/map.test.ts`; this file exists for the two things
 * those cannot reach.
 *
 * FIRST, THE REAL GEOJSON. Every test below lets `MapChart` load and index the SHIPPED
 * us-states file, so the extent under test is the one production uses rather than a fixture that
 * agrees with itself. A walk that handled the wrong nesting depth, or an index wired up after the
 * first render instead of before it, passes every unit test and fails here.
 *
 * SECOND, THE WIRING. A pure function that computes a perfect message and a component that never
 * renders it is exactly the "control that saves and does not paint" shape this programme has closed
 * eight times, in the other direction. The assertions are on what reaches the DOM.
 *
 * WHAT THIS FILE DOES NOT PROVE: where a pin lands. jsdom runs no layout, so the projection never
 * executes here — the evidence for placement is #434's live pixel measurement and the SSR render
 * recorded in `options/map.ts`. What is proved here is which rows are counted as reachable, which
 * is a predicate over degrees and needs no renderer at all.
 */
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option }: { option: unknown }) => (
    <div
      data-testid="echarts"
      data-pins={String(
        ((option as { series?: { type?: string; data?: unknown[] }[] }).series ?? [])
          .find((s) => s.type === "scatter")?.data?.length ?? 0,
      )}
    />
  ),
}));

const COLUMNS = [
  { name: "city", type: "varchar" },
  { name: "lat", type: "double" },
  { name: "lon", type: "double" },
];

/** Two American cities and, where a test wants one, somewhere that is not on a map of America. */
const SF = ["San Francisco", 37.7749, -122.4194];
const DENVER = ["Denver", 39.7392, -104.9903];
const PARIS = ["Paris", 48.8566, 2.3522];

const pinParams = (extra: Record<string, unknown> = {}): AnyChartParams => ({
  version: 2,
  spec: {
    metrics: [], dimensions: [], columns: ["lat", "lon", "city"],
    filters: [], orderby: [], row_limit: 1000, rollup_groups: [],
  },
  settings: {
    "map.key": "us_states", "map.type": "pin",
    "map.latitude_column": "lat", "map.longitude_column": "lon", "map.label_column": "city",
    ...extra,
  },
});

/** Renders and waits out the asynchronous geojson import — until it lands there is no extent, and
 *  every reader is required to treat that as "no opinion" rather than "outside". */
async function draw(
  rows: unknown[][],
  extra: Record<string, unknown> = {},
  onFinished?: () => void,
) {
  const view = render(
    <MapChart columns={COLUMNS} rows={rows} params={pinParams(extra)} onFinished={onFinished} />,
  );
  // Once the placeholder is gone the extent is available: `indexRegionExtent` runs inside the same
  // `.then` as `indexRegionNames` and BEFORE the `forceUpdate` that clears "Loading map…", so
  // there is no render in which the map is ready and its extent is not. The assertions below
  // depend on that ordering, and would report every row as on-map if it were reversed.
  await waitFor(() => expect(view.container.textContent).not.toContain("Loading map"));
  return view;
}

describe("MapChart — a pin map accounts for its own rows", () => {
  it("says nothing at all when every row is on the map", async () => {
    const { container } = await draw([SF, DENVER]);
    expect(container.querySelector('[data-testid="echarts"]')).toBeTruthy();
    expect(container.textContent).not.toContain("Showing");
  });

  /**
   * THE DEFECT, AGAINST THE REAL MAP. Paris is a legal coordinate that projects 252px past the
   * right edge of a 540px card (measured — see `options/map.ts`), so before this change the reader
   * saw two dots for three rows and nothing said why.
   */
  it("counts the rows it can show against the rows it has, and names the map", async () => {
    const { container } = await draw([SF, DENVER, PARIS]);
    expect(container.textContent).toContain("Showing 2 of 3 rows");
    expect(container.textContent).toContain("1 is outside the US states map");
    expect(container.textContent).toMatch(/zoom out/i);
  });

  /** The pins that ARE right are not thrown away to complain about the one that is not — and the
   *  off-map pin stays in the series, because zooming out can reach it. */
  it("still draws the chart, and still draws the off-map pin", async () => {
    const { container } = await draw([SF, DENVER, PARIS]);
    const chart = container.querySelector('[data-testid="echarts"]');
    expect(chart).toBeTruthy();
    expect(chart?.getAttribute("data-pins")).toBe("3");
  });

  /** A row with no coordinate is the case #434's own live check hit: 21 rows in, 20 pins out. */
  it("accounts for a row that has no coordinate at all", async () => {
    const { container } = await draw([SF, DENVER, ["nowhere", null, null]]);
    expect(container.textContent).toContain("Showing 2 of 3 rows");
    expect(container.textContent).toContain("1 has no latitude or longitude");
  });

  /**
   * MOVED. Paris and Tokyo on the US states map are both OFF-MAP, and off-map pins are drawn.
   *
   * This asserted that the card is replaced by "None of these 2 rows can be shown … Try a map that
   * covers them" — a sentence that told the reader another map would cover these rows while
   * removing the map on which they had, in fact, already been plotted. Both pins exist in the
   * option; only the extent was elsewhere. So the card keeps its chart, and the note underneath
   * says what is true: nothing is in view, and here is how to reach it.
   *
   * The replacement presentation is unchanged and still tested — see the transposition case below,
   * where the coordinates are not coordinates and no roaming will ever find them.
   */
  it("keeps the chart when every pin is off the map, and says nothing is in view", async () => {
    const { container } = await draw([PARIS, ["Tokyo", 35.6895, 139.6917]]);
    expect(container.querySelector('[data-testid="echarts"]')).not.toBeNull();
    expect(container.textContent).toContain("Showing 0 of 2 rows");
    expect(container.textContent).toContain("2 are outside the US states map");
    expect(container.textContent).toContain("Zoom out or pan the map to reach them");
    // Never both messages on one card.
    expect(container.textContent).not.toContain("None of these");
  });

  /**
   * THE TRANSPOSITION, which is the failure #434 named as the one it was most worried about and
   * the only one a range check can name a FIX for. Latitude and longitude the wrong way round puts
   * -122.4194 in the latitude column, and that is not a latitude anywhere.
   */
  it("tells the reader the pair is backwards rather than blaming the map", async () => {
    const { container } = await draw([
      ["San Francisco", -122.4194, 37.7749], ["Denver", -104.9903, 39.7392],
    ]);
    expect(container.textContent).toContain("the other way round");
    // And it does NOT say they are outside the map, which would send the reader to change the map
    // and get the same empty result.
    expect(container.textContent).not.toContain("outside the US states map");
  });

  /**
   * THE `onFinished` GAP THE REPORT RENDERER FOUND. `pinProblem`'s message — "the other way
   * round" above — is a REPLACEMENT for the chart, the same as "No data" or "Couldn't load the
   * map", not an error: nothing threw, the card is accounting for its own rows exactly the way
   * `pinNote`'s sibling case does. A print page (`routes/print/PrintObject.tsx`) that never hears
   * `onFinished` for this state would wait forever, since it has no chart to report ECharts'
   * `finished` event and no timer fallback by design.
   *
   * Deliberately treated as READY, not FAILED: it is the same category as `ChartRenderer`'s own
   * empty-result branches (`EmptyState`) and the sankey's cycle message (`sankeyIssue`) — a valid,
   * final, non-error presentation the print page is meant to capture and show the report's reader,
   * not a defect that should abort the run.
   */
  it("still reports finished when nothing on the card is plottable at all", async () => {
    const onFinished = vi.fn();
    await draw(
      [["San Francisco", -122.4194, 37.7749], ["Denver", -104.9903, 39.7392]],
      {},
      onFinished,
    );
    expect(onFinished).toHaveBeenCalled();
  });

  /** A REGION MAP IS UNTOUCHED. It has no coordinates to count, and a note under a choropleth
   *  would be a sentence about a thing that never happened. */
  it("says nothing on a region map", async () => {
    const view = render(
      <MapChart
        columns={[{ name: "state", type: "varchar" }, { name: "rev", type: "double" }]}
        rows={[["California", 10], ["Texas", 7]]}
        params={{
          version: 2,
          spec: {
            metrics: ["rev"], dimensions: ["state"], columns: [],
            filters: [], orderby: [], row_limit: 1000, rollup_groups: [],
          },
          settings: { "map.key": "us_states", "map.type": "region", "map.region": "state", "map.metric": "rev" },
        }}
      />,
    );
    await waitFor(() => expect(view.container.textContent).not.toContain("Loading map"));
    expect(view.container.querySelector('[data-testid="echarts"]')).toBeTruthy();
    expect(view.container.textContent).not.toContain("Showing");
    expect(view.container.textContent).not.toContain("None of these");
  });
});

/**
 * KI-046: a REGION map dropped a row whose region names no shape with nothing on screen to say so,
 * and its legend still ran up to that row's value. Against the shipped Vietnam geojson, whose keys
 * are names such as `TP. Hồ Chí Minh` — the spelling a reader most easily misses.
 */
describe("MapChart — a region map accounts for rows that match no region", () => {
  const regionParams: AnyChartParams = {
    version: 2,
    spec: { metrics: ["rev"], dimensions: ["province"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings: { "map.key": "vietnam", "map.type": "region", "map.region": "province", "map.metric": "rev" },
  };
  const cols = [{ name: "province", type: "varchar" }, { name: "rev", type: "double" }];
  const drawRegion = async (rows: unknown[][]) => {
    const view = render(<MapChart columns={cols} rows={rows} params={regionParams} />);
    // The Vietnam geojson is the largest of the three (600 KB), and its first import is slow.
    await waitFor(() => expect(view.container.textContent).not.toContain("Loading map"), { timeout: 10_000 });
    return view;
  };

  it("says nothing when every row lands on a province", { timeout: 15_000 }, async () => {
    const { container } = await drawRegion([["Hà Nội", 10], ["TP. Hồ Chí Minh", 30]]);
    expect(container.textContent).not.toContain("Showing");
  });

  it("names the rows it could not place, under the map", async () => {
    const { container } = await drawRegion([["Hà Nội", 10], ["Thành phố Hồ Chí Minh", 30]]);
    expect(container.textContent).toContain(
      'Showing 1 of 2 rows: 1 matches no region on the Vietnam provinces map ("Thành phố Hồ Chí Minh").',
    );
  });
});
