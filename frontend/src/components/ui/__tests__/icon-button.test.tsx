import { render, screen } from "@testing-library/react";
import { IconButton } from "@/components/ui/icon-button";

/**
 * `IconButton` used to set `disabled:pointer-events-none`, and on THIS component
 * that was the worst place for it: an icon-only control has no text, so a `title`
 * is the only thing that can say why it is unavailable — and `pointer-events:
 * none` takes the element out of hit-testing, which is what decides whether the
 * browser opens that tooltip at all.
 *
 * Same caveat as `button.test.tsx`: this is a class assertion, and jsdom has no
 * layout or hit-testing to check the real thing with
 * (`docs/en/ui-conventions.md` §10). It exists so the class cannot come back
 * silently. `disabled` — which is what actually blocks the click — is asserted
 * beside it so this can never read as "the button became clickable".
 */
describe("IconButton — disabled, but still pointable", () => {
  test("keeps hit-testing on, and blocks activation with the attribute instead", () => {
    render(<IconButton disabled title="why not" aria-label="Remove" />);
    const btn = screen.getByRole("button", { name: "Remove" });

    expect([...btn.classList]).not.toContain("disabled:pointer-events-none");
    expect([...btn.classList]).not.toContain("pointer-events-none");
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute("title", "why not");
    // Unlike `Button`, this component knows its own resting paint (it declares
    // `text-muted-foreground` and no background right here), so it CAN restore it
    // under `disabled:hover:` exactly, rather than guessing at a caller's.
    expect([...btn.classList]).toContain("disabled:hover:bg-transparent");
    expect([...btn.classList]).toContain("disabled:hover:text-muted-foreground");
  });
});
