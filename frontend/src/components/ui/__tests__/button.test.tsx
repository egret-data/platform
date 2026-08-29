import { readFileSync } from "node:fs";
import path from "node:path";

import { render, screen } from "@testing-library/react";
import { Button } from "@/components/ui/button";

test("renders a button with its label", () => {
  render(<Button>Click me</Button>);
  expect(screen.getByRole("button", { name: "Click me" })).toBeInTheDocument();
});

/**
 * `Button` no longer sets `disabled:pointer-events-none` — it hid the `title`
 * that says why a disabled button is disabled, on sixteen buttons in the tree.
 * The evidence for THAT is a browser measurement, not a class name, and it lives
 * in `tests/e2e/disabled-control-affordances.spec.ts`; jsdom has no layout and no
 * hit-testing, so nothing here can tell "reachable" from "painted over"
 * (`docs/en/ui-conventions.md` §10, and §5.5 on what a class assertion is worth).
 *
 * What jsdom CAN settle is the branch that has no live example to measure.
 * `asChild` renders through Radix `Slot` onto whatever child is passed — an
 * `<a>`, a `<div>` — and `disabled` is an attribute of `<button>` and nothing
 * else, so on that path it is inert: the element stays hit-testable, stays in the
 * tab order, and its click still fires. `pointer-events-none` was the only thing
 * holding that case shut. There are currently zero `<Button asChild disabled>` in
 * `src/`, so removing the class broke nothing — it armed a trap for whoever
 * writes the first one. These pin the replacement guard.
 *
 * These are class/attribute assertions and are weak on purpose: they say the
 * guard is applied, not that a browser refuses the click. Their value is that
 * they fail if the guard is deleted, which is exactly how the class it replaces
 * came to be deleted safely. Each was checked by mutation — see the notes in the
 * report — rather than assumed to be load-bearing.
 */
describe("Button — a disabled Slot cannot be left interactive", () => {
  test("a real <button> is left hit-testable, so its title can be read", () => {
    render(
      <Button disabled title="why not">
        Save
      </Button>,
    );
    const btn = screen.getByRole("button", { name: "Save" });
    // The class that hid the tooltip must not come back. Asserted on `classList`
    // tokens, not a substring: the base legitimately keeps
    // `[&_svg]:pointer-events-none` so that an icon inside a button never becomes
    // the hit target instead of the button, and a substring match cannot tell the
    // two apart. (It didn't — the first version of this assertion failed on it.)
    // `disabled` itself is what blocks activation and is asserted alongside, so
    // this cannot be mistaken for "the button became clickable".
    expect([...btn.classList]).not.toContain("disabled:pointer-events-none");
    expect([...btn.classList]).not.toContain("pointer-events-none");
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute("title", "why not");
  });

  test("asChild + disabled renders a non-interactive Slot instead", () => {
    render(
      <Button asChild disabled>
        <a href="/nowhere">Go</a>
      </Button>,
    );
    const link = screen.getByText("Go");
    expect(link.tagName).toBe("A");
    // `disabled` does nothing to an <a>, so the guard has to be explicit.
    expect([...link.classList]).toContain("pointer-events-none");
    expect(link).toHaveAttribute("aria-disabled", "true");
    expect(link).toHaveAttribute("tabindex", "-1");
  });

  test("asChild WITHOUT disabled stays fully interactive", () => {
    render(
      <Button asChild>
        <a href="/somewhere">Go</a>
      </Button>,
    );
    const link = screen.getByRole("link", { name: "Go" });
    // The inverse assertion: without it, a guard that fired unconditionally
    // would pass the test above and quietly deaden all 22 `asChild` call sites.
    expect([...link.classList]).not.toContain("pointer-events-none");
    expect(link).not.toHaveAttribute("aria-disabled");
    expect(link).not.toHaveAttribute("tabindex");
  });
});

