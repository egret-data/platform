/**
 * ImpersonateDialog tests
 *
 * `useStartPlatformImpersonation`/`useStartWsImpersonation` (from
 * `@/api/impersonation`) are mocked so no network requests are made and each
 * mutation's `mutateAsync` call can be asserted directly. `react-router-dom`'s
 * `useNavigate` is mocked (rest of the module kept real) so the post-success
 * `navigate("/")` call — the fix for the Control-Plane-Users-to-Forbidden
 * dead-end — can be asserted directly too.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

const useStartPlatformImpersonation = vi.fn();
const useStartWsImpersonation = vi.fn();
const mockNavigate = vi.fn();

vi.mock("@/api/impersonation", () => ({
  useStartPlatformImpersonation: () => useStartPlatformImpersonation(),
  useStartWsImpersonation: (workspace: string) => useStartWsImpersonation(workspace),
}));

vi.mock("react-router-dom", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react-router-dom")>();
  return { ...actual, useNavigate: () => mockNavigate };
});

import { ImpersonateDialog } from "@/components/ImpersonateDialog";

function mutationStub() {
  return {
    mutate: vi.fn(),
    mutateAsync: vi.fn().mockResolvedValue(undefined),
    isPending: false,
    reset: vi.fn(),
  };
}

let platformMutation: ReturnType<typeof mutationStub>;
let wsMutation: ReturnType<typeof mutationStub>;

beforeEach(() => {
  platformMutation = mutationStub();
  wsMutation = mutationStub();
  useStartPlatformImpersonation.mockReset().mockReturnValue(platformMutation);
  useStartWsImpersonation.mockReset().mockReturnValue(wsMutation);
  mockNavigate.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

function renderDialog(props: Partial<React.ComponentProps<typeof ImpersonateDialog>> = {}) {
  const onOpenChange = vi.fn();
  render(
    <ImpersonateDialog
      open
      onOpenChange={onOpenChange}
      targetUserId="user-1"
      targetEmail="bob@egret.local"
      {...props}
    />,
  );
  return { onOpenChange };
}

describe("ImpersonateDialog", () => {
  test("shows the target email and disables submit until a reason is entered", async () => {
    const user = userEvent.setup();
    renderDialog();

    expect(screen.getByDisplayValue("bob@egret.local")).toBeInTheDocument();
    const submit = screen.getByRole("button", { name: /impersonate/i });
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText(/reason/i), "Debugging a support ticket");
    expect(submit).toBeEnabled();
  });

  test("uses the platform hook by default and submits the reason + password", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog();

    await user.type(screen.getByLabelText(/reason/i), "Debugging a support ticket");
    await user.type(screen.getByLabelText(/^password$/i), "hunter2");
    await user.click(screen.getByRole("button", { name: /impersonate/i }));

    expect(useStartWsImpersonation).toHaveBeenCalledWith("");
    expect(platformMutation.mutateAsync).toHaveBeenCalledWith({
      target_user_id: "user-1",
      reason: "Debugging a support ticket",
      password: "hunter2",
    });
    expect(wsMutation.mutateAsync).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
    // Navigates away from whatever (possibly permission-gated) page opened
    // the dialog — see ImpersonateDialog's doc comment for the Forbidden
    // dead-end this avoids.
    expect(mockNavigate).toHaveBeenCalledWith("/");
  });

  test("submits without a password when left blank", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByLabelText(/reason/i), "Debugging a support ticket");
    await user.click(screen.getByRole("button", { name: /impersonate/i }));

    expect(platformMutation.mutateAsync).toHaveBeenCalledWith({
      target_user_id: "user-1",
      reason: "Debugging a support ticket",
      password: undefined,
    });
  });

  test("uses the workspace hook when workspaceScope is set", async () => {
    const user = userEvent.setup();
    renderDialog({ workspaceScope: "acme" });

    expect(useStartWsImpersonation).toHaveBeenCalledWith("acme");

    await user.type(screen.getByLabelText(/reason/i), "Investigating a bug report");
    await user.click(screen.getByRole("button", { name: /impersonate/i }));

    expect(wsMutation.mutateAsync).toHaveBeenCalledWith({
      target_user_id: "user-1",
      reason: "Investigating a bug report",
      password: undefined,
    });
    expect(platformMutation.mutateAsync).not.toHaveBeenCalled();
    // Uniform behaviour across both entry points: the ws-scoped start also
    // navigates to "/" on success.
    expect(mockNavigate).toHaveBeenCalledWith("/");
  });

  test("does not navigate when the start mutation fails", async () => {
    platformMutation.mutateAsync.mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByLabelText(/reason/i), "Debugging a support ticket");
    await user.click(screen.getByRole("button", { name: /impersonate/i }));

    expect(await screen.findByText("boom")).toBeInTheDocument();
    expect(mockNavigate).not.toHaveBeenCalled();
  });
});
