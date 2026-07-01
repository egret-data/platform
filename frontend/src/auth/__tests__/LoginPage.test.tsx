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
