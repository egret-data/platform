import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RuleEditor } from "../RuleEditor";

// The column picker is the shared TagCombobox, which fetches its suggestions through react-query
// — the same component the policy editors use. Every real caller already sits under a provider.
function renderEditor(ui: React.ReactElement) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      {ui}
    </QueryClientProvider>,
  );
}

describe("RuleEditor", () => {
  it("builds a single-color rule and saves it", () => {
    const onSave = vi.fn();
    renderEditor(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "100", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ type: "single", operator: ">", value: "100", columns: ["revenue"] }));
  });
});

describe("RuleEditor column picker", () => {
  // It used to be one checkbox per column, so the list grew with the width of the result and
  // pushed the operator, value and colour off the panel on a wide table.
  it("shows the chosen columns as chips rather than a checkbox per column", () => {
    renderEditor(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByLabelText("Columns")).toBeInTheDocument();
    expect(screen.getByText("revenue")).toBeInTheDocument();
    // the unchosen column is not rendered as a standing row
    expect(screen.queryByText("region")).not.toBeInTheDocument();
  });
});
