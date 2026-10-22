import { readFileSync, readdirSync } from "node:fs";
import { resolve, relative } from "node:path";
import { describe, it, expect } from "vitest";
import { formatValue } from "../format";
import type { ColumnFormat } from "../format-types";

/**
 * "DISPLAY AS" IS THE FLAT TABLE'S, AND NOTHING ELSE'S.
 *
 * WHY THIS FILE EXISTS. `FormatSection` offered a `Display as` select (auto / text / link / email /
 * image) and two link-template boxes to EVERY viz type, and `ChartTable.renderTableCell` is the only
 * code in the app that branches on what they say. So a pivot row-dimension, a cartesian x-axis, a
 * pie dimension, a sankey node column and an object-detail field each showed a control that saved a
 * value and painted nothing — the same defect `show_mini_bar` had, closed the same way, with an
 * `allowViewAs` gate mirroring `allowMiniBar`.
 *
 * That gate is only correct while the premise under it holds, and the premise is a claim about
 * every OTHER renderer in the app. `SharedDisplaySections.test.tsx` pins the control's absence;
 * this file pins the reason it is safe to be absent, from two directions:
 *
 *   1. THE SHARED FORMATTER IGNORES THEM. Every viz but the table turns a cell value into text
 *      through `formatValue` — the ECharts label/tooltip formatters, the pivot's cells, the object
 *      detail's fields, the big number, the map's tooltip. If `formatValue` is blind to these three
 *      fields then none of those surfaces can be anything but blind to them too.
 *
 *   2. NOBODY ELSE MENTIONS THEM. A renderer could always start reading `view_as` directly rather
 *      than through `formatValue`, and no behavioural test over today's charts would notice the
 *      DAY IT WAS ADDED — it would notice only if someone thought to write a case for that chart.
 *      So the second assertion is a source scan with a named allow-list: a new file that touches
 *      any of the three keys fails here and has to be argued for.
 *
 * HOW THE PREMISE WAS ESTABLISHED IN THE FIRST PLACE — by running, not by reading, because the plan
 * this came from flagged it as a code-read assumption. A probe rendered all nineteen viz types
 * through `ChartRenderer` with `view_as` / `link_text` / `link_url` behind recording getters that
 * captured a stack on every access. `table` was the only viz type whose stacks reached a CONSUMING
 * read (`ChartTable.tsx` `renderTableCell`). Every other access was an object spread copying the
 * whole format into a new one bound for `formatValue` — `cartesian.ts`'s `{ ...col, compact }`,
 * `scalar.ts`'s `compactifyValue`, `boxplot.ts`'s `fmt` — or a `JSON.stringify` memo key in
 * `ChartRenderer`. The probe is not committed: it asserted nothing and its whitelist would have
 * been a list of line numbers. The two assertions below are what it turned into.
 */

const LINKY: ColumnFormat = { view_as: "link", link_text: "L {{value}}", link_url: "/x/{{value}}" };

describe("formatValue is blind to view_as, link_text and link_url", () => {
  // One per branch of `formatValue`: temporal goes to `formatDate`, numeric to `formatNumber`, and
  // a plain string falls out of the bottom. A single string case would leave two thirds of the
  // function free to grow a reader.
  const CASES: { label: string; value: unknown; type: string; base: ColumnFormat }[] = [
    { label: "a text column", value: "https://example.com", type: "varchar", base: {} },
    { label: "a text column with a prefix", value: "abc", type: "varchar", base: { prefix: ">" } },
    { label: "a numeric column", value: 1234.5, type: "double", base: { decimals: 2 } },
    { label: "a currency column", value: 1234.5, type: "bigint", base: { number_style: "currency" } },
    { label: "a temporal column", value: "2026-01-31T05:06:07Z", type: "timestamp", base: { date_style: "M/D/YYYY" } },
  ];

  it.each(CASES)("$label formats identically with and without them", ({ value, type, base }) => {
    const plain = formatValue(value, type, base);
    const linky = formatValue(value, type, { ...base, ...LINKY });
    expect(linky).toBe(plain);
    // Not a tautology through an empty string: a case that formatted to "" would satisfy the
    // equality above however the function behaved.
    expect(plain.length).toBeGreaterThan(0);
  });

  // The declaration `formatDate` reads its own field list from. It is `satisfies
  // Record<keyof ColumnFormat, boolean>`, so a field added to `ColumnFormat` cannot be quietly
  // omitted — but the VALUES are still hand-written, and these three are the ones this gate rests
  // on. Asserted through `specifiesDateRendering`, the only exported thing that consults the map:
  // a date column given nothing but these three still "says nothing about how a date renders".
  it("a format that says only view_as/link_text/link_url specifies no date rendering", async () => {
    const { specifiesDateRendering } = await import("../format");
    expect(specifiesDateRendering(LINKY)).toBe(false);
    expect(specifiesDateRendering({ ...LINKY, date_style: "YYYY" })).toBe(true);
  });
});

