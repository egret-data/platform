import { lazy } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LazyRouteArea } from "@/components/layout/LazyRouteArea";

afterEach(() => vi.restoreAllMocks());

describe("LazyRouteArea", () => {
  it("shows the fallback while the chunk loads, then the area", async () => {
    let resolve!: (m: { default: () => React.JSX.Element }) => void;
    const Area = lazy(() => new Promise<{ default: () => React.JSX.Element }>((r) => (resolve = r)));
    render(
      <LazyRouteArea area="Workflows" fallback={<p>loading area</p>}>
        <Area />
      </LazyRouteArea>,
    );
    expect(screen.getByText("loading area")).toBeInTheDocument();
    resolve({ default: () => <h1>Flows</h1> });
    expect(await screen.findByRole("heading", { name: "Flows" })).toBeInTheDocument();
  });

  it("turns a chunk that fails to load into an error with a reload, not a blank page", async () => {
    // React and the boundary both log the caught error; keep the output clean
    // and prove the boundary did log it.
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const Area = lazy(() => Promise.reject(new TypeError("Failed to fetch dynamically imported module")));
    render(
      <div data-testid="app-root">
        <nav>sidebar stays</nav>
        <LazyRouteArea area="Workflows" fallback={<p>loading area</p>}>
          <Area />
        </LazyRouteArea>
      </div>,
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The Workflows pages could not be loaded");
    expect(alert).toHaveTextContent("Reload the page to try again.");
    // The raw exception is not shown (FE-COPY-2).
    expect(alert).not.toHaveTextContent("dynamically imported module");
    // The rest of the app is still mounted.
    expect(screen.getByText("sidebar stays")).toBeInTheDocument();
    expect(logged).toHaveBeenCalled();
  });

  it("reloads the page from its button", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const reload = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, reload });
    const Area = lazy(() => Promise.reject(new Error("chunk gone")));
    render(
      <LazyRouteArea area="Workflows" fallback={null}>
        <Area />
      </LazyRouteArea>,
    );
    await userEvent.setup().click(await screen.findByRole("button", { name: "Reload page" }));
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