/**
 * ── A DISABLED BUTTON MUST LOOK DISABLED IN BOTH THEMES ──────────────────────
 *
 * Disabled used to be encoded by ALPHA ALONE: `disabled:opacity-50` in the base,
 * nothing else. Measured with `getComputedStyle` on the endpoint-detail Save
 * button of the deployed build, in dark mode:
 *
 *     disabled = true
 *     opacity  = 0.5
 *     background = oklch(0.66 0.13 185)   // unchanged from enabled
 *     color      = oklch(0.1 0 0)         // unchanged
 *
 * Alpha is not a colour change, it is a blend with whatever is behind. On white
 * that blend washes the teal out to a pastel and reads unavailable; on the
 * near-black dark ground the same 50% leaves a saturated teal that reads READY.
 * So the identical class produced opposite meanings in the two themes, and in
 * dark a user could not see that Save does nothing.
 *
 * The fix changes the paint, not the alpha: a disabled button loses its fill
 * (`bg-secondary`), takes a token border (`inset-ring-border`, inset so the box
 * does not grow by 2px and jitter as the form goes dirty/clean), drops its
 * shadow, and takes muted label ink. Ghost and link have no fill to lose, so
 * they take the muted ink plus a modest alpha and keep their transparent ground.
 *
 * ── What these assertions are, and are NOT ───────────────────────────────────
 *
 * They are CLASS assertions. jsdom runs no layout, paints nothing and does not
 * resolve `oklch()`, so nothing in this file can measure a contrast ratio — a
 * jsdom "contrast test" would pass with the fix reverted (`docs/en/ui-conventions.md`
 * §10). The ratios are measured in real Chrome; they live in
 * `tests/e2e/disabled-button-paint.spec.ts` and in the task report.
 *
 * What these DO settle is that the treatment is wired to every variant and is
 * not alpha-only, which is the part that got lost. Each was mutation-checked.
 *
 * Every render below is deliberately OUTSIDE any `<fieldset>`: `toBeDisabled()`
 * walks ancestors and is satisfied by a `fieldset[disabled]`, so a guard test can
 * pass with the guard deleted. The absence is asserted first, before the state.
 */
describe("Button — disabled is a paint change, not an alpha change", () => {
  const classesOf = (name: string) => {
    const btn = screen.getByRole("button", { name });
    // Assert the absence FIRST: nothing but this element's own `disabled`
    // attribute may be what makes `toBeDisabled()` true.
    expect(btn.closest("fieldset")).toBeNull();
    expect(btn).toBeDisabled();
    return [...btn.classList];
  };

  // ONLY the variants whose enabled fill is saturated. `outline` and `secondary`
  // were in this list for one round and it was a net regression on 298 call
  // sites — see the block below, which is the guard against putting them back.
  test.each(["default", "destructive", "ai"] as const)(
    "the %s variant loses its saturated fill and takes a token border when disabled",
    (variant) => {
      render(
        <Button variant={variant} disabled>
          Save
        </Button>,
      );
      const classes = classesOf("Save");

      expect(classes).toContain("disabled:bg-secondary");
      expect(classes).toContain("disabled:text-muted-foreground");
      expect(classes).toContain("disabled:inset-ring-1");
      expect(classes).toContain("disabled:inset-ring-border");
      expect(classes).toContain("disabled:shadow-none");

      // A disabled button is still hit-testable here on purpose (that is what
      // keeps its `title` tooltip reachable), so the variant's `hover:bg-*`
      // still matches it. This pins the belt-and-braces twin, and it is
      // labelled as such: mutation testing showed that DELETING it changes
      // nothing a browser can see, because `disabled:bg-secondary` already
      // outranks `hover:bg-primary/90` on emission order at equal specificity.
      // The behaviour it guards is asserted in real Chrome
      // (`tests/e2e/disabled-button-paint.spec.ts`); what fails there is the
      // loss of the fill, not the loss of this twin.
      expect(classes).toContain("disabled:hover:bg-secondary");

      // And the defect itself: alpha is no longer carrying the state.
      expect(classes.filter((c) => c.startsWith("disabled:opacity-"))).toEqual([]);
      expect(classes).not.toContain("disabled:opacity-50");
    },
  );

  // The variants whose enabled fill is NEUTRAL or absent. `opacity-50` was never
  // broken for these — alpha only preserves a hue when there is a hue to
  // preserve — so they keep it, and the numbers stay identical to it by
  // construction. Giving them the filled treatment measured WORSE than
  // `opacity-50` on the enabled-vs-disabled ink (light 5.08 -> 2.85, dark
  // 3.36 -> 2.51) across 238 `outline` + 60 `secondary` call sites, which is the
  // same defect this component exists to remove, moved to a different variant.
  test.each(["outline", "secondary", "ghost"] as const)(
    "the %s variant keeps the alpha cue instead of gaining a box",
    (variant) => {
      render(
        <Button variant={variant} disabled>
          Save
        </Button>,
      );
      const classes = classesOf("Save");

      expect(classes).toContain("disabled:opacity-50");
      expect(classes).toContain("aria-disabled:opacity-50");
      // A second reason for `ghost` specifically: in dark mode `--secondary` and
      // `--accent` are the same value, and `--accent` is what it paints on
      // HOVER, so a filled disabled ghost would be pixel-identical to a hovered
      // one. A disabled control that looks hovered is worse than the bug.
      expect(classes).not.toContain("disabled:bg-secondary");
      expect(classes).not.toContain("disabled:inset-ring-1");
      // The one thing that IS added for them: the hover paint is neutralised
      // back to their own resting value, which the component knows exactly.
      expect(classes.some((c) => c.startsWith("disabled:hover:bg-"))).toBe(true);
    },
  );

  test("link degrades to plain text, because fading it moves it TOWARDS the enabled colour", () => {
    render(
      <Button variant="link" disabled>
        Save
      </Button>,
    );
    const classes = classesOf("Save");

    // `--primary` sits mid-luminance in both themes, so `--muted-foreground` is
    // 1.11:1 from it in light — a hue difference at equal lightness, i.e. WCAG
    // 1.4.1 (colour alone). And alpha cannot fix it: in dark, fading white ink
    // moves it towards the teal's luminance, not away. `--foreground` is the
    // only token far from `--primary` in both themes at once.
    expect(classes).toContain("disabled:text-foreground");
    expect(classes).toContain("aria-disabled:text-foreground");
    expect(classes).not.toContain("disabled:text-muted-foreground");
    // ...and therefore no alpha, which would undo exactly that.
    expect(classes.filter((c) => c.startsWith("disabled:opacity-"))).toEqual([]);
    expect(classes).not.toContain("disabled:bg-secondary");
  });

  test("the gradient variant also drops its gradient, which a background-COLOR cannot cover", () => {
    render(
      <Button variant="ai" disabled>
        Save
      </Button>,
    );
    const classes = classesOf("Save");
    // `bg-secondary` sets `background-color`; the gradient is a
    // `background-image` and paints on top of it. Without `bg-none` the disabled
    // ai button stays purple→orange at full strength.
    expect(classes).toContain("disabled:bg-none");
    // Same shape one level down: `[&_svg]:text-white` colours the icon from a
    // rule of its own, so the button's `color` change never reaches it.
    // `text-current` rather than the token, so the icon follows whatever the
    // label ends up being — including the `/80` the dark theme applies.
    expect(classes).toContain("disabled:[&_svg]:text-current");
  });

  test("`aria-disabled` gets the same paint — it is the only signal the asChild path has", () => {
    // `disabled` is an attribute of <button> and nothing else, so the `asChild`
    // path conveys the state with `aria-disabled` instead. If only `disabled:*`
    // were painted, a disabled <Button asChild> would look fully enabled.
    render(
      <Button asChild disabled>
        <a href="/nowhere">Go</a>
      </Button>,
    );
    const link = screen.getByText("Go");
    expect(link.closest("fieldset")).toBeNull();
    const classes = [...link.classList];
    expect(link).toHaveAttribute("aria-disabled", "true");
    expect(classes).toContain("aria-disabled:bg-secondary");
    expect(classes).toContain("aria-disabled:text-muted-foreground");
    expect(classes).toContain("aria-disabled:inset-ring-1");
    expect(classes.filter((c) => c.startsWith("aria-disabled:opacity-"))).toEqual([]);
  });
});

