/**
 * ImpersonationBanner tests
 *
 * `useMe`/`useExitImpersonation` (from `@/api/hooks`) and `sonner` are mocked
 * so no network requests are made and the mutation call can be asserted
 * directly.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

const useMe = vi.fn();
const exitMutate = vi.fn();
const useExitImpersonation = vi.fn();

vi.mock("@/api/hooks", () => ({
  useMe: () => useMe(),
  useExitImpersonation: () => useExitImpersonation(),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { ImpersonationBanner } from "@/components/layout/ImpersonationBanner";

const NOW = new Date("2026-07-27T12:00:00Z").getTime();

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  exitMutate.mockReset();
  useExitImpersonation.mockReturnValue({ mutate: exitMutate, mutateAsync: vi.fn(), isPending: false });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function renderBanner() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ImpersonationBanner />
    </QueryClientProvider>,
  );
}

function impersonatingMe(expiresInMs: number) {
  return {
    data: {
      user_id: "a",
      email: "admin@x",
      platform_roles: [],
      permissions: [],
      impersonating: {
        target: { user_id: "b", email: "bob@x" },
        impersonator: { user_id: "a", email: "admin@x" },
        workspace_scope: null,
        expires_at: new Date(NOW + expiresInMs).toISOString(),
      },
    },
  };
}

describe("ImpersonationBanner", () => {
  test("renders nothing when not impersonating", () => {
    useMe.mockReturnValue({ data: { user_id: "a", email: "admin@x", platform_roles: [], permissions: [], impersonating: null } });
    const { container } = renderBanner();
    expect(container).toBeEmptyDOMElement();
  });

  test("shows the target email and an Exit button while impersonating", () => {
    useMe.mockReturnValue(impersonatingMe(30 * 60 * 1000));
    renderBanner();

    expect(screen.getByText("bob@x")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /exit/i })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("30:00 left");
  });

  test("clicking Exit calls the exit mutation", async () => {
    useMe.mockReturnValue(impersonatingMe(30 * 60 * 1000));
    vi.useRealTimers(); // userEvent needs real timers
    const user = userEvent.setup();
    renderBanner();

    await user.click(screen.getByRole("button", { name: /exit/i }));

    expect(exitMutate).toHaveBeenCalledOnce();
  });
});
