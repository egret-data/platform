import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TableColumnsSection } from "../TableColumnsSection";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];

const manyCols = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ name: `col_${i}`, type: "varchar" }));

describe("TableColumnsSection", () => {
  // MOVED in PR B: the per-column CHECKBOX became an eye toggle on a row that also carries the
  // reorder arrows and the formatting drill-in. Same key, same emitted value, different control —
  // so these two assert the same thing they always did, through the control that now exists.
  it("renders a shown eye toggle per column", () => {
    render(<TableColumnsSection columns={COLS} value={{}} onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "Show a" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Show b" })).toHaveAttribute("aria-pressed", "true");
  });

  it("emits hiddenColumns when a column is hidden", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Show a" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a"] }));
  });

  it("emits pinnedLeft from the freeze input", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "1" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 1 }));
  });

  it("emits pagination enabled with default page size", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Paginate"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pagination: { enabled: true, pageSize: 25 } }));
  });

  // Part A: the filter box only earns its keep once the list is long enough that finding a
  // column by eye stops working.
  describe("column filter box", () => {
    it("appears with 40 columns", () => {
      render(<TableColumnsSection columns={manyCols(40)} value={{}} onChange={() => {}} />);
      expect(screen.getByLabelText(/search columns/i)).toBeInTheDocument();
    });

    it("does not appear with 5 columns", () => {
      render(<TableColumnsSection columns={manyCols(5)} value={{}} onChange={() => {}} />);
      expect(screen.queryByLabelText(/search columns/i)).not.toBeInTheDocument();
    });

    // Pin the threshold boundary itself — COLUMN_FILTER_MIN gets tuned in task 7 against the
    // panel's real width, and these two make that a one-line, safely-tested change.
    it("does not appear at 11 columns, one below the threshold", () => {
      render(<TableColumnsSection columns={manyCols(11)} value={{}} onChange={() => {}} />);
      expect(screen.queryByLabelText(/search columns/i)).not.toBeInTheDocument();
    });

    it("appears at 12 columns, exactly the threshold", () => {
      render(<TableColumnsSection columns={manyCols(12)} value={{}} onChange={() => {}} />);
      expect(screen.getByLabelText(/search columns/i)).toBeInTheDocument();
    });

    it("narrows the list to matching columns when typing", () => {
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={() => {}} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      expect(screen.getByRole("button", { name: "Show revenue" })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Show region" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Show col_0" })).not.toBeInTheDocument();
    });

    it("shows a no-match message when the filter matches no column, instead of a bare empty list", () => {
      const cols = [{ name: "revenue", type: "double" }, ...manyCols(15)];
      render(<TableColumnsSection columns={cols} value={{}} onChange={() => {}} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "zzz-nope" } });
      expect(screen.getByText(/no column matches/i)).toBeInTheDocument();
      expect(screen.getByText(/zzz-nope/)).toBeInTheDocument();
    });

    it("does not show the no-match message when the filter is empty", () => {
      render(<TableColumnsSection columns={manyCols(15)} value={{}} onChange={() => {}} />);
      expect(screen.queryByText(/no column matches/i)).not.toBeInTheDocument();
    });
  });

  // Part B: Show all / Hide all must act on every column, not the filtered subset — the trap
  // this task calls out explicitly.
  describe("Show all / Hide all", () => {
    it("Hide all emits every column name in hiddenColumns", () => {
      const onChange = vi.fn();
      render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
      fireEvent.click(screen.getByRole("button", { name: "Hide all" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a", "b"] }));
    });

    it("Show all emits an empty hiddenColumns", () => {
      const onChange = vi.fn();
      render(
        <TableColumnsSection columns={COLS} value={{ hiddenColumns: ["a", "b"] }} onChange={onChange} />,
      );
      fireEvent.click(screen.getByRole("button", { name: "Show all" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: [] }));
    });

    // Load-bearing (pinned): with a filter active, "Hide all" must still hide every column —
    // not just the ones the filter currently shows on screen. A filter is for FINDING a
    // column, not for scoping a bulk action.
    it("Hide all while a filter is active still hides every column, not just the matches", () => {
      const onChange = vi.fn();
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={onChange} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      fireEvent.click(screen.getByRole("button", { name: "Hide all" }));
      const [{ hiddenColumns }] = onChange.mock.calls[0];
      expect(hiddenColumns).toEqual(cols.map((c) => c.name));
    });

    it("toggling one column still works while a filter is active, and emits the raw name", () => {
      const onChange = vi.fn();
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={onChange} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      fireEvent.click(screen.getByRole("button", { name: "Show revenue" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["revenue"] }));
    });
  });

  // Part C: a renamed column shows its title, but stays findable by its raw name too, and
  // every non-display path (hiddenColumns, setAllHidden) still deals in raw names.
  describe("renamed columns (columnSettings)", () => {
    const cols = [
      { name: "total_revenue", type: "double" },
      { name: "region", type: "varchar" },
    ];
    const columnSettings = { total_revenue: { column_title: "Revenue" } };

    // MOVED in PR B, and the assertion INVERTED on purpose. The checkbox list showed the title
    // INSTEAD of the raw name; the merged row shows the title AND the raw name beside it, which is
    // the anatomy the Formatting accordion already had. The raw name is what the Data tab, the SQL
    // and the drill menu all call this column, so a row showing only "Revenue" is unrecognisable
    // from anywhere else in the app.
    it("displays the column_title with the raw name beside it", () => {
      render(
        <TableColumnsSection columns={cols} value={{}} onChange={() => {}} columnSettings={columnSettings} />,
      );
      expect(screen.getByText("Revenue")).toBeInTheDocument();
      expect(screen.getByText("total_revenue")).toBeInTheDocument();
    });

    it("is findable by typing the new title", () => {
      render(
        <TableColumnsSection
          columns={[...cols, ...manyCols(12)]}
          value={{}}
          onChange={() => {}}
          columnSettings={columnSettings}
        />,
      );
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "Revenue" } });
      expect(screen.getByRole("button", { name: "Show total_revenue" })).toBeInTheDocument();
    });

    it("is still findable by typing the original raw name", () => {
      render(
        <TableColumnsSection
          columns={[...cols, ...manyCols(12)]}
          value={{}}
          onChange={() => {}}
          columnSettings={columnSettings}
        />,
      );
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "total_revenue" } });
      expect(screen.getByRole("button", { name: "Show total_revenue" })).toBeInTheDocument();
    });

    it("hiding the renamed column emits its raw name", () => {
      const onChange = vi.fn();
      render(
        <TableColumnsSection columns={cols} value={{}} onChange={onChange} columnSettings={columnSettings} />,
      );
      fireEvent.click(screen.getByRole("button", { name: "Show total_revenue" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["total_revenue"] }));
    });
  });
});

