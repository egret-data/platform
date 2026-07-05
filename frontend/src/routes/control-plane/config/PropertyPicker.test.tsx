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
});
