import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RuleEditor } from "../RuleEditor";

// The column picker is a plain Popover + tick-list, so no data-fetching provider is needed.
const renderEditor = render;

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
  it("collapses the column list behind a trigger that states the choice", () => {
    renderEditor(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    // closed: the trigger names the chosen column, and no column is a standing row
    expect(screen.getByRole("button", { name: "Columns" })).toHaveTextContent("revenue");
    expect(screen.queryByText("region")).not.toBeInTheDocument();
  });
});

describe("RuleEditor column picker — opened", () => {
  const columns = [
    { name: "revenue", type: "double" },
    { name: "orders", type: "bigint" },
    { name: "region", type: "varchar" },
  ];

  it("lists every column with its type once opened, and toggles a choice", async () => {
    const onSave = vi.fn();
    render(
      <RuleEditor
        columns={columns}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Columns" }));
    expect(screen.getByText("orders")).toBeInTheDocument();
    expect(screen.getAllByText("bigint").length).toBeGreaterThan(0);

    await userEvent.click(screen.getByText("orders"));
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ columns: ["revenue", "orders"] }),
    );
  });

  it("summarises rather than listing when several columns are chosen", () => {
    render(
      <RuleEditor
        columns={columns}
        rule={{ columns: ["revenue", "orders", "region"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: "Columns" })).toHaveTextContent("3 columns");
  });
});