// ═══ PR B: the three parallel column lists become one ═══
//
// THREE columns, and the renamed one in the MIDDLE, deliberately. PR A found that a two-column
// fixture cannot see a raw-name/display-name mix-up in an ORDER: losing a column out of the order
// and re-appending it produces the identical layout, so the assertion passes against the bug.
const THREE = [
  { name: "id", type: "bigint" },
  { name: "total_revenue", type: "double" },
  { name: "state", type: "varchar" },
];
const RENAMED = { total_revenue: { column_title: "Revenue" } };

// Reads the ROWS, by the raw column name each one is keyed on — not by the eye button's
// accessible name, which "Show all" above the list also matches.
const rowOrder = () =>
  Array.from(document.querySelectorAll("[data-column]")).map((el) => el.getAttribute("data-column"));

describe("TableColumnsSection — one row per column", () => {
  it("carries reorder, identity, the drill-in and visibility on the same row", () => {
    render(
      <TableColumnsSection columns={THREE} value={{}} onChange={() => {}}
        columnSettings={RENAMED} onColumnSettings={() => {}} />,
    );
    expect(screen.getByLabelText("Move total_revenue up")).toBeInTheDocument();
    expect(screen.getByLabelText("Move total_revenue down")).toBeInTheDocument();
    expect(screen.getByLabelText("settings total_revenue")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show total_revenue" })).toBeInTheDocument();
  });

  it("shows the rename beside the raw name, the anatomy the Formatting list already had", () => {
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} columnSettings={RENAMED} />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.getByText("total_revenue")).toBeInTheDocument();
  });

  it("renders in the stored columnOrder, not in result order", () => {
    render(<TableColumnsSection columns={THREE} value={{ columnOrder: ["state", "total_revenue", "id"] }} onChange={() => {}} />);
    expect(rowOrder()).toEqual(["state", "total_revenue", "id"]);
  });

  it("moving a column emits columnOrder for the whole list", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={THREE} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Move state up"));
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ columnOrder: ["id", "state", "total_revenue"] }),
    );
  });

  it("disables the up arrow on the first row and the down arrow on the last", () => {
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} />);
    expect(screen.getByLabelText("Move id up")).toBeDisabled();
    expect(screen.getByLabelText("Move state down")).toBeDisabled();
    expect(screen.getByLabelText("Move total_revenue up")).not.toBeDisabled();
  });

  /**
   * THE MUTATION GUARD. `columnOrder` is read by `ChartTable` through the RAW column name;
   * `column_title` renames the header without renaming the column. Emitting the displayed string
   * here writes an order the renderer can resolve nothing out of — the drill-on-display-strings
   * mistake this codebase has now made seven times, twice caught only by mutation.
   *
   * Three columns with the renamed one in the middle is the smallest fixture that can see it.
   */
  it("emits the RAW column name in columnOrder even when the column is renamed", () => {
    const onChange = vi.fn();
    render(
      <TableColumnsSection columns={THREE} value={{}} onChange={onChange} columnSettings={RENAMED} />,
    );
    fireEvent.click(screen.getByLabelText("Move total_revenue up"));
    const [{ columnOrder }] = onChange.mock.calls[0];
    expect(columnOrder).toEqual(["total_revenue", "id", "state"]);
    expect(columnOrder).not.toContain("Revenue");
  });

  // The stored order is keyed by raw name too — resolving it through the title would drop every
  // renamed column to the end of the list while the rendered table kept it where it was.
  it("ranks a renamed column by its raw name, not by its title", () => {
    render(
      <TableColumnsSection columns={THREE} value={{ columnOrder: ["state", "total_revenue", "id"] }}
        onChange={() => {}} columnSettings={RENAMED} />,
    );
    expect(rowOrder()).toEqual(["state", "total_revenue", "id"]);
  });
});

