import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";

/**
 * NOTHING PAINTS ITS OWN CHECKBOX, because two of them cannot be fixed once.
 *
 * ── What this pins ──────────────────────────────────────────────────────────
 * #443 fixed the `Checkbox` primitive twice over: `rounded-[4px]` so a 16px box
 * stops reading as a radio (6px of a possible 8 left almost no straight edge),
 * and a transparent `before:-inset-1` so the pointer target is 24×24 while the
 * painted box stays 16px (WCAG 2.2 SC 2.5.8). Both fixes live on
 * `CheckboxPrimitive.Root`. A `<input type="checkbox">` elsewhere in the tree
 * gets NEITHER — it renders the platform control at whatever radius the UA and
 * the local class string produce, and its hit area is exactly its box.
 *
 * Seven such inputs survived #443 in five files. `ControlPlaneStorage`'s showed
 * why the class string is no defence: it read `h-4 w-4 rounded border-border
 * accent-primary`, and measured in the browser against `main`'s bundle it computed
 * to `appearance: auto`, `border-width: 0px`, `border-radius: 0px`. Nothing reset
 * `appearance`, so the UA painted the control and three of those four classes
 * produced no pixels at all. What shipped was the PLATFORM's checkbox — its shape,
 * its tick, its focus ring, and a different one per operating system.
 *
 * ── Why a source sweep and not a render assertion ────────────────────────────
 * A render test can only prove the sites it knows to render. The failure mode
 * here is a NEW call site written the old way — nobody would think to add it to
 * a list of components to check. The set of files that may paint a checkbox is
 * the whole of `src`, so the whole of `src` is what gets read.
 *
 * The allow-list is the primitive itself and nothing else. If a genuine reason
 * to hand-roll one ever appears, this list is where the argument for it goes.
 */

const SRC = path.join(__dirname, "..", "..", "..");

/** The only file allowed to name the raw control — and it names it in prose, not markup. */
const ALLOWED = new Set<string>([]);

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      // `generated/` is orval output and contains no JSX at all; skipping it keeps
      // the sweep honest about what a human could have written.
      if (entry === "generated") continue;
      walk(full, out);
    } else if (/\.tsx?$/.test(entry)) {
      out.push(full);
    }
  }
  return out;
}

/**
 * Comments come out before the scan. This file, and every fixed call site, explains
 * the old markup in prose; without stripping, the explanation itself would keep the
 * test red forever — and Tailwind v4's comment-scanning makes the same class of
 * mistake worse elsewhere, so the habit is worth keeping uniform.
 */
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1 ");
}

describe("every checkbox in the app is the primitive", () => {
  test("no source file renders a raw <input type=\"checkbox\">", () => {
    const offenders: string[] = [];
    for (const file of walk(SRC)) {
      const rel = path.relative(SRC, file);
      if (ALLOWED.has(rel)) continue;
      // Tests may render a raw input to prove something ABOUT one; only app code is swept.
      if (rel.includes("__tests__")) continue;
      const src = stripComments(readFileSync(file, "utf8"));
      // Attributes can be in any order and `type` can sit on its own line, so the
      // match is anchored on the opening tag and bounded by its own close.
      const re = /<input\b[^>]*type\s*=\s*["'{]?\s*checkbox/gs;
      const hits = src.match(re);
      if (hits) offenders.push(`${rel} (${hits.length})`);
    }
    expect(
      offenders,
      "these files paint their own checkbox and so get neither of #443's fixes — " +
        "not the 4px radius that tells a checkbox from a radio, and not the 24×24 " +
        "pointer target SC 2.5.8 asks for:\n  " + offenders.join("\n  "),
    ).toEqual([]);
  });
});
