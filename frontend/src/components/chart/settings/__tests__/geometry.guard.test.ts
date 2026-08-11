import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

/**
 * WHY THIS IS A SOURCE TEST AND NOT A RENDER TEST.
 *
 * jsdom performs no layout: every `getBoundingClientRect()` returns zeros. A test asserting "the
 * select is 352 wide" therefore passes against an unchanged tree and proves nothing — it would be
 * one more green-but-empty assertion, of which this codebase has had six.
 *
 * The thing that actually regresses is a contributor reaching for `h-8` to make one control fit.
 * So that is what is pinned. Re-add a single banned class and this goes red.
 *
 * The primitives' own defaults (`h-10` after this programme, `w-full`) are not overrides and are
 * not banned — only the classes that narrow or shorten a control against them. `data-[size=sm]`
 * on the Select primitive is likewise untouched: it is used outside the chart module.
 */
/**
 * The boundaries are `[^\w-]`, not `\b`. `\b` treats a hyphen as a word boundary, so `\bh-8\b`
 * also matches the `h-8` inside `min-h-8` — a legitimate row-wrapper class (see
 * `TableColumnsSection.tsx:197`) and not a control override. That false positive would have
 * failed this guard for the wrong reason.
 */
const BANNED = /(?<![\w-])(h-7|h-8|w-16|w-24|w-28)(?![\w-])/g;

/**
 * `settings/` is where the panel's controls live, but not the only place: two colour inputs sat in
 * `viz/` on `h-7`, kept their 28px height through the whole height pass, and were found only by
 * measuring the live panel. A guard scoped to one directory would have stayed green through that.
 */
const DIRS = [join(__dirname, ".."), join(__dirname, "..", "..", "viz")];

/**
 * COMMENTS ARE NOT CODE, and this guard reads raw text.
 *
 * The rules below scan for class names, and a doc comment that EXPLAINS why `h-8` is banned
 * contains the string `h-8` — so documenting the ban failed the ban. That happened on the very
 * commit that added the padding rule. Prose is stripped first so a file can describe its own
 * geometry without tripping the test that enforces it.
 *
 * The `[^:]` before `//` keeps `https://` in a URL from eating the rest of its line.
 */
const stripComments = (src: string) =>
  src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");

const filesIn = (d: string) =>
  readdirSync(d)
    .filter((f) => f.endsWith(".tsx"))
    .map((f) => [`${d.split("/").slice(-1)[0]}/${f}`, stripComments(readFileSync(join(d, f), "utf8"))] as const);

describe("settings panel geometry", () => {
  it("has no call site overriding the primitive control geometry", () => {
    const offenders: string[] = [];
    for (const dir of DIRS) {
      for (const [name, src] of filesIn(dir)) {
        for (const m of src.matchAll(BANNED)) offenders.push(`${name}: ${m[0]}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  /**
   * A control's width should come from the panel, not from a number typed at the call site.
   * `w-[352px]` would satisfy the ban above while recreating the defect it exists to prevent —
   * nineteen panels each asserting their own idea of the width.
   */
  /**
   * A typed width is a DECISION, and every one left has to be an argued one.
   *
   * The threshold used to be 100px, on the grounds that anything smaller was a spacer. That let
   * `w-[4.5rem]` — 72px, on the series display select — sit unexamined, so the rule is now: no
   * typed width at all, except the two named here. Both were measured and both have a reason on
   * the line itself; a third has to earn its place in this list, which is the point.
   *
   * Everything else that renders at an odd width does so through `flex-1`, which is one rule
   * producing different pixels per row rather than several decisions. Counting rendered widths
   * conflates the two — that is a limitation of the sweep, not of the panel.
   */
  const ALLOWED = new Set([
    "w-[18px]",   // SegmentRangesSection: a spacer aligning a row against a sibling control
    "w-[4.5rem]", // SeriesListSection: the display select, which decides the row's alignment
  ]);

  /**
   * A PADDED ICON BUTTON MUST DECLARE ITS BOX.
   *
   * `rounded p-0.5` around an `h-3.5 w-3.5` icon renders 18×18: under the 24px floor every other
   * icon control in this panel sits at, and beside a 40px select. Three components had it — the
   * `Remove …` on a dimension slot, the move/delete trio on a formatting rule, and `SortRows`.
   *
   * NEITHER EXISTING RULE COULD SEE IT. The ban above catches classes that NAME a small size
   * (`h-7`, `h-8`); padding names no size at all. And the live sweep missed two of the three
   * because they only render once the user has added a rule or a sort, and every parity chart was
   * measured with none — so this was found on the deployed cluster, on the one chart that happened
   * to have a filled dimension slot.
   *
   * The rule is not "no padding". It is that an INTERACTIVE element choosing `p-0.5`/`p-1` must
   * also choose a `size-*`. `ColorPicker`'s native input passes on that basis — `size-6 … p-0.5`
   * is padding INSIDE a declared 24px box, which is the shape this is asking for.
   *
   * Interactivity is read off `hover:`, and that qualifier is doing real work rather than making
   * the regex tidy: without it the rule flagged `CurrencySelect`'s popover body and `RuleEditor`'s
   * scroll area, which are CONTAINERS insetting their children. Padding on those is not a size
   * decision and there is nothing there to be 18px.
   */
  const PADDED = /(?<![\w-])p-(?:0\.5|1)(?![\w-])/;
  const INTERACTIVE = /hover:/;
  const DECLARES_BOX = /(?<![\w-])(?:size-\d|h-\d)/;

  it("declares a box on every padded icon button", () => {
    const offenders: string[] = [];
    // `SortRows.tsx` sits in the chart root rather than in either scanned directory, but it renders
    // INTO this panel — it is the Sort section of the Data tab — and it carried one of the three.
    const extra: ReadonlyArray<readonly [string, string]> = [
      ["SortRows.tsx", stripComments(readFileSync(join(__dirname, "..", "..", "SortRows.tsx"), "utf8"))],
    ];
    for (const [name, src] of [...DIRS.flatMap(filesIn), ...extra]) {
      // Every string literal in the file, which is where a class list lives whether it is written
      // inline on `className` or hoisted to a constant — both shapes existed among the three.
      for (const m of src.matchAll(/"([^"\n]*)"|`([^`]*)`/g)) {
        const cls = m[1] ?? m[2] ?? "";
        if (PADDED.test(cls) && INTERACTIVE.test(cls) && !DECLARES_BOX.test(cls)) {
          offenders.push(`${name}: ${cls.slice(0, 60)}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("types no width that is not an argued exception", () => {
    const offenders: string[] = [];
    for (const dir of DIRS) {
      for (const [name, src] of filesIn(dir)) {
        // Numeric LENGTHS only. `w-[--radix-popover-trigger-width]` is an arbitrary value too,
        // but it defers to the trigger rather than deciding anything — it is the opposite of a
        // typed width and banning it would be banning the fix.
        for (const m of src.matchAll(/w-\[[\d.]+(?:px|rem|em|%)\]/g)) {
          if (!ALLOWED.has(m[0])) offenders.push(`${name}: ${m[0]}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

});
