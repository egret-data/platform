import { expect } from "vitest";

/**
 * The one guard that says what an endpoint-detail SECTION does to its own
 * width, written once instead of once per section.
 *
 * # Why it has to exist separately from the route guard
 *
 * `EndpointDetail.test.tsx`'s route guard mounts a **sentinel** as the nested
 * route and walks up from it, so every element it sees belongs to the route.
 * That is deliberate — it is what lets one section cap itself while another
 * does not — and it is also a hole exactly the shape of the defect the width
 * pass removed: re-applying `max-w-3xl` to `Config`'s OWN root passed the
 * sentinel guard and all 230 other Config tests. So each section needs its own
 * assertion, and `docs/en/ui-conventions.md` §5.1 records which way round it
 * goes for each: `Config`, `Query` and `Response` use the page, `Access`,
 * `Try it` and `Versions` cap themselves at `max-w-5xl`. Response moved groups
 * in §5.4b, once it held a settings table and a code editor — the criterion is
 * what a section HOLDS, not which tab it is.
 *
 * Three sections had that assertion copy-pasted and two (`Try it`, `Access`)
 * had none at all — which is how `Try it` shipped a `max-w-5xl` nothing was
 * holding in place. One helper, five call sites.
 *
 * # What it can and cannot say
 *
 * A class name, not a layout: jsdom implements no CSS (§10), so whether the
 * width is well spent is a browser question. This exists only so a cap cannot
 * come back, or vanish, silently.
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
 * assertion one level up — which for the negative form would mean asserting
 * about the fieldset, an element that satisfies "no cap" no matter what the
 * section does to itself.
 */
export function sectionRootOf(container: HTMLElement, heading: HTMLElement): HTMLElement {
  const host = hostOf(container);
  let root = heading.parentElement;
  while (root && root.parentElement !== host) root = root.parentElement;
  expect(root).not.toBeNull();
  // The walk overshooting the section and landing on the host itself is the
  // failure mode both forms share, and it is silent in the negative form.
  expect(root!.tagName).not.toBe("FIELDSET");
  return root!;
}

/**
 * The section owns a width cap on its own root.
 *
 * Deliberately unprefixed: a section whose cap were only `lg:max-w-5xl` would
 * be uncapped on exactly the narrow viewports nobody measured.
 */
export function expectSectionCapsItsOwnWidth(container: HTMLElement, heading: HTMLElement) {
  expect(sectionRootOf(container, heading).className).toMatch(/(?:^|\s)max-w-/);
}

/**
 * The section leaves the width to the page.
 *
 * Responsive prefixes included here, the opposite way round: `lg:max-w-3xl`
 * would waste the page on exactly the wide viewports the width pass measured.
 */
export function expectSectionLeavesWidthToThePage(container: HTMLElement, heading: HTMLElement) {
  expect(sectionRootOf(container, heading).className).not.toMatch(
    /(?:^|\s)(?:[a-z0-9]+:)*max-w-/,
  );
}
