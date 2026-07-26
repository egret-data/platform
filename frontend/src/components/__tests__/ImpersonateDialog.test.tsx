/**
 * ImpersonateDialog tests
 *
 * `useStartPlatformImpersonation`/`useStartWsImpersonation` (from
 * `@/api/impersonation`) are mocked so no network requests are made and each
 * mutation's `mutateAsync` call can be asserted directly.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

const useStartPlatformImpersonation = vi.fn();
const useStartWsImpersonation = vi.fn();

vi.mock("@/api/impersonation", () => ({
  useStartPlatformImpersonation: () => useStartPlatformImpersonation(),
  useStartWsImpersonation: (workspace: string) => useStartWsImpersonation(workspace),
}));

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
  });
});