describe("TableColumnsSection — hidden columns stay in the list", () => {
  it("does not move a hidden column to the end or into a separate group", () => {
    render(<TableColumnsSection columns={THREE} value={{ hiddenColumns: ["total_revenue"] }} onChange={() => {}} />);
    expect(rowOrder()).toEqual(["id", "total_revenue", "state"]);
  });

  it("marks the hidden row so it reads as off without leaving the list", () => {
    const { container } = render(
      <TableColumnsSection columns={THREE} value={{ hiddenColumns: ["total_revenue"] }} onChange={() => {}} />,
    );
    expect(container.querySelector('[data-column="total_revenue"]')).toHaveAttribute("data-enabled", "false");
    expect(container.querySelector('[data-column="state"]')).toHaveAttribute("data-enabled", "true");
  });

  it("the eye button toggles the raw name into hiddenColumns", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={THREE} value={{}} onChange={onChange} columnSettings={RENAMED} />);
    fireEvent.click(screen.getByRole("button", { name: "Show total_revenue" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["total_revenue"] }));
  });

  it("the eye button toggles a hidden column back on", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={THREE} value={{ hiddenColumns: ["state"] }} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Show state" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: [] }));
  });

  it("reports its own state through aria-pressed, not only through colour", () => {
    render(<TableColumnsSection columns={THREE} value={{ hiddenColumns: ["state"] }} onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "Show state" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Show id" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("TableColumnsSection — the per-column drill-in", () => {
  it("keeps the formatting controls off screen until the row's button is clicked", async () => {
    const user = userEvent.setup();
    render(
      <TableColumnsSection columns={THREE} value={{}} onChange={() => {}}
        columnSettings={RENAMED} onColumnSettings={() => {}} />,
    );
    expect(screen.queryByLabelText("Number style")).toBeNull();
    await user.click(screen.getByLabelText("settings total_revenue"));
    expect(await screen.findByLabelText("Number style")).toBeInTheDocument();
  });

  /**
   * The rename lives in the drill-in, as Metabase's does — written under the RAW name, and MERGED
   * over the rest of the map.
   *
   * `state` carries a format here purely so the merge is observable. Without a second column's
   * settings in the fixture, a popover that REPLACED `column_settings` with its own entry emitted
   * a map indistinguishable from a correct merge, and that mutant survived.
   */
  it("writes column_settings under the raw column name, merged over the other columns'", async () => {
    const user = userEvent.setup();
    const onColumnSettings = vi.fn();
    render(
      <TableColumnsSection columns={THREE} value={{}} onChange={() => {}}
        columnSettings={{ ...RENAMED, state: { column_title: "Region" } }}
        onColumnSettings={onColumnSettings} />,
    );
    await user.click(screen.getByLabelText("settings total_revenue"));
    fireEvent.change(await screen.findByLabelText("Column title"), { target: { value: "Rev" } });
    expect(onColumnSettings).toHaveBeenCalledWith({
      total_revenue: { column_title: "Rev" },
      state: { column_title: "Region" },
    });
  });

  it("offers no drill-in where the caller has no column_settings writer", () => {
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} />);
    expect(screen.queryByLabelText("settings total_revenue")).toBeNull();
  });
});

