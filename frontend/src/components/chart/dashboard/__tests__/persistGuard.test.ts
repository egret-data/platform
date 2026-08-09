import { describe, it, expect } from "vitest";
import { mayPersistLayout } from "../grid-breakpoints";

describe("mayPersistLayout", () => {
  it("permits only editing on desktop", () => {
    expect(mayPersistLayout(true, "desktop")).toBe(true);
    // Both halves are load-bearing. `editing` alone is not enough: RGL emits an
    // onLayoutChange for the DERIVED one-column layout as soon as it mounts on a phone,
    // and persisting it would flatten every card to x:0 w:1 in the saved desktop layout.
    expect(mayPersistLayout(true, "mobile")).toBe(false);
    expect(mayPersistLayout(false, "desktop")).toBe(false);
    expect(mayPersistLayout(false, "mobile")).toBe(false);
  });
});
