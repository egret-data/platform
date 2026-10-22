/**
 * LoginPage tests
 *
 * Verifies the SSO-primary / break-glass local-admin behaviour:
 * 1. OIDC enabled → SSO link is the primary action, the local form is hidden
 *    until the "Admin sign-in (break-glass)" disclosure is clicked.
 * 2. OIDC disabled → the local email/password form is the only option and is
 *    visible immediately, with no SSO link.
 *
 * `@/api/hooks` is mocked so no network requests are made.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { LoginPage } from "@/auth/LoginPage";

const mutateAsync = vi.fn();
const useAuthConfig = vi.fn();
const useLogin = vi.fn();

vi.mock("@/api/hooks", () => ({
  useAuthConfig: () => useAuthConfig(),
  useLogin: () => useLogin(),
}));

beforeEach(() => {
  mutateAsync.mockReset();
  useLogin.mockReturnValue({ mutateAsync, isPending: false });
});

afterEach(() => vi.restoreAllMocks());

function renderLogin() {
  return render(
    <MemoryRouter>
      <LoginPage />
    </MemoryRouter>,
  );
}

test("OIDC enabled: SSO is primary and local form is hidden behind break-glass", async () => {
  useAuthConfig.mockReturnValue({ data: { oidc_enabled: true } });
  const user = userEvent.setup();
  renderLogin();

  // SSO link is present and prominent.
  expect(
    screen.getByRole("link", { name: /sign in with sso/i }),
  ).toBeInTheDocument();

  // Local password field is NOT rendered initially.
  expect(screen.queryByPlaceholderText("Password")).not.toBeInTheDocument();

  // Expand the break-glass local admin form.
  await user.click(screen.getByRole("button", { name: /admin sign-in/i }));

  // Now the password field is present.
  expect(screen.getByPlaceholderText("Password")).toBeInTheDocument();
});

test("OIDC disabled: local form is visible immediately, no SSO link", () => {
  useAuthConfig.mockReturnValue({ data: { oidc_enabled: false } });
  renderLogin();

  expect(screen.getByPlaceholderText("Email")).toBeInTheDocument();
  expect(screen.getByPlaceholderText("Password")).toBeInTheDocument();
  expect(
    screen.queryByRole("link", { name: /sign in with sso/i }),
  ).not.toBeInTheDocument();
});

// KI-002: a locked-out sign-in says when the user can try again, from the
// server's `retry_after_secs`, rather than "a few minutes".
test("a 429 says when to try again", async () => {
  const { ApiError } = await import("@/api/client");
  useAuthConfig.mockReturnValue({ data: { oidc_enabled: false } });
  mutateAsync.mockRejectedValue(
    new ApiError(429, "too many failed sign-in attempts", "rate_limited", {
      error: "too many failed sign-in attempts",
      code: "rate_limited",
      retry_after_secs: 240,
    }),
  );
  const user = userEvent.setup();
  renderLogin();

  await user.type(screen.getByPlaceholderText("Email"), "b7@example.com");
  await user.type(screen.getByPlaceholderText("Password"), "nope");
  await user.click(screen.getByRole("button", { name: "Sign in" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Too many failed sign-in attempts. Try again in 4 minutes.",
  );
});
