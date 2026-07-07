import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { PrincipalPicker } from "../PrincipalPicker";

vi.mock("@/api/policies", () => ({
  listPrincipals: (_q: string, kind?: "user" | "group") =>
    Promise.resolve(
      [
        { kind: "user", name: "alice@egret.local", label: "Alice" },
        { kind: "group", name: "analysts", label: "analysts" },
      ].filter((p) => !kind || p.kind === kind),
    ),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("PrincipalPicker", () => {
  it("renders selected chips and removes on click", () => {
    const onChange = vi.fn();
    wrap(<PrincipalPicker users={["alice@egret.local"]} groups={["analysts"]} onChange={onChange} />);
    expect(screen.getByText("alice@egret.local")).toBeInTheDocument();
    expect(screen.getByText("analysts")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("remove alice@egret.local"));
    expect(onChange).toHaveBeenCalled();
  });
});