/**
 * Source with its comments taken out, so the scan below sees CODE.
 *
 * Necessary rather than fastidious: this codebase argues at length in prose, and the two files that
 * pass `allowViewAs={false}` explain what they are switching off by naming it. A scan that counted
 * those would put the gate's own paperwork on the suspect list and then have to keep it there
 * forever. Block comments (`/*…*\/`, which covers JSDoc and JSX `{/*…*\/}`) and whole-line `//`
 * are removed; a trailing `//` on a line that also has code is deliberately NOT, because deciding
 * where the code ends means parsing string literals — and over-inclusive is the safe direction
 * here: at worst it asks for a decision that was not strictly needed.
 */
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .filter((l) => !/^\s*(\/\/|\*)/.test(l))
    .join("\n");
}

/** Every `.ts`/`.tsx` under `src`, minus test files — the production surface. */
function productionSources(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = resolve(dir, e.name);
    if (e.isDirectory()) {
      if (e.name !== "__tests__" && e.name !== "generated") productionSources(p, out);
    } else if (/\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name)) {
      out.push(p);
    }
  }
  return out;
}

describe("nothing but ChartTable renders from view_as", () => {
  /**
   * Every production file allowed to mention `view_as`, `link_text` or `link_url`, and what for.
   *
   * The point of the file list is the ABSENCE of a renderer. `ChartTable` is the reader; the pivot,
   * the object detail, the big number, the map and all eleven ECharts charts are not on it and must
   * not become so without this failing first. `PreviewTab` and `ChartExplore` derive DEFAULTS from
   * a dataset column's semantic type and hand them to a `ChartTable` — they write, they do not
   * render — and `FormatPicker` is the dataset's own format editor, a different panel over the same
   * stored shape.
   */
  const ALLOWED: Record<string, string> = {
    "components/chart/ChartTable.tsx": "the ONLY renderer: renderTableCell branches on view_as",
    "components/chart/format-types.ts": "declares the three fields on ColumnFormat",
    "components/chart/format.ts": "CONSULTED_BY_FORMAT_DATE marks all three false",
    "components/chart/settings/FormatSection.tsx": "the control, now gated by allowViewAs",
    "components/dataset/columnDefaults.ts":
      "derives semantic-type defaults that PreviewTab and ChartExplore layer under a ChartTable",
    "components/dataset/FormatPicker.tsx": "the dataset's own format editor",
  };

  const SRC = resolve(__dirname, "../../..");
  const KEYS = /\bview_as\b|\blink_text\b|\blink_url\b/;

  it("only the allow-listed files name the three fields in code", () => {
    const found = productionSources(SRC)
      .filter((p) => KEYS.test(stripComments(readFileSync(p, "utf8"))))
      .map((p) => relative(SRC, p).split("\\").join("/"))
      .sort();
    expect(found).toEqual(Object.keys(ALLOWED).sort());
  });

  it("every allow-listed file says why it is there", () => {
    for (const reason of Object.values(ALLOWED)) expect(reason.length).toBeGreaterThan(20);
  });
});
