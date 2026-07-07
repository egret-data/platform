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
});
