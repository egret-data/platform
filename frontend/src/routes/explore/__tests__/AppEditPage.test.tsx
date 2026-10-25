import { StrictMode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";

// ── mocks ────────────────────────────────────────────────────────────────────

const requestAppTicket = vi.fn();
const publishMutateAsync = vi.fn();
const useAppMock = vi.fn();
const refetchApp = vi.fn();
const restoreMutateAsync = vi.fn();

vi.mock("@/api/apps", () => ({
  requestAppTicket: (...a: unknown[]) => requestAppTicket(...a),
  useApp: (...a: unknown[]) => useAppMock(...a),
  usePublishApp: () => ({ mutateAsync: publishMutateAsync, isPending: false }),
  useAppVersions: () => ({ data: [] }),
  useRestoreVersion: () => ({ mutateAsync: restoreMutateAsync, isPending: false }),
}));

const toastError = vi.fn();
const toastSuccess = vi.fn();
vi.mock("sonner", () => ({
  toast: { error: (...a: unknown[]) => toastError(...a), success: (...a: unknown[]) => toastSuccess(...a) },
}));

import AppEditPage from "@/routes/explore/AppEditPage";

const APP = { id: "a1", name: "Revenue", draft_revision: 7, latest_version_no: null };

function renderPage(strict = false) {
  const tree = (
    <MemoryRouter initialEntries={["/explore/acme/apps/a1/edit"]}>
      <Routes>
        <Route path="/explore/:workspace/apps/:id/edit" element={<AppEditPage />} />
        <Route path="/explore/:workspace/apps" element={<div>apps list</div>} />
      </Routes>
    </MemoryRouter>
  );
  return render(strict ? <StrictMode>{tree}</StrictMode> : tree);
}

beforeEach(() => {
  requestAppTicket.mockReset();
  publishMutateAsync.mockReset();
  toastError.mockReset();
  toastSuccess.mockReset();
  useAppMock.mockReset();
  refetchApp.mockReset();
  refetchApp.mockResolvedValue({ data: APP });
  useAppMock.mockReturnValue({ data: APP, refetch: refetchApp });
});

describe("AppEditPage", () => {
  it("asks for one edit ticket and hands it to a sandboxed, referrer-less iframe on the apps origin", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    const frame = await screen.findByTitle("Revenue editor");
    expect(requestAppTicket).toHaveBeenCalledTimes(1);
    expect(requestAppTicket).toHaveBeenCalledWith("acme", "a1", "edit");
    expect(frame.getAttribute("src")).toBe("https://apps.egret.test/_ticket?t=egt_abc");
    // exactly these four; in particular no allow-top-navigation, no allow-popups
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin allow-forms allow-downloads");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
  });

  it("shows the starting screen until the iframe has loaded, not just until the ticket arrives", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    const frame = await screen.findByTitle("Revenue editor");
    expect(screen.getByRole("status").textContent).toMatch(/Starting your editor/);
    fireEvent.load(frame);
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });

  it("does not spend a second ticket when React mounts the page twice", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage(true);
    await screen.findByTitle("Revenue editor");
    expect(requestAppTicket).toHaveBeenCalledTimes(1);
  });

  it("says why when the caller may not edit, and renders no iframe at all", async () => {
    requestAppTicket.mockRejectedValue(Object.assign(new Error("forbidden"), { status: 403 }));
    renderPage();
    expect(await screen.findByText("You can't edit this app")).toBeTruthy();
    expect(screen.queryByTitle("Revenue editor")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("tells an operator when the apps origin is not configured", async () => {
    requestAppTicket.mockRejectedValue(Object.assign(new Error("not configured"), { status: 503 }));
    renderPage();
    expect(await screen.findByText("Apps are not set up on this deployment")).toBeTruthy();
    expect(screen.queryByTitle("Revenue editor")).toBeNull();
  });

  it("carries the server's own words for a trashed app", async () => {
    requestAppTicket.mockRejectedValue(
      Object.assign(new Error("app is in the trash; restore it first"), { status: 409 }),
    );
    renderPage();
    expect(await screen.findByText(/in the trash/)).toBeTruthy();
  });

  it("Reload editor asks for a NEW ticket and mounts a fresh iframe", async () => {
    requestAppTicket
      .mockResolvedValueOnce("https://apps.egret.test/_ticket?t=egt_one")
      .mockResolvedValueOnce("https://apps.egret.test/_ticket?t=egt_two");
    renderPage();
    const first = await screen.findByTitle("Revenue editor");
    expect(first.getAttribute("src")).toContain("egt_one");
    fireEvent.click(screen.getByRole("button", { name: /Reload editor/ }));
    await waitFor(() => expect(requestAppTicket).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(screen.getByTitle("Revenue editor").getAttribute("src")).toContain("egt_two"),
    );
  });

  it("ignores a ticket that arrives after a newer one was asked for", async () => {
    let first!: (url: string) => void;
    requestAppTicket
      .mockImplementationOnce(() => new Promise<string>((r) => (first = r)))
      .mockResolvedValueOnce("https://apps.egret.test/_ticket?t=egt_new");
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /Reload editor/ }));
    await waitFor(() =>
      expect(screen.getByTitle("Revenue editor").getAttribute("src")).toContain("egt_new"),
    );
    first("https://apps.egret.test/_ticket?t=egt_stale");
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByTitle("Revenue editor").getAttribute("src")).toContain("egt_new");
  });

  it("publishes the revision it shows, so a draft that moved on is a conflict and not a surprise", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    publishMutateAsync.mockResolvedValue({ version_no: 3 });
    renderPage();
    await screen.findByTitle("Revenue editor");
    fireEvent.click(screen.getByRole("button", { name: "Publish draft rev 7" }));
    await waitFor(() => expect(publishMutateAsync).toHaveBeenCalledWith({ expectedRevision: 7 }));
    await waitFor(() => expect(toastSuccess).toHaveBeenCalledWith("Published version 3 from draft revision 7"));
  });

  it("re-reads the revision before publishing, so a save made just before the press is not a conflict (KI-177)", async () => {
    // Arrange: the label still says rev 7, the author's Ctrl+S has already made it 8.
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    refetchApp.mockResolvedValue({ data: { ...APP, draft_revision: 8 } });
    publishMutateAsync.mockResolvedValue({ version_no: 1, from_revision: 8 });
    renderPage();
    await screen.findByTitle("Revenue editor");

    // Act
    fireEvent.click(screen.getByRole("button", { name: "Publish draft rev 7" }));

    // Assert
    await waitFor(() => expect(publishMutateAsync).toHaveBeenCalledWith({ expectedRevision: 8 }));
    expect(refetchApp).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(toastSuccess).toHaveBeenCalledWith("Published version 1 from draft revision 8"));
    expect(toastError).not.toHaveBeenCalled();
  });

  it("says the draft is still the starter when the server refuses to publish it (KI-042)", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    publishMutateAsync.mockRejectedValue(
      Object.assign(new Error("the draft is still the new-app starter"), { status: 409, code: "starter_draft" }),
    );
    renderPage();
    await screen.findByTitle("Revenue editor");
    fireEvent.click(screen.getByRole("button", { name: /Publish draft rev/ }));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(String(toastError.mock.calls[0][0])).toMatch(/still the new-app starter/);
  });

  it("explains a publish conflict instead of dumping the status", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    publishMutateAsync.mockRejectedValue(Object.assign(new Error("stale"), { status: 409 }));
    renderPage();
    await screen.findByTitle("Revenue editor");
    fireEvent.click(screen.getByRole("button", { name: /Publish draft rev/ }));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(String(toastError.mock.calls[0][0])).toMatch(/draft changed/);
  });

  it("polls the app so the revision next to Publish follows the author's saves", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    await screen.findByTitle("Revenue editor");
    expect(useAppMock).toHaveBeenCalledWith("acme", "a1", { refetchMs: 5000 });
  });

  it("Versions opens the versions dialog", async () => {
    requestAppTicket.mockResolvedValue("https://apps.egret.test/_ticket?t=egt_abc");
    renderPage();
    await screen.findByTitle("Revenue editor");
    expect(screen.queryByText("Versions")).toBeTruthy(); // the button itself
    fireEvent.click(screen.getByRole("button", { name: /Versions/ }));
    expect(await screen.findByRole("heading", { name: "Versions" })).toBeTruthy();
  });
});
