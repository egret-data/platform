import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

const smtpMock = vi.fn();
const saveMutate = vi.fn().mockResolvedValue({});
const testMutate = vi.fn().mockResolvedValue({ ok: true, sent_to: ["a@b.c"] });
const deleteMutate = vi.fn().mockResolvedValue({});

vi.mock("@/api/notifications", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useSmtpConfig: () => smtpMock(),
  useSmtpMutations: () => ({
    save: { mutateAsync: saveMutate, isPending: false },
    sendTest: { mutateAsync: testMutate, isPending: false },
    remove: { mutateAsync: deleteMutate, isPending: false },
  }),
}));

import Notifications from "@/routes/explore/settings/Notifications";

const CONFIGURED = {
  host: "smtp.example.com",
  port: 587,
  username: "mailer",
  from_address: "alerts@example.com",
  from_name: "Egret",
  tls_mode: "starttls" as const,
  has_password: true,
  updated_at: "2026-08-11T09:00:00Z",
};

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/settings/notifications"]}>
          <Routes>
            <Route
              path="/explore/:workspace/settings/notifications"
              element={<Notifications />}
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  smtpMock.mockReturnValue({ data: CONFIGURED, isLoading: false, isError: false });
});

describe("Workspace settings — Notifications", () => {
  it("shows the stored settings without ever rendering a password value", () => {
    // The server never sends one; the field must therefore start empty rather
    // than showing dots that imply the real password is present and editable.
    renderPage();
    expect(screen.getByLabelText(/smtp host/i)).toHaveValue("smtp.example.com");
    expect(screen.getByLabelText(/port/i)).toHaveValue(587);
    expect(screen.getByLabelText(/from address/i)).toHaveValue("alerts@example.com");
    expect(screen.getByLabelText(/^password$/i)).toHaveValue("");
  });

  it("says a password is stored, so a blank field is not read as 'none set'", () => {
    renderPage();
    expect(screen.getByText(/password is set/i)).toBeInTheDocument();
  });

  it("omits the password from the save when the field is untouched", async () => {
    // If a blank field meant "clear it", every unrelated edit would silently
    // break sending. The body must carry no password key at all.
    const user = userEvent.setup();
    renderPage();
    await user.clear(screen.getByLabelText(/smtp host/i));
    await user.type(screen.getByLabelText(/smtp host/i), "smtp2.example.com");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(saveMutate).toHaveBeenCalled());
    const body = saveMutate.mock.calls[0][0];
    expect(body.host).toBe("smtp2.example.com");
    expect("password" in body).toBe(false);
  });

  it("sends the password only when the user typed one", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.type(screen.getByLabelText(/^password$/i), "new-secret");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(saveMutate).toHaveBeenCalled());
    expect(saveMutate.mock.calls[0][0].password).toBe("new-secret");
  });

  it("offers a test send and reports what the relay said on failure", async () => {
    // The button is the only thing that distinguishes 'configured' from
    // 'configured correctly', so its failure has to be legible.
    const user = userEvent.setup();
    testMutate.mockRejectedValueOnce(
      Object.assign(new Error("bad"), {
        body: { error: "smtp transport: connection refused" },
      }),
    );
    renderPage();
    await user.click(screen.getByRole("button", { name: /send test/i }));

    await waitFor(() =>
      expect(screen.getByText(/connection refused/i)).toBeInTheDocument(),
    );
  });

  it("confirms a successful test send", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /send test/i }));
    await waitFor(() => expect(testMutate).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText(/test message sent/i)).toBeInTheDocument());
  });

  it("renders an unconfigured workspace as an empty form, not an error", async () => {
    // A 404 from the API means 'not set up yet', which is the normal starting
    // state for every workspace — showing it as a failure would be wrong.
    smtpMock.mockReturnValue({ data: undefined, isLoading: false, isError: true });
    renderPage();
    expect(screen.getByLabelText(/smtp host/i)).toHaveValue("");
    expect(screen.queryByText(/could not load/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/password is set/i)).not.toBeInTheDocument();
  });

  it("cannot send a test before anything is saved", () => {
    smtpMock.mockReturnValue({ data: undefined, isLoading: false, isError: true });
    renderPage();
    expect(screen.getByRole("button", { name: /send test/i })).toBeDisabled();
  });
});
