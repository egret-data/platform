import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PropertyPicker } from "./PropertyPicker";
import { trinoCatalog } from "./trinoCatalog";

const nodeCatalog = trinoCatalog.filter((p) => p.file === "node");

describe("PropertyPicker", () => {
  it("opens on click, filters, and adds a catalog prop", () => {
    const onAddCatalog = vi.fn();
    render(
      <PropertyPicker catalog={nodeCatalog} onAddCatalog={onAddCatalog} onAddCustom={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    fireEvent.change(screen.getByPlaceholderText(/search properties/i), {
      target: { value: "node.environment" },
    });
    fireEvent.click(screen.getByText("node.environment"));
    expect(onAddCatalog).toHaveBeenCalledWith(
      expect.objectContaining({ key: "node.environment" }),
    );
  });

  it("adds a custom free-text key on Enter", () => {
    const onAddCustom = vi.fn();
    render(
      <PropertyPicker catalog={nodeCatalog} onAddCatalog={vi.fn()} onAddCustom={onAddCustom} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    const input = screen.getByPlaceholderText(/search properties/i);
    fireEvent.change(input, { target: { value: "node.custom-thing" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAddCustom).toHaveBeenCalledWith("node.custom-thing");
  });

  it("Enter selects the top catalog match, not a bogus custom key", () => {
    const onAddCatalog = vi.fn();
    const onAddCustom = vi.fn();
    render(
      <PropertyPicker
        catalog={nodeCatalog}
        onAddCatalog={onAddCatalog}
        onAddCustom={onAddCustom}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    const input = screen.getByPlaceholderText(/search properties/i);
    // Partial query that matches node.environment but isn't an exact key.
    fireEvent.change(input, { target: { value: "environ" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAddCatalog).toHaveBeenCalledWith(
      expect.objectContaining({ key: "node.environment" }),
    );
    expect(onAddCustom).not.toHaveBeenCalled();
  });

  it("hides already-present keys via excludeKeys", () => {
    render(
      <PropertyPicker
        catalog={nodeCatalog}
        excludeKeys={new Set(["node.environment"])}
        onAddCatalog={vi.fn()}
        onAddCustom={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /add property/i }));
    fireEvent.change(screen.getByPlaceholderText(/search properties/i), {
      target: { value: "node.environment" },
    });
    // Neither a catalog row nor an "add custom" row for the excluded key.
    expect(screen.queryByText("node.environment")).not.toBeInTheDocument();
  });
});
