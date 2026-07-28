import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FormatSection } from "../FormatSection";

describe("FormatSection", () => {
  it("expands a numeric column on click and writes a format when a setting is edited", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    // collapsed by default
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    // click the column row to expand its settings
    fireEvent.click(screen.getByRole("button", { name: /revenue/ }));
    expect(screen.getByLabelText("Number style")).toBeInTheDocument();
    // editing a native input writes that column's format
    fireEvent.change(screen.getByLabelText("Prefix"), { target: { value: "$" } });
    expect(onChange).toHaveBeenCalledWith({ revenue: { prefix: "$" } });
  });

  it("offers Display-as (not numeric controls) for a text column", () => {
    render(
      <FormatSection
        columns={[{ name: "name", type: "varchar" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Display as")).toBeInTheDocument();
  });

  it("shows an empty Column title input when nothing is stored", () => {
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    expect(screen.getByLabelText("Column title")).toHaveValue("");
  });

  it("emits column_title when typing a new name", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    fireEvent.change(screen.getByLabelText("Column title"), { target: { value: "Revenue" } });
    expect(onChange).toHaveBeenCalledWith({ total_revenue: { column_title: "Revenue" } });
  });

  it("emits column_title: undefined (not empty string) when the title is cleared", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{ total_revenue: { column_title: "Revenue" } }}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    fireEvent.change(screen.getByLabelText("Column title"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ total_revenue: { column_title: undefined } });
  });

  it("offers a Column title input for a text column too, not only numeric", () => {
    render(
      <FormatSection
        columns={[{ name: "name", type: "varchar" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(screen.getByLabelText("Column title")).toBeInTheDocument();
  });

  it("shows a renamed column's title in its collapsed row without changing other rows", () => {
    render(
      <FormatSection
        columns={[
          { name: "total_revenue", type: "double" },
          { name: "region", type: "varchar" },
        ]}
        value={{ total_revenue: { column_title: "Revenue" } }}
        onChange={() => {}}
      />,
    );
    // renamed column's collapsed row shows the new title plus the real name
    const revenueButton = screen.getByRole("button", { name: /Revenue/ });
    expect(revenueButton).toHaveTextContent("Revenue");
    expect(revenueButton).toHaveTextContent("total_revenue");

    // the other column's collapsed row is untouched
    const regionButton = screen.getByRole("button", { name: "region Text" });
    expect(regionButton).not.toHaveTextContent("Revenue");

    // now open the renamed column's panel (making it the "open" column) and confirm
    // the OTHER (closed) row still shows its own name, not the open column's title
    fireEvent.click(revenueButton);
    expect(screen.getByRole("button", { name: "region Text" })).not.toHaveTextContent("Revenue");
  });
});
