import { expect } from "vitest";

/**
 * The one guard that says what an endpoint-detail SECTION does to its own
 * width, written once instead of once per section.
 *
 * # Why it has to exist separately from the route guard
 *
 * `EndpointDetail.test.tsx`'s route guard mounts a **sentinel** as the nested
 * route and walks up from it, so every element it sees belongs to the route.
 * It is scoped to the route by design, and that is a hole exactly the shape of
 * the defect the width pass removed: re-applying `max-w-3xl` to `Config`'s OWN
 * root passed the sentinel guard and all 230 other Config tests. So each
 * section needs its own assertion.
 *
 * # There is only one form left, because there is only one rule left
 *
 * This module used to export a matching pair — `expectSectionCapsItsOwnWidth`
 * and `expectSectionLeavesWidthToThePage` — because `docs/en/ui-conventions.md`
 * §5.1 sorted the six tabs into two groups: `Config`, `Query` and `Response`
 * used the page; `Access`, `Try it` and `Versions` capped themselves at
 * `max-w-5xl`.
 *
 * **§5.1 was reversed and the positive form was deleted with it.** All six
 * sections leave width to the page now, so the capping helper had zero callers.
 * It was deleted rather than kept unused: an exported helper named "caps its
 * own width" is an executable statement that some section is supposed to, and
 * a reader finding it would reasonably conclude the two-group rule still
 * stands. That is the same failure §5.1 itself is being fixed for — a
 * description outliving the thing it described.
 *
 * One thing from it is worth keeping, for whoever reinstates a section cap:
 * **its regex was deliberately unprefixed** (`/(?:^|\s)max-w-/`), the opposite
 * of the surviving one, because a section capped only at `lg:` is uncapped on
 * exactly the narrow viewports nobody measured. Reinstating it as a copy of
 * the negative form's regex would be the bug.
 *
 * # What it can and cannot say
 *
 * A class name, not a layout: jsdom implements no CSS (§10), so whether the
 * width is well spent is a browser question. This exists only so a cap cannot
 * come back silently — and `tests/e2e/data-api-detail-tab-widths.spec.ts` is
 * where the claim that the six tabs actually AGREE is measured, because this
 * file structurally cannot see it.
 */

/**
 * The element the route renders the section into — resolved rather than
 * assumed, so the walk below keeps finding the section root whether or not a
 * given harness mounts the real shell.
 *
 * Under the real `EndpointDetail` that is the `<fieldset>` (disabled only for a
 * trashed endpoint) inside the route's `.p-4` root. Under a harness that stubs
 * the context and renders the section under a bare `Outlet` (`TryIt.test.tsx`),
 * it is the render container itself.
 */
function hostOf(container: HTMLElement): HTMLElement {
  const routeRoot = container.querySelector<HTMLElement>(".p-4");
  if (!routeRoot) return container;
  return routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;
}

/**
 * The section's own root: the ancestor of its `h2` that the host renders
 * directly.
 *
 * Walked, never counted with `parentElement.parentElement`: an extra wrapper
 * added inside the section must break the walk rather than quietly re-point the
 * assertion one level up — which would mean asserting about the fieldset, an
 * element that satisfies "no cap" no matter what the section does to itself.
 */
export function sectionRootOf(container: HTMLElement, heading: HTMLElement): HTMLElement {
  const host = hostOf(container);
  let root = heading.parentElement;
  while (root && root.parentElement !== host) root = root.parentElement;
  expect(root).not.toBeNull();
  // The walk overshooting the section and landing on the host itself is the
  // failure mode, and it is SILENT: the fieldset carries no `max-w-*`, so the
  // assertion passes on an element that is not the section.
  expect(root!.tagName).not.toBe("FIELDSET");
  return root!;
}

/**
 * The section leaves the width to the page — the one rule, for all six.
 *
 * Responsive prefixes are included on purpose: `lg:max-w-3xl` would waste the
 * page on exactly the wide viewports the width pass measured, and would slip
 * past an unprefixed pattern.
 */
export function expectSectionLeavesWidthToThePage(container: HTMLElement, heading: HTMLElement) {
  expect(sectionRootOf(container, heading).className).not.toMatch(
    /(?:^|\s)(?:[a-z0-9]+:)*max-w-/,
  );
}