describe("TableColumnsSection — Add or remove columns", () => {
  const queryFields = () => [
    { label: "Columns", options: ["id", "total_revenue", "state", "vendor"], selected: ["id", "total_revenue", "state"], onChange: vi.fn() },
    { label: "Metrics", options: ["revenue"], selected: [], onChange: vi.fn() },
  ];

  it("is absent where the caller cannot edit the query", () => {
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} />);
    expect(screen.queryByRole("button", { name: /add or remove columns/i })).toBeNull();
  });

  it("swaps the column list for the query field picker, and back", async () => {
    const user = userEvent.setup();
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} queryFields={queryFields()} />);
    expect(rowOrder()).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: /add or remove columns/i }));
    expect(rowOrder()).toHaveLength(0);
    expect(screen.getByLabelText("vendor")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /done picking columns/i }));
    expect(rowOrder()).toHaveLength(3);
  });

  // Disagreement #1: on an aggregated table a metric is in the display list and in NO field list
  // the display tab could reach — it lived in a fourth list, one tab over.
  it("reaches the metrics list from the same panel", async () => {
    const user = userEvent.setup();
    const groups = queryFields();
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} queryFields={groups} />);
    await user.click(screen.getByRole("button", { name: /add or remove columns/i }));
    await user.click(screen.getByLabelText("revenue"));
    expect(groups[1].onChange).toHaveBeenCalledWith(["revenue"]);
  });

  it("adds a column to the query under its raw name", async () => {
    const user = userEvent.setup();
    const groups = queryFields();
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} queryFields={groups} />);
    await user.click(screen.getByRole("button", { name: /add or remove columns/i }));
    await user.click(screen.getByLabelText("vendor"));
    expect(groups[0].onChange).toHaveBeenCalledWith(["id", "total_revenue", "state", "vendor"]);
  });
});

/**
 * A DASHBOARD CARD KNOWS ITS COLUMNS' NAMES AND NOT THEIR TYPES.
 *
 * `DashboardView` builds a card's column list out of the chart's stored SPEC, which carries names
 * only, so every dimension and raw column arrives here as `type: ""`. The old checkbox list showed
 * neither an icon nor a type, so it could not be wrong about one. The merged row shows both — and
 * `colKind("")` falls through to "Text", which would have put a TEXT badge and a text icon on
 * `subtotal` on every dashboard card in the app.
 *
 * An unknown type is rendered as an ABSENT type, not as a guess. (The underlying gap — that a card
 * carries no types at all, which also means its drill-in offers no numeric formatting — is
 * pre-existing and belongs to `DashboardView`, not here.)
 */
describe("TableColumnsSection — a column whose type is unknown", () => {
  const UNTYPED = [{ name: "subtotal", type: "" }, { name: "state", type: "" }];

  it("claims no type rather than claiming Text", () => {
    render(<TableColumnsSection columns={UNTYPED} value={{}} onChange={() => {}} />);
    expect(screen.queryByText("Text")).toBeNull();
    expect(screen.queryByText("Number")).toBeNull();
  });

  it("still badges a column whose type IS known", () => {
    render(<TableColumnsSection columns={THREE} value={{}} onChange={() => {}} />);
    expect(screen.getAllByText("Number").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Text").length).toBeGreaterThan(0);
  });

  it("keeps the row itself intact, so an untyped column is still orderable and hideable", () => {
    render(<TableColumnsSection columns={UNTYPED} value={{}} onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "Show subtotal" })).toBeInTheDocument();
    expect(screen.getByLabelText("Move state up")).toBeInTheDocument();
  });
});

