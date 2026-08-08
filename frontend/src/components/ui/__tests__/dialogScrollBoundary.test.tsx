import { readFileSync } from "node:fs";
import path from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/**
 * A DIALOG THAT IS TALLER THAN THE VIEWPORT MUST STILL BE FULLY REACHABLE.
 *
 * ── The defect this pins ─────────────────────────────────────────────────────
 * `DialogContent` is a `position: fixed` box centred with `top: 50%` +
 * `translateY(-50%)`. With no maximum height it grows to its content and centres
 * the OVERFLOW along with it: half above the viewport and half below. Nothing
 * scrolls it back, because Radix sets `overflow: hidden` on `body` for as long as
 * a modal is open and the content itself was `overflow: visible`. Measured on the
 * kind deployment at 1280×640, on the "Add storage connection" dialog:
 *
 *     content   750px tall at top: -55  (bottom 695, viewport 640)
 *     close     top: -38, bottom: -22   → entirely above the viewport
 *     hit test  document.elementFromPoint(centre of close) === null
 *     scroll    scrollHeight === clientHeight (748 === 748) on content,
 *               body and html — there is nothing to scroll anywhere
 *
 * and `close.focus(); close.scrollIntoView()` left it at top: -38, so the close
 * button could not be reached by keyboard either. The dialog could only be
 * dismissed by guessing that Escape works.
 *
 * ── What this file can and cannot assert ─────────────────────────────────────
 * jsdom runs NO LAYOUT. Every number above is unmeasurable here: heights are 0,
 * `getBoundingClientRect()` is all zeroes, and `scrollHeight` never exceeds
 * `clientHeight`. So the geometry is verified in a real browser and recorded in
 * the PR; what is pinned here is the two things that jsdom CAN see and that the
 * geometry depends on:
 *
 *   1. STRUCTURE — the scroll boundary is an inner `dialog-body`, and the close
 *      button is a sibling of it rather than inside it. That is the whole reason
 *      the close button stays put while the body scrolls, and it is a DOM fact,
 *      not a layout one.
 *   2. THE CAP ITSELF — a viewport-relative maximum height on the content, read
 *      out of the source rather than out of a computed style (jsdom does not
 *      resolve Tailwind at all).
 *
 * ── Why the source is parsed for (2) ─────────────────────────────────────────
 * `toHaveClass` on the rendered node would pass just as well, but Tailwind v4
 * scans COMMENTS for class candidates, so a utility named in prose anywhere in
 * the repo is emitted into the shipped CSS whether or not it is on an element.
 * Reading the class off the source of the primitive, from the region that builds
 * the content's own class string, is the assertion that cannot be satisfied by
 * accident.
 */

const DIALOG_SRC = readFileSync(
  path.join(__dirname, "..", "dialog.tsx"),
  "utf8",
);

/**
 * Comments are stripped before the source is searched, for the reason above:
 * this very file, and the long comment inside `dialog.tsx`, both name the class
 * being asserted. Without stripping, deleting the class from the element would
 * leave the test green because the word still appears in the prose next to it.
 * That is not hypothetical — it is the failure mode `focusIndicatorContrast`
 * documents having been caught by mutation testing.
 */
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1 ");
}

/** The class string of the `DialogPrimitive.Content` element, comments removed. */
function contentClassRegion(): string {
  const src = stripComments(DIALOG_SRC);
  const start = src.indexOf("<DialogPrimitive.Content");
  expect(start, "DialogPrimitive.Content not found in dialog.tsx").toBeGreaterThan(-1);
  const end = src.indexOf(">", src.indexOf("{...props}", start));
  return src.slice(start, end);
}

describe("DialogContent height cap", () => {
  test("caps its height against the viewport", () => {
    const region = contentClassRegion();
    const maxH = region.match(/max-h-\[([^\]]+)\]/);
    expect(
      maxH,
      "DialogContent must carry a max-height; without one it grows past the " +
        "viewport and centres the overflow off-screen",
    ).not.toBeNull();
    // `dvh`, not `vh`: on mobile Safari `vh` is the LARGEST viewport — the one
    // behind the retracted browser chrome — so a `vh` cap still overflows by the
    // height of the toolbar. `%` would be wrong too: the content is `fixed`, so a
    // percentage resolves against the initial containing block, which is fine, but
    // it would not leave the margin that keeps the dialog off the viewport edges.
    expect(maxH![1], `max-height must be viewport-relative, got ${maxH?.[1]}`).toMatch(/dvh/);
  });

  test("clips at the content edge so the cap cannot be escaped", () => {
    // Without `overflow-hidden` the cap would bound the BOX while the children
    // still painted outside it — visually identical to the defect.
    expect(contentClassRegion()).toMatch(/\boverflow-hidden\b/);
  });
});

describe("DialogContent scroll boundary", () => {
  test("scrolls an inner body, not the whole content", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Tall dialog</DialogTitle>
          </DialogHeader>
          <p>body</p>
        </DialogContent>
      </Dialog>,
    );
    const content = document.querySelector('[data-slot="dialog-content"]');
    const body = document.querySelector('[data-slot="dialog-body"]');
    expect(content).not.toBeNull();
    expect(body, "DialogContent must wrap its children in a scrollable body").not.toBeNull();
    expect(content!.contains(body!)).toBe(true);
    expect(body!.className).toMatch(/overflow-y-auto/);
    // `min-h-0` is what actually lets a flex child shrink below its content
    // height. Without it the body refuses to get smaller than its children and
    // the overflow comes straight back out of the capped content.
    expect(body!.className).toMatch(/min-h-0/);
  });

  test("keeps the close button out of the scrolling region", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Tall dialog</DialogTitle>
          </DialogHeader>
          <p>body</p>
        </DialogContent>
      </Dialog>,
    );
    const content = document.querySelector('[data-slot="dialog-content"]')!;
    const body = document.querySelector('[data-slot="dialog-body"]')!;
    const close = screen.getByRole("button", { name: "Close" });
    expect(content.contains(close)).toBe(true);
    expect(
      body.contains(close),
      "the close button must not scroll away with the body — that is the " +
        "control the user needs precisely when the dialog is too tall",
    ).toBe(false);
    // It also has to stay inside Content: Radix traps focus within Content, so a
    // close button hoisted out to the portal would be untabbable.
    expect(close.closest('[data-slot="dialog-content"]')).toBe(content);
  });

  test("puts the children's own gap on the body, where the children now live", () => {
    render(
      <Dialog open>
        <DialogContent>
          <DialogTitle>t</DialogTitle>
        </DialogContent>
      </Dialog>,
    );
    // The gap used to sit on Content, whose only child is now the body — leaving
    // it there would silently collapse the spacing of all 65 dialogs.
    expect(document.querySelector('[data-slot="dialog-body"]')!.className).toMatch(/gap-4/);
  });
});
