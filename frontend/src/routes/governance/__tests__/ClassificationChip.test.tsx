import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";

const useTags = vi.hoisted(() =>
  vi.fn((_opts?: { enabled?: boolean }) => ({
    data: [{ id: "t-sec", key: "sec", name: "Secret", required_roles: ["cleared"], required_groups: [] }],
  })),
);
vi.mock("@/api/tags", async () => {
  const actual = await vi.importActual<typeof import("@/api/tags")>("@/api/tags");
  return { ...actual, useTags };
});
import { ClassificationChip } from "@/routes/governance/ClassificationChip";

const ME = (permissions: string[]): Me => ({ user_id: "u", email: "u@x", platform_roles: [], permissions });
const SECRET = { id: "t-sec", key: "sec", name: "Secret", color: "#e11d48" };

function show(me: Me, props: Partial<React.ComponentProps<typeof ClassificationChip>> = {}) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter>
          <ClassificationChip tag={SECRET} objectLabel="sales.orders.email" {...props} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

describe("ClassificationChip", () => {
  it("shows the colour, links to the classification and says it requires clearance", () => {
    show(ME(["tag:read"]));
    const link = screen.getByRole("link", { name: /Secret/ });
    expect(link).toHaveAttribute("href", "/governance/classifications/t-sec");
    expect(link.parentElement?.querySelector('[aria-hidden="true"][style]')).toHaveStyle({ backgroundColor: "#e11d48" });
    expect(screen.getByText("requires clearance")).toHaveClass("sr-only");
  });

  it("is plain text without tag:read", () => {
    show(ME([]));
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("Secret")).toBeInTheDocument();
  });

  it("offers Remove only when given a handler, and names what it removes from where", async () => {
    const onRemove = vi.fn();
    const { unmount } = show(ME(["tag:read"]));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    unmount();
    show(ME(["tag:read"]), { onRemove });
    await userEvent.click(screen.getByRole("button", { name: "Remove Secret from sales.orders.email" }));
    expect(onRemove).toHaveBeenCalledOnce();
  });

  it("marks an inherited chip and never offers Remove on it", () => {
    show(ME(["tag:read"]), { inherited: true, onRemove: vi.fn() });
    expect(screen.getByText("from table")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("draws a classification without a colour in the default colour", () => {
    show(ME(["tag:read"]), { tag: { ...SECRET, color: "" } });
    const link = screen.getByRole("link", { name: /Secret/ });
    expect(link.parentElement?.querySelector('[aria-hidden="true"][style]')).toHaveStyle({ backgroundColor: "#6366f1" });
  });

  it("links for a platform admin without tag:read", () => {
    show({ user_id: "a", email: "a@x", platform_roles: ["platform_admin"], permissions: [] });
    expect(screen.getByRole("link", { name: /Secret/ })).toHaveAttribute("href", "/governance/classifications/t-sec");
  });

  it("does not read the catalogue without tag:read, and reads it with", () => {
    const { unmount } = show(ME([]));
    expect(useTags).toHaveBeenLastCalledWith({ enabled: false });
    unmount();
    show(ME(["tag:read"]));
    expect(useTags).toHaveBeenLastCalledWith({ enabled: true });
  });

  it("links an inherited chip's 'from table' to the table when told where it is", () => {
    show(ME(["tag:read"]), { inherited: true, inheritedFrom: { href: "/governance/data/ice/sales/orders", label: "ice.sales.orders" } });
    expect(screen.getByRole("link", { name: "from table ice.sales.orders" })).toHaveAttribute("href", "/governance/data/ice/sales/orders");
  });
});
