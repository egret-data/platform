import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { ChartExportMenu } from "../ChartExportMenu";
import { rowsToCsv } from "../export-chart";

/**
 * "COLUMN TITLE" IS *NOT* THE THIRD `show_mini_bar` / `view_as`, AND THIS FILE IS WHY.
 *
 * A config-access sweep reported it as one: the Column-title box is offered on eighteen viz types
 * while only eight builders read `column_title`, and building a gauge, a funnel, a sankey, a map, a
 * box plot, a waterfall, a scalar, a progress bar or an object detail twice — once with the key set,
 * once without — yields a BYTE-IDENTICAL option. Every clause of that is true. The conclusion drawn
 * from it, that a third `allow…` gate should withdraw the control the way `allowMiniBar` and
 * `allowViewAs` did, is not.
 *
 * THE INSTRUMENT MEASURED THE BUILT OPTION, AND THE HEADER NEVER GOES THROUGH ONE. Every chart in
 * the product — every viz type, in the studio (`ChartExplore`) and on a dashboard card
 * (`DashboardCardView`) — carries a `ChartExportMenu`, whose "Download CSV" item runs `rowsToCsv`
 * over the RESULT COLUMNS and the effective `columnSettings`, nowhere near an ECharts option. That
 * function has renamed the header from `column_title` since it was written and says so in its own
 * doc comment: "the header follows the same rename the table shows, so a downloaded file reads the
 * same as the screen it came from". So typing a Column title on a gauge is not a control that saves
 * and paints nothing; it renames a column in the file the reader downloads.
 *
 * A byte-identical option is therefore evidence about the CHART and not about the SETTING, and that
 * is the difference between the two closed cases and this one: `show_mini_bar` and `view_as` are
 * read by `ChartTable.renderTableCell` and by nothing else in the app, export included —
 * `viewAsIsTableOnly.test.ts` pins both directions of that, and it is `formatValue`'s blindness
 * doing the work there. `column_title` has a second consumer that every viz reaches.
 *
 * METABASE AGREES ON BOTH HALVES, which is what settles it rather than the reachability argument
 * alone. `column_title` is not one of its common column settings — it is absent from
 * `COMMON_COLUMN_SETTINGS` (`lib/settings/column.ts:433-456`) and is declared by only four viz
 * definitions: Table, Object detail, List and Pivot table. But its BACKEND applies the same setting
 * to every export regardless of viz — `query_processor/streaming/common.clj`'s `column-titles`,
 * consumed by the CSV and XLSX writers, and by the email renderer in `channel/render/body.clj`.
 * "Column title" means the column's header text system-wide, and an export's header is a place
 * every visualization has.
 *
 * WHAT THE SWEEP DID FIND is one viz that should paint it on screen and did not: the OBJECT DETAIL,
 * whose field label is Metabase's own `column_title` surface (`ObjectDetailsTable.tsx:56-57`) and
 * which drew the raw column name. That is fixed by THREADING — see `ObjectDetail.tsx` and its
 * tests. One of the four vizzes Metabase declares the setting on, brought to parity; nothing
 * withdrawn from the fourteen where the rule says it belongs.
 *
 * (That the control is still OFFERED on every non-table viz is pinned already, as the vacuity guard
 * of `settings/__tests__/SharedDisplaySections.test.tsx`'s `Display as` gate — which asserts
 * `Column title` present in the same accordion body it asserts `Display as` absent from. Adding an
 * `allowColumnTitle` gate would fail there; this file is the reason it should not be added.)
 */

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows: unknown[][] = [["EU", 10]];

describe("column_title reaches every viz through the export", () => {
  /**
   * THE LOAD-BEARING FACT, asserted behaviourally rather than by reading `Props`. `ChartExportMenu`
   * is handed a title, columns, rows, a container ref and the column settings — and no viz type at
   * all. It therefore cannot offer CSV on some charts and withhold it on others, which is what makes
   * "reachable on every viz" a property of the component rather than a coincidence of its two call
   * sites.
   */
  it("offers Download CSV without being told which chart it is on", async () => {
    const user = userEvent.setup();
    render(
      <ChartExportMenu title="t" columns={cols} rows={rows} containerRef={createRef<HTMLElement>()} />,
    );
    await user.click(screen.getByLabelText("Export chart"));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
  });

  it("renames the header of a column no chart builder reads", () => {
    // `rev` here stands for the gauge/funnel/sankey/map case: a metric on a viz whose built option
    // is byte-identical with and without the rename. The downloaded file is not.
    expect(rowsToCsv(cols, rows, { rev: { column_title: "Revenue" } }))
      .toBe("region,Revenue\r\nEU,10");
    // The comparison the sweep never made: not a tautology through an unrenamed run.
    expect(rowsToCsv(cols, rows)).toBe("region,rev\r\nEU,10");
  });
});
