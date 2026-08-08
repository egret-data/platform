import { readFileSync } from "node:fs";
import path from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import { Checkbox } from "@/components/ui/checkbox";

/**
 * SHAPE IS HOW A USER TELLS "CHOOSE ONE" FROM "CHOOSE MANY".
 *
 * ── The defect this pins ─────────────────────────────────────────────────────
 * The checkbox carried `rounded-sm`. In this codebase `@theme inline` defines
 * `--radius-sm: calc(var(--radius) - 4px)` over `--radius: 0.625rem`, i.e. 6px —
 * and Tailwind SUBSTITUTES that calc into the utility, so it never appears as a
 * custom property at runtime and `getComputedStyle(:root)` returns "" for it.
 * On the 16px box that is 6 of a possible 8px: 75% of a full circle. Measured on
 * the kind deployment, Workspace settings → Roles & permissions, "Hide empty
 * roles": `16×16, border-radius: 6px`. Next to a radio (`rounded-full`, 8px) the
 * two controls are all but indistinguishable, and egret uses both — a checkbox
 * that reads as a radio tells the user the selection is exclusive when it is not.
 *
 * ── Why the token was not changed ────────────────────────────────────────────
 * `--radius-sm` is read by tabs, dropdown-menu items, select items, filter pills,
 * placeholder cards and a dozen call sites besides. 6px is right for a 32px pill
 * and wrong for a 16px box; the number is not the bug, applying a fixed radius to
 * a control an eighth of the size is. So the fix is local to the primitive and
 * the token is untouched — a global change would be a different, larger unit.
 *
 * ── Why the ratio, not the pixel value ───────────────────────────────────────
 * Asserting `rounded-[4px]` would pass just as happily if someone later doubled
 * the box to 32px, when 4px would be too tight, or if `--radius` moved and took
 * `rounded-sm` with it. What has to hold is the RELATIONSHIP: the corner radius
 * of a checkbox stays a small fraction of its size, and the radio's stays a full
 * half. So both are resolved to pixels here and compared to their own box.
 */

const UI = path.join(__dirname, "..");
const read = (f: string) => readFileSync(path.join(UI, f), "utf8");

/**
 * Comments are stripped before anything is parsed. This file, and the primitives
 * themselves, both name the utilities under assertion in prose; without stripping,
 * deleting `rounded-[4px]` from the element would leave the test green because the
 * words survive in the comment beside it. Tailwind v4 makes that worse than a
 * stale assertion — it scans comments for class candidates, so the removed rule is
 * still emitted into the shipped CSS and greps exactly like the fix landing.
 */
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1 ");
}

const ROOT_CSS = readFileSync(
  path.join(UI, "..", "..", "index.css"),
  "utf8",
);

/** `--radius` in px, read from index.css rather than assumed. */
function baseRadiusPx(): number {
  const m = stripComments(ROOT_CSS).match(/--radius:\s*([\d.]+)rem/);
  expect(m, "--radius not found in index.css").not.toBeNull();
  return parseFloat(m![1]) * 16;
}

/**
 * Resolve a Tailwind border-radius utility to pixels.
 *
 * `sm`/`md`/`lg`/`xl` are the values this project overrides in its `@theme inline`
 * block; `xs` and `full` are Tailwind v4 defaults the project does not override,
 * so they are taken from the framework's own scale. An arbitrary value in square
 * brackets is read literally. Anything unrecognised throws rather than defaulting,
 * because a silent 0 would make the assertion below trivially true.
 */
function radiusPx(util: string): number {
  const arbitrary = util.match(/^rounded-\[([\d.]+)(px|rem)\]$/);
  if (arbitrary) {
    return arbitrary[2] === "rem" ? parseFloat(arbitrary[1]) * 16 : parseFloat(arbitrary[1]);
  }
  const base = baseRadiusPx();
  const scale: Record<string, number> = {
    "rounded-none": 0,
    "rounded-xs": 2, // Tailwind v4 default --radius-xs, not overridden here
    "rounded-sm": base - 4,
    "rounded-md": base - 2,
    "rounded-lg": base,
    "rounded-xl": base + 4,
    "rounded-full": 9999,
  };
  if (!(util in scale)) throw new Error(`unresolvable radius utility: ${util}`);
  return scale[util];
}

