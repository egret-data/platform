import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthProvider } from "@/auth/AuthProvider";
import { useAuth } from "@/auth/useAuth";

afterEach(() => vi.restoreAllMocks());

function Probe() {
  const { me, isLoading } = useAuth();
  if (isLoading) return <span>loading</span>;
  return <span>{me ? me.email : "anon"}</span>;
}
function wrap() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><AuthProvider><Probe /></AuthProvider></QueryClientProvider>);
}

test("authenticated when /auth/me returns 200", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify({ user_id: "1", email: "a@b.c", platform_roles: [] }), { status: 200 }));
  wrap();
  await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());
});

test("anonymous when /auth/me returns 401", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
  wrap();
  await waitFor(() => expect(screen.getByText("anon")).toBeInTheDocument());
});
