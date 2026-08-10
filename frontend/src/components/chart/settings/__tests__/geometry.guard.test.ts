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
const BANNED = /\b(h-7|h-8|w-16|w-24|w-28)\b/g;
const DIR = join(__dirname, "..");

describe("settings panel geometry", () => {
  it("has no call site overriding the primitive control geometry", () => {
    const offenders: string[] = [];
    for (const f of readdirSync(DIR).filter((f) => f.endsWith(".tsx"))) {
      const src = readFileSync(join(DIR, f), "utf8");
      for (const m of src.matchAll(BANNED)) offenders.push(`${f}: ${m[0]}`);
    }
    expect(offenders).toEqual([]);
  });

  /**
   * A control's width should come from the panel, not from a number typed at the call site.
   * `w-[352px]` would satisfy the ban above while recreating the defect it exists to prevent —
   * nineteen panels each asserting their own idea of the width.
   */
  it("has no call site hard-coding the panel's content width", () => {
    const offenders: string[] = [];
    for (const f of readdirSync(DIR).filter((f) => f.endsWith(".tsx"))) {
      const src = readFileSync(join(DIR, f), "utf8");
      for (const m of src.matchAll(/w-\[\d+px\]/g)) offenders.push(`${f}: ${m[0]}`);
    }
    expect(offenders).toEqual([]);
  });
});
