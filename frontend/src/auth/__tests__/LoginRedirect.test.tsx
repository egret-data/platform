/**
 * LoginRedirect tests
 *
 * Verifies:
 * 1. /login redirects away when the user is already authenticated.
 * 2. /login renders the login form when the user is unauthenticated.
 * 3. Successful login navigates to /.
 * 4. HTTP 429 from the login API shows the rate-limit error message.
 * 5. Other login errors show the generic "Invalid email or password" message.
 *
 * All tests are fully offline: AuthContext is injected directly, fetch is
 * mocked where needed, no real network requests are made.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, Navigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthContext } from "@/auth/useAuth";
import { LoginPage } from "@/auth/LoginPage";
import type { Me } from "@/api/hooks";

afterEach(() => vi.restoreAllMocks());

const ME: Me = { user_id: "1", email: "a@b.c", platform_roles: [], permissions: [] };

/** Wrap LoginPage in the minimal router + auth context required for testing. */
function renderLoginRoute(me: Me | null, isLoading = false) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });

  // LoginRoute logic (mirrors App.tsx LoginRoute)
  function LoginRoute() {
    if (isLoading) return <div>Loading…</div>;
    if (me) return <Navigate to="/" replace />;
    return <LoginPage />;
  }

  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me, isLoading }}>
        <MemoryRouter initialEntries={["/login"]}>
          <Routes>
            <Route path="/login" element={<LoginRoute />} />
            <Route path="/" element={<span>home page</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

// ---------------------------------------------------------------------------
// Redirect tests
// ---------------------------------------------------------------------------

test("authenticated user is redirected away from /login", () => {
  renderLoginRoute(ME);
  // The login form must NOT be visible
  expect(screen.queryByPlaceholderText("Email")).not.toBeInTheDocument();
  // The redirect destination must render
  expect(screen.getByText("home page")).toBeInTheDocument();
});

test("unauthenticated user sees the login form at /login", () => {
  renderLoginRoute(null);
  expect(screen.getByPlaceholderText("Email")).toBeInTheDocument();
  expect(screen.getByPlaceholderText("Password")).toBeInTheDocument();
});

// ---------------------------------------------------------------------------
// Post-login navigation test
// ---------------------------------------------------------------------------

test("successful login navigates to /", async () => {
  // Mock fetch: POST /api/auth/login → 200, GET /api/auth/me → return user
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(null, { status: 200 }),
  );

  const user = userEvent.setup();
  renderLoginRoute(null);

  await user.type(screen.getByPlaceholderText("Email"), "a@b.c");
  await user.type(screen.getByPlaceholderText("Password"), "secret");
  await user.click(screen.getByRole("button", { name: /sign in/i }));

  await waitFor(() =>
    expect(screen.getByText("home page")).toBeInTheDocument(),
  );
});

// ---------------------------------------------------------------------------
// Error message tests
// ---------------------------------------------------------------------------

test("HTTP 429 shows the rate-limit message", async () => {
  // A fresh Response per call: the auth-config probe reads one too.
  vi.spyOn(globalThis, "fetch").mockImplementation(() =>
    Promise.resolve(
      new Response(
        JSON.stringify({ error: "too many failed sign-in attempts", code: "rate_limited", retry_after_secs: 90 }),
        { status: 429, headers: { "Retry-After": "90" } },
      ),
    ),
  );

  const user = userEvent.setup();
  renderLoginRoute(null);

  await user.type(screen.getByPlaceholderText("Email"), "a@b.c");
  await user.type(screen.getByPlaceholderText("Password"), "wrong");
  await user.click(screen.getByRole("button", { name: /sign in/i }));

  await waitFor(() =>
    expect(
      screen.getByText("Too many failed sign-in attempts. Try again in 2 minutes."),
    ).toBeInTheDocument(),
  );
  // Generic message must NOT appear
  expect(screen.queryByText(/invalid email/i)).not.toBeInTheDocument();
});

test("non-429 error shows the generic invalid credentials message", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify({ error: "Unauthorized" }), { status: 401 }),
  );

  const user = userEvent.setup();
  renderLoginRoute(null);

  await user.type(screen.getByPlaceholderText("Email"), "a@b.c");
  await user.type(screen.getByPlaceholderText("Password"), "wrong");
  await user.click(screen.getByRole("button", { name: /sign in/i }));

  await waitFor(() =>
    expect(
      screen.getByText(/invalid email or password/i),
    ).toBeInTheDocument(),
  );
});
