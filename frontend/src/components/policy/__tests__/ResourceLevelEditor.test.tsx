import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { ResourceLevelEditor } from "../ResourceLevelEditor";

vi.mock("@/api/policies", () => ({ resourceLookup: () => Promise.resolve(["sales", "eng"]) }));

function wrap(u: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{u}</QueryClientProvider>);
}

describe("ResourceLevelEditor", () => {
  it("shows values and toggles exclude", () => {
    const onChange = vi.fn();
    wrap(<ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
      value={{ values: ["sales"], isExcludes: false, isRecursive: false }} onChange={onChange}
      excludesSupported />);
    expect(screen.getByText("sales")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Advanced Schema"));
    fireEvent.click(screen.getByLabelText("Exclude Schema"));
    expect(onChange).toHaveBeenCalled();
  });

  it("hides the Recursive switch when the service-def doesn't support it", () => {
    wrap(<ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
      value={{ values: ["sales"], isExcludes: false, isRecursive: false }} onChange={vi.fn()}
      excludesSupported recursiveSupported={false} />);
    fireEvent.click(screen.getByLabelText("Advanced Schema"));
    expect(screen.queryByLabelText("Recursive Schema")).not.toBeInTheDocument();
  });

  it("hides the Advanced gear entirely when neither Exclude nor Recursive is supported", () => {
    wrap(<ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
      value={{ values: ["sales"], isExcludes: false, isRecursive: false }} onChange={vi.fn()}
      excludesSupported={false} recursiveSupported={false} />);
    expect(screen.queryByLabelText("Advanced Schema")).not.toBeInTheDocument();
  });

  it("ties a hint to the value field and shows it only when given", () => {
    const { rerender } = wrap(<ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
      value={{ values: [] }} onChange={vi.fn()} hint="Empty means every schema — saved as *." />);
    const input = screen.getByLabelText("Schema value");
    const hint = screen.getByText("Empty means every schema — saved as *.");
    expect(input.getAttribute("aria-describedby")).toBe(hint.id);
    expect(hint.id).not.toBe("");

    rerender(<QueryClientProvider client={new QueryClient()}>
      <ResourceLevelEditor label="Schema" level="schema" service="trino" parents={{}}
        value={{ values: ["*"] }} onChange={vi.fn()} /></QueryClientProvider>);
    expect(screen.queryByText(/saved as \*/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Schema value")).not.toHaveAttribute("aria-describedby");
  });

  it("shows the invalid message, tied to the field, only for a mandatory level that is invalid", () => {
    const { rerender } = wrap(<ResourceLevelEditor label="Catalog" level="catalog" service="trino" parents={{}}
      value={{ values: [] }} onChange={vi.fn()} mandatory invalid invalidMessage="Name a catalog." />);
    const msg = screen.getByText("Name a catalog.");
    expect(screen.getByLabelText("Catalog value").getAttribute("aria-describedby")).toBe(msg.id);

    // Same props, not mandatory: no message (the hint slot is empty).
    rerender(<QueryClientProvider client={new QueryClient()}>
      <ResourceLevelEditor label="Catalog" level="catalog" service="trino" parents={{}}
        value={{ values: [] }} onChange={vi.fn()} invalid invalidMessage="Name a catalog." /></QueryClientProvider>);
    expect(screen.queryByText("Name a catalog.")).not.toBeInTheDocument();
  });
});