/**
 * THREE BEHAVIOURS THIS FILE'S COMMENTS CLAIM AND NO ASSERTION HELD.
 *
 * A mutation sweep ran 64 mutations across 12 source files against 15 chart test files; 55 of 59
 * scoreable mutants died and four survived EVERY test in the suite. Three of them are here, and all
 * four share one shape: a behaviour argued for at length in a comment, implemented correctly, and
 * pinned by nothing. A comment is not a test — it does not fail when someone deletes the code it
 * describes, which is precisely what each mutation below did.
 *
 * Each was run against its mutant before being kept.
 */
describe("TableColumnsSection — the comments, now asserted", () => {
  /**
   * MUTANT: `disabled={!!q || i === 0}` → `disabled={i === 0}` (both arrows).
   *
   * The comment says "the row above a match is off screen, so 'up' would swap with something the
   * reader cannot see", and that is exactly what the mutant restores: `move()` swaps by the index
   * in `ordered` — the FULL list — while the reader is looking at the filtered one, so pressing ▲
   * on the top visible row silently reorders two columns neither of which is where they are
   * pointing. The search box only appears at `COLUMN_FILTER_MIN` columns, so the fixture has to be
   * that wide for the case to exist at all.
   */
  it("disables both reorder arrows while a search is filtering the list", async () => {
    const cols = manyCols(14);
    render(<TableColumnsSection columns={cols} value={{}} onChange={vi.fn()} />);
    // Unfiltered, a middle row moves in both directions — the state the assertion below contrasts.
    expect(screen.getByLabelText("Move col_7 up")).not.toBeDisabled();
    expect(screen.getByLabelText("Move col_7 down")).not.toBeDisabled();

    await userEvent.type(screen.getByLabelText("search columns"), "col_7");
    expect(screen.getByLabelText("Move col_7 up")).toBeDisabled();
    expect(screen.getByLabelText("Move col_7 down")).toBeDisabled();
    // And it says why, rather than looking broken.
    expect(screen.getByLabelText("Move col_7 up")).toHaveAttribute("title", "Clear the search to reorder");
  });

  it("gives the arrows back when the search is cleared", async () => {
    render(<TableColumnsSection columns={manyCols(14)} value={{}} onChange={vi.fn()} />);
    const box = screen.getByLabelText("search columns");
    await userEvent.type(box, "col_7");
    await userEvent.clear(box);
    expect(screen.getByLabelText("Move col_7 up")).not.toBeDisabled();
  });

  /**
   * MUTANT: `const Icon = c.type ? colIcon(c.type) : null` → `colIcon(c.type)`.
   *
   * `colIcon` falls through to the Text icon for anything it does not recognise, so the mutant
   * draws a confident "this is text" glyph next to `subtotal` on every dashboard card — cards carry
   * no column types at all. Only the BADGE half of that argument was pinned ("claims no type rather
   * than claiming Text" above, which asserts the absent word); the icon half survived, because an
   * icon has no text for `queryByText` to miss.
   */
  it("draws no type icon for a column whose type is unknown", () => {
    const { container } = render(
      <TableColumnsSection columns={[{ name: "subtotal", type: "" }]} value={{}} onChange={() => {}} />,
    );
    const row = container.querySelector('[data-column="subtotal"]')!;
    // Not "no svg anywhere in the row" — the eye and the drill-in are svgs too and are supposed to
    // be there. The icon slot is the element between the arrow stack and the name.
    const slot = row.children[1];
    expect(slot.tagName.toLowerCase()).toBe("span");
    expect(slot.querySelector("svg")).toBeNull();
  });

  it("still draws one where the type IS known, so the assertion above is about the type", () => {
    const { container } = render(
      <TableColumnsSection columns={[{ name: "subtotal", type: "double" }]} value={{}} onChange={() => {}} />,
    );
    const slot = container.querySelector('[data-column="subtotal"]')!.children[1];
    expect(slot.tagName.toLowerCase()).toBe("svg");
  });

  /**
   * MUTANT: `Math.max(0, Math.min(n, columns.length))` → `n`.
   *
   * `ChartTable` pins the first `pinnedLeft` columns and offsets them past the row-index gutter, so
   * a count past the end of the result is a promise the renderer cannot keep. `min`/`max` on the
   * input do not stop it — PR #398 measured that they only set `validity.*` flags at form
   * submission and block no keystroke — so the clamp has to be in the handler, and nothing asserted
   * that it was.
   */
  it("clamps a freeze count past the end of the result", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "99" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 2 }));
  });

  it("clamps a negative freeze count to zero", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "-3" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 0 }));
  });
});
