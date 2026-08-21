import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

/**
 * The property under test is what happens when the lazy chunk FAILS.
 *
 * A rejected `lazy()` import throws during render. Without a boundary the whole
 * React tree unmounts — measured on kind by serving a 404 for
 * `assets/monaco-*.js`: `#root` went empty and the user got a blank page, not a
 * broken editor pane. The trigger is routine: a deploy replaces the
 * content-hashed chunk while someone still has the SPA open.
 *
 * So the mock rejects. A test that renders the Suspense fallback, or asserts a
 * boundary component exists in the tree, would pass with the boundary deleted —
 * the failure path is the only thing that distinguishes them.
 */
vi.mock("@/lib/monaco", () => {
  throw new Error("Failed to fetch dynamically imported module");
});

describe("MonacoEditor", () => {
  it("keeps the page alive when the editor chunk fails to load", async () => {
    // React logs the caught error; silence it so a deliberate failure does not
    // read as a broken run.
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const { default: MonacoEditor } = await import("../MonacoEditor");

    render(
      <div>
        <p>surrounding page</p>
        <MonacoEditor value="SELECT 1" />
      </div>,
    );

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/could not be loaded/i);
    });

    // The half that actually matters: everything around the editor survived.
    // With the boundary removed this node is gone, because the tree unmounted.
    expect(screen.getByText("surrounding page")).toBeInTheDocument();

    consoleError.mockRestore();
  });
});
