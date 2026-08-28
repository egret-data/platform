import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";

import { SettingsNote } from "@/routes/explore/settings/SettingsNote";

/**
 * The colour ruling, tested where the component is rather than inside one
 * consumer.
 *
 * It used to be guarded by a single assertion in `AccessSection.test.tsx`, on
 * that section's own page-level note. Task 3 merged that note into a `Caveat`
 * on the Invoke column, and the guard went with it — leaving five remaining
 * consumers (`MembersPanel`, `ResponseSection`, `Versions`, `Notifications`,
 * `RolesPermissions`, measured by import specifier per §7.2) and NOTHING in the
 * tree asserting the surface at all.
 *
 * Guarding a shared component's own styling from one consumer's test was the
 * wrong place to begin with: it dies whenever that consumer stops using it,
 * which is a change that says nothing about the component. This file is the
 * right place, and it is why the deletion is a net improvement rather than a
 * loss.
 *
 * **The rule it pins:** `--highlight` (amber) is reserved for a page's single
 * primary emphasis. A standing passive orientation note is not that, so
 * `SettingsNote` is deliberately neutral — the component's own doc comment says
 * so, and a doc comment is not a test.
 */
describe("SettingsNote", () => {
  it("renders on the neutral muted surface, never the amber highlight", () => {
    render(<SettingsNote>Publishing does not grant data access.</SettingsNote>);

    const note = screen.getByRole("note");
    expect(note).toHaveTextContent("Publishing does not grant data access.");
    expect(note.className).toContain("bg-muted/40");
    // The half that is the ruling. `toContain` on the positive class alone
    // would pass on a note carrying BOTH.
    expect(note.className).not.toContain("highlight");
  });

  it("is announced as a note, so it is not read as body copy", () => {
    // `role="note"` is the whole reason consumers can find it, and it is the
    // one attribute a "simplify the wrapper" change would drop silently.
    const { container } = render(<SettingsNote>Anything.</SettingsNote>);
    expect(container.querySelector('[role="note"]')).not.toBeNull();
  });

  it("marks its icon decorative, so the sentence is not prefixed by an image", () => {
    // An `Info` glyph with no `aria-hidden` lands in the note's accessible
    // text. There is nothing for a reader to do with it.
    const { container } = render(<SettingsNote>Anything.</SettingsNote>);
    const icon = container.querySelector("svg");
    expect(icon).not.toBeNull();
    expect(icon).toHaveAttribute("aria-hidden", "true");
  });
});
