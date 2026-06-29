import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthContext } from "@/auth/useAuth";
import { RequireAuth, RequireGlobalAdmin, RequirePlatform } from "@/auth/guards";

afterEach(() => vi.restoreAllMocks());
const qc = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

test("RequireAuth redirects to /login when unauthenticated", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: null, isLoading: false }}>
        <MemoryRouter initialEntries={["/secret"]}>
          <Routes>
            <Route path="/secret" element={<RequireAuth><span>secret</span></RequireAuth>} />
            <Route path="/login" element={<span>login page</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("login page")).toBeInTheDocument();
});

test("RequirePlatform blocks a non-platform user", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [] }, isLoading: false }}>
        <MemoryRouter><RequirePlatform><span>admin area</span></RequirePlatform></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("admin area")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test("RequireGlobalAdmin blocks a platform role that is not platform_admin", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: ["data_steward"] }, isLoading: false }}>
        <MemoryRouter><RequireGlobalAdmin><span>control plane</span></RequireGlobalAdmin></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("control plane")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test("RequireGlobalAdmin allows platform_admin", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: ["platform_admin"] }, isLoading: false }}>
        <MemoryRouter><RequireGlobalAdmin><span>control plane</span></RequireGlobalAdmin></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("control plane")).toBeInTheDocument();
});
