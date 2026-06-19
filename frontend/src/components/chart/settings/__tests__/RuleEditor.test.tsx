import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { RuleEditor } from "../RuleEditor";

describe("RuleEditor", () => {
  it("builds a single-color rule and saves it", () => {
    const onSave = vi.fn();
    render(
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