/**
 * The trap this repo has already fallen into once: a token that is spelled
 * correctly, passes tsc, passes every render test, and resolves to NOTHING in
 * one of the two themes — because it was only ever defined in the other. Nothing
 * downstream can catch that; jsdom does not evaluate custom properties and
 * TypeScript never sees a class string. So it is caught here, at the source.
 *
 * This reads the two files as text rather than rendering anything: every colour
 * the disabled treatment names must exist in `:root` AND in `.dark`, and must be
 * mapped into `@theme inline` so Tailwind emits a utility for it at all.
 */
describe("Button — the disabled treatment names only tokens that exist in both themes", () => {
  const source = readFileSync(path.resolve(__dirname, "../button.tsx"), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const css = readFileSync(path.resolve(__dirname, "../../../index.css"), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const block = (selector: string) =>
    css.match(new RegExp(`(?:^|\\n)${selector}\\s*\\{([\\s\\S]*?)\\n\\}`))![1];
  const root = block(":root");
  const dark = block("\\.dark");
  const themeInline = block("@theme inline");

  // `disabled:bg-secondary`, `aria-disabled:hover:text-muted-foreground`,
  // `disabled:[&_svg]:text-muted-foreground` → the colour name at the end.
  const named = [
    ...source.matchAll(
      /(?:disabled|aria-disabled):[a-z0-9:&_[\]-]*?(?:bg|text|inset-ring|border)-([a-z][a-z-]*)/g,
    ),
  ].map((m) => m[1]);

  test("the treatment names at least one colour, so the loop below cannot be vacuously green", () => {
    expect(named.length).toBeGreaterThan(5);
  });

  test.each([...new Set(named)])("`%s` is a real token in light and dark", (name) => {
    // Keywords, not tokens: they need no declaration.
    if (["none", "transparent", "current", "inherit"].includes(name)) return;
    expect(themeInline).toContain(`--color-${name}: var(--${name})`);
    expect(root).toMatch(new RegExp(`--${name}:\\s*oklch`));
    expect(dark).toMatch(new RegExp(`--${name}:\\s*oklch`));
  });
});

/**
 * THE ONE THAT WAS ALREADY BROKEN WHEN THIS TEST WAS WRITTEN.
 *
 * `dark:bg-destructive/60` and `disabled:bg-secondary` have the SAME specificity
 * — `.dark .dark\:bg-destructive\/60` and `.disabled\:bg-secondary:disabled` are
 * both 0-2-0 — so neither wins on merit and the tie goes to whichever Tailwind
 * emits later. It emits `dark:` later. Measured in real Chrome on the first cut
 * of this change, the disabled destructive button in dark mode reported
 * `background-color: oklab(0.7 0.175878 0.0718819 / 0.6)`: its full enabled red,
 * with a muted label sitting on it. The variant that most needs to look
 * unavailable was the one variant that did not.
 *
 * A `dark:disabled:*` twin scores 0-3-0 and settles it. This test is the
 * invariant rather than the instance: any variant that paints a background under
 * `dark:` must also say what that background becomes when the control is
 * disabled, in both spellings of disabled. It fails on the code that shipped the
 * measurement above.
 */
describe("Button — a dark-mode fill must say what it becomes when disabled", () => {
  const source = readFileSync(path.resolve(__dirname, "../button.tsx"), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const variantBlock = source.match(/variants:\s*\{\s*variant:\s*\{([\s\S]*?)\n {6}\},/)![1];
  const entries = [...variantBlock.matchAll(/(\w+):\s*(?:`([^`]*)`|"([^"]*)")/g)].map((m) => ({
    name: m[1],
    classes: m[2] ?? m[3],
  }));

  test("the variant table was parsed, so the loop below is not vacuous", () => {
    expect(entries.map((e) => e.name).sort()).toEqual([
      "ai",
      "default",
      "destructive",
      "ghost",
      "link",
      "outline",
      "secondary",
    ]);
  });

  // A RESTING dark fill (`dark:bg-*`, not `dark:hover:bg-*`) is the one that
  // silently survives into the disabled state — but only for a variant that
  // REPLACES its fill when disabled. A variant that keeps its fill and dims the
  // whole element (`outline`) has nothing to override and is correctly exempt.
  // `${disabledFilled}` is the marker for "this variant replaces its fill when
  // disabled"; the treatment itself is interpolated, so the literal class names
  // are not in the variant's own source text.
  const restingDarkFill = entries.filter(
    (e) => /(?:^|\s)dark:bg-/.test(e.classes) && e.classes.includes("${disabledFilled}"),
  );
  const hoverDarkFill = entries.filter((e) => /dark:hover:bg-/.test(e.classes));

  test("both lists below are non-empty, so neither `each` is vacuously green", () => {
    expect(restingDarkFill.length).toBeGreaterThan(0);
    expect(hoverDarkFill.length).toBeGreaterThan(0);
  });

  test.each(restingDarkFill.map((e) => e.name))(
    "`%s` paints a background under dark: and says what it becomes under dark:disabled:",
    (name) => {
      const { classes } = entries.find((e) => e.name === name)!;
      expect(classes).toMatch(/dark:disabled:bg-/);
      expect(classes).toMatch(/dark:aria-disabled:bg-/);
    },
  );

  test.each(hoverDarkFill.map((e) => e.name))(
    "`%s` paints a background under dark:hover: and overrides that too",
    (name) => {
      // The same belt-and-braces as in light mode, and measured to be exactly
      // that: with these four classes deleted, a real hover over a disabled
      // `outline` button in dark Chrome still reports
      // `oklch(0.27 0.01 286.1)`. They are pinned so the paint does not depend
      // on the order Tailwind happens to emit `dark:hover:` and `dark:disabled:`
      // in, which is all that decides it between two 0-3-0 selectors.
      const { classes } = entries.find((e) => e.name === name)!;
      expect(classes).toMatch(/dark:disabled:hover:bg-/);
      expect(classes).toMatch(/dark:aria-disabled:hover:bg-/);
    },
  );
});
