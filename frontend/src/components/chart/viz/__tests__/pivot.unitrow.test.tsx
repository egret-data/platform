import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { createElement } from "react";
import { pivotDescriptor, type PivotConfig } from "../pivot";
import type { VizControlProps } from "../types";

const d = pivotDescriptor();
const rowsControl = d.controls.find((c) => c.id === "rows")!;
const colsControl = d.controls.find((c) => c.id === "cols")!;

/** Stands in for the shell's real `unitRow`, which only draws for a temporal column. */
const unitRow = (col?: string) =>
  col && col.endsWith("_month") ? <div data-testid={`unit-${col}`}>Group {col} by</div> : null;

const props = (config: Partial<PivotConfig>) => ({
  config: { rows: [], cols: [], metrics: [], ...config },
  onChange: () => {},
  columns: [
    { column_name: "order_month", type: "timestamp" },
    { column_name: "ship_month", type: "timestamp" },
    { column_name: "region", type: "text" },
  ],
  metrics: [], resultColumns: [], rows: [],
  temporalUnits: {}, columnSettings: {}, unitRow,
} as unknown as VizControlProps<PivotConfig>);

/**
 * A grouping control belongs UNDER THE ZONE HOLDING THE FIELD IT GROUPS.
 *
 * Both zones' unit rows were emitted together at the end of `ColsControl`, so a date in ROWS had
 * its control drawn after the entire COLUMNS zone — two headings below the only field it affects.
 * Seen live on the kind deployment: `ROWS / order_month / Add rows… / COLUMNS / Add columns… /
 * UNIT · ORDER_MONTH [Month] / VALUES`.
 *
 * The two controls are rendered SEPARATELY here, exactly as `StudioPanel` renders them, so the test
 * asks the question the panel asks: does the Rows control emit the row for a rows field, and does
 * the Cols control leave it alone? A single combined render could not tell the difference —
 * everything would be present either way and the test would pass against the defect.
 */
describe("the pivot's date-grouping control", () => {
  it("draws a rows field's grouping inside the Rows control", () => {
    render(createElement(rowsControl.render, props({ rows: ["order_month"] })));
    expect(screen.getByTestId("unit-order_month")).toBeInTheDocument();
  });

  it("does not draw a rows field's grouping inside the Columns control", () => {
    render(createElement(colsControl.render, props({ rows: ["order_month"] })));
    expect(screen.queryByTestId("unit-order_month")).not.toBeInTheDocument();
  });

  it("draws a columns field's grouping inside the Columns control", () => {
    render(createElement(colsControl.render, props({ cols: ["ship_month"] })));
    expect(screen.getByTestId("unit-ship_month")).toBeInTheDocument();
  });

  // Both zones dated at once was the case that forced the field name into the label; it must still
  // produce one control per field, each under its own zone.
  it("keeps one control per field when both zones carry a date", () => {
    const cfg = { rows: ["order_month"], cols: ["ship_month"] };
    const { unmount } = render(createElement(rowsControl.render, props(cfg)));
    expect(screen.getByTestId("unit-order_month")).toBeInTheDocument();
    expect(screen.queryByTestId("unit-ship_month")).not.toBeInTheDocument();
    unmount();
    render(createElement(colsControl.render, props(cfg)));
    expect(screen.getByTestId("unit-ship_month")).toBeInTheDocument();
    expect(screen.queryByTestId("unit-order_month")).not.toBeInTheDocument();
  });

  // A zone whose whole option list is empty used to render its heading over nothing at all — live,
  // the Values zone on a dataset that defines no metrics.
  it("explains an empty zone instead of leaving a heading over blank space", () => {
    const values = d.controls.find((c) => c.id === "values")!;
    render(createElement(values.render, props({})));
    expect(screen.getByText(/Nothing available for values/i)).toBeInTheDocument();
  });
});
