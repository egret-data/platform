import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ObjectDetail } from "../ObjectDetail";

const cols = [{ name: "id", type: "int" }, { name: "plan", type: "text" }];
const rows = [[1, "Basic"], [2, "Premium"]];

describe("ObjectDetail", () => {
  it("renders the first record's field/value pairs", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    expect(screen.getByText("id")).toBeTruthy();
    expect(screen.getByText("plan")).toBeTruthy();
    expect(screen.getByText("Basic")).toBeTruthy();
    expect(screen.getByText("1 / 2")).toBeTruthy();
  });

  it("pages to the next record and clamps the buttons", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    const prev = screen.getByLabelText("previous record") as HTMLButtonElement;
    const next = screen.getByLabelText("next record") as HTMLButtonElement;
    expect(prev.disabled).toBe(true);
    fireEvent.click(next);
    expect(screen.getByText("Premium")).toBeTruthy();
    expect(screen.getByText("2 / 2")).toBeTruthy();
    expect((screen.getByLabelText("next record") as HTMLButtonElement).disabled).toBe(true);
  });

  it("hides the nav bar for a single record", () => {
    render(<ObjectDetail columns={cols} rows={[[1, "Basic"]]} />);
    expect(screen.queryByLabelText("next record")).toBeNull();
  });

  it("shows 'No data' for empty rows", () => {
    render(<ObjectDetail columns={cols} rows={[]} />);
    expect(screen.getByText("No data")).toBeTruthy();
  });

  /**
   * Metabase prints the word "Empty" in a muted tone for an absent value. egret rendered "", a
   * blank row — which in a view of ONE record reads as a rendering fault rather than as "this
   * field has no value", the single most important thing a detail view has to be able to say.
   */
  it("says Empty rather than leaving the row blank", () => {
    render(<ObjectDetail columns={cols} rows={[[null, ""]]} />);
    expect(screen.getAllByText("Empty")).toHaveLength(2);
  });

  // A JSON column arrives as one long line. Metabase pretty-prints it in a <pre>; egret ran it
  // through the plain formatter, so a nested object was an unreadable single row.
  it("pretty-prints a JSON value", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "meta", type: "text" }]} rows={[['{"a":1,"b":[2,3]}']]} />,
    );
    const pre = container.querySelector("pre");
    expect(pre).toBeTruthy();
    expect(pre!.textContent).toContain('"a": 1');
    expect(pre!.textContent!.split("\n").length).toBeGreaterThan(3);
  });

  it("leaves a string that only looks like JSON alone", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[["{not json}"]]} />,
    );
    expect(container.querySelector("pre")).toBeNull();
    expect(screen.getByText("{not json}")).toBeTruthy();
  });

  // One 4,000-character note otherwise pushes every other field off the card.
  it("folds a long value behind Show more, and unfolds it", () => {
    const long = "x".repeat(400);
    render(<ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[[long]]} />);
    expect(screen.queryByText(long)).toBeNull();
    fireEvent.click(screen.getByText("Show more"));
    expect(screen.getByText(long, { exact: false })).toBeTruthy();
    expect(screen.getByText("Show less")).toBeTruthy();
  });

  // Paging must not carry one record's expanded state onto a different record's value.
  it("collapses an expanded value when the record changes", () => {
    const long = "y".repeat(400);
    render(<ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[[long], ["short"]]} />);
    fireEvent.click(screen.getByText("Show more"));
    expect(screen.getByText("Show less")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("next record"));
    fireEvent.click(screen.getByLabelText("previous record"));
    expect(screen.getByText("Show more")).toBeTruthy();
  });

  it("renders an image URL as the image", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "avatar", type: "text" }]} rows={[["https://x.test/a.png"]]} />,
    );
    expect(container.querySelector("img")?.getAttribute("src")).toBe("https://x.test/a.png");
  });

  // A plain link is NOT an image. Replacing a readable URL with a broken-image icon is worse than
  // showing the URL, so the match is deliberately narrow.
  it("leaves a non-image URL as text", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "site", type: "text" }]} rows={[["https://x.test/page"]]} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText("https://x.test/page")).toBeTruthy();
  });

  /**
   * THE FIELD LABEL IS WHERE A RENAME BELONGS, and this view drew the raw column name in the one
   * place the whole visualisation consists of.
   *
   * `SharedDisplaySections` hands an object detail a Formatting accordion with a Column title box
   * for every result column, and the `<dd>` beside this label has always honoured the rest of that
   * same `ColumnFormat` — currency, decimals, date style, prefix and suffix all reach `Value`. Only
   * the name half went nowhere.
   *
   * THIS IS METABASE'S OWN BEHAVIOUR, not an egret extension, and it is the reason the fix here is
   * to THREAD rather than to withdraw the control. `column_title` is not a general Metabase column
   * setting: exactly four visualizations declare it (`Table`, `ObjectDetail`, `List`, `PivotTable`
   * — it is absent from `COMMON_COLUMN_SETTINGS` and comes only from a viz's own static
   * `columnSettings`), and Object detail is one of the four. `ObjectDetail.tsx:34` registers it and
   * `components/ObjectDetail/ObjectDetailsTable.tsx:56-57` paints it into the label cell:
   * `columnSettings?.["_column_title_full"] || displayNameForColumn(column)`.
   *
   * THE `title=` ATTRIBUTE FOLLOWS IT. The label is `truncate`d, and the tooltip is what a reader
   * with a long name has to fall back on — leaving it on the raw name would make the hover
   * contradict the text under the cursor.
   *
   * THE DRILL DOES NOT. `onFieldClick` is still handed `c.name`: a filter is built against the
   * COLUMN, and this programme has closed the same raw-vs-display defect eight times in the other
   * direction. The rename is display, and display is where it stops.
   */
  it("draws a renamed column's title as the field label", () => {
    render(
      <ObjectDetail columns={cols} rows={rows} columnSettings={{ plan: { column_title: "Subscription" } }} />,
    );
    expect(screen.getByText("Subscription")).toBeTruthy();
    expect(screen.queryByText("plan")).toBeNull();
  });

  it("puts the rename on the truncation tooltip too", () => {
    const { container } = render(
      <ObjectDetail columns={cols} rows={rows} columnSettings={{ plan: { column_title: "Subscription" } }} />,
    );
    const labels = Array.from(container.querySelectorAll("dt"));
    expect(labels.map((el) => el.getAttribute("title"))).toEqual(["id", "Subscription"]);
  });

  it("leaves an unrenamed column on its own name", () => {
    render(
      <ObjectDetail columns={cols} rows={rows} columnSettings={{ plan: { column_title: "Subscription" } }} />,
    );
    expect(screen.getByText("id")).toBeTruthy();
  });

  it("drills on the RAW column name, not on the rename", () => {
    const clicks: unknown[][] = [];
    render(
      <ObjectDetail columns={cols} rows={rows}
        columnSettings={{ plan: { column_title: "Subscription" } }}
        onFieldClick={(col, value) => clicks.push([col, value])} />,
    );
    fireEvent.click(screen.getByText("Basic"));
    expect(clicks).toEqual([["plan", "Basic"]]);
  });

  // `||`, not `??` — the same choice `rowsToCsv` makes and for the same reason. Settings are not
  // key-whitelisted server-side, so a hand-edited `column_title: ""` is reachable, and `??` would
  // draw a field with no label at all.
  it("falls back to the column name for an empty rename", () => {
    render(<ObjectDetail columns={cols} rows={rows} columnSettings={{ plan: { column_title: "" } }} />);
    expect(screen.getByText("plan")).toBeTruthy();
  });
});