/** The class string of a primitive's root element, comments removed. */
function rootClasses(file: string, marker: string): string {
  const src = stripComments(read(file));
  const start = src.indexOf(marker);
  expect(start, `${marker} not found in ${file}`).toBeGreaterThan(-1);
  const end = src.indexOf("{...props}", start);
  return src.slice(start, end === -1 ? undefined : end);
}

/** Size in px from an `h-N`/`size-N` utility (Tailwind's 0.25rem step). */
function boxSizePx(classes: string): number {
  const m = classes.match(/\b(?:h|size)-(\d+(?:\.\d+)?)\b/);
  expect(m, "no height utility on the primitive root").not.toBeNull();
  return parseFloat(m![1]) * 4;
}

function radiusUtil(classes: string): string {
  // No trailing `\b` on the bracket branch: `]` is a non-word character and so is
  // the `"` that follows it, so there is no boundary between them and the match
  // silently fails — which is how this helper first reported "no radius utility"
  // on a primitive that plainly had one.
  const m = classes.match(/\brounded-(?:\[[^\]]+\]|[a-z]+\b)/);
  expect(m, "no border-radius utility on the primitive root").not.toBeNull();
  return m![0];
}

describe("checkbox and radio are told apart by shape", () => {
  test("the checkbox reads as a box, not a circle", () => {
    const classes = rootClasses("checkbox.tsx", "<CheckboxPrimitive.Root");
    const size = boxSizePx(classes);
    const radius = radiusPx(radiusUtil(classes));
    const fraction = radius / size;
    // A square's corner radius maxes out at half its side; at 3/8 of the side
    // (the measured 6px on 16px) the straight edges are gone and it reads round.
    // A quarter is the bar shadcn's own checkbox sits on: 4px on 16px.
    expect(
      fraction,
      `checkbox corner radius is ${radius}px on a ${size}px box (${Math.round(
        fraction * 100,
      )}% of its side) — at that ratio it reads as a radio button`,
    ).toBeLessThanOrEqual(0.25);
  });

  test("the radio stays a circle", () => {
    const classes = rootClasses("radio-group.tsx", "<RadioGroupPrimitive.Item");
    const size = boxSizePx(classes);
    // The contrast is the point: if the radio ever stopped being fully round the
    // two controls would converge from the other direction.
    expect(radiusPx(radiusUtil(classes))).toBeGreaterThanOrEqual(size / 2);
  });
});

describe("checkbox target size (WCAG 2.2 SC 2.5.8)", () => {
  test("offers a 24px pointer target without growing the painted box", () => {
    const classes = rootClasses("checkbox.tsx", "<CheckboxPrimitive.Root");
    const size = boxSizePx(classes);
    // 2.5.8 (AA) asks 24×24 CSS px. The painted box has to stay 16px — it is
    // aligned against 20px text lines all over the app and growing it would
    // reflow every list — so the target is enlarged with a transparent
    // pseudo-element instead, which is invisible and costs no layout.
    const inset = classes.match(/before:-inset-(\d+(?:\.\d+)?)/);
    expect(
      inset,
      "no hit-area pseudo-element on the checkbox: a bare 16×16 control fails " +
        "SC 2.5.8 wherever the row around it is not itself the target",
    ).not.toBeNull();
    const target = size + parseFloat(inset![1]) * 4 * 2;
    expect(target, `pointer target is ${target}px`).toBeGreaterThanOrEqual(24);
    expect(classes).toMatch(/\brelative\b/); // the inset needs a positioned root
  });
});

describe("the indeterminate state has its own mark", () => {
  test("draws a dash, not a tick", () => {
    render(<Checkbox checked="indeterminate" aria-label="partial" />);
    const box = screen.getByRole("checkbox", { name: "partial" });
    expect(box.getAttribute("data-state")).toBe("indeterminate");
    // lucide sets the icon name as a class; a tick here would claim every item
    // in the list is selected when only some are.
    expect(box.querySelector(".lucide-minus")).not.toBeNull();
    expect(box.querySelector(".lucide-check")).toBeNull();
  });

  test("draws a tick when fully checked", () => {
    render(<Checkbox checked aria-label="all" />);
    const box = screen.getByRole("checkbox", { name: "all" });
    expect(box.querySelector(".lucide-check")).not.toBeNull();
    expect(box.querySelector(".lucide-minus")).toBeNull();
  });
});
