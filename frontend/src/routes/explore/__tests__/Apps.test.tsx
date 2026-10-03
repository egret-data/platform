import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";

const useAppsMock = vi.fn();
const createMutateAsync = vi.fn();

vi.mock("@/api/apps", () => ({
  useApps: (...a: unknown[]) => useAppsMock(...a),
  useCreateApp: () => ({ mutateAsync: createMutateAsync, isPending: false }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import Apps from "@/routes/explore/Apps";

function Where() {
  return <div data-testid="where">{useLocation().pathname}</div>;
}

function renderApps() {
  return render(
    <TooltipProvider>
      <MemoryRouter initialEntries={["/explore/acme/apps"]}>
        <Routes>
          <Route path="/explore/:workspace/apps" element={<Apps />} />
          <Route path="/explore/:workspace/apps/:id/edit" element={<Where />} />
        </Routes>
      </MemoryRouter>
    </TooltipProvider>,
  );
}

const app = (over: Record<string, unknown>) => ({
  id: "a1",
  name: "Revenue",
  description: "",
  updated_at: new Date().toISOString(),
  latest_version_no: null,
  draft_revision: 1,
  is_owner: true,
  bookmarked: false,
  ...over,
});

beforeEach(() => {
  useAppsMock.mockReset();
  createMutateAsync.mockReset();
});

describe("Apps list", () => {
  it("says whether each app is a draft or which version is live", () => {
    useAppsMock.mockReturnValue({
      data: [app({ id: "a1", name: "Draft app" }), app({ id: "a2", name: "Live app", latest_version_no: 3 })],
      isLoading: false,
    });
    renderApps();
    expect(screen.getByText("Draft only")).toBeTruthy();
    expect(screen.getByText("Published v3")).toBeTruthy();
  });

  it("links an app to its editor; a draft-only app's Open stays a disabled promise", () => {
    useAppsMock.mockReturnValue({ data: [app({ id: "a9", name: "Draft app" })], isLoading: false });
    renderApps();
    expect(screen.getByRole("link", { name: "Draft app" }).getAttribute("href")).toBe("/explore/acme/apps/a9/edit");
    expect(screen.getByRole("link", { name: /Edit/ }).getAttribute("href")).toBe("/explore/acme/apps/a9/edit");
    const open = screen.getByRole("button", { name: /Open/ });
    expect(open.getAttribute("aria-disabled")).toBe("true");
  });

  it("a published app's Open is a real link to the viewer page", () => {
    useAppsMock.mockReturnValue({
      data: [app({ id: "a9", name: "Live app", latest_version_no: 2 })],
      isLoading: false,
    });
    renderApps();
    const open = screen.getByRole("link", { name: /Open/ });
    expect(open.getAttribute("href")).toBe("/explore/acme/apps/a9");
    expect(open.getAttribute("aria-disabled")).toBeNull();
  });

  it("explains an empty workspace", () => {
    useAppsMock.mockReturnValue({ data: [], isLoading: false });
    renderApps();
    expect(screen.getByText(/No apps yet/)).toBeTruthy();
  });

  it("creates an app and goes straight to its editor", async () => {
    useAppsMock.mockReturnValue({ data: [], isLoading: false });
    createMutateAsync.mockResolvedValue({ id: "new-id", name: "Fresh" });
    renderApps();
    fireEvent.click(screen.getByRole("button", { name: /New app/ }));
    const input = await screen.findByLabelText("Name");
    fireEvent.change(input, { target: { value: "  Fresh  " } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createMutateAsync).toHaveBeenCalledWith({ name: "Fresh" }));
    await waitFor(() => expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/new-id/edit"));
  });

  it("will not create an app with no name", async () => {
    useAppsMock.mockReturnValue({ data: [], isLoading: false });
    renderApps();
    fireEvent.click(screen.getByRole("button", { name: /New app/ }));
    await screen.findByLabelText("Name");
    expect((screen.getByRole("button", { name: "Create" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a load error", () => {
    useAppsMock.mockReturnValue({ data: undefined, isLoading: false, error: new Error("boom") });
    renderApps();
    expect(screen.getByText("boom")).toBeTruthy();
  });
});
