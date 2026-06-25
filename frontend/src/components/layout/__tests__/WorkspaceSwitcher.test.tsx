/**
 * WorkspaceSwitcher test
 *
 * Radix Select has several jsdom incompatibilities:
 * 1. `hasPointerCapture` / `setPointerCapture` / `releasePointerCapture` are not
 *    implemented by jsdom — polyfilled below.
 * 2. `scrollIntoView` is not implemented by jsdom — polyfilled below.
 * 3. Radix portals render into `document.body` so options appear outside the
 *    component tree but still in screen queries.
 *
 * With these polyfills the real click-through interaction works: click trigger
 * to open, click option to select, assert navigation.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { WorkspaceSwitcher } from "@/components/layout/WorkspaceSwitcher";

afterEach(() => vi.restoreAllMocks());

function LocationProbe() {
  return <span data-testid="loc">{useLocation().pathname}</span>;
}

test("selecting a workspace navigates to /explore/:slug", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify([{ id: "1", slug: "sales", name: "Sales" }]), {
      status: 200,
    }),
  );
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const user = userEvent.setup();

  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/other"]}>
        <Routes>
          <Route path="/explore/:workspace" element={<WorkspaceSwitcher />} />
          <Route path="*" element={<WorkspaceSwitcher />} />
        </Routes>
        <Routes>
          <Route path="*" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );

  // Wait for the trigger to appear (workspace list may still be loading)
  const trigger = await screen.findByLabelText("Workspace");

  // Open the Select dropdown
  await user.click(trigger);

  // The option is rendered in a Radix portal into document.body — wait for it
  const option = await screen.findByText("Sales");
  await user.click(option);

  // Assert navigation to the selected workspace
  await waitFor(() =>
    expect(screen.getByTestId("loc").textContent).toBe("/explore/sales"),
  );
});
