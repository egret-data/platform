// src/components/chart/PieBreakdown.tsx
import { cn } from "@/lib/utils";
import type { PieBreakdownModel } from "@/components/chart/echarts/options/pie";

/** Separator between the ancestor names in the `Within` cell — the breadcrumb that says which of
 * the several wedges called "Widgets" this row is nested in. The tooltip's own header joins the
 * same chain with `>`; a chevron is the quieter form of the same thing in real markup, and it is
 * punctuation a screen reader passes over rather than reading as an operator. */
const CRUMB = " › ";
/** How far a row's name is pushed in per ring. Decoration only — the `Ring` and `Within` cells
 * carry the same fact in a form a screen reader can read (see this file's header). */
const INDENT_PER_RING_PX = 12;

/**
 * The pie's arcs, as a table — the structured alternative to a chart that a screen-reader user
 * cannot see, and a keyboard user cannot point at.
 *
 * WHY THIS EXISTS. A sunburst past two levels encodes meaning in COLOUR (a family, plus a lightened
 * shade per ring) and in POSITION (a nested arc under one specific parent wedge). Neither survives
 * a text summary, so the chart's `aria` description could only ever read out ring 0 — the outer
 * rings simply weren't there for anyone not looking at the picture. `ui-ux-pro-max` grades the
 * shape C for accessibility for exactly this reason and calls a table alternative mandatory for
 * production use. This is it: every drawn slice, under its own parent, with its share OF that
 * parent — the same number the chart's tooltip and labels print, from the same builder.
 *
 * THREE DECISIONS, all deliberate:
 *
 * 1. A VISIBLE DISCLOSURE, not a `sr-only` region. A permanently hidden table is the cheaper
 *    answer and it is the wrong one: the reader who cannot decode three rings of lightened shades
 *    is very often sighted — low vision, colour vision deficiency, or just a 200px dashboard card —
 *    and a hidden table gives them nothing. `<details>` is one line of layout when closed, one
 *    stop in the tab order, and it is announced as a disclosure with its state, so a screen-reader
 *    user finds it by the same route everyone else does rather than by a route built only for them.
 *    Closed by default because the chart is the point and the table can run to dozens of rows.
 *
 * 2. A REAL `<table>`, not a nested list. The number this exists to carry — a slice's share of its
 *    PARENT — is only meaningful next to a stated denominator and only comparable down a labelled
 *    column; in an indented `<li>` it is loose text a reader has to hold in their head. A table
 *    gives row/column navigation and reads each cell WITH its header ("Share of parent, 30.0%"),
 *    which is precisely the pairing that was missing. The hierarchy a nested list would have given
 *    structurally is carried explicitly instead, and more legibly: rows come in the chart's own
 *    depth-first draw order, each states its ring (by dimension name) and the breadcrumb of wedges
 *    it sits inside, and the name is indented as the sighted reader's version of the same fact.
 *
 * 3. MULTI-RING ONLY. `buildPieBreakdown` returns no rows below two rings and this renders nothing
 *    at all. A single-ring pie is a flat (name, value) list, which the chart's `aria` description
 *    already reads out in full; a second copy of it would make a screen-reader user hear the same
 *    thing twice, which is a cost, not a gain.
 *
 * Markup, tokens and semantics follow the React legend beside it (PieLegend.tsx), which is this
 * chart's precedent for "chart-adjacent content is real markup, not canvas".
 */
export function PieBreakdown({ breakdown, defaultOpen }: {
  breakdown: PieBreakdownModel;
  /** Tests and any future caller that wants it open on arrival; the app leaves it closed. */
  defaultOpen?: boolean;
}) {
  const { rows, levels, metric, notes } = breakdown;
  // Nothing to add — a single-ring pie, or no ring at all. See decision 3 above.
  if (rows.length === 0) return null;

  const head = "px-2 py-1 font-medium";
  const cell = "px-2 py-0.5 align-top";
  return (
    <details
      data-pie-breakdown
      open={defaultOpen || undefined}
      className="min-h-0 shrink-0 border-t border-border text-xs"
    >
      {/* Says what opening it gets you. "Details" alone would make the one control a screen-reader
          user has to find in order to read the chart the least descriptive thing on the page. */}
      <summary className="cursor-pointer select-none px-2 py-1 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
        Slice breakdown — {rows.length} slices across {levels.length} rings
      </summary>
      {/* Bounded and scrollable, like the legend's own list: a three-ring chart can run to dozens
          of rows, and an open disclosure must not push the chart out of its own card. */}
      <div className="max-h-56 overflow-auto pb-2">
        {/* Before the table, not after it: this qualifies every number below, and a caveat read
            out once the reader has already been through the rows is a caveat that arrived late. */}
        {notes.map((note) => (
          <p key={note} className="px-2 pb-1 text-muted-foreground">{note}</p>
        ))}
        <table className="w-full border-collapse text-left">
          {/* The table's accessible name, for a reader who lands on it out of context. Visually
              redundant next to the summary that was just read, hence `sr-only`. */}
          <caption className="sr-only">
            Every slice the chart draws, innermost ring first, with its value and its share of the
            slice it sits inside.
          </caption>
          <thead>
            <tr className="text-muted-foreground">
              <th scope="col" className={head}>Ring</th>
              <th scope="col" className={head}>Slice</th>
              <th scope="col" className={head}>Within</th>
              <th scope="col" className={cn(head, "text-right")}>{metric}</th>
              <th scope="col" className={cn(head, "text-right")}>Share of parent</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => (
              // Index-keyed deliberately: this is a static list rebuilt whole whenever the chart's
              // data or settings change, and a name-based key would collide the moment two
              // different parents both fold an "Other" bucket — which is the common case here.
              <tr key={i} className="border-t border-border/50">
                <td className={cn(cell, "text-muted-foreground")}>{levels[row.depth]}</td>
                <td className={cell} style={{ paddingInlineStart: row.depth * INDENT_PER_RING_PX + 8 }}>
                  {row.name}
                </td>
                {/* At ring 0 the "parent" IS the chart, and saying so is the point: the share in
                    the last column changes denominator with depth, so every row states its own. */}
                <td className={cn(cell, "text-muted-foreground")}>
                  {row.parents.length > 0 ? row.parents.join(CRUMB) : "Whole chart"}
                </td>
                <td className={cn(cell, "text-right tabular-nums")}>{row.value}</td>
                <td className={cn(cell, "text-right tabular-nums text-muted-foreground")}>{row.share}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </details>
  );
}
