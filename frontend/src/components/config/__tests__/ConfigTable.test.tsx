import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";

import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";

describe("ConfigTableGroup", () => {
  it("spans the whole table, so a group heading is not mistaken for a Setting", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span /></ConfigTableRow>
      </ConfigTable>,
    );
    const cell = screen.getByText("Cache").closest("td");
    // colSpan is the point: a group row that occupied only the Setting column
    // would read as a setting with no value, which is what the cards were
    // replaced to avoid.
    expect(cell).toHaveAttribute("colspan", "3");
  });

  it("keeps ordinary rows in their three columns, positioned after the group row", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span data-testid="ctl" /></ConfigTableRow>
      </ConfigTable>,
    );
    // Getting the group row by its accessible role, not just its text, means a
    // group that rendered nothing (or rendered without the rowheader role)
    // fails right here, before either assertion below runs.
    const groupRow = screen.getByRole("rowheader", { name: "Cache" }).closest("tr");
    const dataRow = screen.getByText("Enabled").closest("tr");
    // Position, not just presence: the data row has to immediately follow the
    // group row it belongs to. A group that rendered nothing leaves no <tr> to
    // be "before" — this is what `return null` cannot satisfy.
    expect(groupRow?.nextElementSibling).toBe(dataRow);
    // Still three columns of its own — the group's single spanning cell is a
    // different row, not a fourth cell bleeding into this one.
    expect(dataRow?.querySelectorAll("td")).toHaveLength(3);
    expect(screen.getByTestId("ctl")).toBeInTheDocument();
  });

  it("is announced as a heading for its group, not as data", () => {
    render(<ConfigTable><ConfigTableGroup label="Rate limit" /></ConfigTable>);
    // A screen reader hitting a bare cell mid-table learns nothing about
    // structure; the row has to say it starts a section.
    expect(screen.getByRole("rowheader", { name: "Rate limit" })).toBeInTheDocument();
  });
});

/**
 * The Value column's width is a `<th>` class and nothing else can move it.
 * Under `table-layout: fixed` a column is exactly as wide as its header says,
 * so a control that wants more room cannot get it from a wrapper inside the
 * cell — an inner `w-96` in a 208px cell overflows into its neighbour rather
 * than widening the column. `valueClassName` is therefore the only place the
 * knob can live, and these two tests are the additive contract: eleven other
 * consumers pass nothing and must keep the width they have.
 */
/** The `lg:table-cell` Details cell for a row, as opposed to its `lg:hidden` inline twin. */
function detailsCellFor(text: string): HTMLTableCellElement | null {
  return (
    screen
      .getAllByText(text)
      .map((el) => el.closest("td"))
      .find((td) => td?.className.includes("lg:table-cell")) ?? null
  );
}

describe("ConfigTable — Value column width", () => {
  it("defaults the Value column to w-52 and nothing else, so existing consumers are unchanged", () => {
    render(
      <ConfigTable>
        <ConfigTableRow label="Enabled"><span /></ConfigTableRow>
      </ConfigTable>,
    );
    const valueHead = screen.getByRole("columnheader", { name: "Value" });
    // `toHaveClass` is CONTAINMENT, and containment is not what "unchanged"
    // means here: a default of `"w-52 lg:w-96"` satisfies `toHaveClass("w-52")`
    // while silently doubling this column at `lg` on eleven other pages. So
    // collect every width utility on the header, responsive variants included,
    // and pin the whole set.
    const widths = valueHead.className
      .split(/\s+/)
      .filter((c) => /^(?:[a-z0-9]+:)*w-/.test(c));
    expect(widths).toEqual(["w-52"]);
  });

  it("lets a caller widen it, and the default does not come along", () => {
    render(
      <ConfigTable valueClassName="w-96">
        <ConfigTableRow label="Enabled"><span /></ConfigTableRow>
      </ConfigTable>,
    );
    const valueHead = screen.getByRole("columnheader", { name: "Value" });
    expect(valueHead).toHaveClass("w-96");
    // An assertion about the rendered `<th>`, not about how it got there.
    // `TableHead` runs its className through `cn` (= `twMerge`), which already
    // collapses two conflicting width utilities to the last one — so writing
    // the default and the override into the same string still renders `w-96`
    // and is not a defect. What would be a defect is the default winning, and
    // that is exactly what this line catches.
    expect(valueHead).not.toHaveClass("w-52");
  });

  it("leaves the Details cell unwrapped by default, so existing consumers are unchanged", () => {
    render(
      <ConfigTable>
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span /></ConfigTableRow>
      </ConfigTable>,
    );
    // Eleven consumers outside this programme render this cell and none of them
    // were measured. Several (EngineSecurity, StarRocksConfigTab,
    // WorkspaceFeaturesTab) sit on control-plane pages with no width cap, so a
    // measure applied unconditionally here would visibly re-wrap their help
    // text. The default emits no wrapper element at all.
    const cell = detailsCellFor("Serve repeat calls from a cache.");
    expect(cell?.querySelector("div")).toBeNull();
  });

  /**
   * A class-name assertion, and weak on purpose — the same bargain
   * `docs/en/ui-conventions.md` §5.5 records for the width guards. jsdom does
   * no layout (§10), so it cannot say the sentence wraps; the real claim is
   * made in a browser by `tests/e2e/data-api-narrow-and-cells.spec.ts`, which
   * measured 1103px of caveat body, 498px of empty note and 807px of error
   * alert inside a 304px cell before this class existed. That spec is opt-in
   * and `.github/workflows` runs no Playwright step, so nothing in CI would
   * notice this class being deleted as redundant. This is the line that would.
   *
   * All three prose blocks live in ONE cell for a reason (§4.3: an explanation
   * about a control sits beside that control), so the fix belongs to the cell.
   * An earlier pass put `whitespace-normal` on `Caveat`'s `<summary>` alone and
   * the other two blocks kept clipping — which is why this assertion is on the
   * Value cell of `ConfigTable`, not on any caller.
   */
  it("lets prose in the Value cell wrap, rather than inheriting the table's nowrap", () => {
    render(
      <ConfigTable>
        <ConfigTableRow label="Enabled"><span data-testid="ctl" /></ConfigTableRow>
      </ConfigTable>,
    );
    const valueCell = screen.getByTestId("ctl").closest("td");
    expect(valueCell).toHaveClass("whitespace-normal");
    // The other half, and not the same claim: `whitespace-normal` gives the text
    // permission to wrap, `break-words` gives it somewhere to do so. This cell
    // renders a server error message verbatim, which can arrive as one
    // unbreakable token that wrapping alone cannot help.
    expect(valueCell).toHaveClass("break-words");
  });

  it("caps the Details cell's prose at a readable measure when the table asks for it", () => {
    render(
      <ConfigTable detailsClassName="max-w-prose">
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span /></ConfigTableRow>
      </ConfigTable>,
    );
    // With the page-level cap gone, `table-fixed` hands ALL surplus width to
    // the one unsized column — Details. On a wide monitor that is a single
    // muted line hundreds of characters long. The measure is capped on the
    // cell's content so the column may still absorb the width.
    expect(detailsCellFor("Serve repeat calls from a cache.")?.querySelector(".max-w-prose")).not.toBeNull();
  });
});
