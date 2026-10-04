import { StrictMode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";

// ── mocks ────────────────────────────────────────────────────────────────────

const requestAppTicket = vi.fn();
const useAppMock = vi.fn();

vi.mock("@/api/apps", () => ({
  requestAppTicket: (...a: unknown[]) => requestAppTicket(...a),
  useApp: (...a: unknown[]) => useAppMock(...a),
}));

import AppViewPage from "@/routes/explore/AppViewPage";

const APP = { id: "a1", name: "Revenue", latest_version_no: 3 };

function renderPage(strict = false) {
  const tree = (
    <MemoryRouter initialEntries={["/explore/acme/apps/a1"]}>
      <Routes>
        <Route path="/explore/:workspace/apps/:id" element={<AppViewPage />} />
        <Route path="/explore/:workspace/apps" element={<div>apps list</div>} />
      </Routes>
    </MemoryRouter>
  );
  return render(strict ? <StrictMode>{tree}</StrictMode> : tree);
}

beforeEach(() => {
  requestAppTicket.mockReset();
  useAppMock.mockReset();
  useAppMock.mockReturnValue({ data: APP });
});

describe("AppViewPage", () => {
  it("asks for exactly one run ticket, with no version id — the backend resolves the latest", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    const frame = await screen.findByTitle("Revenue");
    expect(requestAppTicket).toHaveBeenCalledTimes(1);
    expect(requestAppTicket).toHaveBeenCalledWith("acme", "a1", "run");
    expect(frame.getAttribute("src")).toBe("https://apps.egret.test/_ticket?t=egt_abc");
  });

  it("sandboxes the iframe the same way the editor does, with no referrer", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    const frame = await screen.findByTitle("Revenue");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin allow-forms allow-downloads");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
  });

  it("shows no Publish, Reload-editor or Versions controls — just the app and a way back", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    await screen.findByTitle("Revenue");
    expect(screen.queryByRole("button", { name: /publish/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /reload/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /versions/i })).toBeNull();
    expect(screen.getByRole("link", { name: /apps/i })).toHaveAttribute("href", "/explore/acme/apps");
  });

  it("does not spend a second ticket when React mounts the page twice", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage(true);
    await screen.findByTitle("Revenue");
    expect(requestAppTicket).toHaveBeenCalledTimes(1);
  });

  it("says why when the viewer may not open it, and renders no iframe at all", async () => {
    requestAppTicket.mockRejectedValue(Object.assign(new Error("forbidden"), { status: 403 }));
    renderPage();
    expect(await screen.findByText("You can't open this app")).toBeTruthy();
    expect(screen.queryByTitle("Revenue")).toBeNull();
  });

  it("tells an operator when the apps origin is not configured", async () => {
    requestAppTicket.mockRejectedValue(Object.assign(new Error("not configured"), { status: 503 }));
    renderPage();
    expect(await screen.findByText("Apps are not set up on this deployment")).toBeTruthy();
  });

  it("distinguishes 'never published' from a generic trashed-app 409", async () => {
    requestAppTicket.mockRejectedValue(
      Object.assign(new Error("the app has no published version to run; publish it first"), { status: 409 }),
    );
    renderPage();
    expect(await screen.findByText("Nobody has published this app yet")).toBeTruthy();
  });

  it("shows the generic trashed-app message for any other 409", async () => {
    requestAppTicket.mockRejectedValue(
      Object.assign(new Error("app is in the trash; restore it first"), { status: 409 }),
    );
    renderPage();
    expect(await screen.findByText("This app can't be opened right now")).toBeTruthy();
  });

  it("shows the starting screen until the iframe has loaded", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    const frame = await screen.findByTitle("Revenue");
    expect(screen.getByRole("status").textContent).toMatch(/Starting/);
    fireEvent.load(frame);
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });
});
