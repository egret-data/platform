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
